use log::{debug, info, warn};

use crate::hl_rest::HyperliquidRest;

/// Funding rate monitor
pub struct FundingMonitor {
    /// Current funding rate per coin (as decimal, e.g. 0.0005 = 0.05%)
    rates: std::collections::HashMap<String, f64>,
    /// REST client for polling
    rest: HyperliquidRest,
    /// Poll interval in seconds
    poll_secs: u64,
}

impl FundingMonitor {
    pub fn new(rest: HyperliquidRest, poll_secs: u64) -> Self {
        Self {
            rates: std::collections::HashMap::new(),
            rest,
            poll_secs,
        }
    }

    /// Poll funding rates from the API
    pub async fn poll(&mut self) {
        match self.rest.fetch_funding_rates().await {
            Ok(rates) => {
                for (coin, rate) in &rates {
                    debug!("Funding rate {}: {:.6}%", coin, rate * 100.0);
                }
                self.rates = rates;
            }
            Err(e) => {
                warn!("Failed to fetch funding rates: {}", e);
            }
        }
    }

    /// Get funding rate for a specific coin
    pub fn get_rate(&self, coin: &str) -> Option<f64> {
        self.rates.get(coin).copied()
    }

    /// Check if funding rate is too high for a LONG position
    /// Longs pay when funding is positive; reject if > 0.05%
    pub fn is_funding_too_high_for_long(&self, coin: &str) -> bool {
        match self.rates.get(coin) {
            Some(&rate) => {
                let threshold = 0.0005; // 0.05%
                if rate > threshold {
                    info!(
                        "Funding too high for LONG {}: {:.4}% > 0.05%",
                        coin,
                        rate * 100.0
                    );
                    return true;
                }
                false
            }
            None => false, // No data -> don't reject
        }
    }

    /// Check if funding rate is too negative for a SHORT position
    /// Shorts pay when funding is negative; reject if < -0.05%
    pub fn is_funding_too_high_for_short(&self, coin: &str) -> bool {
        match self.rates.get(coin) {
            Some(&rate) => {
                let threshold = -0.0005; // -0.05%
                if rate < threshold {
                    info!(
                        "Funding too negative for SHORT {}: {:.4}% < -0.05%",
                        coin,
                        rate * 100.0
                    );
                    return true;
                }
                false
            }
            None => false,
        }
    }

    /// Get poll interval
    pub fn poll_secs(&self) -> u64 {
        self.poll_secs
    }
}
