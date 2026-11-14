//! The cloud status chip next to the provider usage chip in the footer: how many cloud
//! workspaces (codespaces and Docker sandboxes) of the open project are running, so a forgotten
//! one that keeps billing is visible without opening the project menu. Clicking it (or Enter or
//! Space with it focused) opens the Cloud workspaces dialog, the same one the menu item opens.
//!
//! What it reads is [`monocode_core::cloud_sessions::footer_status::FooterStatus`], computed
//! from the overview the dialog loads (`gh codespace list` and `sbx ls`, which never wake a
//! workspace, cached by the core service for 30 s). Asking is deliberately sparse:
//!
//! * only for a project that uses cloud ([`monocode_core::cloud_sessions::CloudSessions::project_uses_cloud`],
//!   a store read) so a plain project never starts `gh`;
//! * at most about once a minute ([`REFRESH_EVERY`]) and only while the window is active;
//! * a failing `gh` or `sbx` is silent: that backend lists nothing, and with nothing running the
//!   chip is hidden. No toast, no error text.
use super::OpenCloudStatus;
use crate::{
    cloud_mark::{CloudTip, backend_name},
    cloud_workspaces_dialog::Env,
    theme::*,
};
use gpui::{
    Context, EventEmitter, FocusHandle, Focusable, Render, Role, SharedString, Window, div,
    prelude::*, px,
};
use monocode_core::cloud_sessions::footer_status::{FooterEntry, FooterStatus};
use std::time::Duration;

/// How often the chip asks again while the window is active. The core service reuses an overview
/// for 30 s ([`monocode_core::cloud_sessions::workspaces::WORKSPACES_TTL`]), so every ask is a
/// real read of `gh` and `sbx`.
pub(crate) const REFRESH_EVERY: Duration = Duration::from_secs(60);
/// The tooltip names at most this many workspaces, then "and N more".
const TIP_ENTRIES: usize = 5;

pub(crate) enum Event {
    /// The chip was activated: open the Cloud workspaces dialog of this project, reading through
    /// the same service the chip reads through.
    Open { project: String, env: Env },
}

pub(crate) struct CloudChip {
    /// `None` until the shell hands over its store; the chip then asks nobody.
    env: Option<Env>,
    /// The local project the chip reports on (a cloud session's own project, not its
    /// `remote://` folder). Empty: none.
    project: String,
    window_active: bool,
    classic: bool,
    status: FooterStatus,
    /// A load is running.
    loading: bool,
    /// A load started since the last tick: events (a new project, the window coming back) do not
    /// ask again until the next one, which keeps it to about one ask per [`REFRESH_EVERY`].
    asked: bool,
    /// Bumped when the project changes, so a late answer for the old one is dropped.
    generation: u64,
    focus: FocusHandle,
    _tick: gpui::Task<()>,
}

impl EventEmitter<Event> for CloudChip {}

impl Focusable for CloudChip {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CloudChip {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_EVERY).await;
                if this
                    .update(cx, |this, cx| {
                        this.asked = false;
                        this.refresh(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            env: None,
            project: String::new(),
            window_active: false,
            classic: false,
            status: FooterStatus::default(),
            loading: false,
            asked: false,
            generation: 0,
            focus: cx.focus_handle(),
            _tick: tick,
        }
    }

    pub(crate) fn set_env(&mut self, env: Env, cx: &mut Context<Self>) {
        self.env = Some(env);
        self.refresh(cx);
    }

    /// The project the chip reports on. A different one hides the chip until it has been read.
    pub(crate) fn set_project(&mut self, project: String, cx: &mut Context<Self>) {
        if self.project == project {
            return;
        }
        self.project = project;
        self.generation += 1;
        self.asked = false;
        if self.status != FooterStatus::default() {
            self.status = FooterStatus::default();
            cx.notify();
        }
        self.refresh(cx);
    }

    /// The window gained or lost focus. Coming back asks again (when the last ask is older than
    /// a tick); while it is inactive nothing is asked.
    pub(crate) fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.window_active == active {
            return;
        }
        self.window_active = active;
        if active {
            self.refresh(cx);
        }
    }

    pub(crate) fn set_classic(&mut self, classic: bool, cx: &mut Context<Self>) {
        if self.classic != classic {
            self.classic = classic;
            cx.notify();
        }
    }

    /// A stop or delete changed what the dialog shows: read again right away (the core service
    /// forgot its cached overview, so this is a real read), if the window is active.
    pub(crate) fn changed(&mut self, cx: &mut Context<Self>) {
        self.asked = false;
        self.refresh(cx);
    }

