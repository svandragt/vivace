---
title: Reasons to use viv, and reasons not to
order: 110
summary: Six things Composer doesn't do, and the trade-offs that come with viv's 0.x, Linux/macOS-only scope.
---

# Reasons to use viv, and reasons not to

## Reasons to use viv

Besides matching Composer's output faster, viv does six things Composer
does not.

See [Merging composer.lock without conflicts](lock-merge.html), [Running
tools without installing them](tools.html), [A PHP per
project](php.html), [Automatic normalisation](normalize.html) and [Two
plugins, one library](isolate.html) for each in full, and [Cache and
offline use](cache.html) for the shared store.

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

[^17]: [`docs/stability.md`](docs/stability.md) states what a minor release may and may not change.
