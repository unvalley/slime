# Remaining conversion error audit — 2026-09-12

Source: `target/evaluation/explicit-gap1500-20260912/manifest.json` and its thirteen evaluation reports. This audit excludes the development extra set and overlapping roman-input validation, retaining the fixed 2,745 kana cases. It changes no product code or policy. Current FFI source was checked byte-for-byte against the accepted phase snapshot.

| Exact-match result | Cases |
| --- | ---: |
| Both LIVE and Space correct | 1,606 |
| LIVE correct, Space incorrect | 92 |
| Space correct, LIVE incorrect | 92 |
| Both incorrect, expected surface in Space candidate pool | 587 |
| Both incorrect, expected surface absent from Space candidate pool | 368 |

Totals imply 1,698 correct for each mode. These are benchmark cases, not unique independent utterances or a national quality comparison. Strict expected surfaces include spelling and orthographic preferences; every mismatch is not necessarily a semantic error.

Of the 587 candidate-present failures, 71 have no expected surface in the LIVE worker candidate scope. Of the remaining 516, 451 have an incorrect recorded worker first candidate, 63 have no recorded ranked candidates, and 2 have the expected first candidate but no applied correction. The report counters independently match these counts. No-ranked is an observable result, not proof that model inference never ran: rejection and postprocessing may also yield no retained ranking.

The two retained correct winners are `が試作されたが、制式採用されなかった` and `容姿には自信があるのか`. Both reopen a stable prefix. They need immutable application-guard diagnosis before considering an exception; these two examples alone do not justify broadening LIVE protections.

The larger opportunity is ranking or rejection diagnosis, followed by candidate recall. Raw model scores must use the actual request context, scope, cost gate, and postprocessing; earlier empty-context diagnostics showed why raw argmax cannot be treated as product output. Preserve the currently verified LIVE behavior when evaluating explicit changes.

Artifacts: `target/evaluation/remaining-error-audit-20260912/summary.json`, `worker-counts.json`, `rejected-winners.json`, and `unranked.json`. Inputs and model are frozen by the source phase manifest. This audit does not add fresh app, latency, or external IME evidence.

## Immutable guard diagnosis

A temporary Core unit probe reproduced both expected candidates being rejected by `reopened_request_changes_are_bounded`; `ranked_target_is_safe` returned true for both. The probe passed, was frozen as `core-diagnostic.rs`, and was removed from product source. Current Core was byte-compared with the accepted snapshot afterward.

For the trial/adoption sentence, both current and expected surfaces are in full-reading N-best 16, and the existing dictionary-aligned two-word predicate accepts their segment differences. The stable prefix is `が施策されたが`; the target includes a comma and inflected text, so the current two-scope exception requiring an all-kanji target does not apply. A candidate experiment can reuse the existing two-aligned-word predicate across the full request while preserving kana and punctuation and limiting changed characters. It must undergo full regression and latency validation before adoption.

For the appearance/confidence sentence, the expected full sentence and expected stable prefix are absent from ordinary N-best 16. The ranked request can contain surfaces outside that path set. Merely widening the same guard does not establish dictionary alignment for this example; it is a separate path-evidence problem. Do not claim that a two-word exception fixes both cases.

Exact probe output is in `guards.log` and `guards-paths.log`. The frozen probe has no model inference and verifies immutable Core guards only; actual FFI reports provide the retained correct-worker-winner evidence.
