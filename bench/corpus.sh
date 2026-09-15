#!/usr/bin/env bash
# Benchmark the pinned public compat corpus (compat/corpus.toml) with
# composer, riff, viv and vivacity, appending rows to bench/results/corpus.md.
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
# COMPAT_CORPUS (corpus.toml path), VIV/RIFF/VIVACITY (binaries under test;
# vivacity, like riff, is optional — its absence is skipped, not a failure).
#
# #170: each project's mirror (BENCH_MIRROR=1) and, for a version-only entry,
# its generated composer.lock live under BENCH_CACHE (default
# ${XDG_CACHE_HOME:-$HOME/.cache}/vivace-bench), keyed by project name and
# pinned commit/version, so a second run reuses both instead of paying
# setup again. BENCH_CORPUS_WORK stays genuinely per-run scratch (checkouts,
# bench/run.sh's own work dir); see bench/results/README.md for how to
# clear the cache.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
corpus=${COMPAT_CORPUS:-$root/compat/corpus.toml}
viv_bin=${VIV:-$root/target/release/viv}
riff_bin=${RIFF:-riff}
vivacity_bin=${VIVACITY:-vivacity}
runs=${BENCH_RUNS:-3}
only=${1:-}
report=${BENCH_CORPUS_REPORT:-$root/bench/results/corpus.md}
cache_root=${BENCH_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/vivace-bench}
mirror_root="$cache_root/mirror"
mkdir -p "$mirror_root"

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

# Last non-empty line of $1, for a one-line failure footnote. `|| true`
# because `grep -v` exits 1 when $1 is all-blank, and pipefail would
# otherwise propagate that through `set -e` and kill the whole script.
last_line() { grep -v '^$' <<< "$1" | tail -1 | redact || true; }

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
problem=0

