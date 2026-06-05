use serde_json::json;
use log::{info, warn};

use crate::decision::SignalDirection;
use crate::hl_rest::{HyperliquidRest, AssetInfo};
use crate::risk::PositionPlan;
use crate::signer::Signer;
use crate::wallet::Wallet;
use crate::error::Result;

/// Order execution engine
pub struct Executor {
    rest: HyperliquidRest,
    signer: Signer,
    wallet: Wallet,
    asset_ids: std::collections::HashMap<String, u32>,
    /// Asset info with sz_decimals for correct order formatting
    asset_info: std::collections::HashMap<String, AssetInfo>,
    dry_run: bool,
}

/// Result of an order attempt
#[derive(Debug, Clone)]
pub struct OrderResult {
    pub success: bool,
    pub oid: Option<u64>,
    pub message: String,
    /// Average fill price (None if not filled or dry run)
    pub fill_price: Option<f64>,
    /// Filled size in units (None if not filled or dry run)
    pub filled_size: Option<f64>,
}

impl OrderResult {
    pub fn filled(&self) -> bool {
        self.success && self.fill_price.is_some()
    }
}

impl Executor {
    pub fn new(
        rest: HyperliquidRest,
        signer: Signer,
        wallet: Wallet,
        asset_ids: std::collections::HashMap<String, u32>,
        asset_info: std::collections::HashMap<String, AssetInfo>,
        dry_run: bool,
    ) -> Self {
        Self {
            rest,
            signer,
            wallet,
            asset_ids,
            asset_info,
            dry_run,
        }
    }

    /// Execute a limit order at best bid/ask with 200ms timeout
    ///
    /// - Long: limit buy at best bid
    /// - Short: limit sell at best ask
    /// - If not filled within 200ms, cancel. No market order fallback.
    pub async fn execute_limit_order(
        &self,
        plan: &PositionPlan,
        best_bid: f64,
        best_ask: f64,
    ) -> Result<OrderResult> {
        let (limit_price, is_buy) = match plan.direction {
            SignalDirection::Long => (best_bid, true),
            SignalDirection::Short => (best_ask, false),
        };

        let asset_id = match self.asset_ids.get(&plan.pair) {
            Some(&id) => id,
            None => {
                return Ok(OrderResult {
                    success: false,
                    oid: None,
                    message: format!("Unknown asset: {}", plan.pair),
                    fill_price: None,
                    filled_size: None,
                });
            }
        };

        // Validate size is positive and finite
        if !plan.size_units.is_finite() || plan.size_units <= 0.0 {
            return Ok(OrderResult {
                success: false,
                oid: None,
                message: format!("Invalid size: {}", plan.size_units),
                fill_price: None,
                filled_size: None,
            });
        }

        // Use correct decimal places from asset metadata
        let sz_decimals = self.asset_info.get(&plan.pair)
            .map(|a| a.sz_decimals as usize)
            .unwrap_or(2);
        let size_str = format!("{:.prec$}", plan.size_units, prec = sz_decimals);
        let price_str = format!("{:.2}", limit_price);

        info!(
            "EXEC {:?} {} {} @ {} (limit)",
            plan.direction, plan.pair, size_str, price_str
        );

        if self.dry_run {
            info!("DRY RUN — order not sent");
            return Ok(OrderResult {
                success: true,
                oid: Some(0),
                message: "Dry run".to_string(),
                fill_price: Some(limit_price),
                filled_size: Some(plan.size_units),
            });
        }

        // Build order action
        let order_action = json!({
            "orders": [{
                "a": asset_id,
                "b": is_buy,
                "p": price_str,
                "s": size_str,
                "r": false,
                "t": { "limit": { "tif": "Ioc" } }
            }],
            "grouping": "na"
        });

        // Sign and send
        let nonce = chrono::Utc::now().timestamp_millis() as u64;
        let signed = self.signer.sign_action(&self.wallet, "place", order_action, nonce)?;

        let result = self.rest.exchange(signed).await?;

        // Parse fill details from response
        let status = result
            .get("statuses")
            .and_then(|s| s.as_array())
            .and_then(|a| a.first());

        // Check for filled order (IOC fill)
        let filled = status.and_then(|s| s.get("filled"));
        if let Some(filled_data) = filled {
            let avg_px = filled_data
                .get("avgPx")
                .and_then(|p| p.as_str())
                .and_then(|s| s.parse::<f64>().ok());
            let total_sz = filled_data
                .get("totalSz")
                .and_then(|s| s.as_str())
                .and_then(|s| s.parse::<f64>().ok());
            let fill_oid = filled_data
                .get("oid")
                .and_then(|o| o.as_u64());

            info!("Order filled! avgPx={:?} totalSz={:?} oid={:?}", avg_px, total_sz, fill_oid);
            return Ok(OrderResult {
                success: true,
                oid: fill_oid,
                message: "Filled".to_string(),
                fill_price: avg_px,
                filled_size: total_sz,
            });
        }

        // Check if order is resting (not immediately filled)
        let oid = status
            .and_then(|s| s.get("resting"))
            .and_then(|r| r.get("oid"))
            .and_then(|o| o.as_u64());

        if let Some(oid) = oid {
            // Order is resting — wait 200ms then cancel
            info!("Order resting (oid={}), waiting 200ms...", oid);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Cancel the order
            match self.cancel_order(&plan.pair, oid).await {
                Ok(_) => {
                    info!("Order {} cancelled after timeout", oid);
                    Ok(OrderResult {
                        success: false,
                        oid: Some(oid),
                        message: "Cancelled after 200ms timeout".to_string(),
                        fill_price: None,
                        filled_size: None,
                    })
                }
                Err(e) => {
                    // Cancel failed — status unknown, don't claim success
                    warn!("Cancel failed for oid {}: {} — fill status unknown", oid, e);
                    Ok(OrderResult {
                        success: false,
                        oid: Some(oid),
                        message: format!("Cancel failed (fill unknown): {}", e),
                        fill_price: None,
                        filled_size: None,
                    })
                }
            }
        } else {
            // No resting order and no filled status — check for error
            let err_msg = status
                .and_then(|s| s.get("error"))
                .and_then(|e| e.as_str())
                .unwrap_or("unknown response");

            warn!("Order response unclear: {}", err_msg);
            Ok(OrderResult {
                success: false,
                oid: None,
                message: format!("Unclear: {}", err_msg),
                fill_price: None,
                filled_size: None,
            })
        }
    }

    /// Cancel an order
    async fn cancel_order(&self, pair: &str, oid: u64) -> Result<()> {
        let asset_id = self.asset_ids.get(pair).copied()
            .ok_or_else(|| crate::error::BotError::Order(format!("Unknown asset for cancel: {}", pair)))?;

        let cancel_action = json!({
            "cancels": [{
                "a": asset_id,
                "o": oid
            }]
        });

        let nonce = chrono::Utc::now().timestamp_millis() as u64;
        let signed = self.signer.sign_action(&self.wallet, "cancel", cancel_action, nonce)?;
        self.rest.exchange(signed).await?;

        Ok(())
    }

    /// Get wallet address (for position fetching)
    pub fn wallet_address(&self) -> &str {
        self.wallet.address()
    }
}
