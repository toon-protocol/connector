# =============================================================================
# connector-rust — container image for the Rust connector (issue #429)
#
# DELIBERATELY A SEPARATE IMAGE from the published `connector` one. Per ADR 0013
# the cutover runs the Rust fleet in PARALLEL under its own address prefix, with
# both networks live at once — so the Rust binary must not be published under the
# TypeScript node's name and tag series. `connector:3.44.0` means "TS node, YAML/
# CONFIG_FILE"; this takes a TOML path as argv. Swapping what a tag means in
# place is the one shape that cutover design rules out.
#
# Published to GHCR as ghcr.io/toon-protocol/connector (rust-* tags) by
# .github/workflows/publish-connector-rust-image.yml (issue #487), tagged by
# content (`sha-<short-sha>`) rather than by version — no crate under crates/
# has a release process yet (every Cargo.toml is still 0.1.0), so a semver tag
# here would assert a stability contract this binary hasn't earned. `latest`
# on `ghcr.io/toon-protocol/connector` is untouched and stays frozen on the
# last TypeScript build.
#
# The base is rust:1.90 (matching CI's stable toolchain), NOT the 1.82 this file
# originally pinned: 1.82 cannot build the locked graph at all — idna_adapter
# 1.2.2 requires the edition2024 Cargo feature. Nothing in CI ever built this
# file, so that was undetected until #465.
#
# One static binary, one config file, no JavaScript runtime anywhere in the
# final image -- contrast the TypeScript connector's root ./Dockerfile, which
# pinned Node >= 22.12 solely for Mina/o1js's ESM packaging (a dependency
# ADR 0002 dropped from this rewrite and ADR 0065 removed from the repository).
#
# Build (single platform, from the repository root so the build context
# includes crates/):
#   docker build -f deploy/connector-rust/Dockerfile -t connector-rust .
#
# Build for both target platforms (per this ticket's AC):
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     -f deploy/connector-rust/Dockerfile -t connector-rust .
#
# `cargo build` compiles natively for whichever architecture buildx/QEMU
# presents as the container's own -- there is no cross-compilation step and
# no BUILDPLATFORM pinning to work around here.
#
# Dependency layer (issue #609): the builder is split into `chef` / `planner`
# / `builder` stages using cargo-chef, so a source-only commit reuses the
# compiled dependency graph instead of recompiling it. The per-stage comments
# below carry the detail -- how the recipe keys the cache, and what makes an
# unresolvable manifest fail the build rather than silently rebuild everything.
# =============================================================================

# ============================================
# Stage 1a: chef -- base image with cargo-chef installed. No source of any
# kind is copied in here, so this stage's own layers are as stable as the
# rust:1.90-alpine3.22 tag itself and are never invalidated by a source or
# manifest edit.
# ============================================
FROM rust:1.90-alpine3.22 AS chef

RUN apk add --no-cache musl-dev
RUN cargo install cargo-chef --locked

WORKDIR /workspace

# ============================================
# Stage 1b: planner -- computes the recipe (issue #609): every workspace
# manifest plus the locked dependency graph, and nothing else. A commit
# that touches only crate *sources* leaves this byte-identical, which is
# what makes the cook layer below reusable.
#
# Deliberately NOT `cargo chef prepare --bin connector`. Narrowing the
# recipe to one binary's closure also prunes the workspace members that
# closure does not reach, and the `Cargo.lock` chef writes beside that
# reduced workspace no longer matches it -- cargo re-resolves (against this
# repo's lock it drops a second `rand_core` and a second `webpki-roots`)
# and then refuses: `error: cannot update the lock file ... because
# --locked was passed`, aborting the build at the cook step below.
# `--locked` is an acceptance criterion of this issue, so the recipe stays
# whole. Nothing is lost by that: `-p connector` on the cook is what
# actually narrows *what gets compiled*, exactly as it narrows the real
# build, and a whole-workspace recipe still only changes when a manifest
# or the lock file does.
# ============================================
FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo chef prepare --recipe-path recipe.json

# ============================================
# Stage 1c: builder -- `cook` builds every third-party dependency the recipe
# names against stub sources, keyed only on the recipe's content. A commit
# that touches `crates/connector-runtime/src/**` (or any other crate source)
# without changing a `Cargo.toml`/`Cargo.lock` produces a byte-identical
# recipe, so this RUN layer -- and everything `ethers`, `solana-*`, `axum`
# and `tokio` cost to compile -- is reused from `cache-from: type=gha`
# instead of rebuilt. Real sources are copied in and built only after: the
# thing that changes on every commit is also the only thing that recompiles
# on every commit.
#
# A crate the workspace cannot resolve -- one added under `crates/` without
# a Cargo.toml, or with a malformed one -- fails the build loudly at the
# planner's `cargo chef prepare`, which cannot run `cargo metadata` over it.
# `--locked` does the same for a crate added without `Cargo.lock` being
# regenerated: cargo refuses rather than silently re-resolving. Neither
# degrades quietly into a full rebuild.
# ============================================
FROM chef AS builder

COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release --locked -p connector --recipe-path recipe.json

# The workspace root and every crate the connector binary's dependency
# closure can reach. Issue #1386 removed `packages/solana-program` (the
# `payment-channel` crate) from the workspace entirely, so it is no longer a
# member whose manifest Cargo needs present to resolve the graph -- `crates/`
# is now the whole workspace.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked -p connector

# ============================================
# Stage 2: runtime -- no JavaScript runtime, no build toolchain
# ============================================
FROM alpine:3.22 AS runtime

# ca-certificates: the connector's HTTP app client (reqwest, rustls-tls)
# verifies TLS against the system root store when an app's handler_url is
# https.
RUN apk add --no-cache ca-certificates && \
    adduser -D -u 10001 connector && \
    # /app/state must exist in the image, owned by the runtime user, BEFORE
    # any volume is mounted over it: a named volume mounted on a path the
    # image does not provide is created root-owned, uid 10001 cannot write
    # it, and the connector refuses to start (state_dir fails closed, issue
    # #605). Providing the path here is what makes a fresh named volume
    # inherit 10001 ownership on first mount.
    mkdir -p /app/state && chown connector:connector /app/state

COPY --from=builder /workspace/target/release/connector /usr/local/bin/connector
# `stub-app` is the connector package's second [[bin]], so `-p connector`
# above already built it -- this copies it rather than compiling anything new.
# It is the payment-oblivious app behind a route in `local/`'s topologies: a
# connector needs *something* to terminate to, and shipping the trivial one in
# the same image is what keeps those topologies from needing an app repo. It
# holds no secret, performs no cryptography and answers `POST /` (see its
# module header), so nothing about it is reachable unless an operator points a
# route at it deliberately.
COPY --from=builder /workspace/target/release/stub-app /usr/local/bin/stub-app

USER connector
ENTRYPOINT ["/usr/local/bin/connector"]
CMD ["/app/config/connector.toml"]
