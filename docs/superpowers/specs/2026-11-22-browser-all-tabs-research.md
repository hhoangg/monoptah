# Mono Browser: agent access to all tabs of a profile - research

Status: research, no code. Date: 2026-11-22. Builds on `2026-11-21-mono-browser-design.md`
(shipped in #198 as the per-Mono Browser tab, `crates/monocode-core/src/mono/browser.rs`).

## Problem

The per-Mono Browser runs `@playwright/mcp@0.0.86 --extension` against the Playwright MCP Bridge
extension (id `mmlmfjhmonkocbjadbfplnigmagldckm`). The extension makes a tab group
`Playwright · <client>` per connection, and the agent sees and controls only tabs in that group.
Tabs the user opened by hand are invisible. Wanted: the agent can read (optionally act on) any tab
of the chosen profile.

Sources were downloaded into `/tmp/brres/*` (untrusted, read only). Line numbers below refer to:

- `PW` = `microsoft/playwright` @ `049226728d2d` (2026-11-20), path `packages/extension/src/` unless
  noted. The extension lives in the playwright repo now, not in `microsoft/playwright-mcp`.
- `CORE` = `packages/playwright-core/src/tools/mcp/` in the same checkout.
- `CU` = `leeguooooo/chrome-use` @ main (2026-11-20, release v1.5.295).

## Q1. Playwright MCP Bridge

### Version comparison

`@playwright/mcp` latest on npm is `0.0.86`, the version Monoptah pins, so there is no newer MCP
server to try (`npm view @playwright/mcp version`). It depends on
`playwright-core 1.66.0-alpha-2026-11-18`; that tarball contains the `--extension`, `--no-tab-group`
and `PLAYWRIGHT_MCP_EXTENSION_TOKEN` code (`lib/coreBundle.js`), matching `CORE` below. The
extension itself ships through the Web Store, so its installed version can differ from repo HEAD
(`packages/extension/manifest.json:4` says `0.6.0`). The behavior below is the documented design
of that HEAD (`packages/extension/design.md`).

### What limits visibility: the owner table, not tab groups alone

The design has one central rule: each tab has zero or one owner (`design.md:5`), and "a session can
send commands only to tabs that it owns" (`design.md:16-20`).

- `TabRegistry` is the only owner table (`tabRegistry.ts:25`). A tab becomes owned only by `claim`
  (`tabRegistry.ts:75-86`): picked on the connect page, dragged into the client's group, created
  by the client (`chrome.tabs.create`, `clientSession.ts:84-91`), or opened as a popup by an owned
  tab (`tabRegistry.ts:106-110`).
- Every command is checked against ownership. `chrome.debugger.attach/detach/sendCommand` and
  `chrome.tabs.remove` go through `_ownedTab` and fail with `Tab N is not owned by this client`
  (`clientSession.ts:102-108`). A non-owned or non-existent tab gives the same text, so a client
  cannot even probe tab ids (`design.md` "Protocol").
- Events (`chrome.tabs.onCreated`, `onRemoved`, debugger events) are sent only for owned tabs
  (`clientSession.ts:48-67`, `ownedTab.ts:77-80`). The MCP server therefore never learns that other
  tabs exist; `browser_tabs` can only list what it was told about.
- `chrome.debugger.attach` is per tab (`ownedTab.ts:43-50`). There is no attach-all or list-all
  message; the whole protocol is the five commands in `clientSession.ts:70-98`.
- Browser-wide CDP methods are refused: `Target.attachToTarget`, `Target.createTarget`,
  `Target.closeTarget`, `Target.activateTarget`, cookie and permission writes (`cdpPolicy.ts:18-31`).
  `Target.getTargets` style discovery cannot bypass the owner table because `attach` demands
  ownership.
- URLs `chrome:`, `edge:`, `devtools:` are never announced to the client (`ownedTab.ts:20-26`,
  `ownedTab.ts:103-108`; also filtered from the picker, `tabPicker.ts:45`).

Reading is not isolated at the browser level: `design.md:22` says a client can read browser-level
data such as the cookies of the full profile (`Storage.getCookies` is not refused). That is a data
exposure, not tab access, but it matters for the safety model (below).

### Any option, flag, env, toggle or message to share existing tabs?

No flag makes the agent see all tabs. Checked in `CORE` and the 0.0.86 README:

| Knob | Effect | Helps? |
|---|---|---|
| `--no-tab-group` / `PLAYWRIGHT_MCP_NO_TAB_GROUP` (`CORE/config.ts:511-513`, header `x-playwright-extension-tab-group: false` at `extensionContextFactory.ts:36`) | The client has no Chrome group; ownership is unchanged | No. Owned tabs just are not grouped, and the user then has no drag target |
| `--session-name`, `--session-color` | Title and color of the client group (`tabGroupManager.ts:87-90`) | Only cosmetic |
| `--background` | Do not activate tabs or focus the window (`connectionBroker.ts:114-123`) | No |
| `PLAYWRIGHT_MCP_EXTENSION_TOKEN` | Skips the approval dialog, connects with an empty selection (`ui/connect.tsx:107-108`: `connect({})`), so the client gets only a fresh blank tab (`tabPicker.ts:56-70`) | This is the token flow Monoptah uses; it can never preselect tabs |
| No token (interactive) | The connect page shows a picker of free tabs and free groups (`tabPicker.ts:34-46`, `ui/connect.tsx:144-186`); the user ticks tabs or one group and clicks Allow (`Selection = {groupId?, tabIds?}`, `types.ts` `Selection`) | Yes, but manual on every connection, and the server waits with no timeout (`extensionContextFactory.ts:44`) |

The relevant protocol messages between the connect page and the service worker are only
`connectionRequested`, `getCandidates`, `connect`, `keepalive`, `getStatus`, `disconnect`
(`types.ts` `PageMessage`, `background.ts:100-112`). `connect` takes the `Selection` above, and the
selection is chosen by the human on the page, never by the MCP server.

### Does dragging a tab into the agent group make it controllable?

Yes. This is the documented path: "You can drag more tabs into the tab group of the client later"
(`ui/connect.tsx:147`). Mechanism: `TabGroupManager._onTabChangedGroup` runs on every
`chrome.tabs.onUpdated` with a `groupId` change (`tabGroupManager.ts:70-73`, `132-147`). If the tab
now sits in a group owned by a session, the registry `transfer`s it to that session
(`tabGroupManager.ts:143-144`, `tabRegistry.ts:97-100`), which fires `claim` and then
`chrome.tabs.onCreated` to the client (`ownedTab.ts:102-108`), so the MCP server sees the tab and
can attach. Dragging a tab out releases it (`tabGroupManager.ts:145-146`). Ownership by another
client and "connect page" tabs are refused (`tabRegistry.ts:80-83`). Tabs on `chrome:` etc. stay
unannounced.

So the only Bridge-based "share a tab" action is a user gesture per tab (drag, or tick on the
picker). That fits "a tab I want the Mono to see", not "the agent may look at anything".

### Verdict for Q1

Not achievable by configuration. All-tab visibility needs a change in the extension (add an
"adopt any tab" command and drop the owner check for read commands). The extension is Apache-2.0
and in the playwright monorepo, but forking it means owning a Web Store listing or an unpacked
developer-mode install, and a custom token flow, which loses the "install from the Web Store" step
of the current UX. Two issues about the group label exist upstream (microsoft/playwright #43823,
#44118, closed), none about sharing all tabs. `gh search` of `microsoft/playwright-mcp` found no
open request.

## Q2. Alternatives

### leeguooooo/chrome-use (`CU`)

- Apache-2.0, fork of `vercel-labs/agent-browser` but "now a standalone project"
  (`README.md` header). 262 stars, created 2026-01-19, last push
  2026-11-20, latest release `v1.5.295` (2026-11-20). One main author, very high release cadence.
- Size: about 291k lines of Rust and about 17k lines of extension JavaScript
  (`extensions/ab-connect`, manifest version 0.5.108). It is a product (side panel, scheduling,
  site adapters, stealth, mobile), not a small bridge.
- Connection: CLI/daemon <-> Chrome native messaging host <-> `ab-connect` extension using
  `chrome.debugger` (README "How it works"; no network port, no token). Dev id
  `ciiljdlhdpfckdcfkphgmfalanpdejep`, Web Store id `knfcmbamhjmaonkfnjhldjedeobeafmk`
  (`cli/src/connect.rs:35,40`). Extension permissions include `debugger`, `tabs`, `tabGroups`,
  `nativeMessaging`, `scripting`, `downloads`, optional host `http(s)://*/*`
  (`extensions/ab-connect/manifest.json`). It also ships an MCP server: `chrome-use mcp`
  (`README.md` "Use from an MCP client", tools `chrome_use_*` in `cli/src/mcp.rs:40-75`).
- All tabs: yes. It is session-grouped by default (one colored group per `--session`), but it can
  list and reach everything in the profile:
  - `chrome-use tab list --all [--limit n]`: "Every tab in the connected Chrome profile, across
    windows, the user's own and other sessions' included ... Read-only even on the first call"
    (`README.md:440`), also MCP `chrome_use_tabs` action `list` with `all` (`cli/src/mcp.rs:646-647`).
  - `tab adopt <url-substring|targetId>`: attach an already-open tab without navigating it
    (`README.md:438`).
  - `--tab <handle> --force <command>`: act on a tab the session does not own, "only when the user
    asked" (`README.md:441`; the `force` property is on every tab-taking MCP tool,
    `cli/src/mcp.rs:442` and following).
  - Pages that cannot be lent: non-http(s) and Web Store (`extensions/ab-connect/tab-grant.js:27-36`).
- Token efficiency: snapshots are accessibility trees with refs (`@e2`), `snapshot -i -c` for
  interactive and compact output, `--max-bytes N --from K` paging, `--diff`, scoped `-s` reads,
  hidden text dropped by default (`docs/en/reading.html` lines 141-221; `README.md:674`). Its own
  docs warn "Byte counts are not token counts; measure each separately", so measure before choosing.
- Safety: content boundaries with a per-call nonce fence untrusted page text in MCP
  (`README.md:675`); ask-first on banking, brokerage, crypto and gov-ID sites from
  `cli/src/native/sensitive_sites.json`, plus payment fields and purchases (`README.md:729`);
  `--action-policy policy.json` with `deny`, `sites.allow/deny/confirm`
  (`docs/en/permissions.html`); enterprise `chrome.storage.managed` keys `allowedDomains`,
  `blockedDomains`, `confirmDomains`; a 200-event permission history on the options page.
  Gaps for Monoptah: no verified "read-only by default" switch (a `deny` list in the action policy
  is the closest; unverified which command names it matches), and no per-Mono log that Monoptah
  can query (timing.jsonl and the permission history are local to chrome-use).
- Risks: the extension is not Microsoft's; one maintainer; large attack surface (stealth,
  autologin, cookie export, Bitwarden hooks, `identity.email` permission); README contains
  advertising and paid-setup sections. Review before any recommendation to users.

