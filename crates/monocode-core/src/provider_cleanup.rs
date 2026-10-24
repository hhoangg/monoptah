//! Optional cleanup of the on-disk files a Claude Code or Codex CLI keeps for a
//! conversation, run after a Monoptah session is deleted so they do not pile up
//! on disk. Account transfer copies transcripts instead of moving them, so every
//! known home of the provider is searched, not only the session's own account.
//! Cleanup is best-effort: problems are reported as messages, never as errors.
use crate::{
    accounts::{
        transfer::{
            codex::{history_base, rollout_matches, state_db},
            descendant_threads,
        },
        Profile,
    },
    provider::Kind,
    store::Session,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What to remove for one deleted session, captured before its record is gone.
#[derive(Clone, Debug)]
pub struct CleanupRequest {
    provider: Kind,
    ids: Vec<String>,
}

impl CleanupRequest {
    pub fn provider(&self) -> Kind {
        self.provider
    }
    pub fn ids(&self) -> &[String] {
        &self.ids
    }
}

/// One provider session id a Mono reset also removes, such as an id retired by
/// context rotation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSessionRef {
    pub harness: String,
    pub id: String,
}

/// Both CLIs name their sessions with canonical UUIDs. Anything else, such as a
/// corrupt id that equals a real directory name like `memory`, must never be
/// turned into a path.
pub fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

/// The CLI that owns on-disk files for this session, if it ever reached it.
/// Decides whether the delete dialog offers to remove them.
pub fn files_kind(session: &Session) -> Option<Kind> {
    if session.provider_id.as_deref().is_none_or(str::is_empty) {
        return None;
    }
    match Kind::parse(&session.provider)? {
        kind @ (Kind::Claude | Kind::Codex) => Some(kind),
        _ => None,
    }
}

/// `side_threads` are the provider thread ids of the session's side (btw)
/// conversations; only Codex keeps those as rollouts of their own. Returns
/// `None` when the harness has no files we manage or no id is a valid UUID.
pub fn cleanup_request(session: &Session, side_threads: &[String]) -> Option<CleanupRequest> {
    let provider = files_kind(session)?;
    let mut ids: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        if is_uuid(id) && !ids.iter().any(|known| known == id) {
            ids.push(id.to_string());
        }
    };
    push(session.provider_id.as_deref()?);
    if provider == Kind::Codex {
        side_threads.iter().for_each(|id| push(id));
    }
    if ids.is_empty() {
        return None;
    }
    Some(CleanupRequest { provider, ids })
}

/// Shortens text that came from the caller before it goes into a message.
fn clip(text: &str) -> String {
    text.chars().take(40).collect()
}

/// Merges the record's own request with the extra sessions of a Mono reset,
/// grouped per provider. Anything that is not a canonical UUID of a supported
/// provider is skipped and reported, never turned into a path. When the record
/// does not exist the extras are ignored and one problem is reported.
pub fn plan_reset_cleanup(
    own: Option<CleanupRequest>,
    extra: &[ProviderSessionRef],
    record_exists: bool,
) -> (Vec<CleanupRequest>, Vec<String>) {
    // Without the record to vouch for the conversation, nothing is deleted on
    // the word of the extra ids alone.
    if !record_exists {
        let problems = if extra.is_empty() {
            Vec::new()
        } else {
            vec!["conversation record not found; provider session files were kept".to_string()]
        };
        return (Vec::new(), problems);
    }
    let mut requests: Vec<CleanupRequest> = own.into_iter().collect();
    let mut problems = Vec::new();
    for session in extra {
        let provider = match session.harness.as_str() {
            "claude" => Kind::Claude,
            "codex" => Kind::Codex,
            other => {
                problems.push(format!(
                    "skipped {} session: unsupported provider",
                    clip(other)
                ));
                continue;
            }
        };
        if !is_uuid(&session.id) {
            problems.push(format!(
                "skipped {} session: invalid session id",
                clip(&session.harness)
            ));
            continue;
        }
        match requests.iter_mut().find(|r| r.provider == provider) {
            Some(request) => {
                if !request.ids.contains(&session.id) {
                    request.ids.push(session.id.clone());
                }
            }
            None => requests.push(CleanupRequest {
                provider,
                ids: vec![session.id.clone()],
            }),
        }
    }
    (requests, problems)
}

/// Runs the cleanup against the default home and every account home under
/// `data_dir`, and returns whatever could not be removed as short messages.
pub fn run_cleanup(data_dir: &Path, request: &CleanupRequest) -> Vec<String> {
    run_cleanup_report(data_dir, std::slice::from_ref(request), false)
}

