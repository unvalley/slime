//! Replays kana input one character at a time and measures LIVE preedit
//! stability. This complements final-candidate evaluation by exposing numeric
//! injections, converted-to-kana reversions, and already displayed characters
//! that are rewritten by a later input prefix.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use slime_core::{EnginePreferences, InputEvent, SlimeEngine, UserData};

const DEFAULT_FAILURES: usize = 10;

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
    let mut items: Vec<EvaluationItem> = serde_json::from_slice(&source)
        .map_err(|error| format!("failed to parse {}: {error}", options.input.display()))?;
    if let Some(limit) = options.limit {
        items.truncate(limit);
    }
    if items.is_empty() {
        return Err("evaluation input has no items".to_owned());
    }
    let report = evaluate(&items, &options);
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|_| "cannot serialize LIVE transition report".to_owned())?
        );
    } else {
        println!(
            "items={} steps={} final-accuracy={:.4} numeric-items={} rewrite-events={} rollback-items={} rollback-characters={} kana-reversion-items={} sealable-rewrite-items={} latency-p95-ms={:.3}",
            report.items,
            report.steps,
            report.final_accuracy,
            report.numeric_injection_items,
            report.rewrite_events,
            report.rollback_items,
            report.rollback_characters,
            report.kana_reversion_items,
            report.sealable_rewrite_items,
            report.latency_ms.p95,
        );
    }
    Ok(())
}

