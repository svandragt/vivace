# Prior art: rspack

rspack is a Rust rewrite of webpack that keeps webpack's configuration,
loaders and plugins and replaces the engine. Its situation is viv's:
a byte-compatible replacement for an incumbent, in Rust, whose users
came for speed and stay only if nothing breaks. This note takes each
principle rspack states for its speed and says what viv already does,
what it lacks, and which research chapter the principle supports.
Sources are rspack's own announcements and configuration pages, read
on 2026-09-24.

## What rspack says makes it fast

| Principle | rspack's claim | viv today |
|---|---|---|
| Native code on every core | Rust, "full advantage of modern multi-core CPUs"; 10 times webpack at 1.0 | Same footing. Extraction and linking run on an 8-way thread pool; the link phase sits at the syscall floor (#270). No lever left here. |
| Incremental rebuilds, separate from caching | Reuse "unaffected intermediate results between compilations"; disabling the cache does not disable incremental work | viv is one-shot, so only the cross-run half applies. That is the store: immutable archives, the per-archive classmap sidecar (#303), the platform probe cache (#178), the platform-check verdict (#300). |
| Persistent cache keyed by snapshot, saved per build step | Snapshot, Occasion, Storage: a file snapshot taken once, a save/restore point at each step, one key-value layer under both | viv has the steps but no shared snapshot: each cache computes its own key from the same inputs (composer.json, the lock, the php binary). See "One snapshot per run" below. |
| Managed paths hashed by version, not content | `node_modules` keyed on package.json version, "rather than content hashing for speed" | Same rule: store entries are keyed by dist reference, never rehashed; the root package alone is fingerprinted by mtime and file count (#269). |
| Lazy work | Lazy compilation builds only what a request needs; `lazyBarrel` defers re-export resolution, 49 percent fewer module resolutions on UI projects | The closure walk still fetches provider files for every name in the request, locked or not. This is candidate C, lock-seeded solving: fetch only what the solve cannot satisfy from the lock. |
| Cheap boundary crossings | Lazy message objects across JS and Rust, hooks skipped when unused, Rust-native plugins to avoid the crossing altogether | viv's native adapters are the Rust-native plugin: no PHP process, no plugin execution (docs/plugin-strategy.md). The lazy-object idea is #177's `forget_repo` and the raw-metadata counters in the closure walk. |
| Sealed phase parallelised with better data structures | 50 percent off one app's build, module concatenation down 76 percent, from data structures before threads | The same order held in milestone 21: six allocation fixes (#302) and the shaped sidecar (#303) moved warm `-o` from 78 to 53 ms before any parallelism question. |
| A regression tool | Rsdoctor locates a slow phase after a reported regression | `make bench-ab` gates the number; the phase logs behind `RUST_LOG=debug` locate it. Not user-facing. |

## What rspack says about compatibility

rspack copies webpack's whole test suite into its tree and reports
compatibility as (passed + will-not-support) / total, with a filter file
that marks each failing test as not yet supported or never supported.
The metric moves only when a test flips, so nobody argues about a
percentage in prose.

viv runs Composer's installer fixtures the same way
(`tests/fixtures/composer/installer/*.test`) and sweeps a corpus of
projects for byte-identical output, but the fixture share is reported
per run and the will-not-support line is implicit: plugin execution is
out by policy, everything else is a bug. Two things worth copying:

- A filter file next to the fixtures that says, per skipped fixture,
  whether it is not yet or never, with the issue or the policy line.
  The README's compatibility figure then comes from a count.
- The 2.0 lesson. rspack's authors write that carrying webpack's
  output formats "carries forward some historical design constraints"
  and that the project is "not just a faster webpack". That is the
  research programme's own premise: compat mode is the control, and
  each chapter is a deliberate departure with a measurement. rspack
  took five years to say it out loud; the programme says it in
  docs/research.md from the start.

## What rspack reworked

- Dependency weight. 2.0 cut the dev server from 192 dependencies to 1
  and the install from 15 MB to 1.4 MB. viv ships one binary and
  `cargo deny` plus `cargo machete` run in `make check`; the lesson is
  already policy.
- Deferred re-exports (`lazyBarrel`). Named above as candidate C's
  precedent: the saving came from not resolving what nobody asked for.
- A 20 percent recompile regression in 0.6 caught by users. Their
  answer was a tool; viv's is the A/B gate on every install-path
  change, which is cheaper than a tool but only catches what the bench
  projects exercise.

## One snapshot per run

The one architectural idea worth taking is the smallest. rspack takes
a file snapshot once per compilation and every step's cache checks
against it. viv computes the same facts several times in one run:
the composer.json bytes are hashed for the content hash, again for
the platform-check verdict, and read once more for the lock freshness
check; the lock is read for install and hashed for the verdict; the
php binary is stat'ed for the probe cache key. Each cache was added
on its own ticket and each invented its own key.

A `Snapshot` struct built once in `install::run_impl` and `update`,
holding the composer.json hash, the lock hash, the php probe key and
the root fingerprint, and passed to every cache, would cost nothing
measurable, make the next cache a one-line key, and remove the class
of bug where two caches disagree about whether the project changed.
It is a refactor, not a chapter, and belongs in the next speed
milestone rather than the programme.

## Not applicable

- Watch mode, hot module replacement and the Rust file watcher: viv
  is one-shot and has no daemon. If a daemon ever appears (a `viv
  serve` for a monorepo), rspack's split between in-memory incremental
  state and on-disk cache is the design to read first.
- Tree shaking, minification and code splitting: no analogue in a
  package installer.
