//! Evaluates conservative typo suggestions without logging input vocabulary.

use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde::Serialize;
use slime_core::{InputEvent, SlimeAction, SlimeEngine};

const EDIT_KINDS: [&str; 8] = [
    "deletion",
    "duplicate",
    "missing_consonant",
    "missing_geminate",
    "missing_syllabic_n",
    "missing_vowel",
    "neighbor",
    "transposition",
];

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
    let mut seen = HashSet::new();
    let positives = load_positive_files(&options.positive_paths, &mut seen)?;
    let negatives = load_negative_files(&options.negative_paths, &mut seen)?;
    let report = evaluate(&positives, &negatives);

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
    "usage: slime-typo-evaluate --positive PATH [--positive PATH ...] \
     --negative PATH [--negative PATH ...] [--max-missing N] \
     [--max-unnecessary N] [--min-per-edit N] [--max-p95-ms N] \
     [--max-corrections N] [--json]\n\
     positive format: raw_input<TAB>corrected_reading<TAB>expected_surface<TAB>edit_kind\n\
     negative format: raw_input<TAB>reason"
}

struct Options {
    positive_paths: Vec<PathBuf>,
    negative_paths: Vec<PathBuf>,
    max_missing: Option<usize>,
    max_unnecessary: Option<usize>,
    min_per_edit: Option<usize>,
    max_p95_ms: Option<f64>,
    max_corrections: Option<usize>,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut positive_paths = Vec::new();
        let mut negative_paths = Vec::new();
        let mut max_missing = None;
        let mut max_unnecessary = None;
        let mut min_per_edit = None;
        let mut max_p95_ms = None;
        let mut max_corrections = None;
        let mut json = false;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--positive" => positive_paths.push(PathBuf::from(
                    arguments.next().ok_or("--positive requires PATH")?,
                )),
                "--negative" => negative_paths.push(PathBuf::from(
                    arguments.next().ok_or("--negative requires PATH")?,
                )),
                "--max-missing" => {
                    max_missing = Some(parse_usize("--max-missing", arguments.next())?);
                }
                "--max-unnecessary" => {
                    max_unnecessary = Some(parse_usize("--max-unnecessary", arguments.next())?);
                }
                "--min-per-edit" => {
                    min_per_edit = Some(parse_usize("--min-per-edit", arguments.next())?);
                }
                "--max-p95-ms" => {
                    max_p95_ms = Some(parse_non_negative_f64("--max-p95-ms", arguments.next())?);
                }
                "--max-corrections" => {
                    max_corrections = Some(parse_usize("--max-corrections", arguments.next())?);
                }
                "--json" => json = true,
                "--help" | "-h" => return Err(usage().to_owned()),
                _ => return Err(format!("unknown argument\n{}", usage())),
            }
        }
        if positive_paths.is_empty() || negative_paths.is_empty() {
            return Err(usage().to_owned());
        }
        Ok(Self {
            positive_paths,
            negative_paths,
            max_missing,
            max_unnecessary,
            min_per_edit,
            max_p95_ms,
            max_corrections,
            json,
        })
    }
}

fn parse_usize(option: &str, value: Option<String>) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{option} requires N"))?;
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative integer"))
}

fn parse_non_negative_f64(option: &str, value: Option<String>) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("{option} requires N"))?;
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("{option} requires a non-negative number"))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!("{option} requires a non-negative number"));
    }
    Ok(parsed)
}

#[derive(Debug)]
struct PositiveCase {
    raw_input: String,
    corrected_reading: String,
    expected_surface: String,
    edit_kind: String,
}

#[derive(Debug)]
struct NegativeCase {
    raw_input: String,
}

fn load_positive_files(
    paths: &[PathBuf],
    seen: &mut HashSet<String>,
) -> Result<Vec<PositiveCase>, String> {
    let mut cases = Vec::new();
    for path in paths {
        let file = fs::File::open(path)
            .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
        cases.extend(
            load_positives(BufReader::new(file), seen)
                .map_err(|error| format!("{}: {error}", path.display()))?,
        );
    }
    if cases.is_empty() {
        return Err("positive inputs contain no cases".to_owned());
    }
    Ok(cases)
}

fn load_negative_files(
    paths: &[PathBuf],
    seen: &mut HashSet<String>,
) -> Result<Vec<NegativeCase>, String> {
    let mut cases = Vec::new();
    for path in paths {
        let file = fs::File::open(path)
            .map_err(|error| format!("failed to open {}: {error}", path.display()))?;
        cases.extend(
            load_negatives(BufReader::new(file), seen)
                .map_err(|error| format!("{}: {error}", path.display()))?,
        );
    }
    if cases.is_empty() {
        return Err("negative inputs contain no cases".to_owned());
    }
    Ok(cases)
}

