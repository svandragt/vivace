---
title: viv install
order: 30
summary: installs the exact versions composer.lock records
---

# viv install

Run this after cloning a project or pulling a `composer.lock` change: it
installs the exact versions the lock file records, without resolving
anything fresh.

## Usage

```
{{help:install}}
```

## Reads and writes

- Reads: `composer.json`, `composer.lock` (and `viv.lock`, when present, to
  reconcile against it), the store under `$XDG_CACHE_HOME/vivace`.
- Writes: `vendor/` (packages, autoload files, `vendor/bin`), the store
  (downloaded archives and their dist pointers, the platform-check and
  root-classmap sidecars).

## Exit codes

- `0` — installed cleanly.
- `1` — a filesystem, network, or archive error; a bad flag.
- `2` — a locked platform requirement failed verification, the same
  dependency-resolution exit code Composer uses.

## See also

[viv update](update.html), [viv dump-autoload](dump-autoload.html)
