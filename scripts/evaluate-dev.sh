#!/usr/bin/env bash
set -euo pipefail

# Evaluates conversion quality on the JWTD-train development set. Use this for
# all tuning; AJIMEE-Bench stays held out for reporting.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/jwtd/2.0"
dev_file="$data_dir/dev_items.json"

if [[ ! -f "$dev_file" ]]; then
  "$workspace_dir/scripts/build-devset.sh"
fi

features=()
for argument in "$@"; do
  if [[ "$argument" == "--neural-model" ]]; then
    features=(--features neural)
  fi
done

cargo run --release --quiet -p slime-tools "${features[@]}" --bin slime-evaluate -- \
  ajimee "$dev_file" "$@"
