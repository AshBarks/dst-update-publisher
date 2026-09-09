mod common;

use common::{cleanup_key, connect_test_redis, make_translated_announcement, unique_key};
use dst_update_publisher::models::{
    ReleaseChannel, RssUpdateItem, UpdatePageEntry, build_canonical,
};
use dst_update_publisher::publisher::publish_canonical;

/// Phase 5（16-B）：canonical proto bytes XADD → XRANGE 读回往返。
#[tokio::test]
async fn redis_stream_xadd_and_readback_canonical() {
    use canonical_msg::prost::Message;

    let stream_name = unique_key("stream:msgs:in");

    let mut conn = connect_test_redis().await;

    let rss_item = RssUpdateItem {
        build_number: "724783".into(),
        link: "https://example.com/update".into(),
        description_html: String::new(),
        pub_date: chrono::Utc::now(),
        revision: "2722".into(),
    };
    let page_entry = UpdatePageEntry {
        revision: "2722".into(),
        build_number: "724783".into(),
        channel: ReleaseChannel::Release,
        is_hotfix: false,
    };
    let translated = make_translated_announcement();
    let canonical = build_canonical(&rss_item, &page_entry, "raw cleaned text", &translated)
        .expect("build_canonical failed");

    publish_canonical(&mut conn, &stream_name, &canonical)
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

    let decoded =
        canonical_msg::CanonicalMessage::decode(bytes.as_slice()).expect("decode canonical failed");
    assert_eq!(decoded.kind, canonical_msg::MsgKind::Update as i32);
    assert_eq!(decoded.dedup_key, "klei-dst-rss:724783");
    assert_eq!(decoded.title, "饥荒联机版更新 build 724783 · r2722");
    match decoded.domain.expect("domain must be set") {
        canonical_msg::canonical_message::Domain::Dst(dst) => {
            assert_eq!(dst.build_number, "724783");
            assert_eq!(dst.revision, "2722");
        }
        _ => panic!("expected dst domain"),
    }

    cleanup_key(&mut conn, &stream_name).await;
}
