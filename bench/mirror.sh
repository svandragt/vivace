#!/usr/bin/env sh
# Records one project's dists and p2 metadata into a local mirror so
# bench/run.sh's cold and update-warm scenarios need no network (#165,
# widened; #171 widened further to every repository, not just Packagist).
# The recorded p2 files are then rewritten so every dist URL points at a
# placeholder local server address (http://127.0.0.1:__PORT__/...) that
# bench/run.sh fills in with the port it actually picks at serve time.
#
# Usage: bench/mirror.sh <project-dir> <mirror-dir>
#
# #171: metadata is recorded from every repository the project's
# composer.json names, not just Packagist. For each locked package (plus
# everything in require/require-dev), Packagist is tried first (unless
# disabled with a `{"packagist.org": false}` repositories entry), then each
# `"composer"`-type repository in the order composer.json lists them,
# stopping at the first that has the package. A v2 repository (`metadata-url`)
# is recorded as-is. A v1 repository (`providers-url`/`provider-includes`,
# e.g. asset-packagist.org) has its provider listing walked once per
# repository to find the package's hash, and the resulting per-package
# `packages` object is written straight into the mirror's own p2 shape — see
# `src/repository.rs`'s `parse_provider_versions_sync`, which accepts that
# same version-keyed shape.
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
  rc=0
  case $url in
    https://api.github.com/*|https://github.com/*|https://codeload.github.com/*)
      if [ -n "$token" ]; then
        curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 -H "Authorization: Bearer $token" -o "$dest.tmp" "$url" || rc=$?
      else
        curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 -o "$dest.tmp" "$url" || rc=$?
      fi
      ;;
    *) curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 -o "$dest.tmp" "$url" || rc=$? ;;
  esac
  # Explicit rc, not just curl's own exit status: a caller using fetch as an
  # `if` condition (record_p2's per-repository fallback, #171) runs under a
  # `set -e` that's suspended for the whole function body, so falling
  # through to `mv` after a failed curl would move a nonexistent .tmp file
  # right past the failure instead of reporting it.
  [ "$rc" -eq 0 ] || return "$rc"
  mv "$dest.tmp" "$dest"
}

# Joins a repository base URL to a path from its packages.json
# (metadata-url/providers-url/provider-includes keys are sometimes absolute,
# sometimes relative) — not a full RFC 3986 join, just enough for the two
# shapes Packagist-alike repositories actually send.
join_url() {
  case $2 in
    http://*|https://*) printf '%s' "$2" ;; # already absolute (Packagist's own metadata-url)
    /*) printf '%s%s' "${1%/}" "$2" ;;
    *)  printf '%s/%s' "${1%/}" "$2" ;;
  esac
}

record_dist() {
  name=$1 url=$2 key=$3
  [ -n "$url" ] && [ "$url" != "null" ] || return 0
  vendor=${name%%/*} pkg=${name#*/}
  fetch "$url" "$mirror/dists/$vendor/$pkg/$key.zip"
}

workdir=$(mktemp -d)
index_py=$(mktemp)
rewrite_py=$(mktemp)
v1_listing_py=$(mktemp)
listing_py=$(mktemp)
missing_py=$(mktemp)
trap 'rm -rf "$workdir"; rm -f "$index_py" "$rewrite_py" "$v1_listing_py" "$listing_py" "$missing_py"' EXIT

cat > "$v1_listing_py" <<'PY'
# Walks a v1 repository's provider-includes tree breadth-first
# (ComposerRepository::loadProviderListings, mirrored in src/repository.rs's
# load_provider_listing) and writes every package name it lists, tab-
# separated with its providers-url hash, to $3. Fetches through curl itself
# (same retry flags as mirror.sh's own fetch()) rather than shelling back
# into it, since this is the one place the script needs real JSON recursion.
import json
import os
import subprocess
import sys
from collections import deque

repo_base, workdir, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
includes = json.loads(sys.stdin.read())