### dylansantwani/openbrowser

MIT, 0 stars, last commit 2026-09-29, unpacked developer-mode extension plus a zero-dependency
Node MCP server over `ws://127.0.0.1:8848` (`README.md`, `mcp-server/src`). Permissions include
`<all_urls>`, `cookies`, `debugger`. The README says an agent "owns only the tabs in its own tab
group" (README line 329 area), and the router queries all normal windows for automatable tabs
(`extension/background/router.js:124`) with a `tabId` on every tool. No evidence of an audit log
or read-only mode. Too young to depend on; useful only as a reference for the compact tree.

### dbalders/agent-browser-extension

Apache-2.0, 0 stars, last commit 2026-10-27. Unpacked extension
(`permissions: tabs, tabGroups, debugger, storage, alarms, downloads, webNavigation`, host only
`http://127.0.0.1/*`, `extension/manifest.json:7-8`) plus a local TypeScript bridge (`src/bridge.ts`)
reached by WebSocket on loopback with a pairing token, not native messaging. Design fits Monoptah
best on paper: "Agents discover existing tabs, claim the ones relevant to their task, or create
background tabs in their own named groups. There is no required tab picker" (`README.md`),
`browser_tabs {query}` lists existing tabs (`docs/capabilities.md:12`), per-origin website
approvals in the extension (ask first, allow for task, always, block, revoke), `SITE_ACCESS_REQUIRED`
errors, and a `SECURITY.md` that admits that tab discovery still returns titles and URLs.
Project status: pre-release, no store listing, 39 tools, no users yet. High risk as a dependency;
good as a design reference or as a fork base because it is small and TypeScript.

