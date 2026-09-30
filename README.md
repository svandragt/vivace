# vivace

`viv` is a Rust reimplementation of Composer that installs from
`composer.lock` and writes the `vendor/` directory Composer would write,
byte for byte. A cold `laravel/laravel` install takes 0.30 s against
Composer's 1.58 s, and the compatibility sweep run before every release
finds an identical `vendor/` on every project viv installs. Merged as a
git merge driver, `composer.lock` conflicted 228 times in 355 real merges
under git and 6 times with viv.

It began as a question, whether a person directing coding agents can build
a faster drop-in Composer, and that question is answered. The compatible
mode is finished and frozen as a control; viv continues as a research
vehicle for package-manager design, one measured chapter at a time.

## Try it

```sh
cargo binstall vivace          # or: brew install svandragt/tap/vivace
viv install                    # in a project with composer.json and composer.lock
```

That also installs a `composer` shim next to `viv`; put it first on `PATH`
and your existing scripts run through viv unedited. If Composer already
wrote `vendor/`, viv adopts it. You can stop at any point: a `vendor/`
viv wrote is a valid Composer install, and `viv cache clean` removes
everything else.

Prebuilt binaries for Linux and macOS, a `.deb`, a container image and a
GitHub Action are on the [releases page](https://github.com/svandragt/vivace/releases)
and in the [install guide](https://vivace.vandragt.com/getting-started/install.html).

## Documentation

- [Getting started](https://vivace.vandragt.com/getting-started/) — install, first
  install, the `composer` shim, CI and Docker.
- [Guides](https://vivace.vandragt.com/guides/) — merging locks without
  conflicts, committing `viv.lock` alone, a PHP per project, running tools
  with `viv x`, workspaces, plugins, speed, and the reasons for and against.
- [Reference](https://vivace.vandragt.com/reference/) — every command with
  its live `--help`, environment variables, exit codes, the files viv
  reads and writes.
- [Architecture and research](https://vivace.vandragt.com/architecture/) —
  how it works, the Composer output contract, what a minor release may
  change, and the research programme with its measurements.
- [Releases](https://vivace.vandragt.com/releases/) — the changelog.

## Is it safe to try

viv's contract is that its output matches Composer's byte for byte, and
the [compatibility sweep](https://vivace.vandragt.com/reference/compatibility.html)
checks it before every release: on v0.20.0, all 20 install rows of the
pinned corpus, all 10 locks and all 10 exported locks are identical, and
every random-sample project Composer could install is identical too. One
pinned project needs `--no-plugins`, for a plugin viv refuses by design.
It stays 0.x; Windows is not supported; a plugin without a native adapter
stops the install with an error naming it.
[Reasons not to use viv](https://vivace.vandragt.com/guides/reasons.html)
has the full list.

## Development

Tooling comes from [devbox](https://www.jetify.com/devbox): PHP, Composer
and hyperfine for the fixtures and benchmarks.

```sh
make install    # put viv on your PATH (~/.cargo/bin); make install-shim adds the composer drop-in
make check      # fmt, clippy, tests, cargo deny, cargo machete, cargo doc
```

To report a bug or ask a question, see the
[Support](https://vivace.vandragt.com/getting-started/support.html) page and
[`SECURITY.md`](SECURITY.md) for anything security-related.
[`JOURNAL.md`](JOURNAL.md) is the engineering log; [`AGENTS.md`](AGENTS.md)
is how the project is worked on.

## Licence

GPL-3.0-or-later. viv ports three Composer plugins whose own source is
GPL-2.0-or-later, so the binary is a derivative work of them and carries
their licence. A few files are vendored from MIT-licensed projects and keep
their original copyright notices. [`NOTICE.md`](NOTICE.md) records every
port, its upstream and its licence.
