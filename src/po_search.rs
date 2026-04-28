use std::collections::HashSet;
use std::io::Read;

use crate::error::{AppError, AppResult};
use crate::models::{PoSearchResult, PoTranslationEntry};

pub struct PoFileIndex {
    entries: Vec<PoIndexEntry>,
}

struct PoIndexEntry {
    msgid: String,
    msgstr: String,
}

pub fn load_po_index(zip_path: &str, po_files: &[String]) -> AppResult<PoFileIndex> {
    let file = std::fs::File::open(zip_path).map_err(|e| {
        AppError::ZipProcessing(format!("failed to open ZIP '{}': {}", zip_path, e))
    })?;

    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        AppError::ZipProcessing(format!("failed to read ZIP '{}': {}", zip_path, e))
    })?;

    let mut all_entries: Vec<PoIndexEntry> = Vec::new();

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
            if msgid.split(" ").count() > 5 {
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

            all_entries.push(PoIndexEntry { msgid, msgstr });
        }
    }

    tracing::info!("loaded {} PO entries from ZIP", all_entries.len());

    Ok(PoFileIndex {
        entries: all_entries,
    })
}

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "shall", "should", "may", "might", "can", "could",
    "must", "of", "in", "to", "for", "with", "on", "at", "by", "from", "as", "into", "through",
    "during", "before", "after", "above", "below", "between", "out", "off", "over", "under",
    "again", "further", "then", "once", "and", "but", "or", "nor", "not", "so", "yet", "both",
    "either", "neither", "each", "every", "all", "any", "few", "more", "most", "other", "some",
    "such", "no", "only", "own", "same", "than", "too", "very", "just", "because", "if", "when",
    "while", "where", "how", "what", "which", "who", "whom", "this", "that", "these", "those",
    "it", "its", "he", "she", "they", "we", "you", "me", "my", "your", "his", "her", "our",
    "their", "its", "i", "am",
];

fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word.to_lowercase().as_str())
}

fn split_and_filter(term: &str) -> Vec<String> {
    term.split_whitespace()
        .filter(|w| !is_stop_word(w))
        .map(|w| w.to_lowercase())
        .collect()
}

impl PoFileIndex {
    pub fn entries_len(&self) -> usize {
        self.entries.len()
    }

    pub fn search_terms(&self, terms: &[&str]) -> Vec<PoSearchResult> {
        let mut results: Vec<Vec<PoTranslationEntry>> = vec![Vec::new(); terms.len()];
        // track terms that match term exactly
        let mut matches: HashSet<usize> = HashSet::new();
        // track terms that term contains entry: e.g. "vargs" -> "varg"
        let mut contains_only: HashSet<usize> = HashSet::new();

        let term_lower: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();

        for entry in self.entries.iter() {
            let entry_lower = entry.msgid.to_lowercase();
            if entry_lower.len() < 3 {
                continue;
            }

            for (term_idx, term) in term_lower.iter().enumerate() {
                if matches.contains(&term_idx) {
                    continue;
                }
                if entry_lower == *term {
                    matches.insert(term_idx);
                    results[term_idx] = vec![PoTranslationEntry {
                        original: entry.msgid.clone(),
                        translation: entry.msgstr.clone(),
                    }];
                } else if contains_only.contains(&term_idx) {
                    if term.contains(&entry_lower) {
                        results[term_idx].push(PoTranslationEntry {
                            original: entry.msgid.clone(),
                            translation: entry.msgstr.clone(),
                        });
                    }
                } else {
                    if term.contains(&entry_lower) {
                        results[term_idx] = vec![PoTranslationEntry {
                            original: entry.msgid.clone(),
                            translation: entry.msgstr.clone(),
                        }];
                        contains_only.insert(term_idx);
                    } else {
                        for word in split_and_filter(term) {
                            if entry_lower.contains(word.as_str()) {
                                results[term_idx].push(PoTranslationEntry {
                                    original: entry.msgid.clone(),
                                    translation: entry.msgstr.clone(),
                                });
                                break;
                            }
                        }
                    }
                }
            }
        }

        terms
            .iter()
            .zip(results)
            .map(|(t, mut candidates)| {
                let term_lower = t.to_lowercase();
                let term_words = split_and_filter(t);

                let mut seen = HashSet::new();
                candidates.retain(|c| seen.insert(c.original.clone()));

                candidates.sort_by(|a, b| {
                    let score = |e: &PoTranslationEntry| -> usize {
                        let el = e.original.to_lowercase();
                        if el == term_lower {
                            return usize::MAX;
                        }
                        let mut s = 0;
                        if term_lower.contains(&el) {
                            s += term_words.len() + 1;
                        }
                        for word in &term_words {
                            if el.contains(word.as_str()) {
                                s += 1;
                            }
                        }
                        s
                    };
                    score(b).cmp(&score(a))
                });

                candidates.truncate(5);

                PoSearchResult {
                    term: t.to_string(),
                    candidates,
                }
            })
            .collect()
    }
}
