#!/usr/bin/env sh
# Records one project's dists and Packagist p2 metadata into a local mirror
# so bench/run.sh's cold and update-warm scenarios need no network (#165,
# widened). The recorded p2 files are then rewritten so every dist URL
# points at a placeholder local server address (http://127.0.0.1:__PORT__/...)
# that bench/run.sh fills in with the port it actually picks at serve time.
#
# Usage: bench/mirror.sh <project-dir> <mirror-dir>
#
# Idempotent: an already-recorded dist or p2 file is left alone, and the
# rewrite step always derives the URL from the package name and dist
# reference (never from whatever is already on disk), so rerunning a
# complete mirror does nothing.
#
# Run inside devbox (`devbox run -- bench/mirror.sh ...`) so curl/jq/python3
# resolve.
set -eu
proj=$(cd "$1" && pwd); shift
mirror=$1; shift
mkdir -p "$mirror/dists" "$mirror/p2"

# GitHub rate-limits unauthenticated zipball downloads; reuse the same
# token Composer itself would use, never printed or logged.
auth="$HOME/.config/composer/auth.json"
token=""
[ -f "$auth" ] && token=$(jq -r '.["github-oauth"]["github.com"] // empty' "$auth" 2>/dev/null || true)

fetch() {
  url=$1 dest=$2
  [ -f "$dest" ] && return 0
  mkdir -p "$(dirname "$dest")"
  case $url in
    https://api.github.com/*|https://github.com/*|https://codeload.github.com/*)
      if [ -n "$token" ]; then
        curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 -H "Authorization: Bearer $token" -o "$dest.tmp" "$url"
      else
        curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 -o "$dest.tmp" "$url"
      fi
      ;;
    *) curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 -o "$dest.tmp" "$url" ;;
  esac
  mv "$dest.tmp" "$dest"
}

record_dist() {
  name=$1 url=$2 ref=$3
  [ -n "$url" ] && [ "$url" != "null" ] && [ -n "$ref" ] && [ "$ref" != "null" ] || return 0
  vendor=${name%%/*} pkg=${name#*/}
  fetch "$url" "$mirror/dists/$vendor/$pkg/$ref.zip"
}

record_p2() {
  name=$1
  case $name in */*) ;; *) return 0 ;; esac # skip php/ext-* virtual packages
  vendor=${name%%/*} pkg=${name#*/}
  for suffix in "" "~dev"; do
    dest="$mirror/p2/$vendor/$pkg$suffix.json"
    [ -f "$dest" ] && continue
    # A missing dev-branch file (no default-branch releases) is normal;
    # don't fail the whole recording over it.
    fetch "https://repo.packagist.org/p2/$vendor/$pkg$suffix.json" "$dest" || true
  done
}

lock="$proj/composer.lock"
jq -r '(.packages // []) + (."packages-dev" // []) | .[] |
  [.name, (.dist.url // ""), (.dist.reference // "")] | @tsv' "$lock" |
while IFS="$(printf '\t')" read -r name url ref; do
  [ -n "$name" ] || continue
  record_dist "$name" "$url" "$ref"
  record_p2 "$name"
done

if [ -f "$proj/composer.json" ]; then
  jq -r '((.require // {}) + (."require-dev" // {})) | keys[]' "$proj/composer.json" |
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    record_p2 "$name"
  done
fi

index_py=$(mktemp)
rewrite_py=$(mktemp)
trap 'rm -f "$index_py" "$rewrite_py"' EXIT

cat > "$index_py" <<'PY'
# Rebuilds packages.json's available-packages list from whatever p2 files
# are actually on disk, so a mirror built across several mirror.sh calls
# (or a partial one, retried) always advertises exactly what it can serve.
import json
import os
import sys

mirror = sys.argv[1]
p2_root = os.path.join(mirror, "p2")
names = []
if os.path.isdir(p2_root):
    for vendor in sorted(os.listdir(p2_root)):
        vdir = os.path.join(p2_root, vendor)
        if not os.path.isdir(vdir):
            continue
        for fname in sorted(os.listdir(vdir)):
            if not fname.endswith(".json") or fname.endswith("~dev.json"):
                continue
            names.append(f"{vendor}/{fname[:-len('.json')]}")

data = {"metadata-url": "/p2/%package%.json", "available-packages": names}
with open(os.path.join(mirror, "packages.json"), "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY

cat > "$rewrite_py" <<'PY'
# Points every dist URL in the recorded p2 files at the mirror's own,
# not-yet-known port; bench/run.sh substitutes __PORT__ once it picks one.
# Derives the URL from the package name and dist reference every time
# (never from what's already on disk), so this is safe to rerun.
import json
import os
import sys

mirror = sys.argv[1]
p2_root = os.path.join(mirror, "p2")
for root, _dirs, files in os.walk(p2_root):
    for fname in files:
        if not fname.endswith(".json"):
            continue
        path = os.path.join(root, fname)
        with open(path) as f:
            data = json.load(f)
        changed = False
        for name, versions in data.get("packages", {}).items():
            if "/" not in name:
                continue
            vendor, pkg = name.split("/", 1)
            for v in versions:
                dist = v.get("dist")
                if not dist or not dist.get("reference"):
                    continue
                new_url = f"http://127.0.0.1:__PORT__/dists/{vendor}/{pkg}/{dist['reference']}.zip"
                if dist.get("url") != new_url:
                    dist["url"] = new_url
                    changed = True
        if changed:
            with open(path, "w") as f:
                json.dump(data, f)
PY

python3 "$index_py" "$mirror"
python3 "$rewrite_py" "$mirror"
