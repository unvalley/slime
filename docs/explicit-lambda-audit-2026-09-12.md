# Long explicit model weight audit — 2026-09-12

This diagnostic reuses the 200 correctly contextualized score tables in `target/evaluation/explicit-cost-gate-20260912/matched-*.tsv` for the development extra set. Their current-weight predictions, including the explicit1500 cost gate and newly-scored hiragana guard, match all 200 current product Space outputs from `live-two-aligned-words-worker-20260912/extra.json`. This validates reuse at lambda0.45, not the complete runtime policy at other weights. Tables contain rounded scores and omit later ranking protections; proposed weights require actual FFI evaluation.

Current extra200 exact matches:100. Offline predictions: lambda0.55 gives103 (3 gains,0losses); lambda0.8 gives110 (13gains,3losses). Some exact-match differences concern orthography or potentially erroneous gold, so neither aggregate alone establishes better Japanese. For example `機種上げ` versus `機首上げ` merits contextual review rather than blindly optimizing the supplied string.

Two actual thirteen-set evaluations are running with the same accepted executable, all other parameters unchanged, explicit long lambda0.55 and0.8 respectively. Production source and QA app retain lambda0.45. Directories: `target/evaluation/explicit-long-lambda055-20260912` and `explicit-long-lambda080-20260912`. The development score sweep and reproducible script are in `target/evaluation/explicit-lambda-audit-20260912`. No candidate has been adopted. These runs measure accuracy, not comparative latency; they run concurrently.

## Completed actual FFI comparison

Both thirteen-report runs terminated successfully. Fixed kana totals (excluding extra and roman overlap):

- lambda055: Space 1698 → 1715; 24 gains, 7 losses, 30 other changes.
- lambda080: Space 1698 → 1739; 68 gains, 27 losses, 89 other changes.

All LIVE outputs remain identical. Neither candidate is adopted: source, QA bundle, and production defaults remain the accepted worker-prepared revision with long lambda0.45. These are promising aggregate gains, but specific semantic regressions include 更生プログラム→構成プログラム and 以後→囲碁. Spelling-only differences and potentially incorrect expected strings also occur, so exact-match deltas alone are insufficient. Further work should examine score margins and candidate ambiguity with the actual context, preserving the gain opportunity instead of assuming every loss is a real quality regression or that zero benchmark losses is the only acceptable outcome. Actual runtime score guards can differ from the offline sweep: extra200 at0.8 is109, not the raw prediction110.
