//! Classifies candidate recall without embedding the evaluated vocabulary.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use serde::Serialize;
use slime_converter::{Dictionary, DictionaryEntry, DictionaryLayer};

const COMPOUND_ENTRIES_PER_SEGMENT: usize = 8;
const COMPOUND_CANDIDATE_LIMIT: usize = 32;
const FIXED_SEGMENT_ENTRIES_PER_SEGMENT: usize = 8;
const FIXED_SEGMENT_CANDIDATE_LIMIT: usize = 22;
const MAX_EXPANDED_READING_CHARACTERS: usize = 8;

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
    let items = load_items(&options.input)?;
    let layers = options
        .dictionaries
        .iter()
        .enumerate()
        .map(|(index, path)| load_dictionary_layer(path, index))
        .collect::<Result<Vec<_>, _>>()?;
    let dictionary_bytes = options.dictionaries.iter().try_fold(0_u64, |total, path| {
        let bytes = fs::metadata(path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?
            .len();
        total
            .checked_add(bytes)
            .ok_or_else(|| "external dictionary byte size overflowed".to_owned())
    })?;
    let report = if layers.is_empty() {
        evaluate(&Dictionary::bundled(), &items)
    } else {
        let baseline = evaluate(&Dictionary::bundled(), &items);
        evaluate(&Dictionary::bundled_with_layers(layers), &items)
            .with_dictionary_baseline(&baseline)
    }
    .with_dictionary_bytes(dictionary_bytes);

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report.output(options.details))
                .map_err(|error| format!("failed to serialize report: {error}"))?
        );
    } else {
        print_report(&report, options.details.unwrap_or(20));
    }

    enforce_thresholds(&options, &report)
}

fn enforce_thresholds(options: &Options, report: &RecallReport) -> Result<(), String> {
    if options
        .max_missing
        .is_some_and(|maximum| report.missing > maximum)
    {
        return Err(format!(
            "missing candidate count {} exceeds --max-missing {}",
            report.missing,
            options.max_missing.expect("checked above")
        ));
    }
    if options
        .min_recovered
        .is_some_and(|minimum| report.dictionary_recovered < minimum)
    {
        return Err(format!(
            "external dictionary recovered {} candidates, below --min-recovered {}",
            report.dictionary_recovered,
            options.min_recovered.expect("checked above")
        ));
    }
    if options
        .max_regressed
        .is_some_and(|maximum| report.dictionary_regressed > maximum)
    {
        return Err(format!(
            "external dictionary regressed {} candidates, exceeding --max-regressed {}",
            report.dictionary_regressed,
            options.max_regressed.expect("checked above")
        ));
    }
    if options
        .max_top1_regressed
        .is_some_and(|maximum| report.dictionary_top1_regressed > maximum)
    {
        return Err(format!(
            "external dictionary regressed {} top-1 candidates, exceeding --max-top1-regressed {}",
            report.dictionary_top1_regressed,
            options.max_top1_regressed.expect("checked above")
        ));
    }
    if options
        .max_top1_changed
        .is_some_and(|maximum| report.dictionary_top1_changed > maximum)
    {
        return Err(format!(
            "external dictionary changed {} top-1 surfaces, exceeding --max-top1-changed {}",
            report.dictionary_top1_changed,
            options.max_top1_changed.expect("checked above")
        ));
    }
    if options
        .max_p95_ms
        .is_some_and(|maximum| report.initial_latency_ms.p95 > maximum)
    {
        return Err(format!(
            "initial candidate generation p95 {:.3} ms exceeds --max-p95-ms {:.3}",
            report.initial_latency_ms.p95,
            options.max_p95_ms.expect("checked above")
        ));
    }
    if options
        .max_dictionary_bytes
        .is_some_and(|maximum| report.dictionary_bytes > maximum)
    {
        return Err(format!(
            "external dictionaries total {} bytes, exceeding --max-dictionary-bytes {}",
            report.dictionary_bytes,
            options.max_dictionary_bytes.expect("checked above")
        ));
    }
    Ok(())
}

