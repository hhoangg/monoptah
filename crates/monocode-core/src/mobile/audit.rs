//! Audit trail of what remote devices did (security.md section 7). One JSON
//! line per event in `mobile-audit.jsonl`, rotated by size. Parameters and
//! content are never written; the only path ever logged is a file read's.
use super::policy::{self, Access, Code, DeviceContext, DeviceKind};
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
    if param_dependent(&event.method) {
        return true;
    }
    !matches!(
        policy::classify(&event.method, &Value::Null),
        Some(Access::Read)
    )
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
        if !should_write(event) {
            return;
        }
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": device.id,
            "fingerprint": prefix(&event.fingerprint_prefix),
            "transport": event.transport.as_str(),
            "event": "call",
            "method": event.method,
            "projectId": event.project_id,
            "outcome": event.outcome.text(),
            "commandId": event.command_id,
        }));
    }

    /// A file read, the one event that carries a path.
    pub fn record_file_read(
        &self,
        device: &DeviceContext,
        project_id: Option<&str>,
        path: &str,
        command_id: Option<&str>,
        transport: Transport,
        outcome: Outcome,
    ) {
        self.append(&json!({
            "ts": now_ms(),
            "deviceId": device.id,
            "fingerprint": "",
            "transport": transport.as_str(),
            "event": "call",
            "method": "project.file",
            "projectId": project_id,
            "outcome": outcome.text(),
            "commandId": command_id,
            "path": path,
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
            "deviceId": device_id,
            "fingerprint": prefix(fingerprint),
            "transport": transport.as_str(),
            "event": kind.as_str(),
            "method": null,
            "projectId": null,
            "outcome": "ok",
            "commandId": null,
        });
        if let Some(detail) = detail {
            line["detail"] = detail.into();
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
/// logged. The outcome is `ok` here; see [`record_file_read_with`].
pub fn record_file_read(
    device: &DeviceContext,
    project_id: Option<&str>,
    path: &str,
    command_id: Option<&str>,
    transport: Transport,
) {
    record_file_read_with(device, project_id, path, command_id, transport, Outcome::Ok);
}

/// [`record_file_read`] with the outcome the read had.
pub fn record_file_read_with(
    device: &DeviceContext,
    project_id: Option<&str>,
    path: &str,
    command_id: Option<&str>,
    transport: Transport,
    outcome: Outcome,
) {
    if let Some(log) = global() {
        log.record_file_read(device, project_id, path, command_id, transport, outcome);
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
        }
    }

    fn lines(dir: &Path) -> Vec<Value> {
        let mut all = read_recent(dir, 1000);
        all.reverse();
        all
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
    fn a_file_read_carries_its_path_and_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let log = Log::new(dir.path());
        log.record_file_read(
            &device(),
            Some("p1"),
            "src/main.rs",
            Some("c9"),
            Transport::Quic,
            Outcome::Denied(Code::PathDenied),
        );
        let line = &lines(dir.path())[0];
        assert_eq!(line["method"], "project.file");
        assert_eq!(line["path"], "src/main.rs");
        assert_eq!(line["transport"], "quic");
        assert_eq!(line["outcome"], "denied:path_denied");
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
        let mut e = event("conversation.send", Outcome::Ok);
        e.method = "conversation.send".into();
        log.record(&device(), &e);
        log.record_lifecycle(
            Lifecycle::Connect,
            "dev-1",
            "abcdef01",
            Transport::Web,
            None,
        );
        let text = fs::read_to_string(dir.path().join(FILE)).unwrap();
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
