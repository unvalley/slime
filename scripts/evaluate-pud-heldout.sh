#!/usr/bin/env bash
set -euo pipefail

# Evaluates a previously frozen model or rule on UD Japanese PUD. This dataset
# is a one-shot held-out gate, not a development set.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
version="r2.18"
revision="4abd575c57bfa125dd4bc564f2ceb8973bbbf422"
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/ud-japanese-pud/$version"
data_file="$data_dir/phrase_test.json"

if [[ ! -f "$data_file" ]]; then
  "$workspace_dir/scripts/build-pud-heldout.sh"
fi

actual_sha256=$(shasum -a 256 "$data_file" | awk '{print $1}')

cargo run --release --quiet -p slime-tools --bin slime-evaluate -- \
  ajimee --input "$data_file" \
  --dataset-name "UD Japanese PUD news/wiki independent held-out" \
  --dataset-revision "$revision" --dataset-sha256 "$actual_sha256" "$@"
