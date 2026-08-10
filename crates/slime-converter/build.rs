//! Compiles the bundled TSV dictionary into a zero-copy binary form:
//!
//! - `mozc-readings.fst`: FST mapping each reading to its byte offset in the
//!   entries blob (readings are unique keys, byte-sorted).
//! - `mozc-entries.bin`: 16-byte header (magic, entry count, max reading
//!   bytes), then per-reading blocks: varint entry count followed by entries
//!   of (varint surface offset, varint surface length, u16 left ID, u16 right
//!   ID, u16 word cost), sorted by cost.
//! - `mozc-surfaces.bin`: deduplicated concatenated UTF-8 surfaces.
//! - `mozc-reverse.fst` / `mozc-reverse.bin`: exact surface-to-reading index
//!   used for explicit reconversion and bounded document-phrase evidence.
//!
//! Parsing 44 MB of TSV at every process start took ~390 ms and duplicated
//! every string on the heap; the compiled form loads by pointer cast.

use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::Path;

struct Entry {
    surface: String,
    left_id: u16,
    right_id: u16,
    word_cost: u16,
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("manifest dir");
    let out_dir = env::var("OUT_DIR").expect("out dir");
    let tsv_path = Path::new(&manifest_dir).join("data/mozc-basic.tsv");
    println!("cargo::rerun-if-changed={}", tsv_path.display());

    let by_reading = read_entries_by_reading(&tsv_path);
    write_reverse_dictionary(&by_reading, Path::new(&out_dir));
    write_compact_dictionary(by_reading, Path::new(&out_dir));
}

fn write_reverse_dictionary(by_reading: &BTreeMap<String, Vec<Entry>>, out: &Path) {
    let productive_single_character_suffixes = by_reading
        .values()
        .flatten()
        .filter(|entry| {
            entry.surface.chars().count() == 1
                && (MOZC_PRODUCTIVE_NOUN_SUFFIX_ID_START..=MOZC_PRODUCTIVE_NOUN_SUFFIX_ID_END)
                    .contains(&entry.right_id)
        })
        .filter_map(|entry| entry.surface.chars().next())
        .collect::<HashSet<_>>();
    let general_noun_single_character_suffixes = general_noun_single_character_surfaces(by_reading);
    let mut by_surface = BTreeMap::<&str, Vec<(&str, u16)>>::new();
    for (reading, entries) in by_reading {
        for entry in entries {
            let context_phrase_entry = is_context_phrase_entry(
                entry,
                &productive_single_character_suffixes,
                &general_noun_single_character_suffixes,
            );
            if entry.surface == *reading
                || (entry.word_cost > MAX_RECONVERSION_WORD_COST && !context_phrase_entry)
            {
                continue;
            }
            by_surface
                .entry(&entry.surface)
                .or_default()
                .push((reading, entry.word_cost));
        }
    }

    let mut blocks = vec![0_u8; REVERSE_HEADER_BYTES];
    let mut reading_pool = Vec::<u8>::new();
    let mut reading_offsets = HashMap::<&str, (usize, usize)>::new();
    let mut fst_builder = fst::raw::Builder::memory();
    for (surface, mut readings) in by_surface {
        readings.sort_unstable_by(|left, right| (left.1, left.0).cmp(&(right.1, right.0)));
        let mut seen = HashSet::with_capacity(readings.len());
        readings.retain(|(reading, _)| seen.insert(*reading));
        let block_offset = blocks.len() as u64;
        push_varint(&mut blocks, readings.len() as u64);
        for (reading, word_cost) in readings {
            let (offset, length) = *reading_offsets.entry(reading).or_insert_with(|| {
                let offset = reading_pool.len();
                reading_pool.extend_from_slice(reading.as_bytes());
                (offset, reading.len())
            });
            push_varint(&mut blocks, offset as u64);
            push_varint(&mut blocks, length as u64);
            blocks.extend_from_slice(&word_cost.to_le_bytes());
        }
        fst_builder
            .insert(surface.as_bytes(), block_offset)
            .expect("insert sorted surface into reverse FST");
    }
    blocks[0..4].copy_from_slice(b"RDE1");
    fs::write(
        out.join("mozc-reverse.fst"),
        fst_builder.into_inner().expect("finish reverse FST"),
    )
    .expect("write reverse FST");
    fs::write(out.join("mozc-reverse.bin"), blocks).expect("write reverse blocks");
    fs::write(out.join("mozc-reverse-readings.bin"), reading_pool)
        .expect("write reverse reading pool");
}

fn general_noun_single_character_surfaces(
    by_reading: &BTreeMap<String, Vec<Entry>>,
) -> HashSet<char> {
    by_reading
        .values()
        .flatten()
        .filter(|entry| {
            entry.surface.chars().count() == 1
                && entry.left_id == MOZC_GENERAL_NOUN_POS_ID
                && entry.right_id == MOZC_GENERAL_NOUN_POS_ID
        })
        .filter_map(|entry| entry.surface.chars().next())
        .collect()
}

