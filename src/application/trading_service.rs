use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::config::{RiskConfig, StrategyConfig};
use super::lesson_engine::LessonEngine;
use super::strategy_engine::StrategyEngine;
use crate::domain::{Order, OrderResult, OrderStatus, Position, PositionTracker, Side, Signal, TradeStore};
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
    store: Arc<dyn TradeStore>,
    strategy_engine: Arc<StrategyEngine>,
    lesson_engine: Arc<LessonEngine>,
    config: StrategyConfig,
    risk: RiskConfig,
}

impl TradingService {
    pub fn new(
        hyperliquid: Arc<HyperliquidClient>,
        sodex: Arc<SodexClient>,
        wallet: Arc<Wallet>,
        store: Arc<dyn TradeStore>,
        strategy_engine: Arc<StrategyEngine>,
        lesson_engine: Arc<LessonEngine>,
        config: StrategyConfig,
        risk: RiskConfig,
    ) -> Self {
        Self {
            hyperliquid,
            sodex,
            wallet,
            positions: Arc::new(RwLock::new(PositionTracker::new())),
            store,
            strategy_engine,
            lesson_engine,
            config,
            risk,
        }
    }

    /// Main trading tick - fetch signals, evaluate with strategy engine, execute, learn
    pub async fn tick(&self) -> Result<Vec<OrderResult>> {
        // Reset daily PnL if new day
        self.positions.write().await.maybe_reset_daily();

        // Fetch signals
        let signals = self.sodex.get_signals().await.unwrap_or_default();
        if signals.is_empty() {
            return Ok(Vec::new());
        }

        // Get market data for strategy engine
        let market = self.hyperliquid.get_market_data().await;

        // Evaluate signals WITH Strategy Engine (applies dynamic rules)
        let decisions = self.evaluate_signals_with_strategy(&signals, &market).await?;
        if decisions.is_empty() {
            return Ok(Vec::new());
        }

        // Execute trades
        let results = self.execute_trades(&decisions).await;

        // Update positions based on fills
        self.update_positions(&results).await;

        // Feed results to Lesson Engine (learn from outcomes)
        self.learn_from_trades(&results, &signals, &market).await;

        Ok(results)
    }

    /// Evaluate signals with Strategy Engine applying dynamic rules
    async fn evaluate_signals_with_strategy(
        &self,
        signals: &[Signal],
        market: &crate::domain::MarketData,
    ) -> Result<Vec<TradeDecision>> {
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

            // Skip open if already have position
            if signal.is_open() && positions.has_position(&signal.coin) {
                continue;
            }

            // Skip close if no position
            if signal.is_close() && !positions.has_position(&signal.coin) {
                continue;
            }

            // Apply Strategy Engine — get adjusted params
            let params = self.strategy_engine.evaluate(signal, market, &positions).await;

            // Check if strategy says skip
            if !params.should_execute {
                debug!(
                    "Strategy skip: {} | {}",
                    signal.coin,
                    params.reasons_summary()
                );
                continue;
            }

            // Check adjusted confidence threshold
            if signal.confidence < params.confidence_threshold {
                debug!(
                    "Confidence {} < threshold {} for {}",
                    signal.confidence, params.confidence_threshold, signal.coin
                );
                continue;
            }

            if let Some(decision) = self.signal_to_decision_with_params(signal, &positions, &params) {
                info!(
                    "Strategy: {} | {}",
                    signal.coin,
                    params.reasons_summary()
                );
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

    /// Convert signal to trade decision with strategy-adjusted params
    fn signal_to_decision_with_params(
        &self,
        signal: &Signal,
        positions: &PositionTracker,
        params: &crate::domain::StrategyParams,
    ) -> Option<TradeDecision> {
        let side = signal.to_side();
        let price = signal.entry_price.unwrap_or(0.0);

        if price <= 0.0 {
            debug!("No price for {}, skipping", signal.coin);
            return None;
        }

        // Apply size multiplier from strategy
        let base_size = self.calc_position_size(price, positions);
        let size = base_size * params.size_multiplier;
        if size <= 0.0 {
            return None;
        }

        let mut order = Order::limit(&signal.coin, side, size, price)
            .with_leverage(params.leverage);

        // Auto SL/TP from strategy-adjusted params
        let sl_pct = params.stop_loss_pct / 100.0;
        let tp_pct = params.take_profit_pct / 100.0;

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
                "Signal {:?}: {} conf {:.2} | {}",
                signal.source, signal.coin, signal.confidence, params.reasons_summary()
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
                        OrderStatus::Cancelled { oid } => {
                            info!("🚫 Cancelled: oid={}", oid);
                        }
                        OrderStatus::Error { message } => {
                            error!("❌ Failed: {}", message);
                        }
                        _ => {}
                    }
                    // Log to database
                    if let Err(e) = self
                        .store
                        .log_trade(
                            &result.order.coin,
                            result.order.side,
                            result.order.size,
                            result.order.price,
                            &result.status,
                            result.timestamp as i64,
                        )
                        .await
                    {
                        warn!("Failed to log trade to DB: {}", e);
                    }
                    results.push(result);
                }
                Err(e) => {
                    warn!("Execution error: {}", e);
                }
            }

            // Rate limit protection: delay between orders
            tokio::time::sleep(Duration::from_millis(200)).await;
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
                // Calculate PnL before updating position tracker
                let pnl = if let Some(pos) = positions.get(&result.order.coin) {
                    let is_opposite = (pos.side == crate::domain::PositionSide::Long
                        && result.order.side == Side::Sell)
                        || (pos.side == crate::domain::PositionSide::Short
                            && result.order.side == Side::Buy);
                    if is_opposite {
                        let close_size = total_sz.min(pos.size);
                        crate::domain::PositionTracker::calc_pnl_static(pos, *avg_px, close_size)
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };

                positions.update(&result.order.coin, result.order.side, *total_sz, *avg_px);

                // Persist PnL to database
                if pnl.abs() > 0.0 {
                    if let Err(e) = self.store.update_trade_pnl(&result.order.coin, pnl).await {
                        warn!("Failed to update PnL in DB: {}", e);
                    }
                }
            }
        }
    }

    /// Feed trade results to Lesson Engine for learning
    async fn learn_from_trades(
        &self,
        results: &[OrderResult],
        signals: &[Signal],
        market: &crate::domain::MarketData,
    ) {
        for result in results {
            // Find the matching signal
            let signal = signals.iter().find(|s| s.coin == result.order.coin);
            if let Some(signal) = signal {
                match self.lesson_engine.analyze_trade(result, signal, market).await {
                    Ok(Some(lesson)) => {
                        if lesson.rule_generated {
                            // Reload strategy rules when new lesson rule is created
                            if let Err(e) = self.strategy_engine.reload_rules().await {
                                warn!("Failed to reload strategy rules: {}", e);
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!("Lesson analysis error: {}", e);
                    }
                }
            }
        }
    }

    fn order_summary(order: &Order) -> String {
        format!(
            "{:?} {:.6} {} @ {:.2}",
            order.side, order.size, order.coin, order.price
        )
    }

    /// Load positions from exchange sync (startup)
    pub async fn load_positions(&self, positions: Vec<Position>) {
        self.positions.write().await.load_positions(positions);
    }

    /// Get position summary
    pub async fn position_summary(&self) -> String {
        self.positions.read().await.summary()
    }

    /// Get strategy engine stats
    pub async fn strategy_stats(&self) -> String {
        self.strategy_engine.rule_stats().await
    }

    /// Get lesson engine stats
    pub async fn lesson_stats(&self) -> String {
        self.lesson_engine.stats_summary().await
    }
}
