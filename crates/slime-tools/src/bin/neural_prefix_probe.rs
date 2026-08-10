//! Reproduces the high-accuracy model-directed local prefix correction.

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::Deserialize;
use slime_converter::{Candidate, Dictionary};
use slime_neural::{Rescorer, ScoreRequest};

const SEARCH_LIMIT: usize = 32;
const SHORT_CANDIDATES: usize = 5;
const CONFIDENCE_BYPASS_CANDIDATES: usize = 8;
const CONSTRAINED_CANDIDATES: usize = 8;
const LONG_INPUT_CHARACTERS: usize = 9;
const VERY_LONG_INPUT_CHARACTERS: usize = 20;
const MAX_BASE_COST_GAP: i32 = 1_000;
const SHORT_MAX_CANDIDATE_COST_GAP: i32 = 1_500;
const LONG_MAX_CANDIDATE_COST_GAP: i32 = 2_500;
const MIN_PREFIX_CHARACTERS: usize = 4;
const MIN_LOGIT_MARGIN: f32 = 2.0;
const MAX_CHANGED_CHARACTERS: usize = 2;
const COST_LOG_SCALE: f64 = 500.0;

#[derive(Debug, Deserialize)]
struct Item {
    index: String,
    #[serde(default)]
    context_text: String,
    #[serde(default)]
    right_context_text: String,
    input: String,
    expected_output: Vec<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_lines)]
