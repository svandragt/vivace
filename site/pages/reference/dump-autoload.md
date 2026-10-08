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

- Reads: `composer.json`, `composer.lock` and the installed packages already
  in `vendor/`.
- Writes: `vendor/autoload.php`, `vendor/composer/*`, `vendor/bin/`.

The command does not need a `composer.lock`. Without one, like Composer, it
takes the package list from `vendor/composer/installed.json`, or an empty
list when `vendor/` does not exist yet, so the autoloader covers only your
own `autoload` rules. In that case it leaves `installed.json` and
`installed.php` as they are.

## Exit codes

- `0` — regenerated cleanly.
- `1` — a filesystem error (a package missing from `vendor/`, an unreadable
  file), or a bad flag.

## See also

[viv install](install.html)
