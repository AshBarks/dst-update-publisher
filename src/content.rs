//! 公告正文获取与结构化（Markdown）。
//!
//! 主路径：抓取 RSS 条目对应的论坛原帖 HTML，把 `section.ipsType_richText`
//! 内的富文本转换为 Markdown（`##` 节标题 / `-` 列表 / 嵌套缩进 / 链接），
//! 翻译链路在保留结构的前提下翻译（见 `translator`）。
//!
//! 降级路径：原帖抓取或解析失败时，从 RSS `<description>` 的缩进层级重建
//! Markdown。RSS 描述是论坛导出的纯文本（无 HTML 标签，但保留 tab 缩进与
//! 空行），层级可恢复但不如原帖精确。
//!
//! `markdown_to_plain` 供 canonical `body_text` 使用（中文纯文本降级链底端）。

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Duration;

use scraper::{CaseSensitivity, ElementRef, Html, Node, Selector};

use crate::error::{AppError, AppResult};

/// 原帖正文选择器：与旧 `dst-update` 仓库一致，已在 25 篇历史公告上验证。
const RICH_TEXT_SELECTOR: &str = "section.ipsType_richText";

/// 主路径超时：原帖页面请求超过该时长即走 RSS 描述降级，避免拖死轮询。
const POST_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 结构标题长度上限：超过视为正文（整段加粗的长句不是标题）。
const HEADING_MAX_CHARS: usize = 60;

/// 正文 Markdown 的来源，仅用于日志与测试断言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownSource {
    /// 论坛原帖 HTML 解析。
    PostHtml,
    /// RSS `<description>` 缩进启发式恢复。
    RssDescription,
}

impl MarkdownSource {
    pub fn as_str(self) -> &'static str {
        match self {
            MarkdownSource::PostHtml => "post_html",
            MarkdownSource::RssDescription => "rss_description",
        }
    }
}

/// 抓取原帖 HTML 并转为 Markdown。
///
/// # Errors
/// HTTP 失败或页面缺少 `section.ipsType_richText`（改版/非公告页）时返回错误，
/// 由调用方决定是否降级。
pub async fn fetch_post_markdown(url: &str) -> AppResult<String> {
    let client = reqwest::Client::builder()
        .timeout(POST_FETCH_TIMEOUT)
        .build()
        .map_err(AppError::Http)?;
    let response = client.get(url).send().await.map_err(AppError::Http)?;
    let body = response.text().await.map_err(AppError::Http)?;
    html_to_markdown(&body)
}

/// 统一入口：优先原帖 HTML，失败/为空时降级 RSS 描述缩进。
///
/// 降级是正常路径而非失败（不触发失败状态机），仅记 WARN。两条路径都拿不到
/// 非空 Markdown 时返回 Err。
///
/// # Errors
/// 原帖不可用且 RSS 描述也无法恢复出非空正文。
pub async fn load_source_markdown(
    link: &str,
    description: &str,
) -> AppResult<(String, MarkdownSource)> {
    match fetch_post_markdown(link).await {
        Ok(markdown) if !markdown.trim().is_empty() => Ok((markdown, MarkdownSource::PostHtml)),
        Ok(_) => {
            tracing::warn!(
                link,
                "post page produced empty markdown; falling back to RSS description"
            );
            fallback_from_description(description)
        }
        Err(e) => {
            tracing::warn!(
                link,
                error = %e,
                "post page fetch/parse failed; falling back to RSS description"
            );
            fallback_from_description(description)
        }
    }
}

fn fallback_from_description(description: &str) -> AppResult<(String, MarkdownSource)> {
    let markdown = description_to_markdown(description);
    if markdown.trim().is_empty() {
        return Err(AppError::HtmlParse(
            "post page unavailable and RSS description is empty".to_string(),
        ));
    }
    Ok((markdown, MarkdownSource::RssDescription))
}

/// 把论坛原帖 HTML 的富文本区域转成 Markdown。
///
/// # Errors
/// 页面缺少正文选择器或正文为空时返回 `HtmlParse`。
pub fn html_to_markdown(html: &str) -> AppResult<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse(RICH_TEXT_SELECTOR).expect("static selector is valid");
    let section = document
        .select(&selector)
        .next()
        .ok_or_else(|| AppError::HtmlParse(format!("selector '{RICH_TEXT_SELECTOR}' not found")))?;

    let mut converter = Converter { out: String::new() };
    converter.render_children(section, 0);
    let markdown = decode_html_entities(&normalize_markdown(&converter.out));
    if markdown.is_empty() {
        return Err(AppError::HtmlParse(
            "rich-text section produced empty markdown".to_string(),
        ));
    }
    Ok(markdown)
}

