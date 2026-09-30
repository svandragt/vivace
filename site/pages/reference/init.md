---
title: viv init
order: 10
summary: writes a new project's composer.json and stops, no interactive prompts
---

# viv init

Reach for this when you're starting a project and want a composer.json
written from sensible defaults — inferred from git and the current
directory — with no interactive question flow. Pass `--require`/
`--require-dev` to chain straight into the same resolve/lock/install `viv
add` runs; `--no-install` opts out.

## Usage

```
{{help:init}}
```

## Reads and writes

- Reads: git config (for the author and package name defaults).
- Writes: `composer.json`; with `--require`/`--require-dev` and no
  `--no-install`, also `composer.lock` and `vendor/`, and reads/writes the
  store under `$XDG_CACHE_HOME/vivace`.

## Exit codes

- `0` — composer.json written (and, if requested, installed).
- `1` — a filesystem error, or a bad flag.
- `2` — a `--require`/`--require-dev` chain into `add` failed to resolve.

## See also

[viv new](new.html), [viv add](add.html)
