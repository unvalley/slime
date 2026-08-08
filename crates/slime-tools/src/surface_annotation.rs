use std::collections::HashMap;
use std::fs;
use std::path::Path;

const MAXIMUM_SURFACE_CHARACTERS: usize = 12;

pub struct SurfaceReadingIndex {
    readings: HashMap<String, Option<String>>,
}

impl SurfaceReadingIndex {
    /// Loads the first two columns of a reading/surface TSV dictionary.
    ///
    /// # Errors
    ///
    /// Returns an error when the dictionary cannot be read as UTF-8 text.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let mut readings: HashMap<String, Option<String>> = HashMap::new();
        for line in content.lines() {
            let mut columns = line.split('\t');
            let (Some(reading), Some(surface)) = (columns.next(), columns.next()) else {
                continue;
            };
            readings
                .entry(surface.to_owned())
                .and_modify(|existing| {
                    if existing.as_deref() != Some(reading) {
                        *existing = None;
                    }
                })
                .or_insert_with(|| Some(reading.to_owned()));
        }
        readings.retain(|_, reading| reading.is_some());
        Ok(Self { readings })
    }

    #[must_use]
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut readings = HashMap::new();
        for (surface, reading) in pairs {
            readings.insert(surface, Some(reading));
        }
        Self { readings }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.readings.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.readings.is_empty()
    }

    #[must_use]
    pub fn reading(&self, text: &str) -> Option<String> {
        self.annotate(text)
            .map(|tokens| tokens.into_iter().map(|(_, reading)| reading).collect())
    }

    /// Greedily maps a surface string to unambiguous dictionary readings.
    /// Unknown kanji reject the complete line so generated training data never
    /// invents a pronunciation. Kana and non-word symbols pass through.
    #[must_use]
    pub fn annotate(&self, text: &str) -> Option<Vec<(String, String)>> {
        self.annotate_with_policy(text, false)
    }

    /// Annotates standalone corpus lines while allowing literal punctuation,
    /// numbers, and Latin text. Unknown kanji remain a hard rejection.
    #[must_use]
    pub fn annotate_plain_text(&self, text: &str) -> Option<Vec<(String, String)>> {
        self.annotate_with_policy(text, true)
    }

    fn annotate_with_policy(
        &self,
        text: &str,
        allow_general_literals: bool,
    ) -> Option<Vec<(String, String)>> {
        let characters: Vec<_> = text.chars().collect();
        let mut tokens = Vec::new();
        let mut position = 0;
        while position < characters.len() {
            let mut matched = false;
            let longest = MAXIMUM_SURFACE_CHARACTERS.min(characters.len() - position);
            for length in (2..=longest).rev() {
                let surface: String = characters[position..position + length].iter().collect();
                if let Some(Some(surface_reading)) = self.readings.get(&surface) {
                    if !is_serializable_token(&surface, surface_reading) {
                        return None;
                    }
                    tokens.push((surface, surface_reading.clone()));
                    position += length;
                    matched = true;
                    break;
                }
            }
            if matched {
                continue;
            }

            let character = characters[position];
            if is_kana(character) {
                tokens.push((
                    character.to_string(),
                    katakana_to_hiragana(character).to_string(),
                ));
                position += 1;
            } else if allow_general_literals && character.is_whitespace() {
                position += 1;
            } else if let Some(Some(surface_reading)) = self.readings.get(&character.to_string()) {
                if !is_serializable_token(&character.to_string(), surface_reading) {
                    return None;
                }
                tokens.push((character.to_string(), surface_reading.clone()));
                position += 1;
            } else if is_kanji(character) || matches!(character, '/' | '\t') {
                return None;
            } else if allow_general_literals || is_strict_literal(character) {
                let literal = character.to_string();
                tokens.push((literal.clone(), literal));
                position += 1;
            } else {
                return None;
            }
        }
        (!tokens.is_empty()).then_some(tokens)
    }
}

fn is_serializable_token(surface: &str, reading: &str) -> bool {
    !surface.contains(['/', ' ', '\t']) && !reading.contains(['/', ' ', '\t'])
}

#[must_use]
pub fn contains_kanji(text: &str) -> bool {
    text.chars().any(is_kanji)
}

fn is_kanji(character: char) -> bool {
    matches!(character, '\u{4e00}'..='\u{9fff}' | '々' | '〆')
}

fn is_kana(character: char) -> bool {
    matches!(character, 'ぁ'..='ゖ' | 'ゝ' | 'ゞ' | 'ァ'..='ヶ' | 'ー' | 'ヽ' | 'ヾ')
}

fn is_strict_literal(character: char) -> bool {
    matches!(
        character,
        '、' | '。'
            | '！'
            | '？'
            | '「'
            | '」'
            | '『'
            | '』'
            | '（'
            | '）'
            | '：'
            | '；'
            | '・'
            | '…'
    )
}

fn katakana_to_hiragana(character: char) -> char {
    match character {
        'ァ'..='ヶ' | 'ヽ' | 'ヾ' => {
            char::from_u32(u32::from(character) - 0x60).expect("valid hiragana scalar")
        }
        _ => character,
    }
}

#[must_use]
pub fn hiragana_to_katakana(reading: &str) -> String {
    reading
        .chars()
        .map(|character| match character {
            'ぁ'..='ゖ' | 'ゝ' | 'ゞ' => {
                char::from_u32(u32::from(character) + 0x60).unwrap_or(character)
            }
            _ => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SurfaceReadingIndex, hiragana_to_katakana};

    #[test]
    fn annotates_unambiguous_dictionary_surfaces_and_literals() {
        let index = SurfaceReadingIndex::from_pairs([
            ("漢字".to_owned(), "かんじ".to_owned()),
            ("変換".to_owned(), "へんかん".to_owned()),
        ]);
        assert_eq!(
            index.annotate("漢字への変換。").unwrap(),
            [
                ("漢字".to_owned(), "かんじ".to_owned()),
                ("へ".to_owned(), "へ".to_owned()),
                ("の".to_owned(), "の".to_owned()),
                ("変換".to_owned(), "へんかん".to_owned()),
                ("。".to_owned(), "。".to_owned()),
            ]
        );
    }

    #[test]
    fn rejects_unknown_kanji_instead_of_inventing_a_reading() {
        let index = SurfaceReadingIndex::from_pairs([]);
        assert!(index.annotate("未知").is_none());
        assert!(index.annotate("2026年").is_none());
    }

    #[test]
    fn plain_text_policy_allows_symbols_without_weakening_strict_annotation() {
        let index = SurfaceReadingIndex::from_pairs([("年".to_owned(), "ねん".to_owned())]);
        assert!(index.annotate("2026年").is_none());
        assert!(index.annotate_plain_text("2026年").is_some());
    }

    #[test]
    fn readings_can_be_emitted_as_katakana() {
        assert_eq!(hiragana_to_katakana("かけい、ゔ"), "カケイ、ヴ");
    }
}
