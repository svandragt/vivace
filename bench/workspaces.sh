#!/usr/bin/env bash
# Measures workspace mode against an aggregate root and independent roots
# (#279) across bench/workspaces-corpus.toml, appending one row per
# repository to bench/results/workspaces.md. See docs/research.md chapter 3
# for the question and bench/corpus.sh for the harness idioms this reuses
# (pinned commits, awk corpus parser, hyperfine scenarios).
#
# Usage: bench/workspaces.sh <rows1-3|rows4-5> [repo-name,...]
#   rows1-3 (git checkouts only, no network beyond cloning):
#     1. members found, with their own composer.lock, with their own CI
#        workflow running composer/viv install in the member directory
#     2. packages in the union of members' own locks versus the sum
#        across members (members without a lock are excluded, not solved,
#        so this phase stays network-free; see the row's footnote)
#     3. distinct package names whose version differs across members' own
#        committed composer.lock files (drift today)
#   rows4-5 (hits Packagist): builds one aggregate root per repository (a
#     generated composer.json requiring every member by name through a
#     `path` repository, symlink: true) and solves it once, then compares:
#     4. each member's standalone `viv update --no-install` versus the
#        aggregate solve, for the packages the member itself requires
#     5. cold/warm install time and store size, N independent installs
#        versus one aggregate install (hyperfine, BENCH_RUNS runs each)
#
# Run each phase inside devbox (`devbox run -- bench/workspaces.sh rows1-3`);
# rows1-3 first, commit the result, then rows4-5.
#
# Env: VIV (binary under test, default target/release/viv — build it first,
# don't use ~/.cargo/bin), BENCH_RUNS (hyperfine runs, default 3),
# WORKSPACES_CORPUS (corpus.toml path), WORKSPACES_REPORT (results path),
# WORKSPACES_WORK (scratch dir, default a scratchpad mktemp -d; set this to
# a persistent directory to let rows4-5 reuse rows1-3's clones instead of
# re-cloning).
set -euo pipefail
shopt -s nullglob

root=$(cd "$(dirname "$0")/.." && pwd)
corpus=${WORKSPACES_CORPUS:-$root/bench/workspaces-corpus.toml}
viv_bin=${VIV:-$root/target/release/viv}
runs=${BENCH_RUNS:-3}
report=${WORKSPACES_REPORT:-$root/bench/results/workspaces.md}
phase=${1:-rows1-3}
only=${2:-}

if [ -n "${WORKSPACES_WORK:-}" ]; then
  work=$WORKSPACES_WORK
  mkdir -p "$work"
else
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
fi
mkdir -p "$work/src"

log() { echo "workspaces: $*" >&2; }

