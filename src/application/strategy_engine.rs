use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::application::config::{RiskConfig, StrategyConfig};
use crate::domain::{
    MarketData, MarketRegime, PositionTracker, Signal, StrategyParams, StrategyRule,
    TradingSession, detect_regime, RuleType,
};
use crate::domain::store::TradeStore;
use crate::error::Result;

/// Strategy Engine — applies dynamic rules to adjust trading parameters
pub struct StrategyEngine {
    store: Arc<dyn TradeStore>,
    rules: RwLock<Vec<StrategyRule>>,
    base_config: StrategyConfig,
    base_risk: RiskConfig,
}

impl StrategyEngine {
    pub fn new(
        store: Arc<dyn TradeStore>,
        config: StrategyConfig,
        risk: RiskConfig,
    ) -> Self {
        Self {
            store,
            rules: RwLock::new(Vec::new()),
            base_config: config,
            base_risk: risk,
        }
    }

    /// Load rules from DB and seed defaults if empty
    pub async fn init(&self) -> Result<()> {
        let rules = self.store.get_active_rules().await?;

        if rules.is_empty() {
            info!("No rules found, seeding default strategy rules...");
            self.seed_default_rules().await?;
        } else {
            info!("Loaded {} active strategy rules", rules.len());
        }

        let rules = self.store.get_active_rules().await?;
        let mut w = self.rules.write().await;
        *w = rules;
        Ok(())
    }

    /// Reload rules (called after lesson generates new rule)
    pub async fn reload_rules(&self) -> Result<()> {
        let rules = self.store.get_active_rules().await?;
        let mut w = self.rules.write().await;
        *w = rules;
        info!("Reloaded {} strategy rules", w.len());
        Ok(())
    }

    /// Core evaluation: signal + market data → adjusted params
    pub async fn evaluate(
        &self,
        signal: &Signal,
        market: &MarketData,
        positions: &PositionTracker,
    ) -> StrategyParams {
        let mut params = StrategyParams::from_config(
            self.base_config.max_leverage,
            self.base_risk.stop_loss_pct,
            self.base_risk.take_profit_pct,
        );

        let rules = self.rules.read().await;

        for rule in rules.iter() {
            if !rule.active {
                continue;
            }

            let applied = match rule.rule_type {
                RuleType::VolatilitySlTp => self.apply_volatility_rule(rule, market, &mut params),
                RuleType::PositionSizing => self.apply_sizing_rule(rule, signal, positions, &mut params),
                RuleType::ConfidenceFilter => self.apply_confidence_rule(rule, signal, &mut params),
                RuleType::CoinFilter => self.apply_coin_rule(rule, signal, &mut params),
                RuleType::LeverageScale => self.apply_leverage_rule(rule, market, &mut params),
                RuleType::SessionFilter => self.apply_session_rule(rule, &mut params),
                RuleType::LessonRule => self.apply_lesson_rule(rule, signal, &mut params),
            };

            if applied {
                let _ = self.store.increment_rule_hits(rule.id).await;
                debug!("Rule '{}' applied (hits: {})", rule.name, rule.hit_count + 1);
            }
        }

        params
    }

    fn apply_volatility_rule(
        &self,
        rule: &StrategyRule,
        market: &MarketData,
        params: &mut StrategyParams,
    ) -> bool {
        // Get recent price data for ATR calculation
        // Since we only have mid prices (not OHLCV), use a simplified approach
        let atr_threshold = rule.conditions.get("atr_pct")
            .and_then(|v| v.as_str())
            .and_then(|s| s.trim_matches(|c| c == '>' || c == '<' || c == '=' || c == ' ')
                .parse::<f64>().ok())
            .unwrap_or(2.0);

        // Use market data to estimate volatility
        // For now, use a simple heuristic based on price spread
        let prices: Vec<f64> = market.mids.values().copied().collect();
        if prices.is_empty() {
            return false;
        }

        let avg_price = prices.iter().sum::<f64>() / prices.len() as f64;
        let variance = prices.iter().map(|p| (p - avg_price).powi(2)).sum::<f64>() / prices.len() as f64;
        let volatility_pct = (variance.sqrt() / avg_price * 100.0).min(10.0);

        if volatility_pct > atr_threshold {
            if let Some(sl_mult) = rule.action.get("sl_multiplier").and_then(|v| v.as_f64()) {
                params.stop_loss_pct *= sl_mult;
                params.add_reason(format!("volatility {:.1}% → SL adjusted", volatility_pct));
            }
            if let Some(tp_mult) = rule.action.get("tp_multiplier").and_then(|v| v.as_f64()) {
                params.take_profit_pct *= tp_mult;
            }
            true
        } else {
            false
        }
    }

