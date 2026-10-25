//! AskAgentDialog.test.ts: the dialog's rules (chips, options, scope subtitle,
//! suggestions, the question, starting and its errors, the provider) in a
//! window over the real page, with the fake backend. The shell side (the chat
//! it starts) is tested in `shell/contributors_tests.rs`.
use super::{
    AskChoices, ContributorsPage, Event, GithubView,
    ask::{
        AskDialog, FALLBACK_ERROR, attached_hint, can_start, error_text, part_of, preselect,
        toggle_part,
    },
    data_tests::{Fake, raw, start_of_tests, synced},
    page_tests::{Ui, click, github, settle, shown},
};
use crate::input;
use gpui::{Entity, Focusable as _, TestAppContext, VisualTestContext};
use monocode_core::contributors::{
    GitIdentity,
    model::{
        ask::{AskAgentRequest, AskChipId, AskContextPart},
        snapshot::SnapshotScope,
    },
};
use monocode_core::provider::Kind;
use std::{cell::RefCell, rc::Rc, sync::Arc};

type Started = Rc<RefCell<Vec<(String, AskAgentRequest, u64)>>>;

struct Rig {
    ui: Ui,
    v: VisualTestContext,
    page: Entity<ContributorsPage>,
    view: Entity<GithubView>,
    started: Started,
    closed: Rc<std::cell::Cell<u32>>,
    _subscription: gpui::Subscription,
}

fn both() -> AskChoices {
    AskChoices {
        providers: vec![Kind::Claude, Kind::Codex],
        default: Some(Kind::Codex),
    }
}

/// Ana and Bob, a few commits each.
fn team() -> Arc<Fake> {
    let fake = Fake::new();
    let mut response = synced("h1");
    let now = start_of_tests();
    let mut data = raw("h1", &now);
    let bob = GitIdentity {
        name: "Bob".into(),
        email: "bob@example.com".into(),
        login: None,
    };
    for hash in ["b1", "b2"] {
        let mut commit = data.commits[0].clone();
        commit.hash = hash.into();
        commit.author = bob.clone();
        commit.committer = bob.clone();
        data.commits.push(commit);
    }
    response.raw = data;
    fake.set_load(response);
    fake
}

fn rig(cx: &mut TestAppContext, fake: Arc<Fake>, choices: AskChoices) -> Rig {
    cx.update(input::bind);
    let (ui, mut v) = github(cx, fake);
    let page = ui.window.root(&mut v).expect("the page");
    let view = ui.view(&mut v);
    let started: Started = Rc::default();
    let log = started.clone();
    let closed = Rc::new(std::cell::Cell::new(0));
    let count = closed.clone();
    let subscription = v.update(|_, cx| {
        cx.subscribe(&page, move |page, event: &Event, cx| match event {
            Event::Ask {
                project,
                scope,
                view,
            } => page.update(cx, |page, cx| {
                page.open_ask(
                    project.clone(),
                    scope.clone(),
                    view.clone(),
                    choices.clone(),
                    cx,
                )
            }),
            Event::StartSession {
                project,
                request,
                start,
            } => log
                .borrow_mut()
                .push((project.clone(), (**request).clone(), *start)),
            Event::Close => count.set(count.get() + 1),
            Event::OpenSettings => {}
        })
    });
    Rig {
        ui,
        v,
        page,
        view,
        started,
        closed,
        _subscription: subscription,
    }
}

impl Rig {
    fn open(&mut self, scope: SnapshotScope) {
        let view = self.view.clone();
        view.update(&mut self.v, |view, cx| assert!(view.open_ask(scope, cx)));
        settle(&mut self.v);
    }

    fn dialog(&mut self) -> Option<Entity<AskDialog>> {
        let page = self.page.clone();
        self.v.read(|cx| page.read(cx).ask_dialog())
    }

    fn dialog_now(&mut self) -> Entity<AskDialog> {
        self.dialog().expect("the dialog is open")
    }

