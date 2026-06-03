# SignalFlow Bot 🚀

High-performance auto-trading bot for Hyperliquid perpetual futures, powered by AI signals from Sodex.

## Features

- **WebSocket Real-time Data** — Low-latency market data streaming
- **Auto Trading** — Execute trades based on AI signals
- **Risk Management** — Stop loss, take profit, daily loss limits
- **Position Tracking** — Real-time PnL and exposure monitoring
- **Clean Architecture** — Modular, testable, maintainable code
- **20x Leverage** — Configurable leverage with tight risk controls
- **Cancel Orders** — Cancel resting orders by ID or all at once
- **Position Sync** — Auto-sync positions from exchange on startup
- **Trade History** — SQLite database for persistent trade logging & PnL tracking
- **Safe Asset Handling** — Unknown coins rejected, no silent BTC fallback
- **TP/SL Trigger Orders** — Proper `normalTpsl` grouping for Hyperliquid
- **Rate Limit Protection** — Exponential backoff retry on 429s
- **Supabase Ready** — Abstract `TradeStore` trait, swap SQLite → PostgreSQL

## Quick Start

### 1. Clone & Build

```bash
git clone <repo-url>
cd signalflow-bot
cargo build --release
```

### 2. Configure

```bash
cp config.example.toml config.toml
# Edit config.toml with your settings
```

### 3. Run

```bash
# Dry run (no real trades)
./target/release/signalflow-bot

# With tmux (keep running in background)
tmux new -s bot
./target/release/signalflow-bot
# Ctrl+B, D to detach
```

## Configuration

### Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `SIGNALFLOW_PRIVATE_KEY` | Private key (overrides config) | No |
| `RUST_LOG` | Log level (debug, info, warn, error) | No |

### Strategy Settings

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_position_size` | $50 | Max USD per trade |
| `max_leverage` | 20 | Maximum leverage |
| `poll_interval_secs` | 30 | Poll interval |
| `dry_run` | true | Log only, no execution |
| `trade_log_path` | `trades.jsonl` | Trade history log file |
| `database_url` | `sqlite:signalflow.db?mode=rwc` | SQLite DB (swap to PostgreSQL for Supabase) |

### Risk Settings

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_total_exposure` | $250 | Max total exposure |
| `stop_loss_pct` | 1.5% | Stop loss % |
| `take_profit_pct` | 3.0% | Take profit % |
| `max_daily_loss` | $50 | Daily loss limit |

## Architecture

```
src/
├── domain/           # Pure business logic
├── application/      # Orchestration
├── infrastructure/   # External APIs
└── presentation/     # Entry point
```

## Testing

```bash
cargo test
```

## Performance

- **WebSocket** for real-time market data
- **Connection pooling** for REST API
- **LTO optimized** release binary
- **Async throughout** with Tokio

## Security

- No secrets in logs
- Environment variable support for private keys
- TLS via rustls (no OpenSSL)
- EIP-712 signing for transactions

## License

MIT
