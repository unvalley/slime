#!/usr/bin/env bash
set -euo pipefail

system_bundle="/Library/Input Methods/Slime.app"
bundle_identifier="com.unvalley.inputmethod.Slime"
package_identifier="com.unvalley.inputmethod.Slime.pkg"

uninstall_privileged() {
  if [[ "$EUID" -ne 0 ]]; then
    echo "privileged uninstall must run as root" >&2
    exit 77
  fi

  if [[ -L "$system_bundle" ]]; then
    echo "refusing to remove a symbolic link at $system_bundle" >&2
    exit 65
  fi
  if [[ -e "$system_bundle" ]]; then
    if [[ ! -d "$system_bundle" ]]; then
      echo "refusing to remove a non-bundle path at $system_bundle" >&2
      exit 65
    fi
    installed_identifier="$(
      /usr/bin/plutil -extract CFBundleIdentifier raw -o - \
        "$system_bundle/Contents/Info.plist" 2>/dev/null || true
    )"
    if [[ "$installed_identifier" != "$bundle_identifier" ]]; then
      echo "refusing to remove bundle with identifier: $installed_identifier" >&2
      exit 65
    fi

    /usr/bin/pkill -x Slime >/dev/null 2>&1 || true
    /bin/rm -rf "$system_bundle"
  fi

  /usr/sbin/pkgutil --forget "$package_identifier" >/dev/null 2>&1 || true
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS input method can only be uninstalled on macOS" >&2
  exit 1
fi

if [[ "$EUID" -eq 0 ]]; then
  uninstall_privileged
else
  script_path="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  printf -v quoted_script '%q' "$script_path"
  privileged_command="$quoted_script"
  /usr/bin/osascript \
    -e 'on run arguments' \
    -e 'do shell script (item 1 of arguments) with administrator privileges' \
    -e 'end run' \
    -- "$privileged_command"
fi

if [[ -e "$system_bundle" ]]; then
  echo "input method bundle remains after uninstall: $system_bundle" >&2
  exit 1
fi
if /usr/sbin/pkgutil --pkg-info "$package_identifier" >/dev/null 2>&1; then
  echo "package receipt remains after uninstall: $package_identifier" >&2
  exit 1
fi

echo "Removed the system input method and package receipt."
echo "User dictionary and learning data were preserved."
echo "Sign out and back in if the removed input source still appears in the input menu."
