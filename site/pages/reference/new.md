---
title: viv new
order: 20
summary: starts a project from a bare name or a vendor/package skeleton
---

# viv new

Use this to start a project in a directory that doesn't exist yet. A bare
directory name runs `init`'s own defaults inside it; a
`vendor/package[:constraint]` spec downloads that package's dist as a
skeleton and installs it there, `npx create-`/Laravel's `new` style.
`create-project` is Composer's own name for this and is kept as an alias,
along with its three-positional `vendor/package dir constraint` shape
(prefer `vendor/package:constraint` instead).

## Usage

```
{{help:new}}
```

## Reads and writes

- Reads: the store under `$XDG_CACHE_HOME/vivace`, git config.
- Writes: the new project directory, `composer.json`, `composer.lock`,
  `vendor/`, and the store.

## Exit codes

- `0` — the project was written and installed.
- `1` — a filesystem error, an existing non-empty directory, or a bad flag.
- `2` — the package's dependencies failed to resolve.

## See also

[viv init](init.html), [viv install](install.html)
