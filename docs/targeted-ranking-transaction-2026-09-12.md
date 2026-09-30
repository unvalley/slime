# Targeted candidate ranking transaction — 2026-09-12

Core now exposes `CandidateRankingPlan` and `rank_candidates_with_recombination`. This is an unused integration API; ordinary Space and LIVE still use the previously adopted behavior. No additional product accuracy gain or latency claim is made.

The callback supplies a validated initial permutation and optionally two current-request source surfaces, starting with its selected candidate. Core permits expansion only for long readings with empty external context and a rankable first menu entry. It asks the bounded converter API for at most two dictionary-derived alternatives and removes any surface already in the full menu. A second callback receives the original request followed by the additions. Core accepts only an exact permutation and performs at most one expansion round. Failed or malformed second results apply the initial ranking without increasing the menu size. Actual candidate mutation occurs after second-result validation, while the engine remains exclusively borrowed. Protected menu positions remain fixed. Subsequent menu expansion starts from the existing menu, preserving appended candidates.

Three focused tests verify the real missing brain-pressure combination at dictionary cost61880, unchanged request prefix and protected positions, successful Enter commit, failed/malformed second-result fallback, and rejection of unknown sources or nonempty external context. The successful second callback deliberately requests another expansion to verify the one-round bound.

Full library checks passed: converter65 (one preexisting ignored), Core267, neural FFI52. Clippy passed with the existing `chunks_exact_to_as_chunks` toolchain allowance. Changes relative to the phase snapshot are limited to the new plan type, method, and three tests; unrelated formatting was preserved.

Next: reuse the first model scores, request expansion only for a high-confidence alternative rejected for inconsistent replacements, score only appended candidates, and preserve the initial stable baseline for final confidence validation. Connect experimentally before evaluating full/fresh accuracy, app replay, and additional latency. This phase has no app rebuild or physical InputMethodKit validation.

Artifacts: `target/evaluation/targeted-ranking-transaction-20260912/` holds source snapshots, scoped patch, focused/full test logs, Clippy log, and status/hash.
