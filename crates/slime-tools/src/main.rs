//! Offline evaluation tools for kana-kanji conversion quality.

mod corpus_bigram;
#[cfg(feature = "neural")]
mod neural;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use corpus_bigram::{CorpusBigramRanker, Diagnostics as BigramDiagnostics, TransitionDiagnostics};
use serde::{Deserialize, Serialize};
use slime_converter::{Candidate, CandidateRanker, CostOnlyRanker, Dictionary};

/// Mozc-style costs approximate `-scale * ln(probability)`. Used to map
/// lattice costs onto the neural log-likelihood axis for interpolation.
const COST_LOG_SCALE: f64 = 500.0;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let options = Options::parse(env::args().skip(1))?;
    let items = load_items(&options)?;
    let dictionary = Dictionary::bundled();
    let word_bigram_ranker = (options.word_bigram_weight > 0 || options.skip_bigram_weight > 0)
        .then(|| {
            CorpusBigramRanker::load(
                &options.word_bigram_corpora,
                options.word_bigram_weight,
                options.skip_bigram_weight,
            )
        })
        .transpose()?;
    let reports = evaluate(&dictionary, &items, &options, word_bigram_ranker.as_ref())?;

    if options.json {
        let serialized = if reports.len() == 1 {
            serde_json::to_string_pretty(&reports[0])
        } else {
            serde_json::to_string_pretty(&reports)
        };
        println!(
            "{}",
            serialized.map_err(|error| format!("failed to serialize report: {error}"))?
        );
    } else if reports.len() == 1 {
        print_report(&reports[0]);
    } else {
        println!("lambda sweep:");
        for report in &reports {
            println!(
                "  lambda={:.2} acc@1={:.4} acc@{}={:.4} mrr@{}={:.4} mincer@1={:.4} \
                 latency p50={:.3} p95={:.3}",
                report.lambda.unwrap_or(0.0),
                report.accuracy_at_1,
                report.top_k,
                report.accuracy_at_k,
                report.top_k,
                report.mrr_at_k,
                report.min_cer_at_1,
                report.latency_ms.p50,
                report.latency_ms.p95,
            );
        }
        let best = reports
            .iter()
            .max_by(|a, b| {
                a.accuracy_at_1
                    .total_cmp(&b.accuracy_at_1)
                    .then(a.mrr_at_k.total_cmp(&b.mrr_at_k))
            })
            .expect("non-empty reports");
        println!();
        println!("best lambda={:.2}:", best.lambda.unwrap_or(0.0));
        print_report(best);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ContextFilter {
    All,
    None,
    Present,
}

impl ContextFilter {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "all" => Ok(Self::All),
            "none" => Ok(Self::None),
            "present" => Ok(Self::Present),
            _ => Err(format!(
                "invalid --context value {value:?}; expected all, none, or present"
            )),
        }
    }

    fn includes(self, item: &AjimeeItem) -> bool {
        match self {
            Self::All => true,
            Self::None => item.context_text.is_empty(),
            Self::Present => !item.context_text.is_empty(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DatasetFormat {
    Ajimee,
    Anthy,
}

impl DatasetFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ajimee" => Ok(Self::Ajimee),
            "anthy" => Ok(Self::Anthy),
            _ => Err(format!(
                "unsupported evaluation format {value:?}\n{}",
                usage()
            )),
        }
    }

    const fn dataset_name(self) -> &'static str {
        match self {
            Self::Ajimee => "AJIMEE-Bench JWTD_v2/v1",
            Self::Anthy => "Anthy conversion corpus",
        }
    }
}

