# Telegram Questions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Forward agent questions left unanswered for 5 minutes to a per-project Telegram group (one forum topic per session) and route answers given in Telegram back to the exact pending question.

**Architecture:** All Telegram traffic lives in a new Rust module `src-tauri/src/telegram/` (Bot API client over `ureq`, a rate-limited send queue, a per-bot-token long-polling thread, an in-memory pending-question registry, a 0600 config file). The frontend decides what to forward (`src/features/telegram/`), sends questions to Rust through commands, listens for `monocode-telegram-answer` events emitted to its own window, answers through the existing `onQuestionReply`, and reports results and closures back.

**Tech Stack:** Rust (Tauri 2, `ureq` 2.12, `serde_json`, std threads), React 19 + TypeScript, Vitest.

**Spec:** `docs/superpowers/specs/2026-10-23-telegram-questions-design.md` (read it first; section numbers below refer to it).

## Global Constraints

- Follow `CLAUDE.md`: fork branding, never-rename identifiers, `npm run tauri:dev` only, verify with `npm run check > /tmp/tg-check.log 2>&1; echo $?` read directly (never piped).
- English for code, identifiers, comments, UI copy and Telegram message text. No em/en dash characters in new copy.
- No new crates or npm packages. HTTP via `ureq` 2.12 in blocking threads (`spawn_blocking` for commands).
- **Secret storage deviation from spec 7 (intentional):** the codebase moved away from the OS keychain (`mail_secrets.rs` encrypts to a file; ClickUp keeps its token in a 0600 JSON file). Store the bot token like ClickUp: `app_data_dir()/telegram-config.json`, written with `clickup::write_secret_file` (0600). Never log the token; never return it to the frontend.
- Per-project key = `projectKey(cwd)` in TS (`src/shared/lib/paths.ts:331`); send that exact string to Rust as `project`.
- Constants (spec): `FORWARD_DELAY_MS = 5 * 60_000`; context tail `600` chars; topic name max `128` chars; callback_data max `64` bytes; per-group budget `20` sends/minute with `>= 1000 ms` spacing; per-bot `30` sends/second; `getUpdates` timeout `50` s with `allowed_updates = ["message","callback_query"]`; network backoff 1, 2, 4 ... capped at 60 s.
- Topic name: `[<ticket id>] <title>` when a ticket id exists, else `<title>` (user decision).
- Commit on branch `feature/remote-questions` after each task, message as given, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.

## Review Focus

1. Two prompts pending at once in the same session, or the same `requestId` reused by a different session: answers must only land on the matching `(sessionId, requestId)`. (Task 4 test `routes_by_session_and_request`, Task 7 test `ignores answer for a different requestId`.)
2. A user taps a button after the question was already answered on desktop: they get "This question has expired" and the session is untouched. (Task 4 `stale_token_reports_expired`, Task 7 `reports stale when no longer pending`.)
3. Group upgraded to a supergroup (chat id migrates) right after Connect: the queued item is retried on the new chat id and config is updated. (Task 1 `classifies_migrate_to_chat_id`, Task 5 `migrated_chat_updates_config`.)
4. Burst of questions (a For each with 25 sessions waiting): no more than 20 sends per minute to that group, and new questions go before status edits. (Task 2 `twenty_per_minute_then_waits`, `questions_before_edits`.)
5. Free-text reply in a topic that is not a reply to a question (plain message, or reply to another message): ignored, not taken as an answer. (Task 4 `plain_topic_message_is_ignored`.)

---

## File Structure

Create (Rust, `src-tauri/src/telegram/`):
- `mod.rs` - module wiring, `init(app)`, Tauri commands.
- `api.rs` - Bot API client + error classification.
- `queue.rs` - pure send queue (budgets, priorities, coalescing, pauses).
- `config.rs` - `telegram-config.json` model, read/write, topic bindings, connect/link codes.
- `render.rs` - pure message text/keyboard rendering and callback tokens.
- `pending.rs` - pure pending-question registry and update handling (returns actions).
- `poller.rs` - per-token polling threads, sender thread, side effects.

