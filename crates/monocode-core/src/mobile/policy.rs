//! Who may call what: the method permission table as data (architecture.md
//! 7.1), the device context every request carries, and the enforcement of
//! permission, desktop toggles and project/Mono selection. A paired QUIC phone
//! (control, no selection) keeps today's answers, except for the deliberate
//! tightenings: full-access is refused for phones too (H-03), and approvals
//! stay off until the U-13 review gate passes (H-08).
use super::cloud_policy;
use super::host::Device;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::RwLock;

/// The kind of device behind a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Phone,
    Web,
}

/// What a device may do: `Read` devices get `read_only` for control methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Read,
    Control,
}

/// The identity, permission and selection of the device behind a request.
/// `None` for `projects` or `monos` means all of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceContext {
    pub id: String,
    pub kind: DeviceKind,
    pub permission: Permission,
    pub projects: Option<Vec<String>>,
    pub monos: Option<Vec<String>>,
}

impl DeviceContext {
    /// A paired QUIC phone, as the host treats it today: it may control, and
    /// sees every project and Mono.
    pub fn phone(device: &Device) -> Self {
        Self {
            id: device.id.clone(),
            kind: DeviceKind::Phone,
            permission: Permission::Control,
            projects: None,
            monos: None,
        }
    }
}

/// Desktop toggles that gate parts of the policy. All default to off, except
/// `show_push_content`, which only picks what a web notification says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PolicyFlags {
    /// "Remote push in Git".
    pub remote_git_push: bool,
    /// "Remote cloud start".
    pub remote_cloud_start: bool,
    /// "Answer approvals from remote devices" (H-08). Effective together with
    /// [`REMOTE_APPROVALS_ENABLED`].
    pub remote_approvals: bool,
    /// "Show message content in notifications": web pushes carry the session
    /// or Mono name and a short preview. Off sends the generic text. Default on.
    pub show_push_content: bool,
}

impl Default for PolicyFlags {
    fn default() -> Self {
        Self {
            remote_git_push: false,
            remote_cloud_start: false,
            remote_approvals: false,
            show_push_content: true,
        }
    }
}

/// The file name of the saved policy settings, next to the mobile config.
pub const SETTINGS_FILE: &str = "mobile-policy.json";

/// Whether remote devices may answer tool approvals. The U-13 review gate
/// passed on 2026-11-18 (H-08), so this is on. The desktop's own toggle
/// (`PolicyFlags::remote_approvals`, default off) must be on as well: see
/// [`approvals_effective`].
pub const REMOTE_APPROVALS_ENABLED: bool = true;

/// Whether remote approvals are on: the U-13 `gate` AND the desktop toggle.
pub fn approvals_effective(flags: &PolicyFlags, gate: bool) -> bool {
    gate && flags.remote_approvals
}

/// [`approvals_effective`] for the process-wide toggles and the real gate.
pub fn approvals_enabled() -> bool {
    approvals_effective(&flags(), REMOTE_APPROVALS_ENABLED)
}

/// The desktop's policy settings: the toggles and, per project path, the
/// extra deny patterns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PolicySettings {
    #[serde(flatten)]
    pub flags: PolicyFlags,
    /// Extra deny patterns by project path.
    pub deny_patterns: BTreeMap<String, Vec<String>>,
    /// The ids of the web devices (rows of this desktop's own list) the user
    /// allowed to open a shell here. Empty by default, written only by the
    /// desktop's own settings page: nothing the web server sends reaches it.
    /// Left out of the file while empty, so an earlier file is unchanged.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub terminal_devices: BTreeSet<String>,
}

static SETTINGS: RwLock<PolicySettings> = RwLock::new(PolicySettings {
    flags: PolicyFlags {
        remote_git_push: false,
        remote_cloud_start: false,
        remote_approvals: false,
        show_push_content: true,
    },
    deny_patterns: BTreeMap::new(),
    terminal_devices: BTreeSet::new(),
});

/// The saved settings at `path`. A missing, unreadable or invalid file gives
/// the defaults.
pub fn load_settings(path: &Path) -> PolicySettings {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Saves `settings` to `path`: written to a temporary file, then renamed.
pub fn save_settings(path: &Path, settings: &PolicySettings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    // A name of its own per call, so two saves never share a temporary file.
    static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let temp = std::path::PathBuf::from(temp);
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

/// Writes the process-wide settings to `path`. One save runs at a time and
/// each writes the settings as they are when it runs, so the last save always
/// holds the latest state.
pub fn persist_settings(path: &Path) -> Result<(), String> {
    static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
    save_settings(path, &settings()).or_else(|_| save_settings(path, &settings()))
}

/// Makes `settings` the process-wide policy settings.
pub fn install_settings(settings: PolicySettings) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = settings;
}

/// The process-wide policy settings.
pub fn settings() -> PolicySettings {
    SETTINGS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The process-wide toggles.
pub fn flags() -> PolicyFlags {
    settings().flags
}

/// Tests that move the process-wide settings or toggles hold this, so they run one at a time.
#[cfg(test)]
pub(crate) static TEST_SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Replaces the process-wide toggles, keeping the deny patterns.
pub fn set_flags(flags: PolicyFlags) {
    SETTINGS.write().unwrap_or_else(|e| e.into_inner()).flags = flags;
}

/// Whether the user allowed the web device `device_id` to open a shell on
/// this desktop ("Allow web terminal", default off, per device).
pub fn terminal_allowed(device_id: &str) -> bool {
    SETTINGS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .terminal_devices
        .contains(device_id)
}

/// Sets the "Allow web terminal" toggle of one device in the process-wide
/// settings. The caller saves the file. Only the desktop's own settings code
/// calls this: there is no wire path to it.
pub fn set_terminal_allowed(device_id: &str, on: bool) {
    let mut settings = SETTINGS.write().unwrap_or_else(|e| e.into_inner());
    if on {
        settings.terminal_devices.insert(device_id.to_owned());
    } else {
        settings.terminal_devices.remove(device_id);
    }
}

/// Declares `Code` with its wire names and the list of all variants in one
/// place, so a new variant cannot be left out of either.
macro_rules! codes {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// Stable error keys for the web client (architecture.md 5.5). The error
        /// text the QUIC transport sends is unchanged.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Code {
            $($variant),*
        }

        impl Code {
            /// Every code.
            pub const ALL: &'static [Code] = &[$(Code::$variant),*];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Code::$variant => $name),*
                }
            }
        }
    };
}

codes! {
    ReadOnly => "read_only",
    Forbidden => "forbidden",
    NotSelected => "not_selected",
    PathDenied => "path_denied",
    NotFound => "not_found",
    Revoked => "revoked",
    Disabled => "disabled",
    TooLarge => "too_large",
    Unsupported => "unsupported",
    UnsupportedTarget => "unsupported_target",
    Busy => "busy",
    Starting => "starting",
    AutoStopping => "auto_stopping",
    InstanceBusy => "instance_busy",
    NotPending => "not_pending",
    NeedsDesktop => "needs_desktop",
    WakeRequired => "wake_required",
    FreshRequired => "fresh_required",
    ReauthRequired => "reauth_required",
    ReauthFailed => "reauth_failed",
}

impl Code {
    /// The failure of this code: its canonical text and the code itself.
    pub fn failure(self) -> Failure {
        Denial::new(self).into()
    }

    /// The one fixed error text of this code. The QUIC transport sends it
    /// and the web channel carries the code itself in the failure.
    pub fn message(self) -> &'static str {
        match self {
            Code::ReadOnly => "This device can only read on this desktop",
            Code::Forbidden => "Not allowed from a remote device",
            Code::NotSelected => "Not in this device's project selection",
            Code::PathDenied => "Hidden: sensitive file",
            Code::NotFound => "File not found",
            Code::Revoked => "Device revoked",
            Code::Disabled => "Turned off on the desktop",
            Code::TooLarge => "Too large for a remote device",
            Code::Unsupported => "Unsupported mobile operation",
            Code::UnsupportedTarget => "Not available for this session",
            Code::Busy => "Desktop is busy; try again",
            Code::Starting => "The cloud instance is still starting",
            Code::AutoStopping => "The cloud instance is stopping on its own",
            Code::InstanceBusy => "The cloud instance is busy with another operation",
            Code::NotPending => "Approval is no longer pending",
            Code::NeedsDesktop => "Answer this request on the desktop",
            Code::WakeRequired => "Sending wakes the stopped cloud instance; confirm to continue",
            Code::FreshRequired => "Confirm with your passkey again to add projects",
            Code::ReauthRequired => "Ask for a new code on the desktop",
            Code::ReauthFailed => "That code is not right",
        }
    }
}

