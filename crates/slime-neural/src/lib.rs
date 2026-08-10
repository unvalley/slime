//! Neural N-best rescoring with a zenz GGUF model.
//!
//! Scores each existing candidate both with and without its trailing EOS under
//! a character-level conditional LM. Rescoring is prefill-only and
//! normally needs a single decode call per item: the shared `context +
//! reading` prefix is assigned to every sequence, and candidate prefixes are
//! represented as a trie so candidates such as long sentence variants do not
//! decode their identical leading tokens once per candidate.
//!
//! Prompt format (zenz-v3):
//! `\u{EE02}<left>\u{EE07}<right>\u{EE00}<katakana input>\u{EE01}<output></s>`.
//! Empty context blocks are omitted.

use std::path::Path;
use std::time::{Duration, Instant};

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{LogOptions, send_logs_to_tracing};

/// Maximum characters of left context fed to the model. Zenzai truncates the
/// context similarly; unbounded context would dominate prefill latency.
const MAX_CONTEXT_CHARACTERS: usize = 40;

/// Candidates scored in parallel as independent sequences in one decode call.
const DEFAULT_MAX_PARALLEL_CANDIDATES: usize = 16;

/// Total KV cells: shared prefix + one suffix per parallel candidate.
const DEFAULT_KV_CELLS: u32 = 4096;

/// zenz is trained with 1024 positions; skip items that would exceed it.
const MAX_POSITIONS: usize = 1024;

const CONTEXT_MARK: char = '\u{EE02}';
const RIGHT_CONTEXT_MARK: char = '\u{EE07}';
const INPUT_MARK: char = '\u{EE00}';
const OUTPUT_MARK: char = '\u{EE01}';

pub struct ScoreRequest {
    pub context: String,
    pub right_context: String,
    pub input_katakana: String,
    pub candidates: Vec<String>,
}

pub struct ScoredItem {
    /// `log P(candidate, EOS | prompt)` per candidate, aligned with the request.
    pub logliks: Vec<f64>,
    /// `log P(candidate | prompt)` before the EOS contribution.
    pub candidate_logliks: Vec<f64>,
    /// Token counts used to derive length-normalized evaluation scores.
    pub candidate_token_counts: Vec<usize>,
    /// Wall-clock time spent scoring this item (prefix + all candidates).
    pub latency: Duration,
    /// First token where the model's most likely continuation differs from
    /// each candidate. Populated only by the explicit diagnostic API.
    pub first_mismatch_prefixes: Vec<Option<PrefixDiagnostic>>,
}

#[derive(Clone, Debug)]
pub struct PrefixDiagnostic {
    /// Candidate prefix followed by the model's preferred next token. When
    /// `alternative_is_eos` is true, this is the candidate prefix before EOS.
    pub prefix: String,
    pub candidate_token_index: usize,
    pub candidate_logit: f32,
    pub alternative_logit: f32,
    pub alternative_is_eos: bool,
}

pub struct Rescorer {
    model: LlamaModel,
    // The backend must outlive every model and context created from it. Rust
    // drops fields in declaration order, so keep it after `model`.
    backend: LlamaBackend,
    max_parallel_candidates: usize,
    kv_cells: u32,
}

impl Rescorer {
    /// Loads a model with the wider evaluation-time runtime bounds.
    ///
    /// # Errors
    ///
    /// Returns an error when the llama backend, model, or runtime context
    /// cannot be initialized.
    pub fn load(model_path: &Path) -> Result<Self, String> {
        Self::load_bounded(
            model_path,
            DEFAULT_MAX_PARALLEL_CANDIDATES,
            DEFAULT_KV_CELLS,
        )
    }