    /// The people of the data on screen: (id, name).
    fn people(&mut self) -> Vec<(String, String)> {
        let view = self.view.clone();
        self.v.read(|cx| {
            let (model, _) = view.read(cx).ask_inputs().expect("a model");
            model
                .stats
                .rows
                .iter()
                .map(|row| (row.person_id.clone(), row.name.clone()))
                .collect()
        })
    }

    fn person(&mut self, name: &str) -> String {
        self.people()
            .into_iter()
            .find(|(_, n)| n == name)
            .unwrap_or_else(|| panic!("no {name} in {:?}", self.people()))
            .0
    }

    fn type_text(&mut self, text: &str) {
        let input = self
            .dialog_now()
            .read_with(&self.v, |d, _| d.question_input());
        input.update(&mut self.v, |input, cx| {
            input.set_text(text.to_string(), cx)
        });
        settle(&mut self.v);
    }

    fn start(&mut self) -> (AskAgentRequest, serde_json::Value) {
        click(&mut self.v, "ask-start");
        let request = self.started.borrow().last().expect("a start").1.clone();
        let snapshot = serde_json::from_str(&request.snapshot_json).unwrap();
        (request, snapshot)
    }

    /// An Escape or a Cmd/Ctrl+Enter reaches the focused control.
    fn keys(&mut self, keys: &str) {
        self.v.simulate_keystrokes(keys);
        settle(&mut self.v);
    }

    /// The answer to the latest start.
    fn finished(&mut self, result: Result<(), String>) {
        let start = self.started.borrow().last().expect("a start").2;
        self.finished_for(start, result);
    }

    fn finished_for(&mut self, start: u64, result: Result<(), String>) {
        let page = self.page.clone();
        page.update(&mut self.v, |page, cx| page.ask_finished(start, result, cx));
        settle(&mut self.v);
    }

    fn shown(&mut self, selector: &'static str) -> bool {
        shown(&mut self.v, selector)
    }
}

const SUBMIT: &str = if cfg!(target_os = "macos") {
    "cmd-enter"
} else {
    "ctrl-enter"
};

// ---- pure rules -----------------------------------------------------------

#[test]
fn only_the_data_chips_stand_for_a_part_of_the_snapshot() {
    assert_eq!(part_of(AskChipId::People), Some(AskContextPart::People));
    assert_eq!(part_of(AskChipId::Scores), Some(AskContextPart::Scores));
    assert_eq!(part_of(AskChipId::Formula), Some(AskContextPart::Formula));
    assert_eq!(part_of(AskChipId::Ai), Some(AskContextPart::Ai));
    for id in [
        AskChipId::Project,
        AskChipId::Branch,
        AskChipId::Period,
        AskChipId::Focus,
    ] {
        assert_eq!(part_of(id), None, "{id:?}");
    }
}

#[test]
fn a_chip_is_removed_by_one_press_and_added_back_by_the_next() {
    let mut omit = Vec::new();
    toggle_part(&mut omit, AskContextPart::Formula);
    toggle_part(&mut omit, AskContextPart::Ai);
    assert_eq!(omit, [AskContextPart::Formula, AskContextPart::Ai]);
    toggle_part(&mut omit, AskContextPart::Formula);
    assert_eq!(omit, [AskContextPart::Ai]);
}

#[test]
fn a_session_needs_a_question_and_no_start_in_flight() {
    assert!(!can_start("", false));
    assert!(!can_start("   \n\t", false));
    assert!(!can_start("\u{feff}", false), "JS trim drops a BOM");
    assert!(can_start(" hello ", false));
    assert!(!can_start("hello", true));
}

#[test]
fn the_default_provider_is_preselected_when_offered_else_the_first() {
    let choices = |providers: &[Kind], default| AskChoices {
        providers: providers.to_vec(),
        default,
    };
    let both = [Kind::Claude, Kind::Codex];
    assert_eq!(
        preselect(&choices(&both, Some(Kind::Codex))),
        Some(Kind::Codex)
    );
    assert_eq!(preselect(&choices(&both, None)), Some(Kind::Claude));
    // A default that is hidden or not installed is not offered.
    assert_eq!(
        preselect(&choices(&both, Some(Kind::Cursor))),
        Some(Kind::Claude)
    );
    assert_eq!(preselect(&choices(&[], Some(Kind::Codex))), None);
}

