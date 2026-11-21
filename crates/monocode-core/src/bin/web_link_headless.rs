//! Headless desktop binary for the web link (the desktop side of the E2E harness).
//!
//! Commands (parsed by hand, clap is not a dependency):
//!
//! ```text
//! web_link_headless capabilities
//! web_link_headless check-vectors <path-to-vectors.json>
//! web_link_headless pair --server <http(s) url> --data-dir <dir> [--machine-name <name>] [--timeout-secs <n>]
//! web_link_headless serve --data-dir <dir> [--confirm-pending <read-only|control>]
//! ```
//!
//! Exit codes: 0 ok, 1 check-vectors found a failure, 2 usage or IO error (also `serve` without a pairing),
//! 4 pairing was denied, expired or timed out, 5 the server could not be reached.
//!
//! `pair` plays the desktop side of the device authorization of architecture.md 6.4 through the public host
//! API (`MobileHost::web_pair_start` and `web_pair_poll`): the identity is sealed in `<data-dir>/store.db`,
//! the same store the app uses. stdout carries JSON lines, never a token, a device code or a verifier:
//!
//! ```text
//! {"event":"pairing","userCode","confirmationCode","fingerprint","link","expiresIn"}
//! {"event":"paired","desktopId","account","fingerprint"}
//! ```
//!
//! `serve` opens the host in `<data-dir>` (after `pair`), publishes the E2E catalog (project "E2E Project",
//! session "E2E session", ids `e2e-project` and `e2e-session`; the values of `web/e2e/lib/catalog.ts`), turns the
//! web link on and answers the app requests with a deterministic fake until it is killed, told to `quit`, or
//! unpaired:
//!
//! - `conversation.read` gives the turns of `e2e-session` (a seeded user and assistant turn, then the sent ones).
//! - `conversation.send` appends the user text and answers `{sessionId}`; 1500 ms later an assistant turn
//!   `Echo: <text>` is pushed over every watch. A text containing `[hold]` gets no reply until `session.cancel`.
//! - `session.cancel` (the stop method of `web/protocol/methods.json`) ends the streaming: busy turns false.
//! - `conversation.watch` opens a watch: the current turns at once, then one frame per change.
//! - `chat.mentionedFile` runs the real core implementation (`monocode_core::mobile::mentioned_file::handle`,
//!   the one the app uses) against a seeded folder, when the environment seeds one (see below); otherwise it
//!   answers `unsupported`.
//! - `file.prepare` runs the real core implementation (`monocode_core::mobile::mentioned_file::transfer::handle`)
//!   over the same seeded folder as the one project of the device: it hashes, seals and uploads the file to
//!   the web server's R2 bucket with this desktop's token, and answers the object and the key. A Control device
//!   may open any readable file, a read-only one only the seeded folder. Hide the capability `fileTransfer` to
//!   behave like a desktop that predates it.
//! - any other app method answers the error code `unsupported`.
//!
//! Seeding `chat.mentionedFile` and `file.prepare` (environment of the `serve` process):
//!
//! ```text
//! WEB_LINK_HEADLESS_MENTION_DIR=<folder>   required to enable it; the session folder and the root of project `e2e-project`
//! WEB_LINK_HEADLESS_MENTION_TEXT=<text>    optional: the one assistant message that mentions files; `{dir}` is replaced by
//!                                          the canonical folder. Default: a message that mentions, as an absolute path in
//!                                          backticks, every top-level regular file of the folder whose name does not start
//!                                          with `unmentioned`
//! WEB_LINK_HEADLESS_MENTION_TOOL_TEXT=<text>  optional: the files the agent ran a tool on, written like a message: every
//!                                          file it mentions becomes one COMPLETED `Read` tool call (`{file_path}`), the
//!                                          evidence a file OUTSIDE the device's projects needs besides the assistant
//!                                          message (plain text is no evidence); `{dir}` is replaced as above. Default: the
//!                                          same text as the assistant message, as if the agent read every file it names.
//!                                          Set it to the empty string for a conversation without any tool row (a file
//!                                          outside the projects is then `forbidden`)
//! WEB_LINK_HEADLESS_MENTION_ROOT=<folder>  optional: a local project folder of a Mono (a root for relative mentions,
//!                                          never a project of the device, so a file in it is outside the projects)
//! WEB_LINK_HEADLESS_MENTION_BASH_TEXT=<text>  optional: one COMPLETED `Bash` call whose command line is this text (`{dir}`
//!                                          is replaced as above): every path it names is evidence, like a report a
//!                                          script wrote
//! WEB_LINK_HEADLESS_HIDE_CAPABILITIES=<a,b>  optional: capabilities the desktop does not advertise (for example
//!                                          `mentionedFiles2`, as in the 0.54.1 release)
//! WEB_LINK_HEADLESS_MENTION_OLD_RULES=1    optional: the 0.54.1 rules, a shell command does not count as evidence
//! WEB_LINK_HEADLESS_MENTION_LEGACY_FLAG=<file>  optional: while this file exists the desktop answers `unsupported`, as
//!                                          a desktop older than `chat.mentionedFile` does
//! ```
//!
//! Seeding a Mono (environment of the `serve` process):
//!
//! ```text
//! WEB_LINK_HEADLESS_MONO=1   publish one Mono `e2e-mono` ("E2E Mono") in the catalog; its chat, the conversation
//!                            `mono:e2e-mono`, is the same fake conversation as `e2e-session` (shared state)
//! ```
//!
//! The conversation id is `e2e-session`. The seeded assistant message is the only one that authorizes paths (the
//! fake `Echo:` turns do not). The folder is also the project `e2e-project` (selected when the device selection is
//! all or lists it), so replies carry `project`. Files and subfolders are read at request time: a spec may create
//! files after the start. The deny-list, budgets and paging are the real ones; no audit file is written.
//!
//! `--confirm-pending` is for the E2E harness only: it confirms every pending device with that permission on
//! its own. It is never a default; without it a device stays pending until an `approve` command.
//!
//! stdout events of `serve`, one JSON object per line (none carries a token):
//!
//! ```text
//! {"event":"online","desktopId","fingerprint"}              the relay link is connected
//! {"event":"offline"}                                       the link dropped (it retries by itself)
//! {"event":"confirmed","deviceId","permission"}             a device was confirmed ("read" or "control")
//! {"event":"permission","deviceId","permission"}            a `permission` command changed a device
//! {"event":"request","method"}                              the app received a request
//! {"event":"watch","action":"start"|"stop"}
//! {"event":"revoked","scope":"device","deviceId"}           a paired device is gone (revoked on either side)
//! {"event":"revoked","scope":"desktop"}                     the relay refused this desktop for good
//! {"event":"unpaired"}                                      after the `unpair` command
//! {"event":"status","state":"off"|"connecting"|"connected"|"offline"|"revoked"}   the answer to `status`
//! {"event":"error","message"}
//! ```
//!
//! `deviceId` is the relay's device id (what `POST /api/devices` returned). Commands are JSON lines on stdin;
//! the end of stdin does not stop `serve`:
//!
//! ```text
//! {"cmd":"approve","deviceId":ID,"permission":"read-only"|"control"}    confirm a pending device
//! {"cmd":"deny","deviceId":ID}                                          refuse a pending device
//! {"cmd":"permission","deviceId":ID,"permission":"read-only"|"control"} change a paired device
//! {"cmd":"revoke","deviceId":ID}                                        desktop-side revoke of a paired device
//! {"cmd":"status"}                                                      print one `status` event
//! {"cmd":"unpair"}                                                      unpair (revokes the token), then exit 0
//! {"cmd":"quit"}                                                        exit 0
//! ```
use monocode_core::mobile::host::{Event, MobileHost, Push, Request};
use monocode_core::mobile::web_link::frame::{
    split_message, Frame, FrameType, Plaintext, MAX_REQUEST_BYTES,
};
use monocode_core::mobile::web_link::{self, PairPoll};
use monocode_core::mobile::{DeviceKind, Permission, WebState};
use monocode_core::store::Store;
use monocode_mobile_link::Failure;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const USAGE: &str = "usage:
  web_link_headless capabilities
  web_link_headless check-vectors <path-to-vectors.json>
  web_link_headless pair --server <http(s) url> --data-dir <dir> [--machine-name <name>] [--timeout-secs <n>]
  web_link_headless serve --data-dir <dir> [--confirm-pending <read-only|control>]
  web_link_headless --help

  --confirm-pending is for the E2E harness only: it confirms every pending device on its own. It is never a default.
  serve reads JSON-line commands on stdin (approve, deny, permission, revoke, unpair, quit); see the module doc.";

const CAPABILITIES_LINE: &str =
    r#"{"version":1,"serve":true,"pair":true,"reason":null,"checkVectors":true}"#;
