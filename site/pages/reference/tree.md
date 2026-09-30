---
title: viv tree
order: 120
summary: shorthand for show --tree, printing the require graph
---

# viv tree

The shorthand for `show --tree`, printing the require graph of installed
packages.

## Usage

```
{{help:tree}}
```

## Reads and writes

Same as [viv show](show.html): reads `vendor/composer/installed.json` and
`composer.json`; writes nothing.

## Exit codes

Same as [viv show](show.html): `0` printed, `1` a filesystem error, unknown
package or bad flag.

## See also

[viv show](show.html), [viv why](why.html)
