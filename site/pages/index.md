# viv

`viv` is to Composer what uv was to pip: a Rust reimplementation that
installs from `composer.lock` and writes the `vendor/` directory Composer
would write, byte for byte.

## Try it

```sh
cargo binstall --git https://github.com/svandragt/vivace vivace
viv install     # in a project with composer.json and composer.lock
```

That also installs a `composer` shim next to `viv`; put it first on `PATH`
and your existing scripts run through viv unedited. Prebuilt binaries and
build-from-source instructions are on the [releases
page](https://github.com/svandragt/vivace/releases).

## The numbers

A cold `laravel/laravel` install (109 packages, no cache, no `vendor/`)
takes **{{viv_cold}}** with viv against Composer's **{{composer_cold}}**.
<span class="source">source: {{viv_cold_source}}</span>

The compatibility sweep finds a byte-identical `vendor/` on **{{compat_identical}}**
pinned projects viv installs (Laravel, Symfony, Drupal, WordPress Bedrock and
more).
<span class="source">source: {{compat_source}}</span>

Plugins other than the shipped adapters and Composer's long tail of commands
stay with Composer; see [`docs/stability.md`](https://github.com/svandragt/vivace/blob/main/docs/stability.md)
for exactly what is and isn't covered.

{{posts}}
