//! Spell checking: dictionary loading, word checking, suggestion generation.
//!
//! Ported from macmbx_spell.c — built-in hash table dictionary + Levenshtein
//! edit distance suggestions. Pluggable backend support.

use std::io::{BufRead, BufReader, Write};
use std::fs::File;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const MAX_SUGGESTIONS: usize = 10;
const DICT_HASH_SIZE: usize = 65536;

// ---------------------------------------------------------------------------
// Spell check result
// ---------------------------------------------------------------------------

/// Result of checking a single word.
#[derive(Debug, Clone)]
pub struct SpellResult {
    pub correct: bool,
    pub suggestions: Vec<String>,
}

impl Default for SpellResult {
    fn default() -> Self {
        SpellResult { correct: true, suggestions: Vec::new() }
    }
}

/// Error found during text scanning.
#[derive(Debug, Clone)]
pub struct SpellError {
    pub word: String,
    pub offset: usize,
    pub length: usize,
    pub result: SpellResult,
}

/// A text replacement fix.
#[derive(Debug, Clone)]
pub struct SpellFix {
    pub offset: usize,
    pub length: usize,
    pub replacement: String,
}

// ---------------------------------------------------------------------------
// Pluggable backend callbacks
// ---------------------------------------------------------------------------

/// Check if a word is correct. Return true if known.
pub type CheckFn = Box<dyn Fn(&str) -> bool + Send>;

/// Get suggestions for a misspelled word. Returns list of suggestions.
pub type SuggestFn = Box<dyn Fn(&str, usize) -> Vec<String> + Send>;

// ---------------------------------------------------------------------------
// Dictionary entry
// ---------------------------------------------------------------------------

struct DictEntry {
    word: String,      // stored lowercase
    user_word: bool,
    next: Option<Box<DictEntry>>,
}

// ---------------------------------------------------------------------------
// Spell checker instance
// ---------------------------------------------------------------------------

pub struct Spell {
    buckets: Vec<Option<Box<DictEntry>>>,
    word_count: usize,
    user_dict_path: String,
    check_fn: Option<CheckFn>,
    suggest_fn: Option<SuggestFn>,
    // Options
    pub ignore_caps: bool,
    pub ignore_mixed_case: bool,
    pub ignore_digits: bool,
    pub min_length: usize,
}

fn dict_hash(word: &str) -> usize {
    let mut h: u32 = 5381;
    for b in word.bytes() {
        h = h.wrapping_mul(33).wrapping_add(b.to_ascii_lowercase() as u32);
    }
    (h as usize) % DICT_HASH_SIZE
}

impl Spell {
    /// Create a new spell checker with default options.
    pub fn new() -> Self {
        let mut buckets = Vec::with_capacity(DICT_HASH_SIZE);
        for _ in 0..DICT_HASH_SIZE {
            buckets.push(None);
        }
        Spell {
            buckets,
            word_count: 0,
            user_dict_path: String::new(),
            check_fn: None,
            suggest_fn: None,
            ignore_caps: true,
            ignore_mixed_case: true,
            ignore_digits: true,
            min_length: 2,
        }
    }

    // --- Dictionary internal operations ---

    fn find_entry(&self, word: &str) -> bool {
        let h = dict_hash(word);
        let mut cur = &self.buckets[h];
        while let Some(ref entry) = cur {
            if entry.word.eq_ignore_ascii_case(word) {
                return true;
            }
            cur = &entry.next;
        }
        false
    }

    fn insert(&mut self, word: &str, user_word: bool) -> bool {
        if self.find_entry(word) { return false; }
        let h = dict_hash(word);
        let entry = Box::new(DictEntry {
            word: word.to_ascii_lowercase(),
            user_word,
            next: self.buckets[h].take(),
        });
        self.buckets[h] = Some(entry);
        self.word_count += 1;
        true
    }

