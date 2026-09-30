---
title: Compatibility and scope
order: 1
summary: What viv's byte-identical promise does and doesn't cover.
---
# Compatibility and scope

What viv's byte-identical promise actually covers, what stays out of scope,
and how that's checked before every release.

## What viv promises

{{include:docs/stability.md#What viv promises}}

## What is not covered

{{include:docs/stability.md#What is not covered}}

## Works with

- Repositories: Packagist, Private Packagist and Satis, including a local
  `file://` mirror, `path`, `vcs`, git, GitHub and `package` (inline
  declarations) sources; redirects are followed.
- Packages: zip and tar dists, a git checkout when there is no dist,
  `preferred-install: source`, sha1 checks, credentials from `auth.json`
  and `COMPOSER_AUTH`.
- Autoload: PSR-4, PSR-0, classmap and files; `--optimize-autoloader` and
  `--classmap-authoritative`; `platform_check.php`; `vendor/bin` proxies;
  lifecycle scripts.
- Commands: `install`, full and partial `update` including
  `--minimal-changes` and Composer's default blocking of versions with a
  security advisory (`--no-blocking` to allow them), `add`, `rm`,
  `dump-autoload`, and offline mode with `--offline` or
  `COMPOSER_DISABLE_NETWORK`. `install` refuses before writing anything when
  the lock needs a PHP version, extension or library the detected platform
  lacks, honouring `config.platform`, `--ignore-platform-reqs` and
  `--ignore-platform-req`.
- Plugins: the ones with native adapters, listed under [Plugins](#plugins).

## Reasons not to use viv

- **It stays 0.x.** No 1.0 is planned. Minor releases can change behaviour
  and flags; the release notes and `JOURNAL.md` call those out. The output contract
  (`vendor/` and `composer.lock` identical to Composer's) is the one thing
  that does not move.[^17]
- **Windows is not supported.** Linux and macOS only.
- **Maintenance is on demand.** The compatible mode is complete and no new
  plugin adapters or Composer commands are planned. A bug in it that a real
  project hits gets fixed; open an issue with the project's `composer.json`
  and lock. Releases continue as research chapters land, and the compat
  sweep and benchmark gates run on every change so the drop-in behaviour
  does not regress.
- **Some Composer plugins stop the install.** symfony/flex and any plugin
  without a native adapter make viv exit with an error naming the plugin.
  `--no-plugins` installs as Composer would without them, but the plugin's
  work is not done. Of the 10 pinned test projects, 1 is in this position:
  `symfony/demo`, for symfony/flex, which viv refuses by design.
- **The shim needs a real Composer for everything else.** It maps
  `install`, `dump-autoload`, `normalize`, `create-project`, `update`,
  `require` and `remove` to viv; `search` and the rest go to the Composer on
  your `PATH`, and with no real Composer installed they fail.
- **`vendor/` files are read-only by default.** viv hardlinks them from a
  shared store, so an edit inside `vendor/` fails instead of changing every
  project on the machine. If you patch vendor files by hand, install with
  `--link-mode copy`, or `--link-mode clone` for writable files sharing the
  store's disk space where the filesystem supports it.
- **A `minimum-stability` gap.** A version filtered out by
  `minimum-stability` is still reported as not found rather than as
  filtered; Composer names the cause.

## Is it safe to try

viv's contract is that its output matches Composer's byte for byte. Before
every release, a compatibility sweep installs a mix of pinned popular
projects and a random sample of Packagist packages with both Composer and
viv, then compares the results.[^2] The v0.20.0 sweep: all 20 install rows of the
pinned corpus are identical, all 10 pinned projects resolve the same
lock,[^20] and `viv lock export` reproduces all 10 committed locks byte for
byte. In the random sample, all 6 rows that Composer could install are
identical; the other 8 were skipped because Composer itself could not
resolve the project or its platform check failed.[^3]

One pinned project still needs `--no-plugins`, for a plugin viv refuses by
design rather than one it has yet to port.[^4] See [Plugins](#plugins)
below.

## How compatibility is checked

Every release runs the [compatibility
sweep](https://github.com/svandragt/vivace/blob/main/compat/README.md)
against a mix of pinned popular projects and a random sample of Packagist
packages, comparing viv's `vendor/` and `composer.lock` against Composer's
own. Per-release results live in
[`compat/results/`](https://github.com/svandragt/vivace/blob/main/compat/results);
ongoing sweeps of public projects outside that pinned set are tracked in
[`compat/hunted.md`](https://github.com/svandragt/vivace/blob/main/compat/hunted.md).
The [Compare](/compare.html) page turns the newest numbers into a table.

For the curious, here's exactly what a plain install writes, and how a path
renders inside it — the detail the sweep checks byte for byte.

{{include:docs/composer-contract.md#Files}}

{{include:docs/composer-contract.md#Path rendering}}

## Versioning

{{include:docs/stability.md#Versioning}}

[^17]: [`docs/stability.md`](docs/stability.md) states what a minor release may and may not change.
[^2]: See [`compat/README.md`](compat/README.md) for how the sweep works.
[^20]: The v0.18.0 sweep had one lock differ, [#316](https://github.com/svandragt/vivace/issues/316): craftcms/craft's requirements are satisfied by `yii2-shell` `2.0.6` and by `dev-master`, and the two solvers search in a different order, so the security-advisories feed, by changing which versions of other packages are available, decides whether Composer's search ends on `dev-master`. The v0.19.0 sweep resolved the same lock on all 10; the issue stays open because the feed can flip it again.
[^3]: The skips are packages Composer itself refuses to resolve — security advisories blocking every matching version, a `dev-master`-only package under the default `minimum-stability`, a dependency whose only versions require a framework the root cannot take — or an unmet platform requirement, not something viv got wrong. Full results, including which projects and what was skipped, are in [`compat/results/v0.20.0.md`](compat/results/v0.20.0.md).
[^4]: Of viv's 10 pinned compatibility projects, the one that needs `--no-plugins` is `symfony/demo`, for `symfony/flex`. Flex does its work in `composer require`, so installing from a committed lock loses nothing; see [`docs/plugin-strategy.md`](docs/plugin-strategy.md).
