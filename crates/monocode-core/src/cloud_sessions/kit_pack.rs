//! Builds the tar the Host's `kit.apply` unpacks, from the items a start selected
//! (design 3.2.3 and 3.7, rules D6, D7 and D12).
//!
//! # What goes in
//!
//! Only `skills/<name>/...`, `plugins/<id>/...`, `mcp.json` (`{"mcpServers": {...}}`) and
//! `kit.json`, as a plain, uncompressed tar with regular files and directories only. The
//! Host refuses anything else, so the same limits are checked here first: 128 MiB in total,
//! 64 MiB per file, 50 000 entries, paths of at most 512 bytes and 24 segments.
//!
//! # Deterministic
//!
//! The same selection and the same files give the same bytes: entries sorted by path,
//! mtime 0, owner 0, mode 0644 or 0755. Its SHA-256 is the kit hash `kit.apply` takes.
//!
//! # Safe reading
//!
//! The scan checked links once ([`super::kit`]). A link swapped in afterwards must not pull
//! a private file into the kit, so each file is checked again right before it is read: its
//! recorded canonical path is still inside the item's canonical root, no part of the path
//! is a link now, it is opened without following a link, it is a regular file with the
//! device and inode the scan saw, and its size did not change while it was read. An item
//! with any doubt is refused with a warning and nothing of it is sent.
//!
//! # Secrets
//!
//! A secret value of an MCP server (an `env` or `headers` value, or a URL with a query,
//! userinfo or fragment) never enters the tar. `mcp.json` carries `${MONOPTAH_KIT_<SERVER>_<FIELD>}`
//! in its place, and the values go to the Host once through `kit.secrets` as [`Packed::secrets`].
//! OAuth-style servers (D12) are carried config-only. Nothing here reads the Keychain or a
//! credentials file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::kit::{
    canonical_json, digest_str, find_secrets, hex, mask_secrets, KitFile, KitInventory, KitRoots,
    McpCompat, McpScope, Runtime, SecretLocation,
};
use super::kit_prefs::{self, CloudKitPrefs};

pub const MAX_TAR_ENTRIES: usize = 50_000;
pub const MAX_TAR_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 512;
const MAX_SEGMENT_BYTES: usize = 255;
const MAX_DEPTH: usize = 24;
/// Prefix of every secret name; the Host accepts `MONOPTAH_KIT_[A-Z0-9_]{1,128}`.
pub const SECRET_PREFIX: &str = "MONOPTAH_KIT_";
const MAX_SECRET_TAIL: usize = 128;
/// Entries added after the items: `kit.json`, `mcp.json`, `skills/` and `plugins/`.
const EXTRA_ENTRIES: usize = 4;

/// The key the Host compares paths by: NFC and lower case.
fn fold(path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    path.nfc().collect::<String>().to_lowercase()
}

/// Commands asked of `kit.probe`, in the order of the design.
pub const PROBE_COMMANDS: [&str; 6] = ["npx", "uvx", "bunx", "node", "python3", "docker"];

/// The probe command that stands for `runtime`.
pub fn probe_command(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Npx => "npx",
        Runtime::Uvx => "uvx",
        Runtime::Bunx => "bunx",
        Runtime::Node => "node",
        Runtime::Python => "python3",
        Runtime::Docker => "docker",
    }
}

fn runtime_word(runtime: Runtime) -> &'static str {
    match runtime {
        Runtime::Python => "python",
        other => probe_command(other),
    }
}

/// What `kit.probe` found on the workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Present(Option<BTreeSet<String>>);

impl Present {
    /// The probe answered with these commands.
    pub fn known(commands: impl IntoIterator<Item = String>) -> Self {
        Self(Some(commands.into_iter().collect()))
    }
    /// The probe failed: nothing is dropped for a missing runtime, the health check will say.
    pub fn unknown() -> Self {
        Self(None)
    }
    pub fn is_known(&self) -> bool {
        self.0.is_some()
    }
    fn has(&self, runtime: Runtime) -> bool {
        self.0
            .as_ref()
            .is_none_or(|set| set.contains(probe_command(runtime)))
    }
}

