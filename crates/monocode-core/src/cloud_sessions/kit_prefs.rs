//! The user's choice for the global kit: which items of a [`KitInventory`] go to cloud
//! workspaces, whether the kit is on, and whether the one-time notice was acknowledged
//! (design 3.2.2, rules D6 and D11).
//!
//! # No secrets here
//!
//! Everything in this module is safe to show, log and store. A selection holds item ids
//! (`skill:user:review`, `mcp:user:github`, `plugin:name@market`), never values. A
//! [`KitRow`] says that a server *has* secrets ([`Badge::HasSecrets`]) but carries none.
//!
//! # Defaults
//!
//! `selection: None` means "the defaults": every item that can be sent is checked, secrets
//! included and OAuth-style servers carried config-only. The first edit turns the defaults into an explicit list of
//! **user-scope** items, so a user-scope item that appears later is left unchecked until the
//! user ticks it.
//!
//! Project-scoped items (`skill:project:*`, `mcp:local:*`) are never in that list. They
//! belong to one project, and the same name in another project is a different item, so a
//! list frozen from the project open in Settings would be wrong for every other project.
//! They follow their own project: checked whenever they can be sent and the kit is on.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::kit::{
    build_inventory, Exclusion, KitInstructions, KitInventory, KitMcpServer, KitPlugin, KitRoots,
    KitSkill, LocalOnlyReason, McpCompat, McpScope, Runtime, SecretLocation, SkillScope, Workspace,
};

/// Shown once, the first time the kit is enabled. The wording is fixed by design 3.2.2.
pub const NOTICE: &str = "Your MCP secrets will be copied to your cloud workspaces. They will be visible to the agent, to every MCP server and to every command the agent runs there. MCP servers that use OAuth sign-in are not copied with a sign-in: sign in to them in the cloud.";

/// Shown on the Cloud page when the local key for secret digests could not be used.
pub const KEY_UNAVAILABLE_NOTE: &str =
    "Secret change detection is unavailable: the local key could not be used.";

/// The note for this inventory, if any: only when `key_unavailable`.
pub fn key_unavailable_note(inventory: &KitInventory) -> Option<&'static str> {
    inventory.key_unavailable.then_some(KEY_UNAVAILABLE_NOTE)
}

/// Where the app publishes the preferences. The process has one ([`publish`] and
/// [`published`]); a test makes its own so it never touches the shared one.
pub struct PrefsSlot(std::sync::Mutex<Option<CloudKitPrefs>>);

impl PrefsSlot {
    pub const fn new() -> Self {
        Self(std::sync::Mutex::new(None))
    }
    pub fn publish(&self, prefs: CloudKitPrefs) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(prefs);
        }
    }
    /// The last published preferences; the defaults (kit off) before any were published.
    pub fn get(&self) -> CloudKitPrefs {
        self.0
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .unwrap_or_default()
    }
}

impl Default for PrefsSlot {
    fn default() -> Self {
        Self::new()
    }
}

static PUBLISHED: PrefsSlot = PrefsSlot::new();

/// The app publishes the preferences here whenever they change, so code that runs without
/// the UI (a start on a worker, the app API) reads the same choice the user made.
pub fn publish(prefs: CloudKitPrefs) {
    PUBLISHED.publish(prefs);
}

/// The last published preferences; the defaults (kit off) before the app published any.
pub fn published() -> CloudKitPrefs {
    PUBLISHED.get()
}

/// The global preferences entry `cloudKit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CloudKitPrefs {
    /// Checked item ids. `None` = the defaults (everything that can be sent).
    pub selection: Option<BTreeSet<String>>,
    /// The kit is on for new cloud workspaces.
    pub enabled: bool,
    /// The user has seen [`NOTICE`] and accepted it.
    pub notice_acknowledged: bool,
    /// Carry the user's instructions (`CLAUDE.md`, its imports and rules). On by default,
    /// also for preferences saved before this field existed.
    pub instructions: bool,
}

impl Default for CloudKitPrefs {
    fn default() -> Self {
        Self {
            selection: None,
            enabled: false,
            notice_acknowledged: false,
            instructions: true,
        }
    }
}

