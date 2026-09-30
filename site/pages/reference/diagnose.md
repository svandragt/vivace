---
title: viv diagnose
order: 200
summary: environment and configuration report to paste into a bug report
---

# viv diagnose

Prints an environment and configuration report to paste into a bug report:
cache, auth sources (names only, never credentials), PHP/git/Composer
versions, platform packages, and the plugin decision per lock entry.

## Usage

```
{{help:diagnose}}
```

## Reads and writes

- Reads: `composer.json`, `composer.lock`, `auth.json` (names of configured
  hosts only), the store under `$XDG_CACHE_HOME/vivace`, the system `php`
  and `git` binaries.
- Writes: nothing; prints its report.

## Exit codes

- `0` — printed successfully.
- `1` — a filesystem error, such as a missing composer.json.

## See also

[viv audit](audit.html)
