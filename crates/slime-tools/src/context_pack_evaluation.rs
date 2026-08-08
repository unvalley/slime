//! Evaluates an installed context pack without logging private vocabulary.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use serde::Serialize;
use slime_core::{SlimeEngine, UserData};

const MAX_CASES: usize = 100_000;
const MAX_SURFACE_CHARACTERS: usize = 128;
const MAX_TOP_K: usize = 1_000;

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
    let cases = load_case_files(&options.input_paths)?;
    let report = evaluate_from_directories(
        options.baseline_data_directory.as_deref(),
        &options.data_directory,
        &cases,
        options.top_k,
    )?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("failed to serialize report: {error}"))?
        );
    } else {
        print_report(&report);
    }
    enforce_thresholds(&options, &report)
}

const fn usage() -> &'static str {
    "usage: slime-context-pack-evaluate --data-dir PATH [--baseline-data-dir PATH] --input PATH \
     [--input PATH ...] [--top-k N] [--min-context-rules N] \
     [--min-added-context-rules N] \
     [--min-top1-improved N] [--max-top1-regressed N] \
     [--max-topk-regressed N] [--max-top1-changed N] \
     [--min-accuracy-delta N] [--min-mrr-delta N] \
     [--max-p95-ms N] [--max-pack-load-ms N] [--max-pack-bytes N] [--json]\n\
     input format: previous_surface<TAB>reading<TAB>expected_surface"
}

struct Options {
    data_directory: PathBuf,
    baseline_data_directory: Option<PathBuf>,
    input_paths: Vec<PathBuf>,
    top_k: usize,
    min_context_rules: Option<usize>,
    min_added_context_rules: Option<usize>,
    min_top1_improved: Option<usize>,
    max_top1_regressed: Option<usize>,
    max_topk_regressed: Option<usize>,
    max_top1_changed: Option<usize>,
    min_accuracy_delta: Option<f64>,
    min_mrr_delta: Option<f64>,
    max_p95_ms: Option<f64>,
    max_pack_load_ms: Option<f64>,
    max_pack_bytes: Option<u64>,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut builder = OptionBuilder::default();
        while let Some(argument) = arguments.next() {
            builder.parse_argument(&argument, &mut arguments)?;
        }
        builder.finish()
    }
}

#[derive(Default)]
struct OptionBuilder {
    data_directory: Option<PathBuf>,
    baseline_data_directory: Option<PathBuf>,
    input_paths: Vec<PathBuf>,
    top_k: Option<usize>,
    min_context_rules: Option<usize>,
    min_added_context_rules: Option<usize>,
    min_top1_improved: Option<usize>,
    max_top1_regressed: Option<usize>,
    max_topk_regressed: Option<usize>,
    max_top1_changed: Option<usize>,
    min_accuracy_delta: Option<f64>,
    min_mrr_delta: Option<f64>,
    max_p95_ms: Option<f64>,
    max_pack_load_ms: Option<f64>,
    max_pack_bytes: Option<u64>,
    json: bool,
}

impl OptionBuilder {
    fn parse_argument(
        &mut self,
        argument: &str,
        arguments: &mut impl Iterator<Item = String>,
    ) -> Result<(), String> {
        match argument {
            "--data-dir" => {
                self.data_directory = Some(PathBuf::from(next_value(argument, arguments)?));
            }
            "--baseline-data-dir" => {
                self.baseline_data_directory =
                    Some(PathBuf::from(next_value(argument, arguments)?));
            }
            "--input" => self
                .input_paths
                .push(PathBuf::from(next_value(argument, arguments)?)),
            "--top-k" => self.top_k = Some(parse_positive_usize(argument, arguments)?),
            "--min-context-rules" => {
                self.min_context_rules = Some(parse_usize(argument, arguments)?);
            }
            "--min-added-context-rules" => {
                self.min_added_context_rules = Some(parse_usize(argument, arguments)?);
            }
            "--min-top1-improved" => {
                self.min_top1_improved = Some(parse_usize(argument, arguments)?);
            }
            "--max-top1-regressed" => {
                self.max_top1_regressed = Some(parse_usize(argument, arguments)?);
            }
            "--max-topk-regressed" => {
                self.max_topk_regressed = Some(parse_usize(argument, arguments)?);
            }
            "--max-top1-changed" => {
                self.max_top1_changed = Some(parse_usize(argument, arguments)?);
            }
            "--min-accuracy-delta" => {
                self.min_accuracy_delta = Some(parse_f64(argument, arguments)?);
            }
            "--min-mrr-delta" => self.min_mrr_delta = Some(parse_f64(argument, arguments)?),
            "--max-p95-ms" => self.max_p95_ms = Some(parse_non_negative(argument, arguments)?),
            "--max-pack-load-ms" => {
                self.max_pack_load_ms = Some(parse_non_negative(argument, arguments)?);
            }
            "--max-pack-bytes" => {
                self.max_pack_bytes = Some(parse_u64(argument, arguments)?);
            }
            "--json" => self.json = true,
            "--help" | "-h" => return Err(usage().to_owned()),
            _ => return Err(format!("unknown argument\n{}", usage())),
        }
        Ok(())
    }

