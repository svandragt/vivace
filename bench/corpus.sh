#!/usr/bin/env bash
# Benchmark the pinned public compat corpus (compat/corpus.toml) with
# composer, riff and viv, appending rows to bench/results/corpus.md.
# See bench/run.sh for the cold/warm/noop/update-warm scenarios this reuses,
# and bench/results/README.md for the single-project (bench/laravel) numbers
# this complements with the wider corpus #106 asked for.
#
# Usage: bench/corpus.sh [project-name,...]
#   Default (no argument): every repo/commit entry in compat/corpus.toml,
#   plus any version-only entry (e.g. drupal/recommended-project) that
#   builds with `composer create-project --no-install`, same as
#   compat/run.sh. A project without a lock gets one generated with
#   `composer update --no-install --no-scripts --no-plugins
#   --ignore-platform-reqs`.
#
# Run inside devbox (`devbox run -- bench/corpus.sh`) so php/composer/
# hyperfine resolve. Env: BENCH_RUNS (default 3, forwarded to bench/run.sh),
# BENCH_CORPUS_WORK (scratch dir, default a removed-on-exit mktemp),
# COMPAT_CORPUS (corpus.toml path), VIV/RIFF (binaries under test).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
corpus=${COMPAT_CORPUS:-$root/compat/corpus.toml}
viv_bin=${VIV:-$root/target/release/viv}
riff_bin=${RIFF:-riff}
runs=${BENCH_RUNS:-3}
only=${1:-}
report=${BENCH_CORPUS_REPORT:-$root/bench/results/corpus.md}

if [ -n "${BENCH_CORPUS_WORK:-}" ]; then
  work=$BENCH_CORPUS_WORK
else
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
fi
mkdir -p "$work/src"