#[derive(Debug)]
struct Options {
    format: DatasetFormat,
    inputs: Vec<PathBuf>,
    dataset_revision: Option<String>,
    dataset_sha256: Option<String>,
    top_k: usize,
    search_k: Option<usize>,
    context: ContextFilter,
    limit: Option<usize>,
    failures: usize,
    json: bool,
    neural_model: Option<PathBuf>,
    lambdas: Vec<f64>,
    word_bigram_corpora: Vec<PathBuf>,
    word_bigram_weight: i32,
    skip_bigram_weight: i32,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut arguments = arguments.peekable();
        let Some(format) = arguments.next() else {
            return Err(usage());
        };
        let format = DatasetFormat::parse(&format)?;
        let mut options = Self {
            format,
            inputs: Vec::new(),
            dataset_revision: match format {
                DatasetFormat::Ajimee => env::var("AJIMEE_BENCH_REVISION").ok(),
                DatasetFormat::Anthy => env::var("ANTHY_CORPUS_REVISION").ok(),
            },
            dataset_sha256: match format {
                DatasetFormat::Ajimee => env::var("AJIMEE_BENCH_SHA256").ok(),
                DatasetFormat::Anthy => env::var("ANTHY_CORPUS_SHA256").ok(),
            },
            top_k: 10,
            search_k: None,
            context: ContextFilter::All,
            limit: None,
            failures: 10,
            json: false,
            neural_model: None,
            lambdas: Vec::new(),
            word_bigram_corpora: Vec::new(),
            word_bigram_weight: 0,
            skip_bigram_weight: 0,
        };

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--input" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--input requires a path".to_owned())?;
                    options.inputs.push(PathBuf::from(value));
                }
                "--top-k" => options.top_k = parse_positive("--top-k", arguments.next())?,
                "--search-k" => {
                    options.search_k = Some(parse_positive("--search-k", arguments.next())?);
                }
                "--context" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--context requires a value".to_owned())?;
                    options.context = ContextFilter::parse(&value)?;
                }
                "--limit" => options.limit = Some(parse_positive("--limit", arguments.next())?),
                "--failures" => options.failures = parse_usize("--failures", arguments.next())?,
                "--json" => options.json = true,
                "--neural-model" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--neural-model requires a path".to_owned())?;
                    options.neural_model = Some(PathBuf::from(value));
                }
                "--lambda" => options.lambdas.push(parse_lambda(arguments.next())?),
                "--word-bigram-corpus" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--word-bigram-corpus requires a path".to_owned())?;
                    options.word_bigram_corpora.push(PathBuf::from(value));
                }
                "--word-bigram-weight" => {
                    options.word_bigram_weight =
                        parse_non_negative_i32("--word-bigram-weight", arguments.next())?;
                }
                "--skip-bigram-weight" => {
                    options.skip_bigram_weight =
                        parse_non_negative_i32("--skip-bigram-weight", arguments.next())?;
                }
                "--help" | "-h" => return Err(usage()),
                _ if !argument.starts_with('-') => {
                    // Keep the original `ajimee items.json` invocation valid;
                    // `--input` is preferred when multiple Anthy files are used.
                    options.inputs.push(PathBuf::from(argument));
                }
                _ => return Err(format!("unknown argument {argument:?}\n{}", usage())),
            }
        }
        if options.inputs.is_empty() {
            return Err(format!("at least one --input is required\n{}", usage()));
        }
        if options.format == DatasetFormat::Ajimee && options.inputs.len() != 1 {
            return Err("ajimee format requires exactly one --input".to_owned());
        }
        if options
            .search_k
            .is_some_and(|search_k| search_k < options.top_k)
        {
            return Err("--search-k must be greater than or equal to --top-k".to_owned());
        }
        if (options.word_bigram_weight > 0 || options.skip_bigram_weight > 0)
            && options.word_bigram_corpora.is_empty()
        {
            return Err("bigram weights require --word-bigram-corpus".to_owned());
        }
        if options.lambdas.is_empty() {
            // Default sweep for tuning the interpolation weight on the devset.
            options.lambdas = (0..=10).map(|step| f64::from(step) / 10.0).collect();
            options.lambdas.push(0.95);
            options.lambdas.sort_by(f64::total_cmp);
        }
        Ok(options)
    }
}

fn usage() -> String {
    "usage: slime-evaluate <ajimee|anthy> --input <path> [--input <path> ...] [--top-k N] \
     [--search-k N] \
     [--context all|none|present] [--limit N] [--failures N] [--json] \
     [--neural-model model.gguf] [--lambda X]... \
     [--word-bigram-corpus corpus.txt] [--word-bigram-weight N] \
     [--skip-bigram-weight N]\n\
     --neural-model rescores the N-best with a zenz GGUF model (requires \
     building with --features neural). --lambda selects interpolation \
     weights; without it a default sweep runs. The optional annotated corpus \
     uses whitespace-separated surface/reading tokens and only affects offline \
     N-best ranking."
        .to_owned()
}

