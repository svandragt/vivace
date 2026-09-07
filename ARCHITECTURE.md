# Architecture

vivace (`viv`) is a from-scratch, byte-compatible reimplementation of
Composer: it solves `composer.json` into a `composer.lock`, and installs a
lock into a `vendor/` directory that is a drop-in for Composer's own. It is
to Composer what uv is to pip: same inputs and outputs, most of the speed
comes from how packages are fetched, stored, and materialised, and from a
solver built for throughput rather than PHP's object model.

## Pipeline

```
composer.json ──► repository metadata ──► pool ──► PoolOptimizer ──► CDCL solver
                                                                          │
                                                                          ▼
                                                     transaction ──► lock_writer ──► composer.lock

composer.json + composer.lock
        │
        ▼
  lock::read ──► Vec<Package>, plugins::resolve (native adapter or refusal)
        │
        ▼
  plan: lock vs vendor/composer/installed.json + a small state file
        │   unchanged content-hash/composer.json/--no-dev, no orphans → no-op
        ▼
  fetch (tokio + reqwest) ──► bytes, sha1 checked; or source.rs for path/git
        ▼
  store: extract once into $XDG_CACHE_HOME/vivace/, read-only, `.ok`-marked
        ▼
  link: hardlink every store file into vendor/<vendor>/<name>/ (copy fallback)
        ▼
  autoload + bin: vendor/autoload.php, vendor/composer/*, vendor/bin/*
        ▼
  scripts: pre/post-install-cmd, pre/post-autoload-dump
```

`viv install` runs the lower half only, from an existing lock. `viv update`,
`viv require` and `viv remove` run the solver first and write a new lock;
`viv require`/`viv remove` stop at the lock and do not chain into install.

## Modules