/// One MCP server that went into `mcp.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedServer {
    pub name: String,
    /// Carried config-only, the user signs in in the cloud (D12).
    pub oauth_style: bool,
    /// Hosts the server talks to: the host of its URL, or the package registries of its
    /// runtime. Only used for the sandbox network-policy hint; never a value.
    pub hosts: Vec<String>,
}

/// Package registries a runtime fetches from, for network-policy hints.
fn registry_hosts(runtime: Runtime) -> &'static [&'static str] {
    match runtime {
        Runtime::Npx | Runtime::Bunx | Runtime::Node => &["registry.npmjs.org"],
        Runtime::Uvx | Runtime::Python => &["pypi.org", "files.pythonhosted.org"],
        Runtime::Docker => &[],
    }
}

/// The finished kit. Dropping it deletes the tar and wipes the secret values.
pub struct Packed {
    pub file: tempfile::NamedTempFile,
    /// SHA-256 of the tar, hex: the kit hash.
    pub sha256: String,
    pub size: u64,
    /// `(name, value)` for `kit.secrets`. Values are wiped on drop.
    pub secrets: Vec<(String, Zeroizing<String>)>,
    /// Names in `secrets` whose value is a whole URL (userinfo, query or fragment).
    pub url_secrets: BTreeSet<String>,
    /// More text to keep out of logs: token-looking parts of `args`, `command` and URL paths
    /// of the servers that are sent. Wiped on drop.
    pub scrub_extra: Vec<Zeroizing<String>>,
    /// Things the user should know: dropped items and doubts. Never a secret value.
    pub warnings: Vec<String>,
    pub skills: Vec<String>,
    pub plugins: Vec<String>,
    pub servers: Vec<PackedServer>,
}

impl Packed {
    /// Nothing was selected or everything was refused: there is nothing to send.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty() && self.plugins.is_empty() && self.servers.is_empty()
    }
    /// The secrets as the `values` object of `kit.secrets`.
    pub fn secrets_value(&self) -> Value {
        let mut values = Map::new();
        for (name, value) in &self.secrets {
            values.insert(name.clone(), Value::String(value.as_str().to_owned()));
        }
        Value::Object(values)
    }
}

impl fmt::Debug for Packed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Packed")
            .field("sha256", &self.sha256)
            .field("size", &self.size)
            .field("secrets", &self.secrets.len())
            .field("scrub_extra", &self.scrub_extra.len())
            .field("skills", &self.skills)
            .field("plugins", &self.plugins)
            .field("servers", &self.servers)
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// A token that changes when anything the selection takes changes, secret values included.
/// It mixes the keyed secret digests, so it only compares on this install. Do not log it.
pub fn selection_token(inventory: &KitInventory, prefs: &CloudKitPrefs) -> String {
    let selected = kit_prefs::selected(inventory, prefs);
    let mut hasher = Sha256::new();
    for skill in &selected.skills {
        hasher.update(digest_str(&["skill", &kit_prefs::skill_id(skill), &skill.hash]).as_bytes());
    }
    for plugin in &selected.plugins {
        hasher.update(
            digest_str(&["plugin", &kit_prefs::plugin_id(plugin), &plugin.hash]).as_bytes(),
        );
    }
    for server in &selected.servers {
        hasher.update(
            digest_str(&["mcp", &kit_prefs::mcp_id(server), &server.config_hash]).as_bytes(),
        );
        if let Some(digest) = &server.secrets_digest {
            digest.update(&mut hasher);
        }
    }
    hex(&hasher.finalize())
}

/// One entry of the tar before it is written.
struct Entry {
    dir: bool,
    data: Vec<u8>,
    exec: bool,
}

/// Why a whole item was refused. Never carries a path outside the item or a value.
type Refusal = String;

