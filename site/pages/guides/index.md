---
title: Guides
order: 10
summary: Everyday commands, migrating a project, and what viv does beyond installing.
---

# Guides

Past your first install, the rest of what viv does day to day: the
commands you reach for most, merging `composer.lock` without conflicts,
running a project's tools and PHP version without a container, and moving
a whole project over from Composer.

- [Cheat sheet](cheatsheet.html) — the common commands, one line each.
- [Merging composer.lock without conflicts](lock-merge.html) — the merge driver and what it still can't resolve.
- [viv.lock](viv-lock.html) — the conflict-less lock format on its own, without `composer.lock`.
- [A PHP per project](php.html) — a self-contained PHP build pinned per project, no container.
- [Running tools without installing them](tools.html) — `viv x`, npx-style.
- [Workspaces](workspaces.html) — one resolve and one `vendor/` across several `composer.json` files.
- [Scripts and running commands](scripts-and-run.html) — `viv run`, `viv exec`, and lifecycle scripts.
- [Automatic normalisation](normalize.html) — a `composer.json` that never gets a reordering-only diff.
- [Plugins](plugins.html) — which Composer plugins viv reimplements, ignores or refuses.
- [Two plugins, one library](isolate.html) — prefixing a plugin's bundled dependencies, and why it stays opt-in.
- [Speed](speed.html) — the benchmark numbers and how they're kept honest release to release.
- [Reasons to use viv, and reasons not to](reasons.html) — the honest trade-offs.
- [Scope](scope.html) — the commands, repositories and package types viv covers.
- [Migrating from Composer](migrate.html) — the shim, Dockerfiles and CI flags for moving a whole project over.
- [For your framework](frameworks.html) — the local/Dockerfile/CI snippets and compatibility result for your framework starter.
- [Cache and offline use](cache.html) — the shared store, `--cache-dir` and `--offline`.
- [Troubleshooting](troubleshooting.html) — common errors and what to do about them.
