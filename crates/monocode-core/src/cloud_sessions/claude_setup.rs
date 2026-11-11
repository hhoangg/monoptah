//! The "Claude setup" start step: sends the user's Claude kit (skills, plugins, MCP servers)
//! to a NEW cloud workspace, one way, once, before `Ready` (design 3.2.4, rules D6, D7, D11, D12).
//!
//! # Flow
//!
//! 1. `kit.probe`: which of `npx uvx bunx node python3 docker` the workspace has.
//! 2. [`super::kit_pack::pack`]: the tar for the selection. A server whose runtime is missing
//!    is dropped with a warning.
//! 3. The uploads client (`uploads.*`), then `kit.apply`, then `kit.secrets`.
//! 4. `kit.health`: per-server status, parsed here ([`ServerHealth`]); the result is stored
//!    ([`SetupState`]) for the cloud panel.
//!
//! # Never fails the start
//!
//! [`run`] returns a [`SetupState`], never an error. Every problem becomes a warning in the
//! state and a progress line; a panic is caught and becomes one too.
//!
//! # When it runs
//!
//! Only for a start that created its workspace ([`CloudSessions::setup_plan`]), and only when
//! the global kit is on and acknowledged (`CloudKitPrefs::applies`) and the start did not turn
//! it off (`StartRecord::kit_off`, set by the per-start switch or the app API's `kit: "none"`).
//! Nothing here runs on a reconnect or an attach, and a retry of a start whose step finished
//! does not send again. The one other entry is [`CloudSessions::update_claude_setup`], the
//! user's "Update Claude setup" action.
//!
//! # Secrets
//!
//! They are read from the local config only inside [`run`], wiped when it returns, and every
//! string leaving it (progress lines, warnings, Host error text, stored health) passes
//! [`Scrub`]. OAuth-style servers are carried config-only and listed as "Sign in in the
//! cloud" (D12); nothing is read from the Keychain or a credentials file.
//!
//! # Unapproved project MCP servers (design 7)
//!
//! `claude mcp list` reports a project `.mcp.json` server that was never approved as
//! "pending approval". The Host maps that to `signInNeeded`, which is not a failure, and
//! the list shows it as "Needs approval". Whether a HEADLESS (stream-json) session loads such
//! a server is NOT observable from `mcp list` and must be confirmed by hand on a real
//! workspace; the warning says so and nothing is approved on the user's behalf.

use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::backend::{CloudBackendKind, CloudRef};
use super::kit::{self, KitInventory, KitRoots, SecretLocation, Workspace};
use super::kit_pack::{self, Packed, Present};
use super::kit_prefs::{self, ApiKit, CloudKitPrefs};
use super::{CloudSessions, Host, StartRecord, StepKind, StepStatus};
use crate::accounts::Profile;
use crate::provider::Kind;
use crate::remote::uploads::{upload_file, UploadOptions};
use crate::store::Store;

/// The Host capability for `kit.*`.
pub const CAPABILITY: &str = "claude.kit";
/// The Host capability for `uploads.*`.
pub const UPLOADS_CAPABILITY: &str = "uploads";
/// Store setting holding the JSON map `CloudRef::key() -> SetupState`.
const SETTING: &str = "cloud-claude-setup.v1";
const UPLOAD_NAME: &str = "kit.tar";
const MAX_WARNINGS: usize = 40;

/// Text of the row that carries a server the user has to sign in to in the cloud (D12).
pub const SIGN_IN_LABEL: &str = "Sign in in the cloud";
/// Text for a project server that waits for approval (design 7).
pub const APPROVAL_LABEL: &str = "Needs approval";

/// Finds the local setup of a project.
pub type RootsFn = Arc<dyn Fn(&Path, &Profile) -> Result<KitRoots, String> + Send + Sync>;

/// The seams the step reads the user's setup through. Tests install fixtures.
#[derive(Clone)]
pub struct KitSeams {
    /// The global kit preferences as set in Settings.
    pub prefs: Arc<dyn Fn() -> CloudKitPrefs + Send + Sync>,
    /// Where the local setup lives, for a local project and the Claude account it uses.
    pub roots: RootsFn,
}

impl KitSeams {
    /// The real preferences ([`kit_prefs::published`]) and the Claude account's own config.
    pub fn system() -> Self {
        Self {
            prefs: Arc::new(kit_prefs::published),
            roots: Arc::new(|project, profile| KitRoots::for_profile(profile, project)),
        }
    }
}

