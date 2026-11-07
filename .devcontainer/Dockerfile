# Monoptah dev container image (used by GitHub Codespaces).
FROM mcr.microsoft.com/devcontainers/base:bookworm

ARG NODE_MAJOR=22
# Empty means the latest gh from the official apt repo. Set e.g. "2.60.0" to pin.
ARG GH_VERSION=

# WHY Node and gh come from apt and not from the devcontainer node/github-cli features:
# the Monoptah app reaches the codespace over `gh codespace ssh` and runs its bootstrap
# in a login shell (`sh -l -s`). An sshd session does not get the devcontainer
# containerEnv PATH, and Debian's /etc/profile rebuilds PATH, so every tool must live
# in /usr/bin or /usr/local/bin. The node feature uses nvm under /usr/local/share/nvm,
# which is off that PATH. With NodeSource node in /usr/bin, the claude-code feature's
# `npm install -g` lands `claude` in /usr/bin too.

# Keep the GPUI build dependency list (second group) in step with the desktop job in
# .github/workflows/native.yml.
RUN export DEBIAN_FRONTEND=noninteractive \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        chromium chromium-driver build-essential pkg-config libssl-dev jq curl unzip tmux ripgrep ca-certificates gnupg \
        clang cmake ninja-build pkg-config libfontconfig1-dev libfreetype6-dev libxcb1-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-randr0-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libasound2-dev libx11-xcb-dev libegl1-mesa-dev libvulkan-dev libdbus-1-dev squashfs-tools desktop-file-utils \
    # Node from the official NodeSource apt repo.
    && mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
        | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_${NODE_MAJOR}.x nodistro main" \
        > /etc/apt/sources.list.d/nodesource.list \
    # GitHub CLI from the official apt repo.
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends nodejs \
    && if [ -n "${GH_VERSION}" ]; then apt-get install -y --no-install-recommends "gh=${GH_VERSION}"; \
       else apt-get install -y --no-install-recommends gh; fi \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# The rust feature only sets CARGO_HOME, RUSTUP_HOME and PATH through containerEnv, which
# ssh login shells do not see. Export them for login shells (sourced by `sh -l`, so POSIX sh).
RUN cat > /etc/profile.d/10-rust-path.sh <<'PROFILE' \
    && chmod 644 /etc/profile.d/10-rust-path.sh
# Make the rust toolchain visible to login shells.
export CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
if [ -d /usr/local/cargo/bin ]; then
  case ":${PATH}:" in
    *:/usr/local/cargo/bin:*) ;;
    *) PATH="/usr/local/cargo/bin:${PATH}"; export PATH ;;
  esac
fi
PROFILE

# Codespaces injects user secrets (CLAUDE_CODE_OAUTH_TOKEN) into VS Code sessions, and
# writes them base64-encoded to /workspaces/.codespaces/shared/.env-secrets. The current
# sshd feature no longer restores them for ssh logins, so the app's `sh -l` bootstrap
# would start the host without the token. Export them for login shells, never overriding
# a variable that is already set. A line that does not decode is skipped.
RUN cat > /etc/profile.d/05-codespaces-secrets.sh <<'PROFILE' \
    && chmod 644 /etc/profile.d/05-codespaces-secrets.sh
# Export Codespaces user secrets for ssh login shells.
if [ -r /workspaces/.codespaces/shared/.env-secrets ]; then
  while IFS= read -r _l || [ -n "$_l" ]; do
    _k=${_l%%=*}
    case "$_k" in ''|[0-9]*|*[!A-Za-z0-9_]*) continue ;; esac
    eval "_set=\${$_k+x}"
    [ -z "$_set" ] || continue
    _v=$(printf '%s' "${_l#*=}" | base64 -d 2>/dev/null) || continue
    export "$_k=$_v"
  done < /workspaces/.codespaces/shared/.env-secrets
  unset _l _k _v _set
fi
PROFILE
