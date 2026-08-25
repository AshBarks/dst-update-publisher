mod common;

use common::{connect_test_redis, unique_key};
use dst_update_publisher::models::{
    MatchKind, PoSearchResult, PoTranslationEntry, UpdateNotification,
};
use dst_update_publisher::publisher::publish_update;

#[tokio::test]
async fn redis_pubsub_publish_and_receive() {
    let channel_name = unique_key("ch");

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

    let notification = UpdateNotification {
        build_number: "724783".into(),
        revision: "2722".into(),
        channel: "release".into(),
        is_hotfix: false,
        original_description: "original text".into(),
        translated_description: "翻译文本".into(),
        glossary: vec![PoSearchResult {
            term: "Varg".into(),
            candidates: vec![PoTranslationEntry {
                original: "Varg".into(),
                translation: "座狼".into(),
                kind: MatchKind::Exact,
            }],
        }],
        pub_date: "2025-04-28T12:00:00Z".into(),
        link: "https://example.com/update".into(),
    };

    publish_update(&mut pub_conn, &channel_name, &notification)
        .await
        .expect("publish_update failed");

    let timeout = tokio::time::Duration::from_secs(5);
    let received = tokio::time::timeout(timeout, async {
        use futures::StreamExt;
        message_stream.next().await
    })
    .await
    .expect("timeout waiting for message");

    let msg = received.expect("should receive a message");
    let payload: String = msg.get_payload().expect("should get payload string");

    let parsed: serde_json::Value = serde_json::from_str(&payload).expect("invalid JSON");
    assert_eq!(parsed["build_number"], "724783");
    assert_eq!(parsed["channel"], "release");
    assert_eq!(parsed["is_hotfix"], false);
    // glossary 是数组结构：term + 全部候选（含匹配类型）
    assert_eq!(parsed["glossary"][0]["term"], "Varg");
    assert_eq!(parsed["glossary"][0]["candidates"][0]["original"], "Varg");
    assert_eq!(
        parsed["glossary"][0]["candidates"][0]["translation"],
        "座狼"
    );
    assert_eq!(parsed["glossary"][0]["candidates"][0]["kind"], "exact");
}
