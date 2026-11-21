//! Audit trail of what remote devices did (security.md section 7). One JSON
//! line per event in `mobile-audit.jsonl`, rotated by size. Parameters and
//! content are never written; the only path ever logged is a file read's.
use super::policy::{self, Access, Code, DeviceContext, DeviceKind, Permission};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

/// The log file, inside the host's data directory.
pub const FILE: &str = "mobile-audit.jsonl";
/// The file rotates once it reaches this size.
const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Rotated files kept next to the current one (`.1` newest to `.5` oldest).
const KEEP: usize = 5;
/// Fingerprint characters written.
const FINGERPRINT_CHARS: usize = 8;
/// Longest value written for each field, in characters. A device chooses
/// these strings, so an unbounded line would let it push every earlier record
/// out of the rotation (security finding M1).
const METHOD_CHARS: usize = 64;
const ID_CHARS: usize = 128;
const PATH_CHARS: usize = 1024;
const DETAIL_CHARS: usize = 256;

/// `text` cut to `max` characters (never inside one) with control characters
/// replaced, so a line stays short and readable.
fn capped(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

fn capped_opt(text: &Option<String>, max: usize) -> Option<String> {
    text.as_deref().map(|text| capped(text, max))
}

/// How a request reached the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Quic,
    Web,
}

impl Transport {
    fn as_str(self) -> &'static str {
        match self {
            Transport::Quic => "quic",
            Transport::Web => "web",
        }
    }
}

impl From<DeviceKind> for Transport {
    fn from(kind: DeviceKind) -> Self {
        match kind {
            DeviceKind::Phone => Transport::Quic,
            DeviceKind::Web => Transport::Web,
        }
    }
}

/// What became of the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Denied(Code),
    Error,
}

impl Outcome {
    fn text(self) -> String {
        match self {
            Outcome::Ok => "ok".into(),
            Outcome::Denied(code) => format!("denied:{}", code.as_str()),
            Outcome::Error => "error".into(),
        }
    }
}

/// Moments in a device's life with this desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Enroll,
    Pending,
    Confirm,
    Deny,
    Revoke,
    Connect,
    Disconnect,
    PermissionChange,
    /// A web device widened its own project or Mono selection.
    SelectionWiden,
    /// A web device narrowed its own project or Mono selection.
    SelectionNarrow,
    /// A web device asked to widen and was refused for its ticket.
    SelectionWidenRefused,
    /// The server accepted (and consumed) a passkey ticket to open a terminal.
    TerminalTicketAccepted,
    /// A terminal open was refused for its ticket. The ticket is never logged.
    TerminalTicketRefused,
    /// A shell opened for a web device.
    TerminalOpen,
    /// A shell ended; the detail is why.
    TerminalClose,
    /// The user switched "Allow web terminal" for a device.
    TerminalToggle,
}

impl Lifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Lifecycle::Enroll => "enroll",
            Lifecycle::Pending => "pending",
            Lifecycle::Confirm => "confirm",
            Lifecycle::Deny => "deny",
            Lifecycle::Revoke => "revoke",
            Lifecycle::Connect => "connect",
            Lifecycle::Disconnect => "disconnect",
            Lifecycle::PermissionChange => "permission",
            Lifecycle::SelectionWiden => "selection_widen",
            Lifecycle::SelectionNarrow => "selection_narrow",
            Lifecycle::SelectionWidenRefused => "selection_widen_refused",
            Lifecycle::TerminalTicketAccepted => "terminal_ticket_accepted",
            Lifecycle::TerminalTicketRefused => "terminal_ticket_refused",
            Lifecycle::TerminalOpen => "terminal_open",
            Lifecycle::TerminalClose => "terminal_close",
            Lifecycle::TerminalToggle => "terminal_toggle",
        }
    }
}

/// One audited event. Parameters and content are never part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEvent {
    /// The first characters of the device's key fingerprint.
    pub fingerprint_prefix: String,
    pub transport: Transport,
    pub method: String,
    pub project_id: Option<String>,
    pub outcome: Outcome,
    pub command_id: Option<String>,
    /// The call is not a read once its parameters are looked at (a
    /// `mono.file` carrying `text` is a save), so it is written when it
    /// succeeds.
    pub writes: bool,
    /// The call was admitted and reached the app, which answered it. Set by
    /// the host after the call; a refusal in the core leaves it false.
    pub from_app: bool,
}

impl AuditEvent {
    /// The event of a request: only the method and the ids, never the
    /// parameters or their content. The host builds every call's event here.
    pub fn for_call(
        fingerprint: &str,
        transport: Transport,
        method: &str,
        request: &Value,
    ) -> Self {
        Self {
            fingerprint_prefix: prefix(fingerprint),
            transport,
            method: method.into(),
            project_id: request["params"]["projectId"].as_str().map(Into::into),
            outcome: Outcome::Ok,
            command_id: request["id"].as_str().map(Into::into),
            writes: !matches!(
                policy::classify(method, &request["params"]),
                Some(Access::Read)
            ),
            from_app: false,
        }
    }
}

/// A file read to record: the device's fingerprint, where, what and how it
/// ended.
#[derive(Clone, Copy)]
pub struct FileRead<'a> {
    pub fingerprint: &'a str,
    pub project_id: Option<&'a str>,
    pub path: &'a str,
    pub command_id: Option<&'a str>,
    pub transport: Transport,
    pub outcome: Outcome,
}

