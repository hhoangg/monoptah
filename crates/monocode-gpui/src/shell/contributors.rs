//! Contributors plumbing that outlives the page: the core handle, the
//! reconcile triggers and the footer sync chip.
use super::*;
use crate::contributors::{AskChoices, GithubView, SyncJobs, Tab};
use monocode_core::contributors::{
    api::Contributors,
    jobs::SyncEvent,
    model::{
        agent_session::{
            attachments_supported, compose_session_prompt, contributors_session_title,
            failure_message,
        },
        ask::AskAgentRequest,
        snapshot::SnapshotScope,
    },
};
use std::sync::Arc;

/// What the footer chip shows and does.
pub(super) struct ChipState {
    /// `42%`, empty when only a failure is left.
    pub label: String,
    pub aria: String,
    /// One line per job.
    pub title: String,
    pub failed: bool,
    /// The project a click opens: the failed job's, else the first running one.
    pub root: String,
    /// The failure a click dismisses.
    pub failure: Option<SyncEvent>,
}

/// A private data folder for a store that has none (in memory); removed with the core.
pub(super) struct ScratchDir(std::path::PathBuf);
impl ScratchDir {
    fn new() -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "monocode-contributors-{}-{}-{}",
            std::process::id(),
            monocode_core::time::now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        Ok(Self(dir))
    }
}
impl ScratchDir {
    pub(super) fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A session ready to be shown: created, its settings saved, the snapshot written.
struct Prepared {
    summary: monocode_core::store::Session,
    settings: monocode_core::agent::Settings,
    prompt: String,
    attachments: Vec<std::path::PathBuf>,
}

/// The part of "Start session" that touches disk. Nothing is created until the
/// snapshot is written, so a failure there leaves no stray session.
fn prepare_session(
    store: &Database,
    kind: Kind,
    project: &str,
    request: &AskAgentRequest,
    settings: monocode_core::agent::Settings,
) -> Result<Prepared, String> {
    let path = monocode_core::attachment::write_attachment(
        &request.file_name,
        request.snapshot_json.as_bytes(),
    )
    .map_err(|error| failure_message("write the snapshot", Some(&error)))?;
    // A provider that takes attachments gets the file checked like a dropped one;
    // the others are told where it is.
    let mut attachments = Vec::new();
    if attachments_supported(kind) {
        monocode_core::composer::validate_attachments(std::slice::from_ref(&path))
            .map_err(|error| failure_message("attach the snapshot", Some(&error)))?;
        attachments.push(path.clone());
    }
    let prompt = compose_session_prompt(
        &request.prompt,
        &path.to_string_lossy(),
        !attachments.is_empty(),
    );
    let started = |error: String| failure_message("start the session", Some(&error));
    let store = locked(store).map_err(started)?;
    // Always a chat: the app submits the prompt itself, whatever the surface preference.
    let mut summary = store.create(kind.id(), project).map_err(started)?;
    let title = contributors_session_title(&request.file_name);
    store.title(summary.id, &title).map_err(started)?;
    summary.title = title;
    store
        .save_agent_settings(summary.id, &settings)
        .map_err(started)?;
    Ok(Prepared {
        summary,
        settings,
        prompt,
        attachments,
    })
}

struct SyncTip(Vec<SharedString>);
impl Render for SyncTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut background = shell_fill();
        background.a = 1.;
        div()
            .max_w(px(520.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(background)
            .border_1()
            .border_color(content(0.12))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.85))
            .children(self.0.iter().cloned().map(|line| div().child(line)))
    }
}

impl Shell {
    pub(super) fn init_contributors(&mut self, cx: &mut Context<Self>) {
        let jobs = self.sync_jobs.clone();
        self.observations.push(cx.observe(&jobs, |this, _, cx| {
            this.invalidate_chrome(cx);
            cx.notify();
        }));
    }

    /// The providers the Ask dialog offers for `project`: the ones the model
    /// picker shows, with the project's default preselected.
    pub(super) fn ask_choices(&self, project: &str) -> AskChoices {
        let hidden = self.workspace.preferences.hidden_for(project);
        AskChoices {
            providers: super::provider_prompt::new_session_providers(
                &hidden,
                self.installed_providers.as_deref(),
            ),
            default: Some(self.default_kind_in(project)),
        }
    }

    /// An Ask Agent button was pressed on the GitHub tab of `project`.
    pub(super) fn open_contributors_ask(
        &mut self,
        project: &str,
        scope: &SnapshotScope,
        view: &Entity<GithubView>,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.contributors.clone() else {
            return;
        };
        // The CLIs may have been installed since the last look.
        self.probe_installed_providers(cx);
        let choices = self.ask_choices(project);
        page.update(cx, |page, cx| {
            page.open_ask(
                project.to_string(),
                scope.clone(),
                view.clone(),
                choices,
                cx,
            )
        });
    }

    /// "Start session" in the dialog: the page hears how it went.
    pub(super) fn start_contributors_ask(
        &mut self,
        project: &str,
        request: &AskAgentRequest,
        start: u64,
        cx: &mut Context<Self>,
    ) {
        let page = self.contributors.clone();
        self.start_contributors_session(
            project.to_string(),
            request.clone(),
            cx,
            move |_, result, cx| {
                if let Some(page) = page {
                    page.update(cx, |page, cx| page.ask_finished(start, result, cx));
                }
            },
        );
    }

