//! Contributors plumbing: the app view, its entries, the footer sync chip.
use super::*;
use crate::contributors::{ContributorsPage, Tab};
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use monocode_core::contributors::jobs::{JobState, SyncEvent};
use monocode_core::contributors::sync::SyncStep;

fn event(root: &str, percent: u8) -> SyncEvent {
    SyncEvent {
        repo_key: format!("{root}/.git"),
        project: root.rsplit('/').next().unwrap().into(),
        root: root.into(),
        branch: "main".into(),
        from: 1,
        to: 2,
        step: SyncStep::Commits,
        done: 3200,
        total: 12000,
        percent,
        state: JobState::Running,
        error: None,
    }
}

/// `classic` + `rail`: full rail, compact rail; `!classic`: the new layout.
fn setup(cx: &mut TestAppContext, classic: bool, rail: bool) -> (WindowHandle<Shell>, i64) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (window, _, session) = super::tests::window(cx);
    cx.simulate_window_resize(window.into(), gpui::size(px(1500.), px(900.)));
    window
        .update(cx, |shell, window, cx| {
            shell.workspace.classic_layout = classic;
            shell.workspace.sidebar_visible = true;
            shell.workspace.project_rail_open = rail;
            shell.workspace.right_panel_open = false;
            shell.open(session, window, cx);
            shell.layout_changed(cx);
        })
        .unwrap();
    cx.run_until_parked();
    (window, session)
}

const LAYOUTS: [(&str, bool, bool, &str); 3] = [
    ("new layout", false, true, "sidebar-navigation-Contributors"),
    ("full rail", true, true, "sidebar-navigation-Contributors"),
    ("compact rail", true, false, "rail-contributors"),
];

