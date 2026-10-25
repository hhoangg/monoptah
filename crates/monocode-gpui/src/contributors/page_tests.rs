//! The page and the GitHub tab as the user sees them: ContributorsPage.test.ts
//! and ContributorsView.test.ts for the parts the shell owns (toolbar,
//! alerts, panels, project select, Escape). The rules behind them are tested
//! in `data_tests`; the fake backend keeps git, `gh` and the preferences out.
use super::{
    ContributorsPage, Event, GithubEvent, GithubView, Tab,
    backend::Backend,
    data::{Clock, Snapshot},
    data_tests::{Call, Fake, month_back, raw, start_of_tests, synced, unsynced, window_of},
    github::Section,
    jobs::SyncJobs,
    project_options, resolve_project,
};
use gpui::{
    AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext, WindowHandle,
    WindowOptions,
};
use monocode_core::contributors::{
    GitIdentity,
    jobs::{JobState, SyncEvent},
    model::{
        period::{PeriodKind, current_period},
        types::Person,
    },
    sync::SyncStep,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

pub(super) struct Ui {
    pub(super) window: WindowHandle<ContributorsPage>,
    pub(super) fake: Arc<Fake>,
    jobs: Entity<SyncJobs>,
    closed: Rc<Cell<u32>>,
    _subscription: gpui::Subscription,
}

const PROJECTS: [&str; 2] = ["/work/alpha", "/work/beta"];

pub(super) fn open(
    cx: &mut TestAppContext,
    fake: Arc<Fake>,
    projects: &[&str],
    active: &str,
    tab: Tab,
) -> (Ui, VisualTestContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let clock: Clock = Rc::new(start_of_tests);
    let jobs = cx.update(|cx| cx.new(SyncJobs::new));
    let window = cx
        .update(|cx| {
            let jobs = jobs.clone();
            cx.open_window(WindowOptions::default(), move |_, cx| {
                cx.new(|cx| ContributorsPage::new(jobs, cx).with_clock(clock, cx))
            })
        })
        .unwrap();
    let closed = Rc::new(Cell::new(0));
    let count = closed.clone();
    let page = window.root(cx).unwrap();
    let subscription = cx.update(|cx| {
        cx.subscribe(&page, move |_, event: &Event, _| match event {
            Event::Close => count.set(count.get() + 1),
            Event::Ask { .. } | Event::StartSession { .. } | Event::OpenSettings => {}
        })
    });
    let backend: Arc<dyn Backend> = fake.clone();
    let projects: Vec<String> = projects.iter().map(|p| p.to_string()).collect();
    let active = active.to_string();
    window
        .update(cx, move |page, _, cx| {
            page.set_context(projects, active, cx);
            page.show(Some(tab), None, cx);
            page.set_backend(backend, cx);
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    settle(&mut visual);
    (
        Ui {
            window,
            fake,
            jobs,
            closed,
            _subscription: subscription,
        },
        visual,
    )
}

pub(super) fn settle(v: &mut VisualTestContext) {
    v.run_until_parked();
    v.refresh().unwrap();
}

pub(super) fn click(v: &mut VisualTestContext, selector: &'static str) {
    let bounds = v
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("nothing drawn as {selector}"));
    v.simulate_click(bounds.center(), Default::default());
    settle(v);
}

pub(super) fn shown(v: &mut VisualTestContext, selector: &'static str) -> bool {
    v.debug_bounds(selector).is_some()
}

impl Ui {
    /// How many times the page asked to be closed.
    pub(super) fn closed_count(&self) -> u32 {
        self.closed.get()
    }

    pub(super) fn view(&self, v: &mut VisualTestContext) -> Entity<GithubView> {
        self.window
            .read_with(v, |page, _| page.current_view().expect("a GitHub view"))
            .unwrap()
    }

    pub(super) fn snap(&self, v: &mut VisualTestContext) -> Snapshot {
        let view = self.view(v);
        v.read(|cx| view.read(cx).snapshot(cx))
    }

    fn page<R>(
        &self,
        v: &mut VisualTestContext,
        f: impl FnOnce(&mut ContributorsPage, &mut gpui::Context<ContributorsPage>) -> R,
    ) -> R {
        let out = self.window.update(v, |page, _, cx| f(page, cx)).unwrap();
        settle(v);
        out
    }

    fn emit(&self, v: &mut VisualTestContext, event: SyncEvent) {
        self.fake.list(&event);
        self.jobs.update(v, |jobs, cx| jobs.apply_sync(event, cx));
        settle(v);
    }

    fn running(&self, over: impl FnOnce(&mut SyncEvent)) -> SyncEvent {
        let now = start_of_tests();
        let mut event = super::data_tests::event(
            "/work/alpha",
            window_of(&current_period(PeriodKind::Days7, &now, None), &now),
            JobState::Running,
        );
        over(&mut event);
        event
    }
}

pub(super) fn github(cx: &mut TestAppContext, fake: Arc<Fake>) -> (Ui, VisualTestContext) {
    open(cx, fake, &PROJECTS, "/work/alpha", Tab::Github)
}

fn unsynced_fake() -> Arc<Fake> {
    let fake = Fake::new();
    fake.set_load(unsynced());
    fake
}

// ---- ContributorsPage.test.ts -------------------------------------------

#[gpui::test]
fn offers_a_ticket_and_a_github_tab(cx: &mut TestAppContext) {
    let (_, mut v) = open(cx, Fake::new(), &PROJECTS, "/work/alpha", Tab::Ticket);
    assert!(shown(&mut v, "contributors-page-tab-ticket"));
    assert!(shown(&mut v, "contributors-page-tab-github"));
}

#[gpui::test]
fn opens_on_the_ticket_tab_with_no_project_selector(cx: &mut TestAppContext) {
    let (ui, mut v) = open(cx, Fake::new(), &PROJECTS, "/work/alpha", Tab::Ticket);
    assert!(shown(&mut v, "contributors-body-ticket"));
    assert!(!shown(&mut v, "contributors-select-project"));
    // The GitHub view is made when its tab opens, not before.
    let has_view = ui
        .window
        .read_with(&v, |page, _| page.current_view().is_some())
        .unwrap();
    assert!(!has_view);
    assert!(ui.fake.calls().is_empty());
}

#[gpui::test]
fn the_github_tab_shows_the_project_view_and_its_selector(cx: &mut TestAppContext) {
    let (_, mut v) = github(cx, Fake::new());
    assert!(shown(&mut v, "contributors-github"));
    assert!(shown(&mut v, "contributors-select-project"));
    assert!(!shown(&mut v, "contributors-body-ticket"));
}

#[gpui::test]
fn the_segmented_control_switches_tabs_and_the_page_remembers_the_last_one(
    cx: &mut TestAppContext,
) {
    let (ui, mut v) = open(cx, Fake::new(), &PROJECTS, "/work/alpha", Tab::Ticket);
    click(&mut v, "contributors-page-tab-github");
    assert!(shown(&mut v, "contributors-github"));
    assert_eq!(
        ui.window.read_with(&v, |p, _| p.tab()).unwrap(),
        Tab::Github
    );
    click(&mut v, "contributors-page-tab-ticket");
    assert!(shown(&mut v, "contributors-body-ticket"));
    assert_eq!(
        ui.window.read_with(&v, |p, _| p.tab()).unwrap(),
        Tab::Ticket
    );
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_project_defaults_to_the_active_one() {
    let projects = strings(&["/work/alpha", "/work/beta"]);
    assert_eq!(
        resolve_project(None, &projects, "/work/beta").as_deref(),
        Some("/work/beta")
    );
}

#[test]
fn the_project_falls_back_to_the_first_local_one_when_none_is_active() {
    let projects = strings(&["remote://box/work/app", "/work/alpha", "/work/beta"]);
    for active in [
        "",
        "/",
        "~",
        "/Users/me",
        "/home/me/",
        "/Users/me/Foo.app/Contents",
    ] {
        assert_eq!(
            resolve_project(None, &projects, active).as_deref(),
            Some("/work/alpha"),
            "{active:?}"
        );
    }
}

#[test]
fn a_chosen_project_wins_and_no_project_at_all_is_none() {
    let projects = strings(&["/work/alpha"]);
    assert_eq!(
        resolve_project(Some("/elsewhere/x"), &projects, "/work/alpha").as_deref(),
        Some("/elsewhere/x")
    );
    assert_eq!(resolve_project(None, &[], ""), None);
    assert_eq!(
        resolve_project(None, &strings(&["remote://box/work/app"]), ""),
        None
    );
}

#[test]
fn the_selector_lists_every_known_project_in_rail_order() {
    let projects = strings(&["/work/alpha", "/work/beta"]);
    assert_eq!(
        project_options(&projects, Some("/work/alpha")),
        [
            ("/work/alpha".to_string(), "alpha".to_string()),
            ("/work/beta".to_string(), "beta".to_string())
        ]
    );
}

#[test]
fn the_selector_lists_a_remote_project_and_a_chosen_one_the_rail_does_not_have() {
    let projects = strings(&["remote://box/work/app", "/work/alpha"]);
    let options = project_options(&projects, Some("remote://box/work/app"));
    assert_eq!(options[0].1, "app");
    let options = project_options(&projects, Some("/gone/old"));
    assert_eq!(options[0], ("/gone/old".to_string(), "old".to_string()));
    assert_eq!(options.len(), 3);
}

#[test]
fn two_folders_with_the_same_name_are_told_apart_by_their_path() {
    let projects = strings(&["/work/app", "/other/app", "/work/lib"]);
    let options = project_options(&projects, Some("/work/app"));
    assert_eq!(options[0].1, "app (/work/app)");
    assert_eq!(options[1].1, "app (/other/app)");
    assert_eq!(options[2].1, "lib");
}

#[gpui::test]
fn picking_a_project_shows_it_and_it_survives_a_switch_of_tab(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    assert_eq!(
        ui.view(&mut v)
            .read_with(&v, |view, _| view.project().to_string()),
        "/work/alpha"
    );
    click(&mut v, "contributors-select-project");
    assert!(shown(&mut v, "contributors-option-project-1"));
    click(&mut v, "contributors-option-project-1");
    assert_eq!(
        ui.view(&mut v)
            .read_with(&v, |view, _| view.project().to_string()),
        "/work/beta"
    );
    assert_eq!(
        ui.window
            .read_with(&v, |p, _| p.project().map(String::from))
            .unwrap(),
        Some("/work/beta".to_string())
    );
    click(&mut v, "contributors-page-tab-ticket");
    click(&mut v, "contributors-page-tab-github");
    assert_eq!(
        ui.view(&mut v)
            .read_with(&v, |view, _| view.project().to_string()),
        "/work/beta"
    );
}

#[gpui::test]
fn each_project_keeps_its_own_view_and_data(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    let first = ui.view(&mut v);
    ui.page(&mut v, |page, cx| {
        page.set_project(Some("/work/beta".into()), cx)
    });
    let second = ui.view(&mut v);
    assert_ne!(first.entity_id(), second.entity_id());
    ui.page(&mut v, |page, cx| {
        page.set_project(Some("/work/alpha".into()), cx)
    });
    assert_eq!(ui.view(&mut v).entity_id(), first.entity_id());
}

#[gpui::test]
fn says_so_when_the_app_knows_no_project(cx: &mut TestAppContext) {
    let (_, mut v) = open(cx, Fake::new(), &[], "", Tab::Github);
    assert!(shown(&mut v, "contributors-no-projects"));
    assert!(!shown(&mut v, "contributors-github"));
    assert!(!shown(&mut v, "contributors-select-project"));
}

#[gpui::test]
fn a_remote_project_says_contributors_works_on_local_projects_and_reads_nothing(
    cx: &mut TestAppContext,
) {
    let (ui, mut v) = open(
        cx,
        Fake::new(),
        &["remote://box/work/app"],
        "remote://box/work/app",
        Tab::Github,
    );
    assert!(shown(&mut v, "contributors-remote"));
    assert!(!shown(&mut v, "contributors-period-month"));
    assert!(ui.fake.calls().is_empty());
}

#[gpui::test]
fn a_core_that_cannot_open_is_said_on_the_page(cx: &mut TestAppContext) {
    let (ui, mut v) = open(cx, Fake::new(), &PROJECTS, "/work/alpha", Tab::Github);
    ui.page(&mut v, |page, cx| {
        page.set_core(Err("database is locked".into()), cx)
    });
    assert!(shown(&mut v, "contributors-unavailable"));
}

// ---- Escape ---------------------------------------------------------------

fn focus_page(ui: &Ui, v: &mut VisualTestContext) {
    ui.window
        .update(v, |page, window, cx| {
            let focus = page.focus_handle(cx);
            window.focus(&focus, cx);
        })
        .unwrap();
    settle(v);
}

#[gpui::test]
fn a_plain_escape_closes_the_page_on_either_tab(cx: &mut TestAppContext) {
    for tab in [Tab::Ticket, Tab::Github] {
        let (ui, mut v) = open(cx, Fake::new(), &PROJECTS, "/work/alpha", tab);
        focus_page(&ui, &mut v);
        v.simulate_keystrokes("escape");
        settle(&mut v);
        assert_eq!(ui.closed.get(), 1, "{tab:?}");
    }
}

#[gpui::test]
fn escape_closes_the_page_while_the_github_view_has_the_focus(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    let view = ui.view(&mut v);
    ui.window
        .update(&mut v, |_, window, cx| {
            let focus = view.focus_handle(cx);
            window.focus(&focus, cx);
        })
        .unwrap();
    settle(&mut v);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed.get(), 1);
}

#[gpui::test]
fn escape_closes_only_an_open_select_and_the_next_one_closes_the_page(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    click(&mut v, "contributors-select-branch");
    assert!(shown(&mut v, "contributors-option-branch-0"));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-option-branch-0"));
    assert_eq!(ui.closed.get(), 0);
    focus_page(&ui, &mut v);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed.get(), 1);
}

#[gpui::test]
fn escape_closes_only_the_options_menu(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    click(&mut v, "contributors-more");
    assert!(shown(&mut v, "contributors-clear"));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-clear"));
    assert_eq!(ui.closed.get(), 0);
}

// ---- ContributorsView.test.ts: states -----------------------------------

#[gpui::test]
fn shows_a_loading_state_before_the_data_arrives(cx: &mut TestAppContext) {
    let clock: Clock = Rc::new(start_of_tests);
    let fake = Fake::new();
    cx.update(|cx| cx.set_reduce_motion(true));
    let jobs = cx.update(|cx| cx.new(SyncJobs::new));
    let window = cx
        .update(|cx| {
            cx.open_window(WindowOptions::default(), move |_, cx| {
                let jobs = jobs.clone();
                cx.new(|cx| {
                    let mut view = GithubView::new("/work/alpha".into(), jobs, clock, cx);
                    view.hold_backend_calls();
                    view
                })
            })
        })
        .unwrap();
    let backend: Arc<dyn Backend> = fake;
    window
        .update(cx, |view, _, cx| view.set_backend(backend, cx))
        .unwrap();
    let mut v = VisualTestContext::from_window(window.into(), cx);
    settle(&mut v);
    assert!(shown(&mut v, "contributors-loading"));
    window
        .update(&mut v, |view, _, cx| view.release_all(cx))
        .unwrap();
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-loading"));
    assert!(shown(&mut v, "contributors-panel"));
}

#[gpui::test]
fn renders_the_three_sections_with_the_unmapped_count_on_identities(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = synced("h1");
    let mut ci = response.raw.commits[0].clone();
    ci.hash = "c2".into();
    ci.author = GitIdentity {
        name: "CI".into(),
        email: "ci@build-host".into(),
        login: None,
    };
    response.raw.commits.push(ci);
    fake.set_load(response);
    let (ui, mut v) = github(cx, fake);
    for section in ["stats", "leaderboard", "identities"] {
        assert!(
            v.debug_bounds(match section {
                "stats" => "contributors-section-stats",
                "leaderboard" => "contributors-section-leaderboard",
                _ => "contributors-section-identities",
            })
            .is_some(),
            "{section}"
        );
    }
    let unmapped = ui.snap(&mut v).model.unwrap().unmapped.len();
    assert_eq!(unmapped, 1);
}

#[gpui::test]
fn switches_sections_and_shows_the_marked_body_of_each(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    assert!(shown(&mut v, "contributors-body-stats"));
    click(&mut v, "contributors-section-leaderboard");
    assert!(shown(&mut v, "contributors-body-leaderboard"));
    assert!(!shown(&mut v, "contributors-body-stats"));
    click(&mut v, "contributors-section-identities");
    assert!(shown(&mut v, "contributors-body-identities"));
    assert_eq!(
        ui.view(&mut v).read_with(&v, |view, _| view.section()),
        Section::Identities
    );
}

#[gpui::test]
fn shows_an_empty_state_when_there_are_no_commits_in_the_period(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = synced("h1");
    response.raw.commits.clear();
    fake.set_load(response);
    let (_, mut v) = github(cx, fake);
    assert!(shown(&mut v, "contributors-no-commits"));
    assert!(!shown(&mut v, "contributors-body-stats"));
    // The other sections are not replaced by it.
    click(&mut v, "contributors-section-leaderboard");
    assert!(shown(&mut v, "contributors-body-leaderboard"));
    assert!(!shown(&mut v, "contributors-no-commits"));
}

#[gpui::test]
fn shows_the_load_error_with_a_retry_that_reloads(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.load.once.push_back(Err("git exploded".into())));
    let (ui, mut v) = github(cx, fake);
    assert!(shown(&mut v, "contributors-load-error"));
    assert!(!shown(&mut v, "contributors-panel"));
    click(&mut v, "contributors-load-error-action");
    assert!(!shown(&mut v, "contributors-load-error"));
    assert!(shown(&mut v, "contributors-panel"));
    assert_eq!(ui.fake.loads().len(), 2);
}

