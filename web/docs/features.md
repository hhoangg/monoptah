# Features: current mobile app inventory and web app scope

## 1. Introduction

### 1.1 Scope

This document lists every feature of the current iOS/Expo app (`mobile/`), the desktop method each one calls, and what the web app does about it. It also lists features that do not exist in mobile today (approvals, plans, tasks, syntax highlighting, retry/edit) and the features that are new to the web program (accounts, invites, pairing, multi-desktop, push onboarding, tablet/desktop layouts).

The iOS app talks to the desktop through the native `MonocodeLink` module (QUIC, pinned certificate). The web app cannot do that. It replaces the transport with a Durable Object relay plus Noise IK end-to-end encryption, and keeps the same JSON request/watch model (`{version:1, id, method, params}`) and the same method names. See [architecture.md](architecture.md) for the link, relay and handshake.

### 1.2 Priority legend

| Priority | Meaning |
|---|---|
| MVP | Needed for the first usable release: sign in, link a desktop, list sessions, read a transcript, send, stop, get notified. |
| P1 | Needed for parity with mobile or for safe daily use; ships right after MVP. |
| P2 | Nice to have, large, or platform-limited; ships when time allows. |

P2 orders work, it does not make a mobile feature optional: every F row ships or is listed in the "Explicit drops" table (section 2.17). Only the rows of roadmap.md Phase 7 are backlog.

### 1.3 How to read an entry

| Column | Content |
|---|---|
| ID | `F-nn` = exists in mobile today. `X-nn` = requested but NOT in mobile today. `N-nn` = new web feature (section 4). |
| Mobile source | `path:line` of the main component or function, relative to the repo root. |
| Desktop method(s) | Exact wire method names (section 3 has the full matrix). "none" = client-side only. |
| Web behaviour | What the web app does. "Gap:" marks an iOS Safari / PWA limitation and the mitigation. |
| Pri | MVP, P1 or P2. |

Global rules that apply to every row:

- Routes live under `/d/:desktopId/...` (one prefix per desktop). Layout classes: mobile (single column), tablet (list + detail), desktop (sidebar + list + detail), described in [design-system.md](design-system.md).
- A device with read-only permission can use every read method and cannot call any mutating method. Mutating controls are hidden or disabled with a reason ("This device is read-only").
- Mutations reuse one command id per `[method, params]` signature until success (idempotency), as in `mobile/src/features/workspace/WorkspaceProvider.tsx:418`. The web link keeps this.
- Full-access runtime mode (`runtimeMode: "full-access"`) is forbidden from the web. The desktop rejects it for every remote device (phones included) and the UI never offers it.
- Haptics: iOS Safari has no Vibration API and no haptic API. Every "haptic" in mobile becomes a short visual cue (pressed state, check flash). Not repeated per row below unless relevant.
- Native-only UI (SwiftUI menus, action sheets, `Alert.prompt`, liquid glass, gesture-handler swipe) maps to shadcn DropdownMenu, AlertDialog, Dialog, vaul sheets and CSS `backdrop-filter`.

### 1.4 Cross-cutting mobile behaviour the web must keep

| Behaviour | Mobile source | Web decision |
|---|---|---|
| One request/watch model; 25 s reply wait, 16 pending, 4 watches per device | `crates/monocode-core/src/mobile/host.rs:56` | Same limits; the web client multiplexes requests by `rid` over one Noise channel. |
| Watch frames: `order`, `turns`, `patches` (append offsets in UTF-16 units), `question`, `usageLimit`, `error`; unknown turn id forces a fresh watch with `from` | `mobile/src/features/workspace/watchUpdates.ts:77` | Port to TypeScript in `web/protocol`. JS strings are UTF-16, so offsets map directly. |
| Optimistic local turn (`pending:<ts>`) until the desktop transcript has one more user message; 5 s startup grace | `WorkspaceProvider.tsx:439` | Port as TanStack Query optimistic update. |
| Catalog poll 5 s, changes/git 10 s, pause when inactive | `mobile/src/features/connection/ConnectionProvider.tsx:153` | Poll only while the tab is visible (`visibilitychange`); on resume resync. iOS PWAs drop the WebSocket after about 10 s in background, so reopen the link and re-run `conversation.watch` with `from`. |
| Capability flags `monoTools`, `push`, `monoFiles` from `workspace.list` | `ConnectionProvider.tsx:39` | Keep; hide features the desktop does not advertise. |
| Native Swift transcript view on iOS, JS transcript as fallback | `mobile/modules/monocode-link/ios/TranscriptSession.swift:364`, `mobile/src/features/sessions/Transcript.tsx:77` | The web app is the JS path: React virtualized list. The desktop already sends projected turns, so the web port covers the mobile JS path (`mobile/src/features/sessions/TranscriptTurn.tsx`, `mobile/src/features/sessions/markdownParser.ts`) in TypeScript. |

## 2. Inventory of the current mobile app

### 2.1 Connection and pairing

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-01 | Connect screen: first-run landing, "Connect to desktop" (QR scan) and "Paste a pairing link"; ASCII wave field and mascot rain | `mobile/src/features/connection/ConnectScreen.tsx:40` | none | Replaced by the sign-in / "Link a desktop" flow (N-01..N-12). Web pairing starts from a link or QR shown on the desktop (see N-07), so there is no in-app scanner (dropped, section 2.17). The iPhone camera app opens the QR as a link. | MVP |
| F-02 | Pairing link paste sheet (masked input) | `mobile/src/features/connection/PairingLinkSheet.tsx:9` | none | Optional "Paste a pairing link" field on the Desktops screen, same URL shape as the QR. | P1 |
| F-03 | Invitation parsing (`monocode://pair?i=...`, 12000 chars, expiry, 16 addresses) | `mobile/src/features/connection/invitation.ts:7` | none | New payload (relay URL, desktop public key, pairing code, expiry); parsed in `web/protocol`. No private key in the link (old format embeds one). Expired link shows "This code expired. Choose Pair on your desktop to make a new one." | MVP |
| F-04 | Pair / enroll (identity, proof, `device.enroll`) | `ConnectionProvider.tsx:171` | `device.enroll` | New web flow: browser generates an X25519 key pair with `@noble/curves` and stores the private key wrapped by a non-extractable AES-GCM `CryptoKey` (architecture.md 3.4), Noise IK to the desktop, desktop shows name + fingerprint and the user confirms on the desktop (N-08). Gap: IndexedDB keys can be evicted by Safari after 7 days of no use unless the site is installed as a PWA; WebCrypto X25519 is not used (Safari support is unreliable). | MVP |
| F-05 | Restore on launch (saved pairing, selection, cached catalog) | `ConnectionProvider.tsx:81` | `workspace.list` | Restore from IndexedDB (key, desktop list) and the TanStack Query persisted cache; show cached catalog instantly, then refresh. | MVP |
| F-06 | Catalog refresh and reconnect (5 s poll, address self-heal) | `ConnectionProvider.tsx:114` | `workspace.list` | Poll while visible; addresses are irrelevant on web (relay). A desktop heartbeat from the relay gives online status. | MVP |
| F-07 | Connection notice and "Reconnecting to desktop" capsule; edit screens disable offline | `mobile/src/features/connection/ConnectionNotice.tsx:18` | none | Global offline/reconnect banner (N-17); composer shows "Reconnecting to desktop..." and disables Send/Stop; tap to retry. | MVP |
| F-08 | Disconnect desktop (clears pairing, `device.leave`) | `ConnectionProvider.tsx:238` | `device.leave` | "Remove this desktop from this device" in Settings and in the desktop management screen; also revoke on the account (N-10). | MVP |
| F-09 | Generic idempotent `command()` | `ConnectionProvider.tsx:255` | all mutating | Same, in the link client. | MVP |