fn run() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let input_path = arguments
        .next()
        .ok_or("usage: neural_prefix_probe INPUT.json MODEL.gguf [--failures N]")?;
    let model_path = arguments
        .next()
        .ok_or("usage: neural_prefix_probe INPUT.json MODEL.gguf [--failures N]")?;
    let failure_limit = match (arguments.next(), arguments.next()) {
        (None, None) => 0,
        (Some(flag), Some(value)) if flag == "--failures" => value
            .to_str()
            .ok_or("--failures must be UTF-8")?
            .parse::<usize>()
            .map_err(|error| format!("invalid --failures value: {error}"))?,
        _ => {
            return Err(
                "usage: neural_prefix_probe INPUT.json MODEL.gguf [--failures N]".to_owned(),
            );
        }
    };
    if arguments.next().is_some() {
        return Err("usage: neural_prefix_probe INPUT.json MODEL.gguf [--failures N]".to_owned());
    }
    let items: Vec<Item> = serde_json::from_str(
        &fs::read_to_string(Path::new(&input_path))
            .map_err(|error| format!("failed to read input: {error}"))?,
    )
    .map_err(|error| format!("failed to parse input: {error}"))?;
    let dictionary = Dictionary::bundled();
    let mut candidate_sets = Vec::with_capacity(items.len());
    let mut candidate_counts = Vec::with_capacity(items.len());
    let mut requests = Vec::with_capacity(items.len());
    for item in &items {
        let reading = katakana_to_hiragana(&item.input);
        let candidates = dictionary.candidates_with_surrounding_context_limit(
            &reading,
            &item.context_text,
            &item.right_context_text,
            SEARCH_LIMIT,
        );
        let count = high_accuracy_candidate_count(item, &candidates);
        candidate_counts.push(count);
        if count >= 2 {
            requests.push(ScoreRequest {
                context: item.context_text.clone(),
                right_context: item.right_context_text.clone(),
                input_katakana: item.input.clone(),
                candidates: candidates
                    .iter()
                    .take(count)
                    .map(|candidate| candidate.surface.clone())
                    .collect(),
            });
        }
        candidate_sets.push(candidates);
    }

    let rescorer = Rescorer::load(Path::new(&model_path))?;
    let scored = rescorer.score_all_with_prefix_diagnostics(&requests)?;
    let mut scored = scored.into_iter();
    let mut baseline_correct = 0usize;
    let mut corrected_correct = 0usize;
    let mut improvements = 0usize;
    let mut regressions = 0usize;
    let mut eligible_constraints = 0usize;
    let mut applied_corrections = 0usize;
    let mut reported_failures = 0usize;
    let mut remaining_rank_1_32 = 0usize;
    let mut remaining_rank_33_64 = 0usize;
    let mut remaining_missing_64 = 0usize;
    let mut latencies = Vec::with_capacity(requests.len());
    let mut constrained_latencies = Vec::new();

    for ((item, candidates), &candidate_count) in
        items.iter().zip(&candidate_sets).zip(&candidate_counts)
    {
        let mut baseline = candidates[0].surface.clone();
        let mut corrected = baseline.clone();
        let mut selected_index = 0usize;
        let mut selected_prefix = None;
        if candidate_count >= 2 {
            let scored = scored.next().expect("one score for each selected request");
            latencies.push(scored.latency);
            let selected = rescored_index(item, candidates, &scored.candidate_logliks);
            selected_index = selected;
            baseline.clone_from(&candidates[selected].surface);
            corrected.clone_from(&baseline);
            if let Some(diagnostic) = &scored.first_mismatch_prefixes[selected]
                && !diagnostic.alternative_is_eos
                && diagnostic.prefix.chars().count() >= MIN_PREFIX_CHARACTERS
                && diagnostic.alternative_logit - diagnostic.candidate_logit >= MIN_LOGIT_MARGIN
            {
                eligible_constraints += 1;
                selected_prefix = Some(diagnostic.prefix.clone());
                let reading = katakana_to_hiragana(&item.input);
                let constrained_started = Instant::now();
                let alternative = dictionary
                    .convert_n_best_with_surface_prefix(
                        &reading,
                        &diagnostic.prefix,
                        CONSTRAINED_CANDIDATES,
                    )
                    .into_iter()
                    .next()
                    .map(|conversion| conversion.surface);
                constrained_latencies.push(constrained_started.elapsed());
                if let Some(alternative) = alternative
                    && bounded_local_substitution(&baseline, &alternative, MAX_CHANGED_CHARACTERS)
                {
                    corrected = alternative;
                    applied_corrections += 1;
                }
            }
        }

        let was_correct = matches_expected(&baseline, &item.expected_output);
        let is_correct = matches_expected(&corrected, &item.expected_output);
        baseline_correct += usize::from(was_correct);
        corrected_correct += usize::from(is_correct);
        improvements += usize::from(!was_correct && is_correct);
        regressions += usize::from(was_correct && !is_correct);
        if was_correct != is_correct {
            println!(
                "change\t{}\t{}\tbaseline={}\tcorrected={}\texpected={}",
                item.index,
                if is_correct { "improve" } else { "regress" },
                baseline,
                corrected,
                item.expected_output.join(" | "),
            );
        }
        let rank64 = if is_correct {
            None
        } else {
            let reading = katakana_to_hiragana(&item.input);
            let rank64 = dictionary
                .candidates_with_surrounding_context_limit(
                    &reading,
                    &item.context_text,
                    &item.right_context_text,
                    64,
                )
                .iter()
                .position(|candidate| matches_expected(&candidate.surface, &item.expected_output))
                .map(|index| index + 1);
            match rank64 {
                Some(1..=SEARCH_LIMIT) => remaining_rank_1_32 += 1,
                Some(_) => remaining_rank_33_64 += 1,
                None => remaining_missing_64 += 1,
            }
            rank64
        };
        if !is_correct && reported_failures < failure_limit {
            println!(
                "failure\t{}\tselected={}\tbaseline={}\tcorrected={}\texpected={}\trank64={}\tprefix={}",
                item.index,
                selected_index + 1,
                baseline,
                corrected,
                item.expected_output.join(" | "),
                rank64.map_or_else(|| "missing".to_owned(), |rank| rank.to_string()),
                selected_prefix.as_deref().unwrap_or("-"),
            );
            reported_failures += 1;
        }
    }
    debug_assert!(scored.next().is_none());
    latencies.sort_unstable();
    println!("items={}", items.len());
    println!(
        "baseline_top1={baseline_correct} ({:.6})",
        ratio(baseline_correct, items.len())
    );
    println!(
        "corrected_top1={corrected_correct} ({:.6})",
        ratio(corrected_correct, items.len())
    );
    println!("improvements={improvements}");
    println!("regressions={regressions}");
    println!("eligible_constraints={eligible_constraints}");
    println!("applied_corrections={applied_corrections}");
    println!("remaining_rank_1_32={remaining_rank_1_32}");
    println!("remaining_rank_33_64={remaining_rank_33_64}");
    println!("remaining_missing_64={remaining_missing_64}");
    println!("diagnostic_p50_ms={:.3}", percentile_ms(&latencies, 50));
    println!("diagnostic_p95_ms={:.3}", percentile_ms(&latencies, 95));
    constrained_latencies.sort_unstable();
    println!(
        "constrained_search_p50_ms={:.3}",
        percentile_ms(&constrained_latencies, 50)
    );
    println!(
        "constrained_search_p95_ms={:.3}",
        percentile_ms(&constrained_latencies, 95)
    );
    Ok(())
}

