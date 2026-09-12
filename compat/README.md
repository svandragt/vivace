# Compatibility sweep

Installs a corpus of real-world projects with both Composer and `viv`, and
byte-diffs the resulting `vendor/`. Run before tagging a release
(`JOURNAL.md` records the result). See #49 for the design.

The GitHub Actions workflow also runs the sweep every Monday, using the run
ID as the random sample's seed, in addition to running on a tag push or
manual dispatch. Every run uploads the report and per-project logs as a
`compat-report` artifact retained for 90 days; a scheduled run that fails
opens or updates a `compat` issue titled "Compat sweep drift: <date>" with a
link to the run and the report's first 40 lines.

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
The default scratch (the `mktemp -d` one) is removed on exit; set
`COMPAT_SCRATCH` to keep it around.

Useful environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `COMPAT_SCRATCH` | `mktemp -d` | Root for caches and checkouts |
| `COMPAT_CORPUS` | `compat/corpus.toml` | Corpus file to sweep, for a local, untracked file with `path` entries |
| `COMPAT_SEED` | `date +%Y%m%d` | Seed for the random sample's `shuf` |
| `COMPAT_RANDOM` | `10` | Number of random packages to sample |
| `COMPAT_ONLY` | (unset) | Comma-separated project/package names to run, skipping the rest |
| `VIV` | `target/release/viv` | Binary under test |
| `COMPAT_RESULTS_DIR` | `compat/results` | Where the report, logs and sample cache are written |
| `COMPAT_AUTH_FILE` | (unset) | Path to an `auth.json` to copy into the scratch `COMPOSER_HOME`, for corpus entries behind a private registry |
| `COMPAT_SKIP_PLUGINS` | (unset) | Set to `1` to skip a project whose lock requires a Composer plugin instead of running it (see below) |
| `COMPAT_SKIP_VCS` | (unset) | Set to `1` to skip a project whose `composer.json` declares a `path` or `vcs` repository (#13) instead of running it anyway |
| `COMPAT_LOCKS` | (unset) | Set to `1` to also resolve every pinned project's `composer.json` with both tools and byte-diff the two `composer.lock` files (#180); see below |

The report's header prints the Composer flags used
(`--no-scripts --no-plugins --no-interaction` for `install`, plus
`--ignore-platform-reqs` for the `update --no-install` calls that generate a
missing lock — not for `install`, since that flag changes what Composer
writes and would break the byte-diff) and the random sample's seed. A
project's lock can drop `--no-plugins` from that default; see below. A
project/mode combination is one of:

- **identical** — Composer and viv produced byte-identical `vendor/` trees.
  Files that differ only in the absolute install path (some plugin-generated
  files, such as phpstan/extension-installer's `GeneratedConfig.php`, embed
  it) are normalised before this comparison, and the Details cell notes how
  many were dropped. `vendor/yiisoft/extensions.php` and
  `vendor/craftcms/plugins.php` are compared as sorted lines, because the
  real plugins write entries as each archive finishes extracting and two
  Composer runs on one lock disagree on the order.
- **differs** — the trees diverge; the first ten differing paths are listed.
- **skipped** — a known-unsupported feature, an unmet platform requirement,
  or a Composer command that failed, not a failure: a `path`/`vcs` repository
  under `COMPAT_SKIP_VCS=1` (#13), a required plugin under
  `COMPAT_SKIP_PLUGINS=1` (#12), `preferred-install: source` (#43), an unmet
  platform requirement (`platform: ...`), a pinned corpus entry whose `git
  clone`/checkout failed (`clone failed: ...`), or a missing `composer.lock`
  that `composer update --no-install` also failed to generate. A project without
  a committed lock that *does* generate one is still run; its Details
  column notes `lock generated`.
- **viv error** — `viv install` itself exited non-zero; the stderr tail is
  recorded.

The sweep exits non-zero if any project **differs** or errors; **skipped**
does not fail the run.

Every enabled plugin a project's lock declares (per its `composer.json`'s
`config.allow-plugins`) is checked against `viv`'s native adapters and known-
inert plugins (`src/plugins/mod.rs`). If every one is native or inert, both
Composer and viv run *without* `--no-plugins` for that project only, so the
adapter's real output gets byte-diffed instead of skipped; its Details column
notes `plugins: native`. Any other enabled plugin keeps the default
`--no-plugins` on both sides, and Details notes `plugins: refused <names>`
(#124) — unless `COMPAT_SKIP_PLUGINS=1`, which skips the project outright
instead, as before. The report ends with a summary line, `N of M projects
would refuse without --no-plugins`, counting every project with at least one
refused plugin, independently of `COMPAT_SKIP_PLUGINS`.

A `composer failed`/`composer update failed` Details cell shows only the
last three non-empty lines; the full combined output is saved per
project/mode under `compat/results/<label>-logs/` (gitignored).

The random sample's candidate list (`popular.json` plus `list.json`) is
cached at `compat/results/<label>.sample.json` on first fetch and reused on
a later run with the same label, so a label always samples from the same
candidates.

## Lock compare (`COMPAT_LOCKS=1`)

The install comparison above only ever byte-diffs `vendor/` from a lock
Composer generated; it never checks whether `viv update` would have
resolved that lock itself. `COMPAT_LOCKS=1` adds a second table that does:
for every pinned project, it resolves the same `composer.json` with
`composer update --no-install --no-scripts --no-plugins` and `viv update
--no-install --no-plugins`, then byte-diffs the two `composer.lock` files
(#180).

Both tools resolve against a `bench/mirror.sh` recording of the project's
own already-resolved packages, served over `127.0.0.1`, so neither sees
live Packagist — including the `security-advisories` response, replayed
from a recording by a small stdlib HTTP server since Composer's default
`update` also audits the resolved lock. Neither tool gets
`--ignore-platform-reqs`; the sweep already has to run inside devbox for
`composer`/`php` to resolve at all, so both tools already see the same
real PHP and extensions.

A project's lock is one of:

- **identical** — the two locks match once `_readme` and
  `plugin-api-version` are dropped from both (Composer always writes its own
  version banner into `_readme`, and the two tools' plugin API versions can
  legitimately differ).
- **differs** — a real difference; the first ten diff lines are listed.
- **skipped** — no checkout, no `composer.lock` to seed the mirror
  recording from, the mirror recording failed, or `composer update` itself
  failed to write a lock.
- **viv error** — `viv update` itself exited non-zero.

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

Cloned checkouts keep their `.git` directory, so Composer's root-version guess
from the checkout state matches what a real user would see (#125).
`path`-based entries still have `.git` stripped, since a local checkout's git
state isn't reproducible.

For a project that's assembled via `composer create-project` rather than
cloned (no installable git tree), use `version` instead of `repo`/`commit`.
`make compat-refresh` rewrites every `repo`-based pin to its current
default-branch head; re-run and commit the result to update the corpus.

Use `path` instead of `repo`/`commit`/`version` to sweep a project that
already lives on disk, such as a client codebase you don't want named in this
repo. The sweep copies it into scratch (excluding `vendor/`, `node_modules/`
and `.git/`) and never modifies the original; keep any such entries in a
local, untracked corpus file passed via `COMPAT_CORPUS`.
