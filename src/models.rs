use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use canonical_msg::{
    canonical_message, CanonicalMessage, DstUpdate as CanonicalDstUpdate, Link, MsgKind, Source,
    SourceKind,
};

use crate::error::{AppError, AppResult};

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

/// 候选译文与查询词的匹配方式，可信度从高到低：exact > variant > fuzzy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// 查询词（小写化后）与 PO 条目 msgid 完全一致
    Exact,
    /// 查询词经单复数/时态等变体归一后命中
    Variant,
    /// 按分词在词索引中模糊命中
    Fuzzy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoTranslationEntry {
    pub original: String,
    pub translation: String,
    pub kind: MatchKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoSearchResult {
    pub term: String,
    pub candidates: Vec<PoTranslationEntry>,
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
    /// 术语检索的完整候选集（含匹配类型），仅用于调试观察匹配策略，下游不应依赖其内容
    pub glossary: Vec<PoSearchResult>,
    pub pub_date: String,
    pub link: String,
}

impl UpdateNotification {
    pub fn compose(
        rss_item: &RssUpdateItem,
        page_entry: &UpdatePageEntry,
        translated: &TranslatedAnnouncement,
    ) -> Self {
        // 同一术语可能在多次工具调用中重复出现，按小写去重并保留首次出现的顺序，
        // 保证 glossary 的内容与顺序是确定的
        let mut seen_terms = HashSet::new();
        let glossary: Vec<PoSearchResult> = translated
            .search_results
            .iter()
            .filter(|r| seen_terms.insert(r.term.to_lowercase()))
            .cloned()
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

/// 稳定机器标识："klei-dst-rss"（canonical source.id，与去重键前缀一致）
pub const KLEI_DST_SOURCE_ID: &str = "klei-dst-rss";

/// canonical 信源描述：更新与失败告警共用同一 source（DESIGN §5.2）
pub fn klei_source() -> Source {
    Source {
        kind: SourceKind::Rss as i32,
        id: KLEI_DST_SOURCE_ID.to_string(),
        display_name: "Klei 官方更新公告".to_string(),
    }
}

fn canonical_release_channel(channel: &ReleaseChannel) -> canonical_msg::ReleaseChannel {
    match channel {
        ReleaseChannel::Release => canonical_msg::ReleaseChannel::Release,
        ReleaseChannel::Beta => canonical_msg::ReleaseChannel::Beta,
    }
}

/// 构造 canonical（push.v1）更新消息。字段映射逐条对应 implementation/02 Step 2 表格。
/// translated_text 为空视为翻译失败（返回 Err，进失败状态机）——
/// 防不变量 #1 违规消息上线（body_markdown 是中文译文的主展示正文）。
pub fn build_canonical(
    rss_item: &RssUpdateItem,
    page_entry: &UpdatePageEntry,
    announcement_text: &str,
    translated: &TranslatedAnnouncement,
) -> AppResult<CanonicalMessage> {
    let body_markdown = translated.translated_text.clone();
    if body_markdown.trim().is_empty() {
        return Err(AppError::Translation(
            "translated_text is empty after trim; refusing to build canonical message".to_string(),
        ));
    }

    Ok(CanonicalMessage {
        id: ulid::Ulid::new().to_string(),
        source: Some(klei_source()),
        dedup_key: format!("{KLEI_DST_SOURCE_ID}:{}", rss_item.build_number),
        kind: MsgKind::Update as i32,
        title: format!(
            "饥荒联机版更新 build {} · r{}",
            rss_item.build_number, rss_item.revision
        ),
        body_markdown,
        body_text: announcement_text.to_string(),
        links: vec![Link {
            rel: "original".to_string(),
            url: rss_item.link.clone(),
        }],
        tags: vec![],
        published_at: Some(canonical_msg::pbjson_types::Timestamp {
            seconds: rss_item.pub_date.timestamp(),
            nanos: rss_item.pub_date.timestamp_subsec_nanos() as i32,
        }),
        domain: Some(canonical_message::Domain::Dst(CanonicalDstUpdate {
            build_number: rss_item.build_number.clone(),
            revision: rss_item.revision.clone(),
            channel: canonical_release_channel(&page_entry.channel) as i32,
            is_hotfix: page_entry.is_hotfix,
            original_description: translated.original_text.clone(),
        })),
    })
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub rss_url: String,
    pub update_page_url: String,
    pub redis_url: String,
    pub redis_channel: String,
    pub redis_canonical_channel: String,
    pub redis_dedupe_key: String,
    pub failure_escalate_count: u32,
    pub failure_escalate_hours: i64,
    pub llm_api_base: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_temperature: f32,
    pub po_zip_path: String,
    pub po_zip_po_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DstUpdateEntry {
    pub build_number: u64,
    pub link: String,
    pub release_date: String,
    pub is_hotfix: bool,
    pub status: String,
    pub is_current: bool,
    pub row_id: u64,
    pub release_id: u64,
    pub title: String,
    pub content_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_text_zh: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DstUpdatesFile {
    pub total: u64,
    pub fetched_at: String,
    pub updates: Vec<DstUpdateEntry>,
}

#[derive(Debug, Clone)]
pub enum ProcessOutcome {
    Published { build_number: String },
    AlreadyProcessed { build_number: String },
    NoUpdateAvailable,
    Failed { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_revision_standard_link() {
        let rev = RssUpdateItem::extract_revision_from_link(
            "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/",
        );
        assert_eq!(rev, "2722");
    }

    #[test]
    fn extract_revision_xbox_link() {
        let rev = RssUpdateItem::extract_revision_from_link(
            "https://forums.kleientertainment.com/game-updates/dst_xboxone/112100-r2721/",
        );
        assert_eq!(rev, "2721");
    }

    #[test]
    fn extract_revision_no_revision() {
        let rev = RssUpdateItem::extract_revision_from_link(
            "https://forums.kleientertainment.com/game-updates/dst/",
        );
        assert_eq!(rev, "");
    }

    #[test]
    fn extract_revision_empty_string() {
        let rev = RssUpdateItem::extract_revision_from_link("");
        assert_eq!(rev, "");
    }

    #[test]
    fn extract_revision_invalid_url() {
        let rev = RssUpdateItem::extract_revision_from_link("not a url");
        assert_eq!(rev, "");
    }

    #[test]
    fn is_pc_update_pc_link() {
        let item = RssUpdateItem {
            build_number: "724783".into(),
            link: "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/".into(),
            description_html: String::new(),
            pub_date: Utc::now(),
            revision: "2722".into(),
        };
        assert!(item.is_pc_update());
    }

    #[test]
    fn is_pc_update_xbox_link() {
        let item = RssUpdateItem {
            build_number: "112100".into(),
            link: "https://forums.kleientertainment.com/game-updates/dst_xboxone/112100-r2721/"
                .into(),
            description_html: String::new(),
            pub_date: Utc::now(),
            revision: "2721".into(),
        };
        assert!(!item.is_pc_update());
    }

    #[test]
    fn is_pc_update_ps4_link() {
        let item = RssUpdateItem {
            build_number: "3380".into(),
            link: "https://forums.kleientertainment.com/game-updates/dst_ps4/3380-r2719/".into(),
            description_html: String::new(),
            pub_date: Utc::now(),
            revision: "2719".into(),
        };
        assert!(!item.is_pc_update());
    }

    #[test]
    fn compose_notification_fields() {
        let rss_item = RssUpdateItem {
            build_number: "724783".into(),
            link: "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/".into(),
            description_html: String::new(),
            pub_date: DateTime::parse_from_rfc3339("2025-04-28T12:00:00Z")
                .unwrap()
                .to_utc(),
            revision: "2722".into(),
        };

        let page_entry = UpdatePageEntry {
            revision: "2722".into(),
            build_number: "724783".into(),
            channel: ReleaseChannel::Release,
            is_hotfix: false,
        };

        let translated = TranslatedAnnouncement {
            original_text: "original".into(),
            translated_text: "翻译".into(),
            search_results: vec![PoSearchResult {
                term: "Varg".into(),
                candidates: vec![
                    PoTranslationEntry {
                        original: "Varg".into(),
                        translation: "座狼".into(),
                        kind: MatchKind::Exact,
                    },
                    PoTranslationEntry {
                        original: "Vargling".into(),
                        translation: "座狼崽".into(),
                        kind: MatchKind::Fuzzy,
                    },
                ],
            }],
        };

        let notification = UpdateNotification::compose(&rss_item, &page_entry, &translated);

        assert_eq!(notification.build_number, "724783");
        assert_eq!(notification.revision, "2722");
        assert_eq!(notification.channel, "release");
        assert!(!notification.is_hotfix);
        assert_eq!(notification.original_description, "original");
        assert_eq!(notification.translated_description, "翻译");

        // glossary 应保留全部候选及其匹配类型，而不是只保留一个“最佳匹配”
        assert_eq!(notification.glossary.len(), 1);
        let entry = &notification.glossary[0];
        assert_eq!(entry.term, "Varg");
        assert_eq!(entry.candidates.len(), 2);
        assert_eq!(entry.candidates[0].translation, "座狼");
        assert_eq!(entry.candidates[0].kind, MatchKind::Exact);
        assert_eq!(entry.candidates[1].original, "Vargling");
        assert_eq!(entry.candidates[1].kind, MatchKind::Fuzzy);

        assert!(notification.pub_date.contains("2025"));
        assert!(notification.link.contains("724783"));
    }

    #[test]
    fn compose_glossary_dedupes_terms_case_insensitively() {
        let rss_item = RssUpdateItem {
            build_number: "724783".into(),
            link: "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/".into(),
            description_html: String::new(),
            pub_date: Utc::now(),
            revision: "2722".into(),
        };

        let page_entry = UpdatePageEntry {
            revision: "2722".into(),
            build_number: "724783".into(),
            channel: ReleaseChannel::Release,
            is_hotfix: false,
        };

        let make_result = |term: &str| PoSearchResult {
            term: term.into(),
            candidates: vec![PoTranslationEntry {
                original: "Varg".into(),
                translation: "座狼".into(),
                kind: MatchKind::Exact,
            }],
        };

        // 多次工具调用可能重复查询同一术语（大小写不同），应去重并保留首次出现
        let translated = TranslatedAnnouncement {
            original_text: String::new(),
            translated_text: String::new(),
            search_results: vec![
                make_result("Hound"),
                make_result("Varg"),
                make_result("hound"),
                make_result("varg"),
            ],
        };

        let notification = UpdateNotification::compose(&rss_item, &page_entry, &translated);

        let terms: Vec<&str> = notification
            .glossary
            .iter()
            .map(|r| r.term.as_str())
            .collect();
        assert_eq!(terms, vec!["Hound", "Varg"]);
    }

    #[test]
    fn compose_notification_beta_hotfix() {
        let rss_item = RssUpdateItem {
            build_number: "724783".into(),
            link: "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/".into(),
            description_html: String::new(),
            pub_date: Utc::now(),
            revision: "2722".into(),
        };

        let page_entry = UpdatePageEntry {
            revision: "2722".into(),
            build_number: "724783".into(),
            channel: ReleaseChannel::Beta,
            is_hotfix: true,
        };

        let translated = TranslatedAnnouncement {
            original_text: String::new(),
            translated_text: String::new(),
            search_results: vec![],
        };

        let notification = UpdateNotification::compose(&rss_item, &page_entry, &translated);

        assert_eq!(notification.channel, "beta");
        assert!(notification.is_hotfix);
        assert!(notification.glossary.is_empty());
    }

    #[test]
    fn find_by_revision() {
        let data = UpdatePageData {
            entries: vec![
                UpdatePageEntry {
                    revision: "2722".into(),
                    build_number: "724783".into(),
                    channel: ReleaseChannel::Release,
                    is_hotfix: false,
                },
                UpdatePageEntry {
                    revision: "2714".into(),
                    build_number: "722832".into(),
                    channel: ReleaseChannel::Release,
                    is_hotfix: false,
                },
            ],
        };
        let found = data.find_by_revision("2722");
        assert!(found.is_some());
        assert_eq!(found.unwrap().build_number, "724783");
        assert!(data.find_by_revision("9999").is_none());
    }

    #[test]
    fn find_by_build_number() {
        let data = UpdatePageData {
            entries: vec![UpdatePageEntry {
                revision: "2722".into(),
                build_number: "724783".into(),
                channel: ReleaseChannel::Release,
                is_hotfix: false,
            }],
        };
        let found = data.find_by_build_number("724783");
        assert!(found.is_some());
        assert_eq!(found.unwrap().revision, "2722");
        assert!(data.find_by_build_number("999999").is_none());
    }

    fn canonical_fixtures() -> (RssUpdateItem, UpdatePageEntry, TranslatedAnnouncement) {
        let rss_item = RssUpdateItem {
            build_number: "724783".into(),
            link: "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/".into(),
            description_html: String::new(),
            pub_date: DateTime::parse_from_rfc3339("2025-04-28T12:00:00Z")
                .unwrap()
                .to_utc(),
            revision: "2722".into(),
        };

        let page_entry = UpdatePageEntry {
            revision: "2722".into(),
            build_number: "724783".into(),
            channel: ReleaseChannel::Beta,
            is_hotfix: true,
        };

        let translated = TranslatedAnnouncement {
            original_text: "original english announcement".into(),
            translated_text: "中文译文，已含术语括号注释（座狼 (Varg)）".into(),
            search_results: vec![],
        };

        (rss_item, page_entry, translated)
    }

    #[test]
    fn build_canonical_field_mapping() {
        let (rss_item, page_entry, translated) = canonical_fixtures();

        let msg = build_canonical(&rss_item, &page_entry, "raw cleaned text", &translated)
            .expect("build_canonical failed");

        assert_eq!(msg.kind, MsgKind::Update as i32);
        assert!(!msg.id.is_empty(), "id 应为新 ULID");
        assert!(msg.id.len() == 26, "ULID 应为 26 字符，实际 {}", msg.id);

        let source = msg.source.expect("source missing");
        assert_eq!(source.kind, SourceKind::Rss as i32);
        assert_eq!(source.id, "klei-dst-rss");
        assert_eq!(source.display_name, "Klei 官方更新公告");

        assert_eq!(msg.dedup_key, "klei-dst-rss:724783");
        assert_eq!(msg.title, "饥荒联机版更新 build 724783 · r2722");
        assert_eq!(msg.body_markdown, "中文译文，已含术语括号注释（座狼 (Varg)）");
        assert_eq!(msg.body_text, "raw cleaned text");

        assert_eq!(msg.links.len(), 1);
        assert_eq!(msg.links[0].rel, "original");
        assert_eq!(
            msg.links[0].url,
            "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/"
        );
        assert!(msg.tags.is_empty());

        let ts = msg.published_at.expect("published_at missing");
        assert_eq!(ts.seconds, rss_item.pub_date.timestamp());
        assert_eq!(ts.nanos, rss_item.pub_date.timestamp_subsec_nanos() as i32);

        match msg.domain {
            Some(canonical_message::Domain::Dst(dst)) => {
                assert_eq!(dst.build_number, "724783");
                assert_eq!(dst.revision, "2722");
                assert_eq!(dst.channel, canonical_msg::ReleaseChannel::Beta as i32);
                assert!(dst.is_hotfix);
                assert_eq!(dst.original_description, "original english announcement");
            }
            Some(canonical_message::Domain::Chat(_)) => panic!("domain 应为 Dst，实际 Chat"),
            None => panic!("domain 应为 Dst，实际 None"),
        }
    }

    #[test]
    fn build_canonical_encode_roundtrip() {
        use canonical_msg::prost::Message;

        let (rss_item, page_entry, translated) = canonical_fixtures();
        let msg = build_canonical(&rss_item, &page_entry, "raw text", &translated)
            .expect("build_canonical failed");

        let bytes = msg.encode_to_vec();
        let decoded = CanonicalMessage::decode(bytes.as_slice()).expect("decode failed");
        assert_eq!(decoded, msg);
    }

    #[test]
    fn build_canonical_rejects_empty_translation() {
        let (rss_item, page_entry, translated) = canonical_fixtures();
        let mut empty = translated;
        empty.translated_text = "   \n  ".into();

        let result = build_canonical(&rss_item, &page_entry, "raw text", &empty);
        assert!(result.is_err());
    }
}
