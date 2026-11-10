//! The project's Git status, shared by the right panel and the title bar:
//! GitChangesPanel's `useDiffIndex`, useGitFileStatuses and
//! useProjectDiffStats over one off-thread reader. Reads never overlap; a
//! read requested meanwhile runs once the current one returns, and only a
//! changed snapshot notifies.
use crate::remote_cache::RemoteCache;
use gpui::{Context, EventEmitter, Task};
use monocode_core::scm::{self, DiffIndex, DiffStats};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

/// Without a file watcher, visible status polls on this interval.
const POLL: Duration = Duration::from_secs(2);
const RESUME_TTL: Duration = Duration::from_secs(30);
/// A burst of writes settles into one read this long after the last.
const SETTLE: Duration = Duration::from_millis(300);
/// Watched status still reads this often, in case an event went missing.
const REPAIR: Duration = Duration::from_secs(30);
/// After a reconnect the old snapshot stays read-only for at most this long, so a read that hangs
/// or keeps failing can never lock the panels while the link is reported live. A link that is
/// really down is separate (`link_offline`) and keeps the panels read-only regardless.
const RECOVERY_LIMIT: Duration = Duration::from_secs(30);

/// A disk or Git change: Changes, History and the Explorer reread.
pub(crate) struct GitChanged;