/// The code a failure carries, if it carries a known one.
pub fn code_of_failure(failure: &Failure) -> Option<Code> {
    let wanted = failure.code.as_deref()?;
    Code::ALL
        .iter()
        .copied()
        .find(|code| code.as_str() == wanted)
}

/// The code whose canonical text is exactly `text`.
pub fn code_of_error(text: &str) -> Option<Code> {
    Code::ALL
        .iter()
        .copied()
        .find(|code| code.message() == text)
}

/// A refused call: the stable key and the text the transport reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denial {
    pub code: Code,
    pub error: String,
}

/// The refusal that carries a code to the web device, with its text.
pub use monocode_mobile_link::Failure;

impl From<Denial> for Failure {
    fn from(denial: Denial) -> Self {
        Self::coded(denial.code.as_str(), denial.error)
    }
}

impl Denial {
    /// A denial carrying the canonical text of `code`.
    pub fn new(code: Code) -> Self {
        Self {
            code,
            error: code.message().into(),
        }
    }
}

/// The class of a method in the table. Mirrors `web/protocol/methods.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Read,
    Control,
    /// A shell on the desktop: web devices only, Control, and the desktop's
    /// own per-device toggle. Never implied by `Control`.
    Terminal,
    /// The class depends on the parameters: see [`classify`].
    Param,
    /// Denied for web devices in v1 (or never exposed).
    Denied,
    /// Answered `unsupported` on the web transport.
    UnsupportedWeb,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Read => "read",
            Class::Control => "control",
            Class::Terminal => "terminal",
            Class::Param => "param",
            Class::Denied => "denied",
            Class::UnsupportedWeb => "unsupportedWeb",
        }
    }
}

/// Who answers a method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// The app, through [`super::host::Event::Request`].
    App,
    /// The host itself, inside `MobileHost::handle` or `watch`.
    Host,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::App => "app",
            Transport::Host => "host",
        }
    }
}

/// One row of the permission table.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub name: &'static str,
    pub class: Class,
    pub transport: Transport,
}

const fn app(name: &'static str, class: Class) -> Entry {
    Entry {
        name,
        class,
        transport: Transport::App,
    }
}
const fn host(name: &'static str, class: Class) -> Entry {
    Entry {
        name,
        class,
        transport: Transport::Host,
    }
}

/// Every method a remote device may name, with its class (architecture.md
/// 7.1). A new method must be added here: the classification test fails
/// otherwise.
pub const METHODS: &[Entry] = &[
    app("conversation.read", Class::Read),
    app("conversation.older", Class::Read),
    app("models.list", Class::Read),
    app("accounts.usage", Class::Param),
    app("session.configure", Class::Control),
    app("project.changes", Class::Read),
    app("project.worktrees", Class::Read),
    app("project.files", Class::Read),
    app("project.file", Class::Read),
    app("project.search", Class::Read),
    app("project.fileHighlight", Class::UnsupportedWeb),
    app("chat.mentionedFile", Class::Read),
    app("file.prepare", Class::Read),
    app("project.git", Class::Read),
    app("project.gitAction", Class::Param),
    app("session.start", Class::Param),
    app("conversation.send", Class::Control),
    app("session.answerApproval", Class::Control),
    app("session.resumeUsage", Class::Control),
    app("session.resumeUsageAtReset", Class::Control),
    app("session.answerQuestion", Class::Control),
    app("session.keepQuestionOpen", Class::Control),
    app("session.cancel", Class::Control),
    app("session.rename", Class::Control),
    app("session.delete", Class::Control),
    app("session.archive", Class::Control),
    app("folder.create", Class::Control),
    app("folder.rename", Class::Control),
    app("folder.move", Class::Control),
    app("folder.delete", Class::Control),
    app("mono.document", Class::Read),
    app("mono.profile", Class::Read),
    app("mono.update", Class::Control),
    app("mono.reset", Class::Denied),
    app("mono.delete", Class::Denied),
    app("mono.create", Class::Control),
    app("mono.file", Class::Read),
    app("mono.saveFile", Class::Denied),
    app("cloud.instances", Class::Read),
    app("cloud.startOptions", Class::Read),
    app("cloud.stop", Class::Control),
    app("cloud.resume", Class::Control),
    app("cloud.retryStart", Class::Control),
    app("cloud.keepRunning", Class::Control),
    app("cloud.moveToCloud", Class::Control),
    app("cloud.moveToLocal", Class::Control),
    app("cloud.cancelMove", Class::Control),
    app("desktop.updateStatus", Class::Read),
    app("desktop.update", Class::Control),
    host("device.enroll", Class::Denied),
    host("device.leave", Class::Read),
    host("workspace.list", Class::Read),
    host("sessions.list", Class::Read),
    host("device.push", Class::UnsupportedWeb),
    host("device.selection", Class::Param),
    host("conversation.watch", Class::Read),
    host("terminal.open", Class::Terminal),
    host("terminal.read", Class::Terminal),
    host("terminal.write", Class::Terminal),
    host("terminal.resize", Class::Terminal),
    host("terminal.close", Class::Terminal),
];

/// A desktop toggle a method waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    RemoteCloudStart,
    RemoteGitPush,
}

/// What a call needs, once its parameters are known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Control,
    /// Control, and every listed toggle must be on (web devices).
    ControlWith(Vec<Toggle>),
    /// A shell: a web device with Control and the desktop's per-device toggle.
    Terminal,
    /// Denied for web devices in v1.
    DeniedForWeb,
    /// Denied for web devices unless the toggle is on.
    DeniedForWebUnless(Toggle),
    /// Denied for every remote device, phones included.
    DeniedForRemote,
    /// Answered `unsupported` on the web transport.
    UnsupportedWeb,
}

/// The table row of `method`, if it has one.
pub fn entry(method: &str) -> Option<&'static Entry> {
    METHODS.iter().find(|entry| entry.name == method)
}

fn full_access(params: &Value) -> bool {
    params["runtimeMode"] == "full-access"
}

/// What `method` called with `params` needs, or `None` for an unknown method.
/// Parameter-dependent rules follow architecture.md 7.1.
pub fn classify(method: &str, params: &Value) -> Option<Access> {
    let entry = entry(method)?;
    // `mono.file` saves when it carries `text` (or a save hash), so such a call
    // is exactly `mono.saveFile`, whatever the table class says for a read.
    if method == "mono.file" && (!params["text"].is_null() || !params["expectedHash"].is_null()) {
        return Some(Access::DeniedForWeb);
    }
    Some(match entry.class {
        Class::Read => Access::Read,
        Class::Denied => Access::DeniedForWeb,
        Class::Terminal => Access::Terminal,
        Class::UnsupportedWeb => Access::UnsupportedWeb,
        Class::Control => match method {
            "session.configure" if full_access(params) => Access::DeniedForRemote,
            "cloud.moveToCloud" | "cloud.moveToLocal" | "cloud.cancelMove" => {
                Access::ControlWith(vec![Toggle::RemoteCloudStart, Toggle::RemoteGitPush])
            }
            _ => Access::Control,
        },
        Class::Param => match method {
            "accounts.usage" if params["refresh"] == true => Access::Control,
            "accounts.usage" => Access::Read,
            // The host's selection handler enforces who may narrow and who may
            // widen (it knows the device kind and the change), so the call
            // itself is a read for every parameter.
            "device.selection" => Access::Read,
            "session.start" if full_access(params) => Access::DeniedForRemote,
            "session.start" if !params["target"].is_null() => {
                Access::ControlWith(vec![Toggle::RemoteCloudStart])
            }
            "session.start" => Access::Control,
            "project.gitAction" => match params["action"].as_str() {
                Some("sync" | "pr") => Access::DeniedForWebUnless(Toggle::RemoteGitPush),
                Some("commit") if params["push"] == true || params["createPr"] == true => {
                    Access::DeniedForWebUnless(Toggle::RemoteGitPush)
                }
                Some("commit" | "stage" | "unstage" | "stageAll" | "unstageAll" | "generate") => {
                    Access::Control
                }
                // A missing or unknown action is refused, not assumed safe.
                _ => Access::DeniedForWeb,
            },
            _ => Access::Control,
        },
    })
}

