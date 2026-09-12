#!/usr/bin/env bash
# Release compatibility sweep: install a corpus of real-world projects with
# both Composer and viv and byte-diff vendor/. See compat/README.md.
# Usage: compat/run.sh [label]   (label defaults to `git describe --tags --always`)
# Run inside devbox (`devbox run -- compat/run.sh`, or `make compat`) so
# php/composer resolve.
set -euo pipefail

# Two temp files differing only by their directory prefix must compare equal
# after the prefix-fold sed below (#150). Run with COMPAT_SELFTEST=1.
selftest() {
  local dir_a dir_b
  dir_a=$(mktemp -d) dir_b=$(mktemp -d)
  mkdir -p "$dir_a/vendor/pkg" "$dir_b/vendor/pkg"
  echo "install_path => '$dir_a/vendor/pkg',$'\n'other => 1" > "$dir_a/vendor/pkg/f.php"
  echo "install_path => '$dir_b/vendor/pkg',$'\n'other => 1" > "$dir_b/vendor/pkg/f.php"
  if diff -q <(fold_prefix "$dir_a" "$dir_b" "$dir_a/vendor/pkg/f.php") "$dir_b/vendor/pkg/f.php" >/dev/null 2>&1; then
    echo "compat: selftest ok" >&2
    rm -rf "$dir_a" "$dir_b"
    exit 0
  else
    echo "compat: selftest FAILED" >&2
    rm -rf "$dir_a" "$dir_b"
    exit 1
  fi
}

# Rewrites $3 (a file under $1, the composer_dir) with $1 folded to $2 (the
# viv_dir), on both the raw prefixes and their realpath (#150: a
# generated file may embed the canonicalised path while $composer_dir/
# $viv_dir are the un-canonicalised scratch paths).
fold_prefix() {
  local from=$1 to=$2 file=$3 real_from real_to
  real_from=$(realpath -m "$from") real_to=$(realpath -m "$to")
  sed -e "s|$from|$to|g" -e "s|$real_from|$real_to|g" "$file"
}

root=$(cd "$(dirname "$0")/.." && pwd)
[ "${COMPAT_SELFTEST:-0}" = "1" ] && selftest
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

# COMPAT_LOCKS=1 (#180): also resolve each pinned project's composer.json
# with both tools and diff the two composer.lock files, instead of only
# byte-diffing vendor/ from a lock Composer generated. Neither flag list
# ignores platform requirements: this mode is only meaningful run inside
# devbox anyway (this script's own top comment already requires that for
# php/composer to resolve at all), so both tools see the same real PHP and
# extensions, matching what a user's own install would see.
compat_locks=${COMPAT_LOCKS:-0}
lock_compare_composer_flags=(--no-install --no-scripts --no-plugins --no-interaction)
lock_compare_viv_flags=(--no-install --no-plugins)

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
# affect install): asked of the binary under test (`viv diagnose --adapters`,
# #127) so this list can't drift from what viv actually adapts. The inert
# list is still a const array of names in src/plugins/mod.rs.
native_inert_names=$({
  "$viv" diagnose --adapters | cut -f1
  awk '/^const KNOWN_INERT/,/^\];/' "$root/src/plugins/mod.rs" | grep -oE '"[^"]+"' | tr -d '"'
})
# An empty list means the detection broke, not that viv adapts nothing: the
# v0.8.0 sweep ran once with every project on --no-plugins for that reason.
if [ -z "$native_inert_names" ]; then
  echo "compat: no native adapters detected; refusing to run a sweep that would test none" >&2
  exit 1
