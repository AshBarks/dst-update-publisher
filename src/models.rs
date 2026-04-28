use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::AppError;

#[derive(Debug, Clone)]
pub enum ReleaseChannel {
    Release,
    Beta,
}

impl Serialize for ReleaseChannel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            ReleaseChannel::Release => serializer.serialize_str("release"),
            ReleaseChannel::Beta => serializer.serialize_str("beta"),
        }
    }
}

impl<'de> Deserialize<'de> for ReleaseChannel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "release" => Ok(ReleaseChannel::Release),
            "beta" => Ok(ReleaseChannel::Beta),
            _ => Err(serde::de::Error::custom(format!("unknown channel: {}", s))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RssUpdateItem {
    pub build_number: String,
    pub link: String,
    pub description_html: String,
    pub pub_date: DateTime<Utc>,
    pub revision: String,
}

impl RssUpdateItem {
    pub fn from_rss_item(item: &rss::Item) -> Result<Self, AppError> {
        let title = item.title.clone().unwrap_or_default();
        let link = item.link.clone().unwrap_or_default();

        let description_html = item.description.clone().unwrap_or_default();

        let pub_date_str = item.pub_date.clone().unwrap_or_default();
        let pub_date = chrono::DateTime::parse_from_rfc2822(&pub_date_str)
            .map_err(|e| {
                AppError::RssParse(format!(
                    "failed to parse pub_date '{}': {}",
                    pub_date_str, e
                ))
            })?
            .to_utc();

        let revision = Self::extract_revision_from_link(&link);

        Ok(RssUpdateItem {
            build_number: title,
            link,
            description_html,
            pub_date,
            revision,
        })
    }

    pub fn extract_revision_from_link(link: &str) -> String {
        let url = url::Url::parse(link).ok();
        if let Some(url) = url {
            for segment in url.path_segments().into_iter().flatten() {
                if let Some(idx) = segment.find("-r") {
                    return segment[idx + 2..].to_string();
                }
            }
        }
        String::new()
    }

    pub fn is_pc_update(&self) -> bool {
        self.link.contains("/game-updates/dst/")
            && !self.link.contains("/dst_xboxone/")
            && !self.link.contains("/dst_ps4/")
    }
}

#[derive(Debug, Clone)]
pub struct UpdatePageEntry {
    pub revision: String,
    pub build_number: String,
    pub channel: ReleaseChannel,
    pub is_hotfix: bool,
}

#[derive(Debug, Clone)]
pub struct UpdatePageData {
    pub entries: Vec<UpdatePageEntry>,
}

impl UpdatePageData {
    pub fn find_by_revision(&self, revision: &str) -> Option<&UpdatePageEntry> {
        self.entries.iter().find(|e| e.revision == revision)
    }

    pub fn find_by_build_number(&self, build_number: &str) -> Option<&UpdatePageEntry> {
        self.entries.iter().find(|e| e.build_number == build_number)
    }
}

#[derive(Debug, Clone)]
pub struct PoTranslationEntry {
    pub original: String,
    pub translation: String,
}

#[derive(Debug, Clone)]
pub struct PoSearchResult {
    pub term: String,
    pub candidates: Vec<PoTranslationEntry>,
}

impl PoSearchResult {
    pub fn best_match(&self) -> Option<&PoTranslationEntry> {
        self.candidates
            .iter()
            .find(|c| c.original == self.term)
            .or_else(|| self.candidates.first())
    }
}

#[derive(Debug, Clone)]
pub struct TranslatedAnnouncement {
    pub original_text: String,
    pub translated_text: String,
    pub search_results: Vec<PoSearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateNotification {
    pub build_number: String,
    pub revision: String,
    pub channel: String,
    pub is_hotfix: bool,
    pub original_description: String,
    pub translated_description: String,
    pub glossary: HashMap<String, String>,
    pub pub_date: String,
    pub link: String,
}

impl UpdateNotification {
    pub fn compose(
        rss_item: &RssUpdateItem,
        page_entry: &UpdatePageEntry,
        translated: &TranslatedAnnouncement,
    ) -> Self {
        let glossary: HashMap<String, String> = translated
            .search_results
            .iter()
            .filter_map(|r| r.best_match().map(|b| (r.term.clone(), b.translation.clone())))
            .collect();

        let channel = match page_entry.channel {
            ReleaseChannel::Release => "release",
            ReleaseChannel::Beta => "beta",
        };

        UpdateNotification {
            build_number: rss_item.build_number.clone(),
            revision: rss_item.revision.clone(),
            channel: channel.to_string(),
            is_hotfix: page_entry.is_hotfix,
            original_description: translated.original_text.clone(),
            translated_description: translated.translated_text.clone(),
            glossary,
            pub_date: rss_item.pub_date.to_rfc3339(),
            link: rss_item.link.clone(),
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub rss_url: String,
    pub update_page_url: String,
    pub redis_url: String,
    pub redis_channel: String,
    pub redis_dedupe_key: String,
    pub llm_api_base: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub po_zip_path: String,
    pub po_zip_po_files: Vec<String>,
}
