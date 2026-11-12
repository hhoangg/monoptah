//! Settings > Cloud: the Backends checklist and the Docker accounts. Everything outside the
//! window is a fake: Docker Hub, `gh`, the home folder and the program lookup. No test
//! reaches the network, a real `gh` or `sbx`, or the user's files.
use super::*;
use crate::{
    runtime::Database,
    settings::cloud_env::{CloudServices, StoreHandle},
};
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};
use monocode_core::{
    cloud_sessions::docker_config::{self, DockerProjectConfig},
    codespaces::{GhOutput, GhRunner},
    docker_accounts::{DockerHub, ExchangeError, Secret},
    store::Store,
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const PAT: &str = concat!("dckr_pat_", "exampleTokenValue1234567890");
const PAT2: &str = concat!("dckr_pat_", "secondTokenValue0987654321");
/// A JWT whose `scope` claim is `repo:read sandbox:use`.
const JWT_SANDBOX: &str = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1Iiwic2NvcGUiOiJyZXBvOnJlYWQgc2FuZGJveDp1c2UiLCJleHAiOjQxMDI0NDQ4MDAsImlhdCI6MX0.c2ln";
/// A JWT whose `scope` claim is `repo:read` only.
const JWT_NO_SANDBOX: &str = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1Iiwic2NvcGUiOiJyZXBvOnJlYWQiLCJleHAiOjQxMDI0NDQ4MDAsImlhdCI6MX0.c2ln";
const AUTH_OK: &str = "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const AUTH_NO_SCOPE: &str = "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'repo'\n";
const AUTH_OUT: &str = "You are not logged into any GitHub hosts. To log in, run: gh auth login\n";

/// A folder under the system temp dir, removed on drop.
struct TempDir(PathBuf);
impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "monoptah-cloud-tests-{}-{label}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum Reply {
    Sandbox,
    NoSandbox,
    Rejected,
    Offline,
}
struct FakeHub {
    reply: Mutex<Reply>,
    calls: Mutex<Vec<String>>,
}
impl FakeHub {
    fn new(reply: Reply) -> Arc<Self> {
        Arc::new(Self {
            reply: Mutex::new(reply),
            calls: Mutex::new(Vec::new()),
        })
    }
    fn set(&self, reply: Reply) {
        *self.reply.lock().unwrap() = reply;
    }
}
impl DockerHub for FakeHub {
    fn exchange(&self, docker_id: &str, _pat: &str) -> Result<Secret, ExchangeError> {
        self.calls.lock().unwrap().push(docker_id.to_owned());
        match *self.reply.lock().unwrap() {
            Reply::Sandbox => Ok(Secret::new(JWT_SANDBOX.into())),
            Reply::NoSandbox => Ok(Secret::new(JWT_NO_SANDBOX.into())),
            Reply::Rejected => Err(ExchangeError::Rejected(401)),
            Reply::Offline => Err(ExchangeError::Network("connection refused".into())),
        }
    }
}

