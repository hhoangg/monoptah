//! What the GitHub tab asks of the outside world. Every method blocks (git,
//! disk, SQLite), so callers run them on the background executor. The trait
//! is the seam the page tests use: they never touch real repositories, `gh`
//! or the user's preferences.
use crate::runtime::{Database, locked};
use chrono::{DateTime, Local};
use monocode_core::contributors::{
    ContributorsBranches, GitUser,
    api::{self, Contributors},
    fetch::FetchOutcome,
    jobs::{LoadResponse, SyncEvent},
    model::{
        period::PeriodPick,
        prefs::{
            load_chart_kind, load_color_overrides, load_merge, load_period_pick, save_chart_kind,
            save_color_overrides, save_merge, save_period_pick,
        },
        sync_jobs::StartCall,
    },
    sync::Estimate,
};
use std::{collections::BTreeMap, sync::Arc};

/// Person id to palette color, as the user picked them.
pub(crate) type ColorOverrides = BTreeMap<String, &'static str>;

pub(crate) trait Backend: Send + Sync {
    fn read_config(&self, cwd: &str) -> Result<Option<String>, String>;
    fn write_config(&self, cwd: &str, content: &str) -> Result<(), String>;
    fn read_mailmap(&self, cwd: &str) -> Result<Option<String>, String>;
    fn branches(&self, cwd: &str) -> Result<ContributorsBranches, String>;
    fn git_user(&self, cwd: &str) -> Result<GitUser, String>;
    fn first_commit(&self, cwd: &str, branch: Option<&str>) -> Result<Option<String>, String>;
    fn load(
        &self,
        cwd: &str,
        branch: Option<&str>,
        since: &str,
        until: &str,
    ) -> Result<LoadResponse, String>;
    fn estimate(
        &self,
        cwd: &str,
        branch: Option<&str>,
        from: &str,
        to: &str,
    ) -> Result<Estimate, String>;
    /// Fetches the branch (the default one when `None`) from `origin`, only on
    /// the user's request. Loads never fetch a branch; sync jobs may still ask
    /// the remote for PR heads.
    fn fetch_origin(&self, cwd: &str, branch: Option<&str>) -> Result<FetchOutcome, String>;
    fn start(&self, cwd: &str, call: &StartCall) -> Result<(), String>;
    fn cancel(&self, cwd: &str) -> Result<(), String>;
    fn clear(&self, cwd: &str) -> Result<(), String>;
    /// What the core says still runs, for `SyncJobs::reconcile_with`.
    fn status(&self) -> Vec<SyncEvent>;
    /// The saved period and merge toggle (native values over the Tauri keys).
    fn load_prefs(&self, now: &DateTime<Local>) -> (PeriodPick, bool);
    fn save_pick(&self, pick: &PeriodPick);
    fn save_merge(&self, merge: bool);
    /// The saved chart type text (`line`, `cumulative`, `bars`), shared by every chart.
    fn load_chart_kind(&self) -> Option<String>;
    fn save_chart_kind(&self, kind: &str);
    /// The colors picked for the people of one project.
    fn load_colors(&self, project: &str) -> ColorOverrides;
    fn save_colors(&self, project: &str, colors: &ColorOverrides);
}

/// The real thing: the core's sync API and the shell store's preferences.
pub(crate) struct CoreBackend {
    core: Arc<Contributors>,
    store: Option<Database>,
}
impl CoreBackend {
    pub(crate) fn new(core: Arc<Contributors>, store: Option<Database>) -> Self {
        Self { core, store }
    }
}

