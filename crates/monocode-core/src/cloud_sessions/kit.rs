//! The global kit: what a local Claude setup can take to a cloud workspace.
//!
//! [`build_inventory`] lists skills, MCP servers and plugins for one local project and one
//! provider account, and classifies each item (design 3.2.1, rules D6 and D7). It is pure
//! reading: no network, no Keychain, no credential files, no writes. Transfer is a later ticket.
//!
//! # Sources
//!
//! Each item is labelled with the [`KitSource`] it came from: the account's own config dir
//! (`CLAUDE_CONFIG_DIR`) first, then the user's `~/.claude` (with `~/.claude.json`). When
//! both define the same item the account wins. See design 7 for what is known about how
//! Claude Code merges the two.
//!
//! # Secrets
//!
//! Every MCP `env` and `headers` value is a secret unless it is exactly a `${VAR}`
//! reference, and so is a URL with a query string. Values live in [`Secret`], which
//! redacts itself in `Debug` and `Display`, so an inventory is safe to log.
//!
//! Secret values never enter a plain hash. `config_hash` and `content_hash` cover the
//! config with secret values replaced by a marker, so they are equal across installs and
//! say nothing about the secrets. Changes to the secrets are tracked separately by
//! [`KitMcpServer::secrets_digest`]: an HMAC-SHA256 keyed with a random per-installation
//! key (`kit-hash.key` in the app data dir, mode 0600). Two installs produce unrelated
//! digests, and a low-entropy secret cannot be guessed offline without the key. The
//! digest type redacts itself in `Debug`. Use [`KitInventory::change_token`] to detect
//! "the kit changed", secrets included. If the key cannot be read or created, secrets are
//! left out of the digests and the inventory is flagged `incomplete` (and
//! `key_unavailable`, which unlike a timeout does not stop caching).
//!
//! # For the transfer (F3-3)
//!
//! The scan checks links once. A link swapped afterwards could still point out of the
//! item, so the digest and tar steps must open files without following links: re-
//! canonicalize each path and check `starts_with(root)`, ideally comparing the dev and
//! inode recorded at scan time. A file with several hard links is refused when it is the
//! same file (device and inode) as one found under the credentials folders (`~/.ssh`,
//! `~/.aws`, ...), see [`Guard`]. Other hard links are NOT detected: a link to some
//! other private file cannot be told from a pnpm or Nix store link cheaply.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::accounts::Profile;
use crate::provider::Kind;

/// Largest single skill, measured on disk.
pub const MAX_SKILL_BYTES: u64 = 10 * 1024 * 1024;
/// Largest single plugin, measured on disk.
pub const MAX_PLUGIN_BYTES: u64 = 25 * 1024 * 1024;
/// Largest whole kit after compression.
pub const MAX_KIT_COMPRESSED_BYTES: u64 = 50 * 1024 * 1024;
/// Most entries walked for one skill or plugin.
pub const MAX_ENTRIES_PER_ITEM: usize = 20_000;
/// Most skills read from one directory.
pub const MAX_SKILLS_PER_ROOT: usize = 5_000;
/// Wall-clock budget for one inventory; later items are reported as timed out.
pub const SCAN_BUDGET: Duration = Duration::from_secs(20);

/// A value that must never be printed. `Debug` and `Display` both redact.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Drop for Secret {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// The real value. Call this only when sending it to the workspace.
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where an item was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KitSource {
    /// The provider account's own config dir.
    Account,
    /// The user's `~/.claude` and `~/.claude.json`.
    User,
    /// The local project itself.
    Project,
}

/// Why an item cannot be sent as is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Exclusion {
    TooLarge,
    LinkOutsideHome,
    TooManyFiles,
    ScanTimedOut,
    /// A link inside the item resolves to a path outside the item's own root.
    LinkLeavesItem,
    /// The item root resolves to a place that is never a skill or plugin: `$HOME` itself,
    /// a folder that contains it, a credentials folder or a system folder.
    UnsafeRoot,
    /// A file with several hard links that is the same file as one in a credentials folder.
    HardLinkElsewhere,
    /// The item contains a credentials folder (`.ssh`, `.aws`, `~/.config/gh`, ...).
    SensitiveContent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillScope {
    User,
    ProjectUntracked,
    Plugin,
}

