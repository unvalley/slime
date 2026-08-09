//! Offline evaluation tools for kana-kanji conversion quality.

mod corpus_bigram;
mod discriminative;
#[cfg(feature = "neural")]
use slime_neural as neural;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use corpus_bigram::{
    CorpusBigramRanker, Diagnostics as BigramDiagnostics, TransitionDiagnostics,
    parse_annotated_corpus_line,
};
use serde::{Deserialize, Serialize};
use slime_converter::{Candidate, CandidateRanker, Dictionary};

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
    let word_bigram_ranker = options
        .uses_corpus_ranker()
        .then(|| {
            CorpusBigramRanker::load(
                &options.word_bigram_corpora,
                options.word_bigram_weight,
                options.skip_bigram_weight,
                options.context_bigram_weight,
                options.corpus_bigram_min_count,
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
        println!("ranking parameter sweep:");
        for report in &reports {
            let parameter = report.discriminative_weight.map_or_else(
                || {
                    format!(
                        "lambda={:.2} min_margin={:.2} score_mode={}",
                        report.lambda.unwrap_or(0.0),
                        report.neural_min_margin.unwrap_or(0.0),
                        report
                            .neural_score_mode
                            .unwrap_or(NeuralScoreMode::Total)
                            .as_str()
                    )
                },
                |weight| format!("discriminative_weight={weight:.3}"),
            );
            println!(
                "  {parameter} acc@1={:.4} acc@{}={:.4} mrr@{}={:.4} mincer@1={:.4} \
                 latency p50={:.3} p95={:.3}",
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
        if let Some(weight) = best.discriminative_weight {
            println!("best discriminative weight={weight:.3}:");
        } else {
            println!("best lambda={:.2}:", best.lambda.unwrap_or(0.0));
        }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
enum NeuralScoreMode {
    #[serde(rename = "with_eos")]
    Total,
    #[serde(rename = "without_eos")]
    Candidate,
    #[serde(rename = "mean_with_eos")]
    MeanTotal,
    #[serde(rename = "mean_without_eos")]
    MeanCandidate,
}

impl NeuralScoreMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Total => "with-eos",
            Self::Candidate => "without-eos",
            Self::MeanTotal => "mean-with-eos",
            Self::MeanCandidate => "mean-without-eos",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "with-eos" => Ok(Self::Total),
            "without-eos" => Ok(Self::Candidate),
            "mean-with-eos" => Ok(Self::MeanTotal),
            "mean-without-eos" => Ok(Self::MeanCandidate),
            _ => Err(format!(
                "unknown neural score mode {value:?}; expected with-eos, without-eos, \
                 mean-with-eos, or mean-without-eos"
            )),
        }
    }
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
    Annotated,
}

impl DatasetFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ajimee" => Ok(Self::Ajimee),
            "anthy" => Ok(Self::Anthy),
            "annotated" => Ok(Self::Annotated),
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
            Self::Annotated => "Annotated surface/reading corpus",
        }
    }

    fn revision(self) -> Option<String> {
        match self {
            Self::Ajimee => env::var("AJIMEE_BENCH_REVISION").ok(),
            Self::Anthy => env::var("ANTHY_CORPUS_REVISION").ok(),
            Self::Annotated => None,
        }
    }

    fn sha256(self) -> Option<String> {
        match self {
            Self::Ajimee => env::var("AJIMEE_BENCH_SHA256").ok(),
            Self::Anthy => env::var("ANTHY_CORPUS_SHA256").ok(),
            Self::Annotated => None,
        }
    }
}

#[derive(Debug)]
struct Options {
    format: DatasetFormat,
    inputs: Vec<PathBuf>,
    dataset_name: Option<String>,
    dataset_revision: Option<String>,
    dataset_sha256: Option<String>,
    top_k: usize,
    search_k: Option<usize>,
    context: ContextFilter,
    limit: Option<usize>,
    failures: usize,
    json: bool,
    neural_model: Option<PathBuf>,
    neural_max_cost_gap: Option<i32>,
    neural_max_candidates: Option<usize>,
    neural_long_input_min_characters: Option<usize>,
    neural_long_input_max_candidates: Option<usize>,
    neural_max_candidate_cost_gap: Option<i32>,
    neural_long_input_max_candidate_cost_gap: Option<i32>,
    lambdas: Vec<f64>,
    neural_min_margins: Vec<f64>,
    neural_score_modes: Vec<NeuralScoreMode>,
    discriminative_train: Option<PathBuf>,
    discriminative_teacher_model: Option<PathBuf>,
    discriminative_teacher_lambda: f64,
    export_nbest: Option<PathBuf>,
    discriminative_export_training: Option<PathBuf>,
    discriminative_export_evaluation: Option<PathBuf>,
    discriminative_train_limit: usize,
    discriminative_dimensions: usize,
    discriminative_epochs: usize,
    discriminative_weights: Vec<f32>,
    word_bigram_corpora: Vec<PathBuf>,
    word_bigram_weight: i32,
    skip_bigram_weight: i32,
    context_bigram_weight: i32,
    corpus_bigram_min_count: u32,
}