fn parse_non_negative_i32(name: &str, value: Option<String>) -> Result<i32, String> {
    let value = value.ok_or_else(|| format!("{name} requires a value"))?;
    let parsed = value
        .parse::<i32>()
        .map_err(|_| format!("{name} requires a non-negative integer"))?;
    if parsed < 0 {
        return Err(format!("{name} requires a non-negative integer"));
    }
    Ok(parsed)
}

fn load_items(options: &Options) -> Result<Vec<AjimeeItem>, String> {
    match options.format {
        DatasetFormat::Ajimee => load_ajimee_items(&options.inputs[0]),
        DatasetFormat::Anthy => load_anthy_items(&options.inputs),
    }
}

fn load_ajimee_items(path: &Path) -> Result<Vec<AjimeeItem>, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn load_anthy_items(paths: &[PathBuf]) -> Result<Vec<AjimeeItem>, String> {
    let mut items = Vec::new();
    for path in paths {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        for (line_index, line) in source.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (reading, expected) = parse_anthy_line(line)
                .map_err(|error| format!("{}:{}: {error}", path.display(), line_index + 1))?;
            items.push(AjimeeItem {
                index: format!("{}:{}", path.display(), line_index + 1),
                context_text: String::new(),
                input: reading,
                expected_output: vec![expected],
            });
        }
    }
    Ok(items)
}

fn parse_anthy_line(line: &str) -> Result<(String, String), String> {
    let (reading, expected) = line
        .split_once("| |")
        .ok_or_else(|| "expected a '| |' separator between reading and surface".to_owned())?;
    let concatenate_segments = |value: &str| value.split('|').collect::<String>();
    let reading = concatenate_segments(reading);
    let expected = concatenate_segments(expected);
    if reading.is_empty() || expected.is_empty() {
        return Err("reading and surface must not be empty".to_owned());
    }
    Ok((reading, expected))
}

fn parse_lambda(value: Option<String>) -> Result<f64, String> {
    let parsed: f64 = value
        .ok_or_else(|| "--lambda requires a value".to_owned())?
        .parse()
        .map_err(|_| "--lambda requires a number".to_owned())?;
    if !(0.0..=1.0).contains(&parsed) {
        return Err("--lambda must be between 0 and 1".to_owned());
    }
    Ok(parsed)
}

fn parse_positive(name: &str, value: Option<String>) -> Result<usize, String> {
    let parsed = parse_usize(name, value)?;
    if parsed == 0 {
        return Err(format!("{name} must be greater than zero"));
    }
    Ok(parsed)
}

fn parse_usize(name: &str, value: Option<String>) -> Result<usize, String> {
    value
        .ok_or_else(|| format!("{name} requires a value"))?
        .parse()
        .map_err(|_| format!("{name} requires a non-negative integer"))
}

