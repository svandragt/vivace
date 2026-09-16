# Cheat sheet

> `viv` installs PHP dependencies from `composer.lock` and writes the `vendor/` directory Composer would write.
> Every command takes `-d <dir>` to run against another project. More information: [Commands](commands.html).

- Install a project's dependencies from its `composer.lock`, adopting a `vendor/` Composer already wrote:

`viv install`

- Install without development dependencies, as on a server or in an image:

`viv install --no-dev`

- Show what an install would change without touching anything:

`viv install --dry-run`

- Add a package to `composer.json`, resolve it and install (alias `require`):

`viv add <vendor/package>`

- Remove a package from `composer.json`, resolve the rest and install (alias `remove`):

`viv rm <vendor/package>`

- Update every dependency, rewrite `composer.lock` and install:

`viv update`

- Update one package and its dependencies, leaving the rest at their locked versions:

`viv update <vendor/package> -w`

- Rewrite `composer.lock` after editing `composer.json` by hand, without installing:

`viv update --no-install`

- Write a `composer.json` for a new project and stop, defaults inferred from git and the directory:

`viv init`

- Start a new project from a package skeleton in a directory that does not exist yet:

`viv new <vendor/package>:<constraint> <dir>`

- Regenerate the autoloader and `vendor/bin` from an installed `vendor/`:

`viv dump-autoload -o`

- Show why an installed package is there, listing the packages that require it:

`viv why <vendor/package>`

- List installed packages with a newer version available:

`viv outdated`

- Check installed packages for security advisories:

`viv audit`

- Run a `scripts` entry from `composer.json`:

`viv run <script>`

- Run a package's command-line tool without adding it to the project, npx-style:

`viv x <vendor/package>`

- Install even though the lock names a Composer plugin viv cannot run:

`viv install --no-plugins`

- Install when the machine lacks a PHP version or extension the lock requires:

`viv install --ignore-platform-reqs`

- Remove cache entries no project uses any more:

`viv cache prune`

- Print an environment report to paste into a bug report:

`viv diagnose`

- Update viv itself to the latest release:

`cargo binstall vivace`