# Appends this project's footnotes to $report right after its own rows
# (rather than buffering to the end of the run). `problem` (the exit-code
# trip switch, #220) is set by each call site that appends a genuine,
# unresolved failure, not here: a bench/skips.txt-known skip is still worth
# a footnote so a reader can see what was skipped, but it's expected and
# must not fail a release-cut run on its own. Bullets are prefixed "$name"
# or "$name/$tool", which is unique enough across the whole run without a
# separate numbering scheme.
flush_footnotes() {
  [ ${#footnotes[@]} -eq 0 ] && return 0
  {
    echo ""
    for f in "${footnotes[@]}"; do
      echo "- $f"
    done
  } >> "$report"
  footnotes=()
}

# Clones or builds one corpus entry into $work/src/<safe>, generates a lock
# if missing, benchmarks it, then deletes its work dir (disk: one project at
# a time, see bench/corpus.sh's brief).
run_entry() {
  local name=$1 repo=$2 commit=$3 version=$4
  local safe srcdir key
  safe=$(tr '/' '_' <<< "$name")
  # #170: keys this project's persistent mirror/lock cache by pin, so a
  # commit or version bump in corpus.toml starts a fresh cache entry rather
  # than reusing a stale one.
  key=$(tr '/' '_' <<< "${commit:-$version}")
  srcdir="$work/src/$safe"
  rm -rf "$srcdir"
  footnotes=()

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
      problem=1
      flush_footnotes
      return
    fi
  else
    log "skipping $name (no repo/commit or version pin)"
    return
  fi

  if [ ! -f "$srcdir/composer.lock" ]; then
    # #170: a version-only entry's lock is fully determined by its pin, so
    # it's cached next to the mirror and generated once per pin rather than
    # once per run.
    local cached_lock="$mirror_root/$safe-$key/generated-composer.lock"
    if [ -f "$cached_lock" ]; then
      log "reusing cached generated lock for $name"
      cp "$cached_lock" "$srcdir/composer.lock"
    else
      log "generating lock for $name"
      local lock_out
      if ! lock_out=$(composer -d "$srcdir" update --no-install --no-scripts --no-plugins \
          --ignore-platform-reqs 2>&1); then
        footnotes+=("$name: composer update (lock) failed: $(last_line "$lock_out")")
        problem=1
        flush_footnotes
        rm -rf "$srcdir"
        return
      fi
      mkdir -p "$(dirname "$cached_lock")"
      cp "$srcdir/composer.lock" "$cached_lock"
    fi
  fi

  bench_project "$name" "$srcdir" "$key"
  flush_footnotes
  rm -rf "$srcdir"
}

# Runs bench/run.sh for one project against composer/riff/viv, then appends
# one report row per tool.
bench_project() {
  local name=$1 srcdir=$2 key=$3
  local safe out packages run_out tool
  safe=$(tr '/' '_' <<< "$name")
  packages=$(jq -r '((.packages // []) | length) + ((."packages-dev" // []) | length)' \
    "$srcdir/composer.lock")
  out="$work/out-$safe"
  rm -rf "$out" "$work/bench"
  mkdir -p "$out"

  local skip_tools skip_update skip_offline run_tools
  skips_for "$name"
  run_tools="composer riff viv vivacity"
  for tool in $skip_tools; do
    run_tools=$(sed "s/\b$tool\b//" <<< "$run_tools")
  done

  # BENCH_MIRROR=1 (#165, widened): record this project's dists and p2
  # metadata once, and forward the recording to bench/run.sh so cold and
  # update-warm run against it instead of the real network. #170: the
  # mirror lives under the persistent cache, keyed by pin, not $work, so a
  # later run reuses it instead of recording again.
  local mirror_dir=""
  if [ "${BENCH_MIRROR:-}" = "1" ]; then
    mirror_dir="$mirror_root/$safe-$key"
    log "recording mirror for $name"
    "$root/bench/mirror.sh" "$srcdir" "$mirror_dir"
  fi

  log "benchmarking $name ($packages packages, $runs runs)"
  if ! run_out=$(BENCH_FLAGS="--no-plugins --no-scripts" BENCH_RUNS="$runs" \
      BENCH_WORK="$work/bench" BENCH_OUT="$out" BENCH_SKIP_UPDATE="$skip_update" \
      BENCH_MIRROR="$mirror_dir" \
      "$root/bench/run.sh" "$srcdir" $run_tools 2>&1); then
    log "bench/run.sh reported a problem for $name (see footnote per affected tool)"
  fi

  for tool in composer riff viv vivacity; do
    local cold_json update_json cold warm noop upd
    cold_json="$out/$tool.json"
    update_json="$out/$tool-update.json"
    cold=$(mean_for "$cold_json" "$tool cold")
    warm=$(mean_for "$cold_json" "$tool warm")
    noop=$(mean_for "$cold_json" "$tool noop")
    upd=$(mean_for "$update_json" "$tool update-warm")
    echo "| $name | $packages | $tool | $(fmt "$cold") | $(fmt "$warm") | $(fmt "$noop") | $(fmt "$upd") |" \
      >> "$report"
    case " $skip_tools " in *" $tool "*) continue ;; esac
    if [ -z "$cold" ]; then
      local reason
      # `|| true`: an unmatched grep exits 1, and pipefail would otherwise
      # propagate that through `set -e` and kill the whole script (#106).
      # `\b` boundaries: "viv" is a substring of "vivacity", so a plain
      # `grep -i "$tool"` would pick up vivacity's log lines while looking
      # for viv's failure reason (and vice versa isn't possible, but the
      # boundary costs nothing either way).
      reason=$(grep -iE "\b$tool\b" <<< "$run_out" | grep -iE 'error|fail|warn' | tail -1 || true)
      [ -n "$reason" ] || reason=$(last_line "$run_out")
      footnotes+=("$name/$tool: $(redact <<< "$reason")")
      problem=1
    elif [ -z "$upd" ]; then
      case " $skip_update " in
        *" $tool "*) : ;;  # already footnoted by skips_for
        *)
          local reason
          reason=$(grep -iE "\b$tool\b" <<< "$run_out" | grep -iE 'error|fail|warn' | tail -1 || true)
          [ -n "$reason" ] || reason=$(last_line "$run_out")
          footnotes+=("$name/$tool: $(redact <<< "$reason")")
          problem=1
          ;;
      esac
    elif [ "${BENCH_MIRROR:-}" = "1" ] && [ "$tool" = "viv" ] \
        && [ -z "$(mean_for "$out/$tool-update-offline.json" "$tool update-offline")" ]; then
      # Only with BENCH_MIRROR=1: without a recorded mirror there is no
      # offline source to read, so a missing cell is the run's shape rather
      # than a fault, and failing on it makes a mirrorless corpus run
      # impossible.
      #
      # update-offline has no column of its own in the table above (#165
      # added it as a viv-only, informational-only number, see
      # bench/compare.py), so a mirror that can't serve it — #216 — used to
      # leave no trace at all here: cold and update-warm both had numbers,
      # so nothing above footnoted it, and bench/run.sh's own non-zero exit
      # (it does fail on this, see its own trailing `if [ -n "$failed" ]`)
      # only ever reached a log line, never this report or its exit code.
      case " $skip_offline " in
        *" $tool "*) : ;; # already footnoted by skips_for
        *)
          local reason
          reason=$(grep -iE "\b$tool\b" <<< "$run_out" | grep -iE 'error|fail|warn' | tail -1 || true)
          [ -n "$reason" ] || reason=$(last_line "$run_out")
          footnotes+=("$name/$tool: $(redact <<< "$reason")")
          problem=1
          ;;
      esac
    fi
  done

  rm -rf "$out" "$work/bench"
}

