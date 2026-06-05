# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run Commands

```bash
cargo build --release          # LTO + strip, ~3.9 MB binary
cargo test                     # 45 unit tests, in-file #[cfg(test)] modules (no tests/ dir)
cargo clippy                   # lint
cargo check                    # fast type-check without linking
```

Run: `cp config.example.toml config.toml`, edit with private key, then `./target/release/signalflow-bot`. Env vars: `SIGNALFLOW_PRIVATE_KEY` (overrides config), `RUST_LOG` (default: info). Helper scripts: `start.sh`, `run-tmux.sh`.

## Architecture — Critical: Dual Module Structure

**Active runtime** (`main.rs`) uses flat top-level modules directly. **Changes to trading logic must target these files:**

`main.rs` → `config.rs` → `hl_ws.rs`/`hl_rest.rs` → `orderbook.rs`/`orderflow.rs`/`ta.rs` → `decision.rs` → `macro_filter.rs` → `risk.rs` → `execution.rs` → `lesson.rs`/`store.rs`

A parallel **Clean Architecture** exists under `src/domain/`, `src/application/`, `src/infrastructure/`, `src/presentation/` — this is an in-progress refactoring and is **not wired into the running bot**. It uses `tracing` (vs `log` in the active path) and has its own config with Sodex API integration. Don't modify these unless specifically working on the refactor.

## Data Flow

```
WebSocket (l2Book + trades)
  → OrderbookAnalyzer / OrderflowAnalyzer / TaEngine (per-WS message)
  → DecisionEngine.evaluate_signal() (7-step pipeline, min score 70)
  → MacroFilter.should_block() (Finnhub economic calendar)
  → RiskManager.calculate() (fixed-fractional sizing, ATR-based SL/TP)
  → Executor.execute_limit_order() (IOC limit at best bid/ask, 200ms timeout)
  → LessonEngine.analyze_trade() (on close: classify outcome, generate rules)
```

`FundingMonitor` runs in a separate tokio task, polling funding rates and rejecting trades when adverse (>0.05% long / <-0.05% short).

## Key Modules

| Module | Role |
|--------|------|
| `decision.rs` | 7-step signal: liquidity, ATR filter, EMA crossover + RSI, funding, CVD confirmation, session filter, composite score |
| `risk.rs` | Position sizing (account × risk_per_trade), daily/weekly loss limits, equity curve drawdown protection |
| `execution.rs` | IOC limit orders, 200ms fill-or-cancel, rate-limit retry with exponential backoff (1s/2s/4s) |
| `lesson.rs` | Classifies win/loss causes, generates adaptive `StrategyRule` records (max 50 lesson rules) |
| `store.rs` | SQLite via `sqlx` (TradeStore trait — designed to swap to PostgreSQL for Supabase) |
| `ta.rs` | Builds 1m/5m candles from raw trades, EMA(9)/EMA(21)/RSI(14)/ATR(14) |
| `signer.rs` | EIP-712 signing, Arbitrum chain ID 42161 |
| `hl_rest.rs` | REST client with exponential backoff on rate limits |

## Conventions

- **Async**: Tokio `full` features. Shared state: `Arc<Mutex<>>` (tokio sync mutexes).
- **Logging**: Active path uses `log` + `env_logger`. Clean Architecture uses `tracing`.
- **Errors**: `BotError` enum with `thiserror`, `Result<T>` alias. No panics in production.
- **TLS**: `rustls` everywhere (no OpenSSL) — required for PRoot/ARM64 compatibility.
- **Crypto**: k256 ECDSA + sha3/Keccak256 for EIP-712.
- **Config**: `config.toml` (gitignored), loaded with `toml` crate. See `config.example.toml` for all sections.
- **Database tables**: trades, lessons, strategy_rules, trade_analysis, signals.
- **Strategy rules**: JSON-serialized in SQLite. Types: VolatilitySlTp, PositionSizing, ConfidenceFilter, CoinFilter, LeverageScale, SessionFilter, LessonRule.

## Running Tests

All tests are in-file with `#[cfg(test)]` modules — no separate `tests/` directory. Run a single test with:

```bash
cargo test <test_name>
```
