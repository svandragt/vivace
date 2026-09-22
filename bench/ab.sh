#!/usr/bin/env bash
# Pre-merge check (#293): "did my change make viv slower", viv-against-viv,
# on one machine, no Composer arm. Runs the spanning set below for two viv
# binaries and grades the difference with `bench/compare.py --ab` -- see
# that script's docstring for why this needs a different instrument from
# the CI ratio gate (`make bench-check`).
#
# Usage: bench/ab.sh <before-viv-binary> <after-viv-binary>
# Run inside devbox (`devbox run -- bench/ab.sh ...`) so composer/hyperfine
# resolve. Env: BENCH_RUNS (default 5, forwarded to bench/run.sh),
# BENCH_CACHE (persistent checkout cache, default matches
# bench/storeload.sh's own), BENCH_AB_OUT (default bench/results/ab).
#
# Projects, each the corpus extreme (bench/results/storeload.md's control
# arm) on an axis the others don't cover:
#   laravel/laravel             balanced reference: only project with a
#                                historical baseline (the bench/laravel
#                                fixture already committed here)
#   drupal/recommended-project  file-count extreme: 68 packages but 31150
#                                vendor/ entries, warm 302ms -- a regression
#                                in link_tree or the classmap scan shows up
#                                4x louder here than on laravel
#   symfony/demo                package-count extreme: 153 packages on
#                                13201 entries -- lock parsing, metadata and
#                                the solver scale with this axis, not files
#
# No Composer arm (this is viv-against-viv) and no separate cold-skipping:
# bench/run.sh's cold/warm/noop scenarios share one hyperfine invocation, so
# cold still runs, but `compare.py --ab` only grades warm and noop and
# treats cold as informational, the same split the ratio mode uses.
#
# update-warm/update-offline are skipped outright (BENCH_SKIP_UPDATE) rather
# than run as a third, informational scenario: without a recorded mirror
# they resolve against the real registry, and for drupal/recommended-project
# that means packages.drupal.org, whose provider files this offline flag
# then finds only partially cached -- a flake that has nothing to do with
# viv's own regressions but would still fail this merge gate. Recording a
# mirror per project (bench/mirror.sh, as bench/corpus.sh and
# bench/storeload.sh do) would fix that, at the cost of a cache this script
# doesn't otherwise need; not worth it for a scenario that's informational
# only to begin with.
#
# Checkouts of the two corpus projects are cached under
# $BENCH_CACHE/checkouts the same way bench/storeload.sh does (#271:
# bench/corpus.sh deletes its own checkouts, so this reuses storeload's
# workaround rather than a third one).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
before=${1:?usage: bench/ab.sh <before-viv-binary> <after-viv-binary>}
after=${2:?usage: bench/ab.sh <before-viv-binary> <after-viv-binary>}
runs=${BENCH_RUNS:-5}
cache_root=${BENCH_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/vivace-bench}
checkout_root="$cache_root/checkouts"
mkdir -p "$checkout_root"
out_root=${BENCH_AB_OUT:-$root/bench/results/ab}
rm -rf "$out_root"
mkdir -p "$out_root/before" "$out_root/after"

# Same corpus.toml schema/parser as bench/storeload.sh and bench/corpus.sh
# (kept in sync by hand; see either if this schema changes).
parse_corpus() {
  awk '
    /^\[\[project\]\]/ { if (name != "") print name "|" repo "|" commit "|" version
                          name = ""; repo = ""; commit = ""; version = ""; next }
    /^name *=/    { v = $0; sub(/^name *= *"/, "", v); sub(/" *$/, "", v); name = v }
    /^repo *=/    { v = $0; sub(/^repo *= *"/, "", v); sub(/" *$/, "", v); repo = v }
    /^commit *=/  { v = $0; sub(/^commit *= *"/, "", v); sub(/" *$/, "", v); commit = v }
    /^version *=/ { v = $0; sub(/^version *= *"/, "", v); sub(/" *$/, "", v); version = v }
    END { if (name != "") print name "|" repo "|" commit "|" version }
  ' "$root/compat/corpus.toml"
}

# Checks out (or reuses) one corpus project under the persistent cache,
# generating a lock if it doesn't ship one -- the same checkout_project
# bench/storeload.sh uses (#271), kept in sync by hand.
checkout_project() {
  local name=$1 repo=$2 commit=$3 version=$4 safe key dir
  safe=$(tr '/' '_' <<< "$name")
  key=$(tr '/' '_' <<< "${commit:-$version}")
  dir="$checkout_root/$safe-$key"
  if [ -d "$dir" ]; then
    echo "ab: reusing checkout for $name" >&2
  elif [ -n "$repo" ]; then
    echo "ab: cloning $name @ $commit" >&2
    git clone --quiet "$repo" "$dir"
    git -C "$dir" checkout --quiet "$commit"
    rm -rf "$dir/.git"
  elif [ -n "$version" ]; then
    echo "ab: create-project $name $version" >&2
    composer create-project --no-install --no-scripts --no-interaction \
      --ignore-platform-reqs "$name" "$dir" "$version"
  else
    echo "ab: $name has neither repo/commit nor version pin" >&2
    return 1
  fi
  if [ ! -f "$dir/composer.lock" ]; then
    echo "ab: generating lock for $name" >&2
    composer -d "$dir" update --no-install --no-scripts --no-plugins --ignore-platform-reqs
  fi
  echo "$dir"
}

run_project() {
  local label=$1 dir=$2 which=$3 bin=$4 out
  out="$out_root/$which/$label"
  mkdir -p "$out"
  echo "ab: $which/$label ($bin)" >&2
  BENCH_FLAGS="--no-plugins --no-scripts" BENCH_RUNS="$runs" BENCH_OUT="$out" VIV="$bin" \
    BENCH_SKIP_UPDATE="viv" "$root/bench/run.sh" "$dir" viv
}

for which in before after; do
  bin=$before
  [ "$which" = after ] && bin=$after
  run_project laravel "$root/bench/laravel" "$which" "$bin"
done

while IFS='|' read -r name repo commit version; do
  case $name in
    symfony/demo) label=symfony_demo ;;
    drupal/recommended-project) label=drupal_recommended-project ;;
    *) continue ;;
  esac
  dir=$(checkout_project "$name" "$repo" "$commit" "$version")
  for which in before after; do
    bin=$before
    [ "$which" = after ] && bin=$after
    run_project "$label" "$dir" "$which" "$bin"
  done
done < <(parse_corpus)

python3 "$root/bench/compare.py" --ab "$out_root/before" "$out_root/after" --tolerance "${BENCH_AB_TOLERANCE:-0.15}"
