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
make fuzz                          # cargo-fuzz, 30s per target (#44); nightly toolchain required
make coverage                      # cargo-llvm-cov via nextest, per-file report, no threshold (#45)
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

## Performance rule

No change may make `viv` slower. A feature that touches the install path is
benchmarked before it lands (`make bench`, or `bench/run.sh bench/laravel
viv` with an isolated cache on the same filesystem as `vendor/`) and
compared with `bench/results/`. Warm and no-op times must be at or below the
recorded numbers within noise; cold is recorded but not gated, because
GitHub throttles repeated cold runs. If a feature regresses, optimise it
back before committing, or do not commit.

CI enforces the warm/no-op half of this rule on every push and pull request:
the `bench` job runs `bench/run.sh` on `tests/fixtures/monolog` (a runner-sized
project, unlike the local `bench/laravel` numbers above) and
`bench/compare.py` fails the job if either mean rises more than 15% above
`bench/results/baseline.json`. That baseline is measured on GitHub's runners,
not Sander's machine, so it isn't written by a local run: a maintainer
downloads the `baseline-candidate` artifact from a green CI run and commits it
as `bench/results/baseline.json`. Run `make bench-check` to reproduce the same
check locally against the committed baseline.
