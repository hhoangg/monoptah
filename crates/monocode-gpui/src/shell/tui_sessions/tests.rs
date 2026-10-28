//! `sessionRemovalTui.test.ts` (kill on close, archive and delete; the chat
//! cleanup is untouched), `projectReturn`'s "a terminal session is never
//! blank", and the shell side of `TuiSessionPane.test.ts` (title, busy, exit
//! close). The web app's PTY handoff cases have no counterpart: a moved pane
//! keeps its entity, which `a_pane_moved_to_another_tab_keeps_its_cli` pins.
use super::*;
use crate::{pane_layout::Edge, tui_session::TuiSession};
use gpui::{TestAppContext, WindowHandle};
use monocode_core::{store::Store, surface::Surface};
use std::time::Duration;

const PROJECT: &str = "/work/monocode";

struct Ids {
    /// A terminal session with conversation `conv-1`.
    tui: i64,
    /// Chat sessions with messages.
    chat: i64,
    other: i64,
}

/// `open` is the sessions in the saved workspace, in tab order.
fn fixture(
    cx: &mut TestAppContext,
    terminal_provider: &str,
    conversation: Option<&str>,
    open: impl Fn(&Ids) -> Vec<i64>,
) -> (WindowHandle<Shell>, Ids) {
    let store = Store::open(std::path::Path::new(":memory:")).unwrap();
    let mut chat = store.create("codex", PROJECT).unwrap();
    let mut other = store.create("codex", PROJECT).unwrap();
    chat.has_messages = true;
    other.has_messages = true;
    let mut tui = store
        .create_with_surface(terminal_provider, PROJECT, Surface::Tui)
        .unwrap();
    if let Some(id) = conversation {
        store.set_terminal_conversation(tui.id, id).unwrap();
        tui.provider_id = Some(id.into());
        tui.has_messages = true;
    }
    let ids = Ids {
        tui: tui.id,
        chat: chat.id,
        other: other.id,
    };
    let mut state = Workspace {
        new_layout_boxy: false,
        ..Default::default()
    };
    state.add_project(PROJECT);
    for id in open(&ids) {
        state.open(id);
    }
    let saved = serde_json::to_string(&state).unwrap();
    let store = Arc::new(Mutex::new(store));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                Shell::new(
                    store,
                    vec![tui, chat, other],
                    PROJECT.into(),
                    Some(saved),
                    cx,
                )
            })
        })
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    (window, ids)
}

fn tui(shell: &Shell, id: i64) -> Entity<TuiSession> {
    shell.tui_sessions[&id].clone()
}
fn terminal(shell: &Shell, id: i64, cx: &App) -> Entity<Terminal> {
    shell.tui_sessions[&id]
        .read(cx)
        .terminal()
        .cloned()
        .unwrap()
}
fn args(shell: &Shell, id: i64, cx: &App) -> Vec<String> {
    terminal(shell, id, cx)
        .read(cx)
        .launched()
        .unwrap()
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}
/// The pane has run for a while, so an exit is not an early one.
fn age(shell: &Shell, id: i64, cx: &mut App) {
    shell.tui_sessions[&id].update(cx, |tui, _| tui.age_launch(Duration::from_secs(10)));
}

#[gpui::test]
fn a_terminal_session_launches_when_shown_and_never_becomes_a_conversation(
    cx: &mut TestAppContext,
) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    window
        .update(cx, |shell, w, cx| {
            // Not shown yet: nothing exists for it, not even an entity, and
            // loading it by id (history, app calls) makes no chat and no pane.
            assert!(shell.tui_sessions.is_empty());
            assert!(shell.ensure_session(ids.tui, cx).is_none());
            let summary = shell.session_summaries[&ids.tui].clone();
            shell.add_loaded(summary, cx);
            assert!(!shell.sessions.contains_key(&ids.tui));
            shell.tui_sessions.clear();
            shell.tui_events.clear();
            assert!(shell.ensure_session(ids.tui, cx).is_none());
            assert!(shell.tui_sessions.is_empty());
            shell.open(ids.tui, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.tui_sessions.contains_key(&ids.tui));
            assert!(!shell.sessions.contains_key(&ids.tui));
            assert!(shell.ensure_session(ids.tui, cx).is_none());
            assert_eq!(shell.workspace.active, Some(ids.tui));
            let args = args(shell, ids.tui, cx);
            let at = args.iter().position(|a| a == "--resume").unwrap();
            assert_eq!(args[at + 1], "conv-1");
            // Loading it by id (history, app calls) must not make a chat either.
            let summary = shell.session_summaries[&ids.tui].clone();
            shell.add_loaded(summary, cx);
            assert!(!shell.sessions.contains_key(&ids.tui));
            assert!(shell.session_marks.contains_key(&ids.tui));
            assert!(shell.tab_marks.contains_key(&ids.tui));
            // The chat in the other tab is unaffected.
            assert!(shell.sessions.contains_key(&ids.chat));
            assert!(!shell.tui_sessions.contains_key(&ids.chat));
        })
        .unwrap();
}

