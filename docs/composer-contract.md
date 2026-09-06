# Composer output contract

What `composer install` (2.10, plain, no `--optimize`) writes, and the rules
vivace reproduces byte for byte. Derived from Composer's source
(`AutoloadGenerator`, `FilesystemRepository`, `PackageSorter`, `ArrayDumper`,
`composer/class-map-generator`) and checked against the fixtures in
`tests/fixtures/*/expected/`.

## Files

`vendor/autoload.php`, and in `vendor/composer/`:

| File | When |
|---|---|
| `autoload_namespaces.php`, `autoload_psr4.php`, `autoload_classmap.php`, `autoload_static.php`, `autoload_real.php` | always |
| `autoload_files.php` | any `files` entry, else deleted |
| `platform_check.php` | `config.platform-check` not `false` and at least one PHP or ext requirement, else deleted |
| `ClassLoader.php`, `InstalledVersions.php`, `LICENSE` | verbatim copies from Composer (MIT); vivace embeds them from `src/autoload/templates/` |
| `installed.json`, `installed.php` | always |
| `vendor/bin/*` | packages with `bin`; not in v0.1 |

No `.gitignore` is written. Files are only rewritten when their bytes change.

## Path rendering

Standard layout gives `$vendorDir = dirname(__DIR__); $baseDir = dirname($vendorDir);`.
A path under the vendor dir renders as `$vendorDir . '/psr/log/src'`, anything
else relative to the project as `$baseDir . '/src'`. Paths are normalised
first: `//` collapsed, `./` and `..` resolved, trailing slash stripped, so
`"src/"` and `"src"` both give `/src`. In `autoload_static.php` the same paths
render as `__DIR__ . '/..' . '/psr/log/src'` and `__DIR__ . '/../..' . '/src'`.

## Autoloader suffix

`config.autoloader-suffix`, else the suffix already in `vendor/autoload.php`
(`ComposerAutoloaderInit([^:\s]+)::`), else the lock's `content-hash`, else 32
random hex. Class names: `ComposerAutoloaderInit<suffix>` and
`Composer\Autoload\ComposerStaticInit<suffix>`.

## Ordering

Packages are sorted with `PackageSorter`: each package's weight is decremented
by `1 - importance(user)` for every package that `require`s it (cycles count
0), then sorted by weight ascending with `strnatcasecmp` on the name as the
tie-break. Dependencies therefore come before dependants. The root package is
appended last.

- `files`: sorted order (dependencies first, root last). Key is
  `md5("<package name>:<path as written>")`.
- `psr-0`, `psr-4`, `classmap`: reverse sorted order (root first). Then
  `krsort` on namespace for psr-0/psr-4; path lists keep insertion order.
- `classmap` output is `ksort`ed by class name and always contains
  `Composer\InstalledVersions => $vendorDir . '/composer/InstalledVersions.php'`.
- In dev mode the root's `autoload-dev` is merged into its `autoload`
  (`array_merge_recursive`). With `--no-dev`, packages in
  `dev-package-names` are skipped everywhere, including `platform_check.php`.

## autoload_static.php

Properties emitted in `ClassLoader` declaration order, skipping empty ones:
`$files` (only if autoload_files.php exists), `$prefixLengthsPsr4`,
`$prefixDirsPsr4`, `$fallbackDirsPsr4`, `$prefixesPsr0`, `$fallbackDirsPsr0`,
`$classMap`. `prefixLengthsPsr4` is keyed by first character, then prefix,
value `strlen(prefix)`. Values are PHP `var_export` output, re-indented by four
spaces per level with trailing spaces stripped (so `'P' =>` has no trailing
space). The initializer assigns every property except `$files`, followed by a
blank line before `}, null, ClassLoader::class);`.

## autoload_real.php

Fixed template. `require __DIR__ . '/platform_check.php';` only when that file
is written; the `$filesToLoad` block only when `autoload_files.php` exists;
`$loader->register(true)` unless `config.prepend-autoloader` is `false`.

## Classmap scanning

Only `classmap` entries are scanned on a plain install (PSR dirs only with
`--optimize`). Extensions `.php`, `.inc`, `.hh`. Algorithm
(`PhpFileParser::findClasses`):