/// What asking to turn the kit on led to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enable {
    /// The kit is on.
    Enabled,
    /// Show [`NOTICE`] first; call [`CloudKitPrefs::acknowledge_notice`] when accepted.
    NeedsNotice,
}

/// The `kit` option of the app API target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiKit {
    /// The global kit, as set in Settings.
    Default,
    /// No kit for this start.
    None,
}

impl ApiKit {
    /// `"none"` or `"default"`; anything else (an object, a list of items, other text) is
    /// refused. The API can only narrow the user's choice, so it has no way to name items.
    pub fn parse(value: &Value) -> Result<Self, String> {
        match value.as_str() {
            Some("default") => Ok(Self::Default),
            Some("none") => Ok(Self::None),
            _ => Err("target.kit must be \"none\" or \"default\"".into()),
        }
    }
}

impl CloudKitPrefs {
    pub fn needs_notice(&self) -> bool {
        !self.notice_acknowledged
    }

    /// Turn the kit on, unless the notice has to be shown first.
    pub fn request_enable(&mut self) -> Enable {
        if self.notice_acknowledged {
            self.enabled = true;
            Enable::Enabled
        } else {
            Enable::NeedsNotice
        }
    }

    /// The user accepted the notice: remember it and turn the kit on.
    pub fn acknowledge_notice(&mut self) {
        self.notice_acknowledged = true;
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// Whether a start uses the kit: the user turned it on and accepted the notice, and the
    /// API caller did not ask for none. The caller can only narrow this (rule D11).
    pub fn applies(&self, api: ApiKit) -> bool {
        self.enabled && self.notice_acknowledged && api == ApiKit::Default
    }

    /// Whether `id` is checked. An item that cannot be sent is never checked, whatever was
    /// stored.
    pub fn is_selected(&self, id: &str, selectable: bool) -> bool {
        selectable
            && (follows_project(id)
                || match &self.selection {
                    None => true,
                    Some(ids) => ids.contains(id),
                })
    }

    /// Check or uncheck one item. Items that cannot be sent are ignored.
    pub fn set_selected(&mut self, inventory: &KitInventory, id: &str, checked: bool) {
        if follows_project(id) {
            return;
        }
        let items = items(inventory);
        if !items.iter().any(|item| item.id == id && item.selectable) {
            return;
        }
        let ids = self.selection.get_or_insert_with(|| {
            items
                .iter()
                .filter(|item| item.selectable && !follows_project(&item.id))
                .map(|item| item.id.clone())
                .collect()
        });
        if checked {
            ids.insert(id.to_owned());
        } else {
            ids.remove(id);
        }
    }

    /// Back to the defaults.
    pub fn reset_selection(&mut self) {
        self.selection = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KitTab {
    Skills,
    McpServers,
    Plugins,
}

impl KitTab {
    pub const ALL: [KitTab; 3] = [KitTab::Skills, KitTab::McpServers, KitTab::Plugins];
    pub fn id(self) -> &'static str {
        match self {
            Self::Skills => "skills",
            Self::McpServers => "mcp",
            Self::Plugins => "plugins",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Skills => "Skills",
            Self::McpServers => "MCP servers",
            Self::Plugins => "Plugins",
        }
    }
}

/// A badge on a row, in the words of design 3.2.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    Works,
    NeedsRuntime(Runtime),
    LocalOnly,
    HasSecrets,
    /// An OAuth-style http/sse server: carried config-only (D12).
    SignInInCloud,
    /// A project server or skill that arrives with the clone.
    InRepository,
    /// A plugin with `bin/` or native files: it may not run on Linux.
    MayNotRun,
    Excluded,
}

impl Badge {
    pub fn label(self) -> String {
        match self {
            Self::Works => "Works in the cloud".into(),
            Self::NeedsRuntime(runtime) => {
                format!("Needs {} on the instance", runtime_name(runtime))
            }
            Self::LocalOnly => "Local only - will not work".into(),
            Self::HasSecrets => "Has secrets".into(),
            Self::SignInInCloud => "Sign in in the cloud".into(),
            Self::InRepository => "Already in the repository".into(),
            Self::MayNotRun => "May not run on Linux".into(),
            Self::Excluded => "Cannot be sent".into(),
        }
    }
}

fn runtime_name(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Npx => "npx",
        Runtime::Uvx => "uvx",
        Runtime::Bunx => "bunx",
        Runtime::Node => "node",
        Runtime::Python => "python",
        Runtime::Docker => "docker",
    }
}

