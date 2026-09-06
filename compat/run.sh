#!/usr/bin/env bash
# Release compatibility sweep: install a corpus of real-world projects with
# both Composer and viv and byte-diff vendor/. See compat/README.md.
# Usage: compat/run.sh [label]   (label defaults to `git describe --tags --always`)
# Run inside devbox (`devbox run -- compat/run.sh`, or `make compat`) so
# php/composer resolve.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
label=${1:-$(git -C "$root" describe --tags --always)}
scratch=${COMPAT_SCRATCH:-$(mktemp -d)}
viv=${VIV:-$root/target/release/viv}
corpus=${COMPAT_CORPUS:-$root/compat/corpus.toml}
seed=${COMPAT_SEED:-$(date +%Y%m%d)}
random_n=${COMPAT_RANDOM:-10}
only=${COMPAT_ONLY:-}

# Every tool gets scratch caches, never the user's real ones — see bench/run.sh.
cache_dir="$scratch/cache"
composer_home="$scratch/composer-home"
composer_cache="$scratch/composer-cache"
mkdir -p "$cache_dir" "$composer_home" "$composer_cache" "$scratch/src" "$scratch/work"
export COMPOSER_HOME="$composer_home"
export COMPOSER_CACHE_DIR="$composer_cache"

# --ignore-platform-reqs on `install` changes what Composer writes
# (vendor/composer/platform_check.php, autoload_real.php's require of it),
# which breaks the byte-diff this sweep exists to run — so it's only used
# for the `update --no-install` calls that generate a missing lock.
composer_install_flags=(--no-scripts --no-plugins --no-interaction)
composer_update_flags=(--no-scripts --no-plugins --no-interaction --ignore-platform-reqs)

results_dir="$root/compat/results"
mkdir -p "$results_dir"
report="$results_dir/$label.md"
: > "$report"

failures=0

log() { echo "compat: $*" >&2; }