def fetch(url, dest):
    if os.path.exists(dest):
        return True
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    tmp = dest + ".tmp"
    rc = subprocess.run(
        ["curl", "-fsSL", "--retry", "5", "--retry-all-errors", "--retry-delay", "2", "-o", tmp, url]
    ).returncode
    if rc != 0:
        return False
    os.rename(tmp, dest)
    return True


def join(base, path):
    if path.startswith("http://") or path.startswith("https://"):
        return path
    return f"{base.rstrip('/')}{path}" if path.startswith("/") else f"{base.rstrip('/')}/{path}"


listing = {}
queue = deque(includes.items())
while queue:
    path, meta = queue.popleft()
    sha = meta.get("sha256")
    if not sha:
        continue
    file_path = path.replace("%hash%", sha)
    cache = os.path.join(workdir, "incl-" + file_path.replace("/", "_").replace("%", "_"))
    if not fetch(join(repo_base, file_path), cache):
        continue
    with open(cache) as f:
        data = json.load(f)
    for name, entry in (data.get("providers") or {}).items():
        h = entry.get("sha256")
        if h:
            listing[name.lower()] = h
    for k, v in (data.get("provider-includes") or {}).items():
        queue.append((k, v))

with open(out_path, "w") as f:
    for name, h in listing.items():
        f.write(f"{name}\t{h}\n")
PY

# Builds the ordered repository list (Packagist first, unless disabled, then
# composer.json's "composer"-type repositories in listed order) into
# $workdir/repo-<i>.*: .base/.kind always, plus .metadata_url (v2) or
# .providers_url/.listing (v1). Only metadata-url and providers-url/
# provider-includes are handled — Satis's older eager includes/inline
# packages output isn't in scope, neither WPackagist-alike nor
# asset-packagist.org repositories use it.
repo_count=0
add_repo() {
  base=$1
  root="$workdir/root-$repo_count.json"
  fetch "$(join_url "$base" "/packages.json")" "$root" || return 0
  metadata_url=$(jq -r '.["metadata-url"] // empty' "$root")
  if [ -n "$metadata_url" ]; then
    echo "$base" > "$workdir/repo-$repo_count.base"
    echo "v2" > "$workdir/repo-$repo_count.kind"
    echo "$metadata_url" > "$workdir/repo-$repo_count.metadata_url"
    repo_count=$((repo_count + 1))
    return 0
  fi
  providers_url=$(jq -r '.["providers-url"] // empty' "$root")
  if [ -n "$providers_url" ]; then
    echo "$base" > "$workdir/repo-$repo_count.base"
    echo "v1" > "$workdir/repo-$repo_count.kind"
    echo "$providers_url" > "$workdir/repo-$repo_count.providers_url"
    jq -c '.["provider-includes"] // {}' "$root" |
      python3 "$v1_listing_py" "$base" "$workdir" "$workdir/repo-$repo_count.listing"
    repo_count=$((repo_count + 1))
  fi
}

