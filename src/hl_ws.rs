use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use log::{debug, error, info, warn};

use crate::error::Result;

/// WebSocket message from Hyperliquid
#[derive(Debug, Clone)]
pub struct WsMessage {
    pub channel: String,
    pub data: Value,
}

/// Hyperliquid WebSocket client
pub struct HyperliquidWs {
    ws_url: String,
}

impl HyperliquidWs {
    pub fn new(ws_url: &str) -> Self {
        Self {
            ws_url: ws_url.to_string(),
        }
    }

    /// Subscribe to channels and return a receiver for messages
    pub async fn subscribe(
        &self,
        channels: Vec<WsSubscription>,
    ) -> Result<mpsc::Receiver<WsMessage>> {
        let (tx, rx) = mpsc::channel::<WsMessage>(1024);
        let ws_url = self.ws_url.clone();

        tokio::spawn(async move {
            let mut reconnect_delay = 1u64;
            let mut connected_at = std::time::Instant::now();

            loop {
                info!("Connecting to WebSocket: {}", ws_url);

                match connect_async(&ws_url).await {
                    Ok((ws_stream, _)) => {
                        let (mut write, mut read) = ws_stream.split();

                        // Subscribe to all channels — on failure, reconnect immediately
                        let mut sub_failed = false;
                        for sub in &channels {
                            let msg = json!({
                                "method": "subscribe",
                                "subscription": sub.to_json()
                            });
                            if let Ok(text) = serde_json::to_string(&msg) {
                                if let Err(e) = write.send(Message::Text(text.into())).await {
                                    error!("Failed to send subscription: {} — reconnecting", e);
                                    sub_failed = true;
                                    break;
                                }
                                debug!("Subscribed: {:?}", sub);
                            }
                        }
                        if sub_failed {
                            tokio::time::sleep(std::time::Duration::from_secs(reconnect_delay)).await;
                            reconnect_delay = (reconnect_delay * 2).min(30);
                            continue;
                        }

                        connected_at = std::time::Instant::now();

                        // Read loop — handle Ping inline (Hyperliquid requires prompt Pong)
                        while let Some(msg_result) = read.next().await {
                            match msg_result {
                                Ok(Message::Text(text)) => {
                                    if let Ok(val) = serde_json::from_str::<Value>(&text) {
                                        if let Some(channel) = val.get("channel").and_then(|c| c.as_str()) {
                                            // Skip subscription confirmations
                                            if channel == "subscriptionResponse" {
                                                continue;
                                            }

                                            let data = val.get("data").cloned().unwrap_or(Value::Null);
                                            let ws_msg = WsMessage {
                                                channel: channel.to_string(),
                                                data,
                                            };

                                            if tx.send(ws_msg).await.is_err() {
                                                return; // Receiver dropped
                                            }
                                        }
                                    }
                                }
                                Ok(Message::Binary(data)) => {
                                    // Hyperliquid may send binary messages — log instead of silently dropping
                                    debug!("Received binary message ({} bytes), ignoring", data.len());
                                }
                                Ok(Message::Ping(data)) => {
                                    if let Err(e) = write.send(Message::Pong(data)).await {
                                        warn!("Failed to send Pong: {} — reconnecting", e);
                                        break;
                                    }
                                }
                                Ok(Message::Close(_)) => {
                                    warn!("WebSocket closed by server");
                                    break;
                                }
                                Err(e) => {
                                    error!("WebSocket read error: {}", e);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        error!("WebSocket connection failed: {}", e);
                    }
                }

                // Only reset backoff if connection was alive for > 60 seconds
                if connected_at.elapsed().as_secs() > 60 {
                    reconnect_delay = 1;
                }

                warn!("Reconnecting in {}s...", reconnect_delay);
                tokio::time::sleep(std::time::Duration::from_secs(reconnect_delay)).await;
                reconnect_delay = (reconnect_delay * 2).min(30);
            }
        });

        Ok(rx)
    }
}

/// WebSocket subscription types
#[derive(Debug, Clone)]
pub enum WsSubscription {
    L2Book { coin: String },
    Trades { coin: String },
    AllMids,
}

impl WsSubscription {
    fn to_json(&self) -> Value {
        match self {
            WsSubscription::L2Book { coin } => json!({"type": "l2Book", "coin": coin}),
            WsSubscription::Trades { coin } => json!({"type": "trades", "coin": coin}),
            WsSubscription::AllMids => json!({"type": "allMids"}),
        }
    }
}
