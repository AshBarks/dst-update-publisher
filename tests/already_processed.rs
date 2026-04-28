mod common;

use common::{cleanup_key, connect_test_redis, unique_key};
use dst_update_publisher::publisher::{is_build_processed, mark_build_processed};

#[tokio::test]
async fn already_processed_skips_pipeline() {
    let mut conn = connect_test_redis().await;
    let dedup_key = unique_key("dedup");
    let build_number = "724783";

    mark_build_processed(&mut conn, &dedup_key, build_number)
        .await
        .expect("mark_build_processed failed");

    let is_processed = is_build_processed(&mut conn, &dedup_key, build_number)
        .await
        .expect("is_build_processed failed");
    assert!(
        is_processed,
        "should be already processed, simulating AlreadyProcessed"
    );

    cleanup_key(&mut conn, &dedup_key).await;
}