/// One line of the selection UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitRow {
    pub id: String,
    pub name: String,
    /// Where it comes from: `User`, `Project`, ...
    pub detail: String,
    /// On-disk size, for skills and plugins.
    pub size: Option<u64>,
    pub badges: Vec<Badge>,
    /// False for local-only items and items that cannot be sent: the box cannot be ticked.
    pub selectable: bool,
    /// Follows its own project: always checked when it can be sent, not editable here.
    pub locked: bool,
    pub checked: bool,
    /// Why the item cannot be ticked.
    pub reason: Option<String>,
}

/// An item before the preferences are applied.
struct Item {
    tab: KitTab,
    id: String,
    name: String,
    detail: String,
    size: Option<u64>,
    badges: Vec<Badge>,
    selectable: bool,
    reason: Option<String>,
}

/// Items of one project (`skill:project:*`, `mcp:local:*`). They follow that project.
pub fn follows_project(id: &str) -> bool {
    id.starts_with("skill:project:") || id.starts_with("mcp:local:")
}

pub fn skill_id(skill: &KitSkill) -> String {
    let scope = match skill.scope {
        SkillScope::User => "user",
        SkillScope::ProjectUntracked => "project",
        SkillScope::Plugin => "plugin",
    };
    format!("skill:{scope}:{}", skill.name)
}

pub fn mcp_id(server: &KitMcpServer) -> String {
    let scope = match server.scope {
        McpScope::User => "user",
        McpScope::Local => "local",
        McpScope::Project => "project",
    };
    format!("mcp:{scope}:{}", server.name)
}

pub fn plugin_id(plugin: &KitPlugin) -> String {
    format!("plugin:{}", plugin.id)
}

pub fn exclusion_reason(exclusion: Exclusion) -> &'static str {
    match exclusion {
        Exclusion::TooLarge => "Too large to send",
        Exclusion::LinkOutsideHome => "Links to a file outside your home folder",
        Exclusion::TooManyFiles => "Too many files to send",
        Exclusion::ScanTimedOut => "Reading it took too long",
        Exclusion::LinkLeavesItem => "A link points outside this item",
        Exclusion::UnsafeRoot => "This folder is not a safe place to copy from",
        Exclusion::HardLinkElsewhere => "A file is a hard link to a credentials file",
        Exclusion::SensitiveContent => "It contains a credentials folder",
    }
}

fn local_only_reason(reason: LocalOnlyReason) -> &'static str {
    match reason {
        LocalOnlyReason::AbsoluteCommand => "Runs a program at a path on this computer",
        LocalOnlyReason::PrivateUrl => "Connects to an address only this computer can reach",
        LocalOnlyReason::DockerOnSandbox => "Docker is not available inside a Docker sandbox",
    }
}

fn skill_item(skill: &KitSkill) -> Item {
    let (selectable, badges, reason) = match skill.exclusion {
        Some(exclusion) => (
            false,
            vec![Badge::Excluded],
            Some(exclusion_reason(exclusion).to_owned()),
        ),
        None => (true, vec![Badge::Works], None),
    };
    Item {
        tab: KitTab::Skills,
        id: skill_id(skill),
        name: skill.name.clone(),
        detail: match skill.scope {
            SkillScope::ProjectUntracked => "Project, not committed".into(),
            _ => "Personal".into(),
        },
        size: Some(skill.size),
        badges,
        selectable,
        reason,
    }
}

