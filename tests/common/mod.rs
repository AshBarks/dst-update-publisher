#![allow(dead_code)]

use dst_update_publisher::models::{
    AppConfig, PoSearchResult, PoTranslationEntry, TranslatedAnnouncement,
};
use dst_update_publisher::po_search::{PoFileIndex, build_index_from_po_contents};
use dst_update_publisher::publisher::connect_redis;
use redis::aio::MultiplexedConnection;

pub fn make_config(mock_rss_url: &str, mock_page_url: &str) -> AppConfig {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    AppConfig {
        rss_url: mock_rss_url.to_string(),
        update_page_url: mock_page_url.to_string(),
        redis_url,
        redis_channel: unique_key("ch"),
        redis_dedupe_key: unique_key("dedup"),
        llm_api_base: "http://localhost:0".into(),
        llm_api_key: "test-key".into(),
        llm_model: "test-model".into(),
        po_zip_path: String::new(),
        po_zip_po_files: vec![],
    }
}

pub fn make_config_with_redis(
    mock_rss_url: &str,
    mock_page_url: &str,
    redis_channel: &str,
    redis_dedupe_key: &str,
) -> AppConfig {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    AppConfig {
        rss_url: mock_rss_url.to_string(),
        update_page_url: mock_page_url.to_string(),
        redis_url,
        redis_channel: redis_channel.to_string(),
        redis_dedupe_key: redis_dedupe_key.to_string(),
        llm_api_base: "http://localhost:0".into(),
        llm_api_key: "test-key".into(),
        llm_model: "test-model".into(),
        po_zip_path: String::new(),
        po_zip_po_files: vec![],
    }
}

pub fn unique_key(prefix: &str) -> String {
    format!("itest:{}:{}", prefix, uuid::Uuid::new_v4())
}

pub async fn connect_test_redis() -> MultiplexedConnection {
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let config = AppConfig {
        redis_url: redis_url.clone(),
        ..make_config("", "")
    };
    connect_redis(&config)
        .await
        .unwrap_or_else(|e| panic!("failed to connect Redis at {}: {}", redis_url, e))
}

pub fn make_translated_announcement() -> TranslatedAnnouncement {
    TranslatedAnnouncement {
        original_text: "Changes Added Chessmaster Circuit.".into(),
        translated_text: "更改 新增国际象棋大师电路。".into(),
        search_results: vec![PoSearchResult {
            term: "Chessmaster Circuit".into(),
            candidates: vec![PoTranslationEntry {
                original: "Chessmaster Circuit".into(),
                translation: "国际象棋大师电路".into(),
            }],
        }],
    }
}

pub fn load_fixture_po_index() -> PoFileIndex {
    let po_content =
        std::fs::read_to_string("tests/fixtures/chinese_s.po").expect("failed to read fixture PO");
    build_index_from_po_contents(&[po_content]).expect("failed to build PO index from fixture")
}

pub fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/{}", name))
        .unwrap_or_else(|e| panic!("failed to read fixture '{}': {}", name, e))
}

pub async fn cleanup_key(conn: &mut MultiplexedConnection, key: &str) {
    let _ = redis::cmd("DEL").arg(key).exec_async(conn).await;
}
