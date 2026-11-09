//! Chunked reads of `remote://` files through the Host's `files.readBytes`.
//!
//! `files.readBytes` is keyed by `projectId` (plus an optional worktree `cwd`), not by path,
//! so a `remote://<env>/<path>` path is first mapped onto the registered project that owns
//! it. A Host without the capability is read with the single `read_binary_file` reply instead.
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Bytes asked of the Host per call: its own cap, and a multiple of three so that the
/// base64 of every chunk but the last has no padding.
pub const MAX_READ_CHUNK: u64 = 393_216;
/// Largest file the single-reply `read_binary_file` fallback can carry.
pub(crate) const MAX_LEGACY_BYTES: usize = 10 * 1024 * 1024;
/// The error of a read stopped by the caller.
pub const READ_CANCELLED: &str = "Read cancelled";
const TOO_LARGE: &str = "File is too large to preview (maximum 32 MB).";
const PROJECT_TTL: Duration = Duration::from_secs(5);
const CAPABILITY_TTL: Duration = Duration::from_secs(60);

/// What the chunked read needs from a connection, so tests can stand in a fake Host.
pub(crate) trait Transport {
    /// The connection machine id of a remote environment.
    fn machine_for_environment(&self, environment: &str) -> Result<String, String>;
    fn request(&self, machine: &str, method: &str, params: Value) -> Result<Value, String>;
}

impl Transport for crate::remote::Connections {
    fn machine_for_environment(&self, environment: &str) -> Result<String, String> {
        Self::machine_for_environment(self, environment).map(|machine| machine.id)
    }
    fn request(&self, machine: &str, method: &str, params: Value) -> Result<Value, String> {
        Self::request(self, machine, method, params)
    }
}

#[derive(Clone, Deserialize)]
struct Project {
    id: String,
    cwd: String,
}
#[derive(Clone, Deserialize)]
struct Worktree {
    path: String,
    #[serde(default)]
    missing: bool,
}
#[derive(Deserialize)]
struct Worktrees {
    worktrees: Vec<Worktree>,
}

/// Short lived answers per key, so a burst of reads asks the Host once.
struct Ttl<K, V> {
    entries: Mutex<HashMap<K, (Instant, V)>>,
    ttl: Duration,
}
impl<K: std::hash::Hash + Eq + Clone, V: Clone> Ttl<K, V> {
    fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }
    fn forget(&self, key: &K) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(key);
        }
    }
    fn get_or(&self, key: &K, load: impl FnOnce() -> Result<V, String>) -> Result<V, String> {
        if let Ok(entries) = self.entries.lock() {
            if let Some((at, value)) = entries.get(key) {
                if at.elapsed() < self.ttl {
                    return Ok(value.clone());
                }
            }
        }
        let value = load()?;
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|_, (at, _)| at.elapsed() < self.ttl);
            entries.insert(key.clone(), (Instant::now(), value.clone()));
        }
        Ok(value)
    }
}

pub(crate) struct Cache {
    projects: Ttl<String, Vec<Project>>,
    /// `None` caches a failed listing, so a read does not ask again for every project.
    worktrees: Ttl<(String, String), Option<Vec<Worktree>>>,
    capable: Ttl<String, bool>,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            projects: Ttl::new(PROJECT_TTL),
            worktrees: Ttl::new(PROJECT_TTL),
            capable: Ttl::new(CAPABILITY_TTL),
        }
    }
}

static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

/// Where `files.readBytes` finds a file: a project, its relative path, and the worktree.
#[derive(Debug, PartialEq, Eq)]
struct Located {
    project_id: String,
    rel: String,
    cwd: Option<String>,
}

fn slashed(path: &str) -> String {
    let path = path.replace('\\', "/");
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.to_owned()
    }
}

/// The part of `path` below `root`, or `None` when it is not inside it.
fn below(path: &str, root: &str) -> Option<String> {
    let (path, root) = (slashed(path), slashed(root));
    if !super::is_equal_or_inside(&path, &root) {
        return None;
    }
    let skip = if root == "/" { 0 } else { root.chars().count() };
    Some(
        path.chars()
            .skip(skip)
            .collect::<String>()
            .trim_start_matches('/')
            .to_owned(),
    )
}