/// One regular file of a skill or plugin as the scan saw it: its canonical path and, where
/// the platform has them, the device and inode. The tar builder opens exactly these, after
/// checking each is still the same plain file (design 3.7).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct KitFile {
    pub path: PathBuf,
    pub id: Option<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitSkill {
    pub name: String,
    pub scope: SkillScope,
    pub source: KitSource,
    /// The plugin id for [`SkillScope::Plugin`]; such a skill travels with its plugin.
    pub plugin: Option<String>,
    pub path: PathBuf,
    pub size: u64,
    pub hash: String,
    /// Gzip size of the files; 0 when excluded.
    pub compressed: u64,
    pub exclusion: Option<Exclusion>,
    /// The files the scan found, sorted by path. Empty when excluded.
    pub files: Vec<KitFile>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum McpScope {
    User,
    Local,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Runtime {
    Npx,
    Uvx,
    Bunx,
    Node,
    Python,
    Docker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalOnlyReason {
    /// A stdio command that is an absolute local path.
    AbsoluteCommand,
    /// A URL on localhost, loopback, `*.local`, a private or a link-local address.
    PrivateUrl,
    /// A stdio `docker` command on a Docker sandbox.
    DockerOnSandbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpCompat {
    Ok,
    LocalOnly(LocalOnlyReason),
    NeedsRuntime(Runtime),
    /// An http/sse server with no raw `Authorization` header (D12). The app never learns
    /// whether the user signed in locally: it is carried config-only and the user signs in
    /// in the cloud.
    OAuthStyle,
    Project,
}

/// Which kind of workspace the kit is for. Only the Docker rule depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    Codespace,
    Sandbox,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretLocation {
    Env(String),
    Header(String),
    UrlQuery,
    /// The URL carries `user:password@`.
    UrlUserinfo,
    /// The URL carries a `#fragment`.
    UrlFragment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretField {
    pub location: SecretLocation,
    pub value: Secret,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitMcpServer {
    pub name: String,
    pub scope: McpScope,
    pub source: KitSource,
    pub config_path: PathBuf,
    pub transport: String,
    pub compat: McpCompat,
    pub secrets: Vec<SecretField>,
    /// Hash of the canonical server config with every secret value replaced by a marker,
    /// so it is the same whatever the secrets are.
    pub config_hash: String,
    /// Keyed digest of the secret values (see the module docs). `None` when the server
    /// has no secrets, or when the local key is unavailable.
    pub secrets_digest: Option<KeyedDigest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KitPlugin {
    /// `name@marketplace`.
    pub id: String,
    pub version: String,
    pub source: KitSource,
    pub install_path: PathBuf,
    pub size: u64,
    /// `bin/`, `.lsp.json` or a native `*.node` file: it may not run on Linux.
    pub has_native_bin: bool,
    pub hash: String,
    /// Gzip size of the files; 0 when excluded.
    pub compressed: u64,
    pub exclusion: Option<Exclusion>,
    /// The files the scan found, sorted by path. Empty when excluded.
    pub files: Vec<KitFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KitInventory {
    pub skills: Vec<KitSkill>,
    pub mcp_servers: Vec<KitMcpServer>,
    pub plugins: Vec<KitPlugin>,
    /// Compressed size of every item that is not excluded (plugin skills count with
    /// their plugin).
    pub total_compressed: u64,
    /// True when the total is not known to be under the cap: it is over it, or an item
    /// timed out so the total is a lower bound.
    pub over_total_cap: bool,
    /// An item hit the scan budget. The totals and the hash are then not comparable
    /// between runs, so callers must not treat them as a stable fingerprint.
    pub incomplete: bool,
    /// An item hit the scan budget (a subset of `incomplete`). Only this makes a scan not
    /// worth caching.
    pub scan_timed_out: bool,
    /// The local key was unavailable, so secrets are not in the digests (a subset of
    /// `incomplete`). A rescan would not change that, so it does not block caching.
    pub key_unavailable: bool,
    /// The credential index was cut short, so a hard link to a credentials file could have
    /// gone unseen (a subset of `incomplete`). The tar builder then sends no files.
    pub file_index_incomplete: bool,
    /// Same inputs, same hash. Covers names, scopes, sources, file contents and server
    /// configs with secret values masked, so it is comparable across installs. For
    /// change detection that includes the secrets use [`KitInventory::change_token`].
    pub content_hash: String,
}

impl KitInventory {
    /// A token that changes when anything in the kit changes, secret values included.
    /// It mixes in the keyed secret digests, so it is only comparable on this install.
    /// Do not log it.
    pub fn change_token(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.content_hash.as_bytes());
        for server in &self.mcp_servers {
            hasher.update([0]);
            if let Some(digest) = &server.secrets_digest {
                hasher.update(digest.0.as_bytes());
            }
        }
        hex(&hasher.finalize())
    }
}

/// An HMAC-SHA256 hex digest. It never appears in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyedDigest(String);

impl KeyedDigest {
    /// Mixes the digest into `hasher`, for tokens that must change with the secrets.
    pub(super) fn update(&self, hasher: &mut Sha256) {
        hasher.update(self.0.as_bytes());
    }
}

#[cfg(test)]
impl KeyedDigest {
    pub(super) fn hex_for_test(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for KeyedDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyedDigest(<redacted>)")
    }
}

/// Where to look. Build with [`KitRoots::for_profile`], or by hand in tests.
#[derive(Clone, Debug)]
pub struct KitRoots {
    /// The account config dir. `None`, or equal to `claude_home`, for the default account.
    pub account_dir: Option<PathBuf>,
    /// The user's home. Symlinks may not leave it.
    pub home: PathBuf,
    /// `~/.claude`. Its JSON sibling is `<home>/.claude.json`.
    pub claude_home: PathBuf,
    /// The local project root.
    pub project: PathBuf,
    /// Where the local `kit-hash.key` lives. `None` means no key: secrets are then left
    /// out of the digests and the inventory is flagged `incomplete`.
    pub key_dir: Option<PathBuf>,
}

impl KitRoots {
    pub fn for_profile(profile: &Profile, project: &Path) -> Result<Self, String> {
        let home = crate::scm::shell::dirs_home()
            .map(PathBuf::from)
            .ok_or("Home directory not found")?;
        let claude_home = home.join(".claude");
        let account = profile.home(Kind::Claude)?;
        let account_dir = (account != claude_home).then_some(account);
        let key_dir = app_data_dir(&home);
        Ok(Self {
            account_dir,
            home,
            claude_home,
            project: project.to_path_buf(),
            key_dir,
        })
    }
}

/// The app data dir, as other core modules find it: `MONOCODE_NATIVE_DATA_DIR`, else
/// `~/.cache/monocode-native`.
fn app_data_dir(home: &Path) -> Option<PathBuf> {
    Some(data_dir_from(
        std::env::var_os("MONOCODE_NATIVE_DATA_DIR"),
        home,
    ))
}

/// An empty or relative value would put the key in the process cwd: ignore it.
pub(super) fn data_dir_from(var: Option<std::ffi::OsString>, home: &Path) -> PathBuf {
    var.map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".cache/monocode-native"))
}

/// Inventory of what the kit could carry. Pure local reading: no Keychain, no network.
pub fn build_inventory(roots: &KitRoots, workspace: Workspace) -> KitInventory {
    build_inventory_budgeted(roots, workspace, SCAN_BUDGET)
}

/// [`build_inventory`] with an explicit scan budget (tests use a tiny one).
pub fn build_inventory_budgeted(
    roots: &KitRoots,
    workspace: Workspace,
    budget: Duration,
) -> KitInventory {
    let home = std::fs::canonicalize(&roots.home).unwrap_or_else(|_| roots.home.clone());
    let begun = Instant::now();
    let guard = Guard::new(roots, &home, begun, budget);
    let started = Clock {
        started: begun,
        budget,
        raw_home: &roots.home,
        project: &roots.project,
        guard: &guard,
    };
    let sources = sources(roots);
    let mut inventory = KitInventory::default();
    let mut plugin_dirs: Vec<(String, PathBuf, KitSource)> = Vec::new();
    let key = roots.key_dir.as_deref().and_then(load_or_create_key);

    for source in &sources {
        collect_plugins(source, roots, &home, started, &mut inventory.plugins);
    }
    for plugin in &inventory.plugins {
        plugin_dirs.push((
            plugin.id.clone(),
            plugin.install_path.clone(),
            plugin.source,
        ));
    }
    for source in &sources {
        collect_user_skills(source, &home, started, &mut inventory.skills);
    }
    collect_project_skills(roots, &home, started, &mut inventory.skills);
    for (id, dir, source) in &plugin_dirs {
        collect_plugin_skills(id, dir, *source, &home, started, &mut inventory.skills);
    }
    for source in &sources {
        collect_mcp(source, roots, workspace, &mut inventory.mcp_servers);
    }
    collect_project_mcp(roots, &mut inventory.mcp_servers);

    inventory.skills.sort_by(|a, b| {
        (&a.name, a.scope, a.source, &a.path).cmp(&(&b.name, b.scope, b.source, &b.path))
    });
    inventory
        .plugins
        .sort_by(|a, b| (&a.id, a.source).cmp(&(&b.id, b.source)));
    inventory.mcp_servers.sort_by(|a, b| {
        (&a.name, a.scope, a.source, &a.config_path).cmp(&(
            &b.name,
            b.scope,
            b.source,
            &b.config_path,
        ))
    });

    let timed_out = inventory
        .skills
        .iter()
        .map(|s| s.exclusion)
        .chain(inventory.plugins.iter().map(|p| p.exclusion))
        .any(|e| e == Some(Exclusion::ScanTimedOut));
    // Without a key the secrets cannot be digested, so a change in them would go
    // unseen: flag the inventory instead of looking complete. This does not make the
    // size total a lower bound, so `over_total_cap` ignores it.
    let mut key_missing = false;
    for server in &mut inventory.mcp_servers {
        if server.secrets.is_empty() {
            continue;
        }
        match &key {
            Some(key) => server.secrets_digest = Some(secrets_digest(key, &server.secrets)),
            None => key_missing = true,
        }
    }
    inventory.scan_timed_out = timed_out;
    inventory.key_unavailable = key_missing;
    // A credential index cut short means a hard link to a credential file could be missed.
    inventory.file_index_incomplete = !guard.index_complete;
    inventory.incomplete = timed_out || key_missing || !guard.index_complete;
    inventory.total_compressed = total_compressed(&inventory);
    // A timed-out item has no size, so the total is only a lower bound: report over-cap.
    inventory.over_total_cap = timed_out || inventory.total_compressed > MAX_KIT_COMPRESSED_BYTES;
    inventory.content_hash = content_hash(&inventory);
    inventory
}

struct Source {
    label: KitSource,
    dir: PathBuf,
    claude_json: PathBuf,
}

fn sources(roots: &KitRoots) -> Vec<Source> {
    let mut out = Vec::new();
    if let Some(dir) = &roots.account_dir {
        out.push(Source {
            label: KitSource::Account,
            claude_json: dir.join(".claude.json"),
            dir: dir.clone(),
        });
    }
    out.push(Source {
        label: KitSource::User,
        dir: roots.claude_home.clone(),
        claude_json: roots.home.join(".claude.json"),
    });
    out
}

// ---------------------------------------------------------------------------
// Files: measuring, symlink rules, hashing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    Skill,
    Plugin,
}

/// The scan deadline, shared by every item of one inventory.
#[derive(Clone, Copy)]
struct Clock<'a> {
    started: Instant,
    budget: Duration,
    /// `roots.home` as given, before canonicalizing (it may sit behind a link such as
    /// macOS `/var`).
    raw_home: &'a Path,
    /// The local project, for roots that live outside `$HOME`.
    project: &'a Path,
    guard: &'a Guard,
}

struct Scan {
    files: Vec<KitFile>,
    size: u64,
    has_native: bool,
    exclusion: Option<Exclusion>,
}

/// Folders under `$HOME` that hold credentials. An item root never resolves into one.
const SENSITIVE_HOME_DIRS: [&str; 7] = [
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".docker",
    ".config/gcloud",
    ".config/gh",
];

/// System folders an item root never resolves into.
const SYSTEM_DIRS: [&str; 8] = [
    "/etc",
    "/private/etc",
    "/usr",
    "/bin",
    "/sbin",
    "/System",
    "/Library",
    "/private/var/root",
];

/// Places an item must never reach, and the identity of credential files.
struct Guard {
    /// Credentials folders and files and the claude config dirs. An item root may not be
    /// one of these or contain one. Walking into one is refused too.
    forbidden: Vec<PathBuf>,
    /// The app's own dirs (data dir, `provider-accounts`, account dirs, the key dir). A
    /// root may not be one of these, contain one, or live UNDER one (except the
    /// `skills` and `plugins` folders of the active config dirs, see `exempt`).
    app: Vec<PathBuf>,
    /// The `skills` and `plugins` folders of the active claude config dirs, where real
    /// items live even when that dir sits inside an app dir.
    exempt: Vec<PathBuf>,
    /// (device, inode) of regular files found under the credentials folders, to spot a
    /// hard link to one of them.
    secret_files: HashSet<(u64, u64)>,
    /// False when the scan for `secret_files` ran out of time or entries, so a hard link
    /// to a credential file may go unnoticed.
    index_complete: bool,
}

/// Entries read when collecting credential file identities.
const MAX_GUARD_ENTRIES: usize = 20_000;

/// Folder names that are never part of a skill or plugin, wherever they sit.
const SENSITIVE_NAMES: [&str; 3] = [".ssh", ".aws", ".gnupg"];

/// Credential locations under `$HOME`, in the order they are indexed: the small and
/// important ones first, the folders that may hold logs and caches last.
const INDEXED_HOME_PATHS: [&str; 9] = [
    ".claude/.credentials.json",
    ".ssh",
    ".aws",
    ".config/gh",
    ".gnupg",
    ".codex/auth.json",
    ".kube",
    ".docker",
    ".config/gcloud",
];

/// Folders that hold the app's own data, as the app finds them plus the per-OS layouts
/// under the given home (the home may differ from the process one).
fn app_data_dirs(home: &Path) -> Vec<PathBuf> {
    let id = crate::brand::DESKTOP_APP_ID;
    let mut out = vec![
        home.join("Library/Application Support").join(id),
        home.join(".local/share").join(id),
    ];
    if let Some(dir) = crate::store::monocode_database().parent() {
        out.push(dir.to_path_buf());
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        out.push(PathBuf::from(xdg).join(id));
    }
    out
}

impl Guard {
    fn new(roots: &KitRoots, home: &Path, started: Instant, budget: Duration) -> Self {
        fn with_canonical(list: &mut Vec<PathBuf>, path: PathBuf) {
            if let Ok(real) = std::fs::canonicalize(&path) {
                if real != path {
                    list.push(real);
                }
            }
            list.push(path);
        }
        let mut forbidden: Vec<PathBuf> = Vec::new();
        for base in [home, roots.home.as_path()] {
            for rel in INDEXED_HOME_PATHS {
                with_canonical(&mut forbidden, base.join(rel));
            }
            with_canonical(&mut forbidden, base.join(".claude"));
            with_canonical(&mut forbidden, base.join(".codex"));
        }
        with_canonical(&mut forbidden, roots.claude_home.clone());

        let mut app: Vec<PathBuf> = Vec::new();
        let mut data_dirs = app_data_dirs(&roots.home);
        data_dirs.extend(app_data_dirs(home));
        for dir in data_dirs {
            with_canonical(&mut app, dir.join("provider-accounts"));
            with_canonical(&mut app, dir);
        }
        for dir in [&roots.account_dir, &roots.key_dir].into_iter().flatten() {
            with_canonical(&mut app, dir.clone());
        }
        // Every account dir the app knows lives under `provider-accounts`; those are
        // covered by their parent, but list the ones present for clarity and for links.
        let mut exempt: Vec<PathBuf> = Vec::new();
        for dir in std::iter::once(&roots.claude_home).chain(roots.account_dir.iter()) {
            for sub in ["skills", "plugins"] {
                with_canonical(&mut exempt, dir.join(sub));
            }
        }
        for list in [&mut forbidden, &mut app, &mut exempt] {
            list.sort();
            list.dedup();
        }

        let mut secret_files = HashSet::new();
        let mut budget_left = MAX_GUARD_ENTRIES;
        let mut index_complete = true;
        for rel in INDEXED_HOME_PATHS {
            let path = home.join(rel);
            // A credentials folder may itself be a link (stow-style dotfiles).
            let path = std::fs::canonicalize(&path).unwrap_or(path);
            if !collect_file_ids(&path, &mut secret_files, &mut budget_left, started, budget) {
                index_complete = false;
                break;
            }
        }
        Self {
            forbidden,
            app,
            exempt,
            secret_files,
            index_complete,
        }
    }
}

/// Add the (device, inode) of every regular file under `path`, without following links.
/// Errors are ignored. Returns false when the entry cap or the time budget ran out.
fn collect_file_ids(
    path: &Path,
    out: &mut HashSet<(u64, u64)>,
    left: &mut usize,
    started: Instant,
    budget: Duration,
) -> bool {
    if *left == 0 || started.elapsed() >= budget {
        return false;
    }
    *left -= 1;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return true;
    };
    if meta.is_file() {
        if let Some(id) = file_id(&meta) {
            out.insert(id);
        }
    } else if meta.is_dir() {
        if let Ok(reader) = std::fs::read_dir(path) {
            for entry in reader.flatten() {
                if !collect_file_ids(&entry.path(), out, left, started, budget) {
                    return false;
                }
            }
        }
    }
    true
}

/// Is `real_root` a place that is never a skill or plugin? `root` is the path as named
/// and `canonical_named` the same path with its parent canonicalized.
fn unsafe_root(
    root: &Path,
    canonical_named: &Path,
    real_root: &Path,
    homes: [&Path; 2],
    guard: &Guard,
) -> bool {
    if real_root.parent().is_none() || SYSTEM_DIRS.iter().any(|d| real_root.starts_with(d)) {
        return true;
    }
    // `$HOME` itself or any folder that contains it.
    if homes.iter().any(|home| home.starts_with(real_root)) {
        return true;
    }
    // A folder that contains the path that names the root (`foo -> ..`). The named path
    // is also checked with its parent canonicalized, for a home behind a link.
    if real_root != root
        && (root.starts_with(real_root)
            || (canonical_named != real_root && canonical_named.starts_with(real_root)))
    {
        return true;
    }
    // The app's own dirs: equal, containing, or living under one is refused. The
    // `skills` and `plugins` folders of the active config dirs are the exception.
    let exempt = guard.exempt.iter().any(|dir| real_root.starts_with(dir));
    if guard
        .app
        .iter()
        .any(|dir| dir.starts_with(real_root) || (!exempt && real_root.starts_with(dir)))
    {
        return true;
    }
    // A credentials folder or file, the claude config dirs, or a
    // folder that contains one of them (`foo -> ~/.config`, `foo -> ~/.claude`). Living
    // UNDER `~/.claude` (`~/.claude/skills/foo`) is fine.
    if guard
        .forbidden
        .iter()
        .any(|path| path.starts_with(real_root))
    {
        return true;
    }
    homes.iter().any(|home| {
        SENSITIVE_HOME_DIRS
            .iter()
            .any(|dir| real_root.starts_with(home.join(dir)))
            || real_root.starts_with(home.join("Library/Keychains"))
    })
}

/// Is a directory met while walking an item a credentials folder? `.ssh`, `.aws` and
/// `.gnupg` always are. `.docker` and `.kube` also hold ordinary project files
/// (a Dockerfile, manifests), so they count only when they hold a client config.
fn sensitive_dir(path: &Path, name: &str, guard: &Guard) -> bool {
    if SENSITIVE_NAMES.contains(&name) || guard.forbidden.iter().any(|f| f == path) {
        return true;
    }
    match name {
        ".docker" => path.join("config.json").exists(),
        ".kube" => kube_has_config(path),
        _ => false,
    }
}

/// A `.kube` folder with a `config` file, or a file that looks like a kubeconfig.
fn kube_has_config(dir: &Path) -> bool {
    if dir.join("config").exists() {
        return true;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return false;
    };
    reader.flatten().take(64).any(|entry| {
        let Ok(file) = std::fs::File::open(entry.path()) else {
            return false;
        };
        let mut head = Vec::new();
        let _ = file.take(64 * 1024).read_to_end(&mut head);
        let text = String::from_utf8_lossy(&head);
        text.contains("current-context:") || text.contains("clusters:")
    })
}

/// Walk one skill or plugin directory. Files come back sorted, so every later step is
/// deterministic.
///
/// Link rules:
///
/// - The item ROOT may itself be reached through a link. That is the common dotfiles
///   setup (`~/.claude/skills/foo -> ~/dotfiles/skills/foo`), so a root named under
///   `$HOME` may resolve anywhere under `$HOME`. One that resolves out of `$HOME`
///   (through any component) is refused with [`Exclusion::LinkOutsideHome`]. A root not
///   named under `$HOME` (a project elsewhere, a plugin installed elsewhere) is taken
///   as it resolves.
/// - Links INSIDE the item (its content) must resolve inside the item's own canonical
///   root. Anything else, even a path under `$HOME` or the project, excludes the whole
///   item with [`Exclusion::LinkLeavesItem`], so `creds -> ~/.aws` never travels.
/// - Dead links are skipped, and link loops end through the visited set.
fn scan_item(root: &Path, kind: ItemKind, home: &Path, started: Clock<'_>) -> Scan {
    let mut scan = Scan {
        files: Vec::new(),
        size: 0,
        has_native: false,
        exclusion: None,
    };
    let cap = match kind {
        ItemKind::Skill => MAX_SKILL_BYTES,
        ItemKind::Plugin => MAX_PLUGIN_BYTES,
    };
    let Ok(real_root) = std::fs::canonicalize(root) else {
        return scan;
    };
    let named_inside = root.starts_with(home) || root.starts_with(started.raw_home);
    if named_inside && !real_root.starts_with(home) {
        scan.exclusion = Some(Exclusion::LinkOutsideHome);
        return scan;
    }
    let canonical_named = match (root.parent(), root.file_name()) {
        (Some(parent), Some(name)) => std::fs::canonicalize(parent)
            .map(|p| p.join(name))
            .unwrap_or_else(|_| root.to_path_buf()),
        _ => root.to_path_buf(),
    };
    if unsafe_root(
        root,
        &canonical_named,
        &real_root,
        [home, started.raw_home],
        started.guard,
    ) {
        scan.exclusion = Some(Exclusion::UnsafeRoot);
        return scan;
    }
    if !named_inside {
        // Outside `$HOME` the root must stay in the project or next to where it is named.
        let project = std::fs::canonicalize(started.project).ok();
        let parent = root.parent().and_then(|p| std::fs::canonicalize(p).ok());
        let allowed = project.is_some_and(|p| real_root.starts_with(p))
            || parent.is_some_and(|p| real_root.starts_with(p));
        if !allowed {
            scan.exclusion = Some(Exclusion::UnsafeRoot);
            return scan;
        }
    }
    if kind == ItemKind::Plugin && (root.join("bin").exists() || root.join(".lsp.json").exists()) {
        scan.has_native = true;
    }
    let mut visited: HashSet<PathBuf> = HashSet::new();
    visited.insert(real_root.clone());
    let mut entries = 0usize;
    walk(
        &real_root,
        &real_root,
        kind,
        started,
        cap,
        &mut visited,
        &mut entries,
        &mut scan,
    );
    scan.files.sort();
    scan
}

#[allow(clippy::too_many_arguments)]
fn walk(
    dir: &Path,
    item_root: &Path,
    kind: ItemKind,
    started: Clock<'_>,
    cap: u64,
    visited: &mut HashSet<PathBuf>,
    entries: &mut usize,
    scan: &mut Scan,
) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<_> = reader.flatten().collect();
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if scan.exclusion.is_some() {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        if kind == ItemKind::Skill && name == "node_modules" {
            continue;
        }
        *entries += 1;
        if *entries > MAX_ENTRIES_PER_ITEM {
            scan.exclusion = Some(Exclusion::TooManyFiles);
            return;
        }
        if started.started.elapsed() >= started.budget {
            scan.exclusion = Some(Exclusion::ScanTimedOut);
            return;
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let (path, meta) = if meta.file_type().is_symlink() {
            let real = match resolve_link(&path, item_root) {
                Link::Inside(real) => real,
                Link::Outside => {
                    scan.exclusion = Some(Exclusion::LinkLeavesItem);
                    return;
                }
                Link::Dead => continue,
            };
            let Ok(meta) = std::fs::metadata(&real) else {
                continue;
            };
            (real, meta)
        } else {
            (path, meta)
        };
        if meta.is_dir() {
            if sensitive_dir(&path, &name, started.guard) {
                scan.exclusion = Some(Exclusion::SensitiveContent);
                return;
            }
            if !visited.insert(path.clone()) {
                continue;
            }
            walk(&path, item_root, kind, started, cap, visited, entries, scan);
        } else if meta.is_file() {
            // A file that is a hard link to a credentials file is refused. Other hard
            // links are not detected (see the module docs).
            if started.guard.forbidden.contains(&path) {
                scan.exclusion = Some(Exclusion::SensitiveContent);
                return;
            }
            if link_count(&meta) > 1
                && file_id(&meta).is_some_and(|id| started.guard.secret_files.contains(&id))
            {
                scan.exclusion = Some(Exclusion::HardLinkElsewhere);
                return;
            }
            scan.size += meta.len();
            if scan.size > cap {
                scan.exclusion = Some(Exclusion::TooLarge);
                return;
            }
            if path.extension().is_some_and(|ext| ext == "node") {
                scan.has_native = true;
            }
            scan.files.push(KitFile {
                id: file_id(&meta),
                path,
            });
        }
    }
}

#[cfg(unix)]
fn link_count(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::nlink(meta)
}
#[cfg(not(unix))]
fn link_count(_meta: &std::fs::Metadata) -> u64 {
    1
}
#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}
#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

enum Link {
    /// Resolves to a path inside the item's own canonical root.
    Inside(PathBuf),
    /// Resolves anywhere else: refused.
    Outside,
    /// Dead or unreadable: skipped.
    Dead,
}

/// Resolve a symlink found inside an item. Only a target inside `item_root` (already
/// canonical) is accepted; a dead link is simply skipped.
fn resolve_link(path: &Path, item_root: &Path) -> Link {
    match std::fs::canonicalize(path) {
        Ok(real) if real.starts_with(item_root) => Link::Inside(real),
        Ok(_) => Link::Outside,
        Err(_) => Link::Dead,
    }
}

struct CountingSink(u64);
impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// SHA-256 over the files (relative path and bytes) and their gzip size, in one read.
fn digest_files(root: &Path, files: &[KitFile]) -> (String, u64) {
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut hasher = Sha256::new();
    let mut gzip = flate2::write::GzEncoder::new(CountingSink(0), flate2::Compression::fast());
    let mut buffer = vec![0u8; 64 * 1024];
    for file in files {
        let file = &file.path;
        let relative = file.strip_prefix(&real_root).unwrap_or(file);
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);
        let Ok(mut handle) = std::fs::File::open(file) else {
            continue;
        };
        while let Ok(read) = handle.read(&mut buffer) {
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            let _ = gzip.write_all(&buffer[..read]);
        }
        hasher.update([0]);
    }
    let compressed = gzip.finish().map(|sink| sink.0).unwrap_or(0);
    (hex(&hasher.finalize()), compressed)
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn digest_str(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex(&hasher.finalize())
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

fn skill_from_dir(
    name: &str,
    dir: &Path,
    scope: SkillScope,
    source: KitSource,
    plugin: Option<String>,
    home: &Path,
    started: Clock<'_>,
) -> KitSkill {
    let scan = scan_item(dir, ItemKind::Skill, home, started);
    let (hash, compressed) = if scan.exclusion.is_none() {
        digest_files(dir, &scan.files)
    } else {
        (String::new(), 0)
    };
    let files = if scan.exclusion.is_none() {
        scan.files
    } else {
        Vec::new()
    };
    KitSkill {
        files,
        compressed,
        name: name.to_string(),
        scope,
        source,
        plugin,
        path: dir.to_path_buf(),
        size: scan.size,
        hash,
        exclusion: scan.exclusion,
    }
}

/// Skill directories (those holding a `SKILL.md`) directly under `root`, by name.
fn skill_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(reader) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = reader
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let path = entry.path();
            (!name.starts_with('.') && path.join("SKILL.md").is_file()).then_some((name, path))
        })
        .collect();
    out.sort();
    out.truncate(MAX_SKILLS_PER_ROOT);
    out
}