    fn finish(self) -> Result<Options, String> {
        let data_directory = self.data_directory.ok_or_else(|| usage().to_owned())?;
        if self.input_paths.is_empty() {
            return Err(usage().to_owned());
        }
        let top_k = self.top_k.unwrap_or(10);
        if top_k > MAX_TOP_K {
            return Err(format!("--top-k cannot exceed {MAX_TOP_K}"));
        }
        Ok(Options {
            data_directory,
            baseline_data_directory: self.baseline_data_directory,
            input_paths: self.input_paths,
            top_k,
            min_context_rules: self.min_context_rules,
            min_added_context_rules: self.min_added_context_rules,
            min_top1_improved: self.min_top1_improved,
            max_top1_regressed: self.max_top1_regressed,
            max_topk_regressed: self.max_topk_regressed,
            max_top1_changed: self.max_top1_changed,
            min_accuracy_delta: self.min_accuracy_delta,
            min_mrr_delta: self.min_mrr_delta,
            max_p95_ms: self.max_p95_ms,
            max_pack_load_ms: self.max_pack_load_ms,
            max_pack_bytes: self.max_pack_bytes,
            json: self.json,
        })
    }
}

fn next_value(
    option: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_usize(
    option: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<usize, String> {
    next_value(option, arguments)?
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer"))
}

fn parse_positive_usize(
    option: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<usize, String> {
    let value = parse_usize(option, arguments)?;
    if value == 0 {
        return Err(format!("{option} must be greater than zero"));
    }
    Ok(value)
}

fn parse_u64(option: &str, arguments: &mut impl Iterator<Item = String>) -> Result<u64, String> {
    next_value(option, arguments)?
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer"))
}

fn parse_f64(option: &str, arguments: &mut impl Iterator<Item = String>) -> Result<f64, String> {
    let value = next_value(option, arguments)?
        .parse::<f64>()
        .map_err(|_| format!("{option} requires a number"))?;
    if !value.is_finite() {
        return Err(format!("{option} requires a finite number"));
    }
    Ok(value)
}

fn parse_non_negative(
    option: &str,
    arguments: &mut impl Iterator<Item = String>,
) -> Result<f64, String> {
    let value = parse_f64(option, arguments)?;
    if value < 0.0 {
        return Err(format!("{option} requires a non-negative number"));
    }
    Ok(value)
}

#[derive(Debug)]
struct ContextCase {
    previous_surface: String,
    reading: String,
    expected_surface: String,
}

fn load_case_files(paths: &[PathBuf]) -> Result<Vec<ContextCase>, String> {
    let mut cases = Vec::new();
    let mut seen = HashSet::new();
    for path in paths {
        let file =
            fs::File::open(path).map_err(|error| format!("failed to open input file: {error}"))?;
        load_cases(BufReader::new(file), &mut cases, &mut seen)?;
    }
    if cases.is_empty() {
        return Err("input files contain no cases".to_owned());
    }
    Ok(cases)
}

fn load_cases(
    reader: impl BufRead,
    cases: &mut Vec<ContextCase>,
    seen: &mut HashSet<(String, String)>,
) -> Result<(), String> {
    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(|error| format!("line {line_number}: {error}"))?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if cases.len() == MAX_CASES {
            return Err(format!("inputs exceed the {MAX_CASES} case limit"));
        }
        let mut columns = line.split('\t');
        let previous_surface = required_column(columns.next(), line_number)?;
        let reading = required_column(columns.next(), line_number)?;
        let expected_surface = required_column(columns.next(), line_number)?;
        if columns.next().is_some() {
            return Err(format!("line {line_number} has more than three columns"));
        }
        validate_surface(previous_surface, line_number)?;
        validate_reading(reading, line_number)?;
        validate_surface(expected_surface, line_number)?;
        let key = (previous_surface.to_owned(), reading.to_owned());
        if !seen.insert(key) {
            return Err(format!(
                "line {line_number} duplicates a context and reading"
            ));
        }
        cases.push(ContextCase {
            previous_surface: previous_surface.to_owned(),
            reading: reading.to_owned(),
            expected_surface: expected_surface.to_owned(),
        });
    }
    Ok(())
}

fn required_column(value: Option<&str>, line_number: usize) -> Result<&str, String> {
    value
        .filter(|column| !column.is_empty())
        .ok_or_else(|| format!("line {line_number} has an empty or missing column"))
}

fn validate_surface(surface: &str, line_number: usize) -> Result<(), String> {
    if surface.chars().count() <= MAX_SURFACE_CHARACTERS && !surface.chars().any(char::is_control) {
        Ok(())
    } else {
        Err(format!("line {line_number} has an invalid surface"))
    }
}

fn validate_reading(reading: &str, line_number: usize) -> Result<(), String> {
    if reading.chars().count() <= MAX_SURFACE_CHARACTERS
        && reading
            .chars()
            .all(|character| matches!(character, '\u{3041}'..='\u{3096}' | 'ー'))
    {
        Ok(())
    } else {
        Err(format!("line {line_number} reading must be hiragana"))
    }
}

#[derive(Serialize)]
struct ContextPackReport {
    items: usize,
    top_k: usize,
    baseline_pack_count: usize,
    baseline_entry_count: usize,
    baseline_context_rule_count: usize,
    baseline_pack_bytes: u64,
    pack_count: usize,
    entry_count: usize,
    context_rule_count: usize,
    added_context_rule_count: usize,
    pack_bytes: u64,
    baseline_load_ms: f64,
    pack_load_ms: f64,
    baseline_accuracy_at_1: f64,
    pack_accuracy_at_1: f64,
    accuracy_at_1_delta: f64,
    baseline_accuracy_at_k: f64,
    pack_accuracy_at_k: f64,
    baseline_mrr_at_k: f64,
    pack_mrr_at_k: f64,
    mrr_at_k_delta: f64,
    top1_improved: usize,
    top1_regressed: usize,
    top1_changed: usize,
    topk_recovered: usize,
    topk_regressed: usize,
    baseline_latency_ms: LatencyReport,
    pack_latency_ms: LatencyReport,
}

#[derive(Serialize)]
struct LatencyReport {
    p50: f64,
    p95: f64,
    max: f64,
}

#[derive(Default)]
struct ScoreAccumulator {
    baseline_top1: usize,
    pack_top1: usize,
    baseline_topk: usize,
    pack_topk: usize,
    baseline_mrr: f64,
    pack_mrr: f64,
    top1_improved: usize,
    top1_regressed: usize,
    top1_changed: usize,
    topk_recovered: usize,
    topk_regressed: usize,
    baseline_latencies: Vec<f64>,
    pack_latencies: Vec<f64>,
}

struct PackSummary {
    baseline_pack_count: usize,
    baseline_entry_count: usize,
    baseline_context_rule_count: usize,
    baseline_pack_bytes: u64,
    pack_count: usize,
    entry_count: usize,
    context_rule_count: usize,
    pack_bytes: u64,
    baseline_load_ms: f64,
    pack_load_ms: f64,
}

fn evaluate_from_directories(
    baseline_data_directory: Option<&Path>,
    data_directory: &Path,
    cases: &[ContextCase],
    top_k: usize,
) -> Result<ContextPackReport, String> {
    let baseline_start = Instant::now();
    let (baseline, baseline_pack_count, baseline_entry_count, baseline_context_rule_count) =
        if let Some(directory) = baseline_data_directory {
            load_pack_engine(directory, "baseline")?
        } else {
            (SlimeEngine::bundled(), 0, 0, 0)
        };
    let baseline_load_ms = baseline_start.elapsed().as_secs_f64() * 1_000.0;
    let pack_start = Instant::now();
    let (pack, pack_count, entry_count, context_rule_count) =
        load_pack_engine(data_directory, "candidate")?;
    let pack_load_ms = pack_start.elapsed().as_secs_f64() * 1_000.0;
    let baseline_pack_bytes = baseline_data_directory.map_or(Ok(0), dictionary_pack_bytes)?;
    let pack_bytes = dictionary_pack_bytes(data_directory)?;
    let scores = evaluate_cases(&baseline, &pack, cases, top_k);
    Ok(scores.report(
        cases.len(),
        top_k,
        &PackSummary {
            baseline_pack_count,
            baseline_entry_count,
            baseline_context_rule_count,
            baseline_pack_bytes,
            pack_count,
            entry_count,
            context_rule_count,
            pack_bytes,
            baseline_load_ms,
            pack_load_ms,
        },
    ))
}

fn load_pack_engine(
    data_directory: &Path,
    label: &str,
) -> Result<(SlimeEngine, usize, usize, usize), String> {
    let engine = SlimeEngine::bundled_with_user_data(UserData::load(data_directory));
    if !engine.dictionary_pack_load_errors().is_empty() {
        return Err(format!(
            "{label} dictionary pack loading failed for {} file(s)",
            engine.dictionary_pack_load_errors().len()
        ));
    }
    let (pack_count, entry_count, context_rule_count) =
        engine
            .installed_dictionary_packs()
            .fold((0_usize, 0_usize, 0_usize), |counts, info| {
                (
                    counts.0 + 1,
                    counts.1 + info.entry_count,
                    counts.2 + info.context_rule_count,
                )
            });
    if pack_count == 0 {
        return Err(format!(
            "{label} data directory contains no valid dictionary packs"
        ));
    }
    Ok((engine, pack_count, entry_count, context_rule_count))
}

fn dictionary_pack_bytes(data_directory: &Path) -> Result<u64, String> {
    let directory = data_directory.join("dictionary-packs");
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to inspect dictionary pack directory: {error}"))?;
    let mut bytes = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to inspect dictionary pack: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("slime-dict") {
            continue;
        }
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("failed to inspect dictionary pack: {error}"))?;
        if metadata.file_type().is_file() {
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or_else(|| "dictionary pack byte size overflowed".to_owned())?;
        }
    }
    Ok(bytes)
}