fn toggle_on(toggle: Toggle, flags: &PolicyFlags) -> bool {
    match toggle {
        Toggle::RemoteCloudStart => flags.remote_cloud_start,
        Toggle::RemoteGitPush => flags.remote_git_push,
    }
}

fn require_control(device: &DeviceContext) -> Result<(), Denial> {
    if device.permission == Permission::Read {
        return Err(Denial::new(Code::ReadOnly));
    }
    Ok(())
}

fn listed(selection: &Option<Vec<String>>, id: &str) -> bool {
    selection
        .as_ref()
        .is_none_or(|list| list.iter().any(|item| item == id))
}

/// Refuses ids the device did not select. `None` selections mean all.
fn check_selection(device: &DeviceContext, params: &Value) -> Result<(), Denial> {
    let outside = |ok: bool| {
        if ok {
            Ok(())
        } else {
            Err(Denial::new(Code::NotSelected))
        }
    };
    if let Some(project) = params["projectId"].as_str() {
        outside(listed(&device.projects, project))?;
    }
    if let Some(mono) = params["monoId"].as_str() {
        outside(listed(&device.monos, mono))?;
    }
    if let Some(mono) = params["conversationId"]
        .as_str()
        .and_then(|id| id.strip_prefix("mono:"))
    {
        outside(listed(&device.monos, mono))?;
    }
    Ok(())
}

/// Checks a call before it runs, against the process-wide toggles.
pub fn authorize(device: &DeviceContext, method: &str, params: &Value) -> Result<(), Denial> {
    authorize_full(
        device,
        method,
        params,
        &flags(),
        REMOTE_APPROVALS_ENABLED,
        terminal_allowed(&device.id),
    )
}

/// [`authorize`] with the toggles given. The device's terminal toggle counts
/// as off. Order: permission, forbidden, disabled and unsupported first, then
/// the selection.
pub fn authorize_with(
    device: &DeviceContext,
    method: &str,
    params: &Value,
    flags: &PolicyFlags,
    approvals_gate: bool,
) -> Result<(), Denial> {
    authorize_full(device, method, params, flags, approvals_gate, false)
}

/// [`authorize_with`] with the device's "Allow web terminal" toggle given.
pub fn authorize_full(
    device: &DeviceContext,
    method: &str,
    params: &Value,
    flags: &PolicyFlags,
    approvals_gate: bool,
    terminal_on: bool,
) -> Result<(), Denial> {
    // Fail closed: a method with no class is unsupported, whoever answers it.
    let Some(access) = classify(method, params) else {
        return Err(Denial::new(Code::Unsupported));
    };
    let web = device.kind == DeviceKind::Web;
    match access {
        Access::Read => {}
        Access::Control => require_control(device)?,
        Access::Terminal => {
            // Only a browser opens a shell, and only with Control: a
            // read-only device never gets one, whatever the toggle says.
            if !web {
                return Err(Denial::new(Code::Forbidden));
            }
            require_control(device)?;
            if !terminal_on {
                return Err(Denial::new(Code::Disabled));
            }
        }
        Access::ControlWith(toggles) => {
            require_control(device)?;
            if toggles.iter().any(|toggle| !toggle_on(*toggle, flags)) {
                return Err(Denial::new(Code::Disabled));
            }
        }
        Access::DeniedForWeb => {
            if web {
                return Err(Denial::new(Code::Forbidden));
            }
            require_control(device)?;
        }
        Access::DeniedForWebUnless(toggle) => {
            if web && !toggle_on(toggle, flags) {
                return Err(Denial::new(Code::Forbidden));
            }
            require_control(device)?;
        }
        Access::DeniedForRemote => return Err(Denial::new(Code::Forbidden)),
        Access::UnsupportedWeb => {
            if web {
                return Err(Denial::new(Code::Unsupported));
            }
        }
    }
    if method == "session.answerApproval" && !approvals_effective(flags, approvals_gate) {
        return Err(Denial::new(Code::Disabled));
    }
    check_selection(device, params)
}

/// Shapes a catalog reply (`workspace.list`, `sessions.list`) for `device`:
/// unselected projects and Monos, and what hangs off them, are removed. A
/// device with no selection gets the reply unchanged apart from the cloud
/// scope.
pub fn filter_reply(device: &DeviceContext, method: &str, reply: &mut Value) {
    if matches!(method, "workspace.list" | "sessions.list") {
        narrow_catalog(device, reply);
    }
    cloud_policy::scope_catalog(device, reply);
    if method == "workspace.list" {
        super::folders::scope(reply);
    }
}

fn narrow_catalog(device: &DeviceContext, reply: &mut Value) {
    let selected = |list: &Option<Vec<String>>, id: Option<&str>| match (list, id) {
        (None, _) => true,
        (Some(list), Some(id)) => list.iter().any(|v| v == id),
        (Some(_), None) => false,
    };
    if device.projects.is_some() {
        if let Some(list) = reply.get_mut("projects").and_then(Value::as_array_mut) {
            list.retain(|p| selected(&device.projects, p["id"].as_str()));
        }
    }
    if device.projects.is_some() {
        if let Some(list) = reply.get_mut("folders").and_then(Value::as_array_mut) {
            list.retain(|f| selected(&device.projects, f["projectId"].as_str()));
        }
    }
    // Mono sessions that belong to an unselected Mono.
    let mut hidden_sessions: Vec<String> = Vec::new();
    if let Some(list) = reply.get_mut("monos").and_then(Value::as_array_mut) {
        list.retain(|m| {
            let keep = selected(&device.monos, m["id"].as_str());
            if !keep {
                if let Some(session) = m["sessionId"].as_str() {
                    hidden_sessions.push(session.to_owned());
                }
            }
            keep
        });
        // A remaining Mono must not name projects the device did not select.
        if device.projects.is_some() {
            for mono in list {
                if let Some(ids) = mono.get_mut("projectIds").and_then(Value::as_array_mut) {
                    ids.retain(|id| selected(&device.projects, id.as_str()));
                }
            }
        }
    }
    if let Some(list) = reply.get_mut("sessions").and_then(Value::as_array_mut) {
        list.retain(|s| {
            let mono_ok = s["monoId"]
                .as_str()
                .is_none_or(|id| selected(&device.monos, Some(id)));
            let hidden = s["id"]
                .as_str()
                .is_some_and(|id| hidden_sessions.iter().any(|v| v == id));
            selected(&device.projects, s["projectId"].as_str()) && mono_ok && !hidden
        });
    }
}

/// A validated selection a device narrowed itself to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub projects: Option<Vec<String>>,
    pub monos: Option<Vec<String>>,
}

/// The answer to `device.selection`: what the device is told, and the
/// narrower selection the host must persist, when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionAnswer {
    pub reply: Value,
    pub narrowed: Option<Selection>,
}

/// A requested selection field: left alone, set to all, or set to a list.
pub(crate) enum Requested {
    Keep,
    All,
    List(Vec<String>),
}

pub(crate) fn requested(params: &Value, key: &str) -> Result<Requested, String> {
    const INVALID: &str = "Invalid mobile parameters";
    match params.get(key) {
        None => Ok(Requested::Keep),
        Some(Value::Null) => Ok(Requested::All),
        Some(Value::Array(items)) => {
            if items.len() > 1000 {
                return Err(INVALID.into());
            }
            let mut list: Vec<String> = Vec::new();
            for item in items {
                let id = item.as_str().filter(|id| id.len() <= 512).ok_or(INVALID)?;
                if !list.iter().any(|v| v == id) {
                    list.push(id.to_owned());
                }
            }
            Ok(Requested::List(list))
        }
        Some(_) => Err(INVALID.into()),
    }
}