### 2.2 Workspace and projects

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-10 | Workspace picker: choose which projects and Monos appear on this device | `mobile/src/features/connection/WorkspacePicker.tsx:39` | none (selection is phone-side only, gap G1, defined in security.md section 7) | Kept as "Projects on this device", but enforced on the desktop in `MobileHost::handle` for every device (the selection is stored per device on the desktop and filters `workspace.list`, `sessions.list` and every `project.*`/`conversationId` call). The web app reads it and may only narrow it via `device.selection` (see 3.3). The first-run picker offers "Choose later" (as mobile, `mobile/src/features/connection/WorkspacePicker.tsx:193-205`), which keeps the selection the desktop granted, plus "Refresh desktop" (mobile gate: `mobile/src/features/connection/WorkspaceGate.tsx:23`, `mobile/src/app/settings/workspace.tsx:8`). | MVP |
| F-11 | Persistent sidebar on wide screens (projects, Monos, 20 recent sessions) | `mobile/src/features/workspace/WorkspaceSidebar.tsx:17` | `workspace.list` | Core of the tablet/desktop layout (N-18); shows a desktop switcher above the project list. | P1 |
| F-12 | Drawer: Monos then projects, filter (All/Unread/Working now; Monos/Projects), search, New Mono | `mobile/src/features/workspace/WorkspaceDrawer.tsx:67` | `workspace.list` | Mobile layout uses a vaul drawer with the same content and filters; New Mono only if `monoTools`. | MVP |
| F-13 | Monos-only layout (messaging-style home) | `mobile/src/features/settings/monoOnly.ts:4`, `mobile/src/features/workspace/MonoHome.tsx:56` | `mono.reset`, `mono.delete` (context menu) | Setting "Monos only" in preferences; same list with context menu (mark read/unread, details, hide). Reset and delete are denied for web in v1 (security.md section 7) and hidden. | P1 |
| F-14 | Index redirect and "last place" per desktop; empty state "Make yourself at home" | `mobile/src/app/index.tsx:18` | none | Route `/` redirects to the last place of the last used desktop (`/d/:desktopId/...`, localStorage); `/d/:desktopId` is the catalog home; empty state links to "Choose projects". | MVP |
| F-15 | Project picker route (`/projects`, search, switch in place) | `mobile/src/app/projects/index.tsx:19` | none | Desktop layout has the project list in the sidebar; mobile uses a command-style dialog with search. | P1 |
| F-16 | Project colour hash, mascot per project | `mobile/src/features/workspace/projectColors.ts:3`, `mobile/src/features/mascots/projectMascots.ts:263` | none | Port the hash; opaque ids in routes (no filesystem paths in URLs). | MVP |
| F-17 | Mono unread tracking (phone-local seen map) | `mobile/src/features/workspace/monoSeen.ts:11` | none | Keep client-local in IndexedDB per desktop; not synced across devices (P2 idea: store on desktop). The drawer/menu button shows an unread badge ("Projects and monos, new replies", `mobile/src/features/workspace/ProjectScreen.tsx:288`). | P1 |
| F-18 | Project screen: New session page + Sessions page, header switch | `mobile/src/features/workspace/ProjectScreen.tsx:92` | `project.worktrees`, `project.changes`, `models.list`, `session.start`, `conversation.send`, `session.cancel`, `conversation.watch` | Routes `/d/:desktopId/p/:projectId` (sessions list) and `/d/:desktopId/p/:projectId/new` (New session); from 1024px the sessions list is the shell's second sidebar (classic layout, shown on every screen of the project) and the chat pane shows the New session page on both URLs. | MVP |
| F-19 | Workspace menu: current checkout, new worktree, existing worktree, view changes | `mobile/src/features/workspace/WorkspaceMenu.tsx:69` | `project.worktrees`, `project.changes` | Popover/sheet with the same options; sent as `workspaceMode:"worktree"` or `worktreePath` in `session.start`. Disabled for read-only devices. | P1 |
| F-20 | "Open after starting" preference | `mobile/src/features/settings/newSessions.ts:4` | none | Local preference, default on. | P1 |

### 2.3 Sessions list

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-21 | Sessions list, newest first, debounced search (250 ms), infinite paging (limit 30), per `projectId+query` cache | `mobile/src/features/sessions/SessionsPage.tsx:41`, `mobile/src/features/sessions/useSessionPages.ts:21` | `sessions.list` | TanStack Query `useInfiniteQuery`; virtualized list; dedupe by id as offsets shift. | MVP |
| F-22 | Worktree scoping of the list (client side) | `SessionsPage.tsx:171` | none | Same filter chip. | P1 |
| F-23 | Filter menu: working only, by agent (11 providers) | `mobile/src/features/sessions/SessionFilterMenu.tsx:56` | none | DropdownMenu with the same groups. | P1 |
| F-24 | Session card: title, relative time or "Working..." spinner, branch, model, provider logo | `mobile/src/features/sessions/SessionCard.tsx:35` | none | Same card; terminal braille spinner (`mobile/src/features/sessions/TerminalSpinner.tsx:33`) as CSS/JS; provider SVGs ported (`mobile/src/features/sessions/ProviderLogo.tsx:19`). | MVP |
| F-25 | Row actions: rename, delete (swipe left or long press, confirm) | `mobile/src/features/sessions/useSessionActions.ts:21` | `session.rename`, `session.delete` | Kebab/context menu + AlertDialog (rename is a Dialog). Optional swipe on touch. Hidden for read-only devices. Gap: no `Alert.prompt`; use Dialog. | P1 |
| F-26 | New-arrival animation | `mobile/src/features/sessions/useArrivals.ts:21` | none | `motion` layout animation, honours `prefers-reduced-motion`. | P2 |

### 2.4 Live and finished surfaces

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-27 | Activity tracker: working, finished (last 8, dropped after 60 s missing), arrivals (12 s) | `mobile/src/features/sessions/activityStore.ts:116` | `workspace.list` (derived: `busy`, `waiting`) | Zustand store with the same rules. | MVP |
| F-28 | "Now working" pill: ticker, status ("Needs your approval", "Has a question for you"), Stop, Skip, expandable mini pills | `mobile/src/features/sessions/LiveSessions.tsx:70` | `session.cancel` | Same pill above the composer; on desktop layout it can live in the sidebar. Stop hidden for read-only. This pill is the only place mobile shows a pending approval; web adds a real approval UI (X-01). | MVP |
| F-29 | Finished sessions deck (expand, clear, dismiss) | `mobile/src/features/sessions/FinishedSessions.tsx:57` | none | Same; click instead of long press to dismiss (X on card). | P1 |
| F-30 | Project empty page: logo, "What should we work on?", optional dithered background | `mobile/src/features/sessions/EmptySession.tsx:37` | none | Same page at `/d/:desktopId/p/:projectId/new`; keyboard handling via `visualViewport`. | MVP |

### 2.5 Session screen and transcript

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-31 | Start a new session from the empty page (model, effort, account, access, workspace mode) | `ProjectScreen.tsx:218`, `WorkspaceProvider.tsx:439` | `session.start` | Same, from `/d/:desktopId/p/:projectId/new`. `runtimeMode` limited to supervised, auto-accept-edits, auto; full-access never sent. Disabled for read-only devices. | MVP |
| F-32 | Open session transcript: tail 3 turns, page older 20 at a time, anchor latest user message, day separators, loading skeleton | `Transcript.tsx:77`, `WorkspaceProvider.tsx:376,514` | `conversation.watch`, `conversation.read`, `conversation.older` | React virtualized list (TanStack Virtual) with scroll anchoring; load older on scroll to top; fallback `conversation.read` every 1 s when watch is down. Gap: iOS Safari scroll anchoring and rubber-banding need a manual `scrollTop` fix when prepending older turns. | MVP |
| F-33 | Scroll-to-bottom button (more than 160 px from end) | `Transcript.tsx:331` | none | Same. | MVP |
| F-34 | Turn item types: user, text, header, work, notice, sessions, choices, document | `mobile/src/features/workspace/types.ts:184` | none | Render all eight; unknown types from newer desktops render nothing (as mobile does). | MVP |
| F-35 | User bubble: 4-line clamp with Show more, "N attachments" count, clock time | `mobile/src/features/sessions/TranscriptTurn.tsx:257` | none | Same. Improvement later (P2): attachment names. | MVP |
| F-36 | Fold line "Worked for 1m 12s" with expandable notes and phases; live shimmer and elapsed timer | `TranscriptTurn.tsx:343` | none | Same; shimmer in CSS. | MVP |
| F-37 | Tool call phases and steps (kind icon, last 4 steps while live, "collapse tool calls" setting) | `TranscriptTurn.tsx:436` | none | Same summarised model (labels only; the desktop sends no tool input/output). | MVP |
| F-38 | Turn footer: copy response, metrics button, completion time | `TranscriptTurn.tsx:154` | none | `navigator.clipboard.writeText` inside the click handler. Gap: needs a user gesture; on failure fall back to a selectable text dialog. | MVP |
| F-39 | Turn metrics sheet (cache hit, tok/s, duration, token groups) | `mobile/src/features/sessions/TurnMetricsSheet.tsx:36` | none | vaul sheet / popover. | P1 |
| F-40 | Session header and toolbar: Changes badge, Rename, Copy Session ID, Archive, Delete | `mobile/src/features/sessions/SessionToolbar.tsx:104` | `session.rename`, `session.archive`, `session.delete` | Header actions + menu, including the session menu "Settings" item (`mobile/src/features/sessions/SessionToolbar.tsx:216`, opens the per-desktop settings); archive/delete behind AlertDialog; read-only hides mutations. Copy Session ID needs a user gesture. | P1 |
| F-41 | Error notice row, error sheet, composer takeover bar, "Account unavailable" special case | `mobile/src/features/sessions/sessionError.ts:15`, `mobile/src/features/sessions/SessionErrorSheet.tsx:12` | none (errors arrive as `notice` items) | Same copy and dismiss-for-this-run behaviour. Also renders the pending-approval notices the desktop appends to the last turn ("Waiting for approval on desktop: label"; formatted at `crates/monocode-gpui/src/mobile/transcript.rs:176-187`, fed by `mobile_pending` at `crates/monocode-gpui/src/conversation/mobile.rs:214`, which leaves out questions because they have their own card, F-58). | MVP |
| F-42 | Offline state in the transcript area | `Composer.tsx:79` | none | See F-07. | MVP |
| F-43 | Message list shows Mono "Thinking..." header | `TranscriptTurn.tsx:411` | none | Same, for Mono chats. | P1 |

