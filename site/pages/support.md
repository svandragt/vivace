# Support

## Where to report what

- A crash or wrong behaviour: open a [bug report](https://github.com/svandragt/vivace/issues/new?template=bug.yml). Paste the output of `viv diagnose`; it names auth sources but never prints secrets.
- `vendor/` or `composer.lock` that differ from Composer's: open a [compatibility report](https://github.com/svandragt/vivace/issues/new?template=compat.yml) with a `diff -r` of the two trees.
- A security problem: follow [SECURITY.md](https://github.com/svandragt/vivace/blob/main/SECURITY.md) and report privately.
- Anything else, questions included: open a [blank issue](https://github.com/svandragt/vivace/issues/new). There is no discussion board.

## How long to expect

One person maintains viv. Expect an acknowledgement within a week. Bugs that make viv write something Composer would not write come first; see [Compatibility and scope](compatibility.html) for what viv promises to match.

## What holds across releases

viv is pre-1.0. A minor release may add commands, change progress wording or get faster. It may not change the bytes of `vendor/`, `composer.lock` or the plain-text output of `show`, `why` and `validate` that Composer's pinned version would write for the same input, unless it fixes a bug in an earlier release's output. Each release states which Composer version it targets. The full contract is [docs/stability.md](https://github.com/svandragt/vivace/blob/main/docs/stability.md).

## If the maintainer stops

viv is GPL-3.0-or-later: the source, the test corpus and the compatibility sweep are all in the repository, so anyone can build, fix and release it. [vivacity](compare.html) is an independent implementation checked against the same Composer, so the approach does not rest on one codebase or one maintainer.
