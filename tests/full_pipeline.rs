mod common;

use common::{
    cleanup_key, connect_test_redis, make_config_with_redis, make_translated_announcement,
    read_fixture, unique_key,
};
use dst_update_publisher::models::UpdateNotification;
use dst_update_publisher::publisher::{is_build_processed, mark_build_processed, publish_update};
use dst_update_publisher::rss::{fetch_rss_updates, get_latest_pc_update};
use dst_update_publisher::update_page::{fetch_update_page, get_version_info};

#[tokio::test]
async fn full_pipeline_rss_to_redis() {
    let rss_xml = read_fixture("rss.xml");
    let html = read_fixture("update.html");

    let mut rss_server = mockito::Server::new_async().await;
    rss_server
        .mock("GET", "/rss")
        .with_status(200)
        .with_header("content-type", "application/xml")
        .with_body(&rss_xml)
        .create_async()
        .await;

    let mut page_server = mockito::Server::new_async().await;
    page_server
        .mock("GET", "/")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body(&html)
        .create_async()
        .await;

    let redis_channel = unique_key("ch");
    let redis_dedupe_key = unique_key("dedup");

    let config = make_config_with_redis(
        &format!("{}/rss", rss_server.url()),
        &page_server.url(),
        &redis_channel,
        &redis_dedupe_key,
    );

    let mut redis_conn = connect_test_redis().await;

    let rss_items = fetch_rss_updates(&config)
        .await
        .expect("fetch_rss_updates failed");
    let latest = get_latest_pc_update(&rss_items).expect("should have latest");

    let is_processed = is_build_processed(&mut redis_conn, &redis_dedupe_key, &latest.build_number)
        .await
        .expect("is_build_processed failed");
    assert!(!is_processed, "should not be processed initially");

    let page_data = fetch_update_page(&config)
        .await
        .expect("fetch_update_page failed");
    let page_entry = get_version_info(&page_data, &latest.revision, &latest.build_number)
        .expect("get_version_info failed");

    let translated = make_translated_announcement();
    let notification = UpdateNotification::compose(latest, &page_entry, &translated);

    assert_eq!(notification.build_number, latest.build_number);

    publish_update(&mut redis_conn, &redis_channel, &notification)
        .await
        .expect("publish_update failed");

    mark_build_processed(&mut redis_conn, &redis_dedupe_key, &latest.build_number)
        .await
        .expect("mark_build_processed failed");

    let is_processed = is_build_processed(&mut redis_conn, &redis_dedupe_key, &latest.build_number)
        .await
        .expect("is_build_processed failed");
    assert!(is_processed, "should be processed after full pipeline");

    cleanup_key(&mut redis_conn, &redis_dedupe_key).await;
}