impl Options {
    #[allow(clippy::too_many_lines)]
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut arguments = arguments.peekable();
        let Some(format) = arguments.next() else {
            return Err(usage());
        };
        let format = DatasetFormat::parse(&format)?;
        let mut options = Self {
            format,
            inputs: Vec::new(),
            dataset_name: None,
            dataset_revision: format.revision(),
            dataset_sha256: format.sha256(),
            top_k: 10,
            search_k: None,
            context: ContextFilter::All,
            limit: None,
            failures: 10,
            json: false,
            neural_model: None,
            neural_max_cost_gap: None,
            neural_max_candidates: None,
            neural_long_input_min_characters: None,
            neural_long_input_max_candidates: None,
            neural_max_candidate_cost_gap: None,
            neural_long_input_max_candidate_cost_gap: None,
            lambdas: Vec::new(),
            neural_min_margins: Vec::new(),
            neural_score_modes: Vec::new(),
            discriminative_train: None,
            discriminative_teacher_model: None,
            discriminative_teacher_lambda: 0.8,
            export_nbest: None,
            discriminative_export_training: None,
            discriminative_export_evaluation: None,
            discriminative_train_limit: 10_000,
            discriminative_dimensions: 1 << 18,
            discriminative_epochs: 3,
            discriminative_weights: Vec::new(),
            word_bigram_corpora: Vec::new(),
            word_bigram_weight: 0,
            skip_bigram_weight: 0,
            context_bigram_weight: 0,
            corpus_bigram_min_count: 1,
        };

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--input" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--input requires a path".to_owned())?;
                    options.inputs.push(PathBuf::from(value));
                }
                "--dataset-name" => {
                    options.dataset_name = Some(
                        arguments
                            .next()
                            .ok_or_else(|| "--dataset-name requires a value".to_owned())?,
                    );
                }
                "--dataset-revision" => {
                    options.dataset_revision = Some(
                        arguments
                            .next()
                            .ok_or_else(|| "--dataset-revision requires a value".to_owned())?,
                    );
                }
                "--dataset-sha256" => {
                    options.dataset_sha256 = Some(
                        arguments
                            .next()
                            .ok_or_else(|| "--dataset-sha256 requires a value".to_owned())?,
                    );
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
                "--neural-max-cost-gap" => {
                    options.neural_max_cost_gap = Some(parse_non_negative_i32(
                        "--neural-max-cost-gap",
                        arguments.next(),
                    )?);
                }
                "--neural-max-candidates" => {
                    let maximum = parse_positive("--neural-max-candidates", arguments.next())?;
                    if maximum < 2 {
                        return Err("--neural-max-candidates must be at least 2".to_owned());
                    }
                    options.neural_max_candidates = Some(maximum);
                }
                "--neural-long-input-min-characters" => {
                    options.neural_long_input_min_characters = Some(parse_positive(
                        "--neural-long-input-min-characters",
                        arguments.next(),
                    )?);
                }
                "--neural-long-input-max-candidates" => {
                    let maximum =
                        parse_positive("--neural-long-input-max-candidates", arguments.next())?;
                    if maximum < 2 {
                        return Err(
                            "--neural-long-input-max-candidates must be at least 2".to_owned()
                        );
                    }
                    options.neural_long_input_max_candidates = Some(maximum);
                }
                "--neural-max-candidate-cost-gap" => {
                    options.neural_max_candidate_cost_gap = Some(parse_non_negative_i32(
                        "--neural-max-candidate-cost-gap",
                        arguments.next(),
                    )?);
                }
                "--neural-long-input-max-candidate-cost-gap" => {
                    options.neural_long_input_max_candidate_cost_gap =
                        Some(parse_non_negative_i32(
                            "--neural-long-input-max-candidate-cost-gap",
                            arguments.next(),
                        )?);
                }
                "--lambda" => options.lambdas.push(parse_lambda(arguments.next())?),
                "--neural-min-margin" => options.neural_min_margins.push(parse_non_negative_f64(
                    "--neural-min-margin",
                    arguments.next(),
                )?),
                "--neural-score-mode" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--neural-score-mode requires a value".to_owned())?;
                    options
                        .neural_score_modes
                        .push(NeuralScoreMode::parse(&value)?);
                }
                "--discriminative-train" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--discriminative-train requires a path".to_owned())?;
                    options.discriminative_train = Some(PathBuf::from(value));
                }
                "--discriminative-teacher-model" => {
                    let value = arguments.next().ok_or_else(|| {
                        "--discriminative-teacher-model requires a path".to_owned()
                    })?;
                    options.discriminative_teacher_model = Some(PathBuf::from(value));
                }
                "--discriminative-teacher-lambda" => {
                    options.discriminative_teacher_lambda = parse_lambda(arguments.next())?;
                }
                "--export-nbest" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--export-nbest requires a path".to_owned())?;
                    options.export_nbest = Some(PathBuf::from(value));
                }
                "--discriminative-export-training" => {
                    let value = arguments.next().ok_or_else(|| {
                        "--discriminative-export-training requires a path".to_owned()
                    })?;
                    options.discriminative_export_training = Some(PathBuf::from(value));
                }
                "--discriminative-export-evaluation" => {
                    let value = arguments.next().ok_or_else(|| {
                        "--discriminative-export-evaluation requires a path".to_owned()
                    })?;
                    options.discriminative_export_evaluation = Some(PathBuf::from(value));
                }
                "--discriminative-train-limit" => {
                    options.discriminative_train_limit =
                        parse_positive("--discriminative-train-limit", arguments.next())?;
                }
                "--discriminative-dimensions" => {
                    options.discriminative_dimensions =
                        parse_positive("--discriminative-dimensions", arguments.next())?;
                }
                "--discriminative-epochs" => {
                    options.discriminative_epochs =
                        parse_positive("--discriminative-epochs", arguments.next())?;
                }
                "--discriminative-weight" => options.discriminative_weights.push(
                    parse_non_negative_f32("--discriminative-weight", arguments.next())?,
                ),
                "--word-bigram-corpus" => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| "--word-bigram-corpus requires a path".to_owned())?;
                    options.word_bigram_corpora.push(PathBuf::from(value));
                }
                name @ ("--word-bigram-weight"
                | "--skip-bigram-weight"
                | "--context-bigram-weight") => {
                    options.set_ngram_weight(name, arguments.next())?;
                }
                "--corpus-bigram-min-count" => {
                    options.corpus_bigram_min_count = u32::try_from(parse_positive(
                        "--corpus-bigram-min-count",
                        arguments.next(),
                    )?)
                    .map_err(|_| "--corpus-bigram-min-count is too large".to_owned())?;
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
        if options.uses_corpus_ranker() && options.word_bigram_corpora.is_empty() {
            return Err("n-gram weights require --word-bigram-corpus".to_owned());
        }
        if options.neural_long_input_min_characters.is_some()
            != options.neural_long_input_max_candidates.is_some()
        {
            return Err(
                "long-input neural limits require both --neural-long-input-min-characters and --neural-long-input-max-candidates"
                    .to_owned(),
            );
        }
        if options.neural_long_input_min_characters.is_some()
            && options.neural_max_candidates.is_none()
        {
            return Err("long-input neural limits require --neural-max-candidates".to_owned());
        }
        if options.neural_long_input_max_candidate_cost_gap.is_some()
            && (options.neural_long_input_min_characters.is_none()
                || options.neural_max_candidate_cost_gap.is_none())
        {
            return Err(
                "long-input neural candidate cost gap requires both --neural-long-input-min-characters and --neural-max-candidate-cost-gap"
                    .to_owned(),
            );
        }
        if let (Some(short), Some(long)) = (
            options.neural_max_candidates,
            options.neural_long_input_max_candidates,
        ) && long < short
        {
            return Err(
                "--neural-long-input-max-candidates must be at least --neural-max-candidates"
                    .to_owned(),
            );
        }
        if (options.neural_max_cost_gap.is_some()
            || options.neural_max_candidates.is_some()
            || options.neural_long_input_min_characters.is_some()
            || options.neural_long_input_max_candidates.is_some()
            || options.neural_max_candidate_cost_gap.is_some()
            || options.neural_long_input_max_candidate_cost_gap.is_some()
            || !options.neural_min_margins.is_empty()
            || !options.neural_score_modes.is_empty())
            && options.neural_model.is_none()
        {
            return Err("neural scoring options require --neural-model".to_owned());
        }
        if options.neural_model.is_some() && options.discriminative_train.is_some() {
            return Err(
                "neural and discriminative rerankers cannot be evaluated together".to_owned(),
            );
        }
        if options.discriminative_teacher_model.is_some() && options.discriminative_train.is_none()
        {
            return Err(
                "--discriminative-teacher-model requires --discriminative-train".to_owned(),
            );
        }
        if (options.discriminative_export_training.is_some()
            || options.discriminative_export_evaluation.is_some())
            && options.discriminative_train.is_none()
        {
            return Err("discriminative export requires --discriminative-train".to_owned());
        }
        if options.discriminative_train.is_some()
            && !options.discriminative_dimensions.is_power_of_two()
        {
            return Err("--discriminative-dimensions must be a power of two".to_owned());
        }
        if options.lambdas.is_empty() {
            // Default sweep for tuning the interpolation weight on the devset.
            options.lambdas = (0..=10).map(|step| f64::from(step) / 10.0).collect();
            options.lambdas.push(0.95);
            options.lambdas.sort_by(f64::total_cmp);
        }
        if options.neural_min_margins.is_empty() {
            options.neural_min_margins.push(0.0);
        }
        if options.neural_score_modes.is_empty() {
            options.neural_score_modes.push(NeuralScoreMode::Total);
        }
        if options.discriminative_weights.is_empty() {
            options.discriminative_weights = vec![0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0];
        }
        Ok(options)
    }

    const fn uses_corpus_ranker(&self) -> bool {
        self.word_bigram_weight > 0 || self.skip_bigram_weight > 0 || self.context_bigram_weight > 0
    }

    fn set_ngram_weight(&mut self, name: &str, value: Option<String>) -> Result<(), String> {
        let parsed = parse_non_negative_i32(name, value)?;
        match name {
            "--word-bigram-weight" => self.word_bigram_weight = parsed,
            "--skip-bigram-weight" => self.skip_bigram_weight = parsed,
            "--context-bigram-weight" => self.context_bigram_weight = parsed,
            _ => unreachable!("matched n-gram option"),
        }
        Ok(())
    }
}

