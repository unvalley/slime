//! Replays public kana-conversion items through the production delayed LIVE
//! neural task boundary and compares marked text before and after reranking.

use std::collections::BTreeMap;
use std::env;
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use slime_core::ALL_DATE_FORMATS;
use slime_ffi::{
    ACTION_SHOW_CANDIDATES, ACTION_UPDATE_PREEDIT, EVENT_CHARACTER, EVENT_SPACE, STATUS_OK,
    SlimeActionViewV2, SlimeHandle, SlimeLiveNeuralTask, SlimeStringView, slime_buffer_destroy,
    slime_create, slime_create_with_data_dir, slime_destroy,
    slime_enable_neural_reranker_with_cost_gap, slime_live_neural_task_apply_actions_v2,
    slime_live_neural_task_create, slime_live_neural_task_destroy, slime_live_neural_task_run,
    slime_process_actions_v2, slime_set_explicit_neural_cost_gap,
    slime_set_explicit_neural_long_reading_weight, slime_set_explicit_neural_medium_reading_weight,
    slime_set_external_left_context, slime_set_live_neural_ranking_enabled, slime_set_options_v5,
};

const DEFAULT_CHANGES: usize = 20;
const DEFAULT_LAMBDA: f64 = 0.2;
const DEFAULT_LONG_READING_LAMBDA: f64 = 0.6;
const DEFAULT_MAX_COST_GAP: i32 = 1_000;
const DEFAULT_MINIMUM_SWITCH_MARGIN: f64 = 0.2;
const DEFAULT_LONG_READING_MINIMUM_SWITCH_MARGIN: f64 = 0.3;
const DEFAULT_NUMERIC_BASE_SWITCH_MARGIN: f64 = 0.1;
const DEFAULT_FIXED_SEGMENT_LIMIT: usize = 0;
const DEFAULT_RECOMBINED_LIMIT: usize = 0;

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
    let source = fs::read(&options.input)
        .map_err(|error| format!("failed to read {}: {error}", options.input.display()))?;
    let input: EvaluationInput = serde_json::from_slice(&source)
        .map_err(|error| format!("failed to parse {}: {error}", options.input.display()))?;
    let mut items = input.into_items();
    if let Some(limit) = options.limit {
        items.truncate(limit);
    }
    if items.is_empty() {
        return Err("evaluation input has no items".to_owned());
    }

    // Keep one strong model reference alive so item-isolated engines reuse the
    // process-wide model instead of loading the GGUF once per item.
    let anchor = Engine::configured(&options)?;
    let report = evaluate(&items, &options)?;
    drop(anchor);

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("failed to serialize report: {error}"))?
        );
    } else {
        println!(
            "items={} core-accuracy={:.4} neural-accuracy={:.4} space-accuracy={:.4} oracle-accuracy={:.4} live-oracle-accuracy={:.4} tasks={}/{} completed={} suffix-retries={} changed={} improved={} regressed={} wrong-changed={} correct-changed={} live-wrong-space={} live-wrong-move={} live-wrong-outside-scope={} live-wrong-worker-move={} worker-winner-rejected={} worker-winner-wrong={} worker-unranked={} live-wrong-missing={} run-p95-ms={}",
            report.items,
            report.core_accuracy,
            report.neural_accuracy,
            report.space_accuracy,
            report.candidate_pool_accuracy,
            report.live_candidate_pool_accuracy,
            report.task_created,
            report.items,
            report.task_completed,
            report.suffix_scope_retries,
            report.changed,
            report.improved,
            report.regressed,
            report.wrong_to_wrong_changed,
            report.correct_to_correct_changed,
            report.live_wrong_space_top1_recoveries,
            report.live_wrong_candidate_move_recoveries,
            report.live_wrong_outside_worker_scope,
            report.live_wrong_worker_candidate_moves,
            report.live_wrong_worker_winner_rejected,
            report.live_wrong_worker_winner_incorrect,
            report.live_wrong_worker_unranked,
            report.live_wrong_missing_from_candidate_pool,
            display_optional(report.latency_ms.run.p95),
        );
        if !report.run_statuses.is_empty() {
            println!("run-statuses={:?}", report.run_statuses);
        }
        for change in &report.changes {
            println!(
                "{}: {:?} -> {:?} expected={:?} outcome={:?}",
                change.index,
                change.core_preedit,
                change.neural_preedit,
                change.expected_output,
                change.outcome
            );
        }
        for recovery in &report.recoveries {
            println!(
                "{}: live={:?} space={:?} expected={:?} recovery={:?} task={:?} status={:?}",
                recovery.index,
                recovery.live_preedit,
                recovery.space_preedit,
                recovery.expected_output,
                recovery.class,
                recovery.task_outcome,
                recovery.run_status,
            );
        }
    }
    Ok(())
}

const fn usage() -> &'static str {
    "usage: live_neural_evaluate MODEL.gguf --input ITEMS.json [--data-dir PATH] \
     [--limit N] [--changes N] [--lambda F] [--long-lambda F] \
     [--explicit-confidence] [--explicit-recombination] [--explicit-live-agreement] [--explicit-max-cost-gap N] [--explicit-long-lambda F] [--explicit-min-characters N] [--explicit-medium-lambda F] \
     [--max-cost-gap N] [--switch-margin F] [--long-switch-margin F] \
     [--numeric-margin F] \
     [--fixed-segment-limit N] [--recombined-limit N] \
     [--guarded-candidate-expansion] [--json]"
}

