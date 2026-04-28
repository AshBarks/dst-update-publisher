mod common;

use common::{make_config, read_fixture};
use dst_update_publisher::rss::{fetch_rss_updates, get_latest_pc_update};

#[tokio::test]
async fn rss_fetch_finds_pc_items() {
    let rss_xml = read_fixture("rss.xml");

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/rss")
        .with_status(200)
        .with_header("content-type", "application/xml")
        .with_body(&rss_xml)
        .create_async()
        .await;

    let config = make_config(&format!("{}/rss", server.url()), "");

    let items = fetch_rss_updates(&config)
        .await
        .expect("fetch_rss_updates failed");

    mock.assert_async().await;
    assert!(!items.is_empty(), "should find PC update items");

    for item in &items {
        assert!(item.is_pc_update(), "all items should be PC updates");
    }
}

#[tokio::test]
async fn rss_fetch_latest_pc_update() {
    let rss_xml = read_fixture("rss.xml");

    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/rss")
        .with_status(200)
        .with_header("content-type", "application/xml")
        .with_body(&rss_xml)
        .create_async()
        .await;

    let config = make_config(&format!("{}/rss", server.url()), "");

    let items = fetch_rss_updates(&config)
        .await
        .expect("fetch_rss_updates failed");
    mock.assert_async().await;

    let latest = get_latest_pc_update(&items).expect("should have latest item");

    for item in &items {
        assert!(
            item.pub_date <= latest.pub_date,
            "latest should have the most recent pub_date"
        );
    }

    assert!(!latest.revision.is_empty(), "revision should be extracted");
    assert!(
        !latest.build_number.is_empty(),
        "build_number should be present"
    );
}

#[tokio::test]
async fn rss_fetch_filters_non_pc() {
    let rss_xml = read_fixture("rss.xml");
    let channel = rss::Channel::read_from(rss_xml.as_bytes()).unwrap();
    let total = channel.items.len();

    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/rss")
        .with_status(200)
        .with_header("content-type", "application/xml")
        .with_body(&rss_xml)
        .create_async()
        .await;

    let config = make_config(&format!("{}/rss", server.url()), "");

    let items = fetch_rss_updates(&config)
        .await
        .expect("fetch_rss_updates failed");

    assert!(
        items.len() < total,
        "PC items ({}) should be fewer than total ({})",
        items.len(),
        total
    );
}
