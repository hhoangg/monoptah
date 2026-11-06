use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use model::Gh;
use monocode_core::codespaces::{AuthStatus, GhOutput};
use std::{collections::BTreeMap, sync::Mutex};

const PROJECT: &str = "/work/alpha";
const REPO: &str = "acme/alpha";
const AUTH_OK: &str = "github.com\n  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n  - Token scopes: 'codespace', 'repo'\n";
const AUTH_NO_SCOPE: &str = "github.com\n  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n  - Token scopes: 'repo'\n";

/// A scripted `gh`: auth text, the user's codespaces secrets and the values set so far.
struct FakeGh {
    auth: Mutex<Result<String, String>>,
    secrets: Mutex<BTreeMap<String, Vec<String>>>,
    fail_set: Mutex<Vec<String>>,
    sets: Mutex<Vec<(String, String, Vec<String>)>>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl FakeGh {
    fn new(auth: Result<&str, &str>) -> Arc<Self> {
        Arc::new(Self {
            auth: Mutex::new(auth.map(str::to_owned).map_err(str::to_owned)),
            secrets: Mutex::new(BTreeMap::new()),
            fail_set: Mutex::new(Vec::new()),
            sets: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        })
    }
    fn with_secret(self: Arc<Self>, key: &str, repos: &[&str]) -> Arc<Self> {
        self.secrets
            .lock()
            .unwrap()
            .insert(key.into(), repos.iter().map(|r| r.to_string()).collect());
        self
    }
    fn failing(self: Arc<Self>, key: &str) -> Arc<Self> {
        self.fail_set.lock().unwrap().push(key.into());
        self
    }
    fn sets(&self) -> Vec<(String, String)> {
        self.sets
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect()
    }
    fn ok(stdout: String) -> Result<GhOutput, String> {
        Ok(GhOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

impl GhRunner for FakeGh {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
        self.calls.lock().unwrap().push(args.to_vec());
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["auth", ..] => self.auth.lock().unwrap().clone().map(|stdout| GhOutput {
                success: true,
                stdout,
                stderr: String::new(),
            }),
            ["api", "--paginate", "--slurp", path]
                if path.starts_with("/user/codespaces/secrets?") =>
            {
                let names: Vec<_> = self
                    .secrets
                    .lock()
                    .unwrap()
                    .keys()
                    .map(|name| serde_json::json!({ "name": name }))
                    .collect();
                Self::ok(serde_json::json!([{ "secrets": names }]).to_string())
            }
            ["api", "--paginate", "--slurp", path] => {
                let key = path
                    .trim_start_matches("/user/codespaces/secrets/")
                    .split('/')
                    .next()
                    .unwrap();
                let repos: Vec<_> = self
                    .secrets
                    .lock()
                    .unwrap()
                    .get(key)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|name| serde_json::json!({ "full_name": name }))
                    .collect();
                Self::ok(serde_json::json!([{ "repositories": repos }]).to_string())
            }
            ["secret", "set", key, .., repos] => {
                if self.fail_set.lock().unwrap().iter().any(|k| k == key) {
                    return Ok(GhOutput {
                        success: false,
                        stdout: String::new(),
                        stderr: "HTTP 422".into(),
                    });
                }
                let repos: Vec<String> = repos.split(',').map(str::to_owned).collect();
                self.secrets
                    .lock()
                    .unwrap()
                    .insert((*key).into(), repos.clone());
                self.sets.lock().unwrap().push((
                    (*key).into(),
                    stdin.unwrap_or_default().into(),
                    repos,
                ));
                Self::ok(String::new())
            }
            other => panic!("unexpected gh call {other:?}"),
        }
    }
}

fn origin(_: &Path) -> Option<String> {
    Some(format!("git@github.com:{REPO}.git"))
}
fn no_origin(_: &Path) -> Option<String> {
    None
}
fn claude() -> Option<PathBuf> {
    Some(PathBuf::from("/usr/bin/true"))
}
fn no_claude() -> Option<PathBuf> {
    None
}

fn env(gh: &Arc<FakeGh>, origin: fn(&Path) -> Option<String>) -> Env {
    Env {
        runner: gh.clone(),
        origin,
        claude,
    }
}

fn open(cx: &mut TestAppContext, env: Env) -> WindowHandle<CodespacesDialog> {
    let window = cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| CodespacesDialog::new(PROJECT.into(), "Alpha".into(), env, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
}

fn read<T>(
    window: &WindowHandle<CodespacesDialog>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&CodespacesDialog, &gpui::App) -> T,
) -> T {
    window.read_with(cx, |dialog, cx| f(dialog, cx)).unwrap()
}

fn act(
    window: &WindowHandle<CodespacesDialog>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut CodespacesDialog, &mut Context<CodespacesDialog>),
) {
    window.update(cx, |dialog, _, cx| f(dialog, cx)).unwrap();
    cx.run_until_parked();
}