fn collect_user_skills(source: &Source, home: &Path, started: Clock<'_>, out: &mut Vec<KitSkill>) {
    for (name, dir) in skill_dirs(&source.dir.join("skills")) {
        // The account wins over `~/.claude` for the same name.
        if out
            .iter()
            .any(|skill| skill.scope == SkillScope::User && skill.name == name)
        {
            continue;
        }
        out.push(skill_from_dir(
            &name,
            &dir,
            SkillScope::User,
            source.label,
            None,
            home,
            started,
        ));
    }
}

/// Names under `.claude/skills` that git knows (`git ls-files`, so the index). A name
/// counts as committed when any entry is under it, or when it is itself an entry (a
/// symlink or a gitlink has no trailing `/`). Staged but unpushed skills are therefore
/// skipped too. `None` when git fails or this is not a repository: treated as unknown,
/// so every project skill is offered.
fn tracked_skill_names(project: &Path) -> Option<BTreeSet<String>> {
    let listing = crate::git::git(project, &["ls-files", "-z", "--", ".claude/skills"]).ok()?;
    let mut names = BTreeSet::new();
    for entry in listing.split('\0') {
        if let Some(rest) = entry.strip_prefix(".claude/skills/") {
            let name = rest.split_once('/').map_or(rest, |(name, _)| name);
            if !name.is_empty() {
                names.insert(name.to_string());
            }
        }
    }
    Some(names)
}

