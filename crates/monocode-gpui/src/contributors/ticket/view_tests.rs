//! The Ticket tab as the user sees it (TicketContributorsView.test.ts): the
//! connection states, the toolbar, the Scope panel, the score card, the
//! leaderboard with its member tickets, and the page around it. The rules
//! behind them are tested in `state_tests`; the copy in `ticket::tests`.
use super::{
    TicketEvent,
    backend::TicketBackend,
    fake::{
        Call, TicketFake, assessment, at, clock, event, now, scope, settings, ticket, ticket_with,
        user,
    },
    state::{BodyState, Comparison, Spaces, TicketView},
};
use crate::{
    contributors::{
        ContributorsPage, Event, Tab,
        chart::{ChartKind, ChartView},
        data_tests::Fake as GithubFake,
        jobs::SyncJobs,
        page_tests::{click, open as open_page, settle, shown},
        stats_tests::right_click,
    },
    input,
};
use chrono::{DateTime, Local};
use gpui::{
    AppContext as _, Focusable as _, TestAppContext, VisualTestContext, WindowHandle, WindowOptions,
};
use monocode_core::contributors::{
    model::period::PeriodKind,
    ticket_jobs::TicketJobEvent,
    tickets::tickets::{TicketRecord, WorkspaceScope},
};
use std::{cell::Cell, rc::Rc, sync::Arc};

struct View {
    window: WindowHandle<TicketView>,
    fake: Arc<TicketFake>,
    jobs: gpui::Entity<SyncJobs>,
    v: VisualTestContext,
    settings_asked: Rc<Cell<u32>>,
    _subscription: gpui::Subscription,
}

fn sel(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn open(cx: &mut TestAppContext, configure: impl FnOnce(&TicketFake)) -> View {
    cx.update(input::bind);
    cx.update(|cx| cx.set_reduce_motion(true));
    let fake = TicketFake::new();
    configure(&fake);
    let cell: Rc<Cell<DateTime<Local>>> = Rc::new(Cell::new(now()));
    let jobs = cx.update(|cx| cx.new(SyncJobs::new));
    let window = cx
        .update(|cx| {
            let jobs = jobs.clone();
            let read = clock(&cell);
            cx.open_window(WindowOptions::default(), move |_, cx| {
                cx.new(|cx| TicketView::new(jobs, read, cx))
            })
        })
        .unwrap();
    let asked = Rc::new(Cell::new(0));
    let count = asked.clone();
    let view = window.root(cx).unwrap();
    let subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &TicketEvent, _| {
            assert_eq!(*event, TicketEvent::OpenSettings);
            count.set(count.get() + 1);
        })
    });
    let backend: Arc<dyn TicketBackend> = fake.clone();
    window
        .update(cx, move |view, _, cx| {
            view.set_backend(backend, cx);
            view.mount(cx);
        })
        .unwrap();
    let mut v = VisualTestContext::from_window(window.into(), cx);
    // Everything fits, and blur listeners only see focus moves of an active window.
    v.simulate_resize(gpui::size(gpui::px(1600.), gpui::px(4000.)));
    window
        .update(&mut v, |_, window, _| window.activate_window())
        .unwrap();
    settle(&mut v);
    View {
        window,
        fake,
        jobs,
        v,
        settings_asked: asked,
        _subscription: subscription,
    }
}

impl View {
    fn read<R>(&mut self, f: impl FnOnce(&TicketView) -> R) -> R {
        self.window.read_with(&self.v, |view, _| f(view)).unwrap()
    }

    fn act<R>(
        &mut self,
        f: impl FnOnce(&mut TicketView, &mut gpui::Context<TicketView>) -> R,
    ) -> R {
        let out = self
            .window
            .update(&mut self.v, |view, _, cx| f(view, cx))
            .unwrap();
        settle(&mut self.v);
        out
    }

    fn click(&mut self, selector: &'static str) {
        click(&mut self.v, selector);
    }

    fn shown(&mut self, selector: &'static str) -> bool {
        shown(&mut self.v, selector)
    }

    fn emit(&mut self, event: TicketJobEvent) {
        self.jobs
            .update(&mut self.v, |jobs, cx| jobs.apply_ticket(event, cx));
        settle(&mut self.v);
    }

    fn body(&mut self) -> BodyState {
        self.read(|view| view.body_state())
    }

    fn percent(&mut self) -> f64 {
        self.read(|view| view.settings.percent)
    }

    /// Puts the focus in the coverage number field.
    fn focus_coverage(&mut self) {
        let focus = self.read_coverage_focus();
        self.v.update(|window, cx| window.focus(&focus, cx));
        settle(&mut self.v);
    }

    fn read_coverage_focus(&mut self) -> gpui::FocusHandle {
        let view = self.window.root(&mut self.v).unwrap();
        self.v.read(|cx| {
            view.read(cx)
                .ui
                .coverage
                .as_ref()
                .expect("the coverage field")
                .input
                .read(cx)
                .focus_handle(cx)
        })
    }

    fn type_coverage(&mut self, text: &str) {
        let view = self.window.root(&mut self.v).unwrap();
        let input = self
            .v
            .read(|cx| view.read(cx).ui.coverage.as_ref().unwrap().input.clone());
        let text = text.to_string();
        input.update(&mut self.v, |input, cx| input.set_text(text, cx));
        settle(&mut self.v);
    }

    fn coverage_text(&mut self) -> String {
        let view = self.window.root(&mut self.v).unwrap();
        self.v.read(|cx| {
            let input = view.read(cx).ui.coverage.as_ref().unwrap().input.clone();
            input.read(cx).content.to_string()
        })
    }

    /// Leaves whatever field has the keyboard, as a click elsewhere does.
    fn blur(&mut self) {
        let view = self.window.root(&mut self.v).unwrap();
        let focus = self.v.read(|cx| view.read(cx).focus_handle(cx));
        self.v.update(|window, cx| window.focus(&focus, cx));
        settle(&mut self.v);
    }

    /// Scrolls the tickets table of the open member to its right end, where
    /// the manual level and the note are.
    fn scroll_tickets_right(&mut self) {
        let bounds = self.v.debug_bounds("ticket-row-t1").expect("a ticket row");
        self.v.simulate_event(gpui::ScrollWheelEvent {
            position: bounds.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(-2000.), gpui::px(0.))),
            ..Default::default()
        });
        settle(&mut self.v);
    }

    fn stored(&self) -> monocode_core::contributors::tickets::prefs::TicketBoardSettings {
        self.fake.stored()
    }
}

fn running(kind: &str, done: u64, failed: u64) -> TicketJobEvent {
    TicketJobEvent {
        done,
        total: 4,
        failed,
        ..event(kind, "running", 1, 0)
    }
}

/// The tickets of TicketContributorsView.test.ts: t1 scored S for Alpha, t2
/// shared by Alpha and Beta, t3 Beta's, t4 nobody's.
fn four() -> Vec<TicketRecord> {
    vec![
        ticket_with("t1", |t| t.assessment = Some(assessment(2))),
        ticket_with("t2", |t| {
            t.assignees = vec![user("a", "Alpha"), user("b", "Beta")]
        }),
        ticket_with("t3", |t| t.assignees = vec![user("b", "Beta")]),
        ticket_with("t4", |t| t.assignees.clear()),
    ]
}

fn with_four(fake: &TicketFake) {
    fake.with(|s| s.load.default = Ok(four()));
}

fn unconnected(fake: &TicketFake) {
    fake.with(|s| s.connected.default = Ok(false));
}

// ---- connection -----------------------------------------------------------------------

#[gpui::test]
fn it_shows_the_settings_button_when_clickup_is_not_connected(cx: &mut TestAppContext) {
    let mut view = open(cx, unconnected);
    assert_eq!(
        view.body(),
        BodyState::Problem {
            message: "Connect ClickUp in Settings to score tickets.".into(),
            retry: false,
        }
    );
    assert!(view.shown("ticket-problem"));
    assert!(!view.shown("ticket-coverage"));
    assert!(!view.shown("ticket-retry"));
    assert!(
        !view.shown("contributors-select-workspace"),
        "no toolbar controls"
    );
    view.click("ticket-open-settings");
    assert_eq!(view.settings_asked.get(), 1);
}

#[gpui::test]
fn it_asks_to_pick_a_workspace_when_none_is_chosen(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.stored = settings(None, 100.)));
    assert_eq!(view.body(), BodyState::PickWorkspace);
    assert!(view.shown("ticket-pick-workspace"));
    assert!(!view.shown("ticket-leaderboard"));
    assert!(!view.shown("ticket-score-card"));
}

#[gpui::test]
fn picking_a_workspace_in_the_select_sets_a_fresh_scope(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("contributors-select-workspace");
    view.click("contributors-option-workspace-1");
    assert_eq!(
        view.stored().scope,
        Some(WorkspaceScope {
            workspace_id: "w2".into(),
            name: "Beta Corp".into(),
            ..scope()
        })
    );
}

