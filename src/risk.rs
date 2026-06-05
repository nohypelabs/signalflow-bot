use chrono::{Datelike, Utc};
use log::{info, warn};
use std::collections::VecDeque;

use crate::config::RiskConfig;
use crate::decision::{Signal, SignalDirection};

/// Calculated position details
#[derive(Debug, Clone)]
pub struct PositionPlan {
    pub pair: String,
    pub direction: SignalDirection,
    pub size_units: f64,
    pub size_usd: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub entry_price: f64,
    pub risk_usd: f64,
}

/// Risk manager — position sizing with fixed fractional risk + daily/weekly loss limits
pub struct RiskManager {
    config: RiskConfig,
    account_size_usd: f64,
    open_positions: usize,

    // P&L tracking
    daily_realized_pnl: f64,
    weekly_realized_pnl: f64,
    total_realized_pnl: f64,
    current_day: u32,    // day of year
    current_year: i32,   // year (for boundary detection)
    current_week: u32,   // ISO week number
    current_week_year: i32, // ISO year (for boundary detection)

    // Equity curve tracking
    equity_history: VecDeque<(u64, f64)>, // (timestamp, equity)

    // Halt state
    daily_halt: bool,
    weekly_halt: bool,
    dd_halt: bool,
    // Cooldown: last time dd_halt was set (allow recovery after 1 hour)
    dd_halt_time: Option<i64>,
}

impl RiskManager {
    pub fn new(config: RiskConfig, account_size_usd: f64) -> Self {
        let now = Utc::now();
        let mut equity_history = VecDeque::new();
        equity_history.push_back((now.timestamp_millis() as u64, account_size_usd));
        Self {
            config,
            account_size_usd,
            open_positions: 0,
            daily_realized_pnl: 0.0,
            weekly_realized_pnl: 0.0,
            total_realized_pnl: 0.0,
            current_day: now.ordinal(),
            current_year: now.year(),
            current_week: now.iso_week().week(),
            current_week_year: now.iso_week().year(),
            equity_history,
            daily_halt: false,
            weekly_halt: false,
            dd_halt: false,
            dd_halt_time: None,
        }
    }

    /// Update open position count
    pub fn set_open_positions(&mut self, count: usize) {
        self.open_positions = count;
    }

    /// Check if we can open another position (considers all limits)
    pub fn can_open(&self) -> bool {
        self.open_positions < self.config.max_open_positions
            && !self.daily_halt
            && !self.weekly_halt
            && !self.dd_halt
    }

    /// Record a realized trade result (call after each closed trade)
    pub fn record_trade(&mut self, pnl_usd: f64) {
        // Auto-reset daily PnL on new day (compare year + day to handle year boundary)
        let now = Utc::now();
        let day = now.ordinal();
        let year = now.year();
        if day != self.current_day || year != self.current_year {
            info!("New day — resetting daily PnL (was ${:.2})", self.daily_realized_pnl);
            self.daily_realized_pnl = 0.0;
            self.current_day = day;
            self.current_year = year;
            self.daily_halt = false;
        }

        // Auto-reset weekly PnL on new ISO week (compare year + week to handle year boundary)
        let iso_week = now.iso_week().week();
        let iso_year = now.iso_week().year();
        if iso_week != self.current_week || iso_year != self.current_week_year {
            info!("New week — resetting weekly PnL (was ${:.2})", self.weekly_realized_pnl);
            self.weekly_realized_pnl = 0.0;
            self.current_week = iso_week;
            self.current_week_year = iso_year;
            self.weekly_halt = false;
        }

        // Check if dd_halt cooldown has expired (1 hour)
        if self.dd_halt {
            if let Some(halt_time) = self.dd_halt_time {
                let elapsed_secs = (now.timestamp_millis() - halt_time) / 1000;
                if elapsed_secs > 3600 {
                    info!("DD halt cooldown expired ({}s), resetting", elapsed_secs);
                    self.dd_halt = false;
                    self.dd_halt_time = None;
                }
            }
        }

        self.daily_realized_pnl += pnl_usd;
        self.weekly_realized_pnl += pnl_usd;
        self.total_realized_pnl += pnl_usd;

        // Update account size based on cumulative P&L
        self.account_size_usd += pnl_usd;

        // Update equity curve (use total P&L, not weekly)
        let current_equity = self.account_size_usd;
        self.equity_history.push_back((now.timestamp_millis() as u64, current_equity));
        // Keep last 7 days of equity snapshots (at most ~10080 at 1/min)
        if self.equity_history.len() > 10_080 {
            self.equity_history.pop_front();
        }

        // Check daily loss limit
        if self.daily_realized_pnl <= -self.config.max_daily_loss_usd {
            self.daily_halt = true;
            warn!(
                "DAILY LOSS HALT: ${:.2} exceeded limit of -${:.2}",
                self.daily_realized_pnl, self.config.max_daily_loss_usd
            );
        }

        // Check weekly loss limit
        if self.weekly_realized_pnl <= -self.config.max_weekly_loss_usd {
            self.weekly_halt = true;
            warn!(
                "WEEKLY LOSS HALT: ${:.2} exceeded limit of -${:.2}",
                self.weekly_realized_pnl, self.config.max_weekly_loss_usd
            );
        }

        // Check equity curve drawdown
        if self.config.equity_curve_protection {
            self.check_equity_drawdown();
        }

        info!(
            "Trade recorded: PnL=${:.2} | Daily=${:.2} | Weekly=${:.2} | Halt: D={} W={} DD={}",
            pnl_usd, self.daily_realized_pnl, self.weekly_realized_pnl,
            self.daily_halt, self.weekly_halt, self.dd_halt
        );
    }