Create (TS, `src/features/telegram/`):
- `model/telegram.ts` - invoke wrappers and shared types.
- `model/telegramForward.ts` - pure eligibility, ticket id, topic title, context, due logic.
- `model/workflowSessionContext.ts` - tiny registry sessionId -> workflow context.
- `hooks/useTelegramQuestions.ts` - forwarding, closure detection, answer routing.
- `ui/TelegramProjectDialog.tsx` - per-project settings dialog.
- `ui/LinkTelegramTopicDialog.tsx` - session topic link dialog.
- Tests next to each file (`*.test.ts`), Rust `#[cfg(test)]` modules.

Modify:
- `src-tauri/src/lib.rs` (mod, setup init, invoke_handler).
- `src/app/App.tsx` (mount hook near `useInputNotifications` ~2523, pass `onQuestionReply`).
- `src/app/hooks/workflowRunner.ts` (record workflow context when a session node starts).
- `src/app/shell/useProjectMenu.tsx` (~174, add "Telegram..." item).
- `src/app/shell/Sidebar.tsx` (`sessionMenuItems` ~1520, `onSessionMenuPick` ~1676, dialog near 3270).
- `docs/telegram.md` (new user doc), `docs/agent-access.md` untouched.

---

### Task 1: Bot API client and error classification

**Files:** Create `src-tauri/src/telegram/api.rs`, `src-tauri/src/telegram/mod.rs` (declare submodules only); Modify `src-tauri/src/lib.rs` (`mod telegram;`); Test: `api.rs` `#[cfg(test)]` with a fake server like `gitlab.rs:1143` (`TcpListener` on `127.0.0.1:0`).

**Interfaces (produces):**

```rust
pub struct BotApi { base: String /* "https://api.telegram.org" or test URL */, token: String }
#[derive(Debug, PartialEq)]
pub enum TgError {
    RetryAfter(u64),          // 429 parameters.retry_after
    Migrated(i64),            // 400 parameters.migrate_to_chat_id
    NotModified,              // 400 "message is not modified"
    ThreadNotFound,           // 400 "message thread not found"
    QueryTooOld,              // 400 "query is too old" / "query ID is invalid"
    Conflict { webhook: bool }, // 409; webhook=true when description mentions "webhook"
    Unauthorized,             // 401
    Permanent(String),        // other 400/403 with description
    Network(String),          // transport error
}
impl BotApi {
    pub fn new(base: &str, token: &str) -> Self;
    pub fn get_me(&self) -> Result<Value, TgError>;
    pub fn get_updates(&self, offset: i64, timeout_s: u64) -> Result<Vec<Value>, TgError>;
    pub fn send_message(&self, chat: i64, thread: Option<i64>, html: &str, keyboard: Option<Value>) -> Result<i64 /* message_id */, TgError>;
    pub fn edit_message(&self, chat: i64, message_id: i64, html: &str, keyboard: Option<Value>) -> Result<(), TgError>;
    pub fn create_forum_topic(&self, chat: i64, name: &str) -> Result<i64 /* message_thread_id */, TgError>;
    pub fn answer_callback_query(&self, id: &str, text: &str) -> Result<(), TgError>;
    pub fn get_chat(&self, chat: i64) -> Result<Value, TgError>;
    pub fn get_chat_member(&self, chat: i64, user: i64) -> Result<Value, TgError>;
    pub fn delete_webhook(&self) -> Result<(), TgError>;
}
pub fn classify(status: u16, body: &str) -> TgError;   // pure
```

All calls POST JSON to `{base}/bot{token}/{method}`, parse `{"ok":true,"result":...}`; `parse_mode: "HTML"`, `disable_web_page_preview: true`. `get_updates` agent read timeout = `timeout_s + 10`. Error strings must never contain the token (strip the URL from ureq transport errors).

