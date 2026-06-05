//! Trade store adapter — SQLite-backed persistence for trades, lessons, and strategy rules.
//! Uses domain types from `domain::store::TradeStore` trait.

use async_trait::async_trait;
use log::{debug, info, warn};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

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

        // Indexes
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

        // Signals table — every signal for research
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS signals (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                coin TEXT NOT NULL,
                direction TEXT NOT NULL,
                score INTEGER NOT NULL,
                ema9 REAL NOT NULL,
                ema21 REAL NOT NULL,
                rsi REAL NOT NULL,
                atr REAL NOT NULL,
                atr_pct REAL NOT NULL,
                spread_bps REAL NOT NULL,
                imbalance REAL NOT NULL,
                cvd REAL NOT NULL,
                funding_rate REAL NOT NULL,
                in_session INTEGER NOT NULL,
                executed INTEGER NOT NULL DEFAULT 0,
                reject_reason TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Config(format!("SQLite migration failed: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_signals_timestamp ON signals(timestamp)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_signals_coin ON signals(coin)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_signals_executed ON signals(executed)")
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Config(format!("SQLite index failed: {}", e)))?;

        info!("SQLite migrations complete (trades, lessons, strategy_rules, trade_analysis, signals)");
        Ok(())
    }

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
            OrderStatus::Filled { total_sz, avg_px, oid } => ("filled", *oid as i64, *avg_px, *total_sz),
            OrderStatus::Resting { oid } => ("resting", *oid as i64, 0.0, 0.0),
            OrderStatus::Cancelled { oid } => ("cancelled", *oid as i64, 0.0, 0.0),
            OrderStatus::Error { message } => {
                warn!("Logging error trade: {}", message);
                ("error", 0i64, 0.0, 0.0)
            }
            OrderStatus::Pending => ("pending", 0i64, 0.0, 0.0),
        };

        sqlx::query(
            "INSERT INTO trades (timestamp, coin, side, size, price, status, order_id, avg_fill_price, filled_size) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

    async fn update_trade_pnl(&self, coin: &str, pnl: f64) -> Result<()> {
        sqlx::query(
            "UPDATE trades SET pnl = ? WHERE id = (SELECT id FROM trades WHERE coin = ? AND status = 'filled' ORDER BY timestamp DESC LIMIT 1)",
        )
        .bind(pnl)
        .bind(coin)
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Order(format!("Failed to update PnL: {}", e)))?;
        Ok(())
    }

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

    async fn get_daily_pnl(&self) -> Result<f64> {
        let row: (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(pnl), 0.0) FROM trades WHERE date(created_at) = date('now') AND status = 'filled'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to get daily PnL: {}", e)))?;
        Ok(row.0)
    }

    async fn get_total_pnl(&self) -> Result<f64> {
        let row: (f64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(pnl), 0.0) FROM trades WHERE status = 'filled'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to get total PnL: {}", e)))?;
        Ok(row.0)
    }

    async fn get_trade_count(&self) -> Result<i64> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| BotError::Api(format!("Failed to get trade count: {}", e)))?;
        Ok(row.0)
    }

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
        let cause_json = serde_json::to_string(cause).unwrap_or_else(|_| "{}".to_string());

        let result = sqlx::query(
            "INSERT INTO lessons (trade_id, coin, outcome, entry_price, exit_price, pnl, hold_duration_secs, cause, lesson_type) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
            "SELECT id, trade_id, coin, outcome, entry_price, exit_price, pnl, hold_duration_secs, cause, lesson_type, rule_generated, created_at FROM lessons ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to fetch lessons: {}", e)))?;

        Ok(rows.into_iter().filter_map(|r| r.into_lesson().ok()).collect())
    }

    async fn get_lessons_by_outcome(&self, outcome: Outcome, limit: i64) -> Result<Vec<Lesson>> {
        let rows = sqlx::query_as::<_, LessonRow>(
            "SELECT id, trade_id, coin, outcome, entry_price, exit_price, pnl, hold_duration_secs, cause, lesson_type, rule_generated, created_at FROM lessons WHERE outcome = ? ORDER BY id DESC LIMIT ?",
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
            "INSERT INTO trade_analysis (trade_id, coin, side, entry_price, exit_price, pnl, hold_duration, signal_source, signal_confidence, market_regime, session, slippage_pct, cause) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let wins: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = 'filled' AND pnl > 0.01")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let losses: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM trades WHERE status = 'filled' AND pnl < -0.01")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let avg_win: (f64,) = sqlx::query_as("SELECT COALESCE(AVG(pnl), 0.0) FROM trades WHERE status = 'filled' AND pnl > 0.01")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let avg_loss: (f64,) = sqlx::query_as("SELECT COALESCE(AVG(pnl), 0.0) FROM trades WHERE status = 'filled' AND pnl < -0.01")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let best_coin: (String,) = sqlx::query_as("SELECT COALESCE(coin, '') FROM trades WHERE status = 'filled' GROUP BY coin ORDER BY SUM(pnl) DESC LIMIT 1")
            .fetch_one(&self.pool).await
            .unwrap_or(("N/A".to_string(),));

        let worst_coin: (String,) = sqlx::query_as("SELECT COALESCE(coin, '') FROM trades WHERE status = 'filled' GROUP BY coin ORDER BY SUM(pnl) ASC LIMIT 1")
            .fetch_one(&self.pool).await
            .unwrap_or(("N/A".to_string(),));

        let lesson_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM lessons")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let rule_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM strategy_rules WHERE active = 1")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get stats: {}", e)))?;

        let win_rate = if total.0 > 0 { wins.0 as f64 / total.0 as f64 * 100.0 } else { 0.0 };

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

    async fn save_rule(&self, rule: &StrategyRule) -> Result<i64> {
        let conditions_json = serde_json::to_string(&rule.conditions).unwrap_or_else(|_| "{}".to_string());
        let action_json = serde_json::to_string(&rule.action).unwrap_or_else(|_| "{}".to_string());

        let result = sqlx::query(
            "INSERT INTO strategy_rules (name, rule_type, conditions, action, priority, active, source) VALUES (?, ?, ?, ?, ?, ?, ?)",
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
            "SELECT id, name, rule_type, conditions, action, priority, active, source, hit_count FROM strategy_rules WHERE active = 1 ORDER BY priority DESC",
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

/// Signal logging methods (not part of TradeStore trait — for research)
impl SqliteStore {
    /// Log a signal (whether executed or rejected). Returns signal ID.
    pub async fn log_signal(
        &self,
        coin: &str,
        direction: &str,
        score: u32,
        ema9: f64,
        ema21: f64,
        rsi: f64,
        atr: f64,
        atr_pct: f64,
        spread_bps: f64,
        imbalance: f64,
        cvd: f64,
        funding_rate: f64,
        in_session: bool,
        executed: bool,
        reject_reason: Option<&str>,
    ) -> Result<i64> {
        let timestamp = chrono::Utc::now().timestamp_millis();

        let result = sqlx::query(
            "INSERT INTO signals (timestamp, coin, direction, score, ema9, ema21, rsi, atr, atr_pct, spread_bps, imbalance, cvd, funding_rate, in_session, executed, reject_reason) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(timestamp)
        .bind(coin)
        .bind(direction)
        .bind(score as i64)
        .bind(ema9)
        .bind(ema21)
        .bind(rsi)
        .bind(atr)
        .bind(atr_pct)
        .bind(spread_bps)
        .bind(imbalance)
        .bind(cvd)
        .bind(funding_rate)
        .bind(in_session as i32)
        .bind(executed as i32)
        .bind(reject_reason)
        .execute(&self.pool)
        .await
        .map_err(|e| BotError::Order(format!("Failed to log signal: {}", e)))?;

        let id = result.last_insert_rowid();
        debug!("Logged signal #{}: {} {} score={} executed={}", id, coin, direction, score, executed);
        Ok(id)
    }

    /// Mark a signal as executed
    pub async fn mark_signal_executed(&self, signal_id: i64) -> Result<()> {
        sqlx::query("UPDATE signals SET executed = 1 WHERE id = ?")
            .bind(signal_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Order(format!("Failed to mark signal executed: {}", e)))?;
        Ok(())
    }

    /// Mark a signal as rejected with reason
    pub async fn mark_signal_rejected(&self, signal_id: i64, reason: &str) -> Result<()> {
        sqlx::query("UPDATE signals SET reject_reason = ? WHERE id = ?")
            .bind(reason)
            .bind(signal_id)
            .execute(&self.pool)
            .await
            .map_err(|e| BotError::Order(format!("Failed to mark signal rejected: {}", e)))?;
        Ok(())
    }

    /// Get recent signals for research
    pub async fn get_signals(&self, limit: i64) -> Result<Vec<SignalRecord>> {
        let rows = sqlx::query_as::<_, SignalRow>(
            "SELECT id, timestamp, coin, direction, score, ema9, ema21, rsi, atr, atr_pct, spread_bps, imbalance, cvd, funding_rate, in_session, executed, reject_reason, created_at FROM signals ORDER BY timestamp DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| BotError::Api(format!("Failed to fetch signals: {}", e)))?;

        Ok(rows.into_iter().map(SignalRow::into_record).collect())
    }

    /// Get signal stats for research
    pub async fn get_signal_stats(&self) -> Result<SignalStats> {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM signals")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get signal stats: {}", e)))?;

        let executed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM signals WHERE executed = 1")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get signal stats: {}", e)))?;

        let rejected: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM signals WHERE executed = 0")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get signal stats: {}", e)))?;

        let longs: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM signals WHERE direction = 'Long'")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get signal stats: {}", e)))?;

        let shorts: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM signals WHERE direction = 'Short'")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get signal stats: {}", e)))?;

        let avg_score: (f64,) = sqlx::query_as("SELECT COALESCE(AVG(score), 0.0) FROM signals WHERE executed = 1")
            .fetch_one(&self.pool).await
            .map_err(|e| BotError::Api(format!("Failed to get signal stats: {}", e)))?;

        Ok(SignalStats {
            total_signals: total.0,
            executed: executed.0,
            rejected: rejected.0,
            longs: longs.0,
            shorts: shorts.0,
            avg_executed_score: avg_score.0,
        })
    }
}

