//! Zero-copy access to the bundled dictionary compiled by `build.rs`.
//!
//! The reading index is an FST whose output is the byte offset of the
//! reading's entry block; walking the FST along an input suffix yields every
//! dictionary reading that prefixes it in a single O(length) traversal,
//! replacing one binary search per prefix length.

use std::sync::OnceLock;

static READINGS_FST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mozc-readings.fst"));
static ENTRIES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mozc-entries.bin"));
static SURFACES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mozc-surfaces.bin"));
static REVERSE_FST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mozc-reverse.fst"));
static REVERSE_BLOCKS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mozc-reverse.bin"));
static REVERSE_READINGS: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/mozc-reverse-readings.bin"));

const ENTRIES_HEADER_BYTES: usize = 16;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CompactEntry {
    pub surface: &'static str,
    pub left_id: u16,
    pub right_id: u16,
    pub word_cost: i32,
}

pub(crate) struct CompactDictionary {
    fst: fst::raw::Fst<&'static [u8]>,
    reverse_fst: fst::raw::Fst<&'static [u8]>,
    entry_count: usize,
    max_reading_bytes: usize,
}

impl std::fmt::Debug for CompactDictionary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompactDictionary")
            .field("entry_count", &self.entry_count)
            .field("max_reading_bytes", &self.max_reading_bytes)
            .finish_non_exhaustive()
    }
}

impl CompactDictionary {
    pub(crate) fn bundled() -> &'static Self {
        static INSTANCE: OnceLock<CompactDictionary> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            assert_eq!(&ENTRIES[0..4], b"UDE1", "bundled dictionary magic");
            assert_eq!(&REVERSE_BLOCKS[0..4], b"RDE1", "reverse dictionary magic");
            let entry_count =
                u32::from_le_bytes(ENTRIES[4..8].try_into().expect("header slice")) as usize;
            let max_reading_bytes =
                u32::from_le_bytes(ENTRIES[8..12].try_into().expect("header slice")) as usize;
            Self {
                fst: fst::raw::Fst::new(READINGS_FST).expect("valid bundled reading FST"),
                reverse_fst: fst::raw::Fst::new(REVERSE_FST).expect("valid bundled reverse FST"),
                entry_count,
                max_reading_bytes,
            }
        })
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entry_count
    }

    /// Calls `callback` for every dictionary entry whose reading equals
    /// `reading`.
    pub(crate) fn for_each_exact(&self, reading: &str, mut callback: impl FnMut(CompactEntry)) {
        if let Some(output) = self.fst.get(reading.as_bytes()) {
            for_each_entry_at(output.value(), &mut callback);
        }
    }

    /// Calls `callback(prefix_bytes, entry)` for every dictionary entry whose
    /// reading is a prefix of `suffix`, in ascending prefix length.
    pub(crate) fn for_each_prefix(
        &self,
        suffix: &str,
        mut callback: impl FnMut(usize, CompactEntry),
    ) {
        let bytes = suffix.as_bytes();
        let mut node = self.fst.root();
        let mut output = fst::raw::Output::zero();
        for (index, &byte) in bytes.iter().enumerate() {
            let Some(transition_index) = node.find_input(byte) else {
                return;
            };
            let transition = node.transition(transition_index);
            output = output.cat(transition.out);
            node = self.fst.node(transition.addr);
            if node.is_final() {
                let block = output.cat(node.final_output()).value();
                for_each_entry_at(block, &mut |entry| callback(index + 1, entry));
            }
        }
    }

    pub(crate) fn readings_for_surface(&self, surface: &str) -> Vec<(String, i32)> {
        let Some(output) = self.reverse_fst.get(surface.as_bytes()) else {
            return Vec::new();
        };
        let mut cursor = usize::try_from(output.value()).expect("reverse block offset");
        let count = read_reverse_varint(&mut cursor);
        let mut readings = Vec::with_capacity(usize::try_from(count).expect("reading count"));
        for _ in 0..count {
            let offset = usize::try_from(read_reverse_varint(&mut cursor)).expect("reading offset");
            let length = usize::try_from(read_reverse_varint(&mut cursor)).expect("reading length");
            let reading = std::str::from_utf8(&REVERSE_READINGS[offset..offset + length])
                .expect("valid reverse reading UTF-8")
                .to_owned();
            let cost = u16::from_le_bytes([REVERSE_BLOCKS[cursor], REVERSE_BLOCKS[cursor + 1]]);
            cursor += 2;
            readings.push((reading, i32::from(cost)));
        }
        readings
    }

    /// Calls `callback` for dictionary entries whose surface exactly matches
    /// `surface`, without allocating the reverse readings.
    pub(crate) fn for_each_surface_entry(
        &self,
        surface: &str,
        mut callback: impl FnMut(CompactEntry),
    ) {
        let Some(output) = self.reverse_fst.get(surface.as_bytes()) else {
            return;
        };
        let mut cursor = usize::try_from(output.value()).expect("reverse block offset");
        let count = read_reverse_varint(&mut cursor);
        for _ in 0..count {
            let offset = usize::try_from(read_reverse_varint(&mut cursor)).expect("reading offset");
            let length = usize::try_from(read_reverse_varint(&mut cursor)).expect("reading length");
            let reading = std::str::from_utf8(&REVERSE_READINGS[offset..offset + length])
                .expect("valid reverse reading UTF-8");
            cursor += 2;
            self.for_each_exact(reading, |entry| {
                if entry.surface == surface {
                    callback(entry);
                }
            });
        }
    }

    /// Returns the lowest reverse-index word cost whose reading ends with
    /// `suffix` for the split `prefix + surface` surface.
    pub(crate) fn joined_surface_reading_suffix_cost(
        &self,
        prefix: &str,
        surface: &str,
        suffix: &str,
    ) -> Option<i32> {
        let mut node = self.reverse_fst.root();
        let mut output = fst::raw::Output::zero();
        for byte in prefix.bytes().chain(surface.bytes()) {
            let transition_index = node.find_input(byte)?;
            let transition = node.transition(transition_index);
            output = output.cat(transition.out);
            node = self.reverse_fst.node(transition.addr);
        }
        if !node.is_final() {
            return None;
        }
        let block = output.cat(node.final_output()).value();
        let mut cursor = usize::try_from(block).expect("reverse block offset");
        let count = read_reverse_varint(&mut cursor);
        let mut best_cost = None;
        for _ in 0..count {
            let offset = usize::try_from(read_reverse_varint(&mut cursor)).expect("reading offset");
            let length = usize::try_from(read_reverse_varint(&mut cursor)).expect("reading length");
            let reading = std::str::from_utf8(&REVERSE_READINGS[offset..offset + length])
                .expect("valid reverse reading UTF-8");
            let cost = u16::from_le_bytes([REVERSE_BLOCKS[cursor], REVERSE_BLOCKS[cursor + 1]]);
            cursor += 2;
            if reading.ends_with(suffix) {
                best_cost =
                    Some(best_cost.map_or(i32::from(cost), |best: i32| best.min(i32::from(cost))));
            }
        }
        best_cost
    }
}

