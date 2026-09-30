//! Generates a conservative phrase-promotion dictionary from public Anthy
//! conversion corpus rows.
//!
//! A phrase is emitted only when it appears in distinct source rows at least
//! twice and is the unique most frequent surface for its reading. Common case
//! particles are removed from matching reading/surface suffixes so evidence
//! such as `変換精度に` and `変換精度を` reinforces the same lexical phrase.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use slime_converter::{Dictionary, DictionaryEntry, DictionaryLayer};

#[allow(dead_code)]
mod private_generation;
use private_generation::{
    MAX_LINE_BYTES, MAX_TOKEN_CHARACTERS, normalize_phonetic_reading, read_private_input,
    sha256_hex, valid_token_field, validate_tsv_output, write_new_atomic,
};

const DEFAULT_MINIMUM_COUNT: u32 = 2;
const DEFAULT_MAXIMUM_ENTRIES: usize = 10_000;
const DEFAULT_WORD_COST: i32 = 5_000;
const MAXIMUM_ENTRIES: usize = 100_000;
const MINIMUM_READING_CHARACTERS: usize = 4;
const PARTICLES: &[char] = &['を', 'に', 'が', 'は', 'の', 'と', 'で', 'へ', 'も'];

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let options = Options::parse(arguments)?;
    validate_tsv_output(&options.output)?;
    let (counts, mut report) = load_counts(&options.inputs)?;
    let entries = select_entries(counts, &options, &mut report);
    let entries = retain_effective_promotions(entries, options.word_cost, &mut report);
    if entries.is_empty() {
        return Err("inputs produced no unambiguous repeated phrases".to_owned());
    }
    let output = serialize_entries(&entries, options.word_cost);
    report.input_files = options.inputs.len();
    report.output_entries = entries.len();
    report.output_bytes = output.len();
    report.output_sha256 = sha256_hex(output.as_bytes());
    write_new_atomic(
        &options.output,
        output.as_bytes(),
        "slime-phrase-dictionary",
    )?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| "cannot serialize aggregate report".to_owned())?
        );
    } else {
        println!(
            "entries={}\tobservations={}\tbytes={}\tsha256={}",
            report.output_entries,
            report.accepted_observations,
            report.output_bytes,
            report.output_sha256
        );
    }
    Ok(())
}

const fn usage() -> &'static str {
    "usage: slime-phrase-dictionary --input ANTHY.txt [--input ANTHY.txt ...] \
     --output OUTPUT.tsv [--min-count N] [--max-entries N] [--word-cost N] [--json]"
}

#[derive(Debug)]
struct Options {
    inputs: Vec<PathBuf>,
    output: PathBuf,
    minimum_count: u32,
    maximum_entries: usize,
    word_cost: i32,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut inputs = Vec::new();
        let mut output = None;
        let mut minimum_count = DEFAULT_MINIMUM_COUNT;
        let mut maximum_entries = DEFAULT_MAXIMUM_ENTRIES;
        let mut word_cost = DEFAULT_WORD_COST;
        let mut json = false;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--input" => inputs.push(PathBuf::from(next_value(&mut arguments, "--input")?)),
                "--output" if output.is_none() => {
                    output = Some(PathBuf::from(next_value(&mut arguments, "--output")?));
                }
                "--output" => return Err("--output is duplicated".to_owned()),
                "--min-count" => {
                    minimum_count = parse_positive(&mut arguments, "--min-count")?;
                }
                "--max-entries" => {
                    maximum_entries =
                        usize::try_from(parse_positive(&mut arguments, "--max-entries")?)
                            .map_err(|_| "--max-entries is too large".to_owned())?;
                    if maximum_entries > MAXIMUM_ENTRIES {
                        return Err(format!("--max-entries cannot exceed {MAXIMUM_ENTRIES}"));
                    }
                }
                "--word-cost" => {
                    word_cost = next_value(&mut arguments, "--word-cost")?
                        .parse::<i32>()
                        .ok()
                        .filter(|cost| (0..=100_000).contains(cost))
                        .ok_or_else(|| "--word-cost must be between 0 and 100000".to_owned())?;
                }
                "--json" if !json => json = true,
                "--json" => return Err("--json is duplicated".to_owned()),
                "--help" | "-h" => return Err(usage().to_owned()),
                _ => return Err(format!("unknown option\n{}", usage())),
            }
        }
        if inputs.is_empty() || output.is_none() {
            return Err(usage().to_owned());
        }
        Ok(Self {
            inputs,
            output: output.expect("checked above"),
            minimum_count,
            maximum_entries,
            word_cost,
            json,
        })
    }
}

