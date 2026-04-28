mod common;

use common::{make_config, read_fixture};
use dst_update_publisher::models::ReleaseChannel;
use dst_update_publisher::update_page::{fetch_update_page, get_version_info};

#[tokio::test]
async fn update_page_fetch_parses_entries() {
    let html = read_fixture("update.html");

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body(&html)
        .create_async()
        .await;

    let config = make_config("", &server.url());

    let data = fetch_update_page(&config)
        .await
        .expect("fetch_update_page failed");
    mock.assert_async().await;

    assert!(!data.entries.is_empty(), "should parse entries");
}

#[tokio::test]
async fn update_page_fetch_release_entry() {
    let html = read_fixture("update.html");

    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/")
        .with_status(200)
        .with_body(&html)
        .create_async()
        .await;

    let config = make_config("", &server.url());
    let data = fetch_update_page(&config).await.expect("fetch failed");

    let entry = data.find_by_revision("2714").expect("should find r2714");
    assert!(matches!(entry.channel, ReleaseChannel::Release));
    assert!(!entry.is_hotfix);
}

#[tokio::test]
async fn update_page_fetch_beta_hotfix() {
    let html = read_fixture("update.html");

    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/")
        .with_status(200)
        .with_body(&html)
        .create_async()
        .await;

    let config = make_config("", &server.url());
    let data = fetch_update_page(&config).await.expect("fetch failed");

    let entry = data.find_by_revision("2722").expect("should find r2722");
    assert!(matches!(entry.channel, ReleaseChannel::Beta));
    assert!(entry.is_hotfix);
}

#[tokio::test]
async fn update_page_get_version_info() {
    let html = read_fixture("update.html");

    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/")
        .with_status(200)
        .with_body(&html)
        .create_async()
        .await;

    let config = make_config("", &server.url());
    let data = fetch_update_page(&config).await.expect("fetch failed");

    let result = get_version_info(&data, "2722", "724783");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().build_number, "724783");

    let not_found = get_version_info(&data, "9999", "999999");
    assert!(not_found.is_err());
}