#[gpui::test]
fn a_terminal_session_is_never_blank_so_reusing_a_pane_cannot_kill_its_cli(
    cx: &mut TestAppContext,
) {
    // A provider with no conversation id: the listing flags it as empty.
    let (window, ids) = fixture(cx, "codex", None, |ids| vec![ids.tui]);
    window
        .update(cx, |shell, w, cx| {
            assert!(!shell.session_summaries[&ids.tui].has_messages);
            // Opening a conversation takes over a blank pane, never this one.
            shell.open(ids.chat, w, cx);
            assert!(shell.workspace.tab_of(ids.tui).is_some());
            assert_ne!(
                shell.workspace.tab_of(ids.chat),
                shell.workspace.tab_of(ids.tui)
            );
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.tui_sessions[&ids.tui].read(cx).terminal().is_some());
        })
        .unwrap();
}

#[gpui::test]
fn a_lone_terminal_tab_can_be_closed_though_it_looks_empty(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "codex", None, |ids| vec![ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.visible_tabs(cx).len(), 1);
            assert!(!shell.session_summaries[&ids.tui].has_messages);
            assert!(shell.tab_can_close(ids.tui, cx));
        })
        .unwrap();
}

#[gpui::test]
fn the_project_never_adopts_a_terminal_session_as_its_blank_one(cx: &mut TestAppContext) {
    // Nothing is open, so the shell makes the project a blank chat at startup,
    // with the empty-looking terminal session first in line to be reused.
    let (window, ids) = fixture(cx, "codex", None, |_| vec![]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let active = shell.workspace.active.expect("a blank chat was created");
            assert_ne!(active, ids.tui);
            assert!(shell.sessions.contains_key(&active));
            assert!(shell.tui_sessions.is_empty());
        })
        .unwrap();
}

/// A closed pane's CLI is killed, and reopening the row resumes the conversation.
#[gpui::test]
fn closing_the_tab_kills_the_cli_and_reopening_resumes_it(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    let first = window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.workspace.active, Some(ids.tui));
            let _ = cx;
            tui(shell, ids.tui)
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(first.read(cx).running());
            shell.close(ids.tui, w, cx);
            assert!(first.read(cx).stopped());
            assert!(!shell.tui_sessions.contains_key(&ids.tui));
            // Closing the tab keeps the session: it is still in the list.
            assert!(shell.session_summaries[&ids.tui].has_messages);
            assert!(!shell.session_summaries[&ids.tui].archived);
            shell.open(ids.tui, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let second = tui(shell, ids.tui);
            assert_ne!(second, first);
            let args = args(shell, ids.tui, cx);
            let at = args.iter().position(|a| a == "--resume").unwrap();
            assert_eq!(args[at + 1], "conv-1");
        })
        .unwrap();
}

#[gpui::test]
fn archiving_a_terminal_session_kills_its_cli_and_keeps_the_conversation(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    let session = window
        .update(cx, |shell, _, cx| {
            let session = tui(shell, ids.tui);
            assert!(session.read(cx).running());
            shell.toggle_archive(ids.tui, cx);
            session
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(session.read(cx).stopped());
            assert!(shell.session_summaries[&ids.tui].archived);
            assert!(shell.workspace.tab_of(ids.tui).is_none());
            // The row is saved with its conversation id, which is what lets an
            // archived terminal session be restored.
            let rows = locked(&shell.store).unwrap().sessions().unwrap();
            let row = rows.iter().find(|s| s.id == ids.tui).unwrap();
            assert!(row.archived);
            assert_eq!(row.provider_id.as_deref(), Some("conv-1"));
            assert_eq!(row.surface, Surface::Tui);
        })
        .unwrap();
}

#[gpui::test]
fn deleting_a_terminal_session_kills_its_cli_before_the_row_goes(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    let session = window
        .update(cx, |shell, w, cx| {
            let session = tui(shell, ids.tui);
            assert!(session.read(cx).running());
            shell.delete_session(ids.tui, w, cx);
            // Killed as the pane closes, before the store has deleted anything.
            assert!(session.read(cx).stopped());
            session
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(session.read(cx).stopped());
            assert!(!shell.session_summaries.contains_key(&ids.tui));
            assert!(!shell.tui_sessions.contains_key(&ids.tui));
            let rows = locked(&shell.store).unwrap().sessions().unwrap();
            assert!(rows.iter().all(|s| s.id != ids.tui));
        })
        .unwrap();
}

