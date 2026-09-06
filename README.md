# vivace

`viv` installs PHP dependencies from `composer.lock` and produces a `vendor/`
directory that is byte for byte what Composer would write. Since 0.3 it also
resolves: `viv update`, `viv require` and `viv remove` write a `composer.lock`
that Composer accepts unchanged, using a port of Composer's own solver.

Status: proof of concept, v0.3. Linux and macOS, both tested in CI. Before
each release a [compatibility sweep](compat/README.md) byte-diffs `vendor/`
against Composer on pinned popular projects and a random Packagist sample.

## Numbers

Laravel-sized lock, 101 packages, same machine, three runs each. Details and
raw data in [`bench/results/`](bench/results/README.md).

| Tool | Cold | Warm cache | No-op |
|---|---|---|---|
| composer 2.10.2 | 8.28 s | 1.69 s | 0.50 s |
| riff 0.0.7 | 1.75 s | 0.26 s | 0.23 s |
| viv 0.3.0 | 2.2 s | 0.140 s | 0.007 s |

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
[`docs/composer-contract.md`](docs/composer-contract.md).

## Install

Prebuilt binaries for Linux (x86_64, aarch64, musl) and macOS (x86_64,
aarch64) are attached to each [release](https://github.com/svandragt/vivace/releases).

```sh
# download a tarball from the releases page, or:
cargo binstall vivace
# or build from source:
cargo install --git https://github.com/svandragt/vivace --tag v0.3.0 vivace
```

## Usage

```sh
cargo build --release
target/release/viv install            # in a project with composer.json and composer.lock
target/release/viv install --no-dev
target/release/viv install --dry-run  # show the plan, change nothing
target/release/viv install --link-mode copy
target/release/viv update                # full update, writes composer.lock
target/release/viv update psr/log -w     # partial update with dependencies
target/release/viv require psr/container # edits composer.json, updates the lock
target/release/viv remove psr/container
target/release/viv dump-autoload -o
target/release/viv normalize --check     # composer.json is also normalised on install
target/release/viv cache prune
```

`install` runs the root's lifecycle scripts (`pre-install-cmd`,
`post-autoload-dump`, `post-install-cmd`) like Composer; `--no-scripts` skips
them. Composer plugins cannot run natively: `composer/installers` and the
WordPress core installers are applied natively, anything else is refused
unless you pass `--no-plugins`. See [`docs/plugin-strategy.md`](docs/plugin-strategy.md).

## Using viv as a drop-in composer

`cargo build`/`make install` also builds a `composer` binary. Put it on `PATH`
ahead of the real Composer (or symlink it as `composer` in CI) and it maps
`install`, `dump-autoload` and `normalize` with their supported flags to
`viv`, execing the real Composer for everything else (`update`, `require`,
plugins, unrecognised flags). Point `VIV_COMPOSER_PATH` at the real binary if
it isn't first on `PATH`.

## Scope

In: zip and tar dists, path repositories, git sources without a dist,
credentials from `auth.json` and `COMPOSER_AUTH`, sha1 verification, PSR-4,
PSR-0, classmap and files autoloading, `--optimize-autoloader` and
`--classmap-authoritative`, `platform_check.php`, `vendor/bin` proxies,
lifecycle scripts, `composer/installers` paths, full and partial `update`,
`require`, `remove`, Packagist v2 metadata with a revalidating cache.

Out for now: other Composer plugins (refused, see the plugin strategy),
`preferred-install: source`, `--minimal-changes`, private repositories that
are not Packagist-compatible, and Composer's condensed multi-cause problem
messages.

## Development

Tooling comes from [devbox](https://www.jetify.com/devbox): PHP, Composer
and hyperfine for the fixtures and benchmarks.

```sh
make install    # put viv on your PATH (~/.cargo/bin)
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
