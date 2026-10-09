---
title: Automatic normalisation
order: 80
summary: Stable key order and whitespace for composer.json, on demand.
---

# Automatic normalisation

`viv init` normalises the `composer.json` it writes: stable key order and
whitespace, the same result as running `composer normalize`.[^11] `viv add`
and `viv rm` change only the requirement they add or remove, as Composer
does, and leave the rest of the file as you wrote it. Run `viv normalize` to
normalise it yourself, or `viv normalize --check` to only report.

```sh
viv normalize --check     # exits non-zero if composer.json is not normalised
```

[^11]: Normalisation follows `ergebnis/composer-normalize`'s rules, so a project already using that plugin sees no change.