const fn usage() -> &'static str {
    "usage: slime-recall --input PATH [--dictionary PATH ...] [--details N] \
     [--max-missing N] [--min-recovered N] [--max-regressed N] \
     [--max-top1-regressed N] [--max-top1-changed N] [--max-p95-ms N] \
     [--max-dictionary-bytes N] [--json]\n\
     input format: reading<TAB>expected_surface\n\
     dictionary format: reading<TAB>surface[<TAB>cost]"
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    dictionaries: Vec<PathBuf>,
    details: Option<usize>,
    max_missing: Option<usize>,
    min_recovered: Option<usize>,
    max_regressed: Option<usize>,
    max_top1_regressed: Option<usize>,
    max_top1_changed: Option<usize>,
    max_p95_ms: Option<f64>,
    max_dictionary_bytes: Option<u64>,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut input = None;
        let mut dictionaries = Vec::new();
        let mut details = None;
        let mut max_missing = None;
        let mut min_recovered = None;
        let mut max_regressed = None;
        let mut max_top1_regressed = None;
        let mut max_top1_changed = None;
        let mut max_p95_ms = None;
        let mut max_dictionary_bytes = None;
        let mut json = false;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--input" => {
                    input = Some(PathBuf::from(
                        arguments.next().ok_or("--input requires PATH")?,
                    ));
                }
                "--dictionary" => dictionaries.push(PathBuf::from(
                    arguments.next().ok_or("--dictionary requires PATH")?,
                )),
                "--details" => {
                    details = Some(parse_usize("--details", arguments.next())?);
                }
                "--max-missing" => {
                    max_missing = Some(parse_usize("--max-missing", arguments.next())?);
                }
                "--min-recovered" => {
                    min_recovered = Some(parse_usize("--min-recovered", arguments.next())?);
                }
                "--max-regressed" => {
                    max_regressed = Some(parse_usize("--max-regressed", arguments.next())?);
                }
                "--max-top1-regressed" => {
                    max_top1_regressed =
                        Some(parse_usize("--max-top1-regressed", arguments.next())?);
                }
                "--max-top1-changed" => {
                    max_top1_changed = Some(parse_usize("--max-top1-changed", arguments.next())?);
                }
                "--max-p95-ms" => {
                    max_p95_ms = Some(parse_non_negative_f64("--max-p95-ms", arguments.next())?);
                }
                "--max-dictionary-bytes" => {
                    max_dictionary_bytes =
                        Some(parse_u64("--max-dictionary-bytes", arguments.next())?);
                }
                "--json" => json = true,
                "--help" | "-h" => return Err(usage().to_owned()),
                _ => return Err(format!("unknown argument {argument:?}\n{}", usage())),
            }
        }

        Ok(Self {
            input: input.ok_or_else(|| usage().to_owned())?,
            dictionaries,
            details,
            max_missing,
            min_recovered,
            max_regressed,
            max_top1_regressed,
            max_top1_changed,
            max_p95_ms,
            max_dictionary_bytes,
            json,
        })
    }
}

fn parse_usize(option: &str, value: Option<String>) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{option} requires N"))?;
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer, got {value:?}"))
}

fn parse_u64(option: &str, value: Option<String>) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("{option} requires N"))?;
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer, got {value:?}"))
}

fn parse_non_negative_f64(option: &str, value: Option<String>) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("{option} requires N"))?;
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("{option} requires a non-negative number, got {value:?}"))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!(
            "{option} requires a non-negative number, got {value:?}"
        ));
    }
    Ok(parsed)
}

#[derive(Debug)]
struct RecallItem {
    reading: String,
    surface: String,
}

fn load_items(path: &Path) -> Result<Vec<RecallItem>, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    load_items_from_reader(BufReader::new(file))
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn load_dictionary_layer(path: &Path, index: usize) -> Result<DictionaryLayer, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let entries = load_dictionary_entries(BufReader::new(file))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(DictionaryLayer::new(
        format!("external-{index}"),
        "External evaluation dictionary",
        entries,
    ))
}

fn load_dictionary_entries(reader: impl BufRead) -> Result<Vec<DictionaryEntry>, String> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| format!("line {}: {error}", index + 1))?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let reading = columns
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("line {} has an empty reading", index + 1))?;
        let surface = columns
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("line {} has an empty surface", index + 1))?;
        let cost = columns.next().map_or(Ok(5_000), |value| {
            value
                .parse::<i32>()
                .map_err(|_| format!("line {} has an invalid cost", index + 1))
        })?;
        if columns.next().is_some() {
            return Err(format!("line {} has more than three columns", index + 1));
        }
        if !(0..=100_000).contains(&cost) {
            return Err(format!(
                "line {} cost {cost} is outside 0..=100000",
                index + 1
            ));
        }
        let reading = normalize_reading(reading);
        let key = format!("{reading}\0{surface}");
        if !seen.insert(key) {
            return Err(format!(
                "line {} duplicates an earlier reading and surface pair",
                index + 1
            ));
        }
        entries.push(DictionaryEntry::new(reading, surface, cost));
    }
    if entries.is_empty() {
        return Err("dictionary contains no entries".to_owned());
    }
    Ok(entries)
}

