# vivace

`viv` installs PHP dependencies from a Composer `composer.lock` file and
writes a `vendor/` directory that's byte-identical to what Composer would
write — the same files, down to the last byte. It's a proof of concept at
v0.7.0, tested in CI on Linux and macOS, and not yet at 1.0.

## Try it

Download a prebuilt binary from the [releases
page](https://github.com/svandragt/vivace/releases) (Linux x86_64, aarch64
and musl; macOS x86_64 and aarch64), or install it another way:

```sh
cargo binstall vivace
# or build from source:
cargo install --git https://github.com/svandragt/vivace --tag v0.7.0 vivace
```

Then run it in a project that already has a `composer.json` and
`composer.lock`:

```sh
viv install
```

If Composer already wrote the `vendor/` directory, viv adopts it: it
relinks every package from its own store in place, no flag needed. Only
`composer install` through the shim asks for confirmation first, and only
in a terminal. If a package can't be downloaded (a private package behind
a licence key, for example), viv keeps Composer's copy of that package,
prints a warning, and adopts the rest. To force a relink of a `vendor/`
that viv itself wrote, run `viv install --adopt`.

## Is it safe to try

viv's contract is that its output matches Composer's byte for byte. Before
every release, a [compatibility sweep](compat/README.md) installs a mix of
pinned popular projects and a random sample of Packagist packages with both
Composer and viv, then diffs the two `vendor/` trees. The v0.7.0 sweep: 36
rows identical, 0 differ, 4 skipped (the skips are pre-existing failures on
Composer's side, such as an expired auth token — not something viv got
wrong). Full results, including which projects and what was skipped, are in
[`compat/results/v0.7.0.md`](compat/results/v0.7.0.md).