/// Signal record for research
#[derive(Debug, Clone)]
pub struct SignalRecord {
    pub id: i64,
    pub timestamp: i64,
    pub coin: String,
    pub direction: String,
    pub score: u32,
    pub ema9: f64,
    pub ema21: f64,
    pub rsi: f64,
    pub atr: f64,
    pub atr_pct: f64,
    pub spread_bps: f64,
    pub imbalance: f64,
    pub cvd: f64,
    pub funding_rate: f64,
    pub in_session: bool,
    pub executed: bool,
    pub reject_reason: Option<String>,
    pub created_at: String,
}

/// Signal stats for research
#[derive(Debug)]
pub struct SignalStats {
    pub total_signals: i64,
    pub executed: i64,
    pub rejected: i64,
    pub longs: i64,
    pub shorts: i64,
    pub avg_executed_score: f64,
}

impl std::fmt::Display for SignalStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Signals: {} | Exec: {} ({:.1}%) | Rej: {} | Long: {} | Short: {} | AvgScore: {:.0}",
            self.total_signals,
            self.executed,
            if self.total_signals > 0 { self.executed as f64 / self.total_signals as f64 * 100.0 } else { 0.0 },
            self.rejected,
            self.longs,
            self.shorts,
            self.avg_executed_score,
        )
    }
}

