use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub wallet: WalletConfig,
    pub hyperliquid: HyperliquidConfig,
    pub trading: TradingConfig,
    pub risk: RiskConfig,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default)]
    pub macro_news: MacroNewsConfig,
    #[serde(default = "default_database_url")]
    pub database_url: String,
}

fn default_database_url() -> String {
    "sqlite:signalflow.db?mode=rwc".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct WalletConfig {
    /// Hex private key (with or without 0x prefix)
    pub private_key: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HyperliquidConfig {
    pub base_url: String,
    pub ws_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TradingConfig {
    /// Trading pair, e.g. "SPX"
    pub pair: String,
    /// Account equity in USD for position sizing
    pub account_size_usd: f64,
    /// Minimum position size in USD
    pub min_position_usd: f64,
    /// Poll interval for funding rate in seconds
    pub funding_poll_secs: u64,
    /// OHLCV poll interval in seconds
    pub ohlcv_poll_secs: u64,
    /// Dry run mode (no real orders)
    pub dry_run: bool,
    /// Maximum leverage (SPX max 8x)
    #[serde(default = "default_max_leverage")]
    pub max_leverage: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskConfig {
    /// Risk per trade as fraction of equity (0.0075 = 0.75%)
    pub risk_per_trade: f64,
    /// ATR multiplier for stop loss
    pub atr_sl_mult: f64,
    /// TP = tp_mult * SL distance
    pub tp_mult: f64,
    /// Maximum concurrent open positions
    pub max_open_positions: usize,
    /// Maximum daily loss in USD before halting
    #[serde(default = "default_max_daily_loss")]
    pub max_daily_loss_usd: f64,
    /// Maximum weekly loss in USD before halting
    #[serde(default = "default_max_weekly_loss")]
    pub max_weekly_loss_usd: f64,
    /// Maximum total exposure in USD
    #[serde(default = "default_max_exposure")]
    pub max_total_exposure_usd: f64,
    /// Enable equity curve protection (stop if DD > threshold)
    #[serde(default)]
    pub equity_curve_protection: bool,
    /// Equity drawdown threshold % to trigger protection
    #[serde(default = "default_dd_threshold")]
    pub equity_dd_threshold_pct: f64,
}

/// Strategy filters configuration
#[derive(Debug, Deserialize, Clone)]
pub struct FilterConfig {
    /// Maximum ATR as % of price — skip trade if volatility too high
    #[serde(default = "default_max_atr_pct")]
    pub max_atr_pct: f64,
    /// Minimum ATR as % of price — skip trade if volatility too low
    #[serde(default = "default_min_atr_pct")]
    pub min_atr_pct: f64,
    /// Prefer trades during NY session (UTC hours 13-21)
    #[serde(default)]
    pub session_filter_enabled: bool,
    /// Confidence penalty (0-30) for trades outside preferred session
    #[serde(default = "default_session_penalty")]
    pub session_penalty: u32,
    /// Start of preferred session (UTC hour, inclusive)
    #[serde(default = "default_session_start")]
    pub session_start_utc: u32,
    /// End of preferred session (UTC hour, inclusive)
    #[serde(default = "default_session_end")]
    pub session_end_utc: u32,
    /// Maximum spread in bps to accept signals (default 3.0 for mainnet, raise for testnet)
    #[serde(default = "default_max_spread_bps")]
    pub max_spread_bps: f64,
    /// Minimum orderbook imbalance ratio (default 0.8)
    #[serde(default = "default_min_imbalance")]
    pub min_imbalance: f64,
    /// Maximum orderbook imbalance ratio (default 1.2)
    #[serde(default = "default_max_imbalance")]
    pub max_imbalance: f64,
}

/// Macro news filter config (Finnhub Economic Calendar)
#[derive(Debug, Deserialize, Clone)]
pub struct MacroNewsConfig {
    /// Enable macro news filter
    #[serde(default)]
    pub enabled: bool,
    /// Finnhub API key (get free at https://finnhub.io)
    #[serde(default)]
    pub finnhub_api_key: String,
    /// Block trading N hours before/after HIGH impact events
    #[serde(default = "default_block_hours_high")]
    pub block_hours_high: i64,
    /// Block trading N hours before/after MEDIUM impact events
    #[serde(default = "default_block_hours_medium")]
    pub block_hours_medium: i64,
}

// Defaults
fn default_block_hours_high() -> i64 { 2 }
fn default_block_hours_medium() -> i64 { 1 }
fn default_max_leverage() -> u32 { 10 }
fn default_max_daily_loss() -> f64 { 50.0 }
fn default_max_weekly_loss() -> f64 { 150.0 }
fn default_max_exposure() -> f64 { 250.0 }
fn default_dd_threshold() -> f64 { 8.0 }
fn default_max_atr_pct() -> f64 { 2.5 }
fn default_min_atr_pct() -> f64 { 0.05 }
fn default_session_penalty() -> u32 { 10 }
fn default_session_start() -> u32 { 13 }
fn default_session_end() -> u32 { 21 }
fn default_max_spread_bps() -> f64 { 3.0 }
fn default_min_imbalance() -> f64 { 0.8 }
fn default_max_imbalance() -> f64 { 1.2 }

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            max_atr_pct: default_max_atr_pct(),
            min_atr_pct: default_min_atr_pct(),
            session_filter_enabled: false,
            session_penalty: default_session_penalty(),
            session_start_utc: default_session_start(),
            session_end_utc: default_session_end(),
            max_spread_bps: default_max_spread_bps(),
            min_imbalance: default_min_imbalance(),
            max_imbalance: default_max_imbalance(),
        }
    }
}

impl Default for MacroNewsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            finnhub_api_key: String::new(),
            block_hours_high: default_block_hours_high(),
            block_hours_medium: default_block_hours_medium(),
        }
    }
}