packagist_disabled=false
composer_repos_file="$workdir/composer-repos.txt"
: > "$composer_repos_file"
if [ -f "$proj/composer.json" ]; then
  packagist_disabled=$(jq -r '
    (.repositories // []) | if type == "array"
      then any(.[]; type == "object" and has("packagist.org") and .["packagist.org"] == false)
      else false
    end' "$proj/composer.json")
  jq -r '(.repositories // []) | if type == "array"
    then .[] | select(.type == "composer") | .url
    else empty
  end' "$proj/composer.json" > "$composer_repos_file"
fi

[ "$packagist_disabled" = "true" ] || add_repo "https://repo.packagist.org"
while IFS= read -r url; do
  [ -n "$url" ] || continue
  add_repo "$url"
done < "$composer_repos_file"

record_p2() {
  name=$1
  case $name in */*) ;; *) return 0 ;; esac # skip php/ext-* virtual packages
  vendor=${name%%/*} pkg=${name#*/}
  # Not "dest": fetch() assigns its own $1/$2 straight into the caller's
  # shell (sh has no function-local scoping), so a name shared with fetch()'s
  # own "dest" parameter gets silently clobbered the moment the v1 branch
  # below calls fetch() for something other than this file (#171 regression
  # caught by an end-to-end run, not the syntax check).
  p2_path="$mirror/p2/$vendor/$pkg.json"
  [ -f "$p2_path" ] && return 0
  i=0
  while [ "$i" -lt "$repo_count" ]; do
    base=$(cat "$workdir/repo-$i.base")
    case $(cat "$workdir/repo-$i.kind") in
      v2)
        meta=$(cat "$workdir/repo-$i.metadata_url")
        path=$(printf '%s' "$meta" | sed "s#%package%#$name#")
        if fetch "$(join_url "$base" "$path")" "$p2_path"; then
          # A missing dev-branch file (no default-branch releases) is
          # normal; don't fail the whole recording over it.
          devpath=$(printf '%s' "$meta" | sed "s#%package%#$name~dev#")
          fetch "$(join_url "$base" "$devpath")" "$mirror/p2/$vendor/$pkg~dev.json" || true
          return 0
        fi
        ;;
      v1)
        listing="$workdir/repo-$i.listing"
        hash=""
        [ -f "$listing" ] && hash=$(awk -F'\t' -v n="$name" '$1 == n { print $2; exit }' "$listing")
        if [ -n "$hash" ]; then
          purl=$(cat "$workdir/repo-$i.providers_url")
          ppath=$(printf '%s' "$purl" | sed "s#%package%#$name#; s#%hash%#$hash#")
          cache="$workdir/prov-$i-$(printf '%s' "$name" | tr '/' '_').json"
          if fetch "$(join_url "$base" "$ppath")" "$cache" &&
             jq -e --arg n "$name" '(.packages // {})[$n]' "$cache" >/dev/null 2>&1; then
            # v1 keys each entry by version label (an object), the same
            # shape src/repository.rs's Provider::Providers arm accepts
            # directly; flattened to a plain list here anyway so every
            # recorded p2 file — v1 or v2 sourced — has the one shape the
            # rewrite step below already assumes.
            jq -c --arg n "$name" \
              '{packages: {($n): ((.packages // {})[$n] | if type == "object" then [.[]] else . end)}}' \
              "$cache" > "$p2_path"
            return 0
          fi
        fi
        ;;
    esac
    i=$((i + 1))
  done
}

lock="$proj/composer.lock"
cat > "$listing_py" <<'PY'
# Emits name/url/key for every locked dist, tab-separated, so the shell loop
# below can record each one without re-deriving the key itself.
import hashlib
import json
import sys


def dist_key(url, reference):
    # #173: reference when the lock has one, else the sha1 of the dist URL —
    # WPackagist-style dists (SVN export, no reference) still get a stable
    # key this way. Must match bench/run.sh's dist_key.
    if reference:
        return reference
    return hashlib.sha1(url.encode()).hexdigest()


with open(sys.argv[1]) as f:
    lock = json.load(f)
for key in ("packages", "packages-dev"):
    for pkg in lock.get(key, []):
        dist = pkg.get("dist") or {}
        url = dist.get("url") or ""
        if not url:
            continue
        print(f"{pkg['name']}\t{url}\t{dist_key(url, dist.get('reference') or '')}")
PY
python3 "$listing_py" "$lock" |
while IFS="$(printf '\t')" read -r name url key; do
  [ -n "$name" ] || continue
  record_dist "$name" "$url" "$key"
  record_p2 "$name"
done

cat > "$missing_py" <<'PY'
# Asserts every locked dist landed under dists/ (#173): a miss here fails
# the recording instead of silently falling through to the network (or,
# under unshare -rn, failing bench/run.sh's cold install instead).
import hashlib
import json
import os
import sys


def dist_key(url, reference):
    # Must match bench/run.sh's dist_key.
    if reference:
        return reference
    return hashlib.sha1(url.encode()).hexdigest()


mirror, lock_path = sys.argv[1], sys.argv[2]
with open(lock_path) as f:
    lock = json.load(f)
missing = []
for section in ("packages", "packages-dev"):
    for pkg in lock.get(section, []):
        dist = pkg.get("dist") or {}
        url = dist.get("url") or ""
        if not url:
            continue
        vendor, name = pkg["name"].split("/", 1)
        key = dist_key(url, dist.get("reference") or "")
        path = os.path.join(mirror, "dists", vendor, name, f"{key}.zip")
        if not os.path.isfile(path):
            missing.append(f"{pkg['name']} -> {path}")
