---
title: Research programme
order: 70
summary: One question per chapter, compat mode kept as the control, measured before it's believed.
---

# Research programme

vivace started as one question: can a person directing coding agents build
a faster, byte-compatible Composer? By 0.13 the answer is yes. The compat
sweep finds an identical `vendor/` on every corpus project, and the bench
corpus puts a cold Laravel install at a fifth of Composer's time. That work
is finished and the mode that does it, called compat mode below, is frozen
as the control for what follows.

viv is now a research vehicle for package-manager design. Each chapter asks
one question, states a hypothesis, keeps compat mode as the control,
measures, and ends in a write-up. Behaviour a chapter adds sits behind a
flag or a manifest setting so the default path stays byte-compatible, and
`composer.lock` stays exportable so viv never becomes a fork of itself.
[Generation 3](generation-3.html) asks what viv could be without that
constraint, still opt-in per project.

## How chapters run

{{include:docs/research.md#How chapters run}}

## Chapters

- [Chapter 1: a conflict-less lock](chapter-1.html) — format and driver measured, on hold.
- [Chapter 2: autoload from the store, no vendor tree](chapter-2.html) — measured, not pursued.
- [Chapter 3: workspaces](chapter-3.html) — measured, not pursued.
- [Candidate chapters](candidates.html) — measured and parked or built, one section each.
- [Generation 3: a better PHP package manager](generation-3.html) — the toolchain, plugin isolation and commit-pinned locks, opt-in.