const DEFAULT_MACHINE_NAME: &str = "e2e-headless";
const DEFAULT_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Capabilities,
    CheckVectors {
        path: PathBuf,
    },
    Pair {
        server: String,
        data_dir: PathBuf,
        machine_name: String,
        timeout: Duration,
    },
    Serve {
        data_dir: PathBuf,
        /// E2E only: the permission a pending grant is confirmed with, without asking.
        confirm_pending: Option<ConfirmPermission>,
    },
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmPermission {
    ReadOnly,
    Control,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let Some(command) = args.first() else {
        return Err("missing command".into());
    };
    let rest = &args[1..];
    match command.as_str() {
        "--help" | "-h" | "help" => Ok(Command::Help),
        "capabilities" => {
            if !rest.is_empty() {
                return Err("capabilities takes no arguments".into());
            }
            Ok(Command::Capabilities)
        }
        "check-vectors" => match rest {
            [path] => Ok(Command::CheckVectors {
                path: PathBuf::from(path),
            }),
            _ => Err("check-vectors takes exactly one path".into()),
        },
        "pair" => parse_pair(rest),
        "serve" => parse_serve(rest),
        other => Err(format!("unknown command: {other}")),
    }
}

/// Reads `--flag value` pairs; every flag in `known` may appear once.
fn flag_values<'a>(
    rest: &'a [String],
    command: &str,
    known: &[&'a str],
) -> Result<Vec<Option<&'a String>>, String> {
    let mut found: Vec<Option<&String>> = vec![None; known.len()];
    let mut iter = rest.iter();
    while let Some(flag) = iter.next() {
        let Some(index) = known.iter().position(|name| *name == flag.as_str()) else {
            return Err(format!("unknown option for {command}: {flag}"));
        };
        let value = iter.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if found[index].replace(value).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    Ok(found)
}

fn parse_pair(rest: &[String]) -> Result<Command, String> {
    let found = flag_values(
        rest,
        "pair",
        &["--server", "--data-dir", "--machine-name", "--timeout-secs"],
    )?;
    let server = found[0].ok_or("pair needs --server")?;
    let data_dir = found[1].ok_or("pair needs --data-dir")?;
    let parsed =
        url::Url::parse(server).map_err(|e| format!("--server is not a valid URL: {e}"))?;
    let host = parsed.host_str().unwrap_or_default();
    match parsed.scheme() {
        "https" if !host.is_empty() => {}
        // Dev builds accept plain http only for localhost (R-05); so does this tool.
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]") => {}
        "http" => return Err("--server may use http:// only for localhost".into()),
        _ => return Err("--server must be an http:// or https:// URL with a host".into()),
    }
    if data_dir.is_empty() {
        return Err("--data-dir must not be empty".into());
    }
    let machine_name = found[2]
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|| DEFAULT_MACHINE_NAME.into());
    if machine_name.is_empty() || machine_name.chars().count() > 64 {
        return Err("--machine-name must have 1 to 64 characters".into());
    }
    let timeout_secs = match found[3] {
        Some(text) => text
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("--timeout-secs must be a positive number")?,
        None => DEFAULT_TIMEOUT_SECS,
    };
    Ok(Command::Pair {
        server: server.trim_end_matches('/').to_string(),
        data_dir: PathBuf::from(data_dir),
        machine_name,
        timeout: Duration::from_secs(timeout_secs),
    })
}

fn parse_serve(rest: &[String]) -> Result<Command, String> {
    let found = flag_values(rest, "serve", &["--data-dir", "--confirm-pending"])?;
    let data_dir = found[0].ok_or("serve needs --data-dir")?;
    if data_dir.is_empty() {
        return Err("--data-dir must not be empty".into());
    }
    let confirm_pending = match found[1].map(String::as_str) {
        None => None,
        Some("read-only") => Some(ConfirmPermission::ReadOnly),
        Some("control") => Some(ConfirmPermission::Control),
        Some(_) => return Err("--confirm-pending must be read-only or control".into()),
    };
    Ok(Command::Serve {
        data_dir: PathBuf::from(data_dir),
        confirm_pending,
    })
}

// ---------------------------------------------------------------- host

const STORE_FILE: &str = "store.db";
const MOBILE_FILE: &str = "mobile-access.json";
/// The one conversation the fake app knows, and the catalog it publishes. These are the values of
/// `web/e2e/lib/catalog.ts`: change both together.
const PROJECT_ID: &str = "e2e-project";
const PROJECT_NAME: &str = "E2E Project";
const SESSION_ID: &str = "e2e-session";
const SESSION_TITLE: &str = "E2E session";
/// The Mono published when `WEB_LINK_HEADLESS_MONO` is set, and its conversation id (`mono:<id>`).
const MONO_ID: &str = "e2e-mono";
const MONO_CONVERSATION: &str = "mono:e2e-mono";
/// The look of the published Mono (web/e2e/lib/catalog.ts): one of the ten project mascots, in a Mono swatch.
const MONO_MASCOT: &str = "frog";
const MONO_COLOR: &str = "#39c363";

/// Whether the environment asks for the Mono (see the module docs).
fn mono_enabled() -> bool {
    std::env::var("WEB_LINK_HEADLESS_MONO").is_ok_and(|value| !value.is_empty())
}

/// The conversations the fake app knows: the session, and the Mono's chat when it is published.
fn is_known_conversation(id: Option<&str>) -> bool {
    id == Some(SESSION_ID) || (mono_enabled() && id == Some(MONO_CONVERSATION))
}
/// How long the fake assistant "thinks" before its reply; a `conversation.send` whose text holds
/// [`HOLD_MARKER`] never gets one until `session.cancel` ends it. It is well above the watch pacer interval
/// (500 ms, `web_link/pacer.rs`), so the busy frame and the reply frame can never be merged into one.
const REPLY_DELAY: Duration = Duration::from_millis(1500);
const HOLD_MARKER: &str = "[hold]";

/// Opens the desktop state in `data_dir`: the native store (it creates the tables the sealed web identity
/// lives in and registers the database, `credentials.key` sits next to it) and the mobile host.
fn open_host(data_dir: &Path) -> Result<(MobileHost, async_channel::Receiver<Event>), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let dir = std::fs::canonicalize(data_dir)
        .map_err(|e| format!("cannot resolve {}: {e}", data_dir.display()))?;
    Store::open(&dir.join(STORE_FILE))?;
    MobileHost::open(dir.join(MOBILE_FILE))
}

fn emit(out: &mut dyn Write, line: Value) {
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------- pair

/// The desktop half of the device authorization, through the public host API (`web_pair_start` and
/// `web_pair_poll`): the identity is sealed in the store of `data_dir`, exactly as the app does it. The
/// device code, the verifier and the token never leave the host and are never printed.
/// `poll_every` overrides the server's interval (tests only).
fn run_pair(
    server: &str,
    data_dir: &Path,
    machine_name: &str,
    timeout: Duration,
    poll_every: Option<Duration>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let (host, _events) = match open_host(data_dir) {
        Ok(opened) => opened,
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            return 2;
        }
    };
    let started = Instant::now();
    let ticket = match host.web_pair_start(server, machine_name) {
        Ok(ticket) => ticket,
        Err(e) if e.contains("Cannot reach") => {
            let _ = writeln!(err, "error: cannot reach the server: {e}");
            return 5;
        }
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            return 2;
        }
    };
    emit(
        out,
        json!({
            "event": "pairing",
            "userCode": ticket.user_code,
            "confirmationCode": ticket.confirmation_code,
            "fingerprint": ticket.fingerprint,
            "link": ticket.link,
            "expiresIn": ticket.expires_at.saturating_sub(unix_now()),
        }),
    );
    let interval = poll_every.unwrap_or_else(|| Duration::from_secs(ticket.interval.max(1)));
    loop {
        if started.elapsed() >= timeout {
            host.web_pair_cancel();
            let _ = writeln!(err, "error: pairing timed out");
            return 4;
        }
        match host.web_pair_poll() {
            Ok(PairPoll::Pending) => {}
            Ok(PairPoll::Approved { account }) => {
                emit(
                    out,
                    json!({
                        "event": "paired",
                        "desktopId": host.web_status().desktop_id,
                        "account": account,
                        "fingerprint": ticket.fingerprint,
                    }),
                );
                return 0;
            }
            Ok(PairPoll::Denied) => {
                let _ = writeln!(err, "error: pairing was denied");
                return 4;
            }
            Ok(PairPoll::Expired) => {
                let _ = writeln!(err, "error: pairing was expired");
                return 4;
            }
            Err(e) => {
                let _ = writeln!(err, "error: {e}");
                return 2;
            }
        }
        std::thread::sleep(interval);
    }
}

// ---------------------------------------------------------------- serve