fn gone(fake: &TicketFake) {
    fake.with(|s| {
        s.stored = settings(
            Some(WorkspaceScope {
                workspace_id: "gone".into(),
                name: "Old Co".into(),
                ..scope()
            }),
            50.,
        )
    });
}

#[gpui::test]
fn it_says_the_token_cannot_access_a_saved_workspace(cx: &mut TestAppContext) {
    let mut view = open(cx, gone);
    assert_eq!(
        view.body(),
        BodyState::Problem {
            message: "This ClickUp token cannot access Old Co. Pick another workspace or update the token in Settings."
                .into(),
            retry: false,
        }
    );
    assert!(!view.shown("ticket-leaderboard"));
    assert!(!view.shown("ticket-coverage"));
    // Refresh is off for it.
    let fetches = view.fake.fetches();
    view.click("ticket-refresh");
    assert_eq!(view.fake.fetches(), fetches);
    view.click("ticket-open-settings");
    assert_eq!(view.settings_asked.get(), 1);
}

#[gpui::test]
fn it_lets_the_user_pick_another_workspace_when_the_saved_one_is_gone(cx: &mut TestAppContext) {
    let mut view = open(cx, gone);
    view.click("contributors-select-workspace");
    // The token's workspaces, then the saved one that stays selectable.
    assert!(view.shown("contributors-option-workspace-2"));
    view.click("contributors-option-workspace-1");
    assert_eq!(view.stored().scope.unwrap().workspace_id, "w2");
}

#[gpui::test]
fn it_does_not_call_the_saved_workspace_unreachable_while_the_list_loads(cx: &mut TestAppContext) {
    let mut view = open(cx, gone);
    view.act(|view, cx| {
        view.hold_backend_calls();
        view.mount(cx);
    });
    view.act(|view, cx| {
        view.release("prefs", cx);
        view.release("connected", cx);
    });
    let body = view.body();
    assert!(
        !matches!(body, BodyState::Problem { .. }),
        "nothing is claimed before the list is known: {body:?}"
    );
}

#[gpui::test]
fn it_does_not_flash_no_workspaces_while_the_list_loads(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.stored = settings(None, 100.)));
    view.act(|view, cx| {
        view.hold_backend_calls();
        view.mount(cx);
    });
    view.act(|view, cx| {
        view.release("prefs", cx);
        view.release("connected", cx);
    });
    assert_eq!(view.body(), BodyState::LoadingWorkspaces);
    assert!(view.shown("ticket-loading-workspaces"));
    view.act(|view, cx| view.release("workspaces", cx));
    assert_eq!(view.body(), BodyState::PickWorkspace);
}

#[gpui::test]
fn it_says_a_token_without_workspaces_has_none_once_the_list_loaded(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.stored = settings(None, 100.);
            s.workspaces.default = Ok(vec![]);
        })
    });
    assert_eq!(
        view.body(),
        BodyState::Problem {
            message: "This ClickUp token has no workspaces.".into(),
            retry: false,
        }
    );
}

#[gpui::test]
fn it_offers_settings_and_retry_when_clickup_cannot_be_reached(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| s.connected.once.push_back(Err("network down".into())))
    });
    assert_eq!(
        view.body(),
        BodyState::Problem {
            message: "Could not reach ClickUp. network down".into(),
            retry: true,
        }
    );
    assert!(!view.shown("ticket-checking"));
    view.click("ticket-retry");
    assert_eq!(view.body(), BodyState::Board);
    assert_eq!(view.fake.count(|c| *c == Call::Connected), 2);
    // And Settings, from the first problem.
    let mut view = open(cx, |fake| {
        fake.with(|s| s.connected.once.push_back(Err("network down".into())))
    });
    view.click("ticket-open-settings");
    assert_eq!(view.settings_asked.get(), 1);
}

#[gpui::test]
fn it_offers_settings_and_retry_when_the_workspace_list_fails(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| s.workspaces.once.push_back(Err("403 forbidden".into())))
    });
    assert_eq!(
        view.body(),
        BodyState::Problem {
            message: "Could not load ClickUp workspaces. 403 forbidden".into(),
            retry: true,
        }
    );
    assert!(!view.shown("ticket-leaderboard"));
    assert!(
        !view.shown("contributors-select-workspace"),
        "no toolbar controls"
    );
    view.click("ticket-retry");
    assert_eq!(view.body(), BodyState::Board);
}

#[gpui::test]
fn it_says_it_is_checking_clickup_before_the_first_answer(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.act(|view, cx| {
        view.hold_backend_calls();
        view.mount(cx);
    });
    assert_eq!(view.body(), BodyState::Checking);
    assert!(view.shown("ticket-checking"));
}

#[gpui::test]
fn it_shows_a_job_or_load_error_and_an_export_error_next_to_it(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    assert!(!view.shown("ticket-error"));
    let mut error = event("assess", "error", 5, 0);
    error.error = Some("ClickUp said no".into());
    view.emit(error);
    assert!(view.shown("ticket-error"));
    assert!(!view.shown("ticket-export-error"));
    view.act(|view, _| view.export_error = Some("disk full".into()));
    assert!(view.shown("ticket-error") && view.shown("ticket-export-error"));
}

// ---- period ---------------------------------------------------------------------------------

#[gpui::test]
fn it_offers_month_quarter_year_and_all_and_reports_the_pick(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    for kind in ["month", "quarter", "year", "all"] {
        assert!(
            view.shown(sel(format!("contributors-ticket-period-{kind}"))),
            "{kind}"
        );
    }
    assert!(!view.shown("contributors-ticket-period-7d"));
    view.click("contributors-ticket-period-quarter");
    assert_eq!(view.read(|v| v.period.kind), PeriodKind::Quarter);
    view.click("contributors-ticket-period-all");
    assert_eq!(view.read(|v| v.period.kind), PeriodKind::All);
}

#[gpui::test]
fn it_navigates_with_the_arrows_and_disables_next_at_the_end(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    let label = view.read(|v| v.period.label.clone());
    assert_eq!(label, "Oct 2026");
    view.click("ticket-next");
    assert_eq!(
        view.read(|v| v.period.id.clone()),
        "2026-10",
        "next is off at the end"
    );
    view.click("ticket-previous");
    assert_eq!(view.read(|v| v.period.id.clone()), "2026-09");
    view.click("ticket-next");
    assert_eq!(view.read(|v| v.period.id.clone()), "2026-10");
}

#[gpui::test]
fn it_has_no_arrows_for_all(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("contributors-ticket-period-all");
    assert!(!view.shown("ticket-previous"));
    assert!(!view.shown("ticket-next"));
}

// ---- scope ------------------------------------------------------------------------------------

#[gpui::test]
fn it_lists_the_spaces_and_saves_a_space_pick(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    assert!(!view.shown("ticket-scope"));
    view.click("ticket-scope-toggle");
    assert!(view.shown("ticket-scope"));
    assert!(view.shown("ticket-space-all"));
    view.click("ticket-space-s1");
    assert_eq!(
        view.stored().scope.unwrap().space_ids,
        Some(vec!["s1".to_string()])
    );
    // The last one unchecked is every space again.
    view.click("ticket-space-s1");
    assert_eq!(view.stored().scope.unwrap().space_ids, None);
}

#[gpui::test]
fn all_spaces_is_checked_and_disabled_until_a_space_is_picked(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("ticket-scope-toggle");
    view.click("ticket-space-s1");
    view.click("ticket-space-s2");
    view.click("ticket-space-all");
    assert_eq!(view.stored().scope.unwrap().space_ids, None);
}

#[gpui::test]
fn it_explains_an_empty_or_failed_space_list(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.spaces.default = Ok(vec![])));
    view.click("ticket-scope-toggle");
    assert_eq!(view.read(|v| v.spaces.clone()), Some(Spaces::Ready(vec![])));
    assert!(!view.shown("ticket-space-s1"));
    let mut view = open(cx, |fake| {
        fake.with(|s| s.spaces.default = Err("403".into()))
    });
    view.click("ticket-scope-toggle");
    assert!(view.shown("ticket-spaces-error"));
}

#[gpui::test]
fn it_excludes_a_status_and_offers_every_loaded_status_for_it(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("ticket-scope-toggle");
    // Open ones too.
    assert!(view.shown("ticket-excluded-ready for testing"));
    view.click("ticket-excluded-closed");
    assert_eq!(view.stored().scope.unwrap().excluded_statuses, ["closed"]);
}

#[gpui::test]
fn it_lists_delivered_statuses_with_their_type_and_saves_a_pick(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("ticket-scope-toggle");
    assert!(view.shown("ticket-delivered-closed"));
    assert!(view.shown("ticket-delivered-ready for testing"));
    view.click("ticket-delivered-ready for testing");
    assert_eq!(
        view.stored().scope.unwrap().delivered_statuses,
        ["ready for testing"]
    );
    // It is delivered now, so the board counts both tickets.
    assert_eq!(view.read(|v| v.derived.board.total), 2);
    view.click("ticket-delivered-closed");
    assert_eq!(view.stored().scope.unwrap().excluded_statuses, ["closed"]);
    assert_eq!(view.read(|v| v.derived.board.total), 1);
}

