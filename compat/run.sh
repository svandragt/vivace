#!/usr/bin/env bash
# Release compatibility sweep: install a corpus of real-world projects with
# both Composer and viv and byte-diff vendor/. See compat/README.md.
# Usage: compat/run.sh [label]   (label defaults to `git describe --tags --always`)
# Run inside devbox (`devbox run -- compat/run.sh`, or `make compat`) so
# php/composer resolve.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
label=${1:-$(git -C "$root" describe --tags --always)}
if [ -n "${COMPAT_SCRATCH:-}" ]; then
  scratch=$COMPAT_SCRATCH
else
  scratch=$(mktemp -d)
  trap 'rm -rf "$scratch"' EXIT
fi
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

# Private-registry credentials, if any: copied into the scratch COMPOSER_HOME
# so both Composer and viv (which also reads $COMPOSER_HOME/auth.json) can
# authenticate. Never the user's real auth.json — this is an explicit copy.
auth_file=${COMPAT_AUTH_FILE:-}
if [ -n "$auth_file" ] && [ -f "$auth_file" ]; then
  cp "$auth_file" "$composer_home/auth.json"
  chmod 600 "$composer_home/auth.json"
fi

# --ignore-platform-reqs on `install` changes what Composer writes
# (vendor/composer/platform_check.php, autoload_real.php's require of it),
# which breaks the byte-diff this sweep exists to run — so it's only used
# for the `update --no-install` calls that generate a missing lock.
composer_install_flags=(--no-scripts --no-plugins --no-interaction)
composer_update_flags=(--no-scripts --no-plugins --no-interaction --ignore-platform-reqs)

results_dir=${COMPAT_RESULTS_DIR:-$root/compat/results}
logs_dir="$results_dir/$label-logs"
mkdir -p "$results_dir" "$logs_dir"
report="$results_dir/$label.md"
: > "$report"