#[gpui::test]
fn a_refresh_failure_keeps_the_data_and_offers_a_retry(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    ui.fake
        .with(|s| s.load.once.push_back(Err("offline".into())));
    click(&mut v, "contributors-refresh");
    assert!(shown(&mut v, "contributors-refresh-error"));
    assert!(shown(&mut v, "contributors-panel"));
    click(&mut v, "contributors-refresh-error-action");
    assert!(!shown(&mut v, "contributors-refresh-error"));
}

// ---- toolbar ------------------------------------------------------------

#[gpui::test]
fn offers_all_five_periods_with_no_arrows_for_7d(cx: &mut TestAppContext) {
    let (_, mut v) = github(cx, Fake::new());
    for option in ["7d", "month", "quarter", "year", "all"] {
        let selector = match option {
            "7d" => "contributors-period-7d",
            "month" => "contributors-period-month",
            "quarter" => "contributors-period-quarter",
            "year" => "contributors-period-year",
            _ => "contributors-period-all",
        };
        assert!(shown(&mut v, selector), "{option}");
    }
    assert!(!shown(&mut v, "contributors-previous"));
    assert!(!shown(&mut v, "contributors-next"));
}

#[gpui::test]
fn changes_and_remembers_the_kind(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    click(&mut v, "contributors-period-quarter");
    assert_eq!(ui.snap(&mut v).period.kind, PeriodKind::Quarter);
    assert_eq!(
        ui.fake.with(|s| s.saved_range.clone()).as_deref(),
        Some("quarter")
    );
    assert!(shown(&mut v, "contributors-previous"));
    assert!(shown(&mut v, "contributors-next"));
    click(&mut v, "contributors-period-all");
    assert!(!shown(&mut v, "contributors-previous"));
}