impl Backend for CoreBackend {
    fn read_config(&self, cwd: &str) -> Result<Option<String>, String> {
        api::read_config(cwd)
    }
    fn write_config(&self, cwd: &str, content: &str) -> Result<(), String> {
        api::write_config(cwd, content)
    }
    fn read_mailmap(&self, cwd: &str) -> Result<Option<String>, String> {
        api::read_mailmap(cwd)
    }
    fn branches(&self, cwd: &str) -> Result<ContributorsBranches, String> {
        api::branches(cwd)
    }
    fn git_user(&self, cwd: &str) -> Result<GitUser, String> {
        api::git_user(cwd)
    }
    fn first_commit(&self, cwd: &str, branch: Option<&str>) -> Result<Option<String>, String> {
        api::first_commit(cwd, branch)
    }
    fn load(
        &self,
        cwd: &str,
        branch: Option<&str>,
        since: &str,
        until: &str,
    ) -> Result<LoadResponse, String> {
        api::load(&self.core, cwd, branch, since, until)
    }
    fn estimate(
        &self,
        cwd: &str,
        branch: Option<&str>,
        from: &str,
        to: &str,
    ) -> Result<Estimate, String> {
        api::sync_estimate(&self.core, cwd, branch, from, to)
    }
    fn fetch_origin(&self, cwd: &str, branch: Option<&str>) -> Result<FetchOutcome, String> {
        api::fetch_origin(&self.core, cwd, branch)
    }
    fn start(&self, cwd: &str, call: &StartCall) -> Result<(), String> {
        api::sync_start(
            &self.core,
            cwd,
            call.branch.as_deref(),
            &call.from,
            &call.to,
            call.mode,
            call.old_head.as_deref(),
        )
    }
    fn cancel(&self, cwd: &str) -> Result<(), String> {
        api::sync_cancel(&self.core, cwd)
    }
    fn clear(&self, cwd: &str) -> Result<(), String> {
        api::clear(&self.core, cwd)
    }
    fn status(&self) -> Vec<SyncEvent> {
        api::sync_status(&self.core)
    }
    fn load_prefs(&self, now: &DateTime<Local>) -> (PeriodPick, bool) {
        match self.store.as_ref().map(locked) {
            Some(Ok(store)) => (load_period_pick(&store, now), load_merge(&store)),
            _ => (
                monocode_core::contributors::model::prefs::read_period_pick(None, None, now),
                true,
            ),
        }
    }
    fn save_pick(&self, pick: &PeriodPick) {
        if let Some(Ok(store)) = self.store.as_ref().map(locked) {
            let _ = save_period_pick(&store, pick);
        }
    }
    fn save_merge(&self, merge: bool) {
        if let Some(Ok(store)) = self.store.as_ref().map(locked) {
            let _ = save_merge(&store, merge);
        }
    }
    fn load_chart_kind(&self) -> Option<String> {
        match self.store.as_ref().map(locked) {
            Some(Ok(store)) => load_chart_kind(&store),
            _ => None,
        }
    }
    fn save_chart_kind(&self, kind: &str) {
        if let Some(Ok(store)) = self.store.as_ref().map(locked) {
            let _ = save_chart_kind(&store, kind);
        }
    }
    fn load_colors(&self, project: &str) -> ColorOverrides {
        match self.store.as_ref().map(locked) {
            Some(Ok(store)) => load_color_overrides(&store, project),
            _ => ColorOverrides::new(),
        }
    }
    fn save_colors(&self, project: &str, colors: &ColorOverrides) {
        if let Some(Ok(store)) = self.store.as_ref().map(locked) {
            let _ = save_color_overrides(&store, project, colors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use monocode_core::contributors::{
        jobs::EventSink,
        model::period::{PeriodKind, PeriodPick},
    };
    use monocode_core::{
        contributors::{ticket_jobs::TicketJobEvent, ticket_jobs::TicketSink},
        store::Store,
    };
    use std::{
        path::{Path, PathBuf},
        process::Command,
        sync::{
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };

    struct Quiet;
    impl EventSink for Quiet {
        fn emit(&self, _: &SyncEvent) {}
    }
    impl TicketSink for Quiet {
        fn emit(&self, _: &TicketJobEvent) {}
    }

    /// A unique, empty temp folder; removed when dropped.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "monocode-backend-{}-{}-{}",
                std::process::id(),
                monocode_core::time::now_ms(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A throwaway git repository with one commit; removed when dropped.
    struct Repo(TempDir);
    impl Repo {
        fn new() -> Self {
            let repo = Self(TempDir::new());
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.name", "Ana"]);
            repo.git(&["config", "user.email", "ana@example.com"]);
            std::fs::write(repo.dir().join("a.txt"), "one\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-q", "-m", "first"]);
            repo
        }
        fn dir(&self) -> &Path {
            &(self.0).0
        }
        /// Git without the user's config: no global identity, signing or hooks.
        fn git(&self, args: &[&str]) {
            let out = Command::new("git")
                .args(["-c", "commit.gpgsign=false", "-c", "init.templateDir="])
                .args(args)
                .current_dir(self.dir())
                .env("GIT_CONFIG_GLOBAL", self.dir().join(".no-gitconfig"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_AUTHOR_DATE", "2026-10-01T10:00:00+00:00")
                .env("GIT_COMMITTER_DATE", "2026-10-01T10:00:00+00:00")
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        }
        fn path(&self) -> &str {
            self.dir().to_str().unwrap()
        }
    }

    fn backend() -> CoreBackend {
        let data = std::env::temp_dir();
        let core = Contributors::open(
            Path::new(":memory:"),
            &data,
            Arc::new(Quiet),
            Arc::new(Quiet),
        )
        .unwrap();
        let store = Store::open(Path::new(":memory:")).unwrap();
        CoreBackend::new(Arc::new(core), Some(Arc::new(Mutex::new(store))))
    }

    #[test]
    fn the_core_backend_reads_a_real_repository_and_lists_nothing_running() {
        let repo = Repo::new();
        let backend = backend();
        let branches = backend.branches(repo.path()).unwrap();
        assert_eq!(branches.current, "main");
        assert_eq!(
            backend.git_user(repo.path()).unwrap().email,
            "ana@example.com"
        );
        assert!(backend.first_commit(repo.path(), None).unwrap().is_some());
        assert_eq!(backend.read_mailmap(repo.path()).unwrap(), None);
        assert!(backend.status().is_empty());
        // Nothing synced yet: the period is not covered and holds no numbers.
        let loaded = backend
            .load(
                repo.path(),
                None,
                "2026-09-01T00:00:00Z",
                "2026-10-31T00:00:00Z",
            )
            .unwrap();
        assert!(!loaded.covered);
        assert_eq!(loaded.raw.branch, "main");
        assert!(!loaded.head.is_empty());
        // A folder that is not a repository is an error, not an empty page.
        let elsewhere = TempDir::new();
        assert!(backend.branches(elsewhere.0.to_str().unwrap()).is_err());
    }

    #[test]
    fn the_config_file_round_trips_through_the_backend() {
        let repo = Repo::new();
        let backend = backend();
        assert_eq!(backend.read_config(repo.path()).unwrap(), None);
        backend
            .write_config(repo.path(), "{\"version\":1}\n")
            .unwrap();
        assert_eq!(
            backend.read_config(repo.path()).unwrap().as_deref(),
            Some("{\"version\":1}\n")
        );
    }

    #[test]
    fn the_period_and_merge_preferences_are_saved_in_the_store_and_read_back() {
        let backend = backend();
        let now = chrono::Local
            .with_ymd_and_hms(2026, 10, 15, 10, 0, 0)
            .earliest()
            .unwrap();
        // Nothing saved: the last 7 days, merging on.
        let (pick, merge) = backend.load_prefs(&now);
        assert_eq!(
            pick,
            PeriodPick {
                kind: PeriodKind::Days7,
                id: None
            }
        );
        assert!(merge);
        let pinned = PeriodPick {
            kind: PeriodKind::Month,
            id: Some("2026-07".into()),
        };
        backend.save_pick(&pinned);
        backend.save_merge(false);
        assert_eq!(backend.load_prefs(&now), (pinned, false));
    }

    #[test]
    fn the_chart_kind_and_the_colors_are_saved_in_the_store_and_read_back() {
        let backend = backend();
        assert_eq!(backend.load_chart_kind(), None);
        backend.save_chart_kind("cumulative");
        assert_eq!(backend.load_chart_kind().as_deref(), Some("cumulative"));
        // Colors are kept per project.
        let picked = ColorOverrides::from([("ana@x.io".to_string(), "#3fb950")]);
        backend.save_colors("/work/alpha", &picked);
        assert_eq!(backend.load_colors("/work/alpha"), picked);
        assert!(backend.load_colors("/work/beta").is_empty());
    }
}