    /// Something runs or starts: the chip shows.
    pub(crate) fn visible(&self) -> bool {
        self.status.visible()
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> &FooterStatus {
        &self.status
    }
    #[cfg(test)]
    pub(crate) fn project(&self) -> &str {
        &self.project
    }
    #[cfg(test)]
    pub(crate) fn warns(&self) -> bool {
        self.status.warns()
    }
    #[cfg(test)]
    pub(crate) fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.window_active || self.loading || self.asked || self.project.is_empty() {
            return;
        }
        let Some(env) = self.env.clone() else { return };
        // A test that did not bring its own service must never reach the real gh and sbx.
        if cfg!(test) && env.api.is_none() {
            return;
        }
        self.loading = true;
        self.asked = true;
        let (project, generation) = (self.project.clone(), self.generation);
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_executor()
                .spawn(async move { read(&env, &project) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                // The project changed while this ran: it answers a question nobody asks now.
                if this.generation != generation {
                    this.asked = false;
                    this.refresh(cx);
                    return;
                }
                if this.status != status {
                    this.status = status;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        if let Some(env) = self.env.clone().filter(|_| !self.project.is_empty()) {
            cx.emit(Event::Open {
                project: self.project.clone(),
                env,
            });
        }
    }
}

/// One read, on a worker thread. Anything that goes wrong reads as "nothing runs".
fn read(env: &Env, project: &str) -> FooterStatus {
    let Ok(api) = env.api() else {
        return FooterStatus::default();
    };
    let cloud = api.cloud();
    if !cloud.project_uses_cloud(project) {
        return FooterStatus::default();
    }
    FooterStatus::of(
        &cloud.workspaces_overview(project, false),
        monocode_core::time::now_ms(),
    )
}

/// The chip's text: `2 running`.
pub(crate) fn label(status: &FooterStatus) -> String {
    format!("{} running", status.running)
}

fn entry_line(entry: &FooterEntry) -> String {
    let mut line = format!(
        "{} \u{b7} {} \u{b7} {}",
        backend_name(entry.backend),
        entry.name,
        entry.state.label()
    );
    if let Some(concern) = entry.concern {
        line.push_str(" \u{2014} ");
        line.push_str(concern.label());
    }
    line
}

/// What the tooltip and the accessible description say: the headline, a line per workspace
/// (the ones with a concern first, the rest as the overview ordered them) and how to act.
pub(crate) fn tip_text(status: &FooterStatus) -> String {
    let mut lines = vec![format!("Cloud workspaces: {}", label(status))];
    let mut entries: Vec<&FooterEntry> = status.entries.iter().collect();
    entries.sort_by_key(|e| e.concern.is_none());
    lines.extend(entries.iter().take(TIP_ENTRIES).map(|e| entry_line(e)));
    if entries.len() > TIP_ENTRIES {
        lines.push(format!("and {} more", entries.len() - TIP_ENTRIES));
    }
    lines.push("Click to manage".into());
    lines.join("\n")
}

impl Render for CloudChip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible() {
            return div().into_any_element();
        }
        let warns = self.status.warns();
        let classic = self.classic;
        let tip: SharedString = tip_text(&self.status).into();
        let label = label(&self.status);
        let color = if warns {
            warning_color()
        } else {
            content(0.55)
        };
        div()
            .id("cloud-status-chip")
            .debug_selector(|| "cloud-status-chip".into())
            .role(Role::Button)
            .aria_label(format!(
                "Cloud workspaces: {label}{}",
                if warns { ", needs attention" } else { "" }
            ))
            .aria_description(tip.clone())
            .track_focus(&self.focus)
            .key_context("CloudChip")
            .tab_index(0)
            .h(px(20.))
            .map(|d| {
                if classic {
                    d.px(px(4.)).rounded_md()
                } else {
                    d.px(px(8.)).rounded_full()
                }
            })
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(5.))
            .text_color(color)
            .when(warns, |d| d.bg(warning_color().opacity(0.14)))
            .cursor_pointer()
            .hover(move |s| {
                s.bg(if warns {
                    warning_color().opacity(0.24)
                } else {
                    content(0.1)
                })
            })
            .child(icon("cloud", 12.).text_color(color))
            .child(label)
            .tooltip(move |_, cx| cx.new(|_| CloudTip(tip.clone())).into())
            .on_click(cx.listener(|this, _, _, cx| this.open(cx)))
            .on_action(cx.listener(|this, _: &OpenCloudStatus, _, cx| this.open(cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_workspaces_dialog::tests::{self as fixtures, World};
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};
    use monocode_core::cloud_sessions::{
        CloudBackendKind,
        footer_status::{Concern, FooterEntry},
        workspaces::WorkspaceState,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    // --------------------------------------------------------------- pure text

    fn entry(name: &str, state: WorkspaceState, concern: Option<Concern>) -> FooterEntry {
        FooterEntry {
            name: name.into(),
            backend: CloudBackendKind::Codespaces,
            state,
            concern,
        }
    }

    #[test]
    fn the_label_counts_and_the_tooltip_names_each_workspace_with_the_worrying_ones_first() {
        let status = FooterStatus {
            running: 2,
            entries: vec![
                entry("fake-fine", WorkspaceState::Running, None),
                entry("fake-lost", WorkspaceState::Running, Some(Concern::Orphan)),
            ],
        };
        assert_eq!(label(&status), "2 running");
        let tip = tip_text(&status);
        let lines: Vec<_> = tip.lines().collect();
        assert_eq!(lines[0], "Cloud workspaces: 2 running");
        assert_eq!(
            lines[1],
            "Codespace \u{b7} fake-lost \u{b7} Running \u{2014} no session uses it"
        );
        assert_eq!(lines[2], "Codespace \u{b7} fake-fine \u{b7} Running");
        assert_eq!(lines[3], "Click to manage");
    }

    #[test]
    fn a_long_list_is_cut_after_a_few_names() {
        let entries: Vec<_> = (0..8)
            .map(|i| entry(&format!("fake-{i}"), WorkspaceState::Running, None))
            .collect();
        let status = FooterStatus {
            running: 8,
            entries,
        };
        let tip = tip_text(&status);
        assert_eq!(tip.lines().count(), 1 + TIP_ENTRIES + 1 + 1, "{tip}");
        assert!(tip.contains("and 3 more"), "{tip}");
        assert!(!tip.contains("fake-7"));
    }

    // ------------------------------------------------------------ with a window

    /// Everything of the fixture world that is up has a session and is young: nothing to warn
    /// about (the world's hibernating, unused sandbox is removed).
    fn quiet(w: &World) {
        w.sbx.boxes.lock().unwrap().retain(|b| b.id == "sbx_001a");
    }

    fn chip_env(w: &World) -> Env {
        Env {
            api: Some(w.api.clone()),
            store: w.store.clone(),
        }
    }

    fn open_chip(w: &World, cx: &mut TestAppContext, active: bool) -> WindowHandle<CloudChip> {
        cx.update(super::super::bind);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(CloudChip::new))
                .unwrap()
        });
        let (env, project) = (chip_env(w), w.checkout.path());
        window
            .update(cx, |chip, _, cx| {
                chip.set_env(env, cx);
                chip.set_project(project, cx);
                chip.set_window_active(active, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
    }

    /// Asks again as the stop and delete paths do: the service forgets its cached overview.
    fn reread(window: &WindowHandle<CloudChip>, w: &World, cx: &mut TestAppContext) {
        w.api.cloud().forget_workspace_views();
        window.update(cx, |chip, _, cx| chip.changed(cx)).unwrap();
        cx.run_until_parked();
    }

    fn status_of(window: &WindowHandle<CloudChip>, cx: &mut TestAppContext) -> FooterStatus {
        window
            .read_with(cx, |chip, _| chip.status().clone())
            .unwrap()
    }

    fn set_codespaces(w: &World, list: serde_json::Value) {
        *w.gh.list.lock().unwrap() = list.to_string();
    }

    fn codespace(state: &str, hours: f64) -> serde_json::Value {
        fixtures::codespace(
            fixtures::USED,
            "basicLinux32gb",
            state,
            fixtures::hours_ago(hours),
        )
    }

    fn listed(w: &World) -> usize {
        w.gh.count("codespace list")
    }

    #[gpui::test]
    fn it_shows_only_while_a_workspace_runs_and_counts_what_runs(cx: &mut TestAppContext) {
        let w = fixtures::world();
        quiet(&w);
        let window = open_chip(&w, cx, true);
        let status = status_of(&window, cx);
        assert_eq!(status.running, 2, "{status:?}");
        assert_eq!(label(&status), "2 running");
        window
            .read_with(cx, |chip, _| assert!(chip.visible() && !chip.warns()))
            .unwrap();

        // Everything stops: the chip goes away and takes no room.
        set_codespaces(&w, json!([codespace("Shutdown", 3.)]));
        w.sbx.boxes.lock().unwrap().clear();
        reread(&window, &w, cx);
        window
            .read_with(cx, |chip, _| assert!(!chip.visible()))
            .unwrap();

        // A starting workspace counts as running.
        set_codespaces(&w, json!([codespace("Starting", 0.1)]));
        reread(&window, &w, cx);
        assert_eq!(status_of(&window, cx).running, 1);
    }

    #[gpui::test]
    fn it_warns_for_an_orphan_a_failure_and_a_workspace_up_for_hours(cx: &mut TestAppContext) {
        // The world's unused, hibernating sandbox belongs to this app and no session uses it.
        let w = fixtures::world();
        let window = open_chip(&w, cx, true);
        let status = status_of(&window, cx);
        assert!(status.visible() && status.warns(), "{status:?}");
        assert_eq!(
            status.entries.iter().find_map(|e| e.concern),
            Some(Concern::Orphan)
        );
        assert!(tip_text(&status).contains("no session uses it"));

        // Without it nothing is abnormal.
        quiet(&w);
        reread(&window, &w, cx);
        window
            .read_with(cx, |chip, _| assert!(!chip.warns()))
            .unwrap();

        // A failed workspace next to running ones.
        let failed = fixtures::codespace(
            fixtures::LOST,
            "basicLinux32gb",
            "Failed",
            fixtures::hours_ago(1.),
        );
        set_codespaces(&w, json!([codespace("Available", 1.), failed]));
        reread(&window, &w, cx);
        let status = status_of(&window, cx);
        assert!(status.warns() && status.running == 2, "{status:?}");
        assert!(
            status
                .entries
                .iter()
                .any(|e| e.concern == Some(Concern::Failed))
        );

        // Up for longer than the threshold with no session working on it.
        set_codespaces(&w, json!([codespace("Available", 7.)]));
        w.sbx.boxes.lock().unwrap().clear();
        reread(&window, &w, cx);
        let status = status_of(&window, cx);
        assert_eq!(status.entries[0].concern, Some(Concern::LongRunning));

        // Younger than the threshold: no warning.
        set_codespaces(&w, json!([codespace("Available", 5.)]));
        reread(&window, &w, cx);
        let status = status_of(&window, cx);
        assert!(status.visible() && !status.warns(), "{status:?}");
    }

    /// The footer draws the chip only while it shows.
    #[gpui::test]
    fn the_footer_gives_the_chip_room_only_while_it_shows(cx: &mut TestAppContext) {
        let w = fixtures::world();
        quiet(&w);
        cx.update(super::super::bind);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    super::super::UsageFooter::new(monocode_core::provider::Kind::Codex, cx)
                })
            })
            .unwrap()
        });
        let (env, project) = (chip_env(&w), w.checkout.path());
        let chip = window
            .read_with(cx, |footer, _| footer.cloud_chip().clone())
            .unwrap();
        window
            .update(cx, |footer, _, cx| footer.set_cloud_env(env, cx))
            .unwrap();
        chip.update(cx, |chip, cx| {
            chip.set_project(project, cx);
            chip.set_window_active(true, cx);
        });
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        visual.run_until_parked();
        let footer = visual.debug_bounds("usage-footer").expect("the footer");
        let at = visual.debug_bounds("cloud-status-chip").expect("the chip");
        assert!(
            footer.contains(&at.origin) && at.size.width > px(20.),
            "{footer:?} {at:?}"
        );

        // Nothing runs any more: no chip.
        w.sbx.boxes.lock().unwrap().clear();
        set_codespaces(&w, json!([]));
        w.api.cloud().forget_workspace_views();
        chip.update(&mut visual, |chip, cx| chip.changed(cx));
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("cloud-status-chip").is_none());
    }

    #[gpui::test]
    fn a_click_or_enter_or_space_opens_the_dialog_of_the_project(cx: &mut TestAppContext) {
        let w = fixtures::world();
        quiet(&w);
        let window = open_chip(&w, cx, true);
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let chip = window.entity(cx).unwrap();
        cx.update(|cx| {
            cx.subscribe(&chip, move |_, event: &Event, _| match event {
                Event::Open { project, .. } => log.lock().unwrap().push(project.clone()),
            })
            .detach();
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        visual.run_until_parked();
        let at = visual
            .debug_bounds("cloud-status-chip")
            .expect("the chip")
            .center();
        visual.simulate_click(at, Default::default());
        visual.run_until_parked();
        assert_eq!(*seen.lock().unwrap(), [w.checkout.path()]);

        // Keyboard: the chip takes focus like the usage chip and both keys activate it.
        window
            .update(&mut visual, |chip, window, cx| {
                window.focus(chip.focus(), cx)
            })
            .unwrap();
        visual.simulate_keystrokes("enter");
        visual.simulate_keystrokes("space");
        visual.run_until_parked();
        assert_eq!(seen.lock().unwrap().len(), 3, "{:?}", seen.lock().unwrap());
    }

    #[gpui::test]
    fn nothing_is_asked_while_the_window_is_not_active(cx: &mut TestAppContext) {
        let w = fixtures::world();
        quiet(&w);
        let window = open_chip(&w, cx, false);
        // Minutes pass in the background: the timer ticks, nobody calls gh or sbx.
        cx.executor().advance_clock(REFRESH_EVERY * 5);
        cx.run_until_parked();
        assert_eq!(listed(&w), 0);
        assert!(w.sbx.verbs().is_empty());
        assert!(!status_of(&window, cx).visible());

        // The window comes back: it reads once, at once.
        window
            .update(cx, |chip, _, cx| chip.set_window_active(true, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(listed(&w), 1);
        assert!(status_of(&window, cx).visible());

        // Losing and regaining focus within the minute does not ask again.
        for active in [false, true] {
            w.api.cloud().forget_workspace_views();
            window
                .update(cx, |chip, _, cx| chip.set_window_active(active, cx))
                .unwrap();
            cx.run_until_parked();
        }
        assert_eq!(listed(&w), 1, "at most one ask per minute");

        // The next minute asks again (the cache is forgotten, so it is a real read)...
        w.api.cloud().forget_workspace_views();
        cx.executor().advance_clock(REFRESH_EVERY);
        cx.run_until_parked();
        assert_eq!(listed(&w), 2);

        // ...but not while unfocused.
        window
            .update(cx, |chip, _, cx| chip.set_window_active(false, cx))
            .unwrap();
        w.api.cloud().forget_workspace_views();
        cx.executor().advance_clock(REFRESH_EVERY * 3);
        cx.run_until_parked();
        assert_eq!(listed(&w), 2);
    }

    #[gpui::test]
    fn a_failing_gh_and_sbx_leave_the_chip_hidden_and_say_nothing(cx: &mut TestAppContext) {
        let w = fixtures::world();
        *w.gh.auth.lock().unwrap() = Err("gh is not installed".into());
        *w.sbx.ls_reply.lock().unwrap() = Some(Err("sbx is not installed".into()));
        let window = open_chip(&w, cx, true);
        let status = status_of(&window, cx);
        assert!(!status.visible() && !status.warns(), "{status:?}");
        assert!(status.entries.is_empty());
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("cloud-status-chip").is_none());
    }

    #[gpui::test]
    fn a_project_that_does_not_use_cloud_never_starts_gh_or_sbx(cx: &mut TestAppContext) {
        // A checkout with a GitHub origin, but no cloud session and no saved cloud settings.
        let store = fixtures::memory_store();
        let plain = fixtures::Checkout::new();
        let gh = fixtures::Gh::new(json!([]));
        let sbx = fixtures::Sbx::new(vec![]);
        let api = fixtures::api(&store, &gh, &sbx);
        cx.update(super::super::bind);
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(CloudChip::new))
                .unwrap()
        });
        let env = Env {
            api: Some(api),
            store: store.clone(),
        };
        let project = plain.path();
        window
            .update(cx, |chip, _, cx| {
                chip.set_env(env, cx);
                chip.set_project(project, cx);
                chip.set_window_active(true, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(REFRESH_EVERY * 3);
        cx.run_until_parked();
        assert!(gh.calls.lock().unwrap().is_empty());
        assert!(sbx.calls.lock().unwrap().is_empty());
        assert!(!status_of(&window, cx).visible());
    }

    #[gpui::test]
    fn without_a_service_in_a_test_the_chip_asks_nobody(cx: &mut TestAppContext) {
        let store = fixtures::memory_store();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(CloudChip::new))
                .unwrap()
        });
        window
            .update(cx, |chip, _, cx| {
                chip.set_env(Env::system(store), cx);
                chip.set_project("/work/fake".into(), cx);
                chip.set_window_active(true, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(!status_of(&window, cx).visible());
    }
}
