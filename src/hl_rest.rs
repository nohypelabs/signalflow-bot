use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use log::{debug, info, warn};

use crate::error::{BotError, Result};

/// REST client for Hyperliquid API
pub struct HyperliquidRest {
    client: Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
struct MetaResponse {
    universe: Vec<MetaAsset>,
}

#[derive(Debug, Deserialize)]
struct MetaAsset {
    name: String,
    #[serde(rename = "szDecimals")]
    sz_decimals: Option<u32>,
    #[serde(rename = "maxLeverage")]
    max_leverage: Option<u32>,
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
                .tcp_keepalive(Some(Duration::from_secs(30)))
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            base_url: base_url.to_string(),
        }
    }

    /// Fetch asset metadata (name -> index mapping)
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

    /// Fetch extended asset metadata (with sz_decimals and max_leverage)
    pub async fn fetch_asset_info(&self) -> Result<Vec<AssetInfo>> {
        let payload = json!({"type": "meta", "dex": ""});

        let resp = self
            .client
            .post(format!("{}/info", self.base_url))
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(BotError::Api(format!("Meta fetch failed: HTTP {}", resp.status())));
        }

        let meta: MetaResponse = resp.json().await
            .map_err(|e| BotError::Api(format!("Meta parse failed: {}", e)))?;

        let mut assets = Vec::new();
        for (i, asset) in meta.universe.iter().enumerate() {
            let max_lev = asset.max_leverage.unwrap_or(10);
            assets.push(AssetInfo {
                name: asset.name.clone(),
                index: i as u32,
                sz_decimals: asset.sz_decimals.unwrap_or(2),
                max_leverage: max_lev,
            });
        }

        debug!("Fetched {} asset infos", assets.len());
        Ok(assets)
    }

    /// Get all mid prices
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

    /// Fetch funding rates from predictedFundings
    pub async fn fetch_funding_rates(&self) -> Result<HashMap<String, f64>> {
        let payload = json!({"type": "predictedFundings", "dex": ""});

        let resp = self
            .client
            .post(format!("{}/info", self.base_url))
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(BotError::Api(format!(
                "Funding fetch failed: HTTP {}",
                resp.status()
            )));
        }

        let data: Value = resp
            .json()
            .await
            .map_err(|e| BotError::Api(format!("Funding parse failed: {}", e)))?;

        let mut rates = HashMap::new();

        // predictedFundings format: [[coin, {fundingRate, nextFundingTime}], ...]
        if let Some(arr) = data.as_array() {
            for item in arr {
                if let Some(arr2) = item.as_array() {
                    if arr2.len() >= 2 {
                        let coin = arr2[0].as_str().unwrap_or("").to_string();
                        if let Some(detail) = arr2[1].as_array() {
                            if let Some(first) = detail.first() {
                                if let Some(obj) = first.as_object() {
                                    if let Some(rate_val) = obj.get("fundingRate") {
                                        let rate = rate_val
                                            .as_str()
                                            .unwrap_or("0")
                                            .parse::<f64>()
                                            .unwrap_or(0.0);
                                        rates.insert(coin, rate);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        debug!("Fetched funding rates for {} coins", rates.len());
        Ok(rates)
    }

    /// Fetch OHLCV candles (not directly available from Hyperliquid REST,
    /// so we build from trades or use a helper. This fetches recent trades.)
    pub async fn fetch_recent_trades(&self, coin: &str) -> Result<Vec<TradeRaw>> {
        let payload = json!({"type": "recentTrades", "coin": coin});

        let resp = self
            .client
            .post(format!("{}/info", self.base_url))
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(BotError::Api(format!(
                "Trades fetch failed: HTTP {}",
                resp.status()
            )));
        }

        let trades: Vec<TradeRaw> = resp
            .json()
            .await
            .map_err(|e| BotError::Api(format!("Trades parse failed: {}", e)))?;

        Ok(trades)
    }

    /// Fetch current positions
    pub async fn fetch_positions(&self, address: &str) -> Result<Vec<PositionInfo>> {
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

        if let Some(asset_positions) = data.get("assetPositions").and_then(|ap| ap.as_array()) {
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
                        continue;
                    }

                    let entry_price: f64 = pos_data
                        .get("entryPx")
                        .and_then(|p| p.as_str())
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);

                    let unrealized_pnl: f64 = pos_data
                        .get("unrealizedPnl")
                        .and_then(|p| p.as_str())
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);

                    positions.push(PositionInfo {
                        coin,
                        size: szi,
                        entry_price,
                        unrealized_pnl,
                    });
                }
            }
        }

        info!("Fetched {} positions", positions.len());
        Ok(positions)
    }

    /// Execute exchange action with rate limit retry
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

            if resp.status() == 429 {
                if attempt < max_retries {
                    warn!("Rate limited (429), retrying in {}ms", delay_ms);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms *= 2;
                    continue;
                }
                return Err(BotError::Order("Rate limited after max retries".to_string()));
            }

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(BotError::Order(format!(
                    "Exchange failed: HTTP {} — {}",
                    status, &body[..body.len().min(200)]
                )));
            }

            let exchange_resp: ExchangeResponse = resp
                .json()
                .await
                .map_err(|e| BotError::Order(format!("Exchange parse failed: {}", e)))?;

            if exchange_resp.status == "rate_limit" {
                if attempt < max_retries {
                    warn!("Rate limited (response), retrying in {}ms", delay_ms);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms *= 2;
                    continue;
                }
                return Err(BotError::Order("Rate limited after max retries".to_string()));
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
                let errors: Vec<String> = statuses.iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        s.get("error").and_then(|e| e.as_str())
                            .map(|msg| format!("[{}] {}", i, msg))
                    })
                    .collect();
                if !errors.is_empty() {
                    return Err(BotError::Order(format!("Order errors: {}", errors.join("; "))));
                }
            }

            return Ok(data);
        }

        Err(BotError::Order("Max retries exceeded".to_string()))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TradeRaw {
    pub coin: String,
    pub side: String,
    pub px: String,
    pub sz: String,
    pub time: u64,
    pub hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PositionInfo {
    pub coin: String,
    pub size: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
}

/// Extended asset info (size decimals, max leverage)
#[derive(Debug, Clone)]
pub struct AssetInfo {
    pub name: String,
    pub index: u32,
    pub sz_decimals: u32,
    pub max_leverage: u32,
}