#[derive(Debug, Deserialize)]
struct AjimeeItem {
    index: String,
    context_text: String,
    input: String,
    expected_output: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EvaluationReport {
    dataset: &'static str,
    dataset_revision: Option<String>,
    dataset_sha256: Option<String>,
    context_filter: ContextFilter,
    context_used_by_engine: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lambda: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    word_bigram: Option<NgramReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_bigram: Option<NgramReport>,
    items: usize,
    top_k: usize,
    search_k: usize,
    accuracy_at_1: f64,
    accuracy_at_k: f64,
    mrr_at_k: f64,
    min_cer_at_1: f64,
    min_cer_at_k: f64,
    latency_ms: LatencyReport,
    failures: Vec<Failure>,
}

#[derive(Debug, Serialize)]
struct NgramReport {
    entries: usize,
    weight: i32,
    candidates_scored: u64,
    transitions_scored: u64,
    matched_transitions: u64,
    match_rate: f64,
}

impl NgramReport {
    fn new(diagnostics: TransitionDiagnostics, candidates_scored: u64) -> Self {
        let match_rate = if diagnostics.transitions_scored == 0 {
            0.0
        } else {
            u64_to_f64(diagnostics.matched_transitions) / u64_to_f64(diagnostics.transitions_scored)
        };
        Self {
            entries: diagnostics.entries,
            weight: diagnostics.weight,
            candidates_scored,
            transitions_scored: diagnostics.transitions_scored,
            matched_transitions: diagnostics.matched_transitions,
            match_rate,
        }
    }
}

fn word_bigram_report(diagnostics: Option<BigramDiagnostics>) -> Option<NgramReport> {
    diagnostics.and_then(|diagnostics| {
        diagnostics
            .word
            .map(|word| NgramReport::new(word, diagnostics.candidates_scored))
    })
}

fn skip_bigram_report(diagnostics: Option<BigramDiagnostics>) -> Option<NgramReport> {
    diagnostics.and_then(|diagnostics| {
        diagnostics
            .skip
            .map(|skip| NgramReport::new(skip, diagnostics.candidates_scored))
    })
}

#[derive(Debug, Serialize)]
struct LatencyReport {
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

#[derive(Debug, Serialize)]
struct Failure {
    index: String,
    context_text: String,
    input: String,
    expected_output: Vec<String>,
    candidates: Vec<String>,
}

struct ItemOutcome<'a> {
    item: &'a AjimeeItem,
    candidates: Vec<Candidate>,
    latency: Duration,
}

fn evaluate(
    dictionary: &Dictionary,
    items: &[AjimeeItem],
    options: &Options,
    word_bigram_ranker: Option<&CorpusBigramRanker>,
) -> Result<Vec<EvaluationReport>, String> {
    let selected: Vec<_> = items
        .iter()
        .filter(|item| options.context.includes(item))
        .take(options.limit.unwrap_or(usize::MAX))
        .collect();
    if selected.is_empty() {
        return Err("no evaluation items matched the selected filters".to_owned());
    }

    let ranker = word_bigram_ranker.map_or(&CostOnlyRanker as &dyn CandidateRanker, |ranker| {
        ranker as &dyn CandidateRanker
    });
    let mut outcomes = Vec::with_capacity(selected.len());
    for item in selected {
        if item.expected_output.is_empty() {
            return Err(format!("item {} has no expected output", item.index));
        }
        let reading = katakana_to_hiragana(&item.input);
        let started = Instant::now();
        let search_k = options.search_k.unwrap_or(options.top_k);
        let candidates: Vec<_> = dictionary
            .candidates_with_ranker(&reading, search_k, ranker)
            .into_iter()
            .take(options.top_k)
            .collect();
        let latency = started.elapsed();
        outcomes.push(ItemOutcome {
            item,
            candidates,
            latency,
        });
    }
    let word_bigram_diagnostics = word_bigram_ranker.map(CorpusBigramRanker::diagnostics);

    let Some(model_path) = &options.neural_model else {
        return Ok(vec![compute_report(
            &outcomes,
            None,
            None,
            options,
            word_bigram_diagnostics,
        )]);
    };

    #[cfg(not(feature = "neural"))]
    {
        let _ = model_path;
        Err("--neural-model requires building slime-tools with --features neural".to_owned())
    }

    #[cfg(feature = "neural")]
    {
        let rescorer = neural::Rescorer::load(model_path)?;
        let requests: Vec<_> = outcomes
            .iter()
            .map(|outcome| neural::ScoreRequest {
                context: outcome.item.context_text.clone(),
                input_katakana: outcome.item.input.clone(),
                candidates: outcome
                    .candidates
                    .iter()
                    .map(|candidate| candidate.surface.clone())
                    .collect(),
            })
            .collect();
        let scored = rescorer.score_all(&requests)?;
        let neural = NeuralOutcome {
            logliks: scored.iter().map(|item| item.logliks.clone()).collect(),
            latencies: scored.iter().map(|item| item.latency).collect(),
        };
        Ok(options
            .lambdas
            .iter()
            .map(|&lambda| {
                compute_report(
                    &outcomes,
                    Some(&neural),
                    Some(lambda),
                    options,
                    word_bigram_diagnostics,
                )
            })
            .collect())
    }
}

struct NeuralOutcome {
    logliks: Vec<Vec<f64>>,
    latencies: Vec<Duration>,
}

/// Reorders candidate surfaces by interpolating the lattice cost with the
/// neural log-likelihood: `(1-lambda) * (-cost/scale) + lambda * loglik`.
/// The stable sort keeps the lattice order for ties.
fn rescored_surfaces(candidates: &[Candidate], logliks: &[f64], lambda: f64) -> Vec<String> {
    let mut indexed: Vec<usize> = (0..candidates.len()).collect();
    let combined: Vec<f64> = candidates
        .iter()
        .zip(logliks)
        .map(|(candidate, loglik)| {
            (1.0 - lambda) * (-f64::from(candidate.cost) / COST_LOG_SCALE) + lambda * loglik
        })
        .collect();
    indexed.sort_by(|&a, &b| combined[b].total_cmp(&combined[a]));
    indexed
        .into_iter()
        .map(|index| candidates[index].surface.clone())
        .collect()
}

fn compute_report(
    outcomes: &[ItemOutcome<'_>],
    neural: Option<&NeuralOutcome>,
    lambda: Option<f64>,
    options: &Options,
    word_bigram_diagnostics: Option<BigramDiagnostics>,
) -> EvaluationReport {
    let mut correct_at_1 = 0_usize;
    let mut correct_at_k = 0_usize;
    let mut reciprocal_rank = 0.0;
    let mut min_cer_at_1 = 0.0;
    let mut min_cer_at_k = 0.0;
    let mut latencies = Vec::with_capacity(outcomes.len());
    let mut failures = Vec::new();

    for (outcome_index, outcome) in outcomes.iter().enumerate() {
        let item = outcome.item;
        let candidates: Vec<String> = match (neural, lambda) {
            (Some(neural), Some(lambda)) => {
                rescored_surfaces(&outcome.candidates, &neural.logliks[outcome_index], lambda)
            }
            _ => outcome
                .candidates
                .iter()
                .map(|candidate| candidate.surface.clone())
                .collect(),
        };
        let mut latency = outcome.latency;
        if let Some(neural) = neural {
            latency += neural.latencies[outcome_index];
        }
        latencies.push(latency);

        let normalized_candidates: Vec<String> = candidates
            .iter()
            .map(|candidate| normalize_for_evaluation(candidate, options.format))
            .collect();
        let normalized_expected: Vec<String> = item
            .expected_output
            .iter()
            .map(|expected| normalize_for_evaluation(expected, options.format))
            .collect();
        let rank = normalized_candidates.iter().position(|candidate| {
            normalized_expected
                .iter()
                .any(|expected| expected == candidate)
        });
        if rank == Some(0) {
            correct_at_1 += 1;
        }
        if let Some(rank) = rank {
            correct_at_k += 1;
            reciprocal_rank += 1.0 / usize_to_f64(rank + 1);
        }

        min_cer_at_1 += normalized_candidates.first().map_or(1.0, |candidate| {
            minimum_cer(&normalized_expected, candidate)
        });
        min_cer_at_k += normalized_candidates
            .iter()
            .map(|candidate| minimum_cer(&normalized_expected, candidate))
            .reduce(f64::min)
            .unwrap_or(1.0);

        if rank.is_none() && failures.len() < options.failures {
            failures.push(Failure {
                index: item.index.clone(),
                context_text: item.context_text.clone(),
                input: item.input.clone(),
                expected_output: item.expected_output.clone(),
                candidates,
            });
        }
    }

    let total = usize_to_f64(outcomes.len());
    latencies.sort_unstable();
    EvaluationReport {
        dataset: options.format.dataset_name(),
        dataset_revision: options.dataset_revision.clone(),
        dataset_sha256: options.dataset_sha256.clone(),
        context_filter: options.context,
        context_used_by_engine: neural.is_some(),
        neural_model: options
            .neural_model
            .as_ref()
            .map(|path| path.display().to_string()),
        lambda,
        word_bigram: word_bigram_report(word_bigram_diagnostics),
        skip_bigram: skip_bigram_report(word_bigram_diagnostics),
        items: outcomes.len(),
        top_k: options.top_k,
        search_k: options.search_k.unwrap_or(options.top_k),
        accuracy_at_1: usize_to_f64(correct_at_1) / total,
        accuracy_at_k: usize_to_f64(correct_at_k) / total,
        mrr_at_k: reciprocal_rank / total,
        min_cer_at_1: min_cer_at_1 / total,
        min_cer_at_k: min_cer_at_k / total,
        latency_ms: LatencyReport {
            p50: percentile(&latencies, 50),
            p95: percentile(&latencies, 95),
            p99: percentile(&latencies, 99),
            max: duration_to_millis(*latencies.last().expect("non-empty latencies")),
        },
        failures,
    }
}

fn normalize_for_evaluation(value: &str, format: DatasetFormat) -> String {
    match format {
        DatasetFormat::Ajimee => value.to_owned(),
        DatasetFormat::Anthy => value
            .chars()
            .map(|character| match character {
                '０'..='９' => {
                    char::from_u32(u32::from(character) - 0xFEE0).expect("valid ASCII digit")
                }
                _ => character,
            })
            .collect(),
    }
}

fn print_report(report: &EvaluationReport) {
    println!("dataset: {}", report.dataset);
    if let Some(revision) = &report.dataset_revision {
        println!("dataset revision: {revision}");
    }
    if let Some(sha256) = &report.dataset_sha256 {
        println!("dataset sha256: {sha256}");
    }
    println!("context filter: {:?}", report.context_filter);
    println!("context used by engine: {}", report.context_used_by_engine);
    if let Some(model) = &report.neural_model {
        println!("neural model: {model}");
    }
    if let Some(lambda) = report.lambda {
        println!("lambda: {lambda:.2}");
    }
    if let Some(bigram) = &report.word_bigram {
        println!("word bigram entries: {}", bigram.entries);
        println!("word bigram weight: {}", bigram.weight);
        println!(
            "word bigram candidates scored: {}",
            bigram.candidates_scored
        );
        println!(
            "word bigram transitions scored: {}",
            bigram.transitions_scored
        );
        println!(
            "word bigram matched transitions: {}",
            bigram.matched_transitions
        );
        println!("word bigram match rate: {:.4}", bigram.match_rate);
    }
    if let Some(bigram) = &report.skip_bigram {
        println!("skip bigram entries: {}", bigram.entries);
        println!("skip bigram weight: {}", bigram.weight);
        println!(
            "skip bigram candidates scored: {}",
            bigram.candidates_scored
        );
        println!(
            "skip bigram transitions scored: {}",
            bigram.transitions_scored
        );
        println!(
            "skip bigram matched transitions: {}",
            bigram.matched_transitions
        );
        println!("skip bigram match rate: {:.4}", bigram.match_rate);
    }
    println!("items: {}", report.items);
    println!("search k: {}", report.search_k);
    println!("acc@1: {:.4}", report.accuracy_at_1);
    println!("acc@{}: {:.4}", report.top_k, report.accuracy_at_k);
    println!("mrr@{}: {:.4}", report.top_k, report.mrr_at_k);
    println!("mincer@1: {:.4}", report.min_cer_at_1);
    println!("mincer@{}: {:.4}", report.top_k, report.min_cer_at_k);
    println!(
        "latency ms: p50={:.3} p95={:.3} p99={:.3} max={:.3}",
        report.latency_ms.p50, report.latency_ms.p95, report.latency_ms.p99, report.latency_ms.max
    );
    if !report.failures.is_empty() {
        println!("failures (first {}):", report.failures.len());
        for failure in &report.failures {
            println!(
                "  {} input={} expected={:?} candidates={:?}",
                failure.index, failure.input, failure.expected_output, failure.candidates
            );
        }
    }
}

fn katakana_to_hiragana(input: &str) -> String {
    input
        .chars()
        .map(|character| match character {
            'ァ'..='ヶ' | 'ヽ' | 'ヾ' => {
                char::from_u32(u32::from(character) - 0x60).expect("valid hiragana scalar")
            }
            _ => character,
        })
        .collect()
}

fn minimum_cer(references: &[String], hypothesis: &str) -> f64 {
    references
        .iter()
        .map(|reference| character_error_rate(reference, hypothesis))
        .reduce(f64::min)
        .unwrap_or(1.0)
}

fn character_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference: Vec<_> = reference.chars().collect();
    let hypothesis: Vec<_> = hypothesis.chars().collect();
    if reference.is_empty() {
        return if hypothesis.is_empty() {
            0.0
        } else {
            f64::INFINITY
        };
    }
    let mut previous: Vec<usize> = (0..=hypothesis.len()).collect();
    let mut current = vec![0; hypothesis.len() + 1];
    for (reference_index, reference_character) in reference.iter().enumerate() {
        current[0] = reference_index + 1;
        for (hypothesis_index, hypothesis_character) in hypothesis.iter().enumerate() {
            current[hypothesis_index + 1] = (previous[hypothesis_index + 1] + 1)
                .min(current[hypothesis_index] + 1)
                .min(
                    previous[hypothesis_index]
                        + usize::from(reference_character != hypothesis_character),
                );
        }
        std::mem::swap(&mut previous, &mut current);
    }
    usize_to_f64(previous[hypothesis.len()]) / usize_to_f64(reference.len())
}

fn percentile(sorted_durations: &[Duration], percentile: usize) -> f64 {
    let rank = sorted_durations
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted_durations.len() - 1);
    duration_to_millis(sorted_durations[rank])
}