IFS=',' read -r -a only_list <<< "$only"
wanted() {
  [ ${#only_list[@]} -eq 0 ] && return 0
  local name=$1 w
  for w in "${only_list[@]}"; do [ "$w" = "$name" ] && return 0; done
  return 1
}

# --- report table -----------------------------------------------------------

table_header() {
  {
    echo "| Project | Mode | Result | viv time | Details |"
    echo "|---|---|---|---|---|"
  } >> "$report"
}

emit_row() {
  # $1 name, $2 mode, $3 result, $4 viv_ms (or "-"), $5 details
  local details=$5
  details=${details//$'\n'/<br>}
  echo "| $1 | $2 | $3 | $4 | $details |" >> "$report"
}

# --- unsupported-feature detection -----------------------------------------

# Echoes a skip reason if $1 (a project dir) uses a feature viv doesn't
# support yet, or nothing if it looks installable.
skip_reason_for() {
  local dir=$1
  if [ -f "$dir/composer.json" ] && jq -e '
      (.repositories // [] | (if type == "object" then [.[]] else . end)
        | any(.type == "path" or .type == "vcs"))
    ' "$dir/composer.json" > /dev/null 2>&1; then
    echo "path/vcs repo in composer.json (#13)"
    return
  fi
  if [ -f "$dir/composer.lock" ] && jq -e '
      ([.packages[]?, ."packages-dev"[]?] | any(.type == "composer-plugin"))
    ' "$dir/composer.lock" > /dev/null 2>&1; then
    echo "plugins required (#12)"
    return
  fi
  if [ -f "$dir/composer.json" ] && jq -e '
      .config."preferred-install"? as $p
      | ($p == "source")
        or (($p | type) == "object" and ([$p[]?] | index("source") != null))
    ' "$dir/composer.json" > /dev/null 2>&1; then
    echo "preferred-install source (#43)"
    return
  fi
}

# --- one project, one mode --------------------------------------------------

run_mode() {
  local name=$1 srcdir=$2 mode=$3 note=${4:-}
  local safe workdir mode_flag composer_dir viv_dir prefix=""
  safe=$(echo "$name" | tr '/' '_')
  mode_flag=""
  [ "$mode" = "no-dev" ] && mode_flag="--no-dev"
  [ -n "$note" ] && prefix="$note; "

  workdir="$scratch/work/$safe/$mode"
  rm -rf "$workdir"
  mkdir -p "$workdir"
  composer_dir="$workdir/composer"
  viv_dir="$workdir/viv"
  cp -a "$srcdir" "$composer_dir"
  cp -a "$srcdir" "$viv_dir"
  rm -rf "$composer_dir/vendor" "$viv_dir/vendor"

  local composer_out composer_ms
  local start end
  start=$(date +%s%N)
  if ! composer_out=$(composer -d "$composer_dir" install $mode_flag "${composer_install_flags[@]}" 2>&1); then
    # Composer names its own escape hatch in the error text when the failure
    # is an unmet platform requirement; that's a more reliable signal than
    # pattern-matching the prose around it.
    if grep -qi -- '--ignore-platform-req' <<< "$composer_out"; then
      local platform_line
      platform_line=$(grep -im1 -E 'requires php|your php version|platform' <<< "$composer_out")
      [ -n "$platform_line" ] || platform_line=$(grep -v '^$' <<< "$composer_out" | tail -1)
      emit_row "$name" "$mode" "skipped" "-" "${prefix}platform: $platform_line"
    else
      emit_row "$name" "$mode" "skipped" "-" "${prefix}composer failed: $(tail -3 <<< "$composer_out")"
    fi
    return
  fi
  end=$(date +%s%N)
  composer_ms=$(((end - start) / 1000000))

  local viv_out viv_ms
  start=$(date +%s%N)
  if ! viv_out=$("$viv" install $mode_flag --no-scripts --no-plugins --cache-dir "$cache_dir" -d "$viv_dir" 2>&1); then
    end=$(date +%s%N)
    viv_ms=$(((end - start) / 1000000))
    failures=1
    emit_row "$name" "$mode" "viv error" "${viv_ms}ms" "${prefix}$(tail -5 <<< "$viv_out")"
    return
  fi
  end=$(date +%s%N)
  viv_ms=$(((end - start) / 1000000))

  local diff_out
  if diff_out=$(diff -rq --exclude=.vivace-state --exclude=.git "$composer_dir/vendor" "$viv_dir/vendor" 2>&1); then
    emit_row "$name" "$mode" "identical" "${viv_ms}ms" "${prefix}composer ${composer_ms}ms"
  else
    failures=1
    emit_row "$name" "$mode" "differs" "${viv_ms}ms" "${prefix}$(head -10 <<< "$diff_out")"
  fi
}

# Runs both modes for a project dir, after the shared skip checks.
process_project() {
  local name=$1 srcdir=$2
  local reason note=""
  reason=$(skip_reason_for "$srcdir")
  if [ -n "$reason" ]; then
    emit_row "$name" "dev" "skipped" "-" "$reason"
    emit_row "$name" "no-dev" "skipped" "-" "$reason"
    return
  fi
  if [ ! -f "$srcdir/composer.lock" ]; then
    local update_out
    if ! update_out=$(composer -d "$srcdir" update --no-install "${composer_update_flags[@]}" 2>&1); then
      emit_row "$name" "dev" "skipped" "-" "lock missing, composer update failed: $(tail -3 <<< "$update_out")"
      emit_row "$name" "no-dev" "skipped" "-" "lock missing, composer update failed: $(tail -3 <<< "$update_out")"
      return
    fi
    note="lock generated"
  fi
  run_mode "$name" "$srcdir" "dev" "$note"
  run_mode "$name" "$srcdir" "no-dev" "$note"
}

# --- pinned corpus -----------------------------------------------------------

# corpus.toml is a fixed, hand-written schema (one string per key, one
# key per line): a small line parser is simpler than pulling in a TOML
# reader for ten records.
parse_corpus() {
  awk '
    /^\[\[project\]\]/ { if (name != "") print name "|" repo "|" commit "|" version
                          name = ""; repo = ""; commit = ""; version = ""; next }
    /^name *=/    { v = $0; sub(/^name *= *"/, "", v); sub(/" *$/, "", v); name = v }
    /^repo *=/    { v = $0; sub(/^repo *= *"/, "", v); sub(/" *$/, "", v); repo = v }
    /^commit *=/  { v = $0; sub(/^commit *= *"/, "", v); sub(/" *$/, "", v); commit = v }
    /^version *=/ { v = $0; sub(/^version *= *"/, "", v); sub(/" *$/, "", v); version = v }
    END { if (name != "") print name "|" repo "|" commit "|" version }
  ' "$corpus"
}

run_pinned() {
  echo "## Pinned corpus" >> "$report"
  table_header
  local name repo commit version safe srcdir
  while IFS="|" read -r name repo commit version; do
    wanted "$name" || continue
    safe=$(echo "$name" | tr '/' '_')
    srcdir="$scratch/src/$safe"
    rm -rf "$srcdir"
    if [ -n "$repo" ]; then
      log "cloning $name @ $commit"
      git clone --quiet "$repo" "$srcdir"
      git -C "$srcdir" checkout --quiet "$commit"
      rm -rf "$srcdir/.git"
    else
      log "create-project $name $version"
      if ! composer create-project --no-install --no-scripts --no-interaction \
          --ignore-platform-reqs "$name" "$srcdir" "$version" > /dev/null 2>&1; then
        emit_row "$name" "dev" "skipped" "-" "create-project failed"
        emit_row "$name" "no-dev" "skipped" "-" "create-project failed"
        continue
      fi
      # create-project --no-install writes composer.json but no lock.
      if ! composer -d "$srcdir" update --no-install "${composer_update_flags[@]}" > /dev/null 2>&1; then
        emit_row "$name" "dev" "skipped" "-" "composer failed"
        emit_row "$name" "no-dev" "skipped" "-" "composer failed"
        continue
      fi
    fi
    process_project "$name" "$srcdir"
  done < <(parse_corpus)
}

# --- random sample -----------------------------------------------------------

run_random() {
  echo "" >> "$report"
  if [ "$random_n" -eq 0 ]; then
    echo "## Random sample: disabled" >> "$report"
    return
  fi
  echo "## Random sample (seed \`$seed\`, n=$random_n)" >> "$report"
  table_header

  local candidates="$scratch/candidates.txt"
  : > "$candidates"
  log "fetching popular.json pages"
  for page in 1 2 3; do
    curl -sS "https://packagist.org/explore/popular.json?per_page=100&page=$page" \
      | jq -r '.packages[].name' >> "$candidates" || true
  done
  log "fetching list.json"
  curl -sS "https://packagist.org/packages/list.json" \
    | jq -r '.packageNames[]' >> "$candidates"

  local picks
  picks=$(sort -u "$candidates" | shuf --random-source=<(yes "$seed") -n "$random_n")

  # COMPAT_ONLY scopes the pinned corpus only: the random sample is a
  # separate namespace and its whole point is to run packages you didn't name.
  local pkg safe srcdir
  while IFS= read -r pkg; do
    [ -n "$pkg" ] || continue
    safe=$(echo "$pkg" | tr '/' '_')
    srcdir="$scratch/src/random_$safe"
    rm -rf "$srcdir"
    mkdir -p "$srcdir"
    printf '{"require": {"%s": "*"}}\n' "$pkg" > "$srcdir/composer.json"
    log "locking random pick $pkg"
    if ! composer -d "$srcdir" update --no-install "${composer_update_flags[@]}" > /dev/null 2>&1; then
      emit_row "$pkg" "dev" "skipped" "-" "composer failed"
      emit_row "$pkg" "no-dev" "skipped" "-" "composer failed"
      continue
    fi
    process_project "$pkg" "$srcdir"
  done <<< "$picks"
}

# --- main --------------------------------------------------------------------

{
  echo "# Compatibility sweep — $label"
  echo ""
  echo "Composer install flags: \`${composer_install_flags[*]}\`; viv gets the same plus \`--no-plugins\` (viv refuses plugin-using projects otherwise)."
  echo "Composer update flags, used only to generate a missing lock: \`${composer_update_flags[*]}\`."
  echo "A project whose platform requirements aren't met is reported as \`skipped: platform\`, not a failure."
  echo "Git-source checkouts have their \`.git\` stripped before installing, so the vendor diff excludes \`.git\` metadata on both sides."
  echo ""
} >> "$report"

run_pinned
run_random

log "report written to $report"
cat "$report"

exit $failures
