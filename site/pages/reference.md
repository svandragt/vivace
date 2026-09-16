# Reference

Exit codes, environment variables, the `composer.json` config keys viv
reads, the files it writes, and every global option.

## Exit codes and the stderr/stdout contract

{{doc:docs/stability.md#Interface for the shim and scripts}}

## Environment variables

viv's own:

| Variable | What it does |
|---|---|
| `VIV_METADATA_TTL` | Same as `--metadata-ttl` on `update`, `add` and `rm`: skip revalidating a package's cached metadata while it's younger than this many seconds. The flag wins when both are set. ([`src/update.rs`](https://github.com/svandragt/vivace/blob/main/src/update.rs)) |
| `VIV_COMPOSER_PATH` | Points the `composer` shim at the real Composer binary, for when it isn't first on `PATH`. ([`src/bin/composer.rs`](https://github.com/svandragt/vivace/blob/main/src/bin/composer.rs)) |
| `VIV_SHIM_STRICT` | Set to make the `composer` shim hard-error on a command or flag it doesn't understand, instead of falling back to the real Composer. ([`src/bin/composer.rs`](https://github.com/svandragt/vivace/blob/main/src/bin/composer.rs)) |
| `VIV_MAX_INFLATED_BYTES` | Overrides the computed cap on how many bytes a single archive may inflate to, viv's guard against a zip bomb. ([`src/store.rs`](https://github.com/svandragt/vivace/blob/main/src/store.rs)) |

A few more (`VIV_TEST_NOW`, `VIV_TEST_EXTRACT_WORKERS`, `VIV_TEST_SCAN_WORKERS`)
exist only to make viv's own test suite deterministic; they're not a
documented interface.

Composer's own, that viv also reads:

| Variable | What it does |
|---|---|
| `COMPOSER_HOME` | Where viv looks for `auth.json` and `config.json`, same as Composer: `$COMPOSER_HOME` if set, else `~/.composer` if that directory already exists, else `$XDG_CONFIG_HOME/composer`. ([`src/auth.rs`](https://github.com/svandragt/vivace/blob/main/src/auth.rs)) |
| `COMPOSER_AUTH` | JSON credentials, merged over the composer home's and the project's `auth.json` — highest precedence of the three. ([`src/auth.rs`](https://github.com/svandragt/vivace/blob/main/src/auth.rs)) |
| `COMPOSER_DISABLE_NETWORK` | Any value but unset, empty or `0` acts like `--offline`. ([`src/main.rs`](https://github.com/svandragt/vivace/blob/main/src/main.rs)) |
| `COMPOSER_NO_SECURITY_BLOCKING` | Any value but unset, empty or `0` acts like `--no-blocking`: allow a version with a known security advisory during `update`, `add` or `rm`. ([`src/update.rs`](https://github.com/svandragt/vivace/blob/main/src/update.rs)) |
| `XDG_CACHE_HOME` | Where viv's store lives, as `$XDG_CACHE_HOME/vivace`; falls back to `~/.cache/vivace`. ([`src/update.rs`](https://github.com/svandragt/vivace/blob/main/src/update.rs)) |
| `XDG_CONFIG_HOME` | Falls back into `COMPOSER_HOME`'s own default, above, when neither `COMPOSER_HOME` nor a legacy `~/.composer` is present. ([`src/auth.rs`](https://github.com/svandragt/vivace/blob/main/src/auth.rs)) |
| `COLUMNS` | Terminal width `viv show` wraps its output to; defaults to 80 when unset or not a number. ([`src/show.rs`](https://github.com/svandragt/vivace/blob/main/src/show.rs)) |

`COMPOSER_CACHE_DIR` is Composer's own cache location variable; viv doesn't
read it; use `XDG_CACHE_HOME` or `--cache-dir` instead.

## `composer.json` config keys

viv reads this subset of the root `composer.json`'s `config` block
(`src/lock.rs`, the `Config` struct):

| Key | What it does |
|---|---|
| `autoloader-suffix` | The suffix on the generated `ComposerAutoloaderInit`/`ComposerStaticInit` class names. |
| `platform-check` | Whether and how strictly `platform_check.php` verifies the PHP version and extensions at runtime. |
| `vendor-dir` | Where packages install (default `vendor`). |
| `prepend-autoloader` | Whether the generated autoloader registers itself with `prepend: true`. |
| `bin-dir` | Where `vendor/bin` proxies are written; defaults to `<vendor-dir>/bin` when unset. |
| `bin-compat` | How `vendor/bin` proxy scripts are generated for compatibility across platforms. |
| `optimize-autoloader` | `install -o`'s default: also classmap-scan PSR-0/PSR-4 directories. |
| `classmap-authoritative` | `install -a`'s default: classmap-only autoloading, skipping the PSR fallback. |
| `apcu-autoloader` | `install --apcu-autoloader`'s default. |
| `apcu-autoloader-prefix` | The default value for `install --apcu-autoloader-prefix`. |
| `use-include-path` | Whether the generated loader also searches PHP's include path. |
| `secure-http` | `false` lets dist/repository URLs downgrade to plain `http` (Composer defaults this `true`). |
| `allow-plugins` | Which `composer-plugin` packages viv treats as enabled — see [Plugins](plugins.html). |
| `preferred-install` | `dist`/`source` per package; the project's setting is merged with the Composer home's own `config.json`. |
| `audit.ignore`, `audit.abandoned` | `viv audit`'s ignore list and its abandoned-package policy. |
| `audit.block-insecure`, `audit.block-abandoned` | Whether a version with a known advisory, or an abandoned package, is filtered from the resolver pool during `update`, `add` or `rm`. |

A `config` key not in this table is ignored: viv doesn't read it, and
doesn't warn that it's unread either.

## Files viv writes in `vendor/`

{{doc:docs/composer-contract.md#Files}}

## Global options

```
{{help:}}
```