ver_from() { "$@" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1; }
skips=${BENCH_SKIPS:-$root/bench/skips.txt}

# Known-failure skips (bench/skips.txt): matches on tool+version+project, so
# a newer release is retried automatically. Sets $skip_tools (space-
# separated tools to drop from bench/run.sh's tool list), $skip_update
# (forwarded as BENCH_SKIP_UPDATE) and $skip_offline (checked below, viv's
# update-offline has no BENCH_ flag of its own to forward to) for one
# project, and appends a footnote per skip.
skips_for() {
  local name=$1
  skip_tools=""
  skip_update=""
  skip_offline=""
  # vivacity, unlike composer/riff/viv, isn't assumed installed on every
  # machine running this corpus: a missing binary is recorded here as a
  # skip, same shape as a known bench/skips.txt failure, rather than left
  # to fail bench/run.sh's own attempt (#220: a competitor's absence is
  # data, not a fault in this repo).
  if ! command -v "$vivacity_bin" >/dev/null 2>&1; then
    skip_tools="$skip_tools vivacity"
    footnotes+=("$name/vivacity: binary not found, skipped")
  fi
  [ -f "$skips" ] || return 0
  local tool ver proj scenario tool_ver
  while read -r tool ver proj scenario; do
    [ -z "$tool" ] && continue
    case $tool in \#*) continue ;; esac
    [ "$proj" = "$name" ] || continue
    tool_ver=$(ver_from "$(tool_bin "$tool")")
    [ "$tool_ver" = "$ver" ] || continue
    case $scenario in
      install)
        skip_tools="$skip_tools $tool"
        footnotes+=("$name/$tool: skipped, known failure at $tool $ver (bench/skips.txt)")
        ;;
      update-warm)
        skip_update="$skip_update $tool"
        footnotes+=("$name/$tool: skipped, known failure at $tool $ver (bench/skips.txt)")
        ;;
      update-offline)
        skip_offline="$skip_offline $tool"
        footnotes+=("$name/$tool: skipped, known failure at $tool $ver (bench/skips.txt)")
        ;;
    esac
  done < <(grep -v '^ *#' "$skips" | grep -v '^ *$')
}

tool_bin() {
  case $1 in
    composer) echo composer ;;
    riff) echo "$riff_bin" ;;
    viv) echo "$viv_bin" ;;
    vivacity) echo "$vivacity_bin" ;;
  esac
}

{
  echo ""
  echo "## $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo ""
  echo "viv $(ver_from "$viv_bin"), composer $(ver_from composer), riff $(ver_from "$riff_bin")," \
    "vivacity $(ver_from "$vivacity_bin"), flags \`--no-plugins --no-scripts\`, $runs runs each$( \
      [ "${BENCH_MIRROR:-}" = "1" ] && echo ", from a local mirror")."
  echo ""
  echo "| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |"
  echo "|---|---|---|---|---|---|---|"
} >> "$report"

while IFS='|' read -r name repo commit version _path; do
  wanted "$name" || continue
  run_entry "$name" "$repo" "$commit" "$version"
done < <(parse_corpus)

log "report written to $report"
if [ "$problem" -ne 0 ]; then
  log "one or more projects had a problem; see footnotes in $report"
  exit 1
fi
