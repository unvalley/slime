#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_dir="$workspace_dir/.slime-private/dictionary-packs"
destination_root="${SLIME_DATA_DIR:-${HOME}/Library/Application Support/Slime}"
destination_dir="$destination_root/dictionary-packs"

if [[ ! -d "$source_dir" ]]; then
  echo "Private dictionary staging directory not found: $source_dir" >&2
  exit 1
fi

mkdir -p "$destination_dir"
installed=0
for pack in "$source_dir"/*.slime-dict; do
  [[ -f "$pack" ]] || continue
  install -m 600 "$pack" "$destination_dir/$(basename "$pack")"
  installed=$((installed + 1))
done

if [[ "$installed" -eq 0 ]]; then
  echo "No .slime-dict files found in $source_dir" >&2
  exit 1
fi

echo "Installed $installed local dictionary packs into $destination_dir"
echo "Use the Slime settings Reload button or restart the input source."
