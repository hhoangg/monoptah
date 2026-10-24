//! Ports of `TuiSessionPane.test.ts`. The web app's xterm rendering and its PTY
//! attach/handoff cases have no counterpart here: the terminal is `Terminal`
//! (tested in `terminal.rs`, driven here through its `fake_*` seam) and a moved
//! pane keeps its entity.
use super::*;
use gpui::TestAppContext;
use monocode_core::{
    agent::{AccessMode, Settings},
    store::Store,
    surface::Surface,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Duration,
};

const PROJECT: &str = "/work/project";
/// Claude's answer to `--resume` of an id it never wrote.
const NO_CONVERSATION: &str = "No conversation found with session ID: 1111\r\n";

fn temp(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "monocode-tui-{label}-{}",
        monocode_core::mono::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
fn store_in(dir: &Path) -> Database {
    Arc::new(Mutex::new(Store::open(&dir.join("monocode.db")).unwrap()))
}
fn fake_binary(kind: Kind) -> Result<PathBuf, String> {
    Ok(PathBuf::from(format!("/opt/test/{}", kind.id())))
}
/// A terminal session; `id` is the conversation it already has.
fn terminal_session(store: &Database, provider: &str, id: Option<&str>) -> Session {
    let store = locked(store).unwrap();
    let mut summary = store
        .create_with_surface(provider, PROJECT, Surface::Tui)
        .unwrap();
    if let Some(id) = id {
        store.set_terminal_conversation(summary.id, id).unwrap();
        summary.provider_id = Some(id.into());
        summary.has_messages = true;
    }
    summary
}
fn request(store: &Database, summary: &Session) -> Request {
    Request {
        store: store.clone(),
        id: summary.id,
        kind: Kind::from_id(&summary.provider),
        cwd: summary.cwd.clone(),
        provider_id: summary.provider_id.clone(),
        claim: None,
        fresh: false,
        initial_prompt: None,
        hooks: false,
        book: Book::default(),
        root: temp("accounts"),
        binary: fake_binary,
    }
}
fn args(prepared: &Prepared) -> Vec<String> {
    prepared
        .launch
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}
fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
    let at = args.iter().position(|arg| arg == flag).unwrap();
    &args[at + 1]
}
fn saved_id(store: &Database, id: i64) -> Option<String> {
    locked(store)
        .unwrap()
        .sessions()
        .unwrap()
        .into_iter()
        .find(|session| session.id == id)
        .and_then(|session| session.provider_id)
}
fn save_settings(store: &Database, id: i64, edit: impl FnOnce(&mut Settings)) {
    let store = locked(store).unwrap();
    let mut settings = store.agent_settings(id).unwrap().unwrap_or_default();
    edit(&mut settings);
    store.save_agent_settings(id, &settings).unwrap();
}

// prepare: what a launch decides and saves before anything spawns

#[test]
fn a_new_claude_conversation_id_is_saved_before_the_cli_starts() {
    let store = store_in(&temp("new"));
    let summary = terminal_session(&store, "claude", None);
    let prepared = prepare(request(&store, &summary)).unwrap();
    let minted = prepared.launch.minted_id.clone().unwrap();
    assert_eq!(after(&args(&prepared), "--session-id"), minted);
    assert_eq!(prepared.provider_id.as_deref(), Some(minted.as_str()));
    assert!(!prepared.reused_id && !prepared.claimed);
    // Already on disk when `prepare` returns, which is before any spawn.
    assert_eq!(
        saved_id(&store, summary.id).as_deref(),
        Some(minted.as_str())
    );
    assert!(
        locked(&store)
            .unwrap()
            .sessions()
            .unwrap()
            .iter()
            .any(|s| s.id == summary.id && s.has_messages),
        "the saved id is what lists a terminal session"
    );
}

#[test]
fn a_saved_conversation_is_resumed_without_minting_or_saving_a_new_id() {
    let store = store_in(&temp("resume"));
    let summary = terminal_session(&store, "claude", Some("conv-1"));
    let prepared = prepare(request(&store, &summary)).unwrap();
    let args = args(&prepared);
    assert_eq!(after(&args, "--resume"), "conv-1");
    assert!(!args.contains(&"--session-id".to_string()));
    assert_eq!(prepared.launch.minted_id, None);
    assert_eq!(prepared.provider_id.as_deref(), Some("conv-1"));
    assert!(prepared.reused_id && !prepared.claimed);
    assert_eq!(saved_id(&store, summary.id).as_deref(), Some("conv-1"));
}

#[test]
fn an_id_that_looks_like_a_flag_is_replaced_not_resumed() {
    let store = store_in(&temp("flag"));
    let summary = terminal_session(&store, "claude", Some("--dangerously-skip-permissions"));
    let prepared = prepare(request(&store, &summary)).unwrap();
    let args = args(&prepared);
    assert!(!args.iter().any(|arg| arg.contains("dangerously")));
    let minted = prepared.launch.minted_id.clone().unwrap();
    assert_eq!(after(&args, "--session-id"), minted);
    assert_eq!(
        saved_id(&store, summary.id).as_deref(),
        Some(minted.as_str())
    );
    assert!(!prepared.reused_id, "a replaced id is not a resumed one");
}