fn type_in(
    window: &WindowHandle<CodespacesDialog>,
    cx: &mut TestAppContext,
    key: &str,
    text: &str,
) {
    window
        .update(cx, |dialog, _, cx| {
            let (_, input) = dialog.fields.iter().find(|(k, _)| k == key).unwrap();
            input.update(cx, |input, cx| input.set_text(text.into(), cx));
        })
        .unwrap();
}

fn state_of(
    window: &WindowHandle<CodespacesDialog>,
    cx: &mut TestAppContext,
    key: &str,
) -> Option<SecretState> {
    read(window, cx, |dialog, _| {
        dialog
            .snapshot
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .find(|row| row.key == key)
            .unwrap()
            .state
    })
}

#[test]
fn gh_states_map_to_a_message_and_the_exact_fix() {
    let status = |logged_in, scope| AuthStatus {
        logged_in,
        user: Some("octo".into()),
        scopes: vec![],
        has_codespace_scope: scope,
    };
    assert_eq!(
        model::gh_state(Err("GitHub CLI (gh) is not installed".into())),
        Gh::Missing
    );
    assert_eq!(model::gh_state(Ok(status(false, false))), Gh::NotLoggedIn);
    assert_eq!(
        model::gh_state(Ok(status(true, false))).fix(),
        Some("gh auth refresh -h github.com -s codespace")
    );
    assert_eq!(
        model::gh_state(Ok(status(false, false))).fix(),
        Some("gh auth login")
    );
    let ready = model::gh_state(Ok(status(true, true)));
    assert!(ready.ready() && ready.fix().is_none());
    assert!(matches!(model::gh_state(Err("boom".into())), Gh::Failed(_)));
}

#[test]
fn only_non_empty_fields_are_saved_and_trimmed() {
    let fields = vec![
        ("A".to_owned(), "  one\n".to_owned()),
        ("B".to_owned(), "   ".to_owned()),
        ("C".to_owned(), String::new()),
        ("D".to_owned(), "four".to_owned()),
    ];
    assert_eq!(
        model::to_save(&fields),
        vec![("A".into(), "one".into()), ("D".into(), "four".into())]
    );
}

#[test]
fn load_maps_secret_status_and_always_lists_the_claude_token() {
    let gh = FakeGh::new(Ok(AUTH_OK))
        .with_secret("CLAUDE_CODE_OAUTH_TOKEN", &[REPO])
        .with_secret("OTHER", &["acme/else"]);
    let snapshot = model::load(
        gh.as_ref(),
        Path::new("/nonexistent"),
        Some("https://github.com/acme/alpha"),
    );
    assert_eq!(snapshot.repo.as_deref(), Some(REPO));
    assert!(snapshot.gh.ready());
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].key, "CLAUDE_CODE_OAUTH_TOKEN");
    assert_eq!(snapshot.rows[0].state, Some(SecretState::Set));
}

