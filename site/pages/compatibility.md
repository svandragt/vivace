# Compatibility and scope

What viv's byte-identical promise actually covers, what stays out of scope,
and how that's checked before every release.

## What viv promises

{{doc:docs/stability.md#What viv promises}}

## What is not covered

{{doc:docs/stability.md#What is not covered}}

## Works with

{{readme:Works with}}

## Reasons not to use viv

{{readme:Reasons not to use viv}}

## Is it safe to try

{{readme:Is it safe to try}}

## How compatibility is checked

Every release runs the [compatibility
sweep](https://github.com/svandragt/vivace/blob/main/compat/README.md)
against a mix of pinned popular projects and a random sample of Packagist
packages, comparing viv's `vendor/` and `composer.lock` against Composer's
own. Per-release results live in
[`compat/results/`](https://github.com/svandragt/vivace/blob/main/compat/results);
ongoing sweeps of public projects outside that pinned set are tracked in
[`compat/hunted.md`](https://github.com/svandragt/vivace/blob/main/compat/hunted.md).
The [Compare](compare.html) page turns the newest numbers into a table.

For the curious, here's exactly what a plain install writes, and how a path
renders inside it — the detail the sweep checks byte for byte.

{{doc:docs/composer-contract.md#Files}}

{{doc:docs/composer-contract.md#Path rendering}}

## Versioning

{{doc:docs/stability.md#Versioning}}
