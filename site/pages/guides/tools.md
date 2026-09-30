---
title: Running tools without installing them
order: 50
summary: viv x installs a package into an isolated cache and runs its binary, npx-style.
---

# Running tools without installing them

`viv x vendor/package[:constraint]` installs the package into an isolated,
cached environment and runs its binary, the way `uvx` and `npx` do. Nothing
is added to your `composer.json` or `vendor/`:

```sh
viv x phpunit/phpunit:^11 tests      # 27 packages, 0.07 s on a warm cache
viv x friendsofphp/php-cs-fixer fix src
viv x --list                         # environments in the cache
```
