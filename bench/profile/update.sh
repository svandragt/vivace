#!/usr/bin/env sh
# Flamegraph a `viv update` (#55): metadata closure + solve dominate, per
# bench/results/profile.md §2.3. Same `perf` access caveat as install.sh.
# Usage: bench/profile/update.sh <project-dir> [package...]
# Run inside devbox (`devbox run -- bench/profile/update.sh bench/laravel`).
set -eu
root=$(pwd)
proj=$(cd "$1" && pwd); shift
work=${BENCH_WORK:-/tmp/vivace-profile}
dir="$work/update"; rm -rf "$dir"; mkdir -p "$dir"
cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
export XDG_CACHE_HOME="$work/xdg-cache"

name=full
[ $# -gt 0 ] && name=partial
out="$root/bench/results/flamegraph-update-$name.svg"
cargo flamegraph --release --bin viv -o "$out" -- update -d "$dir" "$@"
echo "$out"