#[test]
fn a_claim_launches_under_session_id_with_the_same_saved_id() {
    let store = store_in(&temp("claim"));
    let summary = terminal_session(&store, "claude", Some("conv-1"));
    let prepared = prepare(Request {
        claim: Some("conv-1".into()),
        ..request(&store, &summary)
    })
    .unwrap();
    let args = args(&prepared);
    assert_eq!(after(&args, "--session-id"), "conv-1");
    assert!(!args.contains(&"--resume".to_string()));
    assert_eq!(prepared.provider_id.as_deref(), Some("conv-1"));
    // A failed claim still offers a fresh start.
    assert!(prepared.reused_id && prepared.claimed);
}

#[test]
fn a_new_claude_conversation_starts_with_the_initial_prompt_as_its_last_argument() {
    let store = store_in(&temp("prompt"));
    let summary = terminal_session(&store, "claude", None);
    let prepared = prepare(Request {
        initial_prompt: Some("Fix the build".into()),
        ..request(&store, &summary)
    })
    .unwrap();
    let args = args(&prepared);
    assert_eq!(args.last().map(String::as_str), Some("Fix the build"));
    assert!(args.contains(&"--session-id".to_string()));
}

#[test]
fn a_resumed_conversation_never_gets_the_initial_prompt() {
    let store = store_in(&temp("prompt-resume"));
    let summary = terminal_session(&store, "claude", Some("conv-1"));
    let prepared = prepare(Request {
        initial_prompt: Some("Fix the build".into()),
        ..request(&store, &summary)
    })
    .unwrap();
    assert!(!args(&prepared).iter().any(|arg| arg == "Fix the build"));
}

#[test]
fn a_cli_without_a_first_prompt_slot_ignores_the_initial_prompt() {
    let store = store_in(&temp("prompt-codex"));
    let summary = terminal_session(&store, "codex", None);
    let prepared = prepare(Request {
        initial_prompt: Some("Fix the build".into()),
        ..request(&store, &summary)
    })
    .unwrap();
    assert!(prepared.launch.args.is_empty());
}

#[test]
fn starting_fresh_mints_a_new_id_and_saves_it() {
    let store = store_in(&temp("fresh"));
    let summary = terminal_session(&store, "claude", Some("conv-1"));
    let prepared = prepare(Request {
        fresh: true,
        ..request(&store, &summary)
    })
    .unwrap();
    let minted = prepared.launch.minted_id.clone().unwrap();
    assert_ne!(minted, "conv-1");
    assert!(!args(&prepared).contains(&"--resume".to_string()));
    assert_eq!(
        saved_id(&store, summary.id).as_deref(),
        Some(minted.as_str())
    );
    assert!(!prepared.reused_id);
}

#[test]
fn other_providers_launch_their_bare_cli_and_save_nothing() {
    let store = store_in(&temp("codex"));
    let summary = terminal_session(&store, "codex", None);
    let prepared = prepare(request(&store, &summary)).unwrap();
    assert_eq!(prepared.launch.program, OsString::from("/opt/test/codex"));
    assert!(prepared.launch.args.is_empty());
    assert_eq!(prepared.launch.minted_id, None);
    assert_eq!(prepared.provider_id, None);
    assert_eq!(saved_id(&store, summary.id), None);
}

#[test]
fn nothing_is_ready_when_the_conversation_id_cannot_be_saved() {
    let store = store_in(&temp("unsaved"));
    // A chat session cannot take a terminal conversation id.
    let chat = locked(&store).unwrap().create("claude", PROJECT).unwrap();
    let error = prepare(request(&store, &chat)).err().unwrap();
    assert!(
        error.starts_with("Could not save the conversation id"),
        "{error}"
    );
    assert_eq!(saved_id(&store, chat.id), None);
}

#[test]
fn a_binary_that_cannot_be_found_is_an_error() {
    let store = store_in(&temp("nobinary"));
    let summary = terminal_session(&store, "claude", None);
    let error = prepare(Request {
        binary: |kind| Err(format!("Could not find the {} CLI", kind.label())),
        ..request(&store, &summary)
    })
    .err()
    .unwrap();
    assert_eq!(error, "Could not find the Claude Code CLI");
    assert_eq!(
        saved_id(&store, summary.id),
        None,
        "nothing saved for a launch that cannot happen"
    );
}

#[test]
fn the_session_model_and_access_shape_a_claude_launch() {
    let store = store_in(&temp("shape"));
    let summary = terminal_session(&store, "claude", None);
    save_settings(&store, summary.id, |settings| {
        settings.model = Some("claude:opus-5-5".into());
        settings.access = AccessMode::ReadOnly;
    });
    let args = args(&prepare(request(&store, &summary)).unwrap());
    assert_eq!(after(&args, "--model"), "claude-opus-5-5");
    assert_eq!(after(&args, "--permission-mode"), "plan");
    // No saved model: the flag is omitted.
    let other = terminal_session(&store, "claude", None);
    let args = args_of(&prepare(request(&store, &other)).unwrap());
    assert!(!args.contains(&"--model".to_string()));
}
fn args_of(prepared: &Prepared) -> Vec<String> {
    args(prepared)
}

#[test]
fn the_cli_runs_in_the_worktree_unless_it_was_removed() {
    let store = store_in(&temp("cwd"));
    let summary = terminal_session(&store, "claude", None);
    assert_eq!(prepare(request(&store, &summary)).unwrap().cwd, PROJECT);
    save_settings(&store, summary.id, |settings| {
        settings.worktree_cwd = Some("/work/project-wt".into());
    });
    assert_eq!(
        prepare(request(&store, &summary)).unwrap().cwd,
        "/work/project-wt"
    );
    save_settings(&store, summary.id, |settings| {
        settings.worktree_removed = true
    });
    assert_eq!(prepare(request(&store, &summary)).unwrap().cwd, PROJECT);
}

