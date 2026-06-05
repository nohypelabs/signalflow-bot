use thiserror::Error;

#[derive(Error, Debug)]
pub enum BotError {
    #[error("Config error: {0}")]
    Config(String),

    #[error("API error: {0}")]
    Api(String),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("Signing error: {0}")]
    Signing(String),

    #[error("Order error: {0}")]
    Order(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, BotError>;