1. Strip comments, replace string/heredoc/nowdoc bodies with `null`, drop
   anything outside `<?php ... ?>`.
2. Regex, case-insensitive:
   `\b(?<![\\$:>])(class|interface|trait|enum)\s++([a-zA-Z_\x7f-\xff:][a-zA-Z0-9_\x7f-\xff:\-]*+)`
   or `\b(?<![\\$:>])namespace(\s++<name(\s*\\\s*name)*>)?\s*+[\{;]`.
3. Walk matches: `namespace X\Y` sets prefix `X\Y\`; bare `namespace {` sets
   `''`. Skip names `extends`/`implements` (anonymous classes). For `enum`,
   cut the name at the last `:`. Emit `ltrim(prefix.name, '\\')`.

The lookbehind rejects `Foo::class`, `$class`, `->class`; `\b` rejects
`subclass`. First occurrence of a class wins. vivace visits files in sorted
path order, so which occurrence is first is deterministic; Composer's Finder
order depends on the filesystem.

## installed.json

```json
{ "packages": [ ...sorted by name... ], "dev": true, "dev-package-names": [ ...sorted... ] }
```

Each package is the lock entry re-dumped by `ArrayDumper` in this key order:
`name`, `version`, `version_normalized`, `target-dir`, `source`, `dist`,
`require`, `conflict`, `provide`, `replace`, `require-dev`, `suggest`, `time`,
`default-branch`, `bin`, `type`, `extra`, `installation-source` (`"dist"`),
`autoload`, `autoload-dev`, `notification-url`, `include-path`, `php-ext`,
`archive`, `scripts`, `license`, `authors`, `description`, `homepage`,
`keywords`, `repositories`, `support`, `funding`, `abandoned`,
`transport-options`, `install-path` (`"../vendor/name"`). Link maps are
`ksort`ed, `keywords` sorted, empty arrays and nulls dropped, `shasum: ""`
kept. JSON is pretty-printed with 4 spaces, unescaped slashes and unicode,
trailing newline.

## installed.php

```php
<?php return array(
    'root' => array(
        'name' => 'vendor/root',
        'pretty_version' => '1.0.0+no-version-set',
        'version' => '1.0.0.0',
        'reference' => null,
        'type' => 'project',
        'install_path' => __DIR__ . '/../../',
        'aliases' => array(),
        'dev' => true,
    ),
    'versions' => array(
        'foo/bar' => array(
            'pretty_version' => '3.11.0',
            'version' => '3.11.0.0',
            'reference' => '<dist reference>',
            'type' => 'library',
            'install_path' => __DIR__ . '/../foo/bar',
            'aliases' => array(),
            'dev_requirement' => false,
        ),
        'psr/log-implementation' => array(
            'dev_requirement' => false,
            'provided' => array(
                0 => '3.0.0',
            ),
        ),
    ),
);
```

`versions` is `ksort`ed and includes the root. `replace` and `provide` links
(skipping `php*`, `ext-*`, `lib-*`, `composer*`) add `replaced`/`provided`
lists of pretty constraints; `self.version` becomes the provider's version.
Null renders as lowercase `null`. Version normalisation: pad to four numeric
components (`3.11.0` becomes `3.11.0.0`), root without a version is
`1.0.0+no-version-set` / `1.0.0.0`.

## platform_check.php

Inputs are the `require` links of the root and every installed non-dev
package. For `php`, the highest lower bound across constraints gives
`PHP_VERSION_ID >= 80100` and the message `">= 8.1.0"`. With
`config.platform-check: true` (default is `"php-only"`), `ext-*` requirements
add `extension_loaded('x') || $missingExtensions[] = 'x';` lines, `ksort`ed,
unless another package `replace`s or `provide`s the extension.

## Zip extraction

If the archive has exactly one top-level entry and it is a directory
(`.DS_Store` ignored), its contents become the package root. Otherwise the
archive root is. Composer extracts with system `unzip`, so exec bits survive.

## Dist cache

Composer: `~/.cache/composer/files/<vendor>/<name>/<sha1 of dist url>.zip`.
`dist.shasum` is the sha1 of the archive and is `""` for GitHub zipballs, so
verification is skipped when empty.