fn is_context_phrase_entry(
    entry: &Entry,
    productive_single_character_suffixes: &HashSet<char>,
    general_noun_single_character_suffixes: &HashSet<char>,
) -> bool {
    (entry.word_cost <= MAX_CONTEXT_PHRASE_WORD_COST
        && (MOZC_PRODUCTIVE_NOUN_SUFFIX_ID_START..=MOZC_PRODUCTIVE_NOUN_SUFFIX_ID_END)
            .contains(&entry.right_id))
        || (entry.word_cost <= MAX_IDEOGRAPHIC_SUFFIX_CONTEXT_PHRASE_WORD_COST
            && entry.left_id == MOZC_VERBAL_NOUN_POS_ID
            && entry.right_id == MOZC_GENERAL_NOUN_SUFFIX_POS_ID
            && is_three_character_ideographic_compound(&entry.surface))
        || (entry.word_cost <= MAX_KATAKANA_CONTEXT_PHRASE_WORD_COST
            && is_bounded_katakana_stem_compound(
                &entry.surface,
                productive_single_character_suffixes,
            ))
        || (entry.word_cost <= MAX_KATAKANA_GENERAL_NOUN_CONTEXT_PHRASE_WORD_COST
            && entry.left_id == MOZC_GENERAL_NOUN_POS_ID
            && entry.right_id == MOZC_GENERAL_NOUN_POS_ID
            && is_bounded_katakana_stem_compound(
                &entry.surface,
                general_noun_single_character_suffixes,
            ))
        || (entry.word_cost <= MAX_KATAKANA_IDEOGRAPHIC_TAIL_CONTEXT_PHRASE_WORD_COST
            && entry.left_id == MOZC_GENERAL_NOUN_POS_ID
            && entry.right_id == MOZC_GENERAL_NOUN_SUFFIX_POS_ID
            && is_bounded_katakana_ideographic_tail_compound(
                &entry.surface,
                productive_single_character_suffixes,
            ))
        || (entry.word_cost <= MAX_GENERAL_VERBAL_NOUN_COMPOUND_WORD_COST
            && entry.left_id == MOZC_GENERAL_NOUN_POS_ID
            && entry.right_id == MOZC_VERBAL_NOUN_POS_ID
            && is_bounded_ideographic_compound(&entry.surface))
        || (entry.word_cost <= MAX_GENERAL_NOUN_CONTEXT_PHRASE_WORD_COST
            && entry.left_id == MOZC_GENERAL_NOUN_POS_ID
            && entry.right_id == MOZC_GENERAL_NOUN_POS_ID
            && is_bounded_ideographic_compound(&entry.surface))
        || (entry.word_cost <= MAX_SIBLING_CONTEXT_PHRASE_WORD_COST
            && is_bounded_sibling_compound(&entry.surface))
        || (entry.word_cost <= MAX_COORDINATION_CONTEXT_PHRASE_WORD_COST
            && entry.left_id == MOZC_GENERAL_NOUN_POS_ID
            && entry.right_id == MOZC_GENERAL_NOUN_POS_ID
            && is_bounded_coordination_phrase(&entry.surface))
        || (entry.word_cost <= MAX_GENITIVE_CONTEXT_PHRASE_WORD_COST
            && ((entry.left_id == MOZC_GENERAL_NOUN_POS_ID
                && entry.right_id == MOZC_GENERAL_NOUN_POS_ID)
                || (entry.left_id == MOZC_VERBAL_NOUN_POS_ID
                    && entry.right_id == MOZC_VERBAL_NOUN_POS_ID))
            && is_bounded_genitive_phrase(&entry.surface))
}

fn read_entries_by_reading(tsv_path: &Path) -> BTreeMap<String, Vec<Entry>> {
    let tsv = fs::read_to_string(tsv_path).expect("read bundled dictionary TSV");
    let mut by_reading: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for line in tsv.lines() {
        let mut columns = line.split('\t');
        let reading = columns.next().expect("reading column");
        let surface = columns.next().expect("surface column");
        let left_id = columns
            .next()
            .expect("left ID column")
            .parse()
            .expect("numeric left ID");
        let right_id = columns
            .next()
            .expect("right ID column")
            .parse()
            .expect("numeric right ID");
        let word_cost: u16 = columns
            .next()
            .expect("cost column")
            .parse()
            .expect("numeric cost");
        assert!(columns.next().is_none(), "bundled dictionary column count");
        by_reading
            .entry(reading.to_owned())
            .or_default()
            .push(Entry {
                surface: surface.to_owned(),
                left_id,
                right_id,
                word_cost,
            });
    }

    by_reading
}

