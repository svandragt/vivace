---
title: Using viv as composer
order: 30
summary: Drop the composer shim into scripts and CI so they run through viv unedited.
---

# Using viv as composer

`make install-shim` installs a `composer` binary next to `viv`. Plain
`make install` installs `viv` only, and refreshes the shim only if one is
already there, so it never replaces your real Composer. Put it on `PATH`
ahead of the real Composer, or symlink it as `composer` in CI: everyday
`install`, `dump-autoload`, `normalize` and `create-project` run through
viv; every other command falls through to your real Composer install.[^12]

This is also the cheapest way to check whether a project migrates cleanly:
alias `composer` to the shim and run your existing scripts unedited.

In GitHub Actions, one step installs viv and puts the shim first on `PATH`, so an existing `composer install` step runs through viv unedited:

```yaml
- uses: svandragt/vivace/action@v0
- run: composer install --no-dev
```

The action downloads the release tarball for the runner's OS and architecture, checks it against the release's `SHA256SUMS`, and installs nothing else. Pin a release with `with: { version: v0.21.0 }`; set `shim: false` to get `viv` on `PATH` without the `composer` shim. Cache viv's store with `actions/cache` on `~/.cache/vivace`, keyed on `composer.lock`.

A command or flag the shim doesn't understand falls back to the real
Composer with a note on stderr naming what wasn't understood, so a migration
that quietly stopped using viv is visible instead of just slower. Set
`VIV_SHIM_STRICT=1` to make that fallback a hard error instead, for a CI job
that wants a red build rather than a silent return to Composer.

You can also point a script straight at `viv`. The CI idiom
`--prefer-dist --no-interaction --no-progress` already describes what viv
does, so `viv install` and `viv dump-autoload` accept those flags and
ignore them instead of failing.

[^12]: The shim maps `install`, `dump-autoload`, `normalize`, `create-project`, `update`, `require` and `remove` with their supported flags (`update`'s partial-update package arguments and `-w`/`-W` included) to `viv`; everything else, `search`, an unrecognised flag, it hands through to the real Composer binary unchanged. Point `VIV_COMPOSER_PATH` at the real binary if it isn't first on `PATH`. The shim only has something to hand through to if a real Composer is on `PATH` in the first place: if there isn't one, those commands fail rather than silently falling back to viv. `composer --version` is the exception: it prints `viv <version> (composer shim)`, then the real Composer's own version if one is on `PATH`, and exits `0` either way.
