#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
target="${1:?usage: generate-third-party-licenses.sh TARGET OUTPUT}"
output="${2:?usage: generate-third-party-licenses.sh TARGET OUTPUT}"

case "$target" in
  aarch64-apple-darwin | x86_64-apple-darwin | x86_64-pc-windows-msvc | x86_64-unknown-linux-gnu) ;;
  *)
    echo "unsupported release target: $target" >&2
    exit 1
    ;;
esac

LC_ALL=C cargo about generate \
  --config "$root/.github/licenses/about.toml" \
  --manifest-path "$root/crates/tauri-wd/Cargo.toml" \
  --target "$target" \
  --frozen \
  --fail \
  --output-file "$output" \
  "$root/.github/licenses/third-party.hbs"

printf '\n---\n\n' >>"$output"
cat "$root/crates/tauri-wd/THIRD_PARTY_LICENSES.md" >>"$output"
test -s "$output"
