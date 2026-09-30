---
title: Getting started
order: 10
summary: What viv is, and the five-minute path from install to your first `viv install`.
---

# Getting started

`viv` is a Rust reimplementation of Composer that installs from
`composer.lock` and writes the `vendor/` directory Composer would write,
byte for byte. On the compatibility corpus a cold `laravel/laravel` install
takes 0.30 s against Composer's 1.58 s, and replayed over 355 real client
merges, `composer.lock` conflicts under plain git 228 times against 6 once
viv's merge driver and commit-pinning are both in place. It began as a
question, whether a person directing coding agents can build a faster
drop-in Composer, and that question is answered: the compatible mode
described in this section is finished and frozen as a control.

## In five minutes

1. **Install viv.** See [Install and upgrade](install.html) for the exact
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
shim](shim.html) on a terminal prompts before it touches a Composer-written
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

{{include:README.md#Starting from nothing}}

## Stopping

{{include:README.md#Stopping}}

## Is it safe to try

{{include:README.md#Is it safe to try}}

## In this section

- [Install and upgrade](install.html) — binstall, Homebrew, `cargo install`, the `.deb` and the tarball.
- [Using viv as composer](shim.html) — the `composer` shim, for scripts that call Composer by name.
- [In a Dockerfile](docker.html) — the published image, in place of `composer:2`.
- [In CI](ci.html) — the GitHub Action, caching the store, and a bare runner with no PHP.
- [Support](support.html) — where to report a bug, and what holds across releases.

## What to read next

- [Guides](../guides/index.html) — everyday commands, migrating a project, and the rest of what viv does beyond installing.
- [Architecture](../architecture/index.html) — what viv's byte-identical promise covers, and how it's built.