#[gpui::test]
fn steps_through_periods_and_next_is_disabled_on_the_current_one(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    click(&mut v, "contributors-period-month");
    click(&mut v, "contributors-next");
    assert_eq!(ui.snap(&mut v).period.id, month_back(0).id);
    click(&mut v, "contributors-previous");
    assert_eq!(ui.snap(&mut v).period.id, month_back(1).id);
    assert_eq!(
        ui.fake.with(|s| s.saved_range.clone()),
        Some(format!("month:{}", month_back(1).id))
    );
    click(&mut v, "contributors-next");
    assert_eq!(ui.snap(&mut v).period.id, month_back(0).id);
}

#[gpui::test]
fn toggles_merge_identities_and_remembers_it(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    assert!(ui.snap(&mut v).merge);
    click(&mut v, "contributors-toggle-merge");
    assert!(!ui.snap(&mut v).merge);
    assert_eq!(
        ui.fake.with(|s| s.saved_merge.clone()).as_deref(),
        Some("false")
    );
    assert!(!ui.snap(&mut v).model.unwrap().merge);
}

#[gpui::test]
fn offers_the_branches_and_collects_the_chosen_one(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    click(&mut v, "contributors-select-branch");
    assert!(shown(&mut v, "contributors-option-branch-0"));
    assert!(shown(&mut v, "contributors-option-branch-1"));
    click(&mut v, "contributors-option-branch-1");
    assert_eq!(ui.snap(&mut v).branch, "dev");
    let Some(Call::Load { branch, .. }) = ui.fake.loads().pop() else {
        panic!("no load");
    };
    assert_eq!(branch.as_deref(), Some("dev"));
}