#[gpui::test]
fn it_names_the_excluded_members_and_restores_one_without_a_refetch(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        with_four(fake);
        fake.with(|s| {
            s.stored.scope = Some(WorkspaceScope {
                excluded_members: vec![
                    monocode_core::contributors::tickets::tickets::ExcludedMember {
                        id: "z".into(),
                        name: "Gone Gus".into(),
                    },
                ],
                ..scope()
            })
        });
    });
    view.click("ticket-scope-toggle");
    assert!(view.shown("ticket-restore-z"));
    let fetches = view.fake.fetches();
    view.click("ticket-restore-z");
    assert!(view.stored().scope.unwrap().excluded_members.is_empty());
    assert!(!view.shown("ticket-restore-z"));
    assert_eq!(view.fake.fetches(), fetches);
}

#[gpui::test]
fn nobody_excluded_has_no_restore_button(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("ticket-scope-toggle");
    assert!(view.shown("ticket-scope-members"));
    assert!(!view.shown("ticket-restore-a"));
}

#[gpui::test]
fn the_scope_button_is_gone_for_an_unreachable_workspace(cx: &mut TestAppContext) {
    let mut view = open(cx, gone);
    assert!(!view.shown("ticket-scope-toggle"));
    let mut view = open(cx, |_| {});
    assert!(view.shown("ticket-scope-toggle"));
}

// ---- the score card -----------------------------------------------------------------------------

#[gpui::test]
fn it_counts_from_the_board_not_from_the_loaded_window(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| s.load.default = Ok((0..40).map(|i| ticket(&format!("x{i}"))).collect()))
    });
    assert_eq!(view.read(|v| v.derived.board.total), 40);
    let mut view2 = open(cx, |fake| {
        fake.with(|s| {
            let mut tickets = four();
            // Delivered long ago: in the loaded window, not in the period.
            tickets.extend((0..40).map(|i| {
                ticket_with(&format!("old{i}"), |t| {
                    t.done_at = Some(super::fake::at(8, 1, 10))
                })
            }));
            s.load.default = Ok(tickets);
        })
    });
    assert_eq!(view2.read(|v| v.derived.board.total), 4);
    assert_eq!(view2.read(|v| v.derived.board.assessed), 1);
    assert_eq!(view.read(|v| v.period.id.clone()), "2026-10");
}

#[gpui::test]
fn it_says_so_when_every_status_is_excluded_and_not_while_one_is_not(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.stored.scope = Some(WorkspaceScope {
                excluded_statuses: vec!["closed".into(), "Ready For Testing".into()],
                ..scope()
            })
        })
    });
    assert!(view.shown("ticket-all-excluded"));
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.stored.scope = Some(WorkspaceScope {
                excluded_statuses: vec!["closed".into()],
                ..scope()
            })
        })
    });
    assert!(!view.shown("ticket-all-excluded"));
}

#[gpui::test]
fn it_shows_the_approximate_dates_notice_only_when_flagged(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    assert!(!view.shown("ticket-approximate"));
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.stored.scope = Some(WorkspaceScope {
                delivered_statuses: vec!["ready for testing".into()],
                ..scope()
            })
        })
    });
    assert!(view.read(|v| v.derived.board.dates_approximate));
    assert!(view.shown("ticket-approximate"));
}

#[gpui::test]
fn score_scores_and_is_off_when_the_coverage_is_reached_or_a_job_runs(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("ticket-score");
    assert_eq!(
        view.fake
            .count(|c| matches!(c, Call::Assess { force: false, .. })),
        1
    );
    // A job runs: Score is off.
    view.emit(running("assess", 1, 0));
    view.click("ticket-score");
    assert_eq!(view.fake.count(|c| matches!(c, Call::Assess { .. })), 1);
    view.emit(event("assess", "done", 1, 0));
    // Coverage reached: nothing to score.
    view.act(|v, cx| v.set_percent(0., cx));
    view.click("ticket-score");
    assert_eq!(view.fake.count(|c| matches!(c, Call::Assess { .. })), 1);
}

#[gpui::test]
fn the_coverage_is_committed_on_blur_not_while_typing(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.focus_coverage();
    view.type_coverage("80");
    assert_eq!(view.percent(), 100., "not while typing");
    assert_eq!(view.coverage_text(), "80");
    view.blur();
    assert_eq!(view.percent(), 80.);
    assert_eq!(view.stored().percent, 80.);
}

#[gpui::test]
fn the_coverage_is_committed_on_enter_clamped_to_0_100(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.stored.percent = 50.));
    view.focus_coverage();
    view.type_coverage("250");
    view.v.simulate_keystrokes("enter");
    settle(&mut view.v);
    assert_eq!(view.percent(), 100.);
    assert_eq!(view.coverage_text(), "100");
}

#[gpui::test]
fn an_emptied_coverage_field_stays_empty_until_blur_then_is_restored(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.stored.percent = 50.));
    view.focus_coverage();
    view.type_coverage("");
    assert_eq!(view.coverage_text(), "");
    assert_eq!(view.percent(), 50.);
    view.blur();
    assert_eq!(view.percent(), 50.);
    assert_eq!(view.coverage_text(), "50");
}

#[gpui::test]
fn text_that_is_not_a_number_is_put_back(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.stored.percent = 50.));
    view.focus_coverage();
    view.type_coverage("abc");
    view.blur();
    assert_eq!(view.percent(), 50.);
    assert_eq!(view.coverage_text(), "50");
}

#[gpui::test]
fn a_saved_coverage_shows_in_an_idle_field(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.act(|v, cx| v.set_percent(35., cx));
    assert_eq!(view.coverage_text(), "35");
}

#[gpui::test]
fn the_slider_sets_the_coverage_from_a_click_and_the_keys(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    let bounds = view.v.debug_bounds("ticket-slider").expect("the slider");
    view.v.simulate_click(bounds.center(), Default::default());
    settle(&mut view.v);
    assert_eq!(view.percent(), 50.);
    view.v.simulate_click(
        gpui::point(bounds.left(), bounds.center().y),
        Default::default(),
    );
    settle(&mut view.v);
    assert_eq!(view.percent(), 0.);
    view.v.simulate_click(
        gpui::point(bounds.right() - gpui::px(1.), bounds.center().y),
        Default::default(),
    );
    settle(&mut view.v);
    assert_eq!(view.percent(), 100.);
    // The slider has the keyboard after a click.
    view.v.simulate_keystrokes("left left");
    settle(&mut view.v);
    assert_eq!(view.percent(), 98.);
    view.v.simulate_keystrokes("home");
    settle(&mut view.v);
    assert_eq!(view.percent(), 0.);
    view.v.simulate_keystrokes("end");
    settle(&mut view.v);
    assert_eq!(view.percent(), 100.);
    assert_eq!(view.coverage_text(), "100");
}

#[gpui::test]
fn it_shows_progress_and_cancel_while_scoring_and_blocks_score(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    assert!(!view.shown("ticket-progress-row"));
    view.emit(running("assess", 1, 1));
    assert!(view.shown("ticket-progress-row"));
    assert!(view.shown("ticket-progress"));
    view.click("ticket-cancel");
    assert_eq!(view.fake.count(|c| *c == Call::Cancel), 1);
}

#[gpui::test]
fn it_shows_a_fetch_job_and_blocks_refresh_and_score(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.emit(event("fetch", "queued", 3, 1));
    assert!(view.shown("ticket-progress-row"));
    assert!(view.read(|v| v.refresh_disabled()));
    let fetches = view.fake.fetches();
    view.click("ticket-refresh");
    assert_eq!(view.fake.fetches(), fetches);
    view.click("ticket-score");
    assert_eq!(view.fake.count(|c| matches!(c, Call::Assess { .. })), 0);
}

#[gpui::test]
fn it_keeps_refresh_enabled_during_an_assess_job(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.emit(running("assess", 1, 0));
    assert!(!view.read(|v| v.refresh_disabled()));
}

#[gpui::test]
fn refresh_fetches_the_window_again(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.emit(event("fetch", "done", 1, 0));
    let fetches = view.fake.fetches();
    view.click("ticket-refresh");
    assert_eq!(view.fake.fetches(), fetches + 1);
}

#[gpui::test]
fn it_reports_tickets_that_could_not_be_scored_after_a_run(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    let mut done = event("assess", "done", 1, 0);
    done.done = 4;
    done.total = 4;
    done.failed = 2;
    view.emit(done);
    assert!(view.shown("ticket-failed-note"));
    assert!(!view.shown("ticket-progress-row"));
    // A run that cancelled with failures says nothing more than the next one.
    view.emit(running("assess", 0, 0));
    assert!(!view.shown("ticket-failed-note"));
}

