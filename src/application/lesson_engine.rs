use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::application::config::RiskConfig;
use crate::domain::{
    CauseInfo, Lesson, LessonStats, LessonType, LossCause, MarketData, Outcome,
    Signal, StrategyRule, TradingSession, WinCause, TradeAnalysis, RuleType,
};
use crate::domain::order::{OrderResult, OrderStatus, Side};
use crate::domain::store::TradeStore;
use crate::error::Result;

/// Lesson Engine — analyzes trades and generates rules for Strategy Engine
pub struct LessonEngine {
    store: Arc<dyn TradeStore>,
    risk: RiskConfig,
}

impl LessonEngine {
    pub fn new(store: Arc<dyn TradeStore>, risk: RiskConfig) -> Self {
        Self { store, risk }
    }

    /// Analyze a trade outcome and generate lesson if applicable
    pub async fn analyze_trade(
        &self,
        result: &OrderResult,
        signal: &Signal,
        market: &MarketData,
    ) -> Result<Option<Lesson>> {
        // Only analyze filled trades
        let (filled_sz, avg_px, _oid) = match &result.status {
            OrderStatus::Filled { total_sz, avg_px, oid } => (*total_sz, *avg_px, *oid),
            _ => return Ok(None),
        };

        // Calculate PnL
        let entry_price = result.order.price;
        let exit_price = avg_px;
        let pnl = match result.order.side {
            Side::Buy => (exit_price - entry_price) * filled_sz,
            Side::Sell => (entry_price - exit_price) * filled_sz,
        };

        let outcome = Outcome::from_pnl(pnl);

        // Skip breakeven trades
        if outcome == Outcome::Breakeven {
            return Ok(None);
        }

        // Analyze the cause
        let session = TradingSession::current();
        let cause = self.analyze_cause(
            &outcome,
            entry_price,
            exit_price,
            signal,
            market,
            session.as_str(),
        );

        // Determine lesson type
        let lesson_type = match outcome {
            Outcome::Win => LessonType::Pattern,
            Outcome::Loss => LessonType::AntiPattern,
            Outcome::Breakeven => LessonType::Observation,
        };

        // Save lesson
        let hold_duration = 0; // Will be calculated when position closes
        let lesson_id = self.store.save_lesson(
            0, // trade_id - will be linked later
            &result.order.coin,
            outcome.clone(),
            entry_price,
            exit_price,
            pnl,
            hold_duration,
            &cause,
            lesson_type.clone(),
        ).await?;

        // Save trade analysis
        let analysis = TradeAnalysis {
            trade_id: lesson_id,
            coin: result.order.coin.clone(),
            side: format!("{:?}", result.order.side),
            entry_price,
            exit_price,
            pnl,
            hold_duration,
            signal_source: format!("{:?}", signal.source),
            signal_confidence: signal.confidence,
            market_regime: "unknown".to_string(),
            session: session.as_str().to_string(),
            slippage_pct: ((avg_px - entry_price).abs() / entry_price * 100.0),
            cause: cause.details.clone(),
        };
        let _ = self.store.save_trade_analysis(&analysis).await;

        // Generate rule if applicable
        let rule_generated = self.maybe_generate_rule(&outcome, &cause, &result.order.coin, signal).await?;

        let lesson = Lesson {
            id: lesson_id,
            trade_id: 0,
            coin: result.order.coin.clone(),
            outcome: outcome.clone(),
            entry_price,
            exit_price,
            pnl,
            hold_duration_secs: hold_duration,
            cause,
            lesson_type,
            rule_generated,
            created_at: String::new(),
        };

        info!(
            "📚 Lesson #{}: {} {} pnl={:.2} {}",
            lesson_id,
            outcome.as_str(),
            result.order.coin,
            pnl,
            if rule_generated { "→ rule generated" } else { "" }
        );

        Ok(Some(lesson))
    }

