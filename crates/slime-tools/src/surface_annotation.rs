use compact_str::CompactString;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const MAXIMUM_SURFACE_CHARACTERS: usize = 12;
const MAXIMUM_RANKED_ENTRIES_PER_SURFACE: usize = 12;
const MAXIMUM_VITERBI_STATES_PER_POSITION: usize = 256;
const BOS_EOS_POS_ID: u16 = 0;
const UNKNOWN_POS_ID: u16 = 1851;
const UNKNOWN_COST: i32 = 10_000;
const INVALID_CONNECTION_COST: i32 = 30_000;

pub struct SurfaceReadingIndex {
    readings: HashMap<String, Option<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RankedSurfaceEntry {
    reading: CompactString,
    left_id: u16,
    right_id: u16,
    word_cost: i32,
}

/// Independently annotates surface text by minimizing Mozc word and connection
/// costs. Development data accepts a reading only when this lattice agrees
/// with the longest unambiguous surface annotation above. The agreement gate
/// prevents a valid dictionary entry from being used across a word boundary,
/// such as reading `曲が` as the stem of `曲がる` in `曲が多い`.
pub struct SurfaceViterbiIndex {
    entries: HashMap<CompactString, RankedSurface>,
    connection: ConnectionMatrix,
}

struct RankedSurface {
    entries: Vec<RankedSurfaceEntry>,
    unambiguous: bool,
}

#[derive(Clone, Debug)]
struct SurfacePath {
    cost: i32,
    previous: Option<usize>,
    reading: String,
    right_id: u16,
}

struct ConnectionMatrix {
    bytes: Vec<u8>,
    size: usize,
    offsets_start: usize,
    modes_start: usize,
    entries_start: usize,
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

impl SurfaceViterbiIndex {
    /// Loads Mozc surface entries and its connection matrix for offline corpus
    /// annotation. Only a bounded number of the cheapest POS-distinct entries
    /// per surface are retained because this is an independent plausibility
    /// check, not a second product dictionary.
    ///
    /// # Errors
    ///
    /// Returns an error when either artifact cannot be read or contains an
    /// invalid TSV row or connection-matrix header.
    pub fn load(dictionary_path: &Path, connection_path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(dictionary_path).map_err(|error| {
            format!(
                "failed to read surface dictionary {}: {error}",
                dictionary_path.display()
            )
        })?;
        let mut entries = HashMap::<CompactString, RankedSurface>::new();
        for (line_index, line) in content.lines().enumerate() {
            let mut columns = line.split('\t');
            let (Some(reading), Some(surface), Some(left_id), Some(right_id), Some(word_cost)) = (
                columns.next(),
                columns.next(),
                columns.next(),
                columns.next(),
                columns.next(),
            ) else {
                return Err(format!(
                    "{} line {} has fewer than five columns",
                    dictionary_path.display(),
                    line_index + 1
                ));
            };
            if columns.next().is_some() {
                return Err(format!(
                    "{} line {} has more than five columns",
                    dictionary_path.display(),
                    line_index + 1
                ));
            }
            if surface.chars().count() > MAXIMUM_SURFACE_CHARACTERS {
                continue;
            }
            let left_id = left_id.parse::<u16>().map_err(|_| {
                format!(
                    "{} line {} has an invalid left ID",
                    dictionary_path.display(),
                    line_index + 1
                )
            })?;
            let right_id = right_id.parse::<u16>().map_err(|_| {
                format!(
                    "{} line {} has an invalid right ID",
                    dictionary_path.display(),
                    line_index + 1
                )
            })?;
            let word_cost = word_cost.parse::<i32>().map_err(|_| {
                format!(
                    "{} line {} has an invalid word cost",
                    dictionary_path.display(),
                    line_index + 1
                )
            })?;
            entries
                .entry(surface.into())
                .or_insert_with(|| RankedSurface {
                    entries: Vec::new(),
                    unambiguous: true,
                })
                .entries
                .push(RankedSurfaceEntry {
                    reading: reading.into(),
                    left_id,
                    right_id,
                    word_cost,
                });
        }
        drop(content);
        compact_ranked_entries(&mut entries);
        Ok(Self {
            entries,
            connection: ConnectionMatrix::load(connection_path)?,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .values()
            .filter(|surface| surface.unambiguous)
            .count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Greedy longest-match annotation retained as one side of the consensus
    /// gate. Ambiguous exact surfaces are skipped just as in
    /// `SurfaceReadingIndex`.
    #[must_use]
    pub fn annotate(&self, text: &str) -> Option<Vec<(String, String)>> {
        let characters = text.chars().collect::<Vec<_>>();
        let mut tokens = Vec::new();
        let mut position = 0;
        while position < characters.len() {
            let mut matched = false;
            let longest = MAXIMUM_SURFACE_CHARACTERS.min(characters.len() - position);
            for length in (2..=longest).rev() {
                let surface = characters[position..position + length]
                    .iter()
                    .collect::<String>();
                let Some(ranked) = self.entries.get(surface.as_str()) else {
                    continue;
                };
                if !ranked.unambiguous {
                    continue;
                }
                let surface_reading = &ranked.entries[0].reading;
                if !is_serializable_token(&surface, surface_reading) {
                    return None;
                }
                tokens.push((surface, surface_reading.to_string()));
                position += length;
                matched = true;
                break;
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
            } else if let Some(ranked) = self.entries.get(character.to_string().as_str()) {
                if !ranked.unambiguous {
                    return None;
                }
                let surface = character.to_string();
                let surface_reading = &ranked.entries[0].reading;
                if !is_serializable_token(&surface, surface_reading) {
                    return None;
                }
                tokens.push((surface, surface_reading.to_string()));
                position += 1;
            } else if is_kanji(character) || matches!(character, '/' | '\t') {
                return None;
            } else if is_strict_literal(character) {
                let literal = character.to_string();
                tokens.push((literal.clone(), literal));
                position += 1;
            } else {
                return None;
            }
        }
        (!tokens.is_empty()).then_some(tokens)
    }

    #[must_use]
    pub fn longest_reading(&self, text: &str) -> Option<String> {
        self.annotate(text)
            .map(|tokens| tokens.into_iter().map(|(_, reading)| reading).collect())
    }

    /// Returns the longest-match tokens only when an independent
    /// connection-cost Viterbi annotation derives the identical reading.
    #[must_use]
    pub fn consensus_annotation(&self, text: &str) -> Option<Vec<(String, String)>> {
        let tokens = self.annotate(text)?;
        let greedy_reading = tokens
            .iter()
            .map(|(_, reading)| reading.as_str())
            .collect::<String>();
        (self.viterbi_reading(text).as_deref() == Some(greedy_reading.as_str())).then_some(tokens)
    }

    #[must_use]
    pub fn viterbi_reading(&self, text: &str) -> Option<String> {
        if text.is_empty() {
            return None;
        }
        let mut boundaries = text
            .char_indices()
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        boundaries.push(text.len());
        let character_count = boundaries.len() - 1;
        let mut arena = vec![SurfacePath {
            cost: 0,
            previous: None,
            reading: String::new(),
            right_id: BOS_EOS_POS_ID,
        }];
        let mut states = vec![HashMap::<u16, usize>::new(); character_count + 1];
        states[0].insert(BOS_EOS_POS_ID, 0);

        for start in 0..character_count {
            // All edges into this position have now been constructed. Trim only
            // here so a state is never discarded before later incoming spans
            // have had a chance to replace it with a cheaper path.
            trim_surface_states(&mut states[start], &arena);
            if states[start].is_empty() {
                continue;
            }
            let predecessors = states[start].values().copied().collect::<Vec<_>>();
            let mut matched = false;
            let longest = MAXIMUM_SURFACE_CHARACTERS.min(character_count - start);
            for length in 1..=longest {
                let end = start + length;
                let surface = &text[boundaries[start]..boundaries[end]];
                let Some(surface_entries) = self.entries.get(surface) else {
                    continue;
                };
                matched = true;
                for entry in &surface_entries.entries {
                    self.extend_paths(&predecessors, &mut arena, &mut states[end], entry);
                }
            }
            if !matched {
                let end = start + 1;
                let literal = &text[boundaries[start]..boundaries[end]];
                let entry = RankedSurfaceEntry {
                    reading: literal
                        .chars()
                        .map(katakana_to_hiragana)
                        .collect::<String>()
                        .into(),
                    left_id: UNKNOWN_POS_ID,
                    right_id: UNKNOWN_POS_ID,
                    word_cost: UNKNOWN_COST,
                };
                self.extend_paths(&predecessors, &mut arena, &mut states[end], &entry);
            }
        }

        let winner = states[character_count]
            .values()
            .copied()
            .min_by_key(|&index| {
                arena[index]
                    .cost
                    .saturating_add(self.connection.cost(arena[index].right_id, BOS_EOS_POS_ID))
            })?;
        let mut segments = Vec::new();
        let mut current = Some(winner);
        while let Some(index) = current {
            let node = &arena[index];
            if !node.reading.is_empty() {
                segments.push(node.reading.as_str());
            }
            current = node.previous;
        }
        segments.reverse();
        Some(segments.concat())
    }

    fn extend_paths(
        &self,
        predecessors: &[usize],
        arena: &mut Vec<SurfacePath>,
        destination: &mut HashMap<u16, usize>,
        entry: &RankedSurfaceEntry,
    ) {
        for &previous_index in predecessors {
            let previous = &arena[previous_index];
            let cost = previous
                .cost
                .saturating_add(self.connection.cost(previous.right_id, entry.left_id))
                .saturating_add(entry.word_cost);
            if let Some(&existing_index) = destination.get(&entry.right_id) {
                if cost < arena[existing_index].cost {
                    arena[existing_index] = SurfacePath {
                        cost,
                        previous: Some(previous_index),
                        reading: entry.reading.to_string(),
                        right_id: entry.right_id,
                    };
                }
            } else {
                let index = arena.len();
                arena.push(SurfacePath {
                    cost,
                    previous: Some(previous_index),
                    reading: entry.reading.to_string(),
                    right_id: entry.right_id,
                });
                destination.insert(entry.right_id, index);
            }
        }
    }
}

fn compact_ranked_entries(entries: &mut HashMap<CompactString, RankedSurface>) {
    for ranked_surface in entries.values_mut() {
        let surface_entries = &mut ranked_surface.entries;
        ranked_surface.unambiguous = surface_entries
            .iter()
            .map(|entry| entry.reading.as_str())
            .all(|reading| reading == surface_entries[0].reading.as_str());
        surface_entries.sort_unstable_by(|left, right| {
            left.reading
                .cmp(&right.reading)
                .then(left.left_id.cmp(&right.left_id))
                .then(left.right_id.cmp(&right.right_id))
                .then(left.word_cost.cmp(&right.word_cost))
        });
        surface_entries.dedup_by(|left, right| {
            left.reading == right.reading
                && left.left_id == right.left_id
                && left.right_id == right.right_id
        });
        surface_entries.sort_unstable_by(|left, right| {
            left.word_cost
                .cmp(&right.word_cost)
                .then(left.reading.cmp(&right.reading))
                .then(left.left_id.cmp(&right.left_id))
                .then(left.right_id.cmp(&right.right_id))
        });
        surface_entries.truncate(MAXIMUM_RANKED_ENTRIES_PER_SURFACE);
    }
}

fn trim_surface_states(states: &mut HashMap<u16, usize>, arena: &[SurfacePath]) {
    if states.len() <= MAXIMUM_VITERBI_STATES_PER_POSITION {
        return;
    }
    let mut ranked = states
        .iter()
        .map(|(&right_id, &index)| (arena[index].cost, right_id, index))
        .collect::<Vec<_>>();
    ranked.sort_unstable();
    ranked.truncate(MAXIMUM_VITERBI_STATES_PER_POSITION);
    states.clear();
    states.extend(
        ranked
            .into_iter()
            .map(|(_, right_id, index)| (right_id, index)),
    );
}

impl ConnectionMatrix {
    fn load(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if bytes.len() < 8 || &bytes[..4] != b"UCN2" {
            return Err(format!(
                "{} is not a UCN2 connection matrix",
                path.display()
            ));
        }
        let size = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
        let offsets_start = 8;
        let modes_start = offsets_start + (size + 1) * 4;
        let entries_start = modes_start + size * 2;
        if entries_start > bytes.len() {
            return Err(format!("{} has a truncated UCN2 header", path.display()));
        }
        let matrix = Self {
            bytes,
            size,
            offsets_start,
            modes_start,
            entries_start,
        };
        let entry_count = matrix.offset(size)?;
        if entries_start.saturating_add(entry_count.saturating_mul(4)) > matrix.bytes.len() {
            return Err(format!("{} has truncated UCN2 entries", path.display()));
        }
        Ok(matrix)
    }

    fn cost(&self, right_id: u16, left_id: u16) -> i32 {
        let right = usize::from(right_id);
        let left = usize::from(left_id);
        if right >= self.size || left >= self.size {
            return INVALID_CONNECTION_COST;
        }
        let Some(mut low) = self.offset(right).ok() else {
            return INVALID_CONNECTION_COST;
        };
        let Some(mut high) = self.offset(right + 1).ok() else {
            return INVALID_CONNECTION_COST;
        };
        while low < high {
            let middle = low + (high - low) / 2;
            let entry_offset = self.entries_start + middle * 4;
            let entry_left = usize::from(u16::from_le_bytes([
                self.bytes[entry_offset],
                self.bytes[entry_offset + 1],
            ]));
            match entry_left.cmp(&left) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => {
                    return i32::from(u16::from_le_bytes([
                        self.bytes[entry_offset + 2],
                        self.bytes[entry_offset + 3],
                    ]));
                }
            }
        }
        let mode_offset = self.modes_start + right * 2;
        i32::from(u16::from_le_bytes([
            self.bytes[mode_offset],
            self.bytes[mode_offset + 1],
        ]))
    }

    fn offset(&self, row: usize) -> Result<usize, String> {
        let offset = self.offsets_start + row * 4;
        let bytes = self
            .bytes
            .get(offset..offset + 4)
            .ok_or_else(|| "connection matrix offset is truncated".to_owned())?;
        Ok(u32::from_le_bytes(bytes.try_into().expect("four bytes")) as usize)
    }

    #[cfg(test)]
    fn zeroed(size: u16) -> Self {
        let encoded_size = size;
        let size = usize::from(encoded_size);
        let offsets_start = 8;
        let modes_start = offsets_start + (size + 1) * 4;
        let entries_start = modes_start + size * 2;
        let mut bytes = vec![0_u8; entries_start];
        bytes[..4].copy_from_slice(b"UCN2");
        bytes[4..6].copy_from_slice(&encoded_size.to_le_bytes());
        Self {
            bytes,
            size,
            offsets_start,
            modes_start,
            entries_start,
        }
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
    use super::{
        ConnectionMatrix, RankedSurface, RankedSurfaceEntry, SurfaceReadingIndex,
        SurfaceViterbiIndex, compact_ranked_entries, hiragana_to_katakana,
    };
    use std::collections::HashMap;

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

    #[test]
    fn viterbi_annotation_rejects_a_greedy_cross_boundary_reading() {
        let mut entries = HashMap::from([
            (
                "曲が".into(),
                RankedSurface {
                    entries: vec![RankedSurfaceEntry {
                        reading: "まが".into(),
                        left_id: 1,
                        right_id: 1,
                        word_cost: 11_214,
                    }],
                    unambiguous: true,
                },
            ),
            (
                "曲".into(),
                RankedSurface {
                    entries: vec![RankedSurfaceEntry {
                        reading: "きょく".into(),
                        left_id: 1,
                        right_id: 1,
                        word_cost: 2_926,
                    }],
                    unambiguous: true,
                },
            ),
            (
                "が".into(),
                RankedSurface {
                    entries: vec![RankedSurfaceEntry {
                        reading: "が".into(),
                        left_id: 1,
                        right_id: 1,
                        word_cost: 1_000,
                    }],
                    unambiguous: true,
                },
            ),
        ]);
        compact_ranked_entries(&mut entries);
        let index = SurfaceViterbiIndex {
            entries,
            connection: ConnectionMatrix::zeroed(2_000),
        };
        assert_eq!(index.longest_reading("曲が").as_deref(), Some("まが"));
        assert_eq!(index.viterbi_reading("曲が").as_deref(), Some("きょくが"));
        assert!(index.consensus_annotation("曲が").is_none());
    }
}