fn feed(window: WindowHandle<Shell>, visual: &mut VisualTestContext, event: SyncEvent) {
    window
        .update(visual, |shell, _, cx| {
            shell
                .sync_jobs
                .update(cx, |jobs, cx| jobs.apply_sync(event, cx))
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn page(window: WindowHandle<Shell>, visual: &mut VisualTestContext) -> (Tab, Option<String>) {
    window
        .read_with(visual, |shell, cx| {
            let page: &ContributorsPage = shell.contributors.as_ref().unwrap().read(cx);
            (page.tab(), page.project().map(String::from))
        })
        .unwrap()
}

fn open_page(window: WindowHandle<Shell>, visual: &mut VisualTestContext) {
    window
        .update(visual, |shell, w, cx| {
            shell.open_contributors(None, None, w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
}

/// Opens the core (in memory) and leaves the page again, so events fed after
/// this are not dropped by the reconcile that follows the core opening.
fn ready(window: WindowHandle<Shell>, visual: &mut VisualTestContext) {
    open_page(window, visual);
    window
        .update(visual, |shell, w, cx| shell.close_app_view(w, cx))
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn open(window: WindowHandle<Shell>, visual: &mut VisualTestContext) -> bool {
    window
        .read_with(visual, |shell, _| {
            shell.active_app_view == Some(app_views::AppView::Contributors)
        })
        .unwrap()
}

// SidebarContributors.test.ts: the entry is labelled Contributors and opens the page.
#[gpui::test]
fn the_sidebar_entry_is_labelled_contributors_in_every_layout(cx: &mut TestAppContext) {
    for (name, classic, rail, selector) in LAYOUTS {
        let (window, _) = setup(cx, classic, rail);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(visual.debug_bounds(selector).is_some(), "{name}");
        assert!(
            visual
                .debug_bounds("sidebar-navigation-Ticket Contributors")
                .is_none(),
            "{name}"
        );
        assert_eq!(app_views::AppView::Contributors.label(), "Contributors");
        assert_eq!(app_views::AppView::Contributors.icon(), "chart-square");
    }
}

#[gpui::test]
fn the_sidebar_entry_opens_the_page_on_the_ticket_tab(cx: &mut TestAppContext) {
    for (name, classic, rail, selector) in LAYOUTS {
        let (window, _) = setup(cx, classic, rail);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let entry = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(entry.center(), Default::default());
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(open(window, visual), "{name}");
        assert_eq!(page(window, visual), (Tab::Ticket, None), "{name}");
        assert!(visual.debug_bounds("contributors-page").is_some(), "{name}");
    }
}

#[gpui::test]
fn the_visibility_toggle_hides_the_entry_and_is_saved(cx: &mut TestAppContext) {
    for (name, classic, rail, selector) in LAYOUTS {
        let (window, _) = setup(cx, classic, rail);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let items = window
            .read_with(visual, |shell, _| shell.sidebar_options_menu_items())
            .unwrap();
        assert!(items.iter().any(|item| matches!(item,
            MenuItem::Action(a) if a.id == "sidebar-contributors" && a.label == "Contributors")));
        window
            .update(visual, |shell, _, cx| {
                shell.change_sidebar_option("sidebar-contributors", cx)
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds(selector).is_none(), "{name}");
        window
            .update(visual, |shell, _, _| {
                let saved = shell.latest_state.lock().unwrap().clone();
                let restored = Workspace::restore(Some(&saved), &[], "/work/monocode");
                assert!(!restored.sidebar_contributors_visible);
            })
            .unwrap();
        window
            .update(visual, |shell, _, cx| {
                shell.change_sidebar_option("sidebar-contributors", cx)
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds(selector).is_some(), "{name}");
    }
}

#[gpui::test]
fn the_entry_is_shown_by_default(cx: &mut TestAppContext) {
    assert!(Workspace::default().sidebar_contributors_visible);
    assert!(Workspace::restore(None, &[], "/work").sidebar_contributors_visible);
    let _ = cx;
}

// ProjectContributorsMenu.test.ts: the page opens from its own entry, never from the project menu.
#[gpui::test]
fn the_project_menu_has_no_contributors_item(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, true, true);
    window
        .update(cx, |shell, w, cx| {
            for (path, rail) in [
                ("/work/monocode", true),
                ("/work/monocode", false),
                ("remote://box/work/monocode", true),
            ] {
                let target = MenuTarget::Project {
                    path: path.into(),
                    rail,
                };
                let (_, items, _, _) = shell.menu_items(&target, &mut Vec::new(), w, cx);
                let labels: Vec<_> = items
                    .into_iter()
                    .filter_map(|item| match item {
                        MenuItem::Action(a) => Some(a.label.to_string()),
                        _ => None,
                    })
                    .collect();
                assert!(
                    labels.iter().all(|l| !l.starts_with("Contributors")),
                    "{labels:?}"
                );
                assert!(labels.iter().any(|l| l == "Background image"), "{labels:?}");
                assert!(labels.iter().any(|l| l == "Archive"), "{labels:?}");
            }
        })
        .unwrap();
}

#[gpui::test]
fn a_clickup_problem_on_the_ticket_tab_opens_settings_on_inbox(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    assert!(open(window, visual));
    let page = window
        .read_with(visual, |shell, _| shell.contributors.clone().unwrap())
        .unwrap();
    page.update(visual, |_, cx| {
        cx.emit(crate::contributors::Event::OpenSettings)
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    window
        .read_with(visual, |shell, cx| {
            assert_eq!(
                shell.active_app_view,
                Some(app_views::AppView::Contributors),
                "the page stays under Settings"
            );
            let settings = shell.settings.as_ref().expect("Settings is open");
            assert_eq!(
                settings.read(cx).section,
                crate::settings::Section::Inbox,
                "on the Inbox section, where ClickUp is connected"
            );
        })
        .unwrap();
    assert!(!ticket_mounted(window, visual), "hidden by Settings");
    window
        .update(visual, |shell, w, cx| shell.close_settings(w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(open(window, visual), "closing Settings returns to the page");
    assert!(
        ticket_mounted(window, visual),
        "the Ticket tab is mounted again"
    );
}

fn ticket_mounted(window: WindowHandle<Shell>, visual: &mut VisualTestContext) -> bool {
    window
        .read_with(visual, |shell, cx| {
            shell
                .contributors
                .as_ref()
                .unwrap()
                .read(cx)
                .ticket_mounted(cx)
        })
        .unwrap()
}

#[gpui::test]
fn leaving_the_page_unmounts_the_ticket_tab_and_opening_it_mounts_it(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    assert!(ticket_mounted(window, visual));
    window
        .update(visual, |shell, w, cx| shell.close_app_view(w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(
        !ticket_mounted(window, visual),
        "a closed page listens to nothing"
    );
    open_page(window, visual);
    assert!(ticket_mounted(window, visual));
}

#[gpui::test]
fn settings_over_the_page_unmount_the_ticket_tab_until_they_close(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    window
        .update(visual, |shell, w, cx| {
            shell.open_settings(crate::settings::Section::Inbox, w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(open(window, visual), "the page is underneath");
    assert!(!ticket_mounted(window, visual), "but hidden by Settings");
    window
        .update(visual, |shell, w, cx| shell.close_settings(w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(ticket_mounted(window, visual), "back with the page");
}

#[gpui::test]
fn open_contributors_with_a_tab_and_project_shows_that_project(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, w, cx| {
            shell.open_contributors(Some(Tab::Github), Some("/work/alpha".into()), w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(open(window, visual));
    assert_eq!(
        page(window, visual),
        (Tab::Github, Some("/work/alpha".into()))
    );
    // It never changes the current project.
    window
        .read_with(visual, |shell, _| {
            assert_eq!(shell.workspace.project, "/work/monocode")
        })
        .unwrap();
}

#[gpui::test]
fn reopening_after_leaving_always_starts_on_ticket_with_no_project(cx: &mut TestAppContext) {
    let (window, session) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, w, cx| {
            shell.open_contributors(Some(Tab::Github), Some("/work/alpha".into()), w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    // Already open: the sidebar entry changes nothing.
    open_page(window, visual);
    assert_eq!(
        page(window, visual),
        (Tab::Github, Some("/work/alpha".into()))
    );
    // Closed and opened again: Ticket, no chosen project (Tauri sets the view to null).
    window
        .update(visual, |shell, w, cx| shell.close_app_view(w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(!open(window, visual));
    open_page(window, visual);
    assert_eq!(page(window, visual), (Tab::Ticket, None));
    // Leaving to a session and coming back through the title tab is the same.
    window
        .update(visual, |shell, w, cx| {
            shell.open_contributors(Some(Tab::Github), Some("/work/alpha".into()), w, cx);
            shell.open(session, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(!open(window, visual));
    window
        .update(visual, |shell, w, cx| {
            shell.open_title_tab(&TitleTab::App(app_views::AppView::Contributors), w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(page(window, visual), (Tab::Ticket, None));
}

#[gpui::test]
fn escape_closes_the_page(cx: &mut TestAppContext) {
    cx.update(super::bind_keys);
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    assert!(open(window, visual));
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(!open(window, visual));
}

#[gpui::test]
fn it_is_a_title_tab_that_closes_like_the_other_app_views(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    assert!(visual.debug_bounds("app-title-tab-Contributors").is_some());
    window
        .read_with(visual, |shell, cx| {
            let tab = TitleTab::App(app_views::AppView::Contributors);
            assert!(shell.visible_tabs(cx).contains(&tab));
            assert_eq!(shell.active_title_tab(cx), Some(tab));
        })
        .unwrap();
    let close = visual.debug_bounds("close-app-tab-Contributors").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    assert!(!open(window, visual));
}

// Tauri 78697559, 3a766f37, 928aa891: leaving through any of these closes the page.
#[gpui::test]
fn opening_a_session_closes_the_page(cx: &mut TestAppContext) {
    let (window, session) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    window
        .update(visual, |shell, w, cx| shell.open(session, w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(!open(window, visual));
}

#[gpui::test]
fn switching_project_closes_the_page(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    window
        .update(visual, |shell, w, cx| {
            shell.select_project("/work/another-project".into(), w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(!open(window, visual));
}

#[gpui::test]
fn picking_a_sidebar_tool_tab_closes_the_page(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, true, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    window
        .update(visual, |shell, w, cx| {
            shell.select_classic_sidebar_tab(classic_sidebar::ClassicSidebarTab::Files, w, cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert!(!open(window, visual));
}

#[gpui::test]
fn opening_a_pane_file_closes_the_page(cx: &mut TestAppContext) {
    let (window, session) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    window
        .update(visual, |shell, w, cx| shell.open_file_pane(session, w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(!open(window, visual));
}

#[gpui::test]
fn a_terminal_cannot_open_underneath_the_page(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    window
        .update(visual, |shell, w, cx| shell.new_terminal(w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(open(window, visual), "the page stays open");
    window
        .read_with(visual, |shell, _| {
            assert!(
                shell
                    .workspace
                    .docks
                    .get(&shell.workspace.project)
                    .is_none_or(|d| d.terminals.is_empty())
            );
        })
        .unwrap();
}

// UsageFooterSync.test.ts
#[gpui::test]
fn the_chip_renders_nothing_while_no_sync_runs(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("contributors-sync").is_none());
    assert!(visual.debug_bounds("footer-terminal").is_some());
}

#[gpui::test]
fn the_chip_shows_the_percent_and_the_tooltip_of_a_running_job(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    feed(window, visual, event("/work/alpha", 42));
    assert!(visual.debug_bounds("contributors-sync").is_some());
    let chip = window
        .read_with(visual, |shell, cx| shell.sync_chip_state(cx))
        .unwrap()
        .unwrap();
    assert_eq!(chip.label, "42%");
    assert_eq!(chip.aria, "Syncing contributors 42%");
    assert_eq!(
        chip.title,
        "Syncing contributors · alpha · Commits 3,200/12,000"
    );
    assert!(!chip.failed);
}

#[gpui::test]
fn the_chip_sits_left_of_the_terminal_button(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    feed(window, visual, event("/work/alpha", 42));
    let chip = visual.debug_bounds("contributors-sync").unwrap();
    let terminal = visual.debug_bounds("footer-terminal").unwrap();
    assert!(chip.right() <= terminal.left());
}

#[gpui::test]
fn the_chip_shows_while_an_app_view_is_open(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, w, cx| shell.open_notes(w, cx))
        .unwrap();
    visual.run_until_parked();
    assert!(visual.debug_bounds("footer-terminal").is_none());
    feed(window, visual, event("/work/alpha", 42));
    assert!(visual.debug_bounds("contributors-sync").is_some());
}

// The original layout hides the whole footer while an application view is
// open, as the Tauri app does, so there is no chip to show there.
#[gpui::test]
fn the_original_layout_has_no_footer_while_an_app_view_is_open(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, true, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    feed(window, visual, event("/work/alpha", 42));
    assert!(visual.debug_bounds("contributors-sync").is_some());
    window
        .update(visual, |shell, w, cx| shell.open_notes(w, cx))
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("contributors-sync").is_none());
}

#[gpui::test]
fn two_jobs_combine_in_the_chip(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    feed(window, visual, event("/work/alpha", 20));
    let mut beta = event("/work/beta", 60);
    beta.step = SyncStep::Github;
    beta.done = 1;
    beta.total = 4;
    feed(window, visual, beta);
    let chip = window
        .read_with(visual, |shell, cx| shell.sync_chip_state(cx))
        .unwrap()
        .unwrap();
    assert_eq!(chip.label, "40%");
    assert_eq!(
        chip.title.split('\n').collect::<Vec<_>>(),
        [
            "Syncing contributors · alpha · Commits 3,200/12,000",
            "Syncing contributors · beta · GitHub 1/4"
        ]
    );
}

#[gpui::test]
fn the_chip_goes_away_when_the_job_is_done(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    feed(window, visual, event("/work/alpha", 42));
    let mut done = event("/work/alpha", 100);
    done.state = JobState::Done;
    feed(window, visual, done);
    assert!(visual.debug_bounds("contributors-sync").is_none());
}

#[gpui::test]
fn clicking_the_chip_opens_the_first_jobs_project_on_the_github_tab(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    ready(window, visual);
    feed(window, visual, event("/work/alpha", 42));
    let mut beta = event("/work/beta", 10);
    beta.from = 5;
    feed(window, visual, beta);
    let chip = visual.debug_bounds("contributors-sync").unwrap();
    visual.simulate_click(chip.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(open(window, visual));
    assert_eq!(
        page(window, visual),
        (Tab::Github, Some("/work/alpha".into()))
    );
    // A running job stays on screen.
    assert!(visual.debug_bounds("contributors-sync").is_some());
}

#[gpui::test]
fn a_failure_turns_the_chip_red_and_a_click_opens_the_page_and_dismisses_it(
    cx: &mut TestAppContext,
) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    feed(window, visual, event("/work/alpha", 42));
    let mut failed = event("/work/alpha", 42);
    failed.state = JobState::Error;
    failed.error = Some("git exploded".into());
    feed(window, visual, failed);
    let chip = window
        .read_with(visual, |shell, cx| shell.sync_chip_state(cx))
        .unwrap()
        .unwrap();
    assert!(chip.failed);
    assert_eq!(chip.aria, "Contributors sync stopped");
    assert_eq!(chip.label, "");
    assert_eq!(
        chip.title,
        "Contributors sync stopped · alpha · git exploded"
    );
    let bounds = visual.debug_bounds("contributors-sync").unwrap();
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(
        page(window, visual),
        (Tab::Github, Some("/work/alpha".into()))
    );
    assert!(visual.debug_bounds("contributors-sync").is_none());
    window
        .read_with(visual, |shell, cx| {
            assert!(shell.sync_jobs.read(cx).jobs().is_empty())
        })
        .unwrap();
}

#[gpui::test]
fn a_listed_job_shows_and_an_unlisted_one_does_not(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |shell, _, cx| {
            shell.sync_jobs.update(cx, |jobs, cx| {
                let ticket = jobs.begin_reconcile();
                jobs.finish_reconcile(ticket, &[event("/work/alpha", 77)], cx);
            })
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    let chip = window
        .read_with(visual, |shell, cx| shell.sync_chip_state(cx))
        .unwrap()
        .unwrap();
    assert_eq!(chip.label, "77%");
    window
        .update(visual, |shell, _, cx| {
            shell.sync_jobs.update(cx, |jobs, cx| {
                let ticket = jobs.begin_reconcile();
                jobs.finish_reconcile(ticket, &[], cx);
            })
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("contributors-sync").is_none());
}

#[gpui::test]
fn events_from_the_core_sinks_reach_the_chip_and_the_core_opens_in_memory(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_page(window, visual);
    window
        .read_with(visual, |shell, cx| {
            let page = shell.contributors.as_ref().unwrap().read(cx);
            assert!(page.core().is_some(), "the core opened for the page");
            assert!(shell.contributors_core.is_some());
        })
        .unwrap();
    let sink = window
        .read_with(visual, |shell, cx| shell.sync_jobs.read(cx).sync_sink())
        .unwrap();
    sink.emit(&event("/work/alpha", 55));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("contributors-sync").is_some());
}

#[gpui::test]
fn a_job_the_open_core_does_not_run_is_dropped_by_the_reconcile(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    ready(window, visual);
    feed(window, visual, event("/work/alpha", 42));
    assert!(visual.debug_bounds("contributors-sync").is_some());
    // What window activation runs: the core (in memory, idle) lists nothing.
    window
        .update(visual, |shell, _, cx| shell.reconcile_sync_jobs(cx))
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("contributors-sync").is_none());
}

#[gpui::test]
fn opening_the_page_again_reuses_the_core(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    ready(window, visual);
    let first = window
        .read_with(visual, |shell, _| shell.contributors_core.clone().unwrap())
        .unwrap();
    open_page(window, visual);
    window
        .read_with(visual, |shell, cx| {
            let core = shell.contributors_core.as_ref().unwrap();
            assert!(std::sync::Arc::ptr_eq(&first, core));
            let page = shell.contributors.as_ref().unwrap().read(cx);
            assert!(std::sync::Arc::ptr_eq(&first, page.core().unwrap()));
            assert!(page.core_error().is_none());
        })
        .unwrap();
}

#[gpui::test]
fn an_in_memory_store_gets_a_private_data_folder_that_goes_with_the_core(cx: &mut TestAppContext) {
    let (window, _) = setup(cx, false, true);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    ready(window, visual);
    let dir = window
        .read_with(visual, |shell, _| {
            shell
                .contributors_scratch
                .as_ref()
                .expect("a private folder")
                .path()
                .to_path_buf()
        })
        .unwrap();
    assert!(dir.is_dir());
    assert_ne!(dir, std::env::temp_dir());
    assert!(dir.starts_with(std::env::temp_dir()));
    window
        .update(visual, |shell, _, _| shell.contributors_scratch = None)
        .unwrap();
    assert!(!dir.exists());
}

// ---- Ask Agent: starting the session

mod ask {
    use super::*;
    use crate::shell::surface_tests::{NoClis, all_terminal, assert_chat, known, since};
    use monocode_core::contributors::model::{ask::AskAgentRequest, snapshot::SnapshotScope};
    use monocode_core::provider::Kind;
    use monocode_core::transcript::attachments;
    use std::{cell::RefCell, rc::Rc};

    const SNAPSHOT: &str = "{\n  \"focus\": \"team\",\n  \"caf\u{e9}\": 1\n}";
    const PROMPT: &str = "Look at the attached snapshot.\n\nWho drove this?";

    fn request(harness: Option<Kind>, file_name: &str) -> AskAgentRequest {
        AskAgentRequest {
            question: "Who drove this?".into(),
            prompt: PROMPT.into(),
            snapshot_json: SNAPSHOT.into(),
            file_name: file_name.into(),
            harness,
            scope: SnapshotScope::Team,
        }
    }

    fn project() -> String {
        let dir = std::env::temp_dir().join(format!(
            "monocode-contributors-ask-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    type Outcome = Rc<RefCell<Option<Result<(), String>>>>;

    /// Starts a session in `window` and returns the sessions known before and
    /// what the shell reported.
    fn run(
        cx: &mut TestAppContext,
        window: WindowHandle<Shell>,
        project: &str,
        request: AskAgentRequest,
    ) -> (std::collections::HashSet<i64>, Outcome) {
        let outcome: Outcome = Rc::default();
        let report = outcome.clone();
        let before = window
            .update(cx, |shell, _, cx| {
                let before = known(shell);
                shell.start_contributors_session(
                    project.to_string(),
                    request,
                    cx,
                    move |_, result, _| *report.borrow_mut() = Some(result),
                );
                before
            })
            .unwrap();
        cx.run_until_parked();
        (before, outcome)
    }

    fn start(
        cx: &mut TestAppContext,
        project: &str,
        request: AskAgentRequest,
        prepare: impl FnOnce(&mut Shell),
    ) -> (WindowHandle<Shell>, std::collections::HashSet<i64>, Outcome) {
        let (window, _, _) = super::super::tests::window(cx);
        window.update(cx, |shell, _, _| prepare(shell)).unwrap();
        let (before, outcome) = run(cx, window, project, request);
        (window, before, outcome)
    }

    /// The text of the prompt row and the attached paths of a session's transcript.
    fn transcript(shell: &Shell, id: i64, cx: &gpui::App) -> (String, Vec<std::path::PathBuf>) {
        let rows = shell.sessions[&id]
            .read(cx)
            .transcript
            .read(cx)
            .source_rows();
        let prompt = rows
            .iter()
            .filter(|row| row.is_prompt())
            .map(|row| row.text.as_str())
            .collect();
        let paths = rows
            .iter()
            .filter(|row| row.role == attachments::ROLE)
            .flat_map(|row| attachments::paths(row).unwrap())
            .collect();
        (prompt, paths)
    }

    #[gpui::test]
    fn a_start_makes_a_chat_with_the_snapshot_attached_and_opens_it(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        let project = project();
        let (window, before, outcome) = start(
            cx,
            &project,
            request(Some(Kind::Claude), "contributors-2026-10.json"),
            all_terminal,
        );
        assert_eq!(*outcome.borrow(), Some(Ok(())));
        window
            .update(cx, |shell, _, cx| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                let id = ids[0];
                // A chat whatever the surface preference: the app submits the prompt itself.
                assert_chat(shell, id);
                let summary = &shell.session_summaries[&id];
                assert_eq!(summary.title, "Contributors 2026-10");
                assert_eq!(summary.provider, "claude");
                assert_eq!(summary.cwd, project);
                let (prompt, paths) = transcript(shell, id, cx);
                assert_eq!(
                    prompt, PROMPT,
                    "the attached file is not named in the prompt"
                );
                assert_eq!(paths.len(), 1);
                assert!(
                    paths[0]
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .ends_with("-contributors-2026-10.json")
                );
                assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), SNAPSHOT);
                // The session is open and the page is gone, as in the React app.
                assert_eq!(shell.workspace.active, Some(id));
                assert_eq!(shell.workspace.project, project);
                assert_eq!(shell.active_app_view, None);
                let _ = std::fs::remove_file(&paths[0]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_provider_that_cannot_take_attachments_gets_the_path_in_the_prompt(
        cx: &mut TestAppContext,
    ) {
        let _clis = NoClis::new();
        let (window, before, outcome) = start(
            cx,
            &project(),
            request(Some(Kind::Fx), "contributors-all.json"),
            |_| {},
        );
        assert_eq!(*outcome.borrow(), Some(Ok(())));
        window
            .update(cx, |shell, _, cx| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_eq!(shell.session_summaries[&ids[0]].title, "Contributors all time");
                let (prompt, paths) = transcript(shell, ids[0], cx);
                assert!(paths.is_empty(), "nothing is attached");
                let (head, path) = prompt
                    .split_once("\n\nThe snapshot could not be attached to this provider. Read it from this file: ")
                    .expect("the fallback sentence");
                assert_eq!(head, PROMPT);
                assert_eq!(std::fs::read_to_string(path).unwrap(), SNAPSHOT);
                let _ = std::fs::remove_file(path);
            })
            .unwrap();
    }

    #[gpui::test]
    fn without_a_chosen_provider_the_project_default_is_used(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        let project = project();
        let (window, before, _) = start(
            cx,
            &project,
            request(None, "contributors-2026-10.json"),
            |shell| {
                shell.workspace.preferences.default_provider = Some("codex".into());
                shell
                    .workspace
                    .preferences
                    .project_providers
                    .entry(project.clone())
                    .or_default()
                    .provider = Some("claude".into());
            },
        );
        window
            .update(cx, |shell, _, cx| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_eq!(shell.session_summaries[&ids[0]].provider, "claude");
                let (_, paths) = transcript(shell, ids[0], cx);
                for path in paths {
                    let _ = std::fs::remove_file(path);
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_remote_or_missing_project_is_refused_before_anything_is_written(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        for project in ["", "remote://box/work/monocode"] {
            let (window, before, outcome) = start(
                cx,
                project,
                request(Some(Kind::Claude), "contributors-2026-10.json"),
                |_| {},
            );
            assert_eq!(
                *outcome.borrow(),
                Some(Err("Contributors works on local projects only.".to_string())),
                "{project:?}"
            );
            window
                .update(cx, |shell, _, _| assert!(since(shell, &before).is_empty()))
                .unwrap();
        }
    }

    #[gpui::test]
    fn a_snapshot_that_cannot_be_written_leaves_no_session(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        let (window, _, _) = super::super::tests::window(cx);
        let project = project();
        let blocker = std::env::temp_dir().join(format!(
            "monocode-contributors-ask-file-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::write(&blocker, "not a folder").unwrap();
        // The attachment folder is `$TMPDIR/monocode-attachments`: under a file it cannot exist.
        let tmp = TmpDir::set(&blocker);
        let (before, outcome) = run(
            cx,
            window,
            &project,
            request(Some(Kind::Claude), "contributors-2026-10.json"),
        );
        drop(tmp);
        let _ = std::fs::remove_file(blocker);
        let error = outcome.borrow().clone().unwrap().unwrap_err();
        assert!(
            error.starts_with("Could not write the snapshot: "),
            "{error}"
        );
        window
            .update(cx, |shell, _, _| assert!(since(shell, &before).is_empty()))
            .unwrap();
    }

    #[gpui::test]
    fn the_page_asking_to_start_reaches_the_shell(cx: &mut TestAppContext) {
        let _clis = NoClis::new();
        let (window, _) = setup(cx, false, true);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        open_page(window, visual);
        let project = project();
        let before = window.read_with(visual, |shell, _| known(shell)).unwrap();
        let page = window
            .read_with(visual, |shell, _| shell.contributors.clone().unwrap())
            .unwrap();
        page.update(visual, |_, cx| {
            cx.emit(crate::contributors::Event::StartSession {
                project: project.clone(),
                request: Box::new(request(Some(Kind::Claude), "contributors-2026-10.json")),
                start: 1,
            })
        });
        visual.run_until_parked();
        window
            .update(visual, |shell, _, cx| {
                let ids = since(shell, &before);
                assert_eq!(ids.len(), 1);
                assert_eq!(
                    shell.session_summaries[&ids[0]].title,
                    "Contributors 2026-10"
                );
                assert_eq!(shell.active_app_view, None, "the page is left");
                let (_, paths) = transcript(shell, ids[0], cx);
                for path in paths {
                    let _ = std::fs::remove_file(path);
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_dialog_offers_the_providers_the_picker_shows_with_the_project_default(
        cx: &mut TestAppContext,
    ) {
        let (window, _, _) = super::super::tests::window(cx);
        window
            .update(cx, |shell, _, _| {
                let prefs = &mut shell.workspace.preferences;
                prefs.default_provider = Some("codex".into());
                prefs.hidden_providers = vec!["grok".into()];
                let project = prefs
                    .project_providers
                    .entry("/work/alpha".into())
                    .or_default();
                project.provider = Some("claude".into());
                project.hidden = vec!["cursor".into()];
                // Not probed yet: nothing is dropped for being missing.
                shell.installed_providers = None;
                let all = shell.ask_choices("/work/alpha");
                assert!(all.providers.contains(&Kind::Claude));
                assert!(all.providers.contains(&Kind::Codex));
                assert!(
                    !all.providers.contains(&Kind::Cursor),
                    "hidden for the project"
                );
                assert!(!all.providers.contains(&Kind::Grok), "hidden everywhere");
                assert_eq!(all.default, Some(Kind::Claude), "the project's own default");
                // Once probed, only the installed CLIs are offered.
                shell.installed_providers = Some(vec![Kind::Codex, Kind::Cursor]);
                assert_eq!(shell.ask_choices("/work/alpha").providers, [Kind::Codex]);
                let elsewhere = shell.ask_choices("/work/beta");
                assert_eq!(elsewhere.providers, [Kind::Codex, Kind::Cursor]);
                assert_eq!(elsewhere.default, Some(Kind::Codex));
            })
            .unwrap();
    }

    /// Points `TMPDIR` somewhere else for a test (single test thread).
    struct TmpDir(Option<std::ffi::OsString>);
    impl TmpDir {
        fn set(path: &std::path::Path) -> Self {
            let old = std::env::var_os("TMPDIR");
            // SAFETY: gpui tests run with one test thread (`--test-threads=1`).
            unsafe { std::env::set_var("TMPDIR", path) };
            Self(old)
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            // SAFETY: as in `set`.
            unsafe {
                match self.0.take() {
                    Some(old) => std::env::set_var("TMPDIR", old),
                    None => std::env::remove_var("TMPDIR"),
                }
            }
        }
    }
}