fn next_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_positive(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<u32, String> {
    next_value(arguments, option)?
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{option} requires a positive integer"))
}

type PhraseKey = (String, String);

#[derive(Default, Serialize)]
struct Report {
    input_files: usize,
    input_bytes: u64,
    lines: usize,
    accepted_lines: usize,
    malformed_lines: usize,
    rejected_segments: usize,
    accepted_observations: usize,
    unique_phrases: usize,
    below_minimum_count: usize,
    ambiguous_readings: usize,
    baseline_already_top: usize,
    ineffective_promotions: usize,
    output_entries: usize,
    output_bytes: usize,
    output_sha256: String,
}

fn load_counts(paths: &[PathBuf]) -> Result<(BTreeMap<PhraseKey, u32>, Report), String> {
    let mut counts = BTreeMap::new();
    let mut report = Report::default();
    for path in paths {
        let source = read_private_input(path, "Anthy corpus", &mut report.input_bytes)?;
        for (index, line) in source.lines().enumerate() {
            report.lines = report.lines.saturating_add(1);
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Ok(pairs) = parse_anthy_segments(line, index + 1) else {
                report.malformed_lines = report.malformed_lines.saturating_add(1);
                continue;
            };
            report.accepted_lines = report.accepted_lines.saturating_add(1);
            let mut line_phrases = HashSet::new();
            for (reading, surface) in pairs {
                if let Some(phrase) = normalized_phrase(&reading, &surface) {
                    line_phrases.insert(phrase);
                } else {
                    report.rejected_segments = report.rejected_segments.saturating_add(1);
                }
            }
            report.accepted_observations = report
                .accepted_observations
                .saturating_add(line_phrases.len());
            for phrase in line_phrases {
                let count = counts.entry(phrase).or_insert(0_u32);
                *count = count.saturating_add(1);
            }
        }
    }
    report.unique_phrases = counts.len();
    Ok((counts, report))
}

fn parse_anthy_segments(line: &str, line_number: usize) -> Result<Vec<(String, String)>, String> {
    if line.len() > MAX_LINE_BYTES {
        return Err(format!("Anthy line {line_number} exceeds the byte limit"));
    }
    let (reading, surface) = line
        .split_once("| |")
        .ok_or_else(|| format!("Anthy line {line_number} is missing the '| |' separator"))?;
    let readings: Vec<_> = reading
        .split('|')
        .filter(|value| !value.is_empty())
        .collect();
    let surfaces: Vec<_> = surface
        .split('|')
        .filter(|value| !value.is_empty())
        .collect();
    if readings.len() != surfaces.len() || readings.is_empty() {
        return Err(format!("Anthy line {line_number} has unaligned segments"));
    }
    Ok(readings
        .into_iter()
        .zip(surfaces)
        .map(|(reading, surface)| (reading.to_owned(), surface.to_owned()))
        .collect())
}

fn normalized_phrase(reading: &str, surface: &str) -> Option<PhraseKey> {
    let mut reading = normalize_phonetic_reading(reading)?;
    let mut surface = surface.to_owned();
    if let (Some(reading_suffix), Some(surface_suffix)) =
        (reading.chars().last(), surface.chars().last())
        && reading_suffix == surface_suffix
        && PARTICLES.contains(&reading_suffix)
    {
        reading.pop();
        surface.pop();
    }
    if reading.chars().count() < MINIMUM_READING_CHARACTERS
        || surface.chars().count() > MAX_TOKEN_CHARACTERS
        || !valid_token_field(&surface)
        || !surface.chars().all(is_compound_character)
        || surface
            .chars()
            .filter(|character| is_kanji(*character))
            .count()
            < 2
    {
        return None;
    }
    Some((reading, surface))
}

fn is_compound_character(character: char) -> bool {
    is_kanji(character) || matches!(character, 'ァ'..='ヶ' | 'ヽ' | 'ヾ' | 'ー' | '・')
}

fn is_kanji(character: char) -> bool {
    matches!(character, '\u{4e00}'..='\u{9fff}' | '々' | '〆')
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OutputEntry {
    reading: String,
    surface: String,
    count: u32,
}

fn select_entries(
    counts: BTreeMap<PhraseKey, u32>,
    options: &Options,
    report: &mut Report,
) -> Vec<OutputEntry> {
    let mut by_reading = HashMap::<String, Vec<(String, u32)>>::new();
    for ((reading, surface), count) in counts {
        if count < options.minimum_count {
            report.below_minimum_count = report.below_minimum_count.saturating_add(1);
            continue;
        }
        by_reading
            .entry(reading)
            .or_default()
            .push((surface, count));
    }
    let mut entries = Vec::new();
    for (reading, mut surfaces) in by_reading {
        surfaces.sort_unstable_by(|left, right| {
            right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
        });
        if surfaces
            .get(1)
            .is_some_and(|second| second.1 >= surfaces[0].1)
        {
            report.ambiguous_readings = report.ambiguous_readings.saturating_add(1);
            continue;
        }
        let (surface, count) = surfaces.remove(0);
        entries.push(OutputEntry {
            reading,
            surface,
            count,
        });
    }
    entries.sort_unstable_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.reading.cmp(&right.reading))
            .then_with(|| left.surface.cmp(&right.surface))
    });
    entries.truncate(options.maximum_entries);
    entries
}

