//! Builds a private dictionary-entry TSV for terms that bounded generation
//! cannot currently recall.
//!
//! Generated entries contain vocabulary and must stay in the private build
//! workspace. Reports and errors intentionally contain aggregate data only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use slime_converter::{Dictionary, DictionaryEntry, DictionaryLayer};

mod private_generation;
use private_generation::{
    MAX_LINE_BYTES, MAX_TOKEN_CHARACTERS, Token, hash_tokens, ignorable_line,
    normalize_phonetic_reading, parse_annotated_line, read_private_input, sha256_hex,
    valid_token_field, validate_tsv_output, write_new_atomic as write_private_atomic,
};

const MAX_DICTIONARY_ENTRIES: usize = 250_000;
const MAX_UNIQUE_TERMS: usize = 1_000_000;
const MAX_UNIQUE_READINGS: usize = 100_000;
const MAX_OUTPUT_ENTRIES: usize = 100_000;

const MIN_COMPOUND_TOKENS: usize = 2;
const MAX_COMPOUND_TOKENS: usize = 6;
const MIN_SINGLE_READING_CHARACTERS: usize = 2;
const MIN_COMPOUND_READING_CHARACTERS: usize = 4;
const MAX_TERM_READING_CHARACTERS: usize = 32;
const MAX_SINGLE_SURFACE_CHARACTERS: usize = 32;

const INITIAL_CANDIDATES: usize = 10;
const EXPANDED_CANDIDATES: usize = 32;
const MAX_EXPANDED_READING_CHARACTERS: usize = 8;
const COMPOUND_ENTRIES_PER_SEGMENT: usize = 4;
const COMPOUND_CANDIDATE_LIMIT: usize = 16;
const FIXED_SEGMENT_ENTRIES_PER_SEGMENT: usize = 8;
const FIXED_SEGMENT_CANDIDATE_LIMIT: usize = 22;

const DEFAULT_MINIMUM_COUNT: u32 = 3;
const DEFAULT_MAXIMUM_ENTRIES: usize = 10_000;
const DEFAULT_MAXIMUM_SURFACES_PER_READING: usize = 4;
const DEFAULT_WORD_COST: i32 = 5_000;

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

    let mut input_bytes = 0_u64;
    let exclusions = load_exclusion_hashes(&options.exclusion_inputs, &mut input_bytes)?;
    let (counts, mut report) = load_term_counts(&options.inputs, &exclusions, &mut input_bytes)?;
    let external_entries = load_external_entries(&options.dictionaries, &mut input_bytes)?;
    let dictionary_bytes = external_entries
        .iter()
        .fold(0_u64, |total, source| total.saturating_add(source.bytes));
    let dictionary_entries = external_entries.iter().fold(0_usize, |total, source| {
        total.saturating_add(source.entries.len())
    });
    let baseline = dictionary_with_external_entries(&external_entries, &[]);
    let mut entries = select_missing_terms(&baseline, counts, &options, &mut report)?;
    entries = retain_recovered_entries(&external_entries, entries, &options, &mut report);
    if entries.is_empty() {
        return Err("training inputs produced no recoverable dictionary entries".to_owned());
    }

    let output = serialize_entries(&entries, options.word_cost);
    report.training_files = options.inputs.len();
    report.exclusion_files = options.exclusion_inputs.len();
    report.dictionary_files = options.dictionaries.len();
    report.input_bytes = input_bytes;
    report.dictionary_bytes = dictionary_bytes;
    report.dictionary_entries = dictionary_entries;
    report.output_entries = entries.len();
    report.output_bytes = output.len();
    report.output_sha256 = sha256_hex(output.as_bytes());
    write_private_atomic(&options.output, output.as_bytes(), "slime-term-dictionary")?;

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| "cannot serialize aggregate report".to_owned())?
        );
    } else {
        println!(
            "entries={}\tunique-terms={}\toccurrences={}\tbytes={}\tsha256={}",
            report.output_entries,
            report.unique_terms,
            report.term_occurrences,
            report.output_bytes,
            report.output_sha256
        );
    }
    Ok(())
}

