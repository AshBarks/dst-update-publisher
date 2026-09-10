mod common;

use canonical_msg::prost::Message;
use common::{cleanup_key, connect_test_redis, unique_key};
use dst_update_publisher::models::{
    ReleaseChannel, RssUpdateItem, TranslatedAnnouncement, UpdatePageEntry, build_canonical,
};
use dst_update_publisher::publisher::{
    canonical_dedup_key, is_build_processed, mark_build_processed, publish_canonical,
};

fn sample_canonical() -> canonical_msg::CanonicalMessage {
    let rss_item = RssUpdateItem {
        build_number: "724783".into(),
        link: "https://forums.kleientertainment.com/game-updates/dst/724783-r2722/".into(),
        description_text: String::new(),
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
    build_canonical(&rss_item, &page_entry, &translated).expect("build_canonical failed")
}

#[tokio::test]
async fn canonical_xadd_readback_roundtrip() {
    let mut conn = connect_test_redis().await;
    let stream_name = unique_key("stream:msgs:in");

    let msg = sample_canonical();
    publish_canonical(&mut conn, &stream_name, &msg)
        .await
        .expect("publish_canonical failed");

    let reply: redis::streams::StreamRangeReply = redis::cmd("XRANGE")
        .arg(&stream_name)
        .arg("-")
        .arg("+")
        .arg("COUNT")
        .arg(1)
        .query_async(&mut conn)
        .await
        .expect("XRANGE failed");

    assert_eq!(reply.ids.len(), 1, "stream 应恰好读到一条 entry");
    let bytes: Vec<u8> = reply.ids[0].get("data").expect("entry 应含 data field");

    let decoded = canonical_msg::CanonicalMessage::decode(bytes.as_slice())
        .expect("payload should decode to canonical");

    assert_eq!(decoded, msg);

    cleanup_key(&mut conn, &stream_name).await;
}

#[tokio::test]
async fn mark_writes_both_legacy_and_canonical_keys() {
    let mut conn = connect_test_redis().await;
    let old_key = unique_key("dedup");
    // 随机 build 号，避免写入真实 dedup:klei-dst-rss:* 命名空间
    let build_number = format!("b{}", uuid::Uuid::new_v4());

    mark_build_processed(&mut conn, &old_key, &build_number)
        .await
        .expect("mark_build_processed failed");

    let old: String = redis::cmd("GET")
        .arg(&old_key)
        .query_async(&mut conn)
        .await
        .expect("GET legacy key failed");
    let new: String = redis::cmd("GET")
        .arg(canonical_dedup_key(&build_number))
        .query_async(&mut conn)
        .await
        .expect("GET canonical key failed");

    assert_eq!(old, build_number);
    assert_eq!(new, build_number);

    // 判定逻辑不变：是否已处理仍只看旧键
    assert!(
        is_build_processed(&mut conn, &old_key, &build_number)
            .await
            .expect("is_build_processed failed"),
        "legacy key should drive the dedup decision"
    );

    cleanup_key(&mut conn, &old_key).await;
    cleanup_key(&mut conn, &canonical_dedup_key(&build_number)).await;
}

/// 回归防护（2026-09-10）：Phase 3 删旧格式时 `run_pipeline` 里残留了第二处
/// `publish_canonical` 调用，每条 UPDATE 被 XADD 两次（双推事故）。既有集成测试
/// 都直接调 `publish_canonical`，覆盖不到 `main.rs` 的调用点，这里静态断言唯一性。
#[test]
fn pipeline_publishes_canonical_exactly_once() {
    let main_rs = include_str!("../src/main.rs");
    assert_eq!(
        main_rs.matches("publish_canonical(").count(),
        1,
        "run_pipeline 必须恰好调用一次 publish_canonical（重复调用会导致双推）"
    );
}