/// 解码正文里残留的 HTML 实体并把不换行空格归一为普通空格。
///
/// RSS `<description>` 是论坛导出的纯文本，`&nbsp;` 会原样保留；原帖 HTML
/// 也可能带实体。翻译前统一清掉，避免实体进入译文与 `body_text`。
pub fn decode_html_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&#160;", " ")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace('\u{00A0}', " ")
}

/// RSS `<description>` 纯文本 → Markdown（缩进启发式，仅在原帖不可用时使用）。
///
/// 规则：以句末标点或长文本判定"明确条目"，取这些条目缩进深度的众数作为列表
/// 单位；深度低于单位或非单位整数倍、且短且无句末标点的行视为节标题，其余按
/// 缩进层级输出 `-` 列表。无缩进的旧格式直接按空行分段返回。
pub fn description_to_markdown(description: &str) -> String {
    decode_html_entities(&description_to_markdown_inner(description))
}

fn description_to_markdown_inner(description: &str) -> String {
    let mut lines: Vec<(usize, String)> = Vec::new();
    for raw in description.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        lines.push((indent_depth(raw), raw.trim().to_string()));
    }
    if lines.is_empty() {
        return String::new();
    }
    if lines.iter().all(|(depth, _)| *depth == 0) {
        return normalize_markdown(description);
    }

    let unit = list_unit(&lines);
    let mut blocks: Vec<String> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    for (depth, text) in &lines {
        let depth = *depth;
        let is_heading = depth < unit
            && text.chars().count() <= HEADING_MAX_CHARS
            && !ends_with_terminal_punctuation(text);
        if is_heading {
            flush_paragraph(&mut blocks, &mut paragraph);
            blocks.push(format!("## {text}"));
        } else if depth < unit {
            paragraph.push(text.clone());
        } else {
            flush_paragraph(&mut blocks, &mut paragraph);
            let level = (depth / unit).max(1);
            blocks.push(format!("{}- {text}", "  ".repeat(level - 1)));
        }
    }
    flush_paragraph(&mut blocks, &mut paragraph);
    join_blocks(&blocks)
}

/// 列表单位：明确条目（句末标点或长文本）缩进深度的众数；同数取最浅。
/// 没有明确条目时退化为全部深度的最大公约数。
fn list_unit(lines: &[(usize, String)]) -> usize {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for (depth, text) in lines {
        if *depth > 0 && (ends_with_terminal_punctuation(text) || text.chars().count() > 60) {
            *counts.entry(*depth).or_default() += 1;
        }
    }
    if let Some((depth, _)) = counts
        .iter()
        .max_by_key(|(depth, count)| (**count, Reverse(**depth)))
    {
        return *depth;
    }
    let mut unit = 0;
    for (depth, _) in lines {
        unit = gcd(unit, *depth);
    }
    unit.max(1)
}

fn flush_paragraph(blocks: &mut Vec<String>, paragraph: &mut Vec<String>) {
    if paragraph.is_empty() {
        return;
    }
    blocks.push(paragraph.join(" "));
    paragraph.clear();
}

/// 块之间插空行；连续列表项保持紧凑（tight list）。
fn join_blocks(blocks: &[String]) -> String {
    let mut out = String::new();
    let mut previous_was_item = false;
    for block in blocks {
        let is_item = block.trim_start().starts_with("- ");
        if !out.is_empty() && !(is_item && previous_was_item) {
            out.push('\n');
        }
        out.push_str(block);
        out.push('\n');
        previous_was_item = is_item;
    }
    out.trim().to_string()
}

/// 把 Markdown 转为纯文本（去掉标题/列表/强调/链接标记），供 `body_text` 使用。
pub fn markdown_to_plain(markdown: &str) -> String {
    let markdown = decode_html_entities(markdown);
    let mut out = String::new();
    let mut in_code_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_fence = !in_code_fence;
            continue;
        }
        if in_code_fence {
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }
        if trimmed == "---" {
            continue;
        }
        let mut text = strip_heading_prefix(trimmed);
        while let Some(rest) = text.strip_prefix('>') {
            text = rest.trim_start();
        }
        text = strip_list_prefix(text);
        let text = strip_inline_markers(text);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        out.push_str(&text);
        out.push('\n');
    }
    normalize_markdown(&out)
}

