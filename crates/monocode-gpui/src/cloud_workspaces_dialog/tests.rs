//! The Cloud workspaces dialog with `gh` and `sbx` faked: both backends' rows, orphans, the
//! estimated cost, Stop and Delete (a hibernating sandbox included), and a missing `sbx`.
use super::*;
use crate::cloud_mark::Tone;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use monocode_core::{
    cloud_sessions::{
        Host,
        backend::docker::{SandboxShell, ShellOutput},
        docker_start::{ClaudeAuth, SandboxSecrets},
        workspaces::{WorkspaceOverview, WorkspaceState},
    },
    codespaces::{GhOutput, GhRunner},
    remote::Machine,
    sandboxes::{SbxOutput, SbxRunner},
    store::Store,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const AUTH_OK: &str = "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const USED: &str = "octo-used";
const LOST: &str = "octo-lost";
const SBX_USED: &str = "monoptah-fix-login-a1b2c3";
const SBX_LOST: &str = "monoptah-main-d4e5f6";

/// A time `hours` before now, as gh and sbx write it.
fn hours_ago(hours: f64) -> String {
    let ms = monocode_core::time::now_ms() - (hours * 3_600_000.0) as u64;
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------- fakes

pub(crate) struct Gh {
    pub auth: Mutex<Result<GhOutput, String>>,
    pub list: Mutex<String>,
    pub calls: Mutex<Vec<String>>,
}

impl Gh {
    pub(crate) fn new(list: Value) -> Arc<Self> {
        Arc::new(Self {
            auth: Mutex::new(Ok(ok(AUTH_OK))),
            list: Mutex::new(list.to_string()),
            calls: Mutex::new(vec![]),
        })
    }
    pub(crate) fn count(&self, word: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains(word))
            .count()
    }
}

fn ok(stdout: &str) -> GhOutput {
    GhOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

impl GhRunner for Gh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        if line.starts_with("auth status") {
            return self.auth.lock().unwrap().clone();
        }
        if line.starts_with("codespace list") {
            return Ok(ok(&self.list.lock().unwrap()));
        }
        if line.starts_with("codespace view") {
            return Ok(ok(r#"{"state":"Available"}"#));
        }
        if line.starts_with("codespace delete") {
            // Gone from the account's list from now on.
            let name = &args[3];
            let mut list = self.list.lock().unwrap();
            let mut items: Vec<Value> = serde_json::from_str(&list).unwrap();
            items.retain(|item| item["name"] != name.as_str());
            *list = Value::Array(items).to_string();
            return Ok(ok(""));
        }
        if line.starts_with("codespace stop") {
            return Ok(ok(""));
        }
        if line.starts_with("api user/codespaces/") {
            return Ok(ok("https://octo-used.github.dev\n"));
        }
        panic!("unexpected gh call: {line}");
    }
}

pub(crate) fn codespace(name: &str, machine: &str, state: &str, created: String) -> Value {
    json!({
        "name": name, "displayName": format!("monoptah-{name}"), "state": state,
        "gitStatus": {"ref": "fix/login"}, "machineName": machine,
        "repository": "acme/alpha", "lastUsedAt": hours_ago(0.5), "createdAt": created,
    })
}

#[derive(Clone)]
pub(crate) struct Bx {
    pub id: &'static str,
    pub name: &'static str,
    pub state: String,
    pub cpus: u32,
    pub memory_mib: u64,
    pub created: String,
}

pub(crate) struct Sbx {
    pub boxes: Mutex<Vec<Bx>>,
    pub calls: Mutex<Vec<String>>,
    pub ls_reply: Mutex<Option<Result<SbxOutput, String>>>,
}

impl Sbx {
    pub(crate) fn new(boxes: Vec<Bx>) -> Arc<Self> {
        Arc::new(Self {
            boxes: Mutex::new(boxes),
            calls: Mutex::new(vec![]),
            ls_reply: Mutex::new(None),
        })
    }
    pub(crate) fn verbs(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.split_whitespace().next().unwrap().to_owned())
            .collect()
    }
    pub(crate) fn gone(&self, id: &str) -> bool {
        !self.boxes.lock().unwrap().iter().any(|b| b.id == id)
    }
}

fn json_of(b: &Bx) -> Value {
    json!({"id": b.id, "name": b.name, "agent": "shell", "state": b.state, "cpus": b.cpus,
        "memory_mib": b.memory_mib, "created_at": b.created})
}

fn sbx_ok(stdout: String) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success: true,
        stdout,
        stderr: String::new(),
    })
}