### Comparison

| | Playwright MCP Bridge | chrome-use | openbrowser | agent-browser-extension |
|---|---|---|---|---|
| List all tabs | No | Yes (`tab list --all`) | Yes (router) | Yes (`browser_tabs`) |
| Read non-owned tab | No | Yes (`tab adopt`, `--force`) | Unclear | Yes (claim) |
| Act on non-owned tab | No | Yes (`--force`) | Unclear | Yes (claim) |
| Transport | WebSocket relay, token | Native messaging | WebSocket 8848 | WebSocket + pairing code |
| Store listing | Yes (Microsoft) | Yes (id above) | No | No |
| Origin allow/deny | `--allowed/--blocked-origins` (not a security boundary per README) | action policy, sites, managed policy | allow/blocklist | per-origin approvals |
| License | Apache-2.0 | Apache-2.0 | MIT | Apache-2.0 |
| Activity | Microsoft, daily | 1 maintainer, daily | last commit 2026-09-29 | last commit 2026-10-27 |
| Stars | n/a | 262 | 0 | 0 |

## Q3. Chrome constraints

### chrome.debugger on arbitrary tabs

- Needs the `debugger` permission (https://developer.chrome.com/docs/extensions/reference/api/debugger).
- Chrome shows the "started debugging this browser" infobar on attached tabs. The reference page
  itself does not mention it; evidence is the products that fight it: chrome-use documents the bar
  and a `--silent-debugger-extension-api` restart to hide it (`CU README.md:285`), and Playwright
  sets a green check badge instead (`ownedTab.ts:134-145`). Treat the bar as unavoidable for a
  store extension.
- Restricted targets: `chrome://`, `devtools://` and `edge://` pages are not debuggable
  (`ownedTab.ts:20-26`); the Chrome Web Store is refused by Chrome ("Chrome lets no extension drive
  the Web Store", `CU tab-grant.js:32`; "Chrome detaches debuggers on chromewebstore pages",
  `CU cli/src/connect.rs:3104-3108`). Other extensions' pages need
  `--silent-debugger-extension-api` (debugger reference). Enterprise policy can block hosts:
  "This page cannot be scripted due to an ExtensionsSettings policy"
  (https://developer.chrome.com/blog/debugger-enterprise-policy-restrictions). Since Chrome 86,
  `attach` checks the last committed URL, so a brand new tab can fail until it commits
  (https://groups.google.com/a/chromium.org/g/chromium-extensions/c/avq_G6bVmaE).
- Per tab only one debugger client: if DevTools or another tool is attached, attach fails, and a
  detach from the user closing the bar must be handled (`ownedTab.ts:82-88`).

### Remote debugging port on the default profile

Confirmed (https://developer.chrome.com/blog/remote-debugging-port): from Chrome 136 "These
switches will no longer be respected if attempting to debug the default Chrome data directory",
and they "must now be accompanied by the `--user-data-dir` switch to point to a non-standard
directory", because a non-default directory uses a different encryption key. This applies to
`--remote-debugging-port` and `--remote-debugging-pipe`. So "attach to my real profile over CDP
port" is closed; an extension (debugger API) or a separate user-data-dir is the only way. A
separate directory means a copy of logins is not available, which defeats the goal.

### Native messaging host manifest locations on macOS

Rule: user-level hosts live in `NativeMessagingHosts/` under each browser's user data directory.
System-level paths are fixed per vendor.

| Browser | User-level manifest dir | Source |
|---|---|---|
| Chrome | `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/` | developer.chrome.com native-messaging |
| Chrome Beta / Canary | `~/Library/Application Support/Google/Chrome Beta/...`, `.../Chrome Canary/...` | by the user-data-dir rule; also chrome-use `connect.rs:1727-1730` |
| Chrome (system) | `/Library/Google/Chrome/NativeMessagingHosts/` | developer.chrome.com |
| Chromium | `~/Library/Application Support/Chromium/NativeMessagingHosts/` (system `/Library/Application Support/Chromium/NativeMessagingHosts/`) | developer.chrome.com |
| Edge | `~/Library/Application Support/Microsoft Edge/NativeMessagingHosts/` (`Microsoft Edge Beta/Dev/Canary` for other channels; system `/Library/Microsoft/Edge/NativeMessagingHosts/`) | learn.microsoft.com Edge native messaging |
| Brave | `~/Library/Application Support/BraveSoftware/Brave-Browser/NativeMessagingHosts/` | chrome-use `connect.rs:1733`, not an official doc |
| Arc | `~/Library/Application Support/Arc/User Data/NativeMessagingHosts/` | only a third-party crate (victauri-browser installer); unverified, test on a real Arc |

The manifest needs an absolute `path`, `type: stdio` and an `allowed_origins` list of exact
`chrome-extension://<id>/` entries, no wildcards. chrome-use writes the same manifest into Chrome,
Beta, Canary, Chromium, Edge and Brave (`connect.rs:1392-1404`, `1718-1740`) and not Arc. Chrome
for Testing uses its own paths from Chrome 146.

## Recommendation

### (a) With the Playwright MCP Bridge only: partial, user-driven

No config reaches "any tab". What works today, with no code in the extension:

1. Keep token mode for the agent's own tabs.
2. Tell the user to drag a tab into the group named after the Mono to share it. Name the group
   with `--session-name "<Mono name>"` and a fixed `--session-color` (both exist in 0.0.86) so the
   target is recognizable, instead of `Playwright · <client>`. This is a one-line change in
   `ServerSpec::new` (`crates/monocode-core/src/mono/browser.rs`) plus a hint in the Browser tab.
3. Optional: a "Share tabs" test button that starts the server without a token so the connect page
   picker opens. It blocks until the user clicks Allow, so it is a setup aid, not a runtime mode.

This satisfies "the user chooses which tabs", not "the agent can look at any tab". Keep it as the
default and as the safe fallback.

### (b) For true all-tab access: reuse chrome-use as an optional second backend, do not fork it first

Reasons: it already has `tab list --all` (read only), `tab adopt` and `--force` (`CU README.md:438-441`),
native messaging with no token or port, an MCP server, a compact snapshot, and ask-first safety.
A fork carries about 291k lines of Rust plus a 17k-line extension; Monoptah cannot maintain that,
and the extension has no Monoptah-owned store id (a fork needs its own listing and its own
`allowed_origins` id, or a developer-mode install).

Plan, as an opt-in "All tabs" mode of the existing Browser tab:

1. Backend choice per Mono: `Bridge` (today) or `ChromeUse`. New field on `MonoBrowser`
   (`mono/browser.rs:102`): `backend`, serde default `Bridge`, so old rosters load.
2. Same profile picker (`profile_dir`) and prerequisites; chrome-use then needs its extension in
   that profile (Web Store id `knfcmbamhjmaonkfnjhldjedeobeafmk`, same install flow as
   `install_url`) and the native host registered by `chrome-use extension install`. Monoptah must
   not write Chrome's `NativeMessagingHosts` itself in v1; shell out to the CLI and show its result.
3. MCP config: add a `chrome_use` server (`chrome-use mcp`, pinned release, sha256 check on the
   installer) to the same per-Mono `--mcp-config` for Claude and to the `-c mcp_servers...`
   overrides for Codex (the paths in `provider.rs:1485` and `:1550`). No token is needed, which
   removes the `PLAYWRIGHT_MCP_EXTENSION_TOKEN` env step for this backend.
4. Profile targeting: chrome-use binds a profile per session; confirm in a spike that
   `chrome-use` can be pinned to one profile when several are open (README documents profile
   rules and "never opens the site in a different profile", line 308 area).
5. Policy mapping from existing Mono settings: `blocked_origins` and `allowed_origins` to an
   `--action-policy` file (`sites.deny`, `sites.allow`) written under the Mono's output dir;
   `confirm_actions` to `sites.ask` plus the sensitive-site default; read-only default through the
   same `READ_TOOLS` / `WRITE_TOOLS` split in `browser.rs:42-66` (map to `chrome_use_snapshot`,
   `chrome_use_read`, `chrome_use_tabs` list, `chrome_use_screenshot`; ask for the rest, never
   offer `chrome_use_eval`, `chrome_use_network_route`, `chrome_use_download*`, `chrome_use_upload`,
   `chrome_use_skills`, `chrome_use_schedules`, matching `BLOCKED_TOOLS`).
6. Audit: reuse `audit_entry` (`browser.rs:696`) on the `chrome_use_*` tool-call events, with the
   tab URL from the tool result when `force` or `adopt` is used. `--force` calls must be logged
   with `forced: true`.
7. Prompt block: "read the tabs the user asks about; `tab list --all` first; adopt, do not force,
   unless the user named the tab; never act on a tab the user did not mention; save big snapshots
   to the output dir".
8. Compact extraction: use `snapshot -i -c --max-bytes`, and measure tokens on three real pages
   against Playwright's snapshot before choosing the default.

Effort (one engineer, native only):

| Step | Estimate |
|---|---|
| Spike: install chrome-use in the dev profile, verify profile pinning, `tab list --all`, MCP under Claude and Codex, action-policy deny | 1 day |
| `backend` field, settings UI toggle, prerequisites for chrome-use, install flow | 2 to 3 days |
| MCP config for Claude and Codex, policy file generation, tool allow/deny mapping | 2 days |
| Audit, prompt block, tests, docs | 2 days |
| Review, security review of the policy mapping, Arc/Brave path checks | 1 day |
| Total | about 8 to 9 days |

### (c) If chrome-use is rejected on trust grounds

Fork `dbalders/agent-browser-extension` (small, TypeScript, Apache-2.0, claim-based model, per-origin
approvals) and add: read-only default (commands that mutate return `READ_ONLY`), an
`allowedOrigins` list pushed from Monoptah on pairing, an append-only audit file, and a compact
page extractor. Cost: about 4 to 6 weeks including a Web Store submission or a developer-mode
install story, plus owning the security surface. Not recommended before (b) is tried.

### Decision

Ship (a) now as a small improvement (named group, hint to drag tabs). Run the one-day chrome-use spike
(b.1). If it passes, add chrome-use as an optional backend with read-only and origin policy
defaults. Do not fork the Playwright extension or chrome-use up front.

## Open items

- Confirm the installed Web Store version of the Playwright MCP Bridge matches repo HEAD
  (`0.6.0`); behavior was read from source, not exercised on this machine.
- Verify chrome-use `--action-policy` `deny` command names and a read-only mode in the spike.
- Verify Arc's native messaging directory on a real install.
- Snapshot token comparison was not measured; only the documented options were read.
- Privacy and trust review of the chrome-use extension (permissions `identity.email`, `downloads`,
  `scripting`, optional all-sites host access) before shipping it to users.

## Addendum: one stable tab group per Mono (long-lived HTTP server)

Bug: every Mono turn made a new Chrome tab group with the same name. Each turn starts a new agent
process, hence a new stdio `@playwright/mcp`, hence a new extension connection.

### Verified facts

Sources: `@playwright/mcp@0.0.86` and `playwright-core@1.66.0-alpha-2026-11-18` tarballs
(`lib/coreBundle.js`), and the extension in `microsoft/playwright` `packages/extension/src/`.

- The extension makes one group per connection and never reuses one. `TabGroupManager.register`
  creates a `ClientTabGroup` per `ClientSession`; when the session closes, `orphan(session)` greys
  the group out and forgets it (`tabGroupManager.ts`, `connectionBroker.ts` `session.onClose`). A
  reconnect with the same title makes a new group. The only way to join an existing group is the
  manual `adopt` from the connect page, which the token flow skips. So reconnects must be rare:
  the connection has to outlive the turns.
- `@playwright/mcp` flags (the `--help` text in `coreBundle.js`, `decorateMCPCommand`): `--port`,
  `--host`, `--allowed-hosts`, `--shared-browser-context`, `--session-name` and `--session-color`
  (with `--extension` these are the tab group title and color; colors: blue, red, yellow, green,
  pink, purple, cyan, orange).
- Transports with `--port` (`tools/utils/mcp/http.ts`): streamable HTTP on any path except `/sse*`
  (the app uses `/mcp`), and legacy SSE on `/sse`. Streamable sessions run a 3 s ping heartbeat
  that closes the session when the client does not answer; SSE sessions have no heartbeat.
- With `--shared-browser-context` all clients share one browser (one extension connection, one
  group), but when the last client leaves, `dispose` closes it (`last = !clientCount`). A server
  with no client left therefore drops the group's connection.
- Codex supports streamable HTTP MCP servers (`mcp_servers.<name>.url`, plus `bearer_token_env_var`,
  `http_headers`; seen in the 0.171.0 binary). Claude takes `{"type":"http","url":...}`.
- The server exits when its stdin closes (`setupExitWatchdog`).

### Design (implemented in `mono/browser/server.rs`)

- One `@playwright/mcp` process per Mono with an active browser, started on the first run that
  needs it, on a free port, bound to `127.0.0.1`, kept across turns. Name: the Mono's name
  (`--session-name`), color: the Mono color mapped to the nearest tab group color.
- Each run only points Claude (`--mcp-config` http entry) or Codex (`-c mcp_servers.<name>.url`)
  at it. The disallowed tools, `disabled_tools`, approval mode, origin flags and the audit are
  unchanged (they are client side or in the same server args). The token lives only in the server
  process environment.
- The app keeps one MCP client of its own connected (the keeper, over SSE, which has no heartbeat),
  so the shared browser connection and the group survive the gap between turns. Chrome quitting
  or the extension's service worker stopping still ends the connection, and the next run then
  gets a new group (the keeper repeats its opening call on the next run).
- Lifecycle: restart when the process died or its launch changed (profile, token, origin rules,
  Mono name or color); stopped when the browser is switched off or its profile or origin rules
  change (not for "ask before"), the token changes, the Mono is deleted, the app quits (process
  guard) or after 30 minutes without a run. A run holds a lease on the server (dropped when the
  run ends), a server with a lease is never stopped as idle, and idle time counts from the last
  release. Runs of one Mono that start together share one start (a per-Mono lock).

### Server-side tool restriction: none

`browser_evaluate` and `browser_run_code_unsafe` have capability `core`, which `filteredTools`
always keeps; `--caps` only adds capabilities (vision, pdf, devtools, ...), and there is no flag
that removes a tool. The restrictions stay client side (`--disallowedTools` for Claude,
`disabled_tools` for Codex), exactly as with the stdio server.

### Residual risk

The HTTP server has no authentication: it checks the `Host` header only (DNS rebinding), not a
token or `Origin`, and the path is not secret (any non-`/sse` path is the MCP endpoint; checked:
`/mcp/anything` returns 200). Protection used: random port, `127.0.0.1` only,
`--allowed-hosts 127.0.0.1:<port>` (a request with another `Host` gets 403). Any local process of
the same user that finds the port can drive the Mono's Chrome profile without the tool
restrictions (they are enforced by the agent clients, not the server) and without the audit. It
cannot read the token.

Cross-site requests from a web page, checked against a real 0.0.86 server: `POST /mcp` with
`Content-Type: text/plain` and a foreign `Origin` gets 415, without the `Accept` pair 406, an
`OPTIONS` preflight gets 400 without CORS headers, and `POST /sse?sessionId=...` with
`text/plain` gets 400. A page cannot send `application/json` cross-origin without a preflight, so
a browser page cannot drive the server (a foreign `Origin` with the JSON type is accepted when
sent by a non-browser client, since the server does not check `Origin`).

An authenticating proxy inside the app (bearer token check, forwarding to the server) was
considered and not built: the server only listens on TCP (`--port`; no unix socket or pipe), so
the proxy would sit next to an unauthenticated port that a local process can still reach directly.
It adds code and a failure point without closing the gap.

Not verified live (needs Chrome with the extension): that a second client of the shared server
reuses the first client's tabs and group. It follows from the code above.
