use crate::error::{AppError, AppResult};
use crate::models::{AppConfig, RssUpdateItem};

pub async fn fetch_rss_updates(config: &AppConfig) -> AppResult<Vec<RssUpdateItem>> {
    let response = reqwest::get(&config.rss_url)
        .await
        .map_err(AppError::Http)?;

    let body = response.text().await.map_err(AppError::Http)?;

    parse_rss_items(&body)
}

pub fn parse_rss_items(body: &str) -> AppResult<Vec<RssUpdateItem>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const RSS_XML: &str = include_str!("../tests/fixtures/rss.xml");

    #[test]
    fn parse_rss_finds_pc_items() {
        let items = parse_rss_items(RSS_XML).expect("failed to parse RSS");
        assert!(!items.is_empty(), "should find PC update items");
        for item in &items {
            assert!(
                item.is_pc_update(),
                "all returned items should be PC updates"
            );
        }
    }

    #[test]
    fn parse_rss_filters_non_pc() {
        let channel = rss::Channel::read_from(RSS_XML.as_bytes()).unwrap();
        let total = channel.items.len();
        let pc_items = parse_rss_items(RSS_XML).unwrap();
        assert!(
            pc_items.len() < total,
            "PC items should be fewer than total items"
        );
    }

    #[test]
    fn parse_rss_revision_extracted() {
        let items = parse_rss_items(RSS_XML).unwrap();
        let first = items.first().expect("should have at least one item");
        assert!(!first.revision.is_empty(), "revision should be extracted");
    }

    #[test]
    fn get_latest_returns_most_recent() {
        let items = parse_rss_items(RSS_XML).unwrap();
        let latest = get_latest_pc_update(&items);
        assert!(latest.is_some());
        let latest = latest.unwrap();
        for item in &items {
            assert!(
                item.pub_date <= latest.pub_date,
                "latest item should have the most recent pub_date"
            );
        }
    }

    #[test]
    fn parse_rss_empty_feed() {
        let empty = "<rss version=\"2.0\"><channel><title>Test</title></channel></rss>";
        let items = parse_rss_items(empty).unwrap();
        assert!(items.is_empty());
    }
}