#[test]
fn devcontainer_secrets_get_a_row_each_with_their_status() {
    let dir = std::env::temp_dir().join(format!(
        "monoptah-cs-dialog-{}",
        monocode_core::mono::new_id()
    ));
    std::fs::create_dir_all(dir.join(".devcontainer")).unwrap();
    std::fs::write(
        dir.join(".devcontainer/devcontainer.json"),
        r#"{ // comment
  "secrets": { "NPM_TOKEN": { "description": "npm" }, "OTHER": {}, }
}"#,
    )
    .unwrap();
    let gh = FakeGh::new(Ok(AUTH_OK))
        .with_secret("NPM_TOKEN", &["acme/else"])
        .with_secret("OTHER", &[REPO]);
    let snapshot = model::load(gh.as_ref(), &dir, Some("https://github.com/acme/alpha.git"));
    std::fs::remove_dir_all(dir).ok();
    let states: Vec<_> = snapshot
        .rows
        .iter()
        .map(|r| (r.key.as_str(), r.state))
        .collect();
    assert_eq!(
        states,
        vec![
            ("CLAUDE_CODE_OAUTH_TOKEN", Some(SecretState::Missing)),
            ("NPM_TOKEN", Some(SecretState::NotForRepo)),
            ("OTHER", Some(SecretState::Set)),
        ]
    );
}

#[test]
fn an_unready_gh_or_a_missing_repo_reads_no_secrets() {
    let gh = FakeGh::new(Ok(AUTH_NO_SCOPE));
    let snapshot = model::load(
        gh.as_ref(),
        Path::new("/x"),
        Some("git@github.com:acme/alpha.git"),
    );
    assert!(matches!(snapshot.gh, Gh::MissingScope(_)));
    assert!(snapshot.rows.iter().all(|r| r.state.is_none()));
    let gh = FakeGh::new(Ok(AUTH_OK));
    let snapshot = model::load(gh.as_ref(), Path::new("/x"), None);
    assert!(snapshot.repo.is_none());
    assert!(snapshot.rows.iter().all(|r| r.state.is_none()));
    let calls: Vec<_> = gh
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c[0].clone())
        .collect();
    assert_eq!(calls, ["auth"]);
}

#[gpui::test]
fn the_dialog_loads_status_and_renders_rows(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK)).with_secret("CLAUDE_CODE_OAUTH_TOKEN", &[REPO]);
    let window = open(cx, env(&gh, origin));
    assert_eq!(
        state_of(&window, cx, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some(SecretState::Set)
    );
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(
        visual
            .debug_bounds("codespaces-status-CLAUDE_CODE_OAUTH_TOKEN")
            .is_some()
    );
    assert!(visual.debug_bounds("codespaces-generate").is_some());
    assert!(visual.debug_bounds("codespaces-save").is_some());
}

#[gpui::test]
fn save_stores_only_typed_fields_clears_them_and_refreshes_status(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK));
    let window = open(cx, env(&gh, origin));
    assert_eq!(
        state_of(&window, cx, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some(SecretState::Missing)
    );
    // Nothing typed: Save does nothing.
    act(&window, cx, |dialog, cx| dialog.save(cx));
    assert!(gh.sets().is_empty());
    type_in(
        &window,
        cx,
        "CLAUDE_CODE_OAUTH_TOKEN",
        "  sk-ant-oat-secret ",
    );
    act(&window, cx, |dialog, cx| dialog.save(cx));
    assert_eq!(
        gh.sets(),
        vec![("CLAUDE_CODE_OAUTH_TOKEN".into(), "sk-ant-oat-secret".into())]
    );
    assert_eq!(gh.sets.lock().unwrap()[0].2, vec![REPO.to_owned()]);
    // The secret reached gh over stdin, never argv.
    assert!(
        gh.calls
            .lock()
            .unwrap()
            .iter()
            .all(|argv| !argv.iter().any(|a| a.contains("sk-ant-oat-secret")))
    );
    read(&window, cx, |dialog, cx| {
        assert!(
            dialog
                .fields
                .iter()
                .all(|(_, i)| i.read(cx).content.is_empty())
        );
        assert!(!dialog.busy);
        assert!(dialog.error.is_none());
        assert!(
            dialog
                .notice
                .as_deref()
                .unwrap()
                .contains("CLAUDE_CODE_OAUTH_TOKEN")
        );
    });
    assert_eq!(
        state_of(&window, cx, "CLAUDE_CODE_OAUTH_TOKEN"),
        Some(SecretState::Set)
    );
}

