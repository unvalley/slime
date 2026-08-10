//! Measures dictionary-constrained greedy conversion from a local zenz model.

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use serde::Deserialize;
use slime_converter::{Candidate, Dictionary};
use slime_neural::{PrefixDiagnostic, Rescorer, ScoreRequest};

const MAXIMUM_OUTPUT_TOKENS: usize = 64;
const MINIMUM_GENERATIVE_READING_CHARACTERS: usize = 6;
const MAXIMUM_GENERATIVE_READING_CHARACTERS: usize = 32;
const SEARCH_LIMIT: usize = 32;
const SHORT_CANDIDATES: usize = 5;
const CONFIDENCE_BYPASS_CANDIDATES: usize = 8;
const LONG_INPUT_CHARACTERS: usize = 9;
const VERY_LONG_INPUT_CHARACTERS: usize = 20;
const MAX_BASE_COST_GAP: i32 = 1_000;
const SHORT_MAX_CANDIDATE_COST_GAP: i32 = 1_500;
const LONG_MAX_CANDIDATE_COST_GAP: i32 = 2_500;
const COST_LOG_SCALE: f64 = 500.0;
const SUPPLEMENTAL_ADDITIONAL_MARGIN: f64 = 1.5;
const MIN_PREFIX_CHARACTERS: usize = 4;
const MIN_LOGIT_MARGIN: f32 = 1.5;
const CONSTRAINED_CANDIDATES: usize = 8;
const MAX_CHANGED_CHARACTERS: usize = 2;

#[derive(Debug, Deserialize)]
struct Item {
    #[serde(default)]
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
        .ok_or("usage: neural_greedy_probe INPUT.json MODEL.gguf")?;
    let model_path = arguments
        .next()
        .ok_or("usage: neural_greedy_probe INPUT.json MODEL.gguf")?;
    if arguments.next().is_some() {
        return Err("usage: neural_greedy_probe INPUT.json MODEL.gguf".to_owned());
    }
    let items: Vec<Item> = serde_json::from_str(
        &fs::read_to_string(Path::new(&input_path))
            .map_err(|error| format!("failed to read input: {error}"))?,
    )
    .map_err(|error| format!("failed to parse input: {error}"))?;
    let requests = items
        .iter()
        .map(|item| ScoreRequest {
            context: item.context_text.clone(),
            right_context: item.right_context_text.clone(),
            input_katakana: item.input.clone(),
            candidates: Vec::new(),
        })
        .collect::<Vec<_>>();
    let rescorer = Rescorer::load(Path::new(&model_path))?;
    let generated = rescorer.generate_greedy_outputs(&requests, MAXIMUM_OUTPUT_TOKENS)?;
    let dictionary = Dictionary::bundled();

    let mut ordinary_sets = Vec::with_capacity(items.len());
    let mut augmented_sets = Vec::with_capacity(items.len());
    let mut original_counts = Vec::with_capacity(items.len());
    let mut score_requests = Vec::new();
    for (item, generated) in items.iter().zip(&generated) {
        let reading = katakana_to_hiragana(&item.input);
        let ordinary = dictionary.candidates_with_surrounding_context_limit(
            &reading,
            &item.context_text,
            &item.right_context_text,
            SEARCH_LIMIT,
        );
        let original_count = high_accuracy_candidate_count(item, &ordinary);
        let mut augmented = ordinary
            .iter()
            .take(original_count)
            .cloned()
            .collect::<Vec<_>>();
        if original_count >= 2
            && generated.stopped_at_eos
            && let Some(conversion) = dictionary
                .convert_n_best_with_surface_prefix(
                    &reading,
                    &generated.surface,
                    CONSTRAINED_CANDIDATES,
                )
                .into_iter()
                .find(|conversion| conversion.surface == generated.surface)
            && !augmented
                .iter()
                .any(|candidate| candidate.surface == conversion.surface)
            && item.input.chars().count() <= MAXIMUM_GENERATIVE_READING_CHARACTERS
            && bounded_multi_region_substitution(&ordinary[0].surface, &conversion.surface)
        {
            let maximum_gap = if item.input.chars().count() >= LONG_INPUT_CHARACTERS {
                LONG_MAX_CANDIDATE_COST_GAP
            } else {
                SHORT_MAX_CANDIDATE_COST_GAP
            };
            if conversion.cost.saturating_sub(ordinary[0].cost) <= maximum_gap {
                augmented.push(Candidate {
                    surface: conversion.surface,
                    cost: conversion.cost,
                });
            }
        }
        if original_count >= 2 {
            score_requests.push(ScoreRequest {
                context: item.context_text.clone(),
                right_context: item.right_context_text.clone(),
                input_katakana: item.input.clone(),
                candidates: augmented
                    .iter()
                    .map(|candidate| candidate.surface.clone())
                    .collect(),
            });
        }
        ordinary_sets.push(ordinary);
        augmented_sets.push(augmented);
        original_counts.push(original_count);
    }
    let mut scored_items = rescorer
        .score_all_with_prefix_diagnostics(&score_requests)?
        .into_iter();

