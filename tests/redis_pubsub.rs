mod common;

use common::{connect_test_redis, make_translated_announcement, unique_key};
use dst_update_publisher::models::{build_canonical, ReleaseChannel, RssUpdateItem, UpdatePageEntry};
use dst_update_publisher::publisher::publish_canonical;

/// Phase 3 后（06 Step 5.4）：canonical proto bytes 发布/订阅往返。
#[tokio::test]
async fn redis_pubsub_publish_and_receive_canonical() {
    use canonical_msg::prost::Message;

    let channel_name = unique_key("msgs.in");

    let mut pub_conn = connect_test_redis().await;

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

    publish_canonical(&mut pub_conn, &channel_name, &canonical)
        .await
        .expect("publish_canonical failed");

    let timeout = tokio::time::Duration::from_secs(5);
    let received = tokio::time::timeout(timeout, async {
        use futures::StreamExt;
        message_stream.next().await
    })
    .await
    .expect("timeout waiting for message");

    let msg = received.expect("should receive a message");
    let payload: Vec<u8> = msg.get_payload().expect("should get payload bytes");

    let decoded = canonical_msg::CanonicalMessage::decode(payload.as_slice())
        .expect("decode canonical failed");
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
}