/// A `chat.mentionedFile` call to record: the conversation, the canonical path
/// served (or the requested one when refused), the asset asked for, the bytes
/// of the reply, how it ended and, for a refusal, which rule refused it
/// (`not_mentioned`, `not_exact`, `no_evidence`, `deny_list`, ...: see
/// `mentioned_file::rule`). The rule is for this log only; the device gets the
/// short code of the failure.
#[derive(Clone, Copy)]
pub struct MentionedFile<'a> {
    pub fingerprint: &'a str,
    pub conversation_id: Option<&'a str>,
    pub path: &'a str,
    pub asset: Option<&'a str>,
    pub bytes: u64,
    pub rule: Option<&'a str>,
    pub command_id: Option<&'a str>,
    pub transport: Transport,
    pub outcome: Outcome,
}

/// A `file.prepare` call to record: the conversation, the canonical path (or the requested one
/// when refused), the asset, the size of the file, whether a Control device opened a file
/// outside its shared projects, whether the server already held the object, the rule that
/// refused it and how it ended. Never the content or the key.
#[derive(Clone, Copy)]
pub struct FileTransfer<'a> {
    pub fingerprint: &'a str,
    pub conversation_id: Option<&'a str>,
    pub path: &'a str,
    pub asset: Option<&'a str>,
    pub bytes: u64,
    pub outside_projects: bool,
    pub reused: bool,
    pub rule: Option<&'a str>,
    pub command_id: Option<&'a str>,
    pub transport: Transport,
    pub outcome: Outcome,
}

/// One answer to a remote approval, written by the app once it resolved the
/// project. Ids and the decision only: never the label or detail text.
#[derive(Clone, Copy)]
pub struct ApprovalAnswer<'a> {
    pub fingerprint: &'a str,
    pub project_id: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub request_id: Option<&'a str>,
    /// `allow` or `deny` when the call named a valid decision.
    pub decision: Option<&'a str>,
    pub command_id: Option<&'a str>,
    pub transport: Transport,
    pub outcome: Outcome,
}

/// One `commands.run` call to record: the command id from the `/` menu (a closed set, never
/// the text of the message), whether it switched a mode on or off, the conversation and how it
/// ended. `operator` gives the thread access to Monoptah, which is why every outcome is kept.
#[derive(Clone, Copy)]
pub struct SlashCommand<'a> {
    pub fingerprint: &'a str,
    pub conversation_id: Option<&'a str>,
    pub command: &'a str,
    pub enabled: Option<bool>,
    pub command_id: Option<&'a str>,
    pub transport: Transport,
    pub outcome: Outcome,
}

/// Methods whose class depends on parameters `record` does not get. They are
/// written even when they succeed, because the no-parameter class may be the
/// harmless one while the call was an edit (a selection change, a git action,
/// a session start). `accounts.usage` is the exception: without `refresh` it
/// is a read, and it is polled, so it counts as one.
fn param_dependent(method: &str) -> bool {
    matches!(
        method,
        "device.selection" | "project.gitAction" | "session.start"
    )
}

/// Whether a call with this outcome is written: every denial and error,
/// everything that is not a read, and nothing for a successful read.
fn should_write(event: &AuditEvent) -> bool {
    if event.outcome != Outcome::Ok {
        return true;
    }
    // A shell's polls and keystrokes would flood the log: its opening and its
    // end are written as lifecycle lines (with the reason), refusals above.
    if event.method.starts_with("terminal.") {
        return false;
    }
    if param_dependent(&event.method) || event.writes {
        return true;
    }
    !matches!(
        policy::classify(&event.method, &Value::Null),
        Some(Access::Read)
    )
}

/// A file read is written once, by the app, with its path and the outcome the
/// deny-list gave. The host's own line for it would be a second one, so it is
/// dropped when the app has answered (`ok`, or a refusal the app words:
/// `not_selected`, `path_denied`, `unsupported_target`). A refusal from the
/// core before the app, and any other failure, still write theirs.
fn written_by_the_app(event: &AuditEvent) -> bool {
    // An approval answer is written by the app for every outcome it words with
    // a code (and `ok`); an uncoded failure is still the host's line.
    if event.method == "session.answerApproval" {
        return event.from_app && matches!(event.outcome, Outcome::Ok | Outcome::Denied(_));
    }
    mentioned_file_answered(event)
        || file_transfer_answered(event)
        || slash_command_answered(event)
        || event.method == "project.file"
            && event.from_app
            && matches!(
                event.outcome,
                Outcome::Ok
                    | Outcome::Denied(
                        Code::NotSelected | Code::PathDenied | Code::UnsupportedTarget
                    )
            )
}

/// `chat.mentionedFile` is audited by the app for every outcome, successes
/// too, with its own fields (conversation, asset, bytes). Every answer the app
/// gave (`ok`, a coded refusal, an uncoded failure) is its line; the host's
/// would be a second. Only a failure from the core before the app is the host's.
fn mentioned_file_answered(event: &AuditEvent) -> bool {
    event.method == "chat.mentionedFile" && event.from_app
}

/// `commands.run` is audited by the app for every outcome, with the command it ran.
fn slash_command_answered(event: &AuditEvent) -> bool {
    event.method == "commands.run" && event.from_app
}

