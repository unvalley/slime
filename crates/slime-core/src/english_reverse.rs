//! Recovers the ASCII word a mangled kana reading was meant to spell.
//!
//! Typing an English word without leaving kana mode runs it through the
//! romaji table, so `github` becomes ぎてゅb. Reversing each kana back to its
//! spellings (trying every alternative, e.g. し -> si/shi) reconstructs the
//! keystrokes and lets known ASCII surfaces match the reading.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReverseMatch {
    Exact,
    Prefix,
}

/// Minimum ASCII letters a key needs before reverse matching applies, so a
/// couple of stray letters do not surface unrelated words.
const MIN_KEY_LENGTH: usize = 3;

/// Lowercases `surface` and keeps `[a-z0-9]` as the comparison key.
///
/// Returns `None` for surfaces that are not predominantly ASCII words.
#[must_use]
pub fn surface_key(surface: &str) -> Option<String> {
    if !surface.is_ascii() {
        return None;
    }
    let key: String = surface
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    (key.chars().filter(char::is_ascii_alphabetic).count() >= MIN_KEY_LENGTH).then_some(key)
}

/// Reports whether `reading` retypes `key` exactly or as a strict prefix.
#[must_use]
pub fn reverse_match(reading: &str, key: &str) -> Option<ReverseMatch> {
    if key.len() < MIN_KEY_LENGTH || reading.is_empty() {
        return None;
    }
    let reading: Vec<char> = reading.chars().collect();
    // Require most of the key before a prefix counts, so short fragments do
    // not pull in every surface that shares a first syllable.
    let minimum_matched = MIN_KEY_LENGTH.max(key.len().saturating_sub(4));
    matches(&reading, key.as_bytes()).and_then(|matched| {
        if matched == key.len() {
            Some(ReverseMatch::Exact)
        } else {
            (matched >= minimum_matched).then_some(ReverseMatch::Prefix)
        }
    })
}

/// Returns how many bytes of `key` the whole reading can spell, if any.
fn matches(reading: &[char], key: &[u8]) -> Option<usize> {
    if reading.is_empty() {
        return Some(0);
    }
    let character = reading[0];

    if character.is_ascii() {
        let byte = character.to_ascii_lowercase() as u8;
        if key.first() == Some(&byte) {
            return matches(&reading[1..], &key[1..]).map(|matched| matched + 1);
        }
        return None;
    }

    if character == 'っ' {
        // A sokuon doubles the consonant that starts the next syllable.
        if let (Some(&byte), Some(&next)) = (key.first(), key.get(1))
            && byte == next
            && !matches!(byte, b'a' | b'i' | b'u' | b'e' | b'o')
        {
            return matches(&reading[1..], &key[1..]).map(|matched| matched + 1);
        }
        return None;
    }

    for (kana, spelling) in spellings() {
        let kana_characters: &[char] = kana;
        if reading.starts_with(kana_characters)
            && key.starts_with(spelling.as_bytes())
            && let Some(matched) =
                matches(&reading[kana_characters.len()..], &key[spelling.len()..])
        {
            return Some(matched + spelling.len());
        }
    }
    None
}

fn spellings() -> &'static [(Vec<char>, &'static str)] {
    static SPELLINGS: OnceLock<Vec<(Vec<char>, &'static str)>> = OnceLock::new();
    SPELLINGS.get_or_init(|| {
        let mut by_kana: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for (kana, romaji) in slime_romaji::kana_spellings() {
            if romaji.contains('\'') {
                continue;
            }
            by_kana.entry(kana).or_default().push(romaji);
        }
        by_kana.insert("ん", vec!["n", "nn"]);

        let mut spellings: Vec<(Vec<char>, &'static str)> = by_kana
            .into_iter()
            .flat_map(|(kana, romajis)| {
                let characters: Vec<char> = kana.chars().collect();
                romajis
                    .into_iter()
                    .map(move |romaji| (characters.clone(), romaji))
            })
            .collect();
        // Longer kana first so てゅ is tried before て, and longer spellings
        // first so a prefix spelling cannot shadow a complete one.
        spellings.sort_by(|left, right| {
            right
                .0
                .len()
                .cmp(&left.0.len())
                .then_with(|| right.1.len().cmp(&left.1.len()))
        });
        spellings
    })
}

#[cfg(test)]
mod tests {
    use super::{ReverseMatch, reverse_match, surface_key};

    #[test]
    fn mangled_readings_reverse_to_their_ascii_words() {
        assert_eq!(
            reverse_match("ぎてゅb", "github"),
            Some(ReverseMatch::Exact)
        );
        assert_eq!(
            reverse_match("pyてょん", "python"),
            Some(ReverseMatch::Exact)
        );
        assert_eq!(reverse_match("でしgn", "design"), Some(ReverseMatch::Exact));
        assert_eq!(reverse_match("しft", "shift"), Some(ReverseMatch::Exact));
        assert_eq!(
            reverse_match("ぎてゅ", "github"),
            Some(ReverseMatch::Prefix)
        );
    }

    #[test]
    fn unrelated_readings_do_not_match() {
        assert_eq!(reverse_match("にほんご", "github"), None);
        assert_eq!(reverse_match("ぎてゅb", "gitlab"), None);
        assert_eq!(reverse_match("ぎ", "github"), None);
    }

    #[test]
    fn surface_keys_keep_ascii_words_only() {
        assert_eq!(surface_key("GitHub"), Some("github".to_owned()));
        assert_eq!(surface_key("Node.js"), Some("nodejs".to_owned()));
        assert_eq!(surface_key("C++"), None);
        assert_eq!(surface_key("ギットハブ"), None);
    }
}