/// What the fake app says about the one conversation: its turns and whether the assistant is busy.
struct Conversation {
    turns: Vec<Value>,
    busy: bool,
    next_turn: u64,
    /// Replies not yet sent: when they are due and their text. Cleared by `session.cancel`.
    replies: Vec<(Instant, String)>,
}

impl Conversation {
    fn new() -> Self {
        let mut conversation = Self {
            turns: Vec::new(),
            busy: false,
            next_turn: 1,
            replies: Vec::new(),
        };
        conversation.add_user("Please look at the E2E task.");
        conversation.add_assistant("Canned E2E answer.");
        conversation
    }

    fn turn_id(&mut self) -> String {
        let id = format!("t{}", self.next_turn);
        self.next_turn += 1;
        id
    }

    fn add_user(&mut self, text: &str) {
        let id = self.turn_id();
        self.turns.push(json!({
            "id": id,
            "stampAt": 1_700_000_000_000u64 + self.next_turn,
            "items": [{"type": "user", "id": format!("{id}-u"), "text": text}],
        }));
    }

    fn add_assistant(&mut self, text: &str) {
        let id = self.turn_id();
        self.turns.push(json!({
            "id": id,
            "stampAt": 1_700_000_000_000u64 + self.next_turn,
            "completedAt": 1_700_000_000_000u64 + self.next_turn,
            "durationMs": 1,
            "items": [{"type": "text", "id": format!("{id}-a"), "text": text}],
        }));
    }

    /// `conversation.read` of the session.
    #[cfg(test)]
    fn read(&self) -> Value {
        self.read_as(SESSION_ID)
    }

    /// `conversation.read` of the conversation `id`.
    fn read_as(&self, id: &str) -> Value {
        json!({
            "id": id,
            "turns": self.turns,
            "busy": self.busy,
            "usageLimit": null,
            "older": false,
        })
    }

    /// A watch frame: every turn, in order (a frame that carries all of them is valid).
    fn frame(&self) -> Value {
        let order: Vec<&Value> = self.turns.iter().map(|turn| &turn["id"]).collect();
        json!({
            "usageLimit": null,
            "busy": self.busy,
            "older": false,
            "order": order,
            "turns": self.turns,
            "question": null,
        })
    }

    /// `conversation.send`: the user turn is appended at once, the reply comes later over the watch.
    fn send(&mut self, text: &str, now: Instant) {
        self.add_user(text);
        self.busy = true;
        if !text.contains(HOLD_MARKER) {
            self.replies
                .push((now + REPLY_DELAY, format!("Echo: {text}")));
        }
    }

    /// `session.cancel`: no reply is coming any more.
    fn cancel(&mut self) {
        self.replies.clear();
        self.busy = false;
    }

    /// Sends the replies that are due; returns whether anything changed.
    fn tick(&mut self, now: Instant) -> bool {
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.replies)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        self.replies = later;
        for (_, text) in &due {
            self.add_assistant(text);
        }
        if !due.is_empty() && self.replies.is_empty() {
            self.busy = false;
        }
        !due.is_empty()
    }
}

/// The catalog the fake app publishes (the values of `web/e2e/lib/catalog.ts`).
fn e2e_catalog() -> Value {
    catalog_with_mono(mono_enabled())
}

fn catalog_with_mono(with_mono: bool) -> Value {
    let monos = if with_mono {
        json!([{
            "id": MONO_ID,
            "name": "E2E Mono",
            // What the app sends for every Mono: a pixel mascot name and a `#rrggbb` color (`mobile_host.rs`).
            "mascot": MONO_MASCOT,
            "color": MONO_COLOR,
            "projectIds": [PROJECT_ID],
            "unread": 0,
            "status": "idle",
        }])
    } else {
        json!([])
    };
    json!({
        "projects": [{
            "id": PROJECT_ID,
            "name": PROJECT_NAME,
            "path": "/e2e/project",
            "branch": "main",
        }],
        "monos": monos,
        "sessions": [{
            "id": SESSION_ID,
            "projectId": PROJECT_ID,
            "title": SESSION_TITLE,
            "harness": "claude",
            "model": "e2e-model",
            "branch": "main",
            "updatedAt": 1_700_000_000u64,
        }],
    })
}

/// One line the serve loop prints, or the end of the run.
enum Output {
    Line(Value),
    Exit(i32),
}

type Lines = std::sync::mpsc::Sender<Output>;

fn say(lines: &Lines, line: Value) {
    let _ = lines.send(Output::Line(line));
}

fn permission_name(permission: Permission) -> &'static str {
    match permission {
        Permission::Read => "read",
        Permission::Control => "control",
    }
}

impl ConfirmPermission {
    fn permission(self) -> Permission {
        match self {
            ConfirmPermission::ReadOnly => Permission::Read,
            ConfirmPermission::Control => Permission::Control,
        }
    }
}

/// The fake app: answers the events of the host from a deterministic conversation, on its own thread.
fn app_loop(
    host: MobileHost,
    events: async_channel::Receiver<Event>,
    lines: Lines,
    stop: Arc<AtomicBool>,
    data_dir: PathBuf,
) {
    let mut conversation = Conversation::new();
    // Watch ids and whether the watch still has to be sent the current state.
    let mut watches: Vec<(String, bool)> = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        let mut idle = true;
        while let Ok(event) = events.try_recv() {
            idle = false;
            match event {
                Event::Request(request) => {
                    say(
                        &lines,
                        json!({"event": "request", "method": request.method}),
                    );
                    let result = if request.method == "file.prepare" {
                        answer_prepare(&host, &data_dir, &request)
                    } else {
                        answer(&mut conversation, &request)
                    };
                    if request.method == "conversation.send" || request.method == "session.cancel" {
                        for watch in &mut watches {
                            watch.1 = true;
                        }
                    }
                    request.reply(result);
                }
                Event::Watch(start) => {
                    if is_known_conversation(Some(start.conversation_id.as_str())) {
                        say(
                            &lines,
                            json!({"event": "watch", "action": "start", "conversationId": start.conversation_id}),
                        );
                        watches.push((start.id.clone(), true));
                        start.accept(Ok(()));
                    } else {
                        start.accept(Err(Failure::from("Unknown conversation")));
                    }
                }
                Event::WatchStop(id) => {
                    say(&lines, json!({"event": "watch", "action": "stop"}));
                    watches.retain(|(watch, _)| *watch != id);
                }
            }
        }
        if conversation.tick(Instant::now()) {
            idle = false;
            for watch in &mut watches {
                watch.1 = true;
            }
        }
        // Send the state to watches that are behind; a busy watch keeps its flag and is retried.
        watches.retain_mut(|(id, dirty)| {
            if !*dirty {
                return true;
            }
            match host.push(id, conversation.frame()) {
                Ok(()) => {
                    *dirty = false;
                    true
                }
                Err(Push::Busy) => true,
                Err(Push::Gone) => false,
            }
        });
        if idle {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The seeded folder and message of `chat.mentionedFile`, read from the environment once.
struct MentionSeed {
    dir: std::path::PathBuf,
    text: Option<String>,
    /// `Some("")` is no tool row at all; `None` is the default (the assistant's text). Every file
    /// it mentions becomes a completed `Read` call.
    tool_text: Option<String>,
    /// A Mono project folder: a root for relative mentions, not a project of the device.
    root: Option<String>,
    /// The command line of one completed `Bash` call.
    bash_text: Option<String>,
    /// The 0.54.1 rules: only structured file-tool evidence counts outside the projects.
    old_rules: bool,
    /// While this file exists the desktop answers as one that lacks the method.
    legacy_flag: Option<std::path::PathBuf>,
}

fn mention_seed() -> Option<&'static MentionSeed> {
    static SEED: std::sync::OnceLock<Option<MentionSeed>> = std::sync::OnceLock::new();
    SEED.get_or_init(|| {
        let dir = std::env::var("WEB_LINK_HEADLESS_MENTION_DIR").ok()?;
        let dir = std::fs::canonicalize(dir).ok()?;
        let text = std::env::var("WEB_LINK_HEADLESS_MENTION_TEXT")
            .ok()
            .map(|text| text.replace("{dir}", &dir.to_string_lossy()));
        let tool_text = std::env::var("WEB_LINK_HEADLESS_MENTION_TOOL_TEXT")
            .ok()
            .map(|text| text.replace("{dir}", &dir.to_string_lossy()));
        let replaced = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|text| text.replace("{dir}", &dir.to_string_lossy()))
        };
        let root = std::env::var("WEB_LINK_HEADLESS_MENTION_ROOT")
            .ok()
            .and_then(|root| std::fs::canonicalize(root).ok())
            .map(|root| root.to_string_lossy().into_owned());
        let bash_text = replaced("WEB_LINK_HEADLESS_MENTION_BASH_TEXT");
        let legacy_flag = std::env::var("WEB_LINK_HEADLESS_MENTION_LEGACY_FLAG")
            .ok()
            .map(std::path::PathBuf::from);
        let old_rules =
            std::env::var("WEB_LINK_HEADLESS_MENTION_OLD_RULES").is_ok_and(|v| v == "1");
        Some(MentionSeed {
            old_rules,
            dir,
            text,
            tool_text,
            root,
            bash_text,
            legacy_flag,
        })
    })
    .as_ref()
}

