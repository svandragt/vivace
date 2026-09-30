---
title: viv outdated
order: 140
summary: flags installed packages with a newer version available
---

# viv outdated

Lists installed packages that have a newer version available
(`show --latest --outdated`).

## Usage

```
{{help:outdated}}
```

## Reads and writes

- Reads: `vendor/composer/installed.json`, `composer.json`, and fetches
  each repository's metadata over the network (unless `--offline`); reads
  the store under `$XDG_CACHE_HOME/vivace` for cached metadata.
- Writes: nothing; prints its report.

## Exit codes

- `0` — no installed package is outdated.
- `1` — at least one installed package has a newer version available, or a
  filesystem/network error occurred.

## See also

[viv show](show.html), [viv audit](audit.html)
