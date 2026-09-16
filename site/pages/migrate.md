# Migrating from Composer

The migration story already lives in the README; this page pulls the three
sections a team moving a project onto viv actually needs, kept in sync with
the README at build time.

## The `composer` shim

{{shim_from_readme}}

{{shim_section}}

## In a Dockerfile

{{dockerfile_from_readme}}

{{dockerfile_section}}

## CI flags

{{ci_from_readme}}

A script that already runs Composer with `--prefer-dist --no-interaction
--no-progress` needs no changes: `viv install` and `viv dump-autoload`
accept those flags and ignore them, the same way they do for Composer.
`--ignore-platform-reqs` is a real flag on both tools, not a no-op.

The one behaviour that differs: a plugin outside viv's native list stops
the install with an error naming the plugin, loudly, rather than silently
skipping its work. `--no-plugins` turns that into a warning and installs
the way Composer's own `--no-plugins` would — see
[`docs/plugin-strategy.md`](https://github.com/svandragt/vivace/blob/main/docs/plugin-strategy.md)
for which plugins that's safe for.

## What's not covered

{{stability_summary}}
