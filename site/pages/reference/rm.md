---
title: viv rm
order: 60
summary: removes a dependency from composer.json, resolves the rest and installs
---

# viv rm

Edits `composer.json` to remove a dependency, resolves the rest of the
dependency graph and installs it, minus the package you removed.
`--no-install`/`--no-update` opt out of the install or the resolve.

## Usage

```
{{help:rm}}
```

## Reads and writes

- Reads: `composer.json`, `composer.lock`, the store under
  `$XDG_CACHE_HOME/vivace`.
- Writes: `composer.json`, `composer.lock`, and, unless `--no-install`,
  `vendor/` and the store.

## Exit codes

Same as [viv add](add.html): `0` success, `1` a filesystem/network error or
bad flag, `2` the solver found no solution for the remaining requirements.

## See also

[viv add](add.html), [viv update](update.html)