/// The new value of one field, refusing a widening.
fn narrowed_field(
    current: &Option<Vec<String>>,
    request: Requested,
) -> Result<Option<Vec<String>>, Failure> {
    match request {
        Requested::Keep => Ok(current.clone()),
        Requested::All if current.is_none() => Ok(None),
        Requested::All => Err(Code::Forbidden.failure()),
        Requested::List(list) => match current {
            Some(have) if !list.iter().all(|id| have.contains(id)) => {
                Err(Code::Forbidden.failure())
            }
            _ => Ok(Some(list)),
        },
    }
}

/// Answers `device.selection`. Without `projects` or `monos` it reports the
/// current selection. With them, a phone may only narrow its selection;
/// widening is refused here as `forbidden`. A web device goes through the
/// host's own selection handler instead, where a control device may widen
/// with a server ticket.
pub fn device_selection(
    device: &DeviceContext,
    params: &Value,
) -> Result<SelectionAnswer, Failure> {
    let projects = requested(params, "projects")?;
    let monos = requested(params, "monos")?;
    let selection = Selection {
        projects: narrowed_field(&device.projects, projects)?,
        monos: narrowed_field(&device.monos, monos)?,
    };
    let reply = json!({"projects": selection.projects, "monos": selection.monos});
    let changed = selection.projects != device.projects || selection.monos != device.monos;
    Ok(SelectionAnswer {
        reply,
        narrowed: changed.then_some(selection),
    })
}

/// The capabilities the desktop advertises.
#[allow(dead_code)]
pub fn capabilities(flags: &PolicyFlags) -> Vec<String> {
    let mut list = capabilities_with(flags, REMOTE_APPROVALS_ENABLED);
    let hidden = HIDDEN_CAPABILITIES
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    list.retain(|name| !hidden.contains(name));
    list
}

/// [`capabilities`] of one device: the desktop's list, plus `terminal` for a
/// web device that has Control and the desktop's "Allow web terminal" toggle.
pub fn device_capabilities(flags: &PolicyFlags, device: &Device) -> Vec<String> {
    let mut list = capabilities(flags);
    if device.kind == DeviceKind::Web
        && device.permission == Permission::Control
        && terminal_allowed(&device.id)
        && !HIDDEN_CAPABILITIES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|name| name == "terminal")
    {
        list.push("terminal".to_owned());
    }
    list
}

static HIDDEN_CAPABILITIES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Stops advertising `names`. Only the headless E2E desktop calls this, to behave like an older
/// release that lacks them; the app never does.
#[doc(hidden)]
pub fn hide_capabilities(names: &[String]) {
    *HIDDEN_CAPABILITIES
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = names.to_vec();
}

fn capabilities_with(flags: &PolicyFlags, approvals_gate: bool) -> Vec<String> {
    // The desktop supports cloud sessions: the web shows its cloud UI on this.
    let mut list = vec!["cloud".to_owned()];
    if flags.remote_git_push {
        list.push("gitPush".to_owned());
    }
    if flags.remote_cloud_start {
        list.push("cloudStart".to_owned());
    }
    if flags.remote_cloud_start && flags.remote_git_push {
        list.push("cloudMove".to_owned());
    }
    if approvals_effective(flags, approvals_gate) {
        list.push("approvals".to_owned());
    }
    // The app answers `chat.mentionedFile` (a read, always on).
    list.push("mentionedFiles".to_owned());
    // The widened evidence of rule 2 (a shell command or its output naming the file, or a file
    // made after the conversation began): a web that sees this knows a refusal is final.
    list.push("mentionedFiles2".to_owned());
    // `file.prepare`: files are opened through encrypted objects in R2 (a read, always on).
    list.push("fileTransfer".to_owned());
    list
}

#[cfg(test)]
mod tests {
    use super::super::host::{APP_METHODS, HOST_METHODS};
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn every_app_and_host_method_has_a_class() {
        for method in APP_METHODS.iter().chain(HOST_METHODS) {
            assert!(entry(method).is_some(), "{method} has no class");
        }
        let known: BTreeSet<_> = APP_METHODS.iter().chain(HOST_METHODS).copied().collect();
        for entry in METHODS {
            assert!(known.contains(entry.name), "{} is not a method", entry.name);
        }
        let mut names = BTreeSet::new();
        for entry in METHODS {
            assert!(names.insert(entry.name), "{} listed twice", entry.name);
        }
    }

    #[test]
    fn transport_matches_who_answers() {
        for method in APP_METHODS {
            assert_eq!(entry(method).unwrap().transport, Transport::App, "{method}");
        }
        for method in HOST_METHODS {
            assert_eq!(
                entry(method).unwrap().transport,
                Transport::Host,
                "{method}"
            );
        }
    }

    #[test]
    fn unknown_methods_have_no_class() {
        assert_eq!(classify("cloud.delete", &json!({})), None);
    }

    #[test]
    fn parameter_dependent_classes_follow_the_table() {
        let c = |m: &str, p: Value| classify(m, &p).unwrap();
        assert_eq!(c("accounts.usage", json!({})), Access::Read);
        assert_eq!(
            c("accounts.usage", json!({"refresh": true})),
            Access::Control
        );
        for action in ["stage", "unstage", "stageAll", "unstageAll", "generate"] {
            assert_eq!(
                c("project.gitAction", json!({"action": action})),
                Access::Control
            );
        }
        assert_eq!(
            c("project.gitAction", json!({"action": "commit"})),
            Access::Control
        );
        let push = Access::DeniedForWebUnless(Toggle::RemoteGitPush);
        for params in [
            json!({"action": "sync"}),
            json!({"action": "pr"}),
            json!({"action": "commit", "push": true}),
            json!({"action": "commit", "createPr": true}),
        ] {
            assert_eq!(c("project.gitAction", params), push);
        }
        assert_eq!(c("project.gitAction", json!({})), Access::DeniedForWeb);
        assert_eq!(
            c("project.gitAction", json!({"action": "rebase"})),
            Access::DeniedForWeb
        );
        assert_eq!(c("device.selection", json!({})), Access::Read);
        // Who may narrow or widen is decided by the host's selection handler,
        // so the call is a read for every parameter.
        assert_eq!(c("device.selection", json!({"projects": []})), Access::Read);
        assert_eq!(
            c(
                "device.selection",
                json!({"addProjects": ["r"], "ticket": "t"})
            ),
            Access::Read
        );
        assert_eq!(c("session.start", json!({})), Access::Control);
        assert_eq!(
            c("session.start", json!({"target": {"kind": "cloud"}})),
            Access::ControlWith(vec![Toggle::RemoteCloudStart])
        );
        for method in ["session.start", "session.configure"] {
            assert_eq!(
                c(method, json!({"runtimeMode": "full-access"})),
                Access::DeniedForRemote
            );
        }
        for method in ["mono.saveFile", "mono.reset", "mono.delete"] {
            assert_eq!(c(method, json!({})), Access::DeniedForWeb);
        }
        // A plain read stays a read; a write through mono.file is a save.
        assert_eq!(c("mono.file", json!({"file": "soul"})), Access::Read);
        assert_eq!(
            c("mono.file", json!({"file": "soul", "text": "x"})),
            Access::DeniedForWeb
        );
        assert_eq!(
            c("mono.file", json!({"file": "soul", "expectedHash": "h"})),
            Access::DeniedForWeb
        );
        for method in ["device.push", "project.fileHighlight"] {
            assert_eq!(c(method, json!({})), Access::UnsupportedWeb);
        }
    }