### 2.6 Markdown and streaming

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-44 | Markdown subset: paragraphs, headings, fenced code, nested lists, quotes, tables, rules, inline code, bold, italic, strike, links | `mobile/src/features/sessions/markdownParser.ts:80`, `mobile/src/features/sessions/Markdown.tsx:44` | none | Port of `markdownParser.ts` to an AST renderer that emits React elements (no HTML sink). Never raw HTML, no `dangerouslySetInnerHTML`; link rule as security.md section 8 rule 2 (`https`, `http` with a warning, `mailto`, in-app `/d/...`; everything else is plain text), `rel="noopener noreferrer nofollow"`; images are not fetched automatically. Required by the XSS rules in [security.md](security.md). | MVP |
| F-45 | File mentions (inline code, markdown links, `~/` and absolute paths, optional `:12` or `#L12`) are chips with the icon of their file type, in session chats and in Mono chats | `Markdown.tsx:321` | `project.file`, `chat.mentionedFile` | A text or source file inside the session's own project opens the Files route with the line jump (`project.file`). Everything else (html, markdown, images, files outside the project, every file of a Mono chat) opens the preview dialog (F-60) through `chat.mentionedFile`, only when the desktop advertises `mentionedFiles`; otherwise a file inside the session's project still opens the Files route (any kind) and only a file outside it gets the dialog that says to update the desktop. A mention with a line in an html or markdown file asks `chat.mentionedFile` with `source: true` and shows the raw source with the line marked. Without an `onOpenFile` host the chip renders plain. | P1 |
| F-46 | Streaming word reveal (hold half-written word 150 ms, per-word fade, close open markers) | `mobile/src/features/sessions/wordReveal.ts:51` | none | Port as is (pure JS); CSS opacity transition; off under `prefers-reduced-motion`. | P1 |

### 2.7 Composer and attachments

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-47 | Composer: morphing glass pill/card, context row, model chip, Send/Stop/Dictate button, placeholders, 160 px max, offline and error states | `mobile/src/features/sessions/Composer.tsx:79` | `conversation.send`, `session.start`, `session.cancel` | `textarea` with auto-grow; Enter sends on desktop layouts, Shift+Enter newline; on touch layouts Enter is newline and the button sends. While a turn runs, Send stays available when the draft is non-empty (the desktop steers or queues per its own setting, `crates/monocode-gpui/src/conversation/mobile.rs:105-106`; mobile source `mobile/src/features/sessions/Composer.tsx:39-40,470-483`), with the placeholder "Send a follow-up..." and the follow-up shown as an optimistic pending turn; Stop shows when the draft is empty. Disabled for read-only devices ("Read-only"). Gap: iOS keyboard resize via `visualViewport` and `interactive-widget=resizes-content`. | MVP |
| F-48 | Attachments: Photo Library and Files, max 10, 8 MiB total base64 per message (the relay request cap, architecture.md 5.2), per-file cap lowered to match, photos scaled to 2048 px JPEG 0.85 | `mobile/src/features/sessions/attachments.ts:20` | `conversation.send`, `session.start` (`attachments:[{name,mimeType,data}]`) | `<input type="file" multiple>` (iOS shows photo library, camera and Files in one picker); canvas resize; same count limit; the size copy says "Attachments are limited to 8 MiB per message". Large payloads are chunked over the relay (see architecture.md). Gap: HEIC is converted by canvas only if the browser decodes it (Safari does); no Share-target in MVP. | MVP |
| F-49 | Dictation (waveform, 5 min cap, on-device Apple transcription) | `mobile/src/features/sessions/DictationBar.tsx:63` | none | Optional Web Speech API button (`webkitSpeechRecognition`). Gap: unreliable in iOS PWAs, needs network and user gesture, no waveform guarantee; hide the button when unsupported. | P2 |
| F-50 | Reply quoting in Mono chats (`> quote` prefix) | `mobile/src/app/mono/[monoId].tsx:95` | `conversation.send` | Same prefix format. | P2 |

### 2.8 Models, effort, accounts, usage

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-51 | Model list by provider, search, current checked; existing session pick debounced 400 ms | `mobile/src/features/models/ModelPicker.tsx:229` | `models.list`, `session.configure` | Dialog (desktop) or vaul sheet (mobile) with inline search. | MVP |
| F-52 | Options: effort slider (hot values tinted), fast mode, other settings | `ModelPicker.tsx:481` | `session.configure` | shadcn Slider with step marks, Switch, RadioGroup. | MVP |
| F-53 | Access modes: Supervised, Auto-accept edits, Auto, Full access | `mobile/src/features/models/access.ts:9` | `session.configure`, `session.start` | The first three are offered. Full access is NOT offered and the desktop rejects it from every remote device. If a session is already in full-access (set on the desktop) the UI shows it read-only as "Full access (set on desktop)". | MVP |
| F-54 | Account picker with usage meters (claude, codex), refresh; switching opens a new draft chat | `mobile/src/features/models/AccountPicker.tsx:18` | `accounts.usage` (`refresh` optional) | Same; the refresh button is allowed for read-only devices only if the desktop allows probing (default: control only). | P1 |
| F-55 | Usage limit card: Resume, Resume at reset | `mobile/src/features/sessions/UsageLimitCard.tsx:9` | `session.resumeUsage`, `session.resumeUsageAtReset` | Same, including the "Cancel automatic resume" toggle state, the offline hint ("Reconnect to desktop to change continuation settings") and the inline error (`mobile/src/features/sessions/UsageLimitCard.tsx:42-76`); disabled for read-only. | MVP |
| F-56 | Phone-wide default model, provider and access (Providers/Models/Permissions settings) | `mobile/src/app/settings/providers.tsx:25`, `mobile/src/app/settings/models.tsx:27`, `mobile/src/app/settings/permissions.tsx:17` | `models.list` | Stored per desktop in IndexedDB (key `defaultModel:<desktopId>`). Permissions page omits full access. | P1 |
| F-57 | Mono model, sub-session model settings ("Same as <Mono>") | `mobile/src/features/models/useMonoSubsessions.ts:28`, `mobile/src/features/models/useMonoModel.ts:11`, `mobile/src/app/mono-profile/[monoId]/model-options.tsx:7` | `models.list`, `mono.update` | Same, inside Mono edit, including the Mono model options and access screen (`model-options.tsx`). | P2 |

### 2.9 Questions

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-58 | Question card: multi-step, radio/checkbox, "Other" custom text, Skip, timed prompts (60 s countdown, `keepQuestionOpen` on first touch) | `mobile/src/features/sessions/QuestionCard.tsx:34` | `session.answerQuestion`, `session.keepQuestionOpen` | Same card above the composer; keyboard accessible; answers need control permission (read-only devices see the card disabled with "Answer on desktop or use a control device"). | MVP |

### 2.10 Files

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-59 | File tree: lazy folders, ignored entries dimmed, prefetch 4, pull-to-refresh. Implemented but unreachable in mobile today (`FilesView` not mounted) | `mobile/src/features/files/FilesView.tsx:68`, `mobile/src/features/connection/useFileTree.ts:43` | `project.files` | Mounted as a "Files" tab on tablet/desktop layouts; refresh button instead of pull. Desktop denies sensitive paths (see 2.10 note). | P1 |
| F-60 | File viewer: virtualized code, wrap toggle, markdown preview, truncation notice, background highlight job | `mobile/src/features/files/FileScreen.tsx:40`, `mobile/src/features/files/CodeView.tsx:90` | `project.file`, `project.fileHighlight` | Reached from transcript file mentions and the tree. Highlighting: shiki on the client (themes match light/dark), so the `highlightJob` polling is not used in the web; the desktop `highlighted` palette is ignored. Markdown preview uses the safe renderer (F-44). Denied files return "This file is hidden by Monoptah" (desktop deny-list for `.env`, keys, `.git/`). **Mention preview** (`chat.mentionedFile`, paged by `nextOffset`): text and source in the same highlighted lines with the line marked; markdown rendered with relative images fetched through `asset` (remote images never load); html in an `<iframe sandbox="">` from a `blob:` URL (the desktop's sanitized document, images inlined as `data:` URIs, CSP meta prefixed, no srcdoc; the app CSP has `frame-src blob:`); images in a zoom viewer from `data:` URIs of allowlisted image types (svg only through `<img>`; no `blob:` URL is made for desktop bytes except the sandboxed html document); other files "Cannot preview this file" with size, path and a copy button. A client cap of 64 assets and 40 MiB per preview (the main document's pages count too) shows a notice. Errors, each a short reason: `path_denied` "This file is hidden by Monoptah" (blocked file), `forbidden` "This file is outside this conversation" (or, when the desktop does not advertise `mentionedFiles2`, "Update Monoptah on your desktop": an older desktop refuses a file made by a command with the same code), `unsupported` "Update Monoptah on your desktop", `too_large`, `not_found` "This file could not be found", `busy` (with Try again), `not_selected`. | P1 |
| F-61 | File search with debounce 150 ms | `mobile/src/features/connection/useFileSearch.ts:14` | `project.search` | Same; Cmd/Ctrl+P quick-open on desktop layout. | P1 |
| F-62 | File missing/error illustration | `mobile/src/features/files/FileMissing.tsx:50` | none | Same illustration (SVG). | P2 |
| F-63 | Document viewer for Mono documents (markdown or visual tree) | `mobile/src/features/files/DocumentScreen.tsx:29` | `mono.document` | Markdown via F-44; visual trees via F-73. | P2 |

Note on files: the desktop deny-list (`.env`, `.env.*`, `*.pem`, `id_rsa*`, `.npmrc`, `.git/` contents) applies to `project.files`, `project.file`, `project.search` and to diff content, for all devices, not just web.