fn evaluate_cases(
    baseline: &SlimeEngine,
    pack: &SlimeEngine,
    cases: &[ContextCase],
    top_k: usize,
) -> ScoreAccumulator {
    let mut scores = ScoreAccumulator {
        baseline_latencies: Vec::with_capacity(cases.len()),
        pack_latencies: Vec::with_capacity(cases.len()),
        ..ScoreAccumulator::default()
    };
    for case in cases {
        let start = Instant::now();
        let baseline_candidates =
            baseline.conversion_candidates_with_left_context(&case.previous_surface, &case.reading);
        scores
            .baseline_latencies
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        let start = Instant::now();
        let pack_candidates =
            pack.conversion_candidates_with_left_context(&case.previous_surface, &case.reading);
        scores
            .pack_latencies
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        scores.record(
            &baseline_candidates,
            &pack_candidates,
            &case.expected_surface,
            top_k,
        );
    }
    scores
}

impl ScoreAccumulator {
    fn record(&mut self, baseline: &[String], pack: &[String], expected: &str, top_k: usize) {
        let baseline_rank = rank(baseline, expected, top_k);
        let pack_rank = rank(pack, expected, top_k);
        self.baseline_top1 += usize::from(baseline_rank == Some(0));
        self.pack_top1 += usize::from(pack_rank == Some(0));
        self.baseline_topk += usize::from(baseline_rank.is_some());
        self.pack_topk += usize::from(pack_rank.is_some());
        self.baseline_mrr += reciprocal_rank(baseline_rank);
        self.pack_mrr += reciprocal_rank(pack_rank);
        self.top1_improved += usize::from(baseline_rank != Some(0) && pack_rank == Some(0));
        self.top1_regressed += usize::from(baseline_rank == Some(0) && pack_rank != Some(0));
        self.top1_changed += usize::from(baseline.first() != pack.first());
        self.topk_recovered += usize::from(baseline_rank.is_none() && pack_rank.is_some());
        self.topk_regressed += usize::from(baseline_rank.is_some() && pack_rank.is_none());
    }

