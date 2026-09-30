---
title: In CI
order: 50
summary: The GitHub Action, caching the store, and a PHP-free runner.
---

# In CI

## The GitHub Action

In GitHub Actions, one step installs viv and puts the shim first on `PATH`, so an existing `composer install` step runs through viv unedited:

```yaml
- uses: svandragt/vivace/action@v0
- run: composer install --no-dev
```

The action downloads the release tarball for the runner's OS and architecture, checks it against the release's `SHA256SUMS`, and installs nothing else. Pin a release with `with: { version: v0.20.0 }`; set `shim: false` to get `viv` on `PATH` without the `composer` shim.

See [Using viv as composer](shim.html) for what the shim does and does not map to viv, and `VIV_SHIM_STRICT` for turning a silent Composer fallback into a hard build failure.

## Caching the store

Cache viv's store with `actions/cache` on `~/.cache/vivace`, keyed on `composer.lock`, so a runner that already built the cache once skips the network on every later run.

## A runner with no PHP

A runner image with no PHP installed can still run PHP tooling: `viv php
install 8.4` downloads a self-contained PHP build into the store, with no
system package manager involved, and `viv run phpunit`, `viv run phpcs` and
similar then run that tool on the pinned build. See [A PHP per
project](../guides/php.html) for the full command set.
