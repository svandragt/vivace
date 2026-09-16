# Cheat sheet

The commands most people reach for. Every one takes `-d <dir>` to run against another project.

- Install a project's dependencies from its `composer.lock`. A `vendor/` Composer already wrote is adopted:

      viv install

- Install without development dependencies, as on a server or in an image:

      viv install --no-dev

- Add a package to `composer.json`, resolve it and install (alias `require`):

      viv add <vendor/package>

- Remove a package, resolve the rest and install (alias `remove`):

      viv rm <vendor/package>

- Update every dependency, rewrite `composer.lock` and install:

      viv update

- Update one package and its dependencies, leaving the rest at their locked versions:

      viv update <vendor/package> -w

- Run a package's command-line tool without adding it to the project, npx-style:

      viv x <vendor/package>

- Print an environment report to paste into a bug report:

      viv diagnose

Everything else, with every flag, is on the [Commands](commands.html) page.