/// The seeded assistant message: the given text, or one that mentions every top-level file.
fn mention_text(seed: &MentionSeed) -> String {
    if let Some(text) = &seed.text {
        return text.clone();
    }
    let mut names: Vec<String> = std::fs::read_dir(&seed.dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("unmentioned"))
        .collect();
    names.sort();
    let mentions: Vec<String> = names
        .iter()
        .map(|name| format!("`{}`", seed.dir.join(name).display()))
        .collect();
    format!("Files: {}", mentions.join(" "))
}

fn answer_mentioned(request: &Request) -> Result<Value, Failure> {
    use monocode_core::mobile::mentioned_file as mention;
    let Some(seed) = mention_seed() else {
        return Err(Failure::coded(
            "unsupported",
            "The E2E desktop does not serve this method",
        ));
    };
    if seed.legacy_flag.as_ref().is_some_and(|flag| flag.exists()) {
        return Err(Failure::coded(
            "unsupported",
            "The E2E desktop does not serve this method",
        ));
    }
    if request.params["conversationId"].as_str() != Some(SESSION_ID) {
        return Err(Failure::from("Unknown conversation"));
    }
    let assistant = mention_text(seed);
    let tool = seed.tool_text.clone().unwrap_or_else(|| assistant.clone());
    let texts = vec![assistant];
    // Structured evidence: one completed `Read` call per file the tool text mentions.
    let mut tool_texts: Vec<String> = mention::mentions::extract_mentions(&tool)
        .into_iter()
        .map(|path| {
            let mut call = monocode_core::tool::ToolCall::new("e2e-read", "Read");
            call.state = monocode_core::tool::ToolState::Completed;
            call.input = json!({ "file_path": path }).to_string();
            serde_json::to_string(&call).unwrap_or_default()
        })
        .collect();
    // Loose evidence: a completed shell command that names files (a script that wrote a report).
    if let Some(command) = seed.bash_text.as_ref().filter(|_| !seed.old_rules) {
        let mut call = monocode_core::tool::ToolCall::new("e2e-bash", "Bash");
        call.state = monocode_core::tool::ToolState::Completed;
        call.input = json!({ "command": command }).to_string();
        tool_texts.push(serde_json::to_string(&call).unwrap_or_default());
    }
    let id = PROJECT_ID.to_owned();
    let selected = request
        .device
        .projects
        .as_ref()
        .is_none_or(|list| list.contains(&id));
    let projects = vec![mention::Project::new(
        &id,
        &seed.dir,
        selected,
        monocode_core::mobile::denylist::DenyList::default(),
    )];
    let cwd = seed.dir.to_string_lossy().into_owned();
    let roots: Vec<String> = seed.root.iter().cloned().collect();
    let mut ctx = mention::Context::new(&texts, &cwd, &roots, &projects);
    ctx.tool_texts = &tool_texts;
    mention::handle(&request.device.id, &request.params, &ctx).result
}

/// `file.prepare` through the real core policy and upload, over the seeded folder (the one project
/// of the device). `WEB_LINK_HEADLESS_MENTION_LEGACY_FLAG` makes it answer as a desktop without it.
fn answer_prepare(host: &MobileHost, data_dir: &Path, request: &Request) -> Result<Value, Failure> {
    use monocode_core::mobile::{file_transfer, mentioned_file as mention};
    let Some(seed) = mention_seed() else {
        return Err(Failure::coded(
            "unsupported",
            "The E2E desktop does not serve this method",
        ));
    };
    if seed.legacy_flag.as_ref().is_some_and(|flag| flag.exists()) {
        return Err(Failure::coded(
            "unsupported",
            "The E2E desktop does not serve this method",
        ));
    }
    let Some((base, token)) = file_transfer::endpoint(host) else {
        return Err(Failure::from("Not paired"));
    };
    let selected = request
        .device
        .projects
        .as_ref()
        .is_none_or(|list| list.contains(&PROJECT_ID.to_owned()));
    let projects = vec![mention::Project::new(
        PROJECT_ID,
        &seed.dir,
        selected,
        monocode_core::mobile::denylist::DenyList::default(),
    )];
    let cwd = seed.dir.to_string_lossy().into_owned();
    let roots: Vec<String> = seed.root.iter().cloned().collect();
    let no_texts: [String; 0] = [];
    let mut ctx = mention::Context::new(&no_texts, &cwd, &roots, &projects);
    ctx.private_dirs = vec![data_dir.to_path_buf()];
    let store = file_transfer::HttpStore {
        base: &base,
        token: &token,
    };
    let keys = file_transfer::keystore::KeyStore::shared(data_dir);
    mention::transfer::handle(&request.device, &request.params, &ctx, &store, &keys).result
}

fn answer(conversation: &mut Conversation, request: &Request) -> Result<Value, Failure> {
    let known = is_known_conversation(request.params["conversationId"].as_str());
    match request.method.as_str() {
        "chat.mentionedFile" => answer_mentioned(request),
        "conversation.read" if known => Ok(conversation.read_as(
            request.params["conversationId"]
                .as_str()
                .unwrap_or(SESSION_ID),
        )),
        "conversation.send" if known => match request.params["text"].as_str() {
            Some(text) if !text.is_empty() => {
                conversation.send(text, Instant::now());
                Ok(json!({"sessionId": SESSION_ID}))
            }
            _ => Err(Failure::from("Message text is missing")),
        },
        "session.cancel" if known => {
            conversation.cancel();
            Ok(json!({"sessionId": SESSION_ID}))
        }
        "conversation.read" | "conversation.send" | "session.cancel" => {
            Err(Failure::from("Unknown conversation"))
        }
        _ => Err(Failure::coded(
            "unsupported",
            "The E2E desktop does not serve this method",
        )),
    }
}

/// Watches the web link: prints state changes, confirms pending devices when asked to (E2E only) and prints
/// devices that disappeared.
fn monitor_loop(
    host: MobileHost,
    confirm: Option<ConfirmPermission>,
    known: Known,
    lines: Lines,
    stop: Arc<AtomicBool>,
) {
    let mut last_state = None;
    let mut last_error: Option<String> = None;
    while !stop.load(Ordering::SeqCst) {
        let status = host.web_status();
        if last_state != Some(status.state) {
            match status.state {
                WebState::Connected => say(
                    &lines,
                    json!({"event": "online", "desktopId": status.desktop_id, "fingerprint": status.fingerprint}),
                ),
                WebState::Revoked => say(&lines, json!({"event": "revoked", "scope": "desktop"})),
                WebState::Offline if last_state == Some(WebState::Connected) => {
                    say(&lines, json!({"event": "offline"}));
                }
                _ => {}
            }
            last_state = Some(status.state);
        }
        if status.error != last_error {
            if let Some(message) = &status.error {
                say(&lines, json!({"event": "error", "message": message}));
            }
            last_error = status.error.clone();
        }
        if let Some(confirm) = confirm {
            for pending in &status.pending {
                match host.web_confirm(&pending.fingerprint, confirm.permission(), None, None) {
                    Ok(device) => say(
                        &lines,
                        json!({
                            "event": "confirmed",
                            "deviceId": device.server_device_id,
                            "permission": permission_name(device.permission),
                        }),
                    ),
                    Err(message) => say(&lines, json!({"event": "error", "message": message})),
                }
            }
        }
        let now: HashSet<String> = status
            .devices
            .iter()
            .filter_map(|device| device.server_device_id.clone())
            .collect();
        let mut known = known.lock().unwrap_or_else(|e| e.into_inner());
        for gone in known.difference(&now) {
            say(
                &lines,
                json!({"event": "revoked", "scope": "device", "deviceId": gone}),
            );
        }
        *known = now;
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The relay ids of the paired devices the monitor saw last. A desktop-side `revoke` command removes its device
/// here and prints the event itself, so a device revoked between two monitor ticks is still reported, once.
type Known = Arc<std::sync::Mutex<HashSet<String>>>;

/// A stdin command of `serve`.
#[derive(Debug, PartialEq, Eq)]
enum Cmd {
    Revoke {
        device_id: String,
    },
    Approve {
        device_id: String,
        permission: Permission,
    },
    Deny {
        device_id: String,
    },
    Permission {
        device_id: String,
        permission: Permission,
    },
    Status,
    Unpair,
    Quit,
}

fn parse_command(line: &str) -> Result<Cmd, String> {
    let value: Value = serde_json::from_str(line).map_err(|e| format!("not a JSON line: {e}"))?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} is missing"))
    };
    let permission = || match text("permission")?.as_str() {
        "read-only" => Ok(Permission::Read),
        "control" => Ok(Permission::Control),
        _ => Err("permission must be read-only or control".to_owned()),
    };
    match text("cmd")?.as_str() {
        "revoke" => Ok(Cmd::Revoke {
            device_id: text("deviceId")?,
        }),
        "approve" => Ok(Cmd::Approve {
            device_id: text("deviceId")?,
            permission: permission()?,
        }),
        "deny" => Ok(Cmd::Deny {
            device_id: text("deviceId")?,
        }),
        "permission" => Ok(Cmd::Permission {
            device_id: text("deviceId")?,
            permission: permission()?,
        }),
        "status" => Ok(Cmd::Status),
        "unpair" => Ok(Cmd::Unpair),
        "quit" => Ok(Cmd::Quit),
        other => Err(format!("unknown command: {other}")),
    }
}

