# Two aligned word LIVE correction candidate — 2026-09-12

Status: adopted after moving dictionary evidence preparation to the worker. Current source and QA app contain the verified worker-prepared revision. No installation or publication. Earlier candidate investigations below are historical.

The worker may repair two dictionary-aligned words across a stable-prefix boundary when the complete reading N-best 16 contains both paths. Existing character validation restricts this to three or four changed Han characters, preserving every other character and total character count. Existing segment validation requires exactly two changed words, identical per-segment readings, unchanged segmentation, and bounded all-Han word differences. The immutable snapshot reuses its existing joint-path OnceLock. Other LIVE application protections still run.

Fixed 2,745 kana cases: LIVE 1,698 → 1,699, one gain and zero losses or other changes. The gain is `が施策されたが、正式採用されなかった` → `が試作されたが、制式採用されなかった`. Space remains 1,698 and all explicit outputs are identical across all thirteen reports. Roman final outputs are unchanged.

The extra development 200 has one partial correction, `軍舞台／避難` → `軍部隊／非難`, but the full sentence is still incorrect. It is not counted as an exact-match gain. No other report outputs changed. Core 263 and neural FFI 48 tests passed. Full evaluator process terminated successfully; all thirteen reports and pairwise aggregate invariants were checked.

Artifacts: `target/evaluation/live-two-aligned-words-20260912/` contains source before/candidate snapshots, frozen evaluator, manifest/model/input hashes, build/test logs, all full reports, and `comparison.json`. Baseline is `explicit-gap1500-20260912`; candidate `adoption.json` explicitly remains false until remaining validation. No latency claim follows from the full accuracy sweep, which overlapped unit tests.

## App and latency validation revealed input-thread work

The additional 200 validation cases from explicit-gap1500 have identical LIVE/Space outputs and run statuses. macOS before/after replay of 232 inputs has 119 correct LIVE rows on both sides, zero output changes in either mode, and zero regressions. The newly added fixed-evaluator gain is already correct in the baseline incremental app replay. Direct LIVE-to-Enter replay matches LIVE in all 232 rows. Ad-hoc signature verification and Clippy passed. The baseline dylib hash matches the accepted explicit-gap1500 artifact.

The 62-case alternating three-repeat measurement revealed an application latency increase: median-of-run p95 0.023625 → 0.399375 ms, and max 0.412416 → 4.814458 ms. Worker p95 79.7655 → 77.528417 ms; these mixed timings are not a speed improvement claim. The candidate had not included its new lazy dictionary evidence in worker preparation, so application could initialize it. This prevents adopting that implementation as-is.

The next candidate adds the same immutable evidence check to `prepare_ranked_prefix_validation`, reusing the existing snapshot OnceLock. A focused regression test verifies that worker preparation fills the path cache before application and retains kana/punctuation protections. That test passes. New candidate artifacts are under `target/evaluation/live-two-aligned-words-worker-20260912`; source contains this revision. The current QA bundle still represents the earlier candidate until rebuilt. Adoption remains pending a new full comparison, timing, and app validation.

Worker-prepared revision timing: application p95 0.025083 → 0.027166 ms and max 0.383 → 0.380208 ms (medians of three runs). Worker p95 67.662625 → 72.267709 ms. Space p95 104.844875 → 112.302708 ms despite unchanged explicit policy; timing noise remains. The input-side dictionary work is resolved in this measurement. Full thirteen-report evaluation was restarted for the revised candidate; no adoption yet.

## Adopted revision

Authoritative final artifacts: `target/evaluation/live-two-aligned-words-worker-20260912/`. All thirteen reports completed successfully. Fixed 2,745 kana: LIVE 1,698 → 1,699 (one gain, zero losses), Space 1,698 unchanged with identical outputs. Development extra200 has the previously described partial correction; roman outputs unchanged. Separate validation200 is identical to baseline in LIVE/Space results and run statuses.

Final source tests: Core264 and neural FFI48 pass, including worker cache preparation and kana/punctuation protection. Clippy and QA signature verification pass. Final QA232 replay preserves all LIVE/Space outputs compared with the accepted baseline; all232 direct Enter commitments equal LIVE and normal-mode LIVE. Incremental app replay does not show an additional exact-match gain: the fixed-evaluator improvement was already correct there. Final source/model/evaluator/app hashes are recorded. The current Core source matches the frozen final candidate snapshot exactly.

The worker prepares lazy dictionary paths before application. Final timing is recorded above and in `latency-comparison.json`; no claim of overall speed improvement. No physical InputMethodKit host-app verification, installation, signed distribution release, or national superiority is established by these results.