/// An opaque change marker. It never prints.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthStatus {
    Connected,
    Failed,
    SignInNeeded,
    Disabled,
    Configured,
}

impl HealthStatus {
    fn parse(text: &str) -> Self {
        match text {
            "connected" => Self::Connected,
            "failed" => Self::Failed,
            "signInNeeded" => Self::SignInNeeded,
            "disabled" => Self::Disabled,
            _ => Self::Configured,
        }
    }
}

/// What the health check says about one MCP server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerHealth {
    pub name: String,
    pub status: HealthStatus,
    pub sign_in_needed: bool,
    pub transport: String,
    /// A short generic reason from a fixed list ("command not found", "could not connect"),
    /// never the Host's text: a status line can carry a command, a URL or an argument.
    #[serde(default)]
    pub reason: String,
    /// A project server that waits for approval (design 7).
    #[serde(default)]
    pub needs_approval: bool,
    /// The kit sent this server config-only (D12).
    #[serde(default)]
    pub oauth_style: bool,
}

impl ServerHealth {
    /// The word the cloud panel shows: `connected`, `failed - <why>`, `Sign in in the cloud`...
    pub fn label(&self) -> String {
        match self.status {
            HealthStatus::Connected => "connected".into(),
            HealthStatus::SignInNeeded if self.needs_approval => APPROVAL_LABEL.into(),
            HealthStatus::SignInNeeded => SIGN_IN_LABEL.into(),
            HealthStatus::Failed if self.reason.is_empty() => "failed".into(),
            HealthStatus::Failed => format!("failed - {}", self.reason),
            HealthStatus::Disabled => "disabled".into(),
            HealthStatus::Configured => "not checked".into(),
        }
    }
    /// An error the user may need to act on (not a sign-in, which is expected).
    pub fn is_problem(&self) -> bool {
        self.status == HealthStatus::Failed
    }
}

/// The result of the last Claude setup of one workspace, kept for the cloud panel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupState {
    /// Unix milliseconds.
    pub at_ms: u64,
    /// A kit was applied on the workspace.
    pub applied: bool,
    /// `4 skills, 2 MCP servers`.
    pub summary: String,
    /// Marks the local setup the kit was made from; see [`local_changed`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<Token>,
    pub health: Vec<ServerHealth>,
    pub warnings: Vec<String>,
    /// The last "Update Claude setup" failed: the rest of this state is the last GOOD result,
    /// kept as it was, and this says why the update did not go through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