#[test]
fn claude_hooks_need_the_preference_a_data_folder_and_a_non_windows_host() {
    let dir = temp("hooks");
    let store = store_in(&dir);
    let summary = terminal_session(&store, "claude", None);
    let settings_json = |prepared: &Prepared| {
        let args = args(prepared);
        args.iter()
            .position(|arg| arg == "--settings")
            .map(|at| args[at + 1].clone())
    };
    assert_eq!(
        settings_json(&prepare(request(&store, &summary)).unwrap()),
        None
    );
    let on = prepare(Request {
        hooks: true,
        ..request(&store, &summary)
    })
    .unwrap();
    let json = settings_json(&on).expect("hooks inject --settings");
    let command = serde_json::from_str::<serde_json::Value>(&json).unwrap()["hooks"]["Stop"][0]
        ["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .to_owned();
    let exe = std::env::current_exe().unwrap();
    let spool = dir.join("tui-hooks-native");
    assert_eq!(
        command,
        format!(
            "{} tui-hook --spool {}",
            tui_launch::shell_path(&exe),
            tui_launch::shell_path(&spool)
        )
    );
    // No data folder (in-memory store): nowhere to spool, so nothing injected.
    let memory: Database = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let session = terminal_session(&memory, "claude", None);
    let none = prepare(Request {
        hooks: true,
        ..request(&memory, &session)
    })
    .unwrap();
    assert_eq!(settings_json(&none), None);
    // Other providers have no hooks.
    let codex = terminal_session(&store, "codex", None);
    let bare = prepare(Request {
        hooks: true,
        ..request(&store, &codex)
    })
    .unwrap();
    assert!(bare.launch.args.is_empty());
}

fn account_book(root: &Path) -> (Book, String) {
    let mut book = Book::default();
    let account = book.add(Kind::Claude, "Work").unwrap();
    let _ = root;
    (book, account.id)
}

#[test]
fn the_default_account_sends_no_account_env() {
    let store = store_in(&temp("default-account"));
    let summary = terminal_session(&store, "claude", None);
    let prepared = prepare(request(&store, &summary)).unwrap();
    assert!(prepared.env_set.is_empty() && prepared.env_remove.is_empty());
}

#[test]
fn the_selected_account_is_pinned_on_the_first_launch_and_not_moved_later() {
    let store = store_in(&temp("pin"));
    let summary = terminal_session(&store, "claude", None);
    let root = temp("accounts-root");
    let (mut book, account) = account_book(&root);
    book.select(Kind::Claude, PROJECT, &account).unwrap();
    let launch = |book: &Book| {
        prepare(Request {
            book: book.clone(),
            root: root.clone(),
            provider_id: saved_id(&store, summary.id),
            ..request(&store, &summary)
        })
        .unwrap()
    };
    let first = launch(&book);
    let dir = root.join("provider-accounts").join("claude").join(&account);
    assert!(
        first
            .env_set
            .contains(&("CLAUDE_CONFIG_DIR".to_owned(), dir.clone().into_os_string())),
        "{:?}",
        first.env_set
    );
    assert!(first.env_remove.contains(&"ANTHROPIC_API_KEY"));
    assert_eq!(
        locked(&store)
            .unwrap()
            .agent_settings(summary.id)
            .unwrap()
            .unwrap()
            .account
            .id,
        account,
        "pinned to the session"
    );
    // The selection moves on; the conversation stays with its account.
    book.select(Kind::Claude, PROJECT, core_accounts::DEFAULT)
        .unwrap();
    let second = launch(&book);
    assert!(
        second
            .env_set
            .contains(&("CLAUDE_CONFIG_DIR".to_owned(), dir.into_os_string()))
    );
}

#[test]
fn a_removed_account_refuses_to_launch_and_saves_nothing() {
    let store = store_in(&temp("removed"));
    let summary = terminal_session(&store, "claude", None);
    save_settings(&store, summary.id, |settings| {
        settings.account = Profile::new("account-gone", Path::new("/x"));
    });
    let error = prepare(request(&store, &summary)).err().unwrap();
    assert_eq!(error, REMOVED_ACCOUNT);
    assert_eq!(saved_id(&store, summary.id), None);
}

// the entity

struct Fixture {
    session: Entity<TuiSession>,
    events: Rc<RefCell<Vec<TuiEvent>>>,
    store: Database,
    summary: Session,
}
impl Fixture {
    fn new(cx: &mut TestAppContext, provider: &str, id: Option<&str>) -> Self {
        let store = store_in(&temp("session"));
        let summary = terminal_session(&store, provider, id);
        Self::over(cx, store, summary)
    }
    /// A session the store knows as a chat, so a conversation id cannot be saved for it.
    fn chat(cx: &mut TestAppContext) -> Self {
        let store = store_in(&temp("chat"));
        let summary = locked(&store).unwrap().create("claude", PROJECT).unwrap();
        Self::over(cx, store, summary)
    }
    fn over(cx: &mut TestAppContext, store: Database, summary: Session) -> Self {
        let session = cx.new(|_| TuiSession::new(&summary, store.clone(), None));
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &TuiEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach()
        });
        Self {
            session,
            events,
            store,
            summary,
        }
    }
    fn start(&self, cx: &mut TestAppContext) {
        self.session.update(cx, |s, cx| s.start(cx));
        cx.run_until_parked();
    }
    fn terminal(&self, cx: &TestAppContext) -> Entity<Terminal> {
        self.session
            .read_with(cx, |s, _| s.terminal().cloned())
            .expect("a running terminal")
    }
    fn args(&self, cx: &TestAppContext) -> Vec<String> {
        self.terminal(cx).read_with(cx, |t, _| {
            t.launched()
                .unwrap()
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        })
    }
    fn exit(&self, cx: &mut TestAppContext, code: Option<i32>) {
        self.terminal(cx).update(cx, |t, cx| t.fake_exit(code, cx));
        cx.run_until_parked();
    }
    fn output(&self, cx: &mut TestAppContext, text: &str) {
        self.terminal(cx)
            .update(cx, |t, cx| t.fake_output(text, cx));
    }
    fn age(&self, cx: &mut TestAppContext, by: Duration) {
        self.session.update(cx, |s, _| s.age_launch(by));
    }
    fn events(&self) -> Vec<TuiEvent> {
        self.events.borrow().clone()
    }
    fn exited(&self) -> Vec<Exit> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                TuiEvent::Exited(exit) => Some(exit),
                _ => None,
            })
            .collect()
    }
    fn closed(&self) -> bool {
        self.events().contains(&TuiEvent::Close)
    }
    fn offers_fresh(&self, cx: &TestAppContext) -> bool {
        self.session.read_with(cx, |s, _| s.offers_fresh())
    }
}

