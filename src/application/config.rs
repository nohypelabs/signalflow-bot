use crate::error::{BotError, Result};
use serde::Deserialize;
use std::path::Path;

/// Environment variable name for database URL
const DATABASE_URL_ENV: &str = "DATABASE_URL";

/// Environment variable name for private key
const PRIVATE_KEY_ENV: &str = "SIGNALFLOW_PRIVATE_KEY";

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub wallet: WalletConfig,
    pub hyperliquid: HyperliquidConfig,
    pub sodex: SodexConfig,
    pub strategy: StrategyConfig,
    pub risk: RiskConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WalletConfig {
    pub private_key: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HyperliquidConfig {
    pub base_url: String,
    pub ws_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SodexConfig {
    pub api_url: String,
    pub api_key: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StrategyConfig {
    pub max_position_size: f64,
    pub max_leverage: u32,
    pub funding_rate_threshold: f64,
    pub poll_interval_secs: u64,
    pub dry_run: bool,
    #[serde(default = "default_trade_log_path")]
    pub trade_log_path: String,
    #[serde(default = "default_database_url")]
    pub database_url: String,
}

fn default_trade_log_path() -> String {
    "trades.jsonl".to_string()
}

fn default_database_url() -> String {
    "sqlite:signalflow.db?mode=rwc".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskConfig {
    pub max_total_exposure: f64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub max_daily_loss: f64,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| BotError::Config(format!("Failed to read config: {}", e)))?;
        let mut config: Config = toml::from_str(&content)
            .map_err(|e| BotError::Config(format!("Failed to parse config: {}", e)))?;

        // Override private key from environment variable if set
        if let Ok(env_key) = std::env::var(PRIVATE_KEY_ENV) {
            if !env_key.is_empty() {
                config.wallet.private_key = env_key;
            }
        }

        // Override database URL from environment variable if set
        if let Ok(env_db) = std::env::var(DATABASE_URL_ENV) {
            if !env_db.is_empty() {
                config.strategy.database_url = env_db;
            }
        }

        // Validate private key format
        let key = config
            .wallet
            .private_key
            .strip_prefix("0x")
            .unwrap_or(&config.wallet.private_key);
        if key.len() != 64 {
            return Err(BotError::Config(format!(
                "Private key must be 32 bytes (64 hex chars), got {} chars",
                key.len()
            )));
        }

        // Validate URLs
        if config.hyperliquid.base_url.is_empty() {
            return Err(BotError::Config(
                "Hyperliquid base_url is empty".to_string(),
            ));
        }
        if config.hyperliquid.ws_url.is_empty() {
            return Err(BotError::Config("Hyperliquid ws_url is empty".to_string()));
        }

        // Validate strategy values
        if config.strategy.max_position_size <= 0.0 {
            return Err(BotError::Config(
                "max_position_size must be > 0".to_string(),
            ));
        }
        if config.strategy.max_leverage == 0 {
            return Err(BotError::Config("max_leverage must be > 0".to_string()));
        }
        if config.strategy.poll_interval_secs == 0 {
            return Err(BotError::Config(
                "poll_interval_secs must be > 0".to_string(),
            ));
        }

        // Validate risk values
        if config.risk.max_total_exposure <= 0.0 {
            return Err(BotError::Config(
                "max_total_exposure must be > 0".to_string(),
            ));
        }
        if config.risk.stop_loss_pct <= 0.0 || config.risk.stop_loss_pct > 100.0 {
            return Err(BotError::Config(
                "stop_loss_pct must be between 0 and 100".to_string(),
            ));
        }
        if config.risk.take_profit_pct <= 0.0 || config.risk.take_profit_pct > 100.0 {
            return Err(BotError::Config(
                "take_profit_pct must be between 0 and 100".to_string(),
            ));
        }
        if config.risk.max_daily_loss <= 0.0 {
            return Err(BotError::Config("max_daily_loss must be > 0".to_string()));
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_private_key_length() {
        let config_str = r#"
[wallet]
private_key = "0x1234"

[hyperliquid]
base_url = "https://api.hyperliquid.xyz"
ws_url = "wss://api.hyperliquid.xyz/ws"

[sodex]
api_url = "https://api.sodex.io"
api_key = "test"

[strategy]
max_position_size = 100.0
max_leverage = 20
funding_rate_threshold = 0.01
poll_interval_secs = 30
dry_run = true

[risk]
max_total_exposure = 500.0
stop_loss_pct = 1.5
take_profit_pct = 3.0
max_daily_loss = 50.0
"#;

        std::fs::write("/tmp/test_config_invalid.toml", config_str).unwrap();
        let result = Config::load(Path::new("/tmp/test_config_invalid.toml"));
        assert!(result.is_err());
        std::fs::remove_file("/tmp/test_config_invalid.toml").unwrap();
    }
}
