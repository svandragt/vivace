#!/bin/sh
# Refreshes psr-repo/ and psr-project/composer.lock: a real
# `composer/satis` build for psr/log and psr/container, served locally, and
# a real `composer update` against it. Run via `make record-satis`; needs
# `devbox run` (php, composer) and network access to Packagist for the
# `composer create-project` and the Satis build itself.
#
# `composer/satis` (current `dev-main`) no longer writes the classic
# `providers-url`/`provider-includes` protocol at all — it writes
# `metadata-url` (v2) plus a legacy `includes` fallback. The
# `providers-url`/`provider-includes` path (`v1_providers_url_*` in
# `tests/repository.rs`) is exercised by hand-built fixtures under `hand/`
# instead, since no installable Satis version emits it to record from.
set -eu

root="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'kill "${server_pid:-0}" 2>/dev/null || true; rm -rf "$work"' EXIT

satis="$work/satis"
composer create-project composer/satis "$satis" --stability=dev --no-interaction

port=8791
cat > "$satis/satis-fixture.json" <<EOF
{
    "name": "vivace/satis-providers-fixture",
    "homepage": "http://127.0.0.1:$port",
    "repositories": [{"type": "composer", "url": "https://repo.packagist.org"}],
    "require-all": false,
    "require": {"psr/log": "^3.0", "psr/container": "^2.0"},
    "output-html": false
}
EOF
php "$satis/bin/satis" build "$satis/satis-fixture.json" "$work/output" --no-interaction

php -S "127.0.0.1:$port" -t "$work/output" >"$work/php-server.log" 2>&1 &
server_pid=$!
sleep 1

project="$work/project"
mkdir -p "$project"
cat > "$project/composer.json" <<EOF
{
    "name": "vivace/satis-fixture",
    "require": {"psr/log": "^3.0", "psr/container": "^2.0"},
    "repositories": [
        {"type": "composer", "url": "http://127.0.0.1:$port"},
        {"packagist.org": false}
    ],
    "config": {"secure-http": false}
}
EOF
composer -d "$project" update --no-interaction

kill "$server_pid" 2>/dev/null || true

rm -rf "$root/psr-repo/127.0.0.1"
mkdir -p "$root/psr-repo/127.0.0.1"
cp -r "$work/output/." "$root/psr-repo/127.0.0.1/"
cp "$project/composer.json" "$root/psr-project/composer.json"
cp "$project/composer.lock" "$root/psr-project/composer.lock"

echo "psr-repo/127.0.0.1 and psr-project/composer.{json,lock} refreshed."
