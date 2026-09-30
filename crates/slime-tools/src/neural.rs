//! Neural N-best rescoring with a zenz GGUF model (Phase 2 feasibility).
//!
//! Scores each existing candidate as `log P(candidate, EOS | context, reading)`
//! under a character-level conditional LM. Rescoring is prefill-only and
//! normally needs a single decode call per item: the shared `context +
//! reading` prefix is assigned to every sequence and each candidate continues
//! its own sequence in the same batch. Identical candidate token prefixes are
//! also shared, while divergent continuations retain separate sequence IDs.
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
// CandidateTrie stores membership in one bit per parallel sequence.
const _: () = assert!(MAX_PARALLEL_CANDIDATES <= 16);

/// Total KV cells for offline evaluation batches: shared prefix + one suffix
/// per parallel candidate.
const KV_CELLS: u32 = 4096;

/// Interactive conversion uses a bounded arena to limit first-Space allocation
/// latency and is allowed to fall
/// back to the base order when an unusually long request does not fit.
const INTERACTIVE_KV_CELLS: u32 = 1024;

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
    /// `log P(candidate | prompt)` without the terminal token, in the same order.
    pub candidate_logliks: Vec<f64>,
    /// Wall-clock time spent scoring this item (prefix + all candidates).
    pub latency: Duration,
}

pub struct Rescorer {
    // Fields drop in declaration order: the model must be freed before
    // `llama_backend_free` runs for the backend it was loaded with.
    model: LlamaModel,
    backend: LlamaBackend,
    share_candidate_prefixes: bool,
}