- [ ] **Step 1: Tests**: `classifies_retry_after` (429 body with `parameters.retry_after: 7` -> `RetryAfter(7)`), `classifies_migrate_to_chat_id` (400 with `parameters.migrate_to_chat_id: -100123` -> `Migrated(-100123)`), `classifies_not_modified`, `classifies_thread_not_found`, `classifies_query_too_old`, `classifies_conflict_webhook` and `classifies_conflict_poller`, `send_message_posts_thread_and_keyboard` (fake server captures body: has `message_thread_id`, `reply_markup`, `parse_mode:"HTML"`; returns 42), `network_error_hides_token` (unreachable base -> `Network(msg)` where `!msg.contains(token)`).
- [ ] **Step 2:** `cargo test --manifest-path src-tauri/Cargo.toml telegram::api` -> FAIL. **Step 3:** implement. **Step 4:** PASS.
- [ ] **Step 5: Commit** `"Telegram: Bot API client"`.

### Task 2: Send queue (pure)

**Files:** Create `src-tauri/src/telegram/queue.rs`; Test in-module.

**Interfaces (produces):**

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    CreateTopic { project: String, session_id: String, name: String },
    SendQuestion { key: String /* pending key */ },
    EditStatus { chat: i64, message_id: i64, html: String },  // keyboard removed
}
pub struct SendQueue { /* per-chat FIFO by priority + budgets */ }
impl SendQueue {
    pub fn new() -> Self;
    pub fn push(&mut self, chat: i64, job: Job);            // EditStatus with same (chat,message_id) replaces the queued one
    pub fn next_ready(&mut self, now_ms: u64) -> Option<(i64, Job)>; // respects budgets and pauses; CreateTopic/SendQuestion before EditStatus
    pub fn next_wake(&self, now_ms: u64) -> Option<u64>;    // earliest ms a job could become ready
    pub fn pause(&mut self, chat: i64, until_ms: u64);       // 429 / backoff
    pub fn requeue_front(&mut self, chat: i64, job: Job);    // retry same job first
    pub fn rename_chat(&mut self, from: i64, to: i64);       // migrate
}
```

Budgets: per chat at most 20 sends in any rolling 60 000 ms window and >= 1000 ms since the previous send; across all chats at most 30 per rolling 1000 ms.

- [ ] Tests: `twenty_per_minute_then_waits` (21 jobs at t=0 with 1 s steps: 20 released by t=19 000, the 21st not before t=60 000), `one_second_spacing`, `questions_before_edits`, `edits_to_same_message_coalesce`, `pause_blocks_until`, `requeue_front_goes_first`, `rename_chat_moves_jobs`, `global_thirty_per_second`. FAIL -> implement -> PASS. Commit `"Telegram: rate-limited send queue"`.

### Task 3: Config file, codes and topic bindings

**Files:** Create `src-tauri/src/telegram/config.rs`; Test in-module (temp dir).

**Interfaces (produces):**

```rust
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ProjectConfig { pub token: String, pub bot_username: String, pub chat_id: Option<i64>,
    pub chat_title: Option<String>, pub enabled: bool,
    pub topics: BTreeMap<String /* sessionId */, i64 /* message_thread_id */>, pub problem: Option<String> }
#[derive(Serialize, Deserialize, Default)]
pub struct TelegramConfig { pub projects: BTreeMap<String /* projectKey */, ProjectConfig> }
pub fn config_path(app_data: &Path) -> PathBuf;                 // app_data/telegram-config.json
pub fn load(path: &Path) -> TelegramConfig;                     // missing/invalid -> default (invalid also logged without content)
pub fn save(path: &Path, cfg: &TelegramConfig) -> Result<(), String>; // via clickup::write_secret_file (0600)
#[derive(Clone, Debug, PartialEq)]
pub enum CodePurpose { Connect { project: String }, Link { project: String, session_id: String } }
pub struct Codes { /* in memory */ }
impl Codes { pub fn issue(&mut self, purpose: CodePurpose, now_ms: u64) -> String; // 8 chars [A-Z2-9], valid 15 min, single use
             pub fn take(&mut self, code: &str, now_ms: u64) -> Option<CodePurpose>; }
