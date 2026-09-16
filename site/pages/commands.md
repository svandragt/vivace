# Commands

Every `viv` subcommand, in the order `viv --help` lists them, each with its
full `--help` output so the flags on this page never drift from the binary.

[TOC]

## viv install

Run this after cloning a project or pulling a `composer.lock` change: it
installs the exact versions the lock file records.

```
{{help:install}}
```

## viv update

Run this when `composer.json` has changed and you want viv to resolve fresh
versions, write the lock and install them.

```
{{help:update}}
```

## viv update-lock

Use this to re-derive `composer.lock` from itself, without solving or
installing, for example after a manual edit.

```
{{help:update-lock}}
```

## viv add

Edits `composer.json`, resolves the new dependency and installs it in one
step.

```
{{help:add}}
```

## viv rm

Edits `composer.json`, resolves the rest of the dependency graph and
installs it, minus the package you removed.

```
{{help:rm}}
```

## viv dump-autoload

Run this after you've added classes or changed autoload rules, without
needing to reinstall any package.

```
{{help:dump-autoload}}
```

## viv init

Writes a new project's `composer.json` and stops, with no interactive
prompts.

```
{{help:init}}
```

## viv new

Starts a project from scratch: a bare name creates an empty directory and
runs `init`'s defaults inside it, or a `vendor/package` spec downloads that
package's dist as a skeleton, Laravel's `new` command style.

```
{{help:new}}
```

## viv show

Lists what's installed, or inspects a single package's detail with
`--tree`.

```
{{help:show}}
```

## viv tree

The shorthand for `show --tree`, printing the require graph.

```
{{help:tree}}
```

## viv why

Finds which installed packages require the package you name.

```
{{help:why}}
```

## viv outdated

Flags installed packages that have a newer version available.

```
{{help:outdated}}
```

## viv audit

Run this before a release to check for known security advisories and
abandoned packages among your installed or locked dependencies.

```
{{help:audit}}
```

## viv validate

Catches a malformed `composer.json`, or a lock file that's out of sync
with it, before you commit it.

```
{{help:validate}}
```

## viv normalize

Tidies `composer.json`'s key order and formatting, the same result `add`,
`rm` and `init` already apply automatically when they write.

```
{{help:normalize}}
```

## viv run

Runs a `scripts` entry from the root `composer.json`, the same as
`composer run-script`.

```
{{help:run}}
```

## viv exec

Runs a `vendor/bin` binary with `vendor/bin` prepended to `PATH`, so you
don't need the full path.

```
{{help:exec}}
```

## viv x

Installs a package into an isolated, cached environment and runs its
binary, `npx`-style, without touching your `composer.json` or `vendor/`.
Handy for a one-off tool like PHPUnit or PHP CS Fixer.

```
{{help:x}}
```

## viv cache

Manages viv's shared store: prune stale entries, check its size, or remove
it outright.

```
{{help:cache}}
```

## viv diagnose

Prints an environment and configuration report to paste into a bug report.

```
{{help:diagnose}}
```
