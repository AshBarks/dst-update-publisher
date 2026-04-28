mod common;

use common::make_config;
use dst_update_publisher::rss::{fetch_rss_updates, get_latest_pc_update};

#[tokio::test]
async fn no_update_when_only_console_entries() {
    let non_pc_rss = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
<channel>
<title>DST Updates</title>
<item>
<title>112100</title>
<link>https://forums.kleientertainment.com/game-updates/dst_xboxone/112100-r2721/</link>
<description>Xbox update</description>
<pubDate>Wed, 22 Apr 2026 03:33:51 +0800</pubDate>
</item>
<item>
<title>3380</title>
<link>https://forums.kleientertainment.com/game-updates/dst_ps4/3380-r2719/</link>
<description>PS4 update</description>
<pubDate>Tue, 21 Apr 2026 05:59:22 +0800</pubDate>
</item>
</channel>
</rss>"#;

    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/rss")
        .with_status(200)
        .with_header("content-type", "application/xml")
        .with_body(non_pc_rss)
        .create_async()
        .await;

    let config = make_config(&format!("{}/rss", server.url()), "");

    let items = fetch_rss_updates(&config)
        .await
        .expect("fetch_rss_updates failed");

    assert!(items.is_empty(), "should find no PC update items");

    let latest = get_latest_pc_update(&items);
    assert!(latest.is_none(), "no latest when no PC items");
}