fn locate(
    transport: &dyn Transport,
    cache: &Cache,
    machine: &str,
    host_path: &str,
) -> Result<Located, String> {
    let outside = || format!("{host_path} is outside every project on this machine.");
    // Dot segments are not resolved here (symlinks make that the Host's job).
    if host_path
        .split(['/', '\\'])
        .any(|part| part == ".." || part == ".")
    {
        return Err(outside());
    }
    let projects = cache.projects.get_or(&machine.to_owned(), || {
        serde_json::from_value(transport.request(machine, "projects.list", json!({}))?)
            .map_err(|error| format!("Invalid project list: {error}"))
    })?;
    let worktrees = |project: &Project| -> Vec<Worktree> {
        cache
            .worktrees
            .get_or(&(machine.to_owned(), project.id.clone()), || {
                let listed = transport
                    .request(machine, "git.worktrees", json!({"projectId": project.id}))
                    .and_then(|listed| {
                        serde_json::from_value::<Worktrees>(listed).map_err(|e| e.to_string())
                    });
                Ok(listed.ok().map(|listed| listed.worktrees))
            })
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    // The longest root wins, so a project nested in another is read through itself.
    let owner = projects
        .iter()
        .filter(|project| below(host_path, &project.cwd).is_some())
        .max_by_key(|project| slashed(&project.cwd).len());
    // Worktrees often sit beside the project, or inside it, so they are always consulted.
    let mut best: Option<(&Project, String, Option<String>, usize)> = owner.map(|project| {
        let rel = below(host_path, &project.cwd).unwrap_or_default();
        (project, rel, None, slashed(&project.cwd).len())
    });
    let candidates: Vec<&Project> = match owner {
        Some(project) => vec![project],
        None => projects.iter().collect(),
    };
    for project in candidates {
        for tree in worktrees(project).iter().filter(|tree| !tree.missing) {
            let Some(rel) = below(host_path, &tree.path) else {
                continue;
            };
            let len = slashed(&tree.path).len();
            if best
                .as_ref()
                .is_none_or(|(_, _, _, longest)| len > *longest)
            {
                best = Some((project, rel, Some(tree.path.clone()), len));
            }
        }
    }
    let (project, rel, cwd, _) = best.ok_or_else(outside)?;
    if rel.is_empty() {
        return Err(format!("{host_path} is a folder, not a file."));
    }
    Ok(Located {
        project_id: project.id.clone(),
        rel,
        cwd,
    })
}

fn has_capability(transport: &dyn Transport, cache: &Cache, machine: &str) -> Result<bool, String> {
    cache.capable.get_or(&machine.to_owned(), || {
        let described = transport.request(machine, "environment.describe", json!({}))?;
        Ok(described["capabilities"]
            .as_array()
            .is_some_and(|all| all.iter().any(|c| c == "files.readBytes")))
    })
}

fn invalid(what: &str) -> String {
    format!("Invalid remote file content: {what}")
}

/// The outcome of [`read_bytes`].
#[derive(Debug, PartialEq, Eq)]
pub enum Fetched {
    Bytes(Vec<u8>),
    /// `accept` turned the file down after its first chunk; no further chunk was asked for.
    Refused(String),
}

/// Reads the whole host file behind a `remote://` path into memory.
///
/// `progress(done, total)` is called after every chunk. `cancel` is checked before every
/// chunk, so a cancel stops after the chunk in flight and returns [`READ_CANCELLED`].
/// `accept(first chunk, total size)` runs once the size is known and can refuse the file
/// before any more of it is downloaded.
///
/// When the path cannot be mapped onto a project, or the Host turns out not to know
/// `files.readBytes`, the single reply `read_binary_file` is used and the Host decides.
pub(crate) fn read_bytes(
    transport: &dyn Transport,
    cache: &Cache,
    path: &std::path::Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
    accept: &dyn Fn(&[u8], u64) -> Option<String>,
) -> Result<Fetched, String> {
    let remote = crate::remote::parse_remote_path(&path.to_string_lossy())
        .ok_or("Invalid remote file path")?;
    let machine = transport.machine_for_environment(&remote.environment_id)?;
    if !has_capability(transport, cache, &machine)? {
        return legacy(transport, &machine, &remote.host_path);
    }
    let Ok(found) = locate(transport, cache, &machine, &remote.host_path) else {
        return legacy(transport, &machine, &remote.host_path);
    };
    let mut out = Vec::new();
    let mut size = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(READ_CANCELLED.into());
        }
        let offset = out.len() as u64;
        let mut params = json!({
            "projectId": found.project_id,
            "path": found.rel,
            "offset": offset,
            "length": MAX_READ_CHUNK,
        });
        if let Some(cwd) = &found.cwd {
            params["cwd"] = json!(cwd);
        }
        let reply = match transport.request(&machine, "files.readBytes", params) {
            Ok(reply) => reply,
            Err(error)
                if offset == 0
                    && error
                        .to_ascii_lowercase()
                        .contains("unsupported host method") =>
            {
                // The capability cache was stale (the Host was replaced by an older one).
                cache.capable.forget(&machine);
                return legacy(transport, &machine, &remote.host_path);
            }
            Err(error) => return Err(error),
        };
        let data = base64::engine::general_purpose::STANDARD
            .decode(reply["data"].as_str().ok_or_else(|| invalid("no data"))?)
            .map_err(|error| invalid(&error.to_string()))?;
        let total = reply["size"].as_u64().ok_or_else(|| invalid("no size"))?;
        if reply["offset"].as_u64() != Some(offset + data.len() as u64)
            || data.len() as u64 > MAX_READ_CHUNK
        {
            return Err(invalid("unexpected range"));
        }
        if total > super::document::MAX_PREVIEW_BYTES {
            return Err(TOO_LARGE.into());
        }
        if size.is_some_and(|first| first != total) || (data.is_empty() && offset < total) {
            return Err("The file changed while it was being read. Try again.".into());
        }
        if size.is_none() {
            if let Some(reason) = accept(&data, total) {
                return Ok(Fetched::Refused(reason));
            }
        }
        size = Some(total);
        out.extend_from_slice(&data);
        progress(out.len() as u64, total);
        if out.len() as u64 >= total {
            return Ok(Fetched::Bytes(out));
        }
    }
}

