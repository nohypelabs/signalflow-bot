mod rest;
mod ws;

pub use rest::HyperliquidRest;
pub use ws::HyperliquidWs;

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::signer::Signer;
use super::wallet::Wallet;
use crate::domain::{MarketData, Order, OrderResult, OrderStatus};
use crate::error::Result;

/// Combined Hyperliquid client (WebSocket + REST)
pub struct HyperliquidClient {
    rest: HyperliquidRest,
    ws: HyperliquidWs,
    signer: Signer,
    market_data: Arc<RwLock<MarketData>>,
    asset_cache: Arc<RwLock<Option<std::collections::HashMap<String, u32>>>>,
}

impl HyperliquidClient {
    pub async fn new(base_url: &str, ws_url: &str) -> Result<Self> {
        let market_data = Arc::new(RwLock::new(MarketData::new()));
        let asset_cache = Arc::new(RwLock::new(None));

        Ok(Self {
            rest: HyperliquidRest::new(base_url),
            ws: HyperliquidWs::new(ws_url, market_data.clone()),
            signer: Signer::new(),
            market_data,
            asset_cache,
        })
    }

    /// Start WebSocket connection for real-time market data
    pub async fn start_stream(&self) -> Result<()> {
        self.ws.connect().await
    }

    /// Get current market data
    pub async fn get_market_data(&self) -> MarketData {
        self.market_data.read().await.clone()
    }

    /// Get price for a specific coin
    pub async fn get_price(&self, coin: &str) -> Option<f64> {
        self.market_data.read().await.get_price(coin)
    }

    /// Fetch asset IDs and cache them
    pub async fn fetch_asset_ids(&self) -> Result<()> {
        let assets = self.rest.fetch_meta().await?;
        let mut cache = self.asset_cache.write().await;
        *cache = Some(assets);
        info!("Asset IDs cached");
        Ok(())
    }

    /// Get asset ID for a coin
    pub async fn get_asset_id(&self, coin: &str) -> u32 {
        // Check cache
        {
            let cache = self.asset_cache.read().await;
            if let Some(ref map) = *cache {
                if let Some(&id) = map.get(&coin.to_uppercase()) {
                    return id;
                }
            }
        }

        // Cache miss - fetch
        match self.rest.fetch_meta().await {
            Ok(map) => {
                let id = *map.get(&coin.to_uppercase()).unwrap_or(&0);
                let mut cache = self.asset_cache.write().await;
                *cache = Some(map);
                id
            }
            Err(e) => {
                warn!("Failed to fetch asset IDs: {}", e);
                Self::fallback_asset_id(coin)
            }
        }
    }

    /// Place an order with signing
    pub async fn place_order(&self, wallet: &Wallet, order: &Order) -> Result<OrderResult> {
        order.validate().map_err(crate::error::BotError::Order)?;

        let asset_id = self.get_asset_id(&order.coin).await;
        let nonce = Self::now_millis();

        let signed = self.signer.sign_action(
            wallet,
            "order",
            self.build_order_payload(order, asset_id),
            nonce,
        )?;

        let result = self.rest.exchange(signed).await?;

        Ok(OrderResult {
            order: order.clone(),
            status: Self::parse_order_status(&result),
            timestamp: nonce,
        })
    }

    /// Update leverage
    pub async fn update_leverage(
        &self,
        wallet: &Wallet,
        coin: &str,
        leverage: u32,
        is_cross: bool,
    ) -> Result<()> {
        let asset_id = self.get_asset_id(coin).await;
        let nonce = Self::now_millis();

        let signed = self.signer.sign_action(
            wallet,
            "updateLeverage",
            serde_json::json!({
                "asset": asset_id,
                "isCross": is_cross,
                "leverage": leverage
            }),
            nonce,
        )?;

        self.rest.exchange(signed).await?;
        Ok(())
    }

