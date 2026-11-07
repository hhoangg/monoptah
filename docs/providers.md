# Provider setup

Install and sign in to at least one supported agent before using Monoptah. Monoptah runs the CLIs you install and reuses their existing logins.

Follow each provider’s linked installation guide for platform requirements and installation options. Use the installation and login commands below where provided.

- [Claude Code](https://claude.com/product/claude-code) - `claude auth login`
- [Codex](https://developers.openai.com/codex/cli) - `codex login`
- [Cursor CLI](https://cursor.com/cli) - `agent login`
- [Grok Build](https://docs.x.ai/build/overview) - `curl -fsSL https://x.ai/cli/install.sh | bash` then `grok login`
- [OpenCode](https://opencode.ai) - `opencode auth login`
- [Antigravity](https://antigravity.google/docs/cli-install) (macOS/Linux) - `curl -fsSL https://antigravity.google/cli/install.sh | bash`, then run `agy` once to sign in
- [Pi](https://pi.dev/) - `npm install -g @earendil-works/pi-coding-agent`
- [omp](https://omp.sh) - `curl -fsSL https://omp.sh/install | sh`
- [fx](https://fx.sh) - `curl -fsSL https://fx.sh/setup.sh | bash` then `fx login`
- [Hermes Agent](https://github.com/NousResearch/hermes-agent) - macOS/Linux: `curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash`; Windows PowerShell: `iex (irm https://hermes-agent.nousresearch.com/install.ps1)`; then run `hermes model`
- [Devin CLI](https://docs.devin.ai/cli) - macOS/Linux: `curl -fsSL https://cli.devin.ai/install.sh | bash` (or `brew install --cask devin-cli`); Windows PowerShell: `irm https://static.devin.ai/cli/setup.ps1 | iex`; then run `devin auth login` (Monoptah reuses that login)

One provider is enough. Monoptah probes for each CLI at startup and disables the ones it can’t find, with a hint about how to install them, so a missing Codex doesn’t stop you from working on anything else.

Once a provider is installed and signed in, open a project folder in Monoptah, choose that agent, and send a message. For sessions on another machine, install and authenticate the providers on the host as described in [remote access](remote-access.md).

To run Monoptah from a checkout, see [building from source](building.md). Return to the [README](../README.md) for desktop downloads.

### Native Claude token and API key accounts

In the native app, open **Settings → Providers → Claude → Add account** and enter
an account name. The chevron next to **Sign in and add** picks how the account
authenticates:

- **Sign in with browser** (default): the usual browser login.
- **Use setup-token**: run `claude setup-token` yourself in a terminal, then paste
  only the resulting token into the masked field. Wrapped lines are accepted.
- **Use API key**: paste an Anthropic API key (`sk-ant-api...`).

When editing an account, the same menu starts with **Keep current sign-in**
(rename only). Choosing a token or key replaces the other; **Sign in with
browser** removes any saved token or key first. Changing authentication stops
running turns for that account. The default CLI profile and existing accounts
keep their login.

The secret is stored encrypted (AES-256-GCM) in the app database (the native `.gpui.sqlite3` file beside `monocode.db`), bound to its
account, so a row copied to another account cannot be read. The encryption key is
a separate file, `credentials.key`, in the app data directory. That protects a
copied database, but not someone with access to the whole data directory.
The secret is passed through the
environment of that account's unmodified Claude Code process as
`CLAUDE_CODE_OAUTH_TOKEN` (setup-token) or `ANTHROPIC_API_KEY` (API key), so tools
and hooks that process runs can read it. It is not saved in account or session
settings or sent directly to model or usage APIs. Codespace authentication is
separate. Deleting the account or switching it to browser sign-in deletes the
saved secret. Removing it locally does not revoke it at Anthropic.

"Token configured" and "API key configured" describe local configuration, not
verified identity, validity or expiry. These accounts do not show inferred browser
identity or subscription usage. If Claude Code rejects the credential, generate a
replacement and update the account. If the key file is missing or damaged, Monoptah
asks you to replace the token or API key; it does not fall back to another
account's credentials or plaintext storage.

This is user-controlled credential configuration, not a certification of provider
policy compliance or guaranteed account safety. See the provider's current
[authentication documentation](https://code.claude.com/docs/en/authentication).