// ---- the leaderboard ----------------------------------------------------------------------------------

#[gpui::test]
fn it_ranks_members_with_an_unassigned_row(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    assert!(view.shown("ticket-leaderboard"));
    assert!(view.shown("ticket-member-a"));
    assert!(view.shown("ticket-member-b"));
    assert!(view.shown("ticket-unassigned"));
    view.read(|v| {
        let board = &v.derived.board;
        // Alpha: t1 (score 2 -> S = 8) + half of t2 (unscored).
        assert_eq!(board.members[0].id, "a");
        assert_eq!(board.members[0].points, 8.);
        assert_eq!(
            (board.members[0].tickets, board.members[0].assessed),
            (2, 1)
        );
        assert_eq!(board.members[1].id, "b");
        assert_eq!(
            (board.members[1].tickets, board.members[1].assessed),
            (2, 0)
        );
        assert_eq!(
            (board.unassigned.tickets, board.unassigned.assessed),
            (1, 0)
        );
    });
}

#[gpui::test]
fn there_is_no_unassigned_row_when_everyone_has_an_assignee(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    assert!(!view.shown("ticket-unassigned"));
}

#[gpui::test]
fn it_says_so_when_no_ticket_closed_in_the_period(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.load.default = Ok(vec![])));
    assert!(view.shown("ticket-empty"));
    assert!(!view.shown("ticket-leaderboard"));
    view.click("ticket-score");
    assert_eq!(view.fake.count(|c| matches!(c, Call::Assess { .. })), 0);
    // And Export is off.
    assert!(view.read(|v| v.export_disabled()));
}

#[gpui::test]
fn a_member_expands_to_their_tickets_and_collapses_again(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    assert!(!view.shown("ticket-row-t1"));
    view.click("ticket-expand-a");
    assert!(view.shown("ticket-row-t1"));
    assert!(view.shown("ticket-row-t2"));
    assert!(!view.shown("ticket-row-t3"), "another member's ticket");
    view.click("ticket-expand-a");
    assert!(!view.shown("ticket-row-t1"));
}

#[gpui::test]
fn an_active_only_member_has_no_delivered_tickets(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![ticket_with("w3", |t| {
                t.status = "doing".into();
                t.status_type = "custom".into();
                t.done_at = None;
            })])
        })
    });
    assert!(
        view.shown("ticket-leaderboard"),
        "listed for the work in progress"
    );
    view.click("ticket-expand-a");
    assert!(view.shown("ticket-tickets-a"));
    assert!(!view.shown("ticket-reassess"));
}

#[gpui::test]
fn the_reasons_of_a_criterion_open_and_close(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-expand-a");
    assert!(!view.shown("ticket-reasons-t1"));
    view.click("ticket-criterion-t1-ambiguity");
    assert!(view.shown("ticket-reasons-t1"));
    view.click("ticket-criterion-t1-scope");
    assert!(!view.shown("ticket-reasons-t1"));
    // A ticket nobody assessed has no criteria to open.
    assert!(!view.shown("ticket-criterion-t2-ambiguity"));
}

#[gpui::test]
fn a_level_is_overridden_from_the_set_level_menu(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-expand-a");
    view.scroll_tickets_right();
    view.click("ticket-level-t1");
    assert!(view.shown("menu-action-ai"));
    view.click("menu-action-level:XL");
    assert!(view.fake.calls().contains(&Call::Override {
        id: "t1".into(),
        level: Some("XL".into()),
        note: None,
    }));
    assert!(!view.shown("menu-action-ai"), "the menu closed");
}

#[gpui::test]
fn ai_level_clears_an_override(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![ticket_with("t1", |t| {
                t.assessment = Some(assessment(2));
                t.override_level = Some("L".into());
                t.override_note = Some("big".into());
            })])
        })
    });
    view.click("ticket-expand-a");
    view.scroll_tickets_right();
    view.click("ticket-level-t1");
    view.click("menu-action-ai");
    assert!(view.fake.calls().contains(&Call::Override {
        id: "t1".into(),
        level: None,
        note: None,
    }));
}

#[gpui::test]
fn the_note_is_saved_on_blur_only_for_an_edited_ticket(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![
                ticket_with("t1", |t| t.override_level = Some("L".into())),
                ticket("t2"),
            ])
        })
    });
    view.click("ticket-expand-a");
    view.scroll_tickets_right();
    let note = |view: &mut View, id: &str| {
        let root = view.window.root(&mut view.v).unwrap();
        let id = id.to_string();
        view.v.read(|cx| root.read(cx).ui.notes[&id].input.clone())
    };
    // t1 has a manual level: its note is saved when the field is left.
    let input = note(&mut view, "t1");
    let focus = view.v.read(|cx| input.read(cx).focus_handle(cx));
    view.v.update(|window, cx| window.focus(&focus, cx));
    input.update(&mut view.v, |input, cx| {
        input.set_text("  why  ".into(), cx)
    });
    view.blur();
    assert!(view.fake.calls().contains(&Call::Override {
        id: "t1".into(),
        level: Some("L".into()),
        note: Some("why".into()),
    }));
    // An unchanged note saves nothing.
    let overrides = view.fake.count(|c| matches!(c, Call::Override { .. }));
    view.v.update(|window, cx| window.focus(&focus, cx));
    view.blur();
    assert_eq!(
        view.fake.count(|c| matches!(c, Call::Override { .. })),
        overrides
    );
}

#[gpui::test]
fn escape_in_a_note_field_leaves_it_and_saves_the_note(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![ticket_with("t1", |t| {
                t.override_level = Some("L".into())
            })])
        })
    });
    view.click("ticket-expand-a");
    view.scroll_tickets_right();
    let root = view.window.root(&mut view.v).unwrap();
    let input = view.v.read(|cx| root.read(cx).ui.notes["t1"].input.clone());
    let focus = view.v.read(|cx| input.read(cx).focus_handle(cx));
    view.v.update(|window, cx| window.focus(&focus, cx));
    input.update(&mut view.v, |input, cx| input.set_text("why".into(), cx));
    settle(&mut view.v);
    view.v.simulate_keystrokes("escape");
    settle(&mut view.v);
    assert!(!view.v.update(|window, _| focus.is_focused(window)));
    assert!(view.fake.calls().contains(&Call::Override {
        id: "t1".into(),
        level: Some("L".into()),
        note: Some("why".into()),
    }));
}

#[gpui::test]
fn choosing_a_level_sends_the_note_typed_so_far(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![ticket_with("t1", |t| {
                t.override_level = Some("L".into())
            })])
        })
    });
    view.click("ticket-expand-a");
    view.scroll_tickets_right();
    let root = view.window.root(&mut view.v).unwrap();
    let input = view.v.read(|cx| root.read(cx).ui.notes["t1"].input.clone());
    input.update(&mut view.v, |input, cx| {
        input.set_text("because".into(), cx)
    });
    settle(&mut view.v);
    view.click("ticket-level-t1");
    view.click("menu-action-level:M");
    assert!(view.fake.calls().contains(&Call::Override {
        id: "t1".into(),
        level: Some("M".into()),
        note: Some("because".into()),
    }));
}

#[gpui::test]
fn the_selected_tickets_are_reassessed_and_the_selection_cleared(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-expand-a");
    // Nothing selected: off.
    view.click("ticket-reassess");
    assert_eq!(view.fake.count(|c| matches!(c, Call::Assess { .. })), 0);
    view.click("ticket-select-t1");
    view.click("ticket-reassess");
    assert!(view.fake.calls().contains(&Call::Assess {
        ids: vec!["t1".into()],
        force: true,
    }));
    view.read(|v| assert!(v.ui.selected.is_empty()));
}

#[gpui::test]
fn reassess_selected_is_off_while_a_job_runs(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.emit(running("assess", 1, 0));
    view.click("ticket-expand-a");
    view.click("ticket-select-t1");
    view.click("ticket-reassess");
    assert_eq!(view.fake.count(|c| matches!(c, Call::Assess { .. })), 0);
}

#[gpui::test]
fn a_selection_does_not_survive_collapsing_the_member(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-expand-a");
    view.click("ticket-select-t1");
    view.click("ticket-expand-a");
    view.click("ticket-expand-a");
    view.read(|v| assert!(v.ui.selected.is_empty()));
}

// ---- excluded members -------------------------------------------------------------------------------------

#[gpui::test]
fn every_member_row_has_an_exclude_button_that_takes_the_member_off_without_a_refetch(
    cx: &mut TestAppContext,
) {
    let mut view = open(cx, with_four);
    assert!(view.shown("ticket-exclude-a"));
    assert!(view.shown("ticket-exclude-b"));
    let fetches = view.fake.fetches();
    view.click("ticket-exclude-b");
    assert_eq!(
        view.stored().scope.unwrap().excluded_members,
        [
            monocode_core::contributors::tickets::tickets::ExcludedMember {
                id: "b".into(),
                name: "Beta".into(),
            }
        ]
    );
    assert!(!view.shown("ticket-member-b"));
    assert_eq!(view.fake.fetches(), fetches);
    assert!(!view.shown("ticket-tickets-b"), "the row did not expand");
}