    fn apply_sizing_rule(
        &self,
        rule: &StrategyRule,
        signal: &Signal,
        positions: &PositionTracker,
        params: &mut StrategyParams,
    ) -> bool {
        let method = rule.action.get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("fixed");

        match method {
            "kelly" => {
                // Simplified Kelly Criterion: f = (bp - q) / b
                // b = avg_win/avg_loss ratio, p = win_rate, q = 1 - p
                // Use signal confidence as proxy for p
                let p = signal.confidence;
                let q = 1.0 - p;
                let b = 2.0; // Assume 2:1 reward/risk ratio

                let kelly = (b * p - q) / b;
                let kelly = kelly.clamp(0.1, 1.5); // Cap between 10% and 150%

                params.size_multiplier *= kelly;
                params.add_reason(format!("kelly {:.0}%", kelly * 100.0));
                true
            }
            "confidence_weighted" => {
                let multiplier = (signal.confidence * 1.5).clamp(0.3, 1.5);
                params.size_multiplier *= multiplier;
                params.add_reason(format!("confidence-weighted {:.0}%", multiplier * 100.0));
                true
            }
            _ => false,
        }
    }

    fn apply_confidence_rule(
        &self,
        rule: &StrategyRule,
        signal: &Signal,
        params: &mut StrategyParams,
    ) -> bool {
        let threshold = rule.conditions.get("confidence")
            .and_then(|v| v.as_str())
            .and_then(|s| s.trim_matches(|c| c == '<' || c == '>' || c == '=' || c == ' ')
                .parse::<f64>().ok())
            .unwrap_or(0.6);

        let direction = rule.conditions.get("confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if direction.starts_with('<') && signal.confidence < threshold {
            if let Some(mult) = rule.action.get("size_multiplier").and_then(|v| v.as_f64()) {
                params.size_multiplier *= mult;
                params.add_reason(format!(
                    "low confidence {:.2} → size {:.0}%",
                    signal.confidence, mult * 100.0
                ));
            }
            true
        } else {
            false
        }
    }

