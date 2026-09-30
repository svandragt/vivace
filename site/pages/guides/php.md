---
title: A PHP per project
order: 40
summary: A self-contained PHP build pinned per project, no root and no container needed.
---

# A PHP per project

`viv php install 8.4` downloads a self-contained PHP build from
[static-php-cli](https://static-php-cli.dev) into the store and pins it as
`config.platform.php` in `composer.json`, with no root and no system
package manager. `viv run <name>` then runs a `scripts` entry, a
`vendor/bin` binary or any command on `PATH` with that PHP first, and
installs the pinned build on first use, so `phpunit`, `phpcs` or a git
hook run on the project's PHP version without booting a container:

```sh
viv php install 8.4          # newest 8.4.x, once per machine
viv run phpunit --filter Foo # vendor/bin/phpunit on the pinned PHP
viv run php -v               # the pinned build itself
viv php list                 # builds in the cache
```

viv's own flags go before the name (`viv run -d ../app phpunit`);
everything after it belongs to the tool. Projects whose PHP runs in a
container (ddev, Altis local-server) keep their own `wp`/`exec`
wrappers; the pin is for host-side tools and CI.

Linux and macOS on x86_64 and aarch64. The build carries the common
extensions including `intl`; the newest version upstream publishes is
8.4 today.
