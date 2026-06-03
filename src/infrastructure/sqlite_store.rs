use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tracing::{debug, info, warn};

use crate::domain::order::{OrderStatus, Side};
use crate::domain::store::{TradeRecord, TradeStore};
use crate::error::{BotError, Result};

/// SQLite-backed trade store
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Create a new SQLite store with connection pool
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|e| BotError::Config(format!("SQLite connection failed: {}", e)))?;

        info!("SQLite connected: {}", database_url);
        Ok(Self { pool })
    }
}

#[async_trait]
impl TradeStore for SqliteStore {
    /// Run migrations — create tables if not exist
    async fn init(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                coin TEXT NOT NULL,
                side TEXT NOT NULL,
                size REAL NOT NULL,
                price REAL NOT NULL,
                status TEXT NOT NULL,
                order_id INTEGER NOT NULL DEFAULT 0,
                avg_fill_price REAL NOT NULL DEFAULT 0.0,
                filled_size REAL NOT NULL DEFAULT 0.0,
                pnl REAL NOT NULL DEFAULT 0.0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Config(format!("SQLite migration failed: {}", e)))?;

        // Index for fast queries
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_trades_coin ON trades(coin)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_trades_timestamp ON trades(timestamp)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_trades_created_at ON trades(created_at)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        info!("SQLite migrations complete");
        Ok(())
    }

    /// Log a trade execution
    async fn log_trade(
        &self,
        coin: &str,
        side: Side,
        size: f64,
        price: f64,
        status: &OrderStatus,
        timestamp: i64,
    ) -> Result<()> {
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        let (status_str, order_id, avg_px, filled_sz) = match status {
            OrderStatus::Filled {
                total_sz,
                avg_px,
                oid,
            } => ("filled", *oid as i64, *avg_px, *total_sz),
            OrderStatus::Resting { oid } => ("resting", *oid as i64, 0.0, 0.0),
            OrderStatus::Cancelled { oid } => ("cancelled", *oid as i64, 0.0, 0.0),
            OrderStatus::Error { message } => {
                warn!("Logging error trade: {}", message);
                ("error", 0i64, 0.0, 0.0)
            }
            OrderStatus::Pending => ("pending", 0i64, 0.0, 0.0),
        };

        sqlx::query(
            r#"
            INSERT INTO trades (timestamp, coin, side, size, price, status, order_id, avg_fill_price, filled_size)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(timestamp)
        .bind(coin)
        .bind(side_str)
        .bind(size)
        .bind(price)
        .bind(status_str)
        .bind(order_id)
        .bind(avg_px)
        .bind(filled_sz)
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Order(format!("Failed to log trade: {}", e)))?;

        debug!("Logged trade: {} {} {:.6} @ {:.2}", side_str, coin, size, price);
        Ok(())
    }

    /// Update PnL for the most recent trade on a coin
    async fn update_trade_pnl(&self, coin: &str, pnl: f64) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE trades SET pnl = ?
            WHERE id = (
                SELECT id FROM trades
                WHERE coin = ? AND status = 'filled'
                ORDER BY timestamp DESC
                LIMIT 1
            )
            "#,
        )
        .bind(pnl)
        .bind(coin)
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Order(format!("Failed to update PnL: {}", e)))?;

        Ok(())
    }

    /// Get recent trades
    async fn get_trades(&self, limit: i64) -> Result<Vec<TradeRecord>> {
        let rows = sqlx::query_as::<_, TradeRow>(
            "SELECT id, timestamp, coin, side, size, price, status, order_id, avg_fill_price, filled_size, pnl, created_at FROM trades ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to fetch trades: {}", e)))?;

        Ok(rows.into_iter().map(TradeRow::into_record).collect())
    }

    /// Get trades for a specific coin
    async fn get_trades_by_coin(&self, coin: &str, limit: i64) -> Result<Vec<TradeRecord>> {
        let rows = sqlx::query_as::<_, TradeRow>(
            "SELECT id, timestamp, coin, side, size, price, status, order_id, avg_fill_price, filled_size, pnl, created_at FROM trades WHERE coin = ? ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(coin)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to fetch trades: {}", e)))?;

        Ok(rows.into_iter().map(TradeRow::into_record).collect())
    }

    /// Get total realized PnL for today (UTC)
    async fn get_daily_pnl(&self) -> Result<f64> {
        let row: (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(pnl), 0.0) FROM trades WHERE date(created_at) = date('now') AND status = 'filled'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to get daily PnL: {}", e)))?;

        Ok(row.0)
    }

    /// Get total realized PnL (all time)
    async fn get_total_pnl(&self) -> Result<f64> {
        let row: (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(pnl), 0.0) FROM trades WHERE status = 'filled'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to get total PnL: {}", e)))?;

        Ok(row.0)
    }

    /// Get trade count
    async fn get_trade_count(&self) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get trade count: {}", e)))?;

        Ok(row.0)
    }
}

/// SQLite row mapping
#[derive(sqlx::FromRow)]
struct TradeRow {
    id: i64,
    timestamp: i64,
    coin: String,
    side: String,
    size: f64,
    price: f64,
    status: String,
    order_id: i64,
    avg_fill_price: f64,
    filled_size: f64,
    pnl: f64,
    created_at: String,
}

impl TradeRow {
    fn into_record(self) -> TradeRecord {
        TradeRecord {
            id: self.id,
            timestamp: self.timestamp,
            coin: self.coin,
            side: self.side,
            size: self.size,
            price: self.price,
            status: self.status,
            order_id: self.order_id,
            avg_fill_price: self.avg_fill_price,
            filled_size: self.filled_size,
            pnl: self.pnl,
            created_at: self.created_at,
        }
    }
}
