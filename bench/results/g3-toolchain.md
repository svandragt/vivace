# Candidate 3.1: the PHP toolchain across the corpus (#329)

Generation 3, candidate 3.1 (`docs/research.md`): how often does installing
a project depend on a PHP version or extension set the machine does not
have, and how much does a managed, per-project PHP remove? A measurement
first, nothing built yet. Method: `bench/g3-toolchain/static.py` (Part A)
and `bench/g3-toolchain/setup-time.py` (Part B). Run 2026-09-27.

## Baseline, measured

`docker run ubuntu:24.04`, `apt-get install -y php-cli`:

- Version: **PHP 8.3.6** (cli), package `php8.3-cli 2:8.3+93ubuntu2`.
- `php -m`: calendar, ctype, date, exif, FFI, fileinfo, filter, ftp,
  gettext, hash, iconv, json, libxml, openssl, pcntl, pcre, PDO, Phar,
  posix, random, readline, Reflection, session, shmop, sockets, sodium,
  SPL, standard, sysvmsg, sysvsem, sysvshm, tokenizer, Zend OPcache, zlib.
  Notably absent: curl, dom/simplexml/xml/xmlwriter/xmlreader, mbstring,
  gd, zip, intl, bcmath, mysqli/pdo_mysql, pgsql/pdo_pgsql, sqlite3,
  soap, ldap, imap, redis.
- `apt-cache search php8.3-` in the same container lists one package per
  extension (`php8.3-mbstring`, `php8.3-xml` -- DOM, SimpleXML, XML,
  XMLWriter and XMLReader all in one package -- `php8.3-curl`,
  `php8.3-gd`, `php8.3-zip`, `php8.3-intl`, `php8.3-bcmath`,
  `php8.3-mysql`, `php8.3-sqlite3`, `php8.3-soap`, `php8.3-ldap`,
  `php8.3-imap`, `php8.3-redis`, and more); `bench/g3-toolchain/static.py`'s
  `EXT_TO_APT_PACKAGE` transcribes the mapping actually used by the corpus.
  Composer itself (not the project) also needs the zip extension or a
  system `unzip`/`7z` binary to extract dist archives -- found empirically
  in Part B, not from this search, since it's a tooling need rather than
  a project one.

## Part A: static PHP/extension needs

