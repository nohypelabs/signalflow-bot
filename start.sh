#!/bin/bash
# SignalFlow Bot Startup Script

cd /root/signalflow-bot

# Check if config exists
if [ ! -f config.toml ]; then
    echo "❌ config.toml not found!"
    echo "Run: cp config.example.toml config.toml"
    exit 1
fi

# Check if binary exists
if [ ! -f target/release/signalflow-bot ]; then
    echo "❌ Binary not found! Building..."
    cargo build --release
fi

# Set log level
export RUST_LOG=${RUST_LOG:-info}

echo "🚀 Starting SignalFlow Bot..."
echo "📊 Config: config.toml"
echo "📝 Logs: RUST_LOG=$RUST_LOG"
echo ""

# Run the bot
./target/release/signalflow-bot
