//! The project's Git status, shared by the right panel and the title bar:
//! GitChangesPanel's `useDiffIndex`, useGitFileStatuses and
//! useProjectDiffStats over one off-thread reader. Reads never overlap; a
//! read requested meanwhile runs once the current one returns, and only a
//! changed snapshot notifies.
use crate::remote_cache::RemoteCache;
use gpui::{AsyncApp, Context, EventEmitter, Task, WeakEntity};
use monocode_core::files::remote_watch::{self, Changes, LiveWatchHost, WatchHost};
use monocode_core::scm::{self, DiffIndex, DiffStats};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
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
/// A cloud workspace with a Host watch still reads its status this often, because a change in a
/// folder nobody has expanded is not watched.
const WATCHED_STATUS: Duration = Duration::from_secs(10);
/// Status polling of a cloud workspace while the window is unfocused or the panel is hidden.
const SLOW_POLL: Duration = Duration::from_secs(15);
/// Without the watch capability the loaded folders are listed again this often.
const RELIST: Duration = Duration::from_secs(5);
/// Folders the Explorer opens or closes are sent to the Host after this quiet time.
const DIRS_DEBOUNCE: Duration = Duration::from_millis(500);
/// After a failed watch call the next try comes this much later.
const WATCH_RETRY: Duration = Duration::from_secs(5);
/// A Host that cannot be asked about its capabilities is asked again this often.
const CAPABILITY_RETRY: Duration = Duration::from_secs(30);
/// A cloud workspace stops being watched or polled after this long without user input in the
/// window (ticks of `IDLE_TICK`), so an open panel cannot keep a workspace running.
const IDLE_TICK: Duration = Duration::from_secs(30);
const IDLE_TICKS: u32 = 20;

/// A disk or Git change: Changes, History and the Explorer reread.
pub(crate) struct GitChanged;

/// Folders of a cloud workspace the Explorer should list again: those named, or every loaded
/// one (`None`).
pub(crate) struct ReloadDirs {
    pub root: String,
    pub dirs: Option<Vec<String>>,
}

/// What a cloud workspace is doing to learn about changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// A local checkout: file system events, and a timer where there are none.
    Local,
    /// A cloud workspace while the window is unfocused or the panel is hidden: a slow poll.
    Slow,
    /// A cloud workspace in use: the Host watch, or polling when the Host has none.
    Live,
}

