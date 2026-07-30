#!/usr/bin/env bash
set -euo pipefail

tag="${1:?usage: generate-changelog.sh TAG [OUTPUT]}"
output="${2:-/dev/stdout}"

if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "invalid release tag: $tag" >&2
  exit 1
fi

git rev-parse --verify "${tag}^{commit}" >/dev/null

previous="$(
  git tag --merged "$tag" --sort=-version:refname \
    | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' \
    | grep -Fvx "$tag" \
    | head -n 1 \
    || true
)"

if [ -n "$previous" ]; then
  range="${previous}..${tag}"
else
  range="$tag"
fi

features=""
fixes=""
performance=""
documentation=""
maintenance=""
other=""

strip_prefix() {
  printf '%s\n' "$1" | sed -E 's/^[a-z]+(\([^)]*\))?!?: //'
}

while IFS= read -r message; do
  [ -z "$message" ] && continue
  case "$message" in
    "docs: changelog for v"*|"docs: update changelog for v"*)
      continue
      ;;
    feat\(*\):*|feat:*|feat\(*\)!:*|feat!:*)
      features="${features}- $(strip_prefix "$message")"$'\n'
      ;;
    fix\(*\):*|fix:*|fix\(*\)!:*|fix!:*)
      fixes="${fixes}- $(strip_prefix "$message")"$'\n'
      ;;
    perf\(*\):*|perf:*|perf\(*\)!:*|perf!:*)
      performance="${performance}- $(strip_prefix "$message")"$'\n'
      ;;
    docs\(*\):*|docs:*|docs\(*\)!:*|docs!:*)
      documentation="${documentation}- $(strip_prefix "$message")"$'\n'
      ;;
    build:*|build\(*\):*|ci:*|ci\(*\):*|chore:*|chore\(*\):*|refactor:*|refactor\(*\):*|test:*|test\(*\):*)
      maintenance="${maintenance}- $(strip_prefix "$message")"$'\n'
      ;;
    *)
      other="${other}- ${message}"$'\n'
      ;;
  esac
done < <(git log --pretty=tformat:"%s" "$range" --no-merges)

version="${tag#v}"
release_date="$(git show -s --format=%cs "$tag")"

{
  printf '## %s (%s)\n\n' "$version" "$release_date"
  [ -n "$features" ] && printf '### Features\n\n%s\n' "$features"
  [ -n "$fixes" ] && printf '### Fixes\n\n%s\n' "$fixes"
  [ -n "$performance" ] && printf '### Performance\n\n%s\n' "$performance"
  [ -n "$documentation" ] && printf '### Documentation\n\n%s\n' "$documentation"
  [ -n "$maintenance" ] && printf '### Maintenance\n\n%s\n' "$maintenance"
  [ -n "$other" ] && printf '### Other\n\n%s\n' "$other"
  :
} >"$output"