    fn report(mut self, items: usize, top_k: usize, summary: &PackSummary) -> ContextPackReport {
        let denominator = bounded_f64(items);
        let baseline_accuracy_at_1 = bounded_f64(self.baseline_top1) / denominator;
        let pack_accuracy_at_1 = bounded_f64(self.pack_top1) / denominator;
        let baseline_mrr_at_k = self.baseline_mrr / denominator;
        let pack_mrr_at_k = self.pack_mrr / denominator;
        ContextPackReport {
            items,
            top_k,
            baseline_pack_count: summary.baseline_pack_count,
            baseline_entry_count: summary.baseline_entry_count,
            baseline_context_rule_count: summary.baseline_context_rule_count,
            baseline_pack_bytes: summary.baseline_pack_bytes,
            pack_count: summary.pack_count,
            entry_count: summary.entry_count,
            context_rule_count: summary.context_rule_count,
            added_context_rule_count: summary
                .context_rule_count
                .saturating_sub(summary.baseline_context_rule_count),
            pack_bytes: summary.pack_bytes,
            baseline_load_ms: summary.baseline_load_ms,
            pack_load_ms: summary.pack_load_ms,
            baseline_accuracy_at_1,
            pack_accuracy_at_1,
            accuracy_at_1_delta: pack_accuracy_at_1 - baseline_accuracy_at_1,
            baseline_accuracy_at_k: bounded_f64(self.baseline_topk) / denominator,
            pack_accuracy_at_k: bounded_f64(self.pack_topk) / denominator,
            baseline_mrr_at_k,
            pack_mrr_at_k,
            mrr_at_k_delta: pack_mrr_at_k - baseline_mrr_at_k,
            top1_improved: self.top1_improved,
            top1_regressed: self.top1_regressed,
            top1_changed: self.top1_changed,
            topk_recovered: self.topk_recovered,
            topk_regressed: self.topk_regressed,
            baseline_latency_ms: latency_report(&mut self.baseline_latencies),
            pack_latency_ms: latency_report(&mut self.pack_latencies),
        }
    }
}

