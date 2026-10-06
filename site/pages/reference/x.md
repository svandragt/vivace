---
title: viv x
order: 160
summary: installs and runs a package's binary in an isolated, cached environment
---

# viv x

Installs (if needed) and runs a package's bin in an isolated,
content-hashed environment, `npx`-style. Handy for a one-off tool like
PHPUnit or PHP CS Fixer, without touching your project's `composer.json`
or `vendor/`.

## Usage

```
{{help:x}}
```

## Reads and writes

- Reads: the store under `$XDG_CACHE_HOME/vivace` for an already-built env
  matching the same content hash, and the current directory's
  `composer.json` for a `config.platform.php` pin. With a pin, the tool
  resolves against that PHP instead of the one on `PATH`.
- Writes: the store's `tools-v0` bucket (a fresh synthetic-root env, when
  none matches yet), and its `php-v0` bucket when the pinned PHP isn't
  installed yet (not with `--offline`).

## Exit codes

- `0` — the tool ran and exited `0`.
- non-zero — the tool's own exit code, passed straight through; `1` if
  viv itself failed to build or resolve the environment.

## See also

[viv run](run.html), [viv exec](exec.html)
