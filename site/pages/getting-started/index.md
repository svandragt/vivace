---
title: Getting started
order: 10
summary: What viv is, and the five-minute path from install to your first `viv install`.
---

# Getting started

`viv` is a Rust reimplementation of Composer that installs from
`composer.lock` and writes the `vendor/` directory Composer would write,
byte for byte. On the compatibility corpus a cold `laravel/laravel` install from a local package mirror
takes 0.30 s against Composer's 1.58 s, and replayed over 355 real client
merges, `composer.lock` conflicts under plain git 228 times against 6 once
viv's merge driver and commit-pinning are both in place. It began as a
question, whether a person directing coding agents can build a faster
drop-in Composer, and that question is answered: the compatible mode
described in this section is finished and frozen as a control.

## In five minutes

1. **Install viv.** See [Install and upgrade](/getting-started/install.html) for the exact
   command for your platform.
2. **Run `viv install` in a project** that already has a `composer.json`
   and `composer.lock`.
3. **Adopt an existing `vendor/`.** If Composer already wrote it, viv
   adopts it automatically, no flag needed: it relinks every installed
   package from its own store in place, so the result matches what a
   fresh viv install would have written.
4. **Stop at any time** and go back to Composer with no clean-up.

The rest of this page fills in each step; [What to read
next](#what-to-read-next) points at the pages beyond it.

## Your first `viv install`

In a project that already has a `composer.json` and `composer.lock`, run:

```sh
viv install
```

If there's no `vendor/` yet, viv resolves the lock and writes one. If
Composer already wrote `vendor/`, viv adopts it automatically, no flag
needed: it relinks every installed package from its own store in place, so
the result matches what a fresh viv install would have written.

Only one path asks first: running `composer install` through the [`composer`
shim](/getting-started/shim.html) on a terminal prompts before it touches a Composer-written
`vendor/`, since typing `composer install` didn't opt into viv rewriting
your tree —

```
This will relink every installed package from the store, overwriting vendor/ in place. Continue? [y/N]
```

— answer `y` to continue, anything else (including a bare Enter) aborts. A
plain `viv install`, or a script that already runs through the shim,
proceeds without asking. If a package can't be downloaded — a private
package behind a licence key, for example — viv keeps Composer's copy of
that package, prints a warning, and adopts the rest.

### Starting from nothing

No `composer.json` yet? `viv init` writes one and stops, with no prompts:
the package name is guessed from `git config user.name` and the directory,
`type` is `project`, `license` is `MIT`, and `autoload.psr-4` points at
`src/` when that directory exists. Pass `--name`, `--license` or `--type` to
override a default, or `--require`/`--require-dev` to add dependencies in
the same command:

```sh
mkdir demo && cd demo && viv init --require psr/log
```

That resolves `psr/log`, writes `composer.lock`, and installs `vendor/`,
the same as `viv add` would on an existing project (`--no-install` opts
out). Run it again with `--force` to start over.

`viv init` is for the directory you're already in; `viv new` is for one
that doesn't exist yet. A bare name creates it and runs `init`'s own
defaults inside:

```sh
viv new demo
```

`vendor/package[:constraint]` downloads that package's dist as a project
skeleton (constraint defaults to the newest stable version), drops its own
VCS metadata, and installs it, running the `post-root-package-install`/
`post-create-project-cmd` scripts a skeleton like Laravel's relies on
(`--no-scripts` opts out):

```sh
viv new laravel/laravel:^11 my-app
```

`create-project` is Composer's own name for this, kept as an alias.

## Stopping

You can stop using viv at any point and go back to Composer with no
clean-up. A `vendor/` that viv wrote is a valid Composer install:
`installed.json` and the autoload files are the same bytes Composer would
have written, so `composer install` on it is a no-op and `composer update`
replaces packages as usual. The only extra file is a small state file in
`vendor/composer/`, which Composer ignores.

The links from `vendor/` into viv's store are hardlinks, not symlinks: each
file in `vendor/` is a real file that shares its data with the store copy,
so deleting the store (`viv cache clean`) leaves `vendor/` complete and
working. If you would rather reinstall it with Composer anyway:

```sh
rm -rf vendor && composer install
```

To remove viv itself:

```sh
viv cache clean            # deletes viv's store under ~/.cache/vivace
rm ~/.cargo/bin/viv ~/.cargo/bin/composer   # the binary and the shim
```

Use `apt remove vivace` if you installed the .deb instead. Nothing else is
written outside the project and the cache.

## Is it safe to try

viv's contract is that its output matches Composer's byte for byte. Before
every release, a compatibility sweep installs a mix of pinned popular
projects and a random sample of Packagist packages with both Composer and
viv, then compares the results.[^2] The v0.21.0 sweep: all 20 install rows of the
pinned corpus are identical, 9 of the 10 pinned projects resolve the same
lock (craftcms/craft differs[^20]), and `viv lock export` reproduces all 10
committed locks byte for byte. In the random sample, all 12 rows that
Composer could install are identical; the other 8 were skipped because
Composer itself could not resolve the project.[^3]

One pinned project still needs `--no-plugins`, for a plugin viv refuses by
design rather than one it has yet to port.[^4] See [Plugins](#plugins)
below.

## In this section

- [Install and upgrade](/getting-started/install.html) — binstall, Homebrew, `cargo install`, the `.deb` and the tarball.
- [Using viv as composer](/getting-started/shim.html) — the `composer` shim, for scripts that call Composer by name.
- [In a Dockerfile](/getting-started/docker.html) — the published image, in place of `composer:2`.
- [In CI](/getting-started/ci.html) — the GitHub Action, caching the store, and a bare runner with no PHP.
- [Support](/getting-started/support.html) — where to report a bug, and what holds across releases.

## What to read next

- [Guides](/guides/) — everyday commands, migrating a project, and the rest of what viv does beyond installing.
- [Architecture](/architecture/) — what viv's byte-identical promise covers, and how it's built.

[^2]: See [`compat/README.md`](compat/README.md) for how the sweep works.
[^20]: The v0.18.0 sweep had one lock differ, [#316](https://github.com/svandragt/vivace/issues/316): craftcms/craft's requirements are satisfied by `yii2-shell` `2.0.6` and by `dev-master`, and the two solvers search in a different order, so the security-advisories feed, by changing which versions of other packages are available, decides whether Composer's search ends on `dev-master`. The v0.19.0 and v0.20.0 sweeps resolved the same lock on all 10, and the v0.21.0 sweep differs again, on `symfony/var-dumper` (v7.4.18 in Composer's lock, v5.4.48 in viv's). The issue stays open because the feed can flip the result.
[^3]: The skips are packages Composer itself refuses to resolve, not something viv got wrong. In the v0.21.0 sweep, 3 of the 4 skipped projects have only `dev-` or alpha versions, which the default `minimum-stability` excludes; Composer could not find a dependency of the fourth. Earlier sweeps also skipped a project when security advisories blocked every matching version, or its platform requirements were unmet. Full results, including which projects and what was skipped, are in [`compat/results/v0.21.0.md`](compat/results/v0.21.0.md).
[^4]: Of viv's 10 pinned compatibility projects, the one that needs `--no-plugins` is `symfony/demo`, for `symfony/flex`. Flex does its work in `composer require`, so installing from a committed lock loses nothing; see [`docs/plugin-strategy.md`](docs/plugin-strategy.md).