impl SetupState {
    fn new(at_ms: u64) -> Self {
        Self {
            at_ms,
            applied: false,
            summary: String::new(),
            token: None,
            health: Vec::new(),
            warnings: Vec::new(),
            failure: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Scrubbing
// ---------------------------------------------------------------------------

/// Removes secret values (and the parts of a URL secret) from text.
///
/// It learns values twice: from the scanned inventory, and again from the values that are
/// really packed, because a config can change between the scan and the pack. A value shorter
/// than [`MIN_SCRUB`] cannot be masked without mangling ordinary text, so it makes the scrub
/// opaque: text that came from the Host is then replaced by a fixed sentence
/// ([`Scrub::host_text`]) instead of being trusted.
#[derive(Default)]
pub struct Scrub {
    forms: std::cell::RefCell<Vec<Zeroizing<String>>>,
    opaque: std::cell::Cell<bool>,
}

/// Values shorter than this are not scrubbed: they would mangle ordinary text.
const MIN_SCRUB: usize = 4;
/// Shown in place of Host text while a secret too short to mask is in play.
const HIDDEN: &str = "(details hidden: a secret is too short to mask)";

impl Scrub {
    fn from_inventory(inventory: &KitInventory, prefs: &CloudKitPrefs) -> Self {
        let scrub = Self::default();
        for server in kit_prefs::selected(inventory, prefs).servers {
            for field in &server.secrets {
                let url = matches!(
                    field.location,
                    SecretLocation::UrlQuery
                        | SecretLocation::UrlUserinfo
                        | SecretLocation::UrlFragment
                );
                scrub.add_value(field.value.expose(), url);
            }
        }
        scrub
    }

    /// Learns the values that are actually about to be sent.
    fn learn_packed(&self, packed: &Packed) {
        for (name, value) in &packed.secrets {
            self.add_value(value.as_str(), packed.url_secrets.contains(name));
        }
        for value in &packed.scrub_extra {
            self.add_value(value.as_str(), false);
        }
    }

    fn add_value(&self, value: &str, url: bool) {
        if value.is_empty() {
            return;
        }
        if value.len() < MIN_SCRUB {
            self.opaque.set(true);
        }
        let mut found: BTreeSet<String> = BTreeSet::new();
        let mut add = |text: &str| {
            if text.len() >= MIN_SCRUB {
                found.insert(text.to_owned());
            }
        };
        add(value);
        add(Value::String(value.to_owned())
            .to_string()
            .trim_matches('"'));
        if url {
            if let Ok(parsed) = url::Url::parse(value) {
                add(parsed.query().unwrap_or(""));
                // A query value on its own is what a tool prints back.
                for (_, part) in parsed.query_pairs() {
                    add(&part);
                }
                add(parsed.fragment().unwrap_or(""));
                add(parsed.username());
                add(parsed.password().unwrap_or(""));
            }
        }
        for line in value.lines().map(str::trim).filter(|l| l.len() >= 8) {
            add(line);
        }
        let mut forms = self.forms.borrow_mut();
        forms.extend(found.into_iter().map(Zeroizing::new));
        // Longest first, so a secret that contains another one is removed whole.
        forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        forms.dedup();
    }

    #[cfg(test)]
    pub(super) fn from_values_for_test(values: &[&str]) -> Self {
        let scrub = Self::default();
        for value in values {
            scrub.add_value(value, false);
        }
        scrub
    }

    /// `text` without any secret.
    pub fn apply(&self, text: &str) -> String {
        let mut out = crate::sandboxes::redact(text);
        for form in self.forms.borrow().iter() {
            out = out.replace(form.as_str(), "[redacted]");
        }
        out
    }

    /// Text that came from the Host (errors, warnings, status lines). Scrubbed like any text,
    /// and withheld entirely when a secret is too short to be masked reliably.
    pub fn host_text(&self, text: &str) -> String {
        if self.opaque.get() && !text.trim().is_empty() {
            HIDDEN.to_owned()
        } else {
            self.apply(text)
        }
    }
}

// ---------------------------------------------------------------------------
// The step
// ---------------------------------------------------------------------------

/// What a start (or an update) needs to run the step.
#[derive(Clone)]
pub struct SetupPlan {
    pub seams: KitSeams,
    /// The preferences as read when the plan was made.
    pub prefs: CloudKitPrefs,
    pub workspace: Workspace,
    /// "Update Claude setup" replaces the secrets file even when there are none now, and
    /// sends an empty kit when nothing is selected, so a revoked item is removed.
    pub replace_secrets: bool,
    /// The Claude account whose setup is read (a sandbox start names one); else the account
    /// the project uses.
    pub account: Option<String>,
}

pub(super) struct Env<'a> {
    pub host: &'a dyn Host,
    pub store: &'a Arc<Mutex<Store>>,
    pub halt: &'a AtomicBool,
    pub now: u64,
}

pub(super) struct Target<'a> {
    pub local_project: &'a str,
    pub cloud: &'a CloudRef,
    pub machine_id: &'a str,
    pub project_id: Option<&'a str>,
}

/// How long each Host call may take. The Host allows `kit.health` 60 s plus a grace of 2 s,
/// and a large kit takes a while to unpack. Everything else keeps the default limit.
pub(super) fn timeout_for(method: &str) -> Option<Duration> {
    match method {
        "kit.health" => Some(Duration::from_secs(75)),
        "kit.apply" => Some(Duration::from_secs(120)),
        "kit.secrets" => Some(Duration::from_secs(30)),
        _ => None,
    }
}

/// The Claude account whose setup is read: `account` when it is a known account, else the one
/// the project uses.
fn profile_for(store: &Arc<Mutex<Store>>, project: &str, account: Option<&str>) -> Profile {
    let Ok(store) = store.lock() else {
        return Profile::default();
    };
    let book = store.provider_accounts().unwrap_or_default();
    let id = match account {
        Some(account) if book.exists(Kind::Claude, account) => account.to_owned(),
        _ => book.selected(Kind::Claude, project),
    };
    let root = store
        .data_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    Profile::new(&id, &root)
}

/// Runs the step and stores its result. Never fails and never panics out.
///
/// A halted run (cancelled, or the start or workspace was deleted) stores nothing, so a state
/// that `forget_cloud_starts` removed is not brought back. A failed "Update Claude setup" keeps
/// the last good result and records the failure next to it.
pub(super) fn run(
    env: &Env<'_>,
    target: &Target<'_>,
    plan: &SetupPlan,
    progress: &mut dyn FnMut(&str),
) -> SetupState {
    let outcome =
        std::panic::catch_unwind(AssertUnwindSafe(|| run_inner(env, target, plan, progress)));
    let state = outcome.unwrap_or_else(|_| {
        let mut state = SetupState::new(env.now);
        state
            .warnings
            .push("The Claude setup stopped unexpectedly. Your workspace works without it.".into());
        state
    });
    if env.halt.load(Ordering::Relaxed) {
        return state;
    }
    if let Ok(store) = env.store.lock() {
        if plan.replace_secrets && !state.applied {
            if let Some(mut good) = setup_state(&store, target.cloud).filter(|p| p.applied) {
                let why: Vec<&str> = state.warnings.iter().take(3).map(String::as_str).collect();
                good.failure = Some(if why.is_empty() {
                    "The update did not go through.".to_owned()
                } else {
                    why.join(" ")
                });
                let _ = save_state(&store, target.cloud, &good);
                return good;
            }
        }
        let _ = save_state(&store, target.cloud, &state);
    }
    state
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn run_inner(
    env: &Env<'_>,
    target: &Target<'_>,
    plan: &SetupPlan,
    progress: &mut dyn FnMut(&str),
) -> SetupState {
    let mut state = SetupState::new(env.now);
    let warn =
        |state: &mut SetupState, scrub: &Scrub, text: &str, progress: &mut dyn FnMut(&str)| {
            let text = scrub.apply(text);
            progress(&text);
            if state.warnings.len() < MAX_WARNINGS {
                state.warnings.push(text);
            }
        };
    // A progress line first (it is also where a cancelled or deleted start is noticed), then
    // the check.
    let halted = || env.halt.load(Ordering::Relaxed);
    let none = Scrub::default();
    progress("Preparing your Claude setup");
    if halted() {
        return state;
    }
    let profile = profile_for(env.store, target.local_project, plan.account.as_deref());
    let roots = match (plan.seams.roots)(Path::new(target.local_project), &profile) {
        Ok(roots) => roots,
        Err(error) => {
            warn(
                &mut state,
                &none,
                &format!("Could not read your Claude setup: {error}"),
                progress,
            );
            return state;
        }
    };
    let inventory = kit::build_inventory(&roots, plan.workspace);
    // Secrets are in the inventory from here on: nothing leaves without the scrub.
    let scrub = Scrub::from_inventory(&inventory, &plan.prefs);
    state.token = Some(Token(kit_pack::selection_token(&inventory, &plan.prefs)));
    let request = |method: &str, params: Value| -> Result<Value, String> {
        env.host
            .request_timeout(target.machine_id, method, params, timeout_for(method))
            .map_err(|e| scrub.host_text(&e))
    };

    // The Host must know the kit and uploads; an older one is a warning, not a failure.
    if let Ok(info) = env.host.host_info(target.machine_id) {
        if !info.supports(CAPABILITY) || !info.supports(UPLOADS_CAPABILITY) {
            warn(
                &mut state,
                &scrub,
                "This workspace's Host is too old for the Claude setup, so nothing was sent.",
                progress,
            );
            return state;
        }
    }

    progress("Checking which tools the workspace has");
    if halted() {
        return state;
    }
    let present = match request("kit.probe", json!({ "commands": kit_pack::PROBE_COMMANDS })) {
        Ok(reply) => reply
            .get("present")
            .and_then(Value::as_array)
            .map(|list| Present::known(list.iter().filter_map(|c| c.as_str().map(str::to_owned))))
            .unwrap_or_else(Present::unknown),
        Err(error) => {
            warn(
                &mut state,
                &scrub,
                &format!("Could not check the workspace's tools ({error}); MCP servers were sent without that check."),
                progress,
            );
            Present::unknown()
        }
    };

    let packed = match kit_pack::pack(&inventory, &roots, &plan.prefs, &present) {
        Ok(packed) => packed,
        Err(error) => {
            warn(
                &mut state,
                &scrub,
                &format!("Could not build the kit: {error}"),
                progress,
            );
            return state;
        }
    };
    // The values actually sent, which may differ from the scanned ones.
    scrub.learn_packed(&packed);
    for line in &packed.warnings {
        warn(&mut state, &scrub, line, progress);
    }
    if packed.is_empty() && !plan.replace_secrets {
        progress("Nothing to send for the Claude setup");
        state.summary = "nothing selected".into();
        return state;
    }
    state.summary = if packed.is_empty() {
        // An update with nothing selected still goes through: an empty kit and an empty
        // secrets file replace the old ones, so what the user took out is removed.
        "nothing selected: the earlier setup was removed".into()
    } else {
        summary_of(&packed)
    };

    progress(&format!(
        "Uploading the Claude setup ({})",
        kit_prefs::size_label(packed.size)
    ));
    if halted() {
        return state;
    }
    let mut last = 0u64;
    let mut on_progress = |done: u64, total: u64| {
        let percent = (done * 100).checked_div(total).unwrap_or(100);
        if percent >= last + 25 {
            last = percent;
            progress(&format!("Uploading the Claude setup: {percent}%"));
        }
    };
    let uploaded = upload_file(
        &mut |method, params| request(method, params),
        packed.file.path(),
        UPLOAD_NAME,
        UploadOptions {
            resume_id: None,
            progress: Some(&mut on_progress),
            cancel: Some(env.halt),
        },
    );
    let uploaded = match uploaded {
        Ok(uploaded) => uploaded,
        Err(error) => {
            warn(
                &mut state,
                &scrub,
                &format!("Could not upload the Claude setup: {error}"),
                progress,
            );
            return state;
        }
    };

    progress("Installing the Claude setup");
    if halted() {
        let _ = request("uploads.discard", json!({ "uploadId": uploaded.upload_id }));
        return state;
    }
    let applied = request(
        "kit.apply",
        json!({ "uploadId": uploaded.upload_id, "hash": packed.sha256 }),
    );
    // The Host keeps a committed upload for an hour unless it is discarded.
    let _ = request("uploads.discard", json!({ "uploadId": uploaded.upload_id }));
    match applied {
        Ok(reply) => {
            state.applied = true;
            for line in reply
                .get("warnings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                warn(&mut state, &scrub, &scrub.host_text(line), progress);
            }
        }
        Err(error) => {
            warn(
                &mut state,
                &scrub,
                &format!("The workspace could not install the Claude setup: {error}"),
                progress,
            );
            return state;
        }
    }

    if !packed.secrets.is_empty() || plan.replace_secrets {
        progress("Storing MCP secrets on the workspace");
        if halted() {
            return state;
        }
        if let Err(error) = request("kit.secrets", json!({ "values": packed.secrets_value() })) {
            warn(
                &mut state,
                &scrub,
                &format!("Could not store the MCP secrets: {error}. Servers that need them will not connect."),
                progress,
            );
        }
    }

    progress("Checking MCP servers");
    if halted() {
        return state;
    }
    let mut params = json!({});
    if let Some(project) = target.project_id {
        params["projectId"] = json!(project);
    }
    match request("kit.health", params) {
        Ok(reply) => {
            let (health, notes) = parse_health(&reply, &scrub);
            state.health = merge_health(health, &packed);
            for note in notes {
                warn(&mut state, &scrub, &note, progress);
            }
        }
        Err(error) => {
            warn(
                &mut state,
                &scrub,
                &format!("Could not check the MCP servers: {error}"),
                progress,
            );
            state.health = merge_health(Vec::new(), &packed);
        }
    }
    for line in health_warnings(&state.health, &packed, plan.workspace) {
        warn(&mut state, &scrub, &line, progress);
    }
    for entry in &state.health {
        progress(&format!("{}: {}", entry.name, entry.label()));
    }
    state
}

fn summary_of(packed: &Packed) -> String {
    let mut parts = Vec::new();
    if !packed.skills.is_empty() {
        parts.push(plural(packed.skills.len(), "skill", "skills"));
    }
    if !packed.servers.is_empty() {
        parts.push(plural(packed.servers.len(), "MCP server", "MCP servers"));
    }
    if !packed.plugins.is_empty() {
        parts.push(plural(packed.plugins.len(), "plugin", "plugins"));
    }
    parts.join(", ")
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// The part of a `claude mcp list` line that is the STATUS: what follows the last " - " (the
/// whole text when there is none, as a fixed Host returns it). Only keywords are read from it.
/// The text before the separator is the server's command, arguments or URL, which can hold a credential. A
/// Host from before the fix returns the whole line, so the app cuts it here as well.
fn status_part(line: &str) -> &str {
    line.rsplit_once(" - ").map_or(line, |(_, status)| status)
}

/// Whether the status says the server waits for approval.
fn is_approval(status: &str) -> bool {
    status.to_lowercase().contains("pending approval")
}

/// A generic reason for a failure, chosen from a fixed list by keywords of the status. The
/// status text itself is never kept.
fn reason_of(status: &str) -> String {
    let lower = status.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["not found", "enoent"]) {
        "command not found"
    } else if has(&["timed out", "timeout"]) {
        "timed out"
    } else if has(&["401", "403", "unauthorized", "forbidden"]) {
        "not authorized"
    } else if has(&[
        "refused",
        "enotfound",
        "dns",
        "could not connect",
        "failed to connect",
    ]) {
        "could not connect"
    } else {
        ""
    }
    .into()
}

/// Reads a `kit.health` reply: the Host's own per-server list, else the raw `claude mcp
/// list` text through [`crate::mcp::parse_claude_health`]. Returns the servers and notes.
pub fn parse_health(reply: &Value, scrub: &Scrub) -> (Vec<ServerHealth>, Vec<String>) {
    let mut notes = Vec::new();
    let mut servers: Vec<ServerHealth> = reply
        .get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_owned();
            let status = HealthStatus::parse(entry.get("status")?.as_str()?);
            // Only keywords are read from it, through `scrub` first; it is never stored.
            let detail = scrub.host_text(entry.get("detail").and_then(Value::as_str).unwrap_or(""));
            Some(ServerHealth {
                name,
                status,
                sign_in_needed: entry
                    .get("signInNeeded")
                    .and_then(Value::as_bool)
                    .unwrap_or(status == HealthStatus::SignInNeeded),
                transport: entry
                    .get("transport")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                reason: if status == HealthStatus::Failed {
                    reason_of(status_part(&detail))
                } else {
                    String::new()
                },
                needs_approval: is_approval(status_part(&detail)),
                oauth_style: false,
            })
        })
        .collect();
    if servers.is_empty() {
        if let Some(text) = reply.get("stdout").and_then(Value::as_str) {
            servers = crate::mcp::parse_claude_health(&scrub.host_text(text))
                .into_iter()
                .map(|(name, status)| {
                    let status = match status.as_str() {
                        "Connected" => HealthStatus::Connected,
                        "Needs authentication" => HealthStatus::SignInNeeded,
                        "Failed" => HealthStatus::Failed,
                        "Disabled" => HealthStatus::Disabled,
                        _ => HealthStatus::Configured,
                    };
                    ServerHealth {
                        name,
                        status,
                        sign_in_needed: status == HealthStatus::SignInNeeded,
                        transport: "unknown".into(),
                        reason: String::new(),
                        needs_approval: false,
                        oauth_style: false,
                    }
                })
                .collect();
        }
    }
    if reply.get("timedOut").and_then(Value::as_bool) == Some(true) {
        notes.push("The MCP health check timed out; some servers may still be starting.".into());
    }
    if reply.get("kitServers").and_then(Value::as_bool) == Some(false) {
        notes.push("The health check did not include the servers that were sent.".into());
    }
    (servers, notes)
}