fn load_positives(
    reader: impl BufRead,
    seen: &mut HashSet<String>,
) -> Result<Vec<PositiveCase>, String> {
    let mut cases = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(|error| format!("line {line_number}: {error}"))?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let raw_input = required_column(columns.next(), line_number)?;
        let corrected_reading = required_column(columns.next(), line_number)?;
        let expected_surface = required_column(columns.next(), line_number)?;
        let edit_kind = required_column(columns.next(), line_number)?;
        if columns.next().is_some() {
            return Err(format!("line {line_number} has more than four columns"));
        }
        validate_raw_input(raw_input, line_number)?;
        if !EDIT_KINDS.contains(&edit_kind) {
            return Err(format!("line {line_number} has an unsupported edit kind"));
        }
        if !seen.insert(raw_input.to_owned()) {
            return Err(format!("line {line_number} duplicates an input"));
        }
        cases.push(PositiveCase {
            raw_input: raw_input.to_owned(),
            corrected_reading: corrected_reading.to_owned(),
            expected_surface: expected_surface.to_owned(),
            edit_kind: edit_kind.to_owned(),
        });
    }
    Ok(cases)
}

fn load_negatives(
    reader: impl BufRead,
    seen: &mut HashSet<String>,
) -> Result<Vec<NegativeCase>, String> {
    let mut cases = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line.map_err(|error| format!("line {line_number}: {error}"))?;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let raw_input = required_column(columns.next(), line_number)?;
        let _reason = required_column(columns.next(), line_number)?;
        if columns.next().is_some() {
            return Err(format!("line {line_number} has more than two columns"));
        }
        validate_raw_input(raw_input, line_number)?;
        if !seen.insert(raw_input.to_owned()) {
            return Err(format!("line {line_number} duplicates an input"));
        }
        cases.push(NegativeCase {
            raw_input: raw_input.to_owned(),
        });
    }
    Ok(cases)
}

fn required_column(value: Option<&str>, line_number: usize) -> Result<&str, String> {
    value
        .filter(|column| !column.is_empty())
        .ok_or_else(|| format!("line {line_number} has an empty or missing column"))
}

fn validate_raw_input(raw_input: &str, line_number: usize) -> Result<(), String> {
    if raw_input.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        Ok(())
    } else {
        Err(format!(
            "line {line_number} raw input must contain only ASCII letters"
        ))
    }
}

#[derive(Default, Serialize)]
struct EditReport {
    total: usize,
    recalled: usize,
}

#[derive(Clone, Copy, Serialize)]
struct LatencyReport {
    p50: f64,
    p95: f64,
    max: f64,
}

#[derive(Serialize)]
struct EvaluationReport {
    positive_total: usize,
    positive_recalled: usize,
    positive_missing: usize,
    negative_total: usize,
    unnecessary_corrections: usize,
    by_edit: BTreeMap<String, EditReport>,
    latency_ms: LatencyReport,
    max_corrections_observed: usize,
}

fn evaluate(positives: &[PositiveCase], negatives: &[NegativeCase]) -> EvaluationReport {
    let mut by_edit: BTreeMap<String, EditReport> = EDIT_KINDS
        .iter()
        .map(|edit| ((*edit).to_owned(), EditReport::default()))
        .collect();
    let mut positive_recalled = 0;
    let mut unnecessary_corrections = 0;
    let mut latencies = Vec::with_capacity(positives.len() + negatives.len());
    let mut max_corrections_observed = 0;

    for case in positives {
        let result = evaluate_input(&case.raw_input);
        let label = format!(
            "{}　（{}に訂正）",
            case.expected_surface, case.corrected_reading
        );
        let original_stays_first = result
            .candidates
            .first()
            .is_some_and(|candidate| candidate == &result.preedit);
        let recalled = result.preedit != case.expected_surface
            && original_stays_first
            && result.candidates.contains(&case.expected_surface)
            && result.labels.contains(&label);
        positive_recalled += usize::from(recalled);
        let edit = by_edit
            .get_mut(&case.edit_kind)
            .expect("validated edit kind");
        edit.total += 1;
        edit.recalled += usize::from(recalled);
        latencies.push(result.latency_ms);
        max_corrections_observed =
            max_corrections_observed.max(correction_suggestion_count(&result.labels));
    }

    for case in negatives {
        let result = evaluate_input(&case.raw_input);
        unnecessary_corrections += usize::from(
            result
                .labels
                .iter()
                .any(|candidate| candidate.contains("に訂正）")),
        );
        latencies.push(result.latency_ms);
        max_corrections_observed =
            max_corrections_observed.max(correction_suggestion_count(&result.labels));
    }

    EvaluationReport {
        positive_total: positives.len(),
        positive_recalled,
        positive_missing: positives.len() - positive_recalled,
        negative_total: negatives.len(),
        unnecessary_corrections,
        by_edit,
        latency_ms: latency_report(latencies),
        max_corrections_observed,
    }
}

