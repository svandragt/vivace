---
title: viv validate
order: 150
summary: checks composer.json (and composer.lock) against Composer's own rules
---

# viv validate

Catches a malformed `composer.json`, or a lock file that's out of sync
with it, before you commit it, against Composer's own hand-written rules.

## Usage

```
{{help:validate}}
```

## Reads and writes

- Reads: `composer.json` (or the `FILE`/`--project-dir` you name),
  `composer.lock`.
- Writes: with `--fix`, rewrites `composer.json` (through the normalizer)
  and, if its `content-hash` was stale, `composer.lock`.

## Exit codes

- `0` — no errors or warnings (or `--fix` left none).
- `1` — warnings only, or `--strict` escalating any finding to failing.
- `2` — errors found.
- `3` — the file wasn't found or wasn't valid JSON.

## See also

[viv normalize](normalize.html)