IFS=',' read -r -a only_list <<< "$only"
wanted() {
  [ ${#only_list[@]} -eq 0 ] && return 0
  local name=$1 w
  for w in "${only_list[@]}"; do [ "$w" = "$name" ] && return 0; done
  return 1
}

log() { echo "corpus: $*" >&2; }

# Redacts anything token/credential-shaped before it reaches the committed
# report (same spirit as compat/run.sh's log handling, belt and braces here
# since this file gets committed straight into results/).
redact() { sed -E 's/(token|password|authorization)[=: ]+[^ ]+/\1=[redacted]/Ig'; }

# Last non-empty line of $1, for a one-line failure footnote.
last_line() { grep -v '^$' <<< "$1" | tail -1 | redact; }

# corpus.toml line parser, copied from compat/run.sh's parse_corpus (kept in
# sync by hand; see compat/run.sh if this schema changes).
parse_corpus() {
  awk '
    /^\[\[project\]\]/ { if (name != "") print name "|" repo "|" commit "|" version "|" path
                          name = ""; repo = ""; commit = ""; version = ""; path = ""; next }
    /^name *=/    { v = $0; sub(/^name *= *"/, "", v); sub(/" *$/, "", v); name = v }
    /^repo *=/    { v = $0; sub(/^repo *= *"/, "", v); sub(/" *$/, "", v); repo = v }
    /^commit *=/  { v = $0; sub(/^commit *= *"/, "", v); sub(/" *$/, "", v); commit = v }
    /^version *=/ { v = $0; sub(/^version *= *"/, "", v); sub(/" *$/, "", v); version = v }
    /^path *=/    { v = $0; sub(/^path *= *"/, "", v); sub(/" *$/, "", v); path = v }
    END { if (name != "") print name "|" repo "|" commit "|" version "|" path }
  ' "$corpus"
}

# Mean seconds for command $2 in hyperfine export $1, empty if either is
# missing (tool failed or didn't run).
mean_for() {
  local json=$1 name=$2
  [ -f "$json" ] || return 0
  jq -r --arg n "$name" '.results[] | select(.command == $n) | .mean' "$json" 2>/dev/null || true
}

# $1 seconds or empty -> "n/a" or 3-decimal seconds.
fmt() { [ -n "$1" ] && printf '%.3f\n' "$1" || echo "n/a"; }

footnotes=()

# Clones or builds one corpus entry into $work/src/<safe>, generates a lock
# if missing, benchmarks it, then deletes its work dir (disk: one project at
# a time, see bench/corpus.sh's brief).
run_entry() {
  local name=$1 repo=$2 commit=$3 version=$4
  local safe srcdir
  safe=$(tr '/' '_' <<< "$name")
  srcdir="$work/src/$safe"
  rm -rf "$srcdir"

  if [ -n "$repo" ]; then
    log "cloning $name @ $commit"
    git clone --quiet "$repo" "$srcdir"
    git -C "$srcdir" checkout --quiet "$commit"
    rm -rf "$srcdir/.git"
  elif [ -n "$version" ]; then
    log "create-project $name $version"
    local create_out
    if ! create_out=$(composer create-project --no-install --no-scripts --no-interaction \
        --ignore-platform-reqs "$name" "$srcdir" "$version" 2>&1); then
      footnotes+=("$name: create-project failed: $(last_line "$create_out")")
      return
    fi
  else
    log "skipping $name (no repo/commit or version pin)"
    return
  fi

  if [ ! -f "$srcdir/composer.lock" ]; then
    log "generating lock for $name"
    local lock_out
    if ! lock_out=$(composer -d "$srcdir" update --no-install --no-scripts --no-plugins \
        --ignore-platform-reqs 2>&1); then
      footnotes+=("$name: composer update (lock) failed: $(last_line "$lock_out")")
      rm -rf "$srcdir"
      return
    fi
  fi

  bench_project "$name" "$srcdir"
  rm -rf "$srcdir"
}

# Runs bench/run.sh for one project against composer/riff/viv, then appends
# one report row per tool.
bench_project() {
  local name=$1 srcdir=$2
  local safe out packages run_out tool
  safe=$(tr '/' '_' <<< "$name")
  packages=$(jq -r '((.packages // []) | length) + ((."packages-dev" // []) | length)' \
    "$srcdir/composer.lock")
  out="$work/out-$safe"
  rm -rf "$out" "$work/bench"
  mkdir -p "$out"

  log "benchmarking $name ($packages packages, $runs runs)"
  if ! run_out=$(BENCH_FLAGS="--no-plugins --no-scripts" BENCH_RUNS="$runs" \
      BENCH_WORK="$work/bench" BENCH_OUT="$out" \
      "$root/bench/run.sh" "$srcdir" composer riff viv 2>&1); then
    log "bench/run.sh reported a problem for $name (see footnote per affected tool)"
  fi

  for tool in composer riff viv; do
    local cold_json update_json cold warm noop upd
    cold_json="$out/$tool.json"
    update_json="$out/$tool-update.json"
    cold=$(mean_for "$cold_json" "$tool cold")
    warm=$(mean_for "$cold_json" "$tool warm")
    noop=$(mean_for "$cold_json" "$tool noop")
    upd=$(mean_for "$update_json" "$tool update-warm")
    echo "| $name | $packages | $tool | $(fmt "$cold") | $(fmt "$warm") | $(fmt "$noop") | $(fmt "$upd") |" \
      >> "$report"
    if [ -z "$cold" ] || [ -z "$upd" ]; then
      local reason
      reason=$(grep -i "$tool" <<< "$run_out" | grep -iE 'error|fail|warn' | tail -1)
      [ -n "$reason" ] || reason=$(last_line "$run_out")
      footnotes+=("$name/$tool: $(redact <<< "$reason")")
    fi
  done

  rm -rf "$out" "$work/bench"
}

ver_from() { "$@" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1; }

{
  echo ""
  echo "## $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo ""
  echo "viv $(ver_from "$viv_bin"), composer $(ver_from composer), riff $(ver_from "$riff_bin")," \
    "flags \`--no-plugins --no-scripts\`, $runs runs each."
  echo ""
  echo "| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |"
  echo "|---|---|---|---|---|---|---|"
} >> "$report"

while IFS='|' read -r name repo commit version _path; do
  wanted "$name" || continue
  run_entry "$name" "$repo" "$commit" "$version"
done < <(parse_corpus)

if [ ${#footnotes[@]} -gt 0 ]; then
  {
    echo ""
    for f in "${footnotes[@]}"; do
      echo "- $f"
    done
  } >> "$report"
fi

log "report written to $report"
