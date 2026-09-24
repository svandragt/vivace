# vivace

`viv` is a Rust reimplementation of Composer that installs from
`composer.lock` and writes the `vendor/` directory Composer would write,
byte for byte. On the [bench corpus](bench/results/corpus.md) (2026-09-15)
a cold `laravel/laravel` install takes 0.30 s against Composer's 1.58 s,
and the [compat sweep](compat/hunted.md) finds an identical `vendor/` on
every project viv installs. Plugins other than the shipped adapters and
Composer's long tail of commands stay with Composer;
[`docs/stability.md`](docs/stability.md) lists exactly what is and isn't
covered.

It began as a question, whether a person directing coding agents can build
a faster drop-in Composer, and that question is answered. The compatible
mode is finished and frozen as a control. viv continues as a research
vehicle for package-manager design, one measured chapter at a time;
[`docs/research.md`](docs/research.md) has the programme and the current
chapter.

## Try it

```sh
cargo binstall vivace          # or: brew install svandragt/tap/vivace
viv install     # in a project with composer.json and composer.lock
```

That also installs a `composer` shim next to `viv`; put it first on `PATH`
and your existing scripts run through viv unedited (see [Using viv as
composer](#using-viv-as-composer)).

Prebuilt binaries are on the [releases
page](https://github.com/svandragt/vivace/releases) (Linux x86_64 as glibc
and static musl builds, aarch64 as static musl, also packaged as a .deb;
macOS x86_64 and aarch64). To build from source instead:

```sh
cargo install vivace --locked
```

Both commands also upgrade an existing install: binstall only downloads when
the release is newer than the one installed; add `--force` to
`cargo install` when the version has not changed.

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

Use `apt remove vivace` if you installed the .deb instead. Nothing else is
written outside the project and the cache.

## Is it safe to try

viv's contract is that its output matches Composer's byte for byte. Before
every release, a compatibility sweep installs a mix of pinned popular
projects and a random sample of Packagist packages with both Composer and
viv, then compares the results.[^2] The v0.16.0 sweep: 20 of 20 pinned rows
identical, all 10 pinned projects resolve the same `composer.lock` as
Composer as well as installing the same `vendor/`; in the random sample 10
identical and 10 skipped where Composer itself failed.[^3]

One pinned project still needs `--no-plugins`, for a plugin viv refuses by
design rather than one it has yet to port.[^4] See [Plugins](#plugins)
below.

## Speed

10 projects from viv's compatibility corpus, 2026-09-15, from a local
mirror so no network is measured, AMD Ryzen 9 7900X3D (24 threads) on
ext4, viv 0.13.0, composer 2.10.2, riff 0.0.7, vivacity 0.6.0,
`--no-plugins --no-scripts` on every tool and `--no-fallback` on vivacity
so a run it hands to Composer can never count as its own, three runs each.
Each cell is the geometric mean of how many times faster viv is, with the
range across projects; below 1× viv is slower. Per-project numbers are in
[`bench/results/corpus.md`](bench/results/corpus.md), section
`2026-09-15T06:54:06Z`. riff's column measures the same job as viv and
Composer (checksums, `platform_check.php`, proxies, `installed.*`); the
cosmetic differences are listed in
[`bench/results/README.md`](bench/results/README.md). vivacity is the
closest comparison: it makes the same byte-identical promise. Its column
covers the 6 projects it installs; on the other 4 it exits rather than
install a lock naming a plugin outside its list, even with
`--no-plugins`.[^7]

| Scenario | viv vs Composer | viv vs riff | viv vs vivacity |
|---|---|---|---|
| Cold | 5.6× (2.8 to 11.1) | 2.5× (1.2 to 101.3)[^6] | 2.0× (1.8 to 2.8) |
| Warm | 17.8× (6.8 to 42.8) | 6.9× (2.7 to 149.8)[^6] | 2.0× (1.1 to 3.2) |
| No-op | 42.0× (19.0 to 105.5) | 13.9× (2.5 to 57.9) | 6.8× (3.0 to 15.2) |
| Update-warm | 1.9× (1.3 to 4.0) | n/a | 1.5× (1.1 to 1.8) |

The table has not been re-measured since 0.13.0; each release since is
gated against the previous one instead. v0.15.0 against v0.16.0 with
`make bench-ab` on laravel, symfony/demo and drupal: warm and no-op flat
or faster on all three (drupal warm 296 ms to 285 ms, laravel 41 ms to
39 ms, symfony 47 ms to 47 ms; no-op within 0.2 ms).

A warm update still revalidates every package's metadata with the registry,
one conditional request each, even when nothing changed. `--metadata-ttl
<seconds>` on `update`, `add` and `rm` (or `VIV_METADATA_TTL`; the flag wins)
skips that revalidation for a package whose cached metadata is younger than
the window, so a second update run shortly after the first makes no metadata
requests at all. It's off by default (`0`, always revalidate, matching
Composer), so turn it on only where a slightly stale registry view for a few
minutes is an acceptable trade for the extra speed. `--offline` always wins
over a configured window.

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
viv update --no-blocking  # allow versions with a security advisory, as Composer's flag does
viv add psr/container      # edits composer.json, updates the lock and installs (alias: require)
viv rm psr/container       # same, minus the package (--no-install opts out too, alias: remove)
viv dump-autoload -o
viv normalize --check     # add/rm/init also normalise when they write
viv cache prune
viv diagnose              # environment/config report to paste into a bug report
viv lock convert          # translate an existing composer.lock into viv.lock
viv lock merge base ours theirs   # git merge driver for composer.lock/viv.lock
viv workspace list        # list a workspace's members and their inter-requirements
viv update --lock native  # resolves normally, also writes viv.lock beside composer.lock (implied once viv.lock exists)
```

`install` runs your project's setup scripts, the same way Composer does,
unless you pass `--no-scripts`.[^10]

`add`, `rm` and `init` also tidy up `composer.json` when they write it, so
two branches that each add a dependency merge cleanly instead of fighting
over ordering. `install` and `update` never touch `composer.json`.[^11]

`viv init` writes `composer.lock merge=viv` (and `viv.lock merge=viv` once
the project has adopted that file) to `.gitattributes`; an existing project
adds the line once by hand instead. `viv install` then sets `git config
merge.viv.driver 'viv lock merge %O %A %B'` in each clone that has the
attribute, so every future `git merge` hands both branches' locks to viv,
which merges them record by record and re-solves what diverged; you only
see conflict markers when the merged `composer.json` cannot be satisfied. A
clone that merges before anyone has run `install` there yet has no driver
configured, so git leaves plain conflict markers instead — `install`
notices, resolves the lock straight from git's own index stages the same
way, and wires the clone so the next merge doesn't need to.

## Using viv as composer

`make install-shim` installs a `composer` binary next to `viv` (plain
`make install` leaves your real Composer untouched). Put it on `PATH`
ahead of the real Composer, or symlink it as `composer` in CI: everyday
`install`, `dump-autoload`, `normalize` and `create-project` run through
viv; every other command falls through to your real Composer install.[^12]

This is also the cheapest way to check whether a project migrates cleanly:
alias `composer` to the shim and run your existing scripts unedited.

In GitHub Actions, one step installs viv and puts the shim first on `PATH`, so an existing `composer install` step runs through viv unedited:

```yaml
- uses: svandragt/vivace/action@v0
- run: composer install --no-dev
```

The action downloads the release tarball for the runner's OS and architecture, checks it against the release's `SHA256SUMS`, and installs nothing else. Pin a release with `with: { version: v0.16.0 }`; set `shim: false` to get `viv` on `PATH` without the `composer` shim. Cache viv's store with `actions/cache` on `~/.cache/vivace`, keyed on `composer.lock`.

A command or flag the shim doesn't understand falls back to the real
Composer with a note on stderr naming what wasn't understood, so a migration
that quietly stopped using viv is visible instead of just slower. Set
`VIV_SHIM_STRICT=1` to make that fallback a hard error instead, for a CI job
that wants a red build rather than a silent return to Composer.

You can also point a script straight at `viv`. The CI idiom
`--prefer-dist --no-interaction --no-progress` already describes what viv
does, so `viv install` and `viv dump-autoload` accept those flags and
ignore them instead of failing.

## In a Dockerfile

To build a `vendor/` stage without a PHP runtime, use the published image
in place of `composer:2`:

```dockerfile
FROM ghcr.io/svandragt/vivace:0 AS vendor
COPY composer.json composer.lock ./
RUN ["viv", "install", "--no-dev"]

FROM php:8.4-fpm
COPY --from=vendor /app/vendor /app/vendor
```

Two things differ from the `composer:2` stage it replaces:

- Write `RUN` in exec form, as above. The image has no shell, so the
  familiar `RUN viv install --no-dev` does not work.
- The image runs `viv` by default and ships the `composer` shim beside it,
  so `RUN ["composer", "install", "--no-dev"]` works too if you would
  rather not edit the command.
- The image carries no real Composer to fall back to, so a command or flag
  the shim doesn't understand hard-errors there instead of silently running
  Composer, the way it would on a machine that still has Composer installed.

Tags are `:0.16`, `:0.16.0` and `:0`. There is no `:latest`: a moving tag
that silently resolves to nothing breaks scripted installs, which is the
mistake that kept `releases/latest` returning 404 for ten releases.

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

Besides matching Composer's output faster, viv does four things Composer
does not.

### Merging composer.lock without conflicts

Two branches that each ran `update` conflict in `composer.lock` almost
every time, because git merges it line by line and the file is one large
JSON array. viv merges it as a git merge driver, record by record, and
re-solves only the packages the two branches changed differently. `viv
init` writes the `.gitattributes` line and `viv install` wires the driver
into each clone, so after the first install nobody runs anything extra.
Replayed over 355 real merges from four client projects, `composer.lock`
conflicted 228 times under git and 51 times with the driver; the rest
were branch heads the registry had since overwritten or packages it no
longer lists, which viv marks and names.[^19] The setup is described under
"Everyday commands".

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

- **It stays 0.x.** No 1.0 is planned. Minor releases can change behaviour
  and flags; the release notes and `JOURNAL.md` call those out. The output contract
  (`vendor/` and `composer.lock` identical to Composer's) is the one thing
  that does not move.[^17]
- **Windows is not supported.** Linux and macOS only.
- **Maintenance is on demand.** The compatible mode is complete and no new
  plugin adapters or Composer commands are planned. A bug in it that a real
  project hits gets fixed; open an issue with the project's `composer.json`
  and lock. Releases continue as research chapters land, and the compat
  sweep and benchmark gates run on every change so the drop-in behaviour
  does not regress.
- **Some Composer plugins stop the install.** symfony/flex and any plugin
  without a native adapter make viv exit with an error naming the plugin.
  `--no-plugins` installs as Composer would without them, but the plugin's
  work is not done. Of the 10 pinned test projects, 1 is in this position:
  `symfony/demo`, for symfony/flex, which viv refuses by design.
- **The shim needs a real Composer for everything else.** It maps
  `install`, `dump-autoload`, `normalize`, `create-project`, `update`,
  `require` and `remove` to viv; `search` and the rest go to the Composer on
  your `PATH`, and with no real Composer installed they fail.
- **`vendor/` files are read-only by default.** viv hardlinks them from a
  shared store, so an edit inside `vendor/` fails instead of changing every
  project on the machine. If you patch vendor files by hand, install with
  `--link-mode copy`, or `--link-mode clone` for writable files sharing the
  store's disk space where the filesystem supports it.
- **Two `update` gaps.** `--ignore-platform-reqs` on `update`, `require`
  and `remove` affects the autoload write, not the solve, so a package
  pinned to a PHP this interpreter lacks still fails to resolve
  ([#242](https://github.com/svandragt/vivace/issues/242)). And a version
  filtered out by `minimum-stability` is still reported as not found rather
  than as filtered; Composer names the cause.

## Scope

viv covers the Composer commands you run every day. Everything else stays
with Composer, and the `composer` shim passes those commands through.

Commands viv runs itself, with the same output as Composer:

- Start and resolve: `init`, `new` (`create-project`), `install`, `update`,
  `update-lock`, `add` (`require`), `rm` (`remove`), `dump-autoload`,
  `normalize`.
- Inspect: `show`, `tree`, `why`, `outdated`, `audit`, `validate`.
- Maintain: `lock` (convert, merge), `workspace` (list).
- Run: `run`, `exec`, the lifecycle scripts, and the cache commands.
- `diagnose` prints viv's own report, not Composer's.

Commands that stay with Composer: `search`, `config`, `global`,
`self-update`, `licenses`, `depends` and the rest.

### Works with

- Repositories: Packagist, Private Packagist and Satis, including a local
  `file://` mirror, `path`, `vcs`, git, GitHub and `package` (inline
  declarations) sources; redirects are followed.
- Packages: zip and tar dists, a git checkout when there is no dist,
  `preferred-install: source`, sha1 checks, credentials from `auth.json`
  and `COMPOSER_AUTH`.
- Autoload: PSR-4, PSR-0, classmap and files; `--optimize-autoloader` and
  `--classmap-authoritative`; `platform_check.php`; `vendor/bin` proxies;
  lifecycle scripts.
- Commands: `install`, full and partial `update` including
  `--minimal-changes` and Composer's default blocking of versions with a
  security advisory (`--no-blocking` to allow them), `add`, `rm`,
  `dump-autoload`, and offline mode with `--offline` or
  `COMPOSER_DISABLE_NETWORK`.
- Plugins: the ones with native adapters, listed under [Plugins](#plugins).

## Development

Tooling comes from [devbox](https://www.jetify.com/devbox): PHP, Composer
and hyperfine for the fixtures and benchmarks.

```sh
make install    # put viv on your PATH (~/.cargo/bin); make install-shim adds the composer drop-in
make check      # fmt, clippy, tests, cargo deny, cargo machete, cargo doc
make test
make bench      # composer vs riff vs viv on bench/laravel
VIVACE_TEST_NETWORK=1 make test   # includes the end-to-end install
```

To report a bug or ask a question, see the [Support](https://vivace.vandragt.com/support.html) page and [`SECURITY.md`](SECURITY.md) for anything security-related.

There's an engineering log of what was measured and decided along the
way, including the study of prior art that shaped viv's design.[^15]

## Licence

GPL-3.0-or-later. viv ports three Composer plugins whose own source is
GPL-2.0-or-later, so the binary is a derivative work of them and carries
their licence.[^18] A few files are vendored from MIT-licensed projects and
keep their original copyright notices; MIT permits their use here.[^16]
[`NOTICE.md`](NOTICE.md) records every port, its upstream and its licence.

[^1]: viv relinks every package from its own content-addressed store into `vendor/`, using hardlinks so files aren't copied or re-extracted.
[^2]: See [`compat/README.md`](compat/README.md) for how the sweep works.
[^3]: The skips are packages Composer itself refuses to resolve — security advisories blocking every matching version, a `dev-master`-only package under the default `minimum-stability`, a dependency whose only versions require a framework the root cannot take — not something viv got wrong. Full results, including which projects and what was skipped, are in [`compat/results/v0.16.0.md`](compat/results/v0.16.0.md).
[^4]: Of viv's 10 pinned compatibility projects, the one that needs `--no-plugins` is `symfony/demo`, for `symfony/flex`. Flex does its work in `composer require`, so installing from a committed lock loses nothing; see [`docs/plugin-strategy.md`](docs/plugin-strategy.md).
[^6]: riff's phpunit/phpunit cold and warm times (7.4 s and 7.3 s) are an outlier against its other rows in this corpus; kept in the range, not dropped; see [`bench/results/corpus.md`](bench/results/corpus.md).
[^7]: roots/bedrock, drupal/recommended-project, yiisoft/yii2-app-basic and craftcms/craft, recorded in [`bench/skips.txt`](bench/skips.txt) against vivacity 0.6.0 so a newer release is retried. A refusal is an `n/a` cell, never a slow one.
[^10]: `install` runs the root's lifecycle scripts (`pre-install-cmd`, `post-autoload-dump`, `post-install-cmd`) as Composer does.
[^11]: Normalisation follows `ergebnis/composer-normalize`'s rules, so a project already using that plugin sees no change.
[^12]: The shim maps `install`, `dump-autoload`, `normalize`, `create-project`, `update`, `require` and `remove` with their supported flags (`update`'s partial-update package arguments and `-w`/`-W` included) to `viv`; everything else, `search`, an unrecognised flag, it hands through to the real Composer binary unchanged. Point `VIV_COMPOSER_PATH` at the real binary if it isn't first on `PATH`. The shim only has something to hand through to if a real Composer is on `PATH` in the first place: if there isn't one, those commands fail rather than silently falling back to viv.
[^13]: [`docs/plugin-strategy.md`](docs/plugin-strategy.md) lists which plugin falls into which category.
[^14]: The store lives under `$XDG_CACHE_HOME/vivace/`, keyed by the sha256 hash of each package archive, and installing hardlinks files from it into `vendor/` instead of extracting them again. Store files are read-only, so an accidental edit to a vendor file fails instead of silently changing every project that shares that file on disk; projects that patch their vendor files should use `--link-mode copy` instead. A no-op install compares the lock against `vendor/composer/installed.json` and a small state file, without spawning PHP or making a network request. The autoloader files (`vendor/autoload.php`, `vendor/composer/*.php`, `installed.json`, `installed.php`, `platform_check.php`) are generated by a port of Composer's own generator, tested against Composer's own golden test cases, and checked end to end by byte-diffing `vendor/` against real Composer 2.10.2 output. See [`ARCHITECTURE.md`](ARCHITECTURE.md), [`docs/composer-contract.md`](docs/composer-contract.md) and [`docs/stability.md`](docs/stability.md) for the detail.
[^15]: [`JOURNAL.md`](JOURNAL.md), including the study of [Riff](https://github.com/shyim/riff) and [Presto](https://github.com/paramientos/presto) that preceded the code.
[^16]: `src/autoload/templates/` contains Composer's `ClassLoader.php`, `InstalledVersions.php` and licence, copied verbatim under Composer's MIT licence. `src/spdx-licenses.json` is `composer/spdx-licenses`' own resource file, also copied verbatim under its MIT licence. Test fixtures under `tests/fixtures/composer/` are Composer's own, also MIT.
[^17]: [`docs/stability.md`](docs/stability.md) states what a minor release may and may not change.
[^18]: `drupal/core-composer-scaffold`, `johnpbloch/wordpress-core-installer` and `roots/wordpress-core-installer` are all GPL-2.0-or-later. Their `or later` term is what allows GPL-3.0 here. See [#245](https://github.com/svandragt/vivace/issues/245) for the provenance of each port.
[^19]: The replay and its counts are in [`bench/results/lockmerge.md`](bench/results/lockmerge.md); the design and the remaining cases are chapter 1 of [`docs/research.md`](docs/research.md). Client projects are anonymised.
