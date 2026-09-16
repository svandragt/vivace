# Troubleshooting

## A plugin was refused

**Symptom:** the install stops with `` viv cannot run the Composer plugin
`vendor/package`. ``

viv has no PHP runtime, so it can only run a plugin it has a native adapter
for — see [Plugins](plugins.html) for the full list and how a plugin is
categorised. The error itself says whether skipping the plugin is safe.

Pass `--no-plugins` to install without it, the way Composer's own
`--no-plugins` would. If the plugin does real work viv hasn't ported, check
what it writes before relying on the result.

A CI job that wants to know the moment a plugin gap reopens the door back to
Composer can set `VIV_SHIM_STRICT=1` on the [`composer` shim](shim.html): it
turns an unrecognised command or flag into a hard error instead of a silent
fallback (this is separate from a refused plugin, which always stops the
install unless you pass `--no-plugins`).

## The shim ran the real Composer

**Symptom:** stderr says `composer (viv shim): running the real Composer`.

A command or flag the [`composer` shim](shim.html) doesn't map to viv falls
back to your real Composer install, with that note so a migration that
quietly stopped using viv is visible instead of just slower. Set
`VIV_SHIM_STRICT=1` to turn that fallback into a hard error instead, for a CI
job that wants a red build rather than a silent return to Composer.

## A private package can't be downloaded

**Symptom:** a warning names a package viv couldn't fetch.

If a package can't be downloaded — behind a licence key Composer already had
credentials for, for example — viv keeps Composer's existing copy of that
package in `vendor/`, warns, and adopts the rest rather than failing the
whole install. To fix the download itself, give viv the same credentials
Composer used: an `auth.json` (project or Composer home) or `COMPOSER_AUTH`
— see [Environment variables](reference.html) for how viv reads each.

## `vendor/` differs from what Composer wrote

**Symptom:** a file in `vendor/` doesn't match a plain `composer install`, or
you're not sure it does.

If `vendor/` was written by Composer, viv adopts it automatically on the next
`viv install` — no flag needed. To force viv to re-relink a `vendor/` it
already wrote itself, run `viv install --adopt`. The [Compare](compare.html)
page shows viv's compatibility sweep results against real Composer output.

To file an issue, include a `viv diagnose` report:

```
{{help:diagnose}}
```

## The lock is stale, or a requirement is missing

**Symptom:** `install` reports a package "is not present in the lock file",
followed by a note about merge conflicts.

This is Composer's own stale-lock message: `composer.json` names a
requirement `composer.lock` doesn't have an entry for, usually from a bad
merge or a hand edit. Run `viv update` to resolve fresh versions and rewrite
the lock, or `viv update <package> -w` for a partial update including that
package's dependencies.

## Platform requirement failures

**Symptom:** `update`, `add` or `rm` fails to resolve because of a PHP
version or extension the interpreter running viv lacks.

Pass `--ignore-platform-reqs` to install as if those requirements were met.
Two gaps to know about: on `update`, `add` and `rm` this flag affects the
autoload write, not the solve, so a package pinned to a PHP version this
interpreter lacks can still fail to resolve; and a version filtered out by
`minimum-stability` is reported as not found rather than as filtered, where
Composer names the actual cause.