#[test]
fn a_blank_failure_gets_the_generic_line_and_others_are_kept() {
    assert_eq!(error_text(""), "Could not start the session.");
    assert_eq!(error_text("  \n"), FALLBACK_ERROR);
    assert_eq!(
        error_text(" No provider is available "),
        "No provider is available"
    );
}

#[test]
fn the_hint_names_the_snapshot_file() {
    assert_eq!(
        attached_hint("contributors-2026-10.json"),
        "Opens a new session with contributors-2026-10.json attached"
    );
}

// ---- header and context ---------------------------------------------------

#[gpui::test]
fn it_is_a_titled_dialog_with_the_scope_as_its_description(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    assert!(!rig.shown("ask-dialog"));
    click(&mut rig.v, "contributors-ask");
    assert!(rig.shown("ask-dialog"));
    assert!(rig.shown("ask-subtitle"));
    let dialog = rig.dialog_now();
    let (_, _, subtitle) = dialog.update(&mut rig.v, |d, cx| d.shown(cx)).unwrap();
    assert_eq!(subtitle, "about the whole team");
    let input = dialog.read_with(&rig.v, |d, _| d.question_input());
    assert_eq!(
        input.read_with(&rig.v, |i, _| i.placeholder().to_string()),
        "Ask anything about this data..."
    );
}

#[gpui::test]
fn it_says_about_your_score_for_me_and_the_name_for_a_person(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    let (ana, bob) = (rig.person("Ana"), rig.person("Bob"));
    rig.open(SnapshotScope::Me { person_id: ana });
    let dialog = rig.dialog_now();
    let (chips, _, subtitle) = dialog.update(&mut rig.v, |d, cx| d.shown(cx)).unwrap();
    assert_eq!(subtitle, "about your score");
    let focus = chips.iter().find(|c| c.id == AskChipId::Focus).unwrap();
    assert!(focus.label.contains("Ana (you)"), "{}", focus.label);
    // A second dialog for a person, after the first is closed.
    rig.keys("escape");
    assert!(rig.dialog().is_none());
    rig.open(SnapshotScope::Person { person_id: bob });
    let dialog = rig.dialog_now();
    let (chips, _, subtitle) = dialog.update(&mut rig.v, |d, cx| d.shown(cx)).unwrap();
    assert_eq!(subtitle, "about Bob");
    let by = |id| chips.iter().find(|c| c.id == id).unwrap();
    assert!(by(AskChipId::Focus).label.contains("Bob (focus)"));
    assert!(by(AskChipId::People).label.contains("Whole team"));
    assert!(by(AskChipId::People).removable);
    assert!(rig.shown("ask-chip-toggle-people"));
}

#[gpui::test]
fn only_the_data_chips_can_be_removed(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    for id in ["project", "branch", "period"] {
        let chip: &'static str = Box::leak(format!("ask-chip-{id}").into_boxed_str());
        let toggle: &'static str = Box::leak(format!("ask-chip-toggle-{id}").into_boxed_str());
        assert!(rig.shown(chip), "{id}");
        assert!(!rig.shown(toggle), "{id}");
    }
    for selector in [
        "ask-chip-toggle-people",
        "ask-chip-toggle-scores",
        "ask-chip-toggle-formula",
        "ask-chip-toggle-ai",
    ] {
        assert!(rig.shown(selector), "{selector}");
    }
    let dialog = rig.dialog_now();
    let (chips, _, _) = dialog.update(&mut rig.v, |d, cx| d.shown(cx)).unwrap();
    let project = chips.iter().find(|c| c.id == AskChipId::Project).unwrap();
    assert!(project.label.contains("alpha"), "{}", project.label);
}

