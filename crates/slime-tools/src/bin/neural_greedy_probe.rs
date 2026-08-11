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
const WHOLE_RESULT_MAXIMUM_READING_CHARACTERS: usize = 40;
const LONG_WHOLE_RESULT_MINIMUM_COST_GAP: i32 = 500;
const SEARCH_LIMIT: usize = 32;
const SHORT_CANDIDATES: usize = 5;
const CONFIDENCE_BYPASS_CANDIDATES: usize = 8;
const LONG_INPUT_CHARACTERS: usize = 9;
const VERY_LONG_INPUT_CHARACTERS: usize = 20;
const MAX_BASE_COST_GAP: i32 = 1_000;
const SHORT_MAX_CANDIDATE_COST_GAP: i32 = 1_500;
const LONG_MAX_CANDIDATE_COST_GAP: i32 = 2_500;
const EXTENDED_GENERATIVE_COST_GAP: i32 = 3_100;
const COST_LOG_SCALE: f64 = 500.0;
const SUPPLEMENTAL_ADDITIONAL_MARGIN: f64 = 1.5;
const MIN_PREFIX_CHARACTERS: usize = 4;
const MIN_LOGIT_MARGIN: f32 = 1.5;
const CONSTRAINED_CANDIDATES: usize = 8;
const PREFIX_CONSTRAINED_CANDIDATES: usize = 32;
const MAX_CHANGED_CHARACTERS: usize = 2;
const GENERATIVE_CONSENSUS_MIN_MODEL_ADVANTAGE: f64 = 0.1;
const GENERATIVE_CONSENSUS_MAX_MODEL_ADVANTAGE: f64 = 0.2;
const MULTI_REGION_CONSENSUS_MAX_MODEL_ADVANTAGE: f64 = 0.25;
const CONTEXT_CONTRAST_WEIGHT: f64 = 0.1;
const WHOLE_RESULT_COST_GAP: i32 = 1_000;
const WHOLE_GENERATION_COST_GAPS: [i32; 7] = [500, 1_000, 1_500, 2_500, 3_100, 5_000, i32::MAX];
const SUPPLEMENTAL_WHOLE_COST_GAPS: [i32; 5] = [1_400, 1_500, 2_000, 2_500, 3_100];
const SUPPLEMENTAL_MODEL_MARGINS: [f64; 5] = [0.0, 0.5, 1.0, 1.5, 2.0];
const MODEL_TOP_MARGIN_THRESHOLDS: [f64; 6] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0];
const WHOLE_RESULT_READING_MAXIMUMS: [usize; 5] = [32, 40, 48, 64, usize::MAX];
const DELAYED_LONG_READING_MAXIMUMS: [usize; 4] = [40, 48, 64, usize::MAX];
const LONG_WHOLE_RESULT_COST_FLOORS: [i32; 5] = [0, 250, 500, 750, 1_000];
const SCORED_LONG_VARIANTS: [&str; 3] = ["eligible_top", "beats_base", "global_model_top"];
const WHOLE_RESULT_BLOCKS: [&str; 9] = [
    "incomplete",
    "reading_window",
    "long_pre_gate",
    "not_lattice_backed",
    "cost_gap",
    "long_cost_floor",
    "ascii",
    "kanji_deconversion",
    "personal_name",
];

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
        .ok_or("usage: neural_greedy_probe INPUT.json MODEL.gguf [--failures N]")?;
    let model_path = arguments
        .next()
        .ok_or("usage: neural_greedy_probe INPUT.json MODEL.gguf [--failures N]")?;
    let failure_limit = match (arguments.next(), arguments.next()) {
        (None, None) => 0,
        (Some(flag), Some(value)) if flag == "--failures" => value
            .to_str()
            .ok_or("--failures must be UTF-8")?
            .parse::<usize>()
            .map_err(|error| format!("invalid --failures value: {error}"))?,
        _ => {
            return Err(
                "usage: neural_greedy_probe INPUT.json MODEL.gguf [--failures N]".to_owned(),
            );
        }
    };
    if arguments.next().is_some() {
        return Err("usage: neural_greedy_probe INPUT.json MODEL.gguf [--failures N]".to_owned());
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
    let mut broad_supplemental_sets = Vec::with_capacity(items.len());
    let mut broad_supplemental_added = Vec::with_capacity(items.len());
    let mut original_counts = Vec::with_capacity(items.len());
    let mut score_requests = Vec::new();
    let mut broad_supplemental_score_requests = Vec::new();
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
            && (MINIMUM_GENERATIVE_READING_CHARACTERS..=MAXIMUM_GENERATIVE_READING_CHARACTERS)
                .contains(&item.input.chars().count())
        {
            let is_long = item.input.chars().count() >= LONG_INPUT_CHARACTERS;
            let is_multi_region =
                bounded_multi_region_substitution(&ordinary[0].surface, &conversion.surface);
            let is_surface_compression =
                bounded_multi_region_surface_compression(&ordinary[0].surface, &conversion.surface);
            let maximum_gap = if is_long && is_multi_region {
                EXTENDED_GENERATIVE_COST_GAP
            } else if is_long && is_surface_compression {
                LONG_MAX_CANDIDATE_COST_GAP
            } else if is_multi_region || is_surface_compression {
                SHORT_MAX_CANDIDATE_COST_GAP
            } else {
                MAX_BASE_COST_GAP
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
        let reading = katakana_to_hiragana(&item.input);
        let mut broad_supplemental = augmented.clone();
        let broad_conversion = (original_count >= 2
            && generated.stopped_at_eos
            && (MINIMUM_GENERATIVE_READING_CHARACTERS..=MAXIMUM_GENERATIVE_READING_CHARACTERS)
                .contains(&item.input.chars().count())
            && ordinary[0].surface.chars().count() == generated.surface.chars().count()
            && preserves_ascii_alphanumerics(&ordinary[0].surface, &generated.surface)
            && preserves_kanji_from_hiragana_deconversion(
                &ordinary[0].surface,
                &generated.surface,
            )
            && !dictionary.changes_exact_personal_name_segment(
                &reading,
                &ordinary[0].surface,
                &generated.surface,
            )
            && !broad_supplemental
                .iter()
                .any(|candidate| candidate.surface == generated.surface))
        .then(|| {
            dictionary
                .convert_n_best_with_surface_prefix(
                    &reading,
                    &generated.surface,
                    CONSTRAINED_CANDIDATES,
                )
                .into_iter()
                .find(|conversion| {
                    conversion.surface == generated.surface
                        && conversion.cost.saturating_sub(ordinary[0].cost).max(0)
                            <= *SUPPLEMENTAL_WHOLE_COST_GAPS
                                .last()
                                .expect("supplemental cost sweep")
                })
        })
        .flatten();
        let added_broad_supplemental = broad_conversion.is_some();
        if let Some(conversion) = broad_conversion {
            if broad_supplemental.len() >= SEARCH_LIMIT {
                broad_supplemental.pop();
            }
            broad_supplemental.push(Candidate {
                surface: conversion.surface,
                cost: conversion.cost,
            });
        }
        if added_broad_supplemental {
            broad_supplemental_score_requests.push(ScoreRequest {
                context: item.context_text.clone(),
                right_context: item.right_context_text.clone(),
                input_katakana: item.input.clone(),
                candidates: broad_supplemental
                    .iter()
                    .map(|candidate| candidate.surface.clone())
                    .collect(),
            });
        }
        ordinary_sets.push(ordinary);
        augmented_sets.push(augmented);
        broad_supplemental_sets.push(broad_supplemental);
        broad_supplemental_added.push(added_broad_supplemental);
        original_counts.push(original_count);
    }
    let mut scored_items = rescorer
        .score_all_with_prefix_diagnostics(&score_requests)?
        .into_iter();
    let mut broad_supplemental_scored_items = rescorer
        .score_all(&broad_supplemental_score_requests)?
        .into_iter();
    let context_ablated_requests = score_requests
        .iter()
        .filter(|request| !request.context.is_empty() || !request.right_context.is_empty())
        .map(|request| ScoreRequest {
            context: String::new(),
            right_context: String::new(),
            input_katakana: request.input_katakana.clone(),
            candidates: request.candidates.clone(),
        })
        .collect::<Vec<_>>();
    let mut context_ablated_items = rescorer.score_all(&context_ablated_requests)?.into_iter();

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
    let mut existing_generated = 0usize;
    let mut existing_generated_correct = 0usize;
    let mut existing_generated_improvements = 0usize;
    let mut existing_generated_regressions = 0usize;
    let mut near_tie_improvements = 0usize;
    let mut near_tie_regressions = 0usize;
    let mut consensus_correct = 0usize;
    let mut consensus_improvements = 0usize;
    let mut consensus_regressions = 0usize;
    let mut context_contrast_correct = 0usize;
    let mut context_contrast_improvements = 0usize;
    let mut context_contrast_regressions = 0usize;
    let mut multi_consensus_correct = 0usize;
    let mut multi_consensus_improvements = 0usize;
    let mut multi_consensus_regressions = 0usize;
    let mut whole_generation_correct = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut whole_generation_improvements = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut whole_generation_regressions = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut same_length_whole_correct = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut same_length_whole_improvements = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut same_length_whole_regressions = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut preserving_whole_correct = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut preserving_whole_improvements = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut preserving_whole_regressions = [0usize; WHOLE_GENERATION_COST_GAPS.len()];
    let mut supplemental_whole_correct = [0usize; SUPPLEMENTAL_WHOLE_COST_GAPS.len()];
    let mut supplemental_whole_improvements = [0usize; SUPPLEMENTAL_WHOLE_COST_GAPS.len()];
    let mut supplemental_whole_regressions = [0usize; SUPPLEMENTAL_WHOLE_COST_GAPS.len()];
    let mut supplemental_model_correct =
        [[0usize; SUPPLEMENTAL_MODEL_MARGINS.len()]; SUPPLEMENTAL_WHOLE_COST_GAPS.len()];
    let mut supplemental_model_improvements =
        [[0usize; SUPPLEMENTAL_MODEL_MARGINS.len()]; SUPPLEMENTAL_WHOLE_COST_GAPS.len()];
    let mut supplemental_model_regressions =
        [[0usize; SUPPLEMENTAL_MODEL_MARGINS.len()]; SUPPLEMENTAL_WHOLE_COST_GAPS.len()];
    let mut model_top_margin_correct = [0usize; MODEL_TOP_MARGIN_THRESHOLDS.len()];
    let mut model_top_margin_improvements = [0usize; MODEL_TOP_MARGIN_THRESHOLDS.len()];
    let mut model_top_margin_regressions = [0usize; MODEL_TOP_MARGIN_THRESHOLDS.len()];
    let mut final_present_in_64 = 0usize;
    let mut final_missing_from_64 = 0usize;
    let mut final_whole_block_counts = [0usize; WHOLE_RESULT_BLOCKS.len()];
    let mut final_accepted_but_incorrect = 0usize;
    let mut whole_reading_correct = [0usize; WHOLE_RESULT_READING_MAXIMUMS.len()];
    let mut whole_reading_improvements = [0usize; WHOLE_RESULT_READING_MAXIMUMS.len()];
    let mut whole_reading_regressions = [0usize; WHOLE_RESULT_READING_MAXIMUMS.len()];
    let mut delayed_long_reading_correct = [0usize; DELAYED_LONG_READING_MAXIMUMS.len()];
    let mut delayed_long_reading_improvements = [0usize; DELAYED_LONG_READING_MAXIMUMS.len()];
    let mut delayed_long_reading_regressions = [0usize; DELAYED_LONG_READING_MAXIMUMS.len()];
    let mut long_whole_correct = [0usize; LONG_WHOLE_RESULT_COST_FLOORS.len()];
    let mut long_whole_improvements = [0usize; LONG_WHOLE_RESULT_COST_FLOORS.len()];
    let mut long_whole_regressions = [0usize; LONG_WHOLE_RESULT_COST_FLOORS.len()];
    let mut scored_long_correct = [0usize; SCORED_LONG_VARIANTS.len()];
    let mut scored_long_improvements = [0usize; SCORED_LONG_VARIANTS.len()];
    let mut scored_long_regressions = [0usize; SCORED_LONG_VARIANTS.len()];
    let mut reported_failures = 0usize;
    let mut latencies = Vec::with_capacity(generated.len());
    let mut eligible_latencies = Vec::new();
    let mut context_ablated_latencies = Vec::new();
    for (item_index, ((((item, generated), ordinary), augmented), &original_count)) in items
        .iter()
        .zip(&generated)
        .zip(&ordinary_sets)
        .zip(&augmented_sets)
        .zip(&original_counts)
        .enumerate()
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
            .find(|conversion| conversion.surface == generated.surface);
        let backed_matches = backed
            .as_ref()
            .is_some_and(|conversion| matches_expected(&conversion.surface, &item.expected_output));
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

        let mut existing_generation_stats = None;
        let mut contrast_generation_stats = None;
        let mut scored_candidate_logliks = Vec::new();
        let mut broad_supplemental_selected = None;
        let mut broad_supplemental_model_margin = None;
        let ordinary_generated_cost_gap = if item.input.chars().count() >= LONG_INPUT_CHARACTERS {
            LONG_MAX_CANDIDATE_COST_GAP
        } else {
            SHORT_MAX_CANDIDATE_COST_GAP
        };
        let extended_generated_candidate = augmented.get(original_count).is_some_and(|candidate| {
            candidate.surface == generated.surface
                && candidate.cost.saturating_sub(ordinary[0].cost) > ordinary_generated_cost_gap
        });
        let (
            current_surface,
            augmented_surface,
            current_prefix,
            augmented_prefix,
            contrast_surface,
            contrast_prefix,
        ) = if original_count >= 2 {
            let scored = scored_items
                .next()
                .expect("one score for each eligible candidate set");
            if broad_supplemental_added[item_index] {
                let broad_scored = broad_supplemental_scored_items
                    .next()
                    .expect("one broad score for each added supplemental candidate");
                let generated_index = broad_supplemental_sets[item_index].len() - 1;
                broad_supplemental_selected = Some(rescored_index(
                    item,
                    &broad_supplemental_sets[item_index],
                    &broad_scored.candidate_logliks,
                    true,
                ));
                broad_supplemental_model_margin = broad_scored
                    .candidate_logliks
                    .iter()
                    .enumerate()
                    .filter(|&(index, _)| index != generated_index)
                    .map(|(_, score)| *score)
                    .max_by(f64::total_cmp)
                    .map(|runner_up| broad_scored.candidate_logliks[generated_index] - runner_up);
            }
            scored_candidate_logliks.clone_from(&scored.candidate_logliks);
            let context_ablated =
                (!item.context_text.is_empty() || !item.right_context_text.is_empty()).then(|| {
                    context_ablated_items
                        .next()
                        .expect("one ablated score for each contextual request")
                });
            if let Some(context_ablated) = &context_ablated {
                context_ablated_latencies.push(context_ablated.latency);
            }
            let contrast_logliks = context_ablated.as_ref().map_or_else(
                || scored.candidate_logliks.clone(),
                |ablated| {
                    scored
                        .candidate_logliks
                        .iter()
                        .zip(&ablated.candidate_logliks)
                        .map(|(full, ablated)| full + CONTEXT_CONTRAST_WEIGHT * (full - ablated))
                        .collect::<Vec<_>>()
                },
            );
            let current_index = rescored_index(
                item,
                &augmented[..original_count],
                &scored.candidate_logliks[..original_count],
                false,
            );
            if let Some(generated_index) = augmented[..original_count]
                .iter()
                .position(|candidate| candidate.surface == generated.surface)
            {
                let has_right_context = !item.right_context_text.is_empty();
                let lambda = if !has_right_context
                    && item.input.chars().count() >= VERY_LONG_INPUT_CHARACTERS
                {
                    0.74
                } else {
                    0.8
                };
                let combined = |index: usize| {
                    (1.0 - lambda) * (-f64::from(augmented[index].cost) / COST_LOG_SCALE)
                        + lambda * scored.candidate_logliks[index]
                };
                existing_generation_stats = Some((
                    generated_index + 1,
                    current_index + 1,
                    scored.candidate_logliks[generated_index]
                        - scored.candidate_logliks[current_index],
                    combined(generated_index) - combined(current_index),
                    augmented[generated_index]
                        .cost
                        .saturating_sub(augmented[0].cost),
                ));
            }
            let current_surface = augmented[current_index].surface.clone();
            let current_prefix = apply_prefix_correction(
                &dictionary,
                &reading,
                &current_surface,
                scored.first_mismatch_prefixes[current_index].as_ref(),
            )
            .unwrap_or_else(|| current_surface.clone());
            let augmented_index = if extended_generated_candidate {
                original_count
            } else {
                rescored_index(
                    item,
                    augmented,
                    &scored.candidate_logliks,
                    augmented.len() > original_count,
                )
            };
            let augmented_surface = augmented[augmented_index].surface.clone();
            let augmented_prefix = apply_prefix_correction(
                &dictionary,
                &reading,
                &augmented_surface,
                scored.first_mismatch_prefixes[augmented_index].as_ref(),
            )
            .unwrap_or_else(|| augmented_surface.clone());
            let contrast_index = if extended_generated_candidate {
                original_count
            } else {
                rescored_index(
                    item,
                    augmented,
                    &contrast_logliks,
                    augmented.len() > original_count,
                )
            };
            if let Some(generated_index) = augmented[..original_count]
                .iter()
                .position(|candidate| candidate.surface == generated.surface)
            {
                contrast_generation_stats = Some((
                    generated_index,
                    contrast_logliks[generated_index] - contrast_logliks[contrast_index],
                ));
            }
            let contrast_surface = augmented[contrast_index].surface.clone();
            let contrast_prefix = apply_prefix_correction(
                &dictionary,
                &reading,
                &contrast_surface,
                scored.first_mismatch_prefixes[contrast_index].as_ref(),
            )
            .unwrap_or_else(|| contrast_surface.clone());
            (
                current_surface,
                augmented_surface,
                current_prefix,
                augmented_prefix,
                contrast_surface,
                contrast_prefix,
            )
        } else {
            (
                base.clone(),
                base.clone(),
                base.clone(),
                base.clone(),
                base.clone(),
                base.clone(),
            )
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
        let contrast_iterative = apply_followup_prefix(
            &rescorer,
            &dictionary,
            item,
            &reading,
            &contrast_surface,
            &contrast_prefix,
        )?
        .unwrap_or_else(|| contrast_prefix.clone());
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
        let generated_existing = (generated.stopped_at_eos
            && (MINIMUM_GENERATIVE_READING_CHARACTERS..=MAXIMUM_GENERATIVE_READING_CHARACTERS)
                .contains(&item.input.chars().count())
            && augmented.first().is_some_and(|base| {
                bounded_local_substitution(
                    &base.surface,
                    &generated.surface,
                    MAX_CHANGED_CHARACTERS,
                )
            }))
        .then(|| {
            augmented[..original_count]
                .iter()
                .find(|candidate| candidate.surface == generated.surface)
                .map(|candidate| candidate.surface.as_str())
        })
        .flatten();
        let multi_region_generated_existing = (generated.stopped_at_eos
            && (MINIMUM_GENERATIVE_READING_CHARACTERS..=MAXIMUM_GENERATIVE_READING_CHARACTERS)
                .contains(&item.input.chars().count())
            && augmented.first().is_some_and(|base| {
                bounded_multi_region_substitution(&base.surface, &generated.surface)
            }))
        .then(|| {
            augmented[..original_count]
                .iter()
                .find(|candidate| candidate.surface == generated.surface)
                .map(|candidate| candidate.surface.as_str())
        })
        .flatten();
        let mut consensus_matches = augmented_iterative_matches;
        let mut contrast_consensus_surface = contrast_iterative.clone();
        if let Some(generated_existing) = generated_existing {
            let generated_matches = matches_expected(generated_existing, &item.expected_output);
            existing_generated += 1;
            existing_generated_correct += usize::from(generated_matches);
            existing_generated_improvements +=
                usize::from(!current_iterative_matches && generated_matches);
            existing_generated_regressions +=
                usize::from(current_iterative_matches && !generated_matches);
            if existing_generation_stats.is_some_and(|(_, _, model_delta, _, _)| {
                (GENERATIVE_CONSENSUS_MIN_MODEL_ADVANTAGE
                    ..=GENERATIVE_CONSENSUS_MAX_MODEL_ADVANTAGE)
                    .contains(&model_delta)
            }) {
                near_tie_improvements +=
                    usize::from(!current_iterative_matches && generated_matches);
                near_tie_regressions +=
                    usize::from(current_iterative_matches && !generated_matches);
                consensus_matches = generated_matches;
            }
            if contrast_generation_stats.is_some_and(|(_, model_delta)| {
                (GENERATIVE_CONSENSUS_MIN_MODEL_ADVANTAGE
                    ..=GENERATIVE_CONSENSUS_MAX_MODEL_ADVANTAGE)
                    .contains(&model_delta)
            }) {
                generated_existing.clone_into(&mut contrast_consensus_surface);
            }
            if current_iterative_matches != generated_matches {
                let stats = existing_generation_stats.map_or_else(
                    || {
                        "rank=-\tselected=-\tmodel_delta=-\tcombined_delta=-\tcost_gap=-".to_owned()
                    },
                    |(rank, selected, model_delta, combined_delta, cost_gap)| {
                        format!(
                            "rank={rank}\tselected={selected}\tmodel_delta={model_delta:.4}\tcombined_delta={combined_delta:.4}\tcost_gap={cost_gap}"
                        )
                    },
                );
                println!(
                    "existing_generation_change\t{}\t{}\t{}\tcurrent={}\tgenerated={}\texpected={}",
                    item.index,
                    if generated_matches {
                        "improve"
                    } else {
                        "regress"
                    },
                    stats,
                    current_iterative,
                    generated_existing,
                    item.expected_output.join(" | "),
                );
            }
        }
        consensus_correct += usize::from(consensus_matches);
        consensus_improvements += usize::from(!augmented_iterative_matches && consensus_matches);
        consensus_regressions += usize::from(augmented_iterative_matches && !consensus_matches);
        let contrast_consensus_matches =
            matches_expected(&contrast_consensus_surface, &item.expected_output);
        context_contrast_correct += usize::from(contrast_consensus_matches);
        context_contrast_improvements +=
            usize::from(!consensus_matches && contrast_consensus_matches);
        context_contrast_regressions +=
            usize::from(consensus_matches && !contrast_consensus_matches);
        let mut multi_consensus_surface = contrast_consensus_surface.as_str();
        if let Some(generated_existing) = multi_region_generated_existing
            && contrast_generation_stats.is_some_and(|(_, model_delta)| {
                (GENERATIVE_CONSENSUS_MIN_MODEL_ADVANTAGE
                    ..=MULTI_REGION_CONSENSUS_MAX_MODEL_ADVANTAGE)
                    .contains(&model_delta)
            })
        {
            multi_consensus_surface = generated_existing;
        }
        let multi_matches = matches_expected(multi_consensus_surface, &item.expected_output);
        multi_consensus_correct += usize::from(multi_matches);
        multi_consensus_improvements += usize::from(!contrast_consensus_matches && multi_matches);
        multi_consensus_regressions += usize::from(contrast_consensus_matches && !multi_matches);
        let reading_characters = item.input.chars().count();
        let eligible_scored_candidate = ((MAXIMUM_GENERATIVE_READING_CHARACTERS + 1)..=40)
            .contains(&reading_characters)
            .then(|| {
                ordinary
                    .iter()
                    .take(original_count)
                    .enumerate()
                    .skip(1)
                    .filter(|(_, candidate)| {
                        (LONG_WHOLE_RESULT_MINIMUM_COST_GAP..=WHOLE_RESULT_COST_GAP)
                            .contains(&candidate.cost.saturating_sub(ordinary[0].cost).max(0))
                    })
                    .max_by(|(left, _), (right, _)| {
                        scored_candidate_logliks[*left].total_cmp(&scored_candidate_logliks[*right])
                    })
                    .map(|(index, _)| index)
            })
            .flatten();
        let global_model_top = scored_candidate_logliks
            .iter()
            .take(original_count)
            .enumerate()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(index, _)| index);
        let scored_variants = [
            eligible_scored_candidate,
            eligible_scored_candidate
                .filter(|&index| scored_candidate_logliks[index] > scored_candidate_logliks[0]),
            eligible_scored_candidate.filter(|&index| Some(index) == global_model_top),
        ];
        for (index, candidate) in scored_variants.into_iter().enumerate() {
            let surface = candidate
                .map(|candidate| ordinary[candidate].surface.as_str())
                .filter(|candidate| {
                    preserves_ascii_alphanumerics(multi_consensus_surface, candidate)
                        && preserves_kanji_from_hiragana_deconversion(
                            multi_consensus_surface,
                            candidate,
                        )
                        && !dictionary.changes_exact_personal_name_segment(
                            &reading,
                            multi_consensus_surface,
                            candidate,
                        )
                })
                .unwrap_or(multi_consensus_surface);
            let matches = matches_expected(surface, &item.expected_output);
            scored_long_correct[index] += usize::from(matches);
            scored_long_improvements[index] += usize::from(!multi_matches && matches);
            scored_long_regressions[index] += usize::from(multi_matches && !matches);
        }
        let generated_cost_gap = backed
            .as_ref()
            .map(|conversion| conversion.cost.saturating_sub(ordinary[0].cost).max(0));
        let in_reading_window = (MINIMUM_GENERATIVE_READING_CHARACTERS
            ..=WHOLE_RESULT_MAXIMUM_READING_CHARACTERS)
            .contains(&reading_characters);
        let meets_long_cost_floor = reading_characters <= MAXIMUM_GENERATIVE_READING_CHARACTERS
            || generated_cost_gap
                .is_some_and(|cost_gap| cost_gap >= LONG_WHOLE_RESULT_MINIMUM_COST_GAP);
        let passes_long_pre_gate = reading_characters <= MAXIMUM_GENERATIVE_READING_CHARACTERS
            || eligible_scored_candidate
                .is_some_and(|candidate| Some(candidate) == global_model_top);
        let preserves_personal_name = !dictionary.changes_exact_personal_name_segment(
            &reading,
            multi_consensus_surface,
            &generated.surface,
        );
        let preserves_ascii =
            preserves_ascii_alphanumerics(multi_consensus_surface, &generated.surface);
        let in_whole_window = generated.stopped_at_eos
            && in_reading_window
            && passes_long_pre_gate
            && meets_long_cost_floor
            && preserves_personal_name
            && preserves_ascii;
        let same_length =
            multi_consensus_surface.chars().count() == generated.surface.chars().count();
        let preserves_kanji =
            preserves_kanji_from_hiragana_deconversion(multi_consensus_surface, &generated.surface);
        let whole_result_accepts = in_whole_window
            && preserves_kanji
            && generated_cost_gap.is_some_and(|cost_gap| cost_gap <= WHOLE_RESULT_COST_GAP);
        let whole_result_surface = if whole_result_accepts {
            generated.surface.as_str()
        } else {
            multi_consensus_surface
        };
        let whole_result_matches = matches_expected(whole_result_surface, &item.expected_output);
        let model_top_with_margin = if scored_candidate_logliks.len() >= 2 {
            let mut model_order = (0..original_count).collect::<Vec<_>>();
            model_order.sort_by(|&left, &right| {
                scored_candidate_logliks[right].total_cmp(&scored_candidate_logliks[left])
            });
            Some((
                model_order[0],
                scored_candidate_logliks[model_order[0]] - scored_candidate_logliks[model_order[1]],
            ))
        } else {
            None
        };
        for (index, minimum_margin) in MODEL_TOP_MARGIN_THRESHOLDS.iter().enumerate() {
            let alternative = model_top_with_margin
                .filter(|&(candidate, margin)| candidate != 0 && margin >= *minimum_margin)
                .map(|(candidate, _)| ordinary[candidate].surface.as_str())
                .filter(|candidate| {
                    preserves_ascii_alphanumerics(whole_result_surface, candidate)
                        && preserves_kanji_from_hiragana_deconversion(
                            whole_result_surface,
                            candidate,
                        )
                        && !dictionary.changes_exact_personal_name_segment(
                            &reading,
                            whole_result_surface,
                            candidate,
                        )
                });
            let matches = alternative.map_or(whole_result_matches, |candidate| {
                matches_expected(candidate, &item.expected_output)
            });
            model_top_margin_correct[index] += usize::from(matches);
            model_top_margin_improvements[index] += usize::from(!whole_result_matches && matches);
            model_top_margin_regressions[index] += usize::from(whole_result_matches && !matches);
        }
        for (index, maximum_cost_gap) in SUPPLEMENTAL_WHOLE_COST_GAPS.iter().enumerate() {
            let broad_candidates = &broad_supplemental_sets[item_index];
            let generated_index = broad_supplemental_added[item_index]
                .then(|| broad_candidates.len().checked_sub(1))
                .flatten();
            let accepts = generated_index.is_some_and(|generated_index| {
                Some(generated_index) == broad_supplemental_selected
                    && broad_candidates[generated_index]
                        .cost
                        .saturating_sub(ordinary[0].cost)
                        .max(0)
                        <= *maximum_cost_gap
            });
            let matches = if accepts {
                raw_matches
            } else {
                whole_result_matches
            };
            supplemental_whole_correct[index] += usize::from(matches);
            supplemental_whole_improvements[index] += usize::from(!whole_result_matches && matches);
            supplemental_whole_regressions[index] += usize::from(whole_result_matches && !matches);
            for (margin_index, minimum_margin) in SUPPLEMENTAL_MODEL_MARGINS.iter().enumerate() {
                let accepts_model_consensus = generated_index.is_some_and(|generated_index| {
                    broad_candidates[generated_index]
                        .cost
                        .saturating_sub(ordinary[0].cost)
                        .max(0)
                        <= *maximum_cost_gap
                        && broad_supplemental_model_margin
                            .is_some_and(|margin| margin >= *minimum_margin)
                });
                let model_matches = if accepts_model_consensus {
                    raw_matches
                } else {
                    whole_result_matches
                };
                supplemental_model_correct[index][margin_index] += usize::from(model_matches);
                supplemental_model_improvements[index][margin_index] +=
                    usize::from(!whole_result_matches && model_matches);
                supplemental_model_regressions[index][margin_index] +=
                    usize::from(whole_result_matches && !model_matches);
                if *maximum_cost_gap == 3_100
                    && (*minimum_margin - 1.5).abs() < f64::EPSILON
                    && accepts_model_consensus
                    && model_matches != whole_result_matches
                {
                    println!(
                        "supplemental_model_change\t{}\t{}\tcost_gap={}\tmodel_margin={:.4}\tcurrent={}\tgenerated={}\texpected={}",
                        item.index,
                        if model_matches { "improve" } else { "regress" },
                        generated_index
                            .map(|generated_index| broad_candidates[generated_index]
                                .cost
                                .saturating_sub(ordinary[0].cost)
                                .max(0))
                            .unwrap_or_default(),
                        broad_supplemental_model_margin.unwrap_or_default(),
                        whole_result_surface,
                        generated.surface,
                        item.expected_output.join(" | "),
                    );
                }
            }
        }
        let generated_existing_index = ordinary
            .iter()
            .take(original_count)
            .position(|candidate| candidate.surface == generated.surface);
        let strict_delayed_long_support = generated_existing_index.is_some_and(|candidate| {
            candidate != 0
                && Some(candidate) == global_model_top
                && (LONG_WHOLE_RESULT_MINIMUM_COST_GAP..=WHOLE_RESULT_COST_GAP).contains(
                    &ordinary[candidate]
                        .cost
                        .saturating_sub(ordinary[0].cost)
                        .max(0),
                )
        });
        for (index, maximum_reading) in DELAYED_LONG_READING_MAXIMUMS.iter().enumerate() {
            let accepts_extension = reading_characters > WHOLE_RESULT_MAXIMUM_READING_CHARACTERS
                && reading_characters <= *maximum_reading
                && generated.stopped_at_eos
                && strict_delayed_long_support
                && preserves_personal_name
                && preserves_ascii
                && preserves_kanji;
            let matches = if accepts_extension {
                raw_matches
            } else {
                whole_result_matches
            };
            delayed_long_reading_correct[index] += usize::from(matches);
            delayed_long_reading_improvements[index] +=
                usize::from(!whole_result_matches && matches);
            delayed_long_reading_regressions[index] +=
                usize::from(whole_result_matches && !matches);
        }
        for (index, maximum_reading) in WHOLE_RESULT_READING_MAXIMUMS.iter().enumerate() {
            let accepts = generated.stopped_at_eos
                && item.input.chars().count() >= MINIMUM_GENERATIVE_READING_CHARACTERS
                && item.input.chars().count() <= *maximum_reading
                && preserves_personal_name
                && preserves_ascii
                && preserves_kanji
                && generated_cost_gap.is_some_and(|cost_gap| cost_gap <= WHOLE_RESULT_COST_GAP);
            let matches = if accepts { raw_matches } else { multi_matches };
            whole_reading_correct[index] += usize::from(matches);
            whole_reading_improvements[index] += usize::from(!multi_matches && matches);
            whole_reading_regressions[index] += usize::from(multi_matches && !matches);
            if *maximum_reading == 40 && matches != multi_matches {
                let outcome = if matches { "improvement" } else { "regression" };
                println!(
                    "whole_result_reading_change\t{}\toutcome={}\treading_chars={}\tcost_gap={}\trank={}\trescored={}\tbase={}\tgenerated={}\texpected={}",
                    item.index,
                    outcome,
                    item.input.chars().count(),
                    generated_cost_gap.unwrap_or_default(),
                    ordinary
                        .iter()
                        .position(|candidate| candidate.surface == generated.surface)
                        .map_or_else(|| "missing".to_owned(), |rank| (rank + 1).to_string()),
                    original_count,
                    multi_consensus_surface,
                    generated.surface,
                    item.expected_output.join(" | "),
                );
            }
        }
        for (index, cost_floor) in LONG_WHOLE_RESULT_COST_FLOORS.iter().enumerate() {
            let accepts = generated.stopped_at_eos
                && (MINIMUM_GENERATIVE_READING_CHARACTERS..=40).contains(&reading_characters)
                && (reading_characters <= MAXIMUM_GENERATIVE_READING_CHARACTERS
                    || generated_cost_gap.is_some_and(|cost_gap| cost_gap >= *cost_floor))
                && preserves_personal_name
                && preserves_ascii
                && preserves_kanji
                && generated_cost_gap.is_some_and(|cost_gap| cost_gap <= WHOLE_RESULT_COST_GAP);
            let matches = if accepts { raw_matches } else { multi_matches };
            long_whole_correct[index] += usize::from(matches);
            long_whole_improvements[index] += usize::from(!multi_matches && matches);
            long_whole_regressions[index] += usize::from(multi_matches && !matches);
        }
        for (index, maximum_cost_gap) in WHOLE_GENERATION_COST_GAPS.iter().enumerate() {
            let accepts = in_whole_window
                && generated_cost_gap.is_some_and(|cost_gap| cost_gap <= *maximum_cost_gap);
            let whole_matches = if accepts { raw_matches } else { multi_matches };
            whole_generation_correct[index] += usize::from(whole_matches);
            whole_generation_improvements[index] += usize::from(!multi_matches && whole_matches);
            whole_generation_regressions[index] += usize::from(multi_matches && !whole_matches);

            let same_length_matches = if accepts && same_length {
                raw_matches
            } else {
                multi_matches
            };
            same_length_whole_correct[index] += usize::from(same_length_matches);
            same_length_whole_improvements[index] +=
                usize::from(!multi_matches && same_length_matches);
            same_length_whole_regressions[index] +=
                usize::from(multi_matches && !same_length_matches);

            let preserving_matches = if accepts && preserves_kanji {
                raw_matches
            } else {
                multi_matches
            };
            preserving_whole_correct[index] += usize::from(preserving_matches);
            preserving_whole_improvements[index] +=
                usize::from(!multi_matches && preserving_matches);
            preserving_whole_regressions[index] +=
                usize::from(multi_matches && !preserving_matches);
        }
        if raw_matches != multi_matches {
            let generation_diagnostics = existing_generation_stats.map_or_else(
                || "rank=missing\tselected=-\tmodel_delta=-\tcombined_delta=-".to_owned(),
                |(rank, selected, model_delta, combined_delta, _)| {
                    format!(
                        "rank={rank}\tselected={selected}\tmodel_delta={model_delta:.4}\tcombined_delta={combined_delta:.4}"
                    )
                },
            );
            println!(
                "whole_generation_change\t{}\t{}\treading_chars={}\tcost_gap={}\tsame_length={}\tpreserves_kanji={}\t{}\tcurrent={}\tgenerated={}\texpected={}",
                item.index,
                if raw_matches { "improve" } else { "regress" },
                reading_characters,
                generated_cost_gap.map_or_else(|| "missing".to_owned(), |gap| gap.to_string()),
                same_length,
                preserves_kanji,
                generation_diagnostics,
                multi_consensus_surface,
                generated.surface,
                item.expected_output.join(" | "),
            );
        }
        if consensus_matches != contrast_consensus_matches {
            println!(
                "context_contrast_change\t{}\t{}\tcurrent={}\tcontrast={}\texpected={}",
                item.index,
                if contrast_consensus_matches {
                    "improve"
                } else {
                    "regress"
                },
                if consensus_matches {
                    item.expected_output.join(" | ")
                } else {
                    augmented_iterative.clone()
                },
                contrast_consensus_surface,
                item.expected_output.join(" | "),
            );
        }
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
        if !whole_result_matches && reported_failures < failure_limit {
            let wide_candidates = dictionary.candidates_with_surrounding_context_limit(
                &reading,
                &item.context_text,
                &item.right_context_text,
                64,
            );
            let expected_rank = wide_candidates
                .iter()
                .position(|candidate| matches_expected(&candidate.surface, &item.expected_output));
            if expected_rank.is_some() {
                final_present_in_64 += 1;
            } else {
                final_missing_from_64 += 1;
            }
            let whole_result_block = if !generated.stopped_at_eos {
                "incomplete"
            } else if !in_reading_window {
                "reading_window"
            } else if !passes_long_pre_gate {
                "long_pre_gate"
            } else if backed.is_none() {
                "not_lattice_backed"
            } else if generated_cost_gap.is_some_and(|cost_gap| cost_gap > WHOLE_RESULT_COST_GAP) {
                "cost_gap"
            } else if !meets_long_cost_floor {
                "long_cost_floor"
            } else if !preserves_ascii {
                "ascii"
            } else if !preserves_kanji {
                "kanji_deconversion"
            } else if !preserves_personal_name {
                "personal_name"
            } else {
                "accepted_but_incorrect"
            };
            if let Some(block) = WHOLE_RESULT_BLOCKS
                .iter()
                .position(|&block| block == whole_result_block)
            {
                final_whole_block_counts[block] += 1;
            } else {
                final_accepted_but_incorrect += 1;
            }
            println!(
                "final_failure\t{}\trank64={}\twhole_block={}\tbase={}\tfinal={}\tgenerated={}\texpected={}",
                item.index,
                expected_rank.map_or_else(|| "missing".to_owned(), |rank| (rank + 1).to_string()),
                whole_result_block,
                ordinary[0].surface,
                whole_result_surface,
                generated.surface,
                item.expected_output.join(" | "),
            );
            reported_failures += 1;
        }
    }
    debug_assert!(scored_items.next().is_none());
    debug_assert!(context_ablated_items.next().is_none());

    latencies.sort_unstable();
    eligible_latencies.sort_unstable();
    context_ablated_latencies.sort_unstable();
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
    println!("existing_generated={existing_generated}");
    println!("existing_generated_correct={existing_generated_correct}");
    println!("existing_generated_improvements={existing_generated_improvements}");
    println!("existing_generated_regressions={existing_generated_regressions}");
    println!("near_tie_improvements={near_tie_improvements}");
    println!("near_tie_regressions={near_tie_regressions}");
    println!("consensus_top1={consensus_correct}");
    println!("consensus_improvements={consensus_improvements}");
    println!("consensus_regressions={consensus_regressions}");
    println!("context_contrast_consensus_top1={context_contrast_correct}");
    println!("context_contrast_improvements={context_contrast_improvements}");
    println!("context_contrast_regressions={context_contrast_regressions}");
    println!("multi_region_consensus_top1={multi_consensus_correct}");
    println!("multi_region_consensus_improvements={multi_consensus_improvements}");
    println!("multi_region_consensus_regressions={multi_consensus_regressions}");
    for (index, maximum_cost_gap) in WHOLE_GENERATION_COST_GAPS.iter().enumerate() {
        println!(
            "whole_generation_cost_gap={maximum_cost_gap}\ttop1={}\timprovements={}\tregressions={}",
            whole_generation_correct[index],
            whole_generation_improvements[index],
            whole_generation_regressions[index],
        );
        println!(
            "same_length_whole_cost_gap={maximum_cost_gap}\ttop1={}\timprovements={}\tregressions={}",
            same_length_whole_correct[index],
            same_length_whole_improvements[index],
            same_length_whole_regressions[index],
        );
        println!(
            "preserving_whole_cost_gap={maximum_cost_gap}\ttop1={}\timprovements={}\tregressions={}",
            preserving_whole_correct[index],
            preserving_whole_improvements[index],
            preserving_whole_regressions[index],
        );
    }
    for (index, maximum_cost_gap) in SUPPLEMENTAL_WHOLE_COST_GAPS.iter().enumerate() {
        println!(
            "supplemental_whole_cost_gap={maximum_cost_gap}\ttop1={}\timprovements={}\tregressions={}",
            supplemental_whole_correct[index],
            supplemental_whole_improvements[index],
            supplemental_whole_regressions[index],
        );
        for (margin_index, minimum_margin) in SUPPLEMENTAL_MODEL_MARGINS.iter().enumerate() {
            println!(
                "supplemental_model_cost_gap={maximum_cost_gap}\tmodel_margin={minimum_margin:.2}\ttop1={}\timprovements={}\tregressions={}",
                supplemental_model_correct[index][margin_index],
                supplemental_model_improvements[index][margin_index],
                supplemental_model_regressions[index][margin_index],
            );
        }
    }
    for (index, minimum_margin) in MODEL_TOP_MARGIN_THRESHOLDS.iter().enumerate() {
        println!(
            "model_top_margin={minimum_margin:.2}\ttop1={}\timprovements={}\tregressions={}",
            model_top_margin_correct[index],
            model_top_margin_improvements[index],
            model_top_margin_regressions[index],
        );
    }
    let whole_result_index = WHOLE_GENERATION_COST_GAPS
        .iter()
        .position(|&cost_gap| cost_gap == WHOLE_RESULT_COST_GAP)
        .expect("selected whole-result cost gap must be part of the diagnostic sweep");
    println!(
        "whole_result_consensus_top1={}",
        preserving_whole_correct[whole_result_index]
    );
    println!(
        "whole_result_consensus_improvements={}",
        preserving_whole_improvements[whole_result_index]
    );
    println!(
        "whole_result_consensus_regressions={}",
        preserving_whole_regressions[whole_result_index]
    );
    println!("final_present_in_64={final_present_in_64}");
    println!("final_missing_from_64={final_missing_from_64}");
    for (block, count) in WHOLE_RESULT_BLOCKS.iter().zip(final_whole_block_counts) {
        println!("final_whole_block_{block}={count}");
    }
    println!("final_accepted_but_incorrect={final_accepted_but_incorrect}");
    for (index, maximum_reading) in WHOLE_RESULT_READING_MAXIMUMS.iter().enumerate() {
        println!(
            "whole_result_reading_max={maximum_reading}\ttop1={}\timprovements={}\tregressions={}",
            whole_reading_correct[index],
            whole_reading_improvements[index],
            whole_reading_regressions[index],
        );
    }
    for (index, maximum_reading) in DELAYED_LONG_READING_MAXIMUMS.iter().enumerate() {
        println!(
            "delayed_long_reading_max={maximum_reading}\ttop1={}\timprovements={}\tregressions={}",
            delayed_long_reading_correct[index],
            delayed_long_reading_improvements[index],
            delayed_long_reading_regressions[index],
        );
    }
    for (index, cost_floor) in LONG_WHOLE_RESULT_COST_FLOORS.iter().enumerate() {
        println!(
            "long_whole_result_cost_floor={cost_floor}\ttop1={}\timprovements={}\tregressions={}",
            long_whole_correct[index],
            long_whole_improvements[index],
            long_whole_regressions[index],
        );
    }
    for (index, variant) in SCORED_LONG_VARIANTS.iter().enumerate() {
        println!(
            "scored_long_variant={variant}\ttop1={}\timprovements={}\tregressions={}",
            scored_long_correct[index],
            scored_long_improvements[index],
            scored_long_regressions[index],
        );
    }
    println!(
        "context_ablated_p50_ms={:.3}",
        percentile_ms(&context_ablated_latencies, 50)
    );
    println!(
        "context_ablated_p95_ms={:.3}",
        percentile_ms(&context_ablated_latencies, 95)
    );
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
    let is_safe = |alternative: &String| {
        bounded_local_substitution(current, alternative, MAX_CHANGED_CHARACTERS)
            && preserves_kanji_from_hiragana_deconversion(current, alternative)
            && !dictionary.changes_exact_personal_name_segment(reading, current, alternative)
    };
    let initial = dictionary.convert_n_best_with_surface_prefix(
        reading,
        &diagnostic.prefix,
        CONSTRAINED_CANDIDATES,
    );
    if let Some(alternative) = initial
        .into_iter()
        .map(|conversion| conversion.surface)
        .find(is_safe)
    {
        return Some(alternative);
    }
    dictionary
        .convert_n_best_with_surface_prefix(
            reading,
            &diagnostic.prefix,
            PREFIX_CONSTRAINED_CANDIDATES,
        )
        .into_iter()
        .map(|conversion| conversion.surface)
        .find(is_safe)
}