#[gpui::test]
fn excluding_a_member_collapses_their_open_row(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-expand-b");
    assert!(view.shown("ticket-tickets-b"));
    view.click("ticket-exclude-b");
    assert!(!view.shown("ticket-tickets-b"));
    // Restored: it does not come back expanded.
    view.act(|v, cx| v.set_scope(Some(scope()), cx));
    assert!(view.shown("ticket-member-b"));
    assert!(!view.shown("ticket-tickets-b"));
}

// ---- export -------------------------------------------------------------------------------------------------

#[gpui::test]
fn export_is_enabled_when_delivered_tickets_are_shown(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    assert!(!view.read(|v| v.export_disabled()));
}

#[gpui::test]
fn export_is_disabled_without_a_workspace_while_a_fetch_runs_or_waits_and_without_tickets(
    cx: &mut TestAppContext,
) {
    let mut view = open(cx, |fake| fake.with(|s| s.stored = settings(None, 100.)));
    assert!(view.read(|v| v.export_disabled()));
    let mut view = open(cx, |_| {});
    view.emit(running("fetch", 0, 0));
    assert!(view.read(|v| v.export_disabled()));
    // A fetch queued behind an assessment: the latest event is the assessment,
    // so the view is not "fetching", and Refresh stays on.
    let mut view = open(cx, |_| {});
    view.emit(event("assess", "queued", 4, 1));
    assert!(view.read(|v| v.export_disabled()));
    assert!(!view.read(|v| v.refresh_disabled()));
    let mut view = open(cx, |fake| fake.with(|s| s.load.default = Ok(vec![])));
    assert!(view.read(|v| v.export_disabled()));
}

#[gpui::test]
fn the_export_button_runs_the_export_and_the_saved_message_shows(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    assert!(!view.shown("ticket-exported"));
    view.click("ticket-export");
    assert!(view.read(|v| v.exporting));
    assert!(view.read(|v| v.export_disabled()), "Preparing report...");
    let dir = std::env::temp_dir().join(format!("monocode-ticket-view-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("report.html");
    let target = file.clone();
    view.act(|v, _| v.reveal = Rc::new(|_, _| {}));
    view.v.simulate_new_path_selection(move |_| Some(target));
    settle(&mut view.v);
    assert!(view.shown("ticket-exported"));
    assert!(file.exists());
    let _ = std::fs::remove_dir_all(&dir);
    // The period changes: the message goes.
    view.click("ticket-previous");
    assert!(!view.shown("ticket-exported"));
}

#[gpui::test]
fn a_failed_export_shows_its_error(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.covered.default = Ok(true)));
    view.click("ticket-export");
    let missing = std::env::temp_dir()
        .join(format!("monocode-ticket-missing-{}", std::process::id()))
        .join("report.html");
    view.v.simulate_new_path_selection(move |_| Some(missing));
    settle(&mut view.v);
    assert!(view.shown("ticket-export-error"));
    assert!(!view.shown("ticket-exported"));
}

// ---- keyboard ------------------------------------------------------------------------------------------------

#[gpui::test]
fn escape_in_the_coverage_field_leaves_it_and_commits(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.stored.percent = 50.));
    view.focus_coverage();
    view.type_coverage("70");
    view.v.simulate_keystrokes("escape");
    settle(&mut view.v);
    assert_eq!(view.percent(), 70., "leaving commits");
    let focus = view.read_coverage_focus();
    assert!(!view.v.update(|window, _| focus.is_focused(window)));
}

// ---- the page around it -------------------------------------------------------------------------------------------

fn page(
    cx: &mut TestAppContext,
    tab: Tab,
) -> (
    crate::contributors::page_tests::Ui,
    VisualTestContext,
    Arc<TicketFake>,
) {
    cx.update(input::bind);
    let ticket_fake = TicketFake::new();
    let (ui, mut v) = open_page(
        cx,
        GithubFake::new(),
        &["/work/alpha", "/work/beta"],
        "/work/alpha",
        tab,
    );
    let backend: Arc<dyn TicketBackend> = ticket_fake.clone();
    ui.window
        .update(&mut v, |page, _, cx| page.set_ticket_backend(backend, cx))
        .unwrap();
    settle(&mut v);
    (ui, v, ticket_fake)
}

#[gpui::test]
fn the_page_shows_the_ticket_tab_and_reads_through_the_ticket_backend(cx: &mut TestAppContext) {
    let (_ui, mut v, fake) = page(cx, Tab::Ticket);
    assert!(shown(&mut v, "contributors-body-ticket"));
    assert!(shown(&mut v, "ticket-leaderboard"));
    assert!(fake.fetches() >= 1);
}

#[gpui::test]
fn escape_in_a_ticket_field_blurs_it_and_the_next_escape_closes_the_page(cx: &mut TestAppContext) {
    let (ui, mut v, _fake) = page(cx, Tab::Ticket);
    let page_view: gpui::Entity<ContributorsPage> = ui.window.root(&mut v).unwrap();
    let ticket = v.read(|cx| page_view.read(cx).ticket.clone());
    let focus = v.read(|cx| {
        ticket
            .read(cx)
            .ui
            .coverage
            .as_ref()
            .unwrap()
            .input
            .read(cx)
            .focus_handle(cx)
    });
    v.update(|window, cx| window.focus(&focus, cx));
    settle(&mut v);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 0, "a text field handles its own Escape");
    assert!(!v.update(|window, _| focus.is_focused(window)));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
}

#[gpui::test]
fn the_page_tells_the_shell_when_the_user_wants_settings(cx: &mut TestAppContext) {
    let (ui, mut v, fake) = page(cx, Tab::Ticket);
    fake.with(|s| s.connected.default = Ok(false));
    let asked = Rc::new(Cell::new(0));
    let count = asked.clone();
    let page_view = ui.window.root(&mut v).unwrap();
    let _subscription = v.update(|_, cx| {
        cx.subscribe(&page_view, move |_, event: &Event, _| {
            if matches!(event, Event::OpenSettings) {
                count.set(count.get() + 1);
            }
        })
    });
    // Look again at the connection, as coming back to the window does.
    ui.window
        .update(&mut v, |page, _, cx| page.window_activated(cx))
        .unwrap();
    settle(&mut v);
    click(&mut v, "ticket-open-settings");
    assert_eq!(asked.get(), 1);
}

#[gpui::test]
fn opening_the_ticket_tab_again_mounts_it_afresh(cx: &mut TestAppContext) {
    let (ui, mut v, fake) = page(cx, Tab::Github);
    let connects = fake.count(|c| *c == Call::Connected);
    ui.window
        .update(&mut v, |page, _, cx| page.set_tab(Tab::Ticket, cx))
        .unwrap();
    settle(&mut v);
    assert_eq!(fake.count(|c| *c == Call::Connected), connects + 1);
    // Staying on it is not a new mount.
    ui.window
        .update(&mut v, |page, _, cx| page.set_tab(Tab::Ticket, cx))
        .unwrap();
    settle(&mut v);
    assert_eq!(fake.count(|c| *c == Call::Connected), connects + 1);
}

#[gpui::test]
fn the_ticket_tab_is_unmounted_with_the_tab_and_with_the_page(cx: &mut TestAppContext) {
    let (ui, mut v, fake) = page(cx, Tab::Ticket);
    let page_view: gpui::Entity<ContributorsPage> = ui.window.root(&mut v).unwrap();
    let ticket = v.read(|cx| page_view.read(cx).ticket.clone());
    let jobs = v.read(|cx| page_view.read(cx).jobs.clone());
    let mounted = |v: &mut VisualTestContext| v.read(|cx| ticket.read(cx).mounted);
    assert!(mounted(&mut v));
    // The GitHub tab is shown: the Ticket tab is gone, as React unmounts it.
    ui.window
        .update(&mut v, |page, _, cx| page.set_tab(Tab::Github, cx))
        .unwrap();
    settle(&mut v);
    assert!(!mounted(&mut v));
    fake.clear_calls();
    jobs.update(&mut v, |jobs, cx| {
        jobs.apply_ticket(event("assess", "done", 4, 0), cx)
    });
    settle(&mut v);
    ui.window
        .update(&mut v, |page, _, cx| page.window_activated(cx))
        .unwrap();
    settle(&mut v);
    assert_eq!(fake.calls(), Vec::<Call>::new());
    // Back on it: mounted afresh.
    ui.window
        .update(&mut v, |page, _, cx| page.set_tab(Tab::Ticket, cx))
        .unwrap();
    settle(&mut v);
    assert!(mounted(&mut v));
    // The page closes (another view, Settings): the tab is unmounted, and shown again with the page.
    ui.window
        .update(&mut v, |page, _, cx| page.hide(cx))
        .unwrap();
    settle(&mut v);
    assert!(!mounted(&mut v));
    fake.clear_calls();
    jobs.update(&mut v, |jobs, cx| {
        jobs.apply_ticket(event("fetch", "error", 5, 0), cx)
    });
    settle(&mut v);
    assert_eq!(fake.calls(), Vec::<Call>::new());
    ui.window
        .update(&mut v, |page, _, cx| page.reveal(cx))
        .unwrap();
    settle(&mut v);
    assert!(mounted(&mut v));
    assert!(fake.calls().contains(&Call::Connected));
}

