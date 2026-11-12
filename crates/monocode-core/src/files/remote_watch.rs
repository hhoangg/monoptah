//! Client side of the Host's change watch (`watch_changes`, `update_watch`, `wait_changes`,
//! `unwatch_changes`, capability `workspace.watch`).
//!
//! One watch belongs to one remote root. The Host answers `wait_changes` after at most 15 s,
//! so the long-poll gets its own, longer timeout and its own connection: it never holds the
//! tunnel or any lock other requests need. Every call goes through
//! [`Connections::request_timeout`], which for a cloud machine uses a live tunnel only. Nothing
//! here can wake a codespace or resume a sandbox.
use crate::remote::{default_connections, parse_remote_path};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

/// The Host capability that serves the four watch commands.
pub const CAPABILITY: &str = "workspace.watch";
/// The Host waits 15 s at most; this is the client's bound on the whole request.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(25);
/// The Host takes at most this many directories (relative to the root).
pub const MAX_DIRS: usize = 256;
/// The Host's error when a watch id is unknown or its lease expired.
pub const WATCH_UNKNOWN: &str = "watch_unknown";

/// What changed since the last `wait_changes`, with directories as full `remote://` paths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Git metadata changed (HEAD, index, refs).
    pub git: bool,
    /// Watched folders whose entries changed.
    pub dirs: Vec<String>,
    /// Events were dropped: every loaded folder should be listed again.
    pub overflow: bool,
}

/// Whether a failed call means the watch is gone and must be created again.
pub fn is_watch_unknown(error: &str) -> bool {
    error.contains(WATCH_UNKNOWN)
}

/// What a watch needs from a connection, so tests can stand in a fake Host.
pub trait WatchHost: Send + Sync {
    /// Whether the Host of `root` offers `workspace.watch`. `Err` when that cannot be
    /// learned right now (the link is down); it never wakes anything.
    fn supports_watch(&self, root: &str) -> Result<bool, String>;
    /// Starts a watch on `root` and the given folders (full remote paths); returns its id.
    fn watch(&self, root: &str, dirs: &[String]) -> Result<String, String>;
    /// Replaces the watched folders.
    fn update(&self, root: &str, id: &str, dirs: &[String]) -> Result<(), String>;
    /// Blocks until something changed or the Host's wait ran out (an empty `Changes`).
    fn wait(&self, root: &str, id: &str) -> Result<Changes, String>;
    /// Ends the watch. Best effort: the Host also drops it when its lease expires.
    fn unwatch(&self, root: &str, id: &str);
}

/// Whether the Host refused a folder (it is gone, is not a folder, or lies outside the
/// workspace). The Host validates the whole list and refuses all of it for one such folder,
/// and does not say which one.
pub fn is_path_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "enoent",
        "enotdir",
        "eacces",
        "no such file",
        "not a directory",
        "permission denied",
        "outside the workspace",
        "must be relative",
        "invalid watch director",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// The folders of `dirs` the Host accepts, found by halving; `ok` asks the Host about a list.
/// `dirs` as a whole was already refused for a path error.
fn bisect(
    dirs: &[String],
    ok: &mut dyn FnMut(&[String]) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    if dirs.len() <= 1 {
        return Ok(Vec::new());
    }
    let (left, right) = dirs.split_at(dirs.len() / 2);
    let mut good = Vec::new();
    for half in [left, right] {
        match ok(half) {
            Ok(()) => good.extend_from_slice(half),
            Err(error) if is_path_error(&error) => good.extend(bisect(half, ok)?),
            Err(error) => return Err(error),
        }
    }
    Ok(good)
}

/// Replaces the watched folders. When the Host refuses the list for a bad folder, the good
/// ones are found and watched anyway; the folders left out are returned.
pub fn update_filtered(
    host: &dyn WatchHost,
    root: &str,
    id: &str,
    dirs: &[String],
) -> Result<Vec<String>, String> {
    match host.update(root, id, dirs) {
        Ok(()) => Ok(Vec::new()),
        Err(error) if is_path_error(&error) => {
            let good = bisect(dirs, &mut |part| host.update(root, id, part))?;
            host.update(root, id, &good)?;
            Ok(dirs
                .iter()
                .filter(|dir| !good.contains(dir))
                .cloned()
                .collect())
        }
        Err(error) => Err(error),
    }
}

/// Starts a watch on `root` and `dirs`. A bad folder does not stop it: the watch starts on the
/// root alone and takes the good folders through [`update_filtered`]. Returns the id and the
/// folders left out.
pub fn start_filtered(
    host: &dyn WatchHost,
    root: &str,
    dirs: &[String],
) -> Result<(String, Vec<String>), String> {
    match host.watch(root, dirs) {
        Ok(id) => Ok((id, Vec::new())),
        Err(error) if is_path_error(&error) && !dirs.is_empty() => {
            let id = host.watch(root, &[root.to_owned()])?;
            match update_filtered(host, root, &id, dirs) {
                Ok(left_out) => Ok((id, left_out)),
                Err(error) => {
                    host.unwatch(root, &id);
                    Err(error)
                }
            }
        }
        Err(error) => Err(error),
    }
}