    /// Loads a model with bounded runtime buffers for an interactive caller.
    /// The configured parallel count should match the caller's candidate cap;
    /// evaluation keeps the wider defaults through [`Self::load`].
    ///
    /// # Errors
    ///
    /// Returns an error for zero bounds or when the llama backend or model
    /// cannot be initialized.
    pub fn load_bounded(
        model_path: &Path,
        max_parallel_candidates: usize,
        kv_cells: u32,
    ) -> Result<Self, String> {
        if max_parallel_candidates == 0 || kv_cells == 0 {
            return Err("neural runtime bounds must be positive".to_owned());
        }
        send_logs_to_tracing(LogOptions::default().with_logs_enabled(false));
        let backend = LlamaBackend::init()
            .map_err(|error| format!("failed to initialize llama backend: {error}"))?;
        let mut model_params = LlamaModelParams::default();
        if std::env::var_os("SLIME_NEURAL_CPU").is_some() {
            model_params = model_params.with_n_gpu_layers(0);
        }
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|error| format!("failed to load model {}: {error}", model_path.display()))?;
        Ok(Self {
            model,
            backend,
            max_parallel_candidates,
            kv_cells,
        })
    }

    /// Scores every request. One llama context is created for the whole run.
    ///
    /// # Errors
    ///
    /// Returns an error when context creation, tokenization, batching, or
    /// model decoding fails.
    ///
    /// # Panics
    ///
    /// Panics only if llama.cpp returns an invalid negative token ID or a
    /// configured token position exceeds `i32`, both of which violate the
    /// validated model and bounded-context contract.
    pub fn score_all(&self, requests: &[ScoreRequest]) -> Result<Vec<ScoredItem>, String> {
        self.score_all_internal(requests, false)
    }

    /// Scores requests and reports the model's first preferred alternative
    /// prefix for every candidate. This performs additional vocabulary scans
    /// and should be enabled only by an explicit high-accuracy policy.
    ///
    /// # Errors
    ///
    /// Returns an error when context creation or decoding fails. An individual
    /// alternative token that cannot be rendered as UTF-8 is omitted without
    /// discarding the candidate scores.
    pub fn score_all_with_prefix_diagnostics(
        &self,
        requests: &[ScoreRequest],
    ) -> Result<Vec<ScoredItem>, String> {
        self.score_all_internal(requests, true)
    }

    fn score_all_internal(
        &self,
        requests: &[ScoreRequest],
        diagnose_prefixes: bool,
    ) -> Result<Vec<ScoredItem>, String> {
        let sequence_count = u32::try_from(self.max_parallel_candidates)
            .map_err(|_| "parallel candidate count does not fit u32".to_owned())?;
        let context_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(self.kv_cells))
            .with_n_batch(self.kv_cells)
            .with_n_ubatch(self.kv_cells)
            .with_n_seq_max(sequence_count)
            .with_kv_unified(true);
        let mut context = self
            .model
            .new_context(&self.backend, context_params)
            .map_err(|error| format!("failed to create llama context: {error}"))?;
        let mut batch = LlamaBatch::new(
            usize::try_from(self.kv_cells).expect("kv cells fit usize"),
            i32::try_from(self.max_parallel_candidates)
                .map_err(|_| "parallel candidate count does not fit i32".to_owned())?,
        );
        let mut timing = Timing::default();
        let scored: Result<Vec<ScoredItem>, String> = requests
            .iter()
            .map(|request| {
                self.score_item(
                    &mut context,
                    &mut batch,
                    request,
                    &mut timing,
                    diagnose_prefixes,
                )
            })
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
        diagnose_prefixes: bool,
    ) -> Result<ScoredItem, String> {
        let started = Instant::now();
        let prompt = build_prompt(
            &request.context,
            &request.right_context,
            &request.input_katakana,
        );
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
                candidate_logliks: vec![0.0; request.candidates.len()],
                candidate_token_counts: candidate_tokens.iter().map(Vec::len).collect(),
                latency: started.elapsed(),
                first_mismatch_prefixes: vec![None; request.candidates.len()],
            });
        }

        // The whole item is decoded in a single call when the candidates fit
        // into the parallel sequences: the prefix tokens are shared by every
        // sequence and each candidate continues its own sequence. Metal decode
        // has a large fixed launch overhead, so decode calls are minimized.
        let sequences: Vec<i32> = (0..self.max_parallel_candidates)
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
        let mut candidate_logliks = Vec::with_capacity(candidate_tokens.len());
        let mut first_token_scores: Option<LogDistribution> = None;
        let mut first_token_maximum: Option<(LlamaToken, f32)> = None;
        let mut first_mismatch_prefixes = Vec::with_capacity(candidate_tokens.len());
        for (chunk_index, chunk) in candidate_tokens
            .chunks(self.max_parallel_candidates)
            .enumerate()
        {
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
            // decode. Candidate rows follow in trie-node insertion order.
            // Each node belongs to every sequence with that exact token
            // prefix, letting llama.cpp share its KV state until candidates
            // diverge instead of decoding an identical sentence prefix once
            // per candidate.
            let trie = CandidateTokenTrie::build(chunk);
            let candidate_row_base = usize::from(merged_prefix);
            for node in &trie.nodes {
                batch
                    .add(
                        node.token,
                        position(prefix_tokens.len() + node.depth - 1),
                        &node.sequences,
                        true,
                    )
                    .map_err(|error| format!("failed to build candidate batch: {error}"))?;
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
                // logits-enabled batch token; `row` is below the prefix row
                // plus the number of trie nodes decoded in this batch.
                unsafe {
                    std::slice::from_raw_parts(
                        logits_base.as_ptr().add(row * vocabulary),
                        vocabulary,
                    )
                }
            };
            if merged_prefix {
                first_token_scores = Some(LogDistribution::from_logits(logits_row(0)));
                if diagnose_prefixes {
                    first_token_maximum = Some(maximum_token(logits_row(0)));
                }
            }
            let first_token_scores = first_token_scores
                .as_ref()
                .expect("prefix distribution captured in the first chunk");
            let node_maxima = diagnose_prefixes
                .then(|| {
                    (0..trie.nodes.len())
                        .map(|node| maximum_token(logits_row(candidate_row_base + node)))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for (tokens, path) in chunk.iter().zip(&trie.candidate_paths) {
                let Some(first) = tokens.first() else {
                    logliks.push(f64::NEG_INFINITY);
                    candidate_logliks.push(f64::NEG_INFINITY);
                    first_mismatch_prefixes.push(None);
                    continue;
                };
                let mut loglik = first_token_scores.log_probability(*first);
                for (index, token) in tokens.iter().enumerate().skip(1) {
                    let preceding_node = path[index - 1];
                    loglik += token_log_probability(
                        logits_row(candidate_row_base + preceding_node),
                        *token,
                    );
                }
                candidate_logliks.push(loglik);
                let final_node = *path.last().expect("non-empty candidate has a trie path");
                loglik += token_log_probability(logits_row(candidate_row_base + final_node), eos);
                logliks.push(loglik);

                if diagnose_prefixes {
                    let first_maximum =
                        first_token_maximum.expect("prefix maximum captured in the first chunk");
                    let mismatch = tokens.iter().enumerate().find_map(|(index, &token)| {
                        let (alternative, alternative_logit, candidate_logit) = if index == 0 {
                            (
                                first_maximum.0,
                                first_maximum.1,
                                first_token_scores.logit(token),
                            )
                        } else {
                            let preceding_node = path[index - 1];
                            let maximum = node_maxima[preceding_node];
                            (
                                maximum.0,
                                maximum.1,
                                logits_row(candidate_row_base + preceding_node)[token_index(token)],
                            )
                        };
                        (alternative != token).then_some((
                            index,
                            alternative,
                            candidate_logit,
                            alternative_logit,
                        ))
                    });
                    let diagnostic = mismatch.and_then(
                        |(index, alternative, candidate_logit, alternative_logit)| {
                            let alternative_is_eos = alternative == eos;
                            let mut prefix_tokens = tokens[..index].to_vec();
                            if !alternative_is_eos {
                                prefix_tokens.push(alternative);
                            }
                            Some(PrefixDiagnostic {
                                prefix: self.tokens_to_string(&prefix_tokens).ok()?,
                                candidate_token_index: index,
                                candidate_logit,
                                alternative_logit,
                                alternative_is_eos,
                            })
                        },
                    );
                    first_mismatch_prefixes.push(diagnostic);
                } else {
                    first_mismatch_prefixes.push(None);
                }
            }
            timing.scoring += scoring_started.elapsed();
        }

        Ok(ScoredItem {
            logliks,
            candidate_logliks,
            candidate_token_counts: candidate_tokens.iter().map(Vec::len).collect(),
            latency: started.elapsed(),
            first_mismatch_prefixes,
        })
    }

    fn tokens_to_string(&self, tokens: &[LlamaToken]) -> Result<String, String> {
        let mut bytes = Vec::with_capacity(tokens.len().saturating_mul(4));
        for &token in tokens {
            let piece = self
                .model
                .token_to_piece_bytes(token, 32, false, None)
                .map_err(|error| format!("failed to decode alternative token: {error}"))?;
            bytes.extend_from_slice(&piece);
        }
        String::from_utf8(bytes)
            .map_err(|error| format!("alternative prefix is not valid UTF-8: {error}"))
    }
}