// Each bool is an independent command-line flag.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
struct Options {
    model: PathBuf,
    input: PathBuf,
    data_directory: Option<PathBuf>,
    limit: Option<usize>,
    changes: usize,
    lambda: f64,
    explicit_long_lambda: Option<f64>,
    explicit_medium_lambda: Option<f64>,
    explicit_min_characters: usize,
    explicit_max_cost_gap: Option<i32>,
    explicit_policy: ExplicitPolicy,
    explicit_live_agreement: bool,
    long_reading_lambda: f64,
    max_cost_gap: i32,
    minimum_switch_margin: f64,
    long_reading_minimum_switch_margin: f64,
    numeric_base_switch_margin: f64,
    fixed_segment_limit: usize,
    recombined_limit: usize,
    guarded_candidate_expansion: bool,
    reopen_particle_boundary: bool,
    json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExplicitPolicy {
    Legacy,
    Confidence,
    Recombination,
}

impl Options {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep evaluation flags in one CLI dispatch"
    )]
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let model = parse_model_path(&mut arguments)?;
        let mut input = None;
        let mut data_directory = None;
        let mut limit = None;
        let mut changes = DEFAULT_CHANGES;
        let mut lambda = DEFAULT_LAMBDA;
        let mut explicit_long_lambda = None;
        let mut explicit_medium_lambda = None;
        let mut explicit_min_characters = 20;
        let mut explicit_max_cost_gap = None;
        let mut explicit_policy = ExplicitPolicy::Legacy;
        let mut explicit_live_agreement = false;
        let mut long_reading_lambda = DEFAULT_LONG_READING_LAMBDA;
        let mut max_cost_gap = DEFAULT_MAX_COST_GAP;
        let mut minimum_switch_margin = DEFAULT_MINIMUM_SWITCH_MARGIN;
        let mut long_reading_minimum_switch_margin = DEFAULT_LONG_READING_MINIMUM_SWITCH_MARGIN;
        let mut numeric_base_switch_margin = DEFAULT_NUMERIC_BASE_SWITCH_MARGIN;
        let mut fixed_segment_limit = DEFAULT_FIXED_SEGMENT_LIMIT;
        let mut recombined_limit = DEFAULT_RECOMBINED_LIMIT;
        let mut guarded_candidate_expansion = false;
        let mut reopen_particle_boundary = false;
        let mut json = false;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--reopen-particle-boundary" => reopen_particle_boundary = true,
                "--input" if input.is_none() => {
                    input = Some(PathBuf::from(next_value(&mut arguments, "--input")?));
                }
                "--input" => return Err("--input is duplicated".to_owned()),
                "--data-dir" if data_directory.is_none() => {
                    data_directory = Some(PathBuf::from(next_value(&mut arguments, "--data-dir")?));
                }
                "--data-dir" => return Err("--data-dir is duplicated".to_owned()),
                "--limit" if limit.is_none() => {
                    limit = Some(parse_positive_usize(&mut arguments, "--limit")?);
                }
                "--limit" => return Err("--limit is duplicated".to_owned()),
                "--changes" => {
                    changes = parse_non_negative_usize(&mut arguments, "--changes")?;
                }
                "--lambda" => lambda = parse_unit_f64(&mut arguments, "--lambda")?,
                "--explicit-live-agreement" => explicit_live_agreement = true,
                "--explicit-confidence" => {
                    if explicit_policy == ExplicitPolicy::Legacy {
                        explicit_policy = ExplicitPolicy::Confidence;
                    }
                }
                "--explicit-recombination" => {
                    explicit_policy = ExplicitPolicy::Recombination;
                }
                "--explicit-max-cost-gap" => {
                    explicit_max_cost_gap = Some(parse_non_negative_i32(
                        &mut arguments,
                        "--explicit-max-cost-gap",
                    )?);
                }
                "--explicit-medium-lambda" => {
                    explicit_medium_lambda =
                        Some(parse_unit_f64(&mut arguments, "--explicit-medium-lambda")?);
                }
                "--explicit-long-lambda" => {
                    explicit_long_lambda =
                        Some(parse_unit_f64(&mut arguments, "--explicit-long-lambda")?);
                }
                "--explicit-min-characters" => {
                    explicit_min_characters =
                        parse_positive_usize(&mut arguments, "--explicit-min-characters")?;
                }
                "--long-lambda" => {
                    long_reading_lambda = parse_unit_f64(&mut arguments, "--long-lambda")?;
                }
                "--max-cost-gap" => {
                    max_cost_gap = parse_non_negative_i32(&mut arguments, "--max-cost-gap")?;
                }
                "--switch-margin" => {
                    minimum_switch_margin =
                        parse_non_negative_f64(&mut arguments, "--switch-margin")?;
                }
                "--long-switch-margin" => {
                    long_reading_minimum_switch_margin =
                        parse_non_negative_f64(&mut arguments, "--long-switch-margin")?;
                }
                "--numeric-margin" => {
                    numeric_base_switch_margin =
                        parse_non_negative_f64(&mut arguments, "--numeric-margin")?;
                }
                "--fixed-segment-limit" => {
                    fixed_segment_limit =
                        parse_non_negative_usize(&mut arguments, "--fixed-segment-limit")?;
                }
                "--recombined-limit" => {
                    recombined_limit =
                        parse_non_negative_usize(&mut arguments, "--recombined-limit")?;
                }
                "--guarded-candidate-expansion" if !guarded_candidate_expansion => {
                    guarded_candidate_expansion = true;
                }
                "--guarded-candidate-expansion" => {
                    return Err("--guarded-candidate-expansion is duplicated".to_owned());
                }
                "--json" if !json => json = true,
                "--json" => return Err("--json is duplicated".to_owned()),
                "--help" | "-h" => return Err(usage().to_owned()),
                _ => return Err(format!("unknown option {argument:?}\n{}", usage())),
            }
        }

        Ok(Self {
            model,
            input: input.ok_or_else(|| usage().to_owned())?,
            data_directory,
            limit,
            changes,
            lambda,
            explicit_long_lambda,
            explicit_medium_lambda,
            explicit_min_characters,
            explicit_max_cost_gap,
            explicit_policy,
            explicit_live_agreement,
            long_reading_lambda,
            max_cost_gap,
            minimum_switch_margin,
            long_reading_minimum_switch_margin,
            numeric_base_switch_margin,
            fixed_segment_limit,
            recombined_limit,
            guarded_candidate_expansion,
            reopen_particle_boundary,
            json,
        })
    }
}

fn parse_model_path(arguments: &mut impl Iterator<Item = String>) -> Result<PathBuf, String> {
    arguments
        .next()
        .filter(|argument| !argument.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| usage().to_owned())
}