    /// Analyze what caused the trade outcome
    fn analyze_cause(
        &self,
        outcome: &Outcome,
        entry_price: f64,
        exit_price: f64,
        signal: &Signal,
        market: &MarketData,
        session: &str,
    ) -> CauseInfo {
        let price_change_pct = (exit_price - entry_price).abs() / entry_price * 100.0;

        match outcome {
            Outcome::Loss => {
                // Determine loss cause
                let cause = if price_change_pct < self.risk.stop_loss_pct * 0.5 {
                    // Price barely moved but hit SL — likely a wick
                    LossCause::TightStopLoss
                } else if price_change_pct > self.risk.stop_loss_pct * 3.0 {
                    // Big move against us — market reversal
                    LossCause::MarketReversal
                } else if signal.confidence < 0.6 {
                    // Low confidence signal
                    LossCause::BadSignalQuality
                } else if session == "asia" {
                    // Low liquidity session
                    LossCause::WrongSession
                } else {
                    // Check market volatility
                    let prices: Vec<f64> = market.mids.values().copied().collect();
                    if prices.len() > 1 {
                        let avg = prices.iter().sum::<f64>() / prices.len() as f64;
                        let var = prices.iter().map(|p| (p - avg).powi(2)).sum::<f64>() / prices.len() as f64;
                        let vol = (var.sqrt() / avg * 100.0).min(10.0);
                        if vol > 3.0 {
                            LossCause::HighVolatility
                        } else {
                            LossCause::Unknown
                        }
                    } else {
                        LossCause::Unknown
                    }
                };

                CauseInfo::loss(
                    cause.clone(),
                    format!(
                        "{}: price_change={:.2}%, confidence={:.2}, session={}",
                        cause.description(),
                        price_change_pct,
                        signal.confidence,
                        session
                    ),
                )
            }
            Outcome::Win => {
                let cause = if signal.confidence >= 0.8 {
                    WinCause::GoodSignalQuality
                } else if price_change_pct > self.risk.take_profit_pct * 0.8 {
                    WinCause::StrongTrend
                } else if session == "us" || session == "europe" {
                    WinCause::RightSession
                } else {
                    WinCause::Unknown
                };

                CauseInfo::win(
                    cause.clone(),
                    format!(
                        "win: price_change={:.2}%, confidence={:.2}, session={}",
                        price_change_pct,
                        signal.confidence,
                        session
                    ),
                )
            }
            _ => CauseInfo::win(WinCause::Unknown, "breakeven".to_string()),
        }
    }

