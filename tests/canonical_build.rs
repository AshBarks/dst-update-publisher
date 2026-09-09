mod common;

use canonical_msg::{MsgKind, canonical_message};
use dst_update_publisher::failure::{AlertDecision, build_alert_message, decide};
use dst_update_publisher::models::{
    ReleaseChannel, RssUpdateItem, TranslatedAnnouncement, UpdatePageEntry, build_canonical,
};

fn canonical_fixture() -> (RssUpdateItem, UpdatePageEntry, TranslatedAnnouncement) {
    let rss_item = RssUpdateItem {
        build_number: "724783".into(),
        link: "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/".into(),
        description_html: String::new(),
        pub_date: chrono::DateTime::parse_from_rfc3339("2025-04-28T12:00:00Z")
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
        original_text: "Changes Added Chessmaster Circuit.".into(),
        translated_text: "更改 新增国际象棋大师电路。".into(),
        search_results: vec![],
    };

    (rss_item, page_entry, translated)
}

#[test]
fn build_canonical_maps_all_fields() {
    let (rss_item, page_entry, translated) = canonical_fixture();

    let msg = build_canonical(&rss_item, &page_entry, "raw cleaned text", &translated)
        .expect("build_canonical failed");

    assert_eq!(msg.kind, MsgKind::Update as i32);
    assert_eq!(msg.id.len(), 26, "id 应为 ULID");
    assert_eq!(msg.source.as_ref().unwrap().id, "klei-dst-rss");
    assert_eq!(msg.dedup_key, "klei-dst-rss:724783");
    assert_eq!(msg.title, "饥荒联机版更新 build 724783 · r2722");
    assert_eq!(msg.body_markdown, "更改 新增国际象棋大师电路。");
    assert_eq!(msg.body_text, "raw cleaned text");
    assert_eq!(msg.links.len(), 1);
    assert_eq!(msg.links[0].rel, "original");
    assert_eq!(
        msg.links[0].url,
        "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/"
    );
    assert!(msg.tags.is_empty());
    assert_eq!(
        msg.published_at.as_ref().unwrap().seconds,
        rss_item.pub_date.timestamp()
    );
    match msg.domain.as_ref() {
        Some(canonical_message::Domain::Dst(dst)) => {
            assert_eq!(dst.build_number, "724783");
            assert_eq!(dst.revision, "2722");
            assert_eq!(dst.channel, canonical_msg::ReleaseChannel::Release as i32);
            assert!(!dst.is_hotfix);
            assert_eq!(
                dst.original_description,
                "Changes Added Chessmaster Circuit."
            );
        }
        Some(canonical_message::Domain::Chat(_)) => panic!("domain 应为 Dst"),
        None => panic!("domain 缺失"),
    }
}

#[test]
fn build_canonical_encode_roundtrip() {
    use canonical_msg::prost::Message;

    let (rss_item, page_entry, translated) = canonical_fixture();
    let msg = build_canonical(&rss_item, &page_entry, "raw", &translated).unwrap();
    let bytes = msg.encode_to_vec();
    let decoded = canonical_msg::CanonicalMessage::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn build_canonical_rejects_empty_translation() {
    let (rss_item, page_entry, translated) = canonical_fixture();
    let mut translated = translated;
    translated.translated_text = " \t\n ".into();
    assert!(build_canonical(&rss_item, &page_entry, "raw", &translated).is_err());
}

#[test]
fn decide_first_on_first_failure() {
    assert_eq!(decide(1, 1000, 2000, 5, 24), AlertDecision::First);
}

#[test]
fn decide_silent_between_first_and_escalation() {
    for count in 2..5 {
        assert_eq!(decide(count, 1000, 2000, 5, 24), AlertDecision::Silent);
    }
}

#[test]
fn decide_escalate_by_count_threshold() {
    assert_eq!(decide(5, 1000, 2000, 5, 24), AlertDecision::Escalate);
    assert_eq!(decide(9, 1000, 2000, 5, 24), AlertDecision::Escalate);
}

#[test]
fn decide_escalate_by_time_threshold() {
    let sec = 24 * 3600;
    // 距首次失败 24h 整前仍 Silent，达到即 Escalate
    assert_eq!(
        decide(2, 1000, 1000 + sec - 1, 5, 24),
        AlertDecision::Silent
    );
    assert_eq!(decide(2, 1000, 1000 + sec, 5, 24), AlertDecision::Escalate);
}

#[test]
fn alert_first_construction() {
    // 1700000000 = 2023-11-14T22:13:20Z（UTC 日期用于 dedup_key）
    let first_ts = 1700000000;
    let now_ts = 1700001000;

    let msg = build_alert_message(first_ts, now_ts, "fetch_rss", "boom", 1, false);

    assert_eq!(msg.kind, MsgKind::Alert as i32);
    assert_eq!(msg.title, "DST 更新抓取失败");
    assert!(msg.tags.is_empty());
    assert_eq!(msg.dedup_key, "klei-dst-rss:alert:2023-11-14");
    assert!(msg.body_markdown.contains("fetch_rss"));
    assert!(msg.body_markdown.contains("boom"));
    assert!(msg.body_markdown.contains("1"));
    assert_eq!(msg.body_text, msg.body_markdown);
    assert!(msg.links.is_empty());
    assert!(msg.domain.is_none());
    assert_eq!(msg.source.as_ref().unwrap().id, "klei-dst-rss");
    assert_eq!(msg.published_at.as_ref().unwrap().seconds, now_ts);
}

#[test]
fn alert_escalated_construction() {
    let msg = build_alert_message(1700000000, 1700001000, "translate", "boom", 5, true);

    assert_eq!(msg.title, "DST 更新抓取失败（持续失败升级）");
    assert_eq!(msg.tags, vec!["alert:escalated"]);
    assert_eq!(msg.dedup_key, "klei-dst-rss:alert:2023-11-14");
}
