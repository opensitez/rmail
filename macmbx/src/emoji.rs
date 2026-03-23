//! Emoji mapping: ASCII emoticon <-> Unicode emoji.
//!
//! Provides a table of ASCII smileys and their Unicode emoji equivalents,
//! plus text replacement functions in both directions.
//! Respects word boundaries to avoid mangling URLs or code.
//!
//! No external dependency. Pure Rust text transform.

/// A single emoji mapping entry.
#[derive(Debug, Clone)]
pub struct Emoji {
    pub ascii: &'static str,
    pub emoji: &'static str,
    pub name: &'static str,
}

/// The full emoji table, sorted longest-ASCII-first for greedy matching.
static EMOJI_TABLE: &[Emoji] = &[
    // Longer patterns first to avoid partial matches
    Emoji { ascii: ">:-(", emoji: "\u{1F620}", name: "angry" },
    Emoji { ascii: ">:(", emoji: "\u{1F620}", name: "angry" },
    Emoji { ascii: "O:-)", emoji: "\u{1F607}", name: "angel" },
    Emoji { ascii: "O:)", emoji: "\u{1F607}", name: "angel" },
    Emoji { ascii: ":'-(", emoji: "\u{1F622}", name: "cry" },
    Emoji { ascii: ":'(", emoji: "\u{1F622}", name: "cry" },
    Emoji { ascii: "</3", emoji: "\u{1F494}", name: "broken heart" },
    Emoji { ascii: ":-)", emoji: "\u{1F600}", name: "smile" },
    Emoji { ascii: ";-)", emoji: "\u{1F609}", name: "wink" },
    Emoji { ascii: ":-D", emoji: "\u{1F603}", name: "big grin" },
    Emoji { ascii: ":-P", emoji: "\u{1F61B}", name: "tongue" },
    Emoji { ascii: ":-p", emoji: "\u{1F61B}", name: "tongue" },
    Emoji { ascii: ":-(", emoji: "\u{1F61E}", name: "sad" },
    Emoji { ascii: ":-/", emoji: "\u{1F615}", name: "confused" },
    Emoji { ascii: ":-|", emoji: "\u{1F610}", name: "neutral" },
    Emoji { ascii: ":-O", emoji: "\u{1F62E}", name: "surprised" },
    Emoji { ascii: ":-o", emoji: "\u{1F62E}", name: "surprised" },
    Emoji { ascii: ":-*", emoji: "\u{1F618}", name: "kiss" },
    Emoji { ascii: ":-S", emoji: "\u{1F616}", name: "confounded" },
    Emoji { ascii: ":-s", emoji: "\u{1F616}", name: "confounded" },
    Emoji { ascii: ":-X", emoji: "\u{1F910}", name: "mute" },
    Emoji { ascii: ":^)", emoji: "\u{1F60F}", name: "smirk" },
    Emoji { ascii: "B-)", emoji: "\u{1F60E}", name: "cool" },
    Emoji { ascii: ":)", emoji: "\u{1F600}", name: "smile" },
    Emoji { ascii: ";)", emoji: "\u{1F609}", name: "wink" },
    Emoji { ascii: ":D", emoji: "\u{1F603}", name: "big grin" },
    Emoji { ascii: ":P", emoji: "\u{1F61B}", name: "tongue" },
    Emoji { ascii: ":p", emoji: "\u{1F61B}", name: "tongue" },
    Emoji { ascii: ":(", emoji: "\u{1F61E}", name: "sad" },
    Emoji { ascii: ":/", emoji: "\u{1F615}", name: "confused" },
    Emoji { ascii: ":|", emoji: "\u{1F610}", name: "neutral" },
    Emoji { ascii: ":O", emoji: "\u{1F62E}", name: "surprised" },
    Emoji { ascii: ":o", emoji: "\u{1F62E}", name: "surprised" },
    Emoji { ascii: ":*", emoji: "\u{1F618}", name: "kiss" },
    Emoji { ascii: ":S", emoji: "\u{1F616}", name: "confounded" },
    Emoji { ascii: ":s", emoji: "\u{1F616}", name: "confounded" },
    Emoji { ascii: ":X", emoji: "\u{1F910}", name: "mute" },
    Emoji { ascii: "B)", emoji: "\u{1F60E}", name: "cool" },
    Emoji { ascii: "XD", emoji: "\u{1F606}", name: "laughing" },
    Emoji { ascii: "xD", emoji: "\u{1F606}", name: "laughing" },
    Emoji { ascii: "<3", emoji: "\u{2764}\u{FE0F}", name: "heart" },
];

/// Get the full emoji table.
pub fn table() -> &'static [Emoji] {
    EMOJI_TABLE
}

