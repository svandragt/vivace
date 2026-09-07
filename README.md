# vivace

`viv` installs PHP dependencies from `composer.lock` and produces a `vendor/`
directory that is byte for byte what Composer would write. Since 0.3 it also
resolves: `viv update`, `viv require` and `viv remove` write a `composer.lock`
that Composer accepts unchanged, using a port of Composer's own solver.

Status: proof of concept, v0.5. Linux and macOS, both tested in CI. Before
each release a [compatibility sweep](compat/README.md) byte-diffs `vendor/`
against Composer on pinned popular projects and a random Packagist sample.

## Numbers

Laravel-sized lock, 101 packages, same machine, three runs each. Details and
raw data in [`bench/results/`](bench/results/README.md).

| Tool | Cold | Warm cache | No-op | Update, warm metadata |
|---|---|---|---|---|
| composer 2.10.2 | 7.32 s | 1.05 s | 0.48 s | 1.16 s |
| riff 0.0.7 | 1.62 s | 0.25 s | 0.23 s | n/a\* |
| viv 0.5.0 | 2.21 s | 0.042 s | 0.007 s | 5.11 s |

\* riff 0.0.7 can't resolve this lock's `update`: see
[`bench/results/README.md`](bench/results/README.md#update-warm-metadata).

The update column is the one viv still loses to Composer: the lock it
writes is identical to Composer's, and 0.5 halved the time by pruning the
pool and compiling constraints once, but two hotspots remain
([#89](https://github.com/svandragt/vivace/issues/89),
[#90](https://github.com/svandragt/vivace/issues/90)). Tracked per release
like the install numbers; 0.6's target is to beat Composer here too.

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
cargo install --git https://github.com/svandragt/vivace --tag v0.5.0 vivace
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

## Scope

viv aims to replace Composer for the commands people run every day, not the
whole command reference. Covered, byte for byte where output is a file or
Composer's plain text: `install`, `update`, `update-lock`, `require`,
`remove`, `dump-autoload`, `normalize`, `show`, `tree`, `why`, `outdated`,
`audit`, `validate`, `run`, `exec`, the lifecycle scripts, the cache
commands, and Satis or Private Packagist repositories. Beyond Composer:
`viv x vendor/tool` runs a Packagist tool without installing it into the
project, the way uvx does. Everything else
(`create-project`, `init`, `search`, `config`, `global`, `self-update`,
`diagnose`, `licenses`, `depends` and friends) stays with Composer, and the
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

Out for now: other Composer plugins (refused, see the plugin strategy),
`--minimal-changes`, private repositories that are not Packagist-compatible,
Composer's condensed multi-cause problem messages, and update speed: it
resolves correctly, but at about four times Composer's time on a large lock,
tracked in [#89](https://github.com/svandragt/vivace/issues/89) and
[#90](https://github.com/svandragt/vivace/issues/90).

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
licence. Test fixtures under `tests/fixtures/composer/` are Composer's own,
also MIT.