fn rank(candidates: &[String], expected: &str, top_k: usize) -> Option<usize> {
    candidates
        .iter()
        .take(top_k)
        .position(|candidate| candidate == expected)
}

fn reciprocal_rank(rank: Option<usize>) -> f64 {
    rank.map_or(0.0, |rank| 1.0 / bounded_f64(rank + 1))
}

fn bounded_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).expect("evaluation bounds fit in u32"))
}

fn latency_report(values: &mut [f64]) -> LatencyReport {
    values.sort_by(f64::total_cmp);
    LatencyReport {
        p50: percentile(values, 50),
        p95: percentile(values, 95),
        max: values.last().copied().unwrap_or_default(),
    }
}

fn percentile(values: &[f64], percentile: usize) -> f64 {
    let index = (values.len() * percentile).div_ceil(100).saturating_sub(1);
    values.get(index).copied().unwrap_or_default()
}

fn enforce_thresholds(options: &Options, report: &ContextPackReport) -> Result<(), String> {
    minimum(
        "context rules",
        report.context_rule_count,
        options.min_context_rules,
    )?;
    minimum(
        "added context rules",
        report.added_context_rule_count,
        options.min_added_context_rules,
    )?;
    minimum(
        "top-1 improvements",
        report.top1_improved,
        options.min_top1_improved,
    )?;
    maximum(
        "top-1 regressions",
        report.top1_regressed,
        options.max_top1_regressed,
    )?;
    maximum(
        "top-k regressions",
        report.topk_regressed,
        options.max_topk_regressed,
    )?;
    maximum(
        "top-1 changes",
        report.top1_changed,
        options.max_top1_changed,
    )?;
    minimum_float(
        "accuracy@1 delta",
        report.accuracy_at_1_delta,
        options.min_accuracy_delta,
    )?;
    minimum_float("MRR delta", report.mrr_at_k_delta, options.min_mrr_delta)?;
    maximum_float(
        "pack p95 ms",
        report.pack_latency_ms.p95,
        options.max_p95_ms,
    )?;
    maximum_float(
        "pack load ms",
        report.pack_load_ms,
        options.max_pack_load_ms,
    )?;
    maximum_u64("pack bytes", report.pack_bytes, options.max_pack_bytes)
}