fn usage() -> String {
    "usage: slime-evaluate <ajimee|anthy|annotated> --input <path> [--input <path> ...] \
     [--dataset-name NAME] [--dataset-revision REV] [--dataset-sha256 HEX] [--top-k N] \
     [--search-k N] \
     [--context all|none|present] [--limit N] [--failures N] [--json] \
     [--neural-model model.gguf] [--neural-max-cost-gap N] \
     [--neural-max-candidates N] [--neural-max-candidate-cost-gap N] \
     [--neural-long-input-min-characters N] \
     [--neural-long-input-max-candidates N] \
     [--neural-long-input-max-candidate-cost-gap N] \
     [--lambda X]... [--neural-min-margin X]... \
     [--neural-score-mode with-eos|without-eos|mean-with-eos|mean-without-eos]... \
     [--export-nbest path] \
     [--discriminative-train items.json] [--discriminative-train-limit N] \
     [--discriminative-teacher-model model.gguf] [--discriminative-teacher-lambda X] \
     [--discriminative-export-training path] [--discriminative-export-evaluation path] \
     [--discriminative-dimensions N] [--discriminative-epochs N] \
     [--discriminative-weight X]... \
     [--word-bigram-corpus corpus.txt] [--word-bigram-weight N] \
     [--skip-bigram-weight N] [--context-bigram-weight N] \
     [--corpus-bigram-min-count N]\n\
     --neural-model rescores the N-best with a zenz GGUF model (requires \
     building with --features neural). --neural-max-cost-gap skips neural \
     scoring when the base top-two cost gap exceeds N. --neural-max-candidates \
     restricts neural reordering to the first N lattice candidates while \
     preserving the remaining base order. The paired long-input options replace \
     that limit once the input reaches the configured character count. \
     --neural-max-candidate-cost-gap \
     further restricts that prefix to candidates no more than N cost units \
     above the base winner. The corresponding long-input option replaces that \
     cost gap at the same character boundary. --lambda selects interpolation \
     weights; without it a default sweep runs. --neural-min-margin accepts a \
     new top candidate only when its interpolated score exceeds the base top by \
     at least X; it is repeatable and defaults to zero. --neural-score-mode \
     controls EOS inclusion and token-length normalization, is repeatable, and \
     defaults to with-eos. The discriminative \
     options train an evaluation-only hashed averaged perceptron on a disjoint \
     AJIMEE-format training file. The optional annotated corpus \
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

fn parse_non_negative_f32(name: &str, value: Option<String>) -> Result<f32, String> {
    let value = value.ok_or_else(|| format!("{name} requires a value"))?;
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("{name} requires a non-negative number"))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!("{name} requires a non-negative number"));
    }
    Ok(parsed)
}

fn load_items(options: &Options) -> Result<Vec<AjimeeItem>, String> {
    load_items_from_paths(options.format, &options.inputs)
}

fn load_items_from_paths(
    format: DatasetFormat,
    paths: &[PathBuf],
) -> Result<Vec<AjimeeItem>, String> {
    match format {
        DatasetFormat::Ajimee if paths.len() == 1 => load_ajimee_items(&paths[0]),
        DatasetFormat::Ajimee => Err("ajimee format requires exactly one input".to_owned()),
        DatasetFormat::Anthy => load_anthy_items(paths),
        DatasetFormat::Annotated => load_annotated_items(paths),
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
                source_split: None,
                index: format!("{}:{}", path.display(), line_index + 1),
                context_text: String::new(),
                right_context_text: String::new(),
                input: reading,
                expected_output: vec![expected],
            });
        }
    }
    Ok(items)
}

