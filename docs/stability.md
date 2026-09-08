# Stability

This is the contract viv (0.7.0) offers scripts, CI pipelines and the
`composer` shim that drive it instead of Composer.

## What viv promises

For the commands listed in the README's [Scope](../README.md#scope)
section, viv writes a `vendor/` directory — including `vendor/composer/*` —
and, for `update`, `update-lock` and `require`/`remove`, a `composer.lock`
that are byte-identical to what Composer 2.10 (the version pinned in
`devbox.json`) writes for the same `composer.json`, lock file and cache
state. This is the contract, not an implementation detail: if viv's output
differs from Composer's for a covered command, that's a bug.

Composer moves; viv tracks one version of it at a time. Each release states
which Composer version it targets, and the compat sweep (see below) is run
against that version before the release ships.

## What is not covered

- **Plugins.** Only the native adapters viv ships (see
  [`docs/plugin-strategy.md`](plugin-strategy.md)) are covered. Any other
  plugin is out of scope; viv either skips it or the `composer` shim hands
  the invocation to the real Composer.
- **Commands Composer keeps.** `create-project`, `init`, `search`, `config`,
  `global`, `self-update`, `diagnose`, `licenses`, `depends` and the rest of
  the long tail stay with Composer and aren't part of viv's contract.
- **Progress wording on stdout during a run.** Only the files viv writes and
  the plain-text output of commands such as `show`, `why` and `validate` are
  contractual. Spinner text, progress counters and similar in-flight
  chatter can change between releases without notice.
- **Timing.** The numbers in the README track viv's speed but aren't a
  promise; a release can get slower on a given machine without breaking the
  contract, though the project tries hard not to let that happen.

## Versioning

viv is pre-1.0: expect breaking changes between minor releases, and expect
them to be called out in `JOURNAL.md`. Within that, a minor release may:

- add new commands or flags,
- make internals faster,
- change stdout progress wording or timing.

A minor release may not change the output bytes (`vendor/`, `composer.lock`,
or the contractual stdout of `show`/`why`/`validate`) that Composer's pinned
version would produce for a given input, unless it's fixing a bug in a
previous release's output.

When a release moves the pinned Composer version, its `JOURNAL.md` entry
says so, and the compat sweep results in `compat/results/` are regenerated
against the new version as part of that release (see
[`AGENTS.md`](../AGENTS.md), "Before a release").

## Interface for the shim and scripts

viv's exit codes, defined in `src/main.rs`:

- `0` — success.
- `1` — a command failed (an `anyhow::Error` bubbled up from the command,
  or an `outdated`/`audit` check that isn't a lock mismatch or vulnerable
  package).
- `2` — the dependency resolver couldn't find a solution
  (`resolver_error`).

`outdated --strict`-style checks that report "something out of date" use
`1` rather than a dedicated code; treat any non-zero exit as failure unless
a command's own docs say otherwise.

stderr is for humans: warnings, confirmation prompts and diagnostics can
reword between releases without notice. stdout for `show`, `why`,
`validate` and the other commands that mirror Composer's plain-text output
is contractual and covered by the same guarantee as `vendor/` and
`composer.lock`.

## Deprecations

A flag or behaviour viv drops isn't removed outright. It stays for one
minor release as a no-op: it's accepted, has no effect, and prints a single
warning to stderr the first time it fires. The next minor release removes
it.

The first instance is `--no-normalize` on `install` and `dump-autoload`
(#95): since 0.6, neither command touches `composer.json`, so the flag has
nothing left to disable. It's kept as a silent-except-for-the-warning no-op
so that a script or CI job that still passes it doesn't fail, and prints
`--no-normalize is a no-op on install since 0.6; install no longer touches
composer.json` (or the `dump-autoload` equivalent) once.

## How to report a contract break

If viv's `vendor/`, `composer.lock` or contractual stdout differs from what
the pinned Composer version produces, file an issue with:

- `composer.json` and `composer.lock` (or the pair before/after, for an
  `update`),
- the Composer version you compared against,
- a diff of the mismatch (a directory diff for `vendor/`, or the two lock
  files).

The [compat sweep](../compat/README.md) is the tool that finds these
mismatches before release; if you can reproduce the break with a project
from its corpus, name it in the issue and that's enough.