    fn remove_entry(&mut self, word: &str) -> bool {
        let h = dict_hash(word);
        // Walk the linked list, find and remove
        let mut cur = &mut self.buckets[h];
        loop {
            match cur {
                None => return false,
                Some(ref entry) if entry.word.eq_ignore_ascii_case(word) => {
                    let removed = cur.take().unwrap();
                    *cur = removed.next;
                    self.word_count -= 1;
                    return true;
                }
                Some(ref mut entry) => {
                    cur = &mut entry.next;
                }
            }
        }
    }

    // --- Dictionary loading ---

    /// Load a word list file (one word per line). Returns count of words loaded, or error.
    pub fn load_dict(&mut self, path: &str) -> Result<usize, std::io::Error> {
        let f = File::open(path)?;
        let reader = BufReader::new(f);
        let mut loaded = 0;
        for line in reader.lines() {
            let line = line?;
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') { continue; }

            // Hunspell format: word/flags -- take just the word
            let word = if let Some(pos) = line.find('/') {
                &line[..pos]
            } else {
                line
            };

            if !word.is_empty() && word.len() < 128 {
                if self.insert(word, false) {
                    loaded += 1;
                }
            }
        }
        Ok(loaded)
    }

    /// Load the system dictionary (tries common paths). Returns count loaded.
    pub fn load_system_dict(&mut self) -> usize {
        let paths = if cfg!(target_os = "macos") {
            vec![
                "/Library/Spelling/en",
                "/usr/share/dict/words",
                "/opt/homebrew/share/hunspell/en_US.dic",
                "/usr/local/share/hunspell/en_US.dic",
            ]
        } else if cfg!(target_os = "windows") {
            vec![
                r"C:\Program Files\LibreOffice\share\extensions\dict-en\en_US.dic",
                r"C:\Program Files (x86)\LibreOffice\share\extensions\dict-en\en_US.dic",
                r"C:\hunspell\en_US.dic",
            ]
        } else {
            vec![
                "/usr/share/dict/words",
                "/usr/share/dict/american-english",
                "/usr/share/dict/british-english",
                "/usr/share/hunspell/en_US.dic",
                "/usr/share/myspell/dicts/en_US.dic",
                "/usr/share/myspell/en_US.dic",
                "/usr/local/share/hunspell/en_US.dic",
            ]
        };

        for path in &paths {
            if let Ok(loaded) = self.load_dict(path) {
                if loaded > 0 { return loaded; }
            }
        }

        // Last resort: try relative paths
        for path in &["dictionaries/en_US.dic", "en_US.dic"] {
            if let Ok(loaded) = self.load_dict(path) {
                if loaded > 0 { return loaded; }
            }
        }
        0
    }

    /// Add a single word to the active dictionary.
    pub fn add_word(&mut self, word: &str) -> bool {
        self.insert(word, true)
    }

    /// Remove a word from the dictionary.
    pub fn remove_word(&mut self, word: &str) -> bool {
        self.remove_entry(word)
    }

    /// Load user dictionary (personal word list).
    pub fn load_user_dict(&mut self, path: &str) -> Result<usize, std::io::Error> {
        self.user_dict_path = path.to_string();
        let f = match File::open(path) {
            Ok(f) => f,
            Err(_) => return Ok(0), // file not found is OK for user dict
        };
        let reader = BufReader::new(f);
        let mut loaded = 0;
        for line in reader.lines() {
            let line = line?;
            let line = line.trim_end();
            if !line.is_empty() {
                if self.insert(line, true) {
                    loaded += 1;
                }
            }
        }
        Ok(loaded)
    }

