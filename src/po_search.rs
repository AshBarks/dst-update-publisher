use std::collections::{HashMap, HashSet};
use std::io::Read;

use crate::error::{AppError, AppResult};
use crate::models::{PoSearchResult, PoTranslationEntry};

pub struct PoFileIndex {
    entries: Vec<PoIndexEntry>,
    exact_map: HashMap<String, Vec<usize>>,
    word_index: HashMap<String, Vec<usize>>,
}

struct PoIndexEntry {
    msgid: String,
    msgid_lower: String,
    msgstr: String,
}

static STOP_WORDS: std::sync::LazyLock<HashSet<&'static str>> = std::sync::LazyLock::new(|| {
    [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has",
        "had", "do", "does", "did", "will", "would", "shall", "should", "may", "might", "can",
        "could", "must", "of", "in", "to", "for", "with", "on", "at", "by", "from", "as",
        "into", "through", "during", "before", "after", "above", "below", "between", "out",
        "off", "over", "under", "again", "further", "then", "once", "and", "but", "or", "nor",
        "not", "so", "yet", "both", "either", "neither", "each", "every", "all", "any", "few",
        "more", "most", "other", "some", "such", "no", "only", "own", "same", "than", "too",
        "very", "just", "because", "if", "when", "while", "where", "how", "what", "which",
        "who", "whom", "this", "that", "these", "those", "it", "its", "he", "she", "they",
        "we", "you", "me", "my", "your", "his", "her", "our", "their", "i", "am",
    ]
    .into_iter()
    .collect()
});

fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(word.to_lowercase().as_str())
}

fn split_and_filter(term: &str) -> Vec<String> {
    term.split_whitespace()
        .filter(|w| !is_stop_word(w) && w.len() >= 3)
        .map(|w| w.to_lowercase())
        .collect()
}

fn generate_variants(word: &str) -> Vec<String> {
    let mut variants = Vec::new();

    if word.len() < 3 {
        variants.push(word.to_string());
        return variants;
    }

    variants.push(word.to_string());

    if word.ends_with("ies") && word.len() > 4 {
        variants.push(format!("{}y", &word[..word.len() - 3]));
    }
    if word.ends_with("ves") && word.len() > 4 {
        variants.push(format!("{}f", &word[..word.len() - 3]));
        variants.push(format!("{}fe", &word[..word.len() - 3]));
    }
    if word.ends_with("ses") || word.ends_with("xes") || word.ends_with("zes")
        || word.ends_with("ches") || word.ends_with("shes")
    {
        variants.push(word[..word.len() - 2].to_string());
    }
    if word.ends_with('s') && !word.ends_with("ss") && !word.ends_with("us") {
        variants.push(word[..word.len() - 1].to_string());
    }
    if word.ends_with("ing") && word.len() > 5 {
        variants.push(word[..word.len() - 3].to_string());
        if word.len() > 6 && word.as_bytes()[word.len() - 4] == word.as_bytes()[word.len() - 5] {
            variants.push(word[..word.len() - 4].to_string());
        }
        variants.push(format!("{}e", &word[..word.len() - 3]));
    }
    if word.ends_with("ed") && word.len() > 4 {
        variants.push(word[..word.len() - 2].to_string());
        if word.len() > 5 && word.as_bytes()[word.len() - 3] == word.as_bytes()[word.len() - 4] {
            variants.push(word[..word.len() - 3].to_string());
        }
        variants.push(format!("{}e", &word[..word.len() - 2]));
    }
    if word.ends_with("ly") && word.len() > 4 {
        variants.push(word[..word.len() - 2].to_string());
    }
    if word.ends_with("er") && word.len() > 4 {
        variants.push(word[..word.len() - 2].to_string());
        variants.push(format!("{}e", &word[..word.len() - 2]));
    }
    if word.ends_with("est") && word.len() > 5 {
        variants.push(word[..word.len() - 3].to_string());
        variants.push(format!("{}e", &word[..word.len() - 3]));
    }

    if !word.ends_with('s') && !word.ends_with("ing") && !word.ends_with("ed") {
        variants.push(format!("{}s", word));
        if word.ends_with('s') || word.ends_with('x') || word.ends_with('z')
            || word.ends_with("ch") || word.ends_with("sh")
        {
            variants.push(format!("{}es", word));
        }
        if word.ends_with('y') && word.len() > 2 && !is_vowel(word.as_bytes()[word.len() - 2]) {
            variants.push(format!("{}ies", &word[..word.len() - 1]));
        }
        if word.ends_with("f") && !word.ends_with("ff") {
            variants.push(format!("{}ves", &word[..word.len() - 1]));
        }
        if let Some(stripped) = word.strip_suffix("fe") {
            variants.push(format!("{}ves", stripped));
        }
    }

    variants
}

fn is_vowel(b: u8) -> bool {
    matches!(b, b'a' | b'e' | b'i' | b'o' | b'u')
}

