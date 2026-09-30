---
title: viv show
order: 110
summary: lists installed packages, or inspects one
---

# viv show

Lists what's installed, or inspects a single package's detail. Pass
`--tree`/`-t` for the require graph instead of a flat list.

## Usage

```
{{help:show}}
```

## Reads and writes

- Reads: `vendor/composer/installed.json`, `composer.json`.
- Writes: nothing; prints its report.

## Exit codes

- `0` — printed successfully.
- `1` — a filesystem error (no `vendor/`), an unknown package name, or a
  bad flag.

## See also

[viv tree](tree.html), [viv why](why.html), [viv outdated](outdated.html)
