# Candidate recall versus search width, 2026-09-09

The accepted converter was evaluated with 10, 16, and 32 dictionary candidates. This investigation does **not** change the product's search limits. More candidates recover additional reference strings, but substantially increase candidate-generation time; these results do not establish better neural first-choice accuracy.

## Results

| Dataset | Items | Correct surface in 10 | In 16 | In 32 | Generation p95, 10 / 16 / 32 |
| --- | ---: | ---: | ---: | ---: | --- |
| JWTD development sample | 400 | 285 | 298 | 325 | 11.0 / 20.4 / 69.5 ms |
| AJIMEE sample | 200 | 171 | 174 | 179 | 14.0 / 28.7 / 99.8 ms |

These are one sequential pass per configuration on the local Apple M3, without model scoring. Timing is diagnostic, not a repeated performance benchmark. The cost-ranked first candidate stayed unchanged (117/400 and 107/200 correct). No previously included reference surface was lost at either wider limit. The 10-candidate surfaces and costs exactly reproduced the previously accepted exports.

Restricting expansion from 10 to 16 to readings of at least 20 characters would add 12 reference surfaces in JWTD and two in AJIMEE. For readings of 30–49 characters, the increases were four of 166 and zero of 33 respectively. Length alone therefore does not identify a consistently valuable expansion. Some gains are spelling choices, such as 宿所として造った城; recall is not equivalent to resolving an objectively incorrect conversion.

The current neural runtime processes at most 16 candidates per chunk and preserves the original token-capacity fallback. Enlarging an explicit candidate pool also changes model work and fallback behavior. A future experiment must check those boundaries, first-choice regressions, and complete Space latency before adoption. The accepted 10-candidate explicit policy and LIVE policy remain unchanged.

## Reproduction and artifacts

Artifacts are in `target/evaluation/recall-frontier-20260909/`. Each `{dataset}-{limit}.json` contains metrics; the corresponding `-nbest.json` records every candidate and cost. `summary.json`, `length-analysis.json`, and `source-binary-sha256.txt` preserve aggregate results, individual recovered items, and provenance.

Use the preserved `target/evaluation/overnight-20260909/slime-evaluate-accepted` binary with:

```sh
slime-evaluate-accepted ajimee --input DATASET --top-k LIMIT --search-k LIMIT \
  --failures 1000 --json --export-nbest OUTPUT
```

Datasets:

- `target/evaluation/jwtd/2.0/dev_items.json`
- `target/evaluation/ajimee-bench/401666cd56d1a570c2021798b64b6da4396bfd45/evaluation_items.json`

These are previously reused evaluation samples, not newly independent held-out evidence. No source behavior, app artifact, installed input method, or user data was changed in this investigation.
