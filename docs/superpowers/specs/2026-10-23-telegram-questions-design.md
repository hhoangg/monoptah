# Telegram Questions - Design

Date: 2026-10-23. Status: agreed in chat with the user; this document records it.

## 1. Intent

Agents stop and wait whenever they ask the user a question (Claude `AskUserQuestion`, Codex
`requestUserInput`, Cursor/OpenCode/Grok/OMP questions). When the user is away from the
desk, a workflow or a long session sits idle until they come back. The mobile companion
only works on the same network and cannot answer questions.

This feature forwards a question that has gone unanswered for 5 minutes (the default of a
per-project setting, section 14) to a Telegram
group over the internet, with enough context to answer it, and routes the answer given
in Telegram back to the exact session and question, so the agent continues.

## 2. Goals

1. Per-project Telegram configuration: each project has its own bot token and its own
   group (a supergroup with Topics enabled).
2. Forward **questions** (not approvals) from **every session of a configured project**,
   when a question is still pending **5 minutes** after it was asked (a default; section 14).
3. One forum topic per session. Default: the bot creates it, named `[<ticket id>] <title>`
   when the session has a ticket, otherwise `<title>`. Alternatively the user links an
   existing topic to a session.
4. The message carries context and one inline button per option; free text ("Other") is
   answered by replying to the bot message.
5. **Any member of the group** may answer. **First answer wins**, desktop or Telegram.
6. Answers arrive in near real time (long polling), are validated, and reach the session
   through the same code path as answering in the app.
7. The Telegram message is updated to show the outcome (answered by whom, answered on
   desktop, expired) and its buttons are removed.
8. Respect Telegram rate limits with a send queue and `retry_after` handling.

## 3. Non-goals (v1)

- Approvals (Allow/Deny), plan "Build", orchestration proposals. The design must leave
  room for approvals later.
- Surviving an app restart: the app is assumed to stay open. Pending questions live in
  memory. (Topic bindings are persisted, see section 8.)
- Webhooks or any server component.
- TUI (terminal) sessions: the app has no question content for them.
- Remote-host (SSH) sessions: out of scope for v1 unless trivially covered by the same
  answer path (builder's call, see section 12).
- Mono-relayed questions (habits/workflows relay approvals only today).

## 4. Anti-goals

- An answer reaching the wrong session or a stale question. Every answer must match
  `(sessionId, requestId)` of a question that is still pending.
- Strangers answering: only updates from the configured group (and topic) count.
- Leaking the bot token: it never appears in logs, settings files, exports or the UI after
  saving.
- Spamming the group or getting the bot throttled into silence.

## 5. Behaviour

### 5.1 Configuration (per project)

A "Telegram" section in the project's settings (exact placement: builder's call, next to
existing per-project settings). Fields and actions:

- **Bot token** (stored like the ClickUp token: a 0600 file in app data; the codebase moved away from the OS keychain). "Test" calls `getMe`
  and shows the bot username.
- **Connect group**: the app shows a one-time code and a deep link that adds the bot to a
  group with the "Manage topics" right (section 13.4). The app records `chat_id` and confirms in the
  group. Shows whether the group has Topics enabled and whether the bot is an admin with
  "Manage topics" (`getChatMember`); if not, a clear note explains how to fix it.
- **Enabled** toggle. Delay is fixed at 5 minutes in v1 (constant, not a setting). Superseded:
  the delay is now a per-project setting (section 14).
- Warning when the same bot token is configured in two projects with different groups is
  fine (one poller per token, routing by chat), and a note that running two Monoptah
  instances (installed + dev) on the same bot makes `getUpdates` conflict (HTTP 409); the
  app shows "Another app is reading this bot" when it gets 409.

### 5.2 When a question is forwarded

- A session of a configured project gets a pending question (`Session.pendingQuestion`).
- A per-question timer starts at the moment it was asked. If the question is answered,
  skipped or cancelled within the delay (5 minutes by default, section 14), nothing is sent.