impl Rescorer {
    /// Loads a GGUF model and initializes its local inference backend.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend cannot initialize or the model cannot
    /// be loaded from `model_path`.
    pub fn load(model_path: &Path) -> Result<Self, String> {
        let mut backend = LlamaBackend::init()
            .map_err(|error| format!("failed to initialize llama backend: {error}"))?;
        if std::env::var_os("SLIME_NEURAL_LOG").is_none() {
            backend.void_logs();
        }
        let mut model_params = LlamaModelParams::default();
        let cpu_only = std::env::var_os("SLIME_NEURAL_CPU").is_some();
        if cpu_only {
            model_params = model_params.with_n_gpu_layers(0);
        }
        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|error| format!("failed to load model {}: {error}", model_path.display()))?;
        // CPU quantized kernels change scores more substantially when batch
        // shape changes. Keep their existing sequence layout.
        let share_candidate_prefixes = !cpu_only && backend.supports_gpu_offload();
        Ok(Self {
            model,
            backend,
            share_candidate_prefixes,
        })
    }

    /// Scores every request. One llama context is created for the whole run.
    ///
    /// # Errors
    ///
    /// Returns an error when context creation, tokenization, batch assembly,
    /// or model decoding fails.
    pub fn score_all(&self, requests: &[ScoreRequest]) -> Result<Vec<ScoredItem>, String> {
        self.score_requests(requests, KV_CELLS)
    }

    /// Scores one bounded interactive request with a smaller transient arena.
    ///
    /// # Errors
    ///
    /// Returns an error when the request cannot be tokenized, fit in the
    /// interactive batch, or decoded.
    pub fn score_interactive(&self, request: &ScoreRequest) -> Result<ScoredItem, String> {
        self.score_requests(std::slice::from_ref(request), INTERACTIVE_KV_CELLS)?
            .pop()
            .ok_or_else(|| "interactive scoring returned no item".to_owned())
    }

    /// Runs related interactive requests with one lazily allocated work arena.
    /// Each decode starts from cleared KV state; only the allocation is reused. The
    /// arena remains local to this synchronous call and is discarded on errors.
    /// No arena is allocated if the callback never asks for a score.
    pub fn with_interactive_scoring<T>(
        &self,
        work: impl FnOnce(&mut dyn FnMut(&ScoreRequest) -> Result<ScoredItem, String>) -> T,
    ) -> T {
        let mut resources = None;
        let mut timing = Timing::default();
        let result = work(&mut |request| {
            if resources.is_none() {
                resources = Some(self.new_scoring_context(INTERACTIVE_KV_CELLS)?);
            }
            let (context, batch) = resources.as_mut().ok_or("scoring arena unavailable")?;
            let scored = self.score_item(
                context,
                batch,
                request,
                &mut timing,
                usize::try_from(INTERACTIVE_KV_CELLS).map_err(|error| error.to_string())?,
            );
            if scored.is_err() {
                resources = None;
            }
            scored
        });
        if std::env::var_os("SLIME_NEURAL_TIMING").is_some() {
            eprintln!(
                "neural timing: decode_submit={:?} sync_and_scoring={:?}",
                timing.candidate_decode, timing.scoring
            );
        }
        result
    }

    fn new_scoring_context(
        &self,
        kv_cells: u32,
    ) -> Result<(LlamaContext<'_>, LlamaBatch<'_>), String> {
        let sequence_count =
            u32::try_from(MAX_PARALLEL_CANDIDATES).expect("parallel candidates fit u32");
        let context_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(kv_cells))
            .with_n_batch(kv_cells)
            .with_n_ubatch(kv_cells)
            .with_n_seq_max(sequence_count)
            .with_kv_unified(true);
        let context = self
            .model
            .new_context(&self.backend, context_params)
            .map_err(|error| format!("failed to create llama context: {error}"))?;
        let batch_capacity = usize::try_from(kv_cells).expect("kv cells fit usize");
        let batch = LlamaBatch::new(
            batch_capacity,
            i32::try_from(MAX_PARALLEL_CANDIDATES).expect("parallel candidates fit i32"),
        );
        Ok((context, batch))
    }

    fn score_requests(
        &self,
        requests: &[ScoreRequest],
        kv_cells: u32,
    ) -> Result<Vec<ScoredItem>, String> {
        let (mut context, mut batch) = self.new_scoring_context(kv_cells)?;
        let batch_capacity = usize::try_from(kv_cells).expect("kv cells fit usize");
        let mut timing = Timing::default();
        let scored: Result<Vec<ScoredItem>, String> = requests
            .iter()
            .map(|request| {
                self.score_item(
                    &mut context,
                    &mut batch,
                    request,
                    &mut timing,
                    batch_capacity,
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
        batch_capacity: usize,
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
                candidate_logliks: vec![0.0; request.candidates.len()],
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
        let mut candidate_logliks = Vec::with_capacity(candidate_tokens.len());
        let mut first_token_scores: Option<LogDistribution> = None;
        let chunks =
            candidate_chunk_ranges(&candidate_tokens, prefix_tokens.len(), batch_capacity)?;
        for (chunk_index, range) in chunks.into_iter().enumerate() {
            let chunk = &candidate_tokens[range];
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
            let trie = CandidateTrie::new(chunk, self.share_candidate_prefixes);
            let row_offset = usize::from(merged_prefix);
            let next_row = row_offset + trie.nodes.len();
            let mut members = [0; MAX_PARALLEL_CANDIDATES];
            for node in &trie.nodes {
                let mut count = 0;
                for (slot, sequence) in sequences.iter().enumerate() {
                    if node.sequences & (1 << slot) != 0 {
                        members[count] = *sequence;
                        count += 1;
                    }
                }
                batch
                    .add(
                        node.token,
                        position(prefix_tokens.len() + node.depth),
                        &members[..count],
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
                assert!(row < next_row, "logits row must belong to this batch");
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
            let mut cumulative = Vec::with_capacity(trie.nodes.len());
            let mut normalizers = Vec::with_capacity(trie.nodes.len());
            for (index, node) in trie.nodes.iter().enumerate() {
                let loglik = node.parent.map_or_else(
                    || first_token_scores.log_probability(node.token),
                    |parent| {
                        cumulative[parent]
                            + token_log_probability(
                                logits_row(row_offset + parent),
                                node.token,
                                normalizers[parent],
                            )
                    },
                );
                cumulative.push(loglik);
                normalizers.push(log_sum_exp(logits_row(row_offset + index)));
            }
            candidate_logliks.extend(
                trie.leaves
                    .iter()
                    .map(|leaf| leaf.map_or(f64::NEG_INFINITY, |index| cumulative[index])),
            );
            logliks.extend(trie.leaves.iter().map(|leaf| {
                leaf.map_or(f64::NEG_INFINITY, |index| {
                    cumulative[index]
                        + token_log_probability(
                            logits_row(row_offset + index),
                            eos,
                            normalizers[index],
                        )
                })
            }));
            timing.scoring += scoring_started.elapsed();
        }

        Ok(ScoredItem {
            logliks,
            candidate_logliks,
            latency: started.elapsed(),
        })
    }
}

/// Keep the existing sequence width when it fits, but split long candidates
/// into smaller groups instead of abandoning all scores for the request.
fn candidate_chunk_ranges<T>(
    candidates: &[Vec<T>],
    prefix_tokens: usize,
    batch_capacity: usize,
) -> Result<Vec<std::ops::Range<usize>>, String> {
    let capacity = batch_capacity
        .checked_sub(prefix_tokens)
        .ok_or_else(|| "scoring prefix exceeds the token limit".to_owned())?;
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut remaining = capacity;
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.len() > capacity {
            return Err("single candidate exceeds the token limit".to_owned());
        }
        if index - start == MAX_PARALLEL_CANDIDATES || candidate.len() > remaining {
            ranges.push(start..index);
            start = index;
            remaining = capacity;
        }
        remaining -= candidate.len();
    }
    if start < candidates.len() {
        ranges.push(start..candidates.len());
    }
    Ok(ranges)
}

struct CandidateTrieNode {
    token: LlamaToken,
    parent: Option<usize>,
    first_child: Option<usize>,
    next_sibling: Option<usize>,
    depth: usize,
    sequences: u16,
}

/// Shares only identical token prefixes. Parents always precede children, and
/// sequence membership keeps divergent continuations in separate attention paths.
struct CandidateTrie {
    nodes: Vec<CandidateTrieNode>,
    leaves: Vec<Option<usize>>,
}

impl CandidateTrie {
    fn new(candidates: &[Vec<LlamaToken>], share_prefixes: bool) -> Self {
        assert!(candidates.len() <= MAX_PARALLEL_CANDIDATES);
        let mut nodes: Vec<CandidateTrieNode> = Vec::new();
        let mut leaves = Vec::with_capacity(candidates.len());
        let mut first_root = None;
        for (slot, tokens) in candidates.iter().enumerate() {
            let mut parent: Option<usize> = None;
            for (depth, token) in tokens.iter().enumerate() {
                let head = if share_prefixes {
                    parent.map_or(first_root, |index| nodes[index].first_child)
                } else {
                    None
                };
                let mut found = head;
                while let Some(index) = found {
                    if nodes[index].token == *token {
                        break;
                    }
                    found = nodes[index].next_sibling;
                }
                let index = found.unwrap_or_else(|| {
                    let index = nodes.len();
                    nodes.push(CandidateTrieNode {
                        token: *token,
                        parent,
                        first_child: None,
                        next_sibling: head,
                        depth,
                        sequences: 0,
                    });
                    if let Some(parent) = parent {
                        nodes[parent].first_child = Some(index);
                    } else {
                        first_root = Some(index);
                    }
                    index
                });
                nodes[index].sequences |= 1 << slot;
                parent = Some(index);
            }
            leaves.push(parent);
        }
        Self { nodes, leaves }
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
    let (chunks, remainder) = logits.as_chunks::<8>();
    for chunk in chunks {
        for (sum, &value) in sums.iter_mut().zip(chunk) {
            *sum += exp_approx((value - maximum).max(-80.0));
        }
    }
    let mut total: f64 = sums.iter().copied().map(f64::from).sum();
    for &value in remainder {
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
    let (chunks, remainder) = values.as_chunks::<8>();
    for chunk in chunks {
        for (accumulator, &value) in accumulators.iter_mut().zip(chunk) {
            *accumulator = accumulator.max(value);
        }
    }
    let mut maximum = f32::NEG_INFINITY;
    for &value in remainder {
        maximum = maximum.max(value);
    }
    for &accumulator in &accumulators {
        maximum = maximum.max(accumulator);
    }
    maximum
}

fn token_log_probability(logits: &[f32], token: LlamaToken, log_normalizer: f64) -> f64 {
    let index = usize::try_from(token.0).expect("token id is non-negative");
    f64::from(logits[index]) - log_normalizer
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
    #[test]
    fn token_chunks_preserve_width_when_every_group_fits() {
        let candidates = vec![vec![0; 2]; super::MAX_PARALLEL_CANDIDATES + 2];
        assert_eq!(
            super::candidate_chunk_ranges(&candidates, 3, 100).unwrap(),
            vec![
                0..super::MAX_PARALLEL_CANDIDATES,
                super::MAX_PARALLEL_CANDIDATES..candidates.len()
            ]
        );
    }

    #[test]
    fn token_chunks_split_at_capacity_without_losing_candidates() {
        let candidates = vec![vec![0; 4], vec![0; 3], vec![0; 5], vec![], vec![0; 2]];
        let ranges = super::candidate_chunk_ranges(&candidates, 3, 10).unwrap();
        assert_eq!(ranges, vec![0..2, 2..5]);
        assert_eq!(
            ranges.into_iter().flatten().collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(super::candidate_chunk_ranges(&candidates, 6, 10).is_err());
        assert!(super::candidate_chunk_ranges(&candidates, 11, 10).is_err());
        assert!(
            super::candidate_chunk_ranges::<u8>(&[], 0, 0)
                .unwrap()
                .is_empty()
        );
    }

    use super::{build_prompt, exp_approx, log_sum_exp};

    #[test]
    fn candidate_trie_preserves_branches_duplicate_paths_and_prefix_leaves() {
        let candidates: Vec<Vec<_>> = [
            vec![1, 2, 3],
            vec![1, 2, 4],
            vec![1],
            vec![2, 3],
            vec![1, 2, 3],
            vec![],
        ]
        .into_iter()
        .map(|tokens| tokens.into_iter().map(super::LlamaToken).collect())
        .collect();
        let trie = super::CandidateTrie::new(&candidates, true);
        assert_eq!(trie.nodes.len(), 6);
        assert_eq!(trie.leaves[0], trie.leaves[4]);
        assert_eq!(trie.leaves[5], None);
        for (slot, expected) in candidates.iter().enumerate() {
            let mut current = trie.leaves[slot];
            let mut path = Vec::new();
            let mut visited = vec![false; trie.nodes.len()];
            while let Some(index) = current {
                let node = &trie.nodes[index];
                assert!(node.parent.is_none_or(|parent| parent < index));
                assert_eq!(node.depth + path.len() + 1, expected.len());
                visited[index] = true;
                path.push(node.token);
                current = node.parent;
            }
            path.reverse();
            assert_eq!(&path, expected);
            for (node, visited) in trie.nodes.iter().zip(visited) {
                assert_eq!(node.sequences & (1 << slot) != 0, visited);
            }
        }
    }

    #[test]
    fn candidate_trie_supports_every_parallel_sequence_bit() {
        let candidates = vec![vec![super::LlamaToken(1)]; super::MAX_PARALLEL_CANDIDATES];
        let trie = super::CandidateTrie::new(&candidates, true);
        assert_eq!(trie.nodes.len(), 1);
        assert_eq!(trie.nodes[0].sequences, u16::MAX);
    }

    #[test]
    fn independent_candidates_keep_the_original_batch_layout() {
        let candidates = vec![
            vec![super::LlamaToken(1), super::LlamaToken(2)],
            vec![super::LlamaToken(1), super::LlamaToken(3)],
        ];
        let trie = super::CandidateTrie::new(&candidates, false);
        assert_eq!(
            trie.nodes.iter().map(|n| n.token.0).collect::<Vec<_>>(),
            vec![1, 2, 1, 3]
        );
        assert_eq!(
            trie.nodes.iter().map(|n| n.sequences).collect::<Vec<_>>(),
            vec![1, 1, 2, 2]
        );
        assert_eq!(trie.leaves, vec![Some(1), Some(3)]);
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