#[gpui::test]
fn the_hint_names_the_snapshot_file_of_the_period(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    assert!(rig.shown("ask-hint"));
    let dialog = rig.dialog_now();
    let file = dialog
        .update(&mut rig.v, |d, cx| d.shown(cx))
        .map(|(chips, ..)| chips.len());
    assert!(file.is_some());
    rig.type_text("q");
    let (request, _) = rig.start();
    assert!(request.file_name.starts_with("contributors-"));
    assert!(request.file_name.ends_with(".json"));
}

// ---- snapshot options -----------------------------------------------------

#[gpui::test]
fn it_sends_everything_by_default(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    let options = rig
        .dialog_now()
        .read_with(&rig.v, |d, _| d.options().clone());
    assert!(options.per_pr);
    assert!(!options.raw_commits);
    assert!(options.allow_tools);
    assert!(options.omit.is_empty());
    rig.type_text("Who drove this?");
    let (request, snapshot) = rig.start();
    assert!(snapshot.get("formula").is_some());
    assert!(snapshot.get("prs").is_some());
    assert!(snapshot.get("commits").is_none());
    assert!(snapshot["people"][0].get("aiAssistedPct").is_some());
    assert!(snapshot["people"][0].get("parts").is_some());
    assert!(request.prompt.contains("read-only git and gh"));
}

#[gpui::test]
fn removing_the_formula_chip_removes_the_formula(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-chip-toggle-formula");
    rig.type_text("q");
    let (_, snapshot) = rig.start();
    assert!(snapshot.get("formula").is_none());
    assert!(snapshot["people"][0].get("parts").is_some());
}

#[gpui::test]
fn removing_the_ai_chip_removes_those_fields(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-chip-toggle-ai");
    rig.type_text("q");
    let (_, snapshot) = rig.start();
    let people = snapshot["people"].as_array().unwrap();
    assert!(people.iter().all(|p| p.get("aiAssistedPct").is_none()));
    assert!(snapshot.get("formula").is_some());
}

#[gpui::test]
fn removing_the_scores_chip_keeps_score_and_rank_but_not_the_breakdown(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-chip-toggle-scores");
    rig.type_text("q");
    let (_, snapshot) = rig.start();
    assert!(snapshot["people"][0].get("parts").is_none());
    assert!(snapshot["people"][0].get("breakdown").is_none());
    assert!(snapshot["people"][0].get("score").is_some());
}

#[gpui::test]
fn removing_the_people_chip_drops_identities(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-chip-toggle-people");
    rig.type_text("q");
    let (_, snapshot) = rig.start();
    let people = snapshot["people"].as_array().unwrap();
    assert!(people.iter().all(|p| p.get("identities").is_none()));
}

#[gpui::test]
fn removing_the_people_chip_leaves_no_email_in_the_snapshot_or_prompt(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    let bob = rig.person("Bob");
    let scope = SnapshotScope::Person {
        person_id: bob.clone(),
    };
    rig.open(scope.clone());
    click(&mut rig.v, "ask-chip-toggle-people");
    click(&mut rig.v, "ask-option-raw-commits");
    rig.type_text("q");
    let (request, snapshot) = rig.start();
    assert!(!request.snapshot_json.contains('@'));
    assert!(!request.prompt.contains('@'));
    let id = snapshot["scope"]["personId"].as_str().unwrap();
    assert!(id.starts_with("person-") && id[7..].chars().all(|c| c.is_ascii_digit()));
    assert_eq!(request.scope, scope, "the request keeps the real scope");
}

#[gpui::test]
fn a_removed_chip_can_be_added_back(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-chip-toggle-formula");
    let omitted = rig
        .dialog_now()
        .read_with(&rig.v, |d, _| d.options().omit.clone());
    assert_eq!(omitted, [AskContextPart::Formula]);
    click(&mut rig.v, "ask-chip-toggle-formula");
    let omitted = rig
        .dialog_now()
        .read_with(&rig.v, |d, _| d.options().omit.clone());
    assert!(omitted.is_empty());
    rig.type_text("q");
    let (_, snapshot) = rig.start();
    assert!(snapshot.get("formula").is_some());
}

