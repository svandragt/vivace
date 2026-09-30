---
title: Automatic normalisation
order: 80
summary: A composer.json that never gets a reordering-only diff.
---

# Automatic normalisation

Every command that writes `composer.json` (`add`, `rm`, `init`) also
normalises it: stable key order and whitespace, the same result as running
`composer normalize`. You never commit a diff that is only reordering.[^11]
Run `viv normalize --check` for an explicit run that only reports without
writing.

```sh
viv normalize --check     # add/rm/init also normalise when they write
```

[^11]: Normalisation follows `ergebnis/composer-normalize`'s rules, so a project already using that plugin sees no change.