#[gpui::test]
fn closing_a_chat_leaves_terminal_cleanup_alone(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| {
        vec![ids.chat, ids.other]
    });
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            shell.toggle_archive(ids.chat, cx);
            shell.delete_session(ids.other, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.tui_sessions.is_empty());
            assert!(shell.session_summaries[&ids.chat].archived);
            assert!(!shell.session_summaries.contains_key(&ids.other));
        })
        .unwrap();
}

fn exit_cleanly(window: &WindowHandle<Shell>, cx: &mut TestAppContext, id: i64) {
    window
        .update(cx, |shell, _, cx| {
            age(shell, id, cx);
            let terminal = terminal(shell, id, cx);
            terminal.update(cx, |t, cx| t.fake_exit(Some(0), cx));
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn a_clean_exit_closes_just_the_pane_of_a_split_tab(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    window
        .update(cx, |shell, w, cx| {
            shell.open(ids.tui, w, cx);
            shell.workspace.place(ids.tui, ids.chat, Edge::Right);
            shell.persist(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let tab = shell.workspace.tab_of(ids.tui).unwrap();
            assert!(shell.workspace.layout(tab).is_split());
        })
        .unwrap();
    exit_cleanly(&window, cx, ids.tui);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.workspace.tab_of(ids.tui).is_none());
            assert!(shell.workspace.tab_of(ids.chat).is_some());
            assert!(!shell.tui_sessions.contains_key(&ids.tui));
            // Kept in the history, not archived.
            assert!(shell.session_summaries[&ids.tui].has_messages);
            assert!(!shell.session_summaries[&ids.tui].archived);
        })
        .unwrap();
}

#[gpui::test]
fn a_clean_exit_closes_the_tab_when_there_are_others(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    exit_cleanly(&window, cx, ids.tui);
    window
        .update(cx, |shell, _, _| {
            assert!(shell.workspace.tab_of(ids.tui).is_none());
            assert_eq!(shell.workspace.active, Some(ids.chat));
            assert!(!shell.session_summaries[&ids.tui].archived);
        })
        .unwrap();
}

#[gpui::test]
fn a_clean_exit_turns_the_projects_last_tab_into_a_blank_chat(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
    cx.run_until_parked();
    exit_cleanly(&window, cx, ids.tui);
    // The blank chat is created in the background.
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let active = shell
                .workspace
                .active
                .expect("a blank chat replaced the tab");
            assert_ne!(active, ids.tui);
            assert!(shell.workspace.tab_of(ids.tui).is_none());
            assert!(shell.sessions.contains_key(&active));
            assert!(!shell.session_summaries[&active].has_messages);
            assert!(!shell.session_summaries[&ids.tui].archived);
            assert!(shell.session_summaries[&ids.tui].has_messages);
        })
        .unwrap();
}

#[gpui::test]
fn a_clean_exit_never_replaces_the_last_tab_with_another_terminal(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            // The default provider opens new sessions as a terminal.
            shell
                .workspace
                .preferences
                .provider_surfaces
                .insert("claude".into(), Surface::Tui);
            shell.workspace.preferences.default_provider = Some("claude".into());
            assert_eq!(shell.provider_surface(Kind::Claude), Surface::Tui);
        })
        .unwrap();
    exit_cleanly(&window, cx, ids.tui);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let active = shell
                .workspace
                .active
                .expect("a blank session replaced the tab");
            assert_ne!(active, ids.tui);
            assert_eq!(shell.session_summaries[&active].surface, Surface::Chat);
            assert!(shell.sessions.contains_key(&active));
            // The exited CLI did not come back, and nothing else launched.
            assert!(shell.tui_sessions.is_empty());
            assert!(
                !shell.blank_is_chat,
                "the one-shot flag does not outlive the close"
            );
            // A later, unrelated session follows the preference again.
            shell.create_session(Kind::Claude, false, cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let active = shell.workspace.active.unwrap();
            assert_eq!(shell.session_summaries[&active].surface, Surface::Tui);
        })
        .unwrap();
}

#[gpui::test]
fn an_early_or_failed_exit_keeps_the_pane_for_its_bar(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let terminal = terminal(shell, ids.tui, cx);
            terminal.update(cx, |t, cx| t.fake_exit(Some(0), cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.workspace.tab_of(ids.tui).is_some());
            let exit = shell.tui_sessions[&ids.tui]
                .read(cx)
                .exit()
                .cloned()
                .unwrap();
            assert!(exit.early);
        })
        .unwrap();
}

