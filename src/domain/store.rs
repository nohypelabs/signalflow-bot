use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::domain::order::{OrderStatus, Side};
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
}