/// Builds the kit for the selection in `prefs`. `Err` only when the tar cannot be written
/// at all; every problem with an item is a warning in [`Packed::warnings`].
pub fn pack(
    inventory: &KitInventory,
    roots: &KitRoots,
    prefs: &CloudKitPrefs,
    present: &Present,
) -> Result<Packed, String> {
    let selected = kit_prefs::selected(inventory, prefs);
    let mut warnings: Vec<String> = Vec::new();
    let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
    let mut total: u64 = 0;
    let mut skills = Vec::new();
    let mut plugins = Vec::new();

    // Files are sent only when the scan could vouch for every one of them.
    let mut files_ok = true;
    if inventory.file_index_incomplete {
        files_ok = false;
        warnings.push(
            "Skills and plugins were not sent: the scan could not prove that no file is a link to a credentials file."
                .into(),
        );
    } else if kit_prefs::selected_compressed(inventory, prefs).0
        > super::kit::MAX_KIT_COMPRESSED_BYTES
    {
        files_ok = false;
        warnings.push(
            "Skills and plugins were not sent: the selection is larger than 50 MB compressed. Untick some in Settings > Cloud."
                .into(),
        );
    }

    if files_ok {
        let mut names = BTreeSet::new();
        for skill in &selected.skills {
            if !valid_skill_name(&skill.name) {
                warnings.push(format!(
                    "Skill \"{}\" was not sent: its name is not a plain lowercase name.",
                    skill.name
                ));
                continue;
            }
            if !names.insert(skill.name.clone()) {
                warnings.push(format!(
                    "Skill \"{}\" was not sent: another skill with the same name is.",
                    skill.name
                ));
                continue;
            }
            let prefix = format!("skills/{}", skill.name);
            match item_entries(&prefix, &skill.path, &skill.files, entries.len(), total) {
                Ok((items, bytes)) => {
                    total += bytes;
                    entries.extend(items);
                    skills.push(skill.name.clone());
                }
                Err(why) => warnings.push(format!("Skill \"{}\" was not sent: {why}.", skill.name)),
            }
        }
        let mut ids = BTreeSet::new();
        for plugin in &selected.plugins {
            if !valid_plugin_id(&plugin.id) {
                warnings.push(format!(
                    "Plugin \"{}\" was not sent: its id has characters the workspace does not accept.",
                    plugin.id
                ));
                continue;
            }
            if !ids.insert(fold(&plugin.id)) {
                warnings.push(format!(
                    "Plugin \"{}\" was not sent: another plugin whose id differs only in letter case is.",
                    plugin.id
                ));
                continue;
            }
            let prefix = format!("plugins/{}", plugin.id);
            match item_entries(
                &prefix,
                &plugin.install_path,
                &plugin.files,
                entries.len(),
                total,
            ) {
                Ok((items, bytes)) => {
                    total += bytes;
                    entries.extend(items);
                    plugins.push(plugin.id.clone());
                    if plugin.has_native_bin {
                        warnings.push(format!(
                            "Plugin \"{}\" has programs built for this computer and may not run on Linux.",
                            plugin.id
                        ));
                    }
                }
                Err(why) => warnings.push(format!("Plugin \"{}\" was not sent: {why}.", plugin.id)),
            }
        }
    }

    let mcp = build_mcp(&selected.servers, roots, present, &mut warnings);
    if !mcp.servers.is_empty() {
        let mut text = String::new();
        canonical_json(
            &json!({ "mcpServers": Value::Object(mcp.config) }),
            &mut text,
        );
        entries.insert(
            "mcp.json".into(),
            Entry {
                dir: false,
                data: text.into_bytes(),
                exec: false,
            },
        );
    }

    let packed_nothing = skills.is_empty() && plugins.is_empty() && mcp.servers.is_empty();
    if !packed_nothing {
        let mut manifest = String::new();
        canonical_json(
            &json!({
                "version": 1,
                "contentHash": inventory.content_hash,
                "skills": skills,
                "plugins": plugins,
                "mcpServers": mcp.servers.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                "signInInCloud": mcp.servers.iter().filter(|s| s.oauth_style).map(|s| s.name.clone()).collect::<Vec<_>>(),
            }),
            &mut manifest,
        );
        entries.insert(
            "kit.json".into(),
            Entry {
                dir: false,
                data: manifest.into_bytes(),
                exec: false,
            },
        );
        if !skills.is_empty() {
            entries.insert("skills".into(), dir_entry());
        }
        if !plugins.is_empty() {
            entries.insert("plugins".into(), dir_entry());
        }
    }

    let (file, sha256, size) = write_tar(&entries)?;
    Ok(Packed {
        file,
        sha256,
        size,
        secrets: mcp.secrets,
        url_secrets: mcp.url_secrets,
        scrub_extra: mcp.scrub_extra,
        warnings,
        skills,
        plugins,
        servers: mcp.servers,
    })
}