impl SbxRunner for Sbx {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<SbxOutput, String> {
        let args: Vec<&str> = args
            .iter()
            .map(String::as_str)
            .filter(|a| !matches!(*a, "--cloud" | "--no-input"))
            .collect();
        self.calls.lock().unwrap().push(args.join(" "));
        let mut boxes = self.boxes.lock().unwrap();
        match args[0] {
            "ls" => match self.ls_reply.lock().unwrap().clone() {
                Some(reply) => reply,
                None => sbx_ok(json!(boxes.iter().map(json_of).collect::<Vec<_>>()).to_string()),
            },
            "inspect" => {
                let Some(b) = boxes.iter_mut().find(|b| b.id == args[1]) else {
                    return Ok(SbxOutput {
                        success: false,
                        stdout: String::new(),
                        stderr: format!("error: sandbox '{}' not found", args[1]),
                    });
                };
                // A box that is going down has arrived by the time it is asked about.
                if matches!(b.state.as_str(), "stopping" | "hibernating") {
                    b.state = "stopped".into();
                }
                sbx_ok(json_of(b).to_string())
            }
            "stop" => {
                if let Some(b) = boxes.iter_mut().find(|b| b.id == args[1]) {
                    b.state = "stopping".into();
                }
                sbx_ok(String::new())
            }
            "rm" => {
                let id = args[args.len() - 1];
                boxes.retain(|b| b.id != id);
                sbx_ok(String::new())
            }
            other => panic!("unexpected sbx call: {other}"),
        }
    }
}

struct NoSsh;
impl SandboxShell for NoSsh {
    fn run(
        &self,
        _: &str,
        remote: &str,
        _: &str,
        _: &mut dyn FnMut(&str),
        _: &dyn Fn() -> bool,
        _: std::time::Duration,
    ) -> Result<ShellOutput, String> {
        panic!("ssh must not run: {remote}");
    }
}

struct NoSecrets;
impl SandboxSecrets for NoSecrets {
    fn claude_auth(&self, _: Option<&str>) -> Result<Option<ClaudeAuth>, String> {
        Ok(None)
    }
    fn git_payload(&self, _: &str, _: &str) -> Result<zeroize::Zeroizing<String>, String> {
        Err("no credentials in this test".into())
    }
}

#[derive(Default)]
pub(crate) struct FakeHost(pub Mutex<Vec<String>>, AtomicBool);
impl Host for FakeHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(vec![])
    }
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &AtomicBool,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        panic!("the overview must not connect to a workspace")
    }
    fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        panic!("the overview must not wake {id}")
    }
    fn request(&self, id: &str, method: &str, _: Value) -> Result<Value, String> {
        panic!("unexpected host request {id} {method}")
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("disconnect {id}"));
        let _ = self.1.load(Ordering::Relaxed);
        Ok(())
    }
}

/// A real git checkout whose origin is the GitHub repository `acme/alpha`; removed on drop.
pub(crate) struct Checkout(pub PathBuf);
impl Checkout {
    pub(crate) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "monoptah-workspaces-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["remote", "add", "origin", "git@github.com:acme/alpha.git"],
        ] {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }
        Self(dir)
    }
    pub(crate) fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn memory_store() -> Database {
    Arc::new(Mutex::new(
        Store::open(std::path::Path::new(":memory:")).unwrap(),
    ))
}

