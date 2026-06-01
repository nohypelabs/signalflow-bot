use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::debug;

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

    /// Execute exchange action (place order, cancel, etc)
    pub async fn exchange(&self, signed_payload: Value) -> Result<Value> {
        let resp = self
            .client
            .post(format!("{}/exchange", self.base_url))
            .json(&signed_payload)
            .send()
            .await?;

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

        Ok(data)
    }
}