#[gpui::test]
fn without_a_branch_list_the_select_offers_only_the_branch_shown(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.branches.default = Err("no".into()));
    let (_, mut v) = github(cx, fake);
    click(&mut v, "contributors-select-branch");
    assert!(shown(&mut v, "contributors-option-branch-0"));
    assert!(!shown(&mut v, "contributors-option-branch-1"));
}

#[gpui::test]
fn refresh_reloads_the_synced_data(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    click(&mut v, "contributors-refresh");
    assert_eq!(ui.fake.loads().len(), 2);
}

// ---- Ask Agent ------------------------------------------------------------

#[gpui::test]
fn ask_agent_reports_the_team_scope_once_the_data_is_on_screen(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    let asked = Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = asked.clone();
    let view = ui.view(&mut v);
    let _watch = v.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &GithubEvent, _| {
            let GithubEvent::Ask(scope) = event;
            log.borrow_mut().push(scope.clone());
        })
    });
    click(&mut v, "contributors-ask");
    assert_eq!(
        *asked.borrow(),
        [monocode_core::contributors::model::snapshot::SnapshotScope::Team]
    );
}

#[gpui::test]
fn ask_agent_is_disabled_until_the_data_is_ready_and_never_fires_later(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.load.once.push_back(Err("offline".into())));
    let (ui, mut v) = github(cx, fake);
    let asked = Rc::new(Cell::new(0));
    let count = asked.clone();
    let view = ui.view(&mut v);
    let _watch = v.update(|_, cx| {
        cx.subscribe(&view, move |_, _: &GithubEvent, _| {
            count.set(count.get() + 1)
        })
    });
    // After a failed load there is nothing to ask about.
    click(&mut v, "contributors-ask");
    assert_eq!(asked.get(), 0);
    // Retrying opens nothing as a side effect.
    click(&mut v, "contributors-load-error-action");
    assert_eq!(asked.get(), 0);
}

