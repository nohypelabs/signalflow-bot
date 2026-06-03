use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::domain::{Position, PositionSide};
use crate::error::{BotError, Result};

/// REST client for Hyperliquid API
pub struct HyperliquidRest {
    client: Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct MetaResponse {
    universe: Vec<AssetInfo>,
}

#[derive(Debug, Deserialize)]
struct AssetInfo {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ExchangeResponse {
    status: String,
    response: Option<ExchangeResponseData>,
}

#[derive(Debug, Deserialize)]
struct ExchangeResponseData {
    data: Option<Value>,
}

impl HyperliquidRest {
    pub fn new(base_url: &str) -> Self {
        Self {
            client: Client::builder()
                .pool_max_idle_per_host(10)
                .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            base_url: base_url.to_string(),
        }
    }

    /// Fetch asset metadata
    pub async fn fetch_meta(&self) -> Result<HashMap<String, u32>> {
        let payload = json!({"type": "meta", "dex": ""});

        let resp = self
            .client
            .post(format!("{}/info", self.base_url))
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(BotError::Api(format!(
                "Meta fetch failed: HTTP {}",
                resp.status()
            )));
        }

        let meta: MetaResponse = resp
            .json()
            .await
            .map_err(|e| BotError::Api(format!("Meta parse failed: {}", e)))?;

        let mut map = HashMap::new();
        for (i, asset) in meta.universe.iter().enumerate() {
            map.insert(asset.name.clone(), i as u32);
        }

        debug!("Fetched {} asset IDs", map.len());
        Ok(map)
    }

    /// Get all mid prices (REST fallback)
    pub async fn get_all_mids(&self) -> Result<HashMap<String, String>> {
        let payload = json!({"type": "allMids", "dex": ""});

        let resp = self
            .client
            .post(format!("{}/info", self.base_url))
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(BotError::Api(format!(
                "Mids fetch failed: HTTP {}",
                resp.status()
            )));
        }

        resp.json()
            .await
            .map_err(|e| BotError::Api(format!("Mids parse failed: {}", e)))
    }

    /// Fetch current positions from Hyperliquid clearinghouse state
    pub async fn fetch_positions(&self, address: &str) -> Result<Vec<Position>> {
        let payload = json!({
            "type": "clearinghouseState",
            "user": address,
            "dex": ""
        });

        let resp = self
            .client
            .post(format!("{}/info", self.base_url))
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(BotError::Api(format!(
                "Position fetch failed: HTTP {}",
                resp.status()
            )));
        }

        let data: Value = resp
            .json()
            .await
            .map_err(|e| BotError::Api(format!("Position parse failed: {}", e)))?;

        let mut positions = Vec::new();

        if let Some(asset_positions) = data
            .get("assetPositions")
            .and_then(|ap| ap.as_array())
        {
            for ap in asset_positions {
                if let Some(pos_data) = ap.get("position") {
                    let coin = pos_data
                        .get("coin")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();

                    let szi: f64 = pos_data
                        .get("szi")
                        .and_then(|s| s.as_str())
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);

                    if szi.abs() < 1e-10 {
                        continue; // No position
                    }

                    let entry_price: f64 = pos_data
                        .get("entryPx")
                        .and_then(|p| p.as_str())
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);

                    let side = if szi > 0.0 {
                        PositionSide::Long
                    } else {
                        PositionSide::Short
                    };

                    let unrealized_pnl: f64 = pos_data
                        .get("unrealizedPnl")
                        .and_then(|p| p.as_str())
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);

                    positions.push(Position {
                        coin,
                        side,
                        size: szi.abs(),
                        entry_price,
                        unrealized_pnl,
                        timestamp: 0,
                    });
                }
            }
        }

        info!("Fetched {} positions from Hyperliquid", positions.len());
        Ok(positions)
    }

    /// Execute exchange action (place order, cancel, etc) with rate limit retry
    pub async fn exchange(&self, signed_payload: Value) -> Result<Value> {
        let max_retries = 3;
        let mut delay_ms = 1000u64;

        for attempt in 0..=max_retries {
            let resp = self
                .client
                .post(format!("{}/exchange", self.base_url))
                .json(&signed_payload)
                .send()
                .await?;

            // Rate limited — retry with backoff
            if resp.status() == 429 {
                if attempt < max_retries {
                    warn!(
                        "Rate limited (429), retrying in {}ms (attempt {}/{})",
                        delay_ms,
                        attempt + 1,
                        max_retries
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms *= 2;
                    continue;
                }
                return Err(BotError::Order(
                    "Rate limited after max retries".to_string(),
                ));
            }

            if !resp.status().is_success() {
                return Err(BotError::Order(format!(
                    "Exchange failed: HTTP {}",
                    resp.status()
                )));
            }

            let exchange_resp: ExchangeResponse = resp
                .json()
                .await
                .map_err(|e| BotError::Order(format!("Exchange parse failed: {}", e)))?;

            // Also check for rate limit in response body
            if exchange_resp.status == "rate_limit" || exchange_resp.status == "ratelimited" {
                if attempt < max_retries {
                    warn!(
                        "Rate limited (response), retrying in {}ms (attempt {}/{})",
                        delay_ms,
                        attempt + 1,
                        max_retries
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms *= 2;
                    continue;
                }
                return Err(BotError::Order(
                    "Rate limited after max retries".to_string(),
                ));
            }

            if exchange_resp.status != "ok" {
                return Err(BotError::Order(format!(
                    "Exchange rejected: {}",
                    exchange_resp.status
                )));
            }

            let data = exchange_resp
                .response
                .and_then(|r| r.data)
                .unwrap_or_default();

            if let Some(statuses) = data.get("statuses").and_then(|s| s.as_array()) {
                if let Some(first) = statuses.first() {
                    if first.get("error").is_some() {
                        let msg = first
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("Unknown error");
                        return Err(BotError::Order(format!("Order error: {}", msg)));
                    }
                }
            }

            return Ok(data);
        }

        Err(BotError::Order("Max retries exceeded".to_string()))
    }
}