const fn usage() -> &'static str {
    concat!(
        "usage: slime-term-dictionary --input PATH [--input PATH ...] --output OUTPUT.tsv \\\n",
        "  [--exclude-input PATH ...] [--dictionary PATH ...] [--min-count N] \\\n",
        "  [--max-entries N] [--max-surfaces-per-reading N] [--word-cost N] [--json]\n",
        "corpus format: whitespace-separated surface/reading tokens\n",
        "dictionary format: reading<TAB>surface[<TAB>cost]",
    )
}

#[derive(Debug)]
struct Options {
    inputs: Vec<PathBuf>,
    exclusion_inputs: Vec<PathBuf>,
    dictionaries: Vec<PathBuf>,
    output: PathBuf,
    minimum_count: u32,
    maximum_entries: usize,
    maximum_surfaces_per_reading: usize,
    word_cost: i32,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut inputs = Vec::new();
        let mut exclusion_inputs = Vec::new();
        let mut dictionaries = Vec::new();
        let mut output = None;
        let mut minimum_count = DEFAULT_MINIMUM_COUNT;
        let mut maximum_entries = DEFAULT_MAXIMUM_ENTRIES;
        let mut maximum_surfaces_per_reading = DEFAULT_MAXIMUM_SURFACES_PER_READING;
        let mut word_cost = DEFAULT_WORD_COST;
        let mut json = false;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--input" => inputs.push(PathBuf::from(next_value(&mut arguments, "--input")?)),
                "--exclude-input" => exclusion_inputs.push(PathBuf::from(next_value(
                    &mut arguments,
                    "--exclude-input",
                )?)),
                "--dictionary" => {
                    dictionaries.push(PathBuf::from(next_value(&mut arguments, "--dictionary")?));
                }
                "--output" if output.is_none() => {
                    output = Some(PathBuf::from(next_value(&mut arguments, "--output")?));
                }
                "--output" => return Err("--output is duplicated".to_owned()),
                "--min-count" => {
                    minimum_count = parse_positive_u32(&mut arguments, "--min-count")?;
                }
                "--max-entries" => {
                    maximum_entries =
                        parse_bounded_usize(&mut arguments, "--max-entries", MAX_OUTPUT_ENTRIES)?;
                }
                "--max-surfaces-per-reading" => {
                    maximum_surfaces_per_reading = parse_bounded_usize(
                        &mut arguments,
                        "--max-surfaces-per-reading",
                        INITIAL_CANDIDATES,
                    )?;
                }
                "--word-cost" => {
                    let value = next_value(&mut arguments, "--word-cost")?
                        .parse::<i32>()
                        .map_err(|_| "--word-cost requires an integer".to_owned())?;
                    if !(0..=100_000).contains(&value) {
                        return Err("--word-cost must be between 0 and 100000".to_owned());
                    }
                    word_cost = value;
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
            exclusion_inputs,
            dictionaries,
            output: output.expect("checked above"),
            minimum_count,
            maximum_entries,
            maximum_surfaces_per_reading,
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

fn parse_positive_u32(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<u32, String> {
    next_value(arguments, option)?
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{option} requires a positive integer"))
}

fn parse_bounded_usize(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
    maximum: usize,
) -> Result<usize, String> {
    let value = usize::try_from(parse_positive_u32(arguments, option)?)
        .map_err(|_| format!("{option} is too large"))?;
    if value > maximum {
        return Err(format!("{option} cannot exceed {maximum}"));
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Evidence {
    count: u32,
    maximum_tokens: u8,
}

type TermKey = (String, String);
type TermCounts = BTreeMap<TermKey, Evidence>;

#[derive(Default, Serialize)]
struct Report {
    training_files: usize,
    exclusion_files: usize,
    dictionary_files: usize,
    input_bytes: u64,
    dictionary_bytes: u64,
    dictionary_entries: usize,
    lines: usize,
    accepted_lines: usize,
    duplicate_lines: usize,
    excluded_lines: usize,
    tokens: usize,
    single_term_windows: usize,
    compound_windows: usize,
    rejected_windows: usize,
    term_occurrences: usize,
    unique_terms: usize,
    below_minimum_count: usize,
    already_reachable: usize,
    missing_terms: usize,
    per_reading_limited: usize,
    total_limited: usize,
    not_recovered_after_overlay: usize,
    output_entries: usize,
    output_bytes: usize,
    output_sha256: String,
}

fn load_exclusion_hashes(
    paths: &[PathBuf],
    total_bytes: &mut u64,
) -> Result<HashSet<[u8; 32]>, String> {
    let mut hashes = HashSet::new();
    for path in paths {
        let source = read_private_input(path, "exclusion corpus", total_bytes)?;
        for (index, line) in source.lines().enumerate() {
            if ignorable_line(line) {
                continue;
            }
            let tokens = parse_annotated_line(line, index + 1)?;
            hashes.insert(hash_tokens(&tokens));
        }
    }
    Ok(hashes)
}

fn load_term_counts(
    paths: &[PathBuf],
    exclusions: &HashSet<[u8; 32]>,
    total_bytes: &mut u64,
) -> Result<(TermCounts, Report), String> {
    let mut counts = TermCounts::new();
    let mut seen_lines = HashSet::new();
    let mut report = Report::default();

    for path in paths {
        let source = read_private_input(path, "training corpus", total_bytes)?;
        for (index, line) in source.lines().enumerate() {
            report.lines = report.lines.saturating_add(1);
            if ignorable_line(line) {
                continue;
            }
            let tokens = parse_annotated_line(line, index + 1)?;
            let hash = hash_tokens(&tokens);
            if exclusions.contains(&hash) {
                report.excluded_lines = report.excluded_lines.saturating_add(1);
                continue;
            }
            if !seen_lines.insert(hash) {
                report.duplicate_lines = report.duplicate_lines.saturating_add(1);
                continue;
            }
            report.accepted_lines = report.accepted_lines.saturating_add(1);
            report.tokens = report.tokens.saturating_add(tokens.len());
            let mut line_terms = HashMap::new();
            collect_line_terms(&tokens, &mut line_terms, &mut report);
            report.term_occurrences = report.term_occurrences.saturating_add(line_terms.len());
            for (key, token_count) in line_terms {
                let entry = counts.entry(key).or_default();
                entry.count = entry.count.saturating_add(1);
                entry.maximum_tokens = entry.maximum_tokens.max(token_count);
                if counts.len() > MAX_UNIQUE_TERMS {
                    return Err(format!(
                        "training corpus exceeds the {MAX_UNIQUE_TERMS} unique term limit"
                    ));
                }
            }
        }
    }
    if report.accepted_lines == 0 {
        return Err("training corpus contains no usable lines".to_owned());
    }
    if counts.is_empty() {
        return Err("training corpus contains no eligible term observations".to_owned());
    }
    report.unique_terms = counts.len();
    Ok((counts, report))
}

fn collect_line_terms(tokens: &[Token], terms: &mut HashMap<TermKey, u8>, report: &mut Report) {
    for token in tokens {
        report.single_term_windows = report.single_term_windows.saturating_add(1);
        if let Some(key) = standalone_term(token) {
            terms
                .entry(key)
                .and_modify(|tokens| *tokens = (*tokens).max(1))
                .or_insert(1);
        } else {
            report.rejected_windows = report.rejected_windows.saturating_add(1);
        }
    }

    for start in 0..tokens.len() {
        let maximum_end = (start + MAX_COMPOUND_TOKENS).min(tokens.len());
        for end in start + MIN_COMPOUND_TOKENS..=maximum_end {
            report.compound_windows = report.compound_windows.saturating_add(1);
            let window = &tokens[start..end];
            if let Some(key) = compound_term(window) {
                let token_count = u8::try_from(window.len()).expect("compound token limit fits u8");
                terms
                    .entry(key)
                    .and_modify(|tokens| *tokens = (*tokens).max(token_count))
                    .or_insert(token_count);
            } else {
                report.rejected_windows = report.rejected_windows.saturating_add(1);
            }
        }
    }
}

fn standalone_term(token: &Token) -> Option<TermKey> {
    let reading = normalize_phonetic_reading(&token.reading)?;
    let reading_characters = reading.chars().count();
    let surface_characters = token.surface.chars().count();
    if !(MIN_SINGLE_READING_CHARACTERS..=MAX_TERM_READING_CHARACTERS).contains(&reading_characters)
        || !(2..=MAX_SINGLE_SURFACE_CHARACTERS).contains(&surface_characters)
        || !token.surface.chars().all(is_lexical_character)
        || !token.surface.chars().any(is_kanji_or_katakana)
    {
        return None;
    }
    Some((reading, token.surface.clone()))
}

fn compound_term(tokens: &[Token]) -> Option<TermKey> {
    if !tokens.iter().all(is_compound_element) {
        return None;
    }
    let kanji_elements = tokens
        .iter()
        .filter(|token| token.surface.chars().any(is_kanji))
        .count();
    let all_katakana = tokens.iter().all(|token| {
        token
            .surface
            .chars()
            .all(|character| is_katakana(character) || matches!(character, 'ー' | '・'))
    });
    if kanji_elements < 2 && !all_katakana {
        return None;
    }
    let readings = tokens
        .iter()
        .map(|token| normalize_phonetic_reading(&token.reading))
        .collect::<Option<Vec<_>>>()?;
    let reading: String = readings.iter().map(String::as_str).collect();
    if !(MIN_COMPOUND_READING_CHARACTERS..=MAX_TERM_READING_CHARACTERS)
        .contains(&reading.chars().count())
    {
        return None;
    }
    let surface: String = tokens.iter().map(|token| token.surface.as_str()).collect();
    (surface.chars().count() <= MAX_TOKEN_CHARACTERS).then_some((reading, surface))
}

fn is_compound_element(token: &Token) -> bool {
    let mut characters = token.surface.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    is_lexical_character(first)
        && characters.next().is_some_and(is_lexical_character)
        && characters.all(is_lexical_character)
}

fn is_lexical_character(character: char) -> bool {
    is_kanji(character) || is_katakana(character) || matches!(character, 'ー' | '・')
}

fn is_kanji_or_katakana(character: char) -> bool {
    is_kanji(character) || is_katakana(character)
}

fn is_kanji(character: char) -> bool {
    matches!(character, '\u{4e00}'..='\u{9fff}' | '々' | '〆')
}

fn is_katakana(character: char) -> bool {
    matches!(character, 'ァ'..='ヶ' | 'ヽ' | 'ヾ')
}

#[derive(Clone)]
struct ExternalEntrySource {
    entries: Vec<DictionaryEntry>,
    bytes: u64,
}

fn load_external_entries(
    paths: &[PathBuf],
    total_bytes: &mut u64,
) -> Result<Vec<ExternalEntrySource>, String> {
    let mut sources = Vec::new();
    let mut total_entries = 0_usize;
    for path in paths {
        let before = *total_bytes;
        let source = read_private_input(path, "dictionary", total_bytes)?;
        let entries = parse_dictionary_entries(&source)?;
        total_entries = total_entries
            .checked_add(entries.len())
            .ok_or_else(|| "dictionary entry total overflowed".to_owned())?;
        if total_entries > MAX_DICTIONARY_ENTRIES {
            return Err(format!(
                "dictionaries exceed the {MAX_DICTIONARY_ENTRIES} entry limit"
            ));
        }
        sources.push(ExternalEntrySource {
            entries,
            bytes: total_bytes.saturating_sub(before),
        });
    }
    Ok(sources)
}

fn parse_dictionary_entries(source: &str) -> Result<Vec<DictionaryEntry>, String> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.len() > MAX_LINE_BYTES {
            return Err(format!(
                "dictionary line {line_number} exceeds the byte limit"
            ));
        }
        let mut columns = line.split('\t');
        let reading = columns.next().unwrap_or_default();
        let surface = columns.next().unwrap_or_default();
        let cost = columns.next().map_or(Ok(DEFAULT_WORD_COST), |value| {
            value
                .parse::<i32>()
                .map_err(|_| format!("dictionary line {line_number} has an invalid cost"))
        })?;
        if columns.next().is_some() || !valid_token_field(surface) || !(0..=100_000).contains(&cost)
        {
            return Err(format!("dictionary line {line_number} is invalid"));
        }
        let Some(reading) = normalize_phonetic_reading(reading) else {
            return Err(format!(
                "dictionary line {line_number} has an invalid reading"
            ));
        };
        if !seen.insert((reading.clone(), surface.to_owned())) {
            return Err(format!(
                "dictionary line {line_number} duplicates an earlier entry"
            ));
        }
        entries.push(DictionaryEntry::new(reading, surface, cost));
    }
    if entries.is_empty() {
        return Err("dictionary contains no entries".to_owned());
    }
    Ok(entries)
}

fn dictionary_with_external_entries(
    external: &[ExternalEntrySource],
    generated: &[GeneratedEntry],
) -> Dictionary {
    let mut layers: Vec<_> = external
        .iter()
        .enumerate()
        .map(|(index, source)| {
            DictionaryLayer::new(
                format!("term-source-{index}"),
                "Term generation dictionary",
                source.entries.clone(),
            )
        })
        .collect();
    if !generated.is_empty() {
        layers.push(DictionaryLayer::new(
            "generated-terms",
            "Generated term candidates",
            generated
                .iter()
                .map(|entry| DictionaryEntry::new(&entry.reading, &entry.surface, entry.word_cost))
                .collect(),
        ));
    }
    if layers.is_empty() {
        Dictionary::bundled()
    } else {
        Dictionary::bundled_with_layers(layers)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GeneratedEntry {
    reading: String,
    surface: String,
    count: u32,
    maximum_tokens: u8,
    word_cost: i32,
}

fn select_missing_terms(
    dictionary: &Dictionary,
    counts: TermCounts,
    options: &Options,
    report: &mut Report,
) -> Result<Vec<GeneratedEntry>, String> {
    let mut reachable_by_reading: HashMap<String, HashSet<String>> = HashMap::new();
    let mut missing = Vec::new();

    for ((reading, surface), evidence) in counts {
        if evidence.count < options.minimum_count {
            report.below_minimum_count = report.below_minimum_count.saturating_add(1);
            continue;
        }
        if !reachable_by_reading.contains_key(&reading) {
            if reachable_by_reading.len() == MAX_UNIQUE_READINGS {
                return Err(format!(
                    "training corpus exceeds the {MAX_UNIQUE_READINGS} candidate-reading limit"
                ));
            }
            reachable_by_reading.insert(reading.clone(), reachable_surfaces(dictionary, &reading));
        }
        if reachable_by_reading[&reading].contains(&surface) {
            report.already_reachable = report.already_reachable.saturating_add(1);
            continue;
        }
        missing.push(GeneratedEntry {
            reading,
            surface,
            count: evidence.count,
            maximum_tokens: evidence.maximum_tokens,
            word_cost: options.word_cost,
        });
    }

    missing.sort_unstable_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| right.maximum_tokens.cmp(&left.maximum_tokens))
            .then_with(|| {
                right
                    .reading
                    .chars()
                    .count()
                    .cmp(&left.reading.chars().count())
            })
            .then_with(|| left.reading.cmp(&right.reading))
            .then_with(|| left.surface.cmp(&right.surface))
    });
    report.missing_terms = missing.len();

    let mut per_reading = HashMap::<String, usize>::new();
    missing.retain(|entry| {
        let count = per_reading.entry(entry.reading.clone()).or_default();
        if *count == options.maximum_surfaces_per_reading {
            report.per_reading_limited = report.per_reading_limited.saturating_add(1);
            return false;
        }
        *count += 1;
        true
    });
    report.total_limited = missing.len().saturating_sub(options.maximum_entries);
    missing.truncate(options.maximum_entries);
    Ok(missing)
}

fn reachable_surfaces(dictionary: &Dictionary, reading: &str) -> HashSet<String> {
    let mut surfaces = HashSet::new();
    surfaces.extend(
        dictionary
            .candidates(reading)
            .into_iter()
            .map(|candidate| candidate.surface),
    );
    let reading_characters = reading.chars().count();
    if reading_characters <= MAX_EXPANDED_READING_CHARACTERS {
        surfaces.extend(
            dictionary
                .candidates_with_limit(reading, EXPANDED_CANDIDATES)
                .into_iter()
                .map(|candidate| candidate.surface),
        );
    }
    surfaces.extend(
        dictionary
            .compound_candidates(
                reading,
                COMPOUND_ENTRIES_PER_SEGMENT,
                COMPOUND_CANDIDATE_LIMIT,
            )
            .into_iter()
            .map(|candidate| candidate.surface),
    );
    if reading_characters > MAX_EXPANDED_READING_CHARACTERS {
        surfaces.extend(dictionary.fixed_segment_variants(
            reading,
            FIXED_SEGMENT_ENTRIES_PER_SEGMENT,
            FIXED_SEGMENT_CANDIDATE_LIMIT,
        ));
    }
    surfaces
}

fn retain_recovered_entries(
    external: &[ExternalEntrySource],
    entries: Vec<GeneratedEntry>,
    options: &Options,
    report: &mut Report,
) -> Vec<GeneratedEntry> {
    let dictionary = dictionary_with_external_entries(external, &entries);
    let mut initial_by_reading = HashMap::<String, HashSet<String>>::new();
    let recovered: Vec<_> = entries
        .into_iter()
        .filter(|entry| {
            let surfaces = initial_by_reading
                .entry(entry.reading.clone())
                .or_insert_with(|| {
                    dictionary
                        .candidates_with_limit(&entry.reading, INITIAL_CANDIDATES)
                        .into_iter()
                        .map(|candidate| candidate.surface)
                        .collect()
                });
            if surfaces.contains(&entry.surface) {
                true
            } else {
                report.not_recovered_after_overlay =
                    report.not_recovered_after_overlay.saturating_add(1);
                false
            }
        })
        .collect();
    debug_assert!(
        recovered
            .iter()
            .all(|entry| entry.word_cost == options.word_cost)
    );
    recovered
}

fn serialize_entries(entries: &[GeneratedEntry], word_cost: i32) -> String {
    let mut sorted = entries.to_vec();
    sorted.sort_unstable_by(|left, right| {
        left.reading
            .cmp(&right.reading)
            .then_with(|| left.surface.cmp(&right.surface))
    });
    let mut output = String::new();
    for entry in sorted {
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
        DEFAULT_MAXIMUM_ENTRIES, DEFAULT_MAXIMUM_SURFACES_PER_READING, DEFAULT_MINIMUM_COUNT,
        DEFAULT_WORD_COST, GeneratedEntry, Options, Report, TermCounts, Token, collect_line_terms,
        hash_tokens, load_term_counts, parse_annotated_line, retain_recovered_entries,
        select_missing_terms, serialize_entries, write_private_atomic,
    };
    use slime_converter::Dictionary;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn options() -> Options {
        Options {
            inputs: Vec::new(),
            exclusion_inputs: Vec::new(),
            dictionaries: Vec::new(),
            output: PathBuf::from("terms.tsv"),
            minimum_count: DEFAULT_MINIMUM_COUNT,
            maximum_entries: DEFAULT_MAXIMUM_ENTRIES,
            maximum_surfaces_per_reading: DEFAULT_MAXIMUM_SURFACES_PER_READING,
            word_cost: DEFAULT_WORD_COST,
            json: true,
        }
    }

    #[test]
    fn extracts_standalone_terms_and_bounded_compounds() {
        let tokens = parse_annotated_line(
            "蒼峰/そうほう 研究所/けんきゅうじょ は/は 新規/しんき 商品/しょうひん を/を扱う/あつかう",
            1,
        )
        .unwrap();
        let mut terms = HashMap::new();
        let mut report = Report::default();
        collect_line_terms(&tokens, &mut terms, &mut report);
        assert_eq!(terms[&("そうほう".to_owned(), "蒼峰".to_owned())], 1);
        assert_eq!(
            terms[&("そうほうけんきゅうじょ".to_owned(), "蒼峰研究所".to_owned())],
            2
        );
        assert_eq!(
            terms[&("しんきしょうひん".to_owned(), "新規商品".to_owned())],
            2
        );
        assert!(!terms.keys().any(|(_, surface)| surface.contains('を')));
    }

    #[test]
    fn keeps_only_terms_missing_from_all_bounded_paths() {
        let mut counts = TermCounts::new();
        counts.insert(
            ("かんじ".to_owned(), "漢字".to_owned()),
            super::Evidence {
                count: 4,
                maximum_tokens: 1,
            },
        );
        counts.insert(
            ("そうほう".to_owned(), "蒼峰".to_owned()),
            super::Evidence {
                count: 4,
                maximum_tokens: 1,
            },
        );
        let mut report = Report::default();
        let missing = select_missing_terms(
            &Dictionary::new(Vec::new()),
            counts,
            &options(),
            &mut report,
        )
        .unwrap();
        assert_eq!(missing.len(), 2);

        let mut bundled_counts = TermCounts::new();
        bundled_counts.insert(
            ("かんじ".to_owned(), "漢字".to_owned()),
            super::Evidence {
                count: 4,
                maximum_tokens: 1,
            },
        );
        let mut bundled_report = Report::default();
        assert!(
            select_missing_terms(
                &Dictionary::bundled(),
                bundled_counts,
                &options(),
                &mut bundled_report,
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(bundled_report.already_reachable, 1);
    }

    #[test]
    fn generated_entry_must_reach_the_initial_candidate_pool() {
        let entry = GeneratedEntry {
            reading: "そうほう".to_owned(),
            surface: "蒼峰".to_owned(),
            count: 3,
            maximum_tokens: 1,
            word_cost: DEFAULT_WORD_COST,
        };
        let mut report = Report::default();
        let recovered = retain_recovered_entries(&[], vec![entry], &options(), &mut report);
        assert_eq!(recovered.len(), 1);
        assert_eq!(report.not_recovered_after_overlay, 0);
    }

    #[test]
    fn serialization_is_canonical_and_contains_no_evidence_metadata() {
        let entries = vec![
            GeneratedEntry {
                reading: "そうほう".to_owned(),
                surface: "蒼峰".to_owned(),
                count: 9,
                maximum_tokens: 1,
                word_cost: DEFAULT_WORD_COST,
            },
            GeneratedEntry {
                reading: "あおみね".to_owned(),
                surface: "青峰".to_owned(),
                count: 3,
                maximum_tokens: 1,
                word_cost: DEFAULT_WORD_COST,
            },
        ];
        assert_eq!(
            serialize_entries(&entries, DEFAULT_WORD_COST),
            "あおみね\t青峰\t5000\nそうほう\t蒼峰\t5000\n"
        );
    }

    #[test]
    fn parser_errors_do_not_echo_private_content_and_hash_normalizes_kana() {
        let private_value = "非公開顧客語";
        let error = parse_annotated_line(private_value, 7).unwrap_err();
        assert!(error.contains("line 7"));
        assert!(!error.contains(private_value));

        let hiragana = vec![Token {
            surface: "語".to_owned(),
            reading: "ご".to_owned(),
        }];
        let katakana = vec![Token {
            surface: "語".to_owned(),
            reading: "ゴ".to_owned(),
        }];
        assert_eq!(hash_tokens(&hiragana), hash_tokens(&katakana));
    }

    #[test]
    fn atomic_output_never_replaces_an_existing_file() {
        let directory = test_directory();
        let output = directory.join("terms.tsv");
        fs::write(&output, "keep").unwrap();
        assert_eq!(
            write_private_atomic(&output, b"replace", "slime-term-dictionary").unwrap_err(),
            "output already exists"
        );
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn excludes_held_out_and_duplicate_lines_before_counting() {
        let directory = test_directory();
        let training = directory.join("private-training.txt");
        fs::write(
            &training,
            "長文/ちょうぶん 蒼峰/そうほう\n\
             長文/ちょうぶん 蒼峰/そうほう\n\
             短文/たんぶん 蒼峰/そうほう\n",
        )
        .unwrap();
        let excluded = parse_annotated_line("短文/たんぶん 蒼峰/そうほう", 1).unwrap();
        let exclusions = HashSet::from([hash_tokens(&excluded)]);
        let mut bytes = 0;
        let (counts, report) = load_term_counts(&[training], &exclusions, &mut bytes).unwrap();
        assert_eq!(report.accepted_lines, 1);
        assert_eq!(report.duplicate_lines, 1);
        assert_eq!(report.excluded_lines, 1);
        assert_eq!(counts[&("そうほう".to_owned(), "蒼峰".to_owned())].count, 1);
        fs::remove_dir_all(directory).unwrap();
    }

    fn test_directory() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "slime-term-dictionary-test-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
}