#[gpui::test]
fn per_pr_off_drops_the_prs_and_raw_commits_on_adds_them(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-option-per-pr");
    click(&mut rig.v, "ask-option-raw-commits");
    rig.type_text("q");
    let (_, snapshot) = rig.start();
    assert!(snapshot.get("prs").is_none());
    assert!(!snapshot["commits"].as_array().unwrap().is_empty());
}

#[gpui::test]
fn turning_tools_off_changes_the_prompt_wording(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-option-tools");
    rig.type_text("q");
    let (request, _) = rig.start();
    assert!(request.prompt.contains("Do not run commands"));
    assert!(!request.prompt.contains("read-only git and gh"));
}

#[gpui::test]
fn it_passes_the_scope_the_file_name_and_the_trimmed_question_through(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    let scope = SnapshotScope::Person {
        person_id: rig.person("Bob"),
    };
    rig.open(scope.clone());
    rig.type_text("  Why is Bob low?  ");
    let (request, snapshot) = rig.start();
    assert_eq!(request.scope, scope);
    assert_eq!(request.question, "Why is Bob low?");
    assert!(request.file_name.starts_with("contributors-"));
    assert_eq!(snapshot["focus"]["name"], "Bob");
    assert_eq!(rig.started.borrow()[0].0, "/work/alpha");
}

// ---- suggestions and the question -----------------------------------------

#[gpui::test]
fn a_suggestion_fills_the_question(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    let dialog = rig.dialog_now();
    let (_, suggestions, _) = dialog.update(&mut rig.v, |d, cx| d.shown(cx)).unwrap();
    assert!(!suggestions.is_empty());
    assert!(rig.shown("ask-suggestion-0"));
    click(&mut rig.v, "ask-suggestion-1");
    assert_eq!(
        dialog.read_with(&rig.v, |d, cx| d.question_text(cx)),
        suggestions[1]
    );
}

#[gpui::test]
fn suggestions_differ_per_scope(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    let (ana, bob) = (rig.person("Ana"), rig.person("Bob"));
    let mut asked = Vec::new();
    for scope in [
        SnapshotScope::Team,
        SnapshotScope::Me { person_id: ana },
        SnapshotScope::Person { person_id: bob },
    ] {
        rig.open(scope);
        let dialog = rig.dialog_now();
        let (_, suggestions, _) = dialog.update(&mut rig.v, |d, cx| d.shown(cx)).unwrap();
        asked.push(suggestions);
        rig.keys("escape");
    }
    assert!(
        asked[0].contains(&"Which modules depend on a single person (bus factor)?".to_string())
    );
    assert!(asked[1].contains(&"What would raise my Quality factor?".to_string()));
    assert!(!asked[1].contains(&asked[0][0]));
    assert!(asked[2].contains(&"Summarize Bob's contributions this period.".to_string()));
}

#[gpui::test]
fn all_time_leaves_out_the_questions_about_a_previous_period(cx: &mut TestAppContext) {
    let mut rig = rig(cx, team(), both());
    click(&mut rig.v, "contributors-period-all");
    let ana = rig.person("Ana");
    let shown = |rig: &mut Rig, scope| {
        rig.open(scope);
        let dialog = rig.dialog_now();
        let (_, suggestions, _) = dialog.update(&mut rig.v, |d, cx| d.shown(cx)).unwrap();
        rig.keys("escape");
        suggestions
    };
    let team = shown(&mut rig, SnapshotScope::Team);
    assert_eq!(team.len(), 4);
    assert!(!team.join("\n").contains("previous period"));
    assert!(!team.join("\n").contains("the last 3"));
    let me = shown(&mut rig, SnapshotScope::Me { person_id: ana });
    assert!(!me.join("\n").contains("previous period"));
}