fn next_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_non_negative_usize(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<usize, String> {
    next_value(arguments, option)?
        .parse::<usize>()
        .map_err(|_| format!("{option} requires a non-negative integer"))
}

fn parse_positive_usize(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<usize, String> {
    let value = parse_non_negative_usize(arguments, option)?;
    if value == 0 {
        return Err(format!("{option} requires a positive integer"));
    }
    Ok(value)
}

fn parse_non_negative_i32(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<i32, String> {
    let value = next_value(arguments, option)?
        .parse::<i32>()
        .map_err(|_| format!("{option} requires a non-negative integer"))?;
    if value < 0 {
        return Err(format!("{option} requires a non-negative integer"));
    }
    Ok(value)
}

fn parse_unit_f64(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<f64, String> {
    let value = parse_non_negative_f64(arguments, option)?;
    if value > 1.0 {
        return Err(format!("{option} requires a value from 0 to 1"));
    }
    Ok(value)
}

fn parse_non_negative_f64(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<f64, String> {
    let value = next_value(arguments, option)?
        .parse::<f64>()
        .map_err(|_| format!("{option} requires a non-negative finite number"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{option} requires a non-negative finite number"));
    }
    Ok(value)
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EvaluationInput {
    Direct(Vec<EvaluationItem>),
    Wrapped { items: Vec<EvaluationItem> },
}

impl EvaluationInput {
    fn into_items(self) -> Vec<EvaluationItem> {
        match self {
            Self::Direct(items) | Self::Wrapped { items } => items,
        }
    }
}

#[derive(Debug, Deserialize)]
struct EvaluationItem {
    index: String,
    #[serde(default)]
    context_text: String,
    input: String,
    expected_output: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Report {
    items: usize,
    core_correct: usize,
    neural_correct: usize,
    core_accuracy: f64,
    neural_accuracy: f64,
    space_correct: usize,
    space_accuracy: f64,
    candidate_pool_correct: usize,
    candidate_pool_accuracy: f64,
    live_candidate_pool_correct: usize,
    live_candidate_pool_accuracy: f64,
    task_created: usize,
    task_unavailable: usize,
    task_completed: usize,
    suffix_scope_retries: usize,
    changed: usize,
    improved: usize,
    regressed: usize,
    wrong_to_wrong_changed: usize,
    correct_to_correct_changed: usize,
    unchanged: usize,
    live_wrong_space_top1_recoveries: usize,
    live_wrong_candidate_move_recoveries: usize,
    live_wrong_outside_worker_scope: usize,
    live_wrong_worker_candidate_moves: usize,
    live_wrong_worker_winner_rejected: usize,
    live_wrong_worker_winner_incorrect: usize,
    live_wrong_worker_unranked: usize,
    live_wrong_missing_from_candidate_pool: usize,
    run_statuses: BTreeMap<u32, usize>,
    latency_ms: TaskLatencyReport,
    single_scope_run_latency_ms: LatencyReport,
    suffix_retry_run_latency_ms: LatencyReport,
    changes: Vec<Change>,
    recoveries: Vec<Recovery>,
    explicit_results: Vec<ExplicitResult>,
    space_latency_ms: LatencyReport,
}

/// Complete per-item outcomes, including items already correct in LIVE.
/// Recovery-only records cannot reveal all explicit-conversion regressions.
#[derive(Debug, Serialize)]
struct ExplicitResult {
    index: String,
    input: String,
    expected_output: Vec<String>,
    preedit: String,
    latency_ms: f64,
}

#[derive(Debug, Serialize)]
struct TaskLatencyReport {
    snapshot: LatencyReport,
    run: LatencyReport,
    apply: LatencyReport,
}

#[derive(Debug, Serialize)]
struct LatencyReport {
    p50: Option<f64>,
    p95: Option<f64>,
    p99: Option<f64>,
    max: Option<f64>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ChangeOutcome {
    Improved,
    Regressed,
    WrongToWrong,
    CorrectToCorrect,
}

#[derive(Debug, Serialize)]
struct Change {
    index: String,
    context_text: String,
    input: String,
    expected_output: Vec<String>,
    core_preedit: String,
    neural_preedit: String,
    outcome: ChangeOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RecoveryClass {
    SpaceTop1,
    CandidateMove,
    MissingFromCandidatePool,
}

#[derive(Debug, Serialize)]
struct Recovery {
    index: String,
    context_text: String,
    input: String,
    expected_output: Vec<String>,
    live_preedit: String,
    space_preedit: String,
    candidates: Vec<String>,
    live_candidates: Vec<String>,
    live_ranked_candidates: Vec<String>,
    live_request_reading: Option<String>,
    live_request_left_context: Option<String>,
    live_base_target_surface: Option<String>,
    live_has_stable_prefix: bool,
    live_reopens_stable_prefix: bool,
    class: RecoveryClass,
    task_outcome: LiveTaskOutcome,
    run_status: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LiveTaskOutcome {
    Unavailable,
    WorkerRejected,
    CompletedNoChange,
    CompletedChanged,
}

struct LiveTaskEvaluation {
    preedit: String,
    candidates: Vec<String>,
    ranked_candidates: Vec<String>,
    request_reading: Option<String>,
    request_left_context: Option<String>,
    base_target_surface: Option<String>,
    has_stable_prefix: bool,
    reopens_stable_prefix: bool,
    outcome: LiveTaskOutcome,
    run_status: Option<u32>,
}

fn evaluate(items: &[EvaluationItem], options: &Options) -> Result<Report, String> {
    let mut state = EvaluationState::default();
    for (position, item) in items.iter().enumerate() {
        state.evaluate_item(item, options)?;
        report_progress(position, items.len());
    }
    Ok(state.into_report(items.len()))
}

#[derive(Default)]
struct EvaluationState {
    core_correct: usize,
    neural_correct: usize,
    space_correct: usize,
    candidate_pool_correct: usize,
    live_candidate_pool_correct: usize,
    task_created: usize,
    task_unavailable: usize,
    task_completed: usize,
    suffix_scope_retries: usize,
    changed: usize,
    improved: usize,
    regressed: usize,
    wrong_to_wrong_changed: usize,
    correct_to_correct_changed: usize,
    unchanged: usize,
    live_wrong_space_top1_recoveries: usize,
    live_wrong_candidate_move_recoveries: usize,
    live_wrong_outside_worker_scope: usize,
    live_wrong_worker_candidate_moves: usize,
    live_wrong_worker_winner_rejected: usize,
    live_wrong_worker_winner_incorrect: usize,
    live_wrong_worker_unranked: usize,
    live_wrong_missing_from_candidate_pool: usize,
    run_statuses: BTreeMap<u32, usize>,
    snapshot_latencies: Vec<Duration>,
    run_latencies: Vec<Duration>,
    single_scope_run_latencies: Vec<Duration>,
    suffix_retry_run_latencies: Vec<Duration>,
    apply_latencies: Vec<Duration>,
    change_details: Vec<Change>,
    recovery_details: Vec<Recovery>,
    explicit_results: Vec<ExplicitResult>,
    space_latencies: Vec<Duration>,
}

impl EvaluationState {
    fn evaluate_item(&mut self, item: &EvaluationItem, options: &Options) -> Result<(), String> {
        let engine = Engine::configured(options)?;
        engine.set_context(&item.context_text)?;
        let core_preedit = engine.type_text(&katakana_to_hiragana(&item.input))?;
        let was_correct = is_correct(&core_preedit, &item.expected_output);
        self.core_correct = self.core_correct.saturating_add(usize::from(was_correct));

        let live_task = self.evaluate_live_task(&engine, options, &core_preedit)?;
        let neural_preedit = live_task.preedit.clone();
        let is_now_correct = is_correct(&neural_preedit, &item.expected_output);
        self.neural_correct = self
            .neural_correct
            .saturating_add(usize::from(is_now_correct));

        if neural_preedit == core_preedit {
            self.unchanged = self.unchanged.saturating_add(1);
        } else {
            self.record_change(
                item,
                options.changes,
                core_preedit,
                neural_preedit.clone(),
                was_correct,
                is_now_correct,
            );
        }

        let space_started = Instant::now();
        let explicit = engine.explicit_conversion()?;
        let space_elapsed = space_started.elapsed();
        self.space_latencies.push(space_elapsed);
        self.explicit_results.push(ExplicitResult {
            index: item.index.clone(),
            input: item.input.clone(),
            expected_output: item.expected_output.clone(),
            preedit: explicit.preedit.clone().unwrap_or_default(),
            latency_ms: space_elapsed.as_secs_f64() * 1_000.0,
        });
        self.record_recovery(item, options.changes, is_now_correct, explicit, live_task);
        Ok(())
    }

    fn evaluate_live_task(
        &mut self,
        engine: &Engine,
        options: &Options,
        core_preedit: &str,
    ) -> Result<LiveTaskEvaluation, String> {
        let snapshot_started = Instant::now();
        let task = engine.create_task(options);
        self.snapshot_latencies.push(snapshot_started.elapsed());
        let Some(mut task) = task else {
            self.task_unavailable = self.task_unavailable.saturating_add(1);
            return Ok(LiveTaskEvaluation {
                preedit: core_preedit.to_owned(),
                candidates: Vec::new(),
                ranked_candidates: Vec::new(),
                request_reading: None,
                request_left_context: None,
                base_target_surface: None,
                has_stable_prefix: false,
                reopens_stable_prefix: false,
                outcome: LiveTaskOutcome::Unavailable,
                run_status: None,
            });
        };
        self.task_created = self.task_created.saturating_add(1);

        if !task.configure_candidate_expansion(options) {
            return Err("failed to configure evaluator candidate expansion".to_owned());
        }
        if !task.configure_long_reading_switch_margin(options) {
            return Err("failed to configure evaluator long-reading margin".to_owned());
        }
        let run_started = Instant::now();
        let run_status = task.run();
        let run_elapsed = run_started.elapsed();
        let candidates = task.rankable_surfaces();
        let ranked_candidates = task.ranked_surfaces();
        let request_reading = task.request_reading();
        let request_left_context = task.request_left_context();
        let base_target_surface = task.base_target_surface();
        let has_stable_prefix = task.has_stable_prefix();
        let reopens_stable_prefix = task.reopens_stable_prefix();
        if task.retried_suffix_scope() {
            self.suffix_scope_retries = self.suffix_scope_retries.saturating_add(1);
            self.suffix_retry_run_latencies.push(run_elapsed);
        } else {
            self.single_scope_run_latencies.push(run_elapsed);
        }
        self.run_latencies.push(run_elapsed);
        *self.run_statuses.entry(run_status).or_default() += 1;
        if run_status != STATUS_OK {
            return Ok(LiveTaskEvaluation {
                preedit: core_preedit.to_owned(),
                candidates,
                ranked_candidates,
                request_reading,
                request_left_context,
                base_target_surface,
                has_stable_prefix,
                reopens_stable_prefix,
                outcome: LiveTaskOutcome::WorkerRejected,
                run_status: Some(run_status),
            });
        }
        self.task_completed = self.task_completed.saturating_add(1);

        let apply_started = Instant::now();
        let applied = engine.apply_task(&task)?;
        self.apply_latencies.push(apply_started.elapsed());
        let outcome = if applied.is_some() {
            LiveTaskOutcome::CompletedChanged
        } else {
            LiveTaskOutcome::CompletedNoChange
        };
        Ok(LiveTaskEvaluation {
            preedit: applied.unwrap_or_else(|| core_preedit.to_owned()),
            candidates,
            ranked_candidates,
            request_reading,
            request_left_context,
            base_target_surface,
            has_stable_prefix,
            reopens_stable_prefix,
            outcome,
            run_status: Some(run_status),
        })
    }

    fn record_change(
        &mut self,
        item: &EvaluationItem,
        detail_limit: usize,
        core_preedit: String,
        neural_preedit: String,
        was_correct: bool,
        is_now_correct: bool,
    ) {
        self.changed = self.changed.saturating_add(1);
        let outcome = match (was_correct, is_now_correct) {
            (false, true) => {
                self.improved = self.improved.saturating_add(1);
                ChangeOutcome::Improved
            }
            (true, false) => {
                self.regressed = self.regressed.saturating_add(1);
                ChangeOutcome::Regressed
            }
            (false, false) => {
                self.wrong_to_wrong_changed = self.wrong_to_wrong_changed.saturating_add(1);
                ChangeOutcome::WrongToWrong
            }
            (true, true) => {
                self.correct_to_correct_changed = self.correct_to_correct_changed.saturating_add(1);
                ChangeOutcome::CorrectToCorrect
            }
        };
        if self.change_details.len() < detail_limit {
            self.change_details.push(Change {
                index: item.index.clone(),
                context_text: item.context_text.clone(),
                input: item.input.clone(),
                expected_output: item.expected_output.clone(),
                core_preedit,
                neural_preedit,
                outcome,
            });
        }
    }

    fn record_recovery(
        &mut self,
        item: &EvaluationItem,
        detail_limit: usize,
        live_correct: bool,
        explicit: Capture,
        live_task: LiveTaskEvaluation,
    ) {
        let LiveTaskEvaluation {
            preedit: live_preedit,
            candidates: live_candidates,
            ranked_candidates: live_ranked_candidates,
            request_reading: live_request_reading,
            request_left_context: live_request_left_context,
            base_target_surface: live_base_target_surface,
            has_stable_prefix: live_has_stable_prefix,
            reopens_stable_prefix: live_reopens_stable_prefix,
            outcome: task_outcome,
            run_status,
        } = live_task;
        let space_preedit = explicit.preedit.unwrap_or_default();
        let space_correct = is_correct(&space_preedit, &item.expected_output);
        let candidate_pool_correct = explicit
            .candidates
            .iter()
            .any(|candidate| is_correct(candidate, &item.expected_output));
        let live_candidate_pool_correct = live_candidates
            .iter()
            .any(|candidate| is_correct(candidate, &item.expected_output));
        self.space_correct = self
            .space_correct
            .saturating_add(usize::from(space_correct));
        self.candidate_pool_correct = self
            .candidate_pool_correct
            .saturating_add(usize::from(candidate_pool_correct));
        self.live_candidate_pool_correct = self
            .live_candidate_pool_correct
            .saturating_add(usize::from(live_candidate_pool_correct));

        if !live_correct && !space_correct && candidate_pool_correct {
            if live_candidate_pool_correct {
                self.live_wrong_worker_candidate_moves =
                    self.live_wrong_worker_candidate_moves.saturating_add(1);
                match live_ranked_candidates.first() {
                    Some(winner) if is_correct(winner, &item.expected_output) => {
                        self.live_wrong_worker_winner_rejected =
                            self.live_wrong_worker_winner_rejected.saturating_add(1);
                    }
                    Some(_) => {
                        self.live_wrong_worker_winner_incorrect =
                            self.live_wrong_worker_winner_incorrect.saturating_add(1);
                    }
                    None => {
                        self.live_wrong_worker_unranked =
                            self.live_wrong_worker_unranked.saturating_add(1);
                    }
                }
            } else {
                self.live_wrong_outside_worker_scope =
                    self.live_wrong_outside_worker_scope.saturating_add(1);
            }
        }

        let Some(class) = recovery_class(live_correct, space_correct, candidate_pool_correct)
        else {
            return;
        };
        match class {
            RecoveryClass::SpaceTop1 => {
                self.live_wrong_space_top1_recoveries =
                    self.live_wrong_space_top1_recoveries.saturating_add(1);
            }
            RecoveryClass::CandidateMove => {
                self.live_wrong_candidate_move_recoveries =
                    self.live_wrong_candidate_move_recoveries.saturating_add(1);
            }
            RecoveryClass::MissingFromCandidatePool => {
                self.live_wrong_missing_from_candidate_pool = self
                    .live_wrong_missing_from_candidate_pool
                    .saturating_add(1);
            }
        }
        if self.recovery_details.len() < detail_limit {
            self.recovery_details.push(Recovery {
                index: item.index.clone(),
                context_text: item.context_text.clone(),
                input: item.input.clone(),
                expected_output: item.expected_output.clone(),
                live_preedit,
                space_preedit,
                candidates: explicit.candidates,
                live_candidates,
                live_ranked_candidates,
                live_request_reading,
                live_request_left_context,
                live_base_target_surface,
                live_has_stable_prefix,
                live_reopens_stable_prefix,
                class,
                task_outcome,
                run_status,
            });
        }
    }

    fn into_report(self, item_count: usize) -> Report {
        debug_assert_eq!(
            self.live_wrong_space_top1_recoveries
                + self.live_wrong_candidate_move_recoveries
                + self.live_wrong_missing_from_candidate_pool,
            item_count.saturating_sub(self.neural_correct),
            "every wrong LIVE result must have exactly one recovery class"
        );
        debug_assert!(
            self.candidate_pool_correct >= self.space_correct,
            "the Space top candidate must also be present in the candidate pool"
        );
        debug_assert_eq!(
            self.live_wrong_worker_candidate_moves,
            self.live_wrong_worker_winner_rejected
                + self.live_wrong_worker_winner_incorrect
                + self.live_wrong_worker_unranked,
            "worker candidate moves must have one ranking outcome"
        );
        Report {
            items: item_count,
            core_correct: self.core_correct,
            neural_correct: self.neural_correct,
            core_accuracy: ratio(self.core_correct, item_count),
            neural_accuracy: ratio(self.neural_correct, item_count),
            space_correct: self.space_correct,
            space_accuracy: ratio(self.space_correct, item_count),
            candidate_pool_correct: self.candidate_pool_correct,
            candidate_pool_accuracy: ratio(self.candidate_pool_correct, item_count),
            live_candidate_pool_correct: self.live_candidate_pool_correct,
            live_candidate_pool_accuracy: ratio(self.live_candidate_pool_correct, item_count),
            task_created: self.task_created,
            task_unavailable: self.task_unavailable,
            task_completed: self.task_completed,
            suffix_scope_retries: self.suffix_scope_retries,
            changed: self.changed,
            improved: self.improved,
            regressed: self.regressed,
            wrong_to_wrong_changed: self.wrong_to_wrong_changed,
            correct_to_correct_changed: self.correct_to_correct_changed,
            unchanged: self.unchanged,
            live_wrong_space_top1_recoveries: self.live_wrong_space_top1_recoveries,
            live_wrong_candidate_move_recoveries: self.live_wrong_candidate_move_recoveries,
            live_wrong_outside_worker_scope: self.live_wrong_outside_worker_scope,
            live_wrong_worker_candidate_moves: self.live_wrong_worker_candidate_moves,
            live_wrong_worker_winner_rejected: self.live_wrong_worker_winner_rejected,
            live_wrong_worker_winner_incorrect: self.live_wrong_worker_winner_incorrect,
            live_wrong_worker_unranked: self.live_wrong_worker_unranked,
            live_wrong_missing_from_candidate_pool: self.live_wrong_missing_from_candidate_pool,
            run_statuses: self.run_statuses,
            latency_ms: TaskLatencyReport {
                snapshot: latency_report(self.snapshot_latencies),
                run: latency_report(self.run_latencies),
                apply: latency_report(self.apply_latencies),
            },
            single_scope_run_latency_ms: latency_report(self.single_scope_run_latencies),
            suffix_retry_run_latency_ms: latency_report(self.suffix_retry_run_latencies),
            changes: self.change_details,
            recoveries: self.recovery_details,
            explicit_results: self.explicit_results,
            space_latency_ms: latency_report(self.space_latencies),
        }
    }
}

fn report_progress(position: usize, total: usize) {
    let completed = position.saturating_add(1);
    if completed == total || completed.is_multiple_of(50) {
        eprintln!("evaluated {completed}/{total}");
    }
}

fn is_correct(surface: &str, expected: &[String]) -> bool {
    expected.iter().any(|candidate| candidate == surface)
}

fn recovery_class(
    live_correct: bool,
    space_correct: bool,
    candidate_pool_correct: bool,
) -> Option<RecoveryClass> {
    if live_correct {
        None
    } else if space_correct {
        Some(RecoveryClass::SpaceTop1)
    } else if candidate_pool_correct {
        Some(RecoveryClass::CandidateMove)
    } else {
        Some(RecoveryClass::MissingFromCandidatePool)
    }
}

struct Engine(*mut SlimeHandle);

impl Engine {
    fn configured(options: &Options) -> Result<Self, String> {
        let handle = if let Some(directory) = &options.data_directory {
            if !directory.is_dir() {
                return Err("data directory must be an existing directory".to_owned());
            }
            let directory = path_bytes(directory, "data directory")?;
            // SAFETY: The path bytes remain readable for this synchronous call.
            unsafe { slime_create_with_data_dir(directory.as_ptr(), directory.len()) }
        } else {
            slime_create()
        };
        let engine = Self(handle);
        if engine.0.is_null() {
            return Err("failed to create engine".to_owned());
        }

        let model = path_bytes(&options.model, "model path")?;
        // SAFETY: The handle is exclusively owned and the path bytes remain
        // readable for this synchronous call.
        let status = unsafe {
            slime_enable_neural_reranker_with_cost_gap(
                engine.0,
                model.as_ptr(),
                model.len(),
                options.lambda,
                options.max_cost_gap,
            )
        };
        if status != STATUS_OK {
            return Err(format!("failed to attach neural model: status={status}"));
        }
        if options.explicit_policy != ExplicitPolicy::Legacy {
            // SAFETY: The evaluator exclusively owns this live handle.
            if !unsafe { slime_ffi::evaluation_set_explicit_confidence(engine.0, true) } {
                return Err("failed to enable explicit confidence".to_owned());
            }
        }
        if options.explicit_policy == ExplicitPolicy::Recombination {
            // SAFETY: The evaluator exclusively owns this live handle.
            if !unsafe { slime_ffi::evaluation_set_explicit_recombination(engine.0, true) } {
                return Err("failed to enable explicit recombination".to_owned());
            }
        }
        if options.explicit_live_agreement {
            // SAFETY: The evaluator exclusively owns this live handle.
            if !unsafe { slime_ffi::evaluation_set_explicit_live_agreement(engine.0, true) } {
                return Err("failed to enable explicit LIVE agreement".to_owned());
            }
        }
        if let Some(gap) = options.explicit_max_cost_gap {
            // SAFETY: The evaluator exclusively owns this live handle.
            let status = unsafe { slime_set_explicit_neural_cost_gap(engine.0, gap) };
            if status != STATUS_OK {
                return Err(format!(
                    "failed to configure explicit cost gap: status={status}"
                ));
            }
        }
        if let Some(lambda) = options.explicit_long_lambda {
            // SAFETY: The evaluator exclusively owns this live handle.
            let status = unsafe {
                slime_set_explicit_neural_long_reading_weight(
                    engine.0,
                    options.explicit_min_characters,
                    lambda,
                )
            };
            if status != STATUS_OK {
                return Err(format!(
                    "failed to configure explicit weight: status={status}"
                ));
            }
        }
        if let Some(lambda) = options.explicit_medium_lambda {
            // SAFETY: The evaluator exclusively owns this live handle.
            let status =
                unsafe { slime_set_explicit_neural_medium_reading_weight(engine.0, lambda) };
            if status != STATUS_OK {
                return Err(format!(
                    "failed to configure explicit medium weight: status={status}"
                ));
            }
        }
        // SAFETY: The evaluator owns the handle exclusively and schedules a
        // delayed LIVE task for every eligible final composition.
        let status = unsafe { slime_set_live_neural_ranking_enabled(engine.0, true) };
        if status != STATUS_OK {
            return Err(format!(
                "failed to enable delayed LIVE ranking: status={status}"
            ));
        }

        // SAFETY: The engine is exclusively owned. The returned buffer is
        // released exactly once.
        unsafe {
            slime_buffer_destroy(slime_set_options_v5(
                engine.0,
                true,
                false,
                false,
                0,
                false,
                ALL_DATE_FORMATS,
            ));
        }
        Ok(engine)
    }

    fn set_context(&self, context: &str) -> Result<(), String> {
        // SAFETY: This evaluator accesses the engine serially and the context
        // bytes remain readable for the duration of the call.
        let status =
            unsafe { slime_set_external_left_context(self.0, context.as_ptr(), context.len()) };
        if status == STATUS_OK {
            Ok(())
        } else {
            Err(format!("failed to set left context: status={status}"))
        }
    }

    fn type_text(&self, input: &str) -> Result<String, String> {
        let mut capture = Capture::default();
        for character in input.chars() {
            // SAFETY: The evaluator is the exclusive engine owner. The
            // callback copies every borrowed string before returning.
            let status = unsafe {
                slime_process_actions_v2(
                    self.0,
                    EVENT_CHARACTER,
                    u32::from(character),
                    (&raw mut capture).cast(),
                    Some(capture_action),
                )
            };
            if status != STATUS_OK {
                return Err(format!(
                    "failed to type character {character:?}: status={status}"
                ));
            }
        }
        capture
            .preedit
            .ok_or_else(|| format!("typing {input:?} produced no preedit"))
    }

    fn create_task(&self, options: &Options) -> Option<Task> {
        // SAFETY: Snapshot creation only reads the exclusively owned engine.
        let task = unsafe {
            slime_live_neural_task_create(
                self.0,
                options.minimum_switch_margin,
                options.numeric_base_switch_margin,
                options.long_reading_lambda,
            )
        };
        (!task.is_null()).then_some(Task(task))
    }

    fn apply_task(&self, task: &Task) -> Result<Option<String>, String> {
        let mut capture = Capture::default();
        // SAFETY: Task scoring has completed and both values are exclusively
        // accessed. The callback copies borrowed strings synchronously.
        let status = unsafe {
            slime_live_neural_task_apply_actions_v2(
                self.0,
                task.0,
                (&raw mut capture).cast(),
                Some(capture_action),
            )
        };
        if status == STATUS_OK {
            Ok(capture.preedit)
        } else {
            Err(format!("failed to apply LIVE neural task: status={status}"))
        }
    }

    fn explicit_conversion(&self) -> Result<Capture, String> {
        let mut capture = Capture::default();
        // SAFETY: The evaluator exclusively owns the engine and copies all
        // borrowed action data before this synchronous callback returns.
        let status = unsafe {
            slime_process_actions_v2(
                self.0,
                EVENT_SPACE,
                0,
                (&raw mut capture).cast(),
                Some(capture_action),
            )
        };
        if status != STATUS_OK {
            return Err(format!(
                "failed to start explicit conversion: status={status}"
            ));
        }
        if capture.preedit.is_none() {
            return Err("explicit conversion produced no preedit".to_owned());
        }
        Ok(capture)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: This wrapper owns the handle and drops it exactly once.
        unsafe { slime_destroy(self.0) };
    }
}

struct Task(*mut SlimeLiveNeuralTask);

impl Task {
    fn configure_long_reading_switch_margin(&mut self, options: &Options) -> bool {
        // SAFETY: This evaluator exclusively owns an unrun task.
        unsafe { &mut *self.0 }.evaluation_set_long_reading_minimum_switch_margin(
            options.long_reading_minimum_switch_margin,
        )
    }

    fn configure_candidate_expansion(&mut self, options: &Options) -> bool {
        // SAFETY: This evaluator exclusively owns an unrun task.
        let task = unsafe { &mut *self.0 };
        if options.reopen_particle_boundary {
            task.prepare_particle_boundary_reopen();
        }

        if options.guarded_candidate_expansion {
            task.evaluation_set_guarded_candidate_expansion(
                options.fixed_segment_limit,
                options.recombined_limit,
            )
        } else {
            task.evaluation_set_candidate_expansion(
                options.fixed_segment_limit,
                options.recombined_limit,
            )
        }
    }

    fn run(&mut self) -> u32 {
        // SAFETY: The evaluator runs each task at most once and waits before
        // applying or destroying it.
        unsafe { slime_live_neural_task_run(self.0) }
    }

    fn rankable_surfaces(&self) -> Vec<String> {
        // SAFETY: This evaluator exclusively owns the live task and copies the
        // diagnostic surfaces before the task is applied or destroyed.
        unsafe { &*self.0 }.evaluation_rankable_surfaces()
    }

    fn ranked_surfaces(&self) -> Vec<String> {
        // SAFETY: This evaluator exclusively owns the completed task and
        // copies all diagnostic surfaces before apply or destruction.
        unsafe { &*self.0 }.evaluation_ranked_surfaces()
    }

    fn request_reading(&self) -> Option<String> {
        // SAFETY: This evaluator exclusively owns the task.
        unsafe { &*self.0 }.evaluation_request_reading()
    }

    fn request_left_context(&self) -> Option<String> {
        // SAFETY: This evaluator exclusively owns the completed task.
        unsafe { &*self.0 }
            .evaluation_request()
            .map(|request| request.left_context)
    }

    fn base_target_surface(&self) -> Option<String> {
        // SAFETY: This evaluator exclusively owns the task.
        unsafe { &*self.0 }.evaluation_base_target_surface()
    }

    fn has_stable_prefix(&self) -> bool {
        // SAFETY: This evaluator exclusively owns the task.
        unsafe { &*self.0 }.evaluation_has_stable_prefix()
    }

    fn reopens_stable_prefix(&self) -> bool {
        // SAFETY: This evaluator exclusively owns the task.
        unsafe { &*self.0 }.evaluation_reopens_stable_prefix()
    }

    fn retried_suffix_scope(&self) -> bool {
        // SAFETY: This evaluator exclusively owns the completed task.
        unsafe { &*self.0 }.evaluation_retried_suffix_scope()
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        // SAFETY: This wrapper owns the task and drops it exactly once.
        unsafe { slime_live_neural_task_destroy(self.0) };
    }
}

#[derive(Default)]
struct Capture {
    preedit: Option<String>,
    candidates: Vec<String>,
}

unsafe extern "C" fn capture_action(context: *mut c_void, action: *const SlimeActionViewV2) {
    // SAFETY: The FFI callback contract supplies valid synchronous pointers.
    let capture = unsafe { &mut *context.cast::<Capture>() };
    // SAFETY: The action view remains valid for the callback duration.
    let action = unsafe { &*action };
    if action.kind == ACTION_UPDATE_PREEDIT {
        capture.preedit = Some(copy_string_view(action.text));
    } else if action.kind == ACTION_SHOW_CANDIDATES {
        capture.candidates.clear();
        if action.candidate_count == 0 {
            return;
        }
        // SAFETY: The callback contract exposes this candidate array for the
        // callback duration. Every string is copied before returning.
        let candidates =
            unsafe { std::slice::from_raw_parts(action.candidates, action.candidate_count) };
        capture.candidates.extend(
            candidates
                .iter()
                .map(|candidate| copy_string_view(candidate.value)),
        );
    }
}

fn copy_string_view(view: SlimeStringView) -> String {
    if view.len == 0 {
        return String::new();
    }
    // SAFETY: Callers pass a live synchronous FFI string view. The bytes are
    // copied into an owned string before the callback returns.
    let bytes = unsafe { std::slice::from_raw_parts(view.data, view.len) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn path_bytes<'a>(path: &'a Path, label: &str) -> Result<&'a [u8], String> {
    path.to_str()
        .map(str::as_bytes)
        .ok_or_else(|| format!("{label} must be valid UTF-8: {}", path.display()))
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

fn latency_report(mut durations: Vec<Duration>) -> LatencyReport {
    if durations.is_empty() {
        return LatencyReport {
            p50: None,
            p95: None,
            p99: None,
            max: None,
        };
    }
    durations.sort_unstable();
    LatencyReport {
        p50: Some(percentile(&durations, 50)),
        p95: Some(percentile(&durations, 95)),
        p99: Some(percentile(&durations, 99)),
        max: durations.last().copied().map(duration_to_millis),
    }
}

fn percentile(sorted: &[Duration], percentile: usize) -> f64 {
    let rank = sorted
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    duration_to_millis(sorted[rank])
}

fn duration_to_millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    let numerator = u32::try_from(numerator).expect("evaluation count fits u32");
    let denominator = u32::try_from(denominator).expect("evaluation count fits u32");
    f64::from(numerator) / f64::from(denominator)
}

fn display_optional(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.3}"))
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_LONG_READING_MINIMUM_SWITCH_MARGIN, EvaluationInput, Options, RecoveryClass,
        katakana_to_hiragana, recovery_class,
    };

    #[test]
    fn reads_direct_and_wrapped_evaluation_inputs() {
        let direct: EvaluationInput =
            serde_json::from_str(r#"[{"index":"1","input":"カンジ","expected_output":["漢字"]}]"#)
                .unwrap();
        let wrapped: EvaluationInput = serde_json::from_str(
            r#"{"items":[{"index":"1","input":"カンジ","expected_output":["漢字"]}]}"#,
        )
        .unwrap();
        assert_eq!(direct.into_items().len(), 1);
        assert_eq!(wrapped.into_items().len(), 1);
    }

    #[test]
    fn normalizes_katakana_without_changing_punctuation() {
        assert_eq!(katakana_to_hiragana("カンジ、１２３"), "かんじ、１２３");
    }

    #[test]
    fn rejects_zero_item_limit() {
        let error = Options::parse(
            ["model.gguf", "--input", "items.json", "--limit", "0"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap_err();
        assert_eq!(error, "--limit requires a positive integer");
    }

    #[test]
    fn accepts_zero_or_positive_evaluator_candidate_limits() {
        let options = Options::parse(
            [
                "model.gguf",
                "--input",
                "items.json",
                "--fixed-segment-limit",
                "0",
                "--recombined-limit",
                "2",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(options.fixed_segment_limit, 0);
        assert_eq!(options.recombined_limit, 2);
        assert!(!options.guarded_candidate_expansion);
    }

    #[test]
    fn long_switch_margin_has_an_independent_default_and_can_be_overridden() {
        let defaults = Options::parse(
            [
                "model.gguf",
                "--input",
                "items.json",
                "--switch-margin",
                "0.25",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert!((defaults.minimum_switch_margin - 0.25).abs() < f64::EPSILON);
        assert!(
            (defaults.long_reading_minimum_switch_margin
                - DEFAULT_LONG_READING_MINIMUM_SWITCH_MARGIN)
                .abs()
                < f64::EPSILON
        );

        let overridden = Options::parse(
            [
                "model.gguf",
                "--input",
                "items.json",
                "--switch-margin",
                "0.2",
                "--long-switch-margin",
                "0.4",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert!((overridden.minimum_switch_margin - 0.2).abs() < f64::EPSILON);
        assert!((overridden.long_reading_minimum_switch_margin - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_guarded_candidate_expansion_once() {
        let options = Options::parse(
            [
                "model.gguf",
                "--input",
                "items.json",
                "--guarded-candidate-expansion",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert!(options.guarded_candidate_expansion);

        let error = Options::parse(
            [
                "model.gguf",
                "--input",
                "items.json",
                "--guarded-candidate-expansion",
                "--guarded-candidate-expansion",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap_err();
        assert_eq!(error, "--guarded-candidate-expansion is duplicated");
    }

    #[test]
    fn classifies_live_failures_by_space_recovery_boundary() {
        assert_eq!(recovery_class(true, false, false), None);
        assert_eq!(
            recovery_class(false, true, true),
            Some(RecoveryClass::SpaceTop1)
        );
        assert_eq!(
            recovery_class(false, false, true),
            Some(RecoveryClass::CandidateMove)
        );
        assert_eq!(
            recovery_class(false, false, false),
            Some(RecoveryClass::MissingFromCandidatePool)
        );
    }
}