```

- [ ] Tests: `round_trips_and_is_0600` (unix: mode & 0o777 == 0o600), `invalid_file_loads_default`, `code_single_use`, `code_expires_after_15_min`, `codes_are_unambiguous_chars`. FAIL -> implement -> PASS. Commit `"Telegram: config, codes and topic bindings"`.

### Task 4: Rendering and pending registry (pure)

**Files:** Create `src-tauri/src/telegram/render.rs`, `src-tauri/src/telegram/pending.rs`; Tests in-module.

**Interfaces (produces):**

```rust
// Payload sent by the frontend (Task 6/7 mirror this in TS, camelCase via serde rename_all).
#[derive(Deserialize, Clone)] #[serde(rename_all = "camelCase")]
pub struct ForwardPayload { pub project: String, pub session_id: String, pub request_id: u64,
    pub topic_title: String, pub header: String /* project · session · ticket · workflow line */,
    pub context: String /* <= 600 chars */, pub questions: Vec<QuestionIn> }
#[derive(Deserialize, Clone)] #[serde(rename_all = "camelCase")]
pub struct QuestionIn { pub id: String, pub header: Option<String>, pub prompt: String,
    pub multi_select: bool, pub allow_custom: bool, pub options: Vec<OptionIn> }
#[derive(Deserialize, Clone)] pub struct OptionIn { pub id: String, pub label: String, pub description: Option<String> }

// render.rs
pub fn question_html(p: &ForwardPayload, q_index: usize, selected: &BTreeSet<String>) -> String; // HTML-escaped
pub fn keyboard(token: &str, q: &QuestionIn, selected: &BTreeSet<String>) -> Value;
// callback_data formats (all <= 64 bytes): "o:<token>:<optIndex>", "s:<token>" (send, multi), "k:<token>" (skip), "c:<token>" (other)
pub fn status_html(p: &ForwardPayload, q_index: usize, status: &str) -> String;
pub fn new_token() -> String;   // 10 random [a-zA-Z0-9]

// pending.rs
pub struct PendingPrompt { pub owner_window: String, pub payload: ForwardPayload, pub chat: i64,
    pub messages: Vec<Option<i64>> /* per question */, pub tokens: Vec<String>,
    pub selected: Vec<BTreeSet<String>>, pub custom: Vec<Option<String>>, pub done: Vec<bool>, pub awaiting_text: Vec<bool> }