fn duration_to_millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).expect("evaluation counts fit in u32"))
}

fn u64_to_f64(value: u64) -> f64 {
    f64::from(u32::try_from(value).expect("evaluation counts fit in u32"))
}

#[cfg(test)]
mod tests {
    use super::{
        ContextFilter, DatasetFormat, Options, character_error_rate, katakana_to_hiragana,
        normalize_for_evaluation, parse_anthy_line, percentile,
    };
    use std::time::Duration;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn converts_full_width_katakana_without_changing_punctuation() {
        assert_eq!(
            katakana_to_hiragana("ニホンゴ、ヴァイオリン・１２３"),
            "にほんご、ゔぁいおりん・１２３"
        );
    }

    #[test]
    fn character_error_rate_uses_unicode_characters() {
        assert_close(character_error_rate("日本語", "日本"), 1.0 / 3.0);
        assert_close(character_error_rate("日本語", "日本後"), 1.0 / 3.0);
        assert_close(character_error_rate("日本語", "日本語"), 0.0);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let values: Vec<_> = (1..=100).map(Duration::from_nanos).collect();
        assert_close(percentile(&values, 50), 50.0 / 1_000_000.0);
        assert_close(percentile(&values, 95), 95.0 / 1_000_000.0);
        assert_close(percentile(&values, 99), 99.0 / 1_000_000.0);
    }