if missing:
    for m in missing:
        print(f"mirror.sh: missing dist: {m}", file=sys.stderr)
    sys.exit(1)
PY
python3 "$missing_py" "$mirror" "$lock"

if [ -f "$proj/composer.json" ]; then
  jq -r '((.require // {}) + (."require-dev" // {})) | keys[]' "$proj/composer.json" |
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    record_p2 "$name"
  done
fi

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
# Derives the URL from the package name and dist key every time (never from
# what's already on disk), so this is safe to rerun.
import hashlib
import json
import os
import sys


def dist_key(url, reference):
    # #173: reference when the lock has one, else the sha1 of the dist URL —
    # WPackagist-style dists (SVN export, no reference) still get a stable
    # key this way. Must match bench/run.sh's dist_key.
    if reference:
        return reference
    return hashlib.sha1(url.encode()).hexdigest()


PREFIX = "http://127.0.0.1:__PORT__/dists/"


def existing_key(url):
    # A p2 file already rewritten by an earlier mirror.sh run has its dist
    # url pointing at the mirror itself; a no-reference dist's key must be
    # read back off that url rather than re-derived (sha1 of the mirror's
    # own url, not the original remote one, would drift on every rerun).
    if url.startswith(PREFIX) and url.endswith(".zip"):
        return url[len(PREFIX):-len(".zip")].rsplit("/", 1)[-1]
    return None


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
            # A repository's own metadata-url can serve either shape (v2's
            # usual plain list, or a version-keyed object like v1's/
            # wp-packages.org's) — src/repository.rs accepts both
            # (parse_provider_versions_sync), so this does too rather than
            # assuming every recorded file is a list.
            if isinstance(versions, dict):
                versions = versions.values()
            for v in versions:
                dist = v.get("dist")
                if not dist or not dist.get("url"):
                    continue
                key = dist.get("reference") or existing_key(dist["url"]) or dist_key(dist["url"], "")
                new_url = f"{PREFIX}{vendor}/{pkg}/{key}.zip"
                if dist.get("url") != new_url:
                    dist["url"] = new_url
                    changed = True
        if changed:
            with open(path, "w") as f:
                json.dump(data, f)
PY

python3 "$index_py" "$mirror"
python3 "$rewrite_py" "$mirror"

# Records the security-advisories response for this lock's packages (#180),
# so a lock-compare run sees the same advisory data Composer and viv would
# each fetch live, instead of the two tools silently agreeing on "none"
# because the mirror doesn't advertise the endpoint at all. Composer's own
# SecurityAdvisoryPoolFilter asks every advertising repository the same
# full package-constraint map and merges the responses (this file's
# fetch_advisories_from doc says the same); every corpus project so far
# advertises at most one, so only the first repository found with an
# `api-url` is recorded.
record_advisories() {
  [ -f "$lock" ] || return 0
  [ -f "$mirror/advisories.json" ] && return 0
  i=0
  api_url=""
  while [ "$i" -lt "$repo_count" ]; do
    api_url=$(jq -r '.["security-advisories"]["api-url"] // empty' "$workdir/root-$i.json" 2>/dev/null || true)
    [ -n "$api_url" ] && break
    i=$((i + 1))
  done
  [ -n "$api_url" ] || return 0
  set --
  while IFS= read -r pkg_name; do
    [ -n "$pkg_name" ] || continue
    set -- "$@" --data-urlencode "packages[]=$pkg_name"
  done <<EOF
$(jq -r '[(.packages // [])[], (."packages-dev" // [])[] | .name] | unique | .[]' "$lock")
EOF
  if curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 "$@" -o "$mirror/advisories.json.tmp" "$api_url"; then
    mv "$mirror/advisories.json.tmp" "$mirror/advisories.json"
  else
    rm -f "$mirror/advisories.json.tmp"
    echo "mirror.sh: could not record advisories from $api_url; lock-compare will run without advisory data" >&2
  fi
}
record_advisories
