//! Neural N-best rescoring with a zenz GGUF model (Phase 2 feasibility).
//!
//! Scores each existing candidate as `log P(candidate, EOS | context, reading)`
//! under a character-level conditional LM. Rescoring is prefill-only and
//! normally needs a single decode call per item: the shared `context +
//! reading` prefix is assigned to every sequence and each candidate continues
//! its own sequence in the same batch.
//!
//! Prompt format (zenz-v3): `\u{EE02}<context>\u{EE00}<katakana input>\u{EE01}<output></s>`.
//! The context block is omitted when the item has no left context.

use std::path::Path;
use std::time::{Duration, Instant};

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;

/// Maximum characters of left context fed to the model. Zenzai truncates the
/// context similarly; unbounded context would dominate prefill latency.
const MAX_CONTEXT_CHARACTERS: usize = 40;

/// Candidates scored in parallel as independent sequences in one decode call.
const MAX_PARALLEL_CANDIDATES: usize = 16;

/// Total KV cells: shared prefix + one suffix per parallel candidate.
const KV_CELLS: u32 = 4096;

/// zenz is trained with 1024 positions; skip items that would exceed it.
const MAX_POSITIONS: usize = 1024;

const CONTEXT_MARK: char = '\u{EE02}';
const INPUT_MARK: char = '\u{EE00}';
const OUTPUT_MARK: char = '\u{EE01}';

pub struct ScoreRequest {
    pub context: String,
    pub input_katakana: String,
    pub candidates: Vec<String>,
}

pub struct ScoredItem {
    /// `log P(candidate, EOS | prompt)` per candidate, aligned with the request.
    pub logliks: Vec<f64>,
    /// Wall-clock time spent scoring this item (prefix + all candidates).
    pub latency: Duration,
}

pub struct Rescorer {
    backend: LlamaBackend,
    model: LlamaModel,
}

