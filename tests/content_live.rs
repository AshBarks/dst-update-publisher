//! 线上历史版本验证（网络测试，默认跳过；`DST_LIVE_TEST=1` 开启）。
//!
//! 1. RSS 当前全部 PC 更新条目：逐条走"原帖 HTML → Markdown"主路径，并计算
//!    RSS 描述降级路径；
//! 2. 更新索引页（`game-updates/dst`）第 1-2 页的历史 build：验证更早的原帖
//!    （含论坛改版前后的不同排版）也能正常转换。
//!
//! 断言：输出不含 HTML 标签残留、不抛错、绝大多数含标题/列表结构；
//! 描述降级路径始终能产出非空、无 tab 的 Markdown。
//!
//! 运行：`DST_LIVE_TEST=1 cargo test --test content_live -- --nocapture`

mod common;

use common::make_config;
use dst_update_publisher::content::{
    MarkdownSource, description_to_markdown, fetch_post_markdown, load_source_markdown,
};
use dst_update_publisher::rss::fetch_rss_updates;
use futures::stream::{self, StreamExt};

const LIVE_RSS_URL: &str =
    "https://forums.kleientertainment.com/rss/6-dont-starve-together-updates.xml/";

const UPDATE_INDEX_URL: &str = "https://forums.kleientertainment.com/game-updates/dst/";

/// 历史索引验证的页码范围（每页约 25 个 build；默认 1-2 页 ≈ 50 个）。
const HISTORY_INDEX_PAGES: u32 = 2;

fn live_enabled() -> bool {
    std::env::var("DST_LIVE_TEST").ok().as_deref() == Some("1")
}

#[tokio::test]
async fn live_history_posts_convert_to_structured_markdown() {
    if !live_enabled() {
        eprintln!("skipping live history test; set DST_LIVE_TEST=1 to run");
        return;
    }

    let config = make_config(LIVE_RSS_URL, "");
    let items = fetch_rss_updates(&config)
        .await
        .expect("live RSS fetch failed");
    assert!(
        items.len() >= 5,
        "expected several PC update items, got {}",
        items.len()
    );
    println!("validating {} historical PC update posts", items.len());

    let checked = stream::iter(items.into_iter().map(|item| async move {
        let outcome = load_source_markdown(&item.link, &item.description_text).await;
        (item, outcome)
    }))
    .buffer_unordered(5)
    .collect::<Vec<_>>()
    .await;

    for (item, outcome) in checked {
        let (markdown, source) = outcome.unwrap_or_else(|e| {
            panic!(
                "build={} link={} failed: {}",
                item.build_number, item.link, e
            )
        });

        assert_eq!(
            source,
            MarkdownSource::PostHtml,
            "build={} link={} should use the post HTML path",
            item.build_number,
            item.link
        );
        assert!(
            markdown.contains("## ") || markdown.contains("- "),
            "build={} produced no structural markdown:\n{}",
            item.build_number,
            markdown
        );
        assert!(
            !markdown.contains("<p")
                && !markdown.contains("</")
                && !markdown.contains("<ul")
                && !markdown.contains("<li"),
            "build={} leaked raw HTML:\n{}",
            item.build_number,
            markdown
        );
        assert!(
            !markdown.contains("Update Information:"),
            "build={} kept the page boilerplate title",
            item.build_number
        );

        let fallback = description_to_markdown(&item.description_text);
        assert!(
            !fallback.trim().is_empty(),
            "build={} RSS description fallback is empty",
            item.build_number
        );
        assert!(
            !fallback.contains('\t'),
            "build={} fallback kept tabs",
            item.build_number
        );

        let headings = markdown.lines().filter(|l| l.starts_with("## ")).count();
        let items = markdown
            .lines()
            .filter(|l| l.trim_start().starts_with("- "))
            .count();
        println!(
            "build={} headings={} items={} chars={}",
            item.build_number,
            headings,
            items,
            markdown.chars().count()
        );
    }
}

/// 从更新索引页提取原帖链接（选择器与 `update_page.rs` 一致）。
fn extract_update_links(html: &str) -> Vec<String> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("li.cCmsRecord_row a.cRelease").expect("selector");
    document
        .select(&selector)
        .filter_map(|anchor| anchor.value().attr("href").map(str::to_string))
        .collect()
}

#[tokio::test]
async fn live_update_index_history_converts_without_html_leaks() {
    if !live_enabled() {
        eprintln!("skipping live index history test; set DST_LIVE_TEST=1 to run");
        return;
    }

    let mut links: Vec<String> = Vec::new();
    for page in 1..=HISTORY_INDEX_PAGES {
        let url = if page == 1 {
            UPDATE_INDEX_URL.to_string()
        } else {
            format!("{UPDATE_INDEX_URL}page/{page}/")
        };
        let html = reqwest::get(&url)
            .await
            .unwrap_or_else(|e| panic!("index page {page} fetch failed: {e}"))
            .text()
            .await
            .unwrap_or_else(|e| panic!("index page {page} body failed: {e}"));
        links.extend(extract_update_links(&html));
    }
    links.sort();
    links.dedup();
    assert!(
        links.len() >= 25,
        "expected at least 25 historical links, got {}",
        links.len()
    );
    println!("validating {} update-index historical posts", links.len());

    let results = stream::iter(links.into_iter().map(|link| async move {
        let result = fetch_post_markdown(&link).await;
        (link, result)
    }))
    .buffer_unordered(5)
    .collect::<Vec<_>>()
    .await;

    let mut structured = 0_usize;
    for (link, result) in &results {
        let markdown = result
            .as_ref()
            .unwrap_or_else(|e| panic!("{link} failed: {e}"));
        assert!(
            !markdown.contains("<p")
                && !markdown.contains("</")
                && !markdown.contains("<ul")
                && !markdown.contains("<li"),
            "{link} leaked raw HTML:\n{markdown}"
        );
        if markdown.contains("## ") || markdown.contains("- ") {
            structured += 1;
        }
    }
    // 纯散文公告（无标题/列表）占少数属正常，其余必须有结构。
    assert!(
        structured * 10 >= results.len() * 9,
        "only {structured}/{} posts had headings or lists",
        results.len()
    );
    println!(
        "{structured}/{} historical posts contain headings or lists",
        results.len()
    );
}