fn retain_effective_promotions(
    entries: Vec<OutputEntry>,
    word_cost: i32,
    report: &mut Report,
) -> Vec<OutputEntry> {
    let baseline = Dictionary::bundled();
    entries
        .into_iter()
        .filter(|entry| {
            if baseline
                .candidates(&entry.reading)
                .first()
                .is_some_and(|candidate| candidate.surface == entry.surface)
            {
                report.baseline_already_top = report.baseline_already_top.saturating_add(1);
                return false;
            }
            let overlay = Dictionary::bundled_with_layers(vec![DictionaryLayer::new(
                "phrase-promotion-probe",
                "Phrase promotion probe",
                vec![DictionaryEntry::new(
                    &entry.reading,
                    &entry.surface,
                    word_cost,
                )],
            )]);
            if overlay
                .candidates(&entry.reading)
                .first()
                .is_some_and(|candidate| candidate.surface == entry.surface)
            {
                true
            } else {
                report.ineffective_promotions = report.ineffective_promotions.saturating_add(1);
                false
            }
        })
        .collect()
}

fn serialize_entries(entries: &[OutputEntry], word_cost: i32) -> String {
    let mut entries = entries.to_vec();
    entries.sort_unstable_by(|left, right| {
        left.reading
            .cmp(&right.reading)
            .then_with(|| left.surface.cmp(&right.surface))
    });
    let mut output = String::new();
    for entry in entries {
        output.push_str(&entry.reading);
        output.push('\t');
        output.push_str(&entry.surface);
        output.push('\t');
        output.push_str(&word_cost.to_string());
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        Options, OutputEntry, Report, normalized_phrase, parse_anthy_segments,
        retain_effective_promotions, select_entries,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn options() -> Options {
        Options {
            inputs: Vec::new(),
            output: PathBuf::from("phrases.tsv"),
            minimum_count: 2,
            maximum_entries: 10_000,
            word_cost: 5_000,
            json: true,
        }
    }

    #[test]
    fn normalizes_matching_case_particles_into_phrase_evidence() {
        assert_eq!(
            normalized_phrase("へんかんせいどに", "変換精度に"),
            Some(("へんかんせいど".to_owned(), "変換精度".to_owned()))
        );
        assert_eq!(
            normalized_phrase("へんかんせいどを", "変換精度を"),
            Some(("へんかんせいど".to_owned(), "変換精度".to_owned()))
        );
    }

    #[test]
    fn rejects_kana_only_and_short_segments() {
        assert_eq!(normalized_phrase("あったので", "あったので"), None);
        assert_eq!(normalized_phrase("せいど", "精度"), None);
        assert_eq!(normalized_phrase("とうきょうま", "東京ま"), None);
    }

    #[test]
    fn parses_aligned_anthy_segments() {
        assert_eq!(
            parse_anthy_segments("|へんかんせいどに|えいきょうする| |変換精度に|影響する|", 1,)
                .unwrap(),
            [
                ("へんかんせいどに".to_owned(), "変換精度に".to_owned()),
                ("えいきょうする".to_owned(), "影響する".to_owned())
            ]
        );
    }

    #[test]
    fn keeps_only_a_unique_most_frequent_surface() {
        let mut counts = BTreeMap::new();
        counts.insert(("へんかんせいど".to_owned(), "変換精度".to_owned()), 2);
        counts.insert(("さいかい".to_owned(), "再開".to_owned()), 2);
        counts.insert(("さいかい".to_owned(), "再会".to_owned()), 2);
        let mut report = Report::default();
        let entries = select_entries(counts, &options(), &mut report);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].surface, "変換精度");
        assert_eq!(report.ambiguous_readings, 1);
    }

    #[test]
    fn retains_only_entries_that_change_the_base_top_candidate() {
        let entries = vec![
            OutputEntry {
                reading: "いっぴき".to_owned(),
                surface: "一匹".to_owned(),
                count: 2,
            },
            OutputEntry {
                reading: "にほん".to_owned(),
                surface: "日本".to_owned(),
                count: 10,
            },
        ];
        let mut report = Report::default();
        let retained = retain_effective_promotions(entries, 7_000, &mut report);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].surface, "一匹");
        assert_eq!(report.baseline_already_top, 1);
    }
}
