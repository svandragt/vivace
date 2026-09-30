---
title: viv update
order: 40
summary: resolves composer.json, writes the lock and installs
---

# viv update

Run this when `composer.json` has changed and you want viv to resolve fresh
versions, write `composer.lock` (a full or partial update, depending on the
packages you name) and install them.

## Usage

```
{{help:update}}
```

## Reads and writes

- Reads: `composer.json`, the existing `composer.lock` (for a partial
  update's untouched packages), the store under `$XDG_CACHE_HOME/vivace`.
- Writes: `composer.lock`, `viv.lock` (when the project already uses it),
  and, unless `--no-install`, `vendor/` and the store.

## Exit codes

- `0` — resolved and installed cleanly.
- `1` — a filesystem, network, or archive error; a bad flag.
- `2` — the solver found no solution, Composer's
  `ERROR_DEPENDENCY_RESOLUTION_FAILED` code.

## See also

[viv update-lock](update-lock.html), [viv install](install.html), [viv add](add.html)
