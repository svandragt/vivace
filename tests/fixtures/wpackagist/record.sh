#!/bin/sh
# Refreshes wpackagist.org/ under this fixture from the real wpackagist.org
# (#105: a v1 repository whose provider files are non-minified, keyed by
# version label rather than a plain list -- unlike every other v1 fixture
# here, which only ever exercised the array shape). Run via
# `make record-wpackagist`; needs `curl`, `jq` and `sha256sum`/`shasum`, and
# network access to wpackagist.org.
#
# wpackagist's own `provider-includes` files are megabytes each (every
# plugin/theme name it has ever served) and it eagerly loads every one of
# them on repository init (`loadProviderListings`), so this script trims
# both the provider-includes listing and the one provider file it points at
# down to a single small plugin, `wpackagist-plugin/akismet`, keeping only
# its first two versions -- then rewrites the sha256 hashes the trimmed
# files are addressed by, the same way the hand-built `satis/hand/providers`
# fixture invents hashes for its own trimmed content.
set -eu

root="$(cd "$(dirname "$0")" && pwd)/wpackagist.org"
sha256() { (sha256sum 2>/dev/null || shasum -a 256) | cut -d' ' -f1; }

root_json="$(curl -sS https://wpackagist.org/packages.json)"

# Find the one provider-includes file that lists akismet's hash.
includes_path=""
akismet_hash=""
for entry in $(printf '%s' "$root_json" | jq -r '.["provider-includes"] | keys[]'); do
    hash=$(printf '%s' "$root_json" | jq -r --arg k "$entry" '.["provider-includes"][$k].sha256')
    path=$(printf '%s' "$entry" | sed "s/%hash%/$hash/")
    candidate=$(curl -sS "https://wpackagist.org/$path" | jq -r '.providers["wpackagist-plugin/akismet"].sha256 // empty')
    if [ -n "$candidate" ]; then
        includes_path="$entry"
        akismet_hash="$candidate"
        break
    fi
done
[ -n "$includes_path" ] || { echo "wpackagist-plugin/akismet not found in any provider-includes file" >&2; exit 1; }

mkdir -p "$root/p/wpackagist-plugin"

# Trimmed provider file: only akismet's first two versions.
provider_json=$(curl -sS "https://wpackagist.org/p/wpackagist-plugin/akismet\$${akismet_hash}.json" \
    | jq -c '{packages: {"wpackagist-plugin/akismet": (.packages["wpackagist-plugin/akismet"] | to_entries | .[0:2] | from_entries)}}')
provider_hash=$(printf '%s' "$provider_json" | sha256)
printf '%s' "$provider_json" >"$root/p/wpackagist-plugin/akismet\$${provider_hash}.json"

# Trimmed provider-includes listing: only akismet's entry, pointing at the
# provider file's new hash.
includes_json=$(jq -n --arg hash "$provider_hash" \
    -c '{providers: {"wpackagist-plugin/akismet": {sha256: $hash}}}')
includes_hash=$(printf '%s' "$includes_json" | sha256)
mkdir -p "$root/p"
printf '%s' "$includes_json" >"$root/p/providers-fixture\$${includes_hash}.json"

# Root packages.json: real shape, trimmed to the one provider-includes file
# above.
jq -nc --arg hash "$includes_hash" '{
    packages: {},
    "providers-url": "/p/%package%$%hash%.json",
    "provider-includes": {"p/providers-fixture$%hash%.json": {sha256: $hash}},
    "available-package-patterns": ["wpackagist-plugin/*", "wpackagist-theme/*"]
}' >"$root/packages.json"

echo "wpackagist.org/ refreshed (wpackagist-plugin/akismet, 2 versions)."
