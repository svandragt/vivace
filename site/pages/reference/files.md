---
title: Files viv writes
order: 900
summary: vendor/ contents, the store's bucket layout and the viv.lock format
---

# Files viv writes

## vendor/

`viv install` and `viv update` write the following under the project's
`vendor-dir` (default `vendor/`):

- `autoload.php`, `composer/autoload_*.php` — the PSR-0/PSR-4/classmap/files
  autoload maps and the loader entry point.
- `composer/ClassLoader.php`, `composer/InstalledVersions.php` — Composer's
  own runtime classes, copied in verbatim.
- `composer/installed.json`, `composer/installed.php` — the installed-package
  manifest, JSON and PHP-array forms, that `InstalledVersions` and other
  tooling read at runtime.
- `composer/platform_check.php` — the PHP-version and extension check the
  autoloader runs first, when `platform-check` is enabled.
- `composer/.vivace-state` — viv's own state file, which lets a no-op install
  skip all work: the lock's content hash, the `--no-dev` flag, a hash of
  `composer.json` and the prefix of each [isolated](isolate.html) plugin.
  Composer ignores it.
- one directory per installed package, `vendor/<vendor>/<name>/`.
- `vendor/bin/` — proxy scripts for each package's declared `bin` entries.

<!-- src/autoload/generator.rs, src/install.rs -->

## The store

viv's shared cache lives at `$XDG_CACHE_HOME/vivace` (`~/.cache/vivace` by
default), or `--cache-dir`. It holds these buckets:

| Bucket | Holds |
|---|---|
| `archive-v0` | Downloaded package archives, content-addressed. |
| `dists-v0` | Pointers from a package/version to its `archive-v0` entry. |
| `tools-v0` | `viv x`'s per-tool synthetic-root envs, one per content-hashed key. |
| `platform-v0` | The cached `php` platform-detection probe, keyed by interpreter identity. |
| `repo-v0` | Repository metadata (`packages.json` and its includes), one directory per repo host. |
| `vcs-v0` | Mirrored git checkouts for a VCS-sourced package. |
| `root-classmap-v0` | One classmap-scan sidecar per project, for the root package's own directories. |
| `platform-check-v0` | The last-verified-OK platform-check inputs, one file per project. |
| `php-v0` | One static-php-cli build per `<version>-<os>-<arch>`. |
| `commit-meta-v0` | A `dev-*` package's composer.json at one pinned commit, keyed by commit, or a `missing` note for a commit that is gone upstream. |
| `isolate-check-v0` | `viv install`'s cached verdict on whether a plugin's bundled libraries clash with the site's, keyed by the plugin archive and the site lock. |
| `isolated-v0` | A plugin's prefixed `vendor/` tree from `viv isolate`, keyed by the archive, the prefix and the php-scoper version. |
| `.lock` | The store's own file lock. |

`viv cache prune` removes anything not in this list, plus stale entries
within it; `viv cache clean` refuses to remove a directory that holds
anything else.

<!-- src/store.rs:34-96 -->

## viv.lock

`viv lock convert` and a resolve write `viv.lock` as TOML, one record per
locked package:

| Field | Holds |
|---|---|
| `name` | The package name. |
| `version` | The resolved version. |
| `dist_url` | The dist archive's URL, when the resolution has one. |
| `dist_hash` | The dist archive's checksum, when the resolution has one. |
| `source_ref` | The VCS source's commit/ref, when the resolution has one. |
| `dev` | Whether this is a `require-dev`-only package. |
| `root_requirement` | The root composer.json's own constraint on this package, when it names one directly. |
| `time` | The resolved package's commit/release time, RFC 3339, when known. |
| `raw` | The full `composer.lock` package block for this entry, as compact JSON text, so `viv lock export` can reconstruct it byte-for-byte. |

<!-- src/native_lock.rs:373-415 -->