#[gpui::test]
fn start_is_inert_while_the_question_is_empty_or_blank(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "ask-start");
    assert!(rig.started.borrow().is_empty());
    rig.type_text("   ");
    click(&mut rig.v, "ask-start");
    assert!(rig.started.borrow().is_empty());
    assert!(!rig.dialog_now().read_with(&rig.v, |d, _| d.starting()));
    rig.type_text("hello");
    click(&mut rig.v, "ask-start");
    assert_eq!(rig.started.borrow().len(), 1);
}

#[gpui::test]
fn cmd_or_ctrl_enter_submits_but_a_plain_enter_and_an_empty_question_do_not(
    cx: &mut TestAppContext,
) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    // The question has the keyboard as soon as the dialog opens.
    let input = rig
        .dialog_now()
        .read_with(&rig.v, |d, _| d.question_input());
    let focused = |rig: &mut Rig| {
        let handle = rig.v.read(|cx| input.focus_handle(cx));
        rig.ui
            .window
            .update(&mut rig.v, |_, window, _| handle.is_focused(window))
            .unwrap()
    };
    assert!(focused(&mut rig), "the question is focused on open");
    rig.keys(SUBMIT);
    assert!(rig.started.borrow().is_empty(), "empty");
    rig.type_text("question");
    rig.keys("enter");
    assert!(
        rig.started.borrow().is_empty(),
        "a plain Enter is a newline"
    );
    rig.type_text("question");
    rig.keys(SUBMIT);
    assert_eq!(rig.started.borrow().len(), 1);
}

// ---- starting -------------------------------------------------------------

#[gpui::test]
fn it_shows_starting_blocks_a_second_start_and_closes_when_the_session_is_up(
    cx: &mut TestAppContext,
) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    click(&mut rig.v, "ask-start");
    assert!(rig.dialog_now().read_with(&rig.v, |d, _| d.starting()));
    click(&mut rig.v, "ask-start");
    rig.keys(SUBMIT);
    assert_eq!(rig.started.borrow().len(), 1);
    assert!(rig.dialog().is_some(), "still open until the session is up");
    rig.finished(Ok(()));
    assert!(rig.dialog().is_none());
    assert!(!rig.shown("ask-dialog"));
}

#[gpui::test]
fn a_rejection_is_shown_inline_and_the_start_can_be_retried(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    click(&mut rig.v, "ask-start");
    rig.finished(Err("No provider is available".into()));
    let dialog = rig.dialog_now();
    assert_eq!(
        dialog.read_with(&rig.v, |d, _| d.error().map(String::from)),
        Some("No provider is available".to_string())
    );
    assert!(rig.shown("ask-error"));
    assert!(!dialog.read_with(&rig.v, |d, _| d.starting()));
    assert_eq!(dialog.read_with(&rig.v, |d, cx| d.question_text(cx)), "q");
    click(&mut rig.v, "ask-start");
    assert_eq!(rig.started.borrow().len(), 2);
    assert!(!rig.shown("ask-error"), "the error goes with the new try");
    rig.finished(Ok(()));
    assert!(rig.dialog().is_none());
}

#[gpui::test]
fn a_rejection_without_text_gets_a_generic_message(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    click(&mut rig.v, "ask-start");
    rig.finished(Err(String::new()));
    let error = rig
        .dialog_now()
        .read_with(&rig.v, |d, _| d.error().map(String::from));
    assert_eq!(error.as_deref(), Some("Could not start the session."));
}

#[gpui::test]
fn an_answer_after_the_dialog_is_gone_touches_nothing(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    click(&mut rig.v, "ask-start");
    // The page was left and shown again before the session came up.
    let page = rig.page.clone();
    page.update(&mut rig.v, |page, cx| page.show(None, None, cx));
    settle(&mut rig.v);
    assert!(rig.dialog().is_none());
    rig.finished(Ok(()));
    rig.finished(Err("late".into()));
    assert!(rig.dialog().is_none());
    assert_eq!(rig.closed.get(), 0);
}