/// Whether the Host of the live cloud workspace can watch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Live {
    Unknown,
    Watch,
    Fallback,
}

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
    poll: Option<Vec<Task<()>>>,
    mode: Option<Mode>,
    live: Live,
    host: Arc<dyn WatchHost>,
    /// (root, id) of the Host watch in use.
    watch_id: Option<(String, String)>,
    /// The folders the Explorer has open, as full remote paths.
    watch_dirs: Vec<String>,
    dirs_task: Option<Task<()>>,
    window_active: bool,
    panel_visible: bool,
    /// No user input for `IDLE_TICKS` ticks: a cloud workspace is left alone until the next one.
    idle: bool,
    input_epoch: u64,
}
impl EventEmitter<GitChanged> for ProjectGit {}
impl EventEmitter<ReloadDirs> for ProjectGit {}

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
            mode: None,
            live: Live::Unknown,
            host: Arc::new(LiveWatchHost),
            watch_id: None,
            watch_dirs: Vec::new(),
            dirs_task: None,
            window_active: true,
            panel_visible: true,
            idle: false,
            input_epoch: 0,
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
    #[cfg(test)]
    pub(crate) fn watch_dirs_for_test(&self) -> Vec<String> {
        self.watch_dirs.clone()
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
            self.stop_poll(cx);
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
        self.warm_choices(cx);
        // The first read is the recovery's own; the watch and the timers start when it lands.
        self.apply_polling(true, cx);
        if self.mode.is_none() {
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
                    this.apply_polling(true, cx);
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
            self.apply_polling(false, cx);
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
        // The old root's watch ends; the new checkout is watched instead (this reads it
        // straight away).
        self.stop_poll(cx);
        self.watch_dirs.clear();
        self.apply_polling(true, cx);
        if self.mode.is_none() {
            self.reload(cx);
        }
    }
    /// While the project's Git status is visible, read it again whenever the
    /// checkout or its Git directory changes on disk, and every 30s besides.
    /// Without a file watcher, poll every 2s instead. A cloud workspace is watched through its
    /// Host instead (see `Mode`).
    pub(crate) fn set_polling(&mut self, polling: bool, cx: &mut Context<Self>) {
        self.polling_wanted = polling;
        self.apply_polling(true, cx);
    }
    /// The window gained or lost focus. A local checkout stops polling while unfocused; a cloud
    /// workspace polls slowly. Focus counts as user input.
    pub(crate) fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.window_active == active {
            return;
        }
        self.window_active = active;
        if active {
            self.input_epoch += 1;
            self.idle = false;
        }
        self.apply_polling(true, cx);
    }
    /// The right panel is shown or hidden.
    pub(crate) fn set_panel_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.panel_visible != visible {
            self.panel_visible = visible;
            self.apply_polling(true, cx);
        }
    }
    /// A key, click or scroll in the window. Cheap enough to call on every event.
    pub(crate) fn note_input(&mut self, cx: &mut Context<Self>) {
        self.input_epoch += 1;
        if self.idle {
            self.idle = false;
            self.apply_polling(true, cx);
        }
    }
    fn is_remote(&self) -> bool {
        self.project.starts_with("remote://")
    }
    /// What should be running now. Nothing runs for a workspace that is disconnected or whose
    /// reload after a reconnect has not landed, and nothing wakes one.
    fn wanted_mode(&self) -> Option<Mode> {
        if !self.polling_wanted || !self.available() || self.stale() {
            return None;
        }
        if !self.is_remote() {
            return self.window_active.then_some(Mode::Local);
        }
        if self.idle {
            None
        } else if self.window_active && self.panel_visible {
            Some(Mode::Live)
        } else {
            Some(Mode::Slow)
        }
    }
    /// Starts, changes or stops what runs, if what is wanted differs from what runs. `read`:
    /// read the status as the new mode starts.
    fn apply_polling(&mut self, read: bool, cx: &mut Context<Self>) {
        let wanted = self.wanted_mode();
        if wanted == self.mode {
            return;
        }
        self.stop_poll(cx);
        self.mode = wanted;
        match wanted {
            None => {}
            Some(Mode::Local) => self.start_local(read, cx),
            Some(Mode::Slow) => self.start_remote(read, false, cx),
            Some(Mode::Live) => self.start_remote(read, true, cx),
        }
    }
    /// Stops every timer and the watch, and ends the Host watch (best effort: the Host also
    /// drops it when its lease runs out).
    fn stop_poll(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        self.mode = None;
        self.live = Live::Unknown;
        self.dirs_task = None;
        if let Some((root, id)) = self.watch_id.take() {
            let host = self.host.clone();
            cx.background_executor()
                .spawn(async move { host.unwatch(&root, &id) })
                .detach();
        }
    }
    fn start_local(&mut self, read: bool, cx: &mut Context<Self>) {
        if read {
            self.reload(cx);
        }
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
        self.poll = Some(vec![watched, repair]);
    }
    /// A cloud workspace: a slow status poll, or (`live`) the Host watch with a status poll
    /// beside it, or the 2 s poll and the 5 s folder re-list when the Host cannot watch.
    fn start_remote(&mut self, read: bool, live: bool, cx: &mut Context<Self>) {
        if read {
            self.reload(cx);
        }
        let idle = cx.spawn(async move |this, cx| {
            let Ok(mut seen) = this.update(cx, |this, _| this.input_epoch) else {
                return;
            };
            let mut quiet = 0u32;
            loop {
                cx.background_executor().timer(IDLE_TICK).await;
                let Ok(done) = this.update(cx, |this, cx| {
                    if this.input_epoch != seen {
                        seen = this.input_epoch;
                        quiet = 0;
                    } else {
                        quiet += 1;
                    }
                    if quiet < IDLE_TICKS {
                        return false;
                    }
                    this.idle = true;
                    this.apply_polling(false, cx);
                    true
                }) else {
                    return;
                };
                if done {
                    return;
                }
            }
        });
        let status = cx.spawn(async move |this, cx| {
            loop {
                let Ok(every) = this.update(cx, |this, _| {
                    if !live {
                        SLOW_POLL
                    } else if this.live == Live::Watch {
                        WATCHED_STATUS
                    } else {
                        POLL
                    }
                }) else {
                    return;
                };
                cx.background_executor().timer(every).await;
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    return;
                }
            }
        });
        let mut tasks = vec![idle, status];
        if live {
            tasks.push(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(RELIST).await;
                    let Ok(()) = this.update(cx, |this, cx| {
                        if this.live == Live::Fallback {
                            let root = this.project.clone();
                            cx.emit(ReloadDirs { root, dirs: None });
                        }
                    }) else {
                        return;
                    };
                }
            }));
            tasks.push(cx.spawn(async move |this, cx| Self::watch_loop(&this, cx).await));
        }
        self.poll = Some(tasks);
    }
    /// Asks the Host once whether it can watch, then keeps one watch alive: waits for changes,
    /// and subscribes again when the Host forgot the watch.
    async fn watch_loop(this: &WeakEntity<Self>, cx: &mut AsyncApp) {
        let Ok((host, root)) = this.update(cx, |this, _| (this.host.clone(), this.project.clone()))
        else {
            return;
        };
        loop {
            let (asked, root_asked) = (host.clone(), root.clone());
            let capable = cx
                .background_executor()
                .spawn(async move { asked.supports_watch(&root_asked) })
                .await;
            match capable {
                Ok(true) => {}
                Ok(false) => {
                    let _ = this.update(cx, |this, _| this.live = Live::Fallback);
                    return;
                }
                Err(_) => {
                    let _ = this.update(cx, |this, _| this.live = Live::Fallback);
                    cx.background_executor().timer(CAPABILITY_RETRY).await;
                    continue;
                }
            }
            Self::subscription(this, cx, &host, &root).await;
        }
    }
    /// One watch, from its creation to the Host forgetting it or a failure. Returns after the
    /// pause that follows a failure.
    async fn subscription(
        this: &WeakEntity<Self>,
        cx: &mut AsyncApp,
        host: &Arc<dyn WatchHost>,
        root: &str,
    ) {
        let Ok(dirs) = this.update(cx, |this, _| this.watch_dirs.clone()) else {
            return;
        };
        let (h, r, d) = (host.clone(), root.to_owned(), dirs.clone());
        let created = cx
            .background_executor()
            .spawn(async move { h.watch(&r, &d) })
            .await;
        let id = match created {
            Ok(id) => id,
            Err(_) => {
                let _ = this.update(cx, |this, _| this.live = Live::Fallback);
                cx.background_executor().timer(WATCH_RETRY).await;
                return;
            }
        };
        let kept = this.update(cx, |this, cx| {
            this.live = Live::Watch;
            this.watch_id = Some((root.to_owned(), id.clone()));
            if this.watch_dirs != dirs {
                this.send_dirs(cx);
            }
        });
        if kept.is_err() {
            return;
        }
        loop {
            let (h, r, i) = (host.clone(), root.to_owned(), id.clone());
            let started = Instant::now();
            let waited = cx
                .background_executor()
                .spawn(async move { h.wait(&r, &i) })
                .await;
            match waited {
                Ok(changes) => {
                    let empty = changes == Changes::default();
                    if this
                        .update(cx, |this, cx| this.on_changes(root, changes, cx))
                        .is_err()
                    {
                        return;
                    }
                    if empty && started.elapsed() < Duration::from_millis(500) {
                        // The Host answers at once only when something is wrong.
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                    }
                }
                Err(error) => {
                    let unknown = remote_watch::is_watch_unknown(&error);
                    let _ = this.update(cx, |this, cx| {
                        this.watch_id = None;
                        if !unknown {
                            this.live = Live::Fallback;
                            let (h, r, i) = (this.host.clone(), root.to_owned(), id.clone());
                            cx.background_executor()
                                .spawn(async move { h.unwatch(&r, &i) })
                                .detach();
                        }
                    });
                    if !unknown {
                        cx.background_executor().timer(WATCH_RETRY).await;
                    }
                    return;
                }
            }
        }
    }
    /// The Host reported changes in the watched root.
    fn on_changes(&mut self, root: &str, changes: Changes, cx: &mut Context<Self>) {
        if root != self.project || self.stale() {
            return;
        }
        if changes.git {
            self.reload(cx);
        }
        if changes.overflow {
            cx.emit(ReloadDirs {
                root: root.to_owned(),
                dirs: None,
            });
        } else if !changes.dirs.is_empty() {
            cx.emit(ReloadDirs {
                root: root.to_owned(),
                dirs: Some(changes.dirs),
            });
        }
    }
    /// The folders the Explorer has open (full remote paths). The Host is told after a quiet
    /// moment, so opening several folders in a row is one call.
    pub(crate) fn set_watch_dirs(&mut self, mut dirs: Vec<String>, cx: &mut Context<Self>) {
        dirs.sort();
        dirs.dedup();
        if dirs == self.watch_dirs {
            return;
        }
        self.watch_dirs = dirs;
        if self.watch_id.is_none() {
            // The watch picks the folders up when it is created.
            return;
        }
        self.dirs_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DIRS_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.send_dirs(cx));
        }));
    }
    fn send_dirs(&mut self, cx: &mut Context<Self>) {
        let Some((root, id)) = self.watch_id.clone() else {
            return;
        };
        if root != self.project || self.stale() {
            return;
        }
        let (host, dirs) = (self.host.clone(), self.watch_dirs.clone());
        cx.background_executor()
            .spawn(async move {
                // A watch the Host forgot is found by the next wait, which subscribes again.
                let _ = host.update(&root, &id, &dirs);
            })
            .detach();
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
            assert!(
                !git.offline() && git.poll.is_none(),
                "polling waits for the first reload"
            );
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

    // --- the Host watch ---

    use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::Mutex};

    /// A Host that records every call and answers `wait` from a script. It has no way to wake
    /// anything: any call it sees came through the no-wake request path.
    struct FakeHost {
        supports: Mutex<Result<bool, String>>,
        waits: Mutex<VecDeque<Result<Changes, String>>>,
        watch_error: Mutex<Option<String>>,
        calls: Mutex<Vec<String>>,
        ids: Mutex<u32>,
    }
    impl FakeHost {
        fn new(supports: Result<bool, String>) -> Arc<Self> {
            Arc::new(Self {
                supports: Mutex::new(supports),
                waits: Mutex::new(VecDeque::new()),
                watch_error: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
                ids: Mutex::new(0),
            })
        }
        fn script(&self, wait: Result<Changes, String>) {
            self.waits.lock().unwrap().push_back(wait);
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn count(&self, prefix: &str) -> usize {
            self.calls()
                .iter()
                .filter(|c| c.starts_with(prefix))
                .count()
        }
    }
    impl WatchHost for FakeHost {
        fn supports_watch(&self, _: &str) -> Result<bool, String> {
            self.calls.lock().unwrap().push("supports".into());
            self.supports.lock().unwrap().clone()
        }
        fn watch(&self, root: &str, dirs: &[String]) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("watch {root} {}", dirs.join(",")));
            if let Some(error) = self.watch_error.lock().unwrap().clone() {
                return Err(error);
            }
            let mut ids = self.ids.lock().unwrap();
            *ids += 1;
            Ok(format!("w{}", *ids))
        }
        fn update(&self, _: &str, id: &str, dirs: &[String]) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("update {id} {}", dirs.join(",")));
            Ok(())
        }
        fn wait(&self, _: &str, id: &str) -> Result<Changes, String> {
            self.calls.lock().unwrap().push(format!("wait {id}"));
            self.waits
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(Changes::default()))
        }
        fn unwatch(&self, root: &str, id: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(format!("unwatch {root} {id}"));
        }
    }

    fn dirs_changed(dirs: &[&str]) -> Changes {
        Changes {
            dirs: dirs.iter().map(|d| d.to_string()).collect(),
            ..Default::default()
        }
    }

    /// A project bound to `A` and polling, with `host` as its Host.
    fn watched(
        host: &Arc<FakeHost>,
        cx: &mut TestAppContext,
    ) -> (Entity<ProjectGit>, Rc<RefCell<Vec<Option<Vec<String>>>>>) {
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        cx.update(|cx| {
            cx.subscribe(&git, move |_, event: &ReloadDirs, _| {
                sink.borrow_mut().push(event.dirs.clone())
            })
            .detach();
        });
        git.update(cx, |git, cx| {
            git.host = host.clone();
            git.bind(A.into(), false, cx);
            git.set_polling(true, cx);
        });
        cx.run_until_parked();
        (git, events)
    }
    fn reads(git: &Entity<ProjectGit>, cx: &mut TestAppContext) -> usize {
        git.update(cx, |git, _| git.reads)
    }
    fn advance(cx: &mut TestAppContext, by: Duration) {
        cx.executor().advance_clock(by);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn host_events_reload_the_status_and_the_named_folders_or_all_of_them(cx: &mut TestAppContext) {
        let host = FakeHost::new(Ok(true));
        host.script(Ok(dirs_changed(&[&format!("{A}/src")])));
        host.script(Ok(Changes {
            overflow: true,
            ..Default::default()
        }));
        let (git, events) = watched(&host, cx);
        assert_eq!(
            *events.borrow(),
            vec![Some(vec![format!("{A}/src")]), None],
            "named folders first, then every loaded one"
        );
        // Folder events read no status; the unscripted run shows what starting costs.
        let quiet = FakeHost::new(Ok(true));
        let (other, _) = watched(&quiet, cx);
        assert_eq!(reads(&git, cx), reads(&other, cx));
        assert_eq!(host.count("watch "), 1);
        assert!(host.count("wait w1") >= 3);
        git.update(cx, |git, _| assert_eq!(git.live, Live::Watch));
        // A `git` event reads the status once.
        let before = reads(&git, cx);
        git.update(cx, |git, cx| {
            let changes = Changes {
                git: true,
                ..Default::default()
            };
            git.on_changes(A, changes, cx);
        });
        assert_eq!(reads(&git, cx), before + 1);
        assert_eq!(events.borrow().len(), 2, "and lists no folder");
    }

    #[gpui::test]
    fn an_unknown_watch_subscribes_again(cx: &mut TestAppContext) {
        let host = FakeHost::new(Ok(true));
        host.script(Err(
            "Host rejected request: watch_unknown: watcher is unknown or expired".into(),
        ));
        host.script(Ok(Changes {
            git: true,
            ..Default::default()
        }));
        let (git, _) = watched(&host, cx);
        assert_eq!(host.count("watch "), 2, "{:?}", host.calls());
        assert!(host.count("wait w2") >= 1);
        assert_eq!(
            host.count("unwatch"),
            0,
            "the Host already forgot the old watch"
        );
        git.update(cx, |git, _| {
            assert_eq!(git.watch_id.as_ref().map(|w| w.1.as_str()), Some("w2"))
        });
    }

    #[gpui::test]
    fn without_the_capability_it_polls_every_two_seconds_and_lists_every_five(
        cx: &mut TestAppContext,
    ) {
        let host = FakeHost::new(Ok(false));
        let (git, events) = watched(&host, cx);
        assert_eq!(host.count("watch "), 0);
        assert_eq!(host.count("wait"), 0);
        git.update(cx, |git, _| assert_eq!(git.live, Live::Fallback));
        let before = reads(&git, cx);
        advance(cx, Duration::from_millis(2100));
        assert_eq!(reads(&git, cx), before + 1, "the 2 s git poll");
        assert!(events.borrow().is_empty());
        advance(cx, Duration::from_millis(3000));
        assert_eq!(
            *events.borrow(),
            vec![None],
            "the 5 s re-list of loaded folders"
        );
        assert_eq!(reads(&git, cx), before + 2);
    }

    #[gpui::test]
    fn with_a_watch_the_status_is_read_every_ten_seconds_and_folders_are_not_listed_on_a_timer(
        cx: &mut TestAppContext,
    ) {
        let host = FakeHost::new(Ok(true));
        let (git, events) = watched(&host, cx);
        // The first sleep was started before the Host answered; later ones follow the watch.
        advance(cx, Duration::from_millis(2100));
        let before = reads(&git, cx);
        advance(cx, Duration::from_secs(9));
        assert_eq!(reads(&git, cx), before, "no 2 s poll beside a watch");
        advance(cx, Duration::from_secs(2));
        assert_eq!(reads(&git, cx), before + 1, "the 10 s status poll");
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn a_host_that_fails_to_watch_is_polled_like_one_without_the_capability(
        cx: &mut TestAppContext,
    ) {
        let host = FakeHost::new(Ok(true));
        *host.watch_error.lock().unwrap() = Some("Unsupported workspace command".into());
        let (git, events) = watched(&host, cx);
        git.update(cx, |git, _| assert_eq!(git.live, Live::Fallback));
        advance(cx, Duration::from_millis(5100));
        assert_eq!(*events.borrow(), vec![None]);
        // It tries again after a pause.
        assert!(host.count("watch ") >= 2, "{:?}", host.calls());
    }

    #[gpui::test]
    fn an_unfocused_window_polls_slowly_and_focus_brings_the_watch_back(cx: &mut TestAppContext) {
        let host = FakeHost::new(Ok(true));
        let (git, _) = watched(&host, cx);
        git.update(cx, |git, cx| git.set_window_active(false, cx));
        cx.run_until_parked();
        assert_eq!(host.count("unwatch"), 1, "the watch ends when focus goes");
        let (reads_before, waits) = (reads(&git, cx), host.count("wait"));
        advance(cx, Duration::from_secs(14));
        assert_eq!(reads(&git, cx), reads_before);
        advance(cx, Duration::from_secs(2));
        assert_eq!(reads(&git, cx), reads_before + 1, "one read in 15 s");
        assert_eq!(host.count("wait"), waits, "no long-poll while unfocused");
        git.update(cx, |git, cx| {
            git.set_window_active(true, cx);
            assert_eq!(git.mode, Some(Mode::Live));
        });
        cx.run_until_parked();
        assert_eq!(host.count("watch "), 2);
        // A hidden panel is the same as an unfocused window.
        git.update(cx, |git, cx| git.set_panel_visible(false, cx));
        assert_eq!(git.update(cx, |git, _| git.mode), Some(Mode::Slow));
    }

    #[gpui::test]
    fn ten_minutes_without_input_stop_everything_and_input_starts_it_again(
        cx: &mut TestAppContext,
    ) {
        let host = FakeHost::new(Ok(true));
        let (git, _) = watched(&host, cx);
        // Input at nine and a half minutes keeps it going past ten.
        for _ in 0..19 {
            advance(cx, IDLE_TICK);
        }
        git.update(cx, |git, cx| git.note_input(cx));
        for _ in 0..19 {
            advance(cx, IDLE_TICK);
        }
        assert_eq!(git.update(cx, |git, _| git.mode), Some(Mode::Live));
        for _ in 0..3 {
            advance(cx, IDLE_TICK);
        }
        git.update(cx, |git, _| {
            assert!(git.idle && git.mode.is_none() && git.poll.is_none());
        });
        assert_eq!(host.count("unwatch"), 1);
        let (calls, reads_idle) = (host.calls().len(), reads(&git, cx));
        advance(cx, Duration::from_secs(120));
        assert_eq!(
            host.calls().len(),
            calls,
            "an idle window asks the Host nothing"
        );
        assert_eq!(reads(&git, cx), reads_idle);
        git.update(cx, |git, cx| git.note_input(cx));
        cx.run_until_parked();
        assert_eq!(host.count("watch "), 2, "the next input watches again");
        assert_eq!(git.update(cx, |git, _| git.mode), Some(Mode::Live));
    }

    #[gpui::test]
    fn nothing_runs_or_asks_the_host_while_disconnected_or_recovering(cx: &mut TestAppContext) {
        let host = FakeHost::new(Ok(true));
        let (git, events) = watched(&host, cx);
        git.update(cx, |git, cx| git.bind(A.into(), true, cx));
        cx.run_until_parked();
        let (calls, reads_off) = (host.calls(), reads(&git, cx));
        assert_eq!(host.count("unwatch"), 1);
        advance(cx, Duration::from_secs(120));
        git.update(cx, |git, cx| {
            git.set_polling(true, cx);
            git.set_window_active(false, cx);
            git.set_window_active(true, cx);
            git.set_panel_visible(false, cx);
            git.set_panel_visible(true, cx);
            git.note_input(cx);
            git.set_watch_dirs(vec![A.into(), format!("{A}/x")], cx);
            git.resume(cx);
            assert!(git.mode.is_none() && git.poll_is_off_for_test());
        });
        advance(cx, Duration::from_secs(120));
        assert_eq!(
            host.calls(),
            calls,
            "no call of any kind while disconnected"
        );
        assert_eq!(reads(&git, cx), reads_off);
        assert!(events.borrow().is_empty());
        // Connect: the old snapshot is read-only until a read lands, and meanwhile the Host is
        // left alone as well.
        git.update(cx, |git, cx| git.bind(A.into(), false, cx));
        cx.run_until_parked();
        git.update(cx, |git, _| assert!(git.stale() && git.mode.is_none()));
        advance(cx, Duration::from_secs(20));
        assert_eq!(host.calls(), calls, "no call while recovering");
        // The recovery ends (here by its time limit, the remote read cannot succeed in a test),
        // and the watch starts.
        advance(cx, RECOVERY_LIMIT);
        git.update(cx, |git, _| assert_eq!(git.mode, Some(Mode::Live)));
        assert_eq!(host.count("watch "), 2, "{:?}", host.calls());
    }

    #[gpui::test]
    fn switching_the_root_ends_the_old_watch_and_starts_one_on_the_new_root(
        cx: &mut TestAppContext,
    ) {
        let host = FakeHost::new(Ok(true));
        let (git, _) = watched(&host, cx);
        git.update(cx, |git, cx| {
            git.set_watch_dirs(vec![format!("{A}/src")], cx)
        });
        cx.run_until_parked();
        git.update(cx, |git, cx| git.bind(B.into(), false, cx));
        cx.run_until_parked();
        let calls = host.calls();
        assert!(
            calls.contains(&format!("unwatch {A} w1")),
            "the old root's watch is ended: {calls:?}"
        );
        assert!(
            calls.contains(&format!("watch {B} ")),
            "the new root starts with no folders of the old one: {calls:?}"
        );
        git.update(cx, |git, _| {
            assert_eq!(git.watch_id, Some((B.into(), "w2".into())));
            assert!(git.watch_dirs.is_empty());
        });
        // An event of the old root that was already on its way is dropped.
        git.update(cx, |git, cx| {
            git.on_changes(A, dirs_changed(&[A]), cx);
        });
    }

    #[gpui::test]
    fn folders_the_explorer_opens_are_sent_once_after_a_quiet_moment(cx: &mut TestAppContext) {
        let host = FakeHost::new(Ok(true));
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, cx| {
            git.host = host.clone();
            git.bind(A.into(), false, cx);
            // Folders known before the watch exist go into the watch itself.
            git.set_watch_dirs(vec![A.into()], cx);
            git.set_polling(true, cx);
        });
        cx.run_until_parked();
        assert_eq!(host.count(&format!("watch {A} {A}")), 1);
        assert_eq!(host.count("update"), 0);
        git.update(cx, |git, cx| {
            git.set_watch_dirs(vec![A.into(), format!("{A}/b")], cx)
        });
        advance(cx, Duration::from_millis(300));
        git.update(cx, |git, cx| {
            git.set_watch_dirs(vec![A.into(), format!("{A}/b"), format!("{A}/c")], cx)
        });
        advance(cx, Duration::from_millis(300));
        assert_eq!(host.count("update"), 0, "still debouncing");
        advance(cx, Duration::from_millis(300));
        assert_eq!(
            host.calls()
                .iter()
                .filter(|c| c.starts_with("update"))
                .collect::<Vec<_>>(),
            vec![&format!("update w1 {A},{A}/b,{A}/c")]
        );
        // The same list again sends nothing.
        git.update(cx, |git, cx| {
            git.set_watch_dirs(vec![format!("{A}/c"), A.into(), format!("{A}/b")], cx)
        });
        advance(cx, Duration::from_secs(2));
        assert_eq!(host.count("update"), 1);
    }

    #[gpui::test]
    fn a_host_that_cannot_be_asked_is_polled_and_asked_again_later(cx: &mut TestAppContext) {
        let host = FakeHost::new(Err("Machine is unreachable.".into()));
        let (git, events) = watched(&host, cx);
        git.update(cx, |git, _| assert_eq!(git.live, Live::Fallback));
        advance(cx, Duration::from_millis(5100));
        assert_eq!(*events.borrow(), vec![None]);
        *host.supports.lock().unwrap() = Ok(true);
        advance(cx, CAPABILITY_RETRY);
        git.update(cx, |git, _| assert_eq!(git.live, Live::Watch));
        assert_eq!(host.count("watch "), 1);
    }

    #[gpui::test]
    fn a_local_project_keeps_its_own_polling_and_never_asks_the_host(cx: &mut TestAppContext) {
        let host = FakeHost::new(Ok(true));
        let git = cx.new(|cx| ProjectGit::new("~".into(), cx));
        git.update(cx, |git, cx| {
            git.host = host.clone();
            git.bind("/work/local".into(), false, cx);
            git.set_polling(true, cx);
            assert_eq!(git.mode, Some(Mode::Local));
            git.set_window_active(false, cx);
            assert!(git.mode.is_none(), "a local checkout stops while unfocused");
        });
        cx.run_until_parked();
        assert!(host.calls().is_empty());
    }
}