    fn apply_coin_rule(
        &self,
        rule: &StrategyRule,
        signal: &Signal,
        params: &mut StrategyParams,
    ) -> bool {
        let mode = rule.action.get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("blacklist");

        let coins: Vec<String> = rule.action.get("coins")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        match mode {
            "blacklist" => {
                if coins.contains(&signal.coin) {
                    params.should_execute = false;
                    params.add_reason(format!("coin {} is blacklisted", signal.coin));
                    true
                } else {
                    false
                }
            }
            "whitelist" => {
                if !coins.is_empty() && !coins.contains(&signal.coin) {
                    params.should_execute = false;
                    params.add_reason(format!("coin {} not in whitelist", signal.coin));
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn apply_leverage_rule(
        &self,
        rule: &StrategyRule,
        market: &MarketData,
        params: &mut StrategyParams,
    ) -> bool {
        let max_vol = rule.conditions.get("atr_pct")
            .and_then(|v| v.as_str())
            .and_then(|s| s.trim_matches(|c| c == '>' || c == '<' || c == '=' || c == ' ')
                .parse::<f64>().ok())
            .unwrap_or(3.0);

        let prices: Vec<f64> = market.mids.values().copied().collect();
        if prices.is_empty() {
            return false;
        }

        let avg_price = prices.iter().sum::<f64>() / prices.len() as f64;
        let variance = prices.iter().map(|p| (p - avg_price).powi(2)).sum::<f64>() / prices.len() as f64;
        let volatility_pct = (variance.sqrt() / avg_price * 100.0).min(10.0);

        if volatility_pct > max_vol {
            if let Some(max_lev) = rule.action.get("max_leverage").and_then(|v| v.as_u64()) {
                params.leverage = params.leverage.min(max_lev as u32);
                params.add_reason(format!(
                    "volatile market {:.1}% → leverage capped at {}x",
                    volatility_pct, max_lev
                ));
            }
            true
        } else {
            false
        }
    }

    fn apply_session_rule(
        &self,
        rule: &StrategyRule,
        params: &mut StrategyParams,
    ) -> bool {
        let restricted_session = rule.conditions.get("session")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let current = TradingSession::current();

        if current.as_str() == restricted_session {
            if let Some(mult) = rule.action.get("size_multiplier").and_then(|v| v.as_f64()) {
                params.size_multiplier *= mult;
                params.add_reason(format!(
                    "{} session → size {:.0}%",
                    current.as_str(), mult * 100.0
                ));
            }
            true
        } else {
            false
        }
    }

    fn apply_lesson_rule(
        &self,
        rule: &StrategyRule,
        signal: &Signal,
        params: &mut StrategyParams,
    ) -> bool {
        // Check if rule applies to this coin
        if let Some(target_coin) = rule.conditions.get("coin").and_then(|v| v.as_str()) {
            if target_coin != signal.coin {
                return false;
            }
        }

        // Check if rule applies to this signal source
        if let Some(target_source) = rule.conditions.get("signal_source").and_then(|v| v.as_str()) {
            let source_str = format!("{:?}", signal.source).to_lowercase();
            if source_str != target_source {
                return false;
            }
        }

        // Apply action
        if let Some(skip) = rule.action.get("skip").and_then(|v| v.as_bool()) {
            if skip {
                params.should_execute = false;
                params.add_reason(format!("lesson rule '{}' says skip", rule.name));
                return true;
            }
        }

        if let Some(mult) = rule.action.get("size_multiplier").and_then(|v| v.as_f64()) {
            params.size_multiplier *= mult;
            params.add_reason(format!("lesson rule '{}' → size {:.0}%", rule.name, mult * 100.0));
        }

        if let Some(sl_mult) = rule.action.get("sl_multiplier").and_then(|v| v.as_f64()) {
            params.stop_loss_pct *= sl_mult;
        }

        if let Some(tp_mult) = rule.action.get("tp_multiplier").and_then(|v| v.as_f64()) {
            params.take_profit_pct *= tp_mult;
        }

        true
    }

    /// Seed default strategy rules
    async fn seed_default_rules(&self) -> Result<()> {
        let defaults = vec![
            StrategyRule {
                id: 0,
                name: "high_vol_tighten_sl".to_string(),
                rule_type: RuleType::VolatilitySlTp,
                conditions: serde_json::json!({"atr_pct": ">2"}),
                action: serde_json::json!({"sl_multiplier": 0.7, "tp_multiplier": 1.5}),
                priority: 10,
                active: true,
                source: "default".to_string(),
                hit_count: 0,
            },
            StrategyRule {
                id: 0,
                name: "low_confidence_reduce".to_string(),
                rule_type: RuleType::ConfidenceFilter,
                conditions: serde_json::json!({"confidence": "<0.6"}),
                action: serde_json::json!({"size_multiplier": 0.5}),
                priority: 8,
                active: true,
                source: "default".to_string(),
                hit_count: 0,
            },
            StrategyRule {
                id: 0,
                name: "kelly_sizing".to_string(),
                rule_type: RuleType::PositionSizing,
                conditions: serde_json::json!({"always": true}),
                action: serde_json::json!({"method": "kelly"}),
                priority: 5,
                active: true,
                source: "default".to_string(),
                hit_count: 0,
            },
            StrategyRule {
                id: 0,
                name: "asia_session_reduce".to_string(),
                rule_type: RuleType::SessionFilter,
                conditions: serde_json::json!({"session": "asia"}),
                action: serde_json::json!({"size_multiplier": 0.7}),
                priority: 3,
                active: true,
                source: "default".to_string(),
                hit_count: 0,
            },
            StrategyRule {
                id: 0,
                name: "volatile_reduce_leverage".to_string(),
                rule_type: RuleType::LeverageScale,
                conditions: serde_json::json!({"atr_pct": ">3"}),
                action: serde_json::json!({"max_leverage": 10}),
                priority: 9,
                active: true,
                source: "default".to_string(),
                hit_count: 0,
            },
        ];

        for rule in defaults {
            self.store.save_rule(&rule).await?;
        }

        info!("Seeded {} default strategy rules", 5);
        Ok(())
    }

    /// Get stats about rules
    pub async fn rule_stats(&self) -> String {
        let rules = self.rules.read().await;
        let active = rules.iter().filter(|r| r.active).count();
        let lesson_rules = rules.iter().filter(|r| r.source == "lesson").count();
        format!(
            "Rules: {} active ({} from lessons, {} default)",
            active,
            lesson_rules,
            active - lesson_rules
        )
    }
}