fn load_items_from_reader(reader: impl BufRead) -> Result<Vec<RecallItem>, String> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|error| format!("line {}: {error}", index + 1))?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let reading = columns
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("line {} has an empty reading", index + 1))?;
        let surface = columns
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("line {} has an empty surface", index + 1))?;
        if columns.next().is_some() {
            return Err(format!("line {} has more than two columns", index + 1));
        }
        let reading = normalize_reading(reading);
        let key = format!("{reading}\0{surface}");
        if !seen.insert(key) {
            return Err(format!(
                "line {} duplicates an earlier reading and surface pair",
                index + 1
            ));
        }
        items.push(RecallItem {
            reading,
            surface: surface.to_owned(),
        });
    }
    if items.is_empty() {
        return Err("input contains no recall items".to_owned());
    }
    Ok(items)
}

fn normalize_reading(reading: &str) -> String {
    reading
        .chars()
        .map(|character| {
            if ('ァ'..='ヶ').contains(&character) {
                char::from_u32(u32::from(character) - 0x60).unwrap_or(character)
            } else {
                character
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RecallStage {
    Initial,
    Expanded,
    Compound,
    FixedSegment,
    KnownComponents,
    Missing,
}

#[derive(Debug, Serialize)]
struct ItemResult {
    reading: String,
    surface: String,
    stage: RecallStage,
    top1_surface: Option<String>,
    top1_correct: bool,
    initial_latency_ms: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct LatencyReport {
    p50: f64,
    p95: f64,
    max: f64,
}

#[derive(Debug, Serialize)]
struct RecallReport {
    total: usize,
    initial: usize,
    expanded: usize,
    compound: usize,
    fixed_segment: usize,
    known_components: usize,
    unknown_components: usize,
    missing: usize,
    baseline_missing: usize,
    dictionary_recovered: usize,
    dictionary_regressed: usize,
    top1_correct: usize,
    baseline_top1_correct: usize,
    dictionary_top1_improved: usize,
    dictionary_top1_regressed: usize,
    dictionary_top1_changed: usize,
    initial_latency_ms: LatencyReport,
    baseline_initial_latency_ms: LatencyReport,
    dictionary_bytes: u64,
    results: Vec<ItemResult>,
}

#[derive(Debug, Serialize)]
struct RecallReportOutput<'a> {
    total: usize,
    initial: usize,
    expanded: usize,
    compound: usize,
    fixed_segment: usize,
    known_components: usize,
    unknown_components: usize,
    missing: usize,
    baseline_missing: usize,
    dictionary_recovered: usize,
    dictionary_regressed: usize,
    top1_correct: usize,
    baseline_top1_correct: usize,
    dictionary_top1_improved: usize,
    dictionary_top1_regressed: usize,
    dictionary_top1_changed: usize,
    initial_latency_ms: LatencyReport,
    baseline_initial_latency_ms: LatencyReport,
    dictionary_bytes: u64,
    results: Vec<&'a ItemResult>,
}

impl RecallReport {
    fn output(&self, details: Option<usize>) -> RecallReportOutput<'_> {
        let limit = details.unwrap_or(self.results.len());
        RecallReportOutput {
            total: self.total,
            initial: self.initial,
            expanded: self.expanded,
            compound: self.compound,
            fixed_segment: self.fixed_segment,
            known_components: self.known_components,
            unknown_components: self.unknown_components,
            missing: self.missing,
            baseline_missing: self.baseline_missing,
            dictionary_recovered: self.dictionary_recovered,
            dictionary_regressed: self.dictionary_regressed,
            top1_correct: self.top1_correct,
            baseline_top1_correct: self.baseline_top1_correct,
            dictionary_top1_improved: self.dictionary_top1_improved,
            dictionary_top1_regressed: self.dictionary_top1_regressed,
            dictionary_top1_changed: self.dictionary_top1_changed,
            initial_latency_ms: self.initial_latency_ms,
            baseline_initial_latency_ms: self.baseline_initial_latency_ms,
            dictionary_bytes: self.dictionary_bytes,
            results: self.results.iter().take(limit).collect(),
        }
    }

    const fn with_dictionary_bytes(mut self, dictionary_bytes: u64) -> Self {
        self.dictionary_bytes = dictionary_bytes;
        self
    }

    fn with_dictionary_baseline(mut self, baseline: &Self) -> Self {
        assert_eq!(self.results.len(), baseline.results.len());
        self.baseline_missing = baseline.missing;
        self.baseline_top1_correct = baseline.top1_correct;
        self.baseline_initial_latency_ms = baseline.initial_latency_ms;
        for (current, original) in self.results.iter().zip(&baseline.results) {
            if is_unrecalled(original.stage) && !is_unrecalled(current.stage) {
                self.dictionary_recovered += 1;
            } else if !is_unrecalled(original.stage) && is_unrecalled(current.stage) {
                self.dictionary_regressed += 1;
            }
            if !original.top1_correct && current.top1_correct {
                self.dictionary_top1_improved += 1;
            } else if original.top1_correct && !current.top1_correct {
                self.dictionary_top1_regressed += 1;
            }
            if original.top1_surface != current.top1_surface {
                self.dictionary_top1_changed += 1;
            }
        }
        self
    }
}

fn evaluate(dictionary: &Dictionary, items: &[RecallItem]) -> RecallReport {
    let results: Vec<_> = items
        .iter()
        .map(|item| {
            let started = Instant::now();
            let initial = dictionary.candidates(&item.reading);
            let initial_latency_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let top1_surface = initial.first().map(|candidate| candidate.surface.clone());
            let top1_correct = top1_surface.as_deref() == Some(item.surface.as_str());
            let reading_characters = item.reading.chars().count();
            let expanded = if reading_characters <= MAX_EXPANDED_READING_CHARACTERS {
                dictionary.candidates_with_limit(&item.reading, 32)
            } else {
                Vec::new()
            };
            let compound = dictionary.compound_candidates(
                &item.reading,
                COMPOUND_ENTRIES_PER_SEGMENT,
                COMPOUND_CANDIDATE_LIMIT,
            );
            let fixed_segment = if reading_characters > MAX_EXPANDED_READING_CHARACTERS {
                dictionary.fixed_segment_variants(
                    &item.reading,
                    FIXED_SEGMENT_ENTRIES_PER_SEGMENT,
                    FIXED_SEGMENT_CANDIDATE_LIMIT,
                )
            } else {
                Vec::new()
            };
            let stage = classify(
                &item.surface,
                initial.iter().map(|candidate| candidate.surface.as_str()),
                expanded.iter().map(|candidate| candidate.surface.as_str()),
                compound.iter().map(|candidate| candidate.surface.as_str()),
                fixed_segment.iter().map(String::as_str),
            );
            let stage = if stage == RecallStage::Missing
                && dictionary.is_exact_compound_surface(&item.reading, &item.surface)
            {
                RecallStage::KnownComponents
            } else {
                stage
            };
            ItemResult {
                reading: item.reading.clone(),
                surface: item.surface.clone(),
                stage,
                top1_surface,
                top1_correct,
                initial_latency_ms,
            }
        })
        .collect();
    let known_components = count_stage(&results, RecallStage::KnownComponents);
    let unknown_components = count_stage(&results, RecallStage::Missing);
    let missing = known_components + unknown_components;
    let top1_correct = results.iter().filter(|result| result.top1_correct).count();
    let initial_latency_ms = latency_report(
        results
            .iter()
            .map(|result| result.initial_latency_ms)
            .collect(),
    );
    RecallReport {
        total: results.len(),
        initial: count_stage(&results, RecallStage::Initial),
        expanded: count_stage(&results, RecallStage::Expanded),
        compound: count_stage(&results, RecallStage::Compound),
        fixed_segment: count_stage(&results, RecallStage::FixedSegment),
        known_components,
        unknown_components,
        missing,
        baseline_missing: missing,
        dictionary_recovered: 0,
        dictionary_regressed: 0,
        top1_correct,
        baseline_top1_correct: top1_correct,
        dictionary_top1_improved: 0,
        dictionary_top1_regressed: 0,
        dictionary_top1_changed: 0,
        initial_latency_ms,
        baseline_initial_latency_ms: initial_latency_ms,
        dictionary_bytes: 0,
        results,
    }
}

fn latency_report(mut milliseconds: Vec<f64>) -> LatencyReport {
    milliseconds.sort_by(f64::total_cmp);
    LatencyReport {
        p50: percentile(&milliseconds, 50),
        p95: percentile(&milliseconds, 95),
        max: milliseconds.last().copied().unwrap_or(0.0),
    }
}

fn percentile(sorted: &[f64], percentile: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (sorted.len() * percentile).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn classify<'a>(
    expected: &str,
    initial: impl Iterator<Item = &'a str>,
    expanded: impl Iterator<Item = &'a str>,
    compound: impl Iterator<Item = &'a str>,
    fixed_segment: impl Iterator<Item = &'a str>,
) -> RecallStage {
    if initial.into_iter().any(|surface| surface == expected) {
        RecallStage::Initial
    } else if expanded.into_iter().any(|surface| surface == expected) {
        RecallStage::Expanded
    } else if compound.into_iter().any(|surface| surface == expected) {
        RecallStage::Compound
    } else if fixed_segment.into_iter().any(|surface| surface == expected) {
        RecallStage::FixedSegment
    } else {
        RecallStage::Missing
    }
}

const fn is_unrecalled(stage: RecallStage) -> bool {
    matches!(stage, RecallStage::KnownComponents | RecallStage::Missing)
}

fn count_stage(results: &[ItemResult], stage: RecallStage) -> usize {
    results
        .iter()
        .filter(|result| result.stage == stage)
        .count()
}

fn print_report(report: &RecallReport, details: usize) {
    println!("candidate recall:");
    println!("  total: {}", report.total);
    println!("  initial: {}", report.initial);
    println!("  expanded: {}", report.expanded);
    println!("  compound: {}", report.compound);
    println!("  fixed segment: {}", report.fixed_segment);
    println!("  known components: {}", report.known_components);
    println!("  unknown components: {}", report.unknown_components);
    println!("  missing: {}", report.missing);
    println!("  baseline missing: {}", report.baseline_missing);
    println!(
        "  external dictionary recovered: {}",
        report.dictionary_recovered
    );
    println!(
        "  external dictionary regressed: {}",
        report.dictionary_regressed
    );
    println!("  top-1 correct: {}", report.top1_correct);
    println!("  baseline top-1 correct: {}", report.baseline_top1_correct);
    println!(
        "  external dictionary top-1 improved: {}",
        report.dictionary_top1_improved
    );
    println!(
        "  external dictionary top-1 regressed: {}",
        report.dictionary_top1_regressed
    );
    println!(
        "  external dictionary top-1 changed: {}",
        report.dictionary_top1_changed
    );
    println!(
        "  initial latency ms: p50={:.3} p95={:.3} max={:.3}",
        report.initial_latency_ms.p50, report.initial_latency_ms.p95, report.initial_latency_ms.max
    );
    println!(
        "  baseline initial latency ms: p50={:.3} p95={:.3} max={:.3}",
        report.baseline_initial_latency_ms.p50,
        report.baseline_initial_latency_ms.p95,
        report.baseline_initial_latency_ms.max
    );
    println!("  external dictionary bytes: {}", report.dictionary_bytes);
    for result in report
        .results
        .iter()
        .filter(|result| result.stage != RecallStage::Initial)
        .take(details)
    {
        println!(
            "  {:?}\t{}\t{}",
            result.stage, result.reading, result.surface
        );
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use slime_converter::DictionaryEntry;

    use super::*;

    #[test]
    fn parser_normalizes_readings_and_rejects_duplicates() {
        let items = load_items_from_reader(Cursor::new("ニホン\t日本\n")).unwrap();
        assert_eq!(items[0].reading, "にほん");
        assert_eq!(items[0].surface, "日本");

        let duplicate = load_items_from_reader(Cursor::new("にほん\t日本\nニホン\t日本\n"));
        let error = duplicate.unwrap_err();
        assert!(error.contains("duplicates"));
        assert!(!error.contains("にほん"));
        assert!(!error.contains("日本"));
    }

    #[test]
    fn external_dictionary_parser_accepts_optional_costs() {
        let entries = load_dictionary_entries(Cursor::new(
            "# fixture\nニホン\t日本\nあさいり\t浅煎り\t700\n",
        ))
        .unwrap();
        let dictionary = Dictionary::bundled_with_layers(vec![DictionaryLayer::new(
            "fixture", "Fixture", entries,
        )]);

        assert!(dictionary.has_exact_reading("にほん"));
        assert!(
            dictionary
                .candidates("あさいり")
                .iter()
                .any(|candidate| candidate.surface == "浅煎り")
        );

        let private_value = "顧客専用語";
        let invalid = load_dictionary_entries(Cursor::new(format!(
            "ひみつ\t{private_value}\t{private_value}\n"
        )))
        .unwrap_err();
        assert!(invalid.contains("invalid cost"));
        assert!(!invalid.contains(private_value));
    }

    #[test]
    fn classification_keeps_stages_distinct() {
        assert_eq!(
            classify(
                "正解",
                ["正解"].into_iter(),
                [].into_iter(),
                [].into_iter(),
                [].into_iter()
            ),
            RecallStage::Initial
        );
        assert_eq!(
            classify(
                "正解",
                ["別候補"].into_iter(),
                ["正解"].into_iter(),
                [].into_iter(),
                [].into_iter()
            ),
            RecallStage::Expanded
        );
        assert_eq!(
            classify(
                "正解",
                ["別候補"].into_iter(),
                ["別候補"].into_iter(),
                ["正解"].into_iter(),
                [].into_iter()
            ),
            RecallStage::Compound
        );
        assert_eq!(
            classify(
                "正解",
                ["別候補"].into_iter(),
                ["別候補"].into_iter(),
                ["別候補"].into_iter(),
                ["正解"].into_iter()
            ),
            RecallStage::FixedSegment
        );
        assert_eq!(
            classify(
                "正解",
                ["別候補"].into_iter(),
                ["別候補"].into_iter(),
                ["別候補"].into_iter(),
                ["別候補"].into_iter()
            ),
            RecallStage::Missing
        );
    }

    #[test]
    fn evaluator_distinguishes_unrecalled_known_components() {
        let mut entries = Vec::new();
        for (reading, prefix) in [("あい", "左"), ("うえ", "右")] {
            for index in 0..6 {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    index * 100,
                ));
            }
        }
        let item = RecallItem {
            reading: "あいうえ".to_owned(),
            surface: "左5右5".to_owned(),
        };
        let report = evaluate(
            &Dictionary::new(entries.clone()),
            std::slice::from_ref(&item),
        );

        assert_eq!(report.known_components, 1);
        assert_eq!(report.unknown_components, 0);
        assert_eq!(report.missing, 1);
        assert_eq!(report.results[0].stage, RecallStage::KnownComponents);

        entries.push(DictionaryEntry::new("あいうえ", "左5右5", 10));
        let recovered =
            evaluate(&Dictionary::new(entries), &[item]).with_dictionary_baseline(&report);
        assert_eq!(recovered.missing, 0);
        assert_eq!(recovered.baseline_missing, 1);
        assert_eq!(recovered.dictionary_recovered, 1);
        assert_eq!(recovered.dictionary_regressed, 0);
    }

    #[test]
    fn evaluator_counts_initial_and_missing_candidates() {
        let dictionary = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 10),
            DictionaryEntry::new("うえ", "第二", 20),
        ]);
        let items = vec![
            RecallItem {
                reading: "あい".to_owned(),
                surface: "第一".to_owned(),
            },
            RecallItem {
                reading: "あいうえ".to_owned(),
                surface: "第一第二".to_owned(),
            },
            RecallItem {
                reading: "おか".to_owned(),
                surface: "第三".to_owned(),
            },
        ];

        let report = evaluate(&dictionary, &items);
        assert_eq!(report.initial, 2);
        assert_eq!(report.compound, 0);
        assert_eq!(report.missing, 1);
        assert_eq!(report.baseline_missing, 1);
        assert_eq!(report.dictionary_recovered, 0);
        assert_eq!(report.dictionary_regressed, 0);
        assert_eq!(report.top1_correct, 2);
        assert_eq!(report.baseline_top1_correct, 2);
        assert_eq!(report.dictionary_top1_improved, 0);
        assert_eq!(report.dictionary_top1_regressed, 0);
        assert_eq!(report.dictionary_top1_changed, 0);
        assert!(report.initial_latency_ms.p95 >= 0.0);
    }

    #[test]
    fn external_dictionary_delta_counts_recovery_and_regression() {
        let items = vec![
            RecallItem {
                reading: "あい".to_owned(),
                surface: "第一".to_owned(),
            },
            RecallItem {
                reading: "うえ".to_owned(),
                surface: "第二".to_owned(),
            },
        ];
        let baseline = Dictionary::new(vec![DictionaryEntry::new("あい", "第一", 500)]);
        let layered = Dictionary::new(vec![
            DictionaryEntry::new("あい", "第一", 500),
            DictionaryEntry::new("うえ", "第二", 10),
        ]);

        let baseline_report = evaluate(&baseline, &items);
        let recovered = evaluate(&layered, &items).with_dictionary_baseline(&baseline_report);

        assert_eq!(recovered.baseline_missing, 1);
        assert_eq!(recovered.missing, 0);
        assert_eq!(recovered.dictionary_recovered, 1);
        assert_eq!(recovered.dictionary_regressed, 0);
        assert_eq!(recovered.dictionary_top1_improved, 1);
        assert_eq!(recovered.dictionary_top1_regressed, 0);
        assert_eq!(recovered.dictionary_top1_changed, 1);

        let mut regressed = evaluate(&layered, &items);
        regressed.results[0].stage = RecallStage::Missing;
        regressed.results[0].top1_surface = Some("別候補".to_owned());
        regressed.results[0].top1_correct = false;
        regressed.initial -= 1;
        regressed.missing += 1;
        regressed.top1_correct -= 1;
        let regressed = regressed.with_dictionary_baseline(&baseline_report);
        assert_eq!(regressed.dictionary_recovered, 1);
        assert_eq!(regressed.dictionary_regressed, 1);
        assert_eq!(regressed.dictionary_top1_improved, 1);
        assert_eq!(regressed.dictionary_top1_regressed, 1);
        assert_eq!(regressed.dictionary_top1_changed, 2);
    }

    #[test]
    fn evaluator_counts_long_fixed_segment_candidates() {
        let mut entries = Vec::new();
        for (reading, prefix) in [
            ("あいうえおか", "第一"),
            ("きくけこさし", "第二"),
            ("すせそたちつ", "第三"),
        ] {
            for (index, cost) in [10, 20, 30, 40].into_iter().enumerate() {
                entries.push(DictionaryEntry::new(
                    reading,
                    format!("{prefix}{index}"),
                    cost,
                ));
            }
        }
        let dictionary = Dictionary::new(entries);
        let reading = "あいうえおかきくけこさしすせそたちつ";
        let initial = dictionary.candidates(reading);
        let surface = dictionary
            .fixed_segment_variants(
                reading,
                FIXED_SEGMENT_ENTRIES_PER_SEGMENT,
                FIXED_SEGMENT_CANDIDATE_LIMIT,
            )
            .into_iter()
            .find(|surface| {
                !initial
                    .iter()
                    .any(|candidate| candidate.surface == *surface)
            })
            .expect("fixture should include a fixed-segment-only candidate");
        let report = evaluate(
            &dictionary,
            &[RecallItem {
                reading: reading.to_owned(),
                surface,
            }],
        );

        assert_eq!(report.fixed_segment, 1);
        assert_eq!(report.missing, 0);
    }

    #[test]
    fn explicit_json_detail_limit_can_hide_private_vocabulary() {
        let dictionary = Dictionary::new(vec![DictionaryEntry::new("ひみつ", "秘密", 10)]);
        let items = vec![RecallItem {
            reading: "ひみつ".to_owned(),
            surface: "秘密".to_owned(),
        }];
        let report = evaluate(&dictionary, &items);

        let private_output = serde_json::to_string(&report.output(None)).unwrap();
        assert!(private_output.contains("ひみつ"));
        assert!(private_output.contains("秘密"));

        let summary_only = serde_json::to_string(&report.output(Some(0))).unwrap();
        assert!(!summary_only.contains("ひみつ"));
        assert!(!summary_only.contains("秘密"));
        assert!(summary_only.contains("\"total\":1"));
        assert!(summary_only.contains("\"results\":[]"));
    }
}