impl Config {
    pub fn load(path: &str) -> crate::error::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| crate::error::BotError::Config(format!("Cannot read {}: {}", path, e)))?;
        let config: Config = toml::from_str(&content)
            .map_err(|e| crate::error::BotError::Config(format!("Parse error: {}", e)))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> crate::error::Result<()> {
        if self.risk.risk_per_trade <= 0.0 || self.risk.risk_per_trade > 0.05 {
            return Err(crate::error::BotError::Config(
                "risk_per_trade must be > 0 and <= 0.05 (5%)".into(),
            ));
        }
        if self.trading.max_leverage > 50 {
            return Err(crate::error::BotError::Config(
                "max_leverage seems dangerously high (>50)".into(),
            ));
        }
        if self.filters.max_atr_pct <= self.filters.min_atr_pct {
            return Err(crate::error::BotError::Config(
                "max_atr_pct must be greater than min_atr_pct".into(),
            ));
        }
        if self.trading.account_size_usd <= 0.0 {
            return Err(crate::error::BotError::Config(
                "account_size_usd must be > 0".into(),
            ));
        }
        if self.trading.min_position_usd <= 0.0 {
            return Err(crate::error::BotError::Config(
                "min_position_usd must be > 0".into(),
            ));
        }
        if self.trading.funding_poll_secs == 0 {
            return Err(crate::error::BotError::Config(
                "funding_poll_secs must be > 0".into(),
            ));
        }
        if self.risk.max_open_positions == 0 {
            return Err(crate::error::BotError::Config(
                "max_open_positions must be > 0".into(),
            ));
        }
        if self.risk.max_daily_loss_usd <= 0.0 {
            return Err(crate::error::BotError::Config(
                "max_daily_loss_usd must be > 0".into(),
            ));
        }
        if self.risk.max_weekly_loss_usd <= 0.0 {
            return Err(crate::error::BotError::Config(
                "max_weekly_loss_usd must be > 0".into(),
            ));
        }
        if self.risk.equity_dd_threshold_pct <= 0.0 || self.risk.equity_dd_threshold_pct > 100.0 {
            return Err(crate::error::BotError::Config(
                "equity_dd_threshold_pct must be between 0 and 100".into(),
            ));
        }
        if self.filters.session_penalty > 100 {
            return Err(crate::error::BotError::Config(
                "session_penalty must be <= 100".into(),
            ));
        }
        if self.macro_news.block_hours_high < self.macro_news.block_hours_medium {
            return Err(crate::error::BotError::Config(
                "block_hours_high must be >= block_hours_medium".into(),
            ));
        }
        Ok(())
    }
}