pub enum Effect {
    AnswerCallback { id: String, text: String },
    Edit { chat: i64, message_id: i64, html: String, keyboard: Option<Value> },
    Deliver { window: String, session_id: String, request_id: u64, reply: Value /* UserQuestionReply JSON */, by: String },
    Connected { project: String, chat: i64, title: String },
    Linked { project: String, session_id: String, thread: i64 },
    Reply { chat: i64, thread: Option<i64>, html: String },
}
pub struct Registry { /* key "<sessionId>#<requestId>" -> PendingPrompt; token -> (key, qIndex) */ }
impl Registry {
    pub fn insert(&mut self, prompt: PendingPrompt) -> String /* key */;
    pub fn set_message(&mut self, key: &str, q_index: usize, message_id: i64);
    pub fn close(&mut self, session_id: &str, request_id: u64, status: &str) -> Vec<Effect>; // desktop answered/skipped/cancelled/expired
    pub fn handle_update(&mut self, update: &Value, configured_chats: &BTreeSet<i64>, codes: &mut Codes, now_ms: u64) -> Vec<Effect>;
    pub fn delivered(&mut self, key: &str, ok: bool, by: &str) -> Vec<Effect>; // frontend result
}
```

Rules (spec 5.4-5.6, 13): ignore updates from chats not in `configured_chats` (except `/start@bot <code>` / `/connect@bot <code>` with a valid Connect code, which yields `Connected`); `/link@bot <code>` inside a topic (`is_topic_message` + `message_thread_id`) yields `Linked`, in General yields a `Reply` hint; callback `o:` on single-select records the option and marks the question done; on multi-select toggles and re-renders (Edit); `s:` marks done; `k:` marks skipped; `c:` sets `awaiting_text` and replies with a prompt to reply with text; a text message whose `reply_to_message.message_id` equals a question message with `allow_custom` becomes the custom answer; when every question is done -> `Deliver` with `{"kind":"answered","answers":{qid:[optIds]},"custom":{qid:text}}` (custom uses option id `__custom__`), or `{"kind":"skipped"}` when all skipped; unknown/stale token -> `AnswerCallback{text:"This question has expired"}`; every callback yields exactly one `AnswerCallback` first.

- [ ] **Step 1: Tests**: `renders_escaped_html` (`<b>&` in prompt are escaped), `callback_data_fits_64_bytes` (option index 99), `single_select_delivers_answer`, `multi_select_toggles_then_sends`, `skip_one_of_two_questions_then_answer_other` (Deliver answered subset), `all_skipped_delivers_skipped`, `other_then_text_reply_delivers_custom`, `plain_topic_message_is_ignored` (message in topic whose reply_to_message is the topic creation message), `routes_by_session_and_request` (two prompts same requestId different sessions), `stale_token_reports_expired`, `unconfigured_chat_ignored`, `connect_code_connects`, `link_in_topic_links`, `link_in_general_hints`, `close_edits_every_message_of_prompt`.
- [ ] **Step 2-4:** FAIL, implement, PASS. **Step 5: Commit** `"Telegram: message rendering and pending registry"`.

### Task 5: Poller, sender, commands and wiring

**Files:** Create `src-tauri/src/telegram/poller.rs`; Modify `src-tauri/src/telegram/mod.rs`, `src-tauri/src/lib.rs` (`telegram::init(app.handle())` in `.setup` near `reminders::init`, commands in `invoke_handler`); Test: in-module with fake server (sender behaviour), plus pure helpers.

**Interfaces (produces) - Tauri commands (all take `project: String` = projectKey unless noted):**

```rust
#[tauri::command] fn telegram_status(project) -> TelegramStatus;   // { configured, enabled, botUsername, chatId?, chatTitle?, isForum?, canManageTopics?, problem? } - never the token
#[tauri::command] fn telegram_set_token(project, token: String) -> Result<TelegramStatus, String>; // get_me; stores token; restarts poller for that token
#[tauri::command] fn telegram_connect_link(project) -> Result<{ code, url, fallback }, String>; // url = https://t.me/<bot>?startgroup=<code>&admin=manage_topics ; fallback = "/connect@<bot> <code>"
#[tauri::command] fn telegram_set_enabled(project, enabled: bool) -> Result<TelegramStatus, String>;
#[tauri::command] fn telegram_disconnect(project) -> Result<(), String>;   // removes project config
#[tauri::command] fn telegram_remove_webhook(project) -> Result<(), String>;
#[tauri::command] fn telegram_link_code(project, session_id: String) -> Result<{ code, command }, String>; // command = "/link@<bot> <code>"
#[tauri::command] fn telegram_session_topic(project, session_id: String) -> Option<i64>; // bound thread id only
#[tauri::command] fn telegram_forward_question(window: WebviewWindow, payload: ForwardPayload) -> Result<(), String>; // owner = window.label(); guard is_workspace_window
#[tauri::command] fn telegram_question_closed(session_id: String, request_id: u64, status: String) -> Result<(), String>; // status: "Answered on desktop" | "Skipped" | "Cancelled"
#[tauri::command] fn telegram_answer_result(key: String, ok: bool) -> Result<(), String>;
```

Event emitted with `app.emit_to(owner_window, "monocode-telegram-answer", json)`: `{ key, sessionId, requestId, reply, by }`.

Behaviour:
- One polling thread per distinct token among enabled, configured projects (and projects awaiting Connect). Start offset: on first start call `getUpdates(offset=-1, timeout=0)` and continue from the last `update_id + 1` (skip backlog). Loop `getUpdates(offset, 50)`; feed each update to `Registry::handle_update` with that token's configured chats; perform effects (AnswerCallback immediately, not queued; Edit/Reply/Send via queue; Deliver via `emit_to`; Connected/Linked update config, then `get_chat` + `get_chat_member(bot id)` to record `isForum` / `canManageTopics` and a `problem` string when missing, and confirm in the chat).
- `Conflict{webhook:false}` -> `problem = "Another app is reading this bot"`; `Conflict{webhook:true}` -> `problem = "This bot has a webhook"`; both pause that poller and retry every 60 s. `Unauthorized` -> `problem = "Bot token was rejected"`, stop poller.
- Sender thread: loops on `SendQueue::next_ready`, sleeping until `next_wake`. `CreateTopic`: name trimmed to 128 chars; on success store binding in config `topics[sessionId]`; then the session's queued `SendQuestion`s proceed. If topic creation fails with `Permanent` (no rights / not a forum) send into General (`thread = None`) and set `problem`. `SendQuestion`: renders question messages (one per question), records message ids in Registry. On `ThreadNotFound`: drop binding, enqueue CreateTopic, requeue. `RetryAfter(n)`: `pause(chat, now + n*1000)`, `requeue_front`. `Migrated(new)`: update config `chat_id`, `rename_chat`, requeue. `NotModified`: success. `Network`: backoff 1, 2, 4 ... 60 s via `pause`, requeue. Other `Permanent`: drop and set `problem`.
- `telegram_forward_question`: requires the project enabled with `chat_id`; inserts into Registry; enqueues `CreateTopic` if no binding, then `SendQuestion`.

- [ ] **Step 1: Tests** (fake server): `sender_retries_after_429` (first response 429 retry_after 1, then ok: two requests, second >= 1 s later), `migrated_chat_updates_config` (400 migrate -> config chat id updated and resend to new id), `thread_not_found_recreates_topic`, `topic_name_trimmed_to_128`, `status_never_contains_token`, `connect_link_url_format` (`https://t.me/bot?startgroup=CODE&admin=manage_topics`).
- [ ] **Step 2-4:** FAIL, implement, PASS. **Step 5:** `npm run check > /tmp/tg-check.log 2>&1; echo $?` -> `0`. **Step 6: Commit** `"Telegram: poller, sender and commands"`.

