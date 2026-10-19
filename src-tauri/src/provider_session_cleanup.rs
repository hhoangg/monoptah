//! Optional cleanup of the on-disk files a Claude Code or Codex CLI keeps for a
//! conversation, run after a Monoptah session is deleted so they do not pile up
//! on disk. Account transfer copies transcripts instead of moving them, so every
//! known home of the provider is searched, not only the session's own account.
//! Cleanup is best-effort: problems are reported as messages, never as errors.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::codex_mono_store::{history_base, rollout_matches, state_db};
use crate::provider_session_transfer::descendant_threads;

/// What to remove for one deleted session, captured before its record is gone.
pub(crate) struct CleanupRequest {
    provider: Provider,
    ids: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn dir_name(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }
}

/// Both CLIs name their sessions with canonical UUIDs. Anything else, such as a
/// corrupt id that equals a real directory name like `memory`, must never be
/// turned into a path.
fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// Returns `None` when the caller did not opt in, the harness has no files we
/// manage, or the session never reached the provider.
pub(crate) fn cleanup_request(
    wanted: bool,
    harness: &str,
    provider_session_id: Option<&str>,
    blocks: &Value,
) -> Option<CleanupRequest> {
    if !wanted {
        return None;
    }
    let provider = match harness {
        "claude" => Provider::Claude,
        "codex" => Provider::Codex,
        _ => return None,
    };
    let mut ids: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        if is_uuid(id) && !ids.iter().any(|known| known == id) {
            ids.push(id.to_string());
        }
    };
    if let Some(id) = provider_session_id {
        push(id);
    }
    if provider == Provider::Codex {
        for block in blocks.as_array().into_iter().flatten() {
            for thread in block
                .get("btwThreads")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let thread_harness = thread
                    .get("harness")
                    .and_then(Value::as_str)
                    .unwrap_or(harness);
                if thread_harness != "codex" {
                    continue;
                }
                if let Some(id) = thread.get("providerThreadId").and_then(Value::as_str) {
                    push(id);
                }
            }
        }
    }
    if ids.is_empty() {
        return None;
    }
    Some(CleanupRequest { provider, ids })
}

/// Runs the cleanup against the real homes and logs whatever could not be removed.
pub(crate) fn run_cleanup(app: &AppHandle, request: &CleanupRequest) {
    let homes = match provider_homes(app, request.provider) {
        Ok(homes) => homes,
        Err(error) => {
            eprintln!("Provider session cleanup skipped: {error}");
            return;
        }
    };
    let problems: Vec<String> = match request.provider {
        Provider::Claude => homes
            .iter()
            .flat_map(|home| {
                request
                    .ids
                    .iter()
                    .flat_map(|id| clean_claude_home(&home.path, id))
                    .collect::<Vec<_>>()
            })
            .collect(),
        Provider::Codex => {
            let codex_homes: Vec<CodexHome> = homes
                .into_iter()
                .map(|home| {
                    let sqlite_home = if home.is_default {
                        std::env::var_os("CODEX_SQLITE_HOME")
                            .filter(|value| !value.is_empty())
                            .map(PathBuf::from)
                            .unwrap_or_else(|| home.path.clone())
                    } else {
                        home.path.clone()
                    };
                    CodexHome {
                        path: home.path,
                        sqlite_home,
                    }
                })
                .collect();
            clean_codex_homes(&codex_homes, &request.ids)
        }
    };
    for problem in problems {
        eprintln!("Provider session cleanup: {problem}");
    }
}

struct ProviderHome {
    path: PathBuf,
    is_default: bool,
}