    #[test]
    fn a_mentioned_file_read_is_an_app_read_for_every_device_and_follows_the_mono_selection() {
        let entry = entry("chat.mentionedFile").unwrap();
        assert_eq!(
            (entry.class, entry.transport),
            (Class::Read, Transport::App)
        );
        let params = json!({"conversationId": "mono:m1", "path": "a.html"});
        assert_eq!(classify("chat.mentionedFile", &params), Some(Access::Read));
        for kind in [DeviceKind::Phone, DeviceKind::Web] {
            let reader = ctx(kind, Permission::Read);
            assert_eq!(
                call(&reader, "chat.mentionedFile", params.clone(), &ON),
                None
            );
            let mut narrowed = ctx(kind, Permission::Control);
            narrowed.monos = Some(vec!["other".into()]);
            assert_eq!(
                call(&narrowed, "chat.mentionedFile", params.clone(), &ON),
                Some(Code::NotSelected)
            );
            narrowed.monos = Some(vec!["m1".into()]);
            assert_eq!(
                call(&narrowed, "chat.mentionedFile", params.clone(), &ON),
                None
            );
        }
        assert!(capabilities_with(&PolicyFlags::default(), false)
            .contains(&"mentionedFiles".to_owned()));
    }

    #[test]
    fn error_codes_equal_the_web_link_list() {
        let mut ours: Vec<&str> = Code::ALL.iter().map(|c| c.as_str()).collect();
        ours.sort_unstable();
        assert_eq!(ours, super::super::web_link::frame::DESKTOP_ERROR_CODES);
        assert_eq!(Code::ReadOnly.as_str(), "read_only");
        assert_eq!(Code::InstanceBusy.as_str(), "instance_busy");
    }

    fn ctx(kind: DeviceKind, permission: Permission) -> DeviceContext {
        DeviceContext {
            id: "d".into(),
            kind,
            permission,
            projects: None,
            monos: None,
        }
    }

    const ON: PolicyFlags = PolicyFlags {
        remote_git_push: true,
        remote_cloud_start: true,
        remote_approvals: true,
        show_push_content: true,
    };

    fn code(result: Result<(), Denial>) -> Option<Code> {
        result.err().map(|d| d.code)
    }

    fn call(
        device: &DeviceContext,
        method: &str,
        params: Value,
        flags: &PolicyFlags,
    ) -> Option<Code> {
        code(authorize_with(device, method, &params, flags, true))
    }

    #[test]
    fn every_code_round_trips_through_its_text() {
        for code in Code::ALL {
            assert_eq!(code_of_error(code.message()), Some(*code));
            assert_eq!(Denial::new(*code).error, code.message());
        }
        assert_eq!(Code::Unsupported.message(), "Unsupported mobile operation");
        assert_eq!(Code::Busy.message(), "Desktop is busy; try again");
        assert_eq!(code_of_error("something else"), None);
    }

    #[test]
    fn phone_default_context_keeps_todays_answers() {
        let device = Device {
            id: "d".into(),
            name: "Phone".into(),
            fingerprint: "ab".into(),
            push: None,
            kind: DeviceKind::Phone,
            permission: Permission::Control,
            projects: None,
            monos: None,
            server_device_id: None,
            created_at: None,
            last_seen: None,
        };
        let phone = DeviceContext::phone(&device);
        assert_eq!(phone.kind, DeviceKind::Phone);
        assert_eq!(phone.permission, Permission::Control);
        assert!(phone.projects.is_none() && phone.monos.is_none());
        let off = PolicyFlags::default();
        for entry in METHODS {
            let params = match entry.name {
                "project.gitAction" => json!({"action": "stage"}),
                // Held back by design: see the checks below.
                "session.answerApproval"
                | "cloud.moveToCloud"
                | "cloud.moveToLocal"
                | "cloud.cancelMove" => continue,
                // A shell is for browsers only: see the terminal tests.
                _ if entry.class == Class::Terminal => continue,
                _ => json!({}),
            };
            assert_eq!(
                call(&phone, entry.name, params, &off),
                None,
                "{}",
                entry.name
            );
        }
        assert_eq!(call(&phone, "mono.delete", json!({}), &off), None);
        for action in ["sync", "pr"] {
            assert_eq!(
                call(&phone, "project.gitAction", json!({"action": action}), &off),
                None
            );
        }
        // The deliberate tightenings.
        for method in ["session.start", "session.configure"] {
            assert_eq!(
                call(&phone, method, json!({"runtimeMode": "full-access"}), &ON),
                Some(Code::Forbidden)
            );
        }
        assert_eq!(call(&phone, "session.answerApproval", json!({}), &ON), None);
        assert_eq!(
            code(authorize_with(
                &phone,
                "session.answerApproval",
                &json!({}),
                &ON,
                false
            )),
            Some(Code::Disabled)
        );
        assert_eq!(
            code(authorize(&phone, "session.answerApproval", &json!({}))),
            Some(Code::Disabled)
        );
    }

    #[test]
    fn every_method_follows_its_class_per_device() {
        let off = PolicyFlags::default();
        let devices = [
            (DeviceKind::Phone, Permission::Read),
            (DeviceKind::Phone, Permission::Control),
            (DeviceKind::Web, Permission::Read),
            (DeviceKind::Web, Permission::Control),
        ];
        for entry in METHODS {
            let params = match entry.name {
                "project.gitAction" => json!({"action": "stage"}),
                "session.answerApproval" => json!({}),
                _ => json!({}),
            };
            let Some(access) = classify(entry.name, &params) else {
                panic!("{} is unclassified", entry.name);
            };
            for (kind, permission) in devices {
                let device = ctx(kind, permission);
                let web = kind == DeviceKind::Web;
                let read = permission == Permission::Read;
                let expected = match &access {
                    Access::Read => None,
                    Access::Control => read.then_some(Code::ReadOnly),
                    Access::ControlWith(_) => {
                        Some(if read { Code::ReadOnly } else { Code::Disabled })
                    }
                    Access::DeniedForWeb | Access::DeniedForWebUnless(_) => {
                        if web {
                            Some(Code::Forbidden)
                        } else {
                            read.then_some(Code::ReadOnly)
                        }
                    }
                    Access::DeniedForRemote => Some(Code::Forbidden),
                    Access::UnsupportedWeb => web.then_some(Code::Unsupported),
                    // Read-only first, then the toggle, which is off here.
                    Access::Terminal => Some(if !web {
                        Code::Forbidden
                    } else if read {
                        Code::ReadOnly
                    } else {
                        Code::Disabled
                    }),
                };
                let expected = if entry.name == "session.answerApproval" && expected.is_none() {
                    Some(Code::Disabled)
                } else {
                    expected
                };
                assert_eq!(
                    code(authorize_with(&device, entry.name, &params, &off, false)),
                    expected,
                    "{} for {kind:?}/{permission:?}",
                    entry.name
                );
            }
        }
        // An unclassified method fails closed, for every device.
        for device in [
            ctx(DeviceKind::Web, Permission::Read),
            ctx(DeviceKind::Web, Permission::Control),
            ctx(DeviceKind::Phone, Permission::Control),
        ] {
            assert_eq!(
                call(&device, "cloud.delete", json!({}), &off),
                Some(Code::Unsupported)
            );
        }
    }

    #[test]
    fn mono_file_with_text_is_a_save_for_every_device() {
        let write = json!({"monoId": "m1", "file": "soul", "text": "x"});
        let read = json!({"monoId": "m1", "file": "soul"});
        for permission in [Permission::Read, Permission::Control] {
            let web = ctx(DeviceKind::Web, permission);
            assert_eq!(
                call(&web, "mono.file", write.clone(), &ON),
                Some(Code::Forbidden)
            );
            assert_eq!(call(&web, "mono.file", read.clone(), &ON), None);
        }
        let read_phone = ctx(DeviceKind::Phone, Permission::Read);
        assert_eq!(
            call(&read_phone, "mono.file", write.clone(), &ON),
            Some(Code::ReadOnly)
        );
        assert_eq!(call(&read_phone, "mono.file", read.clone(), &ON), None);
        let phone = ctx(DeviceKind::Phone, Permission::Control);
        assert_eq!(call(&phone, "mono.file", write, &ON), None);
        assert_eq!(call(&phone, "mono.file", read, &ON), None);
    }