// ---- unreadable config ----------------------------------------------------

#[gpui::test]
fn an_unreadable_config_shows_a_banner_with_reload_on_every_section_and_blocks_saves(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    fake.with(|s| s.config.default = Ok(Some("{ nope".into())));
    let (ui, mut v) = github(cx, fake);
    for section in [
        "contributors-section-stats",
        "contributors-section-leaderboard",
        "contributors-section-identities",
    ] {
        click(&mut v, section);
        assert!(shown(&mut v, "contributors-locked"), "{section}");
    }
    let view = ui.view(&mut v);
    assert!(v.read(|cx| view.read(cx).locked(cx)));
    // Fix the file and reload from the banner.
    ui.fake.with(|s| s.config.default = Ok(None));
    click(&mut v, "contributors-locked-action");
    assert!(!shown(&mut v, "contributors-locked"));
    assert!(!v.read(|cx| view.read(cx).locked(cx)));
}

#[gpui::test]
fn a_readable_file_shows_no_banner(cx: &mut TestAppContext) {
    let (_, mut v) = github(cx, Fake::new());
    assert!(!shown(&mut v, "contributors-locked"));
}

#[gpui::test]
fn a_failed_save_is_shown_once_at_the_page_level_and_a_good_one_clears_it(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    ui.fake
        .with(|s| s.write.once.push_back(Err("disk full".into())));
    let view = ui.view(&mut v);
    let mut next = (*ui.snap(&mut v).config).clone();
    next.people.push(Person {
        id: "ana".into(),
        name: "Ana".into(),
        identities: vec!["ana@example.com".into()],
    });
    view.update(&mut v, |view, cx| {
        view.save_config_quietly(next.clone(), cx)
    });
    settle(&mut v);
    assert!(shown(&mut v, "contributors-save-error"));
    view.update(&mut v, |view, cx| view.save_config_quietly(next, cx));
    settle(&mut v);
    assert!(!shown(&mut v, "contributors-save-error"));
}

