//! Incremental romaji-to-hiragana composition.

use std::fmt;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RomajiComposer {
    pending: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidRomaji(pub char);

impl fmt::Display for InvalidRomaji {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported romaji character: {:?}", self.0)
    }
}

impl std::error::Error for InvalidRomaji {}

impl RomajiComposer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending: String::new(),
        }
    }

    /// Adds one ASCII letter and returns any hiragana that became unambiguous.
    ///
    /// The uncommitted suffix remains available through [`Self::pending`].
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRomaji`] when `character` is not an ASCII letter or an
    /// apostrophe. The pending composition is not changed in that case.
    pub fn push(&mut self, character: char) -> Result<String, InvalidRomaji> {
        if !character.is_ascii_alphabetic() && character != '\'' {
            return Err(InvalidRomaji(character));
        }

        self.pending.push(character.to_ascii_lowercase());
        Ok(self.resolve(false))
    }

    #[must_use]
    pub fn pending(&self) -> &str {
        &self.pending
    }

    /// Returns the text that should be shown while the romaji is ambiguous.
    ///
    /// A single `n` remains literal because it can still become `な` through
    /// `の`. Two `n`s represent one `ん`, while remaining editable as two key
    /// strokes until the following input resolves the ambiguity.
    #[must_use]
    pub fn preview(&self) -> &str {
        if self.pending == "nn" {
            "ん"
        } else {
            &self.pending
        }
    }

    /// Removes one uncommitted romaji character.
    pub fn backspace(&mut self) -> bool {
        self.pending.pop().is_some()
    }

    /// Resolves a trailing `n` and returns other incomplete input literally.
    pub fn flush(&mut self) -> String {
        let mut output = self.resolve(true);
        output.push_str(&self.pending);
        self.pending.clear();
        output
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    fn resolve(&mut self, flush: bool) -> String {
        let mut output = String::new();

        loop {
            if self.pending.is_empty() {
                break;
            }

            let bytes = self.pending.as_bytes();
            if bytes.len() >= 2 {
                let first = bytes[0];
                let second = bytes[1];

                if first == second && is_consonant(first) && first != b'n' {
                    output.push('っ');
                    self.pending.remove(0);
                    continue;
                }

                if first == b'n' {
                    if second == b'\'' {
                        output.push('ん');
                        self.pending.drain(..2);
                        continue;
                    }
                    if second == b'n' {
                        if bytes.len() == 2 {
                            if flush {
                                output.push('ん');
                                self.pending.clear();
                            }
                            break;
                        }

                        output.push('ん');
                        let third = bytes[2];
                        if is_vowel(third) || third == b'y' {
                            self.pending.remove(0);
                        } else {
                            self.pending.drain(..2);
                        }
                        continue;
                    }
                    if !is_vowel(second) && second != b'y' {
                        output.push('ん');
                        self.pending.remove(0);
                        continue;
                    }
                }
            }

            let matching_entries = matching_entries(&self.pending);
            if let Some((_, kana)) = matching_entries
                .iter()
                .find(|(romaji, _)| *romaji == self.pending)
                && (flush
                    || !matching_entries
                        .iter()
                        .any(|(romaji, _)| romaji.len() > self.pending.len()))
            {
                output.push_str(kana);
                self.pending.clear();
                continue;
            }

            if !matching_entries.is_empty() || self.pending.len() == 1 {
                break;
            }

            // Preserve unsupported but valid ASCII sequences instead of losing input.
            output.push(self.pending.remove(0));
        }

        output
    }
}

const fn is_vowel(byte: u8) -> bool {
    matches!(byte, b'a' | b'i' | b'u' | b'e' | b'o')
}

const fn is_consonant(byte: u8) -> bool {
    byte.is_ascii_alphabetic() && !is_vowel(byte)
}

fn matching_entries(input: &str) -> &'static [(&'static str, &'static str)] {
    static SORTED_TABLE: OnceLock<Box<[(&str, &str)]>> = OnceLock::new();
    let table = SORTED_TABLE.get_or_init(|| {
        let mut table = ROMAJI_TABLE.to_vec();
        table.sort_unstable_by_key(|(romaji, _)| *romaji);
        table.into_boxed_slice()
    });
    let start = table.partition_point(|(romaji, _)| *romaji < input);
    let matching_length = table[start..]
        .iter()
        .take_while(|(romaji, _)| romaji.starts_with(input))
        .count();
    &table[start..start + matching_length]
}

const ROMAJI_TABLE: &[(&str, &str)] = &[
    ("a", "あ"),
    ("i", "い"),
    ("u", "う"),
    ("e", "え"),
    ("o", "お"),
    ("ka", "か"),
    ("ki", "き"),
    ("ku", "く"),
    ("ke", "け"),
    ("ko", "こ"),
    ("kya", "きゃ"),
    ("kyu", "きゅ"),
    ("kyo", "きょ"),
    ("ga", "が"),
    ("gi", "ぎ"),
    ("gu", "ぐ"),
    ("ge", "げ"),
    ("go", "ご"),
    ("gya", "ぎゃ"),
    ("gyu", "ぎゅ"),
    ("gyo", "ぎょ"),
    ("sa", "さ"),
    ("si", "し"),
    ("shi", "し"),
    ("su", "す"),
    ("se", "せ"),
    ("so", "そ"),
    ("sya", "しゃ"),
    ("syu", "しゅ"),
    ("syo", "しょ"),
    ("sha", "しゃ"),
    ("shu", "しゅ"),
    ("sho", "しょ"),
    ("za", "ざ"),
    ("zi", "じ"),
    ("ji", "じ"),
    ("zu", "ず"),
    ("ze", "ぜ"),
    ("zo", "ぞ"),
    ("zya", "じゃ"),
    ("zyu", "じゅ"),
    ("zyo", "じょ"),
    ("ja", "じゃ"),
    ("ju", "じゅ"),
    ("jo", "じょ"),
    ("ta", "た"),
    ("ti", "ち"),
    ("chi", "ち"),
    ("tu", "つ"),
    ("tsu", "つ"),
    ("te", "て"),
    ("to", "と"),
    ("tya", "ちゃ"),
    ("tyu", "ちゅ"),
    ("tyo", "ちょ"),
    ("cha", "ちゃ"),
    ("chu", "ちゅ"),
    ("cho", "ちょ"),
    ("da", "だ"),
    ("di", "ぢ"),
    ("du", "づ"),
    ("de", "で"),
    ("do", "ど"),
    ("dya", "ぢゃ"),
    ("dyu", "ぢゅ"),
    ("dyo", "ぢょ"),
    ("na", "な"),
    ("ni", "に"),
    ("nu", "ぬ"),
    ("ne", "ね"),
    ("no", "の"),
    ("nya", "にゃ"),
    ("nyu", "にゅ"),
    ("nyo", "にょ"),
    ("n", "ん"),
    ("ha", "は"),
    ("hi", "ひ"),
    ("hu", "ふ"),
    ("fu", "ふ"),
    ("he", "へ"),
    ("ho", "ほ"),
    ("hya", "ひゃ"),
    ("hyu", "ひゅ"),
    ("hyo", "ひょ"),
    ("ba", "ば"),
    ("bi", "び"),
    ("bu", "ぶ"),
    ("be", "べ"),
    ("bo", "ぼ"),
    ("bya", "びゃ"),
    ("byu", "びゅ"),
    ("byo", "びょ"),
    ("pa", "ぱ"),
    ("pi", "ぴ"),
    ("pu", "ぷ"),
    ("pe", "ぺ"),
    ("po", "ぽ"),
    ("pya", "ぴゃ"),
    ("pyu", "ぴゅ"),
    ("pyo", "ぴょ"),
    ("ma", "ま"),
    ("mi", "み"),
    ("mu", "む"),
    ("me", "め"),
    ("mo", "も"),
    ("mya", "みゃ"),
    ("myu", "みゅ"),
    ("myo", "みょ"),
    ("ya", "や"),
    ("yu", "ゆ"),
    ("yo", "よ"),
    ("ra", "ら"),
    ("ri", "り"),
    ("ru", "る"),
    ("re", "れ"),
    ("ro", "ろ"),
    ("rya", "りゃ"),
    ("ryu", "りゅ"),
    ("ryo", "りょ"),
    ("wa", "わ"),
    ("wo", "を"),
    ("xya", "ゃ"),
    ("xyu", "ゅ"),
    ("xyo", "ょ"),
    ("lya", "ゃ"),
    ("lyu", "ゅ"),
    ("lyo", "ょ"),
    ("xtu", "っ"),
    ("ltu", "っ"),
    ("xa", "ぁ"),
    ("xi", "ぃ"),
    ("xu", "ぅ"),
    ("xe", "ぇ"),
    ("xo", "ぉ"),
    ("la", "ぁ"),
    ("li", "ぃ"),
    ("lu", "ぅ"),
    ("le", "ぇ"),
    ("lo", "ぉ"),
];

#[cfg(test)]
mod tests {
    use super::{ROMAJI_TABLE, RomajiComposer};

    fn compose(input: &str) -> String {
        let mut composer = RomajiComposer::new();
        let mut output = String::new();
        for character in input.chars() {
            output.push_str(&composer.push(character).unwrap());
        }
        output.push_str(&composer.flush());
        output
    }

    #[test]
    fn converts_basic_syllables() {
        assert_eq!(compose("nihongo"), "にほんご");
        assert_eq!(compose("watashi"), "わたし");
    }

    #[test]
    fn converts_contracted_sounds() {
        assert_eq!(compose("kyoushitsu"), "きょうしつ");
        assert_eq!(compose("ryokou"), "りょこう");
    }

    #[test]
    fn converts_double_consonant() {
        assert_eq!(compose("kitte"), "きって");
        assert_eq!(compose("gakkou"), "がっこう");
    }

    #[test]
    fn handles_syllabic_n() {
        assert_eq!(compose("konna"), "こんな");
        assert_eq!(compose("kanpai"), "かんぱい");
        assert_eq!(compose("kin'youbi"), "きんようび");
        assert_eq!(compose("hon"), "ほん");
        assert_eq!(compose("honn"), "ほん");
        assert_eq!(compose("annai"), "あんない");
    }

    #[test]
    fn previews_ambiguous_n_without_committing_it() {
        let mut composer = RomajiComposer::new();

        assert_eq!(composer.push('n').unwrap(), "");
        assert_eq!(composer.preview(), "n");
        assert_eq!(composer.push('n').unwrap(), "");
        assert_eq!(composer.preview(), "ん");

        assert!(composer.backspace());
        assert_eq!(composer.preview(), "n");
    }

    #[test]
    fn retains_ambiguous_suffix_until_resolved() {
        let mut composer = RomajiComposer::new();
        assert_eq!(composer.push('s').unwrap(), "");
        assert_eq!(composer.push('h').unwrap(), "");
        assert_eq!(composer.pending(), "sh");
        assert_eq!(composer.push('i').unwrap(), "し");
        assert_eq!(composer.pending(), "");
    }

    #[test]
    fn backspace_edits_pending_input() {
        let mut composer = RomajiComposer::new();
        composer.push('k').unwrap();
        assert!(composer.backspace());
        assert_eq!(composer.pending(), "");
        assert!(!composer.backspace());
    }

    #[test]
    fn rejects_non_romaji_characters_without_mutation() {
        let mut composer = RomajiComposer::new();
        assert!(composer.push('1').is_err());
        assert_eq!(composer.pending(), "");
    }

    #[test]
    fn every_table_entry_converts() {
        for (romaji, kana) in ROMAJI_TABLE {
            assert_eq!(compose(romaji), *kana, "failed to convert {romaji}");
        }
    }
}