#[gpui::test]
fn nothing_launches_until_the_pane_is_first_shown_and_it_launches_once(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    cx.run_until_parked();
    assert!(f.session.read_with(cx, |s, _| s.terminal().is_none()));
    f.start(cx);
    let first = f.terminal(cx);
    f.start(cx);
    assert_eq!(f.terminal(cx), first, "shown again: the same process");
    let (program, cwd) = first.read_with(cx, |t, _| {
        (t.launched().unwrap().program.clone(), t.cwd.clone())
    });
    assert_eq!(program, OsString::from("/opt/test/claude"));
    assert_eq!(cwd, PROJECT);
}

#[gpui::test]
fn it_saves_a_new_claude_conversation_id_before_the_cli_starts(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    let args = f.args(cx);
    let minted = after(&args, "--session-id").to_owned();
    assert_eq!(
        saved_id(&f.store, f.summary.id).as_deref(),
        Some(minted.as_str())
    );
    assert_eq!(
        f.session
            .read_with(cx, |s, _| s.provider_id().map(str::to_owned)),
        Some(minted.clone())
    );
    assert!(f.events().contains(&TuiEvent::Conversation(Some(minted))));
}

#[gpui::test]
fn it_does_not_start_the_cli_when_the_conversation_id_cannot_be_saved(cx: &mut TestAppContext) {
    let f = Fixture::chat(cx);
    f.start(cx);
    assert!(f.session.read_with(cx, |s, _| s.terminal().is_none()));
    let exits = f.exited();
    assert_eq!(exits.len(), 1);
    assert!(
        exits[0]
            .error
            .as_deref()
            .unwrap()
            .starts_with("Could not save")
    );
    assert!(!f.closed());
}

#[gpui::test]
fn it_resumes_a_saved_conversation_without_saving_a_new_id(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    let args = f.args(cx);
    assert_eq!(after(&args, "--resume"), "conv-1");
    assert!(
        !f.events()
            .iter()
            .any(|event| matches!(event, TuiEvent::Conversation(_)))
    );
}

#[gpui::test]
fn it_launches_other_providers_fresh_with_their_bare_cli(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "codex", None);
    f.start(cx);
    assert!(f.args(cx).is_empty());
    assert_eq!(saved_id(&f.store, f.summary.id), None);
}

#[gpui::test]
fn started_is_reported_once_after_the_process_spawned(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    assert!(!f.events().contains(&TuiEvent::Started));
    f.terminal(cx).update(cx, |t, cx| t.fake_started(cx));
    assert_eq!(
        f.events()
            .iter()
            .filter(|e| **e == TuiEvent::Started)
            .count(),
        1
    );
}

#[gpui::test]
fn a_spawn_failure_reports_no_start_and_keeps_the_pane(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    f.terminal(cx)
        .update(cx, |t, cx| t.fake_spawn_error("No such file", cx));
    cx.run_until_parked();
    assert!(!f.events().contains(&TuiEvent::Started));
    let exit = f.exited().pop().unwrap();
    assert_eq!(exit.error.as_deref(), Some("No such file"));
    assert_eq!(exit.code, None);
    assert!(!f.closed());
    let notice = f.session.read_with(cx, |s, _| {
        tui_launch::exit_notice(s.kind, s.exit().unwrap())
    });
    assert_eq!(notice.message, "Could not start Claude Code");
    assert_eq!(notice.hint.as_deref(), Some("No such file"));
}

#[gpui::test]
fn it_reports_an_early_exit_and_keeps_the_pane(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "codex", None);
    f.start(cx);
    f.exit(cx, Some(1));
    assert_eq!(
        f.exited(),
        [Exit {
            code: Some(1),
            error: None,
            early: true,
            no_conversation: false
        }]
    );
    assert!(!f.closed());
}

#[gpui::test]
fn a_late_exit_with_a_failure_code_keeps_the_pane_without_a_fresh_start(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.age(cx, Duration::from_secs(10));
    f.exit(cx, Some(2));
    let exit = f.exited().pop().unwrap();
    assert_eq!((exit.code, exit.early), (Some(2), false));
    assert!(!f.closed());
    assert!(!f.offers_fresh(cx));
}

