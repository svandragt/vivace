# vivace

`viv` installs PHP dependencies from `composer.lock` and produces a `vendor/`
directory that is byte for byte what Composer would write. Since 0.3 it also
resolves: `viv update`, `viv require` and `viv remove` write a `composer.lock`
that Composer accepts unchanged, using a port of Composer's own solver.

Status: proof of concept, v0.6. Linux and macOS, both tested in CI. Before
each release a [compatibility sweep](compat/README.md) byte-diffs `vendor/`
against Composer on pinned popular projects and a random Packagist sample.

## Numbers

Laravel-sized lock, 101 packages, same machine, three runs each. Details and
raw data in [`bench/results/`](bench/results/README.md).

| Tool | Cold | Warm cache | No-op | Update, warm metadata |
|---|---|---|---|---|
| composer 2.10.2 | 7.28 s | 1.05 s | 0.47 s | 1.23 s |
| riff 0.0.7 | 1.74 s | 0.24 s | 0.24 s | n/a\* |
| viv 0.6.0 | 2.26 s | 0.043 s | 0.008 s | 1.12 s |

\* riff 0.0.7 can't resolve this lock's `update`: see
[`bench/results/README.md`](bench/results/README.md#update-warm-metadata).

Update is at Composer's speed since the pool builder was made to load
only the versions the accumulated constraints allow, as Composer's does:
108 metadata requests instead of 251 and a pool of 615 packages instead
of 5169 on this lock, with the written lock still identical to
Composer's. Cold install is the one column riff wins, bounded by GitHub's
zipball throttling ([`bench/results/profile.md`](bench/results/profile.md)).
Tracked per release like the install numbers.

### Public corpus

The pinned projects from [`compat/corpus.toml`](compat/corpus.toml), same
machine, `--no-plugins --no-scripts`, three runs, warm metadata cache.
Full columns and footnotes in [`bench/results/corpus.md`](bench/results/corpus.md).

Columns are `update`, warm metadata (the same scenario as the Numbers table's
last column).

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

Warm install and no-op are an order of magnitude under both tools on every
project. Cold install beats Composer everywhere and sits level with riff
within run-to-run noise. yiisoft/yii2-app-basic is in the corpus but not in
the table: riff fails its install by applying a dependency's patches under
`--no-plugins`; Composer and viv install it.

## Beyond Composer

**Fewer merge conflicts.** `viv update`, `viv require` and `viv remove`
normalise `composer.json` when they write it: stable key order and
whitespace, the same result as `composer normalize`. Two branches that each
add a dependency then merge cleanly instead of fighting over ordering. `viv
install` never touches `composer.json`. Pass `--no-normalize` to opt out;
`viv normalize --check` reports without writing.

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

Each package archive is extracted once into a global store under
`$XDG_CACHE_HOME/vivace/`, keyed by the sha256 of the archive. Installing
hardlinks every file from the store into `vendor/`, one `link()` call per
file instead of inflating and writing it. Store files are read-only, so an
accidental edit to a vendor file fails instead of silently changing every
project that shares the inode. Projects that patch vendor use
`--link-mode copy`.

A no-op install compares the lock against `vendor/composer/installed.json`
and a small state file, spawns no PHP and makes no network requests.

The autoloader (`vendor/autoload.php`, `vendor/composer/*.php`,
`installed.json`, `installed.php`, `platform_check.php`) is a port of
Composer's generator. Composer's own golden test cases are in the suite, and
an end-to-end test byte-diffs `vendor/` against Composer 2.10.2 output. See
[`ARCHITECTURE.md`](ARCHITECTURE.md) and
[`docs/composer-contract.md`](docs/composer-contract.md). What viv
promises across releases, and what may change, is in
[`docs/stability.md`](docs/stability.md).

## Install

Prebuilt binaries for Linux (x86_64, aarch64, musl) and macOS (x86_64,
aarch64) are attached to each [release](https://github.com/svandragt/vivace/releases).

```sh
# download a tarball from the releases page, or:
cargo binstall vivace
# or build from source:
cargo install --git https://github.com/svandragt/vivace --tag v0.6.0 vivace
```

## Usage

```sh
cargo build --release
target/release/viv install            # in a project with composer.json and composer.lock
target/release/viv install --no-dev
target/release/viv install --dry-run  # show the plan, change nothing
target/release/viv install --link-mode copy
target/release/viv update                # resolves, writes composer.lock and installs
target/release/viv update psr/log -w     # partial update with dependencies, then installs
target/release/viv update --no-install   # resolve and write the lock only
target/release/viv require psr/container # edits composer.json, updates the lock and installs
target/release/viv remove psr/container  # same, minus the package (--no-install opts out too)
target/release/viv dump-autoload -o
target/release/viv normalize --check     # update/require/remove also normalise when they write
target/release/viv cache prune
target/release/viv diagnose              # environment/config report to paste into a bug report
```

`install` runs the root's lifecycle scripts (`pre-install-cmd`,
`post-autoload-dump`, `post-install-cmd`) like Composer; `--no-scripts` skips
them. Composer plugins cannot run natively: `composer/installers` and the
WordPress core installers are applied natively, anything else is refused
unless you pass `--no-plugins`. See [`docs/plugin-strategy.md`](docs/plugin-strategy.md).

## Using viv as a drop-in composer

`make install-shim` installs a `composer` binary next to `viv` (`make
install` alone leaves your real Composer untouched). Put it on `PATH`
ahead of the real Composer (or symlink it as `composer` in CI) and it maps
`install`, `dump-autoload` and `normalize` with their supported flags to
`viv`, execing the real Composer for everything else (`update`, `require`,
plugins, unrecognised flags). Point `VIV_COMPOSER_PATH` at the real binary if
it isn't first on `PATH`.

`install` adopts a `vendor/` that Composer (or a pre-adopt viv) wrote: it has
`installed.json` but no `.vivace-state`, so viv relinks every package from
the store in place, no flag needed. The relink is safe and reversible either
way, since the lock decides the content. Through the shim, a terminal gets a
confirmation prompt first, because typing `composer install` didn't opt into
viv touching the tree in place; a plain `viv install`, or a script running
under the shim, proceeds unprompted. Automatic adoption is best-effort per
package: a dist it can't fetch (a private package behind an auth key, say)
keeps Composer's own copy in place with a warning instead of failing the
whole install. `--adopt` still force-relinks a viv-written `vendor/` on
request, with its own terminal prompt regardless of the shim, and still fails
hard on a dist it can't fetch.

## Scope

viv aims to replace Composer for the commands people run every day, not the
whole command reference. Covered, byte for byte where output is a file or
Composer's plain text: `install`, `update`, `update-lock`, `require`,
`remove`, `dump-autoload`, `normalize`, `show`, `tree`, `why`, `outdated`,
`audit`, `validate`, `run`, `exec`, `diagnose` (viv's own report, not
Composer's text), the lifecycle scripts, the cache
commands, and Satis or Private Packagist repositories. Beyond Composer:
`viv x vendor/tool` runs a Packagist tool without installing it into the
project, the way uvx does. Everything else
(`create-project`, `init`, `search`, `config`, `global`, `self-update`,
`licenses`, `depends` and friends) stays with Composer, and the
`composer` shim hands those through unchanged.

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

Out for now: other Composer plugins (refused, see the plugin strategy),
`--minimal-changes`, Composer's condensed multi-cause problem messages, and
update speed: it resolves correctly, but at about four times Composer's
time on a large lock,
tracked in [#90](https://github.com/svandragt/vivace/issues/90) and
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
