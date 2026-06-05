mod config;
mod error;
mod signer;
mod wallet;
mod hl_rest;
mod hl_ws;
mod orderbook;
mod orderflow;
mod funding;
mod ta;
mod decision;
mod risk;
mod execution;
mod macro_filter;
mod domain;
mod store;
mod lesson;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use log::{info, error, warn, debug};

use config::Config;
use hl_rest::HyperliquidRest;
use hl_ws::{HyperliquidWs, WsSubscription};
use orderbook::OrderbookAnalyzer;
use orderflow::OrderflowAnalyzer;
use funding::FundingMonitor;
use ta::TaEngine;
use decision::DecisionEngine;
use risk::RiskManager;
use execution::Executor;
use signer::Signer;
use wallet::Wallet;
use macro_filter::MacroFilter;

/// Tracked position state (for PnL detection)
#[derive(Debug, Clone)]
struct TrackedPosition {
    entry_price: f64,
    size: f64,
    unrealized_pnl: f64,
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    info!("=== SignalFlow Bot v0.4.0 ===");

    // Load config
    let config = Config::load("config.toml").expect("Failed to load config.toml");
    info!(
        "Pair: {} | Account: ${:.0} | Dry run: {}",
        config.trading.pair, config.trading.account_size_usd, config.trading.dry_run
    );

    // Init wallet & signer
    let wallet = Wallet::new(&config.wallet.private_key).expect("Invalid private key");
    let signer = Signer::new();

    // Init REST client
    let rest = HyperliquidRest::new(&config.hyperliquid.base_url);

    // Fetch asset metadata
    let asset_ids = rest.fetch_meta().await.expect("Failed to fetch asset metadata");
    info!("Loaded {} asset IDs", asset_ids.len());

    // Fetch extended asset info (sz_decimals, max_leverage)
    let asset_infos = rest.fetch_asset_info().await.expect("Failed to fetch asset info");
    let asset_info_map: std::collections::HashMap<String, hl_rest::AssetInfo> =
        asset_infos.into_iter().map(|a| (a.name.clone(), a)).collect();
    if let Some(info) = asset_info_map.get(&config.trading.pair) {
        info!("{}: sz_decimals={}, max_leverage={}x", config.trading.pair, info.sz_decimals, info.max_leverage);
    }

    // Shared state
    let orderbook = Arc::new(Mutex::new(OrderbookAnalyzer::new()));
    let orderflow = Arc::new(Mutex::new(OrderflowAnalyzer::new(60_000)));
    let ta = Arc::new(Mutex::new(TaEngine::new(500)));
    let funding = Arc::new(Mutex::new(FundingMonitor::new(
        HyperliquidRest::new(&config.hyperliquid.base_url),
        config.trading.funding_poll_secs,
    )));

    let decision_engine = DecisionEngine::new(
        70,                                              // min_score
        config.filters.max_spread_bps,                   // max_spread_bps (configurable)
        config.filters.min_imbalance,                    // min_imbalance (configurable)
        config.filters.max_imbalance,                    // max_imbalance (configurable)
        config.trading.min_position_usd * 1.5,           // min_depth_usd
        config.filters.clone(),
    );

    let risk_manager = Arc::new(Mutex::new(RiskManager::new(
        config.risk.clone(),
        config.trading.account_size_usd,
    )));

    // Init SQLite store + Lesson Engine
    let (store, lesson_engine) = match store::SqliteStore::new(&config.database_url).await {
        Ok(s) => {
            let s = Arc::new(s);
            // Use TradeStore trait to call init()
            use domain::store::TradeStore;
            if let Err(e) = s.init().await {
                warn!("SQLite init failed: {} (journaling disabled)", e);
            } else {
                info!("SQLite store initialized: {}", config.database_url);
            }
            let le = Arc::new(lesson::LessonEngine::new(
                s.clone(),
                config.risk.atr_sl_mult,
                config.risk.tp_mult,
            ));
            (Some(s), Some(le))
        }
        Err(e) => {
            warn!("SQLite connection failed: {} (journaling disabled)", e);
            (None, None)
        }
    };

