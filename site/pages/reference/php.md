---
title: viv php
order: 190
summary: downloads a static-php-cli build and sets extra.viv.php to it
---

# viv php

Downloads a static-php-cli PHP build and sets `extra.viv.php` in
composer.json to it, or lists what's already installed. Reach for this to
give a project a self-contained PHP it doesn't have to share with the rest
of the machine. With `extra.viv.php` set, viv runs that build instead of the
`php` on `PATH`; without it, the `php` on `PATH` runs, and
`config.platform.php` only tells the solver which PHP to resolve for.

## Usage

```
{{help:php}}
```

### viv php install

Downloads the build for `VERSION` (`8.4` resolves to the newest `8.4.x`
upstream, `8.4.17` an exact release) and sets `extra.viv.php` to it, leaving
`config.platform.php` as it is; with no `VERSION`, installs whatever
`extra.viv.php` already names.

```
{{help:php install}}
```

### viv php list

Lists every PHP build already installed in the cache; writes nothing.

```
{{help:php list}}
```

## Reads and writes

- Reads: `composer.json`'s `extra.viv.php`, the store's `php-v0` bucket,
  and fetches the build over the network (unless already cached).
- Writes: `composer.json` (`install` sets `extra.viv.php`), the store's
  `php-v0` bucket.

## Exit codes

- `0` — installed or listed successfully.
- `1` — a network error, an unknown version, or a bad flag.

## See also

[viv install](install.html)