#[derive(Debug)]
struct CandidateTokenTrie {
    nodes: Vec<CandidateTokenNode>,
    candidate_paths: Vec<Vec<usize>>,
}

#[derive(Debug)]
struct CandidateTokenNode {
    token: LlamaToken,
    depth: usize,
    sequences: Vec<i32>,
    children: Vec<usize>,
}

impl CandidateTokenTrie {
    fn build(candidates: &[Vec<LlamaToken>]) -> Self {
        let mut nodes = Vec::<CandidateTokenNode>::new();
        let mut roots = Vec::<usize>::new();
        let mut candidate_paths = Vec::with_capacity(candidates.len());

        for (sequence, tokens) in candidates.iter().enumerate() {
            let sequence = i32::try_from(sequence).expect("candidate sequence fits i32");
            let mut parent: Option<usize> = None;
            let mut path = Vec::with_capacity(tokens.len());
            for (depth, &token) in tokens.iter().enumerate() {
                let siblings =
                    parent.map_or(roots.as_slice(), |index| nodes[index].children.as_slice());
                let existing = siblings
                    .iter()
                    .copied()
                    .find(|&index| nodes[index].token == token);
                let index = existing.unwrap_or_else(|| {
                    let index = nodes.len();
                    nodes.push(CandidateTokenNode {
                        token,
                        depth: depth + 1,
                        sequences: Vec::new(),
                        children: Vec::new(),
                    });
                    if let Some(parent) = parent {
                        nodes[parent].children.push(index);
                    } else {
                        roots.push(index);
                    }
                    index
                });
                nodes[index].sequences.push(sequence);
                path.push(index);
                parent = Some(index);
            }
            candidate_paths.push(path);
        }

        Self {
            nodes,
            candidate_paths,
        }
    }
}

