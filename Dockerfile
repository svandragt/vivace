# The published image (#213): viv and its `composer` shim on the PATH of a
# base small enough that a vendor stage costs less than the `composer:2`
# image it replaces. Built from the musl tarballs `release.yml` already
# produces, so this is a packaging step, not a second build.
#
# Not `FROM scratch`, despite the binary being static musl: viv links
# `rustls-platform-verifier` with `rustls-native-certs` and no
# `webpki-roots`, so it reads the *system* CA store and every network
# command fails with "No CA certificates were loaded from the system" on a
# base that has none. Static linking removes the libc dependency, not the
# need for trust anchors. `distroless/static` ships `ca-certificates` and
# keeps them current upstream.
FROM gcr.io/distroless/static-debian12:latest

# `buildx` sets this per platform; `dist/<arch>/` is laid out to match by
# the workflow that builds this.
ARG TARGETARCH

COPY dist/${TARGETARCH}/viv dist/${TARGETARCH}/composer /usr/local/bin/

# viv resolves its store under `$XDG_CACHE_HOME`, falling back to `$HOME`,
# and distroless sets neither — without this every install stops with
# "HOME is not set; pass --cache-dir". A path of its own rather than a
# pretend home directory, so a BuildKit cache mount has an obvious target.
ENV XDG_CACHE_HOME=/var/cache

WORKDIR /app

# distroless has no shell, so a consuming Dockerfile must use `RUN` in exec
# form: `RUN ["viv", "install", "--no-dev"]`, not `RUN viv install --no-dev`.
ENTRYPOINT ["/usr/local/bin/viv"]