#[gpui::test]
fn the_page_clock_reaches_the_ticket_tab(cx: &mut TestAppContext) {
    let (ui, mut v, _fake) = page(cx, Tab::Ticket);
    let page_view: gpui::Entity<ContributorsPage> = ui.window.root(&mut v).unwrap();
    let ticket = v.read(|cx| page_view.read(cx).ticket.clone());
    let expected = crate::contributors::data_tests::start_of_tests();
    assert_eq!(v.read(|cx| ticket.read(cx).now), expected);
}

// ---- the overview: cards, trend, difficulty map ----------------------------------------------------

fn trend_chart(view: &mut View) -> gpui::Entity<ChartView> {
    let root = view.window.root(&mut view.v).unwrap();
    view.v.read(|cx| {
        root.read(cx)
            .overview
            .chart
            .clone()
            .expect("the trend chart")
    })
}

/// A ticket Alpha closed in September, which the store holds.
fn september(id: &str) -> TicketRecord {
    ticket_with(id, |t| {
        t.done_at = Some(at(9, 5, 10));
        t.updated_at = at(9, 5, 10);
        t.override_level = Some("S".into());
    })
}

/// `n` members with one scored ticket each.
fn crowd(n: usize) -> Vec<TicketRecord> {
    (1..=n)
        .map(|i| {
            ticket_with(&format!("c{i}"), |t| {
                t.assignees = vec![user(&format!("m{i}"), &format!("Member {i}"))];
                t.override_level = Some(["XS", "S", "M", "L", "XL"][i % 5].into());
            })
        })
        .collect()
}

#[gpui::test]
fn the_board_opens_with_cards_a_trend_and_a_difficulty_map(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    for selector in [
        "ticket-stats",
        "ticket-stat-delivered",
        "ticket-stat-points",
        "ticket-stat-scored",
        "ticket-stat-median",
        "ticket-trend-chart",
        "ticket-trend-legend-0",
        "ticket-levels",
        "ticket-levels-row-a",
        "ticket-levels-a-S",
        "ticket-levels-row-b",
        "ticket-levels-row-unassigned",
        "ticket-leaderboard",
    ] {
        assert!(view.shown(selector), "{selector} is drawn");
    }
}

#[gpui::test]
fn nothing_delivered_shows_no_cards_chart_or_map(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.load.default = Ok(vec![])));
    for selector in ["ticket-stats", "ticket-trend-chart", "ticket-levels"] {
        assert!(!view.shown(selector), "{selector} is not drawn");
    }
}

#[gpui::test]
fn tickets_not_scored_yet_are_flagged_on_the_card(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    assert!(view.shown("ticket-stat-scored-flag"));
    let mut all = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![ticket_with("x", |t| {
                t.assessment = Some(assessment(2))
            })])
        })
    });
    assert!(!all.shown("ticket-stat-scored-flag"));
}

#[gpui::test]
fn the_unassigned_row_of_the_map_has_counts_and_no_points(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(vec![
                ticket_with("u1", |t| {
                    t.assignees.clear();
                    t.override_level = Some("M".into());
                }),
                ticket_with("u2", |t| t.assignees.clear()),
            ])
        })
    });
    assert!(view.shown("ticket-levels-row-unassigned"));
    view.read(|v| {
        let row = v.derived.matrix.last().unwrap();
        assert!(row.unassigned);
        assert_eq!((row.total.delivered, row.total.scored), (2, 1));
        assert!(row.cells.iter().all(|c| c.points == 0.0));
        assert_eq!(row.cells[2].delivered, 1);
    });
}

#[gpui::test]
fn a_previous_period_in_the_store_makes_the_cards_say_how_they_moved(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| s.previous.default = Ok(Some(vec![september("p1")])))
    });
    assert!(!view.shown("ticket-comparison-note"));
    view.read(|v| match &v.derived.comparison {
        Comparison::Ready { stats, against, .. } => {
            assert_eq!(stats.delivered, 1);
            assert_eq!(against, "the same days of Sep 2026");
        }
        other => panic!("{other:?}"),
    });
}

#[gpui::test]
fn a_previous_period_the_store_lacks_gets_a_note_and_no_fetch_of_its_own(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    assert!(view.shown("ticket-comparison-note"));
    // The window on screen is fetched, as before; the previous one never.
    let (from, _) = view.read(|v| v.window());
    let fetched: Vec<String> = view
        .fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Fetch { from, .. } => Some(from),
            _ => None,
        })
        .collect();
    assert!(!fetched.is_empty());
    assert!(fetched.iter().all(|f| *f == from), "{fetched:?}");
    assert_eq!(view.fake.previous_reads(), 1);
}

#[gpui::test]
fn the_trend_has_a_legend_of_five_members_and_others(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.load.default = Ok(crowd(8))));
    let chart = trend_chart(&mut view);
    let series = view.v.read(|cx| chart.read(cx).data.series.len());
    assert_eq!(series, 6);
    for n in 0..6 {
        assert!(view.shown(sel(format!("ticket-trend-legend-{n}"))));
    }
    assert!(!view.shown("ticket-trend-legend-6"));
    let last = view.v.read(|cx| chart.read(cx).data.series[5].id.clone());
    assert_eq!(last, "others");
    // Others has no color button; a member has one.
    assert!(!view.shown("ticket-trend-legend-5-color"));
    assert!(view.shown("ticket-trend-legend-0-color"));
}

#[gpui::test]
fn a_legend_click_focuses_a_member_and_a_second_lets_go(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    let chart = trend_chart(&mut view);
    view.click("ticket-trend-legend-0");
    assert_eq!(view.v.read(|cx| chart.read(cx).focus.len()), 1);
    view.click("ticket-trend-legend-0");
    assert!(view.v.read(|cx| chart.read(cx).focus.is_empty()));
}

#[gpui::test]
fn the_chart_type_is_the_apps_and_a_pick_is_saved(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    let chart = trend_chart(&mut view);
    assert_eq!(view.v.read(|cx| chart.read(cx).kind), ChartKind::Line);
    for kind in ["line", "cumulative", "bars"] {
        assert!(view.shown(sel(format!("contributors-chart-kind-{kind}"))));
    }
    view.click("contributors-chart-kind-bars");
    assert_eq!(view.v.read(|cx| chart.read(cx).kind), ChartKind::Bars);
    assert_eq!(
        view.fake.with(|s| s.chart_kind.clone()).as_deref(),
        Some("bars")
    );
    let prefs = view.read(|v| v.chart_prefs.clone());
    assert_eq!(view.v.read(|cx| prefs.read(cx).kind), ChartKind::Bars);
}

/// A frame as a pointer move causes it: no refresh, so cached views are replayed
/// (`settle` refreshes, which draws everything afresh).
fn frame(view: &mut View) {
    view.v.run_until_parked();
    view.v.update(|window, cx| window.draw(cx).clear(cx));
    view.v.run_until_parked();
}

fn chart_len(view: &mut View) -> usize {
    let chart = trend_chart(view);
    view.v.read(|cx| chart.read(cx).data.buckets.len())
}

/// Builds of the tab, of leaderboard rows and of ticket rows so far.
fn built() -> [usize; 3] {
    super::BUILT.with(|b| b.get())
}

#[gpui::test]
fn the_pointer_moves_the_chart_hover_without_building_the_rows_again(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    // A member's tickets open, so both kinds of row are on screen.
    view.click("ticket-expand-a");
    assert!(view.shown("ticket-row-t1"));
    let chart = trend_chart(&mut view);
    let bounds = view.v.debug_bounds("ticket-trend-chart").unwrap();
    let at = |x: f32| {
        gpui::point(
            bounds.origin.x + bounds.size.width * x,
            bounds.origin.y + bounds.size.height * 0.5,
        )
    };
    // Frames that measure the parts, then the hover.
    frame(&mut view);
    frame(&mut view);
    let before = built();
    for x in [0.01, 0.99] {
        view.v.simulate_mouse_move(at(x), None, Default::default());
        frame(&mut view);
        assert_eq!(
            view.v.read(|cx| chart.read(cx).hover),
            Some(if x < 0.5 { 0 } else { chart_len(&mut view) - 1 })
        );
    }
    view.v.simulate_mouse_move(
        gpui::point(gpui::px(1.), gpui::px(1.)),
        None,
        Default::default(),
    );
    frame(&mut view);
    assert_eq!(view.v.read(|cx| chart.read(cx).hover), None);
    let after = built();
    assert_eq!(after[1], before[1], "no leaderboard row was built again");
    assert_eq!(after[2], before[2], "no ticket row was built again");
    // The counter is live: a change of the tab does build the rows.
    view.click("ticket-expand-b");
    assert!(built()[1] > after[1]);
}