fn collect_project_skills(
    roots: &KitRoots,
    home: &Path,
    started: Clock<'_>,
    out: &mut Vec<KitSkill>,
) {
    let tracked = tracked_skill_names(&roots.project).unwrap_or_default();
    for (name, dir) in skill_dirs(&roots.project.join(".claude/skills")) {
        // Committed skills arrive with the clone.
        if tracked.contains(&name) {
            continue;
        }
        out.push(skill_from_dir(
            &name,
            &dir,
            SkillScope::ProjectUntracked,
            KitSource::Project,
            None,
            home,
            started,
        ));
    }
}

fn collect_plugin_skills(
    plugin: &str,
    install_path: &Path,
    source: KitSource,
    home: &Path,
    started: Clock<'_>,
    out: &mut Vec<KitSkill>,
) {
    for (name, dir) in skill_dirs(&install_path.join("skills")) {
        out.push(skill_from_dir(
            &name,
            &dir,
            SkillScope::Plugin,
            source,
            Some(plugin.to_string()),
            home,
            started,
        ));
    }
}

// ---------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------

fn resolve_install_path(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    match raw.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(raw),
    }
}

fn collect_plugins(
    source: &Source,
    roots: &KitRoots,
    home: &Path,
    started: Clock<'_>,
    out: &mut Vec<KitPlugin>,
) {
    let Some(registry) = crate::mcp::read_json(&source.dir.join("plugins/installed_plugins.json"))
    else {
        return;
    };
    let Some(plugins) = registry.get("plugins").and_then(Value::as_object) else {
        return;
    };
    let enabled = crate::mcp::read_json(&source.dir.join("settings.json"))
        .and_then(|settings| settings.get("enabledPlugins").cloned());
    for (id, installed) in plugins {
        let is_enabled = enabled
            .as_ref()
            .and_then(|map| map.get(id))
            .and_then(Value::as_bool)
            == Some(true);
        if !is_enabled || out.iter().any(|plugin| &plugin.id == id) {
            continue;
        }
        let entries: Vec<&Value> = match installed.as_array() {
            Some(entries) => entries.iter().collect(),
            None if installed.is_object() => vec![installed],
            None => continue,
        };
        let Some(entry) = entries
            .into_iter()
            .find(|entry| plugin_applies(entry, roots))
        else {
            continue;
        };
        let Some(raw_path) = entry.get("installPath").and_then(Value::as_str) else {
            continue;
        };
        let install_path = resolve_install_path(raw_path, &roots.home);
        let scan = scan_item(&install_path, ItemKind::Plugin, home, started);
        let (hash, compressed) = if scan.exclusion.is_none() {
            digest_files(&install_path, &scan.files)
        } else {
            (String::new(), 0)
        };
        out.push(KitPlugin {
            compressed,
            id: id.clone(),
            version: entry
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            source: source.label,
            install_path,
            size: scan.size,
            has_native_bin: scan.has_native,
            hash,
            exclusion: scan.exclusion,
            files: if scan.exclusion.is_none() {
                scan.files
            } else {
                Vec::new()
            },
        });
    }
}