    /// Writes the snapshot, creates a chat in `project` (the provider chosen in
    /// the dialog, else the project's default), submits the prompt with the file
    /// attached, opens the session and leaves the page. `done` hears the outcome;
    /// on an error nothing was created.
    pub(super) fn start_contributors_session(
        &mut self,
        project: String,
        request: AskAgentRequest,
        cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<(), String>, &mut Context<Self>) + 'static,
    ) {
        if project.is_empty()
            || monocode_core::files::remote::is_remote(std::path::Path::new(&project))
        {
            done(
                self,
                Err("Contributors works on local projects only.".into()),
                cx,
            );
            return;
        }
        let kind = request
            .harness
            .unwrap_or_else(|| self.default_kind_in(&project));
        let settings = self.project_session_settings(kind, &project, cx);
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { prepare_session(&store, kind, &project, &request, settings) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Prepared {
                    summary,
                    settings,
                    prompt,
                    attachments,
                }) => {
                    let id = summary.id;
                    this.add(summary, cx);
                    this.sessions[&id].update(cx, |view, cx| {
                        view.quick_submit_with(prompt, settings, attachments, cx)
                    });
                    this.activate_session(id, window, cx);
                    this.persist(cx);
                    done(this, Ok(()), cx);
                }
                Err(error) => done(this, Err(error), cx),
            });
        })
        .detach();
    }

    /// Opens the core off the UI thread, once. The page gets it when it is ready;
    /// a failure is shown on the page and the next open tries again.
    pub(super) fn ensure_contributors_core(&mut self, cx: &mut Context<Self>) {
        if self.contributors_core.is_some() || self.contributors_opening {
            return;
        }
        self.contributors_opening = true;
        let jobs = self.sync_jobs.read(cx);
        let (sync, tickets) = (jobs.sync_sink(), jobs.ticket_sink());
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let (path, dir) = {
                let store = locked(&store)?;
                (
                    store.database_path().to_path_buf(),
                    store.data_dir().map(std::path::Path::to_path_buf),
                )
            };
            // Only an in-memory store has no data folder: give it a private one.
            let (dir, scratch) = match dir {
                Some(dir) => (dir, None),
                None => {
                    let scratch = ScratchDir::new()?;
                    (scratch.path().to_path_buf(), Some(scratch))
                }
            };
            Contributors::open(&path, &dir, sync, tickets).map(|core| (Arc::new(core), scratch))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.contributors_opening = false;
                let result = result.map(|(core, scratch)| {
                    this.contributors_scratch = scratch;
                    core
                });
                if let Ok(core) = &result {
                    this.contributors_core = Some(core.clone());
                    this.reconcile_sync_jobs(cx);
                }
                if let Some(page) = this.contributors.clone() {
                    page.update(cx, |page, cx| page.set_core(result, cx));
                }
            });
        })
        .detach();
    }

    /// Asks the core what runs and fixes the job list; no-op before the core is open.
    pub(super) fn reconcile_sync_jobs(&mut self, cx: &mut Context<Self>) {
        if let Some(core) = self.contributors_core.clone() {
            self.sync_jobs
                .update(cx, |jobs: &mut SyncJobs, cx| jobs.reconcile(core, cx));
        }
    }

    pub(super) fn sync_chip_state(&self, cx: &App) -> Option<ChipState> {
        let summary = self.sync_jobs.read(cx).summary()?;
        let failed = summary.failed.is_some();
        Some(ChipState {
            label: summary
                .percent
                .map(|percent| format!("{percent}%"))
                .unwrap_or_default(),
            aria: if failed {
                "Contributors sync stopped".into()
            } else {
                format!("Syncing contributors {}%", summary.percent.unwrap_or(0))
            },
            title: summary.title,
            failed,
            root: summary.target.root,
            failure: summary.failed,
        })
    }

    /// UsageFooter's SyncIndicator: the analytics icon and the percent of the
    /// running syncs. A failure turns it red and keeps it until the click,
    /// which opens that project's page and dismisses the failure.
    pub(super) fn sync_chip(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.sync_chip_state(cx)?;
        let classic = self.workspace.boxy_layout();
        let alone = self.app_view_open() || self.workspace.project.is_empty();
        let ChipState {
            label,
            aria,
            title,
            failed,
            root,
            failure,
        } = state;
        let lines: Vec<SharedString> = title.lines().map(|line| line.to_string().into()).collect();
        Some(
            div()
                .flex_shrink_0()
                .map(|d| match (classic, alone) {
                    (true, true) => d,
                    (true, false) => d.mr(px(terminal_dock::FOOTER_GAP)),
                    (false, true) => d.mr(px(6.)).pb(px(2.)),
                    (false, false) => d.mr(px(terminal_dock::FOOTER_GAP)).pb(px(2.)),
                })
                .child(
                    div()
                        .id("contributors-sync")
                        .debug_selector(|| "contributors-sync".into())
                        .role(Role::Button)
                        .aria_label(aria)
                        .aria_description(title)
                        .cursor_pointer()
                        .h(px(20.))
                        .px(px(6.))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .whitespace_nowrap()
                        .text_size(px(11.))
                        .text_color(if failed {
                            rgb(0xf87171).into()
                        } else {
                            accent_color()
                        })
                        .hover(|s| s.bg(content(0.1)))
                        .tooltip(move |_, cx| cx.new(|_| SyncTip(lines.clone())).into())
                        .child(icon("analytics", 14.))
                        .when(!label.is_empty(), |d| d.child(label))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_contributors(
                                Some(Tab::Github),
                                Some(root.clone()),
                                window,
                                cx,
                            );
                            if let Some(failure) = &failure {
                                this.sync_jobs
                                    .update(cx, |jobs, cx| jobs.dismiss(failure, cx));
                            }
                        })),
                )
                .into_any_element(),
        )
    }
}