// ---- GitHub notice --------------------------------------------------------

fn with_gh(state: Option<&str>, available: bool, note: Option<&str>) -> Arc<Fake> {
    let fake = Fake::new();
    let mut response = synced("h1");
    response.gh_state = state.map(String::from);
    response.raw.gh_available = available;
    response.raw.gh_note = note.map(String::from);
    fake.set_load(response);
    fake
}

#[gpui::test]
fn shows_the_incomplete_github_notice_and_retries_the_github_step(cx: &mut TestAppContext) {
    let (ui, mut v) = github(
        cx,
        with_gh(Some("partial"), true, Some("GitHub rate limit reached.")),
    );
    assert!(shown(&mut v, "contributors-gh-notice"));
    assert!(shown(&mut v, "contributors-panel"));
    click(&mut v, "contributors-gh-retry");
    assert_eq!(ui.fake.starts()[0].mode, "github");
}

#[gpui::test]
fn shows_a_notice_when_github_is_unavailable(cx: &mut TestAppContext) {
    let (_, mut v) = github(
        cx,
        with_gh(Some("unavailable"), false, Some("gh is not installed.")),
    );
    assert!(shown(&mut v, "contributors-gh-notice"));
}

#[gpui::test]
fn shows_a_collector_note_as_information_when_github_is_available(cx: &mut TestAppContext) {
    let (_, mut v) = github(
        cx,
        with_gh(
            Some("complete"),
            true,
            Some("GitHub returned no pull requests for this period."),
        ),
    );
    assert!(shown(&mut v, "contributors-gh-notice"));
}