### 2.11 Changes, diff and git

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-64 | Changes screen: staged/unstaged sections, status letters, +/- counts, stage per file or all, optimistic with rollback | `mobile/src/features/changes/ChangesView.tsx:41` | `project.changes`, `project.gitAction` (`stage`, `unstage`, `stageAll`, `unstageAll`) | Read view MVP-adjacent (P1); staging is a mutation (control only). Polled every 10 s while visible. | P1 |
| F-65 | Context-aware bottom action: Commit N files / Stage All / Publish Branch / Sync / PR #n / Create PR | `ChangesView.tsx:319` | `project.git`, `project.gitAction` | Same button. Push and PR creation use the desktop's git credentials, so they are denied for web in v1 (security.md section 7): `sync`, `pr` and `commit` with `push:true`/`createPr:true` (`crates/monocode-gpui/src/mobile/project.rs:436-455`) work only when the desktop toggle "Remote push in Git" is on, and the buttons are hidden otherwise. When on, a confirm dialog names the remote and branch. Commit without push stays P1. | P1 |
| F-66 | Commit sheet: message, "Write a message" (AI), commit, publish/sync, create/view PR, confirm when pushing the default branch | `mobile/src/features/changes/CommitSheet.tsx:40` | `project.gitAction` (`generate`, `commit`, `sync`, `pr`) | Dialog/sheet; `sync`, `pr` and a pushing `commit` are denied for web unless "Remote push in Git" is on (security.md section 7) and their controls are hidden; long jobs return `{pending:true}`, client re-sends the same id. PR opens with `window.open` (user gesture). "Stop writing" cancels AI message generation only on the client: it discards the pending result with a generation counter (`mobile/src/features/changes/CommitSheet.tsx:129-134,301`), there is no wire cancel, so the web does the same. | P2 |
| F-67 | Git status (branch, ahead/behind, upstream, PR) polled 10 s | `mobile/src/features/connection/useProjectGit.ts:39` | `project.git` | Same, read-only allowed. | P1 |
| F-68 | Diff screen: unified diff, wrap, hunks, gutter, syntax runs; last 4 diffs kept in memory | `mobile/src/features/changes/DiffScreen.tsx:25` | `project.changes` (diff lines inside) | Route `/d/:desktopId/p/:projectId/changes/$` (splat file path), data from the Query cache; shiki for highlighting; side-by-side on desktop layout (P2). | P1 |

### 2.12 Mono chat and Mono profile

