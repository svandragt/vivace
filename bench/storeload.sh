#!/usr/bin/env bash
# Chapter 2 boot harness (#287): control arm only. Measures the compat
# corpus (compat/corpus.toml) under viv's default, byte-compatible install —
# the store-path/no-vendor-tree arm (#284, #285) doesn't exist yet, so
# there's nothing to put in a second column. Appends rows to
# bench/results/storeload.md. See docs/research.md chapter 2 for the
# question and bench/results/storeload.md for the write-up.
#
# Per project: warm/cold install wall time, `linkat` count under
# `strace -c -f` (skipped, with a reason, where strace is unavailable),
# inode and directory-entry count of vendor/ (not bytes — see the chapter:
# under the default hardlink mode vendor/ shares inodes with the store, so a
# bytes figure would show a saving that isn't there), and whether the
# project boots — its own test suite or console entry point, whichever it
# has (see boot_entry_for below) — after a plain `viv install`.
#
# Usage: bench/storeload.sh [project-name,...]
# Run inside devbox (`devbox run -- bench/storeload.sh`) so php/composer/
# hyperfine/strace resolve. Env: BENCH_RUNS (default 3, forwarded to
# bench/run.sh), BENCH_CACHE (persistent checkout/mirror/lock cache,
# default ${XDG_CACHE_HOME:-$HOME/.cache}/vivace-bench, shared with
# bench/corpus.sh), COMPAT_CORPUS (corpus.toml path), VIV (binary under
# test).
#
# #271: bench/corpus.sh deletes its checkout at the end of a run, so a
# repeat profile of anything but laravel starts with a fresh clone. This
# script works around that on its own by keeping checkouts under the
# persistent cache (see park show 259), not by fixing #271 — a second run
# of this script reuses its own checkout even though bench/corpus.sh still
# doesn't reuse its.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
corpus=${COMPAT_CORPUS:-$root/compat/corpus.toml}
viv_bin=${VIV:-$root/target/release/viv}
runs=${BENCH_RUNS:-3}
only=${1:-}
report=${BENCH_STORELOAD_REPORT:-$root/bench/results/storeload.md}
cache_root=${BENCH_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/vivace-bench}
mirror_root="$cache_root/mirror"
checkout_root="$cache_root/checkouts"
mkdir -p "$mirror_root" "$checkout_root"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

IFS=',' read -r -a only_list <<< "$only"
wanted() {
  [ ${#only_list[@]} -eq 0 ] && return 0
  local name=$1 w
  for w in "${only_list[@]}"; do [ "$w" = "$name" ] && return 0; done
  return 1
}

log() { echo "storeload: $*" >&2; }
redact() { sed -E 's/(token|password|authorization)[=: ]+[^ ]+/\1=[redacted]/Ig'; }
last_line() { grep -v '^$' <<< "$1" | tail -1 | redact || true; }

# Kept in sync by hand with bench/corpus.sh's own copy; see that file if
# this schema changes.
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

mean_for() {
  local json=$1 name=$2
  [ -f "$json" ] || return 0
  jq -r --arg n "$name" '.results[] | select(.command == $n) | .mean' "$json" 2>/dev/null || true
}
fmt() { [ -n "$1" ] && printf '%.3f\n' "$1" || echo "n/a"; }

# One boot entry per pinned corpus project, hand-picked from what each
# project actually ships (checked by hand against a real viv install —
# `php --version`'s own PHP exit code is unreliable here: an uncaught
# fatal error still exits 0 under this devbox's PHP build, so boot_check()
# below also greps the output, not just the exit code). Format:
# "kind|relative-path|args". kind is "console" or "test" for the boot
# column's footnote; "none" means the project ships neither without extra
# setup (settings.php + a database, for Drupal) — recorded as a real
# result, not a harness gap.
boot_entry_for() {
  case "$1" in
    laravel/laravel) echo "console|artisan|--version" ;;
    # symfony/demo requires the symfony/runtime plugin to generate
    # vendor/autoload_runtime.php; every project here installs with
    # --no-plugins (see below), so bin/console is expected to fail here —
    # that's the recorded result, not a bug in the entry choice.
    symfony/demo) echo "console|bin/console|--version" ;;
    # Drupal's recommended-project ships no console or test entry that
    # runs without settings.php and a live database (drush isn't a
    # dependency, and core's own test suite needs a configured site even
    # for most groups) — there is nothing to boot pre-install.
    drupal/recommended-project) echo "none||" ;;
    # No console script; pestphp/pest is a require-dev.
    roots/bedrock) echo "test|vendor/bin/pest|--version" ;;
    composer/composer) echo "console|bin/composer|--version" ;;
    # This checkout *is* the phpunit/phpunit source; its own bin/phpunit
    # entry is for consumers, not itself — run the root script directly.
    phpunit/phpunit) echo "console|phpunit|--version" ;;
    # No console script; phpunit.xml + a composer.json test script.
    slimphp/Slim-Skeleton) echo "test|vendor/bin/phpunit|" ;;
    yiisoft/yii2-app-basic) echo "console|yii|help" ;;
    statamic/statamic) echo "console|please|--version" ;;
    # craft --version needs a database connection before Console even
    # parses the flag; `craft help` reaches the same autoload/boot path
    # without one.
    craftcms/craft) echo "console|craft|help" ;;
    *) echo "none||" ;;
  esac
}