/// User-scope entries always apply; project and local entries only for their project.
fn plugin_applies(entry: &Value, roots: &KitRoots) -> bool {
    match entry.get("scope").and_then(Value::as_str) {
        Some("user") | None => true,
        Some("project" | "local") => entry
            .get("projectPath")
            .and_then(Value::as_str)
            .map(|path| {
                let path = resolve_install_path(path, &roots.home);
                roots.project.starts_with(&path)
            })
            .unwrap_or(false),
        Some(_) => false,
    }
}

// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

fn collect_mcp(
    source: &Source,
    roots: &KitRoots,
    workspace: Workspace,
    out: &mut Vec<KitMcpServer>,
) {
    let Some(config) = crate::mcp::read_json(&source.claude_json) else {
        return;
    };
    let project_key = roots.project.to_string_lossy().into_owned();
    let local = config
        .get("projects")
        .and_then(|projects| projects.get(&project_key))
        .and_then(|entry| entry.get("mcpServers"));
    for (scope, servers) in [
        (McpScope::User, config.get("mcpServers")),
        (McpScope::Local, local),
    ] {
        let Some(servers) = servers.and_then(Value::as_object) else {
            continue;
        };
        for (name, server) in servers {
            // The account wins over `~/.claude.json` for the same name and scope.
            if out
                .iter()
                .any(|existing| existing.name == *name && existing.scope == scope)
            {
                continue;
            }
            if let Some(built) = build_server(
                name,
                scope,
                source.label,
                &source.claude_json,
                server,
                workspace,
            ) {
                out.push(built);
            }
        }
    }
}