fn strip_heading_prefix(line: &str) -> &str {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        line[hashes..].trim_start()
    } else {
        line
    }
}

fn strip_list_prefix(line: &str) -> &str {
    if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return rest;
    }
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && line[digits..].starts_with(". ") {
        return &line[digits + 2..];
    }
    line
}

/// 去掉行内标记：`[text](url)` → `text (url)`，`![alt](url)` → `alt`，去强调与反引号。
fn strip_inline_markers(text: &str) -> String {
    let unlinked = replace_links(text);
    unlinked
        .replace("**", "")
        .replace("__", "")
        .replace(['*', '`'], "")
}

fn replace_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let is_image = open > 0 && rest[..open].ends_with('!');
        let keep_from = if is_image { open - 1 } else { open };
        let after = &rest[open + 1..];
        if let Some(close) = after.find("](") {
            let label = &after[..close];
            let after_paren = &after[close + 2..];
            if let Some(end) = after_paren.find(')') {
                let url = &after_paren[..end];
                out.push_str(&rest[..keep_from]);
                out.push_str(label);
                if !is_image && !url.is_empty() {
                    out.push_str(" (");
                    out.push_str(url);
                    out.push(')');
                }
                rest = &after_paren[end + 1..];
                continue;
            }
        }
        out.push_str(&rest[..open + 1]);
        rest = &rest[open + 1..];
    }
    out.push_str(rest);
    out
}

/// 缩进深度：1 个 tab 记 1 层，4 个空格记 1 层（论坛导出两种都可能出现）。
fn indent_depth(raw: &str) -> usize {
    let tabs = raw.chars().take_while(|c| *c == '\t').count();
    let spaces = raw.chars().skip(tabs).take_while(|c| *c == ' ').count();
    tabs + spaces / 4
}

fn ends_with_terminal_punctuation(text: &str) -> bool {
    matches!(
        text.chars().last(),
        Some('.' | '!' | '?' | '。' | '！' | '？' | '…')
    )
}