#[gpui::test]
fn showing_the_page_again_drops_the_dialog_with_its_events(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    let page = rig.page.clone();
    page.update(&mut rig.v, |page, cx| page.show(None, None, cx));
    settle(&mut rig.v);
    assert!(rig.dialog().is_none());
    assert!(
        page.read_with(&rig.v, |page, _| page._ask_events.is_empty()),
        "nothing listens to the dropped dialog"
    );
}

#[gpui::test]
fn the_answer_of_an_old_start_does_not_land_on_a_newer_dialog(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("first");
    click(&mut rig.v, "ask-start");
    let old = rig.started.borrow()[0].2;
    // The page is shown again while the first session is still starting; a new
    // dialog is opened and started.
    let page = rig.page.clone();
    page.update(&mut rig.v, |page, cx| {
        page.show(Some(crate::contributors::Tab::Github), None, cx)
    });
    settle(&mut rig.v);
    click(&mut rig.v, "contributors-ask");
    rig.type_text("second");
    click(&mut rig.v, "ask-start");
    let new = rig.started.borrow()[1].2;
    assert_ne!(old, new);
    // The first start fails late: the second dialog is still starting.
    rig.finished_for(old, Err("late".into()));
    let dialog = rig.dialog_now();
    assert!(dialog.read_with(&rig.v, |d, _| d.starting()));
    assert_eq!(
        dialog.read_with(&rig.v, |d, _| d.error().map(String::from)),
        None
    );
    // And it succeeding late does not close it.
    rig.finished_for(old, Ok(()));
    assert!(rig.dialog().is_some());
    // Its own answer does.
    rig.finished_for(new, Ok(()));
    assert!(rig.dialog().is_none());
}

#[gpui::test]
fn it_cannot_be_closed_while_starting(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    click(&mut rig.v, "ask-start");
    click(&mut rig.v, "ask-cancel");
    click(&mut rig.v, "ask-close");
    // A press outside the card, on the scrim.
    rig.v
        .simulate_click(gpui::point(gpui::px(4.), gpui::px(4.)), Default::default());
    settle(&mut rig.v);
    rig.keys("escape");
    assert!(
        rig.dialog().is_some(),
        "Cancel, X, the scrim and Escape are inert"
    );
    assert_eq!(
        rig.closed.get(),
        0,
        "and Escape does not reach the page either"
    );
    rig.finished(Ok(()));
    assert!(rig.dialog().is_none());
}

#[gpui::test]
fn it_can_be_closed_again_after_a_failed_start(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    click(&mut rig.v, "ask-start");
    rig.finished(Err("nope".into()));
    click(&mut rig.v, "ask-cancel");
    assert!(rig.dialog().is_none());
}

#[gpui::test]
fn escape_closes_the_dialog_from_the_question_and_the_page_stays_open(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    rig.type_text("half a question");
    rig.keys("escape");
    assert!(rig.dialog().is_none());
    assert_eq!(
        rig.closed.get(),
        0,
        "Escape in the dialog never closes the page"
    );
    assert!(rig.started.borrow().is_empty());
    // The keyboard is back on the page: the next Escape is the page's.
    rig.keys("escape");
    assert_eq!(rig.closed.get(), 1);
}

#[gpui::test]
fn cancel_the_x_and_the_scrim_close_without_starting(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    for how in ["ask-cancel", "ask-close"] {
        click(&mut rig.v, "contributors-ask");
        assert!(rig.dialog().is_some());
        click(&mut rig.v, how);
        assert!(rig.dialog().is_none(), "{how}");
    }
    click(&mut rig.v, "contributors-ask");
    rig.v
        .simulate_click(gpui::point(gpui::px(4.), gpui::px(4.)), Default::default());
    settle(&mut rig.v);
    assert!(rig.dialog().is_none(), "the scrim");
    assert!(rig.started.borrow().is_empty());
}