/// `.mcp.json` from the project up to the repository boundary. Listed for information:
/// these arrive with the clone.
fn collect_project_mcp(roots: &KitRoots, out: &mut Vec<KitMcpServer>) {
    for directory in roots.project.ancestors() {
        if directory == roots.home {
            break;
        }
        let path = directory.join(".mcp.json");
        if let Some(config) = crate::mcp::read_json(&path) {
            if let Some(servers) = config.get("mcpServers").and_then(Value::as_object) {
                for (name, server) in servers {
                    if out
                        .iter()
                        .any(|e| e.name == *name && e.scope == McpScope::Project)
                    {
                        continue;
                    }
                    if let Some(built) = build_server(
                        name,
                        McpScope::Project,
                        KitSource::Project,
                        &path,
                        server,
                        Workspace::Codespace,
                    ) {
                        out.push(built);
                    }
                }
            }
        }
        if directory.join(".git").exists() {
            break;
        }
    }
}

fn build_server(
    name: &str,
    scope: McpScope,
    source: KitSource,
    config_path: &Path,
    config: &Value,
    workspace: Workspace,
) -> Option<KitMcpServer> {
    let object = config.as_object()?;
    let transport = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(if object.contains_key("url") {
            "http"
        } else {
            "stdio"
        })
        .to_string();
    let secrets = find_secrets(object);
    let mut canonical = String::new();
    canonical_json(&mask_secrets(config, &secrets), &mut canonical);
    let mut server = KitMcpServer {
        name: name.to_string(),
        scope,
        source,
        config_path: config_path.to_path_buf(),
        compat: McpCompat::Ok,
        secrets,
        config_hash: digest_str(&[&canonical]),
        secrets_digest: None,
        transport,
    };
    server.compat = if scope == McpScope::Project {
        McpCompat::Project
    } else {
        classify(&server, object, workspace)
    };
    Some(server)
}

fn classify(
    server: &KitMcpServer,
    object: &serde_json::Map<String, Value>,
    workspace: Workspace,
) -> McpCompat {
    if server.transport == "stdio" {
        let command = object.get("command").and_then(Value::as_str).unwrap_or("");
        if is_absolute_command(command) {
            return McpCompat::LocalOnly(LocalOnlyReason::AbsoluteCommand);
        }
        return match runtime_of(command) {
            Some(Runtime::Docker) if workspace == Workspace::Sandbox => {
                McpCompat::LocalOnly(LocalOnlyReason::DockerOnSandbox)
            }
            Some(runtime) => McpCompat::NeedsRuntime(runtime),
            None => McpCompat::Ok,
        };
    }
    if let Some(url) = object.get("url").and_then(Value::as_str) {
        if is_private_url(url) {
            return McpCompat::LocalOnly(LocalOnlyReason::PrivateUrl);
        }
    }
    if let Some(helper) = object.get("headersHelper").and_then(Value::as_str) {
        if is_absolute_command(helper.trim_start()) {
            return McpCompat::LocalOnly(LocalOnlyReason::AbsoluteCommand);
        }
    }
    // Only plain http/sse can be OAuth-style. Any other transport (ws, sdk, unknown) is
    // carried config-only without the badge.
    if !matches!(
        server.transport.to_ascii_lowercase().as_str(),
        "http" | "sse"
    ) {
        return McpCompat::Ok;
    }
    if has_raw_credential(server, object) {
        return McpCompat::Ok;
    }
    McpCompat::OAuthStyle
}

/// Header names that always carry a credential, compared exactly and case-insensitively.
const CREDENTIAL_HEADERS: [&str; 8] = [
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "apikey",
    "x-auth-token",
    "x-access-token",
    "cookie",
];

