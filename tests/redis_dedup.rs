mod common;

use common::{cleanup_key, connect_test_redis, unique_key};
use dst_update_publisher::publisher::{is_build_processed, mark_build_processed};

#[tokio::test]
async fn redis_dedup_unprocessed_then_mark_then_processed() {
    let mut conn = connect_test_redis().await;
    let dedup_key = unique_key("dedup");
    let build_number = "724783";

    let is_processed = is_build_processed(&mut conn, &dedup_key, build_number)
        .await
        .expect("is_build_processed failed");
    assert!(!is_processed, "should not be processed initially");

    mark_build_processed(&mut conn, &dedup_key, build_number)
        .await
        .expect("mark_build_processed failed");

    let is_processed = is_build_processed(&mut conn, &dedup_key, build_number)
        .await
        .expect("is_build_processed failed");
    assert!(is_processed, "should be processed after marking");

    cleanup_key(&mut conn, &dedup_key).await;
}

#[tokio::test]
async fn redis_dedup_different_build_not_matched() {
    let mut conn = connect_test_redis().await;
    let dedup_key = unique_key("dedup");

    mark_build_processed(&mut conn, &dedup_key, "724783")
        .await
        .expect("mark failed");

    let is_other_processed = is_build_processed(&mut conn, &dedup_key, "999999")
        .await
        .expect("is_build_processed failed");
    assert!(
        !is_other_processed,
        "different build_number should not match"
    );

    cleanup_key(&mut conn, &dedup_key).await;
}
