---
title: Scope
order: 120
summary: The commands viv runs itself, and the repositories, packages and plugins it works with.
---

# Scope

viv covers the Composer commands you run every day. Everything else stays
with Composer, and the `composer` shim passes those commands through.

Commands viv runs itself, with the same output as Composer:

- Start and resolve: `init`, `new` (`create-project`), `install`, `update`,
  `update-lock`, `add` (`require`), `rm` (`remove`), `dump-autoload`,
  `normalize`.
- Inspect: `show`, `tree`, `why`, `outdated`, `audit`, `validate`.
- Maintain: `lock` (convert, merge, export), `workspace` (list, init, add).
- Run: `run`, `exec`, `php` (install, list), the lifecycle scripts, and the
  cache commands.
- `diagnose` prints viv's own report, not Composer's.
- `isolate` prefixes a plugin's bundled libraries with php-scoper, which
  Composer has no command for, and `completions` prints a bash, zsh or
  fish completion script.

Commands that stay with Composer: `search`, `config`, `global`,
`self-update`, `licenses`, `depends` and the rest.

## Works with

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
  `COMPOSER_DISABLE_NETWORK`. `install` refuses before writing anything when
  the lock needs a PHP version, extension or library the detected platform
  lacks, honouring `config.platform`, `--ignore-platform-reqs` and
  `--ignore-platform-req`.
- Plugins: the ones with native adapters, listed under [Plugins](#plugins).
