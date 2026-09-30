---
title: viv dump-autoload
order: 70
summary: regenerates the autoload files from an already-installed vendor/
---

# viv dump-autoload

Run this after you've added classes or changed autoload rules, without
needing to reinstall any package: it regenerates the autoload files and
`vendor/bin` from what's already installed, fetching nothing.

## Usage

```
{{help:dump-autoload}}
```

## Reads and writes

- Reads: `composer.json`, the installed packages already in `vendor/`.
- Writes: `vendor/autoload.php`, `vendor/composer/*`, `vendor/bin/`.

## Exit codes

- `0` — regenerated cleanly.
- `1` — a filesystem error (missing `vendor/`, an unreadable package), or a
  bad flag.

## See also

[viv install](install.html)
