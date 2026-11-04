# .devcontainer/Dockerfile
#
# The only Rust toolchain for Locus (CLAUDE.md, Environment Setup; SPEC §0).
# Covers the Linux build/test gate, the Windows cross-compile and the web
# (wasm32 + trunk) build. macOS is built on a native CI runner instead.
# Compile-only by design: no display server, GPU or X11/Wayland libraries.

# Pinned Debian base so builds are reproducible.
FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive

# ── System packages ───────────────────────────────────────────────────────────
# build-essential, pkg-config : C toolchain for crates with build scripts
# ca-certificates, curl, git  : rustup, tool downloads, cargo git deps
# gcc/g++-mingw-w64-x86-64    : linker for x86_64-pc-windows-gnu, so the container
#                               can emit a Windows .exe to run on the host
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    ca-certificates \
    curl \
    git \
    gcc-mingw-w64-x86-64 \
    g++-mingw-w64-x86-64 \
  && rm -rf /var/lib/apt/lists/*

# ── Rust via rustup ───────────────────────────────────────────────────────────
# Pinned to an explicit release (not `stable`) so the gate is reproducible.
# Bump deliberately; the workspace MSRV lives in Cargo.toml.
ARG RUST_VERSION=1.99.1
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --no-modify-path --profile minimal --default-toolchain ${RUST_VERSION} \
  && rustup component add clippy rustfmt \
  && rustup target add wasm32-unknown-unknown x86_64-pc-windows-gnu \
  && chmod -R a+w /usr/local/rustup /usr/local/cargo

# ── Prebuilt tools, pinned by version and SHA-256 ─────────────────────────────
# Downloading release binaries is much faster than `cargo install`. The hashes
# are the ones published with each release; a mismatch fails the build.
# trunk fetches the wasm-bindgen CLI matching Cargo.lock on first web build.
ARG TRUNK_VERSION=0.22.1
ARG TRUNK_SHA256=a1fe5cd6b053f5a76f66b598d296022a7f06f56def6013cc16cd6897d53f5363
ARG DENY_VERSION=0.20.2
ARG DENY_SHA256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f

RUN set -eux; cd /tmp; \
  curl -sSfLo trunk.tgz "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-musl.tar.gz"; \
  echo "${TRUNK_SHA256}  trunk.tgz" | sha256sum -c -; \
  tar -xzf trunk.tgz -C /usr/local/bin trunk; \
  curl -sSfLo deny.tgz "https://github.com/EmbarkStudios/cargo-deny/releases/download/${DENY_VERSION}/cargo-deny-${DENY_VERSION}-x86_64-unknown-linux-musl.tar.gz"; \
  echo "${DENY_SHA256}  deny.tgz" | sha256sum -c -; \
  tar -xzf deny.tgz --strip-components=1 -C /usr/local/bin "cargo-deny-${DENY_VERSION}-x86_64-unknown-linux-musl/cargo-deny"; \
  rm -f trunk.tgz deny.tgz; \
  trunk --version; cargo-deny --version

# ── Non-root dev user ─────────────────────────────────────────────────────────
# Match USER_UID/USER_GID to the host to avoid permission problems on the
# bind-mounted checkout (Docker Desktop on Windows/macOS: 1000/1000).
ARG USERNAME=dev
ARG USER_UID=1000
ARG USER_GID=1000
RUN groupadd --gid ${USER_GID} ${USERNAME} \
  && useradd --uid ${USER_UID} --gid ${USER_GID} --create-home ${USERNAME}

USER ${USERNAME}
WORKDIR /workspace

# The checkout is bind-mounted at runtime, never copied in. Run with
# `-e CARGO_HOME=/workspace/.cargo-home` so the registry cache persists in the
# git-ignored workspace.
CMD ["bash"]
