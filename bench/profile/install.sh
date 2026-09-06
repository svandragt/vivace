#!/usr/bin/env sh
# Flamegraph a warm `viv install` (#54). Needs `perf` access
# (`perf_event_paranoid <= 2`, or `CAP_PERFMON`/`CAP_SYS_ADMIN`): a sandboxed
# CI or dev container often doesn't have either, in which case `cargo
# flamegraph` errors with a message pointing at `perf_event_paranoid` — see
# bench/results/profile.md's "What couldn't be measured" section for the
# details of that gap in the environment this profiling ran in.
# Usage: bench/profile/install.sh <project-dir> [-o]
# Run inside devbox (`devbox run -- bench/profile/install.sh bench/laravel`)
# so `perf`/`cargo flamegraph` resolve.
set -eu
root=$(pwd)
proj=$(cd "$1" && pwd); shift
opt=${1:-}
work=${BENCH_WORK:-/tmp/vivace-profile}
dir="$work/install"; rm -rf "$dir"; mkdir -p "$dir"
cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
export XDG_CACHE_HOME="$work/xdg-cache"

# Warm the cache with a plain (unprofiled) install first, then reset vendor/
# so the flamegraph itself only captures the warm path (link + autoload),
# not the cold fetch.
"$root/target/release/viv" install -d "$dir" >/dev/null 2>&1
rm -rf "$dir/vendor"

name=warm
[ "$opt" = "-o" ] && name=warm-o
out="$root/bench/results/flamegraph-install-$name.svg"
cargo flamegraph --release --bin viv -o "$out" -- install -d "$dir" $opt
echo "$out"
