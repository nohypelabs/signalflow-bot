use std::collections::HashMap;

/// Maximum age for market data before considered stale (60 seconds)
const MAX_DATA_AGE_MS: u64 = 60_000;

/// Mid price update
#[derive(Debug, Clone)]
pub struct MidPrice {
    pub coin: String,
    pub price: f64,
    pub timestamp: u64,
}

/// Order book level
#[derive(Debug, Clone)]
pub struct BookLevel {
    pub price: f64,
    pub size: f64,
}

/// Order book snapshot
#[derive(Debug, Clone)]
pub struct OrderBook {
    pub coin: String,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    pub timestamp: u64,
}

/// Market data container with stale detection
#[derive(Debug, Clone, Default)]
pub struct MarketData {
    pub mids: HashMap<String, f64>,
    pub timestamp: u64,
}

impl MarketData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Update mid price
    pub fn update_mid(&mut self, coin: String, price: f64) {
        self.mids.insert(coin, price);
        self.timestamp = Self::now_millis();
    }

    /// Get price if data is fresh (not stale)
    pub fn get_price(&self, coin: &str) -> Option<f64> {
        if self.is_stale() {
            return None;
        }
        self.mids.get(coin).copied()
    }

    /// Get price regardless of staleness (for logging/debugging)
    pub fn get_price_unchecked(&self, coin: &str) -> Option<f64> {
        self.mids.get(coin).copied()
    }

    /// Check if market data is stale
    pub fn is_stale(&self) -> bool {
        if self.timestamp == 0 {
            return true;
        }
        let now = Self::now_millis();
        now.saturating_sub(self.timestamp) > MAX_DATA_AGE_MS
    }

    /// Get data age in milliseconds
    pub fn age_ms(&self) -> u64 {
        if self.timestamp == 0 {
            return u64::MAX;
        }
        Self::now_millis().saturating_sub(self.timestamp)
    }

    fn now_millis() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_market_data_is_stale() {
        let data = MarketData::new();
        assert!(data.is_stale());
        assert_eq!(data.get_price("BTC"), None);
    }

    #[test]
    fn test_fresh_data_not_stale() {
        let mut data = MarketData::new();
        data.update_mid("BTC".to_string(), 100000.0);
        assert!(!data.is_stale());
        assert_eq!(data.get_price("BTC"), Some(100000.0));
    }

    #[test]
    fn test_get_price_unchecked() {
        let mut data = MarketData::new();
        data.update_mid("BTC".to_string(), 100000.0);
        assert_eq!(data.get_price_unchecked("BTC"), Some(100000.0));
    }

    #[test]
    fn test_missing_coin() {
        let mut data = MarketData::new();
        data.update_mid("BTC".to_string(), 100000.0);
        assert_eq!(data.get_price("ETH"), None);
    }

    #[test]
    fn test_multiple_coins() {
        let mut data = MarketData::new();
        data.update_mid("BTC".to_string(), 100000.0);
        data.update_mid("ETH".to_string(), 3000.0);
        assert_eq!(data.get_price("BTC"), Some(100000.0));
        assert_eq!(data.get_price("ETH"), Some(3000.0));
    }
}
