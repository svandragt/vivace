#!/usr/bin/env bash
# One-off: diff riff's installed vendor/ against Composer's (and note viv's)
# across the pinned corpus (compat/corpus.toml). Investigates #142; not a
# permanent sweep column.
#
# For each corpus project with a committed lock or one `composer update
# --no-install --no-scripts --no-plugins --ignore-platform-reqs` can
# generate: clone/build it, install with composer, riff and viv
# (--no-scripts --no-plugins, plus --ignore-platform-reqs where the tool
# supports it), then `diff -rq --exclude=.git` riff's vendor/ against
# Composer's. Prints one Markdown table row per project to stdout.
#
# Run inside devbox: `devbox run -- bench/riff-diff.sh`.
# Env: RIFF_DIFF_WORK (scratch dir, default a removed-on-exit mktemp),
# COMPAT_CORPUS (corpus.toml path), VIV/RIFF (binaries under test).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
corpus=${COMPAT_CORPUS:-$root/compat/corpus.toml}
viv_bin=${VIV:-$root/target/release/viv}
riff_bin=${RIFF:-riff}
cap=${RIFF_DIFF_CAP:-15}

if [ -n "${RIFF_DIFF_WORK:-}" ]; then
  work=$RIFF_DIFF_WORK
  mkdir -p "$work"
else
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
fi

log() { echo "riff-diff: $*" >&2; }

# corpus.toml line parser, copied from bench/corpus.sh's parse_corpus.
parse_corpus() {
  awk '
    /^\[\[project\]\]/ { if (name != "") print name "|" repo "|" commit "|" version
                          name = ""; repo = ""; commit = ""; version = ""; next }
    /^name *=/    { v = $0; sub(/^name *= *"/, "", v); sub(/" *$/, "", v); name = v }
    /^repo *=/    { v = $0; sub(/^repo *= *"/, "", v); sub(/" *$/, "", v); repo = v }
    /^commit *=/  { v = $0; sub(/^commit *= *"/, "", v); sub(/" *$/, "", v); commit = v }
    /^version *=/ { v = $0; sub(/^version *= *"/, "", v); sub(/" *$/, "", v); version = v }
    END { if (name != "") print name "|" repo "|" commit "|" version }
  ' "$corpus"
}

echo "| Project | Result |"
echo "|---|---|"

while IFS='|' read -r name repo commit version; do
  safe=$(tr '/' '_' <<< "$name")
  src="$work/src-$safe"
  rm -rf "$src"

  if [ -n "$repo" ]; then
    log "cloning $name @ $commit (shallow)"
    mkdir -p "$src"
    git -C "$src" init --quiet
    git -C "$src" remote add origin "$repo"
    git -C "$src" fetch --quiet --depth 1 origin "$commit"
    git -C "$src" checkout --quiet FETCH_HEAD
    rm -rf "$src/.git"
  elif [ -n "$version" ]; then
    log "create-project $name $version"
    if ! composer create-project --no-install --no-scripts --no-interaction \
        --ignore-platform-reqs "$name" "$src" "$version" >"$work/create-$safe.log" 2>&1; then
      echo "| $name | create-project failed: $(tail -1 "$work/create-$safe.log") |"
      continue
    fi
  else
    continue
  fi

  if [ ! -f "$src/composer.lock" ]; then
    log "generating lock for $name"
    if ! composer -d "$src" update --no-install --no-scripts --no-plugins \
        --ignore-platform-reqs >"$work/lock-$safe.log" 2>&1; then
      echo "| $name | no committed lock, and \`composer update --no-install\` failed: $(tail -1 "$work/lock-$safe.log") |"
      rm -rf "$src"
      continue
    fi
  fi

  composer_dir="$work/composer-$safe"
  riff_dir="$work/riff-$safe"
  viv_dir="$work/viv-$safe"
  rm -rf "$composer_dir" "$riff_dir" "$viv_dir"
  cp -a "$src" "$composer_dir"
  cp -a "$src" "$riff_dir"
  cp -a "$src" "$viv_dir"
  rm -rf "$src"

  log "composer install ($name)"
  composer_out=$(cd "$composer_dir" && composer install --no-scripts --no-plugins \
    --no-interaction --ignore-platform-reqs 2>&1) || {
    echo "| $name | composer install failed: $(tail -1 <<< "$composer_out") |"
    rm -rf "$composer_dir" "$riff_dir" "$viv_dir"
    continue
  }

  log "riff install ($name)"
  riff_rc=0
  riff_out=$(cd "$riff_dir" && "$riff_bin" install --no-interaction --no-scripts \
    --no-plugins --ignore-platform-reqs 2>&1) || riff_rc=$?

  log "viv install ($name)"
  viv_rc=0
  viv_out=$(cd "$viv_dir" && "$viv_bin" install --no-scripts --no-plugins 2>&1) || viv_rc=$?

  result=""
  if [ "$riff_rc" -ne 0 ]; then
    result="riff install exited $riff_rc: $(tail -1 <<< "$riff_out")"
  else
    diff_out=$(diff -rq --exclude=.git "$riff_dir/vendor" "$composer_dir/vendor" 2>&1 \
      | sed "s#$riff_dir/#riff/#g; s#$composer_dir/#composer/#g" || true)
    if [ -z "$diff_out" ]; then
      result="identical"
    else
      n=$(wc -l <<< "$diff_out")
      result="$(head -"$cap" <<< "$diff_out" | tr '\n' ';' | sed 's/;/; /g')"
      [ "$n" -gt "$cap" ] && result="$result … ($n lines total, capped at $cap)"
    fi
  fi
  if [ "$viv_rc" -ne 0 ]; then
    result="$result / viv install exited $viv_rc: $(tail -1 <<< "$viv_out")"
  fi

  echo "| $name | $result |"
  rm -rf "$composer_dir" "$riff_dir" "$viv_dir"
done < <(parse_corpus)