fn mcp_item(server: &KitMcpServer) -> Item {
    let mut badges = Vec::new();
    let mut selectable = true;
    let mut reason = None;
    match server.compat {
        McpCompat::Ok => badges.push(Badge::Works),
        McpCompat::LocalOnly(why) => {
            badges.push(Badge::LocalOnly);
            selectable = false;
            reason = Some(local_only_reason(why).to_owned());
        }
        McpCompat::NeedsRuntime(runtime) => badges.push(Badge::NeedsRuntime(runtime)),
        McpCompat::OAuthStyle => {
            badges.push(Badge::Works);
            badges.push(Badge::SignInInCloud);
        }
        McpCompat::Project => {
            badges.push(Badge::InRepository);
            selectable = false;
            reason = Some("Arrives with the repository, nothing to send".into());
        }
    }
    if !server.secrets.is_empty() {
        badges.push(Badge::HasSecrets);
    }
    Item {
        tab: KitTab::McpServers,
        id: mcp_id(server),
        name: server.name.clone(),
        detail: format!(
            "{} - {}",
            match server.scope {
                McpScope::User => "Personal",
                McpScope::Local => "This project, local",
                McpScope::Project => "This project",
            },
            server.transport
        ),
        size: None,
        badges,
        selectable,
        reason,
    }
}

fn plugin_item(plugin: &KitPlugin) -> Item {
    let mut badges = Vec::new();
    let mut selectable = true;
    let mut reason = None;
    match plugin.exclusion {
        Some(exclusion) => {
            badges.push(Badge::Excluded);
            selectable = false;
            reason = Some(exclusion_reason(exclusion).to_owned());
        }
        None => badges.push(Badge::Works),
    }
    if plugin.has_native_bin {
        badges.push(Badge::MayNotRun);
    }
    Item {
        tab: KitTab::Plugins,
        id: plugin_id(plugin),
        name: plugin.id.clone(),
        detail: if plugin.version.is_empty() {
            "Plugin".into()
        } else {
            format!("Version {}", plugin.version)
        },
        size: Some(plugin.size),
        badges,
        selectable,
        reason,
    }
}

/// Every offered item. A skill that ships inside a plugin travels with the plugin, so it
/// has no line of its own.
fn items(inventory: &KitInventory) -> Vec<Item> {
    inventory
        .skills
        .iter()
        .filter(|skill| skill.scope != SkillScope::Plugin)
        .map(skill_item)
        .chain(inventory.mcp_servers.iter().map(mcp_item))
        .chain(inventory.plugins.iter().map(plugin_item))
        .collect()
}

/// The inventory the Settings page and the start panel show: the default account of this
/// computer, for `project`. Reads many files: call it from a worker, never from the UI
/// thread.
///
/// `workspace` decides the Docker rule: a stdio `docker` server is local-only on a sandbox.
/// Settings uses [`Workspace::Codespace`], the more permissive view; a start uses the kind it
/// really targets.
pub fn scan(project: &std::path::Path, workspace: Workspace) -> Result<KitInventory, String> {
    let roots = KitRoots::for_profile(&crate::accounts::Profile::default(), project)?;
    Ok(build_inventory(&roots, workspace))
}

/// Compressed size of the checked items, for the 50 MB cap. Items that cannot be sent count
/// for nothing. The flag is false when an item timed out, so the total is only a lower bound.
/// The second value is true when the total is exact.
pub fn selected_compressed(inventory: &KitInventory, prefs: &CloudKitPrefs) -> (u64, bool) {
    let mut total = 0u64;
    let mut exact = true;
    for skill in inventory
        .skills
        .iter()
        .filter(|skill| skill.scope != SkillScope::Plugin)
    {
        if prefs.is_selected(&skill_id(skill), skill.exclusion.is_none()) {
            total += skill.compressed;
        }
    }
    for plugin in &inventory.plugins {
        if prefs.is_selected(&plugin_id(plugin), plugin.exclusion.is_none()) {
            total += plugin.compressed;
        }
    }
    if inventory
        .skills
        .iter()
        .map(|s| s.exclusion)
        .chain(inventory.plugins.iter().map(|p| p.exclusion))
        .any(|e| e == Some(Exclusion::ScanTimedOut))
    {
        exact = false;
    }
    (total, exact)
}

/// The items a start sends: the checked ones that can be sent. A skill that ships inside a
/// plugin travels with the plugin, so it is not listed on its own.
#[derive(Debug, Default)]
pub struct Selected<'a> {
    pub skills: Vec<&'a KitSkill>,
    pub plugins: Vec<&'a KitPlugin>,
    pub servers: Vec<&'a KitMcpServer>,
    /// The user's instructions, when they are on and something can travel.
    pub instructions: Option<&'a KitInstructions>,
}