IFS=',' read -r -a only_list <<< "$only"
wanted() {
  [ ${#only_list[@]} -eq 0 ] && return 0
  local name=$1 w
  for w in "${only_list[@]}"; do [ "$w" = "$name" ] && return 0; done
  return 1
}

# workspaces-corpus.toml line parser, same shape as bench/corpus.sh's
# parse_corpus but for this file's own [[repo]] schema.
parse_corpus() {
  awk '
    /^\[\[repo\]\]/ { if (name != "") print name "|" repo "|" commit "|" members "|" found_via
                       name=""; repo=""; commit=""; members=""; found_via=""; next }
    /^name *=/      { v=$0; sub(/^name *= *"/,"",v); sub(/" *$/,"",v); name=v }
    /^repo *=/      { v=$0; sub(/^repo *= *"/,"",v); sub(/" *$/,"",v); repo=v }
    /^commit *=/    { v=$0; sub(/^commit *= *"/,"",v); sub(/" *$/,"",v); commit=v }
    /^members *=/   { v=$0; sub(/^members *= *"/,"",v); sub(/" *$/,"",v); members=v }
    /^found_via *=/ { v=$0; sub(/^found_via *= *"/,"",v); sub(/" *$/,"",v); found_via=v }
    END { if (name != "") print name "|" repo "|" commit "|" members "|" found_via }
  ' "$corpus"
}

# Clones (or reuses, if WORKSPACES_WORK already has it at the right commit)
# one repository, prints its checkout path.
clone_repo() {
  local name=$1 repo=$2 commit=$3 safe srcdir
  safe=$(tr '/' '_' <<< "$name")
  srcdir="$work/src/$safe"
  if [ -d "$srcdir/.git" ] && [ "$(git -C "$srcdir" rev-parse HEAD 2>/dev/null)" = "$commit" ]; then
    echo "$srcdir"
    return
  fi
  rm -rf "$srcdir"
  log "cloning $name @ $commit"
  git clone --quiet "$repo" "$srcdir" >&2
  git -C "$srcdir" checkout --quiet "$commit" >&2
  echo "$srcdir"
}

# One absolute member directory per line, de-duplicated, for every
# comma-separated glob in $2 (relative to $1).
find_members() {
  local srcdir=$1 globs=$2 g f
  IFS=',' read -r -a garr <<< "$globs"
  for g in "${garr[@]}"; do
    for f in "$srcdir"/$g; do
      [ -f "$f" ] || continue
      dirname "$f"
    done
  done | sort -u
}

# "found|with_lock|with_ci" for the members in $2 (one per line, from
# find_members), $1 the repo checkout root (for relative paths in the CI
# grep). CI evidence: a workflow file that mentions the member's path next
# to a composer/viv install command — grep, not a YAML parse, per #279.
row1() {
  local srcdir=$1 members_file=$2 found=0 with_lock=0 with_ci=0 d rel
  local workflows=("$srcdir"/.github/workflows/*.yml "$srcdir"/.github/workflows/*.yaml)
  while read -r d; do
    [ -n "$d" ] || continue
    found=$((found + 1))
    [ -f "$d/composer.lock" ] && with_lock=$((with_lock + 1))
    rel=${d#"$srcdir"/}
    if [ ${#workflows[@]} -gt 0 ] && grep -lE "(working-directory:|cd )[^\n]*$rel" "${workflows[@]}" 2>/dev/null \
        | xargs -r grep -lqE 'composer (install|update)|viv (install|update)'; then
      with_ci=$((with_ci + 1))
    fi
  done < "$members_file"
  echo "$found|$with_lock|$with_ci"
}

# "sum|union|drift|skipped" from the members in $1 (one per line) that have
# their own composer.lock. Members without a lock are skipped, not solved,
# so this stays git-checkout-only (#279's rows 1-3 rule); skipped is the
# count excluded that way.
row23() {
  local members_file=$1 d tmp sum union drift skipped=0
  tmp=$(mktemp)
  while read -r d; do
    [ -n "$d" ] || continue
    if [ -f "$d/composer.lock" ]; then
      jq -r '((.packages // [])+(."packages-dev" // [])) | .[] | "\(.name)\t\(.version)"' "$d/composer.lock" >> "$tmp"
    else
      skipped=$((skipped + 1))
    fi
  done < "$members_file"
  sum=$(wc -l < "$tmp")
  union=$(cut -f1 "$tmp" | sort -u | wc -l)
  drift=$(sort -u "$tmp" | cut -f1 | sort | uniq -d | wc -l)
  rm -f "$tmp"
  echo "$sum|$union|$drift|$skipped"
}

# Prints $1's composer.json, adding the de-facto packages.drupal.org
# repository when it requires a drupal/* package but doesn't already
# declare one. A Drupal module's own composer.json assumes it's installed
# from within a project that already has this (the drupal.org standard
# project template ships it); a member solved on its own here has no such
# project, so drupal/* would otherwise never resolve at all.
member_manifest() {
  local d=$1
  jq '
    if ((.require // {}) | keys | any(startswith("drupal/")))
      and (((.repositories // {}) | (if type == "array" then map(.url // "") else (to_entries | map(.value.url // "")) end))
           | index("https://packages.drupal.org/8") | not)
    then .repositories = ((.repositories // {}) + {drupal: {type: "composer", url: "https://packages.drupal.org/8"}})
    else .
    end
  ' "$d/composer.json"
}

# Builds a generated aggregate root at $1: requires every member in $2... by
# its composer.json `name` at `*`. viv doesn't parse a top-level `path`-type
# repository yet (issue #13's "path" half; only vcs/git shipped) but does
# link a `dist.type: "path"` package (src/source.rs, same #13), so each
# member is declared instead as a `"package"`-type repository: its own
# composer.json (require and all) plus a `dist` pointing at the member's
# checkout, which viv symlinks in exactly like a `path` repository would
# (`transport-options.symlink` defaults true, same as Composer's). Plus the
# union of members' own `repositories` entries (drupal.org/asset-packagist
# mirrors some declare) so those requirements resolve the same way in the
# aggregate solve as they do alone.
build_aggregate() {
  local agg_dir=$1; shift
  local members=("$@") d n manifest
  mkdir -p "$agg_dir"
  manifest=$(mktemp)
  echo '[]' > "$manifest"
  for d in "${members[@]}"; do
    n=$(jq -r '.name // empty' "$d/composer.json" 2>/dev/null || true)
    [ -n "$n" ] || { log "  skip $d: composer.json has no name"; continue; }
    jq --arg dir "$d" --arg name "$n" --slurpfile json <(member_manifest "$d") \
      '. + [{dir:$dir, name:$name, json:$json[0]}]' "$manifest" > "$manifest.tmp"
    mv "$manifest.tmp" "$manifest"
  done
  jq '{
    name: "vivace-bench/workspace-aggregate",
    "minimum-stability": "dev",
    "prefer-stable": true,
    require: (map({(.name): "*"}) | add // {}),
    repositories: (
      (map({type: "package", package: (.json + {version: "dev-workspace", dist: {type: "path", url: .dir}})}))
      + (map(.json.repositories // {}) | map(if type == "array" then .[] else (to_entries[] | .value) end))
      | unique_by(.package.name // .url // .type)
    )
  }' "$manifest" > "$agg_dir/composer.json"
  rm -f "$manifest"
}

# Row 4 ("differ|compared") for one member: versions its own require keys
# (excluding php/ext-*) resolve to in $1 (standalone lock) versus $2
# (aggregate lock), for $3 the member's own composer.json.
compare_member_versions() {
  local standalone_lock=$1 agg_lock=$2 member_json=$3 pkg v_alone v_agg differ=0 compared=0
  while read -r pkg; do
    [ -n "$pkg" ] || continue
    v_alone=$(jq -r --arg p "$pkg" '((.packages // [])+(."packages-dev" // [])) | .[] | select(.name==$p) | .version' \
      "$standalone_lock" 2>/dev/null | head -1)
    v_agg=$(jq -r --arg p "$pkg" '((.packages // [])+(."packages-dev" // [])) | .[] | select(.name==$p) | .version' \
      "$agg_lock" 2>/dev/null | head -1)
    [ -n "$v_alone" ] && [ -n "$v_agg" ] || continue
    compared=$((compared + 1))
    [ "$v_alone" = "$v_agg" ] || differ=$((differ + 1))
  done < <(jq -r '.require // {} | keys[] | select(. != "php") | select(startswith("ext-") | not)' "$member_json")
  echo "$differ|$compared"
}

# Footnotes accumulate across the whole run (not per repo) and are flushed
# once, after the loop, so a footnote never splits the results table into
# two markdown tables (a blank line + bullet list between two `| ... |`
# rows does that).
footnotes=()
problem=0
flush_footnotes() {
  [ ${#footnotes[@]} -eq 0 ] && return 0
  { echo ""; for f in "${footnotes[@]}"; do echo "- $f"; done; } >> "$report"
  footnotes=()
}

run_rows1_3() {
  local name=$1 repo=$2 commit=$3 members_glob=$4 srcdir members_file
  local found with_lock with_ci sum union drift skipped
  if ! srcdir=$(clone_repo "$name" "$repo" "$commit"); then
    footnotes+=("$name: clone failed")
    problem=1
    return
  fi
  members_file=$(mktemp)
  find_members "$srcdir" "$members_glob" > "$members_file"
  if [ ! -s "$members_file" ]; then
    footnotes+=("$name: no member matched $members_glob")
    problem=1
    rm -f "$members_file"
    return
  fi
  IFS='|' read -r found with_lock with_ci <<< "$(row1 "$srcdir" "$members_file")"
  IFS='|' read -r sum union drift skipped <<< "$(row23 "$members_file")"
  [ "$skipped" -gt 0 ] && footnotes+=("$name: $skipped member(s) with no committed composer.lock excluded from row 2/3 (rows 1-3 stay git-checkout only)")
  echo "| $name | $found / $with_lock / $with_ci | $union / $sum | $drift | pending | pending |" >> "$report"
  rm -f "$members_file"
  log "$name: found=$found lock=$with_lock ci=$with_ci union=$union sum=$sum drift=$drift"
}

run_rows4_5() {
  local name=$1 repo=$2 commit=$3 members_glob=$4 srcdir safe cache
  safe=$(tr '/' '_' <<< "$name")
  if ! srcdir=$(clone_repo "$name" "$repo" "$commit"); then
    footnotes+=("$name: clone failed")
    problem=1
    return
  fi
  local members=()
  while read -r d; do [ -n "$d" ] && members+=("$d"); done < <(find_members "$srcdir" "$members_glob")
  if [ ${#members[@]} -eq 0 ]; then
    footnotes+=("$name: no member matched $members_glob")
    problem=1
    return
  fi

  cache="$work/cache-$safe"
  rm -rf "$work/standalone-$safe" "$work/agg-$safe"
  mkdir -p "$work/standalone-$safe" "$cache/indep" "$cache/agg"

  log "$name: solving ${#members[@]} member(s) standalone"
  local standalone_dirs=() d mdir
  for d in "${members[@]}"; do
    mdir="$work/standalone-$safe/$(basename "$d")"
    rm -rf "$mdir"; mkdir -p "$mdir"
    member_manifest "$d" > "$mdir/composer.json"
    if ! XDG_CACHE_HOME="$cache/indep" "$viv_bin" update -d "$mdir" --no-install --ignore-platform-reqs \
        --no-plugins --no-scripts > "$mdir.log" 2>&1; then
      footnotes+=("$name/$(basename "$d"): standalone \`viv update\` failed: $(tail -1 "$mdir.log")")
      problem=1
      continue
    fi
    standalone_dirs+=("$d")
  done
  if [ ${#standalone_dirs[@]} -eq 0 ]; then
    footnotes+=("$name: no member solved standalone, rows 4-5 skipped")
    return
  fi

  log "$name: building and solving the aggregate root"
  local agg_dir="$work/agg-$safe"
  build_aggregate "$agg_dir" "${standalone_dirs[@]}"
  if ! XDG_CACHE_HOME="$cache/agg" "$viv_bin" update -d "$agg_dir" --no-install --ignore-platform-reqs \
      --no-plugins --no-scripts > "$agg_dir.log" 2>&1; then
    footnotes+=("$name: aggregate \`viv update\` failed: $(tail -1 "$agg_dir.log")")
    problem=1
    return
  fi

  local differ=0 compared=0 pair d2
  for d in "${standalone_dirs[@]}"; do
    mdir="$work/standalone-$safe/$(basename "$d")"
    pair=$(compare_member_versions "$mdir/composer.lock" "$agg_dir/composer.lock" "$d/composer.json")
    differ=$((differ + ${pair%%|*}))
    compared=$((compared + ${pair##*|}))
  done
  log "$name: row4 differ=$differ compared=$compared"

  # Row 5: N independent installs (each member's own store-backed vendor/)
  # versus one aggregate install, cold (empty store) and warm (store kept,
  # vendor/ wiped) — same scenarios as bench/run.sh, viv only.
  local indep_cmd="" d3
  for d3 in "${standalone_dirs[@]}"; do
    mdir="$work/standalone-$safe/$(basename "$d3")"
    indep_cmd+="XDG_CACHE_HOME=$cache/indep $viv_bin install -d $mdir --ignore-platform-reqs --no-plugins --no-scripts && "
  done
  indep_cmd+="true"
  local agg_cmd="XDG_CACHE_HOME=$cache/agg $viv_bin install -d $agg_dir --ignore-platform-reqs --no-plugins --no-scripts"
  local indep_vendors="" d4
  for d4 in "${standalone_dirs[@]}"; do
    indep_vendors+="$work/standalone-$safe/$(basename "$d4")/vendor "
  done

  local hf_json="$work/hf-$safe.json"
  if ! hyperfine --warmup 0 --runs "$runs" --export-json "$hf_json" \
      --command-name "indep cold" --prepare "rm -rf $indep_vendors $cache/indep" "$indep_cmd" \
      --command-name "indep warm" --prepare "rm -rf $indep_vendors" "$indep_cmd" \
      --command-name "agg cold" --prepare "rm -rf $agg_dir/vendor $cache/agg" "$agg_cmd" \
      --command-name "agg warm" --prepare "rm -rf $agg_dir/vendor" "$agg_cmd" \
      > "$work/hf-$safe.log" 2>&1; then
    footnotes+=("$name/row5: hyperfine failed: $(tail -1 "$work/hf-$safe.log")")
    problem=1
  fi
  local cold_i warm_i cold_a warm_a store_i store_a
  cold_i=$(jq -r '.results[] | select(.command=="indep cold") | .mean' "$hf_json" 2>/dev/null || true)
  warm_i=$(jq -r '.results[] | select(.command=="indep warm") | .mean' "$hf_json" 2>/dev/null || true)
  cold_a=$(jq -r '.results[] | select(.command=="agg cold") | .mean' "$hf_json" 2>/dev/null || true)
  warm_a=$(jq -r '.results[] | select(.command=="agg warm") | .mean' "$hf_json" 2>/dev/null || true)
  store_i=$(du -sb "$cache/indep" 2>/dev/null | cut -f1 || echo 0)
  store_a=$(du -sb "$cache/agg" 2>/dev/null | cut -f1 || echo 0)

  # Patches the "pending" row 4/5 cells rows1-3 already wrote for this repo
  # (the two phases share one report row, written in whichever order they run).
  python3 - "$report" "$name" "$differ" "$compared" "$cold_i" "$warm_i" "$cold_a" "$warm_a" "$store_i" "$store_a" <<'PY'
import sys
report, name, differ, compared, cold_i, warm_i, cold_a, warm_a, store_i, store_a = sys.argv[1:11]
fmt = lambda s: f"{float(s):.3f}s" if s else "n/a"
row4 = f"{differ}/{compared} differ"
row5 = f"cold {fmt(cold_i)}→{fmt(cold_a)}, warm {fmt(warm_i)}→{fmt(warm_a)}, store {int(store_i) if store_i else 0}→{int(store_a) if store_a else 0} bytes"
with open(report) as f:
    lines = f.readlines()
prefix = f"| {name} |"
for i, line in enumerate(lines):
    if line.startswith(prefix):
        cells = line.rstrip("\n").split("|")
        # | name | row1 | row2 | row3 | row4 | row5 |  -> 8 fields incl. leading/trailing empty
        cells[-2] = f" {row5} "
        cells[-3] = f" {row4} "
        lines[i] = "|".join(cells) + "\n"
        break
with open(report, "w") as f:
    f.writelines(lines)
PY
}

case "$phase" in
  rows1-3)
    {
      echo ""
      echo "## $(date -u +%Y-%m-%dT%H:%M:%SZ) rows 1-3"
      echo ""
      echo "viv $("$viv_bin" --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')."
      echo ""
      echo "| Repository | Members found / with lock / with CI | Union / sum | Drift today | Aggregate drift removed | Install cold/warm, store (indep→aggregate) |"
      echo "|---|---|---|---|---|---|"
    } >> "$report"
    while IFS='|' read -r name repo commit members_glob found_via; do
      wanted "$name" || continue
      run_rows1_3 "$name" "$repo" "$commit" "$members_glob"
    done < <(parse_corpus)
    ;;
  rows4-5)
    while IFS='|' read -r name repo commit members_glob found_via; do
      wanted "$name" || continue
      run_rows4_5 "$name" "$repo" "$commit" "$members_glob"
    done < <(parse_corpus)
    ;;
  *)
    echo "workspaces.sh: unknown phase '$phase', expected rows1-3 or rows4-5" >&2
    exit 2
    ;;
esac

flush_footnotes
log "report written to $report"
if [ "$problem" -ne 0 ]; then
  log "one or more repositories had a problem; see footnotes in $report"
  exit 1
fi