#[gpui::test]
fn a_clean_exit_well_after_launch_closes_the_pane_after_reporting_it(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    f.age(cx, Duration::from_secs(10));
    f.exit(cx, Some(0));
    let events = f.events();
    let exited = events
        .iter()
        .position(|e| matches!(e, TuiEvent::Exited(_)))
        .unwrap();
    let close = events.iter().position(|e| *e == TuiEvent::Close).unwrap();
    assert!(
        exited < close,
        "an automation run settles before the pane goes"
    );
}

#[gpui::test]
fn a_clean_exit_right_after_launch_keeps_the_pane(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    f.exit(cx, Some(0));
    assert!(!f.closed());
    assert!(f.exited()[0].early);
}

#[gpui::test]
fn a_late_exit_by_signal_keeps_the_pane(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    f.age(cx, Duration::from_secs(10));
    f.exit(cx, None);
    assert!(!f.closed());
    assert_eq!(f.exited()[0].code, None);
}

#[gpui::test]
fn restart_reopens_the_same_claude_conversation(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    let minted = after(&f.args(cx), "--session-id").to_owned();
    f.exit(cx, Some(1));
    f.session.update(cx, |s, cx| s.restart(cx));
    cx.run_until_parked();
    let args = f.args(cx);
    assert_eq!(after(&args, "--resume"), minted);
    assert!(
        f.session
            .read_with(cx, |s, _| s.exit().is_none() && s.running())
    );
}

#[gpui::test]
fn the_initial_prompt_rides_on_the_first_launch_only(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.session
        .update(cx, |s, _| s.set_initial_prompt("Fix the build".into()));
    f.start(cx);
    let args = f.args(cx);
    assert_eq!(args.last().map(String::as_str), Some("Fix the build"));
    let minted = after(&args, "--session-id").to_owned();
    f.exit(cx, Some(1));
    f.session.update(cx, |s, cx| s.restart(cx));
    cx.run_until_parked();
    let args = f.args(cx);
    assert_eq!(after(&args, "--resume"), minted);
    assert!(
        !args.iter().any(|arg| arg == "Fix the build"),
        "a restart resumes the conversation instead of repeating the prompt"
    );
}

#[gpui::test]
fn a_launch_that_fails_to_prepare_keeps_the_initial_prompt_for_the_retry(cx: &mut TestAppContext) {
    // A chat row cannot take a conversation id, so the launch fails to prepare.
    let f = Fixture::chat(cx);
    f.session
        .update(cx, |s, _| s.set_initial_prompt("Fix the build".into()));
    f.start(cx);
    assert!(f.session.read_with(cx, |s, _| s.exit().is_some()));
    assert_eq!(
        f.session.read_with(cx, |s, _| s.initial_prompt.clone()),
        Some("Fix the build".into())
    );
}

#[gpui::test]
fn it_offers_a_fresh_conversation_when_resuming_fails_at_once(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.exit(cx, Some(1));
    assert!(f.offers_fresh(cx));
    // A first launch that minted its id has nothing to start over from.
    let g = Fixture::new(cx, "claude", None);
    g.start(cx);
    g.exit(cx, Some(1));
    assert!(!g.offers_fresh(cx));
}

#[gpui::test]
fn starting_fresh_forgets_the_conversation_and_launches_a_new_one(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.exit(cx, Some(1));
    f.session.update(cx, |s, cx| s.start_fresh(cx));
    cx.run_until_parked();
    let args = f.args(cx);
    let minted = after(&args, "--session-id").to_owned();
    assert_ne!(minted, "conv-1");
    assert!(!args.contains(&"--resume".to_string()));
    assert_eq!(
        saved_id(&f.store, f.summary.id).as_deref(),
        Some(minted.as_str())
    );
    assert!(f.events().contains(&TuiEvent::Conversation(Some(minted))));
}

#[gpui::test]
fn it_claims_the_saved_id_once_when_resume_finds_no_conversation(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    assert_eq!(after(&f.args(cx), "--resume"), "conv-1");
    let first = f.terminal(cx);
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(1));
    // Relaunched under --session-id with the same id, and nothing reported.
    let second = f.terminal(cx);
    assert_ne!(first, second);
    let args = f.args(cx);
    assert_eq!(after(&args, "--session-id"), "conv-1");
    assert!(!args.contains(&"--resume".to_string()));
    assert!(f.exited().is_empty() && !f.closed(), "{:?}", f.events());
}

#[gpui::test]
fn it_does_not_close_while_retrying_a_resume_that_found_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(0));
    assert!(!f.closed());
    assert!(f.exited().is_empty());
}

#[gpui::test]
fn it_shows_the_error_when_the_claim_also_fails_and_offers_a_fresh_start(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(1));
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(1));
    let exits = f.exited();
    assert_eq!(
        exits.len(),
        1,
        "the failed claim is the only thing reported"
    );
    assert!(exits[0].no_conversation && exits[0].early);
    assert!(f.offers_fresh(cx));
    assert!(!f.closed());
}

#[gpui::test]
fn it_does_not_claim_a_second_time_when_the_claim_again_finds_nothing(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    for _ in 0..2 {
        f.output(cx, NO_CONVERSATION);
        f.exit(cx, Some(1));
    }
    // A third process would mean a loop; the terminal stays the claimed one.
    let before = f.terminal(cx);
    cx.run_until_parked();
    assert_eq!(f.terminal(cx), before);
    assert_eq!(f.exited().len(), 1);
}

