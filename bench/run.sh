#!/usr/bin/env sh
# Benchmark `install` from an existing composer.lock across tools.
# Usage: bench/run.sh <project-dir> [tool...]   (tools: composer riff presto viv)
# Scenarios: cold (no cache, no vendor), warm (cache kept, no vendor), noop (vendor present);
# plus update-warm (#55): resolve composer.json against a warm metadata cache
# (composer/viv always, riff only when it's on PATH — no presto, it has no
# update command).
#
# BENCH_MIRROR=<mirror-dir> (#165, widened): serve a mirror recorded by
# bench/mirror.sh over 127.0.0.1 for the whole run, and point every
# scenario's composer.json/composer.lock at it, so cold and update-warm hit
# no real network.
#
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

# BENCH_MIRROR: serve a bench/mirror.sh recording on a free localhost port
# for the whole run, then rewrite this project's composer.json/composer.lock
# once into $work/mirror-src, which every scenario below copies from instead
# of $proj. lock_src/json_src fall back to $proj (a same-content no-op copy)
# when no mirror is set, so the cp lines below don't need a mirror/no-mirror
# branch of their own.
mirror=${BENCH_MIRROR:-}
lock_src=$proj
json_src=$proj
if [ -n "$mirror" ]; then
  mirror=$(cd "$mirror" && pwd)
  served="$work/mirror-served"; rm -rf "$served"; mkdir -p "$served"
  cp -a "$mirror"/. "$served"/
  server_log="$out/mirror-server.log"; : >"$server_log"
  # miniserve, not python's http.server: http.server is HTTP/1.0 by default
  # (a fresh TCP connection and thread per dist) and even with
  # --protocol HTTP/1.1 its ThreadingHTTPServer falls over under real
  # concurrency (curl --parallel across 101 dists: connection resets, ~90s).
  # miniserve (actix-web) serves the same 101 dists over curl --parallel in
  # well under a second and needs no protocol flag.
  miniserve --port 0 --interfaces 127.0.0.1 "$served" >"$server_log" 2>&1 &
  mirror_pid=$!
  trap 'kill "$mirror_pid" >/dev/null 2>&1 || true' EXIT
  mirror_port=""; i=0
  while [ -z "$mirror_port" ] && [ "$i" -lt 50 ]; do
    mirror_port=$(grep -oE 'Bound to [0-9.]+:[0-9]+' "$server_log" 2>/dev/null | grep -oE '[0-9]+$' | head -1)
    [ -n "$mirror_port" ] || { sleep 0.1; i=$((i + 1)); }
  done
  [ -n "$mirror_port" ] || { echo "run.sh: mirror server on $served did not start, see $server_log" >&2; exit 1; }
  find "$served/p2" -name '*.json' -exec sed -i "s/__PORT__/$mirror_port/g" {} +

  mirror_src="$work/mirror-src"; rm -rf "$mirror_src"; mkdir -p "$mirror_src"
  cp "$proj/composer.json" "$proj/composer.lock" "$mirror_src/"
  rewrite_py=$(mktemp)
  cat > "$rewrite_py" <<'PY'
# Points composer.json/composer.lock at the bench mirror's local server:
# every dist URL in the lock, derived from name + reference like
# bench/mirror.sh derives its own, and composer.json's repositories, kept
# to the one composer-type repository plus disabling Packagist so cold
# resolves against the mirror alone. secure-http is turned off for this
# copy only: the mirror is deliberately plain HTTP on 127.0.0.1.
import json
import sys

json_path, lock_path, port = sys.argv[1], sys.argv[2], sys.argv[3]

with open(lock_path) as f:
    lock = json.load(f)
for key in ("packages", "packages-dev"):
    for pkg in lock.get(key, []):
        dist = pkg.get("dist")
        if not dist or not dist.get("reference"):
            continue
        vendor, name = pkg["name"].split("/", 1)
        dist["url"] = f"http://127.0.0.1:{port}/dists/{vendor}/{name}/{dist['reference']}.zip"
with open(lock_path, "w") as f:
    json.dump(lock, f, indent=4)

with open(json_path) as f:
    cj = json.load(f)
cj["repositories"] = [
    {"type": "composer", "url": f"http://127.0.0.1:{port}"},
    {"packagist.org": False},
]
cj.setdefault("config", {})["secure-http"] = False
with open(json_path, "w") as f:
    json.dump(cj, f, indent=4)
