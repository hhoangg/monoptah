//! What the Files and Changes panels are bound to: the local project, or the workspace of a
//! cloud session.
//!
//! The root is resolved from the session in view. A cloud session shows its cloud workspace by
//! default; a per-session switch (in memory, default Cloud) shows the local checkout instead, which
//! is useful before converting a session back. The resolution is pure, so it is tested without a
//! window.
use monocode_core::{
    cloud_sessions::{CloudBackendKind, CloudRef},
    remote::parse_remote_path,
    store::is_pending_cloud_environment,
};

/// Which side of a cloud session the panels show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PanelMode {
    #[default]
    Cloud,
    Local,
}

/// What `ProjectGit`, the Explorer and Changes are bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelRoot {
    /// A local checkout. `cloud` is the workspace of the session in view when the user
    /// switched its panels to Local.
    Local {
        path: String,
        cloud: Option<CloudRef>,
    },
    /// The workspace of a cloud session: `root` is a `remote://env/path` (the session's Host
    /// worktree, else the project key).
    Cloud {
        root: String,
        cloud: CloudRef,
        local_project: String,
    },
    /// The workspace is still being created (a `pending-*` environment). The panels show a
    /// placeholder; `local_project` only keeps `ProjectGit` bound to something harmless, so
    /// nothing is read from a workspace that does not exist yet.
    CloudPending {
        cloud: CloudRef,
        local_project: String,
    },
}

/// The session in view, as far as the panels care.
pub(crate) struct ActiveSession<'a> {
    pub cwd: &'a str,
    pub worktree_cwd: Option<&'a str>,
    /// The workspace the session runs in and its local project; `None` for a local session.
    pub cloud: Option<(CloudRef, &'a str)>,
}

/// Resolves the panel root. `working_copy` is the fallback when no session is in view. A Mono
/// in view is not a cloud session: it never gets the cloud treatment.
pub(crate) fn resolve(
    active: Option<ActiveSession<'_>>,
    mode: PanelMode,
    mono_active: bool,
    working_copy: &str,
) -> PanelRoot {
    let local = |path: &str| PanelRoot::Local {
        path: path.to_owned(),
        cloud: None,
    };
    let Some(session) = active else {
        return local(working_copy);
    };
    let (cloud, local_project) = match session.cloud {
        Some(cloud) if !mono_active => cloud,
        Some(_) => return local(working_copy),
        None => return local(session.worktree_cwd.unwrap_or(session.cwd)),
    };
    match mode {
        PanelMode::Local => PanelRoot::Local {
            path: local_project.to_owned(),
            cloud: Some(cloud),
        },
        PanelMode::Cloud if is_pending(session.cwd) => PanelRoot::CloudPending {
            cloud,
            local_project: local_project.to_owned(),
        },
        PanelMode::Cloud => PanelRoot::Cloud {
            // A cloud session's worktree is a Host path; a local one would bind the wrong side.
            root: session
                .worktree_cwd
                .filter(|cwd| cwd.starts_with("remote://"))
                .unwrap_or(session.cwd)
                .to_owned(),
            cloud,
            local_project: local_project.to_owned(),
        },
    }
}

/// What the panels say while they show the cloud workspace.
pub(crate) fn cloud_banner(cloud: &CloudRef) -> String {
    format!("Cloud files - {} {}", cloud.noun(), cloud.name)
}

fn is_pending(cwd: &str) -> bool {
    parse_remote_path(cwd).is_some_and(|path| is_pending_cloud_environment(&path.environment_id))
}