impl Rescorer {
    pub fn load(model_path: &Path) -> Result<Self, String> {
        let backend = LlamaBackend::init()
            .map_err(|error| format!("failed to initialize llama backend: {error}"))?;
        let mut model_params = LlamaModelParams::default();
        if std::env::var_os("SLIME_NEURAL_CPU").is_some() {
            model_params = model_params.with_n_gpu_layers(0);
        }
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|error| format!("failed to load model {}: {error}", model_path.display()))?;
        Ok(Self { backend, model })
    }

    /// Scores every request. One llama context is created for the whole run.
    pub fn score_all(&self, requests: &[ScoreRequest]) -> Result<Vec<ScoredItem>, String> {
        let sequence_count =
            u32::try_from(MAX_PARALLEL_CANDIDATES).expect("parallel candidates fit u32");
        let context_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(KV_CELLS))
            .with_n_batch(KV_CELLS)
            .with_n_ubatch(KV_CELLS)
            .with_n_seq_max(sequence_count)
            .with_kv_unified(true);
        let mut context = self
            .model
            .new_context(&self.backend, context_params)
            .map_err(|error| format!("failed to create llama context: {error}"))?;
        let mut batch = LlamaBatch::new(
            usize::try_from(KV_CELLS).expect("kv cells fit usize"),
            i32::try_from(MAX_PARALLEL_CANDIDATES).expect("parallel candidates fit i32"),
        );
        let mut timing = Timing::default();
        let scored: Result<Vec<ScoredItem>, String> = requests
            .iter()
            .map(|request| self.score_item(&mut context, &mut batch, request, &mut timing))
            .collect();
        if std::env::var_os("SLIME_NEURAL_TIMING").is_some() {
            eprintln!(
                "neural timing: decode_submit={:?} sync_and_scoring={:?}",
                timing.candidate_decode, timing.scoring
            );
        }
        scored
    }

    #[allow(clippy::too_many_lines)]
    fn score_item(
        &self,
        context: &mut LlamaContext<'_>,
        batch: &mut LlamaBatch,
        request: &ScoreRequest,
        timing: &mut Timing,
    ) -> Result<ScoredItem, String> {
        let started = Instant::now();
        let prompt = build_prompt(&request.context, &request.input_katakana);
        let prefix_tokens = self
            .model
            .str_to_token(&prompt, AddBos::Never)
            .map_err(|error| format!("failed to tokenize prompt: {error}"))?;
        let candidate_tokens: Vec<Vec<LlamaToken>> = request
            .candidates
            .iter()
            .map(|candidate| {
                self.model
                    .str_to_token(candidate, AddBos::Never)
                    .map_err(|error| format!("failed to tokenize candidate: {error}"))
            })
            .collect::<Result<_, _>>()?;

        let longest_candidate = candidate_tokens.iter().map(Vec::len).max().unwrap_or(0);
        if prefix_tokens.is_empty() || prefix_tokens.len() + longest_candidate >= MAX_POSITIONS {
            // Too long to score: report neutral scores so the base order wins.
            return Ok(ScoredItem {
                logliks: vec![0.0; request.candidates.len()],
                latency: started.elapsed(),
            });
        }

        // The whole item is decoded in a single call when the candidates fit
        // into the parallel sequences: the prefix tokens are shared by every
        // sequence and each candidate continues its own sequence. Metal decode
        // has a large fixed launch overhead, so decode calls are minimized.
        let sequences: Vec<i32> = (0..MAX_PARALLEL_CANDIDATES)
            .map(|sequence| i32::try_from(sequence).expect("sequence id fits i32"))
            .collect();
        context.clear_kv_cache();
        batch.clear();
        let last_prefix_index = prefix_tokens.len() - 1;
        for (index, token) in prefix_tokens.iter().enumerate() {
            batch
                .add(
                    *token,
                    position(index),
                    &sequences,
                    index == last_prefix_index,
                )
                .map_err(|error| format!("failed to build prefix batch: {error}"))?;
        }

        let eos = self.model.token_eos();
        let mut logliks = Vec::with_capacity(candidate_tokens.len());
        let mut first_token_scores: Option<LogDistribution> = None;
        for (chunk_index, chunk) in candidate_tokens.chunks(MAX_PARALLEL_CANDIDATES).enumerate() {
            let merged_prefix = chunk_index == 0;
            if !merged_prefix {
                // Trim per-sequence suffixes left over from the previous chunk.
                let prefix_end = u32::try_from(prefix_tokens.len()).expect("prefix fits u32");
                context
                    .clear_kv_cache_seq(None, Some(prefix_end), None)
                    .map_err(|error| format!("failed to trim kv cache: {error}"))?;
                batch.clear();
            }

            // The prefix distribution occupies output row 0 of the merged
            // decode; candidate rows follow in insertion order.
            let mut next_row = usize::from(merged_prefix);
            let mut row_offsets = Vec::with_capacity(chunk.len());
            for (chunk_slot, tokens) in chunk.iter().enumerate() {
                row_offsets.push(next_row);
                let sequence = [sequences[chunk_slot]];
                for (index, token) in tokens.iter().enumerate() {
                    batch
                        .add(
                            *token,
                            position(prefix_tokens.len() + index),
                            &sequence,
                            true,
                        )
                        .map_err(|error| format!("failed to build candidate batch: {error}"))?;
                }
                next_row += tokens.len();
            }
            let decode_started = Instant::now();
            context
                .decode(batch)
                .map_err(|error| format!("failed to decode item: {error}"))?;
            timing.candidate_decode += decode_started.elapsed();

            let scoring_started = Instant::now();
            // `llama_get_logits_ith` synchronizes the backend on every call;
            // fetch the output buffer base once (one synchronization, which
            // also absorbs the asynchronous decode above) and index rows
            // directly. Output rows hold only logits-enabled tokens in
            // insertion order: the shared prefix contributes exactly row 0.
            let logits_base = context.get_logits();
            let vocabulary = usize::try_from(self.model.n_vocab()).expect("n_vocab fits usize");
            let logits_row = |row: usize| -> &[f32] {
                // SAFETY: the output buffer holds one `n_vocab` row per
                // logits-enabled batch token; `row` is below `next_row`, the
                // number of tokens decoded with logits in this batch.
                unsafe {
                    std::slice::from_raw_parts(
                        logits_base.as_ptr().add(row * vocabulary),
                        vocabulary,
                    )
                }
            };
            if merged_prefix {
                first_token_scores = Some(LogDistribution::from_logits(logits_row(0)));
            }
            let first_token_scores = first_token_scores
                .as_ref()
                .expect("prefix distribution captured in the first chunk");
            for (tokens, row_offset) in chunk.iter().zip(&row_offsets) {
                let Some(first) = tokens.first() else {
                    logliks.push(f64::NEG_INFINITY);
                    continue;
                };
                let mut loglik = first_token_scores.log_probability(*first);
                for (index, token) in tokens.iter().enumerate().skip(1) {
                    loglik += token_log_probability(logits_row(row_offset + index - 1), *token);
                }
                loglik += token_log_probability(logits_row(row_offset + tokens.len() - 1), eos);
                logliks.push(loglik);
            }
            timing.scoring += scoring_started.elapsed();
        }

        Ok(ScoredItem {
            logliks,
            latency: started.elapsed(),
        })
    }
}

fn build_prompt(context: &str, input_katakana: &str) -> String {
    let mut prompt = String::new();
    if !context.is_empty() {
        prompt.push(CONTEXT_MARK);
        let characters: Vec<char> = context.chars().collect();
        let start = characters.len().saturating_sub(MAX_CONTEXT_CHARACTERS);
        prompt.extend(&characters[start..]);
    }
    prompt.push(INPUT_MARK);
    prompt.push_str(input_katakana);
    prompt.push(OUTPUT_MARK);
    prompt
}

#[derive(Default)]
struct Timing {
    candidate_decode: Duration,
    scoring: Duration,
}

fn position(index: usize) -> i32 {
    i32::try_from(index).expect("token position fits i32")
}