pub fn load_po_index(zip_path: &str, po_files: &[String]) -> AppResult<PoFileIndex> {
    let file = std::fs::File::open(zip_path).map_err(|e| {
        AppError::ZipProcessing(format!("failed to open ZIP '{}': {}", zip_path, e))
    })?;

    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        AppError::ZipProcessing(format!("failed to read ZIP '{}': {}", zip_path, e))
    })?;

    let mut all_entries: Vec<PoIndexEntry> = Vec::new();
    let mut exact_map: HashMap<String, Vec<usize>> = HashMap::new();
    let mut word_index: HashMap<String, Vec<usize>> = HashMap::new();

    for po_path in po_files {
        let mut zip_file = match archive.by_name(po_path) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("PO file '{}' not found in ZIP: {}, skipping", po_path, e);
                continue;
            }
        };

        tracing::info!("loading PO file from ZIP: {}", po_path);

        let mut content = String::new();
        zip_file
            .read_to_string(&mut content)
            .map_err(|e| AppError::PoProcessing(format!("failed to read '{}': {}", po_path, e)))?;

        let catalog = polib::po_file::parse_from_reader(content.as_bytes()).map_err(|e| {
            AppError::PoProcessing(format!("failed to parse PO '{}': {}", po_path, e))
        })?;

        for message in catalog.messages() {
            if !message.is_translated() || message.is_fuzzy() {
                continue;
            }

            let msgctxt = message.msgctxt().unwrap_or_default();
            if msgctxt.starts_with("STRINGS.CHARACTERS.") {
                continue;
            }

            let msgid = message.msgid().to_string();
            if msgid.split(' ').count() > 5 {
                continue;
            }

            let msgstr = if message.is_plural() {
                message
                    .msgstr_plural()
                    .map(|p| p.join(", "))
                    .unwrap_or_default()
            } else {
                message.msgstr().unwrap_or_default().to_string()
            };

            if msgstr.is_empty() {
                continue;
            }

            let msgid_lower = msgid.to_lowercase();
            if msgid_lower.len() < 3 {
                continue;
            }

            let idx = all_entries.len();

            for variant in generate_variants(&msgid_lower) {
                exact_map.entry(variant).or_default().push(idx);
            }

            for word in split_and_filter(&msgid) {
                word_index.entry(word).or_default().push(idx);
            }

            all_entries.push(PoIndexEntry {
                msgid,
                msgid_lower,
                msgstr,
            });
        }
    }

    tracing::info!(
        "loaded {} PO entries, {} exact map keys, {} word index keys",
        all_entries.len(),
        exact_map.len(),
        word_index.len()
    );

    Ok(PoFileIndex {
        entries: all_entries,
        exact_map,
        word_index,
    })
}

impl PoFileIndex {
    pub fn search_terms(&self, terms: &[&str]) -> Vec<PoSearchResult> {
        terms.iter().map(|term| self.search_single_term(term)).collect()
    }

    fn search_single_term(&self, term: &str) -> PoSearchResult {
        let term_lower = term.to_lowercase();
        let mut seen = HashSet::new();
        let mut candidates: Vec<(usize, PoTranslationEntry)> = Vec::new();

        if let Some(indices) = self.exact_map.get(&term_lower) {
            for &idx in indices {
                if seen.insert(idx) {
                    let entry = &self.entries[idx];
                    candidates.push((
                        usize::MAX,
                        PoTranslationEntry {
                            original: entry.msgid.clone(),
                            translation: entry.msgstr.clone(),
                        },
                    ));
                }
            }
        }

        if candidates.is_empty() {
            for variant in generate_variants(&term_lower) {
                if variant == term_lower {
                    continue;
                }
                if let Some(indices) = self.exact_map.get(&variant) {
                    for &idx in indices {
                        if seen.insert(idx) {
                            let entry = &self.entries[idx];
                            candidates.push((
                                100,
                                PoTranslationEntry {
                                    original: entry.msgid.clone(),
                                    translation: entry.msgstr.clone(),
                                },
                            ));
                        }
                    }
                    break;
                }
            }
        }

        if candidates.is_empty() {
            let term_words = split_and_filter(term);
            let mut candidate_indices: Vec<usize> = Vec::new();
            let mut candidate_set: HashSet<usize> = HashSet::new();

            for word in &term_words {
                if let Some(indices) = self.word_index.get(word) {
                    for &idx in indices {
                        if candidate_set.insert(idx) {
                            candidate_indices.push(idx);
                        }
                    }
                }
            }

            for idx in candidate_indices {
                if seen.insert(idx) {
                    let entry = &self.entries[idx];
                    let mut score = 0;
                    for word in &term_words {
                        if entry.msgid_lower.contains(word.as_str()) {
                            score += 1;
                        }
                    }
                    if score > 0 {
                        candidates.push((
                            score,
                            PoTranslationEntry {
                                original: entry.msgid.clone(),
                                translation: entry.msgstr.clone(),
                            },
                        ));
                    }
                }
            }
        }

        candidates.sort_by_key(|b| std::cmp::Reverse(b.0));

        let mut dedup = HashSet::new();
        candidates.retain(|(_, e)| dedup.insert((e.original.clone(), e.translation.clone())));

        candidates.truncate(5);

        PoSearchResult {
            term: term.to_string(),
            candidates: candidates.into_iter().map(|(_, e)| e).collect(),
        }
    }
}