    fn build_order_payload(&self, order: &Order, asset_id: u32) -> serde_json::Value {
        let is_buy = order.side == crate::domain::Side::Buy;

        // Price with slippage for market orders
        let price = match order.order_type {
            crate::domain::OrderType::Market => {
                if is_buy {
                    order.price * 1.01
                } else {
                    order.price * 0.99
                }
            }
            _ => order.price,
        };

        let order_type = match order.order_type {
            crate::domain::OrderType::Market => serde_json::json!({"limit": {"tif": "Ioc"}}),
            crate::domain::OrderType::Limit => serde_json::json!({"limit": {"tif": "Gtc"}}),
            crate::domain::OrderType::StopLoss => {
                serde_json::json!({"trigger": {"isMarket": true, "triggerPx": price.to_string(), "tpsl": "sl"}})
            }
            crate::domain::OrderType::TakeProfit => {
                serde_json::json!({"trigger": {"isMarket": true, "triggerPx": price.to_string(), "tpsl": "tp"}})
            }
        };

        serde_json::json!({
            "orders": [{
                "a": asset_id,
                "b": is_buy,
                "p": format!("{:.2}", price),
                "s": format!("{:.6}", order.size),
                "r": order.reduce_only,
                "t": order_type,
                "c": order.client_order_id.clone().unwrap_or_default()
            }],
            "grouping": "na"
        })
    }

    fn parse_order_status(result: &serde_json::Value) -> OrderStatus {
        let statuses = result
            .get("statuses")
            .and_then(|s| s.as_array())
            .and_then(|a| a.first());

        match statuses {
            Some(status) => {
                if let Some(resting) = status.get("resting") {
                    OrderStatus::Resting {
                        oid: resting.get("oid").and_then(|o| o.as_u64()).unwrap_or(0),
                    }
                } else if let Some(filled) = status.get("filled") {
                    OrderStatus::Filled {
                        total_sz: filled
                            .get("totalSz")
                            .and_then(|s| s.as_str())
                            .unwrap_or("0")
                            .parse()
                            .unwrap_or(0.0),
                        avg_px: filled
                            .get("avgPx")
                            .and_then(|s| s.as_str())
                            .unwrap_or("0")
                            .parse()
                            .unwrap_or(0.0),
                        oid: filled.get("oid").and_then(|o| o.as_u64()).unwrap_or(0),
                    }
                } else if let Some(err) = status.get("error") {
                    OrderStatus::Error {
                        message: err.as_str().unwrap_or("Unknown").to_string(),
                    }
                } else {
                    OrderStatus::Error {
                        message: "Unknown format".to_string(),
                    }
                }
            }
            None => OrderStatus::Error {
                message: "Empty status".to_string(),
            },
        }
    }

    fn fallback_asset_id(coin: &str) -> u32 {
        match coin.to_uppercase().as_str() {
            "BTC" => 0,
            "ETH" => 1,
            "SOL" => 2,
            "DOGE" => 3,
            "ARB" => 4,
            "AVAX" => 5,
            "LINK" => 6,
            "APE" => 7,
            "DYDX" => 8,
            "ATOM" => 9,
            "NEAR" => 10,
            "FTM" => 11,
            "INJ" => 12,
            "SUI" => 13,
            "APT" => 14,
            "TIA" => 15,
            "SEI" => 16,
            "JUP" => 17,
            "ORDI" => 18,
            "WIF" => 19,
            "STX" => 20,
            "BONK" => 21,
            "PEPE" | "1000PEPE" => 22,
            "WLD" => 23,
            "BLUR" => 24,
            "PENDLE" => 25,
            "ETHFI" => 26,
            "ENA" => 27,
            "W" => 28,
            "TAO" => 29,
            "RENDER" => 30,
            "FET" => 31,
            "ARKM" => 32,
            "IMX" => 33,
            "AAVE" => 34,
            "FLOKI" | "1000FLOKI" => 35,
            "GALA" => 36,
            "SAND" => 37,
            "MANA" => 38,
            "AXS" => 39,
            "OP" => 40,
            "LDO" => 41,
            _ => 0,
        }
    }

    fn now_millis() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}
