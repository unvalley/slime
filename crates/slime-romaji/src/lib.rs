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
    /// `の`. Two `n`s represent one `ん` and stay editable as two key strokes
    /// until the next input or a flush commits them.
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

            // `tch` is the one common sokuon spelling that is not expressed by
            // a doubled consonant (for example, `matcha` -> `まっちゃ`). Keep
            // `tc` pending until the `h` arrives, then consume only the `t`.
            if self.pending == "tc" {
                break;
            }
            if self.pending.starts_with("tch") {
                output.push('っ');
                self.pending.remove(0);
                continue;
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

                        // Mainstream IMEs spend both `n`s on one ん and start
                        // the next syllable fresh: `sennyou` -> せんよう, while
                        // こんな needs `konnna` or `kon'na`.
                        output.push('ん');
                        self.pending.drain(..2);
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

/// Returns every `(kana, romaji)` spelling pair of the composition table.
///
/// Arrow shortcuts are excluded so reverse transliteration only sees kana.
pub fn kana_spellings() -> impl Iterator<Item = (&'static str, &'static str)> {
    ROMAJI_TABLE
        .iter()
        .filter(|(_, kana)| !matches!(*kana, "←" | "↓" | "↑" | "→"))
        .map(|(romaji, kana)| (*kana, *romaji))
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

// Kana mappings follow Mozc's default romaji table. Stateful `n`/sokuon rules
// are handled above. Arrow shortcuts are kept here as composition results so
// they remain editable in the same preedit as kana.
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
    ("kyi", "きぃ"),
    ("kyu", "きゅ"),
    ("kye", "きぇ"),
    ("kyo", "きょ"),
    ("ga", "が"),
    ("gi", "ぎ"),
    ("gu", "ぐ"),
    ("ge", "げ"),
    ("go", "ご"),
    ("gya", "ぎゃ"),
    ("gyi", "ぎぃ"),
    ("gyu", "ぎゅ"),
    ("gye", "ぎぇ"),
    ("gyo", "ぎょ"),
    ("sa", "さ"),
    ("si", "し"),
    ("shi", "し"),
    ("su", "す"),
    ("se", "せ"),
    ("so", "そ"),
    ("sya", "しゃ"),
    ("syi", "しぃ"),
    ("syu", "しゅ"),
    ("sye", "しぇ"),
    ("syo", "しょ"),
    ("sha", "しゃ"),
    ("shu", "しゅ"),
    ("she", "しぇ"),
    ("sho", "しょ"),
    ("za", "ざ"),
    ("zi", "じ"),
    ("ji", "じ"),
    ("zu", "ず"),
    ("ze", "ぜ"),
    ("zo", "ぞ"),
    ("zya", "じゃ"),
    ("zyi", "じぃ"),
    ("zyu", "じゅ"),
    ("zye", "じぇ"),
    ("zyo", "じょ"),
    ("zh", "←"),
    ("zj", "↓"),
    ("zk", "↑"),
    ("zl", "→"),
    ("zm", "→"),
    ("ja", "じゃ"),
    ("jya", "じゃ"),
    ("jyi", "じぃ"),
    ("ju", "じゅ"),
    ("jyu", "じゅ"),
    ("je", "じぇ"),
    ("jye", "じぇ"),
    ("jo", "じょ"),
    ("jyo", "じょ"),
    ("ta", "た"),
    ("ti", "ち"),
    ("chi", "ち"),
    ("tu", "つ"),
    ("tsu", "つ"),
    ("te", "て"),
    ("to", "と"),
    ("tya", "ちゃ"),
    ("tyi", "ちぃ"),
    ("tyu", "ちゅ"),
    ("tye", "ちぇ"),
    ("tyo", "ちょ"),
    ("cha", "ちゃ"),
    ("chu", "ちゅ"),
    ("che", "ちぇ"),
    ("cho", "ちょ"),
    ("cya", "ちゃ"),
    ("cyi", "ちぃ"),
    ("cyu", "ちゅ"),
    ("cye", "ちぇ"),
    ("cyo", "ちょ"),
    ("tsa", "つぁ"),
    ("tsi", "つぃ"),
    ("tse", "つぇ"),
    ("tso", "つぉ"),
    ("tha", "てゃ"),
    ("thi", "てぃ"),
    ("t'i", "てぃ"),
    ("thu", "てゅ"),
    ("the", "てぇ"),
    ("tho", "てょ"),
    ("t'yu", "てゅ"),
    ("twa", "とぁ"),
    ("twi", "とぃ"),
    ("twu", "とぅ"),
    ("twe", "とぇ"),
    ("two", "とぉ"),
    ("t'u", "とぅ"),
    ("da", "だ"),
    ("di", "ぢ"),
    ("du", "づ"),
    ("de", "で"),
    ("do", "ど"),
    ("dya", "ぢゃ"),
    ("dyi", "ぢぃ"),
    ("dyu", "ぢゅ"),
    ("dye", "ぢぇ"),
    ("dyo", "ぢょ"),
    ("dha", "でゃ"),
    ("dhi", "でぃ"),
    ("d'i", "でぃ"),
    ("dhu", "でゅ"),
    ("dhe", "でぇ"),
    ("dho", "でょ"),
    ("d'yu", "でゅ"),
    ("dwa", "どぁ"),
    ("dwi", "どぃ"),
    ("dwu", "どぅ"),
    ("dwe", "どぇ"),
    ("dwo", "どぉ"),
    ("d'u", "どぅ"),
    ("na", "な"),
    ("ni", "に"),
    ("nu", "ぬ"),
    ("ne", "ね"),
    ("no", "の"),
    ("nya", "にゃ"),
    ("nyi", "にぃ"),
    ("nyu", "にゅ"),
    ("nye", "にぇ"),
    ("nyo", "にょ"),
    ("n", "ん"),
    ("xn", "ん"),
    ("ha", "は"),
    ("hi", "ひ"),
    ("hu", "ふ"),
    ("fu", "ふ"),
    ("he", "へ"),
    ("ho", "ほ"),
    ("hya", "ひゃ"),
    ("hyi", "ひぃ"),
    ("hyu", "ひゅ"),
    ("hye", "ひぇ"),
    ("hyo", "ひょ"),
    ("fa", "ふぁ"),
    ("fi", "ふぃ"),
    ("fe", "ふぇ"),
    ("fo", "ふぉ"),
    ("fya", "ふゃ"),
    ("fyu", "ふゅ"),
    ("fyo", "ふょ"),
    ("hwa", "ふぁ"),
    ("hwi", "ふぃ"),
    ("hwe", "ふぇ"),
    ("hwo", "ふぉ"),
    ("hwyu", "ふゅ"),
    ("ba", "ば"),
    ("bi", "び"),
    ("bu", "ぶ"),
    ("be", "べ"),
    ("bo", "ぼ"),
    ("bya", "びゃ"),
    ("byi", "びぃ"),
    ("byu", "びゅ"),
    ("bye", "びぇ"),
    ("byo", "びょ"),
    ("pa", "ぱ"),
    ("pi", "ぴ"),
    ("pu", "ぷ"),
    ("pe", "ぺ"),
    ("po", "ぽ"),
    ("pya", "ぴゃ"),
    ("pyi", "ぴぃ"),
    ("pyu", "ぴゅ"),
    ("pye", "ぴぇ"),
    ("pyo", "ぴょ"),
    ("ma", "ま"),
    ("mi", "み"),
    ("mu", "む"),
    ("me", "め"),
    ("mo", "も"),
    ("mya", "みゃ"),
    ("myi", "みぃ"),
    ("myu", "みゅ"),
    ("mye", "みぇ"),
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
    ("ryi", "りぃ"),
    ("ryu", "りゅ"),
    ("rye", "りぇ"),
    ("ryo", "りょ"),
    ("wa", "わ"),
    ("wi", "うぃ"),
    ("we", "うぇ"),
    ("wo", "を"),
    ("wha", "うぁ"),
    ("whi", "うぃ"),
    ("whu", "う"),
    ("whe", "うぇ"),
    ("who", "うぉ"),
    ("wyi", "ゐ"),
    ("wye", "ゑ"),
    ("wu", "う"),
    ("va", "ゔぁ"),
    ("vi", "ゔぃ"),
    ("vu", "ゔ"),
    ("ve", "ゔぇ"),
    ("vo", "ゔぉ"),
    ("vya", "ゔゃ"),
    ("vyi", "ゔぃ"),
    ("vyu", "ゔゅ"),
    ("vye", "ゔぇ"),
    ("vyo", "ゔょ"),
    ("xya", "ゃ"),
    ("xyu", "ゅ"),
    ("xyo", "ょ"),
    ("xyi", "ぃ"),
    ("xye", "ぇ"),
    ("xwa", "ゎ"),
    ("lya", "ゃ"),
    ("lyu", "ゅ"),
    ("lyo", "ょ"),
    ("lyi", "ぃ"),
    ("lye", "ぇ"),
    ("lwa", "ゎ"),
    ("xtu", "っ"),
    ("xtsu", "っ"),
    ("ltu", "っ"),
    ("ltsu", "っ"),
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
    ("xka", "ヵ"),
    ("xke", "ヶ"),
    ("lka", "ヵ"),
    ("lke", "ヶ"),
    ("ye", "いぇ"),
    ("ca", "か"),
    ("ci", "し"),
    ("cu", "く"),
    ("ce", "せ"),
    ("co", "こ"),
    ("qa", "くぁ"),
    ("qi", "くぃ"),
    ("qu", "く"),
    ("qe", "くぇ"),
    ("qo", "くぉ"),
    ("kwa", "くぁ"),
    ("kwi", "くぃ"),
    ("kwu", "くぅ"),
    ("kwe", "くぇ"),
    ("kwo", "くぉ"),
    ("gwa", "ぐぁ"),
    ("gwi", "ぐぃ"),
    ("gwu", "ぐぅ"),
    ("gwe", "ぐぇ"),
    ("gwo", "ぐぉ"),
    ("swa", "すぁ"),
    ("swi", "すぃ"),
    ("swu", "すぅ"),
    ("swe", "すぇ"),
    ("swo", "すぉ"),
    ("zwa", "ずぁ"),
    ("zwi", "ずぃ"),
    ("zwu", "ずぅ"),
    ("zwe", "ずぇ"),
    ("zwo", "ずぉ"),
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
        assert_eq!(compose("matcha"), "まっちゃ");
    }

    #[test]
    fn converts_foreign_sounds() {
        assert_eq!(compose("pafo"), "ぱふぉ");
        assert_eq!(compose("vaio"), "ゔぁいお");
        assert_eq!(compose("thi"), "てぃ");
        assert_eq!(compose("dhu"), "でゅ");
        assert_eq!(compose("tsa"), "つぁ");
        assert_eq!(compose("kwa"), "くぁ");
        assert_eq!(compose("she"), "しぇ");
    }

    #[test]
    fn converts_alternative_spellings_and_small_kana() {
        assert_eq!(compose("jye"), "じぇ");
        assert_eq!(compose("t'i"), "てぃ");
        assert_eq!(compose("xtsu"), "っ");
        assert_eq!(compose("xka"), "ヵ");
        assert_eq!(compose("wye"), "ゑ");
    }

    #[test]
    fn converts_arrow_shortcuts() {
        assert_eq!(compose("zh"), "←");
        assert_eq!(compose("zj"), "↓");
        assert_eq!(compose("zk"), "↑");
        assert_eq!(compose("zl"), "→");
        assert_eq!(compose("zm"), "→");
        assert_eq!(compose("zhzm"), "←→");
    }

    #[test]
    fn handles_syllabic_n() {
        assert_eq!(compose("kanpai"), "かんぱい");
        assert_eq!(compose("kin'youbi"), "きんようび");
        assert_eq!(compose("hon"), "ほん");
        assert_eq!(compose("honn"), "ほん");
        assert_eq!(compose("sennyou"), "せんよう");
        assert_eq!(compose("sannin"), "さんいん");
        assert_eq!(compose("konnna"), "こんな");
        assert_eq!(compose("kon'na"), "こんな");
        assert_eq!(compose("annnai"), "あんない");
        assert_eq!(compose("minnna"), "みんな");
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
