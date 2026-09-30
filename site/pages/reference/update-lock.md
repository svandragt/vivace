---
title: viv update-lock
order: 45
summary: re-derives composer.lock from itself, without solving or installing
---

# viv update-lock

`update --lock`'s own first-class subcommand: re-derive `composer.lock`
from itself, without solving or installing. Reach for this after a manual
edit to composer.lock, or to refresh its `content-hash` after a
non-dependency composer.json change.

## Usage

```
{{help:update-lock}}
```

## Reads and writes

- Reads: `composer.json`, `composer.lock`.
- Writes: `composer.lock`.

## Exit codes

Same as [viv update](update.html): `0` success, `1` a filesystem error or
bad flag, `2` a dependency-resolution failure.

## See also

[viv update](update.html), [viv lock](lock.html)