fn write_compact_dictionary(by_reading: BTreeMap<String, Vec<Entry>>, out: &Path) {
    let mut surfaces = Vec::<u8>::new();
    let mut surface_offsets = HashMap::<String, usize>::new();
    let mut entries = vec![0_u8; ENTRIES_HEADER_BYTES];
    let mut fst_builder = fst::raw::Builder::memory();
    let mut entry_count = 0_u64;
    let mut max_reading_bytes = 0_usize;

    for (reading, mut reading_entries) in by_reading {
        reading_entries.sort_by(|left, right| {
            (left.word_cost, &left.surface, left.left_id, left.right_id).cmp(&(
                right.word_cost,
                &right.surface,
                right.left_id,
                right.right_id,
            ))
        });
        max_reading_bytes = max_reading_bytes.max(reading.len());

        let block_offset = entries.len() as u64;
        push_varint(&mut entries, reading_entries.len() as u64);
        for entry in &reading_entries {
            let offset = *surface_offsets
                .entry(entry.surface.clone())
                .or_insert_with(|| {
                    let offset = surfaces.len();
                    surfaces.extend_from_slice(entry.surface.as_bytes());
                    offset
                });
            push_varint(&mut entries, offset as u64);
            push_varint(&mut entries, entry.surface.len() as u64);
            entries.extend_from_slice(&entry.left_id.to_le_bytes());
            entries.extend_from_slice(&entry.right_id.to_le_bytes());
            entries.extend_from_slice(&entry.word_cost.to_le_bytes());
            entry_count += 1;
        }
        fst_builder
            .insert(reading.as_bytes(), block_offset)
            .expect("insert sorted reading into FST");
    }

    entries[0..4].copy_from_slice(b"UDE1");
    entries[4..8].copy_from_slice(
        &u32::try_from(entry_count)
            .expect("entry count fits u32")
            .to_le_bytes(),
    );
    entries[8..12].copy_from_slice(
        &u32::try_from(max_reading_bytes)
            .expect("reading length fits u32")
            .to_le_bytes(),
    );

    fs::write(
        out.join("mozc-readings.fst"),
        fst_builder.into_inner().expect("finish FST"),
    )
    .expect("write FST");
    fs::write(out.join("mozc-entries.bin"), entries).expect("write entries");
    fs::write(out.join("mozc-surfaces.bin"), surfaces).expect("write surfaces");
}

const ENTRIES_HEADER_BYTES: usize = 16;
const REVERSE_HEADER_BYTES: usize = 8;
// Keep explicit reconversion broad enough for ordinary writing without
// embedding low-confidence proper-name and spelling variants that dominate
// the full Mozc source size. User and installed dictionaries are always
// indexed at runtime regardless of this bundled cutoff.
const MAX_RECONVERSION_WORD_COST: u16 = 6_500;
// Preserve one additional bounded cost band for dictionary-backed compound
// evidence. Mozc IDs 1936..=1998 are productive noun suffixes. A narrower
// 200-cost extension admits only 2..=8-character katakana stems followed by a
// single ideograph which independently belongs to that productive suffix
// class. This covers forms such as foreign terms plus 体/機 without indexing
// the broad person-name, region-name, and long proper-name classes.
const MAX_CONTEXT_PHRASE_WORD_COST: u16 = 7_500;
// Three-character verbal-noun compounds with Mozc's general noun suffix remain
// useful phrase evidence at slightly lower frequency. Keep this separate from
// the broad suffix band so other suffix classes and proper names stay out.
const MAX_IDEOGRAPHIC_SUFFIX_CONTEXT_PHRASE_WORD_COST: u16 = 7_550;
const MAX_KATAKANA_CONTEXT_PHRASE_WORD_COST: u16 = 7_700;
// Some technical compounds end in an ordinary noun rather than Mozc's noun-
// suffix class (for example リステリア菌). Require both the complete phrase
// and its one-character tail to be general nouns, and use the tighter common-
// phrase cost band so proper names and weak spellings stay excluded.
const MAX_KATAKANA_GENERAL_NOUN_CONTEXT_PHRASE_WORD_COST: u16 = 7_500;
// Mixed compounds such as a foreign-word stem plus a short ideographic noun
// can carry the same productive final suffix while costing more as a complete
// phrase. Bound both scripts and the ideographic tail before widening the
// context-only band; this does not add conversion entries or alter word costs.
const MAX_KATAKANA_IDEOGRAPHIC_TAIL_CONTEXT_PHRASE_WORD_COST: u16 = 8_400;
const MAX_GENERAL_VERBAL_NOUN_COMPOUND_WORD_COST: u16 = 7_500;
// Admit a narrow band of lower-frequency all-kanji common nouns as context
// evidence only. This keeps names and kana-mixed spellings out of the bundled
// reverse index and does not add conversion candidates or alter word costs.
const MAX_GENERAL_NOUN_CONTEXT_PHRASE_WORD_COST: u16 = 7_200;
const MAX_SIBLING_CONTEXT_PHRASE_WORD_COST: u16 = 7_500;
const MAX_COORDINATION_CONTEXT_PHRASE_WORD_COST: u16 = 7_500;
const MAX_GENITIVE_CONTEXT_PHRASE_WORD_COST: u16 = 8_000;
const MOZC_VERBAL_NOUN_POS_ID: u16 = 1_841;
const MOZC_GENERAL_NOUN_POS_ID: u16 = 1_851;
const MOZC_GENERAL_NOUN_SUFFIX_POS_ID: u16 = 1_949;
const MOZC_PRODUCTIVE_NOUN_SUFFIX_ID_START: u16 = 1_936;
const MOZC_PRODUCTIVE_NOUN_SUFFIX_ID_END: u16 = 1_998;

