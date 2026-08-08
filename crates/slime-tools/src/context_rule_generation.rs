//! Builds conservative static left-context rules from private annotated text.
//!
//! The generated TSV contains vocabulary and must stay in the private build
//! workspace. Standard output and errors intentionally expose aggregate counts
//! only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use slime_converter::{Dictionary, DictionaryEntry, DictionaryLayer};

mod private_generation;
use private_generation::{
    MAX_LINE_BYTES, hash_tokens, ignorable_line, normalize_phonetic_reading, parse_annotated_line,
    read_private_input, sha256_hex, valid_token_field, validate_tsv_output,
    write_new_atomic as write_private_atomic,
};

const MAX_DICTIONARY_ENTRIES: usize = 250_000;
const MAX_UNIQUE_CONTEXTS: usize = 1_000_000;
const MAX_UNIQUE_READINGS: usize = 100_000;
const MAX_RULES: usize = 100_000;
const DEFAULT_WORD_COST: i32 = 5_000;
const DEFAULT_MINIMUM_COUNT: u32 = 3;
const DEFAULT_MINIMUM_MARGIN: u32 = 2;
const DEFAULT_MINIMUM_SHARE_BASIS_POINTS: u16 = 7_500;
const DEFAULT_MAXIMUM_RULES: usize = 10_000;

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
    let exclusion_hashes = load_exclusion_hashes(&options.exclusion_inputs, &mut input_bytes)?;
    let (counts, mut report) =
        load_observations(&options.inputs, &exclusion_hashes, &mut input_bytes)?;
    let (dictionary, dictionary_entries, dictionary_bytes) =
        load_dictionary(&options.dictionaries, &mut input_bytes)?;
    report.training_files = options.inputs.len();
    report.exclusion_files = options.exclusion_inputs.len();
    report.dictionary_files = options.dictionaries.len();
    report.dictionary_entries = dictionary_entries;
    report.dictionary_bytes = dictionary_bytes;
    report.input_bytes = input_bytes;

    let rules = select_rules(&dictionary, counts, &options, &mut report)?;
    if rules.is_empty() {
        return Err("training inputs produced no eligible context rules".to_owned());
    }
    let output = serialize_rules(&rules);
    report.output_bytes = output.len();
    report.output_sha256 = sha256_hex(output.as_bytes());
    write_private_atomic(&options.output, output.as_bytes(), "slime-context-rules")?;

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| "cannot serialize aggregate report".to_owned())?
        );
    } else {
        println!(
            "rules={}\tcontexts={}\tobservations={}\tbytes={}\tsha256={}",
            report.selected_rules,
            report.unique_context_readings,
            report.observations,
            report.output_bytes,
            report.output_sha256
        );
    }
    Ok(())
}

const fn usage() -> &'static str {
    concat!(
        "usage: slime-context-rules --input PATH [--input PATH ...] --output OUTPUT.tsv \\\n",
        "  [--exclude-input PATH ...] [--dictionary PATH ...] [--min-count N] \\\n",
        "  [--min-margin N] [--min-share-bps N] [--max-rules N] [--json]\n",
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
    minimum_margin: u32,
    minimum_share_basis_points: u16,
    maximum_rules: usize,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut inputs = Vec::new();
        let mut exclusion_inputs = Vec::new();
        let mut dictionaries = Vec::new();
        let mut output = None;
        let mut minimum_count = DEFAULT_MINIMUM_COUNT;
        let mut minimum_margin = DEFAULT_MINIMUM_MARGIN;
        let mut minimum_share_basis_points = DEFAULT_MINIMUM_SHARE_BASIS_POINTS;
        let mut maximum_rules = DEFAULT_MAXIMUM_RULES;
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
                "--min-margin" => {
                    minimum_margin = parse_positive_u32(&mut arguments, "--min-margin")?;
                }
                "--min-share-bps" => {
                    let value = parse_positive_u32(&mut arguments, "--min-share-bps")?;
                    minimum_share_basis_points = u16::try_from(value)
                        .ok()
                        .filter(|value| *value <= 10_000)
                        .ok_or_else(|| "--min-share-bps must be between 1 and 10000".to_owned())?;
                }
                "--max-rules" => {
                    maximum_rules =
                        usize::try_from(parse_positive_u32(&mut arguments, "--max-rules")?)
                            .map_err(|_| "--max-rules is too large".to_owned())?;
                    if maximum_rules > MAX_RULES {
                        return Err(format!("--max-rules cannot exceed {MAX_RULES}"));
                    }
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
            minimum_margin,
            minimum_share_basis_points,
            maximum_rules,
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

type ContextKey = (String, String);
type ObservationCounts = BTreeMap<ContextKey, BTreeMap<String, u32>>;

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
    observations: usize,
    skipped_non_phonetic: usize,
    unique_context_readings: usize,
    below_minimum_count: usize,
    tied: usize,
    below_minimum_margin: usize,
    below_minimum_share: usize,
    phonetic_winner: usize,
    unreachable_winner: usize,
    already_top1: usize,
    eligible_rules: usize,
    truncated_rules: usize,
    selected_rules: usize,
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
            let line_number = index + 1;
            if ignorable_line(line) {
                continue;
            }
            let tokens = parse_annotated_line(line, line_number)?;
            hashes.insert(hash_tokens(&tokens));
        }
    }
    Ok(hashes)
}