#[gpui::test]
fn the_dialog_follows_the_page_it_is_asked_from_only_once(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    let first = rig.dialog_now();
    // A second ask while one is open changes nothing.
    let (page, view) = (rig.page.clone(), rig.view.clone());
    page.update(&mut rig.v, |page, cx| {
        page.open_ask(
            "/work/alpha".into(),
            SnapshotScope::Team,
            view.clone(),
            both(),
            cx,
        )
    });
    assert_eq!(rig.dialog_now().entity_id(), first.entity_id());
}

// ---- provider -------------------------------------------------------------

#[gpui::test]
fn the_default_provider_is_sent(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    assert!(rig.shown("contributors-select-agent"));
    assert_eq!(
        rig.dialog_now().read_with(&rig.v, |d, _| d.harness()),
        Some(Kind::Codex)
    );
    rig.type_text("q");
    let (request, _) = rig.start();
    assert_eq!(request.harness, Some(Kind::Codex));
}

#[gpui::test]
fn the_provider_picked_from_the_list_is_sent(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "contributors-select-agent");
    click(&mut rig.v, "contributors-option-agent-0");
    assert_eq!(
        rig.dialog_now().read_with(&rig.v, |d, _| d.harness()),
        Some(Kind::Claude)
    );
    rig.type_text("q");
    let (request, _) = rig.start();
    assert_eq!(request.harness, Some(Kind::Claude));
}

#[gpui::test]
fn escape_in_the_provider_list_closes_the_list_and_not_the_dialog(cx: &mut TestAppContext) {
    let mut rig = rig(cx, Fake::new(), both());
    click(&mut rig.v, "contributors-ask");
    click(&mut rig.v, "contributors-select-agent");
    assert!(rig.shown("contributors-option-agent-0"));
    rig.keys("escape");
    assert!(!rig.shown("contributors-option-agent-0"));
    assert!(rig.dialog().is_some());
    assert_eq!(rig.closed.get(), 0);
}

#[gpui::test]
fn a_single_provider_is_a_label_and_none_is_nothing(cx: &mut TestAppContext) {
    let mut rig = rig(
        cx,
        Fake::new(),
        AskChoices {
            providers: vec![Kind::Claude],
            default: Some(Kind::Claude),
        },
    );
    click(&mut rig.v, "contributors-ask");
    assert!(rig.shown("ask-agent-label"));
    assert!(!rig.shown("contributors-select-agent"));
    rig.type_text("q");
    assert_eq!(rig.start().0.harness, Some(Kind::Claude));
    rig.finished(Ok(()));

    let mut none = self::rig(cx, Fake::new(), AskChoices::default());
    click(&mut none.v, "contributors-ask");
    assert!(!none.shown("ask-agent-label"));
    assert!(!none.shown("contributors-select-agent"));
    none.type_text("q");
    assert_eq!(none.start().0.harness, None, "the project default decides");
}

#[gpui::test]
fn a_default_that_is_not_offered_falls_back_to_the_first(cx: &mut TestAppContext) {
    let mut rig = rig(
        cx,
        Fake::new(),
        AskChoices {
            providers: vec![Kind::Claude, Kind::Cursor],
            default: Some(Kind::Codex),
        },
    );
    click(&mut rig.v, "contributors-ask");
    rig.type_text("q");
    assert_eq!(rig.start().0.harness, Some(Kind::Claude));
}

#[gpui::test]
fn the_page_ignores_an_ask_before_the_data_is_on_screen(cx: &mut TestAppContext) {
    let fake = Fake::new();
    fake.with(|s| s.load.once.push_back(Err("offline".into())));
    let mut rig = rig(cx, fake, both());
    let (page, view) = (rig.page.clone(), rig.view.clone());
    page.update(&mut rig.v, |page, cx| {
        page.open_ask(
            "/work/alpha".into(),
            SnapshotScope::Team,
            view.clone(),
            both(),
            cx,
        )
    });
    assert!(rig.dialog().is_none());
}