/// The default home plus every account home Monoptah created for the provider.
fn provider_homes(app: &AppHandle, provider: Provider) -> Result<Vec<ProviderHome>, String> {
    let (var, name) = match provider {
        Provider::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
        Provider::Codex => ("CODEX_HOME", ".codex"),
    };
    let mut homes = Vec::new();
    if let Some(path) = std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::dirs_home().map(|home| PathBuf::from(home).join(name)))
    {
        homes.push(ProviderHome {
            path,
            is_default: true,
        });
    }
    let accounts = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("provider-accounts")
        .join(provider.dir_name());
    if let Ok(entries) = std::fs::read_dir(accounts) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                homes.push(ProviderHome {
                    path: entry.path(),
                    is_default: false,
                });
            }
        }
    }
    Ok(homes)
}

fn clean_claude_home(home: &Path, id: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if !is_uuid(id) {
        return problems;
    }
    let Ok(root) = home.canonicalize() else {
        return problems;
    };
    let mut targets = Vec::new();
    if let Ok(projects) = std::fs::read_dir(home.join("projects")) {
        for project in projects.flatten() {
            // Stray files such as `.DS_Store` are not project directories.
            if project.path().is_dir() {
                targets.push(project.path().join(format!("{id}.jsonl")));
                targets.push(project.path().join(id));
            }
        }
    }
    for dir in ["file-history", "session-env", "tasks"] {
        targets.push(home.join(dir).join(id));
    }
    for target in targets {
        if let Err(error) = remove_inside(&root, &target) {
            problems.push(error);
        }
    }
    problems
}

struct CodexHome {
    path: PathBuf,
    /// Where `state_N.sqlite` lives; differs from `path` with CODEX_SQLITE_HOME.
    sqlite_home: PathBuf,
}

/// Removes the conversation from every home. The thread set is gathered from
/// all homes first, because account transfer copies rollouts and not always the
/// spawn edges, so a child may be recorded in a different home than its parent.
fn clean_codex_homes(homes: &[CodexHome], roots: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    // Subagent threads belong to the conversation that spawned them. History
    // ancestors are not followed: other conversations may still fork from them.
    let mut doomed: Vec<String> = Vec::new();
    let mut queue: Vec<String> = roots.iter().filter(|id| is_uuid(id)).cloned().collect();
    while let Some(id) = queue.pop() {
        if doomed.contains(&id) {
            continue;
        }
        for home in homes {
            match descendant_threads(&home.sqlite_home, &id) {
                Ok(children) => queue.extend(children.into_iter().filter(|c| is_uuid(c))),
                Err(error) => problems.push(format!("could not list subagent threads: {error}")),
            }
        }
        doomed.push(id);
    }
    if doomed.is_empty() {
        return problems;
    }

    let mut rollouts: Vec<(PathBuf, Option<String>)> = Vec::new();
    let mut unreadable = false;
    for home in homes {
        for dir in ["sessions", "archived_sessions"] {
            let mut files = Vec::new();
            collect_rollouts(&home.path.join(dir), &mut files);
            for file in files {
                let base = match history_base(&file) {
                    Ok(base) => base,
                    Err(error) => {
                        problems.push(format!("could not read {}: {error}", file.display()));
                        unreadable = true;
                        None
                    }
                };
                rollouts.push((file, base));
            }
        }
    }
    // Without knowing every rollout's base, a thread another conversation forks
    // from could be deleted, so leave all Codex files and rows alone.
    if unreadable {
        problems.push(
            "Codex files were kept because a rollout's history base could not be determined"
                .to_string(),
        );
        return problems;
    }
    // A rollout that is not being deleted may continue from a thread in the set.
    // Keep that thread, and then whatever the kept thread itself continues from.
    loop {
        let protected: Vec<String> = doomed
            .iter()
            .filter(|id| {
                rollouts.iter().any(|(path, base)| {
                    base.as_deref()
                        .is_some_and(|base| base.eq_ignore_ascii_case(id))
                        && !doomed.iter().any(|doomed| rollout_matches(path, doomed))
                })
            })
            .cloned()
            .collect();
        if protected.is_empty() {
            break;
        }
        for id in &protected {
            eprintln!("Provider session cleanup keeps Codex thread {id}: another conversation continues from it");
        }
        doomed.retain(|id| !protected.contains(id));
    }
    if doomed.is_empty() {
        return problems;
    }

    for home in homes {
        let Ok(root) = home.path.canonicalize() else {
            continue;
        };
        for (file, _) in &rollouts {
            if !file.starts_with(&home.path) || !doomed.iter().any(|id| rollout_matches(file, id)) {
                continue;
            }
            if let Err(error) = remove_inside(&root, file) {
                problems.push(error);
            }
        }
        if let Err(error) = delete_thread_rows(&home.sqlite_home, &doomed) {
            problems.push(format!("could not clean the thread index: {error}"));
        }
    }
    problems
}

