---
title: Compatibility and scope
order: 1
summary: What viv's byte-identical promise does and doesn't cover.
---
# Compatibility and scope

What viv's byte-identical promise actually covers, what stays out of scope,
and how that's checked before every release.

## What viv promises

{{include:docs/stability.md#What viv promises}}

## What is not covered

{{include:docs/stability.md#What is not covered}}

## Works with

{{include:README.md#Works with}}

## Reasons not to use viv

{{include:README.md#Reasons not to use viv}}

## Is it safe to try

{{include:README.md#Is it safe to try}}

## How compatibility is checked

Every release runs the [compatibility
sweep](https://github.com/svandragt/vivace/blob/main/compat/README.md)
against a mix of pinned popular projects and a random sample of Packagist
packages, comparing viv's `vendor/` and `composer.lock` against Composer's
own. Per-release results live in
[`compat/results/`](https://github.com/svandragt/vivace/blob/main/compat/results);
ongoing sweeps of public projects outside that pinned set are tracked in
[`compat/hunted.md`](https://github.com/svandragt/vivace/blob/main/compat/hunted.md).
The [Compare](/compare.html) page turns the newest numbers into a table.

For the curious, here's exactly what a plain install writes, and how a path
renders inside it — the detail the sweep checks byte for byte.

{{include:docs/composer-contract.md#Files}}

{{include:docs/composer-contract.md#Path rendering}}

## Versioning

{{include:docs/stability.md#Versioning}}