### Task 6: Frontend model (pure)

**Files:** Create `src/features/telegram/model/telegram.ts`, `src/features/telegram/model/telegramForward.ts`, `src/features/telegram/model/workflowSessionContext.ts`; Modify `src/app/hooks/workflowRunner.ts` (call `setWorkflowSessionContext` when a session node's session id is known, `clearWorkflowSessionContext` when the run ends); Tests `telegramForward.test.ts`, `workflowSessionContext.test.ts`.

**Interfaces (produces):**

```ts
// telegram.ts - thin invoke wrappers with the Task 5 command names and camelCase payloads
export type TelegramStatus = { configured: boolean; enabled: boolean; botUsername?: string; chatId?: number; chatTitle?: string; isForum?: boolean; canManageTopics?: boolean; problem?: string };
export type ForwardPayload = { project: string; sessionId: string; requestId: number; topicTitle: string; header: string; context: string; questions: { id: string; header?: string; prompt: string; multiSelect: boolean; allowCustom: boolean; options: { id: string; label: string; description?: string }[] }[] };
export type TelegramAnswerEvent = { key: string; sessionId: string; requestId: number; reply: UserQuestionReply; by: string };
export function telegramStatus(project: string): Promise<TelegramStatus>;   // + one wrapper per command

// workflowSessionContext.ts
export type WorkflowSessionContext = { workflowName: string; nodeTitle: string; itemLabel?: string; itemId?: string; round?: number };
export function setWorkflowSessionContext(sessionId: string, ctx: WorkflowSessionContext): void;
export function workflowSessionContext(sessionId: string): WorkflowSessionContext | undefined;
export function clearWorkflowSessionContext(sessionId: string): void;

// telegramForward.ts
export const FORWARD_DELAY_MS = 5 * 60_000;
export function isForwardable(session: Session): boolean;
export function sessionTicketId(session: Session): string | undefined;
export function topicTitle(session: Session): string;            // "[<ticket>] <title>" | "<title>", max 128 chars
export function questionContext(session: Session): string;       // last assistant non-tool, non-internal text, tail 600 chars
export function forwardPayload(session: Session, project: string): ForwardPayload | undefined; // undefined when no pendingQuestion
export function dueForwards(sessions: Session[], firstSeen: Map<string, number>, sent: Set<string>, now: number): Session[]; // key `${id}#${requestId}`
```

Rules: `isForwardable` false for `orchestrationLeadId`, `inboxAsk`, `ephemeral`, `sidebarHidden`, `isTuiSession`, `isMonoSession(id)`, remote projects (`isRemoteProjectPath(cwd)`), Codex non-blocking questions (`pendingQuestion.autoResolveAt` set). Ticket id order: `inboxCard?.identifier`, workflow context `itemId`, `linkedWorkItem` -> `#<number>`, else undefined. Title via `sessionDisplayTitle`. Header: `"<project folder name> · <title>"` plus `" · <ticket>"` and a second line `"Workflow <name> · <node> · <item> · round <n>"` when known.

- [ ] Tests: `excludes orchestration workers, inbox ask, ephemeral, hidden, tui, mono chats, remote, codex auto-resolving`, `ticket from inbox card identifier`, `ticket from workflow item`, `ticket from linked GitHub issue is #12`, `title without ticket is plain`, `title trimmed to 128`, `context takes last assistant text tail 600`, `due only after 5 minutes and only once`, `workflow context set and cleared`. FAIL -> implement -> PASS. Commit `"Telegram: forwarding model"`.

### Task 7: Hook and App wiring

**Files:** Create `src/features/telegram/hooks/useTelegramQuestions.ts`; Modify `src/app/App.tsx` (mount near `useInputNotifications` ~2523 with `sessionsRef`, `onQuestionReply`); Test `useTelegramQuestions.test.ts` (happy-dom, mocked `invoke` and `listen`).

**Interfaces:** `export function useTelegramQuestions(input: { sessions: Session[]; onQuestionReply: (sessionId: string, requestId: number, reply: UserQuestionReply) => void; now?: () => number }): void`.

Behaviour:
- Track `firstSeen` per `${sessionId}#${requestId}` when a forwardable session shows a pending question. Every 15 s (and on sessions change) compute `dueForwards`; for each, check `telegramStatus(projectKey(session.cwd))` is enabled with a chat (cache statuses for 60 s), then `telegram_forward_question(forwardPayload(...))` and add to `sent`. Questions first seen while the project was disabled are never sent later (record the disabled state at first sight).
- Closure: when a `sent` key's session no longer has that pending request id, call `telegram_question_closed(sessionId, requestId, status)` with `"Answered on desktop"` if the question was answered (the session's last question block shows answered), `"Skipped"` if skipped, else `"Cancelled"`, unless the closure came from our own Telegram delivery (then skip).
- Answer event `monocode-telegram-answer`: if the session still has `pendingQuestion.requestId === event.requestId`, call `onQuestionReply(sessionId, requestId, reply)`, mark the key as delivered-by-Telegram, and `telegram_answer_result(key, true)`; otherwise `telegram_answer_result(key, false)`.
- No transcript note in v1 (spec 12 leaves the form to the builder): who answered is shown on the Telegram message only.