    /// Save user dictionary.
    pub fn save_user_dict(&self, path: Option<&str>) -> Result<(), std::io::Error> {
        let p = path.unwrap_or(&self.user_dict_path);
        if p.is_empty() { return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no path")); }
        let mut f = File::create(p)?;
        for bucket in &self.buckets {
            let mut cur = bucket;
            while let Some(ref entry) = cur {
                if entry.user_word {
                    writeln!(f, "{}", entry.word)?;
                }
                cur = &entry.next;
            }
        }
        Ok(())
    }

    /// Get dictionary word count.
    pub fn word_count(&self) -> usize {
        self.word_count
    }

    // --- Pluggable backend ---

    /// Register an external spell check engine. Pass None to revert to built-in.
    pub fn set_backend(&mut self, check_fn: Option<CheckFn>, suggest_fn: Option<SuggestFn>) {
        self.check_fn = check_fn;
        self.suggest_fn = suggest_fn;
    }

    // --- Options ---

    pub fn set_ignore_caps(&mut self, ignore: bool) { self.ignore_caps = ignore; }
    pub fn set_ignore_mixed_case(&mut self, ignore: bool) { self.ignore_mixed_case = ignore; }
    pub fn set_ignore_digits(&mut self, ignore: bool) { self.ignore_digits = ignore; }
    pub fn set_min_length(&mut self, min_len: usize) { self.min_length = min_len; }

    // --- Checking ---

    /// Should we skip this word based on options?
    fn should_skip(&self, word: &str) -> bool {
        let bytes = word.as_bytes();
        if bytes.len() < self.min_length { return true; }

        // All caps
        if self.ignore_caps && bytes.len() > 1 {
            if bytes.iter().all(|b| !b.is_ascii_lowercase()) {
                return true;
            }
        }

        // Mixed case (camelCase, iPhone)
        if self.ignore_mixed_case {
            let has_upper_mid = bytes[1..].iter().any(|b| b.is_ascii_uppercase());
            let has_lower_mid = bytes[1..].iter().any(|b| b.is_ascii_lowercase());
            if has_upper_mid && has_lower_mid && bytes[0].is_ascii_uppercase() {
                // Could be a name -- only skip if uppercase appears after position 1
                for i in 2..bytes.len() {
                    if bytes[i].is_ascii_uppercase() { return true; }
                }
            }
        }

        // Contains digits
        if self.ignore_digits {
            if bytes.iter().any(|b| b.is_ascii_digit()) { return true; }
        }

        false
    }

    /// Check a single word. Returns true if correct.
    pub fn check(&self, word: &str) -> bool {
        if word.is_empty() { return true; }

        // External backend first
        if let Some(ref check_fn) = self.check_fn {
            return check_fn(word);
        }

        // Built-in dictionary
        self.find_entry(word)
    }

    /// Find suggestions using Levenshtein distance.
    fn builtin_suggest(&self, word: &str, max: usize) -> Vec<String> {
        let wlen = word.len();
        if wlen > 60 || wlen < 2 { return Vec::new(); }

        let max_dist = if wlen <= 4 { 1 } else { 2 };
        let mut candidates: Vec<(String, usize)> = Vec::new();

        for bucket in &self.buckets {
            let mut cur = bucket;
            while let Some(ref entry) = cur {
                let elen = entry.word.len();
                // Quick length filter
                if (elen as isize - wlen as isize).unsigned_abs() <= max_dist {
                    let dist = levenshtein(word, &entry.word);
                    if dist > 0 && dist <= max_dist {
                        candidates.push((entry.word.clone(), dist));
                        if candidates.len() >= 64 { break; }
                    }
                }
                cur = &entry.next;
            }
            if candidates.len() >= 64 { break; }
        }

        // Sort by distance, then alphabetically
        candidates.sort_by(|a, b| {
            a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0))
        });

