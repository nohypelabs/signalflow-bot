use std::collections::HashMap;
use log::info;

use super::order::Side;

/// Position side
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Long,
    Short,
}

/// Open position
#[derive(Debug, Clone)]
pub struct Position {
    pub coin: String,
    pub side: PositionSide,
    pub size: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
    pub timestamp: u64,
}

/// Position tracker - manages all open positions
pub struct PositionTracker {
    positions: HashMap<String, Position>,
    daily_realized_pnl: f64,
    last_reset_day: u64,
}

impl PositionTracker {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
            daily_realized_pnl: 0.0,
            last_reset_day: Self::current_day(),
        }
    }

    /// Check if position exists
    pub fn has_position(&self, coin: &str) -> bool {
        self.positions.contains_key(coin)
    }

    /// Get position
    pub fn get(&self, coin: &str) -> Option<&Position> {
        self.positions.get(coin)
    }

    /// Get total exposure in USD
    pub fn total_exposure(&self) -> f64 {
        self.positions
            .values()
            .map(|p| p.size * p.entry_price)
            .sum()
    }

    /// Get open position count
    pub fn count(&self) -> usize {
        self.positions.len()
    }

    /// Get daily realized PnL
    pub fn daily_pnl(&self) -> f64 {
        self.daily_realized_pnl
    }

    /// Update position after fill
    pub fn update(&mut self, coin: &str, side: Side, size: f64, price: f64) {
        if size <= 0.0 {
            return;
        }

        let now = Self::now_millis();
        let pos_side = match side {
            Side::Buy => PositionSide::Long,
            Side::Sell => PositionSide::Short,
        };

        if let Some(existing) = self.positions.get_mut(coin) {
            if (existing.side == PositionSide::Long && side == Side::Buy)
                || (existing.side == PositionSide::Short && side == Side::Sell)
            {
                // Same direction: average in
                let total_value = existing.size * existing.entry_price + size * price;
                existing.size += size;
                existing.entry_price = total_value / existing.size;
                existing.timestamp = now;
                info!("Increased {} position: size={:.6}", coin, existing.size);
            } else {
                // Opposite direction: close/reduce
                if size >= existing.size {
                    let pnl = Self::calc_pnl(existing, price, existing.size);
                    self.daily_realized_pnl += pnl;
                    info!("Closed {} position: pnl=${:.2}", coin, pnl);

                    let remaining = size - existing.size;
                    if remaining > 0.0 {
                        self.positions.insert(
                            coin.to_string(),
                            Position {
                                coin: coin.to_string(),
                                side: pos_side,
                                size: remaining,
                                entry_price: price,
                                unrealized_pnl: 0.0,
                                timestamp: now,
                            },
                        );
                    } else {
                        self.positions.remove(coin);
                    }
                } else {
                    let pnl = Self::calc_pnl(existing, price, size);
                    self.daily_realized_pnl += pnl;
                    existing.size -= size;
                    existing.timestamp = now;
                    info!(
                        "Reduced {} position: remaining={:.6}, pnl=${:.2}",
                        coin, existing.size, pnl
                    );
                }
            }
        } else {
            self.positions.insert(
                coin.to_string(),
                Position {
                    coin: coin.to_string(),
                    side: pos_side,
                    size,
                    entry_price: price,
                    unrealized_pnl: 0.0,
                    timestamp: now,
                },
            );
            info!("Opened {} position: size={:.6} @ {:.2}", coin, size, price);
        }
    }

    /// Update unrealized PnL from prices
    pub fn update_pnl(&mut self, prices: &HashMap<String, f64>) {
        for (coin, pos) in self.positions.iter_mut() {
            if let Some(&price) = prices.get(coin) {
                pos.unrealized_pnl = Self::calc_pnl(pos, price, pos.size);
            }
        }
    }

    /// Reset daily PnL if new day
    pub fn maybe_reset_daily(&mut self) {
        let day = Self::current_day();
        if day != self.last_reset_day {
            self.daily_realized_pnl = 0.0;
            self.last_reset_day = day;
        }
    }

    /// Bulk load positions from exchange (used on startup sync)
    pub fn load_positions(&mut self, positions: Vec<Position>) {
        let count = positions.len();
        for pos in positions {
            self.positions.insert(pos.coin.clone(), pos);
        }
        if count > 0 {
            info!("Synced {} positions from exchange", count);
        }
    }

    /// Get summary of all positions
    pub fn summary(&self) -> String {
        if self.positions.is_empty() {
            return "No open positions".to_string();
        }

        let mut lines = vec![format!(
            "Positions: {} | Exposure: ${:.2} | Daily PnL: ${:.2}",
            self.positions.len(),
            self.total_exposure(),
            self.daily_realized_pnl
        )];

        for (coin, pos) in &self.positions {
            lines.push(format!(
                "  {}: {:?} {:.4} @ {:.2} (unrealized: ${:.2})",
                coin, pos.side, pos.size, pos.entry_price, pos.unrealized_pnl
            ));
        }

        lines.join("\n")
    }

    fn calc_pnl(pos: &Position, current_price: f64, size: f64) -> f64 {
        match pos.side {
            PositionSide::Long => (current_price - pos.entry_price) * size,
            PositionSide::Short => (pos.entry_price - current_price) * size,
        }
    }

    /// Public static PnL calculation (for external use)
    pub fn calc_pnl_static(pos: &Position, current_price: f64, size: f64) -> f64 {
        Self::calc_pnl(pos, current_price, size)
    }

    fn current_day() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 86400
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
    fn test_new_tracker_is_empty() {
        let tracker = PositionTracker::new();
        assert_eq!(tracker.count(), 0);
        assert!(!tracker.has_position("BTC"));
        assert_eq!(tracker.total_exposure(), 0.0);
        assert_eq!(tracker.daily_pnl(), 0.0);
    }

    #[test]
    fn test_open_long_position() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.01, 100000.0);

        assert!(tracker.has_position("BTC"));
        assert_eq!(tracker.count(), 1);

        let pos = tracker.get("BTC").unwrap();
        assert_eq!(pos.side, PositionSide::Long);
        assert!((pos.size - 0.01).abs() < 1e-10);
        assert!((pos.entry_price - 100000.0).abs() < 1e-10);
    }

    #[test]
    fn test_open_short_position() {
        let mut tracker = PositionTracker::new();
        tracker.update("ETH", Side::Sell, 1.0, 3000.0);

        let pos = tracker.get("ETH").unwrap();
        assert_eq!(pos.side, PositionSide::Short);
    }

    #[test]
    fn test_increase_position() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.01, 100000.0);
        tracker.update("BTC", Side::Buy, 0.01, 110000.0);

        let pos = tracker.get("BTC").unwrap();
        assert!((pos.size - 0.02).abs() < 1e-10);
        // Average entry: (0.01 * 100000 + 0.01 * 110000) / 0.02 = 105000
        assert!((pos.entry_price - 105000.0).abs() < 1e-10);
    }

    #[test]
    fn test_close_position_full() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.01, 100000.0);
        tracker.update("BTC", Side::Sell, 0.01, 110000.0);

        assert!(!tracker.has_position("BTC"));
        assert_eq!(tracker.count(), 0);
        // PnL = (110000 - 100000) * 0.01 = 100
        assert!((tracker.daily_pnl() - 100.0).abs() < 1e-10);
    }

    #[test]
    fn test_reduce_position() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.02, 100000.0);
        tracker.update("BTC", Side::Sell, 0.01, 110000.0);

        assert!(tracker.has_position("BTC"));
        let pos = tracker.get("BTC").unwrap();
        assert!((pos.size - 0.01).abs() < 1e-10);
        // PnL = (110000 - 100000) * 0.01 = 100
        assert!((tracker.daily_pnl() - 100.0).abs() < 1e-10);
    }

    #[test]
    fn test_flip_position() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.01, 100000.0);
        tracker.update("BTC", Side::Sell, 0.02, 110000.0);

        // Should flip to short with remaining 0.01
        let pos = tracker.get("BTC").unwrap();
        assert_eq!(pos.side, PositionSide::Short);
        assert!((pos.size - 0.01).abs() < 1e-10);
    }

    #[test]
    fn test_total_exposure() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.01, 100000.0); // $1000
        tracker.update("ETH", Side::Buy, 1.0, 3000.0); // $3000

        assert!((tracker.total_exposure() - 4000.0).abs() < 1e-10);
    }

    #[test]
    fn test_short_pnl() {
        let mut tracker = PositionTracker::new();
        tracker.update("ETH", Side::Sell, 1.0, 3000.0);
        tracker.update("ETH", Side::Buy, 1.0, 2800.0);

        // PnL = (3000 - 2800) * 1.0 = 200
        assert!((tracker.daily_pnl() - 200.0).abs() < 1e-10);
    }

    #[test]
    fn test_ignore_zero_size() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.0, 100000.0);
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn test_summary_empty() {
        let tracker = PositionTracker::new();
        assert_eq!(tracker.summary(), "No open positions");
    }

    #[test]
    fn test_summary_with_positions() {
        let mut tracker = PositionTracker::new();
        tracker.update("BTC", Side::Buy, 0.01, 100000.0);
        let summary = tracker.summary();
        assert!(summary.contains("Positions: 1"));
        assert!(summary.contains("BTC"));
    }
}
