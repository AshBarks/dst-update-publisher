use crate::error::{AppError, AppResult};
use crate::models::{AppConfig, RssUpdateItem};

pub async fn fetch_rss_updates(config: &AppConfig) -> AppResult<Vec<RssUpdateItem>> {
    let response = reqwest::get(&config.rss_url)
        .await
        .map_err(AppError::Http)?;

    let body = response.text().await.map_err(AppError::Http)?;

    let channel = rss::Channel::read_from(body.as_bytes())
        .map_err(|e| AppError::RssParse(format!("failed to parse RSS feed: {}", e)))?;

    let items: Vec<RssUpdateItem> = channel
        .items
        .into_iter()
        .filter_map(|item| RssUpdateItem::from_rss_item(&item).ok())
        .filter(|item| item.is_pc_update())
        .collect();

    tracing::info!("fetched {} PC update items from RSS", items.len());

    Ok(items)
}

pub fn get_latest_pc_update(items: &[RssUpdateItem]) -> Option<&RssUpdateItem> {
    items.iter().max_by_key(|item| item.pub_date)
}