#[derive(Default, Clone, PartialEq)]
pub(crate) struct StatusMap {
    pub files: HashMap<String, String>,
    pub dirs: HashMap<String, String>,
}
fn priority(status: &str) -> u8 {
    match status {
        "modified" => 3,
        "deleted" => 2,
        "added" | "untracked" => 1,
        _ => 0,
    }
}
/// useGitFileStatuses's `buildStatusMaps`: each folder takes its most
/// significant descendant status.
pub(crate) fn status_maps(index: &DiffIndex, root: &str) -> StatusMap {
    let mut map = StatusMap::default();
    for file in &index.files {
        map.files.insert(file.path.clone(), file.status.clone());
        let prio = priority(&file.status);
        if prio == 0 {
            continue;
        }
        let mut dir = parent(&file.path);
        while dir.len() > root.len() {
            let existing = map.dirs.get(dir).map_or(0, |s| priority(s));
            if prio <= existing {
                break;
            }
            map.dirs.insert(dir.to_owned(), file.status.clone());
            dir = parent(dir);
        }
    }
    map
}
fn parent(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

pub(crate) struct ProjectGit {
    /// The active session's working folder (its worktree, if it has one):
    /// Sidebar.tsx's `gitRoot`.
    project: String,
    pub root: String,
    pub index: Option<DiffIndex>,
    pub stats: Option<DiffStats>,
    pub statuses: StatusMap,
    /// Bumped on every observed change, for views that reread on it.
    pub epoch: u64,
    pub review_cache: crate::review::ReviewCache,
    versions: Option<HashMap<String, scm::FileDiffVersion>>,
    remembered: HashMap<String, (u64, DiffIndex)>,
    /// The listing and snapshot of the last 16 cloud workspaces, shown dimmed while the tunnel
    /// to one is down. Local checkouts use `remembered`.
    pub(crate) remote: RemoteCache,
    /// The panels show a cloud workspace that has no live connection: nothing is read, the last
    /// snapshot stays, and the panels are read-only until the link is back.
    link_offline: bool,
    /// The link is back but the first full reload since has not finished: what is shown is still
    /// the old snapshot, so it stays dimmed and read-only until the reload lands.
    recovering: bool,
    /// Reads started so far, and how many had started when the link came back: only a read that
    /// started after the reconnect can end the recovery, whatever else is queued meanwhile.
    read_seq: u64,
    recovery_floor: u64,
    recovery_tries: u32,
    /// Ends the recovery after `RECOVERY_LIMIT` whatever happens to the reads.
    recovery_limit: Option<Task<()>>,
    /// Reads again after a failed first read, with a growing delay.
    recovery_retry: Option<Task<()>>,
    /// The workspace is being woken (the strip says "Workspace starting").
    starting: bool,
    /// Status should be read while visible, whether or not a read can run right now.
    polling_wanted: bool,
    /// Reads started, for tests that prove a disconnected workspace is left alone.
    #[cfg(test)]
    pub(crate) reads: usize,
    clock: u64,
    in_flight: bool,
    pending: bool,
    stats_loaded: Option<Instant>,
    /// Reads while status is visible: a file watcher, or a timer without one.
    poll: Option<[Task<()>; 2]>,
}
impl EventEmitter<GitChanged> for ProjectGit {}

impl ProjectGit {
    pub(crate) fn new(project: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            root: project.clone(),
            project: String::new(),
            index: None,
            stats: None,
            statuses: StatusMap::default(),
            epoch: 0,
            review_cache: Default::default(),
            versions: None,
            remembered: HashMap::new(),
            remote: RemoteCache::default(),
            link_offline: false,
            recovering: false,
            read_seq: 0,
            recovery_floor: 0,
            recovery_tries: 0,
            recovery_limit: None,
            recovery_retry: None,
            starting: false,
            polling_wanted: false,
            #[cfg(test)]
            reads: 0,
            clock: 0,
            in_flight: false,
            pending: false,
            stats_loaded: None,
            poll: None,
        };
        this.set_project(project, cx);
        this
    }
    pub(crate) fn project(&self) -> &str {
        &self.project
    }
    pub(crate) fn available(&self) -> bool {
        !self.project.is_empty() && self.project != "~"
    }
    /// The panels show a cloud workspace the app is not connected to. Nothing is read from it
    /// and nothing may wake it; what is shown is the last snapshot, read-only.
    #[cfg(test)]
    pub(crate) fn poll_is_off_for_test(&self) -> bool {
        self.poll.is_none()
    }
    pub(crate) fn offline(&self) -> bool {
        self.link_offline
    }
    /// What is shown is not live data: the link is down, or the reload after it came back has
    /// not landed yet. Writes stay refused and the panels stay dimmed meanwhile.
    pub(crate) fn stale(&self) -> bool {
        self.link_offline || self.recovering
    }
    /// The workspace is being woken, for the wording of the notes.
    pub(crate) fn starting(&self) -> bool {
        self.starting
    }
    pub(crate) fn set_starting(&mut self, starting: bool, cx: &mut Context<Self>) {
        if self.starting != starting {
            self.starting = starting;
            cx.notify();
        }
    }
    /// How long ago the live workspace of the project was last read, for a remote project.
    pub(crate) fn cached_age(&self) -> Option<std::time::Duration> {
        self.remote.age(&self.project, Instant::now())
    }
    /// Binds to `project` and records whether its workspace is connected. A switch to a
    /// workspace without a connection reads nothing; the link coming back reads everything
    /// again (a full reload, and polling resumes).
    pub(crate) fn bind(&mut self, project: String, offline: bool, cx: &mut Context<Self>) {
        let was = self.link_offline;
        self.link_offline = offline;
        if project != self.project {
            self.set_project(project, cx);
            return;
        }
        if was == offline {
            return;
        }
        if offline {
            // Stop the timers and the watcher; the wish to poll is remembered.
            self.poll = None;
            self.end_recovery();
            cx.notify();
            return;
        }
        // Back online: a user action (Connect) brought the workspace back. Start over; the old
        // snapshot stays read-only until the first reload lands.
        self.begin_recovery(cx);
        self.epoch += 1;
        cx.emit(GitChanged);
        cx.notify();
        if self.polling_wanted {
            self.set_polling(true, cx);
        } else {
            self.warm_choices(cx);
            self.reload(cx);
        }
    }
    fn end_recovery(&mut self) {
        self.recovering = false;
        self.recovery_tries = 0;
        self.recovery_limit = None;
        self.recovery_retry = None;
    }
    /// The link is back: the snapshot is old until a read that starts now lands, and in any case
    /// for no longer than `RECOVERY_LIMIT`.
    fn begin_recovery(&mut self, cx: &mut Context<Self>) {
        self.end_recovery();
        if !self.available() {
            return;
        }
        self.recovering = true;
        self.recovery_floor = self.read_seq;
        self.recovery_limit = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RECOVERY_LIMIT).await;
            let _ = this.update(cx, |this, cx| {
                if this.recovering {
                    this.end_recovery();
                    cx.notify();
                }
            });
        }));
    }
    /// Read `seq` of `project` ended. Only a read that started after the reconnect counts: a
    /// good one ends the recovery, a failed one keeps the panels read-only and reads again soon.
    fn finish_read(&mut self, seq: u64, project: &str, ok: bool, cx: &mut Context<Self>) {
        if !self.recovering || self.project != project || seq <= self.recovery_floor {
            return;
        }
        if ok {
            self.end_recovery();
            cx.notify();
            return;
        }
        self.recovery_tries += 1;
        let delay = Duration::from_millis(1000 << self.recovery_tries.min(3));
        self.recovery_retry = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| this.reload(cx));
        }));
    }
    pub(crate) fn set_link_offline(&mut self, offline: bool, cx: &mut Context<Self>) {
        self.bind(self.project.clone(), offline, cx);
    }
    /// The remembered snapshot of `project`.
    fn remembered_index(&mut self, project: &str) -> Option<DiffIndex> {
        if project.starts_with("remote://") {
            return self.remote.get(project).and_then(|entry| entry.index);
        }
        self.remembered.get(project).map(|(_, i)| i.clone())
    }
    /// The first frame of a switch already shows the new checkout's last
    /// snapshot, never the previous one's files and actions.
    pub(crate) fn set_project(&mut self, project: String, cx: &mut Context<Self>) {
        if project == self.project {
            return;
        }
        self.project = project.clone();
        self.root = project.clone();
        self.end_recovery();
        self.index = self.remembered_index(&project);
        self.stats = self.index.as_ref().map(stats_of);
        self.statuses = self
            .index
            .as_ref()
            .map(|i| status_maps(i, &self.root))
            .unwrap_or_default();
        self.stats_loaded = None;
        self.versions = None;
        self.epoch += 1;
        cx.emit(GitChanged);
        cx.notify();
        self.warm_choices(cx);
        self.poll = None;
        if self.polling_wanted {
            // Watch the new checkout instead; this reads it straight away.
            self.set_polling(true, cx);
        } else {
            self.reload(cx);
        }
    }
    /// While the project's Git status is visible, read it again whenever the
    /// checkout or its Git directory changes on disk, and every 30s besides.
    /// Without a file watcher, poll every 2s instead.
    pub(crate) fn set_polling(&mut self, polling: bool, cx: &mut Context<Self>) {
        self.polling_wanted = polling;
        if polling == self.poll.is_some() {
            return;
        }
        if !polling {
            self.poll = None;
            return;
        }
        if self.link_offline {
            // Polled again once the link is back (see `bind`).
            return;
        }
        self.reload(cx);
        let project = PathBuf::from(&self.project);
        let watched = cx.spawn(async move |this, cx| {
            let watch = cx
                .background_executor()
                .spawn(async move {
                    let mut roots = vec![project.clone()];
                    roots.extend(
                        scm::git_dirs(&project)
                            .into_iter()
                            .filter(|dir| !dir.starts_with(&project)),
                    );
                    crate::fs_watch::watch(&roots)
                })
                .await;
            let Some((_watch, changes)) = watch else {
                loop {
                    cx.background_executor().timer(POLL).await;
                    if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                        return;
                    }
                }
            };
            while changes.recv().await.is_ok() {
                cx.background_executor().timer(SETTLE).await;
                while changes.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    return;
                }
            }
        });
        let repair = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REPAIR).await;
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        });
        self.poll = Some([watched, repair]);
    }
    /// useProjectDiffStats reads again on focus once its snapshot is 30s old.
    pub(crate) fn resume(&mut self, cx: &mut Context<Self>) {
        if self.link_offline {
            return;
        }
        if self
            .stats_loaded
            .is_none_or(|at| at.elapsed() >= RESUME_TTL)
            || self.poll.is_some()
        {
            self.warm_choices(cx);
            self.reload(cx);
        }
    }
    /// A mutation or file operation: reread now and tell every view.
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        self.epoch += 1;
        cx.emit(GitChanged);
        self.warm_choices(cx);
        self.reload(cx);
    }
    /// Project selection warms both menus before the user opens either one.
    fn warm_choices(&self, cx: &Context<Self>) {
        if !self.available() || self.link_offline {
            return;
        }
        let project = self.project.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = crate::git_cache::load(std::path::Path::new(&project));
            })
            .detach();
    }
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.available() || self.link_offline {
            return;
        }
        #[cfg(test)]
        {
            self.reads += 1;
        }
        if self.in_flight {
            self.pending = true;
            return;
        }
        self.in_flight = true;
        self.read_seq += 1;
        let seq = self.read_seq;
        let project = self.project.clone();
        let path = PathBuf::from(&project);
        let work = cx.background_executor().spawn(async move {
            let index = scm::diff_index(&path)?;
            let versions = scm::file_diff_versions(&path, &index).ok();
            Ok::<_, String>((index, versions))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.in_flight = false;
                this.finish_read(seq, &project, result.is_ok(), cx);
                // A slow or unavailable checkout keeps its last snapshot, and
                // never takes another checkout's.
                if this.project == project
                    && let Ok((next, versions)) = result
                {
                    this.stats_loaded = Some(Instant::now());
                    this.remember(&project, &next);
                    if this.index.as_ref() != Some(&next) || this.versions != versions {
                        let had = this.index.is_some();
                        this.stats = Some(stats_of(&next));
                        this.statuses = status_maps(&next, &this.root);
                        this.index = Some(next);
                        this.versions = versions;
                        if had {
                            this.epoch += 1;
                            cx.emit(GitChanged);
                        }
                        cx.notify();
                    }
                }
                if std::mem::take(&mut this.pending) {
                    this.reload(cx);
                }
            });
        })
        .detach();
    }
    /// rememberIndex: recent checkouts open on their last snapshot, within 24
    /// checkouts and 20,000 files.
    fn remember(&mut self, project: &str, index: &DiffIndex) {
        if project.starts_with("remote://") {
            self.remote.put_index(project, index, Instant::now());
            return;
        }
        self.clock += 1;
        self.remembered
            .insert(project.to_owned(), (self.clock, index.clone()));
        let mut files: usize = self.remembered.values().map(|(_, i)| i.files.len()).sum();
        while self.remembered.len() > 24 || files > 20_000 {
            let Some(oldest) = self
                .remembered
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((_, gone)) = self.remembered.remove(&oldest) {
                files -= gone.files.len();
            }
        }
    }
    pub(crate) fn has_changes(&self) -> bool {
        self.stats.as_ref().is_some_and(|s| s.files > 0)
    }
}
fn stats_of(index: &DiffIndex) -> DiffStats {
    DiffStats {
        files: index.files.len() as i64,
        additions: index.additions,
        deletions: index.deletions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Entity, TestAppContext};
    use monocode_core::scm::ChangedFile;

    const A: &str = "remote://env-octo-abc/workspaces/widgets";
    const B: &str = "remote://env-octo-xyz/workspaces/widgets";

    fn snapshot(path: &str) -> DiffIndex {
        DiffIndex {
            files: vec![ChangedFile {
                path: format!("{path}/a.rs"),
                relative: "a.rs".into(),
                status: "modified".into(),
                additions: 1,
                deletions: 0,
                staged: false,
                unstaged: true,
            }],
            ..Default::default()
        }
    }
    fn file(path: &str, status: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            relative: path.trim_start_matches("/r/").into(),
            status: status.into(),
            additions: 0,
            deletions: 0,
            staged: false,
            unstaged: true,
        }
    }
    #[test]
    fn folders_take_their_most_significant_status() {
        let index = DiffIndex {
            files: vec![
                file("/r/a/b/new.rs", "untracked"),
                file("/r/a/b/old.rs", "modified"),
                file("/r/a/gone.rs", "deleted"),
            ],
            ..Default::default()
        };
        let map = status_maps(&index, "/r");
        assert_eq!(map.dirs["/r/a/b"], "modified");
        assert_eq!(map.dirs["/r/a"], "modified");
        assert!(!map.dirs.contains_key("/r"));
        assert_eq!(map.files["/r/a/gone.rs"], "deleted");
    }

    #[gpui::test]
    fn a_disconnected_workspace_is_not_read_or_polled_and_resumes_with_a_full_reload(
        cx: &mut TestAppContext,
    ) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, cx| {
            git.bind(A.into(), false, cx);
            git.set_polling(true, cx);
            assert!(git.poll.is_some() && git.reads >= 1);
        });
        // The tunnel drops: the timers stop, and no read of any kind starts.
        git.update(cx, |git, cx| {
            let (reads, epoch) = (git.reads, git.epoch);
            git.bind(A.into(), true, cx);
            assert!(git.offline() && git.poll.is_none());
            git.set_polling(true, cx);
            git.resume(cx);
            git.reload(cx);
            git.changed(cx);
            git.set_polling(true, cx);
            assert!(git.poll.is_none(), "polling waits for the link");
            assert_eq!(git.reads, reads, "a disconnected workspace is never read");
            assert_eq!(git.epoch, epoch + 1, "only `changed` bumps the epoch");
        });
        // Connect succeeded: one full reload, and polling is back.
        git.update(cx, |git, cx| {
            let (reads, epoch) = (git.reads, git.epoch);
            git.bind(A.into(), false, cx);
            assert!(!git.offline() && git.poll.is_some());
            assert!(
                git.stale(),
                "the old snapshot is not live until the reload lands"
            );
            assert_eq!(git.reads, reads + 1);
            assert!(git.epoch > epoch, "the views reread");
            // Binding again with the same link changes nothing.
            git.bind(A.into(), false, cx);
            assert_eq!(git.reads, reads + 1);
        });
        // Not polled before: back online only reads once, and does not start polling.
        git.update(cx, |git, cx| {
            git.set_polling(false, cx);
            git.bind(A.into(), true, cx);
            let reads = git.reads;
            git.bind(A.into(), false, cx);
            assert!(git.poll.is_none());
            assert_eq!(git.reads, reads + 1);
        });
    }

    #[gpui::test]
    fn a_disconnect_keeps_the_snapshot_of_its_root_and_never_shows_it_for_another(
        cx: &mut TestAppContext,
    ) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, cx| {
            git.bind(A.into(), false, cx);
            let seen = snapshot(A);
            git.remember(A, &seen);
            // A reload that failed because the tunnel is down kept this snapshot; the switch
            // away and back brings it back.
            git.bind(B.into(), false, cx);
            assert_eq!(git.index, None, "root A's snapshot is never root B's");
            assert!(git.statuses.files.is_empty());
            git.bind(A.into(), true, cx);
            assert_eq!(git.index.as_ref(), Some(&seen));
            assert_eq!(git.stats.as_ref().map(|s| s.files), Some(1));
            assert!(git.statuses.files.contains_key(&format!("{A}/a.rs")));
            assert!(git.cached_age().is_some());
        });
    }

    #[gpui::test]
    fn a_disconnected_switch_reads_nothing_and_a_local_root_is_never_offline(
        cx: &mut TestAppContext,
    ) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, cx| {
            git.bind(A.into(), true, cx);
            assert_eq!(
                git.reads, 0,
                "binding an unreachable workspace reads nothing"
            );
            git.bind("/work/local".into(), false, cx);
            assert!(!git.offline());
            assert_eq!(git.reads, 1);
        });
    }

    #[gpui::test]
    fn only_sixteen_remote_snapshots_are_remembered_and_local_ones_stay_apart(
        cx: &mut TestAppContext,
    ) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, _| {
            for n in 0..20 {
                let root = format!("remote://env-{n}/w");
                git.remember(&root, &snapshot(&root));
            }
            assert_eq!(git.remote.len(), crate::remote_cache::CAPACITY);
            assert!(git.remote.peek("remote://env-0/w").is_none());
            assert!(git.remote.peek("remote://env-19/w").is_some());
            git.remember("/work/local", &snapshot("/work/local"));
            assert_eq!(git.remote.len(), crate::remote_cache::CAPACITY);
            assert!(git.remembered.contains_key("/work/local"));
        });
    }

    fn reconnected(git: &Entity<ProjectGit>, cx: &mut TestAppContext) {
        git.update(cx, |git, cx| {
            git.bind(A.into(), true, cx);
            git.bind(A.into(), false, cx);
            assert!(!git.offline() && git.stale());
        });
    }

    #[gpui::test]
    fn a_good_read_that_started_after_the_reconnect_ends_the_recovery_even_with_one_queued(
        cx: &mut TestAppContext,
    ) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        reconnected(&git, cx);
        git.update(cx, |git, cx| {
            // A slow read (longer than the 2 s poll) finishes while the next one is queued.
            git.pending = true;
            let seq = git.read_seq;
            assert!(seq > git.recovery_floor);
            git.finish_read(seq, A, true, cx);
            assert!(!git.stale(), "`pending` must not keep the panels read-only");
        });
    }

    #[gpui::test]
    fn a_read_that_started_before_the_reconnect_does_not_end_the_recovery(cx: &mut TestAppContext) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, cx| {
            git.bind(A.into(), false, cx);
        });
        let old = git.update(cx, |git, _| git.read_seq);
        // The old read lands (and fails here) before the link drops and comes back.
        cx.run_until_parked();
        reconnected(&git, cx);
        git.update(cx, |git, cx| {
            git.finish_read(old, A, true, cx);
            assert!(
                git.stale(),
                "an old read says nothing about the reconnected workspace"
            );
            // Nor does a read of another project.
            let seq = git.read_seq;
            git.finish_read(seq, B, true, cx);
            assert!(git.stale());
            git.finish_read(seq, A, true, cx);
            assert!(!git.stale());
        });
    }

    #[gpui::test]
    fn a_failed_first_read_keeps_the_panels_read_only_and_reads_again(cx: &mut TestAppContext) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        reconnected(&git, cx);
        // The remote read fails here (no connection): still read-only, and a retry is queued.
        cx.run_until_parked();
        git.update(cx, |git, _| {
            assert!(git.stale() && git.recovery_retry.is_some());
            assert_eq!(git.recovery_tries, 1);
        });
        let reads = git.update(cx, |git, _| git.reads);
        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        git.update(cx, |git, _| {
            assert!(git.reads > reads, "the retry read again");
            assert!(git.stale());
        });
        // The escape ends it for a read that keeps failing, but a link that is down again stays
        // read-only.
        cx.executor()
            .advance_clock(RECOVERY_LIMIT + Duration::from_secs(1));
        cx.run_until_parked();
        git.update(cx, |git, cx| {
            assert!(!git.stale());
            git.bind(A.into(), true, cx);
            assert!(git.stale());
        });
    }

    #[gpui::test]
    fn a_hung_read_cannot_lock_the_panels_for_ever(cx: &mut TestAppContext) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, _| git.in_flight = true);
        reconnected(&git, cx);
        git.update(cx, |git, _| assert!(git.stale() && git.pending));
        cx.executor()
            .advance_clock(RECOVERY_LIMIT + Duration::from_secs(1));
        cx.run_until_parked();
        git.update(cx, |git, _| assert!(!git.stale()));
    }
}