Of the 19 pinned projects, 2 still need `--no-plugins` to install with viv:
`symfony/demo` and `roots/bedrock`, both for a Composer plugin viv doesn't
yet support. See [Plugins](#plugins) below.

## Speed

Laravel-sized lock, 101 packages, same machine, three runs each. Details and
raw data in [`bench/results/`](bench/results/README.md).

| Tool | Cold | Warm cache | No-op | Update, warm metadata |
|---|---|---|---|---|
| composer 2.10.2 | 7.28 s | 1.05 s | 0.47 s | 1.23 s |
| riff 0.0.7 | 1.74 s | 0.24 s | 0.24 s | n/a\* |
| viv 0.6.0 | 2.26 s | 0.043 s | 0.008 s | 1.12 s |

\* riff 0.0.7 can't resolve this lock's `update`: see
[`bench/results/README.md`](bench/results/README.md#update-warm-metadata).

This numbers table was last measured on the 0.6.0 release; the figures
above are still that run.

Update is close to Composer's speed because the pool builder loads only the
versions the accumulated constraints allow, as Composer's does: 108
metadata requests instead of 251, and a pool of 615 packages instead of
5169 on this lock, with the written lock still identical to Composer's.
Cold install is the one column riff wins, bounded by GitHub's zipball
throttling ([`bench/results/profile.md`](bench/results/profile.md)).

### Public corpus

The pinned projects from [`compat/corpus.toml`](compat/corpus.toml), same
machine, `--no-plugins --no-scripts`, three runs, warm metadata cache.
Full columns and footnotes in [`bench/results/corpus.md`](bench/results/corpus.md).

Columns are `update`, warm metadata (the same scenario as the Numbers
table's last column).

| Project | Packages | composer | riff | viv |
|---|---|---|---|---|
| laravel/laravel | 109 | 1.75 s | fails | 1.43 s |
| symfony/demo | 153 | 1.66 s | 0.73 s | 1.49 s |
| drupal/recommended-project | 68 | 2.85 s | fails | 1.14 s |
| roots/bedrock | 73 | 2.45 s | fails | 1.56 s |
| composer/composer | 36 | 0.63 s | 0.25 s | 0.39 s |
| phpunit/phpunit | 26 | 0.70 s | fails | 0.26 s |
| slimphp/Slim-Skeleton | 57 | 0.76 s | fails | 0.35 s |
| statamic/statamic | 160 | 2.40 s | fails | 1.88 s |
| craftcms/craft | 118 | 2.15 s | 0.61 s | 1.37 s |

Warm install and no-op are an order of magnitude faster than both tools on
every project. Cold install beats Composer everywhere, and is level with
riff within run-to-run noise. `yiisoft/yii2-app-basic` is in the corpus but
not in the table: riff fails its install by applying a dependency's patches
under `--no-plugins`, where Composer and viv install it fine.

## Everyday commands

```sh
viv install            # in a project with composer.json and composer.lock
viv install --no-dev
viv install --dry-run  # show the plan, change nothing
viv install --link-mode copy
viv update                # resolves, writes composer.lock and installs
viv update psr/log -w     # partial update with dependencies, then installs
viv update --no-install   # resolve and write the lock only
viv require psr/container # edits composer.json, updates the lock and installs
viv remove psr/container  # same, minus the package (--no-install opts out too)
viv dump-autoload -o
viv normalize --check     # update/require/remove also normalise when they write
viv cache prune
viv diagnose              # environment/config report to paste into a bug report
```

`install` runs the root's lifecycle scripts (`pre-install-cmd`,
`post-autoload-dump`, `post-install-cmd`) as Composer does; `--no-scripts`
skips them.

`update`, `require` and `remove` also normalise `composer.json` when they
write it: stable key order and whitespace, the same result as `composer
normalize`. That means two branches that each add a dependency merge
cleanly instead of fighting over ordering. `install` never touches
`composer.json`. Pass `--no-normalize` to opt out; `viv normalize --check`
reports without writing.

## Using viv as composer

`make install-shim` installs a `composer` binary next to `viv` (plain `make
install` leaves your real Composer untouched). Put it on `PATH` ahead of
the real Composer, or symlink it as `composer` in CI, and it maps
`install`, `dump-autoload` and `normalize` with their supported flags to
`viv`. Everything else — `update`, `require`, plugins, an unrecognised flag
— it hands through to the real Composer binary unchanged. Point
`VIV_COMPOSER_PATH` at the real binary if it isn't first on `PATH`.

The shim only has something to hand through to if a real Composer is on
`PATH` in the first place: if there isn't one, those commands fail rather
than silently falling back to viv.

## Plugins

Composer plugins are PHP code that hooks into Composer's own process; viv
has no PHP runtime, so it can't run one as written. Instead:

- A small set of common plugins — `composer/installers`, the WordPress core
  installers, and native adapters for Yii2, Craft, Drupal scaffolding,
  Symfony runtime, phpcs, PHPStan and more — are reimplemented in viv
  itself, so the outcome matches Composer's.
- A few plugins are known to only affect Composer commands viv doesn't
  implement; viv ignores them, same as Composer does when a plugin is
  disabled.
- Any other plugin stops the install with an error naming the plugin.
  `--no-plugins` turns that into a warning and installs the way Composer's
  own `--no-plugins` would.

The full list of which plugin falls into which category is in
[`docs/plugin-strategy.md`](docs/plugin-strategy.md).

## Beyond Composer

**Run a tool without installing it.** `viv x vendor/package[:constraint]`
installs the package into an isolated, content-hashed environment under the
cache and runs its binary, the way `uvx` and `npx` do. Nothing is added to
the project's `composer.json` or `vendor/`:

```sh
viv x phpunit/phpunit:^11 tests      # 27 packages, 0.07 s on a warm cache
viv x friendsofphp/php-cs-fixer fix src
viv x --list                         # environments in the cache
```

## How it works

viv keeps a global store under `$XDG_CACHE_HOME/vivace/`: each package
archive is extracted once, keyed by the sha256 hash of the archive.
Installing hardlinks every file from the store into `vendor/`, rather than
inflating and writing it again each time. Store files are read-only, so an
accidental edit to a vendor file fails instead of silently changing every
project that shares the same file on disk. Projects that patch their vendor
files should use `--link-mode copy` instead.

A no-op install compares the lock against `vendor/composer/installed.json`
and a small state file; it doesn't spawn PHP or make a network request.

The autoloader files (`vendor/autoload.php`, `vendor/composer/*.php`,
`installed.json`, `installed.php`, `platform_check.php`) are generated by
a port of Composer's own generator, tested against Composer's own golden
test cases, and checked end to end by byte-diffing `vendor/` against real
Composer 2.10.2 output. See [`ARCHITECTURE.md`](ARCHITECTURE.md) and
[`docs/composer-contract.md`](docs/composer-contract.md) for the detail,
and [`docs/stability.md`](docs/stability.md) for what viv promises to keep
working across releases and what may still change.

## Scope

viv aims to replace Composer for the commands people run every day, not the
whole command reference. Covered, byte for byte where the output is a file
or Composer's plain text: `install`, `update`, `update-lock`, `require`,
`remove`, `dump-autoload`, `normalize`, `show`, `tree`, `why`, `outdated`,
`audit`, `validate`, `run`, `exec`, `diagnose` (viv's own report, not
Composer's text), the lifecycle scripts, the cache commands, and Satis or
Private Packagist repositories. Beyond Composer: `viv x vendor/tool` runs a
Packagist tool without installing it into the project, the way uvx does.
Everything else (`create-project`, `init`, `search`, `config`, `global`,
`self-update`, `licenses`, `depends` and friends) stays with Composer, and
the `composer` shim hands those through unchanged.

### What is in and out

In: zip and tar dists, path repositories, git sources without a dist,
credentials from `auth.json` and `COMPOSER_AUTH`, sha1 verification, PSR-4,
PSR-0, classmap and files autoloading, `--optimize-autoloader` and
`--classmap-authoritative`, `platform_check.php`, `vendor/bin` proxies,
lifecycle scripts, `composer/installers` paths, full and partial `update`,
`require`, `remove`, Packagist v2 metadata with a revalidating cache.

Also in since 0.4: `preferred-install: source`, offline mode
(`--offline`, `COMPOSER_DISABLE_NETWORK`), native adapters for the phpcs
installer, phpstan extension-installer, spi and composer-patches, extraction
size caps, tar.bz2, `viv cache size`, a classmap cache for `-o`.

Also in since 0.6: `vcs`, `git` and `github` repositories in `update`,
`require` and `remove`; those three commands install after writing the
lock, as Composer does (`--no-install` opts out); a php-http/discovery
adapter; `viv add` and `viv rm`; a weekly compatibility sweep; Debian
packages and a Homebrew formula on each release.

Out for now: other Composer plugins (refused, see [Plugins](#plugins)
above), `--minimal-changes`, Composer's condensed multi-cause problem
messages, and update speed: it resolves correctly, but takes about four
times as long as Composer on a large lock, tracked in
[#90](https://github.com/svandragt/vivace/issues/90) and
[#91](https://github.com/svandragt/vivace/issues/91).

## Development

Tooling comes from [devbox](https://www.jetify.com/devbox): PHP, Composer
and hyperfine for the fixtures and benchmarks.

```sh
make install    # put viv on your PATH (~/.cargo/bin); make install-shim adds the composer drop-in
make check      # fmt, clippy, tests, cargo deny
make test
make bench      # composer vs riff vs viv on bench/laravel
VIVACE_TEST_NETWORK=1 make test   # includes the end-to-end install
```

[`JOURNAL.md`](JOURNAL.md) is the engineering log: what was measured, what
was decided and why, including the study of
[Riff](https://github.com/shyim/riff) and
[Presto](https://github.com/paramientos/presto) that preceded the code.

## Licence

MIT. `src/autoload/templates/` contains Composer's `ClassLoader.php`,
`InstalledVersions.php` and licence, copied verbatim under Composer's MIT
licence. `src/spdx-licenses.json` is `composer/spdx-licenses`' own resource
file, also copied verbatim under its MIT licence. Test fixtures under
`tests/fixtures/composer/` are Composer's own, also MIT.
