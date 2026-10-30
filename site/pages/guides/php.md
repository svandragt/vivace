---
title: A PHP per project
order: 40
summary: A self-contained PHP build set per project, no root and no container needed.
---

# A PHP per project

`viv php install 8.4` downloads a self-contained PHP build from
[static-php-cli](https://static-php-cli.dev) into the store and sets it as
`extra.viv.php` in `composer.json`, with no root and no system package
manager. With `extra.viv.php` set, viv uses that build in place of the
`php` on `PATH`. `viv run <name>` runs a `scripts` entry, a `vendor/bin`
binary or any command on `PATH` with that PHP first, and installs the build
on first use, so `phpunit`, `phpcs` or a git hook run on the project's PHP
version without booting a container. `viv x`, `viv install`, `viv update`
and the other commands that read the PHP's extensions use the same build,
and install it on first use unless `--offline` is set:

```sh
viv php install 8.4          # newest 8.4.x, once per machine
viv run phpunit --filter Foo # vendor/bin/phpunit on the project's PHP
viv run php -v               # the build itself
viv php list                 # builds in the cache
```

`extra.viv.php` takes a minor version (`8.4`, the newest `8.4.x`) or an
exact one (`8.4.17`):

```json
{
    "extra": {
        "viv": { "php": "8.4" }
    }
}
```

Without `extra.viv.php`, viv uses the `php` on `PATH`, as Composer does.
`config.platform.php` does not choose the PHP that runs. As in Composer, it
only tells the solver which PHP version to resolve dependencies for, so a
project can keep it while its CI or devbox supplies the PHP.

viv's own flags go before the name (`viv run -d ../app phpunit`);
everything after it belongs to the tool. Projects whose PHP runs in a
container (ddev, Altis local-server) keep their own `wp`/`exec`
wrappers; `extra.viv.php` is for host-side tools and CI.

Linux and macOS on x86_64 and aarch64. The build carries the common
extensions including `intl`; the newest version upstream publishes is
8.4 today.
