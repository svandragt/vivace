---
title: In a Dockerfile
order: 40
summary: Build a vendor/ stage with the published image, no PHP runtime needed.
---

# In a Dockerfile

To build a `vendor/` stage without a PHP runtime, use the published image
in place of `composer:2`:

```dockerfile
FROM ghcr.io/svandragt/vivace:0 AS vendor
COPY composer.json composer.lock ./
RUN ["viv", "install", "--no-dev"]

FROM php:8.4-fpm
COPY --from=vendor /app/vendor /app/vendor
```

Two things differ from the `composer:2` stage it replaces:

- Write `RUN` in exec form, as above. The image has no shell, so the
  familiar `RUN viv install --no-dev` does not work.
- The image runs `viv` by default and ships the `composer` shim beside it,
  so `RUN ["composer", "install", "--no-dev"]` works too if you would
  rather not edit the command.
- The image carries no real Composer to fall back to, so a command or flag
  the shim doesn't understand hard-errors there instead of silently running
  Composer, the way it would on a machine that still has Composer installed.
  `composer --version` still works: it prints the shim's own version.

Tags are `:0.21`, `:0.21.0` and `:0`. There is no `:latest`: a moving tag
that silently resolves to nothing breaks scripted installs, which is the
mistake that kept `releases/latest` returning 404 for ten releases.
