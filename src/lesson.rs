//! Lesson Engine adapter — analyzes trade outcomes and generates learning rules.
//! Simplified version that works with the active bot's trade data format.

use std::sync::Arc;
use log::{debug, info, warn};

use crate::domain::{
    CauseInfo, Lesson, LessonStats, LessonType, LossCause, Outcome,
    StrategyRule, WinCause, TradeAnalysis, RuleType,
};
use crate::domain::store::TradeStore;
use crate::error::Result;

/// Lesson Engine — analyzes trades and generates rules for strategy improvement
pub struct LessonEngine {
    store: Arc<dyn TradeStore>,
    /// ATR SL multiplier from config (for cause analysis)
    atr_sl_mult: f64,
    /// TP multiplier from config
    tp_mult: f64,
}

/// Simplified trade result for lesson analysis
#[derive(Debug, Clone)]
pub struct TradeResult {
    pub coin: String,
    pub direction: String,  // "Long" or "Short"
    pub entry_price: f64,
    pub exit_price: f64,
    pub size: f64,
    pub pnl: f64,
    pub signal_score: u32,
    pub atr_pct: f64,
    pub in_session: bool,
}

impl LessonEngine {
    pub fn new(store: Arc<dyn TradeStore>, atr_sl_mult: f64, tp_mult: f64) -> Self {
        Self { store, atr_sl_mult, tp_mult }
    }

    /// Analyze a trade outcome and generate lesson if applicable
    pub async fn analyze_trade(&self, trade: &TradeResult) -> Result<Option<Lesson>> {
        let outcome = Outcome::from_pnl(trade.pnl);

        // Skip breakeven trades
        if outcome == Outcome::Breakeven {
            return Ok(None);
        }

        // Guard against division by zero
        if trade.entry_price <= 0.0 {
            warn!("Lesson engine: entry_price is 0, skipping analysis");
            return Ok(None);
        }

        let price_change_pct = (trade.exit_price - trade.entry_price).abs() / trade.entry_price * 100.0;

        // Analyze cause
        let cause = match outcome {
            Outcome::Loss => {
                let loss_cause = if price_change_pct < 0.3 {
                    LossCause::TightStopLoss
                } else if price_change_pct > 3.0 {
                    LossCause::MarketReversal
                } else if trade.signal_score < 70 {
                    LossCause::BadSignalQuality
                } else if !trade.in_session {
                    LossCause::WrongSession
                } else if trade.atr_pct > 1.5 {
                    LossCause::HighVolatility
                } else {
                    LossCause::Unknown
                };

                CauseInfo::loss(
                    loss_cause.clone(),
                    format!(
                        "{}: price_change={:.2}%, score={}, session={}",
                        loss_cause.description(),
                        price_change_pct,
                        trade.signal_score,
                        trade.in_session
                    ),
                )
            }
            Outcome::Win => {
                let win_cause = if trade.signal_score >= 90 {
                    WinCause::GoodSignalQuality
                } else if price_change_pct > 1.5 {
                    WinCause::StrongTrend
                } else if trade.in_session {
                    WinCause::RightSession
                } else {
                    WinCause::Unknown
                };

                CauseInfo::win(
                    win_cause.clone(),
                    format!(
                        "win: price_change={:.2}%, score={}, session={}",
                        price_change_pct,
                        trade.signal_score,
                        trade.in_session
                    ),
                )
            }
            _ => CauseInfo::win(WinCause::Unknown, "breakeven".to_string()),
        };

        let lesson_type = match outcome {
            Outcome::Win => LessonType::Pattern,
            Outcome::Loss => LessonType::AntiPattern,
            Outcome::Breakeven => LessonType::Observation,
        };

        // Save lesson to DB
        let lesson_id = self.store.save_lesson(
            0,
            &trade.coin,
            outcome.clone(),
            trade.entry_price,
            trade.exit_price,
            trade.pnl,
            0, // hold duration unknown
            &cause,
            lesson_type.clone(),
        ).await?;

        // Save trade analysis
        let analysis = TradeAnalysis {
            trade_id: lesson_id,
            coin: trade.coin.clone(),
            side: trade.direction.clone(),
            entry_price: trade.entry_price,
            exit_price: trade.exit_price,
            pnl: trade.pnl,
            hold_duration: 0,
            signal_source: "decision_engine".to_string(),
            signal_confidence: trade.signal_score as f64 / 100.0,
            market_regime: "unknown".to_string(),
            session: if trade.in_session { "us" } else { "off_hours" }.to_string(),
            slippage_pct: 0.0,
            cause: cause.details.clone(),
        };
        if let Err(e) = self.store.save_trade_analysis(&analysis).await {
            warn!("Failed to save trade analysis: {}", e);
        }

        // Maybe generate rule
        let rule_generated = self.maybe_generate_rule(&outcome, &cause, &trade.coin).await?;

        let lesson = Lesson {
            id: lesson_id,
            trade_id: 0,
            coin: trade.coin.clone(),
            outcome: outcome.clone(),
            entry_price: trade.entry_price,
            exit_price: trade.exit_price,
            pnl: trade.pnl,
            hold_duration_secs: 0,
            cause,
            lesson_type,
            rule_generated,
            created_at: String::new(),
        };

        info!(
            "Lesson #{}: {} {} pnl={:.2} {}",
            lesson_id,
            outcome.as_str(),
            trade.coin,
            trade.pnl,
            if rule_generated { "→ rule generated" } else { "" }
        );

        Ok(Some(lesson))
    }

