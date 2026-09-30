---
title: viv workspace
order: 220
summary: discovers and manages a monorepo's member packages
---

# viv workspace

Discovers `extra.viv.workspace` members and reports inter-member
requirements, or writes the aggregate root `composer.json` from member glob
patterns and resolves/installs it.

## Usage

```
{{help:workspace}}
```

### viv workspace list

Lists the workspace's members: name, version, path (relative to the root)
and which other members each one requires. Writes nothing.

```
{{help:workspace list}}
```

### viv workspace init

Writes a top-level composer.json (the aggregate root) from member glob
patterns, e.g. `plugins/* themes/*`, then resolves and installs (unless
`--no-install`). No patterns prints the discovery listing instead, grouped
by the directory a pattern for it would target, and writes nothing.

```
{{help:workspace init}}
```

### viv workspace add

Adds one more member to an existing aggregate root and resolves again
(unless `--no-install`).

```
{{help:workspace add}}
```

## Reads and writes

- Reads: every member directory's `composer.json` under the project
  directory, the store under `$XDG_CACHE_HOME/vivace`.
- Writes: the aggregate root `composer.json`, `composer.lock`, and, unless
  `--no-install`, `vendor/` and the store.

## Exit codes

Same as [viv add](add.html): `0` success, `1` a filesystem error or bad
flag, `2` `init`/`add`'s own resolve found no solution.

## See also

[viv add](add.html), [viv install](install.html)
