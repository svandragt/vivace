#!/usr/bin/env bash
# Rewrites compat/corpus.toml `commit` pins to each project's current
# default-branch head. Run via `make compat-refresh`, then commit the diff.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
corpus="$root/compat/corpus.toml"

name="" repo=""
tmp=$(mktemp)
while IFS= read -r line; do
  case "$line" in
    '[[project]]'*) name=""; repo="" ;;
    'name = "'*) name=$(sed -E 's/^name = "(.*)"$/\1/' <<< "$line") ;;
    'repo = "'*) repo=$(sed -E 's/^repo = "(.*)"$/\1/' <<< "$line") ;;
  esac
  if [[ "$line" =~ ^commit\ =\ \" ]] && [ -n "$repo" ]; then
    head=$(git ls-remote --exit-code "$repo" HEAD | cut -f1)
    echo "compat: $name -> $head" >&2
    echo "commit = \"$head\""
  else
    echo "$line"
  fi
done < "$corpus" > "$tmp"
mv "$tmp" "$corpus"
