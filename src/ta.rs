
/// OHLCV candle
#[derive(Debug, Clone)]
pub struct Candle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub timestamp_ms: u64,
}

/// Technical analysis engine — maintains OHLCV candles and computes indicators
pub struct TaEngine {
    /// 1-minute candles
    candles_1m: Vec<Candle>,
    /// 5-minute candles
    candles_5m: Vec<Candle>,
    /// Maximum candles to keep
    max_candles: usize,
}

impl TaEngine {
    pub fn new(max_candles: usize) -> Self {
        Self {
            candles_1m: Vec::new(),
            candles_5m: Vec::new(),
            max_candles,
        }
    }

    /// Update candles from a new trade (builds 1m candles, aggregates to 5m)
    pub fn on_trade(&mut self, price: f64, size: f64, timestamp_ms: u64) {
        // Guard against NaN/Inf inputs
        if !price.is_finite() || !size.is_finite() || price <= 0.0 || size <= 0.0 {
            return;
        }
        // Align to 1-minute boundary
        let candle_start = (timestamp_ms / 60_000) * 60_000;

        if let Some(last) = self.candles_1m.last_mut() {
            if last.timestamp_ms == candle_start {
                // Update existing candle
                last.high = last.high.max(price);
                last.low = last.low.min(price);
                last.close = price;
                last.volume += size;
                return;
            }
        }

        // New candle
        self.candles_1m.push(Candle {
            open: price,
            high: price,
            low: price,
            close: price,
            volume: size,
            timestamp_ms: candle_start,
        });

        // Trim old candles
        if self.candles_1m.len() > self.max_candles {
            self.candles_1m.remove(0);
        }

        // Rebuild 5m candles from 1m candles
        self.rebuild_5m();
    }

    /// Rebuild 5-minute candles from 1-minute candles
    fn rebuild_5m(&mut self) {
        self.candles_5m.clear();

        let mut i = 0;
        while i < self.candles_1m.len() {
            let bucket_start = (self.candles_1m[i].timestamp_ms / 300_000) * 300_000;
            let mut candle = Candle {
                open: self.candles_1m[i].open,
                high: self.candles_1m[i].high,
                low: self.candles_1m[i].low,
                close: self.candles_1m[i].close,
                volume: self.candles_1m[i].volume,
                timestamp_ms: bucket_start,
            };

            i += 1;
            while i < self.candles_1m.len()
                && (self.candles_1m[i].timestamp_ms / 300_000) * 300_000 == bucket_start
            {
                candle.high = candle.high.max(self.candles_1m[i].high);
                candle.low = candle.low.min(self.candles_1m[i].low);
                candle.close = self.candles_1m[i].close;
                candle.volume += self.candles_1m[i].volume;
                i += 1;
            }

            self.candles_5m.push(candle);
        }
    }

    /// Calculate Exponential Moving Average from close prices
    /// EMA_today = price * k + EMA_yesterday * (1 - k), where k = 2 / (period + 1)
    pub fn ema(&self, period: usize, use_5m: bool) -> Option<f64> {
        let candles = if use_5m {
            &self.candles_5m
        } else {
            &self.candles_1m
        };

        if candles.len() < period {
            return None;
        }

        let k = 2.0 / (period as f64 + 1.0);

        // Seed with SMA of first `period` candles
        let mut ema: f64 = candles[..period].iter().map(|c| c.close).sum::<f64>() / period as f64;

        // Apply EMA formula for remaining candles
        for candle in candles[period..].iter() {
            ema = candle.close * k + ema * (1.0 - k);
        }

        Some(ema)
    }

    /// EMA 9 (1m candles)
    pub fn ema9(&self) -> Option<f64> {
        self.ema(9, false)
    }

    /// EMA 21 (1m candles)
    pub fn ema21(&self) -> Option<f64> {
        self.ema(21, false)
    }

    /// RSI 14 from 1m close prices
    /// RSI = 100 - (100 / (1 + RS)), RS = avg_gain / avg_loss
    pub fn rsi14(&self) -> Option<f64> {
        self.rsi(14, false)
    }

    /// Calculate RSI
    pub fn rsi(&self, period: usize, use_5m: bool) -> Option<f64> {
        let candles = if use_5m {
            &self.candles_5m
        } else {
            &self.candles_1m
        };

        if candles.len() < period + 1 {
            return None;
        }

        let mut gains = Vec::new();
        let mut losses = Vec::new();

        for i in 1..candles.len() {
            let change = candles[i].close - candles[i - 1].close;
            if change >= 0.0 {
                gains.push(change);
                losses.push(0.0);
            } else {
                gains.push(0.0);
                losses.push(-change);
            }
        }

        if gains.len() < period {
            return None;
        }

        // Initial SMA for first `period` values
        let mut avg_gain: f64 = gains[..period].iter().sum::<f64>() / period as f64;
        let mut avg_loss: f64 = losses[..period].iter().sum::<f64>() / period as f64;

        // Smoothed (Wilder's method) for remaining
        for i in period..gains.len() {
            avg_gain = (avg_gain * (period as f64 - 1.0) + gains[i]) / period as f64;
            avg_loss = (avg_loss * (period as f64 - 1.0) + losses[i]) / period as f64;
        }

        if avg_loss < 1e-10 {
            return Some(100.0); // No losses = RSI 100
        }

        let rs = avg_gain / avg_loss;
        Some(100.0 - (100.0 / (1.0 + rs)))
    }

    /// ATR (Average True Range) over 14 periods
    pub fn atr14(&self) -> Option<f64> {
        self.atr(14, false)
    }

    /// Calculate ATR
    pub fn atr(&self, period: usize, use_5m: bool) -> Option<f64> {
        let candles = if use_5m {
            &self.candles_5m
        } else {
            &self.candles_1m
        };

        if candles.len() < period + 1 {
            return None;
        }

        let mut true_ranges = Vec::new();

        for i in 1..candles.len() {
            let tr = (candles[i].high - candles[i].low)
                .max((candles[i].high - candles[i - 1].close).abs())
                .max((candles[i].low - candles[i - 1].close).abs());
            true_ranges.push(tr);
        }

        if true_ranges.len() < period {
            return None;
        }

        // Initial SMA
        let mut atr: f64 = true_ranges[..period].iter().sum::<f64>() / period as f64;

        // Wilder's smoothing
        for i in period..true_ranges.len() {
            atr = (atr * (period as f64 - 1.0) + true_ranges[i]) / period as f64;
        }

        Some(atr)
    }

    /// Latest close price (1m)
    pub fn latest_close(&self) -> Option<f64> {
        self.candles_1m.last().map(|c| c.close)
    }

    /// ATR as percentage of current price (e.g. 1.5 means ATR is 1.5% of price)
    pub fn atr_pct(&self) -> Option<f64> {
        let atr = self.atr14()?;
        let price = self.latest_close()?;
        if price <= 0.0 { return None; }
        Some((atr / price) * 100.0)
    }

    /// Number of 1m candles
    pub fn candle_count_1m(&self) -> usize {
        self.candles_1m.len()
    }

    /// Number of 5m candles
    pub fn candle_count_5m(&self) -> usize {
        self.candles_5m.len()
    }

    /// Get 1m candles (for external use)
    pub fn candles_1m(&self) -> &[Candle] {
        &self.candles_1m
    }

    /// Get 5m candles (for external use)
    pub fn candles_5m(&self) -> &[Candle] {
        &self.candles_5m
    }
}
