---
title: Install and upgrade
order: 20
summary: binstall, Homebrew, cargo install, the .deb, the tarball, and upgrading in place.
---

# Install and upgrade

## Try it

```sh
cargo binstall vivace          # or: brew install svandragt/tap/vivace
viv install     # in a project with composer.json and composer.lock
```

`cargo binstall` and `cargo install` also install a `composer` shim next to
`viv` (the Homebrew formula and the `.deb` install `viv` only); put the shim
first on `PATH` and your existing scripts run through viv unedited (see [Using viv as
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

## Starting from nothing

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

## Prebuilt binaries and the .deb

Prebuilt binaries, including the `.deb`, are covered above in [Try
it](#try-it): the README lists the exact platforms and architectures
(Linux x86_64 as glibc and static musl builds, aarch64 as static musl,
also packaged as a `.deb`; macOS x86_64 and aarch64), all on the
[releases page](https://github.com/svandragt/vivace/releases).

## Man pages and shell completions

The release tarballs, the `.deb` and the Homebrew formula install man pages
(`man viv`, `man viv-install`) and completion scripts for bash, zsh and
fish. `cargo binstall` and `cargo install` install the binaries only. In
that case, print a script and save it where your shell loads completions:

```sh
viv completions bash > ~/.local/share/bash-completion/completions/viv
viv completions zsh > ~/.zfunc/_viv     # a directory on your fpath
viv completions fish > ~/.config/fish/completions/viv.fish
```

## Man pages for cargo installs

`cargo install` and `cargo binstall` install only the binary. To get
`man viv`, download the release tarball for your platform from the
[releases page](https://github.com/svandragt/vivace/releases) and copy its
`man/*.1` files into `~/.local/share/man/man1/`.

## No PHP, no problem

Every install method above needs nothing but the `viv` binary itself: no
PHP runtime, no Composer. For a build stage or a runner with neither
installed, see [In a Dockerfile](docker.html) and [In CI](ci.html).

[^1]: viv relinks every package from its own content-addressed store into `vendor/`, using hardlinks so files aren't copied or re-extracted.