    #[test]
    fn web_cannot_write_monos_and_git_push_needs_the_toggle() {
        let off = PolicyFlags::default();
        let web = ctx(DeviceKind::Web, Permission::Control);
        for method in ["mono.saveFile", "mono.reset", "mono.delete"] {
            assert_eq!(call(&web, method, json!({}), &ON), Some(Code::Forbidden));
        }
        let pushes = [
            json!({"action": "sync"}),
            json!({"action": "pr"}),
            json!({"action": "commit", "push": true}),
            json!({"action": "commit", "createPr": true}),
        ];
        for params in pushes {
            assert_eq!(
                call(&web, "project.gitAction", params.clone(), &off),
                Some(Code::Forbidden)
            );
            assert_eq!(call(&web, "project.gitAction", params.clone(), &ON), None);
            let reader = ctx(DeviceKind::Web, Permission::Read);
            assert_eq!(
                call(&reader, "project.gitAction", params, &ON),
                Some(Code::ReadOnly)
            );
        }
        for action in ["stage", "unstage", "generate", "commit"] {
            let params = json!({"action": action});
            assert_eq!(
                call(&web, "project.gitAction", params.clone(), &off),
                None,
                "{action}"
            );
            let reader = ctx(DeviceKind::Web, Permission::Read);
            assert_eq!(
                call(&reader, "project.gitAction", params, &off),
                Some(Code::ReadOnly)
            );
        }
    }

    #[test]
    fn only_a_control_device_may_update_the_desktop_app() {
        let off = PolicyFlags::default();
        for kind in [DeviceKind::Web, DeviceKind::Phone] {
            let reader = ctx(kind, Permission::Read);
            let control = ctx(kind, Permission::Control);
            assert_eq!(
                call(&reader, "desktop.update", json!({"confirm": true}), &off),
                Some(Code::ReadOnly)
            );
            assert_eq!(call(&control, "desktop.update", json!({}), &off), None);
            // The status is a read, and the update is never one (it is audited).
            assert_eq!(call(&reader, "desktop.updateStatus", json!({}), &off), None);
        }
        assert_eq!(
            classify("desktop.update", &json!({})),
            Some(Access::Control)
        );
        assert_eq!(
            classify("desktop.updateStatus", &json!({})),
            Some(Access::Read)
        );
    }

    #[test]
    fn full_access_is_forbidden_for_every_device() {
        for kind in [DeviceKind::Phone, DeviceKind::Web] {
            let device = ctx(kind, Permission::Control);
            for method in ["session.start", "session.configure"] {
                assert_eq!(
                    call(&device, method, json!({"runtimeMode": "full-access"}), &ON),
                    Some(Code::Forbidden)
                );
                assert_eq!(
                    call(&device, method, json!({"runtimeMode": "default"}), &ON),
                    None
                );
            }
        }
    }

    #[test]
    fn cloud_toggles_gate_every_device_kind() {
        let off = PolicyFlags::default();
        for kind in [DeviceKind::Phone, DeviceKind::Web] {
            let device = ctx(kind, Permission::Control);
            let start = json!({"target": {"kind": "cloud"}});
            assert_eq!(
                call(&device, "session.start", start.clone(), &off),
                Some(Code::Disabled)
            );
            assert_eq!(call(&device, "session.start", start, &ON), None);
            assert_eq!(
                call(&device, "cloud.moveToCloud", json!({}), &off),
                Some(Code::Disabled)
            );
            assert_eq!(call(&device, "cloud.moveToCloud", json!({}), &ON), None);
        }
    }

    #[test]
    fn selection_limits_projects_monos_and_mono_conversations() {
        let mut device = ctx(DeviceKind::Web, Permission::Control);
        device.projects = Some(vec!["p1".into()]);
        device.monos = Some(vec!["m1".into()]);
        let off = PolicyFlags::default();
        let cases = [
            ("project.files", json!({"projectId": "p1"}), None),
            (
                "project.files",
                json!({"projectId": "p2"}),
                Some(Code::NotSelected),
            ),
            ("mono.document", json!({"monoId": "m1"}), None),
            (
                "mono.document",
                json!({"monoId": "m2"}),
                Some(Code::NotSelected),
            ),
            (
                "conversation.read",
                json!({"conversationId": "mono:m1"}),
                None,
            ),
            (
                "conversation.read",
                json!({"conversationId": "mono:m2"}),
                Some(Code::NotSelected),
            ),
            (
                "conversation.read",
                json!({"conversationId": "session-x"}),
                None,
            ),
            (
                "sessions.list",
                json!({"projectId": "p2"}),
                Some(Code::NotSelected),
            ),
        ];
        for (method, params, expected) in cases {
            assert_eq!(
                call(&device, method, params.clone(), &off),
                expected,
                "{method} {params}"
            );
        }
        // Permission errors come before the selection.
        let mut reader = device.clone();
        reader.permission = Permission::Read;
        assert_eq!(
            call(
                &reader,
                "session.cancel",
                json!({"conversationId": "mono:m2"}),
                &off
            ),
            Some(Code::ReadOnly)
        );
        // No selection means all.
        let all = ctx(DeviceKind::Web, Permission::Control);
        assert_eq!(
            call(&all, "project.files", json!({"projectId": "p2"}), &off),
            None
        );
    }

    fn catalog() -> Value {
        json!({
            "projects": [{"id": "p1"}, {"id": "p2"}],
            "monos": [
                {"id": "m1", "projectIds": ["p1", "p2"], "sessionId": "s-m1"},
                {"id": "m2", "projectIds": ["p1"], "sessionId": "s-m2"}
            ],
            "sessions": [
                {"id": "a", "projectId": "p1"},
                {"id": "b", "projectId": "p2"},
                {"id": "s-m1", "projectId": "p1", "monoId": "m1"},
                {"id": "s-m2", "projectId": "p1", "monoId": "m2"}
            ],
            "push": true
        })
    }

