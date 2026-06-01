use serde::{Deserialize, Serialize};

/// Order side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// Order type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Limit,
    Market,
    StopLoss,
    TakeProfit,
}

/// Time in force
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    Gtc, // Good til canceled
    Ioc, // Immediate or cancel
    Alo, // Add liquidity only (post-only)
}

/// Domain order - pure business logic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub coin: String,
    pub side: Side,
    pub order_type: OrderType,
    pub size: f64,
    pub price: f64,
    pub leverage: Option<u32>,
    pub reduce_only: bool,
    pub time_in_force: TimeInForce,
    pub client_order_id: Option<String>,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
}

/// Order status from exchange
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderStatus {
    Pending,
    Resting {
        oid: u64,
    },
    Filled {
        total_sz: f64,
        avg_px: f64,
        oid: u64,
    },
    Cancelled {
        oid: u64,
    },
    Error {
        message: String,
    },
}

/// Order execution result
#[derive(Debug, Clone)]
pub struct OrderResult {
    pub order: Order,
    pub status: OrderStatus,
    pub timestamp: u64,
}

impl Order {
    /// Create a limit order
    pub fn limit(coin: impl Into<String>, side: Side, size: f64, price: f64) -> Self {
        Self {
            coin: coin.into(),
            side,
            order_type: OrderType::Limit,
            size,
            price,
            leverage: None,
            reduce_only: false,
            time_in_force: TimeInForce::Gtc,
            client_order_id: None,
            stop_loss: None,
            take_profit: None,
        }
    }

    /// Validate order
    pub fn validate(&self) -> Result<(), String> {
        if self.size <= 0.0 {
            return Err(format!("Size must be > 0, got {}", self.size));
        }
        if self.price <= 0.0 {
            return Err(format!("Price must be > 0, got {}", self.price));
        }
        if self.size * self.price < 10.0 {
            return Err(format!(
                "Order value must be >= $10, got ${:.2}",
                self.size * self.price
            ));
        }
        Ok(())
    }

    /// Set leverage
    pub fn with_leverage(mut self, leverage: u32) -> Self {
        self.leverage = Some(leverage);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_validation_success() {
        let order = Order::limit("BTC", Side::Buy, 0.001, 100000.0);
        assert!(order.validate().is_ok());
    }

    #[test]
    fn test_order_validation_zero_size() {
        let order = Order::limit("BTC", Side::Buy, 0.0, 100000.0);
        assert!(order.validate().is_err());
        assert!(order.validate().unwrap_err().contains("Size must be > 0"));
    }

    #[test]
    fn test_order_validation_negative_size() {
        let order = Order::limit("BTC", Side::Buy, -1.0, 100000.0);
        assert!(order.validate().is_err());
    }

    #[test]
    fn test_order_validation_zero_price() {
        let order = Order::limit("BTC", Side::Buy, 1.0, 0.0);
        assert!(order.validate().is_err());
        assert!(order.validate().unwrap_err().contains("Price must be > 0"));
    }

    #[test]
    fn test_order_validation_below_minimum_value() {
        // $5 order - below $10 minimum
        let order = Order::limit("BTC", Side::Buy, 0.00005, 100000.0);
        assert!(order.validate().is_err());
        assert!(order
            .validate()
            .unwrap_err()
            .contains("Order value must be >= $10"));
    }

    #[test]
    fn test_order_validation_exact_minimum_value() {
        // $10 order - exactly at minimum
        let order = Order::limit("BTC", Side::Buy, 0.0001, 100000.0);
        assert!(order.validate().is_ok());
    }

    #[test]
    fn test_order_with_leverage() {
        let order = Order::limit("ETH", Side::Buy, 0.1, 3000.0).with_leverage(20);
        assert_eq!(order.leverage, Some(20));
        assert_eq!(order.coin, "ETH");
        assert_eq!(order.side, Side::Buy);
    }

    #[test]
    fn test_side_equality() {
        assert_eq!(Side::Buy, Side::Buy);
        assert_ne!(Side::Buy, Side::Sell);
    }

    #[test]
    fn test_order_type_equality() {
        assert_eq!(OrderType::Limit, OrderType::Limit);
        assert_ne!(OrderType::Limit, OrderType::Market);
    }
}
