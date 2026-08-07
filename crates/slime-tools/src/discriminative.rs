//! Evaluation-only hashed discriminative N-best reranker.
//!
//! This is deliberately separate from the runtime converter. It must prove
//! held-out quality, size, and latency before any product integration.

use std::time::{Duration, Instant};

use slime_converter::Candidate;

const BASE_COST_SCALE: f32 = 500.0;

pub(crate) struct TrainingItem<'a> {
    pub(crate) context: &'a str,
    pub(crate) candidates: &'a [Candidate],
    pub(crate) expected: &'a [String],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Diagnostics {
    pub(crate) dimensions: usize,
    pub(crate) epochs: usize,
    pub(crate) training_items: usize,
    pub(crate) oracle_items: usize,
    pub(crate) updates: usize,
    pub(crate) nonzero_weights: usize,
    pub(crate) model_bytes: usize,
}

pub(crate) struct ScoredItem {
    pub(crate) scores: Vec<f32>,
    pub(crate) latency: Duration,
}

pub(crate) struct HashedPerceptron {
    weights: Vec<f32>,
    diagnostics: Diagnostics,
}

impl HashedPerceptron {
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn train(items: &[TrainingItem<'_>], dimensions: usize, epochs: usize) -> Self {
        assert!(dimensions.is_power_of_two());
        assert!(epochs > 0);
        let mut weights = vec![0.0_f32; dimensions];
        let mut totals = vec![0.0_f32; dimensions];
        let mut last_steps = vec![0_u32; dimensions];
        let mut step = 0_u32;
        let mut updates = 0_usize;
        let mut oracle_items = 0_usize;

        for _ in 0..epochs {
            for item in items {
                let Some(gold) = gold_index(item.candidates, item.expected) else {
                    continue;
                };
                oracle_items += 1;
                step = step.saturating_add(1);
                let predicted = best_index(&weights, item.context, item.candidates, 1.0);
                if predicted == gold {
                    continue;
                }
                updates += 1;
                update(
                    &mut weights,
                    &mut totals,
                    &mut last_steps,
                    step,
                    item.context,
                    &item.candidates[gold].surface,
                    1.0,
                );
                update(
                    &mut weights,
                    &mut totals,
                    &mut last_steps,
                    step,
                    item.context,
                    &item.candidates[predicted].surface,
                    -1.0,
                );
            }
        }

        if step > 0 {
            for (index, weight) in weights.iter_mut().enumerate() {
                totals[index] += (step - last_steps[index]) as f32 * *weight;
                *weight = totals[index] / step as f32;
            }
        }
        let nonzero_weights = weights.iter().filter(|weight| **weight != 0.0).count();
        Self {
            weights,
            diagnostics: Diagnostics {
                dimensions,
                epochs,
                training_items: items.len(),
                oracle_items: oracle_items / epochs,
                updates,
                nonzero_weights,
                model_bytes: dimensions * size_of::<f32>(),
            },
        }
    }

    pub(crate) fn diagnostics(&self) -> Diagnostics {
        self.diagnostics
    }

    pub(crate) fn score(&self, context: &str, candidates: &[Candidate]) -> ScoredItem {
        let started = Instant::now();
        let scores = candidates
            .iter()
            .map(|candidate| feature_score(&self.weights, context, &candidate.surface))
            .collect();
        ScoredItem {
            scores,
            latency: started.elapsed(),
        }
    }
}

pub(crate) fn rescored_surfaces(
    candidates: &[Candidate],
    scores: &[f32],
    weight: f32,
) -> Vec<String> {
    let mut indexed: Vec<usize> = (0..candidates.len()).collect();
    indexed.sort_by(|&left, &right| {
        combined_score(&candidates[right], scores[right], weight).total_cmp(&combined_score(
            &candidates[left],
            scores[left],
            weight,
        ))
    });
    indexed
        .into_iter()
        .map(|index| candidates[index].surface.clone())
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn combined_score(candidate: &Candidate, feature_score: f32, weight: f32) -> f32 {
    -candidate.cost as f32 / BASE_COST_SCALE + weight * feature_score
}

fn gold_index(candidates: &[Candidate], expected: &[String]) -> Option<usize> {
    candidates
        .iter()
        .position(|candidate| expected.contains(&candidate.surface))
}

fn best_index(
    weights: &[f32],
    context: &str,
    candidates: &[Candidate],
    model_weight: f32,
) -> usize {
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let score = combined_score(
                candidate,
                feature_score(weights, context, &candidate.surface),
                model_weight,
            );
            (index, score)
        })
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map_or(0, |(index, _)| index)
}

#[allow(clippy::cast_precision_loss)]
fn update(
    weights: &mut [f32],
    totals: &mut [f32],
    last_steps: &mut [u32],
    step: u32,
    context: &str,
    surface: &str,
    delta: f32,
) {
    for feature in features(context, surface, weights.len()) {
        totals[feature] += (step - last_steps[feature]) as f32 * weights[feature];
        last_steps[feature] = step;
        weights[feature] += delta;
    }
}

fn feature_score(weights: &[f32], context: &str, surface: &str) -> f32 {
    features(context, surface, weights.len())
        .into_iter()
        .map(|feature| weights[feature])
        .sum()
}

fn features(context: &str, surface: &str, dimensions: usize) -> Vec<usize> {
    let surface: Vec<char> = surface.chars().collect();
    let context: Vec<char> = context.chars().rev().take(3).collect();
    let mut result = Vec::with_capacity(surface.len().saturating_mul(8));

    for length in 1..=3 {
        for window in surface.windows(length) {
            result.push(feature_hash(length as u64, &[], window, dimensions));
        }
    }
    for context_length in 1..=context.len() {
        let suffix = &context[..context_length];
        for character in &surface {
            result.push(feature_hash(
                10 + context_length as u64,
                suffix,
                std::slice::from_ref(character),
                dimensions,
            ));
        }
        for window in surface.windows(2) {
            result.push(feature_hash(
                20 + context_length as u64,
                suffix,
                window,
                dimensions,
            ));
        }
    }
    result
}

fn feature_hash(tag: u64, left: &[char], right: &[char], dimensions: usize) -> usize {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64 ^ tag;
    for character in left.iter().chain(right) {
        hash ^= u64::from(u32::from(*character));
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= u64::try_from(left.len()).expect("feature length fits u64") << 48;
    let mask = u64::try_from(dimensions - 1).expect("feature mask fits u64");
    usize::try_from(hash & mask).expect("masked feature hash fits usize")
}

#[cfg(test)]
mod tests {
    use super::{HashedPerceptron, TrainingItem, rescored_surfaces};
    use slime_converter::Candidate;

    fn candidate(surface: &str, cost: i32) -> Candidate {
        Candidate {
            surface: surface.to_owned(),
            cost,
        }
    }

    #[test]
    fn learns_a_contextual_candidate_preference() {
        let candidates = vec![candidate("工程", 1_000), candidate("皇帝", 1_050)];
        let expected = vec!["皇帝".to_owned()];
        let items = vec![TrainingItem {
            context: "次期",
            candidates: &candidates,
            expected: &expected,
        }];
        let model = HashedPerceptron::train(&items, 1 << 12, 4);
        let scored = model.score("次期", &candidates);
        assert_eq!(
            rescored_surfaces(&candidates, &scored.scores, 1.0)[0],
            "皇帝"
        );
        assert!(model.diagnostics().nonzero_weights > 0);
    }
}