#[gpui::test]
fn a_color_picked_in_the_legend_is_saved_for_the_workspace(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    right_click(&mut view.v, "ticket-trend-legend-0");
    assert!(view.shown("ticket-trend-palette-3"));
    view.click("ticket-trend-palette-3");
    let saved = view.fake.calls().into_iter().find_map(|c| match c {
        Call::SaveColors { workspace, colors } => Some((workspace, colors)),
        _ => None,
    });
    let (workspace, colors) = saved.expect("a save");
    assert_eq!(workspace, "w1");
    assert_eq!(colors.len(), 1);
    assert_eq!(
        colors.values().next().copied(),
        Some(monocode_core::contributors::model::palette::PERSON_COLORS[3])
    );
}

// ---- a member's page ---------------------------------------------------------------------------------

fn detail_of(view: &mut View) -> Option<String> {
    view.read(|v| v.detail.clone())
}

#[gpui::test]
fn a_row_opens_the_members_page_in_place_of_the_board(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-board-row-a");
    assert_eq!(detail_of(&mut view), Some("a".into()));
    for selector in [
        "ticket-detail",
        "ticket-detail-back",
        "ticket-detail-header",
        "ticket-detail-kpi-points",
        "ticket-detail-kpi-delivered",
        "ticket-detail-kpi-scored",
        "ticket-detail-kpi-coverage",
        "ticket-detail-chart",
        "ticket-detail-legend-0",
        "ticket-detail-calendar",
        "ticket-detail-rhythm",
        "ticket-detail-levels",
        "ticket-detail-recent",
        "ticket-tickets-a",
        "ticket-row-t1",
    ] {
        assert!(view.shown(selector), "{selector} is drawn");
    }
    for selector in [
        "ticket-leaderboard",
        "ticket-levels",
        "ticket-stats",
        "ticket-trend-chart",
        "ticket-score",
    ] {
        assert!(!view.shown(selector), "{selector} is not drawn");
    }
    assert!(view.shown("ticket-refresh"), "the toolbar stays");
}

#[gpui::test]
fn the_name_opens_the_page_but_the_chevron_and_the_exclude_button_do_not(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-expand-a");
    assert_eq!(detail_of(&mut view), None);
    assert!(view.shown("ticket-tickets-a"));
    view.click("ticket-exclude-b");
    assert_eq!(detail_of(&mut view), None);
    view.click("ticket-member-a");
    assert_eq!(detail_of(&mut view), Some("a".into()));
}

#[gpui::test]
fn a_hover_on_the_page_does_not_build_its_ticket_rows_again(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-board-row-a");
    assert!(view.shown("ticket-row-t1"));
    // The first frames measure the part; a hover comes after they settled.
    frame(&mut view);
    frame(&mut view);
    let before = built();
    for selector in [
        "ticket-detail-calendar",
        "ticket-detail-rhythm",
        "ticket-detail-chart",
        "ticket-detail-tickets-chart",
    ] {
        let bounds = view.v.debug_bounds(selector).unwrap();
        view.v
            .simulate_mouse_move(bounds.center(), None, Default::default());
        frame(&mut view);
    }
    view.v.simulate_mouse_move(
        gpui::point(gpui::px(1.), gpui::px(1.)),
        None,
        Default::default(),
    );
    frame(&mut view);
    assert_eq!(built()[2], before[2], "no ticket row was built again");
    // A change of the page does build them: ticking one.
    view.click("ticket-select-t1");
    assert!(built()[2] > before[2]);
}

#[gpui::test]
fn the_page_has_a_chart_of_points_and_one_of_tickets_following_one_type(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-board-row-a");
    assert!(view.shown("ticket-detail-chart"));
    assert!(view.shown("ticket-detail-tickets-chart"));
    // One set of type tabs, which drive both.
    view.click("contributors-chart-kind-bars");
    let root = view.window.root(&mut view.v).unwrap();
    let kinds = view.v.read(|cx| {
        let tab = root.read(cx);
        (tab.detail_chart_kinds(cx), tab.chart_prefs.read(cx).kind)
    });
    assert_eq!(kinds.0, (ChartKind::Bars, ChartKind::Bars));
    assert_eq!(kinds.1, ChartKind::Bars);
}

#[gpui::test]
fn back_puts_the_list_back_as_it_was(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    // Something of every kind to remember: b's tickets open, one ticked, a scroll.
    view.click("ticket-expand-b");
    view.click("ticket-select-t3");
    view.v
        .simulate_resize(gpui::size(gpui::px(1600.), gpui::px(300.)));
    settle(&mut view.v);
    let offset = gpui::point(gpui::px(0.), gpui::px(-80.));
    view.read(|v| v.ui.scroll.set_offset(offset));
    settle(&mut view.v);
    let snapshot = |view: &mut View| {
        let focus = view.read(|v| v.ui.focus.clone());
        let focused = view.v.update(|window, _| focus.is_focused(window));
        view.read(|v| {
            let mut selected: Vec<String> = v.ui.selected.iter().cloned().collect();
            selected.sort();
            (
                v.ui.open_member.clone(),
                selected,
                v.ui.scroll.offset(),
                focused,
            )
        })
    };
    let before = snapshot(&mut view);
    assert_eq!(before.2, offset, "the scroll took");
    assert_eq!(before.0.as_deref(), Some("b"));
    // The row is scrolled out of the short window, so the page is opened directly.
    let window = view.window;
    window
        .update(&mut view.v, |v, window, cx| {
            v.open_detail("a".into(), window, cx)
        })
        .unwrap();
    settle(&mut view.v);
    // The page opens at the top, with a's own tickets.
    assert_eq!(
        view.read(|v| v.ui.scroll.offset()),
        gpui::point(gpui::px(0.), gpui::px(0.))
    );
    assert!(view.shown("ticket-tickets-a"));
    assert!(!view.shown("ticket-tickets-b"));
    view.click("ticket-detail-back");
    assert_eq!(detail_of(&mut view), None);
    assert!(view.shown("ticket-tickets-b"));
    assert!(!view.shown("ticket-tickets-a"));
    assert_eq!(snapshot(&mut view), before);
}

#[gpui::test]
fn escape_closes_the_members_page_first_and_the_page_next(cx: &mut TestAppContext) {
    let (ui, mut v, _fake) = page(cx, Tab::Ticket);
    click(&mut v, "ticket-board-row-a");
    assert!(shown(&mut v, "ticket-detail"));
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "ticket-detail"));
    assert!(shown(&mut v, "ticket-leaderboard"));
    assert_eq!(ui.closed_count(), 0);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert_eq!(ui.closed_count(), 1);
}

#[gpui::test]
fn escape_closes_the_members_page_wherever_the_focus_is(cx: &mut TestAppContext) {
    let (ui, mut v, _fake) = page(cx, Tab::Ticket);
    click(&mut v, "ticket-board-row-a");
    // The page root, as after a project switch or a reopened page.
    ui.window
        .update(&mut v, |page, window, cx| window.focus(&page.focus, cx))
        .unwrap();
    settle(&mut v);
    v.simulate_keystrokes("escape");
    settle(&mut v);
    assert!(!shown(&mut v, "ticket-detail"));
    assert_eq!(ui.closed_count(), 0, "the page stayed open");
}

#[gpui::test]
fn a_member_missing_from_the_period_gets_a_line_and_the_way_back(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    let window = view.window;
    window
        .update(&mut view.v, |v, window, cx| {
            v.open_detail("nobody".into(), window, cx)
        })
        .unwrap();
    settle(&mut view.v);
    assert!(view.shown("ticket-detail-empty"));
    assert!(view.shown("ticket-detail-back"));
    view.click("ticket-detail-back");
    assert!(view.shown("ticket-leaderboard"));
}

#[gpui::test]
fn a_page_leaves_out_the_cards_that_have_no_data(cx: &mut TestAppContext) {
    // Only unscored work: no points, and nothing in a working status.
    let mut view = open(cx, |fake| {
        fake.with(|s| s.load.default = Ok(vec![ticket("w1")]))
    });
    view.click("ticket-board-row-a");
    assert!(view.shown("ticket-detail-kpi-delivered"));
    assert!(view.shown("ticket-detail-kpi-coverage"));
    assert!(!view.shown("ticket-detail-kpi-points"));
    assert!(!view.shown("ticket-detail-kpi-active"));
    // And the levels list says there is nothing to list.
    assert!(view.shown("ticket-detail-levels"));
}

#[gpui::test]
fn a_page_shows_the_active_card_only_with_work_in_progress(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("ticket-board-row-a");
    assert!(
        view.shown("ticket-detail-kpi-active"),
        "the default fixture has a ticket in testing"
    );
}