    /// Generate a rule from a lesson if pattern is clear
    async fn maybe_generate_rule(
        &self,
        outcome: &Outcome,
        cause: &CauseInfo,
        coin: &str,
        signal: &Signal,
    ) -> Result<bool> {
        match outcome {
            Outcome::Loss => {
                // Generate avoidance rule based on loss cause
                if let Some(loss_cause) = &cause.loss_cause {
                    let rule = match loss_cause {
                        LossCause::TightStopLoss => {
                            Some(StrategyRule {
                                id: 0,
                                name: format!("lesson_{}_widen_sl", coin),
                                rule_type: RuleType::LessonRule,
                                conditions: serde_json::json!({"coin": coin}),
                                action: serde_json::json!({"sl_multiplier": 1.3}),
                                priority: 7,
                                active: true,
                                source: "lesson".to_string(),
                                hit_count: 0,
                            })
                        }
                        LossCause::BadSignalQuality => {
                            Some(StrategyRule {
                                id: 0,
                                name: format!("lesson_{}_low_confidence", coin),
                                rule_type: RuleType::LessonRule,
                                conditions: serde_json::json!({
                                    "coin": coin,
                                    "signal_source": format!("{:?}", signal.source).to_lowercase()
                                }),
                                action: serde_json::json!({"size_multiplier": 0.3}),
                                priority: 6,
                                active: true,
                                source: "lesson".to_string(),
                                hit_count: 0,
                            })
                        }
                        LossCause::HighVolatility => {
                            Some(StrategyRule {
                                id: 0,
                                name: format!("lesson_{}_reduce_vol", coin),
                                rule_type: RuleType::LessonRule,
                                conditions: serde_json::json!({"coin": coin}),
                                action: serde_json::json!({
                                    "size_multiplier": 0.5,
                                    "sl_multiplier": 0.8,
                                    "tp_multiplier": 1.5
                                }),
                                priority: 8,
                                active: true,
                                source: "lesson".to_string(),
                                hit_count: 0,
                            })
                        }
                        LossCause::WrongSession => {
                            Some(StrategyRule {
                                id: 0,
                                name: format!("lesson_{}_bad_session", coin),
                                rule_type: RuleType::LessonRule,
                                conditions: serde_json::json!({"coin": coin}),
                                action: serde_json::json!({"size_multiplier": 0.5}),
                                priority: 4,
                                active: true,
                                source: "lesson".to_string(),
                                hit_count: 0,
                            })
                        }
                        _ => None,
                    };

                    if let Some(rule) = rule {
                        let _ = self.store.save_rule(&rule).await;
                        return Ok(true);
                    }
                }
            }
            Outcome::Win => {
                // Generate reinforcement rule for winning patterns
                if let Some(win_cause) = &cause.win_cause {
                    match win_cause {
                        WinCause::GoodSignalQuality => {
                            // Reinforce: increase size for this source
                            let rule = StrategyRule {
                                id: 0,
                                name: format!("lesson_{}_good_source", coin),
                                rule_type: RuleType::LessonRule,
                                conditions: serde_json::json!({
                                    "coin": coin,
                                    "signal_source": format!("{:?}", signal.source).to_lowercase()
                                }),
                                action: serde_json::json!({"size_multiplier": 1.3}),
                                priority: 6,
                                active: true,
                                source: "lesson".to_string(),
                                hit_count: 0,
                            };
                            let _ = self.store.save_rule(&rule).await;
                            return Ok(true);
                        }
                        WinCause::StrongTrend => {
                            // Reinforce: let winners run with wider TP
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
                            let _ = self.store.save_rule(&rule).await;
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

    /// Analyze patterns across multiple trades (periodic)
    pub async fn analyze_patterns(&self) -> Result<Vec<Lesson>> {
        let recent_losses = self.store.get_lessons_by_outcome(Outcome::Loss, 20).await?;
        let recent_wins = self.store.get_lessons_by_outcome(Outcome::Win, 20).await?;

        let mut lessons = Vec::new();

        // Check for repeated loss patterns on same coin
        let mut coin_losses: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
        for lesson in &recent_losses {
            *coin_losses.entry(lesson.coin.clone()).or_insert(0) += 1;
        }

        for (coin, count) in &coin_losses {
            if *count >= 3 {
                // Coin has multiple losses — generate blacklist rule
                let existing_rules = self.store.get_rule_count_by_source("lesson").await?;
                if existing_rules < 50 { // Cap lesson rules
                    let rule = StrategyRule {
                        id: 0,
                        name: format!("pattern_{}_frequent_loss", coin),
                        rule_type: RuleType::CoinFilter,
                        conditions: serde_json::json!({}),
                        action: serde_json::json!({
                            "mode": "blacklist",
                            "coins": [coin]
                        }),
                        priority: 9,
                        active: true,
                        source: "lesson".to_string(),
                        hit_count: 0,
                    };
                    let _ = self.store.save_rule(&rule).await;
                    info!(
                        "📚 Pattern: {} has {} recent losses → blacklisted",
                        coin, count
                    );
                }
            }
        }

        // Check for repeated loss patterns from same source
        let mut source_losses: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
        for lesson in &recent_losses {
            let source = lesson.cause.details.clone();
            *source_losses.entry(source).or_insert(0) += 1;
        }

        Ok(lessons)
    }

    /// Get performance stats
    pub async fn get_stats(&self) -> Result<LessonStats> {
        self.store.get_lesson_stats().await
    }

    /// Get stats summary string
    pub async fn stats_summary(&self) -> String {
        match self.get_stats().await {
            Ok(stats) => {
                format!(
                    "Trades: {} | Win: {:.1}% | Rules: {} | Lessons: {}",
                    stats.total_trades,
                    stats.win_rate,
                    stats.active_rules,
                    stats.total_lessons
                )
            }
            Err(_) => "Stats unavailable".to_string(),
        }
    }
}