- [ ] Tests: `forwards after 5 minutes when enabled`, `does not forward when answered within 5 minutes`, `never forwards a question first seen while disabled`, `routes answer to onQuestionReply`, `ignores answer for a different requestId` (reports stale), `reports stale when no longer pending`, `reports desktop answer as closed`, `does not report closure for its own delivery`. FAIL -> implement -> PASS. **Step:** full check -> `0`. Commit `"Telegram: forward questions and route answers"`.

### Task 8: Project settings dialog and docs

**Files:** Create `src/features/telegram/ui/TelegramProjectDialog.tsx`, `docs/telegram.md`; Modify `src/app/shell/useProjectMenu.tsx` (~174: item `"telegram-settings"`, label `"Telegram..."`), the component that renders project dialogs (follow how `ProjectBackgroundDialog` is opened); Test `TelegramProjectDialog.test.ts`.

UI (match existing settings/dialog style; use `monoPanelParts`-like rows, existing buttons and inputs):
- Not configured: token field (password input) + "Save" -> `telegram_set_token`; shows bot `@username` after success; errors shown inline.
- Configured, no group: "Connect a group" shows the deep link button (opens URL in browser) and the fallback command with a copy button; poll `telegram_status` every 3 s while open to show "Connected to <chat title>".
- Connected: chat title, Enabled switch, status lines: Topics on/off, bot can manage topics yes/no, with the note "Make the bot an admin with Manage topics so it can create a topic per session." when missing; `problem` line, with a "Remove webhook" button when the problem is the webhook one; "Disconnect".
- Copy: "Questions that wait more than 5 minutes are sent to this group. Any member can answer."
- `docs/telegram.md`: setup steps (BotFather token, Topics, admin right, Connect, link a topic), behaviour, limits, the two-apps-one-bot warning.

