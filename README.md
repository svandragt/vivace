# vivace

`viv install` installs PHP dependencies from an existing `composer.lock` and
produces a `vendor/` directory that is byte for byte what Composer would
write. It is to Composer what uv's `pip sync` was to pip: no dependency
solving yet, all the speed comes from how packages are stored and linked.

Status: proof of concept, v0.1. Linux and macOS (both tested in CI). Not a Composer replacement for
`update`, `require`, plugins or scripts.

## Numbers

Laravel-sized lock, 101 packages, same machine, three runs each. Details and
raw data in [`bench/results/`](bench/results/README.md).

| Tool | Cold | Warm cache | No-op |
|---|---|---|---|
| composer 2.10.2 | 8.28 s | 1.69 s | 0.50 s |
| riff 0.0.7 | 1.75 s | 0.26 s | 0.23 s |
| viv 0.1.0 | 2.22 s | 0.146 s | 0.010 s |

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

## Usage

```sh
cargo build --release
target/release/viv install            # in a project with composer.json and composer.lock
target/release/viv install --no-dev
target/release/viv install --dry-run  # show the plan, change nothing
target/release/viv install --link-mode copy
```

`update`, `require`, `remove` and `dump-autoload` exit with a message
pointing at Composer.

## Using viv as a drop-in composer

`cargo build`/`make install` also builds a `composer` binary. Put it on `PATH`
ahead of the real Composer (or symlink it as `composer` in CI) and it maps
`install`, `dump-autoload` and `normalize` with their supported flags to
`viv`, execing the real Composer for everything else (`update`, `require`,
plugins, unrecognised flags). Point `VIV_COMPOSER_PATH` at the real binary if
it isn't first on `PATH`.

## Scope

In: zip dists from any URL the lock names, credentials from `auth.json` and
`COMPOSER_AUTH`, sha1 verification when the lock carries a checksum, PSR-4,
PSR-0, classmap and files autoloading, root `autoload` and `autoload-dev`,
`platform_check.php`, `vendor/bin` proxies, `--no-dev`.

Out for now: dependency resolution, plugins, scripts, git and path
repositories, tar dists, `--optimize-autoloader`.

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