#[gpui::test]
fn a_failed_save_keeps_that_value_and_reports_it(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK)).failing("CLAUDE_CODE_OAUTH_TOKEN");
    let window = open(cx, env(&gh, origin));
    type_in(&window, cx, "CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-tok");
    act(&window, cx, |dialog, cx| dialog.save(cx));
    read(&window, cx, |dialog, cx| {
        assert_eq!(dialog.fields[0].1.read(cx).content, "sk-ant-tok");
        assert!(
            dialog
                .error
                .as_deref()
                .unwrap()
                .contains("CLAUDE_CODE_OAUTH_TOKEN")
        );
    });
}

#[gpui::test]
fn save_is_blocked_while_gh_is_not_ready_or_the_repo_is_unknown(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_NO_SCOPE));
    let window = open(cx, env(&gh, origin));
    type_in(&window, cx, "CLAUDE_CODE_OAUTH_TOKEN", "tok");
    act(&window, cx, |dialog, cx| dialog.save(cx));
    assert!(gh.sets().is_empty());
    let gh = FakeGh::new(Ok(AUTH_OK));
    let window = open(cx, env(&gh, no_origin));
    type_in(&window, cx, "CLAUDE_CODE_OAUTH_TOKEN", "tok");
    act(&window, cx, |dialog, cx| dialog.save(cx));
    assert!(gh.sets().is_empty());
}

#[gpui::test]
fn generate_opens_a_local_terminal_or_reports_a_missing_claude(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK));
    let window = open(cx, env(&gh, origin));
    act(&window, cx, |dialog, cx| dialog.generate_token(cx));
    read(&window, cx, |dialog, cx| {
        let terminal = dialog.terminal.as_ref().expect("terminal");
        let spec = terminal.read(cx).launched().unwrap();
        assert_eq!(spec.program, std::ffi::OsString::from("/usr/bin/true"));
        assert_eq!(spec.args, vec![std::ffi::OsString::from("setup-token")]);
    });
    act(&window, cx, |dialog, cx| dialog.shutdown(cx));
    read(&window, cx, |dialog, _| assert!(dialog.terminal.is_none()));
    let window = open(
        cx,
        Env {
            claude: no_claude,
            ..env(&gh, origin)
        },
    );
    act(&window, cx, |dialog, cx| dialog.generate_token(cx));
    read(&window, cx, |dialog, _| {
        assert!(dialog.terminal.is_none());
        assert!(dialog.error.as_deref().unwrap().contains("not installed"));
    });
}

#[gpui::test]
fn closing_the_dialog_drops_typed_values(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK));
    let window = open(cx, env(&gh, origin));
    type_in(&window, cx, "CLAUDE_CODE_OAUTH_TOKEN", "tok");
    act(&window, cx, |dialog, cx| dialog.shutdown(cx));
    read(&window, cx, |dialog, cx| assert!(!dialog.has_input(cx)));
}