#[gpui::test]
fn busy_marks_the_session_and_the_cli_exiting_clears_it(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            tui(shell, ids.tui).update(cx, |tui, cx| tui.set_busy(true, cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(shell.busy_sessions.contains(&ids.tui));
            assert!(shell.session_metadata(ids.tui, cx).busy);
            assert!(shell.session_metadata(ids.tui, cx).responding());
            assert!(shell.terminal_busy(ids.tui, cx));
            assert!(shell.tab_marks[&ids.tui].read(cx).state() == MarkState::Working);
            assert!(shell.session_marks[&ids.tui].read(cx).state() == MarkState::Working);
            // A chat in the same shell is not busy.
            assert!(!shell.session_metadata(ids.chat, cx).busy);
            // The CLI exits (early, so the pane stays): idle again.
            terminal(shell, ids.tui, cx).update(cx, |t, cx| t.fake_exit(Some(1), cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.busy_sessions.contains(&ids.tui));
            assert!(!shell.session_metadata(ids.tui, cx).busy);
            assert!(shell.tab_marks[&ids.tui].read(cx).state() == MarkState::Idle);
        })
        .unwrap();
}

#[gpui::test]
fn killing_the_pane_clears_a_busy_mark(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            tui(shell, ids.tui).update(cx, |tui, cx| tui.set_busy(true, cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            assert!(shell.busy_sessions.contains(&ids.tui));
            shell.close(ids.tui, w, cx);
            assert!(!shell.busy_sessions.contains(&ids.tui));
            assert!(!shell.unseen_finished.contains(&ids.tui));
        })
        .unwrap();
}

#[gpui::test]
fn the_cli_titles_a_placeholder_tab_but_not_one_the_user_named(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    let title = |shell: &Shell| shell.session_summaries[&ids.tui].title.clone();
    let stored = |shell: &Shell| {
        locked(&shell.store)
            .unwrap()
            .sessions()
            .unwrap()
            .into_iter()
            .find(|s| s.id == ids.tui)
            .unwrap()
            .title
    };
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(title(shell), "New Claude Code session");
            terminal(shell, ids.tui, cx).update(cx, |t, cx| t.fake_title("✳ Fix the bug", cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(title(shell), "Fix the bug");
            // It keeps following its own title.
            terminal(shell, ids.tui, cx).update(cx, |t, cx| t.fake_title("⠋ Fix the bug now", cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert_eq!(stored(shell), "Fix the bug now");
            // A rename the user typed stays.
            shell.session_summaries.get_mut(&ids.tui).unwrap().title = "My name".into();
            locked(&shell.store)
                .unwrap()
                .title(ids.tui, "My name")
                .unwrap();
            terminal(shell, ids.tui, cx).update(cx, |t, cx| t.fake_title("✳ Other", cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(title(shell), "My name");
            assert_eq!(stored(shell), "My name");
        })
        .unwrap();
}

#[gpui::test]
fn the_sidebar_row_of_a_terminal_session_has_no_preview_card(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    let hover = |shell: &mut Shell, w: &mut Window, cx: &mut Context<Shell>, id| {
        shell.workspace.classic_layout = true;
        shell.close_preview(cx);
        let anchor = AnchorSlot::default();
        anchor.set(Some(Bounds::new(
            w.mouse_position() - gpui::point(px(5.), px(5.)),
            gpui::size(px(20.), px(20.)),
        )));
        shell.hover_session(id, anchor, true, true, w, cx);
    };
    window
        .update(cx, |shell, w, cx| hover(shell, w, cx, ids.other))
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(130));
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(
                shell.session_preview.as_ref().map(|p| p.id),
                Some(ids.other)
            );
        })
        .unwrap();
    window
        .update(cx, |shell, w, cx| hover(shell, w, cx, ids.tui))
        .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(130));
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert!(shell.session_preview.is_none());
            assert!(!shell.sessions.contains_key(&ids.tui));
        })
        .unwrap();
}

#[gpui::test]
fn a_remote_row_never_gets_a_terminal_pane(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    window
        .update(cx, |shell, _, cx| {
            shell.session_summaries.get_mut(&ids.tui).unwrap().cwd =
                "remote://env/home/me/project".into();
            assert!(!shell.is_terminal(ids.tui));
            assert!(shell.ensure_tui(ids.tui, cx).is_none());
            // It loads as the chat it is.
            assert!(shell.ensure_session(ids.tui, cx).is_some());
        })
        .unwrap();
}

