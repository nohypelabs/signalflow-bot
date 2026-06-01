use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::config::{RiskConfig, StrategyConfig};
use crate::domain::{Order, OrderResult, OrderStatus, PositionTracker, Side, Signal};
use crate::error::Result;
use crate::infrastructure::{HyperliquidClient, SodexClient, Wallet};

/// Trading decision
#[derive(Debug)]
pub struct TradeDecision {
    pub order: Order,
    pub reason: String,
}

/// Core trading service - orchestrates the trading flow
pub struct TradingService {
    hyperliquid: Arc<HyperliquidClient>,
    sodex: Arc<SodexClient>,
    wallet: Arc<Wallet>,
    positions: Arc<RwLock<PositionTracker>>,
    config: StrategyConfig,
    risk: RiskConfig,
}

impl TradingService {
    pub fn new(
        hyperliquid: Arc<HyperliquidClient>,
        sodex: Arc<SodexClient>,
        wallet: Arc<Wallet>,
        config: StrategyConfig,
        risk: RiskConfig,
    ) -> Self {
        Self {
            hyperliquid,
            sodex,
            wallet,
            positions: Arc::new(RwLock::new(PositionTracker::new())),
            config,
            risk,
        }
    }

    /// Main trading tick - fetch signals, evaluate, execute
    pub async fn tick(&self) -> Result<Vec<OrderResult>> {
        // Reset daily PnL if new day
        self.positions.write().await.maybe_reset_daily();

        // Fetch signals
        let signals = self.sodex.get_signals().await.unwrap_or_default();
        if signals.is_empty() {
            return Ok(Vec::new());
        }

        // Evaluate and generate decisions
        let decisions = self.evaluate_signals(&signals).await?;
        if decisions.is_empty() {
            return Ok(Vec::new());
        }

        // Execute trades
        let results = self.execute_trades(&decisions).await;

        // Update positions based on fills
        self.update_positions(&results).await;

        Ok(results)
    }

    /// Evaluate signals and generate trade decisions
    async fn evaluate_signals(&self, signals: &[Signal]) -> Result<Vec<TradeDecision>> {
        let mut decisions = Vec::new();
        let mut seen = HashSet::new();
        let positions = self.positions.read().await;

        for signal in signals {
            // Dedup
            if seen.contains(&signal.coin) {
                continue;
            }

            // Check risk limits
            if !self.check_risk_limits(&positions) {
                break;
            }

            // Check confidence
            if signal.confidence < 0.5 {
                continue;
            }

            // Skip open if already have position
            if signal.is_open() && positions.has_position(&signal.coin) {
                continue;
            }

            // Skip close if no position
            if signal.is_close() && !positions.has_position(&signal.coin) {
                continue;
            }

            if let Some(decision) = self.signal_to_decision(signal, &positions) {
                decisions.push(decision);
                seen.insert(signal.coin.clone());
            }
        }

        info!(
            "Generated {} trade decisions from {} signals",
            decisions.len(),
            signals.len()
        );
        Ok(decisions)
    }

    /// Convert signal to trade decision with auto SL/TP
    fn signal_to_decision(
        &self,
        signal: &Signal,
        positions: &PositionTracker,
    ) -> Option<TradeDecision> {
        let side = signal.to_side();
        let price = signal.entry_price.unwrap_or(0.0);

        if price <= 0.0 {
            debug!("No price for {}, skipping", signal.coin);
            return None;
        }

        let size = self.calc_position_size(price, positions);
        if size <= 0.0 {
            return None;
        }

        let mut order =
            Order::limit(&signal.coin, side, size, price).with_leverage(self.config.max_leverage);

        // Auto SL/TP from config if not provided by signal
        let sl_pct = self.risk.stop_loss_pct / 100.0;
        let tp_pct = self.risk.take_profit_pct / 100.0;

        order.stop_loss = signal.stop_loss.or(match side {
            Side::Buy => Some(price * (1.0 - sl_pct)),
            Side::Sell => Some(price * (1.0 + sl_pct)),
        });

        order.take_profit = signal.take_profit.or(match side {
            Side::Buy => Some(price * (1.0 + tp_pct)),
            Side::Sell => Some(price * (1.0 - tp_pct)),
        });

        Some(TradeDecision {
            order,
            reason: format!(
                "Signal {:?}: {} confidence {:.2}",
                signal.source, signal.coin, signal.confidence
            ),
        })
    }

    /// Calculate position size based on risk
    fn calc_position_size(&self, price: f64, positions: &PositionTracker) -> f64 {
        if price <= 0.0 {
            return 0.0;
        }

        let max_size = self.config.max_position_size / price;
        let remaining = (self.risk.max_total_exposure - positions.total_exposure()).max(0.0);
        let exposure_size = remaining / price;

        let size = max_size.min(exposure_size);
        let min_value = 10.0; // Hyperliquid minimum

        if size * price < min_value {
            0.0
        } else {
            size
        }
    }

    /// Check risk limits
    fn check_risk_limits(&self, positions: &PositionTracker) -> bool {
        if positions.total_exposure() >= self.risk.max_total_exposure {
            return false;
        }
        if positions.daily_pnl() <= -self.risk.max_daily_loss {
            return false;
        }
        if positions.count() >= 5 {
            return false;
        }
        true
    }

    /// Execute trades
    async fn execute_trades(&self, decisions: &[TradeDecision]) -> Vec<OrderResult> {
        let mut results = Vec::new();

        for decision in decisions {
            if self.config.dry_run {
                info!(
                    "[DRY RUN] {} | {}",
                    decision.reason,
                    Self::order_summary(&decision.order)
                );
                continue;
            }

            info!(
                "Executing: {} | {}",
                Self::order_summary(&decision.order),
                decision.reason
            );

            match self
                .hyperliquid
                .place_order(&self.wallet, &decision.order)
                .await
            {
                Ok(result) => {
                    match &result.status {
                        OrderStatus::Filled {
                            total_sz,
                            avg_px,
                            oid,
                        } => {
                            info!(
                                "✅ Filled: size={}, avg_price={}, oid={}",
                                total_sz, avg_px, oid
                            );
                        }
                        OrderStatus::Resting { oid } => {
                            info!("⏳ Resting: oid={}", oid);
                        }
                        OrderStatus::Error { message } => {
                            error!("❌ Failed: {}", message);
                        }
                        _ => {}
                    }
                    results.push(result);
                }
                Err(e) => {
                    warn!("Execution error: {}", e);
                }
            }
        }

        results
    }

    /// Update positions from execution results
    async fn update_positions(&self, results: &[OrderResult]) {
        let mut positions = self.positions.write().await;
        for result in results {
            if let OrderStatus::Filled {
                total_sz, avg_px, ..
            } = &result.status
            {
                positions.update(&result.order.coin, result.order.side, *total_sz, *avg_px);
            }
        }
    }

    fn order_summary(order: &Order) -> String {
        format!(
            "{:?} {:.6} {} @ {:.2}",
            order.side, order.size, order.coin, order.price
        )
    }

    /// Get position summary
    pub async fn position_summary(&self) -> String {
        self.positions.read().await.summary()
    }
}