/// Runs the requests like [`run_cleanup`]. `include_mono_stores` adds the
/// private Codex stores that Mono conversations keep their rollouts in.
pub fn run_cleanup_report(
    data_dir: &Path,
    requests: &[CleanupRequest],
    include_mono_stores: bool,
) -> Vec<String> {
    report_with_homes(requests, &|kind| {
        let mut homes = provider_homes(data_dir, kind);
        add_mono_stores(&mut homes, data_dir, kind, include_mono_stores);
        Ok(homes)
    })
}

fn add_mono_stores(homes: &mut Vec<ProviderHome>, data_dir: &Path, kind: Kind, include: bool) {
    if include && kind == Kind::Codex {
        homes.extend(mono_codex_homes(data_dir));
    }
}

/// Every real directory under `<data dir>/codex-monos/`, where Mono Codex
/// conversations keep a private home per account. Links and files are ignored.
fn mono_codex_homes(data_dir: &Path) -> Vec<ProviderHome> {
    let Ok(entries) = std::fs::read_dir(data_dir.join("codex-monos")) else {
        return Vec::new();
    };
    let mut homes: Vec<ProviderHome> = entries
        .flatten()
        // `DirEntry::file_type` does not follow symlinks.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| ProviderHome {
            path: entry.path(),
            is_default: false,
        })
        .collect();
    homes.sort_by(|a, b| a.path.cmp(&b.path));
    homes
}

fn report_with_homes(
    requests: &[CleanupRequest],
    homes_for: &dyn Fn(Kind) -> Result<Vec<ProviderHome>, String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for request in requests {
        let homes = match homes_for(request.provider) {
            Ok(homes) => homes,
            Err(error) => {
                problems.push(format!(
                    "skipped {} cleanup: {error}",
                    request.provider.id()
                ));
                continue;
            }
        };
        match request.provider {
            Kind::Claude => {
                for home in &homes {
                    for id in &request.ids {
                        problems.extend(clean_claude_home(&home.path, id));
                    }
                }
            }
            Kind::Codex => {
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
                problems.extend(clean_codex_homes(&codex_homes, &request.ids));
            }
            _ => {}
        }
    }
    problems
}

struct ProviderHome {
    path: PathBuf,
    is_default: bool,
}

/// The default home plus every account home Monoptah created for the provider.
fn provider_homes(data_dir: &Path, kind: Kind) -> Vec<ProviderHome> {
    let mut homes = Vec::new();
    if let Ok(path) = Profile::default().home(kind) {
        homes.push(ProviderHome {
            path,
            is_default: true,
        });
    }
    homes.extend(account_homes(data_dir, kind));
    homes
}