fn minimum(label: &str, actual: usize, expected: Option<usize>) -> Result<(), String> {
    if expected.is_some_and(|expected| actual < expected) {
        return Err(format!("{label} {actual} is below the required minimum"));
    }
    Ok(())
}

fn maximum(label: &str, actual: usize, expected: Option<usize>) -> Result<(), String> {
    if expected.is_some_and(|expected| actual > expected) {
        return Err(format!("{label} {actual} exceeds the allowed maximum"));
    }
    Ok(())
}

fn minimum_float(label: &str, actual: f64, expected: Option<f64>) -> Result<(), String> {
    if expected.is_some_and(|expected| actual < expected) {
        return Err(format!("{label} {actual:.6} is below the required minimum"));
    }
    Ok(())
}

fn maximum_float(label: &str, actual: f64, expected: Option<f64>) -> Result<(), String> {
    if expected.is_some_and(|expected| actual > expected) {
        return Err(format!("{label} {actual:.6} exceeds the allowed maximum"));
    }
    Ok(())
}

fn maximum_u64(label: &str, actual: u64, expected: Option<u64>) -> Result<(), String> {
    if expected.is_some_and(|expected| actual > expected) {
        return Err(format!("{label} {actual} exceeds the allowed maximum"));
    }
    Ok(())
}

fn print_report(report: &ContextPackReport) {
    println!("context pack evaluation:");
    println!("  items: {}", report.items);
    println!("  baseline packs: {}", report.baseline_pack_count);
    println!("  baseline entries: {}", report.baseline_entry_count);
    println!(
        "  baseline context rules: {}",
        report.baseline_context_rule_count
    );
    println!("  baseline pack bytes: {}", report.baseline_pack_bytes);
    println!("  packs: {}", report.pack_count);
    println!("  entries: {}", report.entry_count);
    println!("  context rules: {}", report.context_rule_count);
    println!("  added context rules: {}", report.added_context_rule_count);
    println!("  pack bytes: {}", report.pack_bytes);
    println!(
        "  accuracy@1: {:.4} -> {:.4} ({:+.4})",
        report.baseline_accuracy_at_1, report.pack_accuracy_at_1, report.accuracy_at_1_delta
    );
    println!(
        "  MRR@{}: {:.4} -> {:.4} ({:+.4})",
        report.top_k, report.baseline_mrr_at_k, report.pack_mrr_at_k, report.mrr_at_k_delta
    );
    println!(
        "  top-1 improved/regressed/changed: {}/{}/{}",
        report.top1_improved, report.top1_regressed, report.top1_changed
    );
    println!(
        "  pack latency p50/p95/max ms: {:.3}/{:.3}/{:.3}",
        report.pack_latency_ms.p50, report.pack_latency_ms.p95, report.pack_latency_ms.max
    );
    println!("  pack load ms: {:.3}", report.pack_load_ms);
}

#[cfg(test)]
mod tests {
    use super::{ContextCase, evaluate_from_directories, load_cases};
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;
    use std::fmt::Write as _;
    use std::fs;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn parser_rejects_duplicates_without_echoing_vocabulary() {
        let mut cases = Vec::new();
        let mut seen = HashSet::new();
        let error = load_cases(
            Cursor::new("非公開前文\tかんじ\t漢字\n非公開前文\tかんじ\t感じ\n"),
            &mut cases,
            &mut seen,
        )
        .unwrap_err();
        assert!(error.contains("duplicates"), "{error}");
        assert!(!error.contains("非公開前文"), "{error}");
        assert!(!error.contains("漢字"), "{error}");
    }