/// `file.prepare` is audited by the app for every outcome, with its own fields.
fn file_transfer_answered(event: &AuditEvent) -> bool {
    event.method == "file.prepare" && event.from_app
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn prefix(fingerprint: &str) -> String {
    fingerprint.chars().take(FINGERPRINT_CHARS).collect()
}

/// An append-only, size-rotated log in one directory.
pub struct Log {
    dir: PathBuf,
    max_bytes: u64,
    keep: usize,
    write: Mutex<()>,
}

impl Log {
    pub fn new(dir: &Path) -> Self {
        Self::with_limits(dir, MAX_BYTES, KEEP)
    }

    /// A log with its own threshold, for tests.
    pub(crate) fn with_limits(dir: &Path, max_bytes: u64, keep: usize) -> Self {
        Self {
            dir: dir.to_path_buf(),
            max_bytes,
            keep,
            write: Mutex::new(()),
        }
    }

    fn base(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    fn rotated(&self, n: usize) -> PathBuf {
        self.dir.join(format!("{FILE}.{n}"))
    }

    /// Writes the call when it is worth keeping (see [`should_write`]).
    pub fn record(&self, device: &DeviceContext, event: &AuditEvent) {
        if !should_write(event) || written_by_the_app(event) {
            return;
        }
        // A method the table does not know is device-chosen text: its name is
        // kept short, in `detail`, and the method says only that it is unknown.
        let known = policy::classify(&event.method, &Value::Null).is_some();
        let mut line = json!({
            "ts": now_ms(),
            "deviceId": capped(&device.id, ID_CHARS),
            "fingerprint": prefix(&event.fingerprint_prefix),
            "transport": event.transport.as_str(),
            "event": "call",
            "method": if known { capped(&event.method, METHOD_CHARS) } else { "unknown".into() },
            "projectId": capped_opt(&event.project_id, ID_CHARS),
            "outcome": event.outcome.text(),
            "commandId": capped_opt(&event.command_id, ID_CHARS),
        });
        if !known {
            line["detail"] = capped(&event.method, METHOD_CHARS).into();
        }
        self.append(&line);
    }

    /// A file read, the one event that carries a path. The app writes it, with
    /// the outcome the deny-list gave; see [`written_by_the_app`].
    pub fn record_file_read(&self, device: &DeviceContext, read: &FileRead) {
        let FileRead {
            fingerprint,
            project_id,
            path,
            command_id,
            transport,
            outcome,
        } = *read;
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": capped(&device.id, ID_CHARS),
            "fingerprint": prefix(fingerprint),
            "transport": transport.as_str(),
            "event": "call",
            "method": "project.file",
            "projectId": project_id.map(|id| capped(id, ID_CHARS)),
            "outcome": outcome.text(),
            "commandId": command_id.map(|id| capped(id, ID_CHARS)),
            "path": capped(path, PATH_CHARS),
        }));
    }

    /// One line per `chat.mentionedFile` call, success included: who, which
    /// conversation, what path, which asset, how many bytes and the outcome.
    pub fn record_mentioned_file(&self, device: &DeviceContext, read: &MentionedFile) {
        let MentionedFile {
            fingerprint,
            conversation_id,
            path,
            asset,
            bytes,
            rule,
            command_id,
            transport,
            outcome,
        } = *read;
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": capped(&device.id, ID_CHARS),
            "fingerprint": prefix(fingerprint),
            "transport": transport.as_str(),
            "event": "call",
            "method": "chat.mentionedFile",
            "projectId": null,
            "outcome": outcome.text(),
            "commandId": command_id.map(|id| capped(id, ID_CHARS)),
            "conversationId": conversation_id.map(|id| capped(id, ID_CHARS)),
            "path": capped(path, PATH_CHARS),
            "asset": asset.map(|asset| capped(asset, PATH_CHARS)),
            "bytes": bytes,
            "rule": rule.map(|rule| capped(rule, ID_CHARS)),
        }));
    }

    /// One line per `file.prepare` call, success included. A Control device opening a file
    /// outside its shared projects is marked `outsideProjects`.
    pub fn record_file_transfer(&self, device: &DeviceContext, read: &FileTransfer) {
        let FileTransfer {
            fingerprint,
            conversation_id,
            path,
            asset,
            bytes,
            outside_projects,
            reused,
            rule,
            command_id,
            transport,
            outcome,
        } = *read;
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": capped(&device.id, ID_CHARS),
            "fingerprint": prefix(fingerprint),
            "transport": transport.as_str(),
            "event": "call",
            "method": "file.prepare",
            "projectId": null,
            "outcome": outcome.text(),
            "commandId": command_id.map(|id| capped(id, ID_CHARS)),
            "conversationId": conversation_id.map(|id| capped(id, ID_CHARS)),
            "path": capped(path, PATH_CHARS),
            "asset": asset.map(|asset| capped(asset, PATH_CHARS)),
            "bytes": bytes,
            "outsideProjects": outside_projects,
            "reused": reused,
            "permission": match device.permission {
                Permission::Read => "read",
                Permission::Control => "control",
            },
            "rule": rule.map(|rule| capped(rule, ID_CHARS)),
        }));
    }

    /// One line per `commands.run` call, success included: who, which conversation, which
    /// command and the outcome. Never the text of a message.
    pub fn record_slash_command(&self, device: &DeviceContext, run: &SlashCommand) {
        let SlashCommand {
            fingerprint,
            conversation_id,
            command,
            enabled,
            command_id,
            transport,
            outcome,
        } = *run;
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": capped(&device.id, ID_CHARS),
            "fingerprint": prefix(fingerprint),
            "transport": transport.as_str(),
            "event": "call",
            "method": "commands.run",
            "projectId": null,
            "outcome": outcome.text(),
            "commandId": command_id.map(|id| capped(id, ID_CHARS)),
            "conversationId": conversation_id.map(|id| capped(id, ID_CHARS)),
            "command": capped(command, METHOD_CHARS),
            "enabled": enabled,
            "permission": match device.permission {
                Permission::Read => "read",
                Permission::Control => "control",
            },
        }));
    }

    /// One line per remote approval answer: decision, conversation, request
    /// and project ids, and the outcome with its refusal code.
    pub fn record_approval_answer(&self, device: &DeviceContext, answer: &ApprovalAnswer) {
        let ApprovalAnswer {
            fingerprint,
            project_id,
            conversation_id,
            request_id,
            decision,
            command_id,
            transport,
            outcome,
        } = *answer;
        let id = |value: Option<&str>| value.map(|v| capped(v, ID_CHARS));
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": capped(&device.id, ID_CHARS),
            "fingerprint": prefix(fingerprint),
            "transport": transport.as_str(),
            "event": "call",
            "method": "session.answerApproval",
            "projectId": id(project_id),
            "outcome": outcome.text(),
            "commandId": id(command_id),
            "conversationId": id(conversation_id),
            "requestId": id(request_id),
            "decision": decision.filter(|d| matches!(*d, "allow" | "deny")),
        }));
    }

    /// One line per web push attempt or drop: category, target kind
    /// (`session`, `mono`, `habit`), device count and the outcome text. Metadata
    /// only: no token, URL, title, name or message text.
    pub fn record_web_push(&self, category: &str, target: &str, devices: usize, outcome: &str) {
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": null,
            "fingerprint": null,
            "transport": Transport::Web.as_str(),
            "event": "webPush",
            "method": null,
            "projectId": null,
            "outcome": capped(outcome, DETAIL_CHARS),
            "commandId": null,
            "category": capped(category, METHOD_CHARS),
            "target": capped(target, METHOD_CHARS),
            "devices": devices,
        }));
    }

    /// One line per app action a Mono took that changes what the user sees on this computer
    /// (`projects.add`): the action, the Mono's name, the canonical path and the outcome.
    /// No device is involved, so the device fields are null.
    pub fn record_app_action(&self, method: &str, mono: &str, path: &str, outcome: &str) {
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": null,
            "fingerprint": null,
            "transport": "app",
            "event": "appAction",
            "method": capped(method, METHOD_CHARS),
            "projectId": null,
            "outcome": capped(outcome, DETAIL_CHARS),
            "commandId": null,
            "mono": capped(mono, METHOD_CHARS),
            "path": capped(path, PATH_CHARS),
        }));
    }

    pub fn record_lifecycle(
        &self,
        kind: Lifecycle,
        device_id: &str,
        fingerprint: &str,
        transport: Transport,
        detail: Option<&str>,
    ) {
        let mut line = json!({
            "ts": now_ms(),
            "deviceId": capped(device_id, ID_CHARS),
            "fingerprint": prefix(fingerprint),
            "transport": transport.as_str(),
            "event": kind.as_str(),
            "method": null,
            "projectId": null,
            "outcome": "ok",
            "commandId": null,
        });
        if let Some(detail) = detail {
            line["detail"] = capped(detail, DETAIL_CHARS).into();
        }
        self.append(&line);
    }

    /// Never fails the caller: any error leaves the line unwritten.
    fn append(&self, line: &Value) {
        let Ok(_guard) = self.write.lock() else {
            return;
        };
        let _ = self.write_line(line);
    }

    fn write_line(&self, line: &Value) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let base = self.base();
        if fs::metadata(&base).is_ok_and(|m| m.len() >= self.max_bytes) {
            self.rotate();
        }
        let mut file = OpenOptions::new().create(true).append(true).open(base)?;
        let mut text = line.to_string();
        text.push('\n');
        file.write_all(text.as_bytes())
    }

    fn rotate(&self) {
        let _ = fs::remove_file(self.rotated(self.keep));
        for n in (1..self.keep).rev() {
            let _ = fs::rename(self.rotated(n), self.rotated(n + 1));
        }
        let _ = fs::rename(self.base(), self.rotated(1));
    }
}