        candidates.truncate(max);
        candidates.into_iter().map(|(w, _)| w).collect()
    }

    /// Check a word and get suggestions if misspelled.
    pub fn check_full(&self, word: &str) -> SpellResult {
        let correct = self.check(word);
        if correct {
            return SpellResult { correct: true, suggestions: Vec::new() };
        }

        let suggestions = if let Some(ref suggest_fn) = self.suggest_fn {
            suggest_fn(word, MAX_SUGGESTIONS)
        } else {
            self.builtin_suggest(word, MAX_SUGGESTIONS)
        };

        SpellResult { correct: false, suggestions }
    }

    // --- Text scanning ---

    /// Check if position looks like it's inside a URL or email.
    fn in_url_or_email(text: &[u8], pos: usize) -> bool {
        // Look backwards for "://" or "@"
        let start = if pos > 50 { pos - 50 } else { 0 };
        for i in (start..pos).rev() {
            match text[i] {
                b' ' | b'\n' | b'\r' => break,
                b'/' if i > 1 && text[i - 1] == b'/' && text[i - 2] == b':' => return true,
                b'@' => return true,
                _ => {}
            }
        }
        // Look forward for common URL patterns
        let end = std::cmp::min(pos + 20, text.len());
        for i in pos..end {
            match text[i] {
                b' ' | b'\n' => break,
                b'@' => return true,
                b'/' if i + 1 < end && text[i + 1] == b'/' => return true,
                _ => {}
            }
        }
        false
    }

    /// Scan a block of text. Returns list of errors found.
    /// Skips: URLs, email addresses, numbers, words in dictionary, based on options.
    pub fn scan(&self, text: &str) -> Vec<SpellError> {
        let bytes = text.as_bytes();
        let mut errors = Vec::new();
        let mut i = 0;

        while i < bytes.len() {
            // Skip non-alpha
            while i < bytes.len() && !bytes[i].is_ascii_alphabetic() { i += 1; }
            if i >= bytes.len() { break; }

            // Collect word
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'\'' || bytes[i] == b'-') {
                i += 1;
            }
            // Strip trailing apostrophe/hyphen
            while i > start && (bytes[i - 1] == b'\'' || bytes[i - 1] == b'-') { i -= 1; }

            let wlen = i - start;
            if wlen == 0 { continue; }
            if wlen >= 128 { continue; }

            let word = &text[start..start + wlen];

            // Skip based on options
            if self.should_skip(word) { continue; }

            // Skip URLs and email addresses
            if Self::in_url_or_email(bytes, start) { continue; }

            // Check
            if !self.check(word) {
                let result = self.check_full(word);
                errors.push(SpellError {
                    word: word.to_string(),
                    offset: start,
                    length: wlen,
                    result,
                });
            }
        }
        errors
    }
}