/// Does the server carry a raw credential of any kind (D7)? Then it is not OAuth-style:
/// it works from its config alone. Names are never guessed by substring.
fn has_raw_credential(server: &KitMcpServer, object: &serde_json::Map<String, Value>) -> bool {
    if object.contains_key("headersHelper") || !server.secrets.is_empty() {
        return true;
    }
    object
        .get("headers")
        .and_then(Value::as_object)
        .is_some_and(|headers| {
            headers
                .keys()
                .any(|k| CREDENTIAL_HEADERS.contains(&k.to_ascii_lowercase().as_str()))
        })
}

fn is_absolute_command(command: &str) -> bool {
    let bytes = command.as_bytes();
    command.starts_with('/')
        || command.starts_with("~/")
        || command.starts_with('\\')
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/'))
}

pub(super) fn runtime_of(command: &str) -> Option<Runtime> {
    let base = command.rsplit(['/', '\\']).next().unwrap_or(command);
    match base.to_ascii_lowercase().as_str() {
        "npx" => Some(Runtime::Npx),
        "uvx" => Some(Runtime::Uvx),
        "bunx" => Some(Runtime::Bunx),
        "node" => Some(Runtime::Node),
        "python" | "python3" => Some(Runtime::Python),
        "docker" => Some(Runtime::Docker),
        _ => None,
    }
}

/// Localhost, loopback, `*.local`, RFC 1918, link-local, CGNAT and unspecified hosts. An
/// unparseable URL (for example one starting with `${HOST}`) is not judged. The `url`
/// crate normalizes the short and hex IPv4 forms (`127.1`, `0x7f000001`) to
/// `127.0.0.1` before this runs, so they are local; and it takes the host after any
/// userinfo, so `http://localhost@evil.com` is `evil.com`, not local.
fn is_private_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    let Some(host) = url.host() else {
        return false;
    };
    match host {
        url::Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost") || domain.ends_with(".local")
        }
        url::Host::Ipv4(ip) => private_v4(ip),
        url::Host::Ipv6(ip) => private_v6(ip),
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        // CGNAT, 100.64.0.0/10
        || (a == 100 && (b & 0xc0) == 64)
}

fn private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    let first = ip.segments()[0];
    ip.is_loopback()
        || ip.is_unspecified()
        || (first & 0xffc0) == 0xfe80
        || (first & 0xfe00) == 0xfc00
}