pub(crate) fn api(store: &Database, gh: &Arc<Gh>, sbx: &Arc<Sbx>) -> Arc<CloudApi> {
    Arc::new(CloudApi::with_sandboxes(
        gh.clone(),
        Arc::new(FakeHost::default()),
        store.clone(),
        sbx.clone(),
        Arc::new(NoSsh),
        Arc::new(NoSecrets),
    ))
}

/// A live session of `local` in `cloud`; returns its id.
pub(crate) fn add_session(store: &Database, local: &str, cloud: &CloudRef) -> i64 {
    let store = store.lock().unwrap();
    let project = store
        .remember_cloud_ref_project(
            &format!("env-{}", cloud.name),
            &json!({"id":"p","cwd":"/workspaces/alpha"}),
            local,
            cloud,
        )
        .unwrap();
    store.create("claude", &project.key).unwrap().id
}

/// Two codespaces and two sandboxes; `USED` and `SBX_USED` have a session.
pub(crate) struct World {
    pub checkout: Checkout,
    pub store: Database,
    pub gh: Arc<Gh>,
    pub sbx: Arc<Sbx>,
    pub api: Arc<CloudApi>,
    pub used_session: i64,
    pub sandbox_session: i64,
}

pub(crate) fn world() -> World {
    world_in(memory_store())
}

/// [`world`] over `store`, which already holds other sessions.
pub(crate) fn world_in(store: Database) -> World {
    let checkout = Checkout::new();
    let gh = Gh::new(json!([
        codespace(USED, "basicLinux32gb", "Available", hours_ago(3.0)),
        codespace(LOST, "standardLinux32gb", "Shutdown", hours_ago(100.0)),
    ]));
    let sbx = Sbx::new(vec![
        Bx {
            id: "sbx_001a",
            name: SBX_USED,
            state: "running".into(),
            cpus: 4,
            memory_mib: 8192,
            created: hours_ago(2.0),
        },
        Bx {
            id: "sbx_001b",
            name: SBX_LOST,
            state: "hibernating".into(),
            cpus: 2,
            memory_mib: 4096,
            created: hours_ago(30.0),
        },
    ]);
    let used_session = add_session(&store, &checkout.path(), &CloudRef::codespace(USED));
    let sandbox_session = add_session(&store, &checkout.path(), &CloudRef::sandbox(SBX_USED));
    let api = api(&store, &gh, &sbx);
    World {
        checkout,
        store,
        gh,
        sbx,
        api,
        used_session,
        sandbox_session,
    }
}

// --------------------------------------------------------------------------- helpers

fn open(w: &World, cx: &mut TestAppContext) -> WindowHandle<CloudWorkspacesDialog> {
    let env = Env {
        api: Some(w.api.clone()),
        store: w.store.clone(),
    };
    let project = w.checkout.path();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| CloudWorkspacesDialog::new(project, "Alpha".into(), env, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
}

fn view(
    window: &WindowHandle<CloudWorkspacesDialog>,
    cx: &mut TestAppContext,
) -> WorkspaceOverview {
    window
        .read_with(cx, |d, _| d.view().cloned().expect("loaded"))
        .unwrap()
}

fn act(
    window: &WindowHandle<CloudWorkspacesDialog>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&mut CloudWorkspacesDialog, &mut Context<CloudWorkspacesDialog>),
) {
    window.update(cx, |d, _, cx| f(d, cx)).unwrap();
    cx.run_until_parked();
}

/// Everything the dialog emitted, in order.
fn record(
    window: &WindowHandle<CloudWorkspacesDialog>,
    cx: &mut TestAppContext,
) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let entity = window.entity(cx).unwrap();
    cx.update(|cx| {
        cx.subscribe(&entity, move |_, event: &Event, _| {
            log.lock().unwrap().push(match event {
                Event::Close => "close".to_owned(),
                Event::Act { cloud, action } => format!("act {action:?} {}", cloud.name),
                Event::Stopped(cloud) => format!("stopped {}", cloud.name),
                Event::Deleted { cloud, removed } => format!("deleted {} {removed:?}", cloud.name),
            })
        })
        .detach();
    });
    seen
}