/// Every real directory under `<data dir>/provider-accounts/<provider>/` whose
/// name is a valid account id. Links and files are ignored.
fn account_homes(data_dir: &Path, kind: Kind) -> Vec<ProviderHome> {
    let Ok(Some(sample)) = Profile::new("sample", data_dir).directory(kind) else {
        return Vec::new();
    };
    let Some(Ok(entries)) = sample.parent().map(std::fs::read_dir) else {
        return Vec::new();
    };
    let mut homes: Vec<ProviderHome> = entries
        .flatten()
        // `DirEntry::file_type` does not follow symlinks.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let path = Profile::new(&name, data_dir).directory(kind).ok()??;
            Some(ProviderHome {
                path,
                is_default: false,
            })
        })
        .collect();
    homes.sort_by(|a, b| a.path.cmp(&b.path));
    homes
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

    struct Fixture(PathBuf, #[allow(dead_code)] tempfile::TempDir);
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            // Tests compare against canonical paths, which differ on macOS.
            Self(dir.path().canonicalize().unwrap(), dir)
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
    fn codex_home(path: &Path) -> CodexHome {
        CodexHome {
            path: path.to_path_buf(),
            sqlite_home: path.to_path_buf(),
        }
    }

    fn session(provider: &str, provider_id: Option<&str>) -> Session {
        Session {
            id: 1,
            title: String::new(),
            provider: provider.into(),
            cwd: "/work".into(),
            provider_id: provider_id.map(str::to_owned),
            has_messages: true,
            updated_at: 0,
            archived: false,
            pinned: false,
            work_item: None,
            surface: Default::default(),
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
        for bad in ["../x", "memory", ""] {
            assert!(cleanup_request(&session("claude", Some(bad)), &[]).is_none());
            assert!(cleanup_request(&session("codex", Some(bad)), &[]).is_none());
        }
    }

    #[test]
    fn request_is_built_only_for_claude_or_codex_with_a_provider_id() {
        let (root, btw1, btw2) = (id(1), id(2), id(3));
        let side = [
            btw1.clone(),
            "../bad".to_string(),
            btw2.clone(),
            btw1.clone(),
        ];

        assert!(cleanup_request(&session("omp", Some(&root)), &side).is_none());
        assert!(cleanup_request(&session("codex", None), &[]).is_none());
        // A side thread alone is not enough: the session never reached the CLI.
        assert!(cleanup_request(&session("codex", None), &side).is_none());

        let codex = cleanup_request(&session("codex", Some(&root)), &side).unwrap();
        assert_eq!(codex.provider, Kind::Codex);
        assert_eq!(codex.ids, vec![root.clone(), btw1, btw2]);

        // Claude has no side threads of its own.
        let claude = cleanup_request(&session("claude", Some(&root)), &side).unwrap();
        assert_eq!(claude.provider, Kind::Claude);
        assert_eq!(claude.ids, vec![root]);
    }

    #[test]
    fn files_kind_needs_a_provider_id_on_a_claude_or_codex_session() {
        assert_eq!(
            files_kind(&session("claude", Some(&id(1)))),
            Some(Kind::Claude)
        );
        assert_eq!(files_kind(&session("codex", Some("x"))), Some(Kind::Codex));
        assert_eq!(files_kind(&session("codex", Some(""))), None);
        assert_eq!(files_kind(&session("codex", None)), None);
        assert_eq!(files_kind(&session("cursor", Some(&id(1)))), None);
    }

    #[cfg(unix)]
    #[test]
    fn account_homes_are_the_real_directories_of_that_provider_only() {
        let fixture = Fixture::new();
        let data = fixture.0.join("data");
        let claude = data.join("provider-accounts").join("claude");
        std::fs::create_dir_all(claude.join("work")).unwrap();
        std::fs::create_dir_all(claude.join("home-2")).unwrap();
        std::fs::create_dir_all(claude.join("bad id")).unwrap();
        fixture.file(&claude.join("stray.txt"));
        let outside = fixture.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, claude.join("linked")).unwrap();
        std::fs::create_dir_all(data.join("provider-accounts").join("codex").join("x")).unwrap();

        let homes: Vec<PathBuf> = account_homes(&data, Kind::Claude)
            .into_iter()
            .map(|home| {
                assert!(!home.is_default);
                home.path
            })
            .collect();

        assert_eq!(homes, vec![claude.join("home-2"), claude.join("work")]);
        assert!(account_homes(&fixture.0, Kind::Codex).is_empty());
    }

    #[test]
    fn report_has_no_problem_for_missing_files_or_a_missing_home() {
        let fixture = Fixture::new();
        let requests = [
            CleanupRequest {
                provider: Kind::Claude,
                ids: vec![id(1)],
            },
            CleanupRequest {
                provider: Kind::Codex,
                ids: vec![id(2)],
            },
        ];
        let missing = fixture.0.join("does-not-exist");

        let problems = report_with_homes(&requests, &|_| {
            Ok(vec![ProviderHome {
                path: missing.clone(),
                is_default: false,
            }])
        });

        assert!(problems.is_empty(), "{problems:?}");
        assert!(!missing.exists());
    }

    #[test]
    fn report_turns_a_home_lookup_failure_into_a_problem() {
        let requests = [CleanupRequest {
            provider: Kind::Claude,
            ids: vec![id(1)],
        }];

        let problems = report_with_homes(&requests, &|_| Err("no app data dir".to_string()));

        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("no app data dir"), "{problems:?}");
    }

    #[cfg(unix)]
    #[test]
    fn report_does_not_follow_a_project_link_out_of_the_home() {
        let fixture = Fixture::new();
        let home = fixture.home();
        let target = id(1);
        let outside = fixture.0.join("outside");
        fixture.file(&outside.join(format!("{target}.jsonl")));
        fixture.file(&outside.join(&target).join("data"));
        std::fs::create_dir_all(home.join("projects")).unwrap();
        std::os::unix::fs::symlink(&outside, home.join("projects").join("-linked")).unwrap();
        let request = CleanupRequest {
            provider: Kind::Claude,
            ids: vec![target.clone()],
        };

        let problems = report_with_homes(&[request], &|_| {
            Ok(vec![ProviderHome {
                path: home.clone(),
                is_default: false,
            }])
        });

        assert!(!problems.is_empty());
        assert!(outside.join(format!("{target}.jsonl")).is_file());
        assert!(outside.join(&target).join("data").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn remove_inside_refuses_a_parent_that_resolves_outside_the_root() {
        let fixture = Fixture::new();
        let root = fixture.home();
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let outside = fixture.0.join("outside");
        fixture.file(&outside.join("precious"));
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

        let result = remove_inside(&root, &root.join("escape").join("precious"));

        assert!(result.is_err());
        assert!(outside.join("precious").is_file());
    }

    fn session_ref(harness: &str, id: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            harness: harness.to_string(),
            id: id.to_string(),
        }
    }

    fn own_request(harness: &str, ids: &[String]) -> CleanupRequest {
        let mut request = cleanup_request(&session(harness, Some(&ids[0])), &[]).unwrap();
        request.ids = ids.to_vec();
        request
    }

    #[test]
    fn reset_plan_merges_retired_ids_and_dedupes() {
        let (live, retired1, retired2) = (id(1), id(2), id(3));
        let own = own_request("claude", std::slice::from_ref(&live));
        let extra = [
            session_ref("claude", &retired1),
            session_ref("claude", &live),
            session_ref("claude", &retired2),
            session_ref("claude", &retired1),
        ];

        let (requests, problems) = plan_reset_cleanup(Some(own), &extra, true);

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].provider, Kind::Claude);
        assert_eq!(requests[0].ids, vec![live, retired1, retired2]);
    }

    #[test]
    fn reset_plan_ignores_extras_when_the_record_is_gone() {
        let extra = [session_ref("codex", &id(1)), session_ref("claude", &id(2))];

        let (requests, problems) = plan_reset_cleanup(None, &extra, false);

        assert!(requests.is_empty(), "{requests:?}");
        assert_eq!(
            problems,
            vec!["conversation record not found; provider session files were kept".to_string()]
        );
        assert!(plan_reset_cleanup(None, &[], false).1.is_empty());
    }

    #[test]
    fn reset_plan_honours_extras_when_the_record_exists() {
        let (a, b) = (id(1), id(2));
        let extra = [session_ref("codex", &a), session_ref("claude", &b)];

        let (requests, problems) = plan_reset_cleanup(None, &extra, true);

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(requests.len(), 2);
        let claude = requests.iter().find(|r| r.provider == Kind::Claude);
        let codex = requests.iter().find(|r| r.provider == Kind::Codex);
        assert_eq!(claude.unwrap().ids, vec![b]);
        assert_eq!(codex.unwrap().ids, vec![a]);
    }

    #[test]
    fn reset_plan_keeps_the_record_provider_separate_from_extras_of_another() {
        let (own_id, other) = (id(1), id(2));
        let own = own_request("codex", std::slice::from_ref(&own_id));

        let (requests, _) = plan_reset_cleanup(Some(own), &[session_ref("claude", &other)], true);

        assert_eq!(requests.len(), 2);
        let codex = requests.iter().find(|r| r.provider == Kind::Codex);
        assert_eq!(codex.unwrap().ids, vec![own_id]);
    }

    #[test]
    fn reset_plan_skips_unsupported_providers_and_invalid_ids() {
        let good = id(1);
        let long = "a".repeat(37);
        let extra = [
            session_ref("cursor", &id(2)),
            session_ref("claude", "../../etc"),
            session_ref("claude", "memory"),
            session_ref("codex", ""),
            session_ref("codex", &long),
            session_ref("claude", &good),
        ];

        let (requests, problems) = plan_reset_cleanup(None, &extra, true);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].provider, Kind::Claude);
        assert_eq!(requests[0].ids, vec![good]);
        assert_eq!(problems.len(), 5, "{problems:?}");
        assert_eq!(problems[0], "skipped cursor session: unsupported provider");
        assert!(problems[1..]
            .iter()
            .all(|p| p.ends_with("session: invalid session id")));
        assert!(problems[1].starts_with("skipped claude session"));
        assert!(problems[3].starts_with("skipped codex session"));
    }

    #[test]
    fn reset_plan_never_echoes_more_than_forty_characters() {
        let long_harness = "h".repeat(200);
        let (requests, problems) =
            plan_reset_cleanup(None, &[session_ref(&long_harness, &id(1))], true);

        assert!(requests.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(!problems[0].contains(&"h".repeat(41)), "{}", problems[0]);
    }

    #[test]
    fn reset_plan_accepts_a_mixed_case_uuid_like_is_uuid_does() {
        let upper = "0190F3A2-7B1C-7D4E-9A3B-123456789ABC";
        assert!(is_uuid(upper));

        let (requests, problems) = plan_reset_cleanup(None, &[session_ref("claude", upper)], true);

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(requests[0].ids, vec![upper.to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn mono_codex_homes_lists_only_real_directories() {
        let fixture = Fixture::new();
        let monos = fixture.0.join("codex-monos");
        std::fs::create_dir_all(monos.join("default")).unwrap();
        std::fs::create_dir_all(monos.join("work")).unwrap();
        fixture.file(&monos.join("stray.txt"));
        let outside = fixture.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, monos.join("linked")).unwrap();

        let mut homes: Vec<PathBuf> = mono_codex_homes(&fixture.0)
            .into_iter()
            .map(|home| {
                assert!(!home.is_default);
                home.path
            })
            .collect();
        homes.sort();

        assert_eq!(homes, vec![monos.join("default"), monos.join("work")]);
    }

    #[test]
    fn mono_codex_homes_is_empty_without_the_directory() {
        let fixture = Fixture::new();
        assert!(mono_codex_homes(&fixture.0).is_empty());
    }

    #[test]
    fn mono_stores_are_added_for_codex_only_and_only_when_asked() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.0.join("codex-monos").join("default")).unwrap();
        let count = |kind, include| {
            let mut homes = Vec::new();
            add_mono_stores(&mut homes, &fixture.0, kind, include);
            homes.len()
        };

        assert_eq!(count(Kind::Codex, true), 1);
        assert_eq!(count(Kind::Codex, false), 0);
        assert_eq!(count(Kind::Claude, true), 0);
    }

    #[test]
    fn codex_rollout_in_a_mono_store_is_removed_and_other_threads_stay() {
        let fixture = Fixture::new();
        let mono_home = fixture.0.join("codex-monos").join("default");
        let (target, other) = (id(1), id(2));
        state_db_with_threads(&mono_home, &[target.clone(), other.clone()], &[]);
        let gone = fixture.rollout(&mono_home, "sessions", &target, "jsonl", None);
        let kept = fixture.rollout(&mono_home, "sessions", &other, "jsonl", None);

        let homes = mono_codex_homes(&fixture.0);
        let request = CleanupRequest {
            provider: Kind::Codex,
            ids: vec![target],
        };
        let problems = report_with_homes(&[request], &|provider| {
            assert_eq!(provider, Kind::Codex);
            Ok(vec![ProviderHome {
                path: homes[0].path.clone(),
                is_default: homes[0].is_default,
            }])
        });

        assert!(problems.is_empty(), "{problems:?}");
        assert!(!gone.exists());
        assert!(kept.is_file());
        assert_eq!(count(&mono_home, "SELECT count(*) FROM threads"), 1);
    }

    #[test]
    fn codex_keeps_a_mono_store_thread_that_a_normal_home_rollout_continues_from() {
        let fixture = Fixture::new();
        let mono_home = fixture.0.join("codex-monos").join("default");
        let normal_home = fixture.home();
        let (target, fork, other) = (id(1), id(2), id(3));
        state_db_with_threads(&mono_home, &[target.clone(), other.clone()], &[]);
        let mono_target = fixture.rollout(&mono_home, "sessions", &target, "jsonl", None);
        let mono_other = fixture.rollout(&mono_home, "sessions", &other, "jsonl", None);
        // A rollout in an ordinary home, not being deleted, continues from the target.
        let fork_rollout = fixture.rollout(&normal_home, "sessions", &fork, "jsonl", Some(&target));
        let request = CleanupRequest {
            provider: Kind::Codex,
            ids: vec![target.clone()],
        };

        let mono_homes = mono_codex_homes(&fixture.0);
        let problems = report_with_homes(&[request], &|_| {
            let mut homes = vec![ProviderHome {
                path: normal_home.clone(),
                is_default: false,
            }];
            homes.extend(mono_homes.iter().map(|home| ProviderHome {
                path: home.path.clone(),
                is_default: home.is_default,
            }));
            Ok(homes)
        });

        assert!(problems.is_empty(), "{problems:?}");
        assert!(mono_target.is_file(), "the continued thread must stay");
        assert!(mono_other.is_file());
        assert!(fork_rollout.is_file());
        assert_eq!(count(&mono_home, "SELECT count(*) FROM threads"), 2);
    }
}
