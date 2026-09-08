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
# Extra flags for every tool's install/update command, e.g. "--no-plugins --no-scripts"
# to compare plugin-heavy projects on the install-from-lock path alone.
flags=${BENCH_FLAGS:-}
out=${BENCH_OUT:-bench/results}; mkdir -p "$out"
# Absolute: the benchmarked command cd-s into its work dir before redirecting
# its output to "$out/<tool>-<scenario>.log".
out=$(cd "$out" && pwd)

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
    composer) echo "composer install --no-interaction --no-progress --quiet $flags" ;;
    riff)     echo "${RIFF:-riff} install --no-interaction $flags" ;;
    presto)   echo "${PRESTO:-presto} install" ;;
    viv)      echo "${VIV:-$root/target/release/viv} install $flags" ;;
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

failed=""
failed_rc=0
for tool in $tools; do
  dir="$work/$tool"; rm -rf "$dir"; mkdir -p "$dir"
  cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
  log="$out/$tool-cold.log"; : >"$log"
  cmd="cd $dir && cp $proj/composer.lock . && export $(env_for "$tool") && $(cmd_for "$tool") >>$log 2>&1"
  cache=$(cache_for "$tool")
  rc=0
  hyperfine --warmup 0 --runs "$runs" --export-json "$out/$tool.json" \
    --command-name "$tool cold" --prepare "rm -rf $dir/vendor $cache" "$cmd" \
    --command-name "$tool warm" --prepare "rm -rf $dir/vendor" "$cmd" \
    --command-name "$tool noop" --prepare "true" "$cmd" || rc=$?
  if [ "${rc:-0}" -ne 0 ]; then
    echo "run.sh: $tool install failed:" >&2
    tail -20 "$log" >&2
    rm -f "$out/$tool.json"
    failed="$failed $tool"
    [ "$failed_rc" -eq 0 ] && failed_rc=$rc
  fi
done

update_cmd_for() {
  case $1 in
    composer) echo "composer update --no-interaction --no-progress --quiet --no-install $flags" ;;
    riff)     echo "${RIFF:-riff} update --no-interaction --no-progress --quiet --no-install $flags" ;;
    viv)      echo "${VIV:-$root/target/release/viv} update $flags" ;;
  esac
}

# update-warm (#55): the metadata cache from `cache_for` is left in place
# (never wiped, unlike the cold/warm install scenarios above) so every
# `/p2/` provider file revalidates with a 304 instead of a cold fetch; only
# composer.json/composer.lock are reset before each timed run, since update
# rewrites the lock.
skip_update=${BENCH_SKIP_UPDATE:-}
for tool in $tools; do
  case $tool in
    composer|viv) ;;
    riff) command -v "${RIFF:-riff}" >/dev/null 2>&1 || continue ;;
    *) continue ;;
  esac
  case " $failed " in
    *" $tool "*) echo "run.sh: $tool update-warm skipped, install failed" >&2; continue ;;
  esac
  case " $skip_update " in
    *" $tool "*) echo "run.sh: $tool update-warm skipped (bench/skips.txt)" >&2; continue ;;
  esac
  dir="$work/$tool-update"; rm -rf "$dir"; mkdir -p "$dir"
  cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
  log="$out/$tool-update.log"; : >"$log"
  cmd="cd $dir && cp $proj/composer.json $proj/composer.lock . && export $(env_for "$tool") && $(update_cmd_for "$tool") >>$log 2>&1"
  if ! hyperfine --warmup 1 --runs "$runs" --export-json "$out/$tool-update.json" \
    --command-name "$tool update-warm" "$cmd"; then
    echo "run.sh: $tool update-warm failed:" >&2
    tail -20 "$log" >&2
    rm -f "$out/$tool-update.json"
    failed="$failed $tool"
    [ "$failed_rc" -eq 0 ] && failed_rc=1
  fi
done

if [ -n "$failed" ]; then
  echo "run.sh: failed:$failed" >&2
  exit "$failed_rc"
fi