struct InputResult {
    preedit: String,
    candidates: Vec<String>,
    labels: Vec<String>,
    latency_ms: f64,
}

fn evaluate_input(raw_input: &str) -> InputResult {
    let started = Instant::now();
    let mut engine = SlimeEngine::bundled();
    for character in raw_input.chars() {
        engine.handle(InputEvent::Character(character));
    }
    let actions = engine.handle(InputEvent::Space);
    let latency_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let snapshot = engine.snapshot();
    let labels = actions
        .into_iter()
        .filter_map(|action| match action {
            SlimeAction::ShowCandidates { candidates, .. } => Some(candidates),
            _ => None,
        })
        .flatten()
        .collect();
    InputResult {
        preedit: snapshot.preedit,
        candidates: snapshot.candidates,
        labels,
        latency_ms,
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

fn correction_suggestion_count(labels: &[String]) -> usize {
    labels
        .iter()
        .filter(|candidate| candidate.contains("に訂正）"))
        .count()
}

fn print_report(report: &EvaluationReport) {
    println!(
        "positive: {}/{} recalled; negative: {}/{} unnecessary; p95: {:.3} ms; max corrections: {}",
        report.positive_recalled,
        report.positive_total,
        report.unnecessary_corrections,
        report.negative_total,
        report.latency_ms.p95,
        report.max_corrections_observed
    );
    for (edit, counts) in &report.by_edit {
        println!("{edit}: {}/{}", counts.recalled, counts.total);
    }
}

fn enforce_thresholds(options: &Options, report: &EvaluationReport) -> Result<(), String> {
    if options
        .max_missing
        .is_some_and(|maximum| report.positive_missing > maximum)
    {
        return Err(format!(
            "positive missing count {} exceeds --max-missing {}",
            report.positive_missing,
            options.max_missing.expect("checked above")
        ));
    }
    if options
        .max_unnecessary
        .is_some_and(|maximum| report.unnecessary_corrections > maximum)
    {
        return Err(format!(
            "unnecessary correction count {} exceeds --max-unnecessary {}",
            report.unnecessary_corrections,
            options.max_unnecessary.expect("checked above")
        ));
    }
    if let Some(minimum) = options.min_per_edit {
        for (edit, counts) in &report.by_edit {
            if counts.total < minimum || counts.recalled < minimum {
                return Err(format!(
                    "edit kind {edit} has {}/{} recalled, below --min-per-edit {minimum}",
                    counts.recalled, counts.total
                ));
            }
        }
    }
    if options
        .max_p95_ms
        .is_some_and(|maximum| report.latency_ms.p95 > maximum)
    {
        return Err(format!(
            "end-to-end p95 {:.3} ms exceeds --max-p95-ms {:.3}",
            report.latency_ms.p95,
            options.max_p95_ms.expect("checked above")
        ));
    }
    if options
        .max_corrections
        .is_some_and(|maximum| report.max_corrections_observed > maximum)
    {
        return Err(format!(
            "correction suggestion count {} exceeds --max-corrections {}",
            report.max_corrections_observed,
            options.max_corrections.expect("checked above")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::io::Cursor;

    use super::{evaluate, load_negatives, load_positives};

    #[test]
    fn aggregate_report_covers_positive_and_negative_cases_without_vocabulary() {
        let mut seen = HashSet::new();
        let positives =
            load_positives(Cursor::new("nihpn\tにほん\t日本\tneighbor\n"), &mut seen).unwrap();
        let negatives = load_negatives(Cursor::new("nihon\texact\n"), &mut seen).unwrap();
        let report = evaluate(&positives, &negatives);
        assert_eq!(report.positive_recalled, 1);
        assert_eq!(report.unnecessary_corrections, 0);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("nihpn"));
        assert!(!json.contains("日本"));
    }

    #[test]
    fn parser_rejects_duplicates_without_echoing_private_input() {
        let mut seen = HashSet::new();
        load_positives(
            Cursor::new("privateword\tよみ\t表記\tdeletion\n"),
            &mut seen,
        )
        .unwrap();
        let error = load_negatives(Cursor::new("privateword\toverlap\n"), &mut seen).unwrap_err();
        assert!(error.contains("duplicates"));
        assert!(!error.contains("privateword"));
    }
}