fn dir_entry() -> Entry {
    Entry {
        dir: true,
        data: Vec::new(),
        exec: true,
    }
}

/// The names the Host accepts (`validateSkillName`).
pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// `^[A-Za-z0-9@][A-Za-z0-9._@-]{0,127}$`.
pub fn valid_plugin_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'@')
        && id.len() <= 128
        && bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b'-'))
}

/// `^[A-Za-z0-9._@-]{1,128}$`.
pub fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b'-'))
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Opens `path` without following a link where the platform allows it.
#[cfg(unix)]
fn open_plain(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}
#[cfg(not(unix))]
fn open_plain(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

#[cfg(unix)]
fn id_and_mode(meta: &std::fs::Metadata) -> (Option<(u64, u64)>, bool) {
    use std::os::unix::fs::MetadataExt;
    (Some((meta.dev(), meta.ino())), meta.mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn id_and_mode(_meta: &std::fs::Metadata) -> (Option<(u64, u64)>, bool) {
    (None, false)
}

/// Reads one scanned file after checking it is still what the scan saw.
fn read_file(file: &KitFile, root: &Path) -> Result<(Vec<u8>, bool), Refusal> {
    let changed = || "a file changed or was replaced after it was scanned".to_string();
    if !file.path.starts_with(root) {
        return Err(changed());
    }
    // No part of the path may be a link now.
    match std::fs::canonicalize(&file.path) {
        Ok(real) if real == file.path => {}
        _ => return Err(changed()),
    }
    match std::fs::symlink_metadata(&file.path) {
        Ok(meta) if meta.file_type().is_file() => {}
        _ => return Err(changed()),
    }
    let mut handle = open_plain(&file.path).map_err(|_| changed())?;
    let meta = handle.metadata().map_err(|_| changed())?;
    if !meta.is_file() {
        return Err(changed());
    }
    let (id, exec) = id_and_mode(&meta);
    if file.id.is_some() && id != file.id {
        return Err(changed());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err("a file is larger than 64 MiB".into());
    }
    let mut data = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    (&mut handle)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|_| "a file could not be read".to_string())?;
    let after = handle.metadata().map_err(|_| changed())?;
    if data.len() as u64 != meta.len() || after.len() != meta.len() {
        return Err(changed());
    }
    Ok((data, exec))
}

fn plain_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.len() <= MAX_SEGMENT_BYTES
        && !segment.contains('\\')
        && !segment.chars().any(char::is_control)
}

/// All entries of one item, or the reason it is refused. `used_entries` and `used_bytes` are
/// what earlier items took, for the caps.
fn item_entries(
    prefix: &str,
    root: &Path,
    files: &[KitFile],
    used_entries: usize,
    used_bytes: u64,
) -> Result<(BTreeMap<String, Entry>, u64), Refusal> {
    if files.is_empty() {
        return Err("it has no files".into());
    }
    let real_root =
        std::fs::canonicalize(root).map_err(|_| "its folder could not be read".to_string())?;
    let mut out = BTreeMap::new();
    out.insert(prefix.to_owned(), dir_entry());
    let mut bytes = 0u64;
    let mut folded: BTreeSet<String> = BTreeSet::new();
    for file in files {
        // The name inside the item, as the item itself names it (a link keeps its own name).
        let mut segments = Vec::new();
        for component in file.name.components() {
            match component {
                Component::Normal(part) => segments.push(
                    part.to_str()
                        .filter(|s| plain_segment(s))
                        .ok_or_else(|| "a file name is not plain text".to_string())?,
                ),
                _ => return Err("a file path is not plain".into()),
            }
        }
        if segments.is_empty() {
            return Err("a file path is not plain".into());
        }
        let path = format!("{prefix}/{}", segments.join("/"));
        if path.len() > MAX_PATH_BYTES || path.matches('/').count() + 1 > MAX_DEPTH {
            return Err("a path is too long or too deep".into());
        }
        let (data, exec) = read_file(file, &real_root)?;
        bytes += data.len() as u64;
        if used_bytes + bytes > MAX_TAR_BYTES {
            return Err("the kit would be larger than 128 MiB".into());
        }
        // Directories between the item folder and the file.
        let mut dir = prefix.to_owned();
        for segment in &segments[..segments.len() - 1] {
            dir = format!("{dir}/{segment}");
            out.entry(dir.clone()).or_insert_with(dir_entry);
        }
        // The Host folds case and normalizes Unicode when it checks for duplicates.
        if !folded.insert(fold(&path)) {
            return Err("two files differ only in letter case or Unicode form".into());
        }
        if out
            .insert(
                path,
                Entry {
                    dir: false,
                    data,
                    exec,
                },
            )
            .is_some()
        {
            return Err("two files have the same path".into());
        }
        // Room for kit.json, mcp.json, skills/ and plugins/, which are added later.
        if used_entries + out.len() + EXTRA_ENTRIES > MAX_TAR_ENTRIES {
            return Err("it has too many files".into());
        }
    }
    Ok((out, bytes))
}

fn write_tar(
    entries: &BTreeMap<String, Entry>,
) -> Result<(tempfile::NamedTempFile, String, u64), String> {
    let mut file = tempfile::Builder::new()
        .prefix("monoptah-kit-")
        .suffix(".tar")
        .tempfile()
        .map_err(|e| format!("Cannot create the kit file: {e}"))?;
    {
        let mut builder = tar::Builder::new(std::io::BufWriter::new(file.as_file_mut()));
        for (path, entry) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_entry_type(if entry.dir {
                tar::EntryType::Directory
            } else {
                tar::EntryType::Regular
            });
            header.set_size(entry.data.len() as u64);
            header.set_mode(if entry.exec { 0o755 } else { 0o644 });
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            builder
                .append_data(&mut header, path, entry.data.as_slice())
                .map_err(|e| format!("Cannot write the kit file: {e}"))?;
        }
        let mut writer = builder
            .into_inner()
            .map_err(|e| format!("Cannot write the kit file: {e}"))?;
        writer
            .flush()
            .map_err(|e| format!("Cannot write the kit file: {e}"))?;
    }
    let mut hasher = Sha256::new();
    let mut reader = File::open(file.path()).map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((file, hex(&hasher.finalize()), size))
}