const fn usage() -> &'static str {
    "usage: slime-live-transition-evaluate --input ITEMS.json [--data-dir PATH] \
     [--limit N] [--failures N] [--delayed-ranking] [--json]"
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    data_directory: Option<PathBuf>,
    limit: Option<usize>,
    failures: usize,
    delayed_ranking: bool,
    json: bool,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut input = None;
        let mut data_directory = None;
        let mut limit = None;
        let mut failures = DEFAULT_FAILURES;
        let mut delayed_ranking = false;
        let mut json = false;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--input" if input.is_none() => {
                    input = Some(PathBuf::from(next_value(&mut arguments, "--input")?));
                }
                "--input" => return Err("--input is duplicated".to_owned()),
                "--data-dir" if data_directory.is_none() => {
                    data_directory = Some(PathBuf::from(next_value(&mut arguments, "--data-dir")?));
                }
                "--data-dir" => return Err("--data-dir is duplicated".to_owned()),
                "--limit" if limit.is_none() => {
                    limit = Some(parse_positive(&mut arguments, "--limit")?);
                }
                "--limit" => return Err("--limit is duplicated".to_owned()),
                "--failures" => failures = parse_non_negative(&mut arguments, "--failures")?,
                "--delayed-ranking" if !delayed_ranking => delayed_ranking = true,
                "--delayed-ranking" => {
                    return Err("--delayed-ranking is duplicated".to_owned());
                }
                "--json" if !json => json = true,
                "--json" => return Err("--json is duplicated".to_owned()),
                "--help" | "-h" => return Err(usage().to_owned()),
                _ => return Err(format!("unknown option\n{}", usage())),
            }
        }
        Ok(Self {
            input: input.ok_or_else(|| usage().to_owned())?,
            data_directory,
            limit,
            failures,
            delayed_ranking,
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

fn parse_non_negative(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<usize, String> {
    next_value(arguments, option)?
        .parse::<usize>()
        .map_err(|_| format!("{option} requires a non-negative integer"))
}

fn parse_positive(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<usize, String> {
    let value = parse_non_negative(arguments, option)?;
    if value == 0 {
        return Err(format!("{option} requires a positive integer"));
    }
    Ok(value)
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
    steps: usize,
    final_accuracy: f64,
    numeric_injection_items: usize,
    numeric_injection_steps: usize,
    rewrite_events: usize,
    rewrite_characters: usize,
    rollback_items: usize,
    rollback_events: usize,
    rollback_characters: usize,
    kana_reversion_items: usize,
    kana_reversion_events: usize,
    sealable_rewrite_items: usize,
    sealable_rewrite_events: usize,
    latency_ms: LatencyReport,
    failures: Vec<ItemFailure>,
}

#[derive(Debug, Serialize)]
struct LatencyReport {
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

#[derive(Debug, Serialize)]
struct ItemFailure {
    index: String,
    input: String,
    expected_output: Vec<String>,
    final_preedit: String,
    numeric_injection_steps: usize,
    rewrite_events: usize,
    rewrite_characters: usize,
    rollback_events: usize,
    rollback_characters: usize,
    kana_reversion_events: usize,
    sealable_rewrite_events: usize,
    transitions: Vec<Transition>,
}

#[derive(Debug, Serialize)]
struct Transition {
    input_prefix: String,
    previous_preedit: String,
    preedit: String,
    #[serde(flatten)]
    flags: TransitionFlags,
    sealable_rewrite: bool,
}

#[derive(Debug, Serialize)]
struct TransitionFlags {
    numeric_injection: bool,
    rewritten_characters: usize,
    converted_rollback: bool,
    kana_reversion: bool,
}

#[derive(Default)]
struct ItemMetrics {
    numeric_injection_steps: usize,
    rewrite_events: usize,
    rewrite_characters: usize,
    rollback_events: usize,
    rollback_characters: usize,
    kana_reversion_events: usize,
    sealable_rewrite_events: usize,
    transitions: Vec<Transition>,
}

fn evaluate(items: &[EvaluationItem], options: &Options) -> Report {
    let mut steps = 0_usize;
    let mut correct = 0_usize;
    let mut numeric_injection_items = 0_usize;
    let mut numeric_injection_steps = 0_usize;
    let mut rewrite_events = 0_usize;
    let mut rewrite_characters = 0_usize;
    let mut rollback_items = 0_usize;
    let mut rollback_events = 0_usize;
    let mut rollback_characters = 0_usize;
    let mut kana_reversion_items = 0_usize;
    let mut kana_reversion_events = 0_usize;
    let mut sealable_rewrite_items = 0_usize;
    let mut sealable_rewrite_events = 0_usize;
    let mut latencies = Vec::new();
    let mut failures = Vec::new();

    for item in items {
        let (final_preedit, metrics) = replay_item(item, options, &mut latencies, &mut steps);
        let final_is_correct = item
            .expected_output
            .iter()
            .any(|expected| expected == &final_preedit);
        if final_is_correct {
            correct = correct.saturating_add(1);
        }
        numeric_injection_items += usize::from(metrics.numeric_injection_steps > 0);
        numeric_injection_steps =
            numeric_injection_steps.saturating_add(metrics.numeric_injection_steps);
        rewrite_events = rewrite_events.saturating_add(metrics.rewrite_events);
        rewrite_characters = rewrite_characters.saturating_add(metrics.rewrite_characters);
        rollback_items += usize::from(metrics.rollback_events > 0);
        rollback_events = rollback_events.saturating_add(metrics.rollback_events);
        rollback_characters = rollback_characters.saturating_add(metrics.rollback_characters);
        kana_reversion_items += usize::from(metrics.kana_reversion_events > 0);
        kana_reversion_events = kana_reversion_events.saturating_add(metrics.kana_reversion_events);
        sealable_rewrite_items += usize::from(metrics.sealable_rewrite_events > 0);
        sealable_rewrite_events =
            sealable_rewrite_events.saturating_add(metrics.sealable_rewrite_events);
        if failures.len() < options.failures
            && (!final_is_correct
                || metrics.numeric_injection_steps > 0
                || metrics.rollback_events > 0
                || metrics.kana_reversion_events > 0)
        {
            failures.push(ItemFailure {
                index: item.index.clone(),
                input: item.input.clone(),
                expected_output: item.expected_output.clone(),
                final_preedit,
                numeric_injection_steps: metrics.numeric_injection_steps,
                rewrite_events: metrics.rewrite_events,
                rewrite_characters: metrics.rewrite_characters,
                rollback_events: metrics.rollback_events,
                rollback_characters: metrics.rollback_characters,
                kana_reversion_events: metrics.kana_reversion_events,
                sealable_rewrite_events: metrics.sealable_rewrite_events,
                transitions: metrics.transitions,
            });
        }
    }

    Report {
        items: items.len(),
        steps,
        final_accuracy: usize_to_f64(correct) / usize_to_f64(items.len()),
        numeric_injection_items,
        numeric_injection_steps,
        rewrite_events,
        rewrite_characters,
        rollback_items,
        rollback_events,
        rollback_characters,
        kana_reversion_items,
        kana_reversion_events,
        sealable_rewrite_items,
        sealable_rewrite_events,
        latency_ms: latency_report(latencies),
        failures,
    }
}

fn replay_item(
    item: &EvaluationItem,
    options: &Options,
    latencies: &mut Vec<Duration>,
    steps: &mut usize,
) -> (String, ItemMetrics) {
    let mut engine = options
        .data_directory
        .as_ref()
        .map_or_else(SlimeEngine::bundled, |directory| {
            SlimeEngine::bundled_with_user_data(UserData::load(directory))
        });
    engine.set_preferences(EnginePreferences {
        live_conversion: true,
        ..EnginePreferences::default()
    });
    engine.set_delayed_live_ranking_available(options.delayed_ranking);
    engine.set_external_left_context(&item.context_text);
    let input = katakana_to_hiragana(&item.input);
    let mut prefix = String::new();
    let mut previous_preedit = String::new();
    let mut metrics = ItemMetrics::default();
    for character in input.chars() {
        prefix.push(character);
        let sealable_surface = engine
            .evaluation_live_sealable_bunsetsu()
            .map(|(_, surface)| surface.to_owned());
        let started = Instant::now();
        engine.handle(InputEvent::Character(character));
        latencies.push(started.elapsed());
        *steps = steps.saturating_add(1);
        let preedit = engine.snapshot().preedit;
        classify_transition(
            &prefix,
            &previous_preedit,
            &preedit,
            sealable_surface.as_deref(),
            &mut metrics,
        );
        previous_preedit = preedit;
    }
    (engine.snapshot().preedit, metrics)
}

fn classify_transition(
    input_prefix: &str,
    previous_preedit: &str,
    preedit: &str,
    sealable_surface: Option<&str>,
    metrics: &mut ItemMetrics,
) {
    let numeric_injection = !contains_numeric(input_prefix) && contains_numeric(preedit);
    metrics.numeric_injection_steps += usize::from(numeric_injection);

    let previous_length = previous_preedit.chars().count();
    let common_prefix = common_prefix_characters(previous_preedit, preedit);
    let rewritten = previous_length.saturating_sub(common_prefix);
    if !previous_preedit.is_empty() && rewritten > 0 {
        metrics.rewrite_events = metrics.rewrite_events.saturating_add(1);
        metrics.rewrite_characters = metrics.rewrite_characters.saturating_add(rewritten);
    }
    let converted_rollback = rewritten > 0 && contains_converted_surface(previous_preedit);
    if converted_rollback {
        metrics.rollback_events = metrics.rollback_events.saturating_add(1);
        metrics.rollback_characters = metrics.rollback_characters.saturating_add(rewritten);
    }

    let kana_reversion = contains_converted_surface(previous_preedit) && preedit == input_prefix;
    metrics.kana_reversion_events += usize::from(kana_reversion);
    let sealable_rewrite = sealable_surface.is_some_and(|surface| !preedit.starts_with(surface));
    metrics.sealable_rewrite_events += usize::from(sealable_rewrite);
    if numeric_injection || converted_rollback || kana_reversion || sealable_rewrite {
        metrics.transitions.push(Transition {
            input_prefix: input_prefix.to_owned(),
            previous_preedit: previous_preedit.to_owned(),
            preedit: preedit.to_owned(),
            flags: TransitionFlags {
                numeric_injection,
                rewritten_characters: rewritten,
                converted_rollback,
                kana_reversion,
            },
            sealable_rewrite,
        });
    }
}

fn contains_numeric(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_ascii_digit() || matches!(character, '０'..='９'))
}

fn contains_converted_character(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(character, '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'))
}

fn contains_converted_surface(value: &str) -> bool {
    contains_converted_character(value)
        || contains_numeric(value)
        || value.chars().any(is_katakana_letter)
}

fn is_katakana_letter(character: char) -> bool {
    (matches!(character, 'ァ'..='ヺ') && !matches!(character, '・' | 'ー'))
        || (matches!(character, 'ｦ'..='ﾟ') && !matches!(character, '･' | 'ｰ' | 'ﾞ' | 'ﾟ'))
}

fn common_prefix_characters(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
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
    durations.sort_unstable();
    LatencyReport {
        p50: percentile(&durations, 50),
        p95: percentile(&durations, 95),
        p99: percentile(&durations, 99),
        max: duration_to_millis(*durations.last().expect("non-empty durations")),
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

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).expect("evaluation count fits u32"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        EvaluationItem, ItemMetrics, Options, classify_transition, contains_converted_surface,
        evaluate,
    };

    #[test]
    fn classifies_initial_numeric_conversion_as_a_rewrite_not_a_rollback() {
        let mut metrics = ItemMetrics::default();
        classify_transition("いち", "い", "1", None, &mut metrics);
        assert_eq!(metrics.numeric_injection_steps, 1);
        assert_eq!(metrics.rewrite_events, 1);
        assert_eq!(metrics.rewrite_characters, 1);
        assert_eq!(metrics.rollback_events, 0);
        assert_eq!(metrics.rollback_characters, 0);
    }

    #[test]
    fn extension_does_not_count_as_a_rollback() {
        let mut metrics = ItemMetrics::default();
        classify_transition("にほんご", "日本", "日本語", None, &mut metrics);
        assert_eq!(metrics.rollback_events, 0);
        assert_eq!(metrics.rollback_characters, 0);
    }

    #[test]
    fn rewrite_after_conversion_counts_as_a_rollback() {
        let mut metrics = ItemMetrics::default();
        classify_transition("にほんご", "二本", "日本語", None, &mut metrics);
        assert_eq!(metrics.rewrite_events, 1);
        assert_eq!(metrics.rollback_events, 1);
        assert_eq!(metrics.rollback_characters, 2);
    }

    #[test]
    fn detects_a_converted_to_kana_reversion() {
        let mut metrics = ItemMetrics::default();
        classify_transition("にほんごを", "日本語", "にほんごを", None, &mut metrics);
        assert_eq!(metrics.kana_reversion_events, 1);
    }

    #[test]
    fn long_vowel_marks_do_not_make_hiragana_a_converted_surface() {
        assert!(!contains_converted_surface("でにーす"));
        assert!(contains_converted_surface("デニース"));
    }

    #[test]
    fn detects_when_a_sealable_bunsetsu_is_rewritten() {
        let mut metrics = ItemMetrics::default();
        classify_transition(
            "へんかんがつ",
            "変換が",
            "へんかんがつ",
            Some("変換が"),
            &mut metrics,
        );
        assert_eq!(metrics.sealable_rewrite_events, 1);
        assert!(metrics.transitions[0].sealable_rewrite);
    }

    #[test]
    fn rejects_a_zero_item_limit() {
        let error = Options::parse(
            ["--input", "items.json", "--limit", "0"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect_err("zero limit must be rejected");
        assert_eq!(error, "--limit requires a positive integer");
    }

    #[test]
    fn reports_a_final_error_even_without_a_transition_anomaly() {
        let report = evaluate(
            &[EvaluationItem {
                index: "silent-final-error".to_owned(),
                context_text: String::new(),
                input: "あ".to_owned(),
                expected_output: vec!["い".to_owned()],
            }],
            &Options {
                input: PathBuf::new(),
                data_directory: None,
                limit: None,
                failures: 1,
                delayed_ranking: false,
                json: false,
            },
        );

        assert!(report.final_accuracy.abs() < f64::EPSILON);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].index, "silent-final-error");
    }
}