fn read_reverse_varint(cursor: &mut usize) -> u64 {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    loop {
        let byte = REVERSE_BLOCKS[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

fn for_each_entry_at(block_offset: u64, callback: &mut impl FnMut(CompactEntry)) {
    let mut cursor = usize::try_from(block_offset).expect("block offset fits usize");
    debug_assert!(cursor >= ENTRIES_HEADER_BYTES);
    let count = read_varint(&mut cursor);
    for _ in 0..count {
        let surface_offset = usize::try_from(read_varint(&mut cursor)).expect("surface offset");
        let surface_length = usize::try_from(read_varint(&mut cursor)).expect("surface length");
        let left_id = u16::from_le_bytes([ENTRIES[cursor], ENTRIES[cursor + 1]]);
        let right_id = u16::from_le_bytes([ENTRIES[cursor + 2], ENTRIES[cursor + 3]]);
        let word_cost = u16::from_le_bytes([ENTRIES[cursor + 4], ENTRIES[cursor + 5]]);
        cursor += 6;
        let surface_bytes = &SURFACES[surface_offset..surface_offset + surface_length];
        callback(CompactEntry {
            surface: std::str::from_utf8(surface_bytes).expect("valid UTF-8 surface"),
            left_id,
            right_id,
            word_cost: i32::from(word_cost),
        });
    }
}

fn read_varint(cursor: &mut usize) -> u64 {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    loop {
        let byte = ENTRIES[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::CompactDictionary;

    #[test]
    fn exact_lookup_returns_known_entries() {
        let dictionary = CompactDictionary::bundled();
        let mut surfaces = Vec::new();
        dictionary.for_each_exact("にほん", |entry| surfaces.push(entry.surface));
        assert!(surfaces.contains(&"日本"), "surfaces: {surfaces:?}");
    }

    #[test]
    fn prefix_walk_yields_every_prefix_reading() {
        let dictionary = CompactDictionary::bundled();
        let mut prefixes = Vec::new();
        dictionary.for_each_prefix("にほんご", |length, _| {
            if !prefixes.contains(&length) {
                prefixes.push(length);
            }
        });
        // に (3 bytes), にほ, にほん, にほんご are all dictionary readings.
        assert_eq!(prefixes, vec![3, 6, 9, 12]);
    }

    #[test]
    fn bundled_dictionary_is_large_and_bounded() {
        let dictionary = CompactDictionary::bundled();
        assert!(dictionary.entry_count() > 1_000_000);
        assert!(dictionary.max_reading_bytes >= 24);
    }

    #[test]
    fn reverse_index_returns_ranked_readings() {
        let readings = CompactDictionary::bundled().readings_for_surface("日本");
        assert!(readings.iter().any(|(reading, _)| reading == "にほん"));
    }

    #[test]
    fn reverse_index_matches_a_reading_suffix_without_allocating() {
        let dictionary = CompactDictionary::bundled();
        assert_eq!(
            dictionary.joined_surface_reading_suffix_cost("格納", "庫", "こ"),
            Some(5_996)
        );
        assert_eq!(
            dictionary.joined_surface_reading_suffix_cost("格納", "庫", "こう"),
            None
        );
        assert_eq!(
            dictionary.joined_surface_reading_suffix_cost("未知", "語", "ご"),
            None
        );
        assert_eq!(
            dictionary.joined_surface_reading_suffix_cost("ナスタアリーク", "体", "たい"),
            Some(7_602)
        );
        assert_eq!(
            dictionary.joined_surface_reading_suffix_cost("アイスクリーム", "屋", "や"),
            None,
            "the katakana compound cost band must remain bounded"
        );
        assert_eq!(
            dictionary.joined_surface_reading_suffix_cost("アイランドセンター", "駅", "えき"),
            None,
            "long proper-name stems must stay outside the context index"
        );
    }

    #[test]
    fn reverse_index_visits_matching_surface_entries_without_allocating() {
        let dictionary = CompactDictionary::bundled();
        let mut entries = Vec::new();
        dictionary.for_each_surface_entry("東海", |entry| {
            entries.push((entry.left_id, entry.right_id));
        });
        assert!(entries.contains(&(1924, 1924)), "entries: {entries:?}");
    }
}
