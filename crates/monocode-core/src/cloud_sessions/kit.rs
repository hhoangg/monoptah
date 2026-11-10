//! The global kit: what a local Claude setup can take to a cloud workspace.
//!
//! [`build_inventory`] lists skills, MCP servers and plugins for one local project and one
//! provider account, and classifies each item (design 3.2.1, rules D6 and D7). It is pure
//! reading: no network, no Keychain, no writes. Transfer is a later ticket.
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
//! left out of the digests and the inventory is flagged `incomplete`.

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillScope {
    User,
    ProjectUntracked,
    Plugin,
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
    SignedIn,
    NotSignedIn,
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
    Some(
        std::env::var_os("MONOCODE_NATIVE_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache/monocode-native")),
    )
}

/// The seam for MCP OAuth sign-in state (F3-5a). `None` means unknown, and an
/// http/sse server without an `Authorization` header then stays `Ok`.
pub fn signed_in(_server: &KitMcpServer) -> Option<bool> {
    None
}

/// Inventory with the real sign-in seam.
pub fn build_inventory(roots: &KitRoots, workspace: Workspace) -> KitInventory {
    build_inventory_with(roots, workspace, &signed_in)
}

/// Inventory with an injected sign-in probe. The probe sees the server before its
/// compat is final (it is `Ok` at that point).
pub fn build_inventory_with(
    roots: &KitRoots,
    workspace: Workspace,
    probe: &dyn Fn(&KitMcpServer) -> Option<bool>,
) -> KitInventory {
    build_inventory_budgeted(roots, workspace, probe, SCAN_BUDGET)
}

/// [`build_inventory_with`] with an explicit scan budget (tests use a tiny one).
pub fn build_inventory_budgeted(
    roots: &KitRoots,
    workspace: Workspace,
    probe: &dyn Fn(&KitMcpServer) -> Option<bool>,
    budget: Duration,
) -> KitInventory {
    let started = Clock {
        started: Instant::now(),
        budget,
        raw_home: &roots.home,
    };
    let home = std::fs::canonicalize(&roots.home).unwrap_or_else(|_| roots.home.clone());
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
        collect_mcp(source, roots, workspace, probe, &mut inventory.mcp_servers);
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
    inventory.incomplete = timed_out || key_missing;
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
}

struct Scan {
    files: Vec<PathBuf>,
    size: u64,
    has_native: bool,
    exclusion: Option<Exclusion>,
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
            if !visited.insert(path.clone()) {
                continue;
            }
            walk(&path, item_root, kind, started, cap, visited, entries, scan);
        } else if meta.is_file() {
            scan.size += meta.len();
            if scan.size > cap {
                scan.exclusion = Some(Exclusion::TooLarge);
                return;
            }
            if path.extension().is_some_and(|ext| ext == "node") {
                scan.has_native = true;
            }
            scan.files.push(path);
        }
    }
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
fn digest_files(root: &Path, files: &[PathBuf]) -> (String, u64) {
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut hasher = Sha256::new();
    let mut gzip = flate2::write::GzEncoder::new(CountingSink(0), flate2::Compression::fast());
    let mut buffer = vec![0u8; 64 * 1024];
    for file in files {
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest_str(parts: &[&str]) -> String {
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
    KitSkill {
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
    probe: &dyn Fn(&KitMcpServer) -> Option<bool>,
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
                probe,
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
                        &|_| None,
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
    probe: &dyn Fn(&KitMcpServer) -> Option<bool>,
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
        classify(&server, object, workspace, probe)
    };
    Some(server)
}

fn classify(
    server: &KitMcpServer,
    object: &serde_json::Map<String, Value>,
    workspace: Workspace,
    probe: &dyn Fn(&KitMcpServer) -> Option<bool>,
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
    let has_auth = object
        .get("headers")
        .and_then(Value::as_object)
        .is_some_and(|headers| {
            headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("authorization"))
        });
    if has_auth {
        return McpCompat::Ok;
    }
    match probe(server) {
        Some(true) => McpCompat::SignedIn,
        Some(false) => McpCompat::NotSignedIn,
        None => McpCompat::Ok,
    }
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

fn runtime_of(command: &str) -> Option<Runtime> {
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

fn find_secrets(object: &serde_json::Map<String, Value>) -> Vec<SecretField> {
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
        let has_query = url::Url::parse(raw)
            .map(|url| url.query().is_some_and(|q| !q.is_empty()))
            // A URL we cannot parse still counts if it carries a `?query`.
            .unwrap_or_else(|_| raw.split_once('?').is_some_and(|(_, q)| !q.is_empty()));
        if has_query {
            secrets.push(SecretField {
                location: SecretLocation::UrlQuery,
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
fn mask_secrets(config: &Value, secrets: &[SecretField]) -> Value {
    let mut masked = config.clone();
    let Some(object) = masked.as_object_mut() else {
        return masked;
    };
    for secret in secrets {
        let marker = Value::String(SECRET_MARKER.to_string());
        match &secret.location {
            SecretLocation::Env(field) => mask_field(object, "env", field, marker),
            SecretLocation::Header(field) => mask_field(object, "headers", field, marker),
            SecretLocation::UrlQuery => {
                object.insert("url".to_string(), marker);
            }
        }
    }
    masked
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

/// The per-installation key, created on first use. Any failure (unreadable, corrupt, not
/// writable) gives `None`: the caller degrades, it never panics and never overwrites an
/// existing file it could not read.
fn load_or_create_key(dir: &Path) -> Option<[u8; KEY_BYTES]> {
    let path = dir.join(KEY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => return parse_key(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return None,
    }
    std::fs::create_dir_all(dir).ok()?;
    let key: [u8; KEY_BYTES] = rand::random();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            if file.write_all(hex(&key).as_bytes()).is_err() {
                drop(file);
                // Do not leave a truncated key that every later run would reject.
                let _ = std::fs::remove_file(&path);
                return None;
            }
            Some(key)
        }
        // Another process created it first: use theirs.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            parse_key(&std::fs::read_to_string(&path).ok()?)
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
fn canonical_json(value: &Value, out: &mut String) {
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
