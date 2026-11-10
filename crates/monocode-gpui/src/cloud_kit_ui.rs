//! The "Claude setup" row of the cloud start panel and the codespace picker: a summary of the
//! global kit, a link to Settings > Cloud and an on/off switch for this one start (design
//! 3.2.2). The selection itself is only edited in Settings.
//!
//! Only counts reach this row, never item values. The scan runs on a worker.
use crate::{settings::Preferences, theme::*};
use gpui::{App, Div, ElementId, Role, SharedString, Stateful, Task, div, prelude::*, px};
use monocode_core::cloud_sessions::{
    kit::{KitInventory, Workspace},
    kit_prefs::{self, ApiKit, CloudKitPrefs, KitSummary},
};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// What the row knows about the kit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Summary {
    Loading,
    Ready(KitSummary),
    /// The scan failed; the row says so and offers Settings.
    Unavailable,
}

/// The `cloudKitEnabled` flag. Off until the kit can be sent (F3-4).
pub(crate) fn flag_on(cx: &App) -> bool {
    cx.try_global::<Preferences>()
        .is_some_and(|prefs| prefs.cloud_kit_enabled)
}

/// The global kit as saved in Settings.
pub(crate) fn kit_prefs(cx: &App) -> CloudKitPrefs {
    cx.try_global::<Preferences>()
        .map(|prefs| prefs.cloud_kit.clone())
        .unwrap_or_default()
}

/// Whether a start begins with the kit on: the user turned it on and accepted the notice.
pub(crate) fn default_on(prefs: &CloudKitPrefs) -> bool {
    prefs.applies(ApiKit::Default)
}

/// How the inventory is read. The app reads this computer; tests install a fixture.
#[derive(Clone, Copy)]
pub(crate) struct KitScanner(pub fn(&Path, Workspace) -> Result<KitInventory, String>);
impl gpui::Global for KitScanner {}

/// The function that reads the inventory.
pub(crate) fn scanner(cx: &App) -> fn(&Path, Workspace) -> Result<KitInventory, String> {
    cx.try_global::<KitScanner>()
        .map_or(kit_prefs::scan as fn(&Path, Workspace) -> _, |scanner| {
            scanner.0
        })
}

/// How long a scan result serves the panel and the picker without reading the disk again.
const FRESH: Duration = Duration::from_secs(30);

struct Cached {
    project: String,
    workspace: Workspace,
    at: Instant,
    inventory: Arc<KitInventory>,
}

/// Recent scans, so opening a panel or picker does not read every skill again.
#[derive(Clone, Default)]
pub(crate) struct KitCache(Arc<Mutex<Vec<Cached>>>);
impl gpui::Global for KitCache {}

impl KitCache {
    fn get(&self, project: &str, workspace: Workspace) -> Option<Arc<KitInventory>> {
        let entries = self.0.lock().ok()?;
        entries
            .iter()
            .find(|e| e.project == project && e.workspace == workspace && e.at.elapsed() < FRESH)
            .map(|e| e.inventory.clone())
    }
    fn put(&self, project: &str, workspace: Workspace, inventory: Arc<KitInventory>) {
        if let Ok(mut entries) = self.0.lock() {
            entries.retain(|e| {
                e.at.elapsed() < FRESH && !(e.project == project && e.workspace == workspace)
            });
            entries.push(Cached {
                project: project.to_owned(),
                workspace,
                at: Instant::now(),
                inventory,
            });
        }
    }
}

/// Counts what the selection takes from `project` for a `workspace`, off the UI thread. A
/// scan younger than 30 seconds for the same project and workspace is reused; the counts
/// are always recomputed from the current preferences.
pub(crate) fn load_summary(
    project: String,
    workspace: Workspace,
    prefs: CloudKitPrefs,
    cx: &mut App,
) -> Task<Summary> {
    let scan = scanner(cx);
    let cache = cx.default_global::<KitCache>().clone();
    cx.background_executor().spawn(async move {
        let inventory = match cache.get(&project, workspace) {
            Some(inventory) => inventory,
            None => match scan(Path::new(&project), workspace) {
                Ok(inventory) => {
                    let inventory = Arc::new(inventory);
                    // A scan that hit its budget is not worth keeping. A missing hash
                    // key is not: a rescan would not change it.
                    if !inventory.scan_timed_out {
                        cache.put(&project, workspace, inventory.clone());
                    }
                    inventory
                }
                Err(_) => return Summary::Unavailable,
            },
        };
        Summary::Ready(KitSummary::of(&inventory, &prefs))
    })
}