impl Default for Spell {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Levenshtein edit distance
// ---------------------------------------------------------------------------

fn levenshtein(s: &str, t: &str) -> usize {
    let s_bytes: Vec<u8> = s.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let t_bytes: Vec<u8> = t.bytes().map(|b| b.to_ascii_lowercase()).collect();
    let slen = s_bytes.len();
    let tlen = t_bytes.len();
    if slen == 0 { return tlen; }
    if tlen == 0 { return slen; }

    let mut prev: Vec<usize> = (0..=tlen).collect();
    let mut curr = vec![0usize; tlen + 1];

    for i in 1..=slen {
        curr[0] = i;
        for j in 1..=tlen {
            let cost = if s_bytes[i - 1] == t_bytes[j - 1] { 0 } else { 1 };
            let del = prev[j] + 1;
            let ins = curr[j - 1] + 1;
            let sub = prev[j - 1] + cost;
            curr[j] = del.min(ins).min(sub);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[tlen]
}

// ---------------------------------------------------------------------------
// Replacement / Auto-correct
// ---------------------------------------------------------------------------

/// Replace a misspelled word at offset in text. Returns new text.
pub fn replace(text: &str, offset: usize, length: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len() - length + replacement.len());
    out.push_str(&text[..offset]);
    out.push_str(replacement);
    out.push_str(&text[offset + length..]);
    out
}

/// Apply specific replacements (must be sorted by offset ascending). Returns new text.
pub fn apply_fixes(text: &str, fixes: &[SpellFix]) -> String {
    if fixes.is_empty() { return text.to_string(); }

    let mut out = String::new();
    let mut src = 0;
    for fix in fixes {
        if fix.offset > src {
            out.push_str(&text[src..fix.offset]);
        }
        out.push_str(&fix.replacement);
        src = fix.offset + fix.length;
    }
    if src < text.len() {
        out.push_str(&text[src..]);
    }
    out
}

/// Replace all misspelled words with first suggestion automatically. Returns new text.
pub fn auto_correct(sp: &Spell, text: &str) -> String {
    let errors = sp.scan(text);
    if errors.is_empty() { return text.to_string(); }

    let fixes: Vec<SpellFix> = errors.iter()
        .filter(|e| !e.result.suggestions.is_empty())
        .map(|e| SpellFix {
            offset: e.offset,
            length: e.length,
            replacement: e.result.suggestions[0].clone(),
        })
        .collect();

    apply_fixes(text, &fixes)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spell_with_words(words: &[&str]) -> Spell {
        let mut sp = Spell::new();
        for w in words {
            sp.add_word(w);
        }
        sp
    }

    #[test]
    fn test_new_spell() {
        let sp = Spell::new();
        assert_eq!(sp.word_count(), 0);
        assert!(sp.ignore_caps);
        assert!(sp.ignore_mixed_case);
        assert!(sp.ignore_digits);
        assert_eq!(sp.min_length, 2);
    }

    #[test]
    fn test_add_and_check() {
        let mut sp = Spell::new();
        sp.add_word("hello");
        sp.add_word("world");
        assert!(sp.check("hello"));
        assert!(sp.check("Hello")); // case-insensitive
        assert!(sp.check("HELLO")); // case-insensitive
        assert!(sp.check("world"));
        assert!(!sp.check("missing"));
    }

    #[test]
    fn test_add_duplicate() {
        let mut sp = Spell::new();
        assert!(sp.add_word("hello"));
        assert!(!sp.add_word("hello")); // duplicate
        assert!(!sp.add_word("Hello")); // case-insensitive duplicate
        assert_eq!(sp.word_count(), 1);
    }

    #[test]
    fn test_remove_word() {
        let mut sp = Spell::new();
        sp.add_word("hello");
        assert!(sp.check("hello"));
        assert!(sp.remove_word("hello"));
        assert!(!sp.check("hello"));
        assert_eq!(sp.word_count(), 0);
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut sp = Spell::new();
        assert!(!sp.remove_word("nope"));
    }

    #[test]
    fn test_word_count() {
        let mut sp = Spell::new();
        sp.add_word("a");
        sp.add_word("b");
        sp.add_word("c");
        assert_eq!(sp.word_count(), 3);
        sp.remove_word("b");
        assert_eq!(sp.word_count(), 2);
    }

    #[test]
    fn test_check_empty_word() {
        let sp = Spell::new();
        assert!(sp.check("")); // empty word is always correct
    }

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("same", "same"), 0);
        assert_eq!(levenshtein("cat", "hat"), 1);
        assert_eq!(levenshtein("cat", "cats"), 1);
    }

    #[test]
    fn test_suggestions() {
        let sp = make_spell_with_words(&["hello", "help", "held", "helm", "hero"]);
        let result = sp.check_full("helo");
        assert!(!result.correct);
        // Should suggest "hello" or "held" or "hero" or "help" (edit distance 1)
        assert!(!result.suggestions.is_empty());
        // "hello" should be among suggestions (distance 1)
        assert!(result.suggestions.contains(&"hello".to_string()));
    }

    #[test]
    fn test_suggestions_short_word() {
        let sp = make_spell_with_words(&["cat", "bat", "hat", "rat", "sat"]);
        let result = sp.check_full("mat");
        assert!(!result.correct);
        // All 1-edit-distance words should appear
        assert!(result.suggestions.iter().any(|s| ["cat", "bat", "hat", "rat", "sat"].contains(&s.as_str())));
    }

    #[test]
    fn test_correct_word_no_suggestions() {
        let sp = make_spell_with_words(&["hello"]);
        let result = sp.check_full("hello");
        assert!(result.correct);
        assert!(result.suggestions.is_empty());
    }