#[gpui::test]
fn a_pane_moved_to_another_tab_keeps_its_cli(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    let (session, term) = window
        .update(cx, |shell, _, cx| {
            (tui(shell, ids.tui), terminal(shell, ids.tui, cx))
        })
        .unwrap();
    // An exited pane keeps its exit through the move too.
    window
        .update(cx, |shell, _, cx| {
            term.update(cx, |t, cx| t.fake_exit(Some(1), cx));
            shell.workspace.place(ids.tui, ids.chat, Edge::Right);
            shell.persist(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let tab = shell.workspace.tab_of(ids.tui).unwrap();
            assert!(shell.workspace.layout(tab).is_split());
            assert_eq!(tui(shell, ids.tui), session);
            assert!(!session.read(cx).stopped());
            assert_eq!(session.read(cx).terminal(), Some(&term));
            assert!(session.read(cx).exit().is_some());
        })
        .unwrap();
}

fn terminal_focused(window: &WindowHandle<Shell>, cx: &mut TestAppContext, id: i64) -> bool {
    window
        .update(cx, |shell, w, cx| {
            terminal(shell, id, cx)
                .read(cx)
                .focus_handle(cx)
                .is_focused(w)
        })
        .unwrap()
}

#[gpui::test]
fn opening_a_terminal_row_focuses_its_cli_once_it_runs(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    window
        .update(cx, |shell, w, cx| shell.open(ids.tui, w, cx))
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    assert!(terminal_focused(&window, cx, ids.tui));
    // Opening a chat moves focus away; coming back focuses the CLI again.
    window
        .update(cx, |shell, w, cx| shell.open(ids.chat, w, cx))
        .unwrap();
    cx.run_until_parked();
    assert!(!terminal_focused(&window, cx, ids.tui));
    window
        .update(cx, |shell, w, cx| shell.open(ids.tui, w, cx))
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    assert!(terminal_focused(&window, cx, ids.tui));
}

#[gpui::test]
fn restoring_the_active_terminal_tab_focuses_its_cli(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    cx.refresh().unwrap();
    // Focus follows the restored pane on its own...
    assert!(terminal_focused(&window, cx, ids.tui));
    // ...and the launch's explicit `focus_active` keeps it there.
    window
        .update(cx, |shell, w, cx| shell.focus_active(w, cx))
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    assert!(terminal_focused(&window, cx, ids.tui));
}

#[gpui::test]
fn activating_a_terminal_pane_of_a_split_focuses_its_cli(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    window
        .update(cx, |shell, w, cx| {
            shell.open(ids.tui, w, cx);
            shell.workspace.place(ids.tui, ids.chat, Edge::Right);
            shell.focus_pane(ids.chat, w, cx);
            shell.persist(cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    assert!(!terminal_focused(&window, cx, ids.tui));
    window
        .update(cx, |shell, w, cx| shell.focus_pane(ids.tui, w, cx))
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    assert!(terminal_focused(&window, cx, ids.tui));
}

#[gpui::test]
fn a_background_exit_does_not_pull_the_user_out_of_an_app_view(cx: &mut TestAppContext) {
    // Active pane of a split, with the Inbox open over the workspace.
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    window
        .update(cx, |shell, w, cx| {
            shell.open(ids.tui, w, cx);
            shell.workspace.place(ids.tui, ids.chat, Edge::Right);
            shell.persist(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.workspace.active, Some(ids.tui));
            shell.active_app_view = Some(app_views::AppView::Inbox);
        })
        .unwrap();
    exit_cleanly(&window, cx, ids.tui);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.active_app_view, Some(app_views::AppView::Inbox));
            assert!(shell.workspace.tab_of(ids.tui).is_none());
            assert!(shell.workspace.tab_of(ids.chat).is_some());
            assert!(!shell.tui_sessions.contains_key(&ids.tui));
        })
        .unwrap();
}

#[gpui::test]
fn a_background_exit_of_a_lone_tab_leaves_the_app_view_open(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            shell.active_app_view = Some(app_views::AppView::Inbox);
        })
        .unwrap();
    exit_cleanly(&window, cx, ids.tui);
    window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.active_app_view, Some(app_views::AppView::Inbox));
            assert!(shell.workspace.tab_of(ids.tui).is_none());
        })
        .unwrap();
}

