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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_rss_parse() {
        let err = AppError::RssParse("bad feed".into());
        assert!(err.to_string().contains("RSS parse failed"));
        assert!(err.to_string().contains("bad feed"));
    }

    #[test]
    fn display_html_parse() {
        let err = AppError::HtmlParse("bad html".into());
        assert!(err.to_string().contains("HTML parse failed"));
    }

    #[test]
    fn display_po_processing() {
        let err = AppError::PoProcessing("bad po".into());
        assert!(err.to_string().contains("PO file processing failed"));
    }

    #[test]
    fn display_zip_processing() {
        let err = AppError::ZipProcessing("bad zip".into());
        assert!(err.to_string().contains("ZIP processing failed"));
    }

    #[test]
    fn display_llm_api() {
        let err = AppError::LlmApi("timeout".into());
        assert!(err.to_string().contains("LLM API call failed"));
    }

    #[test]
    fn display_llm_response_parse() {
        let err = AppError::LlmResponseParse("bad json".into());
        assert!(err.to_string().contains("LLM response parsing failed"));
    }

    #[test]
    fn display_config() {
        let err = AppError::Config("missing var".into());
        assert!(err.to_string().contains("Configuration error"));
    }

    #[test]
    fn display_revision_not_found() {
        let err = AppError::RevisionNotFound("r9999".into());
        assert!(err.to_string().contains("not found"));
    }
}