# Runs the chosen entry point in $1 (an installed project dir) for corpus
# project $2, and prints "pass|<kind> <path>" or "fail|<reason>" or
# "none|<reason>".
boot_check() {
  local dir=$1 name=$2 kind rel args out rc reason
  IFS='|' read -r kind rel args <<< "$(boot_entry_for "$name")"
  if [ "$kind" = "none" ]; then
    echo "none|no console or test entry ships without a configured site"
    return
  fi
  if [ ! -f "$dir/$rel" ]; then
    echo "fail|$rel missing after install"
    return
  fi
  rc=0
  out=$(cd "$dir" && php "$rel" $args 2>&1) || rc=$?
  if [ "$rc" -ne 0 ] || grep -qiE 'Fatal error|Uncaught (Error|Exception|TypeError|ArgumentCountError)' <<< "$out"; then
    reason=$(grep -iE 'Fatal error|Uncaught|error' <<< "$out" | tail -1 | redact)
    [ -n "$reason" ] || reason=$(last_line "$out")
    echo "fail|$reason"
  else
    echo "pass|$kind $rel"
  fi
}

# inode and directory-entry counts of $1/vendor. Dentries: every path under
# vendor/ (files and directories). Inodes: distinct inode numbers — under
# the default hardlink mode most files share their inode with the store, so
# this is not a proxy for bytes (see the chapter and this script's header).
count_vendor() {
  local dir=$1
  local dentries inodes
  dentries=$(find "$dir/vendor" | wc -l)
  inodes=$(find "$dir/vendor" -printf '%i\n' | sort -u | wc -l)
  echo "$dentries|$inodes"
}

# `linkat` call count for one warm install (cache populated, vendor/ freshly
# removed), under `strace -c -f`. Prints the count, or
# "n/a (strace unavailable)" if strace isn't on PATH — skipped gracefully,
# never silently omitted.
#
# XDG_CACHE_HOME must point at the same warm, isolated store bench/run.sh's
# own "viv warm" scenario just used ($work/bench/xdg-cache here, matching
# its internal cache_for()'s "$work/xdg-cache/vivace" one level up) rather
# than viv's default ~/.cache/vivace. Without this, $dir (under $work, on
# whatever filesystem $TMPDIR is) and the real cache (under $HOME) can sit
# on different filesystems, so every link silently falls back to a copy
# (Invalid cross-device link) instead of linkat — undercounting or zeroing
# this measurement — and, worse, writes into the user's real cache, which
# AGENTS.md's "Measuring" section forbids.
measure_linkat() {
  local dir=$1
  if ! command -v strace >/dev/null 2>&1; then
    echo "n/a (strace unavailable)"
    return
  fi
  local log_file="$work/strace-$$.log"
  rm -rf "$dir/vendor"
  ( cd "$dir" && XDG_CACHE_HOME="$work/bench/xdg-cache" \
      strace -c -f -o "$log_file" -- "$viv_bin" install --no-plugins --no-scripts ) >/dev/null 2>&1 || true
  local n
  n=$(grep -wE 'linkat' "$log_file" 2>/dev/null | awk '{print $4}')
  [ -n "$n" ] && echo "$n" || echo "0"
}