#[gpui::test]
fn an_automation_never_continues_a_terminal_session(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
    let automation = |harness: &str| -> monocode_core::automations::Automation {
        serde_json::from_value(serde_json::json!({
            "id": "a", "name": "n", "prompt": "p", "harness": harness, "model": "",
            "cwd": PROJECT, "workspaceMode": "current", "reuseSession": true,
            "runtimeMode": "x", "scheduleKind": "daily", "minute": 0, "time": "09:00",
            "dayOfWeek": 0, "missedRunGraceMinutes": 0, "enabled": true, "nextRunAt": 0,
            "createdAt": 0, "updatedAt": 0
        }))
        .unwrap()
    };
    window
        .update(cx, |shell, _, cx| {
            // A chat of the same provider and project may be continued...
            let chat = shell.store.clone();
            let mut sibling = locked(&chat).unwrap().create("claude", PROJECT).unwrap();
            sibling.has_messages = true;
            let id = sibling.id;
            shell.session_summaries.insert(id, sibling);
            assert!(shell.automation_can_continue(id, &automation("claude"), cx));
            // ...a terminal session of the same provider and project may not.
            assert_eq!(shell.session_summaries[&ids.tui].provider, "claude");
            assert!(!shell.automation_can_continue(ids.tui, &automation("claude"), cx));
        })
        .unwrap();
}

// The hook busy watcher (`shell/tui_busy.rs`). Events are spool files written the
// way the `tui-hook` subcommand writes them.
mod busy_watcher {
    use super::*;
    use crate::shell::tui_busy::{self, POLL};
    use std::{fs, path::Path, time::SystemTime};

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
    fn spool(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monocode-tui-busy-{tag}-{}-{}",
            std::process::id(),
            monocode_core::mono::new_id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn write(dir: &Path, n: u32, id: &str, busy: bool, at_ms: u64) {
        let record = format!(r#"{{"providerSessionId":"{id}","busy":{busy},"atMs":{at_ms}}}"#);
        fs::write(dir.join(format!("{n:020}-0000000001.json")), record).unwrap();
    }
    fn files(dir: &Path) -> usize {
        fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }
    fn watch(window: &WindowHandle<Shell>, dir: &Path, cx: &mut TestAppContext) {
        let shell = window.update(cx, |_, _, cx| cx.weak_entity()).unwrap();
        cx.update(|cx| tui_busy::register(dir.to_path_buf(), shell, cx));
        cx.run_until_parked();
    }
    fn tick(cx: &mut TestAppContext) {
        cx.run_until_parked();
        cx.executor().advance_clock(POLL);
        cx.run_until_parked();
    }
    fn busy(window: &WindowHandle<Shell>, id: i64, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(
                    shell.busy_sessions.contains(&id),
                    shell.terminal_busy(id, cx)
                );
                shell.terminal_busy(id, cx)
            })
            .unwrap()
    }

