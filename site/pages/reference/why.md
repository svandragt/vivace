---
title: viv why
order: 130
summary: lists installed packages that require the named package
---

# viv why

`composer why`/`depends`'s alias: `tree --invert`, listing which installed
packages require the package you name.

## Usage

```
{{help:why}}
```

## Reads and writes

Same as [viv show](show.html): reads `vendor/composer/installed.json` and
`composer.json`; writes nothing.

## Exit codes

Same as [viv show](show.html): `0` printed, `1` a filesystem error, unknown
package or bad flag.

## See also

[viv show](show.html), [viv tree](tree.html)
