#!/usr/bin/env bash
set -euo pipefail

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)

cd "$workspace_dir"
cargo test -p slime-core --test context_adaptation -- --nocapture
cargo test -p slime-core --test typo_corrections -- --nocapture
cargo run --release --quiet -p slime-tools --bin slime-typo-evaluate -- \
  --positive crates/slime-core/testdata/typo_corrections.tsv \
  --negative crates/slime-core/testdata/typo_non_corrections.tsv \
  --max-missing 0 \
  --max-unnecessary 0 \
  --min-per-edit 2 \
  --max-p95-ms 20 \
  --max-corrections 3 \
  --json