#[gpui::test]
fn the_numbers_of_a_page_are_worked_out_again_when_the_board_is_rebuilt(cx: &mut TestAppContext) {
    let mut view = open(cx, |_| {});
    view.click("ticket-board-row-a");
    let shown = |view: &mut View| view.read(|v| v.cached_detail());
    let (seq, delivered) = shown(&mut view).expect("numbers");
    assert_eq!(delivered, 1);
    // The store gains a ticket and a fetch ends: the board is rebuilt.
    view.fake.with(|s| {
        s.load.default = Ok(vec![ticket("done"), ticket("later")]);
    });
    view.emit(event("fetch", "done", 1, 0));
    let (again, delivered) = shown(&mut view).expect("numbers");
    assert!(again > seq);
    assert_eq!(delivered, 2);
}

// ---- hover cost ------------------------------------------------------------------------------------------

/// What one hover costs the ClickUp tab: builds of the tab, leaderboard rows
/// and ticket rows, and the median wall time of a frame, per move.
/// Read through `Debug` by the timing test.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::contributors) struct HoverNumbers {
    pub tab: f64,
    pub member_rows: f64,
    pub ticket_rows: f64,
    pub frame_ms: f64,
    pub moves: usize,
}

fn crowded(members: usize, each: usize) -> Vec<TicketRecord> {
    (0..members * each)
        .map(|i| {
            ticket_with(&format!("h{i}"), |t| {
                t.assignees = vec![user(
                    &format!("m{}", i % members),
                    &format!("Member {}", i % members),
                )];
                t.override_level = Some(["XS", "S", "M", "L", "XL"][i % 5].into());
                t.done_at = Some(at(10, 1 + (i % 16) as u32, 9 + (i % 8) as u32));
                t.updated_at = t.done_at.clone().unwrap();
            })
        })
        .collect()
}

/// Moves the pointer along `selector` and reports the cost per move. `detail`
/// opens the page of member `m0` first.
pub(in crate::contributors) fn hover_numbers(
    cx: &mut TestAppContext,
    members: usize,
    each: usize,
    detail: Option<&'static str>,
    selector: &'static str,
) -> HoverNumbers {
    let mut view = open(cx, |fake| {
        fake.with(|s| s.load.default = Ok(crowded(members, each)))
    });
    if let Some(who) = detail {
        let window = view.window;
        window
            .update(&mut view.v, |v, window, cx| {
                v.open_detail(who.into(), window, cx)
            })
            .unwrap();
        settle(&mut view.v);
    }
    let bounds = view.v.debug_bounds(selector).expect("the hovered element");
    let spot = |x: f32| {
        gpui::point(
            bounds.origin.x + bounds.size.width * x,
            bounds.origin.y + bounds.size.height * 0.5,
        )
    };
    let mut times = vec![];
    let (mut before, mut moves) = ([0; 3], 0);
    for n in 0..24 {
        if n == 4 {
            before = super::BUILT.with(|b| b.get());
        }
        let t = std::time::Instant::now();
        view.v
            .simulate_mouse_move(spot([0.1, 0.5, 0.9][n % 3]), None, Default::default());
        view.v.update(|window, cx| window.draw(cx).clear(cx));
        if n >= 4 {
            times.push(t.elapsed().as_secs_f64() * 1000.);
            moves += 1;
        }
    }
    let after = super::BUILT.with(|b| b.get());
    times.sort_by(|a, b| a.total_cmp(b));
    let per = |i: usize| (after[i] - before[i]) as f64 / moves as f64;
    HoverNumbers {
        tab: per(0),
        member_rows: per(1),
        ticket_rows: per(2),
        frame_ms: times[times.len() / 2],
        moves,
    }
}

// ---- the member's year ---------------------------------------------------------------------------------

/// Alpha's tickets: one this month, one in March (outside the period, inside the year).
fn this_year() -> Vec<TicketRecord> {
    vec![
        ticket_with("now", |t| t.assessment = Some(assessment(2))),
        ticket_with("march", |t| {
            t.done_at = Some(at(3, 9, 10));
            t.updated_at = at(3, 9, 10);
        }),
    ]
}

fn calendar_total(view: &mut View) -> u32 {
    let root = view.window.root(&mut view.v).unwrap();
    view.v.read(|cx| {
        root.read(cx)
            .calendar_days(cx)
            .expect("a calendar")
            .iter()
            .map(|d| d.delivery.tickets)
            .sum()
    })
}

#[gpui::test]
fn the_calendar_covers_the_last_year_read_from_the_store_without_a_fetch(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| fake.with(|s| s.load.default = Ok(this_year())));
    let fetches = view.fake.fetches();
    view.click("ticket-board-row-a");
    // The period holds one ticket; the year holds both.
    assert_eq!(view.read(|v| v.derived.board.total), 1);
    assert_eq!(calendar_total(&mut view), 2);
    assert_eq!(view.fake.fetches(), fetches, "opening the page fetched");
    let reads: Vec<Call> = view
        .fake
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::History { .. }))
        .collect();
    assert_eq!(reads.len(), 1);
    // The year runs from the Sunday 53 weeks back to the next midnight.
    let Call::History {
        workspace, from, ..
    } = &reads[0]
    else {
        unreachable!()
    };
    assert_eq!(workspace, "w1");
    assert!(from.as_str() < at(10, 17, 12).as_str());
    assert_eq!(view.read(|v| v.year_state()), super::detail::Year::Whole);
    // And it stays the year when the period changes with the page open.
    view.act(|v, cx| v.set_kind(PeriodKind::Year, cx));
    assert!(view.shown("ticket-detail"));
    assert_eq!(calendar_total(&mut view), 2);
}

#[gpui::test]
fn a_year_the_store_does_not_hold_whole_says_so(cx: &mut TestAppContext) {
    let mut view = open(cx, |fake| {
        fake.with(|s| {
            s.load.default = Ok(this_year());
            s.history_covered = false;
        })
    });
    view.click("ticket-board-row-a");
    assert_eq!(view.read(|v| v.year_state()), super::detail::Year::Partial);
}

#[gpui::test]
fn the_year_is_read_again_after_a_job_changed_the_store_and_not_while_the_page_is_closed(
    cx: &mut TestAppContext,
) {
    let mut view = open(cx, |fake| fake.with(|s| s.load.default = Ok(this_year())));
    let reads = |view: &mut View| view.fake.count(|c| matches!(c, Call::History { .. }));
    assert_eq!(reads(&mut view), 0, "the board does not need a year");
    view.click("ticket-board-row-a");
    assert_eq!(reads(&mut view), 1);
    view.emit(event("fetch", "done", 1, 0));
    assert_eq!(reads(&mut view), 2);
}

#[gpui::test]
fn a_members_page_frees_its_atlas_tiles_when_it_closes(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-board-row-a");
    frame(&mut view);
    // The image is made once the motion has settled: say it started long ago.
    let root = view.window.root(&mut view.v).unwrap();
    let long_ago = crate::visuals::dither::Motion(
        std::time::Instant::now() - std::time::Duration::from_secs(5),
    );
    view.v.update(|_, cx| {
        let ui = &root.read(cx).detail_ui;
        let (calendar, rhythm) = (ui.calendar.clone().unwrap(), ui.rhythm.clone().unwrap());
        calendar.update(cx, |c, cx| {
            c.motion = long_ago;
            cx.notify();
        });
        rhythm.update(cx, |r, cx| {
            r.motion = long_ago;
            cx.notify();
        });
    });
    frame(&mut view);
    frame(&mut view);
    let images = view.v.read(|cx| {
        let ui = &root.read(cx).detail_ui;
        let of = |s: &crate::visuals::dither::Sheets| s.borrow().as_ref().map(|s| s.image());
        (
            ui.calendar.as_ref().and_then(|c| of(&c.read(cx).sheet)),
            ui.rhythm.as_ref().and_then(|r| of(&r.read(cx).sheet)),
        )
    });
    let (calendar, rhythm) = (
        images.0.expect("a calendar image"),
        images.1.expect("a rhythm image"),
    );
    let held = |view: &mut View| {
        view.v.update(|window, _| {
            (
                window.has_image_atlas_entry(&calendar),
                window.has_image_atlas_entry(&rhythm),
            )
        })
    };
    assert_eq!(held(&mut view), (true, true));
    view.click("ticket-detail-back");
    frame(&mut view);
    assert_eq!(held(&mut view), (false, false));
}

#[gpui::test]
fn unmounting_closes_a_palette_left_open_on_the_overview_chart(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    frame(&mut view);
    let chart = trend_chart(&mut view);
    let id = view.v.read(|cx| chart.read(cx).data.series[0].id.clone());
    view.v.update(|window, cx| {
        chart.update(cx, |c, cx| c.open_palette(&id, window, cx));
    });
    assert!(view.v.read(|cx| chart.read(cx).palette.is_some()));
    view.act(|v, cx| v.unmount(cx));
    assert!(view.v.read(|cx| chart.read(cx).palette.is_none()));
}
