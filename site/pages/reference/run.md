---
title: viv run
order: 170
summary: runs a scripts entry from the root composer.json
---

# viv run

Runs a `scripts` entry from the root `composer.json`, the same as
`composer run-script`, on the project's PHP.

## Usage

```
{{help:run}}
```

## Reads and writes

- Reads: `composer.json`'s `scripts` block, `vendor/bin/` (a script may
  invoke a package binary).
- Writes: whatever the script itself writes.

## Exit codes

- `0` — the script ran and exited `0`.
- non-zero — the script's own exit code, passed straight through; `1` if
  the named script doesn't exist.

## See also

[viv exec](exec.html), [viv x](x.html)