/// `${NAME}` and nothing else.
fn is_pure_reference(value: &str) -> bool {
    let Some(inner) = value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) else {
        return false;
    };
    let mut chars = inner.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn find_secrets(object: &serde_json::Map<String, Value>) -> Vec<SecretField> {
    let mut secrets = Vec::new();
    for (key, location) in [("env", true), ("headers", false)] {
        let Some(map) = object.get(key).and_then(Value::as_object) else {
            continue;
        };
        let mut names: Vec<&String> = map.keys().collect();
        names.sort();
        for field in names {
            let value = match &map[field] {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            if is_pure_reference(&value) {
                continue;
            }
            secrets.push(SecretField {
                location: if location {
                    SecretLocation::Env(field.clone())
                } else {
                    SecretLocation::Header(field.clone())
                },
                value: Secret::new(value),
            });
        }
    }
    if let Some(raw) = object.get("url").and_then(Value::as_str) {
        let parts = url_parts(raw);
        if parts.userinfo || parts.query || parts.fragment {
            secrets.push(SecretField {
                location: if parts.userinfo {
                    SecretLocation::UrlUserinfo
                } else if parts.query {
                    SecretLocation::UrlQuery
                } else {
                    SecretLocation::UrlFragment
                },
                value: Secret::new(raw),
            });
        }
    }
    secrets
}

const SECRET_MARKER: &str = "<secret>";

/// A copy of the config with every secret value replaced by a marker, so the plain
/// config hash does not depend on them. Which fields are secret is decided by
/// [`find_secrets`]; the field names and the shape stay.
pub(super) fn mask_secrets(config: &Value, secrets: &[SecretField]) -> Value {
    let mut masked = config.clone();
    let Some(object) = masked.as_object_mut() else {
        return masked;
    };
    for secret in secrets {
        let marker = Value::String(SECRET_MARKER.to_string());
        match &secret.location {
            SecretLocation::Env(field) => mask_field(object, "env", field, marker),
            SecretLocation::Header(field) => mask_field(object, "headers", field, marker),
            SecretLocation::UrlQuery
            | SecretLocation::UrlUserinfo
            | SecretLocation::UrlFragment => {
                if let Some(raw) = object.get("url").and_then(Value::as_str) {
                    let masked = mask_url(raw);
                    object.insert("url".to_string(), Value::String(masked));
                }
            }
        }
    }
    masked
}

/// Which secret-bearing parts a URL has.
struct UrlParts {
    userinfo: bool,
    query: bool,
    fragment: bool,
}

/// Split a URL that does not parse: `scheme://authority/path?query#fragment`, by hand.
/// Returns (scheme prefix with `://` or empty, userinfo, rest of authority and path,
/// query, fragment).
fn split_unparsed(raw: &str) -> (&str, &str, &str, &str, &str) {
    let (rest, fragment) = raw.split_once('#').unwrap_or((raw, ""));
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (prefix, after) = match rest.find("://") {
        Some(ix) => rest.split_at(ix + 3),
        None => ("", rest),
    };
    let authority_end = after.find('/').unwrap_or(after.len());
    match after[..authority_end].rfind('@') {
        Some(ix) => (prefix, &after[..ix], &after[ix + 1..], query, fragment),
        None => (prefix, "", after, query, fragment),
    }
}

fn url_parts(raw: &str) -> UrlParts {
    match url::Url::parse(raw) {
        Ok(url) => UrlParts {
            userinfo: !url.username().is_empty() || url.password().is_some(),
            query: url.query().is_some_and(|q| !q.is_empty()),
            fragment: url.fragment().is_some_and(|f| !f.is_empty()),
        },
        Err(_) => {
            let (_, userinfo, _, query, fragment) = split_unparsed(raw);
            UrlParts {
                userinfo: !userinfo.is_empty(),
                query: !query.is_empty(),
                fragment: !fragment.is_empty(),
            }
        }
    }
}

/// The plain form of a secret-bearing URL: scheme, host, port and path only. Userinfo,
/// query and fragment are replaced by a marker, so none of them reaches a plain hash.
fn mask_url(raw: &str) -> String {
    let parts = url_parts(raw);
    let mut out;
    if let Ok(url) = url::Url::parse(raw) {
        out = format!("{}://", url.scheme());
        if parts.userinfo {
            out.push_str(SECRET_MARKER);
            out.push('@');
        }
        out.push_str(url.host_str().unwrap_or(""));
        if let Some(port) = url.port() {
            out.push_str(&format!(":{port}"));
        }
        out.push_str(url.path());
    } else {
        let (prefix, _, rest, _, _) = split_unparsed(raw);
        out = prefix.to_string();
        if parts.userinfo {
            out.push_str(SECRET_MARKER);
            out.push('@');
        }
        out.push_str(rest);
    }
    if parts.query {
        out.push('?');
        out.push_str(SECRET_MARKER);
    }
    if parts.fragment {
        out.push('#');
        out.push_str(SECRET_MARKER);
    }
    out
}

fn mask_field(
    object: &mut serde_json::Map<String, Value>,
    group: &str,
    field: &str,
    marker: Value,
) {
    if let Some(map) = object.get_mut(group).and_then(Value::as_object_mut) {
        map.insert(field.to_string(), marker);
    }
}

// ---------------------------------------------------------------------------
// Keyed digest of secrets
// ---------------------------------------------------------------------------

const KEY_FILE: &str = "kit-hash.key";
const KEY_BYTES: usize = 32;

/// The per-installation key, created on first use. Any failure gives `None`: the caller
/// degrades, it never panics.
///
/// - A symlink at the key path is never followed or replaced.
/// - A key file readable by group or others is refused (degrade), never replaced.
/// - A file that cannot be read is left alone. A file that is empty or malformed is
///   garbage (for example a crash while writing an older version): it is renamed aside
///   (see [`retire_garbage`]) and a new key is created. A valid key is not replaced.
/// - A new key is written to a temp file (0600) in the same dir and published with a
///   hard link, which fails instead of replacing, so a crash never leaves a partial key
///   and two processes agree on one key.
fn load_or_create_key(dir: &Path) -> Option<[u8; KEY_BYTES]> {
    let path = dir.join(KEY_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => return None,
        Ok(meta) => {
            if key_file_too_open(&meta) {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            if let Some(key) = parse_key(&text) {
                return Some(key);
            }
            // Garbage: move it aside, never delete blindly.
            if let Some(key) = retire_garbage(&path, file_id(&meta)) {
                return Some(key);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    create_key(dir, &path)
}

#[cfg(unix)]
fn key_file_too_open(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o077 != 0
}

#[cfg(not(unix))]
fn key_file_too_open(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Remove a key file that was unreadable as a key, without replacing a valid one.
/// Returns a key only when a valid one turned up (another process published it).
///
/// Limit: with three or more processes racing, a valid key that is moved aside can be
/// published again by a third process under a new value before it is put back, so the
/// key may change once. Nothing is exposed by that: the digests simply differ from the
/// older ones, as after a reinstall.
///
/// The file is renamed aside atomically, and only when it is still the file we read
/// (same device and inode). Whatever was moved is read again: a valid key is linked
/// back without replacing anything. `None` means the path is clear, so the caller
/// creates a new key.
fn retire_garbage(path: &Path, seen: Option<(u64, u64)>) -> Option<[u8; KEY_BYTES]> {
    let now = std::fs::symlink_metadata(path).ok();
    if now.as_ref().and_then(file_id) != seen {
        // Replaced since we read it: use whatever is there now.
        return parse_key(&std::fs::read_to_string(path).ok()?);
    }
    let aside = path.with_file_name(format!(
        "{KEY_FILE}.garbage-{}-{:08x}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::rename(path, &aside).ok()?;
    let moved = std::fs::read_to_string(&aside)
        .ok()
        .and_then(|t| parse_key(&t));
    let Some(key) = moved else {
        let _ = std::fs::remove_file(&aside);
        return None;
    };
    // We moved a valid key that was published in the meantime: put it back. If someone
    // published another one since, theirs stays and wins.
    match std::fs::hard_link(&aside, path) {
        Ok(()) => {
            let _ = std::fs::remove_file(&aside);
            Some(key)
        }
        Err(_) => {
            let _ = std::fs::remove_file(&aside);
            parse_key(&std::fs::read_to_string(path).ok()?)
        }
    }
}

#[cfg(test)]
pub(super) fn retire_garbage_for_test(
    path: &Path,
    seen: Option<(u64, u64)>,
) -> Option<[u8; KEY_BYTES]> {
    retire_garbage(path, seen)
}

fn create_key(dir: &Path, path: &Path) -> Option<[u8; KEY_BYTES]> {
    std::fs::create_dir_all(dir).ok()?;
    let key: [u8; KEY_BYTES] = rand::random();
    let temp = dir.join(format!(
        "{KEY_FILE}.tmp-{}-{:08x}",
        std::process::id(),
        rand::random::<u32>()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(hex(&key).as_bytes())?;
        file.sync_all()
    });
    let published = written.is_ok().then(|| std::fs::hard_link(&temp, path));
    let _ = std::fs::remove_file(&temp);
    match published? {
        Ok(()) => Some(key),
        // Another process published first: use theirs.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            parse_key(&std::fs::read_to_string(path).ok()?)
        }
        Err(_) => None,
    }
}

fn parse_key(text: &str) -> Option<[u8; KEY_BYTES]> {
    let text = text.trim();
    if text.len() != KEY_BYTES * 2 || !text.is_ascii() {
        return None;
    }
    let mut key = [0u8; KEY_BYTES];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(key)
}

/// HMAC-SHA256 (RFC 2104), built on `sha2`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(data);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

#[cfg(test)]
pub(super) fn hmac_for_test(key: &[u8], data: &[u8]) -> String {
    hex(&hmac_sha256(key, data))
}

/// Keyed digest of a server's secrets, unambiguous (every part is length-prefixed).
fn secrets_digest(key: &[u8; KEY_BYTES], secrets: &[SecretField]) -> KeyedDigest {
    let mut data = b"monoptah-kit-secrets-v1".to_vec();
    for secret in secrets {
        for part in [
            format!("{:?}", secret.location).as_bytes(),
            secret.value.expose().as_bytes(),
        ] {
            data.extend_from_slice(&(part.len() as u64).to_le_bytes());
            data.extend_from_slice(part);
        }
    }
    KeyedDigest(hex(&hmac_sha256(key, &data)))
}

/// Compact JSON with object keys sorted, independent of the map implementation.
pub(super) fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (index, (key, item)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                canonical_json(item, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Totals and hash
// ---------------------------------------------------------------------------

/// Compressed size of every item that would be sent, from the first pass. Plugin
/// skills are inside their plugin, so they are not counted twice.
fn total_compressed(inventory: &KitInventory) -> u64 {
    let skills = inventory
        .skills
        .iter()
        .filter(|skill| skill.scope != SkillScope::Plugin)
        .map(|skill| skill.compressed);
    let plugins = inventory.plugins.iter().map(|plugin| plugin.compressed);
    skills.chain(plugins).sum()
}

fn content_hash(inventory: &KitInventory) -> String {
    let mut hasher = Sha256::new();
    if inventory.incomplete {
        // Which items made it before the deadline depends on machine load.
        hasher.update(b"incomplete");
    }
    let mut line = |parts: &[&str]| {
        hasher.update(digest_str(parts).as_bytes());
    };
    for skill in &inventory.skills {
        line(&[
            "skill",
            &skill.name,
            &format!("{:?}", skill.scope),
            &format!("{:?}", skill.source),
            skill.plugin.as_deref().unwrap_or(""),
            &skill.hash,
        ]);
    }
    for plugin in &inventory.plugins {
        line(&[
            "plugin",
            &plugin.id,
            &plugin.version,
            &format!("{:?}", plugin.source),
            &plugin.hash,
        ]);
    }
    for server in &inventory.mcp_servers {
        line(&[
            "mcp",
            &server.name,
            &format!("{:?}", server.scope),
            &format!("{:?}", server.source),
            &server.config_hash,
        ]);
    }
    hex(&hasher.finalize())
}