fn load_observations(
    paths: &[PathBuf],
    exclusions: &HashSet<[u8; 32]>,
    total_bytes: &mut u64,
) -> Result<(ObservationCounts, Report), String> {
    let mut counts = ObservationCounts::new();
    let mut seen_lines = HashSet::new();
    let mut report = Report::default();
    let mut unique_observations = 0_usize;

    for path in paths {
        let source = read_private_input(path, "training corpus", total_bytes)?;
        for (index, line) in source.lines().enumerate() {
            let line_number = index + 1;
            report.lines = report.lines.saturating_add(1);
            if ignorable_line(line) {
                continue;
            }
            let tokens = parse_annotated_line(line, line_number)?;
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

            for pair in tokens.windows(2) {
                let previous = &pair[0];
                let current = &pair[1];
                let Some(reading) = normalize_phonetic_reading(&current.reading) else {
                    report.skipped_non_phonetic = report.skipped_non_phonetic.saturating_add(1);
                    continue;
                };
                if !valid_context_surface(&previous.surface) {
                    report.skipped_non_phonetic = report.skipped_non_phonetic.saturating_add(1);
                    continue;
                }
                report.observations = report.observations.saturating_add(1);
                let surfaces = counts
                    .entry((previous.surface.clone(), reading))
                    .or_default();
                let entry = surfaces.entry(current.surface.clone()).or_default();
                if *entry == 0 {
                    unique_observations = unique_observations.saturating_add(1);
                    if unique_observations > MAX_UNIQUE_CONTEXTS {
                        return Err(format!(
                            "training corpus exceeds the {MAX_UNIQUE_CONTEXTS} unique observation limit"
                        ));
                    }
                }
                *entry = entry.saturating_add(1);
            }
        }
    }
    if report.accepted_lines == 0 {
        return Err("training corpus contains no usable lines".to_owned());
    }
    if counts.is_empty() {
        return Err("training corpus contains no usable context observations".to_owned());
    }
    report.unique_context_readings = counts.len();
    Ok((counts, report))
}

fn valid_context_surface(surface: &str) -> bool {
    valid_token_field(surface) && surface.chars().any(char::is_alphanumeric)
}