fn draw(
    window: &WindowHandle<CloudWorkspacesDialog>,
    cx: &mut TestAppContext,
) -> VisualTestContext {
    let mut visual = VisualTestContext::from_window((*window).into(), cx);
    visual.refresh().unwrap();
    visual.run_until_parked();
    visual
}

/// `debug_bounds` of a selector built at run time.
fn bounds(visual: &mut VisualTestContext, selector: &str) -> Option<gpui::Bounds<gpui::Pixels>> {
    visual.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

// ---------------------------------------------------------------------------- tests

#[gpui::test]
fn it_lists_a_row_per_workspace_of_both_backends_and_highlights_orphans(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let loaded = view(&window, cx);
    assert_eq!(loaded.rows.len(), 4, "{:?}", loaded.rows);
    assert_eq!(loaded.repo.as_deref(), Some("acme/alpha"));

    let mut visual = draw(&window, cx);
    for name in [USED, LOST, SBX_USED, SBX_LOST] {
        for part in ["row", "state", "cost"] {
            let selector = format!("cloud-ws-{part}-{name}");
            assert!(bounds(&mut visual, &selector).is_some(), "{selector}");
        }
    }
    // Only a workspace nobody uses is marked as an orphan.
    assert!(bounds(&mut visual, &format!("cloud-ws-orphan-{LOST}")).is_some());
    assert!(bounds(&mut visual, &format!("cloud-ws-orphan-{SBX_LOST}")).is_some());
    assert!(bounds(&mut visual, &format!("cloud-ws-orphan-{USED}")).is_none());
    assert!(bounds(&mut visual, &format!("cloud-ws-orphan-{SBX_USED}")).is_none());
    // Open in browser exists for the codespaces only.
    assert!(bounds(&mut visual, &format!("cloud-ws-browser-{USED}")).is_some());
    assert!(bounds(&mut visual, &format!("cloud-ws-browser-{SBX_USED}")).is_none());
    assert!(bounds(&mut visual, "cloud-workspaces-summary").is_some());
    assert!(bounds(&mut visual, "cloud-workspaces-estimate").is_some());
}

#[gpui::test]
fn the_rows_carry_the_state_the_age_the_sessions_and_the_estimated_cost(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let loaded = view(&window, cx);
    let row = |name: &str| loaded.rows.iter().find(|r| r.cloud.name == name).unwrap();

    let used = row(USED);
    assert_eq!(used.state, WorkspaceState::Running);
    assert_eq!(
        model::detail_line(used),
        "fix/login \u{b7} basicLinux32gb \u{b7} 2 cores"
    );
    assert_eq!(
        model::age_line(used, loaded.loaded_ms)
            .unwrap()
            .split(" \u{b7} ")
            .next(),
        Some("Created 3h 0m ago")
    );
    assert!(!model::uses_line(used).1);
    // 3 h x 2 cores x $0.09
    assert_eq!(
        model::cost_line(used),
        "Est. \u{2248} $0.54 so far (at most 3h 0m running) \u{b7} \u{2248} $0.18/h while it runs"
    );

    let sandbox = row(SBX_USED);
    assert_eq!(model::detail_line(sandbox), "Medium \u{b7} 4 vCPU / 8g");
    // 2 h x $0.28
    assert_eq!(
        model::cost_line(sandbox),
        "Est. \u{2248} $0.56 so far (at most 2h 0m running) \u{b7} \u{2248} $0.28/h while it runs"
    );

    let lost = row(LOST);
    assert_eq!(lost.state, WorkspaceState::Stopped);
    assert!(model::uses_line(lost).1, "an orphan");
    assert_eq!(
        model::cost_line(lost),
        "Est. Not running, so no compute cost \u{b7} \u{2248} $0.36/h when it runs \u{b7} storage may still be billed"
    );

    let hibernating = row(SBX_LOST);
    assert_eq!(hibernating.state, WorkspaceState::Hibernating);
    assert_eq!(model::state_tone(hibernating.state), Tone::Busy);
    let sum = model::summary(&loaded);
    assert_eq!((sum.total, sum.running, sum.orphans), (4, 3, 2));
    // used codespace 0.18 + used sandbox 0.28 + hibernating Small 0.14
    assert!((sum.burn - 0.60).abs() < 1e-9, "{}", sum.burn);
    assert_eq!(
        model::summary_line(&sum),
        "4 workspaces \u{b7} 3 running \u{b7} est. $0.60/h while they run \u{b7} 2 with no session"
    );
}

#[gpui::test]
fn opening_only_reads_and_reopening_within_the_cache_asks_nobody(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    draw(&window, cx);
    assert_eq!(w.sbx.verbs(), ["ls"]);
    assert_eq!(w.gh.count("codespace list"), 1);
    assert_eq!(
        w.gh.count("codespace stop") + w.gh.count("codespace delete"),
        0
    );
    // A second dialog opened soon after reads the cache.
    let second = open(&w, cx);
    assert_eq!(view(&second, cx).rows.len(), 4);
    assert_eq!((w.sbx.verbs().len(), w.gh.count("codespace list")), (1, 1));
    // Refresh asks again, and keeps the rows while it runs.
    act(&second, cx, |d, cx| d.refresh(cx));
    assert_eq!((w.sbx.verbs().len(), w.gh.count("codespace list")), (2, 2));
    assert!(!second.read_with(cx, |d, _| d.is_loading()).unwrap());
}

#[gpui::test]
fn without_sbx_the_codespaces_still_list_and_the_notice_says_why(cx: &mut TestAppContext) {
    let w = world();
    *w.sbx.ls_reply.lock().unwrap() =
        Some(Err("Docker Sandboxes CLI (sbx) is not installed".into()));
    let window = open(&w, cx);
    let loaded = view(&window, cx);
    assert_eq!(loaded.rows.len(), 2);
    let notes = model::notices(&loaded);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].id, "cloud-workspaces-sbx");
    assert!(notes[0].text.contains("sbx") && notes[0].text.contains("not installed"));
    assert!(!notes[0].problem, "a missing optional CLI is not an alert");
    let mut visual = draw(&window, cx);
    assert!(bounds(&mut visual, "cloud-workspaces-sbx").is_some());
    assert!(bounds(&mut visual, &format!("cloud-ws-row-{USED}")).is_some());
    assert!(bounds(&mut visual, &format!("cloud-ws-row-{SBX_USED}")).is_none());
}