    let mut raw_correct = 0usize;
    let mut dictionary_backed = 0usize;
    let mut backed_correct = 0usize;
    let mut base_correct = 0usize;
    let mut backed_improvements = 0usize;
    let mut backed_regressions = 0usize;
    let mut stopped_at_eos = 0usize;
    let mut current_rescore_correct = 0usize;
    let mut augmented_rescore_correct = 0usize;
    let mut augmented_improvements = 0usize;
    let mut augmented_regressions = 0usize;
    let mut generated_candidates = 0usize;
    let mut current_prefix_correct = 0usize;
    let mut augmented_prefix_correct = 0usize;
    let mut current_iterative_correct = 0usize;
    let mut augmented_iterative_correct = 0usize;
    let mut iterative_improvements = 0usize;
    let mut iterative_regressions = 0usize;
    let mut latencies = Vec::with_capacity(generated.len());
    let mut eligible_latencies = Vec::new();
    for ((((item, generated), ordinary), augmented), &original_count) in items
        .iter()
        .zip(&generated)
        .zip(&ordinary_sets)
        .zip(&augmented_sets)
        .zip(&original_counts)
    {
        let reading = katakana_to_hiragana(&item.input);
        let base = ordinary
            .first()
            .map_or_else(|| reading.clone(), |candidate| candidate.surface.clone());
        let base_matches = matches_expected(&base, &item.expected_output);
        let raw_matches = matches_expected(&generated.surface, &item.expected_output);
        let backed = dictionary
            .convert_n_best_with_surface_prefix(
                &reading,
                &generated.surface,
                CONSTRAINED_CANDIDATES,
            )
            .into_iter()
            .find(|conversion| conversion.surface == generated.surface)
            .map(|conversion| conversion.surface);
        let backed_matches = backed
            .as_deref()
            .is_some_and(|surface| matches_expected(surface, &item.expected_output));

        base_correct += usize::from(base_matches);
        raw_correct += usize::from(raw_matches);
        dictionary_backed += usize::from(backed.is_some());
        backed_correct += usize::from(backed_matches);
        backed_improvements += usize::from(!base_matches && backed_matches);
        backed_regressions += usize::from(base_matches && backed.is_some() && !backed_matches);
        stopped_at_eos += usize::from(generated.stopped_at_eos);
        latencies.push(generated.latency);
        if (MINIMUM_GENERATIVE_READING_CHARACTERS..=MAXIMUM_GENERATIVE_READING_CHARACTERS)
            .contains(&item.input.chars().count())
        {
            eligible_latencies.push(generated.latency);
        }

        let (current_surface, augmented_surface, current_prefix, augmented_prefix) =
            if original_count >= 2 {
                let scored = scored_items
                    .next()
                    .expect("one score for each eligible candidate set");
                let current_index = rescored_index(
                    item,
                    &augmented[..original_count],
                    &scored.candidate_logliks[..original_count],
                    false,
                );
                let current_surface = augmented[current_index].surface.clone();
                let current_prefix = apply_prefix_correction(
                    &dictionary,
                    &reading,
                    &current_surface,
                    scored.first_mismatch_prefixes[current_index].as_ref(),
                )
                .unwrap_or_else(|| current_surface.clone());
                let augmented_index = rescored_index(
                    item,
                    augmented,
                    &scored.candidate_logliks,
                    augmented.len() > original_count,
                );
                let augmented_surface = augmented[augmented_index].surface.clone();
                let augmented_prefix = apply_prefix_correction(
                    &dictionary,
                    &reading,
                    &augmented_surface,
                    scored.first_mismatch_prefixes[augmented_index].as_ref(),
                )
                .unwrap_or_else(|| augmented_surface.clone());
                (
                    current_surface,
                    augmented_surface,
                    current_prefix,
                    augmented_prefix,
                )
            } else {
                (base.clone(), base.clone(), base.clone(), base.clone())
            };
        let current_iterative = apply_followup_prefix(
            &rescorer,
            &dictionary,
            item,
            &reading,
            &current_surface,
            &current_prefix,
        )?
        .unwrap_or_else(|| current_prefix.clone());
        let augmented_iterative = apply_followup_prefix(
            &rescorer,
            &dictionary,
            item,
            &reading,
            &augmented_surface,
            &augmented_prefix,
        )?
        .unwrap_or_else(|| augmented_prefix.clone());
        let current_matches = matches_expected(&current_surface, &item.expected_output);
        let augmented_matches = matches_expected(&augmented_surface, &item.expected_output);
        current_rescore_correct += usize::from(current_matches);
        augmented_rescore_correct += usize::from(augmented_matches);
        augmented_improvements += usize::from(!current_matches && augmented_matches);
        augmented_regressions += usize::from(current_matches && !augmented_matches);
        generated_candidates += usize::from(augmented.len() > original_count);
        current_prefix_correct +=
            usize::from(matches_expected(&current_prefix, &item.expected_output));
        augmented_prefix_correct +=
            usize::from(matches_expected(&augmented_prefix, &item.expected_output));
        let current_iterative_matches = matches_expected(&current_iterative, &item.expected_output);
        let augmented_iterative_matches =
            matches_expected(&augmented_iterative, &item.expected_output);
        current_iterative_correct += usize::from(current_iterative_matches);
        augmented_iterative_correct += usize::from(augmented_iterative_matches);
        iterative_improvements +=
            usize::from(!current_iterative_matches && augmented_iterative_matches);
        iterative_regressions +=
            usize::from(current_iterative_matches && !augmented_iterative_matches);
        if current_iterative_matches != augmented_iterative_matches {
            println!(
                "iterative_change\t{}\t{}\tcurrent={}\taugmented={}\tgenerated={}\texpected={}",
                item.index,
                if augmented_iterative_matches {
                    "improve"
                } else {
                    "regress"
                },
                current_iterative,
                augmented_iterative,
                generated.surface,
                item.expected_output.join(" | "),
            );
        }
    }
    debug_assert!(scored_items.next().is_none());

