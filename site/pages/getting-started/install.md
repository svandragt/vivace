---
title: Install and upgrade
order: 20
summary: binstall, Homebrew, cargo install, the .deb, the tarball, and upgrading in place.
---

# Install and upgrade

## Try it

{{include:README.md#Try it}}

## Prebuilt binaries and the .deb

Prebuilt binaries, including the `.deb`, are covered above in [Try
it](#try-it): the README lists the exact platforms and architectures
(Linux x86_64 as glibc and static musl builds, aarch64 as static musl,
also packaged as a `.deb`; macOS x86_64 and aarch64), all on the
[releases page](https://github.com/svandragt/vivace/releases).

## No PHP, no problem

Every install method above needs nothing but the `viv` binary itself: no
PHP runtime, no Composer. For a build stage or a runner with neither
installed, see [In a Dockerfile](docker.html) and [In CI](ci.html).
