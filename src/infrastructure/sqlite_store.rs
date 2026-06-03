use async_trait::async_trait;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use tracing::{debug, info, warn};

use crate::domain::lesson::{CauseInfo, Lesson, LessonStats, LessonType, Outcome, TradeAnalysis};
use crate::domain::order::{OrderStatus, Side};
use crate::domain::store::{TradeRecord, TradeStore};
use crate::domain::strategy::StrategyRule;
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

        // Strategy rules table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS strategy_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                rule_type TEXT NOT NULL,
                conditions TEXT NOT NULL,
                action TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                active INTEGER NOT NULL DEFAULT 1,
                source TEXT NOT NULL DEFAULT 'default',
                hit_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Config(format!("SQLite migration failed: {}", e)))?;

        // Lessons table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS lessons (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trade_id INTEGER,
                coin TEXT NOT NULL,
                outcome TEXT NOT NULL,
                entry_price REAL NOT NULL,
                exit_price REAL NOT NULL,
                pnl REAL NOT NULL,
                hold_duration_secs INTEGER NOT NULL,
                cause TEXT NOT NULL,
                lesson_type TEXT NOT NULL,
                rule_generated INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Config(format!("SQLite migration failed: {}", e)))?;

        // Trade analysis table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS trade_analysis (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                trade_id INTEGER NOT NULL,
                coin TEXT NOT NULL,
                side TEXT NOT NULL,
                entry_price REAL NOT NULL,
                exit_price REAL NOT NULL DEFAULT 0.0,
                pnl REAL NOT NULL DEFAULT 0.0,
                hold_duration INTEGER NOT NULL DEFAULT 0,
                signal_source TEXT NOT NULL,
                signal_confidence REAL NOT NULL DEFAULT 0.0,
                market_regime TEXT NOT NULL DEFAULT 'unknown',
                session TEXT NOT NULL DEFAULT 'unknown',
                slippage_pct REAL NOT NULL DEFAULT 0.0,
                cause TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Config(format!("SQLite migration failed: {}", e)))?;

        // Indexes for lessons and rules
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_lessons_outcome ON lessons(outcome)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_lessons_coin ON lessons(coin)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_rules_active ON strategy_rules(active)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        info!("SQLite migrations complete (trades, lessons, strategy_rules, trade_analysis)");
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

    // ===== LESSON METHODS =====

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
    ) -> Result<i64> {
        let cause_json = serde_json::to_string(cause)
            .unwrap_or_else(|_| "{}".to_string());

        let result = sqlx::query(
            r#"
            INSERT INTO lessons (trade_id, coin, outcome, entry_price, exit_price, pnl, hold_duration_secs, cause, lesson_type)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(trade_id)
        .bind(coin)
        .bind(outcome.as_str())
        .bind(entry_price)
        .bind(exit_price)
        .bind(pnl)
        .bind(hold_duration_secs)
        .bind(&cause_json)
        .bind(lesson_type.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Order(format!("Failed to save lesson: {}", e)))?;

        let id = result.last_insert_rowid();
        info!("Saved lesson #{}: {} {} pnl={:.2}", id, outcome.as_str(), coin, pnl);
        Ok(id)
    }

    async fn get_lessons(&self, limit: i64) -> Result<Vec<Lesson>> {
        let rows = sqlx::query_as::<_, LessonRow>(
            "SELECT id, trade_id, coin, outcome, entry_price, exit_price, pnl, hold_duration_secs, cause, lesson_type, rule_generated, created_at FROM lessons ORDER BY id DESC LIMIT ?"
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to fetch lessons: {}", e)))?;

        Ok(rows.into_iter().filter_map(|r| r.into_lesson().ok()).collect())
    }

    async fn get_lessons_by_outcome(&self, outcome: Outcome, limit: i64) -> Result<Vec<Lesson>> {
        let rows = sqlx::query_as::<_, LessonRow>(
            "SELECT id, trade_id, coin, outcome, entry_price, exit_price, pnl, hold_duration_secs, cause, lesson_type, rule_generated, created_at FROM lessons WHERE outcome = ? ORDER BY id DESC LIMIT ?"
        )
        .bind(outcome.as_str())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to fetch lessons: {}", e)))?;

        Ok(rows.into_iter().filter_map(|r| r.into_lesson().ok()).collect())
    }

    async fn save_trade_analysis(&self, analysis: &TradeAnalysis) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO trade_analysis (trade_id, coin, side, entry_price, exit_price, pnl, hold_duration, signal_source, signal_confidence, market_regime, session, slippage_pct, cause)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(analysis.trade_id)
        .bind(&analysis.coin)
        .bind(&analysis.side)
        .bind(analysis.entry_price)
        .bind(analysis.exit_price)
        .bind(analysis.pnl)
        .bind(analysis.hold_duration)
        .bind(&analysis.signal_source)
        .bind(analysis.signal_confidence)
        .bind(&analysis.market_regime)
        .bind(&analysis.session)
        .bind(analysis.slippage_pct)
        .bind(&analysis.cause)
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Order(format!("Failed to save trade analysis: {}", e)))?;

        Ok(())
    }

    async fn get_lesson_stats(&self) -> Result<LessonStats> {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = 'filled'")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let wins: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = 'filled' AND pnl > 0.01")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let losses: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = 'filled' AND pnl < -0.01")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let avg_win: (f64,) = sqlx::query_as("SELECT COALESCE(AVG(pnl), 0.0) FROM trades WHERE status = 'filled' AND pnl > 0.01")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let avg_loss: (f64,) = sqlx::query_as("SELECT COALESCE(AVG(pnl), 0.0) FROM trades WHERE status = 'filled' AND pnl < -0.01")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let best_coin: (String,) = sqlx::query_as("SELECT COALESCE(coin, '') FROM trades WHERE status = 'filled' GROUP BY coin ORDER BY SUM(pnl) DESC LIMIT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let worst_coin: (String,) = sqlx::query_as("SELECT COALESCE(coin, '') FROM trades WHERE status = 'filled' GROUP BY coin ORDER BY SUM(pnl) ASC LIMIT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let lesson_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM lessons")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let rule_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM strategy_rules WHERE active = 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let win_rate = if total.0 > 0 {
            wins.0 as f64 / total.0 as f64 * 100.0
        } else {
            0.0
        };

        Ok(LessonStats {
            total_trades: total.0,
            wins: wins.0,
            losses: losses.0,
            win_rate,
            avg_win_pnl: avg_win.0,
            avg_loss_pnl: avg_loss.0,
            best_coin: best_coin.0,
            worst_coin: worst_coin.0,
            total_lessons: lesson_count.0,
            active_rules: rule_count.0,
        })
    }

    // ===== STRATEGY RULE METHODS =====

    async fn save_rule(&self, rule: &StrategyRule) -> Result<i64> {
        let conditions_json = serde_json::to_string(&rule.conditions)
            .unwrap_or_else(|_| "{}".to_string());
        let action_json = serde_json::to_string(&rule.action)
            .unwrap_or_else(|_| "{}".to_string());

        let result = sqlx::query(
            r#"
            INSERT INTO strategy_rules (name, rule_type, conditions, action, priority, active, source)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&rule.name)
        .bind(serde_json::to_string(&rule.rule_type).unwrap_or_default())
        .bind(&conditions_json)
        .bind(&action_json)
        .bind(rule.priority)
        .bind(rule.active as i32)
        .bind(&rule.source)
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Order(format!("Failed to save rule: {}", e)))?;

        let id = result.last_insert_rowid();
        debug!("Saved rule #{}: {}", id, rule.name);
        Ok(id)
    }

    async fn get_active_rules(&self) -> Result<Vec<StrategyRule>> {
        let rows = sqlx::query_as::<_, RuleRow>(
            "SELECT id, name, rule_type, conditions, action, priority, active, source, hit_count FROM strategy_rules WHERE active = 1 ORDER BY priority DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to fetch rules: {}", e)))?;

        Ok(rows.into_iter().filter_map(|r| r.into_rule().ok()).collect())
    }

    async fn increment_rule_hits(&self, rule_id: i64) -> Result<()> {
        sqlx::query("UPDATE strategy_rules SET hit_count = hit_count + 1 WHERE id = ?")
            .bind(rule_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Order(format!("Failed to increment hits: {}", e)))?;
        Ok(())
    }

    async fn deactivate_rule(&self, rule_id: i64) -> Result<()> {
        sqlx::query("UPDATE strategy_rules SET active = 0 WHERE id = ?")
            .bind(rule_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Order(format!("Failed to deactivate rule: {}", e)))?;
        Ok(())
    }

    async fn get_rule_count_by_source(&self, source: &str) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM strategy_rules WHERE source = ? AND active = 1")
            .bind(source)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to count rules: {}", e)))?;
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