static LOG: Mutex<Option<Arc<Log>>> = Mutex::new(None);

fn global() -> Option<Arc<Log>> {
    LOG.lock().ok().and_then(|log| log.clone())
}

/// Starts writing into `dir`. Until this runs, nothing is recorded.
pub fn init(dir: &Path) {
    if let Ok(mut log) = LOG.lock() {
        *log = Some(Arc::new(Log::new(dir)));
    }
}

/// Records `event` for `device` when it is worth keeping. A no-op before
/// [`init`].
pub fn record(device: &DeviceContext, event: &AuditEvent) {
    if let Some(log) = global() {
        log.record(device, event);
    }
}

/// Records a file read with the requested path, the only place a path is
/// logged, the outcome the read had and the device's fingerprint.
pub fn record_file_read_with(device: &DeviceContext, read: &FileRead) {
    if let Some(log) = global() {
        log.record_file_read(device, read);
    }
}

/// Records one `chat.mentionedFile` call. A no-op before [`init`].
pub fn record_mentioned_file_with(device: &DeviceContext, read: &MentionedFile) {
    if let Some(log) = global() {
        log.record_mentioned_file(device, read);
    }
}

/// Records one `file.prepare` call. A no-op before [`init`].
pub fn record_file_transfer_with(device: &DeviceContext, read: &FileTransfer) {
    if let Some(log) = global() {
        log.record_file_transfer(device, read);
    }
}

/// Records one `commands.run` call. A no-op before [`init`].
pub fn record_slash_command_with(device: &DeviceContext, run: &SlashCommand) {
    if let Some(log) = global() {
        log.record_slash_command(device, run);
    }
}

