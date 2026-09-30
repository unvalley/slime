#!/usr/bin/env bash
set -euo pipefail

package_path="${1:-}"
notary_profile="${SLIME_NOTARY_PROFILE:-}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS notarization can only run on macOS" >&2
  exit 1
fi
if [[ -z "$package_path" || ! -f "$package_path" ]]; then
  echo "usage: scripts/notarize-macos-pkg.sh <signed.pkg>" >&2
  exit 1
fi
if [[ -z "$notary_profile" ]]; then
  echo "SLIME_NOTARY_PROFILE must name a notarytool Keychain profile" >&2
  exit 1
fi

result_file="$(mktemp)"
cleanup() {
  rm -f "$result_file"
}
trap cleanup EXIT
xcrun notarytool submit "$package_path" \
  --keychain-profile "$notary_profile" \
  --wait \
  --output-format json >"$result_file"
status="$(plutil -extract status raw -o - "$result_file")"
submission_id="$(plutil -extract id raw -o - "$result_file")"
if [[ "$status" != "Accepted" || -z "$submission_id" ]]; then
  echo "notarization was not accepted (status: $status, id: ${submission_id:-missing})" >&2
  exit 1
fi
xcrun stapler staple "$package_path"
xcrun stapler validate "$package_path"
spctl --assess --type install --verbose=4 "$package_path"
echo "Notarized and stapled $package_path (submission $submission_id)"