footnotes=()
problem=0
flush_footnotes() {
  [ ${#footnotes[@]} -eq 0 ] && return 0
  { echo ""; for f in "${footnotes[@]}"; do echo "- $f"; done; } >> "$report"
  footnotes=()
}

# Checks out or builds one corpus entry under the persistent cache (#271
# workaround, see this file's header) and generates a lock if the project
# doesn't ship one, reusing both across runs the same way bench/corpus.sh's
# mirror/lock cache does (#170).
checkout_project() {
  local name=$1 repo=$2 commit=$3 version=$4
  local safe key dir
  safe=$(tr '/' '_' <<< "$name")
  key=$(tr '/' '_' <<< "${commit:-$version}")
  dir="$checkout_root/$safe-$key"

  if [ -d "$dir" ]; then
    log "reusing checkout for $name"
  elif [ -n "$repo" ]; then
    log "cloning $name @ $commit"
    git clone --quiet "$repo" "$dir"
    git -C "$dir" checkout --quiet "$commit"
    rm -rf "$dir/.git"
  elif [ -n "$version" ]; then
    log "create-project $name $version"
    local create_out
    if ! create_out=$(composer create-project --no-install --no-scripts --no-interaction \
        --ignore-platform-reqs "$name" "$dir" "$version" 2>&1); then
      footnotes+=("$name: create-project failed: $(last_line "$create_out")")
      problem=1
      rm -rf "$dir"
      return 1
    fi
  else
    log "skipping $name (no repo/commit or version pin)"
    return 1
  fi

  if [ ! -f "$dir/composer.lock" ]; then
    log "generating lock for $name"
    local lock_out
    if ! lock_out=$(composer -d "$dir" update --no-install --no-scripts --no-plugins \
        --ignore-platform-reqs 2>&1); then
      footnotes+=("$name: composer update (lock) failed: $(last_line "$lock_out")")
      problem=1
      return 1
    fi
  fi
  echo "$dir"
}

run_entry() {
  local name=$1 repo=$2 commit=$3 version=$4
  local safe key srcdir out packages run_out
  footnotes=()
  safe=$(tr '/' '_' <<< "$name")
  key=$(tr '/' '_' <<< "${commit:-$version}")

  srcdir=$(checkout_project "$name" "$repo" "$commit" "$version") || { flush_footnotes; return; }
  packages=$(jq -r '((.packages // []) | length) + ((."packages-dev" // []) | length)' \
    "$srcdir/composer.lock")

  local mirror_dir="$mirror_root/$safe-$key"
  log "recording mirror for $name"
  "$root/bench/mirror.sh" "$srcdir" "$mirror_dir"

  out="$work/out-$safe"; rm -rf "$out" "$work/bench"; mkdir -p "$out"
  log "benchmarking $name ($packages packages, $runs runs)"
  if ! run_out=$(BENCH_FLAGS="--no-plugins --no-scripts" BENCH_RUNS="$runs" \
      BENCH_WORK="$work/bench" BENCH_OUT="$out" BENCH_MIRROR="$mirror_dir" \
      "$root/bench/run.sh" "$srcdir" viv 2>&1); then
    log "bench/run.sh reported a problem for $name"
  fi

  local cold_json="$out/viv.json"
  local cold warm boot_result boot_state boot_note counts dentries inodes linkat
  cold=$(mean_for "$cold_json" "viv cold")
  warm=$(mean_for "$cold_json" "viv warm")
  local installed_dir="$work/bench/viv"

  if [ -z "$cold" ] || [ ! -d "$installed_dir/vendor" ]; then
    local reason
    reason=$(grep -iE 'error|fail' <<< "$run_out" | tail -1 || true)
    [ -n "$reason" ] || reason=$(last_line "$run_out")
    footnotes+=("$name: install failed: $(redact <<< "$reason")")
    problem=1
    echo "| $name | $packages | n/a | n/a | n/a | fail: install failed | n/a | n/a |" >> "$report"
    flush_footnotes
    return
  fi

  boot_result=$(boot_check "$installed_dir" "$name")
  boot_state=${boot_result%%|*}
  boot_note=${boot_result#*|}

  counts=$(count_vendor "$installed_dir")
  dentries=${counts%%|*}
  inodes=${counts#*|}

  linkat=$(measure_linkat "$installed_dir")

  echo "| $name | $packages | $(fmt "$cold") | $(fmt "$warm") | $boot_state: $boot_note | $linkat | $dentries | $inodes |" \
    >> "$report"
  if [ "$boot_state" = "fail" ]; then
    footnotes+=("$name: boot failed: $boot_note")
    problem=1
  fi
  flush_footnotes
}

ver_from() { "$@" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1; }

{
  echo ""
  echo "## $(date -u +%Y-%m-%dT%H:%M:%SZ) — control arm"
  echo ""
  echo "viv $(ver_from "$viv_bin"), flags \`--no-plugins --no-scripts\` (same as" \
    "compat/corpus.toml's own control flags), $runs runs each, from a local mirror" \
    "(bench/mirror.sh, no real network in any timed scenario)."
  echo ""
  echo "| Project | Packages | Cold | Warm | Boot | linkat (warm) | vendor/ dentries | vendor/ inodes |"
  echo "|---|---|---|---|---|---|---|---|"
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
