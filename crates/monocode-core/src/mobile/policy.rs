//! Who may call what: the method permission table as data (architecture.md
//! 7.1), the device context every request carries, and the stubs that keep
//! today's behaviour until the policy is enforced. Nothing here denies a call
//! yet, so QUIC phones see exactly what they saw before.
use super::cloud_policy;
use super::host::Device;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// Desktop toggles that gate parts of the policy. Both default to off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PolicyFlags {
    /// "Remote push in Git".
    pub remote_git_push: bool,
    /// "Remote cloud start".
    pub remote_cloud_start: bool,
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

/// A refused call: the stable key and the text the transport reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Denial {
    pub code: Code,
    pub error: String,
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

/// Checks a call before it runs. Never denies yet, so every device keeps
/// today's access.
pub fn authorize(_device: &DeviceContext, _method: &str, _params: &Value) -> Result<(), Denial> {
    Ok(())
}

/// Shapes a catalog reply (`workspace.list`, `sessions.list`) for `device`.
/// No device has a narrower selection yet, so only the cloud scope applies.
pub fn filter_reply(device: &DeviceContext, _method: &str, reply: &mut Value) {
    cloud_policy::scope_catalog(device, reply);
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

/// Answers `device.selection`. The method does not exist yet, so it answers
/// as an unknown method did.
pub fn device_selection(
    _device: &DeviceContext,
    _params: &Value,
) -> Result<SelectionAnswer, String> {
    Err("Unsupported mobile operation".into())
}

/// The capabilities the desktop advertises. Today's desktop advertises none.
#[allow(dead_code)]
pub fn capabilities(_flags: &PolicyFlags) -> Vec<String> {
    Vec::new()
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

    #[test]
    fn stubs_keep_todays_behaviour() {
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
        let ctx = DeviceContext::phone(&device);
        assert_eq!(ctx.kind, DeviceKind::Phone);
        assert_eq!(ctx.permission, Permission::Control);
        assert!(ctx.projects.is_none() && ctx.monos.is_none());
        assert!(authorize(&ctx, "mono.delete", &json!({})).is_ok());
        let mut reply = json!({"sessions": [1]});
        filter_reply(&ctx, "workspace.list", &mut reply);
        assert_eq!(reply, json!({"sessions": [1]}));
        assert_eq!(
            device_selection(&ctx, &json!({})).unwrap_err(),
            "Unsupported mobile operation"
        );
        assert!(capabilities(&PolicyFlags::default()).is_empty());
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