impl PanelRoot {
    /// The path `ProjectGit` is bound to.
    pub(crate) fn project(&self) -> &str {
        match self {
            Self::Local { path, .. } => path,
            Self::Cloud { root, .. } => root,
            Self::CloudPending { local_project, .. } => local_project,
        }
    }
    /// The workspace of the cloud session in view, in either mode.
    pub(crate) fn cloud(&self) -> Option<&CloudRef> {
        match self {
            Self::Local { cloud, .. } => cloud.as_ref(),
            Self::Cloud { cloud, .. } | Self::CloudPending { cloud, .. } => Some(cloud),
        }
    }
    pub(crate) fn mode(&self) -> PanelMode {
        match self {
            Self::Local { .. } => PanelMode::Local,
            Self::Cloud { .. } | Self::CloudPending { .. } => PanelMode::Cloud,
        }
    }
    /// The panels list a real cloud workspace (not the placeholder).
    #[cfg(test)]
    pub(crate) fn is_cloud_workspace(&self) -> bool {
        matches!(self, Self::Cloud { .. })
    }
    pub(crate) fn is_pending(&self) -> bool {
        matches!(self, Self::CloudPending { .. })
    }
    /// A cloud session is in view but its panels show the local checkout, or nothing yet:
    /// writes to the local checkout stay refused (they are not the session's files).
    #[cfg(test)]
    pub(crate) fn refuses_writes(&self) -> bool {
        matches!(
            self,
            Self::Local { cloud: Some(_), .. } | Self::CloudPending { .. }
        )
    }
    /// The line above the panels while a cloud session is in view.
    pub(crate) fn banner(&self) -> Option<String> {
        match self {
            Self::Local { cloud: None, .. } => None,
            Self::Local {
                cloud: Some(cloud), ..
            } => Some(format!(
                "Local files - this session runs in the {} {}",
                cloud.noun(),
                cloud.name
            )),
            Self::Cloud { cloud, .. } | Self::CloudPending { cloud, .. } => {
                Some(cloud_banner(cloud))
            }
        }
    }
    /// Whether the Explorer offers "Open in browser": a codespace has a VS Code for the web,
    /// a sandbox has none.
    pub(crate) fn offers_browser(&self) -> bool {
        matches!(
            self,
            Self::Cloud { cloud, .. } if cloud.kind == CloudBackendKind::Codespaces
        )
    }
}

/// Whether the app has a live connection to the workspace of the session in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Link {
    Connected,
    NotConnected,
    /// The workspace is being created, woken or reconnected.
    Starting,
}

impl Link {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::NotConnected => "Not connected",
            Self::Starting => "Workspace starting",
        }
    }
}

/// The panels show a real cloud workspace the app has no live connection to. They then show the
/// last data dimmed and read-only, read nothing, and never wake the workspace. A local root, the
/// Local switch and a workspace still being created are never offline.
pub(crate) fn is_offline(root: &PanelRoot, link: Link) -> bool {
    matches!(root, PanelRoot::Cloud { .. }) && link != Link::Connected
}

/// Everything the right panel draws above its tabs for a cloud session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chrome {
    pub banner: String,
    pub mode: PanelMode,
    /// The connection strip; only in Cloud mode.
    pub link: Option<Link>,
    /// Show a placeholder instead of the tree: the workspace does not exist yet.
    pub placeholder: bool,
    pub browser: bool,
    /// The strip may offer Connect: the Connect action is the session's Reconnect, so it needs
    /// a live conversation (a session that runs in a terminal has none).
    pub can_connect: bool,
    /// The panels show remembered data: dimmed, read-only, and not polled.
    pub offline: bool,
    /// How old the remembered data is, e.g. "last updated 5 min ago".
    pub cached: Option<String>,
}