    #[gpui::test]
    fn a_hook_event_marks_the_open_session_busy_then_idle(cx: &mut TestAppContext) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
        let dir = spool("busy-idle");
        watch(&window, &dir, cx);
        assert!(!busy(&window, ids.tui, cx));
        write(&dir, 1, "conv-1", true, now_ms());
        tick(cx);
        assert!(busy(&window, ids.tui, cx));
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.session_metadata(ids.tui, cx).busy);
                assert!(shell.tab_marks[&ids.tui].read(cx).state() == MarkState::Working);
                // The chat beside it is untouched.
                assert!(!shell.session_metadata(ids.chat, cx).busy);
            })
            .unwrap();
        assert_eq!(files(&dir), 0, "events are consumed");
        write(&dir, 2, "conv-1", false, now_ms());
        tick(cx);
        assert!(!busy(&window, ids.tui, cx));
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.tab_marks[&ids.tui].read(cx).state() != MarkState::Working);
            })
            .unwrap();
    }

    #[gpui::test]
    fn events_in_one_batch_apply_in_order(cx: &mut TestAppContext) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let dir = spool("order");
        watch(&window, &dir, cx);
        write(&dir, 1, "conv-1", false, now_ms());
        write(&dir, 2, "conv-1", true, now_ms());
        tick(cx);
        assert!(busy(&window, ids.tui, cx));
    }

    #[gpui::test]
    fn an_unknown_conversation_id_is_dropped(cx: &mut TestAppContext) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let dir = spool("unknown");
        watch(&window, &dir, cx);
        write(&dir, 1, "someone-else", true, now_ms());
        tick(cx);
        assert!(!busy(&window, ids.tui, cx));
        assert_eq!(files(&dir), 0, "dropped, not kept for later");
        // It does not come back when a session with that id appears.
        tick(cx);
        assert!(!busy(&window, ids.tui, cx));
    }

    #[gpui::test]
    fn a_session_that_is_not_open_is_not_touched(cx: &mut TestAppContext) {
        // The terminal row exists but its tab is closed, so it has no pane.
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat]);
        let dir = spool("closed");
        watch(&window, &dir, cx);
        write(&dir, 1, "conv-1", true, now_ms());
        tick(cx);
        window
            .update(cx, |shell, _, _| {
                assert!(shell.tui_sessions.is_empty());
                assert!(!shell.busy_sessions.contains(&ids.tui));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_stale_busy_event_is_ignored(cx: &mut TestAppContext) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let dir = spool("stale");
        watch(&window, &dir, cx);
        write(&dir, 1, "conv-1", true, now_ms() - 60_000);
        tick(cx);
        assert!(!busy(&window, ids.tui, cx));
        assert_eq!(files(&dir), 0);
    }

    #[gpui::test]
    fn the_cli_exiting_clears_a_hook_busy_and_a_late_event_cannot_set_it_again(
        cx: &mut TestAppContext,
    ) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let dir = spool("exit");
        watch(&window, &dir, cx);
        write(&dir, 1, "conv-1", true, now_ms());
        tick(cx);
        assert!(busy(&window, ids.tui, cx));
        window
            .update(cx, |shell, _, cx| {
                terminal(shell, ids.tui, cx).update(cx, |t, cx| t.fake_exit(Some(1), cx));
            })
            .unwrap();
        cx.run_until_parked();
        assert!(!busy(&window, ids.tui, cx));
        write(&dir, 2, "conv-1", true, now_ms());
        tick(cx);
        assert!(!busy(&window, ids.tui, cx));
    }

    #[gpui::test]
    fn busy_is_not_saved(cx: &mut TestAppContext) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let dir = spool("persist");
        watch(&window, &dir, cx);
        write(&dir, 1, "conv-1", true, now_ms());
        tick(cx);
        assert!(busy(&window, ids.tui, cx));
        window
            .update(cx, |shell, _, _| {
                let saved = serde_json::to_string(&shell.workspace).unwrap();
                assert!(!saved.contains("busy"), "{saved}");
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_spool_is_pruned_at_boot_before_the_first_drain(cx: &mut TestAppContext) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        window
            .update(cx, |shell, _, cx| {
                tui(shell, ids.tui).update(cx, |t, cx| t.set_busy(true, cx));
            })
            .unwrap();
        let dir = spool("boot");
        // Written by a run long ago: an idle event applies at any age, so only
        // the prune keeps it from ending this turn.
        write(&dir, 1, "conv-1", false, now_ms());
        let old = SystemTime::now() - std::time::Duration::from_secs(120);
        fs::File::options()
            .write(true)
            .open(dir.join(format!("{:020}-0000000001.json", 1)))
            .unwrap()
            .set_modified(old)
            .unwrap();
        watch(&window, &dir, cx);
        tick(cx);
        assert_eq!(files(&dir), 0);
        assert!(busy(&window, ids.tui, cx), "the old event never applied");
        // A fresh idle event still does.
        write(&dir, 2, "conv-1", false, now_ms());
        tick(cx);
        assert!(!busy(&window, ids.tui, cx));
    }

    #[gpui::test]
    fn an_event_reaches_only_the_window_that_owns_the_session(cx: &mut TestAppContext) {
        let (first, a) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let (second, b) = fixture(cx, "claude", Some("conv-2"), |ids| vec![ids.tui]);
        let dir = spool("windows");
        watch(&first, &dir, cx);
        watch(&second, &dir, cx);
        write(&dir, 1, "conv-2", true, now_ms());
        tick(cx);
        assert!(!busy(&first, a.tui, cx));
        assert!(busy(&second, b.tui, cx));
        write(&dir, 2, "conv-1", true, now_ms());
        tick(cx);
        assert!(busy(&first, a.tui, cx));
        assert!(busy(&second, b.tui, cx));
    }

    #[gpui::test]
    fn a_closed_window_is_skipped(cx: &mut TestAppContext) {
        let (first, _) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let (second, b) = fixture(cx, "claude", Some("conv-2"), |ids| vec![ids.tui]);
        let dir = spool("closed-window");
        watch(&first, &dir, cx);
        watch(&second, &dir, cx);
        cx.update(|cx| {
            first
                .update(cx, |_, window, _| window.remove_window())
                .unwrap()
        });
        cx.run_until_parked();
        write(&dir, 1, "conv-2", true, now_ms());
        tick(cx);
        assert!(busy(&second, b.tui, cx));
    }

    #[gpui::test]
    fn a_missing_spool_directory_is_left_alone(cx: &mut TestAppContext) {
        let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.tui]);
        let dir = spool("missing").join("not-there");
        watch(&window, &dir, cx);
        for _ in 0..3 {
            tick(cx);
        }
        assert!(!dir.exists(), "the watcher never creates the spool");
        assert!(!busy(&window, ids.tui, cx));
        // Once a hook creates it, events flow.
        fs::create_dir_all(&dir).unwrap();
        write(&dir, 1, "conv-1", true, now_ms());
        tick(cx);
        assert!(busy(&window, ids.tui, cx));
    }
}

