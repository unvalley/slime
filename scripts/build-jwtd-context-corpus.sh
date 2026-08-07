#!/usr/bin/env bash
set -euo pipefail

# Builds an evaluation-only annotated corpus from JWTD v2 train. Stable source
# lines whose index is divisible by 10 stay out of training and form the fixed
# discriminative dev partition. AJIMEE remains held out because it derives
# from JWTD test, not train.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
version="2.0"
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/jwtd/$version"
train_file="$data_dir/jwtd_v2.0/train.jsonl"
output_file="$data_dir/context_train_items.json"
annotated_file="$data_dir/annotated_context_train.txt"
item_count="${SLIME_CONTEXT_TRAIN_COUNT:-100000}"

if [[ ! -f "$train_file" ]]; then
  "$workspace_dir/scripts/build-devset.sh"
fi

cargo run --release --quiet -p slime-tools --bin slime-devset -- \
  "$train_file" "$workspace_dir/crates/slime-converter/data/mozc-basic.tsv" \
  "$output_file" --count "$item_count" \
  --partition-count 10 --exclude-partition-index 0 \
  --annotated-output "$annotated_file"

echo "JWTD context items: $output_file"
echo "JWTD annotated corpus: $annotated_file"