/// Runs one command. `Ok(Some(code))` ends the run.
fn run_command(
    host: &MobileHost,
    command: Cmd,
    known: &Known,
    lines: &Lines,
) -> Result<Option<i32>, String> {
    let status = host.web_status();
    // A device is named by the relay's id (what the web app and the directory call `deviceId`).
    let paired = |id: &str| {
        status
            .devices
            .iter()
            .find(|d| d.server_device_id.as_deref() == Some(id) || d.id == id)
            .cloned()
            .ok_or_else(|| format!("no paired web device {id}"))
    };
    let pending = |id: &str| {
        status
            .pending
            .iter()
            .find(|p| p.server_device_id == id || p.fingerprint == id)
            .cloned()
            .ok_or_else(|| format!("no pending web device {id}"))
    };
    match command {
        Cmd::Revoke { device_id } => {
            let device = paired(&device_id)?;
            host.web_revoke(&device.id)?;
            if let Some(server_id) = device.server_device_id {
                known
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&server_id);
                say(
                    lines,
                    json!({"event": "revoked", "scope": "device", "deviceId": server_id}),
                );
            }
        }
        Cmd::Approve {
            device_id,
            permission,
        } => {
            let device =
                host.web_confirm(&pending(&device_id)?.fingerprint, permission, None, None)?;
            say(
                lines,
                json!({
                    "event": "confirmed",
                    "deviceId": device.server_device_id,
                    "permission": permission_name(permission),
                }),
            );
        }
        Cmd::Deny { device_id } => host.web_deny(&pending(&device_id)?.fingerprint)?,
        Cmd::Permission {
            device_id,
            permission,
        } => {
            let device = paired(&device_id)?;
            host.set_device_access(
                &device.id,
                DeviceKind::Web,
                permission,
                device.projects.clone(),
                device.monos.clone(),
            )?;
            say(
                lines,
                json!({
                    "event": "permission",
                    "deviceId": device.server_device_id,
                    "permission": permission_name(permission),
                }),
            );
        }
        Cmd::Status => {
            let state = match status.state {
                WebState::Off => "off",
                WebState::Connecting => "connecting",
                WebState::Connected => "connected",
                WebState::Offline => "offline",
                WebState::Revoked => "revoked",
            };
            say(lines, json!({"event": "status", "state": state}));
        }
        Cmd::Unpair => {
            host.web_unpair()?;
            say(lines, json!({"event": "unpaired"}));
            return Ok(Some(0));
        }
        Cmd::Quit => return Ok(Some(0)),
    }
    Ok(None)
}

/// Reads commands until `quit`, `unpair` or the end of the input. The end of the input does NOT end the
/// run (a harness may start `serve` with no stdin at all).
fn command_loop(host: MobileHost, input: Box<dyn BufRead + Send>, known: Known, lines: Lines) {
    for line in input.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let outcome =
            parse_command(&line).and_then(|command| run_command(&host, command, &known, &lines));
        match outcome {
            Ok(Some(code)) => {
                let _ = lines.send(Output::Exit(code));
                return;
            }
            Ok(None) => {}
            Err(message) => say(&lines, json!({"event": "error", "message": message})),
        }
    }
}

/// The desktop side of the web link for the E2E harness. Prints JSON lines on `out`, takes commands on
/// `input`. Runs until `quit`, `unpair` or the process is killed.
fn run_serve(
    data_dir: &Path,
    confirm: Option<ConfirmPermission>,
    input: Box<dyn BufRead + Send>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let (host, events) = match open_host(data_dir) {
        Ok(opened) => opened,
        Err(e) => {
            let _ = writeln!(err, "error: {e}");
            return 2;
        }
    };
    let _ = host.web_refresh_identity();
    if !host.web_status().paired {
        let _ = writeln!(
            err,
            "error: {} has no web pairing: run `pair` first",
            data_dir.display()
        );
        return 2;
    }
    if let Err(e) = host.publish(e2e_catalog()) {
        let _ = writeln!(err, "error: cannot publish the catalog: {e}");
        return 2;
    }
    web_link::start(&host);
    if let Err(e) = host.set_web_enabled(true) {
        let _ = writeln!(err, "error: cannot start the web link: {e}");
        return 2;
    }
    let (sender, receiver) = std::sync::mpsc::channel::<Output>();
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (host, lines, stop) = (host.clone(), sender.clone(), stop.clone());
        let dir = data_dir.to_path_buf();
        std::thread::spawn(move || app_loop(host, events, lines, stop, dir));
    }
    let known = Known::default();
    {
        let (host, lines, stop, known) =
            (host.clone(), sender.clone(), stop.clone(), known.clone());
        std::thread::spawn(move || monitor_loop(host, confirm, known, lines, stop));
    }
    {
        let (host, lines) = (host.clone(), sender.clone());
        std::thread::spawn(move || command_loop(host, input, known, lines));
    }
    drop(sender);
    let code = loop {
        match receiver.recv() {
            Ok(Output::Line(line)) => emit(out, line),
            Ok(Output::Exit(code)) => break code,
            Err(_) => break 0,
        }
    };
    while let Ok(Output::Line(line)) = receiver.try_recv() {
        emit(out, line);
    }
    stop.store(true, Ordering::SeqCst);
    host.shutdown();
    code
}

// ---------------------------------------------------------------- check-vectors

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err(format!("bad hex string of length {}", text.len()));
    }
    (0..text.len() / 2)
        .map(|i| {
            u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|e| format!("bad hex: {e}"))
        })
        .collect()
}

/// `{"hex"}` or `{"prefix"?, "fill", "length"}`: the prefix, then the fill byte repeated up to `length` bytes.
fn bytes(spec: &Value) -> Result<Vec<u8>, String> {
    if let Some(hex) = spec.get("hex").and_then(Value::as_str) {
        return unhex(hex);
    }
    let mut out = match spec.get("prefix").and_then(Value::as_str) {
        Some(prefix) => unhex(prefix)?,
        None => Vec::new(),
    };
    let fill = unhex(
        spec.get("fill")
            .and_then(Value::as_str)
            .ok_or("byte string needs hex or fill")?,
    )?;
    let [fill] = fill[..] else {
        return Err("fill must be one byte".into());
    };
    let length = spec
        .get("length")
        .and_then(Value::as_u64)
        .ok_or("fill needs a length")? as usize;
    if length < out.len() {
        return Err("fill length is shorter than the prefix".into());
    }
    out.resize(length, fill);
    Ok(out)
}

fn uuid(text: &str) -> Result<[u8; 16], String> {
    unhex(&text.replace('-', ""))?
        .try_into()
        .map_err(|_| format!("bad uuid {text}"))
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value.get(key).ok_or_else(|| format!("missing field {key}"))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("field {key} is not a string"))
}

fn u64_field(value: &Value, key: &str) -> Result<u64, String> {
    field(value, key)?
        .as_u64()
        .ok_or_else(|| format!("field {key} is not a number"))
}

fn list<'a>(root: &'a Value, group: &str, key: &str) -> Result<&'a Vec<Value>, String> {
    field(field(root, group)?, key)?
        .as_array()
        .ok_or_else(|| format!("{group}.{key} is not an array"))
}