// ---------------------------------------------------------------------------
// MCP servers
// ---------------------------------------------------------------------------

struct McpPart {
    config: Map<String, Value>,
    servers: Vec<PackedServer>,
    secrets: Vec<(String, Zeroizing<String>)>,
    url_secrets: BTreeSet<String>,
    scrub_extra: Vec<Zeroizing<String>>,
}

/// A config read for packing. Its strings are wiped when it goes out of scope, because it
/// still holds the secret values.
struct Wiped(Value);
impl Drop for Wiped {
    fn drop(&mut self) {
        fn wipe(value: &mut Value) {
            match value {
                Value::String(text) => zeroize::Zeroize::zeroize(text),
                Value::Array(items) => items.iter_mut().for_each(wipe),
                Value::Object(map) => map.values_mut().for_each(wipe),
                _ => {}
            }
        }
        wipe(&mut self.0);
    }
}

/// Parts of a server config that a tool may print back and that look like a credential:
/// `KEY=value` values, long mixed tokens in `args` and `command`, and long mixed path
/// segments of the URL. Short plain words ("-y", package names) are left alone.
fn token_like_parts(object: &Map<String, Value>) -> Vec<String> {
    fn tokenish(text: &str) -> bool {
        text.len() >= 16
            && !text.contains(['/', ' ', '$'])
            && text.bytes().any(|b| b.is_ascii_digit())
            && text.bytes().any(|b| b.is_ascii_alphabetic())
    }
    let mut out = Vec::new();
    let mut consider = |text: &str| {
        if text.contains("${") {
            return;
        }
        if let Some((_, value)) = text.split_once('=') {
            if value.len() >= 4 {
                out.push(value.to_owned());
            }
        } else if tokenish(text) {
            out.push(text.to_owned());
        }
    };
    if let Some(command) = object.get("command").and_then(Value::as_str) {
        consider(command);
    }
    for arg in object
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        consider(arg);
    }
    if let Some(url) = object
        .get("url")
        .and_then(Value::as_str)
        .and_then(|raw| url::Url::parse(raw).ok())
    {
        for segment in url.path_segments().into_iter().flatten() {
            if tokenish(segment) {
                out.push(segment.to_owned());
            }
        }
    }
    out
}