A Mono is a persistent agent with one conversation id `mono:<id>`.

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-69 | Mono chat screen (animated mascot header, accent, compact composer) | `mobile/src/app/mono/[monoId].tsx:68` | `conversation.watch`, `conversation.read`, `conversation.send`, `session.cancel` | Route `/d/:desktopId/monos/:monoId`. Includes the Mono chat empty state (`EmptySession`, `mobile/src/app/mono/[monoId].tsx:354`) and, when `monoTools` is off, the header "Settings" action instead of the profile and menu (`mobile/src/app/mono/[monoId].tsx:200`). | P1 |
| F-70 | Mono menu: details, reset, delete (confirm) | `mobile/src/features/sessions/MonoMenu.tsx:16` | `mono.reset`, `mono.delete` | Details only. Reset and delete are denied for web in v1 (security.md section 7) and hidden. Shown when `monoTools`. | P1 |
| F-71 | Message actions: 12 emoji reactions (phone-local), reply, mark unread, copy, select text | `mobile/src/features/sessions/MessageActionsSheet.tsx:52` | `conversation.send` (reply only) | Context menu; reactions stored in IndexedDB only. Gap: no long-press haptic. | P2 |
| F-72 | Launched session cards and suggested reply chips and document cards | `mobile/src/features/sessions/LaunchedSessionCard.tsx:56`, `mobile/src/features/sessions/MonoCards.tsx:49,96` | `conversation.send` (chips), `mono.document` | Same cards; open launched session in a side panel (modal on mobile). When the desktop answers `not_selected` (the session lives in a project outside this device's selection, section 2.17) the card shows "Not on this device's projects" and has no open action. | P1 |
| F-73 | Visual tree renderer (bar/line/area, gauge, donut, heatmap, progress, canvas, stat, table; halftone dither) | `mobile/src/features/visuals/VisualTree.tsx:22`, `mobile/src/features/visuals/charts.tsx:83` | `mono.document`, `mono.profile` | Inline SVG renderer, same node kinds; the dither fill is a CSS/SVG pattern. Interactive nodes stay static as in mobile. | P2 |
| F-74 | Contribution graph | `mobile/src/features/visuals/charts.tsx:631` | `mono.profile` | SVG grid, 26 weeks. | P2 |
| F-75 | Create Mono (name, 10 mascots, 9 colours) | `mobile/src/app/mono-new.tsx:35` | `mono.create`, `mono.update` | Dialog; control only. | P2 |
| F-76 | Mono profile (identity, today tiles, contributions, up next, habits, documents, projects) | `mobile/src/app/mono-profile/[monoId]/index.tsx:24` | `mono.profile` | Read-only page at `/d/:desktopId/monos/:monoId/profile`; allowed for read-only devices. Includes the Mono documents list page (`mobile/src/app/mono-profile/[monoId]/documents.tsx`, `mobile/src/features/workspace/MonoDocuments.tsx:26`) at `.../profile/documents`, and a document at `.../profile/file/:name`. A refresh button replaces pull-to-refresh. | P2 |
| F-77 | Habit detail (read-only) | `mobile/src/app/mono-profile/[monoId]/habit.tsx:13` | `mono.profile` | Same, at `/d/:desktopId/monos/:monoId/profile/habit/:id`; refresh button where mobile has pull-to-refresh. | P2 |
| F-78 | Edit Mono: name (600 ms autosave), model, sessions it starts, mascot, colour, chat colour, assigned projects | `mobile/src/app/mono-profile/[monoId]/edit.tsx:25`, `.../subsessions.tsx:17`, `.../projects.tsx:16`, chat accent `mobile/src/features/workspace/monoAccent.ts:30` | `mono.update`, `models.list` | Route `.../profile/edit`. Control only. The per-Mono chat accent is phone-local (IndexedDB on web, not sent to the desktop). Assigned projects limited to projects allowed for this device. | P2 |
| F-79 | Soul and Memory editors (hash-checked save) | `mobile/src/app/mono-profile/[monoId]/file.tsx:31` | `mono.file`, `mono.saveFile` | Viewer only on web. Saving (`mono.saveFile`) is denied for web in v1 (security.md section 7), so the Save button is hidden and the editors are read-only. | P2 |

### 2.13 Notifications

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-80 | Three categories: Approvals and questions, Agent finished, Messages from Monos | `mobile/src/features/settings/notifications.ts:9` | `device.push` (carries disabled categories) | Per-desktop toggles stored on the server with the push subscription (N-15). | MVP |
| F-81 | Push delivery via Expo token; desktop posts to `exp.host`; payload has session title and last text | `mobile/src/features/notifications/SessionNotifications.tsx:48`, `crates/monocode-core/src/mobile/notify.rs:7` | `device.push` | Replaced by Web Push (VAPID). Desktop reports events to the server; the server sends a minimal payload (desktop name, project, status; no transcript text, no session title). Obsolete: `device.push` with Expo tokens (see 3.2). Gap: iOS needs Add to Home Screen (iOS/iPadOS 16.4+), permission after a user gesture, badge via `navigator.setAppBadge` works in iOS 16.4+ Home Screen apps, no notification channels, no action buttons guaranteed. | MVP |
| F-82 | Local-notification fallback for old desktops | `SessionNotifications.tsx:153` | none | Dropped (web has push only; section 2.17). | P2 |
| F-83 | Tap routing: Mono habit run to session sheet, Mono to chat, else session | `SessionNotifications.tsx:204` | none | Service worker `notificationclick` focuses or opens the payload `url`: the session for a Mono habit run (the notice carries `habitRun`) and for any non-Mono notice, the Mono chat for a plain Mono message (URL rule in architecture.md section 8). Foreground suppression is changed, section 2.17. | MVP |
| F-84 | Permission prompt after pairing, blocked-state note | `SessionNotifications.tsx:81` | none | Ask only after install (`isStandalone && PushManager`) and from a button (N-14). Blocked state links to browser/OS help (cannot open system settings). | MVP |
| F-85 | Suppress push when desktop window shows that session; push only for sessions started from the phone plus Mono messages | `crates/monocode-gpui/src/shell/mobile_host.rs:1643` | none | Same rules, evaluated on the desktop before it reports an event. | MVP |

### 2.14 Settings, appearance and themes

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-86 | Settings home: desktop status, notifications, preferences, new sessions, layout, workspace, disconnect | `mobile/src/app/settings/index.tsx:33` | none | Route `/settings` (account and app) and `/d/:desktopId/settings` (per-desktop). Includes the "Reconnect" row (`mobile/src/app/settings/index.tsx:229`) that re-runs the handshake and catalog refresh. Settings are local per browser in MVP; account-synced preferences are P2. | MVP |
| F-87 | Colour scheme: System/Light/Dark | `mobile/src/features/settings/appearance.ts:6` | none | `prefers-color-scheme` plus override; sets `color-scheme` and `theme-color` meta. | MVP |
| F-88 | 6 themes (Classic, Cobalt, Jade, Copper, Violet, Blush) and 7 accent colours | `mobile/src/ui/theme.ts:131,208`, `mobile/src/app/settings/appearance.tsx:56` | none | CSS variables per theme, see [design-system.md](design-system.md). | P1 |
| F-89 | "Collapse tool calls" switch | `mobile/src/features/settings/toolCalls.ts:4` | none | Same. | P1 |
| F-90 | Per-project empty-session background photo with opacity (picked, cropped, dithered) | `mobile/src/features/background/useEmptyBackground.ts:51`, `mobile/src/features/background/dither.ts:1` | none | `<input type=file>`, dither in a Web Worker (OffscreenCanvas), stored in IndexedDB. The web shows it behind every chat of the project (New session, existing sessions, and a Mono chat via the first of its projects that has one) under a theme-aware scrim, not only on the empty page. Gap: Safari storage eviction; photo is lost if site data is cleared. | P2 |

### 2.15 Mascots, backgrounds and misc

| ID | Feature | Mobile source | Desktop method(s) | Web behaviour | Pri |
|---|---|---|---|---|---|
| F-91 | Project mascots (10 sprites, rest/talk frames at 300 ms while busy) | `mobile/src/features/mascots/ProjectMascot.tsx:28`, `mobile/src/features/mascots/projectMascots.ts:127` | none | SVG/CSS sprites; animate only when busy and motion allowed. | P1 |
| F-92 | Pixel mascots with status (idle, working, needs-you) for Monos | `mobile/src/features/mascots/PixelMascot.tsx:36` | none | Same. | P2 |
| F-93 | Mascot rain on the connect screen (accelerometer gravity) | `mobile/src/features/mascots/MascotRain.tsx:67` | none | The accelerometer behaviour is dropped (section 2.17); an optional pointer-driven version may appear on the sign-in page. | P2 |
| F-94 | ASCII field background | `mobile/src/features/background/AsciiField.tsx:33` | none | Canvas at 10 fps, still frame under reduced motion; sign-in page only. | P2 |
| F-95 | Dither loading cover | `mobile/src/ui/DitherLoading.tsx` | none | Optional route-transition cover; P2. | P2 |
| F-96 | Copy session ID / copy text / share | `SessionToolbar.tsx:141` | none | Clipboard on gesture; Web Share API only for PR links (P2). Gap: `navigator.share` exists on iOS Safari but needs a gesture. | P2 |

### 2.16 Requested items that do NOT exist in mobile today

Evidence is from the mobile inventory; each row says what the web does. X-09 to X-21 cover sessions that run on a GitHub Codespace, a Docker sandbox or a remote machine, which the mobile app does not handle today (no `codespace`, `sandbox` or `cloud` code in `mobile/src`, checked 2026-11-17). Their desktop side is architecture.md 6.6; billable actions are control-only and cloud delete is never exposed (security.md section 7).

| ID | Item | Evidence (mobile today) | Desktop method needed | Web decision | Pri |
|---|---|---|---|---|---|
| X-01 | Approval UI for tool permission requests (approve/deny commands and file edits) | Mobile only shows `waiting:"approval"` text (`mobile/src/features/sessions/LiveSessions.tsx:469`) and a notification category (`mobile/src/features/settings/notifications.ts:10`). `APP_METHODS` has no approval method (`crates/monocode-core/src/mobile/host.rs:22`); mobile already renders pending approvals as transcript notices (F-41), and the desktop knows them (`mobile_pending`, `crates/monocode-gpui/src/conversation/mobile.rs:214`). | New method required: `session.answerApproval {conversationId, requestId, decision: "allow"|"deny"}` plus the `approvals` field in `conversation.read` results and watch frames (architecture.md 6.6: `requestId`, label from `Approval.title`, detail from `Approval.detail`; the desktop type `crates/monocode-core/src/agent.rs:220-225` has `id`, `title`, `detail`, `can_remember` and no risk field, so there is no risk in v1). | Approval card above the composer, like the question card; allow or deny once; never "always allow"; never enables full-access. Only control devices may answer; read-only devices see it disabled. The desktop re-checks permission and project selection. Push category "Approvals and questions" deep-links to it. Decided in scope: control devices only, one decision per request, never "always allow", never enables full-access. Needs its own security review before it ships (roadmap.md H-08, U-08, U-13; security.md section 7). | P1 |
| X-02 | Plan display and plan approval | No plan UI or plan items in turn types (`mobile/src/features/workspace/types.ts:184`). | New method required: plan item in the watch frame and a `session.answerPlan` reply. | Read-only plan card first (render markdown plan from the transcript if the desktop exposes it); approval later. | P2 |
| X-03 | Tasks / todo list | No task or todo UI; not in the protocol. | New method required (todo list in the watch frame). | Collapsible checklist above the transcript. | P2 |
| X-04 | Syntax highlighting in transcript code blocks | Code blocks are plain monospace, no highlighting, no copy button (`mobile/src/features/sessions/Markdown.tsx:44`). The file viewer uses a desktop palette (`mobile/src/features/files/CodeView.tsx:35`). | none | shiki (lazy-loaded grammars, light/dark themes) for fenced code, file viewer and diffs, plus a Copy button on code blocks. Highlighting runs in a Web Worker for large blocks. Output is escaped token spans only. | P1 |
| X-05 | Retry / regenerate last turn | No such action in the session UI (`TranscriptTurn.tsx`; only expand/collapse on user bubbles). | none (resend with `conversation.send` and the previous text) | "Resend" on the last user message after an error; control only. | P2 |
| X-06 | Edit and resend a message | Not in mobile. | New method required (truncate conversation at a turn, then send). | Not planned; revisit after the desktop supports rollback. | P2 |
| X-07 | Pause, fork, compact, export a session | Not in mobile (`SessionToolbar.tsx` lists the actions). | New methods required. | Out of scope for this program. | P2 |
| X-08 | Camera capture for attachments | Not offered in mobile (`attachments.ts:54`). | none | `<input type=file accept="image/*" capture>` button on touch layouts. | P2 |
| X-09 | Target kind badge on session cards and the session header: local, remote machine, codespace, sandbox | Not in mobile today: the catalog `session` fields are `id, projectId, title, harness, model, modelName, branch, runtimeMode, updatedAt, busy, live` (`crates/monocode-gpui/src/mobile/catalog.rs`, fn `session`); project `machine` is the constant "Desktop" (`crates/monocode-gpui/src/shell/mobile_host.rs:459-470`); cloud sessions have cwd `remote://<env>/...` and the catalog keeps only sessions whose cwd equals a rail project (`mobile_host.rs:414-417`), so they most likely never reach the phone (inferred, H-09 confirms). Moved sessions keep their local row and cwd (`crates/monocode-core/src/cloud_sessions/continue_local.rs:4-7`), so they already reach phones as local sessions; H-09 gives them their cloud `target` on the web transport only. Desktop source: `crates/monocode-core/src/mono/cloud_api.rs:128-157` (`target_json`, `annotate`), `CloudApi::session_targets` `:972`, `Shell::project_of` `crates/monocode-gpui/src/shell/cloud.rs:1017`. | New fields `target {kind, instance?, state?}` on `workspace.list` and `sessions.list` (H-09); catalog groups by local twin project | Icon plus text badge (never colour only) on the card and header; `remote` sessions are read-only in the UI (no Files, Changes, Git: `unsupported_target`). Gap: none on iOS | P1 |
| X-10 | Cloud state and startup steps in the session screen | Not in mobile today: watch frames carry turns, busy, question and usage limit only (`crates/monocode-core/src/mobile/watch.rs:65`); `conversation.read` returns `id, messages, turns, busy, usageLimit, older, question` (`mobile_host.rs:889-899`). Desktop source: `CloudState` `crates/monocode-core/src/cloud_sessions.rs:465` (Available, Starting, Stopping, Stopped, Unavailable, Unknown), `StepKind::ALL` and `StepStatus` `crates/monocode-core/src/cloud_sessions/startup.rs:96,202` (9 steps; pending, running, done, failed, skipped), UI label rules `crates/monocode-gpui/src/cloud_mark.rs:31-43`. | New `cloud` object in `conversation.read` and watch frames (H-09); never logs or secrets | State chip in the header (Unknown shown as "Not connected"); while starting, a 9-step list with status and duration, current step highlighted; while `cloudState` is `starting` Send is disabled (a send is refused with `code: starting` carrying the desktop's `starting_refusal` text, `crates/monocode-core/src/cloud_sessions/startup.rs:740`); a failed start shows a sanitized error and a Retry button (`cloud.retryStart`, X-20). Gap: iOS suspends the socket in the background (N-16), so the list resyncs on resume | P1 |
| X-11 | Sending to a stopped instance wakes it | Not in mobile today (no cloud session is visible). The Operator path wakes and sends (`wake_for_send`, defined at `crates/monocode-gpui/src/cloud_wake.rs:24`, called at `crates/monocode-gpui/src/shell/app_commands.rs:842`) but refuses with "Retry in a minute" while the instance is not ready. The mobile path does not wake: `mobile_submit` defers when history is not ready (`crates/monocode-gpui/src/conversation/mobile.rs:113-118`) and background loads never wake (`crates/monocode-gpui/src/cloud_wake.rs:5-7`). Host reply wait is 25 s (`crates/monocode-core/src/mobile/host.rs:56`), a wake takes minutes. | Existing `conversation.send`; on the web transport only (QUIC phones keep today's semantics, H-10): for a cloud session whose instance is not connected the desktop calls `wake_for_send`, reuses the existing deferral (the reload after a wake flushes queued sends) and replies `{ok: true, wake: true, queued: true}` at once; a second send while one is pending gets `code: busy`; progress through the `cloud` watch field (architecture.md 6.6) | Before sending to a `stopped` or `stopping` session show a cost notice (X-15) and confirm; then a "Waking the codespace" state, the message shown as queued, the reply arrives asynchronously. Disabled for read-only devices and for `unavailable`. A send while the instance is starting is refused (X-10). Gap: a wake can outlast iOS background suspension; on resume the watch resync shows the real state. No push for "instance ready" in v1 | P1 |
| X-12 | Cloud instance list (codespaces and sandboxes, state, sessions using each, last used) | Not in mobile today (`APP_METHODS` has no `cloud.*`, `host.rs:22-54`; unknown methods answer "Unsupported mobile operation", `host.rs:693-695`). Desktop source: `CloudApi::list` `cloud_api.rs:571`, `workspaces_overview` `crates/monocode-core/src/cloud_sessions/workspaces.rs:1-30` (cached 30 s, never wakes). | New read method `cloud.instances` | Screen "Cloud" per desktop listing instances of selected projects with state badge, last used, and the sessions on each (shared instance warning). Delete is never offered. The list comes from the cached overview (30 s TTL) with cost estimates, never a fresh `gh` or `sbx` call, and is rate limited. Failed and orphan instances that still bill (`crates/monocode-core/src/cloud_sessions/footer_status.rs:28-41`) are listed by name only for devices whose selection is all projects; other devices see a count ("2 other instances on this desktop") | P1 |
| X-13 | Stop a cloud instance | Not in mobile today. Desktop source: `CloudApi::stop` `cloud_api.rs:681`, `stop_target` `:697`, `plan_instance_action` `cloud_sessions/instance_action.rs`. | New control method `cloud.stop` | "Stop instance" with a confirm that names busy sessions and the sessions sharing it (the desktop `warning`); result shown through state (`stopping` then `stopped`). `stopped: true` only means requested: the instance row carries `stopReport` (`stopping`, `takingLong`, `stillRunning`, from `crates/monocode-core/src/cloud_sessions/stopping.rs:54-62`) and the UI shows it; a stop while the desktop archives or converts on that instance answers `code: instance_busy`. Stop needs no cost notice. Control devices only | P1 |
| X-14 | Resume a cloud instance without sending | Not in mobile today. Desktop source: `CloudApi::resume` `cloud_api.rs:728` (blocks for minutes, needs a ready session). | New control method `cloud.resume` (async, progress in watch) | "Resume" with cost notice and confirm; no resume needed when the user just sends (X-11). Rate limited | P2 |
| X-15 | Cost notice for billable cloud actions | Not in mobile today. Desktop facts: codespaces billed per core-hour (estimate $0.09, `workspaces.rs:65`), sandboxes per runtime second (`crates/monocode-core/src/cloud_sessions/workspaces.rs:50-52`; `workspaces.rs:45-60` has the rate table); any send, resume or start bills (`sandboxes.rs:15-18`). | none (text from `cloud.instances` estimates when cached) | Notice before send-to-stopped, resume and start: "This starts the codespace and uses compute you pay for." Estimates labelled "estimate". The web never wakes anything by reading | P1 |
| X-16 | Start a new cloud session (codespace or sandbox) | Not in mobile today: `session.start` always creates a local session for a rail project (`mobile_host.rs:1527-1578`) and has no `target`. Desktop source: `CloudApi::begin_start` `cloud_api.rs:948`. | New read method `cloud.startOptions {projectId}` (backend readiness, branches, allowed codespace machines, sandbox size presets) and `session.start` with `target` (H-11, H-13), behind the desktop toggle "Remote cloud start" (default off) | Target picker on the New session page, only when the desktop reports the toggle on; `target` accepts only `kind`, `instance` ("new" or a name from `cloud.instances`), `branch`, `machine` (codespace, from the list) or `size` (sandbox preset); the Claude kit follows the desktop's saved preference (`cloud_sessions/kit_prefs.rs`); the client can never set `kit`, `template`, `image`, `cpus`, `memory` or a `ttl` (rejected `code: forbidden`). Claude only (`cloud_sessions.rs:94`), no draft, no worktree. Progress as X-10. Cloud delete is never offered anywhere | P2 |
| X-17 | Move progress display | Not in mobile today. Desktop source: 7 steps (`snapshot`, `push`, `workspace`, `upload`, `restoreCode`, `restoreConversation`, `flip`) in `crates/monocode-core/src/cloud_sessions/move.rs:164-176`. | none: read-only `cloud.move {step, status}` field in watch frames for a move started on the desktop (architecture.md 6.6) | Progress list of the 7 steps with status, shown on the session while a move runs; hidden otherwise. Gap: resyncs on resume (N-16) | P1 |
| X-18 | Move to cloud, continue locally, cancel a move | Not in mobile today. Desktop source: app API move actions `crates/monocode-core/src/mono/move_api.rs:1-17` | New control methods `cloud.moveToCloud`, `cloud.moveToLocal`, `cloud.cancelMove` (architecture.md 6.6) | Menu actions with a cost notice and confirm. Allowed only while both desktop toggles "Remote cloud start" and "Remote push in Git" are on, because a move pushes a branch (`crates/monocode-core/src/cloud_sessions/move.rs:28-30`); otherwise hidden. Cancel never deletes a workspace | P2 |
| X-19 | Archive or delete a cloud session | Mobile host calls `toggle_archive_now` (`crates/monocode-gpui/src/shell/mobile_host.rs:1433-1437`) and skips the keep/stop question and the local-copy conversion that the app API asks for (`crates/monocode-gpui/src/shell/cloud_app_archive.rs:27-35`). | `session.archive` requires `cloud: "keep"` or `"stop"` for cloud sessions; `session.delete` on a cloud session answers `code: unsupported_target` | Archive dialog asks keep or stop and shows a cost notice, because the conversion may wake the instance. Control only. Delete is hidden for cloud sessions ("Delete on the desktop") | P1 |
| X-20 | Retry a failed start and handle a kept instance | Not in mobile today. Desktop source: failed and orphan instances keep billing (`crates/monocode-core/src/cloud_sessions/footer_status.rs:28-41`). | New control method `cloud.retryStart {sessionId}` | Retry button with a cost notice, rate limited (H-10 limit). Rebuild, open in browser and delete codespace stay desktop-only (denied). When a failed start left a billable instance the `cloud` object carries `kept: true`: the UI says so and Stop is allowed on it | P1 |
| X-21 | Auto-stop notice and keep running | Not in mobile today. Desktop source: `crates/monocode-gpui/src/conversation/auto_stop.rs:7-9`. | `cloud` object field `autoStop {at, reason}` (read); new control method `cloud.keepRunning {sessionId}` (P2) | Notice "Stops at HH:MM, <reason>" (P1). A send during an auto-stop answers `code: auto_stopping` and the UI says so. A web send counts as user activity, watching does not. "Keep running" is the P2 part | P1 |

### 2.17 Explicit drops and changed behaviour

Every F row ships unless it is listed here.

| Item | Mobile source | Decision | Reason |
|---|---|---|---|
| F-01 in-app QR scanner | `mobile/src/features/connection/ConnectScreen.tsx:40` | Dropped; the camera app opens the pairing link | `BarcodeDetector` is missing on Safari; a JS decoder (zxing-wasm) plus camera permission is not worth it |
| F-82 local-notification fallback | `mobile/src/features/notifications/SessionNotifications.tsx:153` | Dropped | The web has Web Push only |
| F-93 accelerometer mascot rain | `mobile/src/features/mascots/MascotRain.tsx:67` | Dropped | `DeviceMotionEvent.requestPermission` on iOS, hardware specific |
| Foldable typing posture | `mobile/modules/monocode-link/ios/TypingPostureView.swift`, `mobile/src/features/sessions/TypingPostureFocus.tsx` | Dropped | Hardware specific |
| Mono row long-press peek | `mobile/src/features/workspace/MonoHome.tsx:430` | Dropped; a click opens the chat | No long-press on pointer devices |
| Keyboard drag-to-dismiss | `mobile/src/features/sessions/SessionScreen.tsx:83` | Not applicable on web | The browser owns the keyboard |
| Foreground notification suppression | `mobile/src/features/notifications/SessionNotifications.tsx:17-26` | Changed: Web Push must show every notification (iOS revokes silent pushes), so the service worker sets `tag` to the session id and the app closes that session's notifications (`registration.getNotifications({tag})`) when it opens or shows the session | Web Push rule, architecture.md section 8 |
| Attachment total | `mobile/src/features/sessions/attachments.ts:24` (24 MiB total base64, 10 MiB per file) | Changed: the web allows 8 MiB total per message and lowers the per-file cap to match | Relay reassembly limit of the Noise channel (architecture.md section 5.2) |
| Mono-launched sessions and habit runs outside the selection | `mobile/src/app/session.tsx:19-21` (a Mono can start a session in any project, picked for the phone or not) | Changed: selection is enforced on the desktop for every device, so a web device with a narrower selection gets `code: not_selected` for such a session; the card shows "Not on this device's projects" with no open action (F-72) | Desktop-enforced selection (security.md section 7, G1) |
| Haptics | global rule, section 1.3 | Dropped; visual cue instead | No haptic API on iOS Safari |
| `device.push` with Expo tokens | `mobile/src/features/notifications/SessionNotifications.tsx:48` | Answered with `code: unsupported` on the web transport | Replaced by Web Push subscriptions held by the server (N-14) |
| `project.fileHighlight` | `mobile/src/features/files/CodeView.tsx:90` | Answered with `code: unsupported` on the web transport | shiki on the client replaces desktop highlight jobs |

## 3. Desktop method coverage matrix

Permission: R = read-only device allowed; C = control device only. The desktop enforces this in `MobileHost::handle` (`crates/monocode-core/src/mobile/host.rs:640`), not the web client. Risk codes (R0..R5) used in the notes below: R0 metadata, R1 reads content, R2 UI-level state, R3 drives the agent, R4 destructive, R5 repo writes visible outside the machine.

### 3.1 `APP_METHODS` (31 entries, `crates/monocode-core/src/mobile/host.rs:22-54`)

| Method | Used by | Read / mutate | Min permission | Web priority | Notes |
|---|---|---|---|---|---|
| `conversation.read` | F-32, fallback poll | read | R | MVP | |
| `conversation.older` | F-32 | read | R | MVP | |
| `models.list` | F-51, F-52, F-56, F-57 | read | R | MVP | Reply must drop `full-access` from `runtimeModes` for every remote device. |
| `accounts.usage` | F-54 | read (cached usage); `refresh` is control | R without `refresh`, C with `refresh` | P1 | |
| `session.configure` | F-51..F-53 | mutate | C | MVP | Rejects `runtimeMode: "full-access"` from every remote device. |
| `project.changes` | F-64, F-68 | read | R | P1 | Diff content subject to the deny-list. |
| `project.worktrees` | F-19 | read | R | P1 | |
| `project.files` | F-59 | read | R | P1 | Deny-list; hides `.git`. |
| `project.file` | F-60 | read | R | P1 | Deny-list for `.env` and similar. |
| `chat.mentionedFile` | F-45, F-60 | read | R | P1 | Needs capability `mentionedFiles`. Only files an assistant message of the conversation mentions (`forbidden` otherwise); outside the device's projects the file must also be named by a completed file tool, shell command or output, or subagent output, or (sessions only, not Monos) have changed since the conversation began inside the session's own folder (the last two need capability `mentionedFiles2`, security.md T17); deny-list `path_denied`; pages of at most 4 MiB. |
| `project.search` | F-61 | read | R | P1 | |
| `project.fileHighlight` | F-60 (mobile only) | read | R | not used on web | Answers `code: unsupported` on the web transport (section 2.17); shiki replaces desktop highlight jobs. |
| `project.git` | F-65, F-67 | read | R | P1 | |
| `project.gitAction` | F-64..F-66 | mutate (R5 for push/PR) | C | P1 (stage, commit) | `sync`, `pr` and a pushing `commit` are denied for web unless the desktop toggle "Remote push in Git" is on (security.md section 7); confirm dialog when on. |
| `session.start` | F-31 | mutate | C | MVP | Rejects `full-access` from every remote device. |
| `conversation.send` | F-47, F-48, F-50, F-71, F-72 | mutate | C | MVP | Chunked attachments; max 10 files, 100,000 chars. |
| `session.resumeUsage` | F-55 | mutate | C | MVP | |
| `session.resumeUsageAtReset` | F-55 | mutate | C | MVP | |
| `session.answerQuestion` | F-58 | mutate | C | MVP | |
| `session.keepQuestionOpen` | F-58 | mutate | C | MVP | |
| `session.cancel` | F-28, F-47 | mutate | C | MVP | |
| `session.rename` | F-25, F-40 | mutate | C | P1 | |
| `session.delete` | F-25, F-40 | mutate (R4) | C | P1 | Confirm dialog; consider desktop-side confirm for web devices later. Answers `code: unsupported_target` for a cloud session (X-19). |
| `session.archive` | F-40, X-19 | mutate | C | P1 | For a cloud session `cloud: "keep"` or `"stop"` is required (X-19). |
| `mono.document` | F-63, F-72 | read | R | P2 | |
| `mono.profile` | F-74, F-76, F-77 | read | R | P2 | |
| `mono.update` | F-75, F-78 | mutate | C | P2 | |
| `mono.reset` | F-70 | mutate (R4) | C | denied for web in v1 (security.md section 7) | Hidden in the UI. |
| `mono.delete` | F-70 | mutate (R4) | C | denied for web in v1 (security.md section 7) | Hidden in the UI. |
| `mono.create` | F-75 | mutate | C | P2 | |
| `mono.file` | F-79 | read | R | P2 | |
| `mono.saveFile` | F-79 | mutate (R3, steers the agent) | C | denied for web in v1 (security.md section 7) | Hidden in the UI; Soul and Memory are read-only on web. |

### 3.2 Host-handled methods (`crates/monocode-core/src/mobile/host.rs:654-690`, watch at `:734`)

| Method | Used by | Read / mutate | Min permission | Web priority | Notes |
|---|---|---|---|---|---|
| `device.enroll` | F-04 | mutate | none (invitation only) | MVP | Obsolete in current form (QUIC invitation identity with private key). Replaced by the Noise-based pairing plus desktop confirmation (N-07, N-08). |
| `device.leave` | F-08 | mutate | R (own device) | MVP | A device may always remove itself. |
| `workspace.list` | F-05, F-06, F-11, F-12 | read | R | MVP | Must be filtered by the device's project selection; for web devices `addresses` are stripped; the catalog still carries absolute paths inside `projectId`, an accepted gap (G13, security.md section 7); the UI shows display names only and never prints paths in URLs it builds itself. |
| `sessions.list` | F-21 | read | R | MVP | Filtered by selection. |
| `device.push` | F-80, F-81 | mutate | R (own device) | obsolete on web | Answers `code: unsupported` on the web transport (section 2.17); Expo tokens do not apply. Replaced by Web Push subscriptions held by the server (N-14); the desktop only reports events. |
| `conversation.watch` | F-32 | read (stream) | R | MVP | Max 4 per device; heartbeat 25 s on the web link (10 s on QUIC); resume with `from`. |

### 3.3 Methods to add

| Method | Purpose | Permission | Priority |
|---|---|---|---|
| `session.answerApproval` | Answer a tool permission request (X-01) | C | P1 |
| `capabilities: string[]` and `desktop.version` in the `state` message (a field, not a method) | N-23 | R | MVP |
| `device.selection` (get/set) | Per-device project and Mono selection stored on the desktop (F-10). Get is read; set may only narrow; widening needs confirmation on the desktop | get R, set C (narrow only) | MVP |
| `device.confirm` (desktop UI event, not a wire call) | Approve a new device by name + fingerprint (N-08) | desktop operator | MVP |
| `device.permission` (desktop UI action) | Switch a device between read-only and control (N-11) | desktop operator | MVP |
| `session.answerPlan`, plan/todo frame fields | X-02, X-03 | C | P2 |
| `target`, `cloudState`, `startup` on `workspace.list` and `sessions.list`; `cloud` (with `move`, `autoStop`, `kept`) on `conversation.read` and watch frames (fields, not methods; `cloud_api::annotate`) | X-09, X-10, X-11, X-17, X-20, X-21 | R | P1 |
| `cloud.instances` | List codespaces and sandboxes (X-12); cached, rate limited | R | P1 |
| `cloud.stop` | Stop an instance (X-13); no cost notice | C | P1 |
| `cloud.resume` | Resume an instance, async (X-14) | C | P2 |
| `cloud.startOptions {projectId}` | Backend readiness, branches, allowed machines and size presets for the start picker (X-16) | R | P2 |
| `session.start` with `target` | Start a cloud session (X-16); toggle "Remote cloud start" | C | P2 |
| `cloud.retryStart {sessionId}` | Retry a failed start (X-20); cost notice, rate limited | C | P1 |
| `cloud.keepRunning {sessionId}` | Cancel an auto-stop (X-21) | C | P2 |
| `cloud.moveToCloud`, `cloud.moveToLocal`, `cloud.cancelMove` | Session moves (X-18); only while "Remote cloud start" and "Remote push in Git" are on | C | P2 |
| `cloud.delete`, `cloud.create`, `pullEvidence` | Not exposed (irreversible or billable; the Operator cannot delete either, `crates/monocode-core/src/app_access.rs:45`) | denied | none |
| Audit log entries (desktop internal, no wire method) | Record enroll, revoke, mutating calls with device id and command id | n/a | P1 |

## 4. New web features

| ID | Feature | Web behaviour | Server / desktop dependencies | Pri |
|---|---|---|---|---|
| N-01 | Registration with invite code | `/register#invite=CODE` (the code sits in the URL fragment, so it stays out of server logs and history): enter the one-time invite code, choose an immutable `handle` (the login and recovery identifier) and a `displayName`, create a passkey, then the recovery codes screen; afterwards the app prompts to add a second passkey. Invalid or expired codes show a generic error. | Worker `/api/auth/*`, D1 `invites`, `users`; WebAuthn via `@simplewebauthn`. | MVP |
| N-02 | Passkey sign-in (passkeys only, multiple per account) | `/login` with a "Sign in with passkey" button (usernameless/discoverable). Settings > Security lists passkeys, add, rename, remove (any passkey except the last one can be removed). Gap: iOS passkey prompts need a user gesture; works inside the installed PWA with iCloud Keychain. | `/api/auth/*`, D1 `passkeys`. | MVP |
| N-03 | Cross-device sign-in by QR | In the browser sign-in dialog, "Use a phone or another device" uses the browser's hybrid (QR) WebAuthn flow; no extra server code. Gap: depends on browser support; desktop Chrome/Safari support it. | none beyond N-02. | P1 |
| N-04 | Recovery codes | 10 one-time codes shown once at registration (copy/download/print, confirm "I saved them" before continuing); can be regenerated while signed in (invalidates old ones). The codes download includes the handle. Signing in with a recovery code forces adding a new passkey and the app prompts to regenerate when 2 or fewer codes remain. Server stores hashes only. Losing passkeys and codes loses the account (journeys: security.md section 3.6). | D1 `recovery_codes` (hashes), rate limits. | MVP |
| N-05 | Sessions list and sign out everywhere | Settings > Security > "Signed-in browsers": list with last used, current marked; sign out one or all. | D1 `sessions`; cookie revocation. | P1 |
| N-06 | Invites management | Settings > Invites: remaining quota (default 3), create code with expiry (default 7 days), copy link, revoke unused, see who joined with each (display name only). | D1 `invites`, `users.invited_by`. | MVP |
| N-07 | Desktop pairing (web side) | Open the link (or scan the QR with the iPhone camera) shown by the desktop. Page `/pair/:code`: sign in if needed (the `#fp` fragment survives the `/login?redirect=` round trip by being kept in `sessionStorage`), shows machine name, the desktop fingerprint and a confirmation code, "Approve" button; the server binds the desktop public key to the account and returns a revocable, narrowly scoped token to the desktop. The browser then pairs as a device (N-08). Link expires in 10 minutes, single use. A missing `#fp` shows a warning and a mismatch blocks approval. An iPhone camera scan opens the link in Safari, not in the installed PWA, so the user signs in there and the PWA pairs as a new device afterwards. | Worker pairing endpoints (device-authorization style), D1 `desktops`; desktop: Settings > Connections > "Web app" section (host URL input, default `https://zptah.app`, Pair button, QR + copyable link, countdown, device list with revoke). | MVP |
| N-08 | New device confirmation on the desktop | Every new browser/device connecting to a desktop appears in a dialog on the desktop with name and fingerprint; until approved the web shows "Waiting for approval on your desktop" with the code. Every device needs one confirmation per desktop; it requests its grant on a desktop when the user first opens `/d/:desktopId` or taps "Request access" in the switcher, never for all desktops at once. The account is a directory, not the trust root. | Desktop dialog (reuses `crates/monocode-gpui/src/modal.rs`); `Device` record gets `permission`, `kind`, `created_at`, `last_seen`. | MVP |
| N-09 | Multi-desktop management screen | `/desktops` and `/desktops/:id` (ticket U-01, Phase 4): desktops list with name, color, online status (relay presence), last seen and version; rename, color, revoke desktop; per desktop the devices and their permission with "lower now" and "request raise", revoke device, an "add another device" link; an account-wide "This account's devices" list (name, fingerprint, last seen, forget device). Navigation groups sessions by desktop under `/d/:desktopId/...`. | D1 `desktops`, `devices`, `device_grants`, DO presence; desktop must reconnect to the relay. | MVP |
| N-10 | Revoke desktop or device | Revoking a desktop removes it from the account and invalidates its token; revoking a device removes its key from the desktop allowlist via the relay (and the desktop UI removes it too). | Server token revocation; desktop `MobileHost::revoke`. | MVP |
| N-11 | Device permissions (read-only vs control) | Shown per device on the web and on the desktop; default for a new web device is "control" only if the desktop operator picks it in the confirmation dialog (default option: read-only). UI shows a "Read-only" chip and disables mutation controls. Changing it is a desktop-side action. | Desktop `Device.permission` and enforcement at `MobileHost::handle`. | MVP |
| N-12 | Desktop switcher | Slim row at the top of the `/d` frame (there is no separate header there) listing desktops with status dots; remembers the last desktop. The app nav icons (Desktops, Account, Install, and Search on a phone) sit right aligned on the same row. | `/api/desktops`. | MVP |
| N-13 | Install guide | Per-platform page (iOS: Share > Add to Home Screen; Android: Install; desktop Chrome/Edge: install icon) with illustrations; detects standalone mode with `display-mode: standalone` / `navigator.standalone`. | `vite-plugin-pwa` manifest. | MVP |
| N-14 | Push permission onboarding | The button shows only when `isStandalone && PushManager` on every platform (after install); a screen explains the three categories and an "Enable notifications" button; `Notification.requestPermission()` is called only from that tap. Denied state shows how to re-enable. Gap: iOS shows no prompt in a normal Safari tab. | Service worker; `GET /api/push/vapid-key`, `POST /api/push/subscriptions`. | MVP |
| N-15 | Per-desktop notification toggles | Settings > Notifications: per desktop, the three categories on/off; a global "Pause all" switch. Stored with the subscription so the server filters before sending. | D1 `push_subscriptions`, `push_prefs`; Worker Web Push sender (VAPID, WebCrypto). | MVP |
| N-16 | Resync on resume | On `visibilitychange` to visible, reconnect the relay WebSocket, re-run `conversation.watch` with `from`, refetch `workspace.list`. Gap: iOS suspends the WebSocket about 10 s after backgrounding. | Link client. | MVP |
| N-17 | Offline / reconnect banner | Banner states: "You are offline", "Desktop offline" (relay says the desktop is not connected, with last seen), "Reconnecting...". Cached catalog stays readable; mutations are blocked. | DO presence; service worker caches the app shell only (no conversation content in Cache Storage). | MVP |
| N-18 | Responsive layouts | Mobile: single column, drawer. Tablet: sidebar + content. Desktop (classic layout): workspace sidebar + project session list sidebar + chat pane edge to edge with session tabs and split panes (N-24) (+ optional files/changes panel); keyboard shortcuts (Cmd/Ctrl+K, Cmd/Ctrl+P, Esc). Breakpoints in [design-system.md](design-system.md). | none | P1 |
| N-19 | Admin page | `/admin` for admin accounts: list users (invited by, created, last seen, desktops count), block a user, block an invite branch (user and everyone they invited), grant extra invites. No access to desktop data. | D1 `users.status`, `users.role`; `audit_log` entries. | P1 |
| N-20 | Account settings | Display name, delete account (removes desktops and subscriptions; requires passkey confirmation). | Worker `PATCH /api/me` (session), `DELETE /api/me` (fresh re-auth). | P2 |
| N-21 | Audit view | Settings > Security > Activity: server-side events (sign-ins, passkey changes, desktops linked or revoked, invites used). Desktop-side audit lives on the desktop (calls per device); an optional viewer in the desktop settings is out of this document. | D1 `audit_log`; `GET /api/audit` (own events). | P2 |
| N-22 | Update prompt | When the service worker has a new version, show "Update available" toast; reload on confirm. Never auto-reload during a send. | `vite-plugin-pwa` prompt mode. | P1 |
| N-23 | Version and capability handling | The encrypted `state` message (architecture.md 3.3 step 4) carries `capabilities: string[]` (for example `approvals`, `cloud`, `cloudStart`, `cloudMove`, `gitPush`, `monoTools`, `monoFiles`) and `desktop.version`. The web hides features the desktop does not advertise and shows "Update Monoptah on your desktop to use <feature>". A `proto` mismatch shows a blocking screen ("Update Monoptah on your desktop" or "Reload to update the web app"). Mirrors mobile ("Update or reload Monocode on your desktop...", `mobile/src/features/connection/useProjectChanges.ts:41`) but says Monoptah. | Desktop `state` message; ticket W-22. | MVP |
| N-24 | Session tabs and split panes | From 640px the chat area has a tab strip per pane (sessions and Mono chats): open, switch, close (button, middle click, shortcut), reorder (drag, Alt+Arrow), overflow scroll, persisted per desktop. From 1024px, or on a tablet wide enough for two 320px panes, panes split right or down, tabs drag into a pane or onto its edge, panes resize and close; the focused pane drives the URL. Shortcuts use Cmd on macOS and Ctrl elsewhere: Mod+D split right, Mod+Shift+D split down, Mod+Alt+W close tab, Mod+Alt+] and [ next and previous tab, Mod+Alt+1..9 tab n (9 is the last), Mod+Alt+Shift+arrows focus another pane (the browser keeps Mod+W, Mod+1..9 and Ctrl+Tab). Below 640px nothing changes: one session at a time. Details in [design-system.md](design-system.md) section 11.2. | `localStorage` `monoptah.layout.tabs.<desktopId>`. Native reference: `crates/monocode-gpui/src/shell.rs` (keymap at 262-325, `split_session`, `close_pane`), `pane_layout.rs`. | P1 |

## 5. Summary: MVP scope at a glance

Feature counts (rows in sections 2.1 to 2.16 and 4; section 2.17 lists drops, not features): F = in mobile today, X = requested but not in mobile, N = new web feature.

| Priority | F | X | N | Total |
|---|---|---|---|---|
| MVP | 43 | 0 | 16 | 59 |
| P1 | 31 | 12 | 5 | 48 |
| P2 | 22 | 9 | 2 | 33 |
| Total | 96 | 21 | 23 | 140 |


| Area | MVP includes |
|---|---|
| Account | Invite-code registration (the first admin uses a script-created bootstrap invite), passkey sign-in, recovery codes, invites (quota 3), sign out |
| Desktops | Pair from desktop link/QR, desktop-side confirmation, management screen (rename, color, devices, permissions, revoke, account-wide device list), switcher, version and capability handling (N-23), read-only vs control |
| Sessions | Sessions list with search, project and sessions routes, live "now working" pill, start session, stop |
| Transcript | Watch with incremental patches, paging older turns, markdown (safe), fold line, tool phases, copy, errors |
| Composer | Text, attachments (10 files), model and effort picker, access modes without full-access, usage limit card |
| Interaction | Questions (Q&A card) |
| Notifications | Web Push (minimal payload), install guide, permission after install, per-desktop toggles, tap routing |
| Shell | Mobile layout, offline/reconnect banner, resync on resume, light/dark |
| Desktop work | WebSocket+Noise transport, Settings > Connections "Web app" section, project selection enforcement, permission checks, deny-list, audit log |
| Not in MVP (all still required for parity, Phase 6 of roadmap.md, except the Phase 7 backlog) | Cloud session display, wake and instance list (P1), approvals (P1), file viewer/tree/search (P1), changes/diff/git (P1), Mono features, themes, tablet/desktop multi-pane polish, dictation, visuals, backgrounds |
