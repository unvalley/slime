#!/usr/bin/env bash
set -euo pipefail

# Evaluates news/blog homophone ranking on UD Japanese GSD dev. Its test split
# intentionally has no convenience command so it stays held out until a model
# and interpolation weight are frozen.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
version="r2.18"
revision="33e7310b58308e85fd2b33a2fc3ef3e434f821c7"
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/ud-japanese-gsd/$version"
data_file="$data_dir/balanced_dev.json"

if [[ ! -f "$data_file" ]]; then
  "$workspace_dir/scripts/build-balanced-devset.sh"
fi

actual_sha256=$(shasum -a 256 "$data_file" | awk '{print $1}')
cargo run --release --quiet -p slime-tools --bin slime-evaluate -- \
  ajimee --input "$data_file" \
  --dataset-name "UD Japanese GSD ambiguous-content dev" \
  --dataset-revision "$revision" --dataset-sha256 "$actual_sha256" "$@"