#[gpui::test]
fn escape_in_the_terminal_never_closes_the_dialog(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK));
    let window = open(cx, env(&gh, origin));
    let closed: std::rc::Rc<std::cell::Cell<u32>> = Default::default();
    let seen = closed.clone();
    window
        .update(cx, |_, _, cx| {
            cx.subscribe(&cx.entity(), move |_, _, _: &Event, _| {
                seen.set(seen.get() + 1)
            })
            .detach()
        })
        .unwrap();
    act(&window, cx, |dialog, cx| dialog.generate_token(cx));
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        let handle = window
            .root::<CodespacesDialog>()
            .flatten()
            .unwrap()
            .read(cx)
            .terminal
            .as_ref()
            .unwrap()
            .read(cx)
            .focus_handle(cx);
        window.focus(&handle, cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        let dialog = window.root::<CodespacesDialog>().flatten().unwrap();
        assert!(dialog.read(cx).terminal_focused(window, cx));
    });
    visual.simulate_keystrokes("escape");
    assert_eq!(closed.get(), 0);
    visual.update(|window, cx| {
        let dialog = window.root::<CodespacesDialog>().flatten().unwrap();
        assert!(dialog.read(cx).terminal.is_some());
        // With the dialog itself focused, Escape still closes it.
        let focus = dialog.read(cx).focus.clone();
        window.focus(&focus, cx);
    });
    visual.simulate_keystrokes("escape");
    assert_eq!(closed.get(), 1);
}

fn field_center(
    window: &WindowHandle<CodespacesDialog>,
    cx: &mut TestAppContext,
) -> gpui::Point<gpui::Pixels> {
    cx.refresh().unwrap();
    let visual = &mut VisualTestContext::from_window((*window).into(), cx);
    visual
        .debug_bounds("codespaces-field-CLAUDE_CODE_OAUTH_TOKEN")
        .expect("the token field is rendered")
        .center()
}

fn typed_token(window: &WindowHandle<CodespacesDialog>, cx: &mut TestAppContext) -> String {
    read(window, cx, |dialog, cx| {
        dialog.fields[0].1.read(cx).content.to_string()
    })
}

#[gpui::test]
fn clicking_the_field_and_typing_enters_text_when_gh_and_repo_are_ready(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK)).with_secret("CLAUDE_CODE_OAUTH_TOKEN", &[REPO]);
    let window = open(cx, env(&gh, origin));
    let center = field_center(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_click(center, Default::default());
    visual.simulate_input("sk-ant-oat-123");
    assert_eq!(typed_token(&window, cx), "sk-ant-oat-123");
    // Pasting goes through the same path as typing.
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.write_to_clipboard(gpui::ClipboardItem::new_string("-pasted".into()));
    visual.simulate_keystrokes("cmd-v");
    assert_eq!(typed_token(&window, cx), "sk-ant-oat-123-pasted");
    assert!(visual.debug_bounds("codespaces-disabled").is_none());
    assert!(
        visual
            .debug_bounds("codespaces-unknown-CLAUDE_CODE_OAUTH_TOKEN")
            .is_none()
    );
}

#[gpui::test]
fn a_gh_without_the_scope_blocks_typing_and_says_why(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_NO_SCOPE));
    let window = open(cx, env(&gh, origin));
    let center = field_center(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_click(center, Default::default());
    visual.simulate_input("nope");
    assert_eq!(typed_token(&window, cx), "");
    assert!(visual.debug_bounds("codespaces-disabled").is_some());
    assert!(
        visual
            .debug_bounds("codespaces-unknown-CLAUDE_CODE_OAUTH_TOKEN")
            .is_some()
    );
}

#[gpui::test]
fn a_missing_repository_blocks_typing_and_says_why(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK));
    let window = open(cx, env(&gh, no_origin));
    let center = field_center(&window, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.simulate_click(center, Default::default());
    visual.simulate_input("nope");
    assert_eq!(typed_token(&window, cx), "");
    assert!(visual.debug_bounds("codespaces-disabled").is_some());
}