#[gpui::test]
fn a_manual_restart_may_claim_the_id_again(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    for _ in 0..2 {
        f.output(cx, NO_CONVERSATION);
        f.exit(cx, Some(1));
    }
    f.session.update(cx, |s, cx| s.restart(cx));
    cx.run_until_parked();
    assert_eq!(after(&f.args(cx), "--resume"), "conv-1");
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(1));
    assert_eq!(after(&f.args(cx), "--session-id"), "conv-1");
    assert_eq!(
        f.exited().len(),
        1,
        "no new report: the second chain claimed again"
    );
}

#[gpui::test]
fn it_does_not_claim_for_any_other_early_exit(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.output(cx, "Error: Session ID conv-1 is already in use.\r\n");
    f.exit(cx, Some(1));
    assert_eq!(after(&f.args(cx), "--resume"), "conv-1");
    assert_eq!(f.exited().len(), 1);
    assert!(!f.exited()[0].no_conversation);
}

#[gpui::test]
fn it_does_not_claim_when_the_message_appears_long_after_launch(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.age(cx, Duration::from_secs(10));
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(0));
    let exit = f.exited().pop().unwrap();
    assert!(exit.no_conversation && !exit.early);
    assert!(!f.closed(), "print mode exits 0 for a missing conversation");
    assert!(f.offers_fresh(cx));
    let notice = f.session.read_with(cx, |s, _| {
        tui_launch::exit_notice(s.kind, s.exit().unwrap())
    });
    assert!(
        notice
            .hint
            .unwrap()
            .contains("could not find the saved conversation")
    );
}

#[gpui::test]
fn it_does_not_claim_a_first_launch_that_was_not_resuming(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    let first = f.terminal(cx);
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(1));
    assert_eq!(f.terminal(cx), first);
    assert_eq!(f.exited().len(), 1);
}

#[gpui::test]
fn the_cli_title_is_reported_cleaned(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    let terminal = f.terminal(cx);
    terminal.update(cx, |t, cx| {
        t.fake_title("✳ Fix the bug", cx);
        t.fake_title("✳", cx);
        t.fake_title("⠋ Fix the bug now", cx);
    });
    let titles: Vec<_> = f
        .events()
        .into_iter()
        .filter_map(|event| match event {
            TuiEvent::Title(title) => Some(title),
            _ => None,
        })
        .collect();
    assert_eq!(titles, ["Fix the bug", "Fix the bug now"]);
}

#[gpui::test]
fn busy_follows_hooks_and_the_cli_exiting_clears_it(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    // Nothing runs yet: a stray event cannot mark the pane busy.
    f.session.update(cx, |s, cx| s.set_busy(true, cx));
    assert!(!f.session.read_with(cx, |s, _| s.busy()));
    f.start(cx);
    f.session.update(cx, |s, cx| s.set_busy(true, cx));
    f.session.update(cx, |s, cx| s.set_busy(true, cx));
    assert!(f.session.read_with(cx, |s, _| s.busy()));
    f.age(cx, Duration::from_secs(10));
    f.exit(cx, Some(0));
    assert!(!f.session.read_with(cx, |s, _| s.busy()));
    assert_eq!(
        f.events()
            .into_iter()
            .filter(|e| matches!(e, TuiEvent::Busy(_)))
            .collect::<Vec<_>>(),
        [TuiEvent::Busy(true), TuiEvent::Busy(false)]
    );
    let events = f.events();
    let idle = events
        .iter()
        .position(|e| *e == TuiEvent::Busy(false))
        .unwrap();
    let exited = events
        .iter()
        .position(|e| matches!(e, TuiEvent::Exited(_)))
        .unwrap();
    assert!(idle < exited);
    // An exited pane stays idle.
    f.session.update(cx, |s, cx| s.set_busy(true, cx));
    assert!(!f.session.read_with(cx, |s, _| s.busy()));
}

#[gpui::test]
fn killing_ends_the_cli_and_nothing_relaunches_it(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.start(cx);
    let terminal = f.terminal(cx);
    f.session.update(cx, |s, cx| s.set_busy(true, cx));
    f.session.update(cx, |s, cx| s.kill(cx));
    assert!(f.session.read_with(cx, |s, _| s.stopped() && !s.busy()));
    // The Exited the dying process would still send is not heard.
    terminal.update(cx, |t, cx| t.fake_exit(None, cx));
    cx.run_until_parked();
    assert!(f.exited().is_empty());
    f.session.update(cx, |s, cx| {
        s.start(cx);
        s.restart(cx);
        s.start_fresh(cx);
    });
    cx.run_until_parked();
    assert!(f.session.read_with(cx, |s, _| s.stopped()));
}

#[gpui::test]
fn killing_during_preparation_drops_the_launch(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", None);
    f.session.update(cx, |s, cx| {
        s.start(cx);
        s.kill(cx);
    });
    cx.run_until_parked();
    assert!(f.session.read_with(cx, |s, _| s.terminal().is_none()));
    assert!(f.exited().is_empty());
}

#[test]
fn only_a_local_terminal_row_runs_as_a_terminal() {
    let store = store_in(&temp("remote"));
    let tui = terminal_session(&store, "claude", None);
    assert!(runs_in_terminal(&tui));
    let chat = locked(&store).unwrap().create("claude", PROJECT).unwrap();
    assert!(!runs_in_terminal(&chat));
    let remote = Session {
        cwd: "remote://env/home/me/project".into(),
        ..tui.clone()
    };
    assert!(!runs_in_terminal(&remote));
    let remote_id = Session { id: -4, ..tui };
    assert!(!runs_in_terminal(&remote_id));
}