    /// Generate a rule from a lesson
    async fn maybe_generate_rule(
        &self,
        outcome: &Outcome,
        cause: &CauseInfo,
        coin: &str,
    ) -> Result<bool> {
        match outcome {
            Outcome::Loss => {
                if let Some(loss_cause) = &cause.loss_cause {
                    let rule = match loss_cause {
                        LossCause::TightStopLoss => Some(StrategyRule {
                            id: 0,
                            name: format!("lesson_{}_widen_sl", coin),
                            rule_type: RuleType::LessonRule,
                            conditions: serde_json::json!({"coin": coin}),
                            action: serde_json::json!({"sl_multiplier": 1.3}),
                            priority: 7,
                            active: true,
                            source: "lesson".to_string(),
                            hit_count: 0,
                        }),
                        LossCause::BadSignalQuality => Some(StrategyRule {
                            id: 0,
                            name: format!("lesson_{}_low_confidence", coin),
                            rule_type: RuleType::LessonRule,
                            conditions: serde_json::json!({"coin": coin}),
                            action: serde_json::json!({"size_multiplier": 0.3}),
                            priority: 6,
                            active: true,
                            source: "lesson".to_string(),
                            hit_count: 0,
                        }),
                        LossCause::HighVolatility => Some(StrategyRule {
                            id: 0,
                            name: format!("lesson_{}_reduce_vol", coin),
                            rule_type: RuleType::LessonRule,
                            conditions: serde_json::json!({"coin": coin}),
                            action: serde_json::json!({"size_multiplier": 0.5, "sl_multiplier": 0.8}),
                            priority: 8,
                            active: true,
                            source: "lesson".to_string(),
                            hit_count: 0,
                        }),
                        LossCause::WrongSession => Some(StrategyRule {
                            id: 0,
                            name: format!("lesson_{}_bad_session", coin),
                            rule_type: RuleType::LessonRule,
                            conditions: serde_json::json!({"coin": coin}),
                            action: serde_json::json!({"size_multiplier": 0.5}),
                            priority: 4,
                            active: true,
                            source: "lesson".to_string(),
                            hit_count: 0,
                        }),
                        _ => None,
                    };

                    if let Some(rule) = rule {
                        // Dedup: check if a rule with this name already exists
                        let existing = self.store.get_active_rules().await.unwrap_or_default();
                        if existing.iter().any(|r| r.name == rule.name) {
                            debug!("Rule '{}' already exists, skipping", rule.name);
                            return Ok(false);
                        }
                        if let Err(e) = self.store.save_rule(&rule).await {
                            warn!("Failed to save rule '{}': {}", rule.name, e);
                            return Ok(false);
                        }
                        return Ok(true);
                    }
                }
            }
            Outcome::Win => {
                if let Some(win_cause) = &cause.win_cause {
                    match win_cause {
                        WinCause::GoodSignalQuality => {
                            let rule = StrategyRule {
                                id: 0,
                                name: format!("lesson_{}_good_source", coin),
                                rule_type: RuleType::LessonRule,
                                conditions: serde_json::json!({"coin": coin}),
                                action: serde_json::json!({"size_multiplier": 1.3}),
                                priority: 6,
                                active: true,
                                source: "lesson".to_string(),
                                hit_count: 0,
                            };
                            let existing = self.store.get_active_rules().await.unwrap_or_default();
                            if existing.iter().any(|r| r.name == rule.name) {
                                return Ok(false);
                            }
                            if let Err(e) = self.store.save_rule(&rule).await {
                                warn!("Failed to save rule: {}", e);
                                return Ok(false);
                            }
                            return Ok(true);
                        }
                        WinCause::StrongTrend => {
                            let rule = StrategyRule {
                                id: 0,
                                name: format!("lesson_{}_trend_win", coin),
                                rule_type: RuleType::LessonRule,
                                conditions: serde_json::json!({"coin": coin}),
                                action: serde_json::json!({"tp_multiplier": 1.5}),
                                priority: 5,
                                active: true,
                                source: "lesson".to_string(),
                                hit_count: 0,
                            };
                            let existing = self.store.get_active_rules().await.unwrap_or_default();
                            if existing.iter().any(|r| r.name == rule.name) {
                                return Ok(false);
                            }
                            if let Err(e) = self.store.save_rule(&rule).await {
                                warn!("Failed to save rule: {}", e);
                                return Ok(false);
                            }
                            return Ok(true);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        Ok(false)
    }

    /// Analyze patterns across multiple trades (call periodically)
    pub async fn analyze_patterns(&self) -> Result<Vec<Lesson>> {
        let recent_losses = self.store.get_lessons_by_outcome(Outcome::Loss, 20).await?;

        let mut coin_losses: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
        for lesson in &recent_losses {
            *coin_losses.entry(lesson.coin.clone()).or_insert(0) += 1;
        }

        let existing_rules = self.store.get_active_rules().await.unwrap_or_default();
        for (coin, count) in &coin_losses {
            if *count >= 3 {
                let rule_name = format!("pattern_{}_frequent_loss", coin);
                // Dedup: skip if rule already exists
                if existing_rules.iter().any(|r| r.name == rule_name) {
                    continue;
                }
                let existing_count = self.store.get_rule_count_by_source("lesson").await?;
                if existing_count < 50 {
                    let rule = StrategyRule {
                        id: 0,
                        name: rule_name,
                        rule_type: RuleType::CoinFilter,
                        conditions: serde_json::json!({}),
                        action: serde_json::json!({"mode": "blacklist", "coins": [coin]}),
                        priority: 9,
                        active: true,
                        source: "lesson".to_string(),
                        hit_count: 0,
                    };
                    if let Err(e) = self.store.save_rule(&rule).await {
                        warn!("Failed to save pattern rule: {}", e);
                    } else {
                        info!("Pattern: {} has {} recent losses → blacklisted", coin, count);
                    }
                }
            }
        }

        Ok(Vec::new())
    }

    /// Get performance stats
    pub async fn get_stats(&self) -> Result<LessonStats> {
        self.store.get_lesson_stats().await
    }

    /// Get stats summary string
    pub async fn stats_summary(&self) -> String {
        match self.get_stats().await {
            Ok(stats) => format!(
                "Trades: {} | Win: {:.1}% | Rules: {} | Lessons: {}",
                stats.total_trades, stats.win_rate, stats.active_rules, stats.total_lessons
            ),
            Err(_) => "Stats unavailable".to_string(),
        }
    }
}