    latencies.sort_unstable();
    eligible_latencies.sort_unstable();
    println!("items={}", items.len());
    println!("base_top1={base_correct}");
    println!("raw_greedy_top1={raw_correct}");
    println!("dictionary_backed={dictionary_backed}");
    println!("dictionary_backed_correct={backed_correct}");
    println!("potential_improvements={backed_improvements}");
    println!("potential_regressions={backed_regressions}");
    println!("generated_candidates={generated_candidates}");
    println!("current_rescore_top1={current_rescore_correct}");
    println!("augmented_rescore_top1={augmented_rescore_correct}");
    println!("augmented_improvements={augmented_improvements}");
    println!("augmented_regressions={augmented_regressions}");
    println!("current_prefix_top1={current_prefix_correct}");
    println!("augmented_prefix_top1={augmented_prefix_correct}");
    println!("current_iterative_top1={current_iterative_correct}");
    println!("augmented_iterative_top1={augmented_iterative_correct}");
    println!("iterative_improvements={iterative_improvements}");
    println!("iterative_regressions={iterative_regressions}");
    println!("stopped_at_eos={stopped_at_eos}");
    println!("generation_p50_ms={:.3}", percentile_ms(&latencies, 50));
    println!("generation_p95_ms={:.3}", percentile_ms(&latencies, 95));
    println!(
        "eligible_generation_p50_ms={:.3}",
        percentile_ms(&eligible_latencies, 50)
    );
    println!(
        "eligible_generation_p95_ms={:.3}",
        percentile_ms(&eligible_latencies, 95)
    );
    Ok(())
}

