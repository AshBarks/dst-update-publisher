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

    let mut contents: Vec<String> = Vec::new();

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

        contents.push(content);
    }

    build_index_from_po_contents(&contents)
}

pub fn build_index_from_po_contents(po_contents: &[String]) -> AppResult<PoFileIndex> {
    let mut all_entries: Vec<PoIndexEntry> = Vec::new();
    let mut exact_map: HashMap<String, Vec<usize>> = HashMap::new();
    let mut word_index: HashMap<String, Vec<usize>> = HashMap::new();

    for content in po_contents {
        let catalog = polib::po_file::parse_from_reader(content.as_bytes()).map_err(|e| {
            AppError::PoProcessing(format!("failed to parse PO content: {}", e))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn variants(word: &str) -> Vec<String> {
        generate_variants(word)
    }

    fn has_variant(word_variants: &[String], expected: &str) -> bool {
        word_variants.iter().any(|v| v == expected)
    }

    #[test]
    fn variants_adds_s() {
        let v = variants("varg");
        assert!(has_variant(&v, "vargs"), "varg should generate vargs");
    }

    #[test]
    fn variants_strips_s() {
        let v = variants("vargs");
        assert!(has_variant(&v, "varg"), "vargs should generate varg");
    }

    #[test]
    fn variants_ies_to_y() {
        let v = variants("berries");
        assert!(has_variant(&v, "berry"), "berries should generate berry");
    }

    #[test]
    fn variants_y_to_ies() {
        let v = variants("berry");
        assert!(has_variant(&v, "berries"), "berry should generate berries");
    }

    #[test]
    fn variants_ves_to_f() {
        let v = variants("wolves");
        assert!(has_variant(&v, "wolf"), "wolves should generate wolf");
        assert!(has_variant(&v, "wolfe"), "wolves should generate wolfe");
    }

    #[test]
    fn variants_f_to_ves() {
        let v = variants("wolf");
        assert!(has_variant(&v, "wolves"), "wolf should generate wolves");
    }

    #[test]
    fn variants_fe_to_ves() {
        let v = variants("knife");
        assert!(has_variant(&v, "knives"), "knife should generate knives");
    }

    #[test]
    fn variants_strips_es_suffix() {
        let v = variants("bushes");
        assert!(has_variant(&v, "bush"), "bushes should generate bush");
    }

    #[test]
    fn variants_adds_es() {
        let v = variants("bush");
        assert!(has_variant(&v, "bushes"), "bush should generate bushes");
    }

    #[test]
    fn variants_strips_ing() {
        let v = variants("healing");
        assert!(has_variant(&v, "heal"), "healing should generate heal");
    }

    #[test]
    fn variants_strips_ing_doubled_consonant() {
        let v = variants("running");
        assert!(has_variant(&v, "run"), "running should generate run");
    }

    #[test]
    fn variants_ing_adds_e() {
        let v = variants("healing");
        assert!(has_variant(&v, "heale"), "healing should generate heale");
    }

    #[test]
    fn variants_strips_ed() {
        let v = variants("cooked");
        assert!(has_variant(&v, "cook"), "cooked should generate cook");
    }

    #[test]
    fn variants_strips_ed_doubled_consonant() {
        let v = variants("stopped");
        assert!(has_variant(&v, "stop"), "stopped should generate stop");
    }

    #[test]
    fn variants_ed_adds_e() {
        let v = variants("cooked");
        assert!(has_variant(&v, "cooke"), "cooked should generate cooke");
    }

    #[test]
    fn variants_strips_ly() {
        let v = variants("recently");
        assert!(has_variant(&v, "recent"), "recently should generate recent");
    }

    #[test]
    fn variants_strips_er() {
        let v = variants("heater");
        assert!(has_variant(&v, "heat"), "heater should generate heat");
    }

    #[test]
    fn variants_strips_est() {
        let v = variants("fastest");
        assert!(has_variant(&v, "fast"), "fastest should generate fast");
    }

    #[test]
    fn variants_short_word_no_transform() {
        let v = variants("ab");
        assert_eq!(v, vec!["ab"], "short words should only return themselves");
    }

    #[test]
    fn variants_ss_not_stripped() {
        let v = variants("boss");
        assert!(!has_variant(&v, "bos"), "boss should not generate bos");
    }

    #[test]
    fn variants_us_not_stripped() {
        let v = variants("cactus");
        assert!(!has_variant(&v, "cactu"), "cactus should not generate cactu");
    }

    #[test]
    fn variants_includes_original() {
        let v = variants("varg");
        assert!(has_variant(&v, "varg"), "variants should always include the original");
    }

    #[test]
    fn variants_strips_fe_suffix() {
        let v = variants("knife");
        assert!(has_variant(&v, "knives"));
    }

    fn load_test_index() -> PoFileIndex {
        let po_content = std::fs::read_to_string("tests/fixtures/chinese_s.po")
            .expect("failed to read tests/fixtures/chinese_s.po");
        build_index_from_po_contents(&[po_content]).expect("failed to build test index")
    }

    #[test]
    fn index_builds_from_po_file() {
        let index = load_test_index();
        assert!(!index.entries.is_empty(), "index should have entries");
        assert!(!index.exact_map.is_empty(), "exact_map should not be empty");
        assert!(!index.word_index.is_empty(), "word_index should not be empty");
    }

    #[test]
    fn search_exact_match_simple() {
        let index = load_test_index();
        let result = index.search_single_term("Varg");
        assert!(!result.candidates.is_empty(), "Varg should have candidates");
        assert!(
            result.candidates.iter().any(|c| c.original == "Varg" && c.translation == "座狼"),
            "should find Varg -> 座狼"
        );
    }

    #[test]
    fn search_exact_match_multiword() {
        let index = load_test_index();
        let result = index.search_single_term("Nightmare Fuel");
        assert!(!result.candidates.is_empty());
        assert!(
            result.candidates.iter().any(|c| c.original == "Nightmare Fuel" && c.translation == "噩梦燃料"),
            "should find Nightmare Fuel -> 噩梦燃料"
        );
    }

    #[test]
    fn search_variant_match_plural() {
        let index = load_test_index();
        let result = index.search_single_term("Vargs");
        assert!(!result.candidates.is_empty(), "Vargs should find via variant");
        assert!(
            result.candidates.iter().any(|c| c.original == "Varg"),
            "Vargs should match Varg entry"
        );
    }

    #[test]
    fn search_case_insensitive() {
        let index = load_test_index();
        let r1 = index.search_single_term("varg");
        let r2 = index.search_single_term("VARG");
        assert!(!r1.candidates.is_empty(), "lowercase varg should match");
        assert!(!r2.candidates.is_empty(), "uppercase VARG should match");
    }

    #[test]
    fn search_no_result() {
        let index = load_test_index();
        let result = index.search_single_term("NonExistentTerm");
        assert!(result.candidates.is_empty(), "nonexistent term should have no candidates");
    }

    #[test]
    fn search_grumble_bee() {
        let index = load_test_index();
        let result = index.search_single_term("Grumble Bees");
        assert!(!result.candidates.is_empty(), "Grumble Bees should have candidates");
    }

    #[test]
    fn search_moleworm() {
        let index = load_test_index();
        let result = index.search_single_term("Moleworm");
        assert!(!result.candidates.is_empty());
        assert!(
            result.candidates.iter().any(|c| c.translation == "鼹鼠"),
            "Moleworm should translate to 鼹鼠"
        );
    }

    #[test]
    fn search_ocuvigil() {
        let index = load_test_index();
        let result = index.search_single_term("Ocuvigil");
        assert!(!result.candidates.is_empty());
        assert!(
            result.candidates.iter().any(|c| c.translation == "月眼守卫"),
            "Ocuvigil should translate to 月眼守卫"
        );
    }

    #[test]
    fn search_dedup_candidates() {
        let index = load_test_index();
        let result = index.search_single_term("Moleworm");
        let mut seen = HashSet::new();
        for c in &result.candidates {
            assert!(
                seen.insert((c.original.clone(), c.translation.clone())),
                "duplicate candidate: original={}, translation={}",
                c.original, c.translation
            );
        }
    }

    #[test]
    fn search_truncates_to_five() {
        let index = load_test_index();
        let result = index.search_single_term("Hound");
        assert!(result.candidates.len() <= 5, "should have at most 5 candidates");
    }

    #[test]
    fn search_batch_test_terms() {
        let index = load_test_index();
        let terms = [
            "Varg", "Nightmare Fuel", "Moleworm", "Ocuvigil", "Grumble Bees",
            "Clockworks", "Evergreens", "Twiggy Trees", "Shoals", "Belongings",
            "Void Masque", "Hounds", "Beard",
        ];
        for term in terms {
            let result = index.search_single_term(term);
            assert!(
                !result.candidates.is_empty(),
                "term '{}' should have at least one candidate",
                term
            );
        }
    }
}