#[derive(sqlx::FromRow)]
struct SignalRow {
    id: i64,
    timestamp: i64,
    coin: String,
    direction: String,
    score: i64,
    ema9: f64,
    ema21: f64,
    rsi: f64,
    atr: f64,
    atr_pct: f64,
    spread_bps: f64,
    imbalance: f64,
    cvd: f64,
    funding_rate: f64,
    in_session: i32,
    executed: i32,
    reject_reason: Option<String>,
    created_at: String,
}

impl SignalRow {
    fn into_record(self) -> SignalRecord {
        SignalRecord {
            id: self.id,
            timestamp: self.timestamp,
            coin: self.coin,
            direction: self.direction,
            score: self.score as u32,
            ema9: self.ema9,
            ema21: self.ema21,
            rsi: self.rsi,
            atr: self.atr,
            atr_pct: self.atr_pct,
            spread_bps: self.spread_bps,
            imbalance: self.imbalance,
            cvd: self.cvd,
            funding_rate: self.funding_rate,
            in_session: self.in_session != 0,
            executed: self.executed != 0,
            reject_reason: self.reject_reason,
            created_at: self.created_at,
        }
    }
}

// ===== Row mappings =====

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
            .unwrap_or(CauseInfo { loss_cause: None, win_cause: None, details: self.cause.clone() });

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
        let rule_type: crate::domain::strategy::RuleType = match serde_json::from_str(&self.rule_type) {
            Ok(rt) => rt,
            Err(e) => {
                warn!("Rule #{}: invalid rule_type '{}': {}, defaulting to LessonRule", self.id, self.rule_type, e);
                crate::domain::strategy::RuleType::LessonRule
            }
        };
        let conditions: serde_json::Value = match serde_json::from_str(&self.conditions) {
            Ok(c) => c,
            Err(e) => {
                warn!("Rule #{}: invalid conditions JSON: {}, defaulting to Null", self.id, e);
                serde_json::Value::Null
            }
        };
        let action: serde_json::Value = match serde_json::from_str(&self.action) {
            Ok(a) => a,
            Err(e) => {
                warn!("Rule #{}: invalid action JSON: {}, defaulting to Null", self.id, e);
                serde_json::Value::Null
            }
        };

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
