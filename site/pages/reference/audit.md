---
title: viv audit
order: 100
summary: checks installed or locked packages for security advisories and abandoned packages
---

# viv audit

Run this before a release to check for known security advisories and
abandoned packages among your installed or locked dependencies.

## Usage

```
{{help:audit}}
```

## Reads and writes

- Reads: `composer.json`, `vendor/composer/installed.json` (or
  `composer.lock` with `--locked`), `auth.json`, and fetches each
  repository's security-advisories endpoint over the network (unless
  `--offline`); reads the store under `$XDG_CACHE_HOME/vivace` for cached
  repository metadata.
- Writes: nothing to disk; prints its report.

## Exit codes

- `0` — clean: no active advisory and no failing abandoned package.
- `1` — an active advisory or a failing abandoned package was found, or no
  installed packages were found at all when the project declares
  dependencies.

`viv audit` never uses `2`: a genuine failure to audit (no lock, no
network, a malformed composer.json) is a plain error, not a finding, and
still exits `1`.

## See also

[viv outdated](outdated.html)
