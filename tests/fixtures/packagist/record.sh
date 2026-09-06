#!/bin/sh
# Refreshes the recorded Packagist v2 fixtures under
# repo.packagist.org/ that tests/repository.rs replays offline. Run via
# `make record-packagist`; Packagist gains releases continuously, so expect
# this to touch the fixture files and the monolog/psr-log version
# assertions in tests/repository.rs from time to time.
set -eu

root="$(cd "$(dirname "$0")" && pwd)/repo.packagist.org"
mkdir -p "$root/p2/monolog" "$root/p2/psr"

curl -sS https://repo.packagist.org/packages.json -o "$root/packages.json"
curl -sS https://repo.packagist.org/p2/monolog/monolog.json -o "$root/p2/monolog/monolog.json"
curl -sS https://repo.packagist.org/p2/monolog/monolog~dev.json -o "$root/p2/monolog/monolog~dev.json"
curl -sS https://repo.packagist.org/p2/psr/log.json -o "$root/p2/psr/log.json"
