#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
umask 077

usage() {
  cat >&2 <<'EOF'
usage: scripts/prepare-private-dictionary-candidate.sh \
  --train PATH [--train PATH ...] --exclude PATH [--exclude PATH ...] \
  [--dictionary PATH ...] [--baseline-pack PATH ...] \
  --recall-target PATH [--recall-target PATH ...] \
  [--recall-held-out PATH ...] --context-target PATH [--context-target PATH ...] \
  [--context-held-out PATH ...] --license-ledger PATH --output DIRECTORY \
  --term-pack-id ID --context-pack-id ID --version VERSION \
  --minimum-slime-version VERSION --published-at YYYY-MM-DD \
  --license LICENSE --provenance PROVENANCE \
  [--max-p95-ms N] [--max-pack-load-ms N] [--max-pack-bytes N] \
  [--max-dictionary-bytes N] [--startup-iterations N] \
  [--max-startup-median-ms N] [--max-startup-p95-ms N] [--max-rss-bytes N] \
  [--max-startup-median-delta-ms N] [--max-startup-p95-delta-ms N] \
  [--max-rss-delta-bytes N]
EOF
  exit 64
}

next_value() {
  local option="$1"
  if (($# < 2)) || [[ -z "$2" ]]; then
    echo "$option requires a value" >&2
    usage
  fi
  printf '%s' "$2"
}

training_inputs=()
exclusion_inputs=()
existing_dictionaries=()
baseline_packs=()
recall_targets=()
recall_held_out=()
context_targets=()
context_held_out=()
license_ledger=""
output_directory=""
term_pack_id=""
context_pack_id=""
version=""
minimum_slime_version=""
published_at=""
pack_license=""
provenance=""
max_p95_ms="20"
max_pack_load_ms="100"
max_pack_bytes="1048576"
max_dictionary_bytes="1048576"
startup_iterations="5"
max_startup_median_ms="250"
max_startup_p95_ms="500"
max_rss_bytes="134217728"
max_startup_median_delta_ms="50"
max_startup_p95_delta_ms="100"
max_rss_delta_bytes="8388608"

while (($#)); do
  case "$1" in
    --train) training_inputs+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --exclude) exclusion_inputs+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --dictionary) existing_dictionaries+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --baseline-pack) baseline_packs+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --recall-target) recall_targets+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --recall-held-out) recall_held_out+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --context-target) context_targets+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --context-held-out) context_held_out+=("$(next_value "$1" "${2:-}")"); shift 2 ;;
    --license-ledger) license_ledger="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --output) output_directory="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --term-pack-id) term_pack_id="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --context-pack-id) context_pack_id="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --version) version="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --minimum-slime-version) minimum_slime_version="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --published-at) published_at="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --license) pack_license="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --provenance) provenance="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-p95-ms) max_p95_ms="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-pack-load-ms) max_pack_load_ms="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-pack-bytes) max_pack_bytes="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-dictionary-bytes) max_dictionary_bytes="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --startup-iterations) startup_iterations="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-startup-median-ms) max_startup_median_ms="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-startup-p95-ms) max_startup_p95_ms="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-rss-bytes) max_rss_bytes="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-startup-median-delta-ms) max_startup_median_delta_ms="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-startup-p95-delta-ms) max_startup_p95_delta_ms="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --max-rss-delta-bytes) max_rss_delta_bytes="$(next_value "$1" "${2:-}")"; shift 2 ;;
    --help|-h) usage ;;
    *) echo "unknown option" >&2; usage ;;
  esac
done