/// Whether a server can be ticked at all (mirrors the badges of [`mcp_item`]).
fn mcp_selectable(server: &KitMcpServer) -> bool {
    !matches!(server.compat, McpCompat::LocalOnly(_) | McpCompat::Project)
}

/// Applies `prefs` to `inventory`, in inventory order.
pub fn selected<'a>(inventory: &'a KitInventory, prefs: &CloudKitPrefs) -> Selected<'a> {
    Selected {
        skills: inventory
            .skills
            .iter()
            .filter(|skill| skill.scope != SkillScope::Plugin)
            .filter(|skill| prefs.is_selected(&skill_id(skill), skill.exclusion.is_none()))
            .collect(),
        plugins: inventory
            .plugins
            .iter()
            .filter(|plugin| prefs.is_selected(&plugin_id(plugin), plugin.exclusion.is_none()))
            .collect(),
        servers: inventory
            .mcp_servers
            .iter()
            .filter(|server| prefs.is_selected(&mcp_id(server), mcp_selectable(server)))
            .collect(),
        instructions: inventory
            .instructions
            .as_ref()
            .filter(|instructions| prefs.instructions && instructions.is_carried()),
    }
}

/// The rows of one tab, with the preferences applied.
pub fn rows(inventory: &KitInventory, prefs: &CloudKitPrefs, tab: KitTab) -> Vec<KitRow> {
    items(inventory)
        .into_iter()
        .filter(|item| item.tab == tab)
        .map(|item| KitRow {
            checked: prefs.is_selected(&item.id, item.selectable),
            locked: follows_project(&item.id),
            id: item.id,
            name: item.name,
            detail: item.detail,
            size: item.size,
            badges: item.badges,
            selectable: item.selectable,
            reason: item.reason,
        })
        .collect()
}

/// How many items of each kind are checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KitSummary {
    pub skills: usize,
    pub mcp_servers: usize,
    pub plugins: usize,
    /// The user's instructions go along.
    pub instructions: bool,
}

impl KitSummary {
    pub fn of(inventory: &KitInventory, prefs: &CloudKitPrefs) -> Self {
        let mut summary = Self {
            instructions: selected(inventory, prefs).instructions.is_some(),
            ..Self::default()
        };
        for item in items(inventory) {
            if !prefs.is_selected(&item.id, item.selectable) {
                continue;
            }
            match item.tab {
                KitTab::Skills => summary.skills += 1,
                KitTab::McpServers => summary.mcp_servers += 1,
                KitTab::Plugins => summary.plugins += 1,
            }
        }
        summary
    }

    pub fn is_empty(&self) -> bool {
        self.skills + self.mcp_servers + self.plugins == 0 && !self.instructions
    }

    /// `6 skills, 4 MCP servers, 3 plugins, your instructions`.
    pub fn text(&self) -> String {
        if self.is_empty() {
            return "nothing selected".into();
        }
        let part =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = Vec::new();
        if self.skills + self.mcp_servers + self.plugins > 0 {
            parts.push(part(self.skills, "skill", "skills"));
            parts.push(part(self.mcp_servers, "MCP server", "MCP servers"));
            parts.push(part(self.plugins, "plugin", "plugins"));
        }
        if self.instructions {
            parts.push("your instructions".to_owned());
        }
        parts.join(", ")
    }
}

/// `1.5 MB`, `12 KB`, `300 B`.
pub fn size_label(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Names of the secret fields of a server, never their values: `env API_KEY`, `header
/// Authorization`, `URL query`.
pub fn secret_field_names(server: &KitMcpServer) -> Vec<String> {
    server
        .secrets
        .iter()
        .map(|field| match &field.location {
            SecretLocation::Env(name) => format!("env {name}"),
            SecretLocation::Header(name) => format!("header {name}"),
            SecretLocation::UrlQuery => "URL query".into(),
            SecretLocation::UrlUserinfo => "URL user and password".into(),
            SecretLocation::UrlFragment => "URL fragment".into(),
        })
        .collect()
}
