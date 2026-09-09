# vivace

`viv` installs PHP dependencies from a Composer `composer.lock` file and
writes a `vendor/` directory that matches what Composer would write, byte
for byte. It's a proof of concept at v0.8.0, tested in CI on Linux and
macOS, and not yet at 1.0.

## Try it

Download a prebuilt binary from the [releases
page](https://github.com/svandragt/vivace/releases) (Linux x86_64, aarch64
and musl; macOS x86_64 and aarch64), or install it another way:

```sh
cargo binstall vivace
# or build from source:
cargo install --git https://github.com/svandragt/vivace --tag v0.8.0 vivace
```

Then run it in a project that already has a `composer.json` and
`composer.lock`:

```sh
viv install
```

If Composer already wrote the `vendor/` directory, viv adopts it
automatically, no flag needed.[^1] Only `composer install` run through the
shim asks for confirmation first, and only in a terminal. If a package
can't be downloaded (a private package behind a licence key, for example),
viv keeps Composer's copy of that package, prints a warning, and adopts
the rest. To force a fresh relink of a `vendor/` that viv itself wrote,
run `viv install --adopt`.

### Starting from nothing

No `composer.json` yet? `viv init` writes one and stops, with no prompts:
the package name is guessed from `git config user.name` and the directory,
`type` is `project`, `license` is `MIT`, and `autoload.psr-4` points at
`src/` when that directory exists. Pass `--name`, `--license` or `--type` to
override a default, or `--require`/`--require-dev` to add dependencies in
the same command:

```sh
mkdir demo && cd demo && viv init --require psr/log
```

That resolves `psr/log`, writes `composer.lock`, and installs `vendor/`,
the same as `viv add` would on an existing project (`--no-install` opts
out). Run it again with `--force` to start over.

`viv init` is for the directory you're already in; `viv new` is for one
that doesn't exist yet. A bare name creates it and runs `init`'s own
defaults inside:

```sh
viv new demo
```

`vendor/package[:constraint]` downloads that package's dist as a project
skeleton (constraint defaults to the newest stable version), drops its own
VCS metadata, and installs it, running the `post-root-package-install`/
`post-create-project-cmd` scripts a skeleton like Laravel's relies on
(`--no-scripts` opts out):

```sh
viv new laravel/laravel:^11 my-app
```

`create-project` is Composer's own name for this, kept as an alias.

## Stopping

You can stop using viv at any point and go back to Composer with no
clean-up. A `vendor/` that viv wrote is a valid Composer install:
`installed.json` and the autoload files are the same bytes Composer would
have written, so `composer install` on it is a no-op and `composer update`
replaces packages as usual. The only extra file is a small state file in
`vendor/composer/`, which Composer ignores.

The links from `vendor/` into viv's store are hardlinks, not symlinks: each
file in `vendor/` is a real file that shares its data with the store copy,
so deleting the store (`viv cache clean`) leaves `vendor/` complete and
working. If you would rather reinstall it with Composer anyway:

```sh
rm -rf vendor && composer install
```

To remove viv itself:

```sh
viv cache clean            # deletes viv's store under ~/.cache/vivace
rm ~/.cargo/bin/viv ~/.cargo/bin/composer   # the binary and the shim
```

Use `apt remove vivace` or `brew uninstall vivace` if you installed a
package instead. Nothing else is written outside the project and the cache.

## Is it safe to try

viv's contract is that its output matches Composer's byte for byte. Before
every release, a compatibility sweep installs a mix of pinned popular
projects and a random sample of Packagist packages with both Composer and
viv, then compares the results.[^2] The v0.8.0 sweep: 36 rows identical, 0
differ, 4 skipped.[^3]