    #[test]
    fn parses_reproducible_evaluation_options() {
        let options = Options::parse(
            [
                "ajimee",
                "--input",
                "items.json",
                "--top-k",
                "5",
                "--search-k",
                "20",
                "--context",
                "none",
                "--limit",
                "25",
                "--failures",
                "0",
                "--json",
                "--word-bigram-corpus",
                "annotated.txt",
                "--word-bigram-weight",
                "500",
                "--skip-bigram-weight",
                "250",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(options.top_k, 5);
        assert_eq!(options.search_k, Some(20));
        assert_eq!(options.format, DatasetFormat::Ajimee);
        assert_eq!(options.inputs, [std::path::PathBuf::from("items.json")]);
        assert_eq!(options.context, ContextFilter::None);
        assert_eq!(options.limit, Some(25));
        assert_eq!(options.failures, 0);
        assert!(options.json);
        assert_eq!(
            options.word_bigram_corpora,
            [std::path::PathBuf::from("annotated.txt")]
        );
        assert_eq!(options.word_bigram_weight, 500);
        assert_eq!(options.skip_bigram_weight, 250);
    }

    #[test]
    fn keeps_the_original_positional_ajimee_input() {
        let options =
            Options::parse(["ajimee", "items.json"].into_iter().map(str::to_owned)).unwrap();

        assert_eq!(options.inputs, [std::path::PathBuf::from("items.json")]);
    }

    #[test]
    fn parses_anthy_segments_without_losing_literal_text() {
        assert_eq!(
            parse_anthy_line("|uim-fepの|あたらしい|ばーじょん| |uim-fepの|新しい|バージョン|")
                .unwrap(),
            (
                "uim-fepのあたらしいばーじょん".to_owned(),
                "uim-fepの新しいバージョン".to_owned()
            )
        );
    }

    #[test]
    fn anthy_scoring_normalizes_full_width_digits() {
        assert_eq!(
            normalize_for_evaluation("今日は２０２６年", DatasetFormat::Anthy),
            "今日は2026年"
        );
        assert_eq!(
            normalize_for_evaluation("今日は２０２６年", DatasetFormat::Ajimee),
            "今日は２０２６年"
        );
    }
}