#[gpui::test]
fn a_signed_out_sbx_and_gh_name_the_command_that_fixes_them(cx: &mut TestAppContext) {
    let w = world();
    *w.sbx.ls_reply.lock().unwrap() = Some(Ok(SbxOutput {
        success: false,
        stdout: String::new(),
        stderr: "not logged in: run `sbx login`".into(),
    }));
    *w.gh.auth.lock().unwrap() = Ok(GhOutput {
        success: false,
        stdout: String::new(),
        stderr: "You are not logged into any GitHub hosts.".into(),
    });
    let window = open(&w, cx);
    let loaded = view(&window, cx);
    assert!(loaded.rows.is_empty());
    assert_eq!(
        w.gh.count("codespace list"),
        0,
        "never list while signed out"
    );
    let notes = model::notices(&loaded);
    let fixes: Vec<_> = notes.iter().map(|n| n.fix.as_deref()).collect();
    assert_eq!(fixes, [Some("gh auth login"), Some("sbx login")]);
    assert!(notes.iter().all(|n| n.problem));
    let mut visual = draw(&window, cx);
    assert!(bounds(&mut visual, "cloud-workspaces-gh").is_some());
    assert!(bounds(&mut visual, "cloud-workspaces-sbx").is_some());
    assert!(bounds(&mut visual, "cloud-workspaces-summary").is_some());
}

