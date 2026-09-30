# Targeted dictionary recombination API — 2026-09-12

Implemented converter operation, not yet integrated into ordinary Space or LIVE ranking. Existing QA app/default behavior remains the adopted consistent-confidence revision. No new full-product quality gain is claimed yet.

`Dictionary::recombined_candidates_from_surfaces(reading, [before, preferred], limit)` locates both surfaces in a bounded distinct-surface16 path search for connected dictionaries (existing heuristic search otherwise). It retains only matching paths, without cloning their segment strings. It reuses dictionary-observed edges, exact word costs and POS connections to generate up to2 alternatives with a64-state intermediate bound. Both input surfaces are excluded. Equal/missing source surfaces, zero output limit, empty input, and over128-character readings return no results. Costs have no external left-context adjustment, so initial runtime integration must respect that boundary. Caller must additionally exclude surfaces already in its full candidate request.

The original recombined_n_best_variants now delegates its unchanged graph body to a private helper, preserving original path search, limits, and intermediate beam calculation. The new API uses distinct-surface source selection only for the targeted operation. It is not a global source-width increase.

Tests cover synthetic crossed paths with exact costs20/40, max2 output despite a larger requested limit, unavailable/equal sources and zero limit, plus the real missing brain-pressure candidate at exact connection-aware cost61880. Full tests: converter65 pass/1preexisting ignored, Core264 pass, FFI52 pass. Final Clippy passes with only the already-used toolchain chunks_exact_to_as_chunks allowance.

An initial whole-file formatting pass expanded an unrelated insert_n_best_node condition across enough lines to trigger Clippy's line limit. Unrelated formatting was restored from the before snapshot and the final strict check passed without adding a line-limit suppression. Earlier exploratory lint logs are superseded by clippy-final.log.

Next integration: hold the existing exclusive engine transaction across initial ranking, bounded dictionary generation, scoring only genuinely new candidates, and exact-permutation application. Reject invalid sources/returned orders, preserve protected menu positions, and retain the prior output on failed expansion/scoring. Source and preferred surfaces must come from the current request; no arbitrary generated text should be accepted. Avoid adding extra model work unless existing score evidence calls for the targeted expansion. Full/fresh accuracy, app replay, and timing remain necessary once connected.

Artifacts: `target/evaluation/targeted-recombination-api-20260912/` contains original/final converter source, focused/full test logs, final Clippy log, and status/hash. All phase processes have completed.
