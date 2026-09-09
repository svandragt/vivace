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
# (rather than buffering to the end of the run), and flags the run as
# having had a problem — one trip switch whether the footnote came from a
# create-project/lock failure or an n/a bench row. Bullets are prefixed
# "$name" or "$name/$tool", which is unique enough across the whole run
# without a separate numbering scheme.
flush_footnotes() {
  [ ${#footnotes[@]} -eq 0 ] && return 0
  problem=1
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
  local safe srcdir
  safe=$(tr '/' '_' <<< "$name")
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
      flush_footnotes
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
      flush_footnotes
      rm -rf "$srcdir"
      return
    fi
  fi

  bench_project "$name" "$srcdir"
  flush_footnotes
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

  local skip_tools skip_update run_tools
  skips_for "$name"
  run_tools="composer riff viv"
  for tool in $skip_tools; do
    run_tools=$(sed "s/\b$tool\b//" <<< "$run_tools")
  done

  # BENCH_MIRROR=1 (#165, widened): record this project's dists and p2
  # metadata once, and forward the recording to bench/run.sh so cold and
  # update-warm run against it instead of the real network.
  local mirror_dir=""
  if [ "${BENCH_MIRROR:-}" = "1" ]; then
    mirror_dir="$work/mirror/$safe"
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
    case " $skip_tools " in *" $tool "*) continue ;; esac
    if [ -z "$cold" ]; then
      local reason
      # `|| true`: an unmatched grep exits 1, and pipefail would otherwise
      # propagate that through `set -e` and kill the whole script (#106).
      reason=$(grep -i "$tool" <<< "$run_out" | grep -iE 'error|fail|warn' | tail -1 || true)
      [ -n "$reason" ] || reason=$(last_line "$run_out")
      footnotes+=("$name/$tool: $(redact <<< "$reason")")
    elif [ -z "$upd" ]; then
      case " $skip_update " in
        *" $tool "*) : ;;  # already footnoted by skips_for
        *)
          local reason
          reason=$(grep -i "$tool" <<< "$run_out" | grep -iE 'error|fail|warn' | tail -1 || true)
          [ -n "$reason" ] || reason=$(last_line "$run_out")
          footnotes+=("$name/$tool: $(redact <<< "$reason")")
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
# separated tools to drop from bench/run.sh's tool list) and $skip_update
# (forwarded as BENCH_SKIP_UPDATE) for one project, and appends a footnote
# per skip.
skips_for() {
  local name=$1
  skip_tools=""
  skip_update=""
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
    esac
  done < <(grep -v '^ *#' "$skips" | grep -v '^ *$')
}

tool_bin() {
  case $1 in
    composer) echo composer ;;
    riff) echo "$riff_bin" ;;
    viv) echo "$viv_bin" ;;
  esac
}

{
  echo ""
  echo "## $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo ""
  echo "viv $(ver_from "$viv_bin"), composer $(ver_from composer), riff $(ver_from "$riff_bin")," \
    "flags \`--no-plugins --no-scripts\`, $runs runs each$( \
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
