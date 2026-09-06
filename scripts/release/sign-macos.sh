#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <release-binary-directory>" >&2
  exit 2
fi

for name in \
  KADUOX_MACOS_SIGNING_CERT_P12_BASE64 \
  KADUOX_MACOS_SIGNING_CERT_PASSWORD \
  KADUOX_MACOS_SIGNING_IDENTITY \
  KADUOX_APPLE_NOTARY_KEY_P8_BASE64 \
  KADUOX_APPLE_NOTARY_KEY_ID \
  KADUOX_APPLE_NOTARY_ISSUER_ID; do
  if [[ -z "${!name:-}" ]]; then
    echo "required native-signing input $name is missing" >&2
    exit 2
  fi
done

binary_dir="$(cd "$1" && pwd -P)"
binaries=(kssh kssh-tui kssh-fleet kssh-inventory)
for binary in "${binaries[@]}"; do
  [[ -f "$binary_dir/$binary" && ! -L "$binary_dir/$binary" ]] || {
    echo "release binary is missing or link-like before Developer ID signing: $binary_dir/$binary" >&2
    exit 2
  }
done

work="${RUNNER_TEMP:?RUNNER_TEMP is required}/kaduox-native-signing"
rm -rf "$work"
mkdir -m 700 "$work"
p12="$work/developer-id.p12"
notary_key="$work/notary-key.p8"
keychain="$work/signing.keychain-db"
notary_dir="$work/notary-payload"
notary_zip="$work/notary-payload.zip"
notary_result="$work/notary-result.json"
keychain_password="$(openssl rand -hex 32)"

cleanup() {
  set +e
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

python - "$p12" "$notary_key" <<'PY'
import base64
import binascii
import os
import pathlib
import sys

for env_name, output in (
    ("KADUOX_MACOS_SIGNING_CERT_P12_BASE64", pathlib.Path(sys.argv[1])),
    ("KADUOX_APPLE_NOTARY_KEY_P8_BASE64", pathlib.Path(sys.argv[2])),
):
    try:
        payload = base64.b64decode(os.environ[env_name], validate=True)
    except (KeyError, binascii.Error, ValueError) as exc:
        raise SystemExit(f"invalid base64 signing credential {env_name}: {exc}")
    if not payload:
        raise SystemExit(f"decoded signing credential {env_name} is empty")
    output.write_bytes(payload)
    output.chmod(0o600)
PY

security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$p12" -k "$keychain" -P "$KADUOX_MACOS_SIGNING_CERT_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" >/dev/null
security list-keychains -d user -s "$keychain"

for binary in "${binaries[@]}"; do
  path="$binary_dir/$binary"
  codesign --force --timestamp --options runtime --sign "$KADUOX_MACOS_SIGNING_IDENTITY" "$path"
  codesign --verify --strict --verbose=2 "$path"
done

mkdir -m 700 "$notary_dir"
for binary in "${binaries[@]}"; do
  cp -p "$binary_dir/$binary" "$notary_dir/$binary"
done

ditto -c -k --keepParent "$notary_dir" "$notary_zip"
xcrun notarytool submit "$notary_zip" \
  --key "$notary_key" \
  --key-id "$KADUOX_APPLE_NOTARY_KEY_ID" \
  --issuer "$KADUOX_APPLE_NOTARY_ISSUER_ID" \
  --wait \
  --output-format json >"$notary_result"

python - "$notary_result" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
try:
    payload = json.loads(path.read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError) as exc:
    raise SystemExit(f"cannot parse notarytool result: {exc}")
status = payload.get("status")
if status != "Accepted":
    raise SystemExit(f"Apple notarization did not return Accepted status: {status!r}")
submission_id = payload.get("id")
if not isinstance(submission_id, str) or not submission_id:
    raise SystemExit("Apple notarization result is missing a submission id")
print(f"Apple notarization accepted submission {submission_id}")
PY

for binary in "${binaries[@]}"; do
  codesign --verify --strict --verbose=2 "$binary_dir/$binary"
done