/// `API-key` -> `API_KEY`: upper case letters and digits, everything else one `_`.
fn sanitize(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "X".into()
    } else {
        trimmed.to_owned()
    }
}

/// `MONOPTAH_KIT_<SERVER>_<FIELD>`, unique among `used`.
fn secret_name(server: &str, field: &str, used: &mut BTreeSet<String>) -> String {
    let mut tail = format!("{}_{}", sanitize(server), sanitize(field));
    tail.truncate(MAX_SECRET_TAIL - 4);
    let mut name = format!("{SECRET_PREFIX}{tail}");
    let mut n = 2;
    while !used.insert(name.clone()) {
        name = format!("{SECRET_PREFIX}{tail}_{n}");
        n += 1;
    }
    name
}

/// The server's own entry in the config file it was found in, read again now.
fn read_config(server: &super::kit::KitMcpServer, roots: &KitRoots) -> Option<Wiped> {
    let json = crate::mcp::read_json(&server.config_path)?;
    let servers = match server.scope {
        McpScope::User => json.get("mcpServers"),
        McpScope::Local => json
            .get("projects")?
            .get(roots.project.to_string_lossy().as_ref())?
            .get("mcpServers"),
        McpScope::Project => None,
    }?;
    servers.get(&server.name).cloned().map(Wiped)
}