fn apply_prefix_correction(
    dictionary: &Dictionary,
    reading: &str,
    current: &str,
    diagnostic: Option<&PrefixDiagnostic>,
) -> Option<String> {
    let diagnostic = diagnostic?;
    if diagnostic.alternative_is_eos
        || diagnostic.prefix.chars().count() < MIN_PREFIX_CHARACTERS
        || diagnostic.alternative_logit - diagnostic.candidate_logit < MIN_LOGIT_MARGIN
    {
        return None;
    }
    let alternative = dictionary
        .convert_n_best_with_surface_prefix(reading, &diagnostic.prefix, CONSTRAINED_CANDIDATES)
        .into_iter()
        .next()?
        .surface;
    bounded_local_substitution(current, &alternative, MAX_CHANGED_CHARACTERS).then_some(alternative)
}

fn apply_followup_prefix(
    rescorer: &Rescorer,
    dictionary: &Dictionary,
    item: &Item,
    reading: &str,
    original: &str,
    corrected: &str,
) -> Result<Option<String>, String> {
    if corrected == original {
        return Ok(None);
    }
    let request = ScoreRequest {
        context: item.context_text.clone(),
        right_context: item.right_context_text.clone(),
        input_katakana: item.input.clone(),
        candidates: vec![corrected.to_owned()],
    };
    let diagnostic = rescorer
        .score_all_with_prefix_diagnostics(&[request])?
        .remove(0)
        .first_mismatch_prefixes
        .remove(0);
    Ok(
        apply_prefix_correction(dictionary, reading, corrected, diagnostic.as_ref())
            .filter(|alternative| alternative != original),
    )
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

fn bounded_multi_region_substitution(current: &str, alternative: &str) -> bool {
    const MINIMUM_REGIONS: usize = 2;
    const MAXIMUM_REGIONS: usize = 4;
    const MAXIMUM_REGION_CHARACTERS: usize = 2;

    let current = current.chars().collect::<Vec<_>>();
    let alternative = alternative.chars().collect::<Vec<_>>();
    if current.len() != alternative.len() {
        return false;
    }
    let mut regions = 0usize;
    let mut region_characters = 0usize;
    for (&current, &alternative) in current.iter().zip(&alternative) {
        if current == alternative {
            region_characters = 0;
            continue;
        }
        if current.is_ascii_alphanumeric() || alternative.is_ascii_alphanumeric() {
            return false;
        }
        if region_characters == 0 {
            regions += 1;
            if regions > MAXIMUM_REGIONS {
                return false;
            }
        }
        region_characters += 1;
        if region_characters > MAXIMUM_REGION_CHARACTERS {
            return false;
        }
    }
    regions >= MINIMUM_REGIONS
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

fn rescored_index(
    item: &Item,
    candidates: &[Candidate],
    logliks: &[f64],
    last_is_supplemental: bool,
) -> usize {
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
    let required_margin = minimum_margin
        + if last_is_supplemental && top + 1 == count {
            SUPPLEMENTAL_ADDITIONAL_MARGIN
        } else {
            0.0
        };
    if top != 0 && combined[top] - combined[0] < required_margin {
        0
    } else {
        top
    }
}

fn matches_expected(surface: &str, expected: &[String]) -> bool {
    expected.iter().any(|expected| expected == surface)
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