fi

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
    return
  fi

  # Some plugin-generated files embed the absolute install path (#129), e.g.
  # phpstan/extension-installer's GeneratedConfig.php. Re-check each "Files
  # X and Y differ" pair with composer_dir normalised to viv_dir before
  # calling the row a real diff.
  local real_diff="" normalised=0 line file_a file_b
  while IFS= read -r line; do
    if [[ $line == "Files "*" and "*" differ" ]]; then
      file_a=${line#Files }
      file_a=${file_a%% and *}
      file_b=${line#* and }
      file_b=${file_b% differ}
      if diff -q <(fold_prefix "$composer_dir" "$viv_dir" "$file_a") "$file_b" >/dev/null 2>&1; then
        normalised=$((normalised + 1))
        continue
      fi
      # yii2-composer and craftcms/plugin-installer append entries from a
      # per-package promise callback, so Composer's own entry order follows
      # extraction timing and differs between two runs on one lock (#130).
      # Compare those two maps as sorted lines: same entries, any order.
      case ${file_b##*/vendor/} in
        yiisoft/extensions.php | craftcms/plugins.php)
          if diff -q <(sort "$file_a") <(sort "$file_b") >/dev/null 2>&1; then
            normalised=$((normalised + 1))
            continue
          fi
          ;;
      esac
    fi
    real_diff+="$line"$'\n'
  done <<< "$diff_out"

  if [ -z "$real_diff" ]; then
    emit_row "$name" "$mode" "identical" "${viv_ms}ms" "${prefix}composer ${composer_ms}ms; path-only differences normalised: $normalised"
  else
    failures=1
    emit_row "$name" "$mode" "differs" "${viv_ms}ms" "${prefix}$(head -10 <<< "$real_diff")"
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
      local clone_out
      if ! clone_out=$(git clone --quiet "$repo" "$srcdir" 2>&1 \
          && git -C "$srcdir" checkout --quiet "$commit" 2>&1); then
        emit_row "$name" "dev" "skipped" "-" "clone failed: $(grep -v '^$' <<< "$clone_out" | head -1)"
        emit_row "$name" "no-dev" "skipped" "-" "clone failed: $(grep -v '^$' <<< "$clone_out" | head -1)"
        continue
      fi
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

# --- lock compare (#180, COMPAT_LOCKS=1) ------------------------------------

# Writes $2/$3 (result/details) for $1 (a project name) to the lock-compare
# table: three columns, not five, since there's no dev/no-dev split and no
# viv time worth reporting for a resolve-only run.
emit_lock_row() {
  local details=$3
  details=${details//\|/\\|}
  details=${details//$'\n'/<br>}
  echo "| $1 | $2 | $details |" >> "$report"
}

# Serves $1 (a directory) over 127.0.0.1 with miniserve, the same way
# bench/run.sh serves a mirror; echoes "pid port" once bound, or nothing
# (with a message on stderr) if it never binds.
serve_dir() {
  local dir=$1 log=$2 pid port="" i=0
  miniserve --port 0 --interfaces 127.0.0.1 "$dir" > "$log" 2>&1 &
  pid=$!
  while [ -z "$port" ] && [ "$i" -lt 50 ]; do
    port=$(grep -oE 'Bound to [0-9.]+:[0-9]+' "$log" 2>/dev/null | grep -oE '[0-9]+$' | head -1)
    [ -n "$port" ] || { sleep 0.1; i=$((i + 1)); }
  done
  if [ -z "$port" ]; then
    kill "$pid" > /dev/null 2>&1 || true
    echo "compat: server on $dir did not start, see $log" >&2
    return 1
  fi
  echo "$pid $port"
}

# A stdlib stand-in for miniserve (which is GET-only): replays $1's bytes as
# `application/json` for any request, GET or POST, ignoring the request
# body — Composer and viv both POST the same full package-name list either
# way (audit.rs's own fetch_advisories_from doc), so replaying one recorded
# response is exactly what a real advertising repository would also do for
# this lock's names. Echoes "pid port" once bound, same shape as serve_dir.
serve_advisories() {
  local body=$1 log=$2 py pid port="" i=0
  py=$(mktemp)
  cat > "$py" <<'PY'
import http.server
import socketserver
import sys

with open(sys.argv[1], "rb") as f:
    body = f.read()

class Handler(http.server.BaseHTTPRequestHandler):
    def _reply(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0) or 0)
        if length:
            self.rfile.read(length)
        self._reply()

    do_GET = _reply

    def log_message(self, *args):
        pass

class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True

with Server(("127.0.0.1", 0), Handler) as httpd:
    print(f"Bound to 127.0.0.1:{httpd.server_address[1]}", flush=True)
    httpd.serve_forever()
PY
  python3 "$py" "$body" > "$log" 2>&1 &
  pid=$!
  while [ -z "$port" ] && [ "$i" -lt 50 ]; do
    port=$(grep -oE 'Bound to [0-9.]+:[0-9]+' "$log" 2>/dev/null | grep -oE '[0-9]+$' | head -1)
    [ -n "$port" ] || { sleep 0.1; i=$((i + 1)); }
  done
  # Only removed once python3 has either read it (to start) or failed doing
  # so: deleting it right after backgrounding the process raced its own
  # open() and made every run report "No such file or directory" (#180).
  rm -f "$py"
  if [ -z "$port" ]; then
    kill "$pid" > /dev/null 2>&1 || true
    echo "compat: advisories server for $body did not start, see $log" >&2
    return 1
  fi
  echo "$pid $port"
}

# Points $1 (a composer.json copy) at the served mirror alone: the one
# composer-type repository plus Packagist disabled, secure-http off since
# the mirror is deliberately plain HTTP on 127.0.0.1 — same shape as
# bench/run.sh's own rewrite_py, minus the composer.lock half, since a lock
# compare's `update` never reads an existing lock's dist URLs. Regenerated
# per project/call rather than once for the whole sweep, so it's never left
# behind under COMPAT_LOCKS=1's own scratch-free temp files.
lock_compare_point_at_mirror() {
  local py
  py=$(mktemp)
  cat > "$py" <<'PY'
import json
import sys

path, url = sys.argv[1], sys.argv[2]
with open(path) as f:
    cj = json.load(f)
cj["repositories"] = [{"type": "composer", "url": url}, {"packagist.org": False}]
cj.setdefault("config", {})["secure-http"] = False
with open(path, "w") as f:
    json.dump(cj, f, indent=4)
PY
  python3 "$py" "$1" "$2"
  rm -f "$py"
}

# Stops $1/$2 (a serve_dir/serve_advisories pid, or "" if never started).
# Not a RETURN trap: that fires on *every* function return for the rest of
# the script, not just this one's, and would reference these two locals
# after they've gone out of scope the moment anything else returns (#180).
stop_lock_servers() {
  [ -z "$1" ] || kill "$1" > /dev/null 2>&1 || true
  [ -z "$2" ] || kill "$2" > /dev/null 2>&1 || true
}

# One project: mirrors $2 (its checkout, already installed by run_pinned,
# lock included) via bench/mirror.sh, serves the recording plus its
# advisories response (if any), resolves the same composer.json with both
# tools against it, and diffs the two resulting composer.lock files
# after folding _readme/plugin-api-version (#180). A fresh cache_dir per
# project, not the shared install-mode one: two projects' mirrors can end up
# on the same 127.0.0.1 port once the OS recycles it, and a cache keyed on
# that URL must never answer project B with project A's cached metadata.
lock_compare_one() {
  local name=$1 srcdir=$2 safe workdir mirror_dir served
  safe=$(echo "$name" | tr '/' '_')
  workdir="$scratch/locks/$safe"
  rm -rf "$workdir"
  mkdir -p "$workdir"

  if [ ! -f "$srcdir/composer.json" ]; then
    emit_lock_row "$name" "skipped" "no checkout (see pinned corpus table)"
    return
  fi
  if [ ! -f "$srcdir/composer.lock" ]; then
    emit_lock_row "$name" "skipped" "no composer.lock to seed the mirror recording"
    return
  fi

  log "recording metadata mirror for $name (lock compare, #180)"
  mirror_dir="$workdir/mirror"
  local mirror_out
  if ! mirror_out=$("$root/bench/mirror.sh" "$srcdir" "$mirror_dir" 2>&1); then
    save_log "$mirror_out" "$name-lock-mirror"
    emit_lock_row "$name" "skipped" "mirror recording failed: $(last_lines "$mirror_out")"
    return
  fi

  served="$workdir/served"
  mkdir -p "$served"
  cp -a "$mirror_dir"/. "$served"/

  local mirror_pid="" adv_pid=""

  local mirror_started mirror_port
  if ! mirror_started=$(serve_dir "$served" "$logs_dir/$safe-lock-mirror-server.log"); then
    emit_lock_row "$name" "skipped" "$mirror_started"
    return
  fi
  mirror_pid=${mirror_started% *}
  mirror_port=${mirror_started#* }
  find "$served/p2" -name '*.json' -exec sed -i "s/__PORT__/$mirror_port/g" {} +

  if [ -f "$served/advisories.json" ]; then
    local adv_started adv_port
    if adv_started=$(serve_advisories "$served/advisories.json" "$logs_dir/$safe-lock-advisories-server.log"); then
      adv_pid=${adv_started% *}
      adv_port=${adv_started#* }
      jq --arg url "http://127.0.0.1:$adv_port/security-advisories" \
        '. + {"security-advisories": {"api-url": $url}}' "$served/packages.json" \
        > "$served/packages.json.tmp" && mv "$served/packages.json.tmp" "$served/packages.json"
    else
      log "$name: advisories server failed to start, resolving without recorded advisory data"
    fi
  fi

  local mirror_url="http://127.0.0.1:$mirror_port"
  local composer_dir="$workdir/composer" viv_dir="$workdir/viv"
  mkdir -p "$composer_dir" "$viv_dir"
  cp "$srcdir/composer.json" "$composer_dir/composer.json"
  cp "$srcdir/composer.json" "$viv_dir/composer.json"
  lock_compare_point_at_mirror "$composer_dir/composer.json" "$mirror_url"
  lock_compare_point_at_mirror "$viv_dir/composer.json" "$mirror_url"

  local lock_cache_dir="$scratch/cache-locks/$safe"
  rm -rf "$lock_cache_dir"
  mkdir -p "$lock_cache_dir"

  local composer_out composer_rc=0
  composer_out=$(composer -d "$composer_dir" update "${lock_compare_composer_flags[@]}" 2>&1) || composer_rc=$?
  if [ ! -f "$composer_dir/composer.lock" ]; then
    save_log "$composer_out" "$name-lock-composer"
    stop_lock_servers "$mirror_pid" "$adv_pid"
    emit_lock_row "$name" "skipped" "composer update failed: $(last_lines "$composer_out")"
    return
  fi
  local note=""
  # A default `update` also runs Composer's post-update audit step, which can
  # exit non-zero on a real advisory match even though the lock it just
  # wrote is fine; the lock is what this mode diffs, so that alone isn't a
  # skip, just a note (#180 doesn't ask for --no-audit, and skipping it would
  # mean this mode never sees the resolver reacting to an advisory match).
  [ "$composer_rc" -eq 0 ] || note="composer exited $composer_rc (lock still written, likely an audit finding); "

  local viv_out
  if ! viv_out=$(XDG_CACHE_HOME="$lock_cache_dir" "$viv" update "${lock_compare_viv_flags[@]}" \
      --cache-dir "$lock_cache_dir" -d "$viv_dir" 2>&1); then
    save_log "$viv_out" "$name-lock-viv"
    stop_lock_servers "$mirror_pid" "$adv_pid"
    emit_lock_row "$name" "viv error" "${note}$(tail -5 <<< "$viv_out")"
    return
  fi
  stop_lock_servers "$mirror_pid" "$adv_pid"

  local diff_out
  diff_out=$(diff -u \
    <(jq 'del(._readme, .["plugin-api-version"])' "$composer_dir/composer.lock") \
    <(jq 'del(._readme, .["plugin-api-version"])' "$viv_dir/composer.lock") 2>&1) || true
  if [ -z "$diff_out" ]; then
    emit_lock_row "$name" "identical" "${note}same resolution; compared through jq, so lock formatting is not what this proves (tests/ covers that byte for byte)"
  else
    failures=1
    emit_lock_row "$name" "differs" "${note}$(head -10 <<< "$diff_out")"
  fi
}

# Re-walks the pinned corpus, reusing each project's checkout from
# run_pinned above (skipping one entirely if that pass never produced a
# composer.json for it) rather than cloning/creating it a second time.
run_lock_compare() {
  [ "$compat_locks" = "1" ] || return 0
  echo "" >> "$report"
  echo "## Lock compare (\`COMPAT_LOCKS=1\`, #180)" >> "$report"
  echo "\`composer update ${lock_compare_composer_flags[*]}\` versus \`viv update ${lock_compare_viv_flags[*]}\`, both against a \`bench/mirror.sh\` recording of the pinned checkout's own resolved packages (dists aren't needed for \`--no-install\`, but the same recording carries the \`security-advisories\` response too). Run inside devbox so both tools see the same PHP/extensions, rather than passing \`--ignore-platform-reqs\` to either." >> "$report"
  {
    echo "| Project | Result | Details |"
    echo "|---|---|---|"
  } >> "$report"
  local name repo commit version path safe srcdir
  while IFS="|" read -r name repo commit version path; do
    wanted "$name" || continue
    safe=$(echo "$name" | tr '/' '_')
    srcdir="$scratch/src/$safe"
    lock_compare_one "$name" "$srcdir"
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
  if [ "$compat_locks" = "1" ]; then
    echo "\`COMPAT_LOCKS=1\`: the pinned corpus also gets a lock-compare pass (#180), described in its own section below."
  fi
  echo ""
} >> "$report"

run_pinned
run_lock_compare
run_random

{
  echo ""
  echo "$refused_projects of $total_projects projects would refuse without --no-plugins."
} >> "$report"

log "report written to $report"
cat "$report"

exit $failures