fn is_bounded_ideographic_compound(surface: &str) -> bool {
    let mut characters = surface.chars();
    let count = characters.clone().take(9).count();
    (3..=8).contains(&count) && characters.all(is_cjk_ideograph)
}

fn is_three_character_ideographic_compound(surface: &str) -> bool {
    let mut characters = surface.chars();
    characters.clone().count() == 3 && characters.all(is_cjk_ideograph)
}

fn is_bounded_sibling_compound(surface: &str) -> bool {
    let mut characters = surface.chars();
    let count = characters.clone().take(9).count();
    (2..=8).contains(&count)
        && characters.clone().all(is_cjk_ideograph)
        && characters
            .next_back()
            .is_some_and(|suffix| matches!(suffix, '兄' | '姉' | '弟' | '妹'))
}

fn is_bounded_coordination_phrase(surface: &str) -> bool {
    let characters = surface.chars().collect::<Vec<_>>();
    if !(3..=8).contains(&characters.len()) {
        return false;
    }
    let Some(connector) = characters
        .iter()
        .position(|character| matches!(character, 'や' | 'と'))
    else {
        return false;
    };
    connector > 0
        && connector + 1 < characters.len()
        && characters[..connector]
            .iter()
            .copied()
            .all(is_cjk_ideograph)
        && characters[connector + 1..]
            .iter()
            .copied()
            .all(is_cjk_ideograph)
}

fn is_bounded_genitive_phrase(surface: &str) -> bool {
    let characters = surface.chars().collect::<Vec<_>>();
    if !(3..=8).contains(&characters.len()) {
        return false;
    }
    let Some(particle) = characters.iter().position(|character| *character == 'の') else {
        return false;
    };
    particle > 0
        && particle + 1 < characters.len()
        && characters[..particle].iter().copied().all(is_cjk_ideograph)
        && characters[particle + 1..]
            .iter()
            .copied()
            .all(is_cjk_ideograph)
}

fn is_bounded_katakana_stem_compound(
    surface: &str,
    productive_single_character_suffixes: &HashSet<char>,
) -> bool {
    let mut characters = surface.chars().collect::<Vec<_>>();
    let Some(suffix) = characters.pop() else {
        return false;
    };
    if !is_cjk_ideograph(suffix)
        || !productive_single_character_suffixes.contains(&suffix)
        || !(2..=8).contains(&characters.len())
    {
        return false;
    }
    let mut letters = 0;
    for character in characters {
        if !matches!(character, '\u{30a0}'..='\u{30ff}') {
            return false;
        }
        if !matches!(character, 'ー' | '・') {
            letters += 1;
        }
    }
    letters >= 2
}

fn is_bounded_katakana_ideographic_tail_compound(
    surface: &str,
    productive_single_character_suffixes: &HashSet<char>,
) -> bool {
    let characters = surface.chars().collect::<Vec<_>>();
    if !(4..=9).contains(&characters.len())
        || !characters
            .last()
            .is_some_and(|suffix| productive_single_character_suffixes.contains(suffix))
    {
        return false;
    }
    let Some(ideographic_start) = characters
        .iter()
        .position(|character| is_cjk_ideograph(*character))
    else {
        return false;
    };
    let katakana_stem = &characters[..ideographic_start];
    let ideographic_tail = &characters[ideographic_start..];
    (2..=8).contains(&katakana_stem.len())
        && (2..=3).contains(&ideographic_tail.len())
        && katakana_stem
            .iter()
            .all(|character| matches!(character, '\u{30a0}'..='\u{30ff}'))
        && katakana_stem
            .iter()
            .filter(|character| !matches!(character, 'ー' | '・'))
            .count()
            >= 2
        && ideographic_tail.iter().copied().all(is_cjk_ideograph)
}

fn is_cjk_ideograph(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
    )
}

fn push_varint(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            output.push(byte);
            break;
        }
        output.push(byte | 0x80);
    }
}