- [ ] Tests: `saves token and shows bot username`, `shows connect link and fallback command`, `shows admin note when bot cannot manage topics`, `shows remove webhook action for webhook problem`, `token is never rendered after save`. FAIL -> implement -> PASS. Commit `"Telegram: project settings"`.

### Task 9: Link a session to an existing topic

**Files:** Create `src/features/telegram/ui/LinkTelegramTopicDialog.tsx`; Modify `src/app/shell/Sidebar.tsx` (`sessionMenuItems` ~1520: item `"link-telegram-topic"`, label `"Link Telegram topic..."`, shown only when the session's project has Telegram configured; `onSessionMenuPick` ~1676; render near ~3270); Test `LinkTelegramTopicDialog.test.ts`.

Dialog: calls `telegram_link_code(project, sessionId)`, shows "Send this in the topic you want to use:" + the command with a copy button, polls `telegram_status` / a per-session binding check every 3 s and shows "Linked" when `telegram_session_topic(project, sessionId)` (Task 5) returns a thread id different from the one at open.

- [ ] Tests: `shows link command`, `shows linked when binding appears`. FAIL -> implement -> PASS.
- [ ] **Final verification:** `npm run check > /tmp/tg-check.log 2>&1; echo $?` -> `0`; `./scripts/check-fork-branding.sh; echo $?` -> `0`. Manual (needs a real bot; if not possible, say so): `npm run tauri:dev`, configure a project, connect a group, start a Claude session that asks an `AskUserQuestion`, wait 5 minutes, answer in Telegram, confirm the session continues and the message shows "Answered by".
- [ ] Commit `"Telegram: link a session to a topic"`.
