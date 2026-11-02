//! 术语表：从英文源文里确定性地提取 PO 官方术语，译前注入提示词、译后校验。
//!
//! 背景：模型自己决定查询哪些术语时，会漏掉成批的物品名（实测 4B/9B/gemma
//! 都漏同一批皮肤名），译后无法凭中文定位错误，所以必须在译前把官方术语
//! 直接喂给模型。
//!
//! 匹配策略：源文按 token 切片，生成 1~5 词 n-gram，按最长优先匹配 PO 的
//! msgid（大小写不敏感）。只保留“像名字”的条目：多词短语全部保留；单词仅
//! 保留首字母大写、长度 ≥4、在 PO 多词条目中出现次数少、且不在通用名词表
//! 里的专名（Moggles/Icker/Sawhorse 保留，Chest/Shadow/Beefalo 丢弃）。

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::po_search::PoFileIndex;

/// 术语表上限：公告再长也不把提示词撑爆。
const MAX_GLOSSARY_TERMS: usize = 200;
/// PO 条目本身已限制 msgid ≤5 词，这里保持一致。
const MAX_TERM_WORDS: usize = 5;
/// 判定“通用名词”的词频阈值：出现在超过该数量多词 msgid 中的单词不算专名。
const SINGLE_WORD_FREQ_LIMIT: usize = 20;

/// 通用名词黑名单：PO 里首字母大写但并非专名，注入术语表只会造成噪声。
const GENERIC_SINGLE_WORDS: &[&str] = &[
    "caves",
    "cave",
    "chasm",
    "chest",
    "shadow",
    "rabbit",
    "beefalo",
    "stone",
    "wood",
    "skins",
    "things",
    "upgrade",
    "details",
    "login",
    "check",
    "choose",
    "craftables",
    "character",
    "little",
    "survival",
    "play",
    "twitch",
    "drop",
    "drops",
    "lots",
    "new",
    "more",
    "many",
    "team",
    "teams",
    "update",
    "updates",
    "game",
    "games",
    "item",
    "items",
];

/// 术语表条目：英文原词 → 官方中文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryEntry {
    pub term: String,
    pub translation: String,
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    start: usize,
    end: usize,
}

/// 源文 token：字母/数字开头，允许词内撇号、连字符与小数点数字（3.0）。
/// 句末标点不会被吞进 token（`Blight.` → `Blight`）。
static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9][A-Za-z0-9'’\-]*(?:\.[0-9]+)*").expect("token regex is valid")
});

