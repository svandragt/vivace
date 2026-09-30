---
title: Migrating from Composer
order: 130
summary: The shim, Dockerfiles and CI flags for moving a whole project onto viv.
---

# Migrating from Composer

The migration story already lives in the README; this page pulls the three
sections a team moving a project onto viv actually needs, kept in sync with
the README at build time.

## The `composer` shim

{{include:README.md#Using viv as composer}}

## In a Dockerfile

{{include:README.md#In a Dockerfile}}

## CI flags

A script that already runs Composer with `--prefer-dist --no-interaction
--no-progress` needs no changes: `viv install` and `viv dump-autoload`
accept those flags and ignore them, the same way they do for Composer.
`--ignore-platform-reqs` is a real flag on both tools, not a no-op.

The one behaviour that differs: a plugin outside viv's native list stops
the install with an error naming the plugin, loudly, rather than silently
skipping its work. `--no-plugins` turns that into a warning and installs
the way Composer's own `--no-plugins` would — see [Plugin
strategy](../architecture/plugin-strategy.html) for which plugins that's
safe for.

## What's not covered

See [Compatibility and scope](../reference/compatibility.html) for what
viv's byte-identical promise covers, and [Reasons not to use
viv](reasons.html) for the rest.