| Module | Job |
|---|---|
| `lock` | Parse `composer.lock` and the root `composer.json` (`autoload`, `autoload-dev`, `config`). Keeps JSON key order (`serde_json` `preserve_order`) because `installed.json` re-emits lock entries. |
| `repository` | Packagist v2 and v1 (Satis/Private Packagist) metadata clients, multi-repository construction from `composer.json`'s `repositories`, an HTTP cache mirroring Composer's disk format. |
| `solver` | Port of Composer's CDCL dependency solver (`pool`, `pool_builder`, `pool_optimizer`, `rule_set_generator`, `rules`, `watch_graph`, `decisions`, `policy`, `solver`, `transaction`, `request`). Full updates only; `viv install` never reaches it. |
| `lock_writer` | Writes `composer.lock` from a solved transaction: top-level key order, `content-hash`, per-package `ArrayDumper` shape. |
| `require` | `viv require`/`viv remove`: constraint synthesis and a format-preserving `composer.json` edit, then a partial update of the touched package(s). |
| `update` | Wires repositories, the solver and `lock_writer` together for `viv update`/`viv update --lock` (`viv update-lock`'s alias). |
| `plan` | Diff the lock against `vendor/composer/installed.json` to decide what to keep, install, and remove. |
| `plugins` | Native adapters for the Composer plugins vivace ports: `composer/installers` and `*-wordpress-core-installer` (install path mapping), `dealerdirect/phpcodesniffer-composer-installer`, `phpstan/extension-installer`, `tbachert/spi` (post-install generators), `cweagans/composer-patches` (patch application). Everything else of type `composer-plugin` is refused unless `--no-plugins`. |
| `source` | Path-repository and dist-less git-source lock entries: symlink/mirror a path package, clone-and-checkout a git one, bypassing the store. |
| `fetch` | Download `dist` archives. Zip only. Bounded concurrency, HTTP/2, follows GitHub API redirects to codeload. Verifies `dist.shasum` (sha1) when non-empty. |
| `auth` | Composer-compatible credentials: `auth.json` (Composer home, then project), then `COMPOSER_AUTH`, ascending precedence. |
| `store` | Global content-addressed store keyed by the archive's sha256, plus a `dists-v0/<vendor>/<name>/<ref>` symlink layer and a `.classmap-v0` sidecar caching the extracted tree's classmap scan. Extraction strips a single top-level directory the way Composer does, rejects zip-slip paths, preserves exec bits, writes read-only files. |
| `link` | Materialise a store tree into `vendor/`. Hardlink per file; falls back to copy across devices or with `--link-mode copy`. |
| `autoload` | Generate the autoloader files (`generator`, `php`, `sort`, `classmap`, `installed`, `platform`). Embeds Composer's `ClassLoader.php`, `InstalledVersions.php`, and `LICENSE` verbatim from `templates/`. |
| `bin` | `vendor/bin` proxies: a port of Composer's `BinaryInstaller` for Unix proxy scripts (PHP and `sh` shapes; no Windows `.bat` writer). |
| `scripts` | Dispatches the four events `install`/`dump-autoload` care about (`pre-install-cmd`, `pre-autoload-dump`, `post-autoload-dump`, `post-install-cmd`) from the root package's own `scripts` section. |
| `install` | Wires lock parsing, plugins, planning, fetch/store/link, autoload and scripts together for `viv install`, plus cache maintenance (`viv cache prune/clean/size`) and `viv dump-autoload`. |
| `show` | `viv show`/`viv tree`/`viv why`/`viv outdated`: read-only inspection of installed packages. |
| `audit` | `viv audit`: security advisories and abandoned packages from Packagist's security-advisories API. |
| `validate` | `viv validate`: a native `ConfigValidator`/`ValidatingArrayLoader` port for `composer.json`, plus lock freshness/completeness checks. |
| `normalize` | `viv normalize`: a native `ergebnis/composer-normalize` for `composer.json`'s key order and formatting. |
| `tool` | `viv x`/`viv run`/`viv exec`: npx-style one-off tool execution, `scripts::Runner` entry points, and a bare `vendor/bin` exec. |
| `version`, `semver` | Composer version normalisation and constraint parsing/matching, shared by the solver, `show`, and the autoloader's version dumps. |
| `bin.rs` (crate root, `viv`) | The CLI: `install`, `update`/`update-lock`, `require`, `remove`, `dump-autoload`, `normalize`, `cache`, `audit`, `show`/`tree`/`why`/`outdated`, `validate`, `x`, `run`, `exec`, plus `--offline` and `--cache-dir`. |

## Why a store and hardlinks

Composer and Riff cache archives and unzip them into `vendor/` on every
install. Unzipping is the dominant cost once downloads are cached (Riff's warm
install on a 101-package Laravel lock spends most of its 230 ms in system
calls from extraction). A store holds each package extracted exactly once.
A warm install then does one `link()` per file, which on ext4 is an order of
magnitude cheaper than inflating and writing it. The same store serves every
project on the machine.

Hardlinks share inodes, so an edit to a vendor file would change the store.
Store files are therefore read-only (0444), which makes hardlinked vendor
files read-only too. Editing fails loudly rather than corrupting other
projects. Projects that patch vendor use `--link-mode copy`, which adds the
owner write bit back so the copied files can be edited directly.

## The no-op path

`vendor/composer/.vivace-state` records the lock's `content-hash`, the
`--no-dev` flag, and a hash of the root `composer.json`. When these match and
`installed.json` has no orphans, `viv install` regenerates nothing and
touches no network. Target: tens of milliseconds.

## Compatibility rules

Everything in `vendor/composer/` follows `docs/composer-contract.md`. The
integration tests install the fixtures in `tests/fixtures/` and diff the
result against `expected/`, produced by real Composer. Differences are bugs,
not style. `compat/` runs the same comparison against a corpus of real
projects before a release.

## Not supported

Each of these fails with a clear error, naming the reason:

- Any `composer-plugin` package without a native adapter, unless
  `config.allow-plugins` is `false`/absent for it — `--no-plugins` downgrades
  the refusal to a warning and installs as Composer would with that flag
  (`docs/plugin-strategy.md`).
- `--minimal-changes`: the flag parses but is not wired into the solver yet,
  so it currently no-ops (`src/solver/policy.rs`).
- `vcs` repositories in `viv update`/`require`/`remove` (#97).
- A non-git VCS-type dist or source (Mercurial, Subversion, Fossil).

## Benchmarks

`bench/run.sh <project> [tools]` runs hyperfine across scenarios: cold
(no cache, no vendor), warm (cache, no vendor), no-op (vendor present), and
`update-warm` (resolve with a warm metadata cache). `bench/laravel/` holds a
101-package lock. Results and methodology live in `bench/results/`.