#[gpui::test]
fn hides_the_github_notice_when_github_data_is_available(cx: &mut TestAppContext) {
    let (_, mut v) = github(cx, with_gh(Some("complete"), true, None));
    assert!(!shown(&mut v, "contributors-gh-notice"));
    let (_, mut v) = github(cx, with_gh(Some("complete"), true, Some("   ")));
    assert!(!shown(&mut v, "contributors-gh-notice"));
}

#[gpui::test]
fn warns_when_the_history_was_too_large_to_read_in_full(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = synced("h1");
    response.raw.truncated = true;
    fake.set_load(response);
    let (_, mut v) = github(cx, fake);
    assert!(shown(&mut v, "contributors-truncated"));
    let (_, mut v) = github(cx, Fake::new());
    assert!(!shown(&mut v, "contributors-truncated"));
}

// ---- sync panels ----------------------------------------------------------

#[gpui::test]
fn a_never_synced_period_offers_sync_with_the_estimate_and_starts_nothing(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, unsynced_fake());
    assert!(shown(&mut v, "contributors-not-synced"));
    assert!(shown(&mut v, "contributors-estimate"));
    assert!(shown(&mut v, "contributors-sync"));
    assert!(!shown(&mut v, "contributors-section-stats"));
    assert!(ui.fake.starts().is_empty());
}

#[gpui::test]
fn says_why_the_estimate_is_missing_and_still_offers_sync(cx: &mut TestAppContext) {
    let fake = unsynced_fake();
    fake.with(|s| s.estimate.default = Err("git is not responding".into()));
    let (_, mut v) = github(cx, fake);
    assert!(shown(&mut v, "contributors-estimate-error"));
    assert!(!shown(&mut v, "contributors-estimate"));
    assert!(shown(&mut v, "contributors-sync"));
}

#[gpui::test]
fn sync_starts_a_full_job_shows_the_step_and_progress_and_cancel_stops_it(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, unsynced_fake());
    click(&mut v, "contributors-sync");
    assert_eq!(ui.fake.starts()[0].mode, "full");
    // Queued first, then the core's progress.
    assert!(shown(&mut v, "contributors-progress-text"));
    assert!(shown(&mut v, "contributors-cancel"));
    assert!(!shown(&mut v, "contributors-sync"));
    ui.emit(
        &mut v,
        ui.running(|e| {
            e.step = SyncStep::PullRequests;
            e.done = 2;
            e.total = 9;
            e.percent = 30;
        }),
    );
    assert!(shown(&mut v, "contributors-progress"));
    click(&mut v, "contributors-cancel");
    assert_eq!(ui.fake.count(|c| *c == Call::Cancel), 1);
}

#[gpui::test]
fn shows_the_data_when_the_job_is_done(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, unsynced_fake());
    click(&mut v, "contributors-sync");
    ui.fake.set_load(synced("h1"));
    ui.emit(&mut v, ui.running(|e| e.state = JobState::Done));
    assert!(!shown(&mut v, "contributors-not-synced"));
    assert!(shown(&mut v, "contributors-panel"));
    assert!(shown(&mut v, "contributors-synced-bar"));
}

#[gpui::test]
fn a_synced_period_shows_when_it_was_synced_and_a_sync_again_button(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    assert!(shown(&mut v, "contributors-synced-at"));
    click(&mut v, "contributors-sync-again");
    assert_eq!(ui.fake.starts()[0].mode, "full");
}

#[gpui::test]
fn shows_a_spinner_and_keeps_the_data_while_a_top_up_runs(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut response = synced("new2");
    response.coverage_head = Some("old1".into());
    fake.set_load(response);
    let (ui, mut v) = github(cx, fake);
    assert_eq!(ui.fake.starts()[0].mode, "topup");
    assert!(shown(&mut v, "contributors-panel"));
    assert!(shown(&mut v, "contributors-progress-text"));
    assert!(shown(&mut v, "contributors-cancel"));
    assert!(!shown(&mut v, "contributors-sync-again"));
}

