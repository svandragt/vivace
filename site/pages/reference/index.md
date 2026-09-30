---
title: Reference
order: 0
summary: every viv command, its global options, environment variables and exit codes
---

# Reference

One page per command, generated from its own `--help` output so the flags
here never drift from the binary.

## Global options

```
{{help:}}
```

## Commands

| Command | What it does |
|---|---|
| [viv init](init.html) | Writes a new project's composer.json and stops, no interactive prompts. |
| [viv new](new.html) | Starts a project from a bare name or a `vendor/package` skeleton. |
| [viv install](install.html) | Installs the exact versions composer.lock records. |
| [viv update](update.html) | Resolves composer.json, writes the lock and installs. |
| [viv add](add.html) | Adds a dependency to composer.json, resolves it and installs. |
| [viv rm](rm.html) | Removes a dependency from composer.json, resolves the rest and installs. |
| [viv dump-autoload](dump-autoload.html) | Regenerates the autoload files from an already-installed vendor/. |
| [viv normalize](normalize.html) | Tidies composer.json's key order and formatting. |
| [viv cache](cache.html) | Prunes, cleans or reports the size of the shared store. |
| [viv audit](audit.html) | Checks installed or locked packages for security advisories and abandoned packages. |
| [viv show](show.html) | Lists installed packages, or inspects one. |
| [viv tree](tree.html) | Shorthand for `show --tree`. |
| [viv why](why.html) | Lists installed packages that require the named package. |
| [viv outdated](outdated.html) | Flags installed packages with a newer version available. |
| [viv validate](validate.html) | Checks composer.json (and composer.lock) against Composer's rules. |
| [viv x](x.html) | Installs and runs a package's binary in an isolated, cached environment. |
| [viv run](run.html) | Runs a scripts entry from the root composer.json. |
| [viv exec](exec.html) | Runs a vendor/bin binary with vendor/bin prepended to PATH. |
| [viv php](php.html) | Downloads a static-php-cli build and pins config.platform.php to it. |
| [viv diagnose](diagnose.html) | Prints an environment and configuration report for a bug report. |
| [viv lock](lock.html) | Converts, merges or exports a viv.lock/composer.lock pair. |
| [viv workspace](workspace.html) | Discovers and manages a monorepo's member packages. |

## Environment variables

viv's own:

| Variable | What it does | Commands |
|---|---|---|
| `VIV_METADATA_TTL` | Same as `--metadata-ttl`: skip revalidating cached metadata younger than this many seconds; the flag wins when both are set. | update, add, rm |
| `VIV_MAX_INFLATED_BYTES` | Overrides the computed cap on how many bytes a single archive may inflate to, viv's zip-bomb guard. | install, update, add, rm |
| `VIV_COMPOSER_PATH` | Points the `composer` shim at the real Composer binary, when it isn't first on PATH. | the composer shim |
| `VIV_SHIM_STRICT` | Set to make the `composer` shim hard-error on an unrecognised command or flag, instead of falling back to the real Composer. | the composer shim |
| `VIV_VIA_SHIM` | Set by the shim itself so `install --adopt`'s own detection can tell it's running under it. | install |
| `VIV_PHP_DIST_URL` | Overrides the static-php-cli build base URL `php install` fetches from. | php install |

Composer's own, that viv also reads:

| Variable | What it does | Commands |
|---|---|---|
| `COMPOSER_HOME` | Where viv looks for auth.json and config.json. | all |
| `COMPOSER_AUTH` | JSON credentials, merged over the Composer home's and the project's auth.json. | all |
| `COMPOSER_DISABLE_NETWORK` | Any value but unset, empty or `0` acts like `--offline`. | all |
| `COMPOSER_NO_SECURITY_BLOCKING` | Any value but unset, empty or `0` acts like `--no-blocking`. | update, add, rm |
| `XDG_CACHE_HOME` | Where viv's store lives, as `$XDG_CACHE_HOME/vivace`; falls back to `~/.cache/vivace`. | all |
| `XDG_CONFIG_HOME` | Falls back into COMPOSER_HOME's own default when neither it nor a legacy `~/.composer` is present. | all |
| `COLUMNS` | Terminal width `viv show` wraps its output to; defaults to 80 when unset or not a number. | show, tree, why |

`COMPOSER_CACHE_DIR` is Composer's own cache location variable; viv doesn't
read it: use `XDG_CACHE_HOME` or `--cache-dir` instead.

## Exit codes

`0` on success. A dependency-resolution failure (`update`, `add`, `rm`,
`install`, `workspace init`/`add`) exits `2`, matching Composer's own
`ERROR_DEPENDENCY_RESOLUTION_FAILED`; every other error, including a bad flag,
exits `1`. `audit` and `validate` use their own exit codes to report a
finding rather than a failure to run; see each page.