/// Today's single reply read, for a Host without `files.readBytes`.
fn legacy(transport: &dyn Transport, machine: &str, host_path: &str) -> Result<Fetched, String> {
    let encoded = transport
        .request(
            machine,
            "workspace.run",
            json!({"command": "read_binary_file", "args": {"path": host_path}}),
        )
        .map_err(super::remote::workspace_error)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_str().ok_or_else(|| invalid("not a string"))?)
        .map_err(|error| invalid(&error.to_string()))?;
    if bytes.len() > MAX_LEGACY_BYTES {
        return Err("File is too large to preview (maximum 10 MB).".into());
    }
    Ok(Fetched::Bytes(bytes))
}

/// [`read_bytes`] over the app's shared connections.
pub(crate) fn read_default(
    path: &std::path::Path,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
    accept: &dyn Fn(&[u8], u64) -> Option<String>,
) -> Result<Fetched, String> {
    let connections = crate::remote::default_connections()?;
    read_bytes(&*connections, &CACHE, path, progress, cancel, accept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A Host with one project holding `data` at `src/big.bin`, serving from memory.
    struct Fake {
        capable: bool,
        data: Vec<u8>,
        calls: RefCell<Vec<(String, Value)>>,
        /// Set this flag once the n-th `files.readBytes` call was served.
        cancel_after: Option<(usize, &'static AtomicBool)>,
        projects_fail: bool,
        worktrees_fail: bool,
        /// Advertises `files.readBytes` but refuses it, like a Host replaced by an old one.
        stale_capability: bool,
    }
    impl Fake {
        fn new(capable: bool, data: Vec<u8>) -> Self {
            Self {
                capable,
                data,
                calls: RefCell::new(Vec::new()),
                cancel_after: None,
                projects_fail: false,
                worktrees_fail: false,
                stale_capability: false,
            }
        }
        fn methods(&self, method: &str) -> Vec<Value> {
            self.calls
                .borrow()
                .iter()
                .filter(|(m, _)| m == method)
                .map(|(_, p)| p.clone())
                .collect()
        }
    }
    impl Transport for Fake {
        fn machine_for_environment(&self, environment: &str) -> Result<String, String> {
            (environment == "env")
                .then(|| "machine".to_owned())
                .ok_or("Machine is no longer connected".into())
        }
        fn request(&self, machine: &str, method: &str, params: Value) -> Result<Value, String> {
            assert_eq!(machine, "machine");
            self.calls
                .borrow_mut()
                .push((method.to_owned(), params.clone()));
            let engine = base64::engine::general_purpose::STANDARD;
            match method {
                "environment.describe" => Ok(
                    json!({"capabilities": if self.capable || self.stale_capability {
                        json!(["files.read", "files.readBytes"])
                    } else {
                        json!(["files.read"])
                    }}),
                ),
                "projects.list" if self.projects_fail => Err("Host is busy".into()),
                "git.worktrees" if self.worktrees_fail => Err("git failed".into()),
                "projects.list" => Ok(json!([
                    {"id": "p1", "cwd": "/work/repo"},
                    {"id": "p2", "cwd": "/work/other"},
                ])),
                "git.worktrees" => Ok(if params["projectId"] == "p1" {
                    json!({"worktrees": [
                        {"path": "/work/repo", "missing": false},
                        {"path": "/work/trees/feature", "missing": false},
                        {"path": "/work/trees/gone", "missing": true},
                    ]})
                } else {
                    json!({"worktrees": []})
                }),
                "files.readBytes" if self.stale_capability => {
                    Err("Unsupported host method: files.readBytes".into())
                }
                "files.readBytes" => {
                    assert!(self.capable, "a Host without the capability was asked");
                    let offset = params["offset"].as_u64().unwrap() as usize;
                    let length = params["length"].as_u64().unwrap() as usize;
                    assert!(length as u64 <= MAX_READ_CHUNK);
                    let end = (offset + length).min(self.data.len());
                    let served = self
                        .calls
                        .borrow()
                        .iter()
                        .filter(|(m, _)| m == "files.readBytes")
                        .count();
                    if let Some((n, flag)) = self.cancel_after {
                        if served == n {
                            flag.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(json!({
                        "data": engine.encode(&self.data[offset.min(end)..end]),
                        "offset": end,
                        "size": self.data.len(),
                    }))
                }
                "workspace.run" => {
                    assert_eq!(params["command"], "read_binary_file");
                    // The Host canonicalises, so dot segments are resolved before the lookup.
                    let mut parts: Vec<&str> = Vec::new();
                    for part in params["args"]["path"].as_str().unwrap().split('/') {
                        match part {
                            "" | "." => {}
                            ".." => {
                                parts.pop();
                            }
                            part => parts.push(part),
                        }
                    }
                    if parts.join("/") == "work/repo/src/big.bin" {
                        Ok(json!(engine.encode(&self.data)))
                    } else {
                        Err("No such file".into())
                    }
                }
                other => panic!("unexpected {other}"),
            }
        }
    }

    type Progress = Vec<(u64, u64)>;

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }
    fn read(host: &Fake, path: &str, cancel: &AtomicBool) -> (Result<Vec<u8>, String>, Progress) {
        let mut seen = Vec::new();
        let result = read_bytes(
            host,
            &Cache::default(),
            std::path::Path::new(path),
            &mut |d, t| seen.push((d, t)),
            cancel,
            &|_, _| None,
        );
        let result = result.map(|fetched| match fetched {
            Fetched::Bytes(bytes) => bytes,
            Fetched::Refused(reason) => panic!("refused: {reason}"),
        });
        (result, seen)
    }

    #[test]
    fn three_megabytes_arrive_in_eight_chunks_and_decode_exactly() {
        let data = pattern(3 * 1024 * 1024);
        let host = Fake::new(true, data.clone());
        let (result, seen) = read(
            &host,
            "remote://env/work/repo/src/big.bin",
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap(), data);
        let reads = host.methods("files.readBytes");
        assert_eq!(reads.len(), 8);
        assert_eq!(
            reads[0],
            json!({"projectId":"p1","path":"src/big.bin","offset":0,"length":MAX_READ_CHUNK})
        );
        assert_eq!(reads[7]["offset"], 7 * MAX_READ_CHUNK);
        assert!(host.methods("workspace.run").is_empty());
        assert_eq!(seen.len(), 8);
        assert_eq!(seen[7], (data.len() as u64, data.len() as u64));
        assert_eq!(seen[0], (MAX_READ_CHUNK, data.len() as u64));
    }

    #[test]
    fn a_ragged_last_chunk_and_an_empty_file_are_exact() {
        let data = pattern(MAX_READ_CHUNK as usize + 5);
        let host = Fake::new(true, data.clone());
        let (result, _) = read(&host, "remote://env/work/repo/a", &AtomicBool::new(false));
        assert_eq!(result.unwrap(), data);
        let host = Fake::new(true, Vec::new());
        let (result, seen) = read(&host, "remote://env/work/repo/a", &AtomicBool::new(false));
        assert!(result.unwrap().is_empty());
        assert_eq!(seen, vec![(0, 0)]);
    }

    #[test]
    fn cancel_stops_after_the_current_chunk() {
        static CANCEL: AtomicBool = AtomicBool::new(false);
        let mut host = Fake::new(true, pattern(3 * 1024 * 1024));
        host.cancel_after = Some((3, &CANCEL));
        let (result, seen) = read(&host, "remote://env/work/repo/src/big.bin", &CANCEL);
        assert_eq!(result.unwrap_err(), READ_CANCELLED);
        assert_eq!(host.methods("files.readBytes").len(), 3);
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn without_the_capability_read_binary_file_is_used() {
        let data = pattern(200_000);
        let host = Fake::new(false, data.clone());
        let (result, seen) = read(
            &host,
            "remote://env/work/repo/src/big.bin",
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap(), data);
        assert!(host.methods("files.readBytes").is_empty());
        assert!(host.methods("projects.list").is_empty());
        assert_eq!(host.methods("workspace.run").len(), 1);
        assert!(seen.is_empty());
    }

    #[test]
    fn a_path_outside_every_project_is_left_to_the_host_and_is_an_error() {
        let host = Fake::new(true, pattern(10));
        for path in [
            "remote://env/etc/passwd",
            "remote://env/work/repository/a",
            "remote://env/work",
            "remote://env/work/repo",
        ] {
            let (result, _) = read(&host, path, &AtomicBool::new(false));
            assert_eq!(result.unwrap_err(), "No such file", "{path}");
        }
        assert!(host.methods("files.readBytes").is_empty());
        let (result, _) = read(
            &host,
            "remote://missing/work/repo/a",
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap_err(), "Machine is no longer connected");
    }

    #[test]
    fn dot_dot_paths_are_resolved_by_the_host_through_the_legacy_read() {
        let data = pattern(1000);
        let host = Fake::new(true, data.clone());
        let (result, _) = read(
            &host,
            "remote://env/work/repo/../repo/src/big.bin",
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap(), data);
        assert!(host.methods("files.readBytes").is_empty());
        assert_eq!(host.methods("workspace.run").len(), 1);
    }

    #[test]
    fn a_failing_project_list_falls_back_to_the_legacy_read() {
        let data = pattern(1000);
        let mut host = Fake::new(true, data.clone());
        host.projects_fail = true;
        let (result, _) = read(
            &host,
            "remote://env/work/repo/src/big.bin",
            &AtomicBool::new(false),
        );
        assert_eq!(result.unwrap(), data);
        assert_eq!(host.methods("workspace.run").len(), 1);
    }

    #[test]
    fn a_failing_worktree_list_is_asked_once_per_burst() {
        let mut host = Fake::new(true, pattern(10));
        host.worktrees_fail = true;
        let cache = Cache::default();
        for _ in 0..3 {
            let found = locate(&host, &cache, "machine", "/work/other/a.rs").unwrap();
            assert_eq!(found.project_id, "p2");
        }
        assert_eq!(host.methods("git.worktrees").len(), 1);
    }

    #[test]
    fn a_stale_capability_is_forgotten_and_the_legacy_read_serves_the_file() {
        let data = pattern(1000);
        let mut host = Fake::new(false, data.clone());
        host.stale_capability = true;
        let cache = Cache::default();
        let read_once = |host: &Fake| {
            read_bytes(
                host,
                &cache,
                std::path::Path::new("remote://env/work/repo/src/big.bin"),
                &mut |_, _| {},
                &AtomicBool::new(false),
                &|_, _| None,
            )
        };
        assert_eq!(read_once(&host).unwrap(), Fetched::Bytes(data));
        assert_eq!(host.methods("files.readBytes").len(), 1);
        // The cached answer was dropped, so the next read probes the Host again.
        read_once(&host).unwrap();
        assert_eq!(host.methods("environment.describe").len(), 2);
    }

    #[test]
    fn a_large_file_is_refused_after_its_first_chunk() {
        let host = Fake::new(true, pattern(20 * 1024 * 1024));
        let result = read_bytes(
            &host,
            &Cache::default(),
            std::path::Path::new("remote://env/work/repo/src/big.bin"),
            &mut |_, _| {},
            &AtomicBool::new(false),
            &|first, size| {
                assert_eq!(first.len() as u64, MAX_READ_CHUNK);
                (size > 1024 * 1024).then(|| "too large".to_owned())
            },
        )
        .unwrap();
        assert_eq!(result, Fetched::Refused("too large".into()));
        assert_eq!(host.methods("files.readBytes").len(), 1);
    }

    #[test]
    fn worktrees_send_their_cwd_and_a_path_relative_to_it() {
        let host = Fake::new(true, pattern(10));
        let cache = Cache::default();
        let found = locate(&host, &cache, "machine", "/work/trees/feature/src/a.rs").unwrap();
        assert_eq!(
            found,
            Located {
                project_id: "p1".into(),
                rel: "src/a.rs".into(),
                cwd: Some("/work/trees/feature".into())
            }
        );
        let found = locate(&host, &cache, "machine", "/work/repo/src/a.rs").unwrap();
        assert_eq!(
            found,
            Located {
                project_id: "p1".into(),
                rel: "src/a.rs".into(),
                cwd: None
            }
        );
        assert!(locate(&host, &cache, "machine", "/work/trees/gone/a.rs").is_err());
        // Answers are cached for a burst of reads.
        assert_eq!(host.methods("projects.list").len(), 1);
    }

    #[test]
    fn oversized_and_inconsistent_replies_are_refused() {
        let host = Fake::new(true, pattern(10));
        // A Host reporting more than the preview cap is refused before the second chunk.
        struct Huge(Fake);
        impl Transport for Huge {
            fn machine_for_environment(&self, e: &str) -> Result<String, String> {
                self.0.machine_for_environment(e)
            }
            fn request(&self, m: &str, method: &str, p: Value) -> Result<Value, String> {
                if method == "files.readBytes" {
                    return Ok(json!({"data": "", "offset": 0, "size": 40u64 * 1024 * 1024}));
                }
                self.0.request(m, method, p)
            }
        }
        let huge = Huge(host);
        let error = read_bytes(
            &huge,
            &Cache::default(),
            std::path::Path::new("remote://env/work/repo/a"),
            &mut |_, _| {},
            &AtomicBool::new(false),
            &|_, _| None,
        )
        .unwrap_err();
        assert!(error.contains("maximum 32 MB"));
    }
}