fn check_valid_frame(item: &Value) -> Result<(), String> {
    let wire = bytes(field(item, "frame")?)?;
    let decoded = field(item, "decoded")?;
    let type_code =
        u8::try_from(u64_field(decoded, "typeCode")?).map_err(|_| "typeCode out of range")?;
    let kind = FrameType::from_code(type_code).ok_or("unknown typeCode")?;
    if kind.name() != str_field(decoded, "type")? {
        return Err("type name does not match typeCode".into());
    }
    let frame = Frame {
        kind,
        channel: u16::try_from(u64_field(decoded, "channel")?)
            .map_err(|_| "channel out of range")?,
        device_id: uuid(str_field(decoded, "deviceId")?)?,
        body: bytes(field(decoded, "body")?)?,
    };
    let encoded = frame
        .encode()
        .map_err(|e| format!("encode failed: {}", e.as_str()))?;
    if encoded != wire {
        return Err("encode(decoded) differs from the frame bytes".into());
    }
    let back = Frame::decode(&wire).map_err(|e| format!("decode failed: {}", e.as_str()))?;
    if back != frame {
        return Err("decode(frame) differs from decoded".into());
    }
    Ok(())
}

fn check_invalid_frame(item: &Value) -> Result<(), String> {
    let want = str_field(item, "error")?;
    match Frame::decode(&bytes(field(item, "frame")?)?) {
        Ok(_) => Err(format!("decoded fine, expected {want}")),
        Err(e) if e.as_str() == want => Ok(()),
        Err(e) => Err(format!("failed with {}, expected {want}", e.as_str())),
    }
}

fn expected_plaintext(decoded: &Value) -> Result<Plaintext, String> {
    Ok(match str_field(decoded, "kind")? {
        "msg" => Plaintext::Msg(bytes(field(decoded, "payload")?)?),
        "chunk" => Plaintext::Chunk {
            msg_id: u32::try_from(u64_field(decoded, "msgId")?)
                .map_err(|_| "msgId out of range")?,
            index: u16::try_from(u64_field(decoded, "index")?).map_err(|_| "index out of range")?,
            last: field(decoded, "last")?
                .as_bool()
                .ok_or("last is not a bool")?,
            data: bytes(field(decoded, "data")?)?,
        },
        "rekey" => Plaintext::Rekey,
        other => return Err(format!("unknown plaintext kind {other}")),
    })
}

fn check_valid_plaintext(item: &Value) -> Result<(), String> {
    let wire = bytes(field(item, "plaintext")?)?;
    let expected = expected_plaintext(field(item, "decoded")?)?;
    let encoded = expected
        .encode()
        .map_err(|e| format!("encode failed: {}", e.as_str()))?;
    if encoded != wire {
        return Err("encode(decoded) differs from the plaintext bytes".into());
    }
    let back = Plaintext::decode(&wire).map_err(|e| format!("decode failed: {}", e.as_str()))?;
    if back != expected {
        return Err("decode(plaintext) differs from decoded".into());
    }
    Ok(())
}

fn check_invalid_plaintext(item: &Value) -> Result<(), String> {
    let want = str_field(item, "error")?;
    match Plaintext::decode(&bytes(field(item, "plaintext")?)?) {
        Ok(_) => Err(format!("decoded fine, expected {want}")),
        Err(e) if e.as_str() == want => Ok(()),
        Err(e) => Err(format!("failed with {}, expected {want}", e.as_str())),
    }
}

fn check_split(item: &Value) -> Result<(), String> {
    let fill = unhex(str_field(item, "messageFill")?)?;
    let [fill] = fill[..] else {
        return Err("messageFill must be one byte".into());
    };
    let message = vec![fill; u64_field(item, "messageLength")? as usize];
    let msg_id = u32::try_from(u64_field(item, "msgId")?).map_err(|_| "msgId out of range")?;
    let want: Vec<u64> = field(item, "partLengths")?
        .as_array()
        .ok_or("partLengths is not an array")?
        .iter()
        .map(|v| v.as_u64().ok_or("partLength is not a number"))
        .collect::<Result<_, _>>()?;
    let parts = split_message(&message, msg_id, MAX_REQUEST_BYTES)
        .map_err(|_| "split_message refused the message")?;
    let got: Vec<u64> = parts.iter().map(|p| p.len() as u64).collect();
    if got != want {
        return Err(format!("part lengths {got:?}, expected {want:?}"));
    }
    for (i, part) in parts.iter().enumerate() {
        let decoded = Plaintext::decode(part)
            .map_err(|e| format!("part {i} does not decode: {}", e.as_str()))?;
        let last = i + 1 == parts.len();
        match (parts.len(), decoded) {
            (1, Plaintext::Msg(_)) => {}
            (
                n,
                Plaintext::Chunk {
                    msg_id: id,
                    index,
                    last: l,
                    ..
                },
            ) if n > 1 => {
                if id != msg_id || usize::from(index) != i || l != last {
                    return Err(format!("part {i} has the wrong chunk header"));
                }
            }
            _ => return Err(format!("part {i} has the wrong kind")),
        }
    }
    Ok(())
}

/// Runs one group of vector checks and records the failures. Returns how many items ran.
fn run_group(
    label: &str,
    items: Result<&Vec<Value>, String>,
    check: fn(&Value) -> Result<(), String>,
    failures: &mut Vec<Value>,
) -> usize {
    let items = match items {
        Ok(items) => items,
        Err(e) => {
            failures.push(json!({"group": label, "name": null, "error": e}));
            return 0;
        }
    };
    if items.is_empty() {
        failures.push(json!({"group": label, "name": null, "error": "the group is empty"}));
    }
    for item in items {
        if let Err(e) = check(item) {
            let name = item.get("name").and_then(Value::as_str).unwrap_or("?");
            failures.push(json!({"group": label, "name": name, "error": e}));
        }
    }
    items.len()
}

/// Validates the frame and plaintext vectors of `vectors.json`. Returns the summary and whether all passed.
fn check_vectors(root: &Value) -> (Value, bool) {
    let mut failures = Vec::new();
    let frames_valid = run_group(
        "frames.valid",
        list(root, "frames", "valid"),
        check_valid_frame,
        &mut failures,
    );
    let frames_invalid = run_group(
        "frames.invalid",
        list(root, "frames", "invalid"),
        check_invalid_frame,
        &mut failures,
    );
    let plaintext_valid = run_group(
        "plaintext.valid",
        list(root, "plaintext", "valid"),
        check_valid_plaintext,
        &mut failures,
    );
    let plaintext_invalid = run_group(
        "plaintext.invalid",
        list(root, "plaintext", "invalid"),
        check_invalid_plaintext,
        &mut failures,
    );
    let plaintext_split = run_group(
        "plaintext.split",
        list(root, "plaintext", "split"),
        check_split,
        &mut failures,
    );
    let ok = failures.is_empty();
    let summary = json!({
        "ok": ok,
        "checked": {
            "framesValid": frames_valid,
            "framesInvalid": frames_invalid,
            "plaintextValid": plaintext_valid,
            "plaintextInvalid": plaintext_invalid,
            "plaintextSplit": plaintext_split,
        },
        "failures": failures,
    });
    (summary, ok)
}

// ---------------------------------------------------------------- entry

/// Runs a command; returns the exit code. Output goes to `out` and `err` so tests can capture it.
fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let command = match parse_args(args) {
        Ok(Command::Help) => {
            let _ = writeln!(out, "{USAGE}");
            return 2;
        }
        Ok(command) => command,
        Err(message) => {
            let _ = writeln!(err, "error: {message}\n{USAGE}");
            return 2;
        }
    };
    match command {
        Command::Help => unreachable!("handled above"),
        Command::Capabilities => {
            let _ = writeln!(out, "{CAPABILITIES_LINE}");
            0
        }
        Command::CheckVectors { path } => run_check_vectors(&path, out, err),
        Command::Pair {
            server,
            data_dir,
            machine_name,
            timeout,
        } => run_pair(&server, &data_dir, &machine_name, timeout, None, out, err),
        Command::Serve {
            data_dir,
            confirm_pending,
        } => run_serve(
            &data_dir,
            confirm_pending,
            Box::new(std::io::BufReader::new(std::io::stdin())),
            out,
            err,
        ),
    }
}

fn run_check_vectors(path: &Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            let _ = writeln!(err, "error: cannot read {}: {e}", path.display());
            return 2;
        }
    };
    let root: Value = match serde_json::from_str(&text) {
        Ok(root) => root,
        Err(e) => {
            let _ = writeln!(err, "error: {} is not valid JSON: {e}", path.display());
            return 2;
        }
    };
    let (summary, ok) = check_vectors(&root);
    let _ = writeln!(out, "{summary}");
    if ok {
        0
    } else {
        1
    }
}