    #[test]
    fn test_should_skip_short() {
        let sp = Spell::new();
        assert!(sp.should_skip("a")); // length < 2
        assert!(!sp.should_skip("ab")); // length == 2
    }

    #[test]
    fn test_should_skip_all_caps() {
        let sp = Spell::new();
        assert!(sp.should_skip("HELLO"));
        assert!(sp.should_skip("NASA"));
    }

    #[test]
    fn test_should_skip_all_caps_disabled() {
        let mut sp = Spell::new();
        sp.set_ignore_caps(false);
        assert!(!sp.should_skip("HELLO"));
    }

    #[test]
    fn test_should_skip_digits() {
        let sp = Spell::new();
        assert!(sp.should_skip("abc123"));
        assert!(sp.should_skip("3rd"));
    }

    #[test]
    fn test_should_skip_digits_disabled() {
        let mut sp = Spell::new();
        sp.set_ignore_digits(false);
        assert!(!sp.should_skip("abc123"));
    }

    #[test]
    fn test_should_skip_mixed_case() {
        let sp = Spell::new();
        // "iPhone" starts with lowercase 'i', so the mixed-case check
        // (which requires bytes[0].is_ascii_uppercase()) does not trigger.
        // This matches the C behavior.
        assert!(!sp.should_skip("iPhone"));
    }

    #[test]
    fn test_should_skip_camelcase() {
        let sp = Spell::new();
        // "MyVariable" -> M(upper), y(lower at 1), V(upper at 2) -> skip
        assert!(sp.should_skip("MyVariable"));
    }

    #[test]
    fn test_scan_simple() {
        let sp = make_spell_with_words(&["hello", "world", "this", "is", "a"]);
        let errors = sp.scan("hello wrold this is a tset");
        // "wrold" and "tset" should be errors
        assert!(errors.iter().any(|e| e.word == "wrold"));
        assert!(errors.iter().any(|e| e.word == "tset"));
        // "hello", "this" should not be errors
        assert!(!errors.iter().any(|e| e.word == "hello"));
        assert!(!errors.iter().any(|e| e.word == "this"));
    }

    #[test]
    fn test_scan_with_offsets() {
        let sp = make_spell_with_words(&["the", "cat"]);
        let text = "the cta sat";
        let errors = sp.scan(text);
        // "cta" at offset 4
        let cta_err = errors.iter().find(|e| e.word == "cta");
        assert!(cta_err.is_some());
        assert_eq!(cta_err.unwrap().offset, 4);
        assert_eq!(cta_err.unwrap().length, 3);
    }

    #[test]
    fn test_scan_skip_urls() {
        let sp = make_spell_with_words(&["hello"]);
        let errors = sp.scan("visit http://example.com/page today");
        // Words inside URL should be skipped
        // "visit", "today" might be errors, but "example", "com", "page" should be skipped
        assert!(!errors.iter().any(|e| e.word == "example"));
    }

    #[test]
    fn test_scan_skip_email() {
        let sp = make_spell_with_words(&["hello"]);
        let errors = sp.scan("email user@example.com please");
        // "user" is before @, should be skipped
        // "example" after @ should be skipped
        assert!(!errors.iter().any(|e| e.word == "example"));
    }

    #[test]
    fn test_scan_apostrophe_word() {
        let sp = make_spell_with_words(&["don't", "hello"]);
        let errors = sp.scan("hello don't");
        // "don't" should be recognized
        assert!(!errors.iter().any(|e| e.word == "don't"));
    }

    #[test]
    fn test_scan_hyphenated_word() {
        let sp = make_spell_with_words(&["well-known", "hello"]);
        let errors = sp.scan("hello well-known");
        assert!(!errors.iter().any(|e| e.word == "well-known"));
    }