struct FakeGh {
    /// `Ok(text)` is what `gh auth status` prints; `Err` means gh cannot start.
    status: Mutex<Result<String, String>>,
    calls: Mutex<usize>,
}
impl FakeGh {
    fn new(status: Result<&str, &str>) -> Arc<Self> {
        Arc::new(Self {
            status: Mutex::new(status.map(str::to_owned).map_err(str::to_owned)),
            calls: Mutex::new(0),
        })
    }
    fn set(&self, status: Result<&str, &str>) {
        *self.status.lock().unwrap() = status.map(str::to_owned).map_err(str::to_owned);
    }
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}
impl GhRunner for FakeGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        assert_eq!(args, ["auth", "status", "--active"], "only the auth check");
        *self.calls.lock().unwrap() += 1;
        self.status.lock().unwrap().clone().map(|stdout| GhOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

static SBX_INSTALLED: AtomicBool = AtomicBool::new(true);
fn resolve(name: &str) -> Option<PathBuf> {
    (name != "sbx" || SBX_INSTALLED.load(Ordering::SeqCst))
        .then(|| PathBuf::from("/fake/bin").join(name))
}

struct World {
    root: TempDir,
    home: TempDir,
    store: Database,
    hub: Arc<FakeHub>,
    gh: Arc<FakeGh>,
}
impl World {
    fn new() -> Self {
        let root = TempDir::new("store");
        let home = TempDir::new("home");
        let store = Store::open(&root.path().join("monocode.db")).unwrap();
        SBX_INSTALLED.store(true, Ordering::SeqCst);
        Self {
            root,
            home,
            store: Arc::new(Mutex::new(store)),
            hub: FakeHub::new(Reply::Sandbox),
            gh: FakeGh::new(Ok(AUTH_OK)),
        }
    }
    fn services(&self) -> CloudServices {
        CloudServices {
            hub: self.hub.clone(),
            gh: self.gh.clone(),
            home: Some(self.home.path().to_owned()),
            resolve,
        }
    }
    fn set_up_ssh(&self) {
        let ssh = self.home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        std::fs::write(ssh.join("config"), "Host sbx_cloud\n  User x\n").unwrap();
    }
    /// A project whose sandbox uses the Docker account `id`.
    fn use_account(&self, cwd: &str, id: &str) {
        let config = DockerProjectConfig {
            repo_url: "https://github.com/o/r.git".into(),
            docker_account: Some(id.into()),
            ..Default::default()
        };
        docker_config::save(&self.store.lock().unwrap(), cwd, &config).unwrap();
    }
    fn files_contain(&self, needle: &str) -> bool {
        fn walk(dir: &Path, needle: &[u8]) -> bool {
            std::fs::read_dir(dir).unwrap().flatten().any(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, needle)
                } else {
                    std::fs::read(&path)
                        .map(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
                        .unwrap_or(false)
                }
            })
        }
        walk(self.root.path(), needle.as_bytes()) || walk(self.home.path(), needle.as_bytes())
    }
}

/// Collects every `log` record so a test can prove a secret never reached the app log.
struct CaptureLog(Mutex<Vec<String>>);
impl log::Log for CaptureLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        self.0.lock().unwrap().push(format!("{}", record.args()));
    }
    fn flush(&self) {}
}
static LOG: CaptureLog = CaptureLog(Mutex::new(Vec::new()));
fn capture_logs() {
    let _ = log::set_logger(&LOG);
    log::set_max_level(log::LevelFilter::Trace);
}

fn open(world: &World, cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    let services = world.services();
    let store = StoreHandle(world.store.clone());
    cx.update(|cx| {
        cx.set_global(services);
        cx.set_global(crate::cloud_kit_ui::KitScanner(
            crate::cloud_kit_ui::fixture::scan,
        ));
    });
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(1400.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = SettingsView::new(
                        Section::General,
                        Preferences::default(),
                        SettingsContext {
                            store: Some(store),
                            ..Default::default()
                        },
                        cx,
                    );
                    view.discovery_task = None;
                    view.discovery_loading = false;
                    view
                })
            },
        )
        .unwrap()
    });
    window
        .update(cx, |view, _, cx| {
            view.selected_project = "/work/app".into();
            view.select_section(Section::Cloud, cx);
        })
        .unwrap();
    settle(cx);
    window
}

fn settle(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.run_until_parked();
}

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
}

fn type_into(
    window: &WindowHandle<SettingsView>,
    visual: &mut VisualTestContext,
    pat: bool,
    text: &str,
) {
    window
        .update(visual, |view, _, cx| {
            let inputs = view.cloud.docker.inputs.as_ref().expect("the form is open");
            let input = if pat { &inputs.pat } else { &inputs.docker_id };
            input.update(cx, |input, cx| input.set_text(text.into(), cx));
        })
        .unwrap();
    visual.run_until_parked();
}

fn pat_field(window: &WindowHandle<SettingsView>, visual: &mut VisualTestContext) -> String {
    window
        .update(visual, |view, _, cx| {
            let inputs = view.cloud.docker.inputs.as_ref().unwrap();
            inputs.pat.read(cx).content.to_string()
        })
        .unwrap()
}

fn has(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    visual.debug_bounds(selector).is_some()
}

/// The text of the form's outcome (the success line or the error).
fn form_outcome(
    window: &WindowHandle<SettingsView>,
    visual: &mut VisualTestContext,
) -> Option<Result<String, String>> {
    window
        .update(visual, |view, _, _| {
            view.cloud
                .docker
                .form
                .as_ref()
                .and_then(|form| form.outcome.clone())
        })
        .unwrap()
}

fn account_ids(window: &WindowHandle<SettingsView>, visual: &mut VisualTestContext) -> Vec<String> {
    window
        .update(visual, |view, _, _| match &view.cloud.docker.list {
            pages::cloud_docker::AccountList::Ready(rows) => {
                rows.iter().map(|r| r.summary.docker_id.clone()).collect()
            }
            _ => Vec::new(),
        })
        .unwrap()
}

