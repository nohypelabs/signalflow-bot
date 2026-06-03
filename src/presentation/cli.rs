use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};

use crate::application::{Config, TradingService};
use crate::error::Result;
use crate::infrastructure::{HyperliquidClient, SodexClient, Wallet};

/// Run the trading bot
pub async fn run(config_path: &Path) -> Result<()> {
    // Load config
    let config = Config::load(config_path)?;
    log_config(&config);

    // Initialize components
    let wallet = Arc::new(Wallet::new(&config.wallet.private_key)?);
    let hyperliquid = Arc::new(
        HyperliquidClient::new(&config.hyperliquid.base_url, &config.hyperliquid.ws_url).await?,
    );
    let sodex = Arc::new(SodexClient::new(
        &config.sodex.api_url,
        &config.sodex.api_key,
    ));

    // Fetch asset IDs
    hyperliquid.fetch_asset_ids().await?;

    // Start WebSocket stream
    hyperliquid.start_stream().await?;

    // Wait for WebSocket to connect and receive first data
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Create trading service
    let service = Arc::new(TradingService::new(
        hyperliquid.clone(),
        sodex,
        wallet.clone(),
        config.strategy.clone(),
        config.risk.clone(),
    ));

    // Sync existing positions from Hyperliquid on startup
    match hyperliquid.fetch_positions(wallet.address()).await {
        Ok(positions) => {
            if !positions.is_empty() {
                service.load_positions(positions).await;
                info!("📊 {}", service.position_summary().await);
            }
        }
        Err(e) => {
            tracing::warn!("Could not sync positions from exchange: {}", e);
        }
    }

    // Graceful shutdown
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Shutdown signal received");
        let _ = shutdown_tx_clone.send(true);
    });

    // Main loop
    info!("🚀 Starting trading loop...");
    let interval = Duration::from_secs(config.strategy.poll_interval_secs);
    let mut ticker = tokio::time::interval(interval);
    let mut tick = 0u64;

    // Skip first immediate tick
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                tick += 1;

                if *shutdown_rx.borrow() {
                    break;
                }

                debug!("Tick {}", tick);

                // Execute trading tick
                match service.tick().await {
                    Ok(results) => {
                        if !results.is_empty() {
                            info!("Tick {}: {} trades executed", tick, results.len());
                        }
                    }
                    Err(e) => {
                        error!("Tick {} error: {}", tick, e);
                    }
                }

                // Periodic status log
                if tick.is_multiple_of(60) {
                    info!("📊 Tick {} | {}", tick, service.position_summary().await);
                }
            }
            _ = shutdown_rx.changed() => {
                break;
            }
        }
    }

    // Cleanup
    info!("🛑 Bot stopped");
    info!("Final: {}", service.position_summary().await);

    Ok(())
}

fn log_config(config: &Config) {
    info!("🚀 SignalFlow Bot v0.2.0");
    info!("  Wallet: {}...", &config.wallet.private_key[..10]);
    info!("  Hyperliquid: {}", config.hyperliquid.base_url);
    info!(
        "  Strategy: {}x leverage, ${} position, {}s poll",
        config.strategy.max_leverage,
        config.strategy.max_position_size,
        config.strategy.poll_interval_secs
    );
    info!(
        "  Risk: {}% SL, {}% TP, ${} daily loss",
        config.risk.stop_loss_pct, config.risk.take_profit_pct, config.risk.max_daily_loss
    );
    info!("  Dry run: {}", config.strategy.dry_run);
}
