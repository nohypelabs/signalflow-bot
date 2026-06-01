use reqwest::Client;
use serde_json::Value;
use tracing::{debug, info};

use crate::domain::{Signal, SignalAction, SignalSource};
use crate::error::{BotError, Result};

/// Sodex API client for fetching trading signals
pub struct SodexClient {
    client: Client,
    api_url: String,
    api_key: String,
}

impl SodexClient {
    pub fn new(api_url: &str, api_key: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            api_url: api_url.to_string(),
            api_key: api_key.to_string(),
        }
    }

    /// Fetch trading signals
    pub async fn get_signals(&self) -> Result<Vec<Signal>> {
        debug!("Fetching signals from Sodex...");

        let resp = self
            .client
            .get(format!("{}/signals", self.api_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(BotError::Api(format!(
                "Sodex API error: HTTP {}",
                resp.status()
            )));
        }

        let data: Value = resp
            .json()
            .await
            .map_err(|e| BotError::Api(format!("Sodex parse failed: {}", e)))?;

        let signals = self.parse_signals(&data);
        info!("Got {} signals from Sodex", signals.len());
        Ok(signals)
    }

    fn parse_signals(&self, data: &Value) -> Vec<Signal> {
        let mut signals = Vec::new();

        let arr = data
            .as_array()
            .or_else(|| data.get("signals").and_then(|s| s.as_array()));

        if let Some(items) = arr {
            for item in items {
                if let Some(signal) = self.parse_single(item) {
                    signals.push(signal);
                }
            }
        }

        signals
    }

    fn parse_single(&self, data: &Value) -> Option<Signal> {
        let coin = data.get("coin")?.as_str()?.to_string();
        let action_str = data.get("action")?.as_str()?;
        let confidence = data
            .get("confidence")
            .and_then(|c| c.as_f64())
            .unwrap_or(0.5);
        let entry_price = data
            .get("target_price")
            .or_else(|| data.get("entry_price"))
            .and_then(|p| p.as_f64());
        let stop_loss = data.get("stop_loss").and_then(|p| p.as_f64());
        let take_profit = data.get("take_profit").and_then(|p| p.as_f64());
        let timestamp = data
            .get("timestamp")
            .and_then(|t| t.as_u64())
            .unwrap_or_else(Self::now_millis);

        let action = match action_str.to_lowercase().as_str() {
            "open_long" | "buy" | "long" => SignalAction::OpenLong,
            "open_short" | "sell" | "short" => SignalAction::OpenShort,
            "close_long" | "close_buy" => SignalAction::CloseLong,
            "close_short" | "close_sell" => SignalAction::CloseShort,
            _ => return None,
        };

        Some(Signal {
            source: SignalSource::Sodex,
            coin,
            action,
            confidence,
            entry_price,
            stop_loss,
            take_profit,
            timestamp,
        })
    }

    fn now_millis() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}