/// Look up ASCII smiley -> emoji. Returns None if not found.
pub fn lookup(ascii: &str) -> Option<&'static str> {
    EMOJI_TABLE.iter()
        .find(|e| e.ascii == ascii)
        .map(|e| e.emoji)
}

/// Look up emoji -> ASCII smiley. Returns None if not found.
pub fn reverse(emoji: &str) -> Option<&'static str> {
    EMOJI_TABLE.iter()
        .find(|e| e.emoji == emoji)
        .map(|e| e.ascii)
}

/// Check if a byte is a word character (ASCII letter or digit).
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// Replace all ASCII smileys with emoji in text.
/// Respects word boundaries (won't mangle URLs or code).
pub fn replace(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = Vec::with_capacity(len * 2);
    let mut i = 0;

    while i < len {
        // Don't replace inside words -- smiley must be at word boundary
        let at_boundary = i == 0 || !is_word_byte(bytes[i - 1]);

        if at_boundary {
            let mut matched = false;
            for entry in EMOJI_TABLE {
                let ascii_bytes = entry.ascii.as_bytes();
                let alen = ascii_bytes.len();
                if i + alen <= len && &bytes[i..i + alen] == ascii_bytes {
                    // Check end boundary
                    let after = i + alen;
                    let mut end_boundary = after >= len || !is_word_byte(bytes[after]);
                    // Short smileys (2 chars or less) -- skip if followed by path-like chars
                    if alen <= 2 && after < len && (bytes[after] == b'/' || bytes[after] == b'\\') {
                        end_boundary = false;
                    }
                    if end_boundary {
                        out.extend_from_slice(entry.emoji.as_bytes());
                        i += alen;
                        matched = true;
                        break;
                    }
                }
            }
            if matched { continue; }
        }

        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Replace all emoji with ASCII smileys in text.
pub fn strip(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        let mut matched = false;
        for entry in EMOJI_TABLE {
            let emoji_bytes = entry.emoji.as_bytes();
            let elen = emoji_bytes.len();
            if i + elen <= len && &bytes[i..i + elen] == emoji_bytes {
                out.extend_from_slice(entry.ascii.as_bytes());
                i += elen;
                matched = true;
                break;
            }
        }
        if !matched {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_not_empty() {
        let t = table();
        assert!(t.len() > 30);
        // Verify every entry has non-empty fields
        for e in t {
            assert!(!e.ascii.is_empty());
            assert!(!e.emoji.is_empty());
            assert!(!e.name.is_empty());
        }
    }

    #[test]
    fn test_table_sorted_longest_first() {
        // Within groups sharing the same emoji, longer ASCII should come first
        let t = table();
        // Check that ":-)" comes before ":)"
        let long_idx = t.iter().position(|e| e.ascii == ":-)").unwrap();
        let short_idx = t.iter().position(|e| e.ascii == ":)").unwrap();
        assert!(long_idx < short_idx);
    }

    #[test]
    fn test_lookup() {
        assert_eq!(lookup(":-)"), Some("\u{1F600}"));
        assert_eq!(lookup(":)"), Some("\u{1F600}"));
        assert_eq!(lookup(";-)"), Some("\u{1F609}"));
        assert_eq!(lookup(":-D"), Some("\u{1F603}"));
        assert_eq!(lookup(":-("), Some("\u{1F61E}"));
        assert_eq!(lookup("<3"), Some("\u{2764}\u{FE0F}"));
        assert_eq!(lookup("</3"), Some("\u{1F494}"));
        assert_eq!(lookup("nonexistent"), None);
    }

    #[test]
    fn test_reverse() {
        assert_eq!(reverse("\u{1F600}"), Some(":-)"));  // first match for smile
        assert_eq!(reverse("\u{1F609}"), Some(";-)"));
        assert_eq!(reverse("\u{1F603}"), Some(":-D"));
        assert_eq!(reverse("\u{2764}\u{FE0F}"), Some("<3"));
        assert_eq!(reverse("\u{1F494}"), Some("</3"));
        assert_eq!(reverse("X"), None);
    }

    #[test]
    fn test_replace_basic() {
        assert_eq!(replace("hello :) world"), "hello \u{1F600} world");
        assert_eq!(replace("hi ;) there"), "hi \u{1F609} there");
        assert_eq!(replace("I <3 you"), "I \u{2764}\u{FE0F} you");
    }

    #[test]
    fn test_replace_prefers_longer_match() {
        // ":-)" should match before ":)" is considered
        assert_eq!(replace("hello :-) world"), "hello \u{1F600} world");
        assert_eq!(replace("feel :-("), "feel \u{1F61E}");
    }

    #[test]
    fn test_replace_at_boundaries() {
        // At start of string
        assert_eq!(replace(":) hello"), "\u{1F600} hello");
        // At end of string
        assert_eq!(replace("hello :)"), "hello \u{1F600}");
        // After punctuation
        assert_eq!(replace("great! :)"), "great! \u{1F600}");
    }

    #[test]
    fn test_replace_no_mangle_inside_words() {
        // After an alphanumeric character, should NOT replace
        let result = replace("hello:)");
        assert_eq!(result, "hello:)");
    }

    #[test]
    fn test_replace_no_mangle_urls() {
        // Short smileys followed by / should NOT be replaced (URL protection)
        let result = replace("http:/something");
        assert_eq!(result, "http:/something");
    }

    #[test]
    fn test_replace_multiple() {
        assert_eq!(
            replace("hi :) how are you :( bye ;)"),
            "hi \u{1F600} how are you \u{1F61E} bye \u{1F609}"
        );
    }

    #[test]
    fn test_replace_no_smileys() {
        let text = "just plain text with no smileys";
        assert_eq!(replace(text), text);
    }

    #[test]
    fn test_replace_empty() {
        assert_eq!(replace(""), "");
    }

    #[test]
    fn test_replace_angry() {
        assert_eq!(replace("I'm >:( about this"), "I'm \u{1F620} about this");
    }

    #[test]
    fn test_replace_angel() {
        assert_eq!(replace("O:) angel"), "\u{1F607} angel");
    }

    #[test]
    fn test_replace_cry() {
        assert_eq!(replace("so sad :'("), "so sad \u{1F622}");
    }

    #[test]
    fn test_replace_broken_heart() {
        assert_eq!(replace("</3 broken"), "\u{1F494} broken");
    }

    #[test]
    fn test_strip_basic() {
        let input = format!("hello {} world", "\u{1F600}");
        assert_eq!(strip(&input), "hello :-) world");
    }

    #[test]
    fn test_strip_multiple() {
        let input = format!("hi {} how {} bye {}", "\u{1F600}", "\u{1F61E}", "\u{1F609}");
        assert_eq!(strip(&input), "hi :-) how :-( bye ;-)");
    }

    #[test]
    fn test_strip_heart() {
        let input = format!("I {} you", "\u{2764}\u{FE0F}");
        assert_eq!(strip(&input), "I <3 you");
    }

    #[test]
    fn test_strip_no_emoji() {
        let text = "just plain text";
        assert_eq!(strip(text), text);
    }

    #[test]
    fn test_strip_empty() {
        assert_eq!(strip(""), "");
    }

    #[test]
    fn test_roundtrip_replace_strip() {
        // replace then strip should (mostly) recover original
        let original = "hello :-) world :-( bye ;-)";
        let with_emoji = replace(original);
        let back = strip(&with_emoji);
        assert_eq!(back, original);
    }

    #[test]
    fn test_roundtrip_strip_replace() {
        // strip then replace should recover emoji text
        let original = format!("hello {} world {} bye {}",
            "\u{1F600}", "\u{1F61E}", "\u{1F609}");
        let with_ascii = strip(&original);
        let back = replace(&with_ascii);
        assert_eq!(back, original);
    }

    #[test]
    fn test_replace_cool() {
        assert_eq!(replace("B-) cool"), "\u{1F60E} cool");
        assert_eq!(replace("B) also cool"), "\u{1F60E} also cool");
    }

    #[test]
    fn test_replace_laughing() {
        assert_eq!(replace("XD haha"), "\u{1F606} haha");
        assert_eq!(replace("xD lol"), "\u{1F606} lol");
    }

    #[test]
    fn test_replace_tongue() {
        assert_eq!(replace(":-P bleh"), "\u{1F61B} bleh");
        assert_eq!(replace(":p bleh"), "\u{1F61B} bleh");
    }

    #[test]
    fn test_replace_mute() {
        assert_eq!(replace(":-X shh"), "\u{1F910} shh");
    }

    #[test]
    fn test_replace_smirk() {
        assert_eq!(replace(":^) heh"), "\u{1F60F} heh");
    }

    #[test]
    fn test_replace_confused() {
        assert_eq!(replace(":-/ hmm"), "\u{1F615} hmm");
    }

    #[test]
    fn test_replace_neutral() {
        assert_eq!(replace(":-| meh"), "\u{1F610} meh");
    }

    #[test]
    fn test_replace_surprised() {
        assert_eq!(replace(":-O wow"), "\u{1F62E} wow");
        assert_eq!(replace(":-o wow"), "\u{1F62E} wow");
    }

    #[test]
    fn test_replace_kiss() {
        assert_eq!(replace(":-* mwah"), "\u{1F618} mwah");
    }

    #[test]
    fn test_replace_confounded() {
        assert_eq!(replace(":-S ugh"), "\u{1F616} ugh");
        assert_eq!(replace(":-s ugh"), "\u{1F616} ugh");
    }
}