/// The words of the row after `Claude setup: `.
pub(crate) fn summary_text(summary: &Summary, globally_enabled: bool) -> String {
    if !globally_enabled {
        return "off. Turn it on in Settings".into();
    }
    match summary {
        Summary::Loading => "reading...".into(),
        Summary::Ready(counts) => counts.text(),
        Summary::Unavailable => "could not be read".into(),
    }
}

/// A 36x20 switch like the Settings toggle. The caller adds `on_click` when it is enabled.
pub(crate) fn switch(
    id: impl Into<ElementId>,
    selector: &'static str,
    label: &'static str,
    on: bool,
    disabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || selector.into())
        .role(Role::Switch)
        .aria_label(label)
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .relative()
        .h(px(20.))
        .w(px(36.))
        .flex_shrink_0()
        .rounded_full()
        .bg(if on { accent_color() } else { content(0.2) })
        .when(disabled, |d| d.opacity(0.4))
        .when(!disabled, |d| d.cursor_pointer())
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(if on { 18. } else { 2. }))
                .size(px(16.))
                .rounded_full()
                .bg(gpui::rgb(0xffffff)),
        )
}

/// The text and the Settings link of the row; the caller places the switch after it.
pub(crate) fn summary_line(
    id: &'static str,
    summary: &Summary,
    globally_enabled: bool,
) -> Stateful<Div> {
    let text: SharedString =
        format!("Claude setup: {}", summary_text(summary, globally_enabled)).into();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Status)
        .aria_label(text.clone())
        .flex_1()
        .min_w_0()
        .truncate()
        .child(text)
}

/// The link that opens Settings > Cloud.
pub(crate) fn settings_link(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Button)
        .aria_label("Open Cloud settings")
        .flex_shrink_0()
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(content(0.55))
        .hover(|s| s.text_color(content(1.)))
        .child("Settings...")
}

#[cfg(test)]
pub(crate) mod fixture {
    use monocode_core::cloud_sessions::kit::*;
    use std::path::{Path, PathBuf};

    /// Fake values only.
    pub(crate) const FAKE_SECRET: &str = "fake-secret-QQQQ7777";

    fn server(name: &str, compat: McpCompat, secret: bool) -> KitMcpServer {
        KitMcpServer {
            name: name.into(),
            scope: McpScope::User,
            source: KitSource::User,
            config_path: PathBuf::from("/x/.claude.json"),
            transport: "stdio".into(),
            compat,
            secrets: if secret {
                vec![SecretField {
                    location: SecretLocation::Env("API_KEY".into()),
                    value: Secret::new(FAKE_SECRET),
                }]
            } else {
                Vec::new()
            },
            config_hash: "c".into(),
            secrets_digest: None,
        }
    }

    /// 2 skills, 3 selectable servers (one with a secret), 1 local-only server, 1 plugin.
    pub(crate) fn inventory() -> KitInventory {
        let skill = |name: &str| KitSkill {
            name: name.into(),
            scope: SkillScope::User,
            source: KitSource::User,
            plugin: None,
            path: PathBuf::from("/x").join(name),
            size: 2048,
            hash: "h".into(),
            compressed: 100,
            exclusion: None,
        };
        KitInventory {
            skills: vec![skill("review"), skill("deploy")],
            mcp_servers: vec![
                server("fetch", McpCompat::NeedsRuntime(Runtime::Npx), false),
                server("github", McpCompat::Ok, true),
                server("linear", McpCompat::OAuthStyle, false),
                server(
                    "mine",
                    McpCompat::LocalOnly(LocalOnlyReason::AbsoluteCommand),
                    true,
                ),
            ],
            plugins: vec![KitPlugin {
                id: "fmt@market".into(),
                version: "1.0.0".into(),
                source: KitSource::User,
                install_path: PathBuf::from("/x/plugins/fmt"),
                size: 4096,
                has_native_bin: false,
                hash: "h".into(),
                compressed: 200,
                exclusion: None,
            }],
            ..Default::default()
        }
    }

    /// Like the real scan, a stdio `docker` server is local-only on a sandbox.
    pub(crate) fn scan(_: &Path, workspace: Workspace) -> Result<KitInventory, String> {
        let mut inventory = inventory();
        inventory.mcp_servers.push(server(
            "dock",
            match workspace {
                Workspace::Codespace => McpCompat::NeedsRuntime(Runtime::Docker),
                Workspace::Sandbox => McpCompat::LocalOnly(LocalOnlyReason::DockerOnSandbox),
            },
            false,
        ));
        Ok(inventory)
    }
}
