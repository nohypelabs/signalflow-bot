mod application;
mod domain;
mod error;
mod infrastructure;
mod presentation;

use std::path::Path;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Run the bot
    presentation::cli::run(Path::new("config.toml")).await?;

    Ok(())
}