fn build_prompt(context: &str, right_context: &str, input_katakana: &str) -> String {
    let mut prompt = String::new();
    if !context.is_empty() {
        prompt.push(CONTEXT_MARK);
        let characters: Vec<char> = context.chars().collect();
        let start = characters.len().saturating_sub(MAX_CONTEXT_CHARACTERS);
        prompt.extend(&characters[start..]);
    }
    if !right_context.is_empty() {
        prompt.push(RIGHT_CONTEXT_MARK);
        prompt.extend(right_context.chars().take(MAX_CONTEXT_CHARACTERS));
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
    f64::from(logits[token_index(token)]) - log_sum_exp(logits)
}

fn token_index(token: LlamaToken) -> usize {
    usize::try_from(token.0).expect("token id is non-negative")
}

fn maximum_token(logits: &[f32]) -> (LlamaToken, f32) {
    let (index, &logit) = logits
        .iter()
        .enumerate()
        .max_by(|left, right| left.1.total_cmp(right.1))
        .expect("model vocabulary is non-empty");
    (
        LlamaToken(i32::try_from(index).expect("vocabulary index fits i32")),
        logit,
    )
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
        f64::from(self.logit(token)) - self.log_normalizer
    }

    fn logit(&self, token: LlamaToken) -> f32 {
        self.logits[token_index(token)]
    }
}

#[cfg(test)]
mod tests {
    use llama_cpp_2::token::LlamaToken;

    use super::{CandidateTokenTrie, build_prompt, exp_approx, log_sum_exp};

    #[test]
    fn candidate_trie_shares_exact_token_prefixes() {
        let trie = CandidateTokenTrie::build(&[
            vec![LlamaToken(1), LlamaToken(2), LlamaToken(3)],
            vec![LlamaToken(1), LlamaToken(2), LlamaToken(4)],
            vec![LlamaToken(1), LlamaToken(5)],
        ]);

        assert_eq!(trie.nodes.len(), 5);
        assert_eq!(trie.candidate_paths[0], [0, 1, 2]);
        assert_eq!(trie.candidate_paths[1], [0, 1, 3]);
        assert_eq!(trie.candidate_paths[2], [0, 4]);
        assert_eq!(trie.nodes[0].sequences, [0, 1, 2]);
        assert_eq!(trie.nodes[1].sequences, [0, 1]);
        assert_eq!(trie.nodes[2].sequences, [0]);
        assert_eq!(trie.nodes[3].sequences, [1]);
        assert_eq!(trie.nodes[4].sequences, [2]);
    }

    #[test]
    fn candidate_trie_keeps_empty_and_disjoint_candidates_aligned() {
        let trie =
            CandidateTokenTrie::build(&[Vec::new(), vec![LlamaToken(7)], vec![LlamaToken(8)]]);

        assert!(trie.candidate_paths[0].is_empty());
        assert_eq!(trie.candidate_paths[1], [0]);
        assert_eq!(trie.candidate_paths[2], [1]);
        assert_eq!(trie.nodes[0].sequences, [1]);
        assert_eq!(trie.nodes[1].sequences, [2]);
    }

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
            build_prompt("彼は", "", "コウテイ"),
            "\u{EE02}彼は\u{EE00}コウテイ\u{EE01}"
        );
    }

    #[test]
    fn omits_context_block_when_context_is_empty() {
        assert_eq!(build_prompt("", "", "コウテイ"), "\u{EE00}コウテイ\u{EE01}");
    }

    #[test]
    fn truncates_context_to_the_last_forty_characters() {
        let context: String = "あ".repeat(60);
        let prompt = build_prompt(&context, "", "カナ");
        let context_part: String = prompt
            .chars()
            .skip(1)
            .take_while(|&character| character != '\u{EE00}')
            .collect();
        assert_eq!(context_part.chars().count(), 40);
    }

    #[test]
    fn builds_zenz_v3_prompt_with_bounded_right_context() {
        let right_context: String = "後".repeat(60);
        let prompt = build_prompt("前", &right_context, "カナ");
        assert!(prompt.starts_with("\u{EE02}前\u{EE07}"));
        let right: String = prompt
            .chars()
            .skip_while(|&character| character != '\u{EE07}')
            .skip(1)
            .take_while(|&character| character != '\u{EE00}')
            .collect();
        assert_eq!(right.chars().count(), 40);
    }
}