fn main() {
    // The HTML transform of `chat.mentionedFile` runs in a child of this executable, like the app.
    monocode_core::mobile::mentioned_file::sandbox::maybe_run_child();
    monocode_core::mobile::mentioned_file::sandbox::enable_subprocess();
    // An E2E desktop that behaves like an older release: it does not advertise these.
    if let Ok(hidden) = std::env::var("WEB_LINK_HEADLESS_HIDE_CAPABILITIES") {
        let names: Vec<String> = hidden
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        monocode_core::mobile::policy::hide_capabilities(&names);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let code = run(&args, &mut stdout.lock(), &mut stderr.lock());
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_mono_carries_a_known_mascot_and_a_hex_color_like_the_app() {
        let catalog = catalog_with_mono(true);
        let mono = &catalog["monos"][0];
        let mascot = mono["mascot"].as_str().expect("mascot");
        assert!(monocode_core::project_appearance::MASCOTS.contains(&mascot));
        let color = mono["color"].as_str().expect("color");
        assert!(
            color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        );
        assert_eq!(catalog_with_mono(false)["monos"], json!([]));
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn vectors_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/protocol/vectors.json")
    }

    fn run_capture(list: &[&str]) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&args(list), &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn parses_each_command() {
        assert_eq!(
            parse_args(&args(&["capabilities"])),
            Ok(Command::Capabilities)
        );
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(
            parse_args(&args(&["check-vectors", "v.json"])),
            Ok(Command::CheckVectors {
                path: "v.json".into()
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "pair",
                "--server",
                "http://localhost:8787/",
                "--data-dir",
                "d"
            ])),
            Ok(Command::Pair {
                server: "http://localhost:8787".into(),
                data_dir: "d".into(),
                machine_name: DEFAULT_MACHINE_NAME.into(),
                timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            })
        );
        // Option order does not matter; https works for any host.
        assert_eq!(
            parse_args(&args(&[
                "pair",
                "--timeout-secs",
                "30",
                "--machine-name",
                " My PC ",
                "--data-dir",
                "d",
                "--server",
                "https://zptah.example"
            ])),
            Ok(Command::Pair {
                server: "https://zptah.example".into(),
                data_dir: "d".into(),
                machine_name: "My PC".into(),
                timeout: Duration::from_secs(30),
            })
        );
        assert_eq!(
            parse_args(&args(&["serve", "--data-dir", "d"])),
            Ok(Command::Serve {
                data_dir: "d".into(),
                confirm_pending: None
            })
        );
        assert_eq!(
            parse_args(&args(&[
                "serve",
                "--confirm-pending",
                "read-only",
                "--data-dir",
                "d"
            ])),
            Ok(Command::Serve {
                data_dir: "d".into(),
                confirm_pending: Some(ConfirmPermission::ReadOnly)
            })
        );
        assert!(matches!(
            parse_args(&args(&[
                "serve",
                "--data-dir",
                "d",
                "--confirm-pending",
                "control"
            ])),
            Ok(Command::Serve {
                confirm_pending: Some(ConfirmPermission::Control),
                ..
            })
        ));
    }

    #[test]
    fn rejects_bad_arguments() {
        for bad in [
            &[][..],
            &["nope"],
            &["capabilities", "extra"],
            &["check-vectors"],
            &["check-vectors", "a", "b"],
            &["pair"],
            &["pair", "--server", "http://localhost:1"],
            &["pair", "--data-dir", "d"],
            // plain http only for localhost
            &["pair", "--server", "http://example.com", "--data-dir", "d"],
            &["pair", "--server", "ws://localhost:1", "--data-dir", "d"],
            &["pair", "--server", "not a url", "--data-dir", "d"],
            &["pair", "--server", "http://localhost:1", "--data-dir", ""],
            &[
                "pair",
                "--server",
                "http://localhost:1",
                "--data-dir",
                "d",
                "--timeout-secs",
                "0",
            ],
            &[
                "pair",
                "--server",
                "http://localhost:1",
                "--data-dir",
                "d",
                "--machine-name",
                " ",
            ],
            &[
                "pair",
                "--server",
                "http://localhost:1",
                "--server",
                "http://localhost:2",
                "--data-dir",
                "d",
            ],
            &["serve"],
            &["serve", "--data-dir"],
            &["serve", "--data-dir", "d", "--confirm-pending", "admin"],
            &["serve", "--data-dir", "d", "--confirm-pending"],
            &["serve", "--data-dir", "d", "--token", "secret"],
            // the old relay/token-file flags are gone: the relay URL comes from the pairing
            &["serve", "--data-dir", "d", "--relay", "ws://x"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn capabilities_prints_one_json_line() {
        let (code, out, _) = run_capture(&["capabilities"]);
        assert_eq!(code, 0);
        assert_eq!(out.lines().count(), 1);
        let value: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(
            value,
            json!({"version": 1, "serve": true, "pair": true, "reason": null, "checkVectors": true})
        );
    }

    #[test]
    fn help_and_unknown_commands_exit_2() {
        assert_eq!(run_capture(&["--help"]).0, 2);
        let (code, _, err) = run_capture(&["bogus"]);
        assert_eq!(code, 2);
        assert!(err.contains("usage:"));
    }

    #[test]
    fn check_vectors_passes_on_the_real_file() {
        let path = vectors_path();
        let (code, out, err) = run_capture(&["check-vectors", path.to_str().unwrap()]);
        assert_eq!(code, 0, "stdout: {out} stderr: {err}");
        let summary: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(summary["ok"], json!(true));
        assert!(summary["checked"]["framesValid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["framesInvalid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["plaintextValid"].as_u64().unwrap() > 0);
        assert!(summary["checked"]["plaintextSplit"].as_u64().unwrap() > 0);
    }

    #[test]
    fn check_vectors_fails_on_a_corrupted_copy() {
        let mut root: Value =
            serde_json::from_str(&std::fs::read_to_string(vectors_path()).unwrap()).unwrap();
        // Change the channel of the first valid frame: encode(decoded) no longer equals the bytes.
        root["frames"]["valid"][0]["decoded"]["channel"] = json!(4242);
        // And break the expected error of the first invalid frame.
        root["frames"]["invalid"][0]["error"] = json!("too_long");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.json");
        std::fs::write(&path, serde_json::to_string(&root).unwrap()).unwrap();
        let (code, out, _) = run_capture(&["check-vectors", path.to_str().unwrap()]);
        assert_eq!(code, 1, "{out}");
        let summary: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(summary["ok"], json!(false));
        assert_eq!(summary["failures"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn check_vectors_exit_2_on_io_error_and_bad_json() {
        assert_eq!(
            run_capture(&["check-vectors", "/nonexistent/vectors.json"]).0,
            2
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(run_capture(&["check-vectors", path.to_str().unwrap()]).0, 2);
    }

    #[test]
    fn serve_needs_a_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_str().unwrap();
        let (code, out, err) = run_capture(&["serve", "--data-dir", data]);
        assert_eq!(code, 2);
        assert!(err.contains("run `pair` first"), "{err}");
        assert!(out.is_empty());
    }

    // ------------------------------------------------------------ the fake app

    #[test]
    fn the_catalog_holds_the_e2e_names() {
        let catalog = e2e_catalog();
        assert_eq!(catalog["projects"][0]["name"], "E2E Project");
        assert_eq!(catalog["sessions"][0]["title"], "E2E session");
        assert_eq!(
            catalog["sessions"][0]["projectId"],
            catalog["projects"][0]["id"]
        );
    }

    #[test]
    fn send_appends_the_text_and_the_reply_comes_later() {
        let mut conversation = Conversation::new();
        let seeded = conversation.turns.len();
        let now = Instant::now();
        conversation.send("hello", now);
        assert_eq!(conversation.turns.len(), seeded + 1);
        assert_eq!(conversation.turns[seeded]["items"][0]["text"], "hello");
        assert!(conversation.read()["busy"].as_bool().unwrap());
        assert!(!conversation.tick(now), "the reply is not due yet");
        assert!(conversation.tick(now + REPLY_DELAY));
        assert_eq!(conversation.turns.len(), seeded + 2);
        assert_eq!(
            conversation.turns[seeded + 1]["items"][0]["text"],
            "Echo: hello"
        );
        assert!(!conversation.busy);
        let frame = conversation.frame();
        assert_eq!(frame["order"].as_array().unwrap().len(), seeded + 2);
        assert_eq!(frame["turns"], json!(conversation.turns));
    }

    #[test]
    fn a_held_send_streams_until_cancel() {
        let mut conversation = Conversation::new();
        let now = Instant::now();
        conversation.send("keep going [hold]", now);
        assert!(!conversation.tick(now + Duration::from_secs(60)));
        assert!(conversation.busy);
        conversation.cancel();
        assert!(!conversation.busy);
        assert!(!conversation.tick(now + Duration::from_secs(120)));
    }

    #[test]
    fn requests_for_other_conversations_and_methods_are_refused() {
        use monocode_core::mobile::DeviceContext;
        let context = DeviceContext {
            id: "d".into(),
            kind: DeviceKind::Web,
            permission: Permission::Control,
            projects: None,
            monos: None,
        };
        let mut conversation = Conversation::new();
        let ask = |conversation: &mut Conversation, method: &str, params: Value| {
            let (request, _) = Request::with_receiver("c", method, params, context.clone());
            answer(conversation, &request)
        };
        let read = ask(
            &mut conversation,
            "conversation.read",
            json!({"conversationId": SESSION_ID}),
        )
        .unwrap();
        assert_eq!(read["turns"].as_array().unwrap().len(), 2);
        let other = ask(
            &mut conversation,
            "conversation.read",
            json!({"conversationId": "nope"}),
        );
        assert!(other.is_err());
        let empty = ask(
            &mut conversation,
            "conversation.send",
            json!({"conversationId": SESSION_ID, "text": ""}),
        );
        assert!(empty.is_err());
        let unsupported = ask(&mut conversation, "project.files", json!({})).unwrap_err();
        assert_eq!(unsupported.code.as_deref(), Some("unsupported"));
    }

    #[test]
    fn commands_parse() {
        assert_eq!(parse_command(r#"{"cmd":"quit"}"#), Ok(Cmd::Quit));
        assert_eq!(parse_command(r#"{"cmd":"unpair"}"#), Ok(Cmd::Unpair));
        assert_eq!(parse_command(r#"{"cmd":"status"}"#), Ok(Cmd::Status));
        assert_eq!(
            parse_command(r#"{"cmd":"revoke","deviceId":"x"}"#),
            Ok(Cmd::Revoke {
                device_id: "x".into()
            })
        );
        assert_eq!(
            parse_command(r#"{"cmd":"approve","deviceId":"x","permission":"read-only"}"#),
            Ok(Cmd::Approve {
                device_id: "x".into(),
                permission: Permission::Read
            })
        );
        assert_eq!(
            parse_command(r#"{"cmd":"permission","deviceId":"x","permission":"control"}"#),
            Ok(Cmd::Permission {
                device_id: "x".into(),
                permission: Permission::Control
            })
        );
        for bad in [
            "not json",
            r#"{"cmd":"revoke"}"#,
            r#"{"cmd":"approve","deviceId":"x"}"#,
            r#"{"cmd":"approve","deviceId":"x","permission":"admin"}"#,
            r#"{"cmd":"dance"}"#,
            "{}",
        ] {
            assert!(parse_command(bad).is_err(), "{bad}");
        }
    }

    // ------------------------------------------------------------ pair against a local HTTP stub

    use std::io::{BufReader, Read};
    use std::net::TcpListener;
    use std::sync::Mutex;

    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    const FAKE_TOKEN: &str = "obviously-fake-desktop-token";
    const FAKE_DEVICE_CODE: &str = "obviously-fake-device-code";
    const DESKTOP_ID: &str = "11111111-2222-4333-8444-555555555555";

    fn approved() -> (u16, String) {
        (
            200,
            format!(
                r#"{{"status":"approved","desktopId":"{DESKTOP_ID}","desktopToken":"{FAKE_TOKEN}","account":{{"handle":"alice"}},"relayUrl":"ws://127.0.0.1:{{PORT}}/relay/desktop/{DESKTOP_ID}"}}"#
            ),
        )
    }

    /// A one-thread HTTP stub of `/api/pair/start` and `/api/pair/poll`. Answers each poll from `polls`
    /// (a status and a body, `{PORT}` replaced by the stub's port); records every request body. Any other
    /// path (the relay the host dials after an approval) gets a 404.
    fn stub(polls: Vec<(u16, String)>) -> (String, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let seen: Seen = Arc::default();
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut polls = polls.into_iter();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let path = request_line.split_whitespace().nth(1).unwrap().to_string();
                let (status, text) = if path == "/api/pair/start" {
                    (
                        200,
                        format!(
                            r#"{{"userCode":"ABCD-2345","deviceCode":"{FAKE_DEVICE_CODE}","confirmationCode":"123456","expiresIn":600,"interval":3}}"#
                        ),
                    )
                } else if path == "/api/pair/poll" {
                    polls.next().unwrap_or((500, "{}".into()))
                } else {
                    (404, "{}".into())
                };
                if path.starts_with("/api/pair/") {
                    log.lock().unwrap().push((
                        path.clone(),
                        serde_json::from_slice(&body).unwrap_or(Value::Null),
                    ));
                }
                let text = text.replace("{PORT}", &port.to_string());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (base, seen)
    }

    fn pair_capture(base: &str, dir: &Path, timeout: Duration) -> (i32, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_pair(
            base,
            dir,
            "stub-machine",
            timeout,
            Some(Duration::from_millis(10)),
            &mut out,
            &mut err,
        );
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn pair_prints_the_user_code_polls_and_seals_the_identity() {
        let (base, seen) = stub(vec![(200, r#"{"status":"pending"}"#.into()), approved()]);
        let dir = tempfile::tempdir().unwrap();
        let (code, out, err) = pair_capture(&base, dir.path(), Duration::from_secs(20));
        assert_eq!(code, 0, "stderr: {err}");
        let lines: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["event"], "pairing");
        assert_eq!(lines[0]["userCode"], "ABCD-2345");
        assert_eq!(lines[0]["confirmationCode"], "123456");
        assert_eq!(lines[1]["event"], "paired");
        assert_eq!(lines[1]["account"], "alice");
        assert_eq!(lines[1]["desktopId"], DESKTOP_ID);
        assert_eq!(lines[1]["fingerprint"], lines[0]["fingerprint"]);
        // The token and the device code never reach stdout or stderr, and no plain token file exists.
        let output = format!("{out}{err}");
        assert!(!output.contains("obviously-fake"));
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() && path.extension().is_none_or(|e| e != "db") {
                let bytes = std::fs::read(&path).unwrap_or_default();
                assert!(
                    !String::from_utf8_lossy(&bytes).contains(FAKE_TOKEN),
                    "{} holds the token in clear",
                    path.display()
                );
            }
        }
        // A new host on the same directory finds the sealed identity.
        let (host, _events) = open_host(dir.path()).unwrap();
        host.web_refresh_identity().unwrap();
        let status = host.web_status();
        assert!(status.paired);
        assert_eq!(status.desktop_id.as_deref(), Some(DESKTOP_ID));
        assert_eq!(status.account.as_deref(), Some("alice"));
        assert_eq!(
            status.fingerprint.as_deref(),
            lines[1]["fingerprint"].as_str()
        );
        // The start body carries the key and a challenge that the poll's verifier answers.
        let seen = seen.lock().unwrap();
        let (start_path, start) = &seen[0];
        assert_eq!(start_path, "/api/pair/start");
        assert_eq!(start["machineName"], "stub-machine");
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(seen[1].1["verifier"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)),
            start["challenge"].as_str().unwrap()
        );
        assert_eq!(seen[1].1["deviceCode"], FAKE_DEVICE_CODE);
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn pair_exit_4_when_denied_and_seals_nothing() {
        let (base, _) = stub(vec![(200, r#"{"status":"denied"}"#.into())]);
        let dir = tempfile::tempdir().unwrap();
        let (code, out, _) = pair_capture(&base, dir.path(), Duration::from_secs(20));
        assert_eq!(code, 4);
        assert!(out.contains("ABCD-2345"));
        let (host, _events) = open_host(dir.path()).unwrap();
        host.web_refresh_identity().unwrap();
        assert!(!host.web_status().paired);
    }

    #[test]
    fn pair_exit_5_when_the_server_is_unreachable() {
        // A port that was just released: nothing listens there.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir = tempfile::tempdir().unwrap();
        let (code, _, err) = pair_capture(
            &format!("http://127.0.0.1:{port}"),
            dir.path(),
            Duration::from_secs(5),
        );
        assert_eq!(code, 5, "{err}");
    }

    #[test]
    fn pair_times_out_with_exit_4() {
        let (base, _) = stub(vec![(200, r#"{"status":"pending"}"#.into()); 1000]);
        let dir = tempfile::tempdir().unwrap();
        let (code, _, err) = pair_capture(&base, dir.path(), Duration::from_millis(200));
        assert_eq!(code, 4);
        assert!(err.contains("timed out"));
    }

    #[test]
    fn serve_runs_after_a_pairing_until_quit_and_prints_no_token() {
        let (base, _) = stub(vec![approved()]);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            pair_capture(&base, dir.path(), Duration::from_secs(20)).0,
            0
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let input = std::io::Cursor::new(
            "not json\n{\"cmd\":\"revoke\",\"deviceId\":\"nobody\"}\n{\"cmd\":\"quit\"}\n",
        );
        let code = run_serve(dir.path(), None, Box::new(input), &mut out, &mut err);
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
        let out = String::from_utf8(out).unwrap();
        let events: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let errors = events.iter().filter(|e| e["event"] == "error").count();
        assert!(errors >= 2, "bad commands are reported: {out}");
        assert!(
            !out.contains("obviously-fake")
                && !String::from_utf8_lossy(&err).contains("obviously-fake")
        );
    }
}
