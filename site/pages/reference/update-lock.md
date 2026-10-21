---
title: viv update-lock
order: 45
summary: re-derives composer.lock from itself, without solving or installing
---

# viv update-lock

`update --lock`'s own first-class subcommand: re-derive `composer.lock`
from itself, without solving or installing. Reach for this after a manual
edit to composer.lock, or to refresh its `content-hash`, `platform`,
`platform-dev`, `platform-overrides`, `minimum-stability`,
`stability-flags` and `prefer-stable` after a non-dependency
composer.json change. `--prefer-stable` and `--prefer-lowest` set the
lock's `prefer-stable` and `prefer-lowest`, and `prefer-lowest` is `false`
without the flag.

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