PY
  python3 "$rewrite_py" "$mirror_src/composer.json" "$mirror_src/composer.lock" "$mirror_port"
  rm -f "$rewrite_py"
  lock_src=$mirror_src
  json_src=$mirror_src
fi

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
  cmd="cd $dir && cp $json_src/composer.json $lock_src/composer.lock . && export $(env_for "$tool") && $(cmd_for "$tool") >>$log 2>&1"
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
    # viv's update-warm has always installed too (unlike composer/riff here),
    # which is fine against the real registry but not against a mirror: the
    # mirror only records the locked dist, and a resolve that legitimately
    # moves to a newer version has nothing to install from. --no-install
    # only under BENCH_MIRROR so the ungated, real-network baseline is
    # unchanged.
    viv)      if [ -n "$mirror" ]; then
                echo "${VIV:-$root/target/release/viv} update --no-install $flags"
              else
                echo "${VIV:-$root/target/release/viv} update $flags"
              fi ;;
  esac
}
update_offline_cmd_for() {
  case $1 in
    viv) echo "${VIV:-$root/target/release/viv} update --offline --no-install $flags" ;;
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
  # riff races itself against a mirror: it issues a duplicate conditional
  # GET for the same p2 file within the same request burst, gets a 304 back
  # for one of them, and treats that empty body as the package having zero
  # versions instead of reusing its first, already-fetched response. This
  # reproduces against both python's http.server and miniserve, so it's
  # riff's own request handling, not one server's 304 behaviour; real
  # Packagist never returns a 304 to a same-burst request, so this only
  # shows up against a low-latency local mirror — see bench/results/
  # README.md's "Local mirror" section.
  if [ "$tool" = "riff" ] && [ -n "$mirror" ]; then
    echo "run.sh: riff update-warm skipped, known mirror-mode failure (304 race, see bench/results/README.md)" >&2
    continue
  fi
  dir="$work/$tool-update"; rm -rf "$dir"; mkdir -p "$dir"
  cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
  log="$out/$tool-update.log"; : >"$log"
  cmd="cd $dir && cp $json_src/composer.json $lock_src/composer.lock . && export $(env_for "$tool") && $(update_cmd_for "$tool") >>$log 2>&1"
  if ! hyperfine --warmup 1 --runs "$runs" --export-json "$out/$tool-update.json" \
    --command-name "$tool update-warm" "$cmd"; then
    echo "run.sh: $tool update-warm failed:" >&2
    tail -20 "$log" >&2
    rm -f "$out/$tool-update.json"
    failed="$failed $tool"
    [ "$failed_rc" -eq 0 ] && failed_rc=1
  fi
done

# update-offline (#165): same warm metadata cache as update-warm above, but
# `--offline` so the solve reads only the cache, no revalidation requests at
# all — isolates parse-plus-solve time from the network. Composer has no
# offline update flag, so this runs for viv only.
for tool in $tools; do
  case $tool in
    viv) ;;
    *) echo "run.sh: $tool update-offline skipped, no offline update flag" >&2; continue ;;
  esac
  case " $failed " in
    *" $tool "*) echo "run.sh: $tool update-offline skipped, install failed" >&2; continue ;;
  esac
  case " $skip_update " in
    *" $tool "*) echo "run.sh: $tool update-offline skipped (bench/skips.txt)" >&2; continue ;;
  esac
  dir="$work/$tool-update"; rm -rf "$dir"; mkdir -p "$dir"
  cp -a "$proj"/. "$dir"/ && rm -rf "$dir/vendor"
  log="$out/$tool-update-offline.log"; : >"$log"
  cmd="cd $dir && cp $json_src/composer.json $lock_src/composer.lock . && export $(env_for "$tool") && $(update_offline_cmd_for "$tool") >>$log 2>&1"
  if ! hyperfine --warmup 1 --runs "$runs" --export-json "$out/$tool-update-offline.json" \
    --command-name "$tool update-offline" "$cmd"; then
    echo "run.sh: $tool update-offline failed:" >&2
    tail -20 "$log" >&2
    rm -f "$out/$tool-update-offline.json"
    failed="$failed $tool"
    [ "$failed_rc" -eq 0 ] && failed_rc=1
  fi
done

if [ -n "$failed" ]; then
  echo "run.sh: failed:$failed" >&2
  exit "$failed_rc"
fi
