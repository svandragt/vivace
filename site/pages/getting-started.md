# Getting started

For a PHP developer who has not used viv before: what it is, your first
install, and where to go next.

## What viv is

`viv` is a Rust reimplementation of Composer that installs from
`composer.lock` and writes the `vendor/` directory Composer would write,
byte for byte, faster than Composer itself. It covers the Composer commands
you run every day; anything else — plugins outside its native adapters, and
Composer's longer tail of commands — stays with Composer, and the `composer`
shim passes those straight through.

## Install

```sh
cargo binstall vivace
```

See [Install and upgrade](install.html) for prebuilt binaries, the `.deb`
package, and how to upgrade.

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

{{readme:Starting from nothing}}

## What to read next

- [Using viv as composer](shim.html) — drop viv into scripts and CI that
  still type `composer`.
- [Migrating from Composer](migrate.html) — the shim, Dockerfiles and CI
  flags for moving a whole project over.
- [Compatibility and scope](compatibility.html) — what viv's byte-identical
  promise covers, and what still needs Composer.
