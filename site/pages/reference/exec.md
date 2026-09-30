---
title: viv exec
order: 180
summary: runs a vendor/bin binary with vendor/bin prepended to PATH
---

# viv exec

Runs a `vendor/bin` binary with `vendor/bin` prepended to `PATH`, so you
don't need the full path.

## Usage

```
{{help:exec}}
```

## Reads and writes

- Reads: `vendor/bin/`.
- Writes: whatever the binary itself writes.

## Exit codes

- `0` — the binary ran and exited `0`.
- non-zero — the binary's own exit code, passed straight through; `1` if
  the named binary isn't in `vendor/bin`.

## See also

[viv run](run.html), [viv x](x.html)