failures=0
total_projects=0
refused_projects=0

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
  details=${details//\|/\\|}
  details=${details//$'\n'/<br>}
  echo "| $1 | $2 | $3 | $4 | $details |" >> "$report"
}

# Last three non-empty lines of $1, for a "composer failed" Details cell.
last_lines() {
  grep -v '^$' <<< "$1" | tail -3
}

# Writes $2 (a project/mode identifier, e.g. "$name-$mode") 's combined
# composer output ($1) to its own log under compat/results/<label>-logs/.
save_log() {
  local out=$1 id=$2
  echo "$out" > "$logs_dir/$(echo "$id" | tr '/' '_').log"
}

# --- unsupported-feature detection -----------------------------------------

# Echoes a skip reason if $1 (a project dir) uses a feature viv doesn't
# support yet, or nothing if it looks installable.
skip_reason_for() {
  local dir=$1
  if [ "${COMPAT_SKIP_VCS:-0}" = "1" ] && [ -f "$dir/composer.json" ] && jq -e '
      (.repositories // [] | (if type == "object" then [.[]] else . end)
        | any(.type == "path" or .type == "vcs"))
    ' "$dir/composer.json" > /dev/null 2>&1; then
    echo "path/vcs repo in composer.json (#13)"
    return
  fi
  if [ "${COMPAT_SKIP_PLUGINS:-0}" = "1" ] && [ -f "$dir/composer.lock" ] && jq -e '
      ([.packages[]?, ."packages-dev"[]?] | any(.type == "composer-plugin"))
    ' "$dir/composer.lock" > /dev/null 2>&1; then
    echo "plugins required, --no-plugins on both sides (#12)"
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

# --- plugin native/inert detection (#124) -----------------------------------

# Package names `viv` ports a native adapter for, or that are inert (don't
# affect install): read straight from src/plugins/mod.rs's own const arrays
# so this list can't drift from what viv actually adapts.
native_inert_names=$({
  awk '/^const NATIVE_ADAPTERS/,/^\];/' "$root/src/plugins/mod.rs"
  grep '^const KNOWN_INERT' "$root/src/plugins/mod.rs"
} | grep -oE '"[^"]+"' | tr -d '"')

# Echoes the composer-plugin package names $1 (a project dir)'s lock
# declares that its composer.json's config.allow-plugins enables: `true`
# enables every plugin, a glob map (Composer's own `vendor/*` syntax) enables
# a name whose matching key is `true`, and absent/`false` enables none.
enabled_plugins_for() {
  local dir=$1
  [ -f "$dir/composer.lock" ] && [ -f "$dir/composer.json" ] || return
  jq -s -r '
    .[0] as $lock | .[1] as $cjson
    | ($cjson.config."allow-plugins" // false) as $allow
    | [$lock.packages[]?, $lock."packages-dev"[]?]
    | map(select(.type == "composer-plugin") | .name)
    | map(select(
        . as $n
        | if ($allow | type) == "boolean" then $allow
          elif ($allow | type) == "object" then
            ($allow | to_entries | any(.value == true and
              (.key as $k | $n | test("^" + ($k | gsub("\\*"; ".*")) + "$"))))
          else false
          end
      ))
    | .[]
  ' "$dir/composer.lock" "$dir/composer.json" 2>/dev/null
}

# Sets $plugin_note ("" if $1 enables no plugins) and $plugin_native (1 if
# every enabled plugin is a native adapter or known-inert, meaning both
# sides can run without --no-plugins; 0 otherwise).
plugin_status_for() {
  local dir=$1 name refused=() any=0
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    any=1
    grep -qxF "$name" <<< "$native_inert_names" || refused+=("$name")
  done < <(enabled_plugins_for "$dir")
  if [ ${#refused[@]} -gt 0 ]; then
    plugin_note="plugins: refused $(IFS=', '; echo "${refused[*]}")"
    plugin_native=0
  elif [ "$any" = "1" ]; then
    plugin_note="plugins: native"
    plugin_native=1
  else
    plugin_note=""
    plugin_native=0
  fi
}

# --- one project, one mode --------------------------------------------------

run_mode() {
  local name=$1 srcdir=$2 mode=$3 note=${4:-} plugins_on=${5:-0}
  local safe workdir mode_flag composer_dir viv_dir prefix="" vendor_dir
  safe=$(echo "$name" | tr '/' '_')
  mode_flag=""
  [ "$mode" = "no-dev" ] && mode_flag="--no-dev"
  [ -n "$note" ] && prefix="$note; "
  # Every enabled plugin is native/inert (#124): run both sides with
  # plugins on for this project only, instead of the default --no-plugins.
  local install_flags=("${composer_install_flags[@]}") viv_plugin_flag=(--no-plugins)
  if [ "$plugins_on" = "1" ]; then
    install_flags=()
    local f
    for f in "${composer_install_flags[@]}"; do
      [ "$f" = "--no-plugins" ] || install_flags+=("$f")
    done
    viv_plugin_flag=()
  fi
  vendor_dir="vendor"
  if [ -f "$srcdir/composer.json" ]; then
    vendor_dir=$(jq -r '.config."vendor-dir" // "vendor"' "$srcdir/composer.json")
  fi

  workdir="$scratch/work/$safe/$mode"
  rm -rf "$workdir"
  mkdir -p "$workdir"
  composer_dir="$workdir/composer"
  viv_dir="$workdir/viv"
  cp -a "$srcdir" "$composer_dir"
  cp -a "$srcdir" "$viv_dir"
  rm -rf "$composer_dir/$vendor_dir" "$viv_dir/$vendor_dir"

  local composer_out composer_ms
  local start end
  start=$(date +%s%N)
  if ! composer_out=$(composer -d "$composer_dir" install $mode_flag "${install_flags[@]}" 2>&1); then
    # Only a genuine platform mismatch counts as a platform skip; a download
    # or auth failure also mentions --ignore-platform-req, so match the
    # requirement wording itself.
    if grep -qiE 'your (php|[a-z0-9_-]+) version|requires (php|ext-)|does not contain a compatible set' <<< "$composer_out"; then
      local platform_line
      platform_line=$(grep -im1 -E 'requires php|your php version|platform' <<< "$composer_out")
      [ -n "$platform_line" ] || platform_line=$(grep -v '^$' <<< "$composer_out" | tail -1)
      save_log "$composer_out" "$name-$mode"
      emit_row "$name" "$mode" "skipped" "-" "${prefix}platform: $platform_line"
    else
      save_log "$composer_out" "$name-$mode"
      emit_row "$name" "$mode" "skipped" "-" "${prefix}composer failed: $(last_lines "$composer_out")"
    fi
    return
  fi
  end=$(date +%s%N)
  composer_ms=$(((end - start) / 1000000))

  local viv_out viv_ms
  start=$(date +%s%N)
  if ! viv_out=$("$viv" install $mode_flag --no-scripts "${viv_plugin_flag[@]}" --cache-dir "$cache_dir" -d "$viv_dir" 2>&1); then
    end=$(date +%s%N)
    viv_ms=$(((end - start) / 1000000))
    failures=1
    emit_row "$name" "$mode" "viv error" "${viv_ms}ms" "${prefix}$(tail -5 <<< "$viv_out")"
    return
  fi
  end=$(date +%s%N)
  viv_ms=$(((end - start) / 1000000))

  local diff_out
  if diff_out=$(diff -rq --exclude=.vivace-state --exclude=.git "$composer_dir/$vendor_dir" "$viv_dir/$vendor_dir" 2>&1); then
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
  total_projects=$((total_projects + 1))
  if [ ! -f "$srcdir/composer.lock" ]; then
    local update_out
    if ! update_out=$(composer -d "$srcdir" update --no-install "${composer_update_flags[@]}" 2>&1); then
      save_log "$update_out" "$name-lock"
      emit_row "$name" "dev" "skipped" "-" "lock missing, composer update failed: $(last_lines "$update_out")"
      emit_row "$name" "no-dev" "skipped" "-" "lock missing, composer update failed: $(last_lines "$update_out")"
      return
    fi
    note="lock generated"
  fi

  # Plugin refusal is a property of the lock regardless of COMPAT_SKIP_PLUGINS,
  # so the summary line counts it even for a project skip_reason_for skips below.
  local plugin_note plugin_native
  plugin_status_for "$srcdir"
  [ -n "$plugin_note" ] && [ "$plugin_native" = "0" ] && refused_projects=$((refused_projects + 1))

  reason=$(skip_reason_for "$srcdir")
  if [ -n "$reason" ]; then
    emit_row "$name" "dev" "skipped" "-" "$reason"
    emit_row "$name" "no-dev" "skipped" "-" "$reason"
    return
  fi

  [ -n "$plugin_note" ] && note=${note:+$note; }$plugin_note
  run_mode "$name" "$srcdir" "dev" "$note" "$plugin_native"
  run_mode "$name" "$srcdir" "no-dev" "$note" "$plugin_native"
}

# --- pinned corpus -----------------------------------------------------------

# corpus.toml is a fixed, hand-written schema (one string per key, one
# key per line): a small line parser is simpler than pulling in a TOML
# reader for ten records.
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

run_pinned() {
  echo "## Pinned corpus" >> "$report"
  table_header
  local name repo commit version path safe srcdir
  while IFS="|" read -r name repo commit version path; do
    wanted "$name" || continue
    safe=$(echo "$name" | tr '/' '_')
    srcdir="$scratch/src/$safe"
    rm -rf "$srcdir"
    if [ -n "$path" ]; then
      log "copying $name from $path"
      mkdir -p "$srcdir"
      if command -v rsync > /dev/null 2>&1; then
        rsync -a --exclude=vendor --exclude=node_modules --exclude=.git "$path/" "$srcdir/"
      else
        cp -a "$path/." "$srcdir/"
        rm -rf "$srcdir/vendor" "$srcdir/node_modules" "$srcdir/.git"
      fi
    elif [ -n "$repo" ]; then
      log "cloning $name @ $commit"
      git clone --quiet "$repo" "$srcdir"
      git -C "$srcdir" checkout --quiet "$commit"
      # .git is kept here (unlike the path/rsync branches above) so Composer's
      # root-version guess from the checkout state matches what a real user
      # sees; see #125.
    else
      log "create-project $name $version"
      local create_out
      if ! create_out=$(composer create-project --no-install --no-scripts --no-interaction \
          --ignore-platform-reqs "$name" "$srcdir" "$version" 2>&1); then
        save_log "$create_out" "$name-lock"
        emit_row "$name" "dev" "skipped" "-" "create-project failed: $(last_lines "$create_out")"
        emit_row "$name" "no-dev" "skipped" "-" "create-project failed: $(last_lines "$create_out")"
        continue
      fi
      # create-project --no-install writes composer.json but no lock.
      local update_out
      if ! update_out=$(composer -d "$srcdir" update --no-install "${composer_update_flags[@]}" 2>&1); then
        save_log "$update_out" "$name-lock"
        emit_row "$name" "dev" "skipped" "-" "composer failed: $(last_lines "$update_out")"
        emit_row "$name" "no-dev" "skipped" "-" "composer failed: $(last_lines "$update_out")"
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
  local sample_file="$results_dir/$label.sample.json"
  if [ -f "$sample_file" ]; then
    log "reusing cached sample $sample_file"
    jq -r '.[]' "$sample_file" > "$candidates"
  else
    : > "$candidates"
    log "fetching popular.json pages"
    for page in 1 2 3; do
      curl -sS "https://packagist.org/explore/popular.json?per_page=100&page=$page" \
        | jq -r '.packages[].name' >> "$candidates" || true
    done
    log "fetching list.json"
    curl -sS "https://packagist.org/packages/list.json" \
      | jq -r '.packageNames[]' >> "$candidates"
    sort -u "$candidates" | jq -R -s 'split("\n") | map(select(length > 0))' > "$sample_file"
  fi

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
    local update_out
    if ! update_out=$(composer -d "$srcdir" update --no-install "${composer_update_flags[@]}" 2>&1); then
      save_log "$update_out" "$pkg-lock"
      emit_row "$pkg" "dev" "skipped" "-" "composer failed: $(last_lines "$update_out")"
      emit_row "$pkg" "no-dev" "skipped" "-" "composer failed: $(last_lines "$update_out")"
      continue
    fi
    process_project "$pkg" "$srcdir"
  done <<< "$picks"
}

# --- main --------------------------------------------------------------------

{
  echo "# Compatibility sweep — $label"
  echo ""
  echo "Composer install flags: \`${composer_install_flags[*]}\`; viv gets the same plus \`--no-plugins\`."
  echo "Composer update flags, used only to generate a missing lock: \`${composer_update_flags[*]}\`."
  echo "A project whose enabled plugins are all native adapters or known-inert (#124) drops \`--no-plugins\` on both sides instead, noted \`plugins: native\` in Details; any other enabled plugin keeps \`--no-plugins\` and is noted \`plugins: refused <names>\`."
  echo "A project whose platform requirements aren't met is reported as \`skipped: platform\`, not a failure."
  echo "Cloned checkouts keep their \`.git\` before installing, so Composer's root-version guess from the checkout state (branch/tag/commit) matches a real user's install (#125); \`path\`-based entries still have \`.git\` stripped, since a local checkout's git state isn't reproducible. The vendor diff excludes \`.git\` metadata on both sides regardless."
  echo ""
} >> "$report"

run_pinned
run_random

{
  echo ""
  echo "$refused_projects of $total_projects projects would refuse without --no-plugins."
} >> "$report"

log "report written to $report"
cat "$report"

exit $failures