Two of the pinned projects still need `--no-plugins` to install with
viv, both for a Composer plugin viv doesn't yet support.[^4] See
[Plugins](#plugins) below.

## Speed

10 projects from viv's compatibility corpus, 2026-09-09, from a local
mirror so no network is measured, AMD Ryzen 9 7900X3D (24 threads) on
ext4, viv 0.7.0, composer 2.10.2, riff 0.0.7, `--no-plugins --no-scripts`,
three runs each. Each cell is how many times faster viv is, with the range across projects; below 1× viv is slower. Per-project numbers are in
[`bench/results/corpus.md`](bench/results/corpus.md). riff's column
measures the same job as viv and Composer (checksums, `platform_check.php`,
proxies, `installed.*`); the cosmetic differences are listed in
[`bench/results/README.md`](bench/results/README.md).

| Scenario | viv vs Composer | viv vs riff |
|---|---|---|
| Cold | 4.5× (2.4 to 12.1) | 1.8× (0.6 to 64.4)[^5][^6] |
| Warm | 16.9× (8.1 to 41.2) | 6.7× (2.0 to 134.4)[^6] |
| No-op | 43.8× (19.7 to 99.8) | 20.1× (2.8 to 115.8) |
| Update-warm | 0.7× (0.5 to 1.6)[^5] | n/a |

## Everyday commands

```sh
viv init                  # write a composer.json for a new project and stop
viv new demo               # same, in a directory that doesn't exist yet
viv new laravel/laravel:^11 my-app   # download a package skeleton and install it
viv install            # in a project with composer.json and composer.lock
viv install --no-dev
viv install --dry-run  # show the plan, change nothing
viv install --link-mode copy
viv install --no-progress  # skip the fetch/link progress line on a terminal
viv update                # resolves, writes composer.lock and installs
viv update psr/log -w     # partial update with dependencies, then installs
viv update --no-install   # resolve and write the lock only
viv add psr/container      # edits composer.json, updates the lock and installs (alias: require)
viv rm psr/container       # same, minus the package (--no-install opts out too, alias: remove)
viv dump-autoload -o
viv normalize --check     # add/rm/init also normalise when they write
viv cache prune
viv diagnose              # environment/config report to paste into a bug report
```

`install` runs your project's setup scripts, the same way Composer does,
unless you pass `--no-scripts`.[^10]

`add`, `rm` and `init` also tidy up `composer.json` when they write it, so
two branches that each add a dependency merge cleanly instead of fighting
over ordering. `install` and `update` never touch `composer.json`.[^11]

## Using viv as composer

`make install-shim` installs a `composer` binary next to `viv` (plain
`make install` leaves your real Composer untouched). Put it on `PATH`
ahead of the real Composer, or symlink it as `composer` in CI: everyday
commands run through viv, and anything viv doesn't cover falls through to
your real Composer install.[^12]

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

The full list of which plugin falls into which category is documented
separately.[^13]

## Reasons to use viv

Besides matching Composer's output faster, viv does three things Composer
does not.

### Running tools without installing them

`viv x vendor/package[:constraint]` installs the package into an isolated,
cached environment and runs its binary, the way `uvx` and `npx` do. Nothing
is added to your `composer.json` or `vendor/`:

```sh
viv x phpunit/phpunit:^11 tests      # 27 packages, 0.07 s on a warm cache
viv x friendsofphp/php-cs-fixer fix src
viv x --list                         # environments in the cache
```

### Automatic normalisation

Every command that writes `composer.json` (`add`, `rm`, `init`) also
normalises it: stable key order and whitespace, the same result as running
`composer normalize`. You never commit a diff that is only reordering.[^11]
Run `viv normalize --check` for an explicit run that only reports without
writing.

### One cache for every project

viv keeps every package it downloads in one store and links from there, so
a second project with the same dependencies installs in milliseconds and
without touching the network.[^14]

## Reasons not to use viv

- **It is pre-1.0.** Minor releases can change behaviour and flags; the
  release notes and `JOURNAL.md` call those out. The output contract
  (`vendor/` and `composer.lock` identical to Composer's) is the one thing
  that does not move.[^17]
- **Windows is not supported.** Linux and macOS only.
- **Some Composer plugins stop the install.** symfony/flex and any plugin
  without a native adapter make viv exit with an error naming the plugin.
  `--no-plugins` installs as Composer would without them, but the plugin's
  work is not done. Of the 19 pinned test projects, 2 are in this position:
  `symfony/demo` and `roots/bedrock`.
- **The shim needs a real Composer for everything else.** Commands viv does
  not cover, such as `create-project` or `search`, are passed to the
  Composer on your `PATH`. With no real Composer installed they fail.
- **`vendor/` files are read-only by default.** viv hardlinks them from a
  shared store, so an edit inside `vendor/` fails instead of changing every
  project on the machine. If you patch vendor files by hand, install with
  `--link-mode copy`, or `--link-mode clone` for writable files sharing the
  store's disk space where the filesystem supports it.
- **Cold installs are not the fastest available.** riff wins that column;
  viv's cold time is bounded by GitHub's download throttling. Warm and no-op
  installs are where viv is far ahead.
- **One error message still differs.** Composer's "found X but it conflicts
  with your root require" wording is not ported yet.

## Scope

viv covers the Composer commands you run every day. Everything else stays
with Composer, and the `composer` shim passes those commands through.

Commands viv runs itself, with the same output as Composer:

- Install and resolve: `install`, `update`, `update-lock`, `require`,
  `remove`, `dump-autoload`, `normalize`.
- Inspect: `show`, `tree`, `why`, `outdated`, `audit`, `validate`.
- Run: `run`, `exec`, the lifecycle scripts, and the cache commands.
- `diagnose` prints viv's own report, not Composer's.

Commands that stay with Composer: `create-project`, `init`, `search`,
`config`, `global`, `self-update`, `licenses`, `depends` and the rest.

### Works with

- Repositories: Packagist, Private Packagist and Satis, `path`, `vcs`, git
  and GitHub sources.
- Packages: zip and tar dists, a git checkout when there is no dist,
  `preferred-install: source`, sha1 checks, credentials from `auth.json`
  and `COMPOSER_AUTH`.
- Autoload: PSR-4, PSR-0, classmap and files; `--optimize-autoloader` and
  `--classmap-authoritative`; `platform_check.php`; `vendor/bin` proxies;
  lifecycle scripts.
- Commands: `install`, full and partial `update` including
  `--minimal-changes`, `require`, `remove`, `dump-autoload`, and offline
  mode with `--offline` or `COMPOSER_DISABLE_NETWORK`.
- Plugins: the ones with native adapters, listed under [Plugins](#plugins).

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

There's an engineering log of what was measured and decided along the
way, including the study of prior art that shaped viv's design.[^15]

## Licence

MIT. A few files are vendored from other MIT-licensed projects and keep
their original copyright notices.[^16]

[^1]: viv relinks every package from its own content-addressed store into `vendor/`, using hardlinks so files aren't copied or re-extracted.
[^2]: See [`compat/README.md`](compat/README.md) for how the sweep works.
[^3]: The skips are pre-existing failures on Composer's side, such as an expired auth token — not something viv got wrong. Full results, including which projects and what was skipped, are in [`compat/results/v0.8.0.md`](compat/results/v0.8.0.md).
[^4]: Of viv's 19 pinned compatibility projects, the two that need `--no-plugins` are `symfony/demo` and `roots/bedrock`.
[^5]: The ratio's range crosses 1×: within noise on some projects.
[^6]: riff's phpunit/phpunit cold and warm times (7.7 s and 8.5 s) are an outlier against its other rows in this corpus; kept in the range, not dropped; see [`bench/results/corpus.md`](bench/results/corpus.md).
[^10]: `install` runs the root's lifecycle scripts (`pre-install-cmd`, `post-autoload-dump`, `post-install-cmd`) as Composer does.
[^11]: Normalisation follows `ergebnis/composer-normalize`'s rules, so a project already using that plugin sees no change.
[^12]: The shim maps `install`, `dump-autoload` and `normalize` with their supported flags to `viv`; everything else — `update`, `require`, plugins, an unrecognised flag — it hands through to the real Composer binary unchanged. Point `VIV_COMPOSER_PATH` at the real binary if it isn't first on `PATH`. The shim only has something to hand through to if a real Composer is on `PATH` in the first place: if there isn't one, those commands fail rather than silently falling back to viv.
[^13]: [`docs/plugin-strategy.md`](docs/plugin-strategy.md) lists which plugin falls into which category.
[^14]: The store lives under `$XDG_CACHE_HOME/vivace/`, keyed by the sha256 hash of each package archive, and installing hardlinks files from it into `vendor/` instead of extracting them again. Store files are read-only, so an accidental edit to a vendor file fails instead of silently changing every project that shares that file on disk; projects that patch their vendor files should use `--link-mode copy` instead. A no-op install compares the lock against `vendor/composer/installed.json` and a small state file, without spawning PHP or making a network request. The autoloader files (`vendor/autoload.php`, `vendor/composer/*.php`, `installed.json`, `installed.php`, `platform_check.php`) are generated by a port of Composer's own generator, tested against Composer's own golden test cases, and checked end to end by byte-diffing `vendor/` against real Composer 2.10.2 output. See [`ARCHITECTURE.md`](ARCHITECTURE.md), [`docs/composer-contract.md`](docs/composer-contract.md) and [`docs/stability.md`](docs/stability.md) for the detail.
[^15]: [`JOURNAL.md`](JOURNAL.md), including the study of [Riff](https://github.com/shyim/riff) and [Presto](https://github.com/paramientos/presto) that preceded the code.
[^16]: `src/autoload/templates/` contains Composer's `ClassLoader.php`, `InstalledVersions.php` and licence, copied verbatim under Composer's MIT licence. `src/spdx-licenses.json` is `composer/spdx-licenses`' own resource file, also copied verbatim under its MIT licence. Test fixtures under `tests/fixtures/composer/` are Composer's own, also MIT.
[^17]: [`docs/stability.md`](docs/stability.md) states what a minor release may and may not change.
