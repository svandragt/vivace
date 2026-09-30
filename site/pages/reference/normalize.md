---
title: viv normalize
order: 80
summary: tidies composer.json's key order and formatting
---

# viv normalize

Tidies `composer.json`'s key order and formatting, a native `composer
normalize` (`ergebnis/composer-normalize`). This is the same pass `add`,
`rm` and `init` already apply automatically when they write, so reach for
it directly only when a manually-edited composer.json has drifted.

## Usage

```
{{help:normalize}}
```

## Reads and writes

- Reads: `composer.json`.
- Writes: `composer.json` (unless `--dry-run`/`--diff`).

## Exit codes

- `0` — already normalized, or normalized successfully.
- `1` — a filesystem error, invalid JSON, or a bad flag; also `--check`
  finding the file not normalized.

## See also

[viv validate](validate.html)
