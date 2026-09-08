# vivace

`viv` installs PHP dependencies from a Composer `composer.lock` file and
writes a `vendor/` directory that matches what Composer would write, byte
for byte. It's a proof of concept at v0.7.0, tested in CI on Linux and
macOS, and not yet at 1.0.

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

If Composer already wrote the `vendor/` directory, viv adopts it
automatically, no flag needed.[^1] Only `composer install` run through the
shim asks for confirmation first, and only in a terminal. If a package
can't be downloaded (a private package behind a licence key, for example),
viv keeps Composer's copy of that package, prints a warning, and adopts
the rest. To force a fresh relink of a `vendor/` that viv itself wrote,
run `viv install --adopt`.

## Is it safe to try

viv's contract is that its output matches Composer's byte for byte. Before
every release, a compatibility sweep installs a mix of pinned popular
projects and a random sample of Packagist packages with both Composer and
viv, then compares the results.[^2] The v0.7.0 sweep: 36 rows identical, 0
differ, 4 skipped.[^3]

Two of the pinned projects still need `--no-plugins` to install with
viv, both for a Composer plugin viv doesn't yet support.[^4] See
[Plugins](#plugins) below.

## Speed

Laravel-sized lock, 101 packages, same machine, three runs each.[^5]

| Tool | Cold | Warm cache | No-op | Update, warm metadata |
|---|---|---|---|---|
| composer 2.10.2 | 7.28 s | 1.05 s | 0.47 s | 1.23 s |
| riff 0.0.7 | 1.74 s | 0.24 s | 0.24 s | n/a[^6] |
| viv 0.6.0 | 2.26 s | 0.043 s | 0.008 s | 1.12 s |

Update is close to Composer's speed, and cold install is the one column
riff still wins.[^7]

### Public corpus

The pinned projects from viv's own compatibility corpus, same machine,
warm metadata cache.[^8]

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

Warm install and no-op beat both tools by an order of magnitude on every
project; cold install beats Composer everywhere and is level with
riff.[^9]

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

`install` runs your project's setup scripts, the same way Composer does,
unless you pass `--no-scripts`.[^10]

`update`, `require` and `remove` also tidy up `composer.json` when they
write it, so two branches that each add a dependency merge cleanly
instead of fighting over ordering. `install` never touches
`composer.json`.[^11]

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

Every command that writes `composer.json` (`update`, `require`, `remove`)
also normalises it: stable key order and whitespace, the same result as
running `composer normalize`. You never commit a diff that is only
reordering.[^11] Pass `--no-normalize` to opt out, or run
`viv normalize --check` to report without writing.

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
  `--link-mode copy`.
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
[^3]: The skips are pre-existing failures on Composer's side, such as an expired auth token — not something viv got wrong. Full results, including which projects and what was skipped, are in [`compat/results/v0.7.0.md`](compat/results/v0.7.0.md).
[^4]: Of viv's 19 pinned compatibility projects, the two that need `--no-plugins` are `symfony/demo` and `roots/bedrock`.
[^5]: Details and raw data are in [`bench/results/README.md`](bench/results/README.md). This table was last measured on the 0.6.0 release; the figures are still that run.
[^6]: riff 0.0.7 can't resolve this lock's `update`: see [`bench/results/README.md`](bench/results/README.md#update-warm-metadata).
[^7]: The pool builder loads only the versions the accumulated constraints allow, as Composer's does: 108 metadata requests instead of 251, and a pool of 615 packages instead of 5169 on this lock, with the written lock still identical to Composer's. Cold install is bounded by GitHub's zipball throttling rather than by viv's own speed; see [`bench/results/profile.md`](bench/results/profile.md).
[^8]: From [`compat/corpus.toml`](compat/corpus.toml), run with `--no-plugins --no-scripts`, three runs each. Full columns and footnotes are in [`bench/results/corpus.md`](bench/results/corpus.md). The "viv" column is `update`, warm metadata — the same scenario as the speed table's last column.
[^9]: `yiisoft/yii2-app-basic` is in the corpus but not in the table: riff fails its install by applying a dependency's patches under `--no-plugins`, where Composer and viv install it fine.
[^10]: `install` runs the root's lifecycle scripts (`pre-install-cmd`, `post-autoload-dump`, `post-install-cmd`) as Composer does.
[^11]: Normalisation follows `ergebnis/composer-normalize`'s rules, so a project already using that plugin sees no change.
[^12]: The shim maps `install`, `dump-autoload` and `normalize` with their supported flags to `viv`; everything else — `update`, `require`, plugins, an unrecognised flag — it hands through to the real Composer binary unchanged. Point `VIV_COMPOSER_PATH` at the real binary if it isn't first on `PATH`. The shim only has something to hand through to if a real Composer is on `PATH` in the first place: if there isn't one, those commands fail rather than silently falling back to viv.
[^13]: [`docs/plugin-strategy.md`](docs/plugin-strategy.md) lists which plugin falls into which category.
[^14]: The store lives under `$XDG_CACHE_HOME/vivace/`, keyed by the sha256 hash of each package archive, and installing hardlinks files from it into `vendor/` instead of extracting them again. Store files are read-only, so an accidental edit to a vendor file fails instead of silently changing every project that shares that file on disk; projects that patch their vendor files should use `--link-mode copy` instead. A no-op install compares the lock against `vendor/composer/installed.json` and a small state file, without spawning PHP or making a network request. The autoloader files (`vendor/autoload.php`, `vendor/composer/*.php`, `installed.json`, `installed.php`, `platform_check.php`) are generated by a port of Composer's own generator, tested against Composer's own golden test cases, and checked end to end by byte-diffing `vendor/` against real Composer 2.10.2 output. See [`ARCHITECTURE.md`](ARCHITECTURE.md), [`docs/composer-contract.md`](docs/composer-contract.md) and [`docs/stability.md`](docs/stability.md) for the detail.
[^15]: [`JOURNAL.md`](JOURNAL.md), including the study of [Riff](https://github.com/shyim/riff) and [Presto](https://github.com/paramientos/presto) that preceded the code.
[^16]: `src/autoload/templates/` contains Composer's `ClassLoader.php`, `InstalledVersions.php` and licence, copied verbatim under Composer's MIT licence. `src/spdx-licenses.json` is `composer/spdx-licenses`' own resource file, also copied verbatim under its MIT licence. Test fixtures under `tests/fixtures/composer/` are Composer's own, also MIT.
[^17]: [`docs/stability.md`](docs/stability.md) states what a minor release may and may not change.