// Quit (inFlight.test.ts: quitWhileBusyMessage, the terminal-only cases; the chat
// cases are not ported because native has no chat-turn quit confirmation).
#[test]
fn quitting_with_a_busy_terminal_says_it_will_stop_with_its_conversation_saved() {
    assert_eq!(
        quit_while_busy_message(1),
        "1 terminal turn is still running. Quit anyway? It will stop, though its conversation is saved."
    );
    assert_eq!(
        quit_while_busy_message(2),
        "2 terminal turns are still running. Quit anyway? They will stop, though their conversations are saved."
    );
}

#[test]
fn the_quit_prompt_is_one_dialog_for_whatever_would_be_lost() {
    assert!(quit_prompt(0, false).is_none());
    let unsaved = quit_prompt(0, true).unwrap();
    assert_eq!(unsaved.message, "Quit with unsaved file edits?");
    assert_eq!(
        unsaved.detail.as_deref(),
        Some("Save your files before quitting to keep your changes.")
    );
    assert_eq!(unsaved.confirm, "Quit Without Saving");
    let busy = quit_prompt(2, false).unwrap();
    assert_eq!(busy.message, quit_while_busy_message(2));
    assert_eq!(busy.detail, None);
    assert_eq!(busy.confirm, "Quit");
    // Both at once: one dialog that names both, never two in a row.
    let both = quit_prompt(1, true).unwrap();
    assert_eq!(both.message, quit_while_busy_message(1));
    assert_eq!(
        both.detail.as_deref(),
        Some("Unsaved file edits will also be lost. Save your files first to keep them.")
    );
    assert_eq!(both.confirm, "Quit Without Saving");
}

#[gpui::test]
fn only_a_busy_terminal_counts_toward_quitting(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            // A running but idle terminal, and a chat, lose nothing.
            assert!(shell.tui_sessions[&ids.tui].read(cx).running());
            assert_eq!(shell.busy_terminals(cx), 0);
            assert!(shell.quit_prompt(cx).is_none());
            assert!(!shell.close_needs_confirmation(cx));
            tui(shell, ids.tui).update(cx, |tui, cx| tui.set_busy(true, cx));
            assert_eq!(shell.busy_terminals(cx), 1);
            assert_eq!(
                shell.quit_prompt(cx).unwrap().message,
                quit_while_busy_message(1)
            );
            assert!(shell.close_needs_confirmation(cx));
        })
        .unwrap();
    // The CLI dying ends its turn: nothing to confirm any more.
    window
        .update(cx, |shell, _, cx| {
            terminal(shell, ids.tui, cx).update(cx, |t, cx| t.fake_exit(Some(1), cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| assert_eq!(shell.busy_terminals(cx), 0))
        .unwrap();
}

#[gpui::test]
fn quit_asks_before_stopping_a_busy_terminal_and_cancel_keeps_the_app(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, w, cx| {
            tui(shell, ids.tui).update(cx, |tui, cx| tui.set_busy(true, cx));
            shell.request_quit(w, cx);
            assert!(shell.quit_prompted);
            // A second request while the dialog is up asks nothing more.
            shell.request_quit(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!cx.has_pending_prompt());
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.quit_prompted);
            // Still running: Cancel stopped nothing.
            assert!(shell.tui_sessions[&ids.tui].read(cx).running());
            assert!(shell.terminal_busy(ids.tui, cx));
        })
        .unwrap();
}

#[gpui::test]
fn closing_the_window_asks_while_a_terminal_turn_runs(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            tui(shell, ids.tui).update(cx, |tui, cx| tui.set_busy(true, cx));
        })
        .unwrap();
    // The title bar's close button goes through the window's should-close hook.
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    assert!(
        !visual.simulate_close(),
        "the window stays while the dialog is up"
    );
    visual.run_until_parked();
    assert!(visual.has_pending_prompt());
    visual.simulate_prompt_answer("Cancel");
    visual.run_until_parked();
    assert!(!visual.has_pending_prompt());
}

#[gpui::test]
fn the_trays_quit_asks_about_a_busy_terminal_instead_of_quitting(cx: &mut TestAppContext) {
    let (window, ids) = fixture(cx, "claude", Some("conv-1"), |ids| vec![ids.chat, ids.tui]);
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            tui(shell, ids.tui).update(cx, |tui, cx| tui.set_busy(true, cx));
            shell
                .mono_manager()
                .update(cx, |monos, cx| monos.request_quit(cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "the tray item goes through request_quit"
    );
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            assert!(!shell.quit_prompted);
            assert!(shell.tui_sessions[&ids.tui].read(cx).running());
        })
        .unwrap();
}
