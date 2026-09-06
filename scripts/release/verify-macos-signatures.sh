#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <published-binary-directory>" >&2
  exit 2
fi

binary_dir="$(cd "$1" && pwd -P)"
binaries=(kssh kssh-tui kssh-fleet kssh-inventory)
for binary in "${binaries[@]}"; do
  path="$binary_dir/$binary"
  [[ -f "$path" && ! -L "$path" ]] || {
    echo "published macOS binary is missing or link-like: $path" >&2
    exit 2
  }
  codesign --verify --strict --verbose=2 "$path"
  codesign -dvv "$path" 2>&1 | grep -F 'Timestamp=' >/dev/null
  spctl -vvv --assess --type exec "$path"
done
