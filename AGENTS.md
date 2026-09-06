# AGENTS.md

Guidance for coding agents working in this repository.

## What this is

vivace (`viv`) installs PHP dependencies from an existing `composer.lock` and
produces a `vendor/` directory that is a drop-in for Composer's. No dependency
solving in v0.1. Read `ARCHITECTURE.md` for the pipeline and
`docs/composer-contract.md` for the exact output rules. `JOURNAL.md` is a
public engineering log: append an entry at the end of every working session.

## Commands

PHP, Composer and hyperfine come from devbox, so run anything that needs them
through `devbox run`. The `Makefile` wraps the common ones:

```sh
make build                         # cargo build --release, binary at target/release/viv
make test                          # cargo nextest run (PHP-dependent tests skip without php)
make check                         # fmt --check, clippy -D warnings, nextest, cargo deny
make bench                         # hyperfine: composer vs riff vs viv on bench/laravel
make hooks                         # install the pre-commit hook that runs `make check`
make fixtures                      # regenerate the monolog fixture's expected Composer output
```

Fallback, or for anything not wrapped:

```sh
cargo build --release              # binary at target/release/viv
devbox run check                   # fmt, clippy -D warnings, nextest, cargo deny
devbox run test                    # cargo nextest run (PHP-dependent tests skip without php)
cargo nextest run -E 'test(classmap)'   # one test or filter
cargo insta review                 # accept or reject snapshot changes
devbox run bench                   # hyperfine: composer vs riff vs viv on bench/laravel
devbox run hooks                   # install the pre-commit hook that runs `check`
```

## Measuring

Measurements and manual tests use `--cache-dir` (viv) or an isolated
`XDG_CACHE_HOME`, never `rm -rf ~/.cache/vivace`. `bench/run.sh` follows this:
it points every tool's cache at a directory under `$BENCH_WORK` and never
touches the user's real cache.

Regenerate a fixture's Composer reference output (only when Composer's
behaviour changes):

```sh
devbox run -- composer -d tests/fixtures/monolog install
cp tests/fixtures/monolog/vendor/composer/*.php tests/fixtures/monolog/expected/dev/composer/
```

## Working method

Red/green TDD. Copy or write the failing test first (a golden from
`tests/fixtures/composer/`, a table, or a snapshot), run it red, implement the
smallest change to green, commit. Byte differences against Composer output are
bugs, not style.

## Layout

| Path | Role |
|---|---|
| `src/lock.rs`, `src/version.rs` | composer.lock and composer.json parsing, Composer version normalisation |
| `src/fetch.rs` | concurrent dist downloads, sha1 check |
| `src/store.rs` | global cache: `archive-v0/<sha256>/` extracted trees, `dists-v0/<vendor>/<name>/<ref>` pointers |
| `src/link.rs` | hardlink (fallback copy) from store into `vendor/` |
| `src/plan.rs` | diff lock against `vendor/composer/installed.json` |
| `src/autoload/` | autoloader generation; `templates/` holds Composer's verbatim files |
| `tests/fixtures/composer/` | upstream Composer test corpora, do not edit |
| `tests/fixtures/monolog/` | end-to-end fixture with Composer's expected output |
| `bench/` | hyperfine script, Laravel-sized lock, results |

## Conventions

Prose in the repo follows British English and the Google developer style
guide. Comments explain why, not what. Deliberate shortcuts carry a
`ponytail:` comment naming the ceiling.
