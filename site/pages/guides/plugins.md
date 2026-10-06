---
title: Plugins
order: 90
summary: Which Composer plugins viv reimplements, ignores or refuses.
---

# Plugins

Composer plugins are PHP code that hooks into Composer's own process; viv
has no PHP runtime, so it can't run one as written. Instead:

- A small set of common plugins — `composer/installers`, the WordPress core
  installers, and native adapters for Yii2, Craft, Altis, Drupal scaffolding,
  Symfony runtime, phpcs, PHPStan and more — are reimplemented in viv
  itself, so the outcome matches Composer's.
- A few plugins are known to only affect Composer commands viv doesn't
  implement; viv ignores them, same as Composer does when a plugin is
  disabled.
- Any other plugin stops the install with an error naming the plugin.
  `--no-plugins` turns that into a warning and installs the way Composer's
  own `--no-plugins` would.

The full list of which plugin falls into which category is documented
separately.[^13] The rule for when a plugin is a data file rather than an
adapter lives in
[`docs/plugin-strategy.md`, "Data file or adapter"](docs/plugin-strategy.md#data-file-or-adapter).

## Inventory

{{include:docs/plugin-strategy.md#Inventory}}

## Rule

{{include:docs/plugin-strategy.md#Rule}}

The full rule for when a plugin is a data file rather than an adapter is in
[Architecture: Plugin strategy](../architecture/plugin-strategy.html).

[^13]: [`docs/plugin-strategy.md`](docs/plugin-strategy.md) lists which plugin falls into which category.