    /// Check equity curve drawdown over last 7 days
    fn check_equity_drawdown(&mut self) {
        if self.equity_history.len() < 2 {
            return;
        }

        let current_equity = self.equity_history.back().map(|(_, e)| *e).unwrap_or(self.account_size_usd);

        // Find peak equity in last 7 days
        let seven_days_ago = Utc::now().timestamp_millis() as u64 - 7 * 24 * 3600 * 1000;
        let peak = self.equity_history
            .iter()
            .filter(|(ts, _)| *ts >= seven_days_ago)
            .map(|(_, e)| *e)
            .fold(0.0_f64, f64::max);

        if peak <= 0.0 {
            return;
        }

        let dd_pct = ((peak - current_equity) / peak) * 100.0;

        if dd_pct > self.config.equity_dd_threshold_pct {
            self.dd_halt = true;
            self.dd_halt_time = Some(Utc::now().timestamp_millis());
            warn!(
                "EQUITY CURVE HALT: Drawdown {:.1}% exceeded threshold {:.1}% (peak=${:.2} current=${:.2})",
                dd_pct, self.config.equity_dd_threshold_pct, peak, current_equity
            );
        }
    }

    /// Get risk status summary
    pub fn status(&self) -> RiskStatus {
        RiskStatus {
            daily_pnl: self.daily_realized_pnl,
            weekly_pnl: self.weekly_realized_pnl,
            daily_limit: self.config.max_daily_loss_usd,
            weekly_limit: self.config.max_weekly_loss_usd,
            daily_halt: self.daily_halt,
            weekly_halt: self.weekly_halt,
            dd_halt: self.dd_halt,
            open_positions: self.open_positions,
            max_positions: self.config.max_open_positions,
        }
    }

    /// Calculate position plan from signal
    ///
    /// - risk_usd = account_size * risk_per_trade (0.75%)
    /// - sl_distance = ATR * atr_sl_mult (1.5)
    /// - tp_distance = sl_distance * tp_mult (2.5)
    /// - size_units = risk_usd / sl_distance
    pub fn calculate(&self, signal: &Signal, entry_price: f64) -> Option<PositionPlan> {
        if !self.can_open() {
            if self.daily_halt {
                warn!("BLOCKED: Daily loss limit reached (${:.2})", self.daily_realized_pnl);
            } else if self.weekly_halt {
                warn!("BLOCKED: Weekly loss limit reached (${:.2})", self.weekly_realized_pnl);
            } else if self.dd_halt {
                warn!("BLOCKED: Equity drawdown protection triggered");
            } else {
                warn!("Max positions {} reached", self.config.max_open_positions);
            }
            return None;
        }

        if signal.atr <= 0.0 || entry_price <= 0.0 {
            warn!("Invalid ATR ({:.4}) or entry ({:.2})", signal.atr, entry_price);
            return None;
        }

        let risk_usd = self.account_size_usd * self.config.risk_per_trade;
        let sl_distance = signal.atr * self.config.atr_sl_mult;
        let tp_distance = sl_distance * self.config.tp_mult;

        let (stop_loss, take_profit) = match signal.direction {
            SignalDirection::Long => (entry_price - sl_distance, entry_price + tp_distance),
            SignalDirection::Short => (entry_price + sl_distance, entry_price - tp_distance),
        };

        let size_units = risk_usd / sl_distance;
        let size_usd = size_units * entry_price;

        // Check total exposure limit — actually cap the size
        let (size_units, size_usd) = if size_usd > self.config.max_total_exposure_usd {
            let capped_size = self.config.max_total_exposure_usd / entry_price;
            warn!(
                "Position size ${:.2} exceeds max exposure ${:.2}, capping to ${:.2}",
                size_usd, self.config.max_total_exposure_usd, self.config.max_total_exposure_usd
            );
            (capped_size, self.config.max_total_exposure_usd)
        } else {
            (size_units, size_usd)
        };

        // Validate stop loss and take profit are positive
        if stop_loss <= 0.0 || take_profit <= 0.0 {
            warn!("Invalid SL={:.2} or TP={:.2} — skipping", stop_loss, take_profit);
            return None;
        }

        info!(
            "Position {}: {:?} entry={:.2} SL={:.2} TP={:.2} size={:.4} (${:.2}) risk=${:.2}",
            signal.pair, signal.direction, entry_price, stop_loss, take_profit, size_units, size_usd, risk_usd
        );

        Some(PositionPlan {
            pair: signal.pair.clone(),
            direction: signal.direction,
            size_units,
            size_usd,
            stop_loss,
            take_profit,
            entry_price,
            risk_usd,
        })
    }
}

/// Risk status for logging/monitoring
#[derive(Debug)]
pub struct RiskStatus {
    pub daily_pnl: f64,
    pub weekly_pnl: f64,
    pub daily_limit: f64,
    pub weekly_limit: f64,
    pub daily_halt: bool,
    pub weekly_halt: bool,
    pub dd_halt: bool,
    pub open_positions: usize,
    pub max_positions: usize,
}

impl std::fmt::Display for RiskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Daily: ${:.2}/${:.2} [{}] | Weekly: ${:.2}/${:.2} [{}] | DD: [{}] | Pos: {}/{}",
            self.daily_pnl, self.daily_limit, if self.daily_halt { "HALT" } else { "OK" },
            self.weekly_pnl, self.weekly_limit, if self.weekly_halt { "HALT" } else { "OK" },
            if self.dd_halt { "HALT" } else { "OK" },
            self.open_positions, self.max_positions,
        )
    }
}
