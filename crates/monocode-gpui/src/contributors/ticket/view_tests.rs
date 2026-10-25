//! The Ticket tab as the user sees it (TicketContributorsView.test.ts): the
//! connection states, the toolbar, the Scope panel, the score card, the
//! leaderboard with its member tickets, and the page around it. The rules
//! behind them are tested in `state_tests`; the copy in `ticket::tests`.
use super::{
    TicketEvent,
    backend::TicketBackend,
    fake::{
        Call, TicketFake, assessment, clock, event, now, scope, settings, ticket, ticket_with, user,
    },
    state::{BodyState, Spaces, TicketView},
};
use crate::{
    contributors::{
        ContributorsPage, Event, Tab,
        data_tests::Fake as GithubFake,
        jobs::SyncJobs,
        page_tests::{click, open as open_page, settle, shown},
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
    view.click("ticket-member-a");
    assert!(view.shown("ticket-row-t1"));
    assert!(view.shown("ticket-row-t2"));
    assert!(!view.shown("ticket-row-t3"), "another member's ticket");
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
    assert!(view.shown("ticket-tickets-a"));
    assert!(!view.shown("ticket-reassess"));
}

#[gpui::test]
fn the_reasons_of_a_criterion_open_and_close(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
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
    view.click("ticket-member-a");
    view.click("ticket-select-t1");
    view.click("ticket-reassess");
    assert_eq!(view.fake.count(|c| matches!(c, Call::Assess { .. })), 0);
}

#[gpui::test]
fn a_selection_does_not_survive_collapsing_the_member(cx: &mut TestAppContext) {
    let mut view = open(cx, with_four);
    view.click("ticket-member-a");
    view.click("ticket-select-t1");
    view.click("ticket-member-a");
    view.click("ticket-member-a");
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
    view.click("ticket-member-b");
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
