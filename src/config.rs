use crate::error::AppError;
use crate::models::AppConfig;

fn load_dotenv() {
    dotenvy::dotenv().ok();

    if std::env::var("LLM_API_BASE").is_err() {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(".env")));

        if let Some(path) = exe_dir
            && path.exists()
        {
            dotenvy::from_path(&path).ok();
            return;
        }

        let cwd_env = std::env::current_dir().ok().map(|d| d.join(".env"));
        if let Some(path) = cwd_env
            && path.exists()
        {
            dotenvy::from_path(&path).ok();
        }
    }
}

pub fn load_config() -> Result<AppConfig, AppError> {
    load_dotenv();

    let rss_url = std::env::var("RSS_URL").unwrap_or_else(|_| {
        "https://forums.kleientertainment.com/rss/6-dont-starve-together-updates.xml/".to_string()
    });

    let update_page_url = std::env::var("UPDATE_PAGE_URL")
        .unwrap_or_else(|_| "https://forums.kleientertainment.com/game-updates/dst/".to_string());

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    let redis_channel =
        std::env::var("REDIS_CHANNEL").unwrap_or_else(|_| "dst-updates".to_string());

    let redis_dedupe_key =
        std::env::var("REDIS_DEDUPE_KEY").unwrap_or_else(|_| "dst:last_build".to_string());

    let llm_api_base = std::env::var("LLM_API_BASE")
        .map_err(|_| AppError::Config("LLM_API_BASE is required".to_string()))?;

    let llm_api_key = std::env::var("LLM_API_KEY")
        .map_err(|_| AppError::Config("LLM_API_KEY is required".to_string()))?;

    let llm_model = std::env::var("LLM_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());

    let po_zip_path = std::env::var("PO_ZIP_PATH")
        .map_err(|_| AppError::Config("PO_ZIP_PATH is required".to_string()))?;

    let po_zip_po_files = std::env::var("PO_ZIP_PO_FILES")
        .map_err(|_| AppError::Config("PO_ZIP_PO_FILES is required (comma-separated PO paths inside the ZIP, e.g. scripts/languages/chinese_s.po)".to_string()))?
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<String>>();

    if po_zip_po_files.is_empty() {
        return Err(AppError::Config(
            "PO_ZIP_PO_FILES must contain at least one PO file path".to_string(),
        ));
    }

    Ok(AppConfig {
        rss_url,
        update_page_url,
        redis_url,
        redis_channel,
        redis_dedupe_key,
        llm_api_base,
        llm_api_key,
        llm_model,
        po_zip_path,
        po_zip_po_files,
    })
}