/// Lists rollout files without following directory symlinks, so a link cannot
/// lead the scan outside the home.
fn collect_rollouts(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            collect_rollouts(&path, found);
        } else if path.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.starts_with("rollout-")
                && (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
        }) {
            found.push(path);
        }
    }
}

fn delete_thread_rows(sqlite_home: &Path, ids: &[String]) -> Result<(), String> {
    let Some(path) = state_db(sqlite_home)? else {
        return Ok(());
    };
    let mut db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    db.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| e.to_string())?;
    let has_table = |db: &rusqlite::Connection, name: &str| -> Result<bool, String> {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
    };
    let has_edges = has_table(&db, "thread_spawn_edges")?;
    let has_threads = has_table(&db, "threads")?;
    if !has_edges && !has_threads {
        return Ok(());
    }
    let marks = vec!["?"; ids.len()].join(",");
    let tx = db.transaction().map_err(|e| e.to_string())?;
    if has_edges {
        tx.execute(
            &format!(
                "DELETE FROM thread_spawn_edges WHERE parent_thread_id IN ({marks}) OR child_thread_id IN ({marks})"
            ),
            rusqlite::params_from_iter(ids.iter().chain(ids.iter())),
        )
        .map_err(|e| e.to_string())?;
    }
    if has_threads {
        tx.execute(
            &format!("DELETE FROM threads WHERE id IN ({marks})"),
            rusqlite::params_from_iter(ids.iter()),
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Removes `target` only when its parent directory resolves inside `root`. A
/// symlink is removed itself and never followed. A missing target is fine.
fn remove_inside(root: &Path, target: &Path) -> Result<(), String> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Ok(());
    };
    let parent = match parent.canonicalize() {
        Ok(parent) => parent,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", target.display())),
    };
    if !parent.starts_with(root) {
        return Err(format!("{} is outside the provider home", target.display()));
    }
    let resolved = parent.join(name);
    let metadata = match std::fs::symlink_metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", resolved.display())),
    };
    let removed = if metadata.is_dir() {
        std::fs::remove_dir_all(&resolved)
    } else {
        std::fs::remove_file(&resolved)
    };
    match removed {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", resolved.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> String {
        format!("00000000-0000-4000-8000-{n:012x}")
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "monoptah-provider-cleanup-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn home(&self) -> PathBuf {
            self.0.join("home")
        }
        fn other_home(&self) -> PathBuf {
            self.0.join("account")
        }
        fn file(&self, path: &Path) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        /// Writes a rollout whose first line is real session metadata.
        fn rollout(
            &self,
            home: &Path,
            dir: &str,
            thread: &str,
            suffix: &str,
            base: Option<&str>,
        ) -> PathBuf {
            let path = home
                .join(dir)
                .join("2026")
                .join("10")
                .join("08")
                .join(format!("rollout-2026-10-08T10-00-00-{thread}.{suffix}"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let meta = serde_json::json!({"type":"session_meta","payload":{"id":thread,"history_base":base.map(|id| serde_json::json!({"thread_id":id}))}});
            let bytes = format!("{meta}\n").into_bytes();
            let bytes = if suffix.ends_with("zst") {
                zstd::encode_all(bytes.as_slice(), 0).unwrap()
            } else {
                bytes
            };
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn codex_home(path: &Path) -> CodexHome {
        CodexHome {
            path: path.to_path_buf(),
            sqlite_home: path.to_path_buf(),
        }
    }

    #[test]
    fn only_canonical_uuids_are_ids() {
        assert!(is_uuid("0190f3a2-7b1c-7d4e-9a3b-123456789abc"));
        assert!(is_uuid("0190F3A2-7B1C-7D4E-9A3B-123456789ABC"));
        for bad in [
            "",
            "memory",
            "..",
            "abc",
            "0190f3a2-7b1c-7d4e-9a3b-123456789ab",
            "0190f3a27b1c7d4e9a3b123456789abcdef0",
            "0190f3a2-7b1c-7d4e-9a3b-123456789abg",
            "../190f3a2-7b1c-7d4e-9a3b-123456789abc",
        ] {
            assert!(!is_uuid(bad), "{bad}");
        }
    }

    #[test]
    fn claude_removes_every_artifact_of_the_session_only() {
        let fixture = Fixture::new();
        let home = fixture.home();
        let (target, keep) = (id(1), id(2));
        let project = home.join("projects").join("-Users-me-app");
        fixture.file(&project.join(format!("{target}.jsonl")));
        fixture.file(
            &project
                .join(&target)
                .join("subagents")
                .join("agent-1.jsonl"),
        );
        fixture.file(
            &home
                .join("projects")
                .join("-Users-me-other")
                .join(format!("{target}.jsonl")),
        );
        fixture.file(&home.join("file-history").join(&target).join("v1"));
        fixture.file(&home.join("session-env").join(&target).join("env"));
        fixture.file(&home.join("tasks").join(&target).join("1.json"));
        fixture.file(&project.join(format!("{keep}.jsonl")));
        fixture.file(&project.join(&keep).join("tool-results").join("r"));
        fixture.file(&project.join("memory").join("MEMORY.md"));
        fixture.file(&home.join("file-history").join(&keep).join("v1"));
        fixture.file(&home.join("history.jsonl"));
        // Stray files where directories are expected must not raise problems.
        fixture.file(&home.join("projects").join(".DS_Store"));

        let problems = clean_claude_home(&home, &target);
        assert!(problems.is_empty(), "{problems:?}");

        assert!(!project.join(format!("{target}.jsonl")).exists());
        assert!(!project.join(&target).exists());
        assert!(!home
            .join("projects")
            .join("-Users-me-other")
            .join(format!("{target}.jsonl"))
            .exists());
        assert!(!home.join("file-history").join(&target).exists());
        assert!(!home.join("session-env").join(&target).exists());
        assert!(!home.join("tasks").join(&target).exists());
        assert!(project.join(format!("{keep}.jsonl")).is_file());
        assert!(project.join(&keep).join("tool-results").join("r").is_file());
        assert!(project.join("memory").join("MEMORY.md").is_file());
        assert!(home.join("file-history").join(&keep).join("v1").is_file());
        assert!(home.join("history.jsonl").is_file());
    }

    #[test]
    fn claude_tolerates_a_missing_home() {
        let fixture = Fixture::new();
        assert!(clean_claude_home(&fixture.home(), &id(1)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn claude_removes_a_symlink_without_touching_its_target() {
        let fixture = Fixture::new();
        let home = fixture.home();
        let target = id(1);
        let outside = fixture.0.join("outside");
        fixture.file(&outside.join("precious"));
        std::fs::create_dir_all(home.join("file-history")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("file-history").join(&target)).unwrap();
        // A project directory that is a link out of the home must not be entered.
        fixture.file(&outside.join(format!("{target}.jsonl")));
        std::fs::create_dir_all(home.join("projects")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("projects").join("-linked")).unwrap();

        let problems = clean_claude_home(&home, &target);

        assert!(!home.join("file-history").join(&target).exists());
        assert!(outside.join("precious").is_file());
        assert!(outside.join(format!("{target}.jsonl")).is_file());
        assert!(!problems.is_empty());
    }

    fn state_db_with_threads(home: &Path, threads: &[String], edges: &[(String, String)]) {
        std::fs::create_dir_all(home).unwrap();
        let db = rusqlite::Connection::open(home.join("state_5.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TABLE threads(id TEXT PRIMARY KEY);
             CREATE TABLE thread_dynamic_tools(thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE, name TEXT);
             CREATE TABLE thread_spawn_edges(parent_thread_id TEXT NOT NULL, child_thread_id TEXT PRIMARY KEY, status TEXT NOT NULL);",
        )
        .unwrap();
        for thread in threads {
            db.execute("INSERT INTO threads VALUES (?1)", [thread])
                .unwrap();
            db.execute(
                "INSERT INTO thread_dynamic_tools VALUES (?1, 'tool')",
                [thread],
            )
            .unwrap();
        }
        for (parent, child) in edges {
            db.execute(
                "INSERT INTO thread_spawn_edges VALUES (?1, ?2, 'open')",
                [parent, child],
            )
            .unwrap();
        }
    }

    fn count(home: &Path, sql: &str) -> i64 {
        rusqlite::Connection::open(home.join("state_5.sqlite"))
            .unwrap()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn codex_removes_the_tree_and_btw_thread_but_not_ancestors_or_others() {
        let fixture = Fixture::new();
        let home = fixture.home();
        let (root, child, grandchild, btw, base, other, stranger) =
            (id(1), id(2), id(3), id(4), id(5), id(6), id(7));
        state_db_with_threads(
            &home,
            &[
                root.clone(),
                child.clone(),
                grandchild.clone(),
                btw.clone(),
                base.clone(),
                other.clone(),
            ],
            &[
                (root.clone(), child.clone()),
                (child.clone(), grandchild.clone()),
                (other.clone(), stranger.clone()),
            ],
        );
        let gone = [
            fixture.rollout(&home, "sessions", &root, "jsonl", Some(&base)),
            fixture.rollout(&home, "sessions", &root, "jsonl.zst", Some(&base)),
            fixture.rollout(&home, "archived_sessions", &child, "jsonl", None),
            fixture.rollout(&home, "sessions", &grandchild, "jsonl", None),
            fixture.rollout(&home, "sessions", &btw, "jsonl", None),
        ];
        let kept = [
            fixture.rollout(&home, "sessions", &base, "jsonl", None),
            fixture.rollout(&home, "sessions", &other, "jsonl", None),
            fixture.rollout(&home, "sessions", &stranger, "jsonl", None),
        ];
        fixture.file(&home.join("session_index.jsonl"));

        let problems = clean_codex_homes(&[codex_home(&home)], &[root.clone(), btw.clone()]);

        assert!(problems.is_empty(), "{problems:?}");
        for path in &gone {
            assert!(!path.exists(), "{} should be gone", path.display());
        }
        for path in &kept {
            assert!(path.is_file(), "{} should stay", path.display());
        }
        assert!(home.join("session_index.jsonl").is_file());
        assert_eq!(count(&home, "SELECT count(*) FROM threads"), 2);
        assert_eq!(count(&home, "SELECT count(*) FROM thread_dynamic_tools"), 2);
        assert_eq!(count(&home, "SELECT count(*) FROM thread_spawn_edges"), 1);
    }

    #[test]
    fn codex_finds_subagents_recorded_in_another_home() {
        let fixture = Fixture::new();
        let (a, b) = (fixture.home(), fixture.other_home());
        let (root, child, grandchild, other) = (id(1), id(2), id(3), id(4));
        // The root -> child edge exists only in B and child -> grandchild only
        // in A, while the rollouts are scattered over both homes.
        state_db_with_threads(
            &a,
            &[root.clone(), grandchild.clone(), other.clone()],
            &[(child.clone(), grandchild.clone())],
        );
        state_db_with_threads(
            &b,
            &[child.clone(), other.clone()],
            &[(root.clone(), child.clone())],
        );
        let gone = [
            fixture.rollout(&a, "sessions", &root, "jsonl", None),
            fixture.rollout(&b, "sessions", &child, "jsonl", None),
            fixture.rollout(&b, "archived_sessions", &grandchild, "jsonl", None),
        ];
        let kept = fixture.rollout(&b, "sessions", &other, "jsonl", None);

        let problems = clean_codex_homes(
            &[codex_home(&a), codex_home(&b)],
            std::slice::from_ref(&root),
        );

        assert!(problems.is_empty(), "{problems:?}");
        for path in &gone {
            assert!(!path.exists(), "{} should be gone", path.display());
        }
        assert!(kept.is_file());
        assert_eq!(count(&a, "SELECT count(*) FROM threads"), 1);
        assert_eq!(count(&b, "SELECT count(*) FROM threads"), 1);
        assert_eq!(count(&a, "SELECT count(*) FROM thread_spawn_edges"), 0);
        assert_eq!(count(&b, "SELECT count(*) FROM thread_spawn_edges"), 0);
    }

    #[test]
    fn codex_keeps_a_thread_another_conversation_continues_from() {
        let fixture = Fixture::new();
        let (a, b) = (fixture.home(), fixture.other_home());
        let (root, child, older, fork) = (id(1), id(2), id(3), id(4));
        state_db_with_threads(
            &a,
            &[root.clone(), child.clone(), older.clone(), fork.clone()],
            &[(root.clone(), child.clone())],
        );
        let root_file = fixture.rollout(&a, "sessions", &root, "jsonl", None);
        // The subagent continues from `older`, which is only kept because the
        // subagent is itself kept.
        let child_file = fixture.rollout(&a, "sessions", &child, "jsonl", Some(&older));
        let older_file = fixture.rollout(&a, "sessions", &older, "jsonl", None);
        // The fork lives in a different home and is not part of the deletion.
        let fork_file = fixture.rollout(&b, "sessions", &fork, "jsonl", Some(&child));

        let problems = clean_codex_homes(
            &[codex_home(&a), codex_home(&b)],
            std::slice::from_ref(&root),
        );

        assert!(problems.is_empty(), "{problems:?}");
        assert!(!root_file.exists());
        assert!(child_file.is_file());
        assert!(older_file.is_file());
        assert!(fork_file.is_file());
        // Only the root row is removed; the kept threads keep theirs.
        assert_eq!(count(&a, "SELECT count(*) FROM threads"), 3);
        assert_eq!(
            count(
                &a,
                &format!("SELECT count(*) FROM threads WHERE id = '{root}'")
            ),
            0
        );
    }

    #[test]
    fn codex_deletes_nothing_when_a_rollout_base_cannot_be_read() {
        let fixture = Fixture::new();
        let (a, b) = (fixture.home(), fixture.other_home());
        let (root, child, unrelated) = (id(1), id(2), id(3));
        state_db_with_threads(
            &a,
            &[root.clone(), child.clone()],
            &[(root.clone(), child.clone())],
        );
        let files = [
            fixture.rollout(&a, "sessions", &root, "jsonl", None),
            fixture.rollout(&b, "sessions", &child, "jsonl", None),
        ];
        let garbage = b
            .join("sessions")
            .join(format!("rollout-2026-10-08T10-00-00-{unrelated}.jsonl"));
        std::fs::write(&garbage, "not json\n").unwrap();

        let problems = clean_codex_homes(
            &[codex_home(&a), codex_home(&b)],
            std::slice::from_ref(&root),
        );

        assert!(problems.iter().any(|p| p.contains("kept")), "{problems:?}");
        for path in &files {
            assert!(path.is_file(), "{} should stay", path.display());
        }
        assert!(garbage.is_file());
        assert_eq!(count(&a, "SELECT count(*) FROM threads"), 2);
        assert_eq!(count(&a, "SELECT count(*) FROM thread_spawn_edges"), 1);
    }

    #[test]
    fn codex_skips_a_state_db_without_the_expected_tables() {
        let fixture = Fixture::new();
        let home = fixture.home();
        std::fs::create_dir_all(&home).unwrap();
        let db = rusqlite::Connection::open(home.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE unrelated(id TEXT);")
            .unwrap();
        let rollout = fixture.rollout(&home, "sessions", &id(1), "jsonl", None);

        assert!(clean_codex_homes(&[codex_home(&home)], &[id(1)]).is_empty());

        assert!(!rollout.exists());
    }

    #[test]
    fn codex_works_without_any_state_db() {
        let fixture = Fixture::new();
        let rollout = fixture.rollout(&fixture.home(), "sessions", &id(1), "jsonl", None);
        assert!(clean_codex_homes(&[codex_home(&fixture.home())], &[id(1)]).is_empty());
        assert!(!rollout.exists());
    }

    #[test]
    fn invalid_ids_delete_nothing() {
        let fixture = Fixture::new();
        let home = fixture.home();
        state_db_with_threads(&home, &[id(1)], &[]);
        let rollout = fixture.rollout(&home, "sessions", &id(1), "jsonl", None);
        fixture.file(&home.join("projects").join("p").join("memory").join("m"));
        fixture.file(&home.join("projects").join("p").join("abc.jsonl"));
        fixture.file(&home.join("history.jsonl"));

        for bad in ["", "..", "../x", "a/b", "memory", "abc", &"a".repeat(129)] {
            assert!(clean_claude_home(&home, bad).is_empty());
            assert!(clean_codex_homes(&[codex_home(&home)], &[bad.to_string()]).is_empty());
        }

        assert!(rollout.is_file());
        assert!(home
            .join("projects")
            .join("p")
            .join("memory")
            .join("m")
            .is_file());
        assert!(home.join("projects").join("p").join("abc.jsonl").is_file());
        assert!(home.join("history.jsonl").is_file());
        assert_eq!(count(&home, "SELECT count(*) FROM threads"), 1);
        assert!(cleanup_request(true, "claude", Some("../x"), &Value::Null).is_none());
        assert!(cleanup_request(true, "claude", Some("memory"), &Value::Null).is_none());
        assert!(cleanup_request(true, "codex", Some(""), &Value::Null).is_none());
    }

    #[test]
    fn request_is_built_only_when_wanted_for_claude_or_codex() {
        let (root, btw1, btw2, claude_btw) = (id(1), id(2), id(3), id(4));
        let blocks = serde_json::json!([
            {"id": "u1", "role": "user", "btwThreads": [
                {"id": "t1", "providerThreadId": btw1},
                {"id": "t2", "harness": "codex", "providerThreadId": btw2},
                {"id": "t3", "harness": "claude", "providerThreadId": claude_btw},
                {"id": "t4", "providerThreadId": "../bad"},
                {"id": "t5"}
            ]},
            {"id": "a1", "role": "assistant"}
        ]);

        assert!(cleanup_request(false, "codex", Some(&root), &blocks).is_none());
        assert!(cleanup_request(true, "omp", Some(&root), &blocks).is_none());
        assert!(cleanup_request(true, "codex", None, &Value::Null).is_none());

        let codex = cleanup_request(true, "codex", Some(&root), &blocks).unwrap();
        assert_eq!(codex.provider, Provider::Codex);
        assert_eq!(codex.ids, vec![root.clone(), btw1, btw2]);

        let claude = cleanup_request(true, "claude", Some(&root), &blocks).unwrap();
        assert_eq!(claude.provider, Provider::Claude);
        assert_eq!(claude.ids, vec![root]);
    }
}