fn add_account(
    window: &WindowHandle<SettingsView>,
    visual: &mut VisualTestContext,
    docker_id: &str,
    pat: &str,
) {
    if !has(visual, "docker-form") {
        click(visual, "docker-add-button");
    }
    type_into(window, visual, false, docker_id);
    type_into(window, visual, true, pat);
    click(visual, "docker-form-save");
}

#[gpui::test]
fn the_page_shows_backends_and_docker_accounts_before_the_kit(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    for id in [
        "cloud-check-codespaces",
        "cloud-check-sandbox",
        "docker-add-button",
        "settings-toggle-cloud-kit-enabled",
    ] {
        assert!(has(visual, id), "{id} is not drawn");
    }
    let backends = visual.debug_bounds("cloud-check-codespaces").unwrap();
    let docker = visual.debug_bounds("docker-add-button").unwrap();
    let kit = visual
        .debug_bounds("settings-toggle-cloud-kit-enabled")
        .unwrap();
    assert!(backends.top() < docker.top() && docker.top() < kit.top());
    window
        .update(visual, |view, _, _| {
            assert_eq!(
                view.section.description(),
                "Applies to every project. Repository, machine and credentials of each project live in its Cloud settings dialog."
            );
        })
        .unwrap();
}

#[gpui::test]
fn backend_chips_reflect_the_checks(cx: &mut TestAppContext) {
    let world = World::new();
    world.set_up_ssh();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    for id in [
        "cloud-chip-gh",
        "cloud-chip-gh-scope",
        "cloud-chip-sbx",
        "cloud-chip-sbx-ssh",
    ] {
        assert!(has(visual, id), "{id} is not drawn");
    }
    assert!(!has(visual, "cloud-fix-codespaces"));
    assert!(!has(visual, "cloud-fix-sandbox"));
    let ready = window
        .update(visual, |view, _, _| {
            (
                view.cloud.backends.codespaces.clone(),
                view.cloud.backends.sandbox,
            )
        })
        .unwrap();
    assert_eq!(
        ready.0,
        Some(crate::cloud_checks::Gh::Ready(Some("octo".into())))
    );
    assert_eq!(
        ready.1,
        Some(crate::cloud_checks::Sbx {
            installed: true,
            ssh_set_up: true
        })
    );

    // No codespace scope and no ssh alias: both can be fixed.
    world.gh.set(Ok(AUTH_NO_SCOPE));
    std::fs::remove_dir_all(world.home.path().join(".ssh")).unwrap();
    click(visual, "cloud-check-codespaces");
    let state = window
        .update(visual, |view, _, _| {
            (
                view.cloud.backends.codespaces.clone(),
                view.cloud.backends.sandbox,
            )
        })
        .unwrap();
    assert_eq!(
        state.0,
        Some(crate::cloud_checks::Gh::MissingScope(Some("octo".into())))
    );
    assert_eq!(
        state.1,
        Some(crate::cloud_checks::Sbx {
            installed: true,
            ssh_set_up: false
        })
    );
    assert!(has(visual, "cloud-fix-codespaces"));
    assert!(has(visual, "cloud-fix-sandbox"));

    // Not signed in, then gh missing; sbx missing has nothing the app can run.
    world.gh.set(Ok(AUTH_OUT));
    click(visual, "cloud-check-codespaces");
    assert_eq!(
        window
            .update(visual, |view, _, _| view.cloud.backends.codespaces.clone())
            .unwrap(),
        Some(crate::cloud_checks::Gh::NotLoggedIn)
    );
    world.gh.set(Err("gh is not installed"));
    SBX_INSTALLED.store(false, Ordering::SeqCst);
    click(visual, "cloud-check-codespaces");
    assert_eq!(
        window
            .update(visual, |view, _, _| view.cloud.backends.codespaces.clone())
            .unwrap(),
        Some(crate::cloud_checks::Gh::Missing)
    );
    assert!(has(visual, "cloud-fix-codespaces"));
    assert!(!has(visual, "cloud-fix-sandbox"));
    SBX_INSTALLED.store(true, Ordering::SeqCst);
}