impl Chrome {
    pub(crate) fn of(root: &PanelRoot, link: Link, can_connect: bool) -> Option<Self> {
        Some(Self {
            banner: root.banner()?,
            mode: root.mode(),
            link: (root.mode() == PanelMode::Cloud).then_some(if root.is_pending() {
                Link::Starting
            } else {
                link
            }),
            placeholder: root.is_pending(),
            browser: root.offers_browser(),
            can_connect,
            offline: is_offline(root, link),
            cached: None,
        })
    }
    /// Says how old the remembered data is; only shown while it is the data on screen.
    pub(crate) fn with_age(mut self, age: Option<std::time::Duration>) -> Self {
        self.cached = age
            .filter(|_| self.offline)
            .map(crate::remote_cache::age_hint);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "remote://env-octo-abc/workspaces/widgets";
    const WORKTREE: &str = "remote://env-octo-abc/workspaces/widgets-wt";
    const LOCAL: &str = "/work/monocode";

    fn cloud() -> CloudRef {
        CloudRef::codespace("octo-abc")
    }
    fn session<'a>(cwd: &'a str, worktree: Option<&'a str>) -> Option<ActiveSession<'a>> {
        Some(ActiveSession {
            cwd,
            worktree_cwd: worktree,
            cloud: Some((cloud(), LOCAL)),
        })
    }

    #[test]
    fn a_cloud_session_binds_its_project_key_by_default() {
        let root = resolve(
            session(PROJECT, None),
            PanelMode::default(),
            false,
            "/other",
        );
        assert_eq!(
            root,
            PanelRoot::Cloud {
                root: PROJECT.into(),
                cloud: cloud(),
                local_project: LOCAL.into()
            }
        );
        assert_eq!(root.project(), PROJECT);
        assert_eq!(PanelMode::default(), PanelMode::Cloud);
    }

    #[test]
    fn a_host_worktree_replaces_the_project_key() {
        let root = resolve(
            session(PROJECT, Some(WORKTREE)),
            PanelMode::Cloud,
            false,
            "/o",
        );
        assert_eq!(root.project(), WORKTREE);
        // A worktree path that is not a Host path never binds the cloud panels to the local disk.
        let root = resolve(
            session(PROJECT, Some("/tmp/wt")),
            PanelMode::Cloud,
            false,
            "/o",
        );
        assert_eq!(root.project(), PROJECT);
    }

    #[test]
    fn a_pending_workspace_is_a_placeholder_bound_to_the_local_project() {
        let pending = "remote://pending-octo-abc/workspaces/widgets";
        let root = resolve(session(pending, None), PanelMode::Cloud, false, "/o");
        assert_eq!(
            root,
            PanelRoot::CloudPending {
                cloud: cloud(),
                local_project: LOCAL.into()
            }
        );
        assert_eq!(root.project(), LOCAL);
        assert!(root.is_pending() && !root.is_cloud_workspace());
        assert!(root.refuses_writes());
    }

    #[test]
    fn the_local_switch_shows_the_local_checkout_and_remembers_the_cloud() {
        let root = resolve(
            session(PROJECT, Some(WORKTREE)),
            PanelMode::Local,
            false,
            "/o",
        );
        assert_eq!(
            root,
            PanelRoot::Local {
                path: LOCAL.into(),
                cloud: Some(cloud())
            }
        );
        assert_eq!(root.mode(), PanelMode::Local);
        assert!(root.refuses_writes());
        assert_eq!(
            root.banner().as_deref(),
            Some("Local files - this session runs in the codespace octo-abc")
        );
        // The switch does not touch a pending workspace's mode: Local is still the local checkout.
        let pending = "remote://pending-octo-abc/workspaces/widgets";
        let root = resolve(session(pending, None), PanelMode::Local, false, "/o");
        assert_eq!(root.project(), LOCAL);
    }

    #[test]
    fn a_local_session_and_no_session_are_plain_local_roots() {
        let local = Some(ActiveSession {
            cwd: LOCAL,
            worktree_cwd: Some("/work/wt"),
            cloud: None,
        });
        let root = resolve(local, PanelMode::Cloud, false, "/o");
        assert_eq!(
            root,
            PanelRoot::Local {
                path: "/work/wt".into(),
                cloud: None
            }
        );
        assert!(!root.refuses_writes());
        assert_eq!(root.banner(), None);
        let none = resolve(None, PanelMode::Cloud, false, "/work/focus");
        assert_eq!(none.project(), "/work/focus");
    }

    #[test]
    fn a_mono_in_view_is_never_a_cloud_root() {
        let root = resolve(
            session(PROJECT, None),
            PanelMode::Cloud,
            true,
            "/work/focus",
        );
        assert_eq!(
            root,
            PanelRoot::Local {
                path: "/work/focus".into(),
                cloud: None
            }
        );
        assert_eq!(root.banner(), None);
        assert!(!root.refuses_writes());
    }

    #[test]
    fn only_a_real_cloud_workspace_without_a_connection_is_offline() {
        let cloud_root = resolve(session(PROJECT, None), PanelMode::Cloud, false, "/o");
        assert!(!is_offline(&cloud_root, Link::Connected));
        assert!(is_offline(&cloud_root, Link::NotConnected));
        // A workspace that is waking is not live yet either.
        assert!(is_offline(&cloud_root, Link::Starting));
        let local_root = resolve(session(PROJECT, None), PanelMode::Local, false, "/o");
        assert!(!is_offline(&local_root, Link::NotConnected));
        let pending = "remote://pending-octo-abc/workspaces/widgets";
        let pending_root = resolve(session(pending, None), PanelMode::Cloud, false, "/o");
        assert!(!is_offline(&pending_root, Link::NotConnected));
        let plain = resolve(None, PanelMode::Cloud, false, "/o");
        assert!(!is_offline(&plain, Link::NotConnected));

        let chrome = Chrome::of(&cloud_root, Link::NotConnected, true)
            .unwrap()
            .with_age(Some(std::time::Duration::from_secs(120)));
        assert!(chrome.offline);
        assert_eq!(chrome.cached.as_deref(), Some("last updated 2 min ago"));
        // Live data carries no age.
        let live = Chrome::of(&cloud_root, Link::Connected, true)
            .unwrap()
            .with_age(Some(std::time::Duration::from_secs(120)));
        assert!(!live.offline && live.cached.is_none());
    }

    #[test]
    fn the_chrome_follows_the_mode_and_the_backend() {
        let cloud_root = resolve(session(PROJECT, None), PanelMode::Cloud, false, "/o");
        let chrome = Chrome::of(&cloud_root, Link::NotConnected, true).unwrap();
        assert_eq!(chrome.banner, "Cloud files - codespace octo-abc");
        assert_eq!(chrome.link, Some(Link::NotConnected));
        assert!(chrome.browser && !chrome.placeholder && chrome.can_connect);
        // Without a live conversation there is nothing to reconnect with: no Connect action.
        let terminal = Chrome::of(&cloud_root, Link::NotConnected, false).unwrap();
        assert_eq!(terminal.link, Some(Link::NotConnected));
        assert!(!terminal.can_connect);

        let local_root = resolve(session(PROJECT, None), PanelMode::Local, false, "/o");
        let chrome = Chrome::of(&local_root, Link::Connected, true).unwrap();
        assert_eq!((chrome.link, chrome.browser), (None, false));

        let pending = "remote://pending-octo-abc/workspaces/widgets";
        let pending_root = resolve(session(pending, None), PanelMode::Cloud, false, "/o");
        let chrome = Chrome::of(&pending_root, Link::Connected, true).unwrap();
        assert_eq!(chrome.link, Some(Link::Starting));
        assert!(chrome.placeholder);

        let sandbox = Some(ActiveSession {
            cwd: PROJECT,
            worktree_cwd: None,
            cloud: Some((CloudRef::sandbox("box-1"), LOCAL)),
        });
        let sandbox_root = resolve(sandbox, PanelMode::Cloud, false, "/o");
        assert_eq!(
            sandbox_root.banner().as_deref(),
            Some("Cloud files - sandbox box-1")
        );
        assert!(!sandbox_root.offers_browser());

        let plain = resolve(None, PanelMode::Cloud, false, "/o");
        assert_eq!(Chrome::of(&plain, Link::Connected, true), None);
    }
}