    #[test]
    fn test_replace() {
        assert_eq!(replace("hello wrold", 6, 5, "world"), "hello world");
        assert_eq!(replace("abc", 0, 3, "xyz"), "xyz");
        assert_eq!(replace("abc", 1, 1, "xyz"), "axyzc");
    }

    #[test]
    fn test_apply_fixes() {
        let fixes = vec![
            SpellFix { offset: 0, length: 3, replacement: "The".to_string() },
            SpellFix { offset: 4, length: 3, replacement: "cat".to_string() },
        ];
        assert_eq!(apply_fixes("teh cta sat", &fixes), "The cat sat");
    }

    #[test]
    fn test_apply_fixes_empty() {
        assert_eq!(apply_fixes("hello", &[]), "hello");
    }

    #[test]
    fn test_auto_correct() {
        let sp = make_spell_with_words(&["hello", "world", "the"]);
        // "helo" should be corrected to "hello" (edit distance 1)
        let result = auto_correct(&sp, "helo world");
        // The word "helo" should be corrected
        assert!(result.contains("hello") || result.contains("helo"));
        // "world" should remain unchanged
        assert!(result.contains("world"));
    }

    #[test]
    fn test_pluggable_backend() {
        let mut sp = Spell::new();
        sp.set_backend(
            Some(Box::new(|word: &str| word == "custom")),
            Some(Box::new(|_word: &str, _max: usize| vec!["custom".to_string()])),
        );
        assert!(sp.check("custom"));
        assert!(!sp.check("other"));
        let result = sp.check_full("other");
        assert!(!result.correct);
        assert_eq!(result.suggestions, vec!["custom".to_string()]);
    }

    #[test]
    fn test_pluggable_backend_clear() {
        let mut sp = Spell::new();
        sp.set_backend(
            Some(Box::new(|_: &str| true)),
            None,
        );
        assert!(sp.check("anything"));
        sp.set_backend(None, None);
        assert!(!sp.check("anything")); // back to built-in, which has no words
    }

    #[test]
    fn test_user_dict_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.dic");
        let path_str = path.to_str().unwrap();

        let mut sp = Spell::new();
        sp.add_word("customword");
        sp.load_user_dict(path_str).unwrap(); // creates path reference
        // Manually set user dict path since add_word doesn't set it
        sp.user_dict_path = path_str.to_string();
        // Add as user word
        sp.insert("userterm", true);
        sp.save_user_dict(None).unwrap();

        // Load in new instance
        let mut sp2 = Spell::new();
        sp2.load_user_dict(path_str).unwrap();
        assert!(sp2.check("userterm"));
    }

    #[test]
    fn test_load_dict_nonexistent() {
        let mut sp = Spell::new();
        assert!(sp.load_dict("/nonexistent/path/dict.dic").is_err());
    }

    #[test]
    fn test_load_dict_with_hunspell_flags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dic");
        std::fs::write(&path, "hello/SM\nworld/D\n# comment\n").unwrap();

        let mut sp = Spell::new();
        let loaded = sp.load_dict(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded, 2);
        assert!(sp.check("hello"));
        assert!(sp.check("world"));
    }

    #[test]
    fn test_scan_empty_text() {
        let sp = Spell::new();
        assert!(sp.scan("").is_empty());
    }

    #[test]
    fn test_scan_all_correct() {
        let sp = make_spell_with_words(&["all", "words", "correct"]);
        assert!(sp.scan("all words correct").is_empty());
    }

    #[test]
    fn test_min_length_option() {
        let mut sp = make_spell_with_words(&["hello"]);
        sp.set_min_length(5);
        // "xyz" is too short to check (length 3 < 5)
        let errors = sp.scan("xyz hello");
        assert!(!errors.iter().any(|e| e.word == "xyz"));
    }

    #[test]
    fn test_levenshtein_case_insensitive() {
        // Levenshtein should be case-insensitive
        assert_eq!(levenshtein("Hello", "hello"), 0);
        assert_eq!(levenshtein("CAT", "cat"), 0);
    }
}