#[gpui::test]
fn stop_goes_through_the_app_api_and_the_row_is_read_again(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let events = record(&window, cx);
    act(&window, cx, |d, cx| {
        d.run(
            CloudRef::codespace(USED),
            Action::Stop,
            LiveBusy::default(),
            cx,
        )
    });
    assert_eq!(w.gh.count("codespace stop -c octo-used"), 1);
    assert_eq!(*events.lock().unwrap(), ["stopped octo-used"]);
    // The list was read again (the stop forgot the cached one).
    assert_eq!(w.gh.count("codespace list"), 2);

    act(&window, cx, |d, cx| {
        d.run(
            CloudRef::sandbox(SBX_USED),
            Action::Stop,
            LiveBusy::default(),
            cx,
        )
    });
    assert!(w.sbx.verbs().contains(&"stop".to_owned()));
}

#[gpui::test]
fn stop_is_offered_only_for_a_workspace_that_is_up(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let loaded = view(&window, cx);
    for (name, stop) in [
        (USED, true),
        (SBX_USED, true),
        (LOST, false),
        (SBX_LOST, false),
    ] {
        let row = loaded.rows.iter().find(|r| r.cloud.name == name).unwrap();
        let offers = model::offers(row);
        assert_eq!(offers.stop, stop, "{name}");
        assert!(offers.delete, "{name}");
        assert_eq!(offers.browser, row.has_web, "{name}");
    }
}

#[gpui::test]
fn delete_asks_first_and_names_what_goes_with_it(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let events = record(&window, cx);
    let used = CloudRef::codespace(USED);
    act(&window, cx, |d, cx| d.ask_to_delete(&used, cx));
    assert!(
        window
            .read_with(cx, |d, _| d.asks_to_delete(&used))
            .unwrap()
    );
    let mut visual = draw(&window, cx);
    assert!(bounds(&mut visual, &format!("cloud-ws-confirm-{USED}")).is_some());
    assert!(bounds(&mut visual, &format!("cloud-ws-confirm-delete-{USED}")).is_some());
    assert_eq!(w.gh.count("codespace delete"), 0, "asking deletes nothing");

    let loaded = view(&window, cx);
    let prompt = model::delete_prompt(loaded.rows.iter().find(|r| r.cloud == used).unwrap());
    assert!(prompt[0].contains("cannot be undone") && prompt[0].contains("codespace"));
    assert!(
        prompt
            .iter()
            .any(|l| l.contains("1 Monoptah session on it will be removed"))
    );

    // Escape leaves the confirmation, not the dialog; nothing was deleted.
    act(&window, cx, |d, cx| {
        assert!(d.cancel_confirm(cx));
        assert!(!d.cancel_confirm(cx));
    });
    assert!(
        !window
            .read_with(cx, |d, _| d.asks_to_delete(&used))
            .unwrap()
    );
    assert_eq!(w.gh.count("codespace delete"), 0);
    assert!(events.lock().unwrap().is_empty());
}

#[gpui::test]
fn deleting_a_codespace_forgets_its_sessions_and_reports_their_ids(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let events = record(&window, cx);
    act(&window, cx, |d, cx| {
        d.run(
            CloudRef::codespace(USED),
            Action::Delete,
            LiveBusy::default(),
            cx,
        )
    });
    assert_eq!(w.gh.count("codespace delete -c octo-used --force"), 1);
    assert_eq!(
        *events.lock().unwrap(),
        [format!("deleted octo-used [{}]", w.used_session)]
    );
    // The session is gone from the store like after any delete, and the row from the list.
    assert!(
        w.store
            .lock()
            .unwrap()
            .remote_session(w.used_session)
            .unwrap()
            .is_none()
    );
    let loaded = view(&window, cx);
    assert!(
        loaded.rows.iter().all(|r| r.cloud.name != USED),
        "{:?}",
        loaded.rows
    );
}

