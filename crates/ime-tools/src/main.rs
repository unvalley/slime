//! Offline evaluation tools for kana-kanji conversion quality.

#[cfg(feature = "neural")]
mod neural;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ime_converter::{Candidate, Dictionary};
use serde::{Deserialize, Serialize};

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
    let bytes = fs::read(&options.input)
        .map_err(|error| format!("failed to read {}: {error}", options.input.display()))?;
    let items: Vec<AjimeeItem> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse {}: {error}", options.input.display()))?;
    let dictionary = Dictionary::bundled();
    let reports = evaluate(&dictionary, &items, &options)?;

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

#[derive(Debug)]
struct Options {
    input: PathBuf,
    dataset_revision: Option<String>,
    dataset_sha256: Option<String>,
    top_k: usize,
    context: ContextFilter,
    limit: Option<usize>,
    failures: usize,
    json: bool,
    neural_model: Option<PathBuf>,
    lambdas: Vec<f64>,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut arguments = arguments.peekable();
        let Some(format) = arguments.next() else {
            return Err(usage());
        };
        if format != "ajimee" {
            return Err(format!(
                "unsupported evaluation format {format:?}\n{}",
                usage()
            ));
        }
        let Some(input) = arguments.next() else {
            return Err(usage());
        };
        let mut options = Self {
            input: PathBuf::from(input),
            dataset_revision: env::var("AJIMEE_BENCH_REVISION").ok(),
            dataset_sha256: env::var("AJIMEE_BENCH_SHA256").ok(),
            top_k: 10,
            context: ContextFilter::All,
            limit: None,
            failures: 10,
            json: false,
            neural_model: None,
            lambdas: Vec::new(),
        };

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--top-k" => options.top_k = parse_positive("--top-k", arguments.next())?,
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
                "--help" | "-h" => return Err(usage()),
                _ => return Err(format!("unknown argument {argument:?}\n{}", usage())),
            }
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
    "usage: ime-evaluate ajimee <evaluation_items.json> [--top-k N] \
     [--context all|none|present] [--limit N] [--failures N] [--json] \
     [--neural-model model.gguf] [--lambda X]...\n\
     --neural-model rescores the N-best with a zenz GGUF model (requires \
     building with --features neural). --lambda selects interpolation \
     weights; without it a default sweep runs."
        .to_owned()
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
    items: usize,
    top_k: usize,
    accuracy_at_1: f64,
    accuracy_at_k: f64,
    mrr_at_k: f64,
    min_cer_at_1: f64,
    min_cer_at_k: f64,
    latency_ms: LatencyReport,
    failures: Vec<Failure>,
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
) -> Result<Vec<EvaluationReport>, String> {
    let selected: Vec<_> = items
        .iter()
        .filter(|item| options.context.includes(item))
        .take(options.limit.unwrap_or(usize::MAX))
        .collect();
    if selected.is_empty() {
        return Err("no evaluation items matched the selected filters".to_owned());
    }

    let mut outcomes = Vec::with_capacity(selected.len());
    for item in selected {
        if item.expected_output.is_empty() {
            return Err(format!("item {} has no expected output", item.index));
        }
        let reading = katakana_to_hiragana(&item.input);
        let started = Instant::now();
        let candidates: Vec<_> = dictionary
            .candidates(&reading)
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

    let Some(model_path) = &options.neural_model else {
        return Ok(vec![compute_report(&outcomes, None, None, options)]);
    };

    #[cfg(not(feature = "neural"))]
    {
        let _ = model_path;
        Err("--neural-model requires building ime-tools with --features neural".to_owned())
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
            .map(|&lambda| compute_report(&outcomes, Some(&neural), Some(lambda), options))
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

        let rank = candidates.iter().position(|candidate| {
            item.expected_output
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

        min_cer_at_1 += candidates.first().map_or(1.0, |candidate| {
            minimum_cer(&item.expected_output, candidate)
        });
        min_cer_at_k += candidates
            .iter()
            .map(|candidate| minimum_cer(&item.expected_output, candidate))
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
        dataset: "AJIMEE-Bench JWTD_v2/v1",
        dataset_revision: options.dataset_revision.clone(),
        dataset_sha256: options.dataset_sha256.clone(),
        context_filter: options.context,
        context_used_by_engine: neural.is_some(),
        neural_model: options
            .neural_model
            .as_ref()
            .map(|path| path.display().to_string()),
        lambda,
        items: outcomes.len(),
        top_k: options.top_k,
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
    println!("items: {}", report.items);
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

#[cfg(test)]
mod tests {
    use super::{ContextFilter, Options, character_error_rate, katakana_to_hiragana, percentile};
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
                "items.json",
                "--top-k",
                "5",
                "--context",
                "none",
                "--limit",
                "25",
                "--failures",
                "0",
                "--json",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(options.top_k, 5);
        assert_eq!(options.context, ContextFilter::None);
        assert_eq!(options.limit, Some(25));
        assert_eq!(options.failures, 0);
        assert!(options.json);
    }
}