#[gpui::test]
fn check_again_runs_the_checks_again(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    let before = world.gh.calls();
    assert!(before >= 1);
    click(visual, "cloud-check-sandbox");
    assert_eq!(world.gh.calls(), before + 1);
}

#[gpui::test]
fn fix_opens_the_terminal_with_the_right_command_and_checks_again_on_exit(cx: &mut TestAppContext) {
    let world = World::new();
    world.gh.set(Ok(AUTH_NO_SCOPE));
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    click(visual, "cloud-fix-codespaces");
    assert!(has(visual, "fix-terminal"));
    let terminal = window
        .update(visual, |view, _, cx| {
            view.cloud
                .backends
                .fix_view()
                .expect("the fix dialog is open")
                .read(cx)
                .terminal()
                .clone()
        })
        .unwrap();
    visual.update(|_, cx| {
        terminal.update(cx, |terminal, _| {
            let spec = terminal.launched().expect("the launch is recorded");
            assert_eq!(spec.program, OsString::from("/fake/bin/gh"));
            assert_eq!(
                spec.args,
                ["auth", "refresh", "-h", "github.com", "-s", "codespace"].map(OsString::from)
            );
            assert!(spec.env_set.is_empty());
        })
    });
    // The fix works: the exit checks again and the scope chip turns ok.
    world.gh.set(Ok(AUTH_OK));
    let calls = world.gh.calls();
    visual.update(|_, cx| {
        terminal.update(cx, |terminal, cx| {
            terminal.fake_started(cx);
            terminal.fake_exit(Some(0), cx);
        })
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(world.gh.calls(), calls + 1);
    assert_eq!(
        window
            .update(visual, |view, _, _| view.cloud.backends.codespaces.clone())
            .unwrap(),
        Some(crate::cloud_checks::Gh::Ready(Some("octo".into())))
    );
    assert!(!has(visual, "cloud-fix-codespaces"));
    // Close stops the program and removes the dialog.
    click(visual, "fix-terminal-close");
    assert!(!has(visual, "fix-terminal"));

    // Docker Sandbox: `sbx --cloud setup ssh`.
    click(visual, "cloud-fix-sandbox");
    let terminal = window
        .update(visual, |view, _, cx| {
            view.cloud
                .backends
                .fix_view()
                .unwrap()
                .read(cx)
                .terminal()
                .clone()
        })
        .unwrap();
    visual.update(|_, cx| {
        terminal.update(cx, |terminal, _| {
            let spec = terminal.launched().unwrap();
            assert_eq!(spec.program, OsString::from("/fake/bin/sbx"));
            assert_eq!(spec.args, ["--cloud", "setup", "ssh"].map(OsString::from));
        })
    });
    // The ssh alias appears; the exit checks again.
    world.set_up_ssh();
    visual.update(|_, cx| terminal.update(cx, |t, cx| t.fake_exit(Some(0), cx)));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(
        window
            .update(visual, |view, _, _| view.cloud.backends.sandbox)
            .unwrap()
            .map(|sbx| sbx.ssh_set_up),
        Some(true)
    );
}

#[gpui::test]
fn adding_an_account_checks_the_token_and_lists_it(cx: &mut TestAppContext) {
    capture_logs();
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    assert!(account_ids(&window, visual).is_empty());
    click(visual, "docker-add-button");
    assert!(has(visual, "docker-form"));
    assert!(has(visual, "docker-form-hint"));
    // The token field is masked.
    window
        .update(visual, |view, _, cx| {
            let inputs = view.cloud.docker.inputs.as_ref().unwrap();
            assert!(inputs.pat.read(cx).is_secret());
            assert!(!inputs.docker_id.read(cx).is_secret());
        })
        .unwrap();
    add_account(&window, visual, "octo-dev", PAT);

    assert_eq!(
        form_outcome(&window, visual),
        Some(Ok(
            "Checked: token works and has Cloud Sandboxes access".into()
        ))
    );
    assert!(has(visual, "docker-form-result"));
    assert_eq!(account_ids(&window, visual), ["octo-dev"]);
    assert!(has(visual, "docker-account-octo-dev"));
    assert!(has(visual, "docker-status-octo-dev"));
    // The PAT field is emptied after a success, the Docker ID too.
    assert_eq!(pat_field(&window, visual), "");
    window
        .update(visual, |view, _, cx| {
            let row = match &view.cloud.docker.list {
                pages::cloud_docker::AccountList::Ready(rows) => &rows[0],
                _ => panic!("not loaded"),
            };
            assert_eq!(
                row.summary.status.label(),
                "Cloud Sandboxes access",
                "the list shows the status"
            );
            assert_eq!(row.summary.expires_at, None, "expiry reads Not known");
            assert!(row.used_by.is_empty());
            let inputs = view.cloud.docker.inputs.as_ref().unwrap();
            assert_eq!(inputs.docker_id.read(cx).content.to_string(), "");
        })
        .unwrap();
    // Close the form; the token is nowhere: not in the page state, the database files, the
    // home folder or the app log.
    click(visual, "docker-form-cancel");
    assert!(!has(visual, "docker-form"));
    assert!(!world.files_contain(PAT));
    assert!(
        LOG.0.lock().unwrap().iter().all(|line| !line.contains(PAT)),
        "the token reached the app log"
    );
}

#[gpui::test]
fn add_errors_are_precise_and_keep_the_token_out_of_sight(cx: &mut TestAppContext) {
    capture_logs();
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();

    world.hub.set(Reply::Rejected);
    add_account(&window, visual, "octo-dev", PAT);
    let Some(Err(rejected)) = form_outcome(&window, visual) else {
        panic!("expected an error");
    };
    assert!(rejected.contains("Wrong Docker ID or token"), "{rejected}");
    assert!(has(visual, "docker-form-error"));
    // The failed save keeps what was typed, so the user can fix it.
    assert_eq!(pat_field(&window, visual), PAT);

    world.hub.set(Reply::NoSandbox);
    click(visual, "docker-form-save");
    let Some(Err(no_scope)) = form_outcome(&window, visual) else {
        panic!("expected an error");
    };
    assert!(no_scope.contains("no Cloud Sandboxes access"), "{no_scope}");

    world.hub.set(Reply::Offline);
    click(visual, "docker-form-save");
    let Some(Err(offline)) = form_outcome(&window, visual) else {
        panic!("expected an error");
    };
    assert!(offline.contains("Could not reach Docker Hub"), "{offline}");

    // A Docker ID that cannot exist is refused before any exchange.
    let calls = world.hub.calls.lock().unwrap().len();
    type_into(&window, visual, false, "x");
    click(visual, "docker-form-save");
    let Some(Err(bad_id)) = form_outcome(&window, visual) else {
        panic!("expected an error");
    };
    assert!(bad_id.contains("Docker ID"), "{bad_id}");
    assert_eq!(world.hub.calls.lock().unwrap().len(), calls);

    for text in [&rejected, &no_scope, &offline, &bad_id] {
        assert!(!text.contains(PAT));
    }
    assert!(account_ids(&window, visual).is_empty());
    assert!(!world.files_contain(PAT));
    assert!(LOG.0.lock().unwrap().iter().all(|line| !line.contains(PAT)));
    // Cancel drops the typed token.
    click(visual, "docker-form-cancel");
    assert_eq!(pat_field(&window, visual), "");
}

#[gpui::test]
fn save_is_disabled_while_the_token_is_checked(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    click(visual, "docker-add-button");
    type_into(&window, visual, false, "octo-dev");
    type_into(&window, visual, true, PAT);
    // Start the save and look before the background work is polled.
    window
        .update(visual, |view, _, cx| view.docker_save(cx))
        .unwrap();
    let checking = window
        .update(visual, |view, _, _| {
            view.cloud.docker.form.as_ref().map(|f| f.checking)
        })
        .unwrap();
    assert_eq!(checking, Some(true));
    visual.refresh().unwrap();
    assert!(has(visual, "docker-form-save"));
    // A second press while checking does nothing.
    window
        .update(visual, |view, _, cx| view.docker_save(cx))
        .unwrap();
    visual.run_until_parked();
    assert_eq!(world.hub.calls.lock().unwrap().len(), 1);
}

#[gpui::test]
fn remove_is_refused_while_a_project_uses_the_account(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    let id = window
        .update(visual, |view, _, _| match &view.cloud.docker.list {
            pages::cloud_docker::AccountList::Ready(rows) => rows[0].summary.id.clone(),
            _ => panic!("not loaded"),
        })
        .unwrap();
    world.use_account("/work/app", &id);
    world.use_account("/work/second", &id);
    // The page reads the projects again when the section opens.
    window
        .update(visual, |view, _, cx| view.refresh_cloud(cx))
        .unwrap();
    settle(visual);
    click(visual, "docker-remove-octo-dev");
    assert!(has(visual, "docker-remove-panel"));
    assert!(has(visual, "docker-remove-message"));
    assert!(
        !has(visual, "docker-remove-confirm"),
        "an account in use cannot be confirmed away"
    );
    let users = window
        .update(visual, |view, _, _| match &view.cloud.docker.removal {
            Some(pages::cloud_docker::Removal {
                stage: pages::cloud_docker::RemoveStage::Refused(users),
                ..
            }) => users.clone(),
            _ => panic!("expected a refusal"),
        })
        .unwrap();
    assert_eq!(users, ["app", "second"]);
    click(visual, "docker-remove-cancel");
    assert_eq!(account_ids(&window, visual), ["octo-dev"]);

    // A project that picks the account between the click and the confirmation still blocks.
    world.use_account("/work/app", "someone-else");
    world.use_account("/work/second", "someone-else");
    window
        .update(visual, |view, _, cx| view.refresh_cloud(cx))
        .unwrap();
    settle(visual);
    click(visual, "docker-remove-octo-dev");
    assert!(has(visual, "docker-remove-confirm"));
    world.use_account("/work/third", &id);
    click(visual, "docker-remove-confirm");
    assert_eq!(account_ids(&window, visual), ["octo-dev"]);
    let refused = window
        .update(visual, |view, _, _| {
            matches!(
                view.cloud.docker.removal,
                Some(pages::cloud_docker::Removal {
                    stage: pages::cloud_docker::RemoveStage::Refused(_),
                    ..
                })
            )
        })
        .unwrap();
    assert!(refused);
}

#[gpui::test]
fn remove_succeeds_when_no_project_uses_the_account(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    click(visual, "docker-form-cancel");
    click(visual, "docker-remove-octo-dev");
    assert!(has(visual, "docker-remove-panel"));
    // Cancel keeps it.
    click(visual, "docker-remove-cancel");
    assert!(!has(visual, "docker-remove-panel"));
    assert_eq!(account_ids(&window, visual), ["octo-dev"]);
    click(visual, "docker-remove-octo-dev");
    click(visual, "docker-remove-confirm");
    assert!(account_ids(&window, visual).is_empty());
    assert!(!has(visual, "docker-remove-panel"));
    assert!(!has(visual, "docker-account-octo-dev"));
}

#[gpui::test]
fn replace_token_updates_the_status(cx: &mut TestAppContext) {
    capture_logs();
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    click(visual, "docker-form-cancel");
    // The token is revoked: checking again shows it.
    world.hub.set(Reply::Rejected);
    click(visual, "docker-check-octo-dev");
    let status = window
        .update(visual, |view, _, _| match &view.cloud.docker.list {
            pages::cloud_docker::AccountList::Ready(rows) => rows[0].summary.status,
            _ => panic!("not loaded"),
        })
        .unwrap();
    assert_eq!(status.label(), "Token revoked or expired");
    assert!(has(visual, "docker-notice"));

    // Replace it with a working token. The Docker ID stays fixed.
    world.hub.set(Reply::Sandbox);
    click(visual, "docker-replace-octo-dev");
    assert!(has(visual, "docker-form-fixed-id"));
    assert!(!has(visual, "docker-form-id"));
    type_into(&window, visual, true, PAT2);
    click(visual, "docker-form-save");
    assert_eq!(
        form_outcome(&window, visual),
        Some(Ok(
            "Checked: token works and has Cloud Sandboxes access".into()
        ))
    );
    let status = window
        .update(visual, |view, _, _| match &view.cloud.docker.list {
            pages::cloud_docker::AccountList::Ready(rows) => rows[0].summary.status,
            _ => panic!("not loaded"),
        })
        .unwrap();
    assert_eq!(status.label(), "Cloud Sandboxes access");
    assert_eq!(pat_field(&window, visual), "");
    assert_eq!(account_ids(&window, visual), ["octo-dev"]);
    assert!(!world.files_contain(PAT2));
    assert!(!world.files_contain(PAT));
    assert!(
        LOG.0
            .lock()
            .unwrap()
            .iter()
            .all(|line| !line.contains(PAT2))
    );
}

#[gpui::test]
fn no_store_means_no_accounts_and_no_crash(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.store = None;
            view.refresh_cloud(cx);
        })
        .unwrap();
    settle(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    assert!(has(visual, "docker-add-button"));
    assert!(!has(visual, "docker-form"));
}