Same project selection `compat/platform-drift.py` already made from
`compat/corpus.toml` and the public projects in `compat/hunted.md`: of 53
projects, 20 commit a composer.lock and clone (`compat/results/platform-
drift.md` has the full 53-project corpus list and the 33 skips). For each
of the 20: the PHP range (root `require.php` and the lock's own floor) and
every `ext-*` named by root or any locked package, prod and dev separately;
the PHP versions its CI tests; and, pinned at PHP 8.3 specifically (not
platform-drift's CI-bounded sweep, which for some projects never reaches
8.3), whether the baseline is excluded outright.

| Project | PHP range | Lock floor | CI PHP | Missing vs. baseline | Extra apt packages |
|---|---|---|---|---|---|
| symfony/demo | >=8.4 | 8.4.1 | 8.4, 8.5 | dom, mbstring, pdo_sqlite, simplexml, xml, xmlwriter | 3 |
| composer/composer | ^7.2.5 \|\| ^8.0 | 7.2.5 | 5.6-8.6 | simplexml (dev only) | 1 |
| phpunit/phpunit | >=8.4.1 | 8.4 | 7.2-8.6 | dom, mbstring, xmlwriter | 2 |
| wp-cli/wp-cli-bundle | >=7.2.24 | 7.2.24 | 5.6, 7.1, 7.2, 7.4, 8.5 | dom, mbstring, simplexml, xmlreader, xmlwriter | 2 |
| gamebetr/provable (sample) | >=7.2 | - | - | dom, mbstring, xml, xmlwriter | 2 |
| reedware/laravel-api (sample) | >=7.1 | 7.1.8 | - | mbstring | 1 |
| phpmyadmin/phpmyadmin | ^8.2 | 8.2 | 7.2, 8.2-8.6 | curl, dom, mbstring, mysqli, simplexml, xml, xmlwriter, zip | 5 |
| matomo-org/matomo | >=8.1.0 | 8.1 | 8.1 | curl, dom, gd, mbstring, simplexml, xmlwriter | 4 |
| monicahq/monica | ^8.3 | 8.2 | 8.3 | bcmath, curl, dom, intl, mbstring, simplexml, xml, xmlreader, xmlwriter | 5 |
| koel/koel | >=8.3 | 8.3 | 8.3, 8.4 | bcmath, curl, dom, gd, intl, mbstring, pdo_sqlite, simplexml, sqlite3, xml, xmlreader, xmlwriter, xsl | 8 |
| pixelfed/pixelfed | ^8.3\|^8.4\|^8.5 | 8.3 | 8.4, 8.5 | bcmath, curl, dom, intl, mbstring, redis, simplexml, xml, xmlwriter, zip | 7 |
| BookStackApp/BookStack | ^8.2.0 | 8.2 | - | curl, dom, gd, mbstring, simplexml, xml, xmlwriter, zip | 5 |
| snipe/snipe-it | ^8.2 | 8.2 | 8.2-8.5 | bcmath, curl, dom, gd, mbstring, simplexml, xml, xmlwriter, zip | 6 |
| mautic/mautic | (none) | 8.2 | 8.2, 8.3, 8.5 | curl, dom, gd, imap, mbstring, simplexml, xml, xmlreader, xmlwriter, zip | 6 |
| kimai/kimai | >=8.2 | 8.2 | 8.2-8.5 | dom, gd, intl, mbstring, simplexml, xml, xmlreader, xmlwriter, xsl, zip | 6 |
| firefly-iii/firefly-iii | >=8.5 | 8.5 | - | bcmath, curl, dom, intl, mbstring, simplexml, xml, xmlwriter | 5 |
| pterodactyl/panel | ^8.2 \|\| ^8.3 | 8.2 | 8.2, 8.3 | bcmath, curl, dom, mbstring, pdo_mysql, simplexml, xmlwriter, zip | 6 |
| librenms/librenms | ^8.2 | 8.2 | 8.2, 8.4 | curl, dom, gd, mbstring, simplexml, xml, xmlwriter, zip | 5 |
| humhub/humhub | >=8.2 | 8.2 | 8.2-8.4 | curl, dom, gd, intl, ldap, mbstring, simplexml, xml, xmlreader, xmlwriter, zip | 7 |
| akaunting/akaunting | ^8.1 | 8.1 | 8.1-8.3 | bcmath, curl, dom, gd, intl, mbstring, simplexml, xml, xmlreader, xmlwriter, zip | 7 |

### Totals

- Projects examined (committed lock, cloned): **20** of 53 (33 skipped: 32
  with no committed lock, 1 failed clone -- same as `compat/results/
  platform-drift.md`).
- Projects whose PHP range excludes the baseline (PHP 8.3): **5 of 20**
  (symfony/demo, phpunit/phpunit, firefly-iii/firefly-iii need a newer PHP;
  gamebetr/provable and reedware/laravel-api are single-package samples
  capped on an older one). Real applications in the corpus split 3
  excluded (newer) to 15 not excluded.
- Projects whose CI tests any PHP version other than 8.3: **15 of 20** --
  a developer working across this corpus needs more than one PHP on their
  own machine to match CI, regardless of what any single project's floor
  requires.
- Projects needing at least one extension missing from the baseline:
  **20 of 20** -- every project in the corpus that commits a lock needs
  something apt's `php-cli` alone doesn't provide.
- Extensions ranked by how many of the 20 need them: mbstring 19, dom 18,
  xmlwriter 18, simplexml 17, xml 14, curl 13, zip 10, gd 9, xmlreader 7,
  bcmath 7, intl 7, pdo_sqlite 2, xsl 2, mysqli 1, sqlite3 1, redis 1,
  imap 1, pdo_mysql 1, ldap 1.
- Extra apt packages needed per project over baseline: median **5**, mean
  **4.65**, range 1-8 (`bench/results/g3-toolchain.raw.json` has the
  per-project list).

## Part B: setup time, network excluded

Three projects, chosen for different needs (Part A's table):

- **composer/composer** -- needs only the baseline PHP (its one
  extra-vs-baseline extension, `ext-simplexml`, is require-dev only;
  installing `--no-dev`, as this measurement does throughout to sidestep
  an unrelated GitHub API rate-limit on one dev dependency's zipball, it
  needs zero extra packages). The closest thing in this corpus to "needs
  only baseline" -- no project examined needs literally nothing beyond
  `php-cli` once dev requirements are counted in.
- **BookStackApp/BookStack** -- needs five extensions (curl, dom→xml, gd,
  mbstring, zip).
- **symfony/demo** -- needs PHP >=8.4, which Ubuntu 24.04's own archive
  cannot provide at all (it ships 8.3 only); the only route to a non-8.3
  PHP is a third-party repository.

Three ways, `composer install --no-dev --no-scripts --no-plugins` timed
to first success in a fresh `docker run --rm` container, `composer.phar`
copied in (not downloaded), one shared `COMPOSER_CACHE_DIR` warmed once
and mounted into every container, `--network none` (apt/ppa ways, which
read a `file://` local repo, no server needed) or `--network` limited to
an `--internal` docker bridge holding only the static-PHP HTTP server (the
static way). Median of 3 runs per cell (task's own allowance -- see "what
was cut" below); one confirmatory (not repeated) run for the one cell
expected to fail.

| Project | Way | PHP setup (median) | Whole run (median) | Bytes from local server |
|---|---|---|---|---|
| composer/composer | apt (Ubuntu php8.3) | 3.68 s | 3.80 s | 15.8 MB |
| composer/composer | static (php-8.3.32 common) | 0.07 s | 0.31 s | 11.7 MB |
| BookStackApp/BookStack | apt (Ubuntu php8.3 + 5 ext) | 6.75 s | 8.27 s | 23.2 MB |
| BookStackApp/BookStack | static (php-8.3.32 common) | 0.07 s | 2.35 s | 11.7 MB |
| symfony/demo | apt (Ubuntu php8.3, baseline set) | -- | **fails**, rc=2 | 23.2 MB (fetched before failing) |
| symfony/demo | ppa (ondrej/php php8.4 + 3 ext) | 8.52 s | 8.95 s | 34.8 MB |
| symfony/demo | static (php-8.4.23 common) | 0.07 s | 1.04 s | 12.4 MB |

`bench/results/g3-toolchain-partb.raw.jsonl` has every run. The `symfony/
demo` "apt" row is the project's own extension set installed on Ubuntu's
plain php8.3 -- deliberately the wrong PHP, to confirm what the range in
Part A predicts: `composer install` refuses at the platform check
("requires ext-pdo_sqlite ... [and php >=8.4]"), one run, not repeated.

**Setup-time reading.** The static binary's PHP setup step is two orders
of magnitude faster than either apt-based way (0.07 s: one HTTP GET plus
an untar, versus 3.7-8.5 s of dependency resolution and unpacking dozens
of `.deb` files) on every project, including the one that needs no extra
extensions at all -- apt's own bookkeeping, not the extensions, is most of
that gap (composer/composer's apt setup, needing nothing beyond `php8.3-cli`
+ `unzip`, is still 3.68 s). The **whole run** gap is smaller once
Composer's own extraction time is added (0.31-2.35 s for static,
3.80-8.95 s for apt/ppa), because a warm shared cache makes Composer's own
step fast on every way; the PHP setup step is where the difference lives.

**Byte reading.** The static tarball's size is fixed per PHP version
(11.7 MB for 8.3.32, 12.4 MB for 8.4.23) regardless of how many extensions
a project needs, because static-php-cli's "common" prebuilt bundle already
contains every extension this corpus asked for (see "what was excluded"
below) -- apt's byte cost instead scales with the extension count (15.8 MB
for zero extra extensions up to 23.2 MB for five). The ondrej/php route is
the heaviest at 34.8 MB for only four PHP extensions (fewer than
BookStack's five, at 23.2 MB the Ubuntu way), because ondrej's `php8.4-*`
packages carry a `Recommends` chain Ubuntu's own `php8.3-*` packages don't:
of the 64 packages `apt-get install php8.4-cli php8.4-mbstring
php8.4-sqlite3 php8.4-xml unzip` actually pulls in, 24 are `cron`, `dbus`,
`systemd` and a `python3-gi`/`python3-dbus` stack that never appears in the
Ubuntu route's own package set at all -- a real, measured cost of the
third-party-repository route beyond the PHP itself.

## What was excluded, and why

- **Local apt repo, not apt-cacher-ng.** Pre-downloaded `.deb`s plus a flat
  `dpkg-scanpackages` index, read over `file://`: no proxy container is
  needed for it (unlike the static way, which the task asks to serve over
  an actual local HTTP server, done with `python -m http.server`). Ubuntu's
  own php8.3 and ondrej's php8.4 are two separate repos, not one merged
  one: apt would otherwise resolve `php8.3-cli` to ondrej's own rebuild of
  it (a higher version, same package name), which stops the "apt" way from
  measuring Ubuntu's own packages at all.
- **`--download-only` in an unpolluted container.** apt only downloads a
  dependency that isn't already installed; a warm container that installs
  `software-properties-common` (to add the PPA) first silently absorbs
  shared libraries into "already satisfied" and never copies their
  `.deb`s. The PPA's `*.sources` file is captured once from a container
  that ran `add-apt-repository` and replayed verbatim in the actual
  download step, which never runs `software-properties-common` itself
  (`bench/g3-toolchain/ondrej-ubuntu-php-noble.sources`).
- **`unzip` on every apt/ppa install, not counted against a project's own
  extension total.** Composer itself, not the target project, needs the
  zip extension or a system `unzip`/`7z` binary to extract dist archives --
  found empirically (the very first run failed on it) rather than
  predicted by Part A's static read, since it's a property of the tool,
  not the project. `unzip` is one universal apt package; static-php-cli's
  build already bundles the zip extension, so the static way needed
  nothing extra for it.
- **`--no-dev` throughout Part B**, not just for the extension counts:
  composer/composer's own require-dev pins `symfony/process` to a specific
  commit whose only dist URL is a GitHub API zipball endpoint, which
  intermittently fails to resolve under repeated automated requests. Since
  the task's own rule is "no network in the timed part", a dependency that
  needs a retry against a live API has no place in it either way; `--no-
  dev` is the same flag a production deploy already uses.
- **`COMPOSER_CACHE_DIR` mounted read-write, not read-only.** The task
  asks for read-only; empirically, Composer skips a read-only cache
  outright and re-fetches over the network instead of erroring, which
  defeats the "network excluded" rule. `sha1sum` of every file in the
  cache before and after the full Part B run is identical, so "one cache,
  identical for all three ways" still holds even though the mount
  permission doesn't literally say read-only.
- **One extension gap in static-php-cli's prebuilt set: `intl`.** Its
  "common" build (`https://dl.static-php.dev/static-php-cli/common/`)
  ships bcmath, bz2, calendar, ctype, curl, dom, exif, fileinfo, filter,
  ftp, gd, gmp, iconv, xml, mbstring, mbregex, mysqlnd, openssl, pcntl,
  pdo, pdo_mysql, pdo_sqlite, pdo_pgsql, pgsql, phar, posix, redis,
  session, simplexml, soap, sockets, sqlite3, tokenizer, xmlwriter,
  xmlreader, zlib, zip -- every extension the three Part B projects need,
  and all but one of the extensions Part A's ranking found across the full
  20: `ext-intl` is **not** in the common build's list. 7 of 20 corpus
  projects need it (monicahq/monica, koel/koel, pixelfed/pixelfed,
  kimai/kimai, humhub/humhub, akaunting/akaunting, firefly-iii/firefly-iii)
  and would need a custom `spc build` rather than the prebuilt download --
  none of the three Part B picks needed it, so this measurement doesn't
  time that path, but a build-if-it-holds decision should know the gap
  exists.
- **3 runs per cell, not 5.** The task allows cutting to 3 given the
  engineering cost of the rest of the harness (a local apt repo built
  twice over, a captured PPA source file, a warmed shared cache, an
  isolated docker network); the six successful cells varied by under 0.2 s
  run to run (`bench/results/g3-toolchain-partb.raw.jsonl`), so 3 already
  reads as stable.
- **Wall time.** Part B's own docker execution (warm once, then measure)
  took under 10 minutes end to end, inside the task's 30-minute cap; the
  containers, image and network it created were removed afterwards
  (`g3-staticphp`, `g3net`, `g3-toolchain-base` -- `ubuntu:24.04` and
  `python:3.12-slim`, both pulled not built, were kept).
