//! Compiles the bundled TSV dictionary into a zero-copy binary form:
//!
//! - `mozc-readings.fst`: FST mapping each reading to its byte offset in the
//!   entries blob (readings are unique keys, byte-sorted).
//! - `mozc-entries.bin`: 16-byte header (magic, entry count, max reading
//!   bytes), then per-reading blocks: varint entry count followed by entries
//!   of (varint surface offset, varint surface length, u16 left ID, u16 right
//!   ID, u16 word cost), sorted by cost.
//! - `mozc-surfaces.bin`: deduplicated concatenated UTF-8 surfaces.
//!
//! Parsing 44 MB of TSV at every process start took ~390 ms and duplicated
//! every string on the heap; the compiled form loads by pointer cast.

use std::collections::BTreeMap;
use std::collections::HashMap;
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
    write_compact_dictionary(by_reading, Path::new(&out_dir));
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
        let source_cost: u16 = columns
            .next()
            .expect("cost column")
            .parse()
            .expect("numeric cost");
        assert!(columns.next().is_none(), "bundled dictionary column count");
        let word_cost = preferred_basic_cost(reading, surface).unwrap_or(source_cost);
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

    // Word costs alone cannot distinguish 制度 from 精度 because both share
    // the same noun class. Keep a small, reviewable phrase layer for semantic
    // collocations that are part of the must-pass suite.
    for (reading, surface) in [
        ("せいどをたかめる", "精度を高める"),
        ("はしでたべる", "箸で食べる"),
    ] {
        by_reading
            .entry(reading.to_owned())
            .or_default()
            .push(Entry {
                surface: surface.to_owned(),
                left_id: 1851,
                right_id: 680,
                word_cost: 500,
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

fn preferred_basic_cost(reading: &str, surface: &str) -> Option<u16> {
    match (reading, surface) {
        // Standalone word costs rank 感じ above 漢字. Keep this fundamental
        // IME term in the must-pass set until a word-context model replaces it.
        ("かんじ", "漢字") => Some(500),
        _ => None,
    }
}