fn preserves_kanji_from_hiragana_deconversion(current: &str, alternative: &str) -> bool {
    current
        .chars()
        .zip(alternative.chars())
        .all(|(current, alternative)| {
            current == alternative || !is_kanji(current) || !is_hiragana(alternative)
        })
}

fn preserves_ascii_alphanumerics(current: &str, alternative: &str) -> bool {
    current
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .eq(alternative.chars().filter(char::is_ascii_alphanumeric))
}

fn is_kanji(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}' | '\u{F900}'..='\u{FAFF}'
    )
}

fn is_hiragana(character: char) -> bool {
    matches!(character, '\u{3041}'..='\u{3096}' | '\u{309D}'..='\u{309F}')
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

#[allow(clippy::too_many_lines)]
fn bounded_multi_region_surface_compression(current: &str, alternative: &str) -> bool {
    const MINIMUM_REGIONS: usize = 2;
    const MAXIMUM_REGIONS: usize = 4;
    const MAXIMUM_REGION_CHARACTERS_PER_SIDE: usize = 4;
    const MAXIMUM_LENGTH_DIFFERENCE: usize = 2;

    #[derive(Clone, Copy)]
    enum Edit {
        Match,
        Substitute(char, char),
        Delete(char),
        Insert(char),
    }

    let current = current.chars().collect::<Vec<_>>();
    let alternative = alternative.chars().collect::<Vec<_>>();
    if current.len() <= alternative.len()
        || current.len() - alternative.len() > MAXIMUM_LENGTH_DIFFERENCE
    {
        return false;
    }

    let width = alternative.len() + 1;
    let mut costs = vec![0usize; (current.len() + 1) * width];
    for row in 0..=current.len() {
        costs[row * width] = row;
    }
    for (column, cost) in costs.iter_mut().take(width).enumerate() {
        *cost = column;
    }
    for row in 1..=current.len() {
        for column in 1..=alternative.len() {
            let substitution = costs[(row - 1) * width + column - 1]
                + usize::from(current[row - 1] != alternative[column - 1]);
            let deletion = costs[(row - 1) * width + column] + 1;
            let insertion = costs[row * width + column - 1] + 1;
            costs[row * width + column] = substitution.min(deletion).min(insertion);
        }
    }

    let mut edits = Vec::with_capacity(current.len().max(alternative.len()));
    let (mut row, mut column) = (current.len(), alternative.len());
    while row > 0 || column > 0 {
        let cost = costs[row * width + column];
        if row > 0
            && column > 0
            && current[row - 1] == alternative[column - 1]
            && cost == costs[(row - 1) * width + column - 1]
        {
            edits.push(Edit::Match);
            row -= 1;
            column -= 1;
        } else if row > 0 && column > 0 && cost == costs[(row - 1) * width + column - 1] + 1 {
            edits.push(Edit::Substitute(current[row - 1], alternative[column - 1]));
            row -= 1;
            column -= 1;
        } else if row > 0 && cost == costs[(row - 1) * width + column] + 1 {
            edits.push(Edit::Delete(current[row - 1]));
            row -= 1;
        } else if column > 0 && cost == costs[row * width + column - 1] + 1 {
            edits.push(Edit::Insert(alternative[column - 1]));
            column -= 1;
        } else {
            return false;
        }
    }
    edits.reverse();

    let mut regions = 0usize;
    let mut inside_region = false;
    let mut current_characters = 0usize;
    let mut alternative_characters = 0usize;
    for edit in edits {
        if matches!(edit, Edit::Match) {
            inside_region = false;
            current_characters = 0;
            alternative_characters = 0;
            continue;
        }
        if !inside_region {
            regions += 1;
            if regions > MAXIMUM_REGIONS {
                return false;
            }
            inside_region = true;
        }
        match edit {
            Edit::Match => unreachable!(),
            Edit::Substitute(current, alternative) => {
                if current.is_ascii_alphanumeric() || alternative.is_ascii_alphanumeric() {
                    return false;
                }
                current_characters += 1;
                alternative_characters += 1;
            }
            Edit::Delete(current) => {
                if current.is_ascii_alphanumeric() {
                    return false;
                }
                current_characters += 1;
            }
            Edit::Insert(alternative) => {
                if alternative.is_ascii_alphanumeric() {
                    return false;
                }
                alternative_characters += 1;
            }
        }
        if current_characters > MAXIMUM_REGION_CHARACTERS_PER_SIDE
            || alternative_characters > MAXIMUM_REGION_CHARACTERS_PER_SIDE
        {
            return false;
        }
    }
    (MINIMUM_REGIONS..=MAXIMUM_REGIONS).contains(&regions)
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
