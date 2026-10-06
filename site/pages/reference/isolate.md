---
title: viv isolate
order: 10
summary: prefixes a plugin's bundled dependencies so they stop clashing with the site's own
---

# viv isolate

Rewrites a plugin's bundled `vendor/` under its own namespace prefix with
php-scoper, so a library the plugin bundles at one version stops clashing
with the same library the site, or another plugin, ships at another.
Reach for it once `viv install` names the clash: it prints the plugin,
the library and both versions, and the exact `viv isolate <plugin>` to
run. `install` and `update` then keep every plugin in `extra.viv.isolate`
prefixed. `--no-plugins` and `--no-scripts` work as they do on `install`:
a lock that names a plugin viv refuses stops `viv isolate` too, unless you
pass `--no-plugins`.

## Usage

```
{{help:isolate}}
```

## Reads and writes

- Reads: `extra.viv.isolate` in `composer.json`, the plugin's own install
  path under `vendor/`.
- Writes: `extra.viv.isolate` in `composer.json` (adding or removing a
  package); the store's `isolated-v0` bucket (the scoped tree) and
  `isolate-check-v0` bucket (the cached per-plugin clash verdict);
  `vendor/composer/.vivace-state` (the prefix and the php-scoper version
  last checked against).

## Exit codes

- `0` — isolated (or already isolated, or removed) and the load check
  passed.
- `1` — `php -l` or the load check failed on the scoped tree, and the
  plugin's plain archive stays linked; or `--offline` found no cached
  scoped tree for the plugin, and the error names it.

## See also

[Two plugins, one library](../guides/isolate.html), [viv x](x.html),
[viv php](php.html), [viv install](install.html)