if ((${#training_inputs[@]} == 0 || ${#exclusion_inputs[@]} == 0 \
  || ${#recall_targets[@]} == 0 || ${#context_targets[@]} == 0)) \
  || [[ -z "$license_ledger" || -z "$output_directory" \
    || -z "$term_pack_id" || -z "$context_pack_id" || -z "$version" \
    || -z "$minimum_slime_version" || -z "$published_at" \
    || -z "$pack_license" || -z "$provenance" ]]; then
  usage
fi

if [[ -e "$output_directory" || -L "$output_directory" ]]; then
  echo "output directory already exists" >&2
  exit 1
fi
output_parent="$(dirname "$output_directory")"
if [[ ! -d "$output_parent" ]]; then
  echo "output parent must be an existing directory" >&2
  exit 1
fi

private_inputs=(
  "${training_inputs[@]}"
  "${exclusion_inputs[@]}"
  "${recall_targets[@]}"
  "${context_targets[@]}"
  "$license_ledger"
)
if ((${#existing_dictionaries[@]})); then
  private_inputs+=("${existing_dictionaries[@]}")
fi
if ((${#baseline_packs[@]})); then
  private_inputs+=("${baseline_packs[@]}")
fi
if ((${#recall_held_out[@]})); then
  private_inputs+=("${recall_held_out[@]}")
fi
if ((${#context_held_out[@]})); then
  private_inputs+=("${context_held_out[@]}")
fi
for private_input in "${private_inputs[@]}"; do
  if [[ -L "$private_input" || ! -f "$private_input" ]]; then
    echo "a private input is not a regular file" >&2
    exit 1
  fi
  if mode="$(stat -f '%Lp' "$private_input" 2>/dev/null)"; then
    :
  else
    mode="$(stat -c '%a' "$private_input" 2>/dev/null)" || {
      echo "cannot inspect private input permissions" >&2
      exit 1
    }
  fi
  if (( (8#$mode & 077) != 0 )); then
    echo "private inputs must not grant group or other permissions" >&2
    exit 1
  fi
done

if ((${#existing_dictionaries[@]} > 0 && ${#baseline_packs[@]} == 0)); then
  echo "--dictionary requires at least one --baseline-pack for runtime evaluation" >&2
  exit 1
fi

working_directory="$(mktemp -d "${TMPDIR:-/tmp}/slime-private-pack-candidate.XXXXXX")"
cleanup() {
  rm -rf "$working_directory"
}
trap cleanup EXIT

generated_directory="$working_directory/generated"
pack_directory="$working_directory/packs"
report_directory="$working_directory/reports"
baseline_data_directory="$working_directory/baseline-data"
candidate_data_directory="$working_directory/candidate-data"
mkdir -p \
  "$generated_directory" \
  "$pack_directory" \
  "$report_directory" \
  "$baseline_data_directory/dictionary-packs" \
  "$candidate_data_directory/dictionary-packs"

entries_tsv="$generated_directory/entries.tsv"
context_tsv="$generated_directory/context-rules.tsv"
term_pack="$pack_directory/terms.slime-dict"
context_pack="$pack_directory/context.slime-dict"

term_arguments=(--output "$entries_tsv" --json)
context_arguments=(--output "$context_tsv" --json)
for input in "${training_inputs[@]}"; do
  term_arguments+=(--input "$input")
  context_arguments+=(--input "$input")
done
for input in "${exclusion_inputs[@]}"; do
  term_arguments+=(--exclude-input "$input")
  context_arguments+=(--exclude-input "$input")
done
if ((${#existing_dictionaries[@]})); then
  for dictionary in "${existing_dictionaries[@]}"; do
    term_arguments+=(--dictionary "$dictionary")
    context_arguments+=(--dictionary "$dictionary")
  done
fi

cd "$repo_dir"
cargo run --release --quiet -p slime-tools --bin slime-term-dictionary -- \
  "${term_arguments[@]}" >"$report_directory/term-generation.json"
context_arguments+=(--dictionary "$entries_tsv")
cargo run --release --quiet -p slime-tools --bin slime-context-rules -- \
  "${context_arguments[@]}" >"$report_directory/context-generation.json"

cargo run --release --quiet -p slime-tools --bin slime-dictionary-pack -- build \
  --id "$term_pack_id" \
  --name "$term_pack_id" \
  --version "$version" \
  --license "$pack_license" \
  --minimum-slime-version "$minimum_slime_version" \
  --published-at "$published_at" \
  --provenance "$provenance/terms" \
  --entries "$entries_tsv" \
  --output "$term_pack" \
  --json >"$report_directory/term-pack.json"

cargo run --release --quiet -p slime-tools --bin slime-dictionary-pack -- build \
  --id "$context_pack_id" \
  --name "$context_pack_id" \
  --version "$version" \
  --license "$pack_license" \
  --minimum-slime-version "$minimum_slime_version" \
  --published-at "$published_at" \
  --provenance "$provenance/context" \
  --context-rules "$context_tsv" \
  --output "$context_pack" \
  --json >"$report_directory/context-pack.json"

cp "$term_pack" "$baseline_data_directory/dictionary-packs/terms.slime-dict"
cp "$term_pack" "$candidate_data_directory/dictionary-packs/terms.slime-dict"
cp "$context_pack" "$candidate_data_directory/dictionary-packs/context.slime-dict"
baseline_pack_index=0
if ((${#baseline_packs[@]})); then
  for baseline_pack in "${baseline_packs[@]}"; do
    cp \
      "$baseline_pack" \
      "$baseline_data_directory/dictionary-packs/existing-$baseline_pack_index.slime-dict"
    cp \
      "$baseline_pack" \
      "$candidate_data_directory/dictionary-packs/existing-$baseline_pack_index.slime-dict"
    baseline_pack_index=$((baseline_pack_index + 1))
  done
fi

report_index=0
for input in "${recall_targets[@]}"; do
  cargo run --release --quiet -p slime-tools --bin slime-recall -- \
    --input "$input" \
    --dictionary "$entries_tsv" \
    --min-recovered 1 \
    --max-regressed 0 \
    --max-top1-regressed 0 \
    --max-p95-ms "$max_p95_ms" \
    --max-dictionary-bytes "$max_dictionary_bytes" \
    --details 0 \
    --json >"$report_directory/recall-target-$report_index.json"
  report_index=$((report_index + 1))
done

report_index=0
if ((${#recall_held_out[@]})); then
  for input in "${recall_held_out[@]}"; do
    cargo run --release --quiet -p slime-tools --bin slime-recall -- \
      --input "$input" \
      --dictionary "$entries_tsv" \
      --max-regressed 0 \
      --max-top1-regressed 0 \
      --max-top1-changed 0 \
      --max-p95-ms "$max_p95_ms" \
      --max-dictionary-bytes "$max_dictionary_bytes" \
      --details 0 \
      --json >"$report_directory/recall-held-out-$report_index.json"
    report_index=$((report_index + 1))
  done
fi

context_gate_arguments=(
  --baseline-data-dir "$baseline_data_directory"
  --data-dir "$candidate_data_directory"
  --min-context-rules 1
  --min-added-context-rules 1
  --max-top1-regressed 0
  --max-topk-regressed 0
  --min-accuracy-delta 0
  --min-mrr-delta 0
  --max-p95-ms "$max_p95_ms"
  --max-pack-load-ms "$max_pack_load_ms"
  --max-pack-bytes "$max_pack_bytes"
  --json
)

report_index=0
for input in "${context_targets[@]}"; do
  cargo run --release --quiet -p slime-tools --bin slime-context-pack-evaluate -- \
    "${context_gate_arguments[@]}" \
    --input "$input" \
    --min-top1-improved 1 >"$report_directory/context-target-$report_index.json"
  report_index=$((report_index + 1))
done

report_index=0
if ((${#context_held_out[@]})); then
  for input in "${context_held_out[@]}"; do
    cargo run --release --quiet -p slime-tools --bin slime-context-pack-evaluate -- \
      "${context_gate_arguments[@]}" \
      --input "$input" >"$report_directory/context-held-out-$report_index.json"
    report_index=$((report_index + 1))
  done
fi

cargo run --release --quiet -p slime-tools --bin slime-pack-startup-evaluate -- \
  --baseline-data-dir "$baseline_data_directory" \
  --data-dir "$candidate_data_directory" \
  --iterations "$startup_iterations" \
  --max-median-ms "$max_startup_median_ms" \
  --max-p95-ms "$max_startup_p95_ms" \
  --max-rss-bytes "$max_rss_bytes" \
  --max-median-delta-ms "$max_startup_median_delta_ms" \
  --max-p95-delta-ms "$max_startup_p95_delta_ms" \
  --max-rss-delta-bytes "$max_rss_delta_bytes" \
  --json >"$report_directory/process-startup.json"

if command -v shasum >/dev/null; then
  ledger_sha256="$(shasum -a 256 "$license_ledger" | awk '{print $1}')"
elif command -v sha256sum >/dev/null; then
  ledger_sha256="$(sha256sum "$license_ledger" | awk '{print $1}')"
else
  echo "a SHA-256 command is required" >&2
  exit 1
fi
printf '%s\n' "$ledger_sha256" >"$report_directory/license-ledger.sha256"

if ! mkdir "$output_directory"; then
  echo "output directory was created concurrently" >&2
  exit 1
fi
cp -R "$generated_directory" "$pack_directory" "$report_directory" "$output_directory/"
printf '%s\n' \
  "This directory contains an unsigned private candidate, not a release artifact." \
  >"$output_directory/UNSIGNED"
touch "$output_directory/READY"

echo "Private dictionary candidate passed generation, recall, context, size, and latency gates."
echo "The candidate is unsigned and must pass the signed-pack release verifier before distribution."