/// 译文中的英文括注：`中文（English）` / `中文 (English)`，只用于删术语表
/// 覆盖的括注，URL 与价格不会被匹配。
static GLOSS_PAREN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[ \t]*[（(][ \t]*([A-Za-z][A-Za-z0-9'’.\- ]{0,64}?)[ \t]*[）)]")
        .expect("gloss regex is valid")
});

/// 从源文提取术语表，按首次出现位置排序。
pub fn collect(source: &str, index: &PoFileIndex) -> Vec<GlossaryEntry> {
    let tokens = tokenize(source);
    let mut used = vec![false; tokens.len()];
    let mut matched: Vec<(usize, GlossaryEntry)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for n in (1..=MAX_TERM_WORDS).rev() {
        if tokens.len() < n {
            continue;
        }
        for i in 0..=tokens.len() - n {
            if used[i..i + n].iter().any(|u| *u) {
                continue;
            }
            // 只允许用空格/制表符连接的多词短语，避免跨标点、跨行拼出假术语。
            if !tokens_adjacent(&tokens[i..i + n], source) {
                continue;
            }
            let phrase = tokens[i..i + n]
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");

            let Some((term, translation)) = lookup(&phrase, n, index) else {
                continue;
            };
            if !keep(&term, &tokens[i].text, index) {
                continue;
            }

            for slot in &mut used[i..i + n] {
                *slot = true;
            }
            if seen.insert(term.to_lowercase()) {
                matched.push((
                    tokens[i].start,
                    GlossaryEntry {
                        term: term.to_string(),
                        translation: translation.to_string(),
                    },
                ));
            }
        }
    }

    matched.sort_by_key(|(start, _)| *start);
    let mut entries: Vec<GlossaryEntry> = matched.into_iter().map(|(_, entry)| entry).collect();
    if entries.len() > MAX_GLOSSARY_TERMS {
        tracing::warn!(
            total = entries.len(),
            limit = MAX_GLOSSARY_TERMS,
            "glossary exceeds limit; truncating (later terms will not be injected)"
        );
        entries.truncate(MAX_GLOSSARY_TERMS);
    }
    entries
}

fn lookup(phrase: &str, words: usize, index: &PoFileIndex) -> Option<(String, String)> {
    if let Some((term, translation)) = index.lookup_msgid_entry(phrase) {
        return Some((term.to_string(), translation.to_string()));
    }
    // 所有格：PO 里 "Bearger" 不会匹配源文 "Bearger's"。
    if words == 1 {
        for suffix in ["'s", "’s"] {
            if let Some(stripped) = phrase.strip_suffix(suffix)
                && let Some((term, translation)) = index.lookup_msgid_entry(stripped)
            {
                return Some((term.to_string(), translation.to_string()));
            }
        }
    }
    None
}

fn keep(term: &str, source_first_token: &str, index: &PoFileIndex) -> bool {
    if !term.chars().any(|c| c.is_uppercase()) || !term.chars().any(|c| c.is_alphabetic()) {
        return false;
    }
    // 带句读的条目是整句 UI 文本，不是术语。
    if term.contains(['?', '!', ':', ';']) {
        return false;
    }

    let words: Vec<&str> = term.split_whitespace().collect();
    if words.len() > 1 {
        return true;
    }

    let lower = term.to_lowercase();
    if lower.chars().count() < 4 {
        return false;
    }
    // 单词术语必须与源文大小写完全一致：PO 里的 "BEES" 是 UI 标签，
    // 不能匹配源文的 "Bees"；"Moggles"/"Varg" 这类专名才通过。
    let source_base = source_first_token
        .trim_end_matches("'s")
        .trim_end_matches("’s");
    if term != source_base {
        return false;
    }
    if !source_first_token
        .chars()
        .next()
        .is_some_and(char::is_uppercase)
    {
        return false;
    }
    if GENERIC_SINGLE_WORDS.contains(&lower.as_str()) {
        return false;
    }
    index.multiword_word_count(&lower) <= SINGLE_WORD_FREQ_LIMIT
}

fn tokenize(source: &str) -> Vec<Token> {
    TOKEN_RE
        .find_iter(source)
        .map(|m| Token {
            text: m.as_str().to_string(),
            start: m.start(),
            end: m.end(),
        })
        .collect()
}

/// 相邻 token 之间只能是空格/制表符（多词短语的合法分隔）。
fn tokens_adjacent(tokens: &[Token], source: &str) -> bool {
    tokens.windows(2).all(|pair| {
        source[pair[0].end..pair[1].start]
            .chars()
            .all(|c| c == ' ' || c == '\t')
    })
}

/// 生成注入提示词的术语表文本；无条目时返回空串。
pub fn prompt_block(entries: &[GlossaryEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut block = String::from(
        "Official glossary (authoritative; use these exact Chinese translations, \
         do not translate them yourself, and never append the English original):\n",
    );
    for entry in entries {
        block.push_str("- ");
        block.push_str(&entry.term);
        block.push_str(" => ");
        block.push_str(&entry.translation);
        block.push('\n');
    }
    block
}

/// 删除译文中术语表覆盖的英文括注（纯中文输出要求）。
pub fn strip_glossary_glosses(text: &str, entries: &[GlossaryEntry]) -> String {
    if entries.is_empty() {
        return text.to_string();
    }
    let terms: HashSet<String> = entries.iter().map(|e| e.term.to_lowercase()).collect();
    GLOSS_PAREN_RE
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let inner = caps
                .get(1)
                .map(|m| m.as_str().trim().to_lowercase())
                .unwrap_or_default();
            if terms.contains(&inner) {
                String::new()
            } else {
                caps.get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default()
            }
        })
        .into_owned()
}

/// 译文中缺失官方译名的术语（仅用于 WARN 指标，不阻断发布）。
pub fn missing_terms<'a>(text: &str, entries: &'a [GlossaryEntry]) -> Vec<&'a str> {
    entries
        .iter()
        .filter(|entry| !text.contains(&entry.translation))
        .map(|entry| entry.term.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str, translation: &str) -> GlossaryEntry {
        GlossaryEntry {
            term: term.to_string(),
            translation: translation.to_string(),
        }
    }

    #[test]
    fn prompt_block_lists_entries() {
        let block = prompt_block(&[entry("Varg", "座狼")]);
        assert!(block.contains("Varg => 座狼"));
    }

    #[test]
    fn prompt_block_empty_when_no_terms() {
        assert!(prompt_block(&[]).is_empty());
    }

    #[test]
    fn strip_removes_glossary_gloss_only() {
        let entries = vec![entry("Varg", "座狼")];
        assert_eq!(
            strip_glossary_glosses("座狼 (Varg) 很危险。", &entries),
            "座狼 很危险。"
        );
        assert_eq!(
            strip_glossary_glosses("座狼（Varg）很危险。", &entries),
            "座狼很危险。"
        );
    }

    #[test]
    fn strip_keeps_non_glossary_parentheses() {
        let entries = vec![entry("Varg", "座狼")];
        // URL 本体保留，术语括注删除。
        let url = "详见 https://example.com/x (Varg)";
        assert_eq!(
            strip_glossary_glosses(url, &entries),
            "详见 https://example.com/x"
        );
        let price = "全套箱子（$15.99usd）";
        assert_eq!(strip_glossary_glosses(price, &entries), price);
    }

    #[test]
    fn strip_is_idempotent() {
        let entries = vec![entry("Varg", "座狼")];
        let once = strip_glossary_glosses("座狼 (Varg)", &entries);
        let twice = strip_glossary_glosses(&once, &entries);
        assert_eq!(once, twice);
    }

    #[test]
    fn missing_terms_reports_absent_translations() {
        let entries = vec![entry("Varg", "座狼"), entry("Moleworm", "鼹鼠")];
        let missing = missing_terms("座狼出现了。", &entries);
        assert_eq!(missing, vec!["Moleworm"]);
    }
}
