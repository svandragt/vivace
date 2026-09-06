# Compatibility sweep

Installs a corpus of real-world projects with both Composer and `viv`, and
byte-diffs the resulting `vendor/`. Run before tagging a release
(`JOURNAL.md` records the result). See #49 for the design.

Two sources feed the sweep:

- A pinned corpus (`compat/corpus.toml`): ten popular projects, each pinned
  to a commit (or, for `drupal/recommended-project`, a released version
  installed via `composer create-project`) so a run is reproducible.
- A random sample: packages drawn from Packagist's popular list and its full
  package list, seeded so a failure can be replayed.

## Running it

```sh
make build          # release viv binary
make compat          # runs compat/run.sh, writes compat/results/<tag>.md
```

Everything happens under a scratch directory (`mktemp -d`, or
`COMPAT_SCRATCH` to pick one) with its own viv cache and `COMPOSER_HOME` /
`COMPOSER_CACHE_DIR` — the real cache and Composer auth are never touched.

Useful environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `COMPAT_SCRATCH` | `mktemp -d` | Root for caches and checkouts |
| `COMPAT_SEED` | `date +%Y%m%d` | Seed for the random sample's `shuf` |
| `COMPAT_RANDOM` | `10` | Number of random packages to sample |
| `COMPAT_ONLY` | (unset) | Comma-separated project/package names to run, skipping the rest |
| `VIV` | `target/release/viv` | Binary under test |

The report's header prints the Composer flags used
(`--no-scripts --no-plugins --no-interaction` for `install`, plus
`--ignore-platform-reqs` for the `update --no-install` calls that generate a
missing lock — not for `install`, since that flag changes what Composer
writes and would break the byte-diff) and the random sample's seed. A
project/mode combination is one of:

- **identical** — Composer and viv produced byte-identical `vendor/` trees.
- **differs** — the trees diverge; the first ten differing paths are listed.
- **skipped** — a known-unsupported feature, an unmet platform requirement,
  or a Composer command that failed, not a failure: path/vcs repositories
  (#13), a required plugin (#12), `preferred-install: source` (#43), an
  unmet platform requirement (`platform: ...`), or a missing
  `composer.lock` that `composer update --no-install` also failed to
  generate. A project without a committed lock that *does* generate one is
  still run; its Details column notes `lock generated`.
- **viv error** — `viv install` itself exited non-zero; the stderr tail is
  recorded.

The sweep exits non-zero if any project **differs** or errors; **skipped**
does not fail the run.

## Replaying a seed

A random-sample failure prints its seed in the report header. Reproduce it
with:

```sh
COMPAT_SEED=<seed> COMPAT_RANDOM=<n> make compat
```

## Adding a pinned project

Add a `[[project]]` block to `compat/corpus.toml`:

```toml
[[project]]
name = "vendor/package"
repo = "https://github.com/vendor/package.git"
commit = "<pinned commit>"
```

For a project that's assembled via `composer create-project` rather than
cloned (no installable git tree), use `version` instead of `repo`/`commit`.
`make compat-refresh` rewrites every `repo`-based pin to its current
default-branch head; re-run and commit the result to update the corpus.
