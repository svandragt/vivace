---
title: Scripts and running commands
order: 70
summary: viv run, viv exec, lifecycle scripts, and the flags-before-name rule.
---

# Scripts and running commands

## Lifecycle scripts

`install` runs your project's setup scripts, the same way Composer does,
unless you pass `--no-scripts`: the root's `pre-install-cmd`,
`post-autoload-dump` and `post-install-cmd` entries, in that order.

## viv run

`viv run <name>` falls through from a `scripts` entry, to
`vendor/bin/<name>`, to a command on `PATH`, with the pinned PHP's
directory first on `PATH` for `run`, `exec` and every script the runner
spawns — so `@php` and a plain `php` inside a script hit the pinned
build. A pinned build not yet in the cache installs on first use, with one
line on stderr; `--offline` keeps the error instead of downloading. See [A
PHP per project](php.html) for pinning the build itself.

```sh
viv run phpunit --filter Foo # vendor/bin/phpunit on the pinned PHP
viv run php -v               # the pinned build itself
```

## viv exec

`viv exec` runs a bare command on `PATH` with the same PHP-first `PATH`
`run` sets up, for a tool that isn't a `scripts` entry or a `vendor/bin`
binary.

## Flags before the name

viv's own flags go before the name it's running
(`viv run -d ../app phpunit`); everything after the name belongs to the
tool, unparsed. `viv x` follows the same rule — see [Running tools without
installing them](tools.html).