- After the delay (5 minutes by default) still pending: the app forwards it (section 5.3). Codex non-blocking
  questions are not forwarded (section 13.3).
- If the question was asked while Telegram was disabled or unconfigured, nothing is sent
  later for it.

### 5.3 Topic for the session

- If the session already has a topic binding for this project's group, use it.
- Otherwise create one with `createForumTopic`, name `[<ticket id>] <title>` or `<title>`,
  trimmed to Telegram's 128-character limit. Persist the binding (section 8).
- Ticket id comes from what the session is linked to: a ClickUp/Jira/Linear inbox item the
  session was started from (ClickUp custom id or task id, Jira key, Linear identifier), a
  GitHub linked work item (`#<number>`), or a workflow For each item's id. Exact field
  extraction is the builder's call; when nothing is linked, no ticket prefix.
- **Link an existing topic**: in the session's menu, "Link Telegram topic" shows a code;
  the user sends `/link <code>` inside the desired topic; the app binds that
  `message_thread_id` to the session and confirms in the topic.
- If the group has no Topics or the bot cannot create topics, post in the group's general
  chat and note the problem once in the app (no crash, no retry loop).

### 5.4 The message

One Telegram message per question (a prompt with several questions, e.g. Claude with up
to 4, becomes several messages; the reply is sent to the agent only when every question
of the prompt is answered or the prompt is skipped). Content (HTML parse mode, escaped):

- Header: project, session title, ticket, and when part of a workflow: workflow name,
  node, item, round.
- "Waiting for <delay>" (the configured delay, omitted when it is 0; section 14).
- Context: the tail of the agent's last assistant message before the question (why it is
  asking), trimmed (about 600 characters).
- The question header and prompt, then option labels with descriptions.
- Inline keyboard:
  - single select: one button per option, plus "Skip";
  - multi select: toggle buttons (show a check mark when selected) plus "Send" and "Skip";
  - "Other" / custom allowed: a button "Other (reply with text)" that sets the message to
    expect a reply; any reply to the bot message in that topic is taken as the custom
    answer.
- Answers via buttons use `callback_query`; free text uses a message whose
  `reply_to_message.message_id` is the question message.

### 5.5 Receiving and routing an answer

- One long-polling loop per bot token (`getUpdates` with a long timeout, e.g. 50 s, and
  `allowed_updates = ["message", "callback_query"]`), run in Rust so it is independent of
  any window being focused or visible.
- Accept only updates whose chat is a configured group for that token. Callback data must
  be short (Telegram limit 64 bytes): use a compact token that maps to the pending entry.
- Look up the pending question entry, then send the answer to the window that owns the
  session (the same owner-routing idea the mobile bridge uses with `pick_owner`). The
  frontend checks the session still has a pending question with the same `requestId` and
  answers through the same function the in-app form uses (`respondHarnessQuestion`,
  via the existing `onQuestionReply` path so the remote-project branch keeps working).
  Owner routing does not reuse `pick_owner` (its catalogs exist only when the mobile
  companion is on): the window that posts a question to Rust is recorded in the pending
  entry as the owner, and answers are emitted to that window label.
- The frontend reports the result back: answered, or stale (already answered, skipped,
  cancelled, unknown session).
- `answerCallbackQuery` is sent as soon as the tap is received (before the frontend round
  trip), because Telegram rejects late answers ("query is too old").