fn build_mcp(
    selected: &[&super::kit::KitMcpServer],
    roots: &KitRoots,
    present: &Present,
    warnings: &mut Vec<String>,
) -> McpPart {
    // One server per name in `mcp.json`: the project's local one wins over a personal one,
    // like Claude's own scopes.
    let mut by_name: BTreeMap<&str, &super::kit::KitMcpServer> = BTreeMap::new();
    for server in selected {
        if let Some(old) = by_name.insert(&server.name, server) {
            if old.scope != server.scope {
                warnings.push(format!(
                    "MCP server \"{}\": the {} one replaces the {} one.",
                    server.name,
                    scope_word(server.scope),
                    scope_word(old.scope)
                ));
            }
        }
    }
    let mut part = McpPart {
        config: Map::new(),
        servers: Vec::new(),
        secrets: Vec::new(),
        url_secrets: BTreeSet::new(),
        scrub_extra: Vec::new(),
    };
    let mut used = BTreeSet::new();
    for (name, server) in by_name {
        if !valid_server_name(name) {
            warnings.push(format!(
                "MCP server \"{name}\" was not sent: its name has characters the workspace does not accept."
            ));
            continue;
        }
        if let McpCompat::NeedsRuntime(runtime) = server.compat {
            if !present.has(runtime) {
                warnings.push(format!(
                    "MCP server \"{name}\" was not sent: it needs {}, which this workspace does not have.",
                    runtime_word(runtime)
                ));
                continue;
            }
        }
        let Some(config) = read_config(server, roots) else {
            warnings.push(format!(
                "MCP server \"{name}\" was not sent: its configuration could not be read again."
            ));
            continue;
        };
        let config = config;
        let Some(object) = config.0.as_object() else {
            continue;
        };
        // A header helper is a local program that prints headers: it cannot run on the
        // workspace and there is no broker for it (D7).
        if object.contains_key("headersHelper") {
            warnings.push(format!(
                "MCP server \"{name}\" was not sent: it uses a headers helper program, which cannot run in the workspace."
            ));
            continue;
        }
        let fresh = find_secrets(object);
        let mut canonical = String::new();
        canonical_json(&mask_secrets(&config.0, &fresh), &mut canonical);
        if digest_str(&[&canonical]) != server.config_hash {
            warnings.push(format!(
                "MCP server \"{name}\" was not sent: its configuration changed after it was scanned."
            ));
            continue;
        }
        let mut out = object.clone();
        for field in &fresh {
            let (group, key, label) = match &field.location {
                SecretLocation::Env(key) => (Some("env"), key.as_str(), key.clone()),
                SecretLocation::Header(key) => {
                    (Some("headers"), key.as_str(), format!("HEADER_{key}"))
                }
                SecretLocation::UrlQuery
                | SecretLocation::UrlUserinfo
                | SecretLocation::UrlFragment => (None, "url", "URL".to_owned()),
            };
            let secret = secret_name(name, &label, &mut used);
            // `${HOME}/x` or `${X:-d}` mixed into text is not expanded as a whole value the
            // way a lone reference is: it travels literally.
            if field.value.expose().contains("${") {
                warnings.push(format!(
                    "MCP server \"{name}\": a value contains a variable reference inside other text, so it is sent as written and not expanded in the workspace."
                ));
            }
            if group.is_none() {
                part.url_secrets.insert(secret.clone());
            }
            let reference = Value::String(format!("${{{secret}}}"));
            match group {
                Some(group) => {
                    if let Some(map) = out.get_mut(group).and_then(Value::as_object_mut) {
                        map.insert(key.to_owned(), reference);
                    }
                }
                None => {
                    out.insert("url".into(), reference);
                }
            }
            part.secrets
                .push((secret, Zeroizing::new(field.value.expose().to_owned())));
        }
        part.scrub_extra
            .extend(token_like_parts(object).into_iter().map(Zeroizing::new));
        part.config.insert(name.to_owned(), Value::Object(out));
        let mut hosts: Vec<String> = object
            .get("url")
            .and_then(Value::as_str)
            .and_then(|raw| url::Url::parse(raw).ok())
            .and_then(|url| url.host_str().map(str::to_owned))
            .into_iter()
            .collect();
        if let McpCompat::NeedsRuntime(runtime) = server.compat {
            hosts.extend(registry_hosts(runtime).iter().map(|h| (*h).to_owned()));
        }
        part.servers.push(PackedServer {
            name: name.to_owned(),
            oauth_style: server.compat == McpCompat::OAuthStyle,
            hosts,
        });
    }
    part.secrets.sort_by(|a, b| a.0.cmp(&b.0));
    part
}

fn scope_word(scope: McpScope) -> &'static str {
    match scope {
        McpScope::User => "personal",
        McpScope::Local => "project-local",
        McpScope::Project => "project",
    }
}