fn load_dictionary(
    paths: &[PathBuf],
    total_input_bytes: &mut u64,
) -> Result<(Dictionary, usize, u64), String> {
    let mut layers = Vec::new();
    let mut total_entries = 0_usize;
    let mut dictionary_bytes = 0_u64;
    for (index, path) in paths.iter().enumerate() {
        let before = *total_input_bytes;
        let source = read_private_input(path, "dictionary", total_input_bytes)?;
        dictionary_bytes = dictionary_bytes
            .checked_add(total_input_bytes.saturating_sub(before))
            .ok_or_else(|| "dictionary byte total overflowed".to_owned())?;
        let entries = parse_dictionary_entries(&source)?;
        total_entries = total_entries
            .checked_add(entries.len())
            .ok_or_else(|| "dictionary entry total overflowed".to_owned())?;
        if total_entries > MAX_DICTIONARY_ENTRIES {
            return Err(format!(
                "dictionaries exceed the {MAX_DICTIONARY_ENTRIES} entry limit"
            ));
        }
        layers.push(DictionaryLayer::new(
            format!("context-source-{index}"),
            "Context rule generation dictionary",
            entries,
        ));
    }
    let dictionary = if layers.is_empty() {
        Dictionary::bundled()
    } else {
        Dictionary::bundled_with_layers(layers)
    };
    Ok((dictionary, total_entries, dictionary_bytes))
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct Rule {
    previous_surface: String,
    reading: String,
    surface: String,
    count: u32,
    margin: u32,
    share_basis_points: u16,
}

fn select_rules(
    dictionary: &Dictionary,
    counts: ObservationCounts,
    options: &Options,
    report: &mut Report,
) -> Result<Vec<Rule>, String> {
    let mut candidates_by_reading: HashMap<String, Vec<String>> = HashMap::new();
    let mut rules = Vec::new();

    for ((previous_surface, reading), surfaces) in counts {
        let total = surfaces.values().fold(0_u64, |total, count| {
            total.saturating_add(u64::from(*count))
        });
        let mut ranked: Vec<_> = surfaces.into_iter().collect();
        ranked.sort_unstable_by(|(left_surface, left_count), (right_surface, right_count)| {
            right_count
                .cmp(left_count)
                .then_with(|| left_surface.cmp(right_surface))
        });
        let (winner, winner_count) = ranked.first().expect("non-empty observations");
        let runner_up_count = ranked.get(1).map_or(0, |(_, count)| *count);
        if *winner_count < options.minimum_count {
            report.below_minimum_count = report.below_minimum_count.saturating_add(1);
            continue;
        }
        if *winner_count == runner_up_count {
            report.tied = report.tied.saturating_add(1);
            continue;
        }
        let margin = winner_count.saturating_sub(runner_up_count);
        if margin < options.minimum_margin {
            report.below_minimum_margin = report.below_minimum_margin.saturating_add(1);
            continue;
        }
        let share_basis_points = u16::try_from(
            u64::from(*winner_count)
                .saturating_mul(10_000)
                .checked_div(total)
                .unwrap_or_default(),
        )
        .unwrap_or(10_000);
        if share_basis_points < options.minimum_share_basis_points {
            report.below_minimum_share = report.below_minimum_share.saturating_add(1);
            continue;
        }
        if normalize_phonetic_reading(winner).as_deref() == Some(reading.as_str()) {
            report.phonetic_winner = report.phonetic_winner.saturating_add(1);
            continue;
        }

        if !candidates_by_reading.contains_key(&reading) {
            if candidates_by_reading.len() == MAX_UNIQUE_READINGS {
                return Err(format!(
                    "training corpus exceeds the {MAX_UNIQUE_READINGS} candidate-reading limit"
                ));
            }
            let candidates = dictionary
                .candidates(&reading)
                .into_iter()
                .map(|candidate| candidate.surface)
                .collect();
            candidates_by_reading.insert(reading.clone(), candidates);
        }
        let candidates = candidates_by_reading
            .get(&reading)
            .expect("candidate cache populated above");
        let Some(position) = candidates.iter().position(|candidate| candidate == winner) else {
            report.unreachable_winner = report.unreachable_winner.saturating_add(1);
            continue;
        };
        if position == 0 {
            report.already_top1 = report.already_top1.saturating_add(1);
            continue;
        }
        rules.push(Rule {
            previous_surface,
            reading,
            surface: winner.clone(),
            count: *winner_count,
            margin,
            share_basis_points,
        });
    }

    rules.sort_unstable_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| right.margin.cmp(&left.margin))
            .then_with(|| right.share_basis_points.cmp(&left.share_basis_points))
            .then_with(|| left.reading.cmp(&right.reading))
            .then_with(|| left.previous_surface.cmp(&right.previous_surface))
            .then_with(|| left.surface.cmp(&right.surface))
    });
    report.eligible_rules = rules.len();
    report.truncated_rules = rules.len().saturating_sub(options.maximum_rules);
    rules.truncate(options.maximum_rules);
    rules.sort_unstable_by(|left, right| {
        left.reading
            .cmp(&right.reading)
            .then_with(|| left.previous_surface.cmp(&right.previous_surface))
            .then_with(|| left.surface.cmp(&right.surface))
    });
    report.selected_rules = rules.len();
    Ok(rules)
}