/// Marks the servers the kit sent config-only, and lists every sent server even when the
/// health output did not mention it.
fn merge_health(mut health: Vec<ServerHealth>, packed: &Packed) -> Vec<ServerHealth> {
    for sent in &packed.servers {
        match health.iter_mut().find(|h| h.name == sent.name) {
            Some(entry) => {
                entry.oauth_style = sent.oauth_style;
                // An OAuth-style server that is merely configured is waiting for its
                // sign-in; the app cannot know more (D12).
                if sent.oauth_style && entry.status == HealthStatus::Configured {
                    entry.status = HealthStatus::SignInNeeded;
                    entry.sign_in_needed = true;
                }
            }
            None => health.push(ServerHealth {
                name: sent.name.clone(),
                status: if sent.oauth_style {
                    HealthStatus::SignInNeeded
                } else {
                    HealthStatus::Configured
                },
                sign_in_needed: sent.oauth_style,
                transport: "unknown".into(),
                reason: String::new(),
                needs_approval: false,
                oauth_style: sent.oauth_style,
            }),
        }
    }
    health.sort_by(|a, b| a.name.cmp(&b.name));
    health
}

/// Warnings the health result calls for: a project server waiting for approval, and on a
/// sandbox the network policy hints.
fn health_warnings(health: &[ServerHealth], packed: &Packed, workspace: Workspace) -> Vec<String> {
    let mut out = Vec::new();
    for entry in health {
        if entry.needs_approval && entry.status == HealthStatus::SignInNeeded {
            out.push(format!(
                "Project MCP server \"{}\" waits for approval. Whether an agent session loads it before it is approved is not confirmed.",
                entry.name
            ));
        }
    }
    if workspace == Workspace::Sandbox {
        let mut hosts: BTreeSet<String> = BTreeSet::new();
        for entry in health.iter().filter(|h| h.is_problem()) {
            if let Some(sent) = packed.servers.iter().find(|s| s.name == entry.name) {
                hosts.extend(sent.hosts.iter().cloned());
            }
        }
        if !hosts.is_empty() {
            out.push(format!(
                "A server may be blocked by the sandbox network policy. Allow its hosts yourself with `sbx policy allow network {}`. `--allow-network` can only be set when a sandbox is created, and the app never changes your policy.",
                hosts.into_iter().collect::<Vec<_>>().join(",")
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Stored state
// ---------------------------------------------------------------------------

fn load_all(store: &Store) -> std::collections::BTreeMap<String, SetupState> {
    store
        .setting(SETTING)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// The last Claude setup result of `cloud`, if one ran.
pub fn setup_state(store: &Store, cloud: &CloudRef) -> Option<SetupState> {
    load_all(store).remove(&cloud.key())
}

fn save_state(store: &Store, cloud: &CloudRef, state: &SetupState) -> Result<(), String> {
    let mut all = load_all(store);
    all.insert(cloud.key(), state.clone());
    store.save_setting(
        SETTING,
        &serde_json::to_string(&all).map_err(|e| e.to_string())?,
    )
}

/// Drops the result of a workspace that no longer exists.
pub(crate) fn forget_state(store: &Store, cloud: &CloudRef) -> Result<(), String> {
    let mut all = load_all(store);
    if all.remove(&cloud.key()).is_some() {
        store.save_setting(
            SETTING,
            &serde_json::to_string(&all).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(())
}

/// True when the local setup changed since the workspace's kit was made: the selected
/// items, their files or their secrets differ. False when nothing was sent or the state has no
/// marker. The inventory must come from the same workspace kind the kit was made for.
pub fn local_changed(state: &SetupState, inventory: &KitInventory, prefs: &CloudKitPrefs) -> bool {
    state.applied
        && state
            .token
            .as_ref()
            .is_some_and(|token| token.0 != kit_pack::selection_token(inventory, prefs))
}

/// The kit kind of a workspace.
pub fn workspace_of(cloud: &CloudRef) -> Workspace {
    match cloud.kind {
        CloudBackendKind::Codespaces => Workspace::Codespace,
        CloudBackendKind::DockerSandbox => Workspace::Sandbox,
    }
}

/// A stored setup result with what the cloud panel adds to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupView {
    pub cloud: CloudRef,
    pub state: SetupState,
    /// The local setup changed since the workspace's kit was made.
    pub changed: bool,
    /// "Update Claude setup" would send something (the kit is on and accepted).
    pub can_update: bool,
}

// ---------------------------------------------------------------------------
// CloudSessions
// ---------------------------------------------------------------------------

impl CloudSessions {
    /// What the Claude setup step of this start should do, `None` when it does not run: the
    /// start attached to an existing workspace, turned the kit off, the global kit is off or not
    /// accepted, or the step already finished (credentials go once).
    pub(super) fn setup_plan(&self, record: &StartRecord) -> Option<SetupPlan> {
        let step = record.step(StepKind::ClaudeSetup);
        // A step that is Skipped without ever starting belongs to a record from before the
        // step existed: such a start never sends (see `StartRecord::normalize`).
        let predates_step = step.predates();
        if !record.created || record.kit_off || step.status == StepStatus::Done || predates_step {
            return None;
        }
        let prefs = (self.kit_seams.prefs)();
        if !prefs.applies(ApiKit::Default) {
            return None;
        }
        Some(SetupPlan {
            seams: self.kit_seams.clone(),
            prefs,
            workspace: workspace_of(&record.cloud()?),
            replace_secrets: false,
            account: record
                .sandbox_start
                .as_ref()
                .and_then(|start| start.config.claude_account.clone()),
        })
    }

    /// Turns the kit off for one start, before `run_start`. The per-start switch and the app
    /// API's `kit: "none"` use it. It can only narrow the global setting (D11).
    pub fn set_start_kit_off(&self, session_id: i64) -> Result<(), String> {
        let store = self.store()?;
        let Some(mut record) = super::startup::start_record(&store, session_id) else {
            return Err("This cloud session is not starting a workspace.".into());
        };
        if !record.kit_off {
            record.kit_off = true;
            super::startup::save_start_record(&store, &record)?;
        }
        Ok(())
    }

    /// "Update Claude setup": the same one-way copy as the start step, run by the user on a
    /// ready session. The only refresh there is. BLOCKS (upload, health): call it on a worker.
    pub fn update_claude_setup(
        &self,
        session_id: i64,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<SetupState, String> {
        let session = self
            .store()?
            .remote_session(session_id)?
            .ok_or("Session not found")?;
        let cloud = session
            .project
            .cloud()
            .ok_or("This session does not run in a cloud workspace.")?;
        let local_project = session
            .project
            .local_project
            .clone()
            .ok_or("This session has no local project.")?;
        let prefs = (self.kit_seams.prefs)();
        if !prefs.applies(ApiKit::Default) {
            return Err("The Claude setup is off. Turn it on in Settings > Cloud.".into());
        }
        let machine = self
            .host
            .machines()?
            .into_iter()
            .find(|m| m.environment_id == session.project.environment_id)
            .ok_or("The workspace is not connected. Reconnect the session first.")?;
        // Never wakes the workspace: the requests below need a live tunnel and fail with the
        // usual "not connected" text otherwise. The panel offers the action only when connected.
        let plan = SetupPlan {
            seams: self.kit_seams.clone(),
            prefs,
            workspace: workspace_of(&cloud),
            replace_secrets: true,
            account: None,
        };
        let env = Env {
            host: &*self.host,
            store: &self.store,
            halt: cancel,
            now: self.now(),
        };
        let target = Target {
            local_project: &local_project,
            cloud: &cloud,
            machine_id: &machine.id,
            project_id: Some(&session.project.project_id),
        };
        Ok(run(&env, &target, &plan, progress))
    }

    /// What the cloud panel shows for the workspace of `session_id`: the stored result of its
    /// Claude setup and whether the local setup changed since. `None` when no setup ever ran
    /// there. Reads the local setup (many files): call it on a worker, never on the UI thread.
    pub fn claude_setup_view(&self, session_id: i64) -> Option<SetupView> {
        let session = self.store().ok()?.remote_session(session_id).ok()??;
        let cloud = session.project.cloud()?;
        let state = setup_state(&*self.store().ok()?, &cloud)?;
        let prefs = (self.kit_seams.prefs)();
        let can_update = prefs.applies(ApiKit::Default);
        // A scan that cannot run says nothing, rather than claiming a change.
        let changed = session
            .project
            .local_project
            .as_deref()
            .and_then(|project| {
                let profile = profile_for(&self.store, project, None);
                (self.kit_seams.roots)(Path::new(project), &profile).ok()
            })
            .map(|roots| {
                let inventory = kit::build_inventory(&roots, workspace_of(&cloud));
                local_changed(&state, &inventory, &prefs)
            })
            .unwrap_or(false);
        Some(SetupView {
            cloud,
            state,
            changed,
            can_update,
        })
    }

    /// The stored result of the last Claude setup of `cloud`.
    pub fn claude_setup_state(&self, cloud: &CloudRef) -> Option<SetupState> {
        setup_state(&*self.store().ok()?, cloud)
    }
}
