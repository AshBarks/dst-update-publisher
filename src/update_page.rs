use scraper::{ElementRef, Html, Selector};

use crate::error::{AppError, AppResult};
use crate::models::{AppConfig, ReleaseChannel, UpdatePageData, UpdatePageEntry};

pub async fn fetch_update_page(config: &AppConfig) -> AppResult<UpdatePageData> {
    let response = reqwest::get(&config.update_page_url)
        .await
        .map_err(AppError::Http)?;

    let body = response.text().await.map_err(AppError::Http)?;

    parse_update_page_html(&body)
}

pub fn parse_update_page_html(html: &str) -> AppResult<UpdatePageData> {
    let document = Html::parse_document(html);

    let row_selector = Selector::parse("li.cCmsRecord_row").unwrap();
    let link_selector = Selector::parse("a.cRelease").unwrap();
    let _release_id_selector = Selector::parse("[data-releaseID]").unwrap();
    let hotfix_selector = Selector::parse("span.cUpdate_hotfix").unwrap();
    let badge_selector = Selector::parse("span.ipsBadge").unwrap();
    let h3_selector = Selector::parse("h3.ipsType_sectionHead").unwrap();

    let mut entries = Vec::new();

    for row in document.select(&row_selector) {
        let link_el = match row.select(&link_selector).next() {
            Some(el) => el,
            None => continue,
        };

        let release_id = link_el.value().attr("data-releaseid").unwrap_or_default();

        let is_hotfix = link_el.select(&hotfix_selector).next().is_some();

        let h3 = match link_el.select(&h3_selector).next() {
            Some(el) => el,
            None => continue,
        };

        let build_number = extract_direct_text(h3);

        let channel = parse_channel_from_badges(link_el, &badge_selector);

        entries.push(UpdatePageEntry {
            revision: release_id.to_string(),
            build_number,
            channel,
            is_hotfix,
        });
    }

    tracing::info!("parsed {} entries from update page", entries.len());

    Ok(UpdatePageData { entries })
}

fn extract_direct_text(element: ElementRef) -> String {
    element
        .children()
        .filter_map(|ch| ch.value().as_text().map(|t| t.text.to_string()))
        .collect::<String>()
        .trim()
        .to_string()
}

fn parse_channel_from_badges(element: ElementRef, badge_selector: &Selector) -> ReleaseChannel {
    for badge in element.select(badge_selector) {
        let badge_text = badge.text().collect::<String>().trim().to_string();
        let title = badge.value().attr("title").unwrap_or_default();

        if title.contains("most current available release") || badge_text == "Release" {
            return ReleaseChannel::Release;
        }
        if title.contains("beta is available") || badge_text == "Test" || badge_text == "Beta" {
            return ReleaseChannel::Beta;
        }
    }

    ReleaseChannel::Beta
}

pub fn get_version_info(
    page_data: &UpdatePageData,
    revision: &str,
    build_number: &str,
) -> AppResult<UpdatePageEntry> {
    page_data
        .find_by_build_number(build_number)
        .or_else(|| page_data.find_by_revision(revision))
        .cloned()
        .ok_or(AppError::RevisionNotFound(format!(
            "build_number={}, revision={}, page_data={:?}",
            build_number, revision, page_data
        )))
}
