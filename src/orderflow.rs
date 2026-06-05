use serde_json::Value;
use std::collections::VecDeque;
use log::debug;


/// Single trade event from the trades channel
#[derive(Debug, Clone)]
pub struct TradeEvent {
    pub coin: String,
    pub side: TradeSide,
    pub price: f64,
    pub size: f64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TradeSide {
    Buy,
    Sell,
}

/// CVD (Cumulative Volume Delta) tracker with rolling window
pub struct CvdTracker {
    /// Rolling window of (timestamp_ms, delta) entries
    window: VecDeque<(u64, f64)>,
    /// Rolling window duration in milliseconds (default 60s)
    window_ms: u64,
    /// Current CVD sum
    cvd: f64,
    /// Price at start of window (for divergence detection)
    price_at_window_start: Option<f64>,
    /// Latest price
    latest_price: Option<f64>,
}

impl CvdTracker {
    pub fn new(window_ms: u64) -> Self {
        Self {
            window: VecDeque::new(),
            window_ms,
            cvd: 0.0,
            price_at_window_start: None,
            latest_price: None,
        }
    }

    /// Process a trade event: calculate delta and add to rolling window
    pub fn on_trade(&mut self, trade: &TradeEvent) {
        // Delta = +volume if buy, -volume if sell
        let delta = match trade.side {
            TradeSide::Buy => trade.price * trade.size,
            TradeSide::Sell => -(trade.price * trade.size),
        };

        let now = trade.timestamp_ms;

        // Add to window
        self.window.push_back((now, delta));
        self.cvd += delta;

        // Update price tracking
        if self.price_at_window_start.is_none() {
            self.price_at_window_start = Some(trade.price);
        }
        self.latest_price = Some(trade.price);

        // Evict old entries
        self.evict_old(now);
    }

    /// Remove entries outside the rolling window
    fn evict_old(&mut self, now: u64) {
        let cutoff = now.saturating_sub(self.window_ms);
        while let Some(&front) = self.window.front() {
            if front.0 < cutoff {
                if let Some((_, delta)) = self.window.pop_front() {
                    self.cvd -= delta;
                }
                // Update price_at_window_start to the new first entry's price
                // (approximate — we don't store price per entry, so use latest)
            } else {
                break;
            }
        }

        // If window is empty, reset price tracking
        if self.window.is_empty() {
            self.price_at_window_start = self.latest_price;
        }
    }

    /// Get current CVD value (rolling sum over window)
    pub fn get_cvd(&self) -> f64 {
        self.cvd
    }

    /// Check if CVD is diverging from price trend
    /// Divergence = price going up but CVD going down, or vice versa
    ///
    /// This is detected by comparing:
    /// - Price trend: current price vs price at window start
    /// - CVD trend: current CVD sign (positive = net buying, negative = net selling)
    pub fn is_cvd_diverging(&self, price_trend_up: bool) -> bool {
        // Need at least some data
        if self.window.len() < 5 {
            return false;
        }

        // Use a small threshold to avoid treating zero CVD as divergence
        let cvd_positive = self.cvd > 1e-10;
        let cvd_negative = self.cvd < -1e-10;

        // Divergence: price up + CVD negative, or price down + CVD positive
        if price_trend_up && cvd_negative {
            debug!(
                "CVD divergence detected: price trending UP but CVD is NEGATIVE ({:.2})",
                self.cvd
            );
            return true;
        }

        if !price_trend_up && cvd_positive {

            debug!(
                "CVD divergence detected: price trending DOWN but CVD is POSITIVE ({:.2})",
                self.cvd
            );
            return true;
        }

        false
    }

    /// Get the number of trades in the current window
    pub fn trade_count(&self) -> usize {
        self.window.len()
    }
}

/// Orderflow analyzer combining CVD tracking per pair
pub struct OrderflowAnalyzer {
    trackers: std::collections::HashMap<String, CvdTracker>,
    window_ms: u64,
}

impl OrderflowAnalyzer {
    pub fn new(window_ms: u64) -> Self {
        Self {
            trackers: std::collections::HashMap::new(),
            window_ms,
        }
    }

    /// Process an incoming trade from the WebSocket
    pub fn on_trade(&mut self, trade: &TradeEvent) {
        let tracker = self
            .trackers
            .entry(trade.coin.clone())
            .or_insert_with(|| CvdTracker::new(self.window_ms));
        tracker.on_trade(trade);
    }

    /// Parse a WebSocket trades message and process each trade
    pub fn on_ws_message(&mut self, data: &Value) {
        // trades channel sends an array of trades
        let trades = match data.as_array() {
            Some(arr) => arr,
            None => return,
        };

        for t in trades {
            let coin = t.get("coin").and_then(|c| c.as_str()).unwrap_or("");
            let side_str = t.get("side").and_then(|s| s.as_str()).unwrap_or("");
            let px = t.get("px").and_then(|p| p.as_str()).unwrap_or("0");
            let sz = t.get("sz").and_then(|s| s.as_str()).unwrap_or("0");
            let time = t.get("time").and_then(|t| t.as_u64()).unwrap_or(0);

            let side = match side_str {
                "B" => TradeSide::Buy,
                "A" => TradeSide::Sell,
                _ => continue,
            };

            let price = px.parse::<f64>().unwrap_or(0.0);
            let size = sz.parse::<f64>().unwrap_or(0.0);

            // Guard: reject NaN, Inf, zero, and negative values
            if !price.is_finite() || !size.is_finite() || price <= 0.0 || size <= 0.0 {
                continue;
            }

            let trade = TradeEvent {
                coin: coin.to_string(),
                side,
                price,
                size,
                timestamp_ms: time,
            };

            self.on_trade(&trade);
        }
    }

    /// Get CVD for a specific pair
    pub fn get_cvd(&self, pair: &str) -> f64 {
        self.trackers
            .get(pair)
            .map(|t| t.get_cvd())
            .unwrap_or(0.0)
    }

    /// Check if CVD is diverging for a pair given price trend
    pub fn is_cvd_diverging(&self, pair: &str, price_trend_up: bool) -> bool {
        self.trackers
            .get(pair)
            .map(|t| t.is_cvd_diverging(price_trend_up))
            .unwrap_or(false)
    }
}
