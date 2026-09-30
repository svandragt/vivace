---
title: viv add
order: 50
summary: adds a dependency to composer.json, resolves it and installs it
---

# viv add

Edits `composer.json` to add a dependency, resolves it and installs it, in
one step. `--no-install`/`--no-update` opt out of the install or the
resolve.

## Usage

```
{{help:add}}
```

## Reads and writes

- Reads: `composer.json`, `composer.lock`, the store under
  `$XDG_CACHE_HOME/vivace`.
- Writes: `composer.json`, `composer.lock`, and, unless `--no-install`,
  `vendor/` and the store.

## Exit codes

- `0` — added, resolved and installed cleanly.
- `1` — a filesystem, network, or archive error; a bad flag.
- `2` — the solver found no solution for the new requirement.

## See also

[viv rm](rm.html), [viv update](update.html)