#[gpui::test]
fn deleting_a_running_sandbox_stops_it_first(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let events = record(&window, cx);
    act(&window, cx, |d, cx| {
        d.run(
            CloudRef::sandbox(SBX_USED),
            Action::Delete,
            LiveBusy::default(),
            cx,
        )
    });
    let verbs = w.sbx.verbs();
    let stop = verbs.iter().position(|v| v == "stop").expect("stopped");
    let rm = verbs.iter().position(|v| v == "rm").expect("removed");
    assert!(stop < rm, "{verbs:?}");
    assert!(w.sbx.gone("sbx_001a"));
    assert_eq!(
        *events.lock().unwrap(),
        [format!("deleted {SBX_USED} [{}]", w.sandbox_session)]
    );
}

#[gpui::test]
fn a_hibernating_sandbox_can_be_deleted_once_it_has_stopped(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let events = record(&window, cx);
    // It is not offered Stop, but Delete waits for it to be stopped and then removes it.
    let hibernating = CloudRef::sandbox(SBX_LOST);
    let loaded = view(&window, cx);
    let row = loaded.rows.iter().find(|r| r.cloud == hibernating).unwrap();
    assert!(!model::offers(row).stop && model::offers(row).delete);
    act(&window, cx, |d, cx| {
        d.run(hibernating.clone(), Action::Delete, LiveBusy::default(), cx)
    });
    assert!(w.sbx.gone("sbx_001b"));
    assert!(
        !w.sbx.verbs().contains(&"stop".to_owned()),
        "already going down"
    );
    assert_eq!(*events.lock().unwrap(), [format!("deleted {SBX_LOST} []")]);
}

#[gpui::test]
fn a_working_session_blocks_the_delete_and_the_row_says_so(cx: &mut TestAppContext) {
    let w = world();
    let window = open(&w, cx);
    let events = record(&window, cx);
    let busy = LiveBusy {
        sessions: [w.used_session].into(),
        ..LiveBusy::default()
    };
    act(&window, cx, |d, cx| {
        d.run(CloudRef::codespace(USED), Action::Delete, busy, cx)
    });
    assert_eq!(w.gh.count("codespace delete"), 0);
    assert!(events.lock().unwrap().is_empty());
    let error = window
        .read_with(cx, |d, _| {
            d.error_of(&CloudRef::codespace(USED)).map(str::to_owned)
        })
        .unwrap()
        .expect("the refusal is shown on the row");
    assert!(error.contains("sessions are working on it"), "{error}");
    let mut visual = draw(&window, cx);
    assert!(bounds(&mut visual, &format!("cloud-ws-error-{USED}")).is_some());
    // The session and the workspace are untouched.
    assert!(
        w.store
            .lock()
            .unwrap()
            .remote_session(w.used_session)
            .unwrap()
            .is_some()
    );
}

#[gpui::test]
fn the_estimate_helpers_read_as_the_dialog_words_them(_cx: &mut TestAppContext) {
    assert_eq!(model::age_text(45), "45s");
    assert_eq!(model::age_text(12 * 60), "12m");
    assert_eq!(model::age_text(3 * 3600 + 12 * 60), "3h 12m");
    assert_eq!(model::age_text(2 * 86_400 + 4 * 3600), "2d 4h");
    assert_eq!(model::money(0.0), "<$0.01");
    assert_eq!(model::money(0.56), "$0.56");
    assert_eq!(model::money(1234.4), "$1234");
    assert!(model::ESTIMATE_NOTE.contains("estimates"));
    assert_eq!(
        model::summary_line(&model::Summary::default()),
        model::NO_WORKSPACES
    );
}