fn load_annotated_items(paths: &[PathBuf]) -> Result<Vec<AjimeeItem>, String> {
    let mut items = Vec::new();
    for path in paths {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        for (line_index, line) in source.lines().enumerate() {
            let tokens = parse_annotated_corpus_line(line)
                .map_err(|error| format!("{}:{}: {error}", path.display(), line_index + 1))?;
            if tokens.is_empty() {
                continue;
            }
            let mut input = String::new();
            let mut expected = String::new();
            for (surface, reading) in tokens {
                input.push_str(&reading);
                expected.push_str(&surface);
            }
            items.push(AjimeeItem {
                source_split: None,
                index: format!("{}:{}", path.display(), line_index + 1),
                context_text: String::new(),
                right_context_text: String::new(),
                input,
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

fn parse_non_negative_f64(name: &str, value: Option<String>) -> Result<f64, String> {
    let parsed: f64 = value
        .ok_or_else(|| format!("{name} requires a value"))?
        .parse()
        .map_err(|_| format!("{name} requires a non-negative number"))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!("{name} requires a finite non-negative number"));
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
    #[serde(default)]
    source_split: Option<String>,
    index: String,
    context_text: String,
    #[serde(default)]
    #[cfg_attr(not(feature = "neural"), allow(dead_code))]
    right_context_text: String,
    input: String,
    expected_output: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EvaluationReport {
    dataset: String,
    dataset_revision: Option<String>,
    dataset_sha256: Option<String>,
    context_filter: ContextFilter,
    context_used_by_engine: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_max_cost_gap: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_max_candidates: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_long_input_min_characters: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_long_input_max_candidates: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_max_candidate_cost_gap: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_long_input_max_candidate_cost_gap: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_scored_items: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_skipped_items: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_scored_candidates: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_mean_candidates_per_scored_item: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lambda: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_min_margin: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    neural_score_mode: Option<NeuralScoreMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discriminative: Option<DiscriminativeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discriminative_weight: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    word_bigram: Option<NgramReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skip_bigram: Option<NgramReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_bigram: Option<NgramReport>,
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

#[derive(Clone, Debug, Serialize)]
struct DiscriminativeReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    teacher_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    teacher_lambda: Option<f64>,
    dimensions: usize,
    epochs: usize,
    training_items: usize,
    oracle_items: usize,
    updates: usize,
    nonzero_weights: usize,
    model_bytes: usize,
    scoring_latency_ms: LatencyReport,
}

impl DiscriminativeReport {
    fn new(outcome: &DiscriminativeOutcome) -> Self {
        let diagnostics = outcome.diagnostics;
        Self {
            teacher_model: outcome.teacher_model.clone(),
            teacher_lambda: outcome.teacher_lambda,
            dimensions: diagnostics.dimensions,
            epochs: diagnostics.epochs,
            training_items: diagnostics.training_items,
            oracle_items: diagnostics.oracle_items,
            updates: diagnostics.updates,
            nonzero_weights: diagnostics.nonzero_weights,
            model_bytes: diagnostics.model_bytes,
            scoring_latency_ms: latency_report(outcome.latencies.clone()),
        }
    }
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

fn context_bigram_report(diagnostics: Option<BigramDiagnostics>) -> Option<NgramReport> {
    diagnostics.and_then(|diagnostics| {
        diagnostics
            .context
            .map(|context| NgramReport::new(context, diagnostics.candidates_scored))
    })
}

#[derive(Clone, Copy, Debug, Serialize)]
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

#[derive(Serialize)]
struct NbestExport {
    label_source: String,
    items: Vec<NbestExportItem>,
}

#[derive(Serialize)]
struct NbestExportItem {
    source_split: Option<String>,
    index: String,
    context_text: String,
    right_context_text: String,
    input: String,
    input_characters: usize,
    candidate_generation_ms: f64,
    expected_output: Vec<String>,
    label_index: Option<usize>,
    candidates: Vec<NbestExportCandidate>,
}

#[derive(Serialize)]
struct NbestExportCandidate {
    surface: String,
    cost: i32,
}

fn evaluate(
    dictionary: &Dictionary,
    items: &[AjimeeItem],
    options: &Options,
    word_bigram_ranker: Option<&CorpusBigramRanker>,
) -> Result<Vec<EvaluationReport>, String> {
    let ranker = word_bigram_ranker.map(|ranker| ranker as &dyn CandidateRanker);
    let selected: Vec<_> = items
        .iter()
        .filter(|item| options.context.includes(item))
        .take(options.limit.unwrap_or(usize::MAX))
        .collect();
    if selected.is_empty() {
        return Err("no evaluation items matched the selected filters".to_owned());
    }

    let outcomes = generate_outcomes(
        dictionary,
        &selected,
        options.search_k.unwrap_or(options.top_k),
        options.top_k,
        ranker,
        None,
    )?;
    if let Some(path) = &options.export_nbest {
        export_nbest(path, "gold", &outcomes, None)?;
    }
    let word_bigram_diagnostics = word_bigram_ranker.map(CorpusBigramRanker::diagnostics);

    if let Some(training_path) = &options.discriminative_train {
        let discriminative =
            train_discriminative(dictionary, training_path, &outcomes, options, ranker)?;
        return Ok(options
            .discriminative_weights
            .iter()
            .map(|&weight| {
                compute_report(
                    &outcomes,
                    None,
                    Some(&discriminative),
                    Some(weight),
                    options,
                    word_bigram_diagnostics,
                )
            })
            .collect());
    }

    let Some(model_path) = &options.neural_model else {
        return Ok(vec![compute_report(
            &outcomes,
            None,
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
        let neural = score_neural_outcomes(
            &rescorer,
            &outcomes,
            options.neural_max_cost_gap,
            options.neural_max_candidates,
            options.neural_long_input_min_characters,
            options.neural_long_input_max_candidates,
            options.neural_max_candidate_cost_gap,
            options.neural_long_input_max_candidate_cost_gap,
        )?;
        let mut reports = Vec::with_capacity(
            options.lambdas.len()
                * options.neural_score_modes.len()
                * options.neural_min_margins.len(),
        );
        for &lambda in &options.lambdas {
            for &score_mode in &options.neural_score_modes {
                for &margin in &options.neural_min_margins {
                    reports.push(compute_report(
                        &outcomes,
                        Some(NeuralReportConfig {
                            outcome: &neural,
                            lambda,
                            min_margin: margin,
                            score_mode,
                        }),
                        None,
                        None,
                        options,
                        word_bigram_diagnostics,
                    ));
                }
            }
        }
        Ok(reports)
    }
}

fn generate_outcomes<'a>(
    dictionary: &Dictionary,
    items: &[&'a AjimeeItem],
    search_k: usize,
    top_k: usize,
    ranker: Option<&dyn CandidateRanker>,
    progress_label: Option<&str>,
) -> Result<Vec<ItemOutcome<'a>>, String> {
    let mut outcomes = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if item.expected_output.is_empty() {
            return Err(format!("item {} has no expected output", item.index));
        }
        let reading = katakana_to_hiragana(&item.input);
        let started = Instant::now();
        let generated = match ranker {
            Some(ranker) => dictionary.candidates_with_context_ranker(
                &reading,
                &item.context_text,
                search_k,
                ranker,
            ),
            None => dictionary.candidates_with_surrounding_context_limit(
                &reading,
                &item.context_text,
                &item.right_context_text,
                search_k,
            ),
        };
        let candidates: Vec<_> = generated.into_iter().take(top_k).collect();
        let latency = started.elapsed();
        outcomes.push(ItemOutcome {
            item,
            candidates,
            latency,
        });
        if let Some(label) = progress_label
            && (index + 1).is_multiple_of(1_000)
        {
            eprintln!(
                "{label}: generated {}/{} candidate sets",
                index + 1,
                items.len()
            );
        }
    }
    Ok(outcomes)
}

struct DiscriminativeOutcome {
    scores: Vec<Vec<f32>>,
    latencies: Vec<Duration>,
    diagnostics: discriminative::Diagnostics,
    teacher_model: Option<String>,
    teacher_lambda: Option<f64>,
}

fn train_discriminative(
    dictionary: &Dictionary,
    training_path: &Path,
    evaluation_outcomes: &[ItemOutcome<'_>],
    options: &Options,
    ranker: Option<&dyn CandidateRanker>,
) -> Result<DiscriminativeOutcome, String> {
    let training_items = load_ajimee_items(training_path)?;
    let excluded: HashSet<(Option<&str>, &str)> = evaluation_outcomes
        .iter()
        .map(|outcome| {
            (
                outcome.item.source_split.as_deref(),
                outcome.item.index.as_str(),
            )
        })
        .collect();
    let eligible: Vec<_> = training_items
        .iter()
        .filter(|item| !excluded.contains(&(item.source_split.as_deref(), item.index.as_str())))
        .collect();
    let count = options.discriminative_train_limit.min(eligible.len());
    if count == 0 {
        return Err("no non-overlapping discriminative training items remain".to_owned());
    }
    let selected: Vec<_> = (0..count)
        .map(|index| &eligible[index * eligible.len() / count])
        .copied()
        .collect();
    eprintln!(
        "discriminative training: selected {} of {} non-overlapping items",
        selected.len(),
        eligible.len()
    );
    let outcomes = generate_outcomes(
        dictionary,
        &selected,
        options.search_k.unwrap_or(options.top_k),
        options.top_k,
        ranker,
        Some("discriminative training"),
    )?;
    let teacher_expected = options
        .discriminative_teacher_model
        .as_ref()
        .map(|path| {
            discriminative_teacher_expected(&outcomes, path, options.discriminative_teacher_lambda)
        })
        .transpose()?;
    if let Some(path) = &options.discriminative_export_training {
        let label_source = options.discriminative_teacher_model.as_ref().map_or_else(
            || "gold".to_owned(),
            |model| format!("teacher:{}", model.display()),
        );
        export_nbest(path, &label_source, &outcomes, teacher_expected.as_deref())?;
    }
    if let Some(path) = &options.discriminative_export_evaluation {
        export_nbest(path, "gold", evaluation_outcomes, None)?;
    }
    let training: Vec<_> = outcomes
        .iter()
        .enumerate()
        .map(|(index, outcome)| discriminative::TrainingItem {
            context: &outcome.item.context_text,
            candidates: &outcome.candidates,
            expected: teacher_expected
                .as_ref()
                .map_or(outcome.item.expected_output.as_slice(), |expected| {
                    expected[index].as_slice()
                }),
        })
        .collect();
    let model = discriminative::HashedPerceptron::train(
        &training,
        options.discriminative_dimensions,
        options.discriminative_epochs,
    );
    let diagnostics = model.diagnostics();
    eprintln!("discriminative diagnostics: {diagnostics:?}");
    let scored: Vec<_> = evaluation_outcomes
        .iter()
        .map(|outcome| model.score(&outcome.item.context_text, &outcome.candidates))
        .collect();
    Ok(DiscriminativeOutcome {
        scores: scored.iter().map(|item| item.scores.clone()).collect(),
        latencies: scored.iter().map(|item| item.latency).collect(),
        diagnostics,
        teacher_model: options
            .discriminative_teacher_model
            .as_ref()
            .map(|path| path.display().to_string()),
        teacher_lambda: options
            .discriminative_teacher_model
            .as_ref()
            .map(|_| options.discriminative_teacher_lambda),
    })
}

fn export_nbest(
    path: &Path,
    label_source: &str,
    outcomes: &[ItemOutcome<'_>],
    expected_override: Option<&[Vec<String>]>,
) -> Result<(), String> {
    let items = outcomes
        .iter()
        .enumerate()
        .map(|(index, outcome)| {
            let expected = expected_override
                .map_or(outcome.item.expected_output.as_slice(), |expected| {
                    expected[index].as_slice()
                });
            NbestExportItem {
                source_split: outcome.item.source_split.clone(),
                index: outcome.item.index.clone(),
                context_text: outcome.item.context_text.clone(),
                right_context_text: outcome.item.right_context_text.clone(),
                input: outcome.item.input.clone(),
                input_characters: outcome.item.input.chars().count(),
                candidate_generation_ms: duration_to_millis(outcome.latency),
                expected_output: expected.to_vec(),
                label_index: outcome
                    .candidates
                    .iter()
                    .position(|candidate| expected.contains(&candidate.surface)),
                candidates: outcome
                    .candidates
                    .iter()
                    .map(|candidate| NbestExportCandidate {
                        surface: candidate.surface.clone(),
                        cost: candidate.cost,
                    })
                    .collect(),
            }
        })
        .collect();
    let export = NbestExport {
        label_source: label_source.to_owned(),
        items,
    };
    let bytes = serde_json::to_vec(&export)
        .map_err(|error| format!("failed to serialize {}: {error}", path.display()))?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, bytes)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    eprintln!(
        "wrote {} N-best items to {}",
        outcomes.len(),
        path.display()
    );
    Ok(())
}

fn discriminative_teacher_expected(
    outcomes: &[ItemOutcome<'_>],
    model_path: &Path,
    lambda: f64,
) -> Result<Vec<Vec<String>>, String> {
    #[cfg(not(feature = "neural"))]
    {
        let _ = (outcomes, model_path, lambda);
        Err("--discriminative-teacher-model requires building with --features neural".to_owned())
    }
    #[cfg(feature = "neural")]
    {
        eprintln!(
            "discriminative teacher: scoring {} candidate sets with {}",
            outcomes.len(),
            model_path.display()
        );
        let rescorer = neural::Rescorer::load(model_path)?;
        let neural =
            score_neural_outcomes(&rescorer, outcomes, None, None, None, None, None, None)?;
        Ok(outcomes
            .iter()
            .enumerate()
            .map(|(index, outcome)| {
                let surface =
                    rescored_surfaces(&outcome.candidates, &neural.with_eos[index], lambda, 0.0)
                        .into_iter()
                        .next()
                        .unwrap_or_default();
                vec![surface]
            })
            .collect())
    }
}

struct NeuralOutcome {
    with_eos: Vec<Vec<f64>>,
    without_eos: Vec<Vec<f64>>,
    mean_with_eos: Vec<Vec<f64>>,
    mean_without_eos: Vec<Vec<f64>>,
    latencies: Vec<Duration>,
    scored_items: usize,
    scored_candidates: usize,
}

impl NeuralOutcome {
    fn scores(&self, index: usize, mode: NeuralScoreMode) -> &[f64] {
        match mode {
            NeuralScoreMode::Total => &self.with_eos[index],
            NeuralScoreMode::Candidate => &self.without_eos[index],
            NeuralScoreMode::MeanTotal => &self.mean_with_eos[index],
            NeuralScoreMode::MeanCandidate => &self.mean_without_eos[index],
        }
    }
}

#[derive(Clone, Copy)]
struct NeuralReportConfig<'a> {
    outcome: &'a NeuralOutcome,
    lambda: f64,
    min_margin: f64,
    score_mode: NeuralScoreMode,
}

#[cfg(feature = "neural")]
fn score_neural_outcomes(
    rescorer: &neural::Rescorer,
    outcomes: &[ItemOutcome<'_>],
    max_cost_gap: Option<i32>,
    max_candidates: Option<usize>,
    long_input_min_characters: Option<usize>,
    long_input_max_candidates: Option<usize>,
    max_candidate_cost_gap: Option<i32>,
    long_input_max_candidate_cost_gap: Option<i32>,
) -> Result<NeuralOutcome, String> {
    let candidate_counts: Vec<_> = outcomes
        .iter()
        .map(|outcome| {
            if should_score_neurally(&outcome.candidates, max_cost_gap) {
                neural_candidate_prefix_len(
                    &outcome.candidates,
                    neural_candidate_limit(
                        &outcome.item.input,
                        max_candidates,
                        long_input_min_characters,
                        long_input_max_candidates,
                    ),
                    neural_candidate_cost_gap(
                        &outcome.item.input,
                        max_candidate_cost_gap,
                        long_input_min_characters,
                        long_input_max_candidate_cost_gap,
                    ),
                )
            } else {
                0
            }
        })
        .collect();
    let requests: Vec<_> = outcomes
        .iter()
        .zip(&candidate_counts)
        .filter(|(_, candidate_count)| **candidate_count >= 2)
        .map(|(outcome, &candidate_count)| neural::ScoreRequest {
            context: outcome.item.context_text.clone(),
            right_context: outcome.item.right_context_text.clone(),
            input_katakana: outcome.item.input.clone(),
            candidates: outcome
                .candidates
                .iter()
                .take(candidate_count)
                .map(|candidate| candidate.surface.clone())
                .collect(),
        })
        .collect();
    let scored = rescorer.score_all(&requests)?;
    let mut scored = scored.into_iter();
    let mut with_eos = Vec::with_capacity(outcomes.len());
    let mut without_eos = Vec::with_capacity(outcomes.len());
    let mut mean_with_eos = Vec::with_capacity(outcomes.len());
    let mut mean_without_eos = Vec::with_capacity(outcomes.len());
    let mut latencies = Vec::with_capacity(outcomes.len());
    for &candidate_count in &candidate_counts {
        if candidate_count >= 2 {
            let item = scored.next().expect("one score per selected request");
            mean_with_eos.push(mean_logliks(
                &item.logliks,
                &item.candidate_token_counts,
                true,
            ));
            mean_without_eos.push(mean_logliks(
                &item.candidate_logliks,
                &item.candidate_token_counts,
                false,
            ));
            with_eos.push(item.logliks);
            without_eos.push(item.candidate_logliks);
            latencies.push(item.latency);
        } else {
            with_eos.push(Vec::new());
            without_eos.push(Vec::new());
            mean_with_eos.push(Vec::new());
            mean_without_eos.push(Vec::new());
            latencies.push(Duration::ZERO);
        }
    }
    debug_assert!(scored.next().is_none());
    Ok(NeuralOutcome {
        with_eos,
        without_eos,
        mean_with_eos,
        mean_without_eos,
        latencies,
        scored_items: candidate_counts
            .iter()
            .filter(|&&candidate_count| candidate_count >= 2)
            .count(),
        scored_candidates: candidate_counts
            .iter()
            .filter(|&&candidate_count| candidate_count >= 2)
            .sum(),
    })
}

#[cfg(any(feature = "neural", test))]
fn neural_candidate_limit(
    input: &str,
    max_candidates: Option<usize>,
    long_input_min_characters: Option<usize>,
    long_input_max_candidates: Option<usize>,
) -> Option<usize> {
    match (long_input_min_characters, long_input_max_candidates) {
        (Some(minimum), Some(maximum)) if input.chars().count() >= minimum => Some(maximum),
        _ => max_candidates,
    }
}

#[cfg(any(feature = "neural", test))]
fn neural_candidate_cost_gap(
    input: &str,
    max_candidate_cost_gap: Option<i32>,
    long_input_min_characters: Option<usize>,
    long_input_max_candidate_cost_gap: Option<i32>,
) -> Option<i32> {
    match (long_input_min_characters, long_input_max_candidate_cost_gap) {
        (Some(minimum), Some(maximum)) if input.chars().count() >= minimum => Some(maximum),
        _ => max_candidate_cost_gap,
    }
}

#[cfg(any(feature = "neural", test))]
fn neural_candidate_prefix_len(
    candidates: &[Candidate],
    max_candidates: Option<usize>,
    max_candidate_cost_gap: Option<i32>,
) -> usize {
    let Some(first_cost) = candidates.first().map(|candidate| candidate.cost) else {
        return 0;
    };
    candidates
        .iter()
        .take(max_candidates.unwrap_or(usize::MAX))
        .take_while(|candidate| {
            max_candidate_cost_gap
                .is_none_or(|maximum| candidate.cost.saturating_sub(first_cost).max(0) <= maximum)
        })
        .count()
}

#[cfg(any(feature = "neural", test))]
fn mean_logliks(logliks: &[f64], token_counts: &[usize], includes_eos: bool) -> Vec<f64> {
    debug_assert_eq!(logliks.len(), token_counts.len());
    logliks
        .iter()
        .zip(token_counts)
        .map(|(&loglik, &tokens)| {
            let denominator = tokens + usize::from(includes_eos);
            if denominator == 0 {
                loglik
            } else {
                loglik / usize_to_f64(denominator)
            }
        })
        .collect()
}

#[cfg(any(feature = "neural", test))]
fn base_cost_gap(candidates: &[Candidate]) -> Option<i32> {
    let first = candidates.first()?.cost;
    candidates
        .iter()
        .skip(1)
        .map(|candidate| candidate.cost)
        .min()
        .map(|alternative| alternative.saturating_sub(first).max(0))
}

#[cfg(any(feature = "neural", test))]
fn should_score_neurally(candidates: &[Candidate], max_cost_gap: Option<i32>) -> bool {
    max_cost_gap.is_none_or(|maximum| base_cost_gap(candidates).is_some_and(|gap| gap <= maximum))
}

/// Reorders candidate surfaces by interpolating the lattice cost with the
/// neural log-likelihood: `(1-lambda) * (-cost/scale) + lambda * loglik`.
/// The stable sort keeps the lattice order for ties.
fn rescored_surfaces(
    candidates: &[Candidate],
    logliks: &[f64],
    lambda: f64,
    min_margin: f64,
) -> Vec<String> {
    let scored_candidates = candidates.len().min(logliks.len());
    let mut indexed: Vec<usize> = (0..scored_candidates).collect();
    let combined: Vec<f64> = candidates
        .iter()
        .take(scored_candidates)
        .zip(logliks)
        .map(|(candidate, loglik)| {
            (1.0 - lambda) * (-f64::from(candidate.cost) / COST_LOG_SCALE) + lambda * loglik
        })
        .collect();
    indexed.sort_by(|&a, &b| combined[b].total_cmp(&combined[a]));
    if indexed
        .first()
        .is_some_and(|&top| top != 0 && combined[top] - combined[0] < min_margin)
    {
        return candidates
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect();
    }
    indexed.extend(scored_candidates..candidates.len());
    indexed
        .into_iter()
        .map(|index| candidates[index].surface.clone())
        .collect()
}

#[allow(clippy::too_many_lines)]
fn compute_report(
    outcomes: &[ItemOutcome<'_>],
    neural: Option<NeuralReportConfig<'_>>,
    discriminative: Option<&DiscriminativeOutcome>,
    discriminative_weight: Option<f32>,
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
        let candidates: Vec<String> = match (neural, discriminative, discriminative_weight) {
            (Some(neural), None, None) => rescored_surfaces(
                &outcome.candidates,
                neural.outcome.scores(outcome_index, neural.score_mode),
                neural.lambda,
                neural.min_margin,
            ),
            (None, Some(discriminative), Some(weight)) => discriminative::rescored_surfaces(
                &outcome.candidates,
                &discriminative.scores[outcome_index],
                weight,
            ),
            _ => outcome
                .candidates
                .iter()
                .map(|candidate| candidate.surface.clone())
                .collect(),
        };
        let mut latency = outcome.latency;
        if let Some(neural) = neural {
            latency += neural.outcome.latencies[outcome_index];
        }
        if let Some(discriminative) = discriminative {
            latency += discriminative.latencies[outcome_index];
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
    let built_in_document_context = word_bigram_diagnostics.is_none()
        && outcomes
            .iter()
            .any(|outcome| !outcome.item.context_text.is_empty());
    EvaluationReport {
        dataset: options
            .dataset_name
            .clone()
            .unwrap_or_else(|| options.format.dataset_name().to_owned()),
        dataset_revision: options.dataset_revision.clone(),
        dataset_sha256: options.dataset_sha256.clone(),
        context_filter: options.context,
        context_used_by_engine: built_in_document_context
            || neural.is_some_and(|neural| neural.outcome.scored_items > 0)
            || discriminative.is_some()
            || word_bigram_diagnostics
                .and_then(|diagnostics| diagnostics.context)
                .is_some_and(|diagnostics| diagnostics.matched_transitions > 0),
        neural_model: options
            .neural_model
            .as_ref()
            .map(|path| path.display().to_string()),
        neural_max_cost_gap: options.neural_max_cost_gap,
        neural_max_candidates: options.neural_max_candidates,
        neural_long_input_min_characters: options.neural_long_input_min_characters,
        neural_long_input_max_candidates: options.neural_long_input_max_candidates,
        neural_max_candidate_cost_gap: options.neural_max_candidate_cost_gap,
        neural_long_input_max_candidate_cost_gap: options.neural_long_input_max_candidate_cost_gap,
        neural_scored_items: neural.map(|neural| neural.outcome.scored_items),
        neural_skipped_items: neural.map(|neural| outcomes.len() - neural.outcome.scored_items),
        neural_scored_candidates: neural.map(|neural| neural.outcome.scored_candidates),
        neural_mean_candidates_per_scored_item: neural.map(|neural| {
            if neural.outcome.scored_items == 0 {
                0.0
            } else {
                usize_to_f64(neural.outcome.scored_candidates)
                    / usize_to_f64(neural.outcome.scored_items)
            }
        }),
        lambda: neural.map(|neural| neural.lambda),
        neural_min_margin: neural.map(|neural| neural.min_margin),
        neural_score_mode: neural.map(|neural| neural.score_mode),
        discriminative: discriminative.map(DiscriminativeReport::new),
        discriminative_weight,
        word_bigram: word_bigram_report(word_bigram_diagnostics),
        skip_bigram: skip_bigram_report(word_bigram_diagnostics),
        context_bigram: context_bigram_report(word_bigram_diagnostics),
        items: outcomes.len(),
        top_k: options.top_k,
        search_k: options.search_k.unwrap_or(options.top_k),
        accuracy_at_1: usize_to_f64(correct_at_1) / total,
        accuracy_at_k: usize_to_f64(correct_at_k) / total,
        mrr_at_k: reciprocal_rank / total,
        min_cer_at_1: min_cer_at_1 / total,
        min_cer_at_k: min_cer_at_k / total,
        latency_ms: latency_report(latencies),
        failures,
    }
}

fn normalize_for_evaluation(value: &str, format: DatasetFormat) -> String {
    match format {
        DatasetFormat::Ajimee | DatasetFormat::Annotated => value.to_owned(),
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

fn print_neural_report(report: &EvaluationReport) {
    if let Some(model) = &report.neural_model {
        println!("neural model: {model}");
    }
    if let Some(maximum) = report.neural_max_cost_gap {
        println!("neural max base cost gap: {maximum}");
    }
    if let Some(maximum) = report.neural_max_candidates {
        println!("neural max candidates: {maximum}");
    }
    if let (Some(minimum), Some(maximum)) = (
        report.neural_long_input_min_characters,
        report.neural_long_input_max_candidates,
    ) {
        println!("neural long-input candidates: {maximum} from {minimum} characters");
    }
    if let Some(maximum) = report.neural_max_candidate_cost_gap {
        println!("neural max candidate cost gap: {maximum}");
    }
    if let (Some(minimum), Some(maximum)) = (
        report.neural_long_input_min_characters,
        report.neural_long_input_max_candidate_cost_gap,
    ) {
        println!("neural long-input max candidate cost gap: {maximum} from {minimum} characters");
    }
    if let Some(scored) = report.neural_scored_items {
        println!("neural scored items: {scored}");
        println!(
            "neural skipped items: {}",
            report.neural_skipped_items.unwrap_or(0)
        );
        println!(
            "neural scored candidates: {}",
            report.neural_scored_candidates.unwrap_or(0)
        );
        println!(
            "neural mean candidates per scored item: {:.2}",
            report.neural_mean_candidates_per_scored_item.unwrap_or(0.0)
        );
    }
    if let Some(lambda) = report.lambda {
        println!("lambda: {lambda:.2}");
    }
    if let Some(margin) = report.neural_min_margin {
        println!("neural minimum acceptance margin: {margin:.2}");
    }
    if let Some(mode) = report.neural_score_mode {
        println!("neural score mode: {}", mode.as_str());
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
    print_neural_report(report);
    if let Some(discriminative) = &report.discriminative {
        print_discriminative_report(discriminative);
    }
    if let Some(weight) = report.discriminative_weight {
        println!("discriminative weight: {weight:.3}");
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
    if let Some(bigram) = &report.context_bigram {
        print_context_report("context bigram", bigram);
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

fn print_context_report(label: &str, report: &NgramReport) {
    println!("{label} entries: {}", report.entries);
    println!("{label} weight: {}", report.weight);
    println!("{label} candidates scored: {}", report.candidates_scored);
    println!("{label} transitions scored: {}", report.transitions_scored);
    println!(
        "{label} matched transitions: {}",
        report.matched_transitions
    );
    println!("{label} match rate: {:.4}", report.match_rate);
}

fn print_discriminative_report(report: &DiscriminativeReport) {
    if let Some(model) = &report.teacher_model {
        println!("discriminative teacher model: {model}");
        println!(
            "discriminative teacher lambda: {:.2}",
            report.teacher_lambda.unwrap_or(0.0)
        );
    }
    println!("discriminative dimensions: {}", report.dimensions);
    println!("discriminative epochs: {}", report.epochs);
    println!(
        "discriminative training/oracle items: {}/{}",
        report.training_items, report.oracle_items
    );
    println!("discriminative updates: {}", report.updates);
    println!("discriminative nonzero weights: {}", report.nonzero_weights);
    println!("discriminative model bytes: {}", report.model_bytes);
    println!(
        "discriminative scoring latency ms: p50={:.3} p95={:.3} p99={:.3} max={:.3}",
        report.scoring_latency_ms.p50,
        report.scoring_latency_ms.p95,
        report.scoring_latency_ms.p99,
        report.scoring_latency_ms.max
    );
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

fn latency_report(mut durations: Vec<Duration>) -> LatencyReport {
    durations.sort_unstable();
    LatencyReport {
        p50: percentile(&durations, 50),
        p95: percentile(&durations, 95),
        p99: percentile(&durations, 99),
        max: duration_to_millis(*durations.last().expect("non-empty latencies")),
    }
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
        ContextFilter, DatasetFormat, NeuralScoreMode, Options, base_cost_gap,
        character_error_rate, katakana_to_hiragana, load_annotated_items, mean_logliks,
        neural_candidate_cost_gap, neural_candidate_limit, neural_candidate_prefix_len,
        normalize_for_evaluation, parse_anthy_line, percentile, rescored_surfaces,
        should_score_neurally,
    };
    use slime_converter::Candidate;
    use std::fs;
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
                "--dataset-name",
                "custom",
                "--dataset-revision",
                "revision",
                "--dataset-sha256",
                "digest",
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
                "--neural-model",
                "model.gguf",
                "--neural-max-cost-gap",
                "750",
                "--neural-max-candidates",
                "5",
                "--neural-long-input-min-characters",
                "9",
                "--neural-long-input-max-candidates",
                "8",
                "--neural-max-candidate-cost-gap",
                "900",
                "--neural-long-input-max-candidate-cost-gap",
                "2500",
                "--neural-min-margin",
                "0.25",
                "--neural-score-mode",
                "without-eos",
                "--export-nbest",
                "nbest.json",
                "--word-bigram-corpus",
                "annotated.txt",
                "--word-bigram-weight",
                "500",
                "--skip-bigram-weight",
                "250",
                "--context-bigram-weight",
                "125",
                "--corpus-bigram-min-count",
                "3",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(options.top_k, 5);
        assert_eq!(options.search_k, Some(20));
        assert_eq!(options.format, DatasetFormat::Ajimee);
        assert_eq!(options.inputs, [std::path::PathBuf::from("items.json")]);
        assert_eq!(options.dataset_name.as_deref(), Some("custom"));
        assert_eq!(options.dataset_revision.as_deref(), Some("revision"));
        assert_eq!(options.dataset_sha256.as_deref(), Some("digest"));
        assert_eq!(options.context, ContextFilter::None);
        assert_eq!(options.neural_min_margins, [0.25]);
        assert_eq!(options.neural_score_modes, [NeuralScoreMode::Candidate]);
        assert_eq!(options.limit, Some(25));
        assert_eq!(options.failures, 0);
        assert!(options.json);
        assert_eq!(options.neural_max_cost_gap, Some(750));
        assert_eq!(options.neural_max_candidates, Some(5));
        assert_eq!(options.neural_long_input_min_characters, Some(9));
        assert_eq!(options.neural_long_input_max_candidates, Some(8));
        assert_eq!(options.neural_max_candidate_cost_gap, Some(900));
        assert_eq!(
            options.neural_long_input_max_candidate_cost_gap,
            Some(2_500)
        );
        assert_eq!(
            options.export_nbest,
            Some(std::path::PathBuf::from("nbest.json"))
        );
        assert_eq!(
            options.word_bigram_corpora,
            [std::path::PathBuf::from("annotated.txt")]
        );
        assert_eq!(options.word_bigram_weight, 500);
        assert_eq!(options.skip_bigram_weight, 250);
        assert_eq!(options.context_bigram_weight, 125);
        assert_eq!(options.corpus_bigram_min_count, 3);
    }

    #[test]
    fn neural_cost_gap_gate_skips_confident_or_single_candidate_items() {
        let candidates = vec![
            Candidate {
                surface: "第一".to_owned(),
                cost: 1_000,
            },
            Candidate {
                surface: "第二".to_owned(),
                cost: 1_600,
            },
            Candidate {
                surface: "第三".to_owned(),
                cost: 2_000,
            },
        ];
        assert_eq!(base_cost_gap(&candidates), Some(600));
        assert!(should_score_neurally(&candidates, None));
        assert!(should_score_neurally(&candidates, Some(600)));
        assert!(!should_score_neurally(&candidates, Some(599)));
        assert!(!should_score_neurally(&candidates[..1], Some(1_000)));
    }

    #[test]
    fn neural_candidate_cost_gap_bounds_the_scored_prefix() {
        let candidates = vec![
            Candidate {
                surface: "第一".to_owned(),
                cost: 1_000,
            },
            Candidate {
                surface: "第二".to_owned(),
                cost: 1_250,
            },
            Candidate {
                surface: "第三".to_owned(),
                cost: 1_800,
            },
            Candidate {
                surface: "第四".to_owned(),
                cost: 1_900,
            },
        ];
        assert_eq!(neural_candidate_prefix_len(&candidates, Some(3), None), 3);
        assert_eq!(
            neural_candidate_prefix_len(&candidates, Some(4), Some(800)),
            3
        );
        assert_eq!(
            neural_candidate_prefix_len(&candidates, Some(4), Some(249)),
            1
        );
    }

    #[test]
    fn neural_candidate_cost_gap_expands_only_for_long_inputs() {
        assert_eq!(
            neural_candidate_cost_gap("ショウブン", Some(1_500), Some(9), Some(2_500)),
            Some(1_500)
        );
        assert_eq!(
            neural_candidate_cost_gap("チョウブンショウニ", Some(1_500), Some(9), Some(2_500)),
            Some(2_500)
        );
    }

    #[test]
    fn neural_candidate_limit_expands_only_long_inputs() {
        assert_eq!(
            neural_candidate_limit("カイ", Some(5), Some(9), Some(8)),
            Some(5)
        );
        assert_eq!(
            neural_candidate_limit("チョウブンショウニ", Some(5), Some(9), Some(8)),
            Some(8)
        );
        assert_eq!(
            neural_candidate_limit("ニホン", Some(5), None, None),
            Some(5)
        );
    }

    #[test]
    fn neural_mean_scores_use_the_selected_eos_denominator() {
        assert_eq!(mean_logliks(&[-6.0, -8.0], &[2, 3], true), [-2.0, -2.0]);
        assert_eq!(mean_logliks(&[-6.0, -8.0], &[2, 4], false), [-3.0, -2.0]);
    }

    #[test]
    fn neural_rescoring_reorders_only_the_scored_prefix() {
        let candidates = vec![
            Candidate {
                surface: "第一".to_owned(),
                cost: 1_000,
            },
            Candidate {
                surface: "第二".to_owned(),
                cost: 1_100,
            },
            Candidate {
                surface: "第三".to_owned(),
                cost: 1_200,
            },
            Candidate {
                surface: "第四".to_owned(),
                cost: 1_300,
            },
        ];
        let surfaces = rescored_surfaces(&candidates, &[-10.0, -1.0], 0.8, 0.0);
        assert_eq!(surfaces, ["第二", "第一", "第三", "第四"]);
        assert_eq!(
            rescored_surfaces(&candidates, &[], 0.8, 0.0),
            ["第一", "第二", "第三", "第四"]
        );
    }

    #[test]
    fn neural_rescoring_requires_the_configured_top_candidate_margin() {
        let candidates = vec![
            Candidate {
                surface: "第一".to_owned(),
                cost: 1_000,
            },
            Candidate {
                surface: "第二".to_owned(),
                cost: 1_100,
            },
        ];
        let logliks = [-2.0, -1.0];

        assert_eq!(
            rescored_surfaces(&candidates, &logliks, 0.8, 0.5),
            ["第二", "第一"]
        );
        assert_eq!(
            rescored_surfaces(&candidates, &logliks, 0.8, 1.0),
            ["第一", "第二"]
        );
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
    fn loads_annotated_sentences_as_evaluation_items() {
        let path = std::env::temp_dir().join(format!(
            "slime-tools-annotated-evaluation-{}.txt",
            std::process::id()
        ));
        fs::write(&path, ";; comment\n夏/なつ は/は 暑い/あつい\n").unwrap();

        let items = load_annotated_items(std::slice::from_ref(&path)).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].input, "なつはあつい");
        assert_eq!(items[0].expected_output, ["夏は暑い"]);

        fs::remove_file(path).unwrap();
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
