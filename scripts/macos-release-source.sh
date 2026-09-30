#!/usr/bin/env bash

slime_release_source_revision() {
  local workspace_dir="$1"
  local workspace_root repository_root revision status

  workspace_root="$(cd "$workspace_dir" && pwd -P)"
  repository_root="$(git -C "$workspace_root" rev-parse --show-toplevel 2>/dev/null)" \
    || {
      echo "macOS release builds require a Git checkout" >&2
      return 1
    }
  repository_root="$(cd "$repository_root" && pwd -P)"
  if [[ "$repository_root" != "$workspace_root" ]]; then
    echo "macOS release build directory must be the repository root" >&2
    return 1
  fi

  revision="$(git -C "$workspace_root" rev-parse --verify 'HEAD^{commit}' 2>/dev/null)" \
    || {
      echo "macOS release builds require a committed source revision" >&2
      return 1
    }
  if [[ ! "$revision" =~ ^[0-9a-f]{40}$ ]]; then
    echo "macOS release source revision is malformed" >&2
    return 1
  fi

  status="$(git -C "$workspace_root" status --porcelain=v1 --untracked-files=all)"
  if [[ -n "$status" ]]; then
    echo "macOS release builds require a clean source checkout" >&2
    return 1
  fi

  printf '%s\n' "$revision"
}