/// The folder as the Host wants it: slash-separated, relative to `root`, "" for the root.
/// Folders outside the root have no such form.
pub fn relative_dir(root: &str, dir: &str) -> Option<String> {
    let root = root.trim_end_matches('/');
    if dir == root {
        return Some(String::new());
    }
    dir.strip_prefix(root)?
        .strip_prefix('/')
        .map(|rest| rest.trim_end_matches('/').to_owned())
}

/// The folders to send: relative to `root`, outside folders dropped, no repeats, shallowest
/// first, at most [`MAX_DIRS`].
pub fn relative_dirs(root: &str, dirs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = dirs
        .iter()
        .filter_map(|dir| relative_dir(root, dir))
        .collect();
    out.sort_by_key(|dir| (dir.matches('/').count(), dir.clone()));
    out.dedup();
    out.truncate(MAX_DIRS);
    out
}

fn absolute(root: &str, relative: &str) -> String {
    let root = root.trim_end_matches('/');
    if relative.is_empty() {
        root.to_owned()
    } else {
        format!("{root}/{relative}")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireChanges {
    #[serde(default)]
    git: bool,
    #[serde(default)]
    dirs: Vec<String>,
    #[serde(default)]
    overflow: bool,
}

/// Reads a `wait_changes` result and maps its folders to full paths under `root`.
pub fn decode_changes(root: &str, value: Value) -> Result<Changes, String> {
    let wire: WireChanges = serde_json::from_value(value)
        .map_err(|error| format!("Invalid remote watch response: {error}"))?;
    Ok(Changes {
        git: wire.git,
        dirs: wire.dirs.iter().map(|dir| absolute(root, dir)).collect(),
        overflow: wire.overflow,
    })
}

/// The real connections.
pub struct LiveWatchHost;

impl LiveWatchHost {
    fn call(
        &self,
        root: &str,
        command: &str,
        args: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, String> {
        let remote = parse_remote_path(root).ok_or("Invalid remote working copy")?;
        let connections = default_connections()?;
        let machine = connections.machine_for_environment(&remote.environment_id)?;
        connections
            .request_timeout(
                &machine.id,
                "workspace.run",
                json!({"command": command, "args": args}),
                timeout,
            )
            .map_err(super::remote::workspace_error)
    }
    fn short(&self, root: &str, command: &str, args: Value) -> Result<Value, String> {
        self.call(root, command, args, None)
    }
    fn host_path(root: &str) -> Result<String, String> {
        parse_remote_path(root)
            .map(|remote| remote.host_path)
            .ok_or_else(|| "Invalid remote working copy".into())
    }
}

impl WatchHost for LiveWatchHost {
    fn supports_watch(&self, root: &str) -> Result<bool, String> {
        let remote = parse_remote_path(root).ok_or("Invalid remote working copy")?;
        let connections = default_connections()?;
        let machine = connections.machine_for_environment(&remote.environment_id)?;
        let info = match connections.host_info(&machine.id) {
            Some(info) => info,
            None => connections.describe_host(&machine.id)?,
        };
        Ok(info.supports(CAPABILITY))
    }
    fn watch(&self, root: &str, dirs: &[String]) -> Result<String, String> {
        let result = self.short(
            root,
            "watch_changes",
            json!({"cwd": Self::host_path(root)?, "dirs": relative_dirs(root, dirs)}),
        )?;
        result
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "Invalid remote watch response".into())
    }
    fn update(&self, root: &str, id: &str, dirs: &[String]) -> Result<(), String> {
        self.short(
            root,
            "update_watch",
            json!({"id": id, "dirs": relative_dirs(root, dirs)}),
        )
        .map(|_| ())
    }
    fn wait(&self, root: &str, id: &str) -> Result<Changes, String> {
        let result = self.call(root, "wait_changes", json!({"id": id}), Some(WAIT_TIMEOUT))?;
        decode_changes(root, result)
    }
    fn unwatch(&self, root: &str, id: &str) {
        let _ = self.short(root, "unwatch_changes", json!({"id": id}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "remote://env/work/app";

    #[test]
    fn folders_are_sent_relative_to_the_root_shallowest_first_and_capped() {
        let dirs = vec![
            format!("{ROOT}/src/ui"),
            ROOT.to_owned(),
            format!("{ROOT}/src"),
            format!("{ROOT}/src"),
            "remote://env/work/app-other/x".to_owned(),
            "remote://other/work/app/y".to_owned(),
        ];
        assert_eq!(relative_dirs(ROOT, &dirs), vec!["", "src", "src/ui"]);
        let many: Vec<String> = (0..300).map(|n| format!("{ROOT}/d{n}")).collect();
        assert_eq!(relative_dirs(ROOT, &many).len(), MAX_DIRS);
        assert_eq!(relative_dir(&format!("{ROOT}/"), ROOT), Some(String::new()));
    }

    #[test]
    fn changes_come_back_as_full_paths_under_the_root() {
        let changes = decode_changes(
            ROOT,
            json!({"git": true, "dirs": ["", "src/ui"], "overflow": false}),
        )
        .unwrap();
        assert_eq!(
            changes,
            Changes {
                git: true,
                dirs: vec![ROOT.into(), format!("{ROOT}/src/ui")],
                overflow: false
            }
        );
        let empty = decode_changes(ROOT, json!({"git": false, "dirs": [], "overflow": true}));
        assert!(empty.unwrap().overflow);
        assert!(decode_changes(ROOT, json!("nope")).is_err());
    }

    #[test]
    fn an_unknown_watch_is_recognised_in_the_hosts_error() {
        assert!(is_watch_unknown(
            "Host rejected request: watch_unknown: watcher is unknown or expired; subscribe again"
        ));
        assert!(!is_watch_unknown("Machine is unreachable."));
    }

    #[test]
    fn the_wait_outlives_the_hosts_fifteen_seconds() {
        assert!(WAIT_TIMEOUT > Duration::from_secs(15));
        assert!(WAIT_TIMEOUT < Duration::from_secs(30) + Duration::from_secs(1));
    }

    use std::sync::Mutex;

    /// A Host whose folders `bad` do not exist: any list holding one is refused.
    struct Gone {
        bad: Vec<String>,
        updates: Mutex<Vec<Vec<String>>>,
        watches: Mutex<Vec<Vec<String>>>,
    }
    impl Gone {
        fn new(bad: &[&str]) -> Self {
            Self {
                bad: bad.iter().map(|d| format!("{ROOT}/{d}")).collect(),
                updates: Mutex::default(),
                watches: Mutex::default(),
            }
        }
        fn check(&self, dirs: &[String]) -> Result<(), String> {
            if dirs.iter().any(|d| self.bad.contains(d)) {
                Err("ENOENT: no such file or directory".into())
            } else {
                Ok(())
            }
        }
    }
    impl WatchHost for Gone {
        fn supports_watch(&self, _: &str) -> Result<bool, String> {
            Ok(true)
        }
        fn watch(&self, _: &str, dirs: &[String]) -> Result<String, String> {
            self.watches.lock().unwrap().push(dirs.to_vec());
            self.check(dirs).map(|_| "w".into())
        }
        fn update(&self, _: &str, _: &str, dirs: &[String]) -> Result<(), String> {
            self.updates.lock().unwrap().push(dirs.to_vec());
            self.check(dirs)
        }
        fn wait(&self, _: &str, _: &str) -> Result<Changes, String> {
            Ok(Changes::default())
        }
        fn unwatch(&self, _: &str, _: &str) {}
    }

    #[test]
    fn path_errors_are_told_from_link_errors() {
        assert!(is_path_error("Host rejected request: ENOENT: no such file"));
        assert!(is_path_error("Not a directory"));
        assert!(is_path_error("Path is outside the workspace"));
        assert!(!is_path_error("Machine is unreachable."));
        assert!(!is_path_error(
            "watch_unknown: watcher is unknown or expired"
        ));
    }

    #[test]
    fn a_watch_starts_without_the_folders_the_host_refuses() {
        let dirs: Vec<String> = ["", "a", "b", "c", "d"]
            .iter()
            .map(|d| absolute(ROOT, d))
            .collect();
        let host = Gone::new(&["c"]);
        let (id, left_out) = start_filtered(&host, ROOT, &dirs).unwrap();
        assert_eq!(id, "w");
        assert_eq!(left_out, vec![format!("{ROOT}/c")]);
        let last = host.updates.lock().unwrap().last().unwrap().clone();
        assert_eq!(last.len(), 4);
        assert!(!last.contains(&format!("{ROOT}/c")));
        let host = Gone::new(&["a", "d"]);
        let (_, left_out) = start_filtered(&host, ROOT, &dirs).unwrap();
        assert_eq!(left_out.len(), 2);
        // A list that is fine goes through in one call.
        let host = Gone::new(&[]);
        let (_, left_out) = start_filtered(&host, ROOT, &dirs).unwrap();
        assert!(left_out.is_empty());
        assert_eq!(host.watches.lock().unwrap().len(), 1);
        assert!(host.updates.lock().unwrap().is_empty());
    }

    #[test]
    fn a_link_error_is_not_mistaken_for_a_bad_folder() {
        struct Down;
        impl WatchHost for Down {
            fn supports_watch(&self, _: &str) -> Result<bool, String> {
                Ok(true)
            }
            fn watch(&self, _: &str, _: &[String]) -> Result<String, String> {
                Err("Machine is unreachable.".into())
            }
            fn update(&self, _: &str, _: &str, _: &[String]) -> Result<(), String> {
                Err("Machine is unreachable.".into())
            }
            fn wait(&self, _: &str, _: &str) -> Result<Changes, String> {
                Err("Machine is unreachable.".into())
            }
            fn unwatch(&self, _: &str, _: &str) {}
        }
        let dirs = vec![ROOT.to_owned(), format!("{ROOT}/a")];
        assert!(start_filtered(&Down, ROOT, &dirs).is_err());
        assert!(update_filtered(&Down, ROOT, "w", &dirs).is_err());
    }
}
