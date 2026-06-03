use serde::{Deserialize, Serialize};

/// Trade outcome
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Loss,
    Breakeven,
}

impl Outcome {
    pub fn from_pnl(pnl: f64) -> Self {
        if pnl > 0.01 {
            Self::Win
        } else if pnl < -0.01 {
            Self::Loss
        } else {
            Self::Breakeven
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Win => "win",
            Self::Loss => "loss",
            Self::Breakeven => "breakeven",
        }
    }
}

/// Lesson type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LessonType {
    /// Positive pattern to repeat
    Pattern,
    /// Negative pattern to avoid
    AntiPattern,
    /// General observation
    Observation,
}

impl LessonType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pattern => "pattern",
            Self::AntiPattern => "anti_pattern",
            Self::Observation => "observation",
        }
    }
}

/// Cause of a trade loss
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LossCause {
    /// Entry terlalu jauh dari support/resistance
    BadEntryTiming,
    /// SL terlalu ketat, kena wick
    TightStopLoss,
    /// SL terlalu lebar, loss besar
    WideStopLoss,
    /// Signal source jelek
    BadSignalQuality,
    /// Market balik arah
    MarketReversal,
    /// Volatility tinggi, SL kena
    HighVolatility,
    /// Spread besar
    LowLiquidity,
    /// Trade di jam sepi
    WrongSession,
    /// Leverage terlalu tinggi
    OverLeverage,
    /// Ada news besar
    NewsEvent,
    /// Tidak bisa ditentukan
    Unknown,
}

impl LossCause {
    pub fn as_str(&self) -> &str {
        match self {
            Self::BadEntryTiming => "bad_entry_timing",
            Self::TightStopLoss => "tight_stop_loss",
            Self::WideStopLoss => "wide_stop_loss",
            Self::BadSignalQuality => "bad_signal_quality",
            Self::MarketReversal => "market_reversal",
            Self::HighVolatility => "high_volatility",
            Self::LowLiquidity => "low_liquidity",
            Self::WrongSession => "wrong_session",
            Self::OverLeverage => "over_leverage",
            Self::NewsEvent => "news_event",
            Self::Unknown => "unknown",
        }
    }

    /// Description for logging
    pub fn description(&self) -> &str {
        match self {
            Self::BadEntryTiming => "Entry price too far from support/resistance",
            Self::TightStopLoss => "Stop loss triggered by normal price wick",
            Self::WideStopLoss => "Stop loss too wide, large loss on reversal",
            Self::BadSignalQuality => "Signal from unreliable source",
            Self::MarketReversal => "Market reversed direction after entry",
            Self::HighVolatility => "High volatility triggered stop loss",
            Self::LowLiquidity => "Low liquidity caused bad fill/slippage",
            Self::WrongSession => "Traded during low-activity session",
            Self::OverLeverage => "Leverage too high for market conditions",
            Self::NewsEvent => "Unexpected news caused adverse move",
            Self::Unknown => "Cause could not be determined",
        }
    }
}

/// Cause of a trade win
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WinCause {
    /// Entry di support/resistance yang bagus
    GoodEntryTiming,
    /// Trend kuat, mudah profit
    StrongTrend,
    /// Signal source bagus
    GoodSignalQuality,
    /// Position size tepat
    ProperSizing,
    /// SL/TP ratio bagus
    GoodRiskReward,
    /// Trade di jam aktif
    RightSession,
    /// Tidak bisa ditentukan
    Unknown,
}

impl WinCause {
    pub fn as_str(&self) -> &str {
        match self {
            Self::GoodEntryTiming => "good_entry_timing",
            Self::StrongTrend => "strong_trend",
            Self::GoodSignalQuality => "good_signal_quality",
            Self::ProperSizing => "proper_sizing",
            Self::GoodRiskReward => "good_risk_reward",
            Self::RightSession => "right_session",
            Self::Unknown => "unknown",
        }
    }
}

/// Lesson learned from a trade
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    pub id: i64,
    pub trade_id: i64,
    pub coin: String,
    pub outcome: Outcome,
    pub entry_price: f64,
    pub exit_price: f64,
    pub pnl: f64,
    pub hold_duration_secs: i64,
    pub cause: CauseInfo,
    pub lesson_type: LessonType,
    pub rule_generated: bool,
    pub created_at: String,
}

/// Cause information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CauseInfo {
    pub loss_cause: Option<LossCause>,
    pub win_cause: Option<WinCause>,
    pub details: String,
}

impl CauseInfo {
    pub fn loss(cause: LossCause, details: String) -> Self {
        Self {
            loss_cause: Some(cause),
            win_cause: None,
            details,
        }
    }

    pub fn win(cause: WinCause, details: String) -> Self {
        Self {
            loss_cause: None,
            win_cause: Some(cause),
            details,
        }
    }
}

/// Detailed trade analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeAnalysis {
    pub trade_id: i64,
    pub coin: String,
    pub side: String,
    pub entry_price: f64,
    pub exit_price: f64,
    pub pnl: f64,
    pub hold_duration: i64,
    pub signal_source: String,
    pub signal_confidence: f64,
    pub market_regime: String,
    pub session: String,
    pub slippage_pct: f64,
    pub cause: String,
}

/// Performance statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LessonStats {
    pub total_trades: i64,
    pub wins: i64,
    pub losses: i64,
    pub win_rate: f64,
    pub avg_win_pnl: f64,
    pub avg_loss_pnl: f64,
    pub best_coin: String,
    pub worst_coin: String,
    pub total_lessons: i64,
    pub active_rules: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outcome_from_pnl() {
        assert_eq!(Outcome::from_pnl(10.0), Outcome::Win);
        assert_eq!(Outcome::from_pnl(-10.0), Outcome::Loss);
        assert_eq!(Outcome::from_pnl(0.005), Outcome::Breakeven);
    }

    #[test]
    fn test_outcome_serde() {
        let outcome = Outcome::Win;
        let json = serde_json::to_string(&outcome).unwrap();
        let back: Outcome = serde_json::from_str(&json).unwrap();
        assert_eq!(outcome, back);
    }

    #[test]
    fn test_loss_cause_description() {
        let cause = LossCause::TightStopLoss;
        assert!(!cause.description().is_empty());
        assert_eq!(cause.as_str(), "tight_stop_loss");
    }

    #[test]
    fn test_cause_info_loss() {
        let info = CauseInfo::loss(LossCause::HighVolatility, "ATR was 5%".to_string());
        assert!(info.loss_cause.is_some());
        assert!(info.win_cause.is_none());
    }

    #[test]
    fn test_cause_info_win() {
        let info = CauseInfo::win(WinCause::StrongTrend, "Clear uptrend".to_string());
        assert!(info.win_cause.is_some());
        assert!(info.loss_cause.is_none());
    }
}