#[gpui::test]
fn reports_a_failed_sync_and_lets_the_user_press_sync_again(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, unsynced_fake());
    ui.emit(
        &mut v,
        ui.running(|e| {
            e.state = JobState::Error;
            e.error = Some("git exploded".into());
        }),
    );
    assert!(shown(&mut v, "contributors-job-error"));
    click(&mut v, "contributors-sync");
    assert!(!shown(&mut v, "contributors-job-error"));
    assert_eq!(ui.fake.starts().len(), 1);
}

#[gpui::test]
fn the_sync_error_can_be_dismissed(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, unsynced_fake());
    ui.emit(
        &mut v,
        ui.running(|e| {
            e.state = JobState::Error;
            e.error = Some("git exploded".into());
        }),
    );
    click(&mut v, "contributors-dismiss");
    assert!(!shown(&mut v, "contributors-job-error"));
}

#[gpui::test]
fn an_incomplete_period_never_shows_partial_numbers_and_offers_sync(cx: &mut TestAppContext) {
    let fake = Fake::new();
    let mut partial = unsynced();
    partial.raw = raw("h1", &start_of_tests());
    partial.missing_commits = 12;
    partial.synced_at = Some(start_of_tests().timestamp() - 60);
    partial.coverage_head = Some("abc123".into());
    fake.set_load(partial);
    let (_, mut v) = github(cx, fake);
    assert!(shown(&mut v, "contributors-not-synced"));
    assert!(!shown(&mut v, "contributors-panel"));
    assert!(shown(&mut v, "contributors-sync"));
}

#[gpui::test]
fn keeps_the_stored_numbers_on_screen_while_a_merges_older_commits_are_read(
    cx: &mut TestAppContext,
) {
    let fake = Fake::new();
    let mut response = synced("new2");
    response.covered = false;
    response.missing_commits = 3;
    response.coverage_head = Some("old1".into());
    fake.set_load(response);
    let (ui, mut v) = github(cx, fake);
    assert!(shown(&mut v, "contributors-panel"));
    assert!(!shown(&mut v, "contributors-not-synced"));
    assert_eq!(ui.fake.starts()[0].mode, "full");
}

#[gpui::test]
fn switching_period_while_syncing_shows_the_new_periods_own_state(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, unsynced_fake());
    click(&mut v, "contributors-sync");
    assert!(shown(&mut v, "contributors-cancel"));
    click(&mut v, "contributors-period-month");
    assert!(shown(&mut v, "contributors-sync"));
    assert!(!shown(&mut v, "contributors-cancel"));
    click(&mut v, "contributors-period-7d");
    assert!(shown(&mut v, "contributors-cancel"));
    assert_eq!(ui.fake.count(|c| *c == Call::Cancel), 0);
}

// ---- Clear synced data ----------------------------------------------------

#[gpui::test]
fn clear_synced_data_asks_first_then_clears_and_reloads(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    click(&mut v, "contributors-more");
    click(&mut v, "contributors-clear");
    assert!(v.has_pending_prompt());
    v.simulate_prompt_answer("Cancel");
    settle(&mut v);
    assert_eq!(ui.fake.count(|c| *c == Call::Clear), 0);
    assert!(shown(&mut v, "contributors-panel"));

    click(&mut v, "contributors-more");
    click(&mut v, "contributors-clear");
    ui.fake.set_load(unsynced());
    v.simulate_prompt_answer("Delete");
    settle(&mut v);
    assert_eq!(ui.fake.count(|c| *c == Call::Clear), 1);
    assert!(shown(&mut v, "contributors-not-synced"));
}

#[gpui::test]
fn a_refused_clear_is_reported_and_the_data_stays(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    ui.fake.with(|s| {
        s.clear.once.push_back(Err(
            "A sync of this repository is running. Cancel it first.".into(),
        ))
    });
    click(&mut v, "contributors-more");
    click(&mut v, "contributors-clear");
    v.simulate_prompt_answer("Delete");
    settle(&mut v);
    assert!(shown(&mut v, "contributors-action-error"));
    assert!(shown(&mut v, "contributors-panel"));
}

// ---- the window coming back ----------------------------------------------

#[gpui::test]
fn the_window_coming_back_revalidates_the_open_github_tab_only(cx: &mut TestAppContext) {
    let (ui, mut v) = github(cx, Fake::new());
    let before = ui.fake.loads().len();
    ui.page(&mut v, |page, cx| page.window_activated(cx));
    assert_eq!(ui.fake.loads().len(), before + 1);
    click(&mut v, "contributors-page-tab-ticket");
    let after = ui.fake.loads().len();
    ui.page(&mut v, |page, cx| page.window_activated(cx));
    assert_eq!(ui.fake.loads().len(), after);
}
