//! 术语表提取与译文清理的集成测试（用真实 PO fixture）。

use dst_update_publisher::content::{decode_html_entities, description_to_markdown};
use dst_update_publisher::glossary::{
    GlossaryEntry, collect, missing_terms, prompt_block, strip_glossary_glosses,
};
use dst_update_publisher::po_search::build_index_from_po_contents;

fn fixture_index() -> dst_update_publisher::po_search::PoFileIndex {
    let po = std::fs::read_to_string("tests/fixtures/chinese_s.po")
        .expect("failed to read tests/fixtures/chinese_s.po");
    build_index_from_po_contents(&[po]).expect("failed to build PO index")
}

fn find<'a>(entries: &'a [GlossaryEntry], term: &str) -> Option<&'a GlossaryEntry> {
    entries
        .iter()
        .find(|entry| entry.term.eq_ignore_ascii_case(term))
}

#[test]
fn collects_official_terms_from_source() {
    let index = fixture_index();
    let source = "Added Varg and Grumble Bees to the Caves.\n\
                  Varg's Nightmare Fuel is required to craft a Moleworm.\n\
                  Choose wisely. The Chest contains an Ocuvigil.";
    let entries = collect(source, &index);

    assert_eq!(
        find(&entries, "Varg").map(|e| e.translation.as_str()),
        Some("座狼")
    );
    assert_eq!(
        find(&entries, "Nightmare Fuel").map(|e| e.translation.as_str()),
        Some("噩梦燃料")
    );
    assert_eq!(
        find(&entries, "Moleworm").map(|e| e.translation.as_str()),
        Some("鼹鼠")
    );
    assert_eq!(
        find(&entries, "Ocuvigil").map(|e| e.translation.as_str()),
        Some("月眼守卫")
    );
    assert!(
        find(&entries, "Nightmare Fuel").is_some(),
        "multi-word terms should be collected"
    );

    // 通用名词与句首动词不应进入术语表。
    for noise in ["Chest", "Caves", "Choose"] {
        assert!(
            find(&entries, noise).is_none(),
            "generic term '{noise}' should be filtered out"
        );
    }

    // 按源文首次出现顺序排列。
    assert_eq!(entries.first().map(|e| e.term.as_str()), Some("Varg"));
}

#[test]
fn matches_terms_before_trailing_punctuation() {
    let index = fixture_index();
    let entries = collect("Added a Varg.", &index);
    assert_eq!(
        find(&entries, "Varg").map(|e| e.translation.as_str()),
        Some("座狼")
    );
}

#[test]
fn matches_possessive_single_word() {
    let index = fixture_index();
    let entries = collect("Varg's howl echoed.", &index);
    assert!(
        find(&entries, "Varg").is_some(),
        "possessive form should still match the base term"
    );
}

#[test]
fn prompt_block_is_authoritative_format() {
    let index = fixture_index();
    let entries = collect("Added a Varg.", &index);
    let block = prompt_block(&entries);
    assert!(block.contains("Varg => 座狼"));
    assert!(block.contains("authoritative"));
}

#[test]
fn strips_english_gloss_for_collected_term() {
    let index = fixture_index();
    let entries = collect("Added a Varg.", &index);
    assert_eq!(
        strip_glossary_glosses("座狼 (Varg) 出现在森林里。", &entries),
        "座狼 出现在森林里。"
    );
    // 非术语表括注保留（价格）；URL 本身保留，术语括注仍删除。
    let text = "详见 https://example.com/a (Varg)；价格（$15.99usd）";
    assert_eq!(
        strip_glossary_glosses(text, &entries),
        "详见 https://example.com/a；价格（$15.99usd）"
    );
}

#[test]
fn missing_terms_flags_unused_glossary_entries() {
    let index = fixture_index();
    let entries = collect("Added a Varg and a Moleworm.", &index);
    let missing = missing_terms("座狼出现了。", &entries);
    assert!(missing.contains(&"Moleworm"));
    assert!(!missing.contains(&"Varg"));
}

#[test]
fn decodes_html_entities_in_source_markdown() {
    assert_eq!(decode_html_entities("a&nbsp;b &amp; c"), "a b & c");
    let markdown = description_to_markdown("The&nbsp;Rosebloom Vest is here!");
    assert!(!markdown.contains("&nbsp;"));
    assert!(markdown.contains("The Rosebloom Vest is here!"));
}