    #[test]
    fn aggregate_report_measures_context_improvement_without_vocabulary() {
        let directory = test_directory();
        let pack_directory = directory.join("dictionary-packs");
        fs::create_dir_all(&pack_directory).unwrap();
        fs::write(
            pack_directory.join("sample.slime-dict"),
            "\
# slime-dictionary-pack-v3
# id: sample-context
# name: 文脈サンプル
# version: 2026.08.1
# license: Example-Test-Only
# minimum-slime-version: 0.1.0
# published-at: 2026-08-08
# provenance: fixture/generated/sample-context
# payload-sha256: dba7dcf657c74cd788ee904f95b5d2dd54d6fd16925e2ec88c96a13d19e4a0b6
# entries
てすとようご\t試験用語
# context-rules
文章\tかんじ\t漢字\t0
",
        )
        .unwrap();
        let report = evaluate_from_directories(
            None,
            &directory,
            &[ContextCase {
                previous_surface: "文章".to_owned(),
                reading: "かんじ".to_owned(),
                expected_surface: "漢字".to_owned(),
            }],
            10,
        )
        .unwrap();
        assert_eq!(report.context_rule_count, 1);
        assert_eq!(report.added_context_rule_count, 1);
        assert_eq!(report.baseline_pack_count, 0);
        assert_eq!(report.top1_improved, 1);
        assert_eq!(report.top1_regressed, 0);
        assert!((report.pack_accuracy_at_1 - 1.0).abs() < f64::EPSILON);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn baseline_data_directory_isolates_context_from_shared_vocabulary() {
        let directory = test_directory();
        let baseline_directory = directory.join("baseline");
        let candidate_directory = directory.join("candidate");
        fs::create_dir_all(baseline_directory.join("dictionary-packs")).unwrap();
        fs::create_dir_all(candidate_directory.join("dictionary-packs")).unwrap();

        let entries = "そうほう\t蒼峰\t5000\n";
        fs::write(
            baseline_directory.join("dictionary-packs/terms.slime-dict"),
            pack_source("terms", entries, ""),
        )
        .unwrap();
        fs::write(
            candidate_directory.join("dictionary-packs/terms.slime-dict"),
            pack_source("terms", entries, ""),
        )
        .unwrap();
        fs::write(
            candidate_directory.join("dictionary-packs/context.slime-dict"),
            pack_source("context", "", "文章\tそうほう\t蒼峰\t0\n"),
        )
        .unwrap();

        let report = evaluate_from_directories(
            Some(&baseline_directory),
            &candidate_directory,
            &[ContextCase {
                previous_surface: "文章".to_owned(),
                reading: "そうほう".to_owned(),
                expected_surface: "蒼峰".to_owned(),
            }],
            10,
        )
        .unwrap();
        assert_eq!(report.baseline_pack_count, 1);
        assert_eq!(report.baseline_entry_count, 1);
        assert_eq!(report.pack_count, 2);
        assert_eq!(report.entry_count, 1);
        assert_eq!(report.context_rule_count, 1);
        assert_eq!(report.added_context_rule_count, 1);
        assert_eq!(report.topk_recovered, 0);
        assert_eq!(report.top1_improved, 1);
        fs::remove_dir_all(directory).unwrap();
    }

    fn pack_source(id: &str, entries: &str, context_rules: &str) -> String {
        let payload = format!("{entries}# context-rules\n{context_rules}");
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(payload.as_bytes()) {
            write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
        }
        format!(
            "# slime-dictionary-pack-v3\n\
             # id: {id}\n\
             # name: {id}\n\
             # version: 2026.08.1\n\
             # license: Example-Test-Only\n\
             # minimum-slime-version: 0.1.0\n\
             # published-at: 2026-08-08\n\
             # provenance: fixture/generated/{id}\n\
             # payload-sha256: {digest}\n\
             # entries\n\
             {payload}"
        )
    }

    fn test_directory() -> std::path::PathBuf {
        let suffix = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "slime-context-pack-evaluation-{}-{suffix}",
            std::process::id()
        ))
    }
}
