use super::order::Side;

/// Signal source
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalSource {
    Sodex,
    Hyperliquid,
    SignalFlow,
    Custom(String),
}

/// Signal action
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalAction {
    OpenLong,
    OpenShort,
    CloseLong,
    CloseShort,
}

/// Trading signal - domain model
#[derive(Debug, Clone)]
pub struct Signal {
    pub source: SignalSource,
    pub coin: String,
    pub action: SignalAction,
    pub confidence: f64, // 0.0 - 1.0
    pub entry_price: Option<f64>,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    pub timestamp: u64,
}

impl Signal {
    /// Convert signal action to order side
    pub fn to_side(&self) -> Side {
        match self.action {
            SignalAction::OpenLong => Side::Buy,
            SignalAction::OpenShort => Side::Sell,
            SignalAction::CloseLong => Side::Sell,
            SignalAction::CloseShort => Side::Buy,
        }
    }

    /// Is this an open position signal?
    pub fn is_open(&self) -> bool {
        matches!(
            self.action,
            SignalAction::OpenLong | SignalAction::OpenShort
        )
    }

    /// Is this a close position signal?
    pub fn is_close(&self) -> bool {
        matches!(
            self.action,
            SignalAction::CloseLong | SignalAction::CloseShort
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signal(action: SignalAction) -> Signal {
        Signal {
            source: SignalSource::Sodex,
            coin: "BTC".to_string(),
            action,
            confidence: 0.8,
            entry_price: Some(100000.0),
            stop_loss: Some(98000.0),
            take_profit: Some(105000.0),
            timestamp: 1000,
        }
    }

    #[test]
    fn test_open_long_to_side() {
        let signal = make_signal(SignalAction::OpenLong);
        assert_eq!(signal.to_side(), Side::Buy);
        assert!(signal.is_open());
        assert!(!signal.is_close());
    }

    #[test]
    fn test_open_short_to_side() {
        let signal = make_signal(SignalAction::OpenShort);
        assert_eq!(signal.to_side(), Side::Sell);
        assert!(signal.is_open());
        assert!(!signal.is_close());
    }

    #[test]
    fn test_close_long_to_side() {
        let signal = make_signal(SignalAction::CloseLong);
        assert_eq!(signal.to_side(), Side::Sell);
        assert!(!signal.is_open());
        assert!(signal.is_close());
    }

    #[test]
    fn test_close_short_to_side() {
        let signal = make_signal(SignalAction::CloseShort);
        assert_eq!(signal.to_side(), Side::Buy);
        assert!(!signal.is_open());
        assert!(signal.is_close());
    }

    #[test]
    fn test_signal_source_equality() {
        assert_eq!(SignalSource::Sodex, SignalSource::Sodex);
        assert_ne!(SignalSource::Sodex, SignalSource::Hyperliquid);
    }
}
