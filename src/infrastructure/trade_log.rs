use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use tracing::{debug, warn};

use crate::domain::{OrderResult, OrderStatus};

/// JSONL trade logger — appends one JSON line per trade to a file
pub struct TradeLog {
    path: PathBuf,
}

/// Serializable trade record
#[derive(serde::Serialize)]
struct TradeRecord {
    timestamp: u64,
    coin: String,
    side: String,
    size: f64,
    price: f64,
    status: String,
    order_id: u64,
    avg_fill_price: f64,
    filled_size: f64,
    pnl: f64,
}

impl TradeLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Log a trade result to the JSONL file
    pub fn log_trade(&self, result: &OrderResult) {
        let (status_str, oid, avg_px, filled_sz) = match &result.status {
            OrderStatus::Filled {
                total_sz,
                avg_px,
                oid,
            } => ("filled".to_string(), *oid, *avg_px, *total_sz),
            OrderStatus::Resting { oid } => {
                ("resting".to_string(), *oid, 0.0, 0.0)
            }
            OrderStatus::Cancelled { oid } => {
                ("cancelled".to_string(), *oid, 0.0, 0.0)
            }
            OrderStatus::Error { message } => {
                (format!("error: {}", message), 0, 0.0, 0.0)
            }
            OrderStatus::Pending => ("pending".to_string(), 0, 0.0, 0.0),
        };

        let side_str = match result.order.side {
            crate::domain::Side::Buy => "buy",
            crate::domain::Side::Sell => "sell",
        };

        let record = TradeRecord {
            timestamp: result.timestamp,
            coin: result.order.coin.clone(),
            side: side_str.to_string(),
            size: result.order.size,
            price: result.order.price,
            status: status_str,
            order_id: oid,
            avg_fill_price: avg_px,
            filled_size: filled_sz,
            pnl: 0.0, // PnL calculated by position tracker, not here
        };

        match serde_json::to_string(&record) {
            Ok(json) => {
                match OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)
                {
                    Ok(mut file) => {
                        if writeln!(file, "{}", json).is_err() {
                            warn!("Failed to write trade record to {:?}", self.path);
                        } else {
                            debug!("Logged trade: {} {} {}", record.side, record.coin, record.size);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to open trade log {:?}: {}", self.path, e);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to serialize trade record: {}", e);
            }
        }
    }
}
