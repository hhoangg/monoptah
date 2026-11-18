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
use std::collections::BTreeMap;
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

/// Desktop toggles that gate parts of the policy. All default to off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PolicyFlags {
    /// "Remote push in Git".
    pub remote_git_push: bool,
    /// "Remote cloud start".
    pub remote_cloud_start: bool,
    /// "Answer approvals from remote devices" (H-08). Only effective while
    /// [`REMOTE_APPROVALS_ENABLED`] is on as well.
    pub remote_approvals: bool,
}

/// The file name of the saved policy settings, next to the mobile config.
pub const SETTINGS_FILE: &str = "mobile-policy.json";

/// Whether remote devices may answer tool approvals. Stays false until the
/// U-13 review gate passes (H-08). Even then the desktop's own toggle
/// (`PolicyFlags::remote_approvals`) must be on: see [`approvals_effective`].
pub const REMOTE_APPROVALS_ENABLED: bool = false;

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
}

static SETTINGS: RwLock<PolicySettings> = RwLock::new(PolicySettings {
    flags: PolicyFlags {
        remote_git_push: false,
        remote_cloud_start: false,
        remote_approvals: false,
    },
    deny_patterns: BTreeMap::new(),
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
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = std::path::PathBuf::from(temp);
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
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

/// Replaces the process-wide toggles, keeping the deny patterns.
pub fn set_flags(flags: PolicyFlags) {
    SETTINGS.write().unwrap_or_else(|e| e.into_inner()).flags = flags;
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
    Revoked => "revoked",
    Disabled => "disabled",
    TooLarge => "too_large",
    Unsupported => "unsupported",
    UnsupportedTarget => "unsupported_target",
    Busy => "busy",
    Starting => "starting",
    AutoStopping => "auto_stopping",
    InstanceBusy => "instance_busy",
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
            Code::Revoked => "Device revoked",
            Code::Disabled => "Turned off on the desktop",
            Code::TooLarge => "Too large for a remote device",
            Code::Unsupported => "Unsupported mobile operation",
            Code::UnsupportedTarget => "Not available for this session",
            Code::Busy => "Desktop is busy; try again",
            Code::Starting => "The cloud instance is still starting",
            Code::AutoStopping => "The cloud instance is stopping on its own",
            Code::InstanceBusy => "The cloud instance is busy with another operation",
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
    host("device.enroll", Class::Denied),
    host("device.leave", Class::Read),
    host("workspace.list", Class::Read),
    host("sessions.list", Class::Read),
    host("device.push", Class::UnsupportedWeb),
    host("device.selection", Class::Param),
    host("conversation.watch", Class::Read),
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
            // Selection edits are only recognised by the field they carry.
            "device.selection"
                if params.get("projects").is_some() || params.get("monos").is_some() =>
            {
                Access::Control
            }
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
    authorize_with(device, method, params, &flags(), REMOTE_APPROVALS_ENABLED)
}

/// [`authorize`] with the toggles given. Order: permission, forbidden,
/// disabled and unsupported first, then the selection.
pub fn authorize_with(
    device: &DeviceContext,
    method: &str,
    params: &Value,
    flags: &PolicyFlags,
    approvals_gate: bool,
) -> Result<(), Denial> {
    // An unknown method is answered "Unsupported mobile operation" by the host.
    let Some(access) = classify(method, params) else {
        return Ok(());
    };
    let web = device.kind == DeviceKind::Web;
    match access {
        Access::Read => {}
        Access::Control => require_control(device)?,
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
enum Requested {
    Keep,
    All,
    List(Vec<String>),
}

fn requested(params: &Value, key: &str) -> Result<Requested, String> {
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
) -> Result<Option<Vec<String>>, String> {
    match request {
        Requested::Keep => Ok(current.clone()),
        Requested::All if current.is_none() => Ok(None),
        Requested::All => Err(Denial::new(Code::Forbidden).error),
        Requested::List(list) => match current {
            Some(have) if !list.iter().all(|id| have.contains(id)) => {
                Err(Denial::new(Code::Forbidden).error)
            }
            _ => Ok(Some(list)),
        },
    }
}

/// Answers `device.selection`. Without `projects` or `monos` it reports the
/// current selection. With them, a device may only narrow its selection;
/// widening it needs the desktop owner to confirm on the desktop, so it is
/// refused here as `forbidden`.
pub fn device_selection(device: &DeviceContext, params: &Value) -> Result<SelectionAnswer, String> {
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
    capabilities_with(flags, REMOTE_APPROVALS_ENABLED)
}

fn capabilities_with(flags: &PolicyFlags, approvals_gate: bool) -> Vec<String> {
    let mut list = Vec::new();
    if flags.remote_git_push {
        list.push("gitPush".to_owned());
    }
    if approvals_effective(flags, approvals_gate) {
        list.push("approvals".to_owned());
    }
    list
}

#[cfg(test)]
mod tests {
    use super::super::host::APP_METHODS;
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    const HOST_METHODS: &[&str] = &[
        "device.enroll",
        "device.leave",
        "workspace.list",
        "sessions.list",
        "device.push",
        "device.selection",
        "conversation.watch",
    ];

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
        assert_eq!(
            c("device.selection", json!({"projects": []})),
            Access::Control
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
        // Unknown methods are the host's to refuse.
        let web = ctx(DeviceKind::Web, Permission::Read);
        assert_eq!(call(&web, "cloud.delete", json!({}), &off), None);
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

        let widen = Denial::new(Code::Forbidden).error;
        assert_eq!(
            device_selection(&device, &json!({"projects": null})).unwrap_err(),
            widen
        );
        assert_eq!(
            device_selection(&device, &json!({"projects": ["a", "c"]})).unwrap_err(),
            widen
        );

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
                invalid,
                "{params}"
            );
        }
        // A read device cannot set a selection at all.
        let reader = ctx(DeviceKind::Web, Permission::Read);
        assert_eq!(
            call(&reader, "device.selection", json!({"projects": []}), &ON),
            Some(Code::ReadOnly)
        );
        assert_eq!(call(&reader, "device.selection", json!({}), &ON), None);
    }

    #[test]
    fn capabilities_follow_flags_and_the_approvals_gate() {
        assert!(capabilities_with(&PolicyFlags::default(), false).is_empty());
        assert_eq!(capabilities_with(&ON, false), ["gitPush"]);
        // The gate alone does not list approvals: the desktop toggle is needed too.
        assert!(capabilities_with(&PolicyFlags::default(), true).is_empty());
        let toggle = PolicyFlags {
            remote_approvals: true,
            ..PolicyFlags::default()
        };
        assert!(capabilities_with(&toggle, false).is_empty());
        assert_eq!(capabilities_with(&toggle, true), ["approvals"]);
        assert_eq!(capabilities_with(&ON, true), ["gitPush", "approvals"]);
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
        assert!(!dir.path().join("mobile-policy.json.tmp").exists());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"remoteGitPush\": true") && text.contains("\"denyPatterns\""));
        let empty = serde_json::to_string(&PolicySettings::default()).unwrap();
        assert_eq!(
            empty,
            r#"{"remoteGitPush":false,"remoteCloudStart":false,"remoteApprovals":false,"denyPatterns":{}}"#
        );
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
}