fn log_sum_exp(logits: &[f32]) -> f64 {
    let maximum = vector_max(logits);
    let mut sums = [0.0_f32; 8];
    let mut chunks = logits.chunks_exact(8);
    for chunk in &mut chunks {
        for (sum, &value) in sums.iter_mut().zip(chunk) {
            *sum += exp_approx((value - maximum).max(-80.0));
        }
    }
    let mut total: f64 = sums.iter().copied().map(f64::from).sum();
    for &value in chunks.remainder() {
        total += f64::from(exp_approx((value - maximum).max(-80.0)));
    }
    f64::from(maximum) + total.ln()
}

/// Branch-free `exp` for the softmax normalizer: range reduction to
/// `[-ln2/2, ln2/2]` plus a degree-5 Taylor polynomial (error < 1e-6). The
/// libm `exp` is scalar-only and dominates rescoring time; this form
/// auto-vectorizes. Inputs must be clamped to `[-80, 0]` by the caller.
#[inline]
fn exp_approx(x: f32) -> f32 {
    const LOG2_E: f32 = std::f32::consts::LOG2_E;
    const LN_2_HI: f32 = 0.693_359_4;
    const LN_2_LO: f32 = -2.121_944_4e-4;
    let n = (x * LOG2_E).round();
    let r = x - n * LN_2_HI - n * LN_2_LO;
    let polynomial =
        1.0 + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0)))));
    #[allow(clippy::cast_possible_truncation)]
    let exponent_bits = ((n as i32 + 127) << 23).cast_unsigned();
    polynomial * f32::from_bits(exponent_bits)
}

/// Independent accumulators let the compiler vectorize the reduction; a naive
/// sequential fold stays scalar and dominates rescoring time.
fn vector_max(values: &[f32]) -> f32 {
    let mut accumulators = [f32::NEG_INFINITY; 8];
    let mut chunks = values.chunks_exact(8);
    for chunk in &mut chunks {
        for (accumulator, &value) in accumulators.iter_mut().zip(chunk) {
            *accumulator = accumulator.max(value);
        }
    }
    let mut maximum = f32::NEG_INFINITY;
    for &value in chunks.remainder() {
        maximum = maximum.max(value);
    }
    for &accumulator in &accumulators {
        maximum = maximum.max(accumulator);
    }
    maximum
}

fn token_log_probability(logits: &[f32], token: LlamaToken) -> f64 {
    let index = usize::try_from(token.0).expect("token id is non-negative");
    f64::from(logits[index]) - log_sum_exp(logits)
}

/// A log-softmax view over one logits vector, copied out so it survives later
/// decode calls (llama.cpp reuses the logits buffer).
struct LogDistribution {
    logits: Vec<f32>,
    log_normalizer: f64,
}

impl LogDistribution {
    fn from_logits(logits: &[f32]) -> Self {
        Self {
            logits: logits.to_vec(),
            log_normalizer: log_sum_exp(logits),
        }
    }

    fn log_probability(&self, token: LlamaToken) -> f64 {
        let index = usize::try_from(token.0).expect("token id is non-negative");
        f64::from(self.logits[index]) - self.log_normalizer
    }
}

#[cfg(test)]
mod tests {
    use super::{build_prompt, exp_approx, log_sum_exp};

    #[test]
    fn exp_approximation_matches_libm_in_the_clamped_range() {
        let mut x = -80.0_f32;
        while x <= 0.0 {
            let exact = f64::from(x).exp();
            let approximate = f64::from(exp_approx(x));
            assert!(
                (approximate - exact).abs() <= exact * 1e-5 + 1e-40,
                "exp({x}) approximation too far off: {approximate} vs {exact}"
            );
            x += 0.037;
        }
    }

    #[test]
    fn log_sum_exp_matches_exact_computation() {
        let logits: Vec<f32> = (0..6000)
            .map(|index| {
                -0.005 * {
                    #[allow(clippy::cast_precision_loss)]
                    let value = index as f32;
                    value
                }
            })
            .collect();
        let exact = {
            let maximum = f64::from(logits[0]);
            let sum: f64 = logits
                .iter()
                .map(|&logit| (f64::from(logit) - maximum).exp())
                .sum();
            maximum + sum.ln()
        };
        assert!((log_sum_exp(&logits) - exact).abs() < 1e-3);
    }

    #[test]
    fn builds_zenz_v3_prompt_with_context() {
        assert_eq!(
            build_prompt("彼は", "コウテイ"),
            "\u{EE02}彼は\u{EE00}コウテイ\u{EE01}"
        );
    }

    #[test]
    fn omits_context_block_when_context_is_empty() {
        assert_eq!(build_prompt("", "コウテイ"), "\u{EE00}コウテイ\u{EE01}");
    }

    #[test]
    fn truncates_context_to_the_last_forty_characters() {
        let context: String = "あ".repeat(60);
        let prompt = build_prompt(&context, "カナ");
        let context_part: String = prompt
            .chars()
            .skip(1)
            .take_while(|&character| character != '\u{EE00}')
            .collect();
        assert_eq!(context_part.chars().count(), 40);
    }
}
