use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("RSS parse failed: {0}")]
    RssParse(String),

    #[error("HTML parse failed: {0}")]
    HtmlParse(String),

    #[error("PO file processing failed: {0}")]
    PoProcessing(String),

    #[error("ZIP processing failed: {0}")]
    ZipProcessing(String),

    #[error("LLM API call failed: {0}")]
    LlmApi(String),

    #[error("LLM response parsing failed: {0}")]
    LlmResponseParse(String),

    #[error("Redis operation failed: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Update page entry not found for revision: {0}")]
    RevisionNotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type AppResult<T> = Result<T, AppError>;