- Rust then edits the Telegram message ("Answered by @user: <labels>", or "Already
  answered on desktop", or "This question has expired"), removes the buttons, and calls
  `answerCallbackQuery` with a short toast.
- The app records in the session that the question was answered from Telegram and by whom
  (a small note in the transcript or on the question block; builder's call on form).

### 5.6 Desktop answers after forwarding

When a forwarded question is answered on the desktop, skipped, or cancelled (turn ended,
session stopped, provider handoff, orchestration lead answering), the app edits the Telegram message to "Answered on desktop" / "Skipped" /
"Cancelled" and removes the buttons. Detection watches the session's `pendingQuestion` disappearing or
changing `requestId`, not the `onQuestionReply` call, because some answers bypass it.

## 6. Rate limiting and retries

- Telegram limits (approximate, public guidance): about 20 messages per minute per group,
  about 1 message per second per chat, about 30 messages per second per bot. Edits and
  topic creation count as sends.
- A send queue per group with a token bucket of 20 per minute and at least 1 s spacing,
  plus a global bucket per bot of 30 per second.
- Priorities: new question messages and topic creation first, status edits after.
  Pending edits to the same message are coalesced (only the latest is sent).
- HTTP 429: read `parameters.retry_after`, pause that group's queue for that long, then
  retry the same item. Network errors: exponential backoff (1 s, 2 s, 4 s, up to 60 s),
  keep the item. 400/403 (bad chat, bot removed, no rights): drop the item, surface a
  one-line problem in the project's Telegram settings.
- `answerCallbackQuery` is sent immediately (it is a reply to the tapping user, needed
  within seconds) and does not go through the group queue.

## 7. Security

- Bot token in a 0600 config file in app data, like the ClickUp token (the codebase moved
  away from the OS keychain). Never logged; the settings UI shows only the bot username after saving.
- Only updates from configured `chat_id`s are processed; everything else is ignored.
- Any member of the configured group may answer (user decision). The answering user's
  Telegram name/username is recorded with the answer.
- Callback tokens are random and single-use per question; a stale token gets "This
  question has expired".
- Message text from Telegram is treated as data: it becomes the custom answer string and
  nothing else.

## 8. Storage

- **Pending questions**: in memory in Rust (the Telegram bridge state): message id ↔
  `(project, window, sessionId, requestId, questionId, options, selected set, prompt
  progress)`. Rationale: the Rust process outlives windows being hidden or reloaded, and
  it is where the poller runs. The truth about whether a question is still pending stays
  in the session (frontend), checked on every answer.
- **Per-project config** (chat id, enabled, topic rights snapshot): app data, alongside
  other per-project integration config (builder picks the exact file, matching existing
  patterns). Token in the same 0600 file.
- **Session topic binding** (`sessionId -> message_thread_id`, per project/group):
  persisted, so a session keeps its topic across app restarts and never gets a duplicate.
- **Telegram update offset**: in memory (the app stays open). On start, the poller skips
  old updates by starting from the latest offset.

## 9. Constraints

- Fork rules in `CLAUDE.md` (branding, never-rename identifiers, `npm run tauri:dev`,
  `npm run check` read directly).
- Rust HTTP uses the existing `ureq` (blocking, in threads); no new HTTP crate unless
  clearly needed. TS cannot call api.telegram.org directly (all Telegram traffic in Rust).
- UI must follow the app's existing settings and menu design.
- English for code, comments, UI copy and Telegram message text.

## 10. Decided by the user

Questions only first; every session of a configured project; send after 5 minutes
unanswered (by default; now a setting, section 14); any group member can answer; first answer wins; per-project bot and group;
bot auto-creates one topic per session named `[ticket id] title` or `title`; user can link
an existing topic; the user is told the bot needs admin "Manage topics"; Telegram over the
internet (not the mobile companion); the app is assumed to stay open; handle rate limits
with retry.

## 11. Assistant's calls (user may revisit)

Long polling instead of webhooks; inline buttons for options and reply-to-message for
free text; one message per question; deep link / `/connect@bot <code>` and `/link@bot <code>` for setup;
pending questions in Rust memory; topic binding persisted; per-group queue with
priorities and coalesced edits; Codex non-blocking questions not forwarded; session types excluded per 13.2; general
chat fallback when topics are unavailable; English message text.

## 12. Left to the builder

Exact settings placement; exact files for config and bindings; ticket id extraction per
source; how the answer note appears in the transcript; whether remote-host sessions work
through the same `onQuestionReply` path (include them if it is free, otherwise exclude);
module boundaries; copy.

## 13. Builder-check answers (assistant's calls, 2026-10-23)

1. **Prompts with several questions.** One Telegram message per question. Each message has
   its own Skip, which skips only that question. Answers are collected per prompt in the
   Rust pending entry; when every question of the prompt is answered or skipped, the reply
   is built with `buildQuestionReply` semantics (answered subset; all skipped = skipped)
   and sent. First answer wins **per prompt**: if the desktop answers the prompt first,
   every Telegram message of that prompt is marked "Answered on desktop". Partial Telegram
   selections are not mirrored into the desktop form in v1.
2. **Which sessions.** Included: normal project sessions, including sessions started by
   Monos, habits' launched project sessions and workflow session nodes (the main use
   case). Excluded: orchestration workers (their lead answers them), hidden/ephemeral
   sessions (habit runs, Ask Mono sessions), Mono chats, Inbox Ask conversations, TUI
   sessions. Topics are created lazily, only when a question is actually forwarded, so a
   large For each only creates topics for sessions that really wait out the delay (5 minutes by default).
3. **Codex non-blocking questions** (the ones attached to a running turn that auto-skip
   after 120 s) are left alone: no `keepQuestionOpen`, so they auto-skip as today and are
   never forwarded. This avoids changing how queued follow-ups steer the turn. Codex
   blocking questions (`requestUserInput`) are forwarded like any other. A Telegram answer
   to a question that is no longer pending is reported as expired; it is never turned
   into a follow-up message.
4. **Connecting a group.** Primary: a deep link
   `https://t.me/<bot>?startgroup=<code>&admin=manage_topics` (opens Telegram, lets the
   user pick the group, requests the "Manage topics" admin right, and the bot receives
   `/start@<bot> <code>` in that group). Fallback shown in settings:
   `/connect@<bot> <code>` (privacy mode only delivers commands addressed to the bot).
   `/link@<bot> <code>` for linking a topic, matched by `message_thread_id` with
   `is_topic_message`; a message in General has no thread id and is rejected with a hint.
5. **Telegram API corrections.**
   - 409 from `getUpdates` has two causes: another poller ("Another app is reading this
     bot") or a webhook set on the token ("This bot has a webhook"; settings offers a
     "Remove webhook" button calling `deleteWebhook`). Polling pauses until resolved.
   - 400 with `parameters.migrate_to_chat_id` (group upgraded to a supergroup when Topics
     are enabled): update the stored chat id and retry the item.
   - "message is not modified" on an edit counts as success.
   - "message thread not found": the bound topic was deleted; drop the binding and create
     a new topic on the next forward.
   - Other 400/403: drop the item and surface the problem in settings.
   - Every message in a topic carries `reply_to_message` (pointing at the topic creation
     message when it is not a real reply); free-text answers match only when
     `reply_to_message.message_id` equals a pending question's message id.

## 14. Follow-ups (2026-10-24)

1. **The delay is a per-project setting.** It replaces the fixed 5 minutes of sections
   5.1 and 5.2 (and every other fixed "5 minutes": sections 1, 2, 5.1, 5.2, 10 and 13.2, which now describe the default). `ProjectConfig.delay_secs`,
   default 300 (5 minutes), range 0 to 3600 seconds ("Immediately" to 1 hour). A config
   file saved before the setting existed, and a fresh or reset config, both read as 300;
   an explicit 0 is kept. The dialog offers Immediately, 30 seconds, 1, 2, 5, 10, 15 and
   30 minutes, and 1 hour, and shows any other stored value as a custom option. The
   app checks it per question: a question is due once it has been pending as long as its
   project's delay, with 0 meaning on the first check after it appeared. A change applies
   to questions already waiting. The rule that a question first seen while forwarding was
   off or unconfigured is never sent later is unchanged. The message line reads
   "Waiting for 30 s / 1 min / 5 min / 1 h" and is left out when the delay is 0.
2. **Context summary.** (Decision recorded with the context summary follow-up.)
