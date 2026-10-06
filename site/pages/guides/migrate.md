---
title: Migrating from Composer
order: 130
summary: The shim, Dockerfiles and CI flags for moving a whole project onto viv.
---

# Migrating from Composer

This page covers the three things a team moving a project onto viv needs:
the shim, a Dockerfile and the CI flags.

## The `composer` shim

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

## In a Dockerfile

To build a `vendor/` stage without a PHP runtime, use the published image
in place of `composer:2`:

```dockerfile
FROM ghcr.io/svandragt/vivace:0 AS vendor
COPY composer.json composer.lock ./
RUN ["viv", "install", "--no-dev"]

FROM php:8.4-fpm
COPY --from=vendor /app/vendor /app/vendor
```

Two things differ from the `composer:2` stage it replaces:

- Write `RUN` in exec form, as above. The image has no shell, so the
  familiar `RUN viv install --no-dev` does not work.
- The image runs `viv` by default and ships the `composer` shim beside it,
  so `RUN ["composer", "install", "--no-dev"]` works too if you would
  rather not edit the command.
- The image carries no real Composer to fall back to, so a command or flag
  the shim doesn't understand hard-errors there instead of silently running
  Composer, the way it would on a machine that still has Composer installed.
  `composer --version` still works: it prints the shim's own version.

Tags are `:0.21`, `:0.21.0` and `:0`. There is no `:latest`: a moving tag
that silently resolves to nothing breaks scripted installs, which is the
mistake that kept `releases/latest` returning 404 for ten releases.

## CI flags

A script that already runs Composer with `--prefer-dist --no-interaction
--no-progress` needs no changes: `viv install` and `viv dump-autoload`
accept those flags and ignore them, the same way they do for Composer.
`--ignore-platform-reqs` is a real flag on both tools, not a no-op.

The one behaviour that differs: a plugin outside viv's native list stops
the install with an error naming the plugin, loudly, rather than silently
skipping its work. `--no-plugins` turns that into a warning and installs
the way Composer's own `--no-plugins` would — see [Plugin
strategy](../architecture/plugin-strategy.html) for which plugins that's
safe for.

## What's not covered

See [Compatibility and scope](../reference/compatibility.html) for what
viv's byte-identical promise covers, and [Reasons not to use
viv](reasons.html) for the rest.

[^12]: The shim maps `install`, `dump-autoload`, `normalize`, `create-project`, `update`, `require` and `remove` with their supported flags (`update`'s partial-update package arguments and `-w`/`-W` included) to `viv`; everything else, `search`, an unrecognised flag, it hands through to the real Composer binary unchanged. Point `VIV_COMPOSER_PATH` at the real binary if it isn't first on `PATH`. The shim only has something to hand through to if a real Composer is on `PATH` in the first place: if there isn't one, those commands fail rather than silently falling back to viv. `composer --version` is the exception: it prints `viv <version> (composer shim)`, then the real Composer's own version if one is on `PATH`, and exits `0` either way.