fn high_accuracy_candidate_count(item: &Item, candidates: &[Candidate]) -> usize {
    if candidates.len() < 2 {
        return 0;
    }
    let is_long = item.input.chars().count() >= LONG_INPUT_CHARACTERS;
    let normally_scored =
        candidates[1].cost.saturating_sub(candidates[0].cost).max(0) <= MAX_BASE_COST_GAP;
    let bypasses_confidence = is_long && item.right_context_text.is_empty() && !normally_scored;
    if !normally_scored && !bypasses_confidence {
        return 0;
    }
    let limit = if bypasses_confidence {
        CONFIDENCE_BYPASS_CANDIDATES
    } else if is_long {
        SEARCH_LIMIT
    } else {
        SHORT_CANDIDATES
    };
    let maximum_gap = if is_long {
        LONG_MAX_CANDIDATE_COST_GAP
    } else {
        SHORT_MAX_CANDIDATE_COST_GAP
    };
    let first_cost = candidates[0].cost;
    candidates
        .iter()
        .take(limit)
        .take_while(|candidate| candidate.cost.saturating_sub(first_cost) <= maximum_gap)
        .count()
}

fn rescored_index(item: &Item, candidates: &[Candidate], logliks: &[f64]) -> usize {
    let count = candidates.len().min(logliks.len());
    let has_right_context = !item.right_context_text.is_empty();
    let characters = item.input.chars().count();
    let is_long = characters >= LONG_INPUT_CHARACTERS;
    let lambda = if !has_right_context && characters >= VERY_LONG_INPUT_CHARACTERS {
        0.74
    } else {
        0.8
    };
    let minimum_margin = if has_right_context || !is_long {
        0.5
    } else {
        0.0
    };
    let combined = candidates
        .iter()
        .take(count)
        .zip(logliks)
        .map(|(candidate, loglik)| {
            (1.0 - lambda) * (-f64::from(candidate.cost) / COST_LOG_SCALE) + lambda * loglik
        })
        .collect::<Vec<_>>();
    let top = (0..count)
        .max_by(|&left, &right| combined[left].total_cmp(&combined[right]))
        .unwrap_or(0);
    if top != 0 && combined[top] - combined[0] < minimum_margin {
        0
    } else {
        top
    }
}

fn bounded_local_substitution(current: &str, alternative: &str, maximum_changes: usize) -> bool {
    let current = current.chars().collect::<Vec<_>>();
    let alternative = alternative.chars().collect::<Vec<_>>();
    if current.len() != alternative.len() {
        return false;
    }
    let changed = current
        .iter()
        .zip(&alternative)
        .enumerate()
        .filter_map(|(index, (current, alternative))| {
            (current != alternative).then_some((index, *current, *alternative))
        })
        .collect::<Vec<_>>();
    let Some((&(first, _, _), &(last, _, _))) = changed.first().zip(changed.last()) else {
        return false;
    };
    changed.len() <= maximum_changes
        && last - first + 1 == changed.len()
        && changed.iter().all(|&(_, current, alternative)| {
            !current.is_ascii_alphanumeric() && !alternative.is_ascii_alphanumeric()
        })
}

fn matches_expected(surface: &str, expected: &[String]) -> bool {
    expected.iter().any(|expected| expected == surface)
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        f64::from(u32::try_from(numerator).expect("evaluation count fits u32"))
            / f64::from(u32::try_from(denominator).expect("evaluation count fits u32"))
    }
}

fn percentile_ms(samples: &[Duration], percentile: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = (samples.len() - 1)
        .saturating_mul(percentile)
        .saturating_add(99)
        / 100;
    samples[index].as_secs_f64() * 1_000.0
}

fn katakana_to_hiragana(input: &str) -> String {
    input
        .chars()
        .map(|character| match character {
            '\u{30A1}'..='\u{30F6}' => {
                char::from_u32(u32::from(character) - 0x60).expect("valid hiragana scalar")
            }
            _ => character,
        })
        .collect()
}
