#!/usr/bin/env sh
# Benchmark `install` from an existing composer.lock across tools.
# Usage: bench/run.sh <project-dir> [tool...]   (tools: composer riff presto viv)
# Scenarios: cold (no cache, no vendor), warm (cache kept, no vendor), noop (vendor present).
# Run inside devbox (`devbox run -- bench/run.sh bench/laravel`) so php/composer/hyperfine resolve.
set -eu
proj=$(cd "$1" && pwd); shift
tools=${*:-"composer riff viv"}
work=${BENCH_WORK:-/tmp/vivace-bench}
runs=${BENCH_RUNS:-5}
out=${BENCH_OUT:-bench/results}; mkdir -p "$out"

cmd_for() {
  case $1 in
    composer) echo "composer install --no-interaction --no-progress --quiet" ;;
    riff)     echo "${RIFF:-riff} install --no-interaction" ;;
    presto)   echo "${PRESTO:-presto} install" ;;
    viv)      echo "${VIV:-target/release/viv} install" ;;
  esac
}
cache_for() {
  case $1 in
    composer) echo "$HOME/.cache/composer/files" ;;
    riff)     echo "$HOME/.cache/riff/files" ;;
    presto)   echo "$HOME/.presto" ;;   # presto has no real cache; kept for symmetry
    viv)      echo "${XDG_CACHE_HOME:-$HOME/.cache}/vivace" ;;
  esac
}

for tool in $tools; do
  dir="$work/$tool"; rm -rf "$dir"; mkdir -p "$dir"
  cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
  cmd="cd $dir && cp $proj/composer.lock . && $(cmd_for "$tool") >/dev/null 2>&1"
  cache=$(cache_for "$tool")
  hyperfine --warmup 0 --runs "$runs" --export-json "$out/$tool.json" \
    --command-name "$tool cold" --prepare "rm -rf $dir/vendor $cache" "$cmd" \
    --command-name "$tool warm" --prepare "rm -rf $dir/vendor" "$cmd" \
    --command-name "$tool noop" --prepare "true" "$cmd"
done
