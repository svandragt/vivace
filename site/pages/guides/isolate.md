---
title: Two plugins, one library
order: 70
summary: Prefixing a plugin's bundled dependencies, and why it stays opt-in.
---

# Two plugins, one library

Run `viv isolate <plugin>` when `viv install` prints `<plugin> bundles
<library> … Run viv isolate <plugin> to keep both`, or when a plugin
fatals with a class or function from a library another plugin or the
site also ships (`Class … not found`, `Call to undefined method`, an
`__PHP_Incomplete_Class` from the wrong version). Don't run it otherwise:
it rewrites the plugin's bundled code, and two copies of a library that
writes to the database would fight.

## What the install tells you

`viv install` always checks for this, on every install: a plugin's
bundled `vendor/` carrying a library, unprefixed, at a version that
differs from the site's own copy. It skips libraries built to coexist
(several plugins register a copy and the newest wins) and libraries
already prefixed by the plugin's own build. When it does find a clash it
prints one line naming the plugin, the library, both versions and the
fix:

```
<plugin> bundles <library> <version> unprefixed; the site has <version>. Run viv isolate <plugin> to keep both.
```

## What isolate does

`viv isolate <plugin>` copies the plugin's bundled `vendor/`, then runs
php-scoper over the copy on the project's own pinned PHP, with a prefix
derived from the plugin's slug. It then runs `php -l` on every file the
scoper wrote and a load check — a generated bootstrap that loads the
scoped `vendor/autoload.php` against the real `php-stubs/wordpress-stubs`
package and requires the plugin's own main file. Either check failing
drops the build and leaves the plugin's plain archive linked. A pass is
cached in the store, keyed to the plugin's archive and the site lock, and
linked into the plugin's install path. The plugin's own code is never
touched, only its bundled `vendor/`.

## What it costs

On an Altis site where a plugin bundles the AWS SDK and the Guzzle stack
(2,386 files): php-scoper itself takes 4.6 seconds, `php -l` across eight
workers takes 51 seconds, 87 seconds in all on a cold cache. A warm cache
— the same plugin archive and site lock already isolated once — links
the cached result instantly, no scoper or lint run again.

## Undo and inspect

`viv isolate --rm <plugin>` removes it from `extra.viv.isolate` and
relinks the plain, unprefixed archive. `viv isolate --list` prints every
isolated plugin and its prefix. `extra.viv.isolate` lives in
`composer.json` and travels in git like any other setting; `viv lock
export` carries the same map into `composer.lock`'s own
`extra.viv.isolate`, a key Composer ignores.

## Why it is opt-in

Detection runs on every install, unconditionally, but prefixing only
happens for a plugin named in `extra.viv.isolate`. Two copies of a
library that migrates or writes to the same database tables would fight
if both ran at once, so no site should pay that risk unasked — isolating
a plugin is a decision for the person who knows what that plugin's
library touches.

## What it does not do

It does not resolve a clash at solve time — two installed packages
declaring incompatible requirements for the same library. That is a
different problem, covered under candidate 3.2 of the research
programme: [Generation 3](../architecture/research/generation-3.html).