/// Lesson row mapping
#[derive(sqlx::FromRow)]
struct LessonRow {
    id: i64,
    trade_id: i64,
    coin: String,
    outcome: String,
    entry_price: f64,
    exit_price: f64,
    pnl: f64,
    hold_duration_secs: i64,
    cause: String,
    lesson_type: String,
    rule_generated: i32,
    created_at: String,
}

impl LessonRow {
    fn into_lesson(self) -> Result<Lesson> {
        let outcome = match self.outcome.as_str() {
            "win" => Outcome::Win,
            "loss" => Outcome::Loss,
            _ => Outcome::Breakeven,
        };

        let lesson_type = match self.lesson_type.as_str() {
            "pattern" => LessonType::Pattern,
            "anti_pattern" => LessonType::AntiPattern,
            _ => LessonType::Observation,
        };

        let cause: CauseInfo = serde_json::from_str(&self.cause)
            .unwrap_or(CauseInfo {
                loss_cause: None,
                win_cause: None,
                details: self.cause.clone(),
            });

        Ok(Lesson {
            id: self.id,
            trade_id: self.trade_id,
            coin: self.coin,
            outcome,
            entry_price: self.entry_price,
            exit_price: self.exit_price,
            pnl: self.pnl,
            hold_duration_secs: self.hold_duration_secs,
            cause,
            lesson_type,
            rule_generated: self.rule_generated != 0,
            created_at: self.created_at,
        })
    }
}

/// Rule row mapping
#[derive(sqlx::FromRow)]
struct RuleRow {
    id: i64,
    name: String,
    rule_type: String,
    conditions: String,
    action: String,
    priority: i32,
    active: i32,
    source: String,
    hit_count: i64,
}

impl RuleRow {
    fn into_rule(self) -> Result<StrategyRule> {
        let rule_type: crate::domain::strategy::RuleType = serde_json::from_str(&self.rule_type)
            .unwrap_or(crate::domain::strategy::RuleType::LessonRule);

        let conditions: serde_json::Value = serde_json::from_str(&self.conditions)
            .unwrap_or(serde_json::Value::Null);

        let action: serde_json::Value = serde_json::from_str(&self.action)
            .unwrap_or(serde_json::Value::Null);

        Ok(StrategyRule {
            id: self.id,
            name: self.name,
            rule_type,
            conditions,
            action,
            priority: self.priority,
            active: self.active != 0,
            source: self.source,
            hit_count: self.hit_count,
        })
    }
}