#[test]
fn disabled_and_unknown_reasons_name_the_actual_problem() {
    let load = |auth, origin| {
        let gh = FakeGh::new(auth);
        model::load(gh.as_ref(), Path::new("/x"), origin)
    };
    let ready = load(Ok(AUTH_OK), Some("git@github.com:acme/alpha.git"));
    assert_eq!(ready.disabled_reason(), None);
    assert_eq!(ready.unknown_reason(&ready.rows[0]), None);

    let no_scope = load(Ok(AUTH_NO_SCOPE), Some("git@github.com:acme/alpha.git"));
    assert!(
        no_scope
            .disabled_reason()
            .unwrap()
            .contains("codespace scope")
    );
    assert!(
        no_scope
            .unknown_reason(&no_scope.rows[0])
            .unwrap()
            .contains("codespace scope")
    );

    let no_repo = load(Ok(AUTH_OK), None);
    assert_eq!(
        no_repo.disabled_reason().as_deref(),
        Some(model::NO_REPO_LINE)
    );

    let broken = load(
        Err("exec format error"),
        Some("git@github.com:acme/alpha.git"),
    );
    assert!(
        broken
            .disabled_reason()
            .unwrap()
            .contains("exec format error")
    );

    // gh is fine but the secrets read failed: inputs stay usable, the row shows the error.
    let mut failed = load(Ok(AUTH_OK), Some("git@github.com:acme/alpha.git"));
    failed.rows[0].state = None;
    failed.error = Some("HTTP 403: Resource not accessible".into());
    assert_eq!(failed.disabled_reason(), None);
    assert_eq!(
        failed.unknown_reason(&failed.rows[0]).as_deref(),
        Some("HTTP 403: Resource not accessible")
    );
}

#[gpui::test]
fn typing_is_blocked_while_the_dialog_is_busy_and_it_says_checking(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_OK));
    let window = open(cx, env(&gh, origin));
    act(&window, cx, |dialog, cx| {
        dialog.busy = true;
        cx.notify();
    });
    let center = field_center(&window, cx);
    let visual = &mut VisualTestContext::from_window(*window, cx);
    visual.simulate_click(center, Default::default());
    visual.simulate_input("nope");
    assert_eq!(typed_token(&window, cx), "");
    assert!(visual.debug_bounds("codespaces-checking").is_some());
    assert!(visual.debug_bounds("codespaces-disabled").is_none());
}

#[gpui::test]
fn checking_again_after_fixing_gh_enables_the_field(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Ok(AUTH_NO_SCOPE));
    let window = open(cx, env(&gh, origin));
    let center = field_center(&window, cx);
    let visual = &mut VisualTestContext::from_window(*window, cx);
    visual.simulate_click(center, Default::default());
    visual.simulate_input("a");
    assert_eq!(typed_token(&window, cx), "");
    assert!(visual.debug_bounds("codespaces-disabled").is_some());

    *gh.auth.lock().unwrap() = Ok(AUTH_OK.to_owned());
    act(&window, cx, |dialog, cx| dialog.refresh(cx));
    let center = field_center(&window, cx);
    let visual = &mut VisualTestContext::from_window(*window, cx);
    assert!(visual.debug_bounds("codespaces-disabled").is_none());
    visual.simulate_click(center, Default::default());
    visual.simulate_input("ok");
    assert_eq!(typed_token(&window, cx), "ok");
}

#[gpui::test]
fn a_gh_that_times_out_ends_busy_and_the_fields_explain_why(cx: &mut TestAppContext) {
    let gh = FakeGh::new(Err(
        "GitHub CLI did not answer within 20 seconds (`gh auth`).",
    ));
    let window = open(cx, env(&gh, origin));
    read(&window, cx, |dialog, _| {
        assert!(!dialog.busy);
        let snapshot = dialog.snapshot.as_ref().unwrap();
        assert!(matches!(snapshot.gh, Gh::Failed(_)));
        assert!(
            snapshot
                .unknown_reason(&snapshot.rows[0])
                .unwrap()
                .contains("did not answer within 20 seconds")
        );
    });
    let center = field_center(&window, cx);
    let visual = &mut VisualTestContext::from_window(*window, cx);
    visual.simulate_click(center, Default::default());
    visual.simulate_input("x");
    assert_eq!(typed_token(&window, cx), "");
    assert!(visual.debug_bounds("codespaces-disabled").is_some());
    assert!(
        visual
            .debug_bounds("codespaces-unknown-CLAUDE_CODE_OAUTH_TOKEN")
            .is_some()
    );
}