// the view

fn window<'a>(
    cx: &'a mut TestAppContext,
    provider: &str,
    id: Option<&str>,
    worktree: Option<&str>,
) -> (
    Entity<TuiSession>,
    Rc<RefCell<Vec<TuiEvent>>>,
    &'a mut gpui::VisualTestContext,
) {
    let store = store_in(&temp("view"));
    let summary = terminal_session(&store, provider, id);
    let worktree = worktree.map(str::to_owned);
    let (view, visual) = cx.add_window_view(|_, _| TuiSession::new(&summary, store, worktree));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &TuiEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach()
    });
    (view, events, visual)
}

#[gpui::test]
fn the_header_shows_new_worktree_disabled_unless_the_session_has_a_worktree(
    cx: &mut TestAppContext,
) {
    let (_, _, visual) = window(cx, "claude", None, None);
    visual.run_until_parked();
    assert!(visual.debug_bounds("tui-header").is_some());
    assert!(visual.debug_bounds("tui-new-worktree").is_some());
    let (_, _, visual) = window(cx, "claude", None, Some("/work/project-wt"));
    visual.run_until_parked();
    assert!(visual.debug_bounds("tui-header").is_some());
    assert!(visual.debug_bounds("tui-new-worktree").is_none());
}

#[gpui::test]
fn the_exit_bar_offers_restart_and_a_fresh_start_only_for_a_failed_resume(cx: &mut TestAppContext) {
    let (view, events, visual) = window(cx, "claude", Some("conv-1"), None);
    view.update(visual, |s, cx| s.start(cx));
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("tui-exit-bar").is_none(),
        "nothing to say while it runs"
    );
    let terminal = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    terminal.update(visual, |t, cx| t.fake_exit(Some(1), cx));
    visual.run_until_parked();
    assert!(visual.debug_bounds("tui-exit-bar").is_some());
    assert!(visual.debug_bounds("tui-restart").is_some());
    let fresh = visual
        .debug_bounds("tui-start-fresh")
        .expect("a resume that failed at once");
    // Start new conversation clears the id and launches under a new one.
    visual.simulate_click(fresh.center(), Default::default());
    visual.run_until_parked();
    let args = view.read_with(visual, |s, cx| {
        s.terminal()
            .unwrap()
            .read(cx)
            .launched()
            .unwrap()
            .args
            .clone()
    });
    assert!(args.iter().any(|arg| arg == "--session-id"));
    assert!(!args.iter().any(|arg| arg == "--resume"));
    assert!(visual.debug_bounds("tui-exit-bar").is_none());
    // A first launch has no id to start over from, only Restart.
    let terminal = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    terminal.update(visual, |t, cx| t.fake_exit(Some(1), cx));
    visual.run_until_parked();
    assert!(visual.debug_bounds("tui-start-fresh").is_none());
    let restart = visual.debug_bounds("tui-restart").unwrap();
    visual.simulate_click(restart.center(), Default::default());
    visual.run_until_parked();
    assert!(view.read_with(visual, |s, _| s.running() && s.exit().is_none()));
    assert!(!events.borrow().contains(&TuiEvent::Close));
}

#[gpui::test]
fn relaunching_after_a_claim_ends_the_old_turn(cx: &mut TestAppContext) {
    let f = Fixture::new(cx, "claude", Some("conv-1"));
    f.start(cx);
    f.session.update(cx, |s, cx| s.set_busy(true, cx));
    f.output(cx, NO_CONVERSATION);
    f.exit(cx, Some(1));
    // Relaunched under --session-id: nothing reported, but no longer busy.
    assert_eq!(after(&f.args(cx), "--session-id"), "conv-1");
    assert!(!f.session.read_with(cx, |s, _| s.busy()));
    assert_eq!(
        f.events()
            .into_iter()
            .filter(|e| matches!(e, TuiEvent::Busy(_)))
            .collect::<Vec<_>>(),
        [TuiEvent::Busy(true), TuiEvent::Busy(false)]
    );
}

#[test]
fn a_session_with_a_worktree_has_one_even_when_it_was_removed() {
    let store = store_in(&temp("wt"));
    let summary = terminal_session(&store, "claude", None);
    assert!(!prepare(request(&store, &summary)).unwrap().has_worktree);
    save_settings(&store, summary.id, |settings| {
        settings.worktree_cwd = Some("/work/project-wt".into());
    });
    assert!(prepare(request(&store, &summary)).unwrap().has_worktree);
    save_settings(&store, summary.id, |settings| {
        settings.worktree_removed = true
    });
    let prepared = prepare(request(&store, &summary)).unwrap();
    assert!(
        prepared.has_worktree,
        "TS hides the button whenever worktreeCwd is set"
    );
    assert_eq!(
        prepared.cwd, PROJECT,
        "but the CLI runs in the project folder"
    );
}

#[gpui::test]
fn a_removed_worktree_still_hides_new_worktree(cx: &mut TestAppContext) {
    let store = store_in(&temp("wt-view"));
    let summary = terminal_session(&store, "claude", None);
    save_settings(&store, summary.id, |settings| {
        settings.worktree_cwd = Some("/work/project-wt".into());
        settings.worktree_removed = true;
    });
    let (view, visual) = cx.add_window_view(|_, _| TuiSession::new(&summary, store, None));
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("tui-new-worktree").is_some(),
        "unknown until the first launch"
    );
    view.update(visual, |s, cx| s.start(cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("tui-header").is_some());
    assert!(visual.debug_bounds("tui-new-worktree").is_none());
}

