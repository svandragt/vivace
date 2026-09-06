# Architecture

vivace (`viv`) installs PHP dependencies from an existing `composer.lock`,
producing a `vendor/` directory that is a drop-in for Composer's. It is to
Composer what uv's `pip sync` was to pip: no dependency solving in v0.1, all
the speed comes from how packages are fetched, stored, and materialised.

## Pipeline

```
composer.json + composer.lock
        │
        ▼
  lock::read ──► Vec<Package> (packages + packages-dev, --no-dev filters)
        │
        ▼
  plan: compare against vendor/composer/installed.json
        │   same name+reference+dev flag → keep; else install; orphans → remove
        ▼
  fetch (tokio + reqwest, N in flight) ──► bytes, sha1 checked when shasum set
        │
        ▼
  store: extract once into $XDG_CACHE_HOME/vivace/pkgs/<vendor>/<name>/<ref>/
        │   files chmod 0444, dir marked complete with a `.ok` file
        ▼
  link: hardlink every store file into vendor/<vendor>/<name>/ (copy fallback)
        │
        ▼
  autoload: write vendor/autoload.php and vendor/composer/* per docs/composer-contract.md
```

## Modules

| Module | Job |
|---|---|
| `lock` | Parse `composer.lock` and the root `composer.json` (`autoload`, `autoload-dev`, `config`). Keeps JSON key order (`serde_json` `preserve_order`) because `installed.json` re-emits lock entries. |
| `fetch` | Download `dist` archives. Zip only. Bounded concurrency, HTTP/2, follows GitHub API redirects to codeload. Verifies `dist.shasum` (sha1) when non-empty. |
| `store` | Global content-addressed store keyed by package name and dist reference. Extraction strips a single top-level directory the way Composer does, rejects zip-slip paths, preserves exec bits, writes read-only files. |
| `link` | Materialise a store tree into `vendor/`. Hardlink per file; falls back to copy across devices or with `--link-mode copy`. |
| `autoload` | Generate the autoloader files. Embeds Composer's `ClassLoader.php`, `InstalledVersions.php`, and `LICENSE` verbatim. Contains the PHP class scanner for `classmap`. |
| `cli` | `viv install [--no-dev] [--dry-run] [--link-mode hardlink|copy]`. Stubs for `update`, `require`, `remove` that exit with a clear "not implemented" message. |

## Why a store and hardlinks

Composer and Riff cache archives and unzip them into `vendor/` on every
install. Unzipping is the dominant cost once downloads are cached (Riff's warm
install on a 101-package Laravel lock spends most of its 230 ms in system
calls from extraction). A store holds each package extracted exactly once.
A warm install then does one `link()` per file, which on ext4 is an order of
magnitude cheaper than inflating and writing it. The same store serves every
project on the machine.

Hardlinks share inodes, so an edit to a vendor file would change the store.
Store files are therefore read-only (0444), which makes vendor files read-only
too. Editing fails loudly rather than corrupting other projects. Projects that
patch vendor use `--link-mode copy`.

## The no-op path

`vendor/composer/installed.json` records each installed package's `dist.reference`
and dev flag. When every lock entry matches and no orphans exist, `viv install`
only regenerates the autoloader if the root `composer.json` autoload section
changed. Target: tens of milliseconds, no network.

## Compatibility rules

Everything in `vendor/composer/` follows `docs/composer-contract.md`. The
integration test installs the fixtures in `tests/fixtures/` and diffs the
result against `expected/`, which was produced by Composer 2.10.2. Differences
are bugs, not style.

Not in v0.1, each failing with a clear error: dependency resolution (`update`,
`require`), plugins, scripts, `path`/`vcs` repositories, private registries and
auth, `dist.type` other than `zip`, `vendor/bin` proxies, `--optimize`.

## Benchmarks

`bench/run.sh <project> [tools]` runs hyperfine across three scenarios: cold
(no cache, no vendor), warm (cache, no vendor), no-op (vendor present).
`bench/laravel/` holds a 101-package lock. Results and methodology live in
`bench/results/`.
