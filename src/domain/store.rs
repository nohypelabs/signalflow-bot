use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::lesson::{CauseInfo, Lesson, LessonType, LessonStats, Outcome, TradeAnalysis};
use crate::domain::order::{OrderStatus, Side};
use crate::domain::strategy::StrategyRule;
use crate::error::Result;

/// Trade record for storage/retrieval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub id: i64,
    pub timestamp: i64,
    pub coin: String,
    pub side: String,
    pub size: f64,
    pub price: f64,
    pub status: String,
    pub order_id: i64,
    pub avg_fill_price: f64,
    pub filled_size: f64,
    pub pnl: f64,
    pub created_at: String,
}

/// Abstract storage trait — implement with SQLite locally, PostgreSQL for Supabase
#[async_trait]
pub trait TradeStore: Send + Sync {
    /// Initialize store (run migrations, create tables)
    async fn init(&self) -> Result<()>;

    // ===== TRADE METHODS =====

    /// Log a trade execution
    async fn log_trade(
        &self,
        coin: &str,
        side: Side,
        size: f64,
        price: f64,
        status: &OrderStatus,
        timestamp: i64,
    ) -> Result<()>;

    /// Update PnL for a trade (after position close)
    async fn update_trade_pnl(&self, coin: &str, pnl: f64) -> Result<()>;

    /// Get recent trades
    async fn get_trades(&self, limit: i64) -> Result<Vec<TradeRecord>>;

    /// Get trades for a specific coin
    async fn get_trades_by_coin(&self, coin: &str, limit: i64) -> Result<Vec<TradeRecord>>;

    /// Get total realized PnL for today
    async fn get_daily_pnl(&self) -> Result<f64>;

    /// Get total realized PnL (all time)
    async fn get_total_pnl(&self) -> Result<f64>;

    /// Get trade count
    async fn get_trade_count(&self) -> Result<i64>;

    // ===== LESSON METHODS =====

    /// Save a lesson learned from a trade
    async fn save_lesson(
        &self,
        trade_id: i64,
        coin: &str,
        outcome: Outcome,
        entry_price: f64,
        exit_price: f64,
        pnl: f64,
        hold_duration_secs: i64,
        cause: &CauseInfo,
        lesson_type: LessonType,
    ) -> Result<i64>;

    /// Get recent lessons
    async fn get_lessons(&self, limit: i64) -> Result<Vec<Lesson>>;

    /// Get lessons by outcome (win/loss)
    async fn get_lessons_by_outcome(&self, outcome: Outcome, limit: i64) -> Result<Vec<Lesson>>;

    /// Save detailed trade analysis
    async fn save_trade_analysis(&self, analysis: &TradeAnalysis) -> Result<()>;

    /// Get lesson stats
    async fn get_lesson_stats(&self) -> Result<LessonStats>;

    // ===== STRATEGY RULE METHODS =====

    /// Save a strategy rule
    async fn save_rule(&self, rule: &StrategyRule) -> Result<i64>;

    /// Get all active rules
    async fn get_active_rules(&self) -> Result<Vec<StrategyRule>>;

    /// Increment rule hit count
    async fn increment_rule_hits(&self, rule_id: i64) -> Result<()>;

    /// Deactivate a rule
    async fn deactivate_rule(&self, rule_id: i64) -> Result<()>;

    /// Get rule count by source
    async fn get_rule_count_by_source(&self, source: &str) -> Result<i64>;
}