fn serialize_rules(rules: &[Rule]) -> String {
    let mut output = String::new();
    for rule in rules {
        output.push_str(&rule.previous_surface);
        output.push('\t');
        output.push_str(&rule.reading);
        output.push('\t');
        output.push_str(&rule.surface);
        output.push_str("\t0\n");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::private_generation::Token;
    use super::{
        DEFAULT_MAXIMUM_RULES, DEFAULT_MINIMUM_COUNT, DEFAULT_MINIMUM_MARGIN,
        DEFAULT_MINIMUM_SHARE_BASIS_POINTS, ObservationCounts, Options, Report, hash_tokens,
        load_observations, parse_annotated_line, select_rules, serialize_rules,
        write_private_atomic,
    };
    use slime_converter::Dictionary;
    use std::collections::HashSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn options() -> Options {
        Options {
            inputs: Vec::new(),
            exclusion_inputs: Vec::new(),
            dictionaries: Vec::new(),
            output: PathBuf::from("rules.tsv"),
            minimum_count: DEFAULT_MINIMUM_COUNT,
            minimum_margin: DEFAULT_MINIMUM_MARGIN,
            minimum_share_basis_points: DEFAULT_MINIMUM_SHARE_BASIS_POINTS,
            maximum_rules: DEFAULT_MAXIMUM_RULES,
            json: true,
        }
    }

    fn counts(previous: &str, reading: &str, observations: &[(&str, u32)]) -> ObservationCounts {
        let mut counts = ObservationCounts::new();
        counts.insert(
            (previous.to_owned(), reading.to_owned()),
            observations
                .iter()
                .map(|(surface, count)| ((*surface).to_owned(), *count))
                .collect(),
        );
        counts
    }

    #[test]
    fn selects_only_a_reachable_non_top1_dominant_surface() {
        let mut report = Report::default();
        let rules = select_rules(
            &Dictionary::bundled(),
            counts("文章", "かんじ", &[("漢字", 5), ("感じ", 1)]),
            &options(),
            &mut report,
        )
        .unwrap();
        assert_eq!(serialize_rules(&rules), "文章\tかんじ\t漢字\t0\n");
        assert_eq!(report.selected_rules, 1);
    }

    #[test]
    fn rejects_ties_unreachable_surfaces_and_existing_top1() {
        let dictionary = Dictionary::bundled();
        let mut tied_report = Report::default();
        assert!(
            select_rules(
                &dictionary,
                counts("文章", "かんじ", &[("漢字", 3), ("感じ", 3)]),
                &options(),
                &mut tied_report,
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(tied_report.tied, 1);

        let mut unreachable_report = Report::default();
        assert!(
            select_rules(
                &dictionary,
                counts("文章", "かんじ", &[("未収録表記", 5)]),
                &options(),
                &mut unreachable_report,
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(unreachable_report.unreachable_winner, 1);

        let mut top1_report = Report::default();
        assert!(
            select_rules(
                &dictionary,
                counts("文章", "かんじ", &[("感じ", 5)]),
                &options(),
                &mut top1_report,
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(top1_report.already_top1, 1);
    }

    #[test]
    fn rejects_a_phonetic_winner_that_only_changes_script_preference() {
        let mut report = Report::default();
        assert!(
            select_rules(
                &Dictionary::bundled(),
                counts("ホームページが", "でき", &[("でき", 8)]),
                &options(),
                &mut report,
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(report.phonetic_winner, 1);
    }

    #[test]
    fn parser_errors_do_not_echo_private_content() {
        let private_value = "非公開顧客語";
        let error = parse_annotated_line(private_value, 7).unwrap_err();
        assert!(error.contains("line 7"));
        assert!(!error.contains(private_value));
    }

    #[test]
    fn normalized_line_hash_is_stable_across_kana_forms() {
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
    fn excludes_held_out_and_duplicate_lines_before_counting() {
        let directory = test_directory();
        let training = directory.join("private-training.txt");
        fs::write(
            &training,
            "長文/ちょうぶん 文章/ぶんしょう 漢字/かんじ\n\
             長文/ちょうぶん 文章/ぶんしょう 漢字/かんじ\n\
             短文/たんぶん 文章/ぶんしょう 漢字/かんじ\n",
        )
        .unwrap();
        let excluded_tokens =
            parse_annotated_line("短文/たんぶん 文章/ぶんしょう 漢字/かんじ", 1).unwrap();
        let exclusions = HashSet::from([hash_tokens(&excluded_tokens)]);
        let mut bytes = 0;
        let (counts, report) = load_observations(&[training], &exclusions, &mut bytes).unwrap();
        assert_eq!(report.accepted_lines, 1);
        assert_eq!(report.duplicate_lines, 1);
        assert_eq!(report.excluded_lines, 1);
        assert_eq!(counts[&("文章".to_owned(), "かんじ".to_owned())]["漢字"], 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_output_never_replaces_an_existing_file() {
        let directory = test_directory();
        let output = directory.join("rules.tsv");
        fs::write(&output, "keep").unwrap();
        assert_eq!(
            write_private_atomic(&output, b"replace", "slime-context-rules").unwrap_err(),
            "output already exists"
        );
        assert_eq!(fs::read_to_string(&output).unwrap(), "keep");
        fs::remove_dir_all(directory).unwrap();
    }

    fn test_directory() -> PathBuf {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "slime-context-rule-test-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
}