    let executor = Executor::new(
        HyperliquidRest::new(&config.hyperliquid.base_url),
        signer,
        wallet,
        asset_ids.clone(),
        asset_info_map.clone(),
        config.trading.dry_run,
    );

    // Track known positions for PnL detection
    let known_positions: Arc<Mutex<HashMap<String, TrackedPosition>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Position sync counter (sync every N signal evaluations to avoid rate limit)
    let mut signal_count: u64 = 0;
    let position_sync_interval: u64 = 5; // sync positions every 5 signals

    // Start WebSocket
    let pair = config.trading.pair.clone();
    let ws = HyperliquidWs::new(&config.hyperliquid.ws_url);
    let mut ws_rx = ws
        .subscribe(vec![
            WsSubscription::L2Book { coin: pair.clone() },
            WsSubscription::Trades { coin: pair.clone() },
        ])
        .await
        .expect("WebSocket subscribe failed");

    // Spawn funding rate poller
    let funding_clone = funding.clone();
    let poll_secs = config.trading.funding_poll_secs;
    tokio::spawn(async move {
        loop {
            {
                let mut f = funding_clone.lock().await;
                f.poll().await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
        }
    });

    // Init macro news filter (Finnhub Economic Calendar)
    let macro_filter = if config.macro_news.enabled && !config.macro_news.finnhub_api_key.is_empty() {
        let mf = Arc::new(MacroFilter::new(
            config.macro_news.finnhub_api_key.clone(),
            config.macro_news.block_hours_high,
            config.macro_news.block_hours_medium,
        ));

        // Fetch events immediately on startup
        match mf.fetch_events().await {
            Ok(n) => info!("Macro filter loaded: {} events", n),
            Err(e) => warn!("Macro filter initial fetch failed: {} (will retry)", e),
        }

        // Spawn daily refresh poller (every 6 hours)
        let mf_clone = mf.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                mf_clone.maybe_refresh().await;
            }
        });

        Some(mf)
    } else {
        if config.macro_news.enabled {
            warn!("Macro news enabled but no finnhub_api_key — filter DISABLED");
        }
        None
    };

    // Initial position sync
    let wallet_addr = executor.wallet_address().to_string();
    let rest_for_sync = HyperliquidRest::new(&config.hyperliquid.base_url);
    let addr_clone = wallet_addr.clone();
    match rest_for_sync.fetch_positions(&addr_clone).await {
        Ok(positions) => {
            let count = positions.len();
            {
                let mut rm = risk_manager.lock().await;
                rm.set_open_positions(count);
            }
            {
                let mut kp = known_positions.lock().await;
                for p in &positions {
                    kp.insert(p.coin.clone(), TrackedPosition {
                        entry_price: p.entry_price,
                        size: p.size,
                        unrealized_pnl: p.unrealized_pnl,
                    });
                }
            }
            info!("Initial position sync: {} open positions", count);
        }
        Err(e) => warn!("Initial position sync failed: {} (will retry)", e),
    }

    info!("Bot started. Entering main loop...");
    info!("Filters: ATR [{:.1}%-{:.1}%] | Session: {} (UTC {}-{}) | Penalty: {}pts | MacroNews: {}",
        config.filters.min_atr_pct, config.filters.max_atr_pct,
        if config.filters.session_filter_enabled { "ON" } else { "OFF" },
        config.filters.session_start_utc, config.filters.session_end_utc,
        config.filters.session_penalty,
        if macro_filter.is_some() { "ON" } else { "OFF" },
    );

    // Main event loop
    loop {
        tokio::select! {
            msg = ws_rx.recv() => {
                let msg = match msg {
                    Some(m) => m,
                    None => {
                        error!("WebSocket channel closed — bot is deaf to market data! Shutting down.");
                        break;
                    }
                };
                match msg.channel.as_str() {
                    "l2Book" => {
                        let mut ob = orderbook.lock().await;
                        ob.update(&msg.data);
                    }
                    "trades" => {
                        // Feed trades into orderflow
                        {
                            let mut of = orderflow.lock().await;
                            of.on_ws_message(&msg.data);
                        }
                        // Feed trades into TA engine
                        if let Some(trades) = msg.data.as_array() {
                            let mut ta_g = ta.lock().await;
                            for t in trades {
                                let px = t.get("px").and_then(|p| p.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                let sz = t.get("sz").and_then(|s| s.as_str()).unwrap_or("0").parse::<f64>().unwrap_or(0.0);
                                let time = t.get("time").and_then(|t| t.as_u64()).unwrap_or(0);
                                if px > 0.0 && sz > 0.0 {
                                    ta_g.on_trade(px, sz, time);
                                }
                            }
                        }
                    }
                    _ => {}
                }

                // Evaluate signal
                let signal = {
                    let ob = orderbook.lock().await;
                    let of = orderflow.lock().await;
                    let f = funding.lock().await;
                    let t = ta.lock().await;
                    decision_engine.evaluate_signal(&pair, &ob, &of, &f, &t)
                };

                if let Some(signal) = signal {
                    signal_count += 1;

                    // Log signal to database for research
                    let signal_id = if let Some(ref s) = store {
                        s.log_signal(
                            &signal.pair,
                            &format!("{:?}", signal.direction),
                            signal.score,
                            signal.ema9,
                            signal.ema21,
                            signal.rsi,
                            signal.atr,
                            signal.atr_pct,
                            signal.spread_bps,
                            signal.imbalance,
                            signal.cvd,
                            signal.funding_rate,
                            signal.in_session,
                            false, // not yet executed
                            None,
                        ).await.ok()
                    } else {
                        None
                    };

                    // Macro news filter — block if high-impact event nearby
                    if let Some(ref mf) = macro_filter {
                        if let Some(reason) = mf.should_block().await {
                            info!("{}: REJECT macro news — {}", pair, reason);
                            // Mark signal as rejected in DB
                            if let (Some(ref s), Some(sid)) = (&store, signal_id) {
                                let _ = s.mark_signal_rejected(sid, &format!("macro_news: {}", reason)).await;
                            }
                            continue;
                        }
                    }

                    // Periodic position sync (every N signals)
                    if signal_count % position_sync_interval == 0 {
                        let rest_sync = HyperliquidRest::new(&config.hyperliquid.base_url);
                        let addr = wallet_addr.clone();
                        let kp = known_positions.clone();
                        let rm = risk_manager.clone();

                        match rest_sync.fetch_positions(&addr).await {
                            Ok(positions) => {
                                let count = positions.len();
                                {
                                    let mut rm_g = rm.lock().await;
                                    rm_g.set_open_positions(count);
                                }

                                // Detect closed positions → record PnL
                                let mut kp_g = kp.lock().await;
                                let current_coins: std::collections::HashSet<String> =
                                    positions.iter().map(|p| p.coin.clone()).collect();

                                // Find positions that disappeared (were closed)
                                let closed_coins: Vec<String> = kp_g.keys()
                                    .filter(|k| !current_coins.contains(*k))
                                    .cloned()
                                    .collect();

                                for coin in closed_coins {
                                    if let Some(closed) = kp_g.remove(&coin) {
                                        info!(
                                            "Position closed: {} | entry={:.2} size={:.4} last_pnl={:.2}",
                                            coin, closed.entry_price, closed.size, closed.unrealized_pnl
                                        );
                                        let mut rm_g = rm.lock().await;
                                        rm_g.record_trade(closed.unrealized_pnl);

                                        // Feed to lesson engine
                                        if let Some(ref le) = lesson_engine {
                                            // Correct exit price: Long => entry + pnl/size, Short => entry - pnl/size
                                            let abs_size = closed.size.abs().max(1e-10);
                                            let exit_price = if closed.size > 0.0 {
                                                closed.entry_price + (closed.unrealized_pnl / abs_size)
                                            } else {
                                                closed.entry_price - (closed.unrealized_pnl / abs_size)
                                            };
                                            let trade = lesson::TradeResult {
                                                coin: coin.clone(),
                                                direction: if closed.size >= 0.0 { "Long".to_string() } else { "Short".to_string() },
                                                entry_price: closed.entry_price,
                                                exit_price,
                                                size: closed.size.abs(),
                                                pnl: closed.unrealized_pnl,
                                                signal_score: 70, // default — could be enriched
                                                atr_pct: 0.0,     // could be enriched
                                                in_session: true,  // could be enriched
                                            };
                                            match le.analyze_trade(&trade).await {
                                                Ok(Some(lesson)) => info!("Lesson learned: {}", lesson.cause.details),
                                                Ok(None) => debug!("No lesson from trade (breakeven)"),
                                                Err(e) => warn!("Lesson engine error: {}", e),
                                            }
                                        }
                                    }
                                }

                                // Update known positions with latest unrealized PnL
                                for p in &positions {
                                    kp_g.insert(p.coin.clone(), TrackedPosition {
                                        entry_price: p.entry_price,
                                        size: p.size,
                                        unrealized_pnl: p.unrealized_pnl,
                                    });
                                }

                                info!("Position sync: {} open", count);
                            }
                            Err(e) => warn!("Position sync failed: {}", e),
                        }
                    }

                    // Get entry price
                    let entry_price = {
                        let ob = orderbook.lock().await;
                        match signal.direction {
                            decision::SignalDirection::Long => ob.best_bid().unwrap_or(0.0),
                            decision::SignalDirection::Short => ob.best_ask().unwrap_or(0.0),
                        }
                    };

                    // Calculate position
                    let plan = {
                        let rm = risk_manager.lock().await;
                        rm.calculate(&signal, entry_price)
                    };

                    if let Some(plan) = plan {
                        let (best_bid, best_ask) = {
                            let ob = orderbook.lock().await;
                            (ob.best_bid().unwrap_or(0.0), ob.best_ask().unwrap_or(0.0))
                        };

                        match executor.execute_limit_order(&plan, best_bid, best_ask).await {
                            Ok(result) => {
                                info!("Order result: {}", result.message);

                                // Mark signal as executed in database
                                if let (Some(ref s), Some(sid)) = (&store, signal_id) {
                                    let _ = s.mark_signal_executed(sid).await;
                                }

                                // If filled, add to known positions
                                if result.filled() {
                                    let fill_px = result.fill_price.unwrap_or(entry_price);
                                    let fill_sz = result.filled_size.unwrap_or(plan.size_units);

                                    let mut kp = known_positions.lock().await;
                                    kp.insert(plan.pair.clone(), TrackedPosition {
                                        entry_price: fill_px,
                                        size: fill_sz,
                                        unrealized_pnl: 0.0,
                                    });
                                    info!("Tracking new position: {} @ {:.2} size={:.4}", plan.pair, fill_px, fill_sz);
                                }
                            }
                            Err(e) => error!("Execution error: {}", e),
                        }
                    }

                    // Log risk status periodically (every signal evaluation)
                    {
                        let rm = risk_manager.lock().await;
                        let status = rm.status();
                        if status.daily_halt || status.weekly_halt || status.dd_halt {
                            warn!("RISK STATUS: {}", status);
                        }
                    }

                    // Log lesson stats periodically (every 10 signals)
                    if signal_count % 10 == 0 {
                        if let Some(ref le) = lesson_engine {
                            let summary = le.stats_summary().await;
                            info!("LESSON STATS: {}", summary);
                        }
                    }
                }
            }

            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received");
                break;
            }
        }
    }

    info!("Bot stopped.");
}
