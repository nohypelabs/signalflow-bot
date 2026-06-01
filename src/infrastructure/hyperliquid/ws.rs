use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::domain::MarketData;
use crate::error::{BotError, Result};

/// Maximum reconnection attempts before giving up
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

/// Initial backoff delay in seconds
const INITIAL_BACKOFF_SECS: u64 = 1;

/// Maximum backoff delay in seconds
const MAX_BACKOFF_SECS: u64 = 30;

/// WebSocket client for real-time market data
pub struct HyperliquidWs {
    url: String,
    market_data: Arc<RwLock<MarketData>>,
}

impl HyperliquidWs {
    pub fn new(url: &str, market_data: Arc<RwLock<MarketData>>) -> Self {
        Self {
            url: url.to_string(),
            market_data,
        }
    }

    /// Connect and start receiving market data with exponential backoff
    pub async fn connect(&self) -> Result<()> {
        let url = self.url.clone();
        let market_data = self.market_data.clone();

        tokio::spawn(async move {
            let mut attempt = 0u32;

            loop {
                match Self::run_ws_loop(&url, &market_data).await {
                    Ok(_) => {
                        warn!("WebSocket disconnected");
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                    }
                }

                attempt += 1;

                if attempt > MAX_RECONNECT_ATTEMPTS {
                    error!(
                        "Max reconnection attempts ({}) exceeded, stopping",
                        MAX_RECONNECT_ATTEMPTS
                    );
                    break;
                }

                // Exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s, 30s, ...
                let backoff = INITIAL_BACKOFF_SECS
                    .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)))
                    .min(MAX_BACKOFF_SECS);

                warn!(
                    "Reconnecting in {}s (attempt {}/{})",
                    backoff, attempt, MAX_RECONNECT_ATTEMPTS
                );
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
            }
        });

        info!("WebSocket stream started");
        Ok(())
    }

    async fn run_ws_loop(url: &str, market_data: &Arc<RwLock<MarketData>>) -> Result<()> {
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| BotError::Network(e.to_string()))?;

        info!("WebSocket connected to {}", url);

        let (mut write, mut read) = ws_stream.split();

        // Subscribe to all mids
        let subscribe = serde_json::json!({
            "method": "subscribe",
            "subscription": {"type": "allMids"}
        });
        write
            .send(Message::Text(subscribe.to_string().into()))
            .await
            .map_err(|e| BotError::Network(e.to_string()))?;

        info!("Subscribed to allMids stream");

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    Self::handle_message(&text, market_data).await;
                }
                Ok(Message::Ping(data)) => {
                    let _ = write.send(Message::Pong(data)).await;
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket closed by server");
                    break;
                }
                Err(e) => {
                    error!("WebSocket read error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        Ok(())
    }

    async fn handle_message(text: &str, market_data: &Arc<RwLock<MarketData>>) {
        let msg: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return,
        };

        // Handle allMids subscription response
        if let Some(channel) = msg.get("channel").and_then(|c| c.as_str()) {
            if channel == "allMids" {
                if let Some(data) = msg.get("data") {
                    if let Some(mids) = data.as_object() {
                        let mut md = market_data.write().await;
                        for (coin, price_str) in mids {
                            if let Some(price) =
                                price_str.as_str().and_then(|s| s.parse::<f64>().ok())
                            {
                                md.update_mid(coin.clone(), price);
                            }
                        }
                        debug!("Updated {} mid prices", mids.len());
                    }
                }
            }
        }
    }
}
