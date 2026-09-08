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
    build_canonical(&rss_item, &page_entry, "raw", &translated).expect("build_canonical failed")
}

#[tokio::test]
async fn canonical_publish_subscribe_roundtrip() {
    let mut conn = connect_test_redis().await;
    let channel_name = unique_key("canon");

    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let sub_client = redis::Client::open(redis_url).expect("redis client");
    let mut pubsub = sub_client
        .get_async_pubsub()
        .await
        .expect("get_async_pubsub failed");

    pubsub
        .subscribe(&channel_name)
        .await
        .expect("subscribe failed");

    let mut message_stream = pubsub.on_message();

    let msg = sample_canonical();
    publish_canonical(&mut conn, &channel_name, &msg)
        .await
        .expect("publish_canonical failed");

    let timeout = tokio::time::Duration::from_secs(5);
    let received = tokio::time::timeout(timeout, async {
        use futures::StreamExt;
        message_stream.next().await
    })
    .await
    .expect("timeout waiting for message");

    let payload: Vec<u8> = received
        .expect("should receive a message")
        .get_payload()
        .expect("should get binary payload");

    let decoded = canonical_msg::CanonicalMessage::decode(payload.as_slice())
        .expect("payload should decode to canonical");

    assert_eq!(decoded, msg);
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