/// Records one remote approval answer. A no-op before [`init`].
pub fn record_approval_answer_with(device: &DeviceContext, answer: &ApprovalAnswer) {
    if let Some(log) = global() {
        log.record_approval_answer(device, answer);
    }
}

/// Records one web push attempt or drop. A no-op before [`init`].
pub fn record_web_push(category: &str, target: &str, devices: usize, outcome: &str) {
    if let Some(log) = global() {
        log.record_web_push(category, target, devices, outcome);
    }
}

/// Records an app action a Mono took (`projects.add`). A no-op before [`init`].
pub fn record_app_action(method: &str, mono: &str, path: &str, outcome: &str) {
    if let Some(log) = global() {
        log.record_app_action(method, mono, path, outcome);
    }
}

/// Records a moment in a device's life. A no-op before [`init`].
pub fn record_lifecycle(
    kind: Lifecycle,
    device_id: &str,
    fingerprint: &str,
    transport: Transport,
    detail: Option<&str>,
) {
    if let Some(log) = global() {
        log.record_lifecycle(kind, device_id, fingerprint, transport, detail);
    }
}

/// Holds the process-wide log on a test's directory; dropping it switches the
/// log off again, so later tests neither write into a deleted directory nor
/// recreate it.
#[cfg(any(test, feature = "test-support"))]
pub struct TestLog(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

#[cfg(any(test, feature = "test-support"))]
impl Drop for TestLog {
    fn drop(&mut self) {
        if let Ok(mut log) = LOG.lock() {
            *log = None;
        }
    }
}

/// Points the process-wide log at `dir` until the guard drops. Tests that read
/// the log back take turns here, so one cannot redirect another's lines.
/// Lines of tests that do not take a turn can still land in `dir`: filter by
/// the device's own fingerprint.
#[cfg(any(test, feature = "test-support"))]
pub fn test_log(dir: &Path) -> TestLog {
    static TURN: Mutex<()> = Mutex::new(());
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    init(dir);
    TestLog(turn)
}

/// The newest `limit` lines of the log in `dir`, newest first, across the
/// current and rotated files. Lines that do not parse are skipped.
pub fn read_recent(dir: &Path, limit: usize) -> Vec<Value> {
    let mut out = Vec::new();
    let files =
        std::iter::once(dir.join(FILE)).chain((1..=KEEP).map(|n| dir.join(format!("{FILE}.{n}"))));
    for file in files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines().rev() {
            if out.len() >= limit {
                return out;
            }
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                out.push(value);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceContext {
        DeviceContext {
            id: "dev-1".into(),
            kind: DeviceKind::Web,
            permission: policy::Permission::Control,
            projects: None,
            monos: None,
        }
    }

    fn event(method: &str, outcome: Outcome) -> AuditEvent {
        AuditEvent {
            fingerprint_prefix: "abcdef0123456789".into(),
            transport: Transport::Web,
            method: method.into(),
            project_id: Some("p1".into()),
            outcome,
            command_id: Some("c1".into()),
            writes: false,
            from_app: false,
        }
    }

    fn lines(dir: &Path) -> Vec<Value> {
        let mut all = read_recent(dir, 1000);
        all.reverse();
        all
    }

    #[test]
    fn an_app_action_writes_the_mono_and_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.record_app_action("projects.add", "Ada", "/work/app", "ok");
        let lines = lines(dir.path());
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "appAction");
        assert_eq!(lines[0]["method"], "projects.add");
        assert_eq!(lines[0]["mono"], "Ada");
        assert_eq!(lines[0]["path"], "/work/app");
        assert_eq!(lines[0]["outcome"], "ok");
        assert!(lines[0]["deviceId"].is_null());
    }

    #[test]
    fn a_web_push_writes_one_metadata_only_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.record_web_push("agentFinished", "habit", 2, "result=retry failed http 500");
        let lines = lines(dir.path());
        assert_eq!(lines.len(), 1);
        let line = lines[0].as_object().unwrap();
        let mut keys: Vec<&str> = line.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "category",
                "commandId",
                "deviceId",
                "devices",
                "event",
                "fingerprint",
                "method",
                "outcome",
                "projectId",
                "target",
                "transport",
                "ts"
            ]
        );
        assert_eq!(line["event"], "webPush");
        assert_eq!(line["category"], "agentFinished");
        assert_eq!(line["target"], "habit");
        assert_eq!(line["devices"], 2);
        assert_eq!(line["outcome"], "result=retry failed http 500");
    }

    #[test]
    fn a_control_call_writes_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.record(&device(), &event("conversation.send", Outcome::Ok));
        let lines = lines(dir.path());
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line["ts"].as_u64().unwrap() > 0);
        assert_eq!(line["deviceId"], "dev-1");
        assert_eq!(line["fingerprint"], "abcdef01");
        assert_eq!(line["transport"], "web");
        assert_eq!(line["event"], "call");
        assert_eq!(line["method"], "conversation.send");
        assert_eq!(line["projectId"], "p1");
        assert_eq!(line["outcome"], "ok");
        assert_eq!(line["commandId"], "c1");
    }

    #[test]
    fn every_remote_update_request_is_audited_and_a_status_read_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.record(&device(), &event("desktop.updateStatus", Outcome::Ok));
        assert!(lines(dir.path()).is_empty());
        log.record(&device(), &event("desktop.update", Outcome::Ok));
        log.record(
            &device(),
            &event("desktop.update", Outcome::Denied(Code::ReadOnly)),
        );
        let lines = lines(dir.path());
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["method"], "desktop.update");
        assert_eq!(lines[0]["outcome"], "ok");
        assert_eq!(lines[1]["outcome"], "denied:read_only");
    }

    #[test]
    fn a_denied_call_writes_its_code() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.record(
            &device(),
            &event("conversation.send", Outcome::Denied(Code::ReadOnly)),
        );
        // Even a read-class method is written once it was denied or failed.
        log.record(
            &device(),
            &event("project.files", Outcome::Denied(Code::NotSelected)),
        );
        log.record(&device(), &event("project.files", Outcome::Error));
        let lines = lines(dir.path());
        assert_eq!(lines[0]["outcome"], "denied:read_only");
        assert_eq!(lines[1]["outcome"], "denied:not_selected");
        assert_eq!(lines[2]["outcome"], "error");
    }

    #[test]
    fn a_successful_read_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        for method in ["project.files", "conversation.read", "accounts.usage"] {
            log.record(&device(), &event(method, Outcome::Ok));
        }
        assert!(lines(dir.path()).is_empty());
        // Parameter-dependent and unknown methods are written when in doubt.
        log.record(&device(), &event("device.selection", Outcome::Ok));
        log.record(&device(), &event("made.up", Outcome::Ok));
        assert_eq!(lines(dir.path()).len(), 2);
    }

    #[test]
    fn an_approval_answer_writes_one_line_with_ids_and_the_refusal_code() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        for (decision, outcome) in [
            (Some("allow"), Outcome::Ok),
            (Some("deny"), Outcome::Denied(Code::NotPending)),
            (Some("allow"), Outcome::Denied(Code::NeedsDesktop)),
        ] {
            log.record_approval_answer(
                &device(),
                &ApprovalAnswer {
                    fingerprint: "abcdef0123456789",
                    project_id: Some("p1"),
                    conversation_id: Some("conv-1"),
                    request_id: Some("ap-1"),
                    decision,
                    command_id: Some("c1"),
                    transport: Transport::Web,
                    outcome,
                },
            );
            // The host's own line for what the app wrote is dropped.
            let mut answered = event("session.answerApproval", outcome);
            answered.from_app = true;
            log.record(&device(), &answered);
        }
        let written = lines(dir.path());
        assert_eq!(written.len(), 3, "{written:?}");
        assert_eq!(written[0]["decision"], "allow");
        assert_eq!(written[0]["conversationId"], "conv-1");
        assert_eq!(written[0]["requestId"], "ap-1");
        assert_eq!(written[0]["projectId"], "p1");
        assert_eq!(written[1]["outcome"], "denied:not_pending");
        assert_eq!(written[1]["decision"], "deny");
        assert_eq!(written[2]["outcome"], "denied:needs_desktop");
        // An uncoded failure is still the host's line, without ids.
        let mut failed = event("session.answerApproval", Outcome::Error);
        failed.from_app = true;
        log.record(&device(), &failed);
        assert_eq!(lines(dir.path()).len(), 4);
    }

    #[test]
    fn a_file_read_carries_its_path_and_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.record_file_read(
            &device(),
            &FileRead {
                fingerprint: "abcdef0123456789",
                project_id: Some("p1"),
                path: "src/main.rs",
                command_id: Some("c9"),
                transport: Transport::Quic,
                outcome: Outcome::Denied(Code::PathDenied),
            },
        );
        let line = &lines(dir.path())[0];
        assert_eq!(line["method"], "project.file");
        assert_eq!(line["path"], "src/main.rs");
        assert_eq!(line["transport"], "quic");
        assert_eq!(line["outcome"], "denied:path_denied");
        assert_eq!(line["fingerprint"], "abcdef01");
    }

    #[test]
    fn a_mentioned_file_call_is_written_on_success_and_on_denial_with_its_fields() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        for (outcome, bytes) in [(Outcome::Ok, 42), (Outcome::Denied(Code::PathDenied), 0)] {
            log.record_mentioned_file(
                &device(),
                &MentionedFile {
                    fingerprint: "abcdef0123456789",
                    conversation_id: Some("mono:m1"),
                    path: "/work/report.html",
                    asset: Some("img/a.png"),
                    bytes,
                    rule: (bytes == 0).then_some("deny_list"),
                    command_id: Some("c9"),
                    transport: Transport::Web,
                    outcome,
                },
            );
        }
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0]["method"], "chat.mentionedFile");
        assert_eq!(all[0]["outcome"], "ok");
        assert_eq!(all[0]["bytes"], 42);
        assert_eq!(all[0]["conversationId"], "mono:m1");
        assert_eq!(all[0]["path"], "/work/report.html");
        assert_eq!(all[0]["asset"], "img/a.png");
        assert_eq!(all[0]["transport"], "web");
        assert_eq!(all[1]["outcome"], "denied:path_denied");
        assert_eq!(all[0]["rule"], Value::Null);
        assert_eq!(all[1]["rule"], "deny_list");
    }

    #[test]
    fn an_operator_run_is_audited_with_its_command_and_never_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        for outcome in [Outcome::Ok, Outcome::Denied(Code::ReadOnly)] {
            log.record_slash_command(
                &device(),
                &SlashCommand {
                    fingerprint: "abcdef0123456789",
                    conversation_id: Some("s1"),
                    command: "operator",
                    enabled: Some(true),
                    command_id: Some("c1"),
                    transport: Transport::Web,
                    outcome,
                },
            );
        }
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0]["method"], "commands.run");
        assert_eq!(all[0]["command"], "operator");
        assert_eq!(all[0]["enabled"], true);
        assert_eq!(all[0]["conversationId"], "s1");
        assert_eq!(all[0]["transport"], "web");
        assert_eq!(all[1]["outcome"], "denied:read_only");
        assert!(all[0].get("text").is_none());
        // The host's own line for a call the app already wrote is dropped.
        let mut answered = event("commands.run", Outcome::Ok);
        answered.from_app = true;
        log.record(&device(), &answered);
        assert_eq!(lines(dir.path()).len(), 2);
    }

    #[test]
    fn a_mentioned_file_call_writes_one_line_not_two() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        // The host's line for what the app already wrote is dropped, whatever the code.
        for outcome in [
            Outcome::Ok,
            Outcome::Denied(Code::Forbidden),
            Outcome::Denied(Code::Busy),
            Outcome::Denied(Code::NotFound),
            Outcome::Error,
        ] {
            let mut answered = event("chat.mentionedFile", outcome);
            answered.from_app = true;
            log.record(&device(), &answered);
        }
        assert!(lines(dir.path()).is_empty());
        // A refusal or failure from the core before the app (read-only is not one: it is a
        // read) still writes: the app never saw the call.
        log.record(&device(), &event("chat.mentionedFile", Outcome::Error));
        log.record(
            &device(),
            &event("chat.mentionedFile", Outcome::Denied(Code::NotSelected)),
        );
        assert_eq!(lines(dir.path()).len(), 2);
    }

    #[test]
    fn a_file_prepare_call_is_audited_with_the_outside_projects_mark_and_never_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        for (outside, outcome, rule) in [
            (true, Outcome::Ok, None),
            (false, Outcome::Denied(Code::PathDenied), Some("deny_list")),
        ] {
            log.record_file_transfer(
                &device(),
                &FileTransfer {
                    fingerprint: "abcdef0123456789",
                    conversation_id: Some("s1"),
                    path: "/tmp/report.txt",
                    asset: None,
                    bytes: 11,
                    outside_projects: outside,
                    reused: false,
                    rule,
                    command_id: Some("c1"),
                    transport: Transport::Web,
                    outcome,
                },
            );
        }
        let all = lines(dir.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0]["method"], "file.prepare");
        assert_eq!(all[0]["outsideProjects"], true);
        assert_eq!(all[0]["path"], "/tmp/report.txt");
        assert_eq!(all[0]["bytes"], 11);
        assert_eq!(all[0]["permission"], "control");
        assert_eq!(all[1]["outsideProjects"], false);
        assert_eq!(all[1]["outcome"], "denied:path_denied");
        assert_eq!(all[1]["rule"], "deny_list");
        // Nothing but metadata: no key, no object name, no content field.
        let text = std::fs::read_to_string(dir.path().join("mobile-audit.jsonl")).unwrap();
        for field in ["\"key\"", "\"nonce\"", "\"object\"", "\"data\"", "\"hash\""] {
            assert!(!text.contains(field), "{field}");
        }
    }

    #[test]
    fn a_file_prepare_call_writes_one_line_not_two() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        for outcome in [
            Outcome::Ok,
            Outcome::Denied(Code::Forbidden),
            Outcome::Denied(Code::PathDenied),
            Outcome::Error,
        ] {
            let mut answered = event("file.prepare", outcome);
            answered.from_app = true;
            log.record(&device(), &answered);
        }
        assert!(lines(dir.path()).is_empty());
        // A refusal from the core before the app (a read-only wire error cannot happen: it is a read).
        log.record(
            &device(),
            &event("file.prepare", Outcome::Denied(Code::NotSelected)),
        );
        assert_eq!(lines(dir.path()).len(), 1);
    }

    #[test]
    fn a_file_read_writes_one_line_not_two() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        // The host's line for what the app already wrote is dropped.
        for outcome in [
            Outcome::Ok,
            Outcome::Denied(Code::PathDenied),
            Outcome::Denied(Code::NotSelected),
        ] {
            let mut answered = event("project.file", outcome);
            answered.from_app = true;
            log.record(&device(), &answered);
        }
        assert!(lines(dir.path()).is_empty());
        // A refusal the core made before the app (the request never reached
        // it, so nothing else writes a line), and a failure, still write.
        log.record(
            &device(),
            &event("project.file", Outcome::Denied(Code::NotSelected)),
        );
        assert_eq!(lines(dir.path()).len(), 1);
        assert_eq!(lines(dir.path())[0]["outcome"], "denied:not_selected");
        let mut revoked = event("project.file", Outcome::Denied(Code::Revoked));
        revoked.from_app = true;
        log.record(&device(), &revoked);
        log.record(&device(), &event("project.file", Outcome::Error));
        assert_eq!(lines(dir.path()).len(), 3);
    }

    #[test]
    fn a_mono_file_save_is_written_and_a_read_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        let call = |params: Value| {
            let request = json!({"id": "c", "params": params});
            AuditEvent::for_call("abcdef0123456789", Transport::Quic, "mono.file", &request)
        };
        log.record(&device(), &call(json!({"monoId": "m", "file": "soul"})));
        assert!(lines(dir.path()).is_empty());
        log.record(
            &device(),
            &call(json!({"monoId": "m", "file": "soul", "text": "x"})),
        );
        let written = lines(dir.path());
        assert_eq!(written.len(), 1);
        assert_eq!(written[0]["method"], "mono.file");
        assert_eq!(written[0]["outcome"], "ok");
        assert!(!fs::read_to_string(dir.path().join(FILE))
            .unwrap()
            .contains("\"x\""));
    }

    #[test]
    fn device_chosen_text_is_capped_and_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        let long = "\u{e9}x".repeat(4_000_000);
        let mut flood = event(&long, Outcome::Denied(Code::Forbidden));
        flood.project_id = Some(long.clone());
        flood.command_id = Some(long.clone());
        log.record(&device(), &flood);
        log.record_file_read(
            &device(),
            &FileRead {
                fingerprint: "abcdef0123456789",
                project_id: Some(&long),
                path: &"a/\n\u{0}b".repeat(10_000),
                command_id: Some(&long),
                transport: Transport::Web,
                outcome: Outcome::Ok,
            },
        );
        log.record_lifecycle(
            Lifecycle::PermissionChange,
            &long,
            "ab",
            Transport::Web,
            Some(&long),
        );
        let text = fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(text.len() < 8 * 1024, "{} bytes", text.len());
        let all = lines(dir.path());
        assert_eq!(all.len(), 3);
        // An unknown method is "unknown", with only its capped name beside it.
        assert_eq!(all[0]["method"], "unknown");
        assert_eq!(all[0]["detail"].as_str().unwrap().chars().count(), 64);
        assert_eq!(all[0]["projectId"].as_str().unwrap().chars().count(), 128);
        assert_eq!(all[1]["path"].as_str().unwrap().chars().count(), 1024);
        assert!(!all[1]["path"].as_str().unwrap().contains('\n'));
        assert_eq!(all[2]["detail"].as_str().unwrap().chars().count(), 256);
    }

    #[test]
    fn each_lifecycle_kind_writes_its_event() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        let kinds = [
            (Lifecycle::Enroll, "enroll"),
            (Lifecycle::Pending, "pending"),
            (Lifecycle::Confirm, "confirm"),
            (Lifecycle::Deny, "deny"),
            (Lifecycle::Revoke, "revoke"),
            (Lifecycle::Connect, "connect"),
            (Lifecycle::Disconnect, "disconnect"),
            (Lifecycle::PermissionChange, "permission"),
            (Lifecycle::SelectionWiden, "selection_widen"),
            (Lifecycle::SelectionNarrow, "selection_narrow"),
            (Lifecycle::SelectionWidenRefused, "selection_widen_refused"),
        ];
        for (kind, _) in kinds {
            log.record_lifecycle(kind, "dev-1", "abcdef0123456789", Transport::Web, None);
        }
        let lines = lines(dir.path());
        assert_eq!(lines.len(), kinds.len());
        for (line, (_, name)) in lines.iter().zip(kinds) {
            assert_eq!(line["event"], name);
            assert_eq!(line["deviceId"], "dev-1");
            assert_eq!(line["fingerprint"], "abcdef01");
            assert!(line["method"].is_null());
        }
        log.record_lifecycle(
            Lifecycle::PermissionChange,
            "dev-1",
            "ab",
            Transport::Web,
            Some("control to read"),
        );
        assert_eq!(read_recent(dir.path(), 1)[0]["detail"], "control to read");
    }

    #[test]
    fn rotation_keeps_five_files_and_drops_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::with_limits(dir.path(), 200, KEEP);
        for i in 0..200 {
            let mut e = event("conversation.send", Outcome::Ok);
            e.command_id = Some(format!("c{i}"));
            log.record(&device(), &e);
        }
        for n in 1..=KEEP {
            assert!(dir.path().join(format!("{FILE}.{n}")).exists(), "{n}");
        }
        assert!(!dir.path().join(format!("{FILE}.{}", KEEP + 1)).exists());
        assert!(dir.path().join(FILE).exists());
        // Newest first across files, and the newest line is the last written.
        let recent = read_recent(dir.path(), 5);
        assert_eq!(recent[0]["commandId"], "c199");
        assert_eq!(recent[1]["commandId"], "c198");
    }

    #[test]
    fn params_and_content_never_reach_the_file() {
        // The event type has no field for them; check the written bytes too,
        // with the markers where a careless caller would put them.
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        // The event is built by the function the host builds every call's
        // event with, from a request whose parameters hold the markers.
        let request = json!({
            "version": 1,
            "id": "c1",
            "method": "conversation.send",
            "params": {
                "projectId": "p1",
                "text": "SECRET-MARKER-123",
                "prompt": "prompt: please do the thing",
                "nested": {"content": "SECRET-MARKER-123"},
            },
        });
        let e = AuditEvent::for_call(
            "abcdef0123456789",
            Transport::Web,
            "conversation.send",
            &request,
        );
        assert_eq!(e.project_id.as_deref(), Some("p1"));
        log.record(&device(), &e);
        log.record_lifecycle(
            Lifecycle::Connect,
            "dev-1",
            "abcdef01",
            Transport::Web,
            None,
        );
        let text = fs::read_to_string(dir.path().join(FILE)).unwrap();
        assert!(text.contains("conversation.send"));
        for marker in ["SECRET-MARKER-123", "prompt: please do the thing"] {
            assert!(!text.contains(marker));
        }
        // Only these keys exist on a call line.
        let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        let mut keys: Vec<_> = line.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "commandId",
                "deviceId",
                "event",
                "fingerprint",
                "method",
                "outcome",
                "projectId",
                "transport",
                "ts"
            ]
        );
    }

    #[test]
    fn a_failing_write_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        fs::write(&blocker, "x").unwrap();
        // The log dir is under a regular file, so creating it fails.
        let log = Log::new(&blocker.join("sub"));
        log.record(&device(), &event("conversation.send", Outcome::Ok));
    }
}