/// 句末陈述标点（不含感叹号——"Streaming Drops!" 这类整段加粗是节标题）。
fn ends_with_statement_punctuation(text: &str) -> bool {
    matches!(text.chars().last(), Some('.' | '?' | '。' | '？' | '…'))
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// 输出前清理：行首空白（保留列表项及其续行的缩进）、行尾空白、
/// 连续空行压缩为一个空行、整体 trim。
fn normalize_markdown(raw: &str) -> String {
    let mut out = String::new();
    let mut previous_was_item = false;
    let mut previous_was_continuation = false;
    for line in raw.lines() {
        let trimmed_end = line.trim_end();
        let leading = trimmed_end.trim_start();
        if leading.is_empty() {
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            previous_was_item = false;
            previous_was_continuation = false;
            continue;
        }
        let is_item = is_list_line(leading);
        let is_continuation =
            (previous_was_item || previous_was_continuation) && trimmed_end.starts_with("  ");
        let line = if is_item || is_continuation {
            trimmed_end
        } else {
            leading
        };
        out.push_str(line);
        out.push('\n');
        previous_was_item = is_item;
        previous_was_continuation = is_continuation;
    }
    out.trim().to_string()
}

/// 是否为 Markdown 列表项行（无序 `- ` 或有序 `1. `）。
fn is_list_line(text: &str) -> bool {
    if text.starts_with("- ") || text.starts_with("* ") || text.starts_with("+ ") {
        return true;
    }
    let digits = text.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0 && text[digits..].starts_with(". ")
}

/// 列表条目正文：去首尾空白、压缩内部空行（供缩进续行）。
fn normalize_item_content(raw: &str) -> String {
    let mut out = String::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

/// HTML 流式文本的空白折叠：连续空白（含换行/tab）折叠为单个空格，
/// 保留首尾各一个空格（避免行内元素之间粘连）。
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.chars() {
        if c.is_whitespace() {
            pending = true;
            continue;
        }
        if pending {
            out.push(' ');
            pending = false;
        }
        out.push(c);
    }
    if pending {
        out.push(' ');
    }
    out
}

fn ensure_blank_line(out: &mut String) {
    if out.is_empty() {
        return;
    }
    let trailing_newlines = out.chars().rev().take_while(|c| *c == '\n').count();
    if trailing_newlines == 0 {
        out.push_str("\n\n");
    } else if trailing_newlines == 1 {
        out.push('\n');
    }
}

/// 判断 `<p>` 是否为整段加粗的节标题（如 `<p><strong>Changes</strong></p>`）。
///
/// 排除长句与句末标点（"…is back!" 是正文不是标题），排除 `<br>` 后带正文的
/// 混合段落（"**标题**<br>正文"）。
fn paragraph_is_heading(p: ElementRef<'_>) -> bool {
    let text = collapse_whitespace(&p.text().collect::<String>());
    let text = text.trim();
    if text.is_empty()
        || text.chars().count() > HEADING_MAX_CHARS
        || ends_with_statement_punctuation(text)
    {
        return false;
    }
    for child in p.children() {
        match child.value() {
            Node::Text(t) => {
                if !t.trim().is_empty() {
                    return false;
                }
            }
            Node::Element(_) => {
                let Some(el) = ElementRef::wrap(child) else {
                    return false;
                };
                match el.value().name() {
                    "strong" | "b" => {}
                    "span" | "u" | "font" | "small" => {
                        if !element_only_bold_text(el) {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
            _ => {}
        }
    }
    true
}

/// 元素内所有非空白文本都位于 `strong`/`b` 内（允许 `span`/`u` 包裹层）。
fn element_only_bold_text(el: ElementRef<'_>) -> bool {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                if !t.trim().is_empty() {
                    return false;
                }
            }
            Node::Element(_) => {
                let Some(child_el) = ElementRef::wrap(child) else {
                    return false;
                };
                match child_el.value().name() {
                    "strong" | "b" => {}
                    "span" | "u" | "font" | "small" => {
                        if !element_only_bold_text(child_el) {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
            _ => {}
        }
    }
    true
}

/// HTML 富文本 → Markdown 的流式转换器。
struct Converter {
    out: String,
}

impl Converter {
    /// 把子树的渲染结果写入独立缓冲并返回，调用方决定包裹/缩进。
    fn capture(&mut self, render: impl FnOnce(&mut Self)) -> String {
        let saved = std::mem::take(&mut self.out);
        render(self);
        std::mem::replace(&mut self.out, saved)
    }

    fn render_children(&mut self, el: ElementRef<'_>, list_depth: usize) {
        for child in el.children() {
            match child.value() {
                Node::Text(text) => self.out.push_str(&collapse_whitespace(&text.text)),
                Node::Element(_) => {
                    if let Some(child_el) = ElementRef::wrap(child) {
                        self.render_element(child_el, list_depth);
                    }
                }
                _ => {}
            }
        }
    }

    fn render_element(&mut self, el: ElementRef<'_>, list_depth: usize) {
        let name = el.value().name();
        match name {
            // 非正文元素：脚本/样式、视频嵌入、图片（卡片/QQ 均无法可靠渲染外链图）、
            // 浏览器扩展注入的匿名节点。
            "script" | "style" | "noscript" | "iframe" | "img" | "grammarly-extension" => {}
            "br" => self.out.push('\n'),
            "hr" => {
                ensure_blank_line(&mut self.out);
                self.out.push_str("---");
                ensure_blank_line(&mut self.out);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let text = collapse_whitespace(&el.text().collect::<String>());
                let text = text.trim();
                // 页面模板标题（"Update Information:"）留给 canonical title，不进正文。
                if text.is_empty()
                    || text == "Update Information:"
                    || el
                        .value()
                        .has_class("ipsType_pageTitle", CaseSensitivity::CaseSensitive)
                {
                    return;
                }
                let level = name[1..].parse::<usize>().unwrap_or(2).clamp(1, 6);
                ensure_blank_line(&mut self.out);
                self.out.push_str(&"#".repeat(level));
                self.out.push(' ');
                self.out.push_str(text);
                ensure_blank_line(&mut self.out);
            }
            "p" => {
                if paragraph_is_heading(el) {
                    let text = collapse_whitespace(&el.text().collect::<String>());
                    ensure_blank_line(&mut self.out);
                    self.out.push_str("## ");
                    self.out.push_str(text.trim());
                    ensure_blank_line(&mut self.out);
                } else {
                    ensure_blank_line(&mut self.out);
                    self.render_children(el, list_depth);
                    ensure_blank_line(&mut self.out);
                }
            }
            "strong" | "b" => self.render_wrapped(el, list_depth, "**", "**"),
            "em" | "i" => self.render_wrapped(el, list_depth, "*", "*"),
            "del" | "s" | "strike" => self.render_wrapped(el, list_depth, "~~", "~~"),
            "code" => self.render_wrapped(el, list_depth, "`", "`"),
            "u" | "span" | "font" | "small" | "sup" | "sub" | "mark" => {
                self.render_children(el, list_depth);
            }
            "a" => {
                let inner = self.capture(|converter| converter.render_children(el, list_depth));
                let text = inner.trim();
                match (el.attr("href"), text.is_empty()) {
                    (Some(href), false) => {
                        self.out.push_str(&format!("[{text}]({href})"));
                    }
                    // 空锚点多为标题上的跳转/埋点链接：丢弃，但若原本是空白文本则
                    // 保留一个空格，避免两侧文本粘连。
                    (_, true) => {
                        if !inner.is_empty() {
                            self.out.push(' ');
                        }
                    }
                    (None, false) => self.out.push_str(text),
                }
            }
            "blockquote" => {
                let inner = self.capture(|converter| converter.render_children(el, list_depth));
                let quoted = inner.trim();
                if !quoted.is_empty() {
                    ensure_blank_line(&mut self.out);
                    for line in quoted.lines() {
                        self.out.push_str("> ");
                        self.out.push_str(line.trim_end());
                        self.out.push('\n');
                    }
                    ensure_blank_line(&mut self.out);
                }
            }
            "pre" => {
                let text = el.text().collect::<String>();
                if !text.trim().is_empty() {
                    ensure_blank_line(&mut self.out);
                    self.out.push_str("```\n");
                    self.out.push_str(text.trim_end());
                    self.out.push_str("\n```");
                    ensure_blank_line(&mut self.out);
                }
            }
            "ul" => {
                ensure_blank_line(&mut self.out);
                self.render_list(el, list_depth, false);
                ensure_blank_line(&mut self.out);
            }
            "ol" => {
                ensure_blank_line(&mut self.out);
                self.render_list(el, list_depth, true);
                ensure_blank_line(&mut self.out);
            }
            "div" => {
                if el
                    .value()
                    .has_class("ipsSpoiler_header", CaseSensitivity::CaseSensitive)
                {
                    return;
                }
                self.render_children(el, list_depth);
            }
            _ => self.render_children(el, list_depth),
        }
    }

    fn render_wrapped(&mut self, el: ElementRef<'_>, list_depth: usize, open: &str, close: &str) {
        let inner = self.capture(|converter| converter.render_children(el, list_depth));
        let text = inner.trim();
        if text.is_empty() {
            return;
        }
        self.out.push_str(open);
        self.out.push_str(text);
        self.out.push_str(close);
    }

    /// 渲染列表；顶层调用方负责前后空行，嵌套调用保持紧凑。
    fn render_list(&mut self, el: ElementRef<'_>, list_depth: usize, ordered: bool) {
        for child in el.children() {
            let Some(li) = ElementRef::wrap(child) else {
                continue;
            };
            if li.value().name() == "li" {
                self.render_list_item(li, list_depth, ordered);
            }
        }
    }

    fn render_list_item(&mut self, li: ElementRef<'_>, list_depth: usize, ordered: bool) {
        let indent = "  ".repeat(list_depth);
        let marker = if ordered { "1. " } else { "- " };

        // 非嵌套列表子节点聚合为条目正文（含 spoiler/段落等块内容）。
        let content = self.capture(|converter| {
            for child in li.children() {
                if let Some(child_el) = ElementRef::wrap(child) {
                    if matches!(child_el.value().name(), "ul" | "ol") {
                        continue;
                    }
                    converter.render_element(child_el, list_depth + 1);
                } else if let Node::Text(text) = child.value() {
                    converter.out.push_str(&collapse_whitespace(&text.text));
                }
            }
        });
        let content = normalize_item_content(&content);
        if !content.is_empty() {
            self.out.push_str(&indent);
            self.out.push_str(marker);
            let mut lines = content.lines();
            if let Some(first) = lines.next() {
                self.out.push_str(first);
                self.out.push('\n');
            }
            let continuation = format!("{indent}{}", " ".repeat(marker.len()));
            for line in lines {
                self.out.push_str(&continuation);
                self.out.push_str(line);
                self.out.push('\n');
            }
        }

        // 嵌套列表
        for child in li.children() {
            let Some(child_el) = ElementRef::wrap(child) else {
                continue;
            };
            match child_el.value().name() {
                "ul" => self.render_list(child_el, list_depth + 1, false),
                "ol" => self.render_list(child_el, list_depth + 1, true),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const POST_752118_HTML: &str = include_str!("../tests/fixtures/post_752118.html");
    const POST_736959_HTML: &str = include_str!("../tests/fixtures/post_736959.html");
    const DESCRIPTION_752118: &str = include_str!("../tests/fixtures/description_752118.txt");

    #[test]
    fn html_converts_headings_lists_and_nested_items() {
        let markdown = html_to_markdown(POST_752118_HTML).unwrap();

        assert!(markdown.starts_with("## Changes"), "got: {markdown}");
        assert!(markdown.contains("\n## Bug Fixes\n"), "got: {markdown}");
        assert!(markdown.contains("- Shrouded Queen:"), "got: {markdown}");
        assert!(
            markdown.contains("\n  - Vine counterattack"),
            "nested item should be indented, got: {markdown}"
        );
        assert!(
            markdown.contains("It will be recorded as “Darkness” again"),
            "spoiler contents should survive, got: {markdown}"
        );
        assert!(
            !markdown.contains("Update Information:"),
            "page title must be stripped, got: {markdown}"
        );
        assert!(
            !markdown.contains("<p>") && !markdown.contains("</"),
            "no raw tags expected, got: {markdown}"
        );
    }

    #[test]
    fn html_renders_inline_link() {
        let markdown = html_to_markdown(POST_736959_HTML).unwrap();
        assert!(markdown.contains("\n## Changes\n"), "got: {markdown}");
        assert!(
            markdown.contains(
                "[here](https://kleiforums.com/forums/topic/172088-dont-starve-together-from-beyond-cursed-confrontation-part-1-klei-fest-2026/)"
            ),
            "got: {markdown}"
        );
    }

    #[test]
    fn html_rejects_missing_rich_text_section() {
        let result = html_to_markdown("<html><body><p>no content</p></body></html>");
        assert!(result.is_err());
    }

    #[test]
    fn description_fallback_recovers_headings_and_nesting() {
        let markdown = description_to_markdown(DESCRIPTION_752118);
        assert!(markdown.starts_with("## Changes"), "got: {markdown}");
        assert!(markdown.contains("\n## Bug Fixes\n"), "got: {markdown}");
        assert!(
            markdown.contains("\n  - Vine counterattack"),
            "got: {markdown}"
        );
        assert!(
            !markdown.contains('\t'),
            "tabs must be consumed: {markdown}"
        );
    }

    #[test]
    fn description_without_indent_stays_paragraph_text() {
        let flat = "Changes Added Chessmaster Circuit. Bug Fixes Fixed a crash.";
        let markdown = description_to_markdown(flat);
        assert!(!markdown.contains("##"));
        assert!(!markdown.contains("- "));
        assert_eq!(markdown, flat);
    }

    #[test]
    fn description_empty_is_empty() {
        assert_eq!(description_to_markdown("  \n\t\n"), "");
    }

    #[test]
    fn plain_strips_headings_lists_and_links() {
        let markdown =
            "## 更新内容\n\n- 新增 [月晶风暴](https://example.com/a) 天气\n- 修复问题\n\n> 引用";
        let plain = markdown_to_plain(markdown);
        assert_eq!(
            plain,
            "更新内容\n\n新增 月晶风暴 (https://example.com/a) 天气\n修复问题\n\n引用"
        );
    }

    #[test]
    fn plain_handles_images_and_nested_items() {
        let markdown = "## 标题\n\n- 父项\n  - 子项 ![图](https://example.com/i.png)";
        let plain = markdown_to_plain(markdown);
        assert_eq!(plain, "标题\n\n父项\n子项 图");
    }

    #[test]
    fn plain_of_plain_text_is_unchanged() {
        assert_eq!(
            markdown_to_plain("纯文本，没有标记。"),
            "纯文本，没有标记。"
        );
    }

    #[test]
    fn decodes_html_entities() {
        assert_eq!(decode_html_entities("a&nbsp;b &amp; c"), "a b & c");
        assert_eq!(decode_html_entities("x\u{00A0}y"), "x y");
    }

    #[test]
    fn description_markdown_has_no_entities() {
        let markdown = description_to_markdown("The&nbsp;Rosebloom Vest is here!");
        assert!(!markdown.contains("&nbsp;"));
        assert!(markdown.contains("The Rosebloom Vest is here!"));
    }
}
