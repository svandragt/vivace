#!/usr/bin/env sh
# Benchmark `install` from an existing composer.lock across tools.
# Usage: bench/run.sh <project-dir> [tool...]   (tools: composer riff presto viv)
# Scenarios: cold (no cache, no vendor), warm (cache kept, no vendor), noop (vendor present);
# plus update-warm (#55): resolve composer.json against a warm metadata cache
# (composer/viv always, riff only when it's on PATH — no presto, it has no
# update command).
# Run inside devbox (`devbox run -- bench/run.sh bench/laravel`) so php/composer/hyperfine resolve.
set -eu
root=$(pwd)
proj=$(cd "$1" && pwd); shift
tools=${*:-"composer riff viv"}
work=${BENCH_WORK:-/tmp/vivace-bench}
runs=${BENCH_RUNS:-5}
out=${BENCH_OUT:-bench/results}; mkdir -p "$out"

# Every tool gets its cache under $work, never the user's real ~/.cache — see #17.
xdg_cache_home="$work/xdg-cache"
composer_home="$work/composer-home"
riff_cache_dir="$work/riff-cache"
mkdir -p "$xdg_cache_home" "$composer_home" "$riff_cache_dir"
for auth in "$HOME/.config/composer/auth.json" "${COMPOSER_HOME:-}/auth.json"; do
  [ -n "$auth" ] && [ -f "$auth" ] && cp "$auth" "$composer_home/auth.json" && break
done

cmd_for() {
  case $1 in
    composer) echo "composer install --no-interaction --no-progress --quiet" ;;
    riff)     echo "${RIFF:-riff} install --no-interaction" ;;
    presto)   echo "${PRESTO:-presto} install" ;;
    viv)      echo "${VIV:-$root/target/release/viv} install" ;;
  esac
}
env_for() {
  case $1 in
    composer) echo "COMPOSER_HOME=$composer_home" ;;
    riff)     echo "RIFF_CACHE_DIR=$riff_cache_dir" ;;
    presto)   echo "" ;;   # presto has no real cache; kept for symmetry
    viv)      echo "XDG_CACHE_HOME=$xdg_cache_home" ;;
  esac
}
cache_for() {
  case $1 in
    composer) echo "$composer_home/cache" ;;
    riff)     echo "$riff_cache_dir" ;;
    presto)   echo "$work/presto-home" ;;   # presto has no real cache; kept for symmetry
    viv)      echo "$xdg_cache_home/vivace" ;;
  esac
}

for tool in $tools; do
  dir="$work/$tool"; rm -rf "$dir"; mkdir -p "$dir"
  cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
  cmd="cd $dir && cp $proj/composer.lock . && export $(env_for "$tool") && $(cmd_for "$tool") >/dev/null 2>&1"
  cache=$(cache_for "$tool")
  hyperfine --warmup 0 --runs "$runs" --export-json "$out/$tool.json" \
    --command-name "$tool cold" --prepare "rm -rf $dir/vendor $cache" "$cmd" \
    --command-name "$tool warm" --prepare "rm -rf $dir/vendor" "$cmd" \
    --command-name "$tool noop" --prepare "true" "$cmd"
done

update_cmd_for() {
  case $1 in
    composer) echo "composer update --no-interaction --no-progress --quiet --no-install" ;;
    riff)     echo "${RIFF:-riff} update" ;;
    viv)      echo "${VIV:-$root/target/release/viv} update" ;;
  esac
}

# update-warm (#55): the metadata cache from `cache_for` is left in place
# (never wiped, unlike the cold/warm install scenarios above) so every
# `/p2/` provider file revalidates with a 304 instead of a cold fetch; only
# composer.json/composer.lock are reset before each timed run, since update
# rewrites the lock.
for tool in $tools; do
  case $tool in
    composer|viv) ;;
    riff) command -v "${RIFF:-riff}" >/dev/null 2>&1 || continue ;;
    *) continue ;;
  esac
  dir="$work/$tool-update"; rm -rf "$dir"; mkdir -p "$dir"
  cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
  cmd="cd $dir && cp $proj/composer.json $proj/composer.lock . && export $(env_for "$tool") && $(update_cmd_for "$tool") >/dev/null 2>&1"
  hyperfine --warmup 1 --runs "$runs" --export-json "$out/$tool-update.json" \
    --command-name "$tool update-warm" "$cmd"
done
