use serde_json::Value;
use log::debug;

/// Single price level in the orderbook
#[derive(Debug, Clone)]
pub struct BookLevel {
    pub price: f64,
    pub size: f64,
}

/// Snapshot of the orderbook with derived metrics
#[derive(Debug, Clone)]
pub struct OrderbookSnapshot {
    /// Top 10 bids (sorted descending by price)
    pub bids: Vec<BookLevel>,
    /// Top 10 asks (sorted ascending by price)
    pub asks: Vec<BookLevel>,
    /// Timestamp from exchange (ms)
    pub timestamp: u64,
}

/// Orderbook analyzer — maintains latest snapshot and computes metrics
pub struct OrderbookAnalyzer {
    snapshot: Option<OrderbookSnapshot>,
}

impl OrderbookAnalyzer {
    pub fn new() -> Self {
        Self { snapshot: None }
    }

    /// Update orderbook from WebSocket l2Book message
    pub fn update(&mut self, data: &Value) {
        let coin = data.get("coin").and_then(|c| c.as_str()).unwrap_or("");
        let time = data.get("time").and_then(|t| t.as_u64()).unwrap_or(0);

        let levels = match data.get("levels").and_then(|l| l.as_array()) {
            Some(l) => l,
            None => return,
        };

        if levels.len() < 2 {
            return;
        }

        // levels[0] = bids, levels[1] = asks
        let bids_raw = levels[0].as_array();
        let asks_raw = levels[1].as_array();

        let mut bids: Vec<BookLevel> = bids_raw
            .map(|arr| {
                arr.iter()
                    .filter_map(|lvl| {
                        let px = lvl.get("px")?.as_str()?.parse::<f64>().ok()?;
                        let sz = lvl.get("sz")?.as_str()?.parse::<f64>().ok()?;
                        // Filter NaN, negative, and zero-size levels (sz=0 means deleted on Hyperliquid)
                        if !px.is_finite() || !sz.is_finite() || px <= 0.0 || sz <= 0.0 {
                            return None;
                        }
                        Some(BookLevel { price: px, size: sz })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut asks: Vec<BookLevel> = asks_raw
            .map(|arr| {
                arr.iter()
                    .filter_map(|lvl| {
                        let px = lvl.get("px")?.as_str()?.parse::<f64>().ok()?;
                        let sz = lvl.get("sz")?.as_str()?.parse::<f64>().ok()?;
                        if !px.is_finite() || !sz.is_finite() || px <= 0.0 || sz <= 0.0 {
                            return None;
                        }
                        Some(BookLevel { price: px, size: sz })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Sort: bids descending, asks ascending (safe for NaN — use unwrap_or(Equal))
        bids.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap_or(std::cmp::Ordering::Equal));
        asks.sort_by(|a, b| a.price.partial_cmp(&b.price).unwrap_or(std::cmp::Ordering::Equal));

        // Keep top 10
        bids.truncate(10);
        asks.truncate(10);

        self.snapshot = Some(OrderbookSnapshot {
            bids,
            asks,
            timestamp: time,
        });

        debug!("Orderbook updated for {}: {} bids, {} asks", coin,
            self.snapshot.as_ref().unwrap().bids.len(),
            self.snapshot.as_ref().unwrap().asks.len());
    }

    /// Get current snapshot
    pub fn snapshot(&self) -> Option<&OrderbookSnapshot> {
        self.snapshot.as_ref()
    }

    /// Calculate spread in basis points
    /// spread_bps = (best_ask - best_bid) / mid_price * 10000
    pub fn spread_bps(&self) -> Option<f64> {
        let snap = self.snapshot.as_ref()?;
        let best_bid = snap.bids.first()?.price;
        let best_ask = snap.asks.first()?.price;

        // Guard against crossed book (stale/corrupted data)
        if best_ask <= best_bid {
            return None;
        }

        let mid = (best_bid + best_ask) / 2.0;

        if mid <= 0.0 {
            return None;
        }

        Some((best_ask - best_bid) / mid * 10000.0)
    }

    /// Imbalance ratio = total bid volume / total ask volume (top 10)
    /// > 1.0 means more buying pressure
    pub fn imbalance_ratio(&self) -> Option<f64> {
        let snap = self.snapshot.as_ref()?;
        let bid_vol: f64 = snap.bids.iter().map(|l| l.size).sum();
        let ask_vol: f64 = snap.asks.iter().map(|l| l.size).sum();

        if ask_vol <= 0.0 {
            return None;
        }

        Some(bid_vol / ask_vol)
    }

    /// Depth in USD for levels 1 through 5
    /// Returns Vec of (level, depth_usd) where depth_usd = sum(price * size) for each side
    pub fn depth_usd_levels(&self) -> Option<Vec<(usize, f64)>> {
        let snap = self.snapshot.as_ref()?;
        let mut result = Vec::new();

        for level in 1..=5 {
            let bid_usd: f64 = snap.bids.iter().take(level).map(|l| l.price * l.size).sum();
            let ask_usd: f64 = snap.asks.iter().take(level).map(|l| l.price * l.size).sum();
            // Use the minimum of bid/ask depth as the effective depth
            let depth = bid_usd.min(ask_usd);
            result.push((level, depth));
        }

        Some(result)
    }

    /// Depth USD at level 1 only
    pub fn depth_usd_level1(&self) -> Option<f64> {
        let snap = self.snapshot.as_ref()?;
        let bid = snap.bids.first()?;
        let ask = snap.asks.first()?;
        let bid_usd = bid.price * bid.size;
        let ask_usd = ask.price * ask.size;
        Some(bid_usd.min(ask_usd))
    }

    /// Best bid price
    pub fn best_bid(&self) -> Option<f64> {
        self.snapshot.as_ref()?.bids.first().map(|l| l.price)
    }

    /// Best ask price
    pub fn best_ask(&self) -> Option<f64> {
        self.snapshot.as_ref()?.asks.first().map(|l| l.price)
    }

    /// Mid price
    pub fn mid_price(&self) -> Option<f64> {
        let bid = self.best_bid()?;
        let ask = self.best_ask()?;
        Some((bid + ask) / 2.0)
    }
}