#[test]
fn the_new_worktree_tooltip_is_the_web_apps_copy() {
    // Copied from the `title` of the disabled button in
    // `src/features/sessions/ui/TuiSessionPane.tsx`.
    assert_eq!(
        NEW_WORKTREE_TIP,
        "Not available in terminal sessions. A terminal session runs in the current checkout. Use a chat session to start in a new worktree."
    );
}

#[gpui::test]
fn the_tooltip_shows_when_hovering_the_disabled_new_worktree_button(cx: &mut TestAppContext) {
    let (_, _, visual) = window(cx, "claude", None, None);
    visual.run_until_parked();
    let tip: &'static str = Box::leak(format!("tui-tip:{NEW_WORKTREE_TIP}").into_boxed_str());
    assert!(visual.debug_bounds(tip).is_none());
    let button = visual.debug_bounds("tui-new-worktree").unwrap();
    visual.simulate_mouse_move(button.center(), None, Default::default());
    visual.executor().advance_clock(Duration::from_millis(1000));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds(tip).is_some(),
        "the card is attached to the button"
    );
    // Away from the button it goes.
    visual.simulate_mouse_move(
        gpui::point(gpui::px(900.), gpui::px(700.)),
        None,
        Default::default(),
    );
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds(tip).is_none());
}

fn terminal_has_keyboard(view: &Entity<TuiSession>, visual: &mut gpui::VisualTestContext) -> bool {
    let terminal = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    visual.update(|w, cx| terminal.read(cx).focus_handle(cx).is_focused(w))
}
fn run_and_draw(view: &Entity<TuiSession>, visual: &mut gpui::VisualTestContext) {
    view.update(visual, |s, cx| s.start(cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
}
fn fail_resume(view: &Entity<TuiSession>, visual: &mut gpui::VisualTestContext) {
    let terminal = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    terminal.update(visual, |t, cx| {
        t.fake_output(NO_CONVERSATION, cx);
        t.fake_exit(Some(1), cx);
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
}

#[gpui::test]
fn a_claim_relaunch_keeps_the_keyboard_when_the_old_terminal_had_it(cx: &mut TestAppContext) {
    let (view, _, visual) = window(cx, "claude", Some("conv-1"), None);
    run_and_draw(&view, visual);
    view.update_in(visual, |s, w, cx| s.focus(w, cx));
    visual.refresh().unwrap();
    assert!(terminal_has_keyboard(&view, visual));
    let first = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    assert!(first.read_with(visual, |t, _| t.has_focus()), "tracked");
    fail_resume(&view, visual);
    let second = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    assert_ne!(first, second, "relaunched under --session-id");
    assert!(terminal_has_keyboard(&view, visual));
}

#[gpui::test]
fn a_claim_relaunch_of_an_unfocused_pane_does_not_take_the_keyboard(cx: &mut TestAppContext) {
    let (view, _, visual) = window(cx, "claude", Some("conv-1"), None);
    run_and_draw(&view, visual);
    assert!(!terminal_has_keyboard(&view, visual));
    fail_resume(&view, visual);
    assert!(!terminal_has_keyboard(&view, visual));
}

#[gpui::test]
fn a_claim_relaunch_after_the_user_looked_away_does_not_take_the_keyboard(cx: &mut TestAppContext) {
    let (view, _, visual) = window(cx, "claude", Some("conv-1"), None);
    run_and_draw(&view, visual);
    view.update_in(visual, |s, w, cx| s.focus(w, cx));
    visual.refresh().unwrap();
    assert!(terminal_has_keyboard(&view, visual));
    // Focus moves elsewhere before the process exits.
    visual.update(|w, cx| w.blur(cx));
    visual.refresh().unwrap();
    fail_resume(&view, visual);
    assert!(!terminal_has_keyboard(&view, visual));
}

#[gpui::test]
fn start_new_conversation_takes_the_keyboard_for_the_new_process(cx: &mut TestAppContext) {
    // The click happens in the visible pane, so the new process takes the keyboard
    // even when the old terminal never had it.
    let (view, _, visual) = window(cx, "claude", Some("conv-1"), None);
    run_and_draw(&view, visual);
    assert!(!terminal_has_keyboard(&view, visual));
    let terminal = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    terminal.update(visual, |t, cx| t.fake_exit(Some(1), cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(view.read_with(visual, |s, _| s.offers_fresh()));
    view.update(visual, |s, cx| s.start_fresh(cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(terminal_has_keyboard(&view, visual));
}

#[gpui::test]
fn restart_keeps_the_keyboard(cx: &mut TestAppContext) {
    let (view, _, visual) = window(cx, "claude", None, None);
    run_and_draw(&view, visual);
    view.update_in(visual, |s, w, cx| s.focus(w, cx));
    visual.refresh().unwrap();
    let terminal = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    terminal.update(visual, |t, cx| t.fake_exit(Some(1), cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    view.update(visual, |s, cx| s.restart(cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(terminal_has_keyboard(&view, visual));
    // And a second Restart keeps it too.
    let terminal = view.read_with(visual, |s, _| s.terminal().cloned().unwrap());
    terminal.update(visual, |t, cx| t.fake_exit(Some(1), cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    view.update(visual, |s, cx| s.restart(cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(terminal_has_keyboard(&view, visual));
}
