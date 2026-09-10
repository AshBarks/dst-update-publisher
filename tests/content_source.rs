//! `content::load_source_markdown` 主/降级路径的集成测试（mock HTTP）。

mod common;

use common::read_fixture;
use dst_update_publisher::content::{MarkdownSource, fetch_post_markdown, load_source_markdown};

#[tokio::test]
async fn prefers_post_html_when_available() {
    let html = read_fixture("post_736959.html");
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/post")
        .with_status(200)
        .with_header("content-type", "text/html")
        .with_body(&html)
        .create_async()
        .await;

    let url = format!("{}/post", server.url());
    let (markdown, source) = load_source_markdown(&url, "RSS description fallback")
        .await
        .expect("load_source_markdown failed");

    assert_eq!(source, MarkdownSource::PostHtml);
    assert!(markdown.contains("## Changes"), "got: {markdown}");
    assert!(markdown.contains("[here]("), "got: {markdown}");
}

#[tokio::test]
async fn falls_back_to_description_on_http_error() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/post")
        .with_status(500)
        .create_async()
        .await;

    let description = read_fixture("description_752118.txt");
    let url = format!("{}/post", server.url());
    let (markdown, source) = load_source_markdown(&url, &description)
        .await
        .expect("fallback should succeed");

    assert_eq!(source, MarkdownSource::RssDescription);
    assert!(markdown.starts_with("## Changes"), "got: {markdown}");
    assert!(markdown.contains("\n## Bug Fixes\n"), "got: {markdown}");
    assert!(
        !markdown.contains('\t'),
        "tabs must be consumed: {markdown}"
    );
}

#[tokio::test]
async fn falls_back_to_description_when_rich_text_section_missing() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/post")
        .with_status(200)
        .with_body("<html><body><p>page redesign without the known section</p></body></html>")
        .create_async()
        .await;

    let description = read_fixture("description_752118.txt");
    let url = format!("{}/post", server.url());
    let (_, source) = load_source_markdown(&url, &description)
        .await
        .expect("fallback should succeed");

    assert_eq!(source, MarkdownSource::RssDescription);
}

#[tokio::test]
async fn errors_when_post_and_description_both_unusable() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/post")
        .with_status(404)
        .create_async()
        .await;

    let url = format!("{}/post", server.url());
    let result = load_source_markdown(&url, "   \n\t").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn fetch_post_markdown_rejects_non_post_page() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/post")
        .with_status(200)
        .with_body("<html><body>not a forum update page</body></html>")
        .create_async()
        .await;

    let url = format!("{}/post", server.url());
    assert!(fetch_post_markdown(&url).await.is_err());
}