    #[test]
    fn filter_reply_hides_unselected_projects_and_monos() {
        let all = ctx(DeviceKind::Web, Permission::Read);
        let mut reply = catalog();
        filter_reply(&all, "workspace.list", &mut reply);
        assert_eq!(reply.to_string(), catalog().to_string());

        let mut device = all.clone();
        device.projects = Some(vec!["p1".into()]);
        let mut reply = catalog();
        filter_reply(&device, "workspace.list", &mut reply);
        assert_eq!(reply["projects"], json!([{"id": "p1"}]));
        assert_eq!(reply["monos"][0]["projectIds"], json!(["p1"]));
        let ids: Vec<_> = reply["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].clone())
            .collect();
        assert_eq!(ids, [json!("a"), json!("s-m1"), json!("s-m2")]);

        device.projects = None;
        device.monos = Some(vec!["m1".into()]);
        let mut reply = catalog();
        filter_reply(&device, "sessions.list", &mut reply);
        assert_eq!(reply["monos"].as_array().unwrap().len(), 1);
        let ids: Vec<_> = reply["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].clone())
            .collect();
        assert_eq!(ids, [json!("a"), json!("b"), json!("s-m1")]);

        // Other methods are left alone.
        let mut reply = catalog();
        filter_reply(&device, "conversation.read", &mut reply);
        assert_eq!(reply, catalog());
    }

    #[test]
    fn device_selection_reports_and_only_narrows() {
        let mut device = ctx(DeviceKind::Web, Permission::Control);
        let get = device_selection(&device, &json!({})).unwrap();
        assert_eq!(get.reply, json!({"projects": null, "monos": null}));
        assert_eq!(get.narrowed, None);

        let narrow = device_selection(&device, &json!({"projects": ["a", "b", "a"]})).unwrap();
        assert_eq!(narrow.reply, json!({"projects": ["a", "b"], "monos": null}));
        assert_eq!(
            narrow.narrowed,
            Some(Selection {
                projects: Some(vec!["a".into(), "b".into()]),
                monos: None
            })
        );

        device.projects = Some(vec!["a".into(), "b".into()]);
        let same = device_selection(&device, &json!({"projects": ["b", "a"]})).unwrap();
        assert!(same.narrowed.is_some());
        let unchanged = device_selection(&device, &json!({"projects": ["a", "b"]})).unwrap();
        assert_eq!(unchanged.narrowed, None);
        let smaller = device_selection(&device, &json!({"projects": ["a"], "monos": []})).unwrap();
        assert_eq!(
            smaller.narrowed,
            Some(Selection {
                projects: Some(vec!["a".into()]),
                monos: Some(vec![])
            })
        );

        let widen = Code::Forbidden.failure();
        assert_eq!(
            device_selection(&device, &json!({"projects": null})).unwrap_err(),
            widen
        );
        assert_eq!(
            device_selection(&device, &json!({"projects": ["a", "c"]})).unwrap_err(),
            widen
        );
        assert_eq!(widen.code.as_deref(), Some("forbidden"));

        let invalid = "Invalid mobile parameters";
        let too_long = "x".repeat(513);
        let too_many: Vec<String> = (0..1001).map(|i| i.to_string()).collect();
        for params in [
            json!({"projects": "a"}),
            json!({"projects": [1]}),
            json!({"monos": {"a": 1}}),
            json!({"projects": [too_long]}),
            json!({"projects": too_many}),
        ] {
            assert_eq!(
                device_selection(&device, &params).unwrap_err(),
                Failure::from(invalid),
                "{params}"
            );
        }
        // The policy lets every device reach the handler, which decides who
        // may narrow (web: read and control) or widen (see `selection.rs`).
        let reader = ctx(DeviceKind::Web, Permission::Read);
        assert_eq!(
            call(&reader, "device.selection", json!({"projects": []}), &ON),
            None
        );
        assert_eq!(call(&reader, "device.selection", json!({}), &ON), None);
    }

    #[test]
    fn capabilities_follow_flags_and_the_approvals_gate() {
        assert_eq!(
            capabilities_with(&PolicyFlags::default(), false),
            ["cloud", "mentionedFiles", "mentionedFiles2", "fileTransfer"]
        );
        assert_eq!(
            capabilities_with(&ON, false),
            [
                "cloud",
                "gitPush",
                "cloudStart",
                "cloudMove",
                "mentionedFiles",
                "mentionedFiles2",
                "fileTransfer"
            ]
        );
        // The gate alone does not list approvals: the desktop toggle is needed too.
        assert_eq!(
            capabilities_with(&PolicyFlags::default(), true),
            ["cloud", "mentionedFiles", "mentionedFiles2", "fileTransfer"]
        );
        let toggle = PolicyFlags {
            remote_approvals: true,
            ..PolicyFlags::default()
        };
        assert_eq!(
            capabilities_with(&toggle, false),
            ["cloud", "mentionedFiles", "mentionedFiles2", "fileTransfer"]
        );
        assert_eq!(
            capabilities_with(&toggle, true),
            [
                "cloud",
                "approvals",
                "mentionedFiles",
                "mentionedFiles2",
                "fileTransfer"
            ]
        );
        assert_eq!(
            capabilities_with(&ON, true),
            [
                "cloud",
                "gitPush",
                "cloudStart",
                "cloudMove",
                "approvals",
                "mentionedFiles",
                "mentionedFiles2",
                "fileTransfer"
            ]
        );
    }

    /// H-11, H-14: `cloudStart` follows the cloud toggle, `cloudMove` needs it
    /// and the push toggle too (a move pushes a branch).
    #[test]
    fn cloud_capabilities_follow_their_toggles() {
        let flags = |push, cloud| PolicyFlags {
            remote_git_push: push,
            remote_cloud_start: cloud,
            remote_approvals: false,
            show_push_content: true,
        };
        assert_eq!(
            capabilities_with(&flags(false, true), false),
            [
                "cloud",
                "cloudStart",
                "mentionedFiles",
                "mentionedFiles2",
                "fileTransfer"
            ]
        );
        assert_eq!(
            capabilities_with(&flags(true, false), false),
            [
                "cloud",
                "gitPush",
                "mentionedFiles",
                "mentionedFiles2",
                "fileTransfer"
            ]
        );
        assert_eq!(
            capabilities_with(&flags(true, true), false),
            [
                "cloud",
                "gitPush",
                "cloudStart",
                "cloudMove",
                "mentionedFiles",
                "mentionedFiles2",
                "fileTransfer"
            ]
        );
        assert_eq!(
            capabilities_with(&flags(false, false), false),
            ["cloud", "mentionedFiles", "mentionedFiles2", "fileTransfer"]
        );
    }

    /// H-08: approvals are effective only with the U-13 gate AND the toggle.
    #[test]
    fn approvals_need_both_the_gate_and_the_desktop_toggle() {
        let phone = ctx(DeviceKind::Phone, Permission::Control);
        let web = ctx(DeviceKind::Web, Permission::Control);
        for (gate, toggle) in [(false, false), (false, true), (true, false), (true, true)] {
            let flags = PolicyFlags {
                remote_approvals: toggle,
                ..PolicyFlags::default()
            };
            let on = gate && toggle;
            assert_eq!(approvals_effective(&flags, gate), on, "{gate}/{toggle}");
            assert_eq!(
                capabilities_with(&flags, gate).contains(&"approvals".to_owned()),
                on,
                "{gate}/{toggle}"
            );
            for device in [&phone, &web] {
                assert_eq!(
                    code(authorize_with(
                        device,
                        "session.answerApproval",
                        &json!({}),
                        &flags,
                        gate
                    )),
                    (!on).then_some(Code::Disabled),
                    "{gate}/{toggle}/{:?}",
                    device.kind
                );
            }
        }
    }

    /// U-13 passed: the real gate is on, so the desktop toggle decides. Toggle
    /// off is refused with the existing code, toggle on lets a control device
    /// answer, and a read device is refused either way.
    #[test]
    fn with_the_real_gate_on_the_toggle_decides() {
        assert!(
            !PolicyFlags::default().remote_approvals,
            "the toggle defaults off"
        );
        assert!(!approvals_effective(
            &PolicyFlags::default(),
            REMOTE_APPROVALS_ENABLED
        ));
        let off = PolicyFlags::default();
        let on = PolicyFlags {
            remote_approvals: true,
            ..PolicyFlags::default()
        };
        let answer = |device: &DeviceContext, flags: &PolicyFlags| {
            code(authorize_with(
                device,
                "session.answerApproval",
                &json!({}),
                flags,
                REMOTE_APPROVALS_ENABLED,
            ))
        };
        for kind in [DeviceKind::Phone, DeviceKind::Web] {
            let control = ctx(kind, Permission::Control);
            let reader = ctx(kind, Permission::Read);
            assert_eq!(answer(&control, &off), Some(Code::Disabled), "{kind:?}");
            assert_eq!(answer(&control, &on), None, "{kind:?}");
            assert_eq!(answer(&reader, &on), Some(Code::ReadOnly), "{kind:?}");
            assert_eq!(answer(&reader, &off), Some(Code::ReadOnly), "{kind:?}");
        }
        assert_eq!(
            capabilities_with(&off, REMOTE_APPROVALS_ENABLED),
            ["cloud", "mentionedFiles", "mentionedFiles2", "fileTransfer"]
        );
        assert_eq!(
            capabilities_with(&on, REMOTE_APPROVALS_ENABLED),
            [
                "cloud",
                "approvals",
                "mentionedFiles",
                "mentionedFiles2",
                "fileTransfer"
            ]
        );
    }

    /// M1: saves from several threads never share a temporary file, so each
    /// one lands whole and none leaves a temp file behind.
    #[test]
    fn concurrent_saves_use_their_own_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut settings = PolicySettings::default();
                    settings.flags.remote_git_push = i % 2 == 0;
                    for _ in 0..20 {
                        save_settings(&path, &settings).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let saved = std::fs::read(&path).unwrap();
        assert!(serde_json::from_slice::<PolicySettings>(&saved).is_ok());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn settings_round_trip_and_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        assert_eq!(load_settings(&path), PolicySettings::default());
        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(load_settings(&path), PolicySettings::default());
        let mut settings = PolicySettings::default();
        settings.flags.remote_git_push = true;
        settings
            .deny_patterns
            .insert("/p".into(), vec!["*.pem".into()]);
        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), settings);
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"remoteGitPush\": true") && text.contains("\"denyPatterns\""));
        let empty = serde_json::to_string(&PolicySettings::default()).unwrap();
        assert_eq!(
            empty,
            r#"{"remoteGitPush":false,"remoteCloudStart":false,"remoteApprovals":false,"showPushContent":true,"denyPatterns":{}}"#
        );
        // A file saved before the setting existed keeps it on.
        std::fs::write(&path, br#"{"remoteGitPush":true,"denyPatterns":{}}"#).unwrap();
        assert!(load_settings(&path).flags.show_push_content);
        std::fs::write(&path, br#"{"showPushContent":false}"#).unwrap();
        assert!(!load_settings(&path).flags.show_push_content);
        let partial: PolicySettings = serde_json::from_str("{}").unwrap();
        assert_eq!(partial, PolicySettings::default());
    }

    /// The Rust table and the TypeScript contract list the same methods with
    /// the same class.
    #[test]
    fn table_matches_the_typescript_contract() {
        let file: Value =
            serde_json::from_str(include_str!("../../../../web/protocol/methods.json")).unwrap();
        let theirs: BTreeMap<String, (String, String)> = file["methods"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["name"].as_str().unwrap().to_owned(),
                    (
                        m["class"].as_str().unwrap().to_owned(),
                        m["transport"].as_str().unwrap().to_owned(),
                    ),
                )
            })
            .collect();
        let ours: BTreeMap<String, (String, String)> = METHODS
            .iter()
            .map(|e| {
                (
                    e.name.to_owned(),
                    (e.class.as_str().to_owned(), e.transport.as_str().to_owned()),
                )
            })
            .collect();
        assert_eq!(ours, theirs);
    }

    const TERMINAL_METHODS: [&str; 5] = [
        "terminal.open",
        "terminal.read",
        "terminal.write",
        "terminal.resize",
        "terminal.close",
    ];

    #[test]
    fn the_terminal_is_its_own_class_and_control_does_not_imply_it() {
        for method in TERMINAL_METHODS {
            assert_eq!(entry(method).unwrap().class, Class::Terminal, "{method}");
            assert_eq!(entry(method).unwrap().transport, Transport::Host);
            assert_eq!(classify(method, &json!({})), Some(Access::Terminal));
        }
        assert_eq!(Class::Terminal.as_str(), "terminal");
        // A device with Control and every other toggle on still gets no shell.
        let control = ctx(DeviceKind::Web, Permission::Control);
        for method in TERMINAL_METHODS {
            assert_eq!(
                code(authorize_full(
                    &control,
                    method,
                    &json!({}),
                    &ON,
                    true,
                    false
                )),
                Some(Code::Disabled),
                "{method}"
            );
        }
        // And the shell toggle opens nothing but the shell.
        assert_eq!(
            code(authorize_full(
                &control,
                "mono.delete",
                &json!({}),
                &PolicyFlags::default(),
                false,
                true
            )),
            Some(Code::Forbidden)
        );
        assert_eq!(
            code(authorize_full(
                &control,
                "session.answerApproval",
                &json!({}),
                &PolicyFlags::default(),
                true,
                true
            )),
            Some(Code::Disabled)
        );
    }

    #[test]
    fn a_web_device_with_control_and_the_toggle_may_use_the_terminal_methods() {
        let control = ctx(DeviceKind::Web, Permission::Control);
        for method in TERMINAL_METHODS {
            let off = PolicyFlags::default();
            assert_eq!(
                code(authorize_full(
                    &control,
                    method,
                    &json!({}),
                    &off,
                    false,
                    true
                )),
                None,
                "{method}"
            );
        }
    }

    #[test]
    fn a_read_only_device_never_gets_the_terminal_whatever_the_toggle_says() {
        let read = ctx(DeviceKind::Web, Permission::Read);
        for method in TERMINAL_METHODS {
            for toggle in [false, true] {
                assert_eq!(
                    code(authorize_full(&read, method, &json!({}), &ON, true, toggle)),
                    Some(Code::ReadOnly),
                    "{method} toggle {toggle}"
                );
            }
        }
    }

    #[test]
    fn a_phone_never_gets_the_terminal() {
        for permission in [Permission::Read, Permission::Control] {
            let phone = ctx(DeviceKind::Phone, permission);
            for method in TERMINAL_METHODS {
                assert_eq!(
                    code(authorize_full(&phone, method, &json!({}), &ON, true, true)),
                    Some(Code::Forbidden),
                    "{method}"
                );
            }
        }
    }

    #[test]
    fn unknown_terminal_methods_fail_closed_even_with_the_toggle() {
        let control = ctx(DeviceKind::Web, Permission::Control);
        for method in [
            "terminal.exec",
            "terminal",
            "terminal.attach",
            "terminal.read ",
        ] {
            assert_eq!(
                code(authorize_full(
                    &control,
                    method,
                    &json!({}),
                    &ON,
                    true,
                    true
                )),
                Some(Code::Unsupported),
                "{method}"
            );
        }
    }

    #[test]
    fn the_terminal_selection_still_applies() {
        let mut control = ctx(DeviceKind::Web, Permission::Control);
        control.projects = Some(vec!["a".into()]);
        let params = json!({"projectId": "b"});
        assert_eq!(
            code(authorize_full(
                &control,
                "terminal.open",
                &params,
                &ON,
                true,
                true
            )),
            Some(Code::NotSelected)
        );
    }

    #[test]
    fn the_terminal_toggle_is_off_by_default_per_device_and_loads_from_old_files() {
        assert!(PolicySettings::default().terminal_devices.is_empty());
        // A file an earlier version wrote has no such field.
        let old: PolicySettings =
            serde_json::from_str(r#"{"remoteGitPush":true,"denyPatterns":{}}"#).unwrap();
        assert!(old.terminal_devices.is_empty());
        let _guard = TEST_SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = settings();
        assert!(!terminal_allowed("policy-test-device"));
        set_terminal_allowed("policy-test-device", true);
        assert!(terminal_allowed("policy-test-device"));
        assert!(!terminal_allowed("another-policy-test-device"));
        // `authorize` reads the toggle of the device's own id.
        let mut web = ctx(DeviceKind::Web, Permission::Control);
        web.id = "policy-test-device".into();
        assert_eq!(code(authorize(&web, "terminal.open", &json!({}))), None);
        web.id = "another-policy-test-device".into();
        assert_eq!(
            code(authorize(&web, "terminal.open", &json!({}))),
            Some(Code::Disabled)
        );
        set_terminal_allowed("policy-test-device", false);
        assert!(!terminal_allowed("policy-test-device"));
        install_settings(before);
    }

    #[test]
    fn the_terminal_toggle_is_saved_with_the_settings() {
        let _guard = TEST_SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = settings();
        let dir = std::env::temp_dir().join(format!("policy-terminal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE);
        set_terminal_allowed("saved-device", true);
        persist_settings(&path).unwrap();
        assert!(load_settings(&path)
            .terminal_devices
            .contains("saved-device"));
        set_terminal_allowed("saved-device", false);
        persist_settings(&path).unwrap();
        assert!(!load_settings(&path)
            .terminal_devices
            .contains("saved-device"));
        std::fs::remove_dir_all(&dir).unwrap();
        install_settings(before);
    }

    #[test]
    fn only_an_allowed_web_device_with_control_is_told_it_has_a_terminal() {
        let _guard = TEST_SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = settings();
        let row = |id: &str, kind, permission| Device {
            id: id.into(),
            name: "Laptop".into(),
            fingerprint: "f".into(),
            push: None,
            kind,
            permission,
            projects: None,
            monos: None,
            server_device_id: None,
            created_at: None,
            last_seen: None,
        };
        let flags = PolicyFlags::default();
        let has = |device: &Device| {
            device_capabilities(&flags, device)
                .iter()
                .any(|name| name == "terminal")
        };
        set_terminal_allowed("cap-web", true);
        set_terminal_allowed("cap-read", true);
        set_terminal_allowed("cap-phone", true);
        assert!(has(&row("cap-web", DeviceKind::Web, Permission::Control)));
        assert!(!has(&row("cap-read", DeviceKind::Web, Permission::Read)));
        assert!(!has(&row(
            "cap-phone",
            DeviceKind::Phone,
            Permission::Control
        )));
        assert!(!has(&row("cap-off", DeviceKind::Web, Permission::Control)));
        // The desktop's own list never carries it.
        assert!(!capabilities(&flags).iter().any(|name| name == "terminal"));
        install_settings(before);
    }
}
