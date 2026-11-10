# Dev image for Monoptah Native (crates/monocode-core + crates/monocode-gpui) on Linux.
# Built for the host architecture, so the same file serves aarch64 and x86_64 sandboxes.
# Consumers: Docker Sandbox cloud sessions (the app runs `devcontainer up` in the sandbox)
# and Codespaces. See docs/native/cloud-dev-environment.md.
FROM mcr.microsoft.com/devcontainers/base:trixie

# Keep in step with .github/workflows/native.yml (the "native Linux build dependencies" list).
# `gh` is the GitHub CLI; it has no credentials here (see the docs).
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install --no-install-recommends -y \
      build-essential clang cmake ninja-build pkg-config \
      libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-shape0-dev \
      libxcb-xfixes0-dev libxcb-randr0-dev libxkbcommon-dev libxkbcommon-x11-dev \
      libwayland-dev libasound2-dev libx11-xcb-dev libegl1-mesa-dev libvulkan-dev \
      libdbus-1-dev squashfs-tools desktop-file-utils gh \
 && rm -rf /var/lib/apt/lists/*

# Rust: a pinned toolchain, not "stable", so every session builds the same way. Bump
# RUST_VERSION on purpose. rustup-init is checked against the sha256 Rust publishes.
ARG RUST_VERSION=1.99.1
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH}
RUN set -eu; \
    case "$(uname -m)" in \
      x86_64)  triple=x86_64-unknown-linux-gnu ;; \
      aarch64) triple=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;; \
    esac; \
    url="https://static.rust-lang.org/rustup/dist/$triple/rustup-init"; \
    curl --proto '=https' --tlsv1.2 -fsSL "$url" -o /tmp/rustup-init; \
    expected=$(curl --proto '=https' --tlsv1.2 -fsSL "$url.sha256" | cut -d' ' -f1); \
    echo "$expected  /tmp/rustup-init" | sha256sum -c -; \
    chmod +x /tmp/rustup-init; \
    /tmp/rustup-init -y --no-modify-path --profile minimal \
      --default-toolchain "$RUST_VERSION" --component rustfmt --component clippy; \
    rm /tmp/rustup-init; \
    chown -R vscode:vscode "$RUSTUP_HOME" "$CARGO_HOME"; \
    rustc --version && cargo fmt --version && cargo clippy --version
