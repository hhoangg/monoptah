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
    sync::{Arc, Mutex, atomic::Ordering},
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

struct World {
    root: TempDir,
    home: TempDir,
    store: Database,
    hub: Arc<FakeHub>,
    gh: Arc<FakeGh>,
    /// Programs the fake lookup cannot find. Per world, so tests can run in parallel.
    missing: Arc<Mutex<Vec<String>>>,
}
impl World {
    fn new() -> Self {
        let root = TempDir::new("store");
        let home = TempDir::new("home");
        let store = Store::open(&root.path().join("monocode.db")).unwrap();
        Self {
            root,
            home,
            store: Arc::new(Mutex::new(store)),
            hub: FakeHub::new(Reply::Sandbox),
            gh: FakeGh::new(Ok(AUTH_OK)),
            missing: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn services(&self) -> CloudServices {
        let missing = self.missing.clone();
        CloudServices {
            hub: self.hub.clone(),
            gh: self.gh.clone(),
            home: Some(self.home.path().to_owned()),
            resolve: Arc::new(move |name| {
                (!missing.lock().unwrap().iter().any(|m| m == name))
                    .then(|| PathBuf::from("/fake/bin").join(name))
            }),
        }
    }
    /// Makes the fake program lookup fail for `name`.
    fn hide(&self, name: &str) {
        self.missing.lock().unwrap().push(name.to_owned());
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
/// Installs the capture and proves it works: a marker logged now must be captured, so an
/// empty log can never pass for "the token was not logged".
fn capture_logs() {
    let _ = log::set_logger(&LOG);
    log::set_max_level(log::LevelFilter::Trace);
    log::info!("capture-control-marker");
    assert!(
        LOG.0
            .lock()
            .unwrap()
            .iter()
            .any(|line| line == "capture-control-marker"),
        "the log capture is not installed, so a clean log proves nothing"
    );
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

/// The error under the form, if the last Save failed.
fn form_error(
    window: &WindowHandle<SettingsView>,
    visual: &mut VisualTestContext,
) -> Option<String> {
    window
        .update(visual, |view, _, _| {
            view.cloud
                .docker
                .form
                .as_ref()
                .and_then(|form| form.error.clone())
        })
        .unwrap()
}

/// The line under the account list: `(ok, text)`.
fn notice(
    window: &WindowHandle<SettingsView>,
    visual: &mut VisualTestContext,
) -> Option<(bool, String)> {
    window
        .update(visual, |view, _, _| {
            view.cloud
                .docker
                .notice
                .as_ref()
                .map(|notice| (notice.ok, notice.text.clone()))
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
    world.hide("sbx");
    click(visual, "cloud-check-codespaces");
    assert_eq!(
        window
            .update(visual, |view, _, _| view.cloud.backends.codespaces.clone())
            .unwrap(),
        Some(crate::cloud_checks::Gh::Missing)
    );
    // Only macOS has an install command to offer (Homebrew).
    assert_eq!(
        has(visual, "cloud-fix-codespaces"),
        cfg!(target_os = "macos")
    );
    assert!(!has(visual, "cloud-fix-sandbox"));
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

    // A success closes the form and says so under the list.
    assert!(!has(visual, "docker-form"));
    assert_eq!(
        notice(&window, visual),
        Some((
            true,
            "Checked: token works and has Cloud Sandboxes access".into()
        ))
    );
    assert!(has(visual, "docker-notice"));
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
    let Some(rejected) = form_error(&window, visual) else {
        panic!("expected an error");
    };
    assert!(rejected.contains("Wrong Docker ID or token"), "{rejected}");
    assert!(has(visual, "docker-form-error"));
    // The failed save keeps what was typed, so the user can fix it.
    assert_eq!(pat_field(&window, visual), PAT);

    world.hub.set(Reply::NoSandbox);
    click(visual, "docker-form-save");
    let Some(no_scope) = form_error(&window, visual) else {
        panic!("expected an error");
    };
    assert!(no_scope.contains("no Cloud Sandboxes access"), "{no_scope}");

    world.hub.set(Reply::Offline);
    click(visual, "docker-form-save");
    let Some(offline) = form_error(&window, visual) else {
        panic!("expected an error");
    };
    assert!(offline.contains("Could not reach Docker Hub"), "{offline}");

    // A Docker ID that cannot exist is refused before any exchange.
    let calls = world.hub.calls.lock().unwrap().len();
    type_into(&window, visual, false, "x");
    click(visual, "docker-form-save");
    let Some(bad_id) = form_error(&window, visual) else {
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
    assert!(!has(visual, "docker-form"));
    assert_eq!(
        notice(&window, visual),
        Some((
            true,
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

/// Every string the Cloud page puts on screen. GPUI's test windows have no accessibility
/// adapter, so the text cannot be read back from a frame; this collects what the render
/// functions are fed instead: the account rows, the notice, the form error, the removal
/// message, the fix lines and what each input draws (a masked input draws stars).
fn page_text(window: &WindowHandle<SettingsView>, visual: &mut VisualTestContext) -> String {
    visual.refresh().unwrap();
    window
        .update(visual, |view, _, cx| {
            let mut out = Vec::new();
            if let pages::cloud_docker::AccountList::Ready(rows) = &view.cloud.docker.list {
                for row in rows {
                    out.push(row.summary.docker_id.clone());
                    out.push(row.summary.status.label().to_owned());
                    out.extend(row.used_by.clone());
                }
            }
            if let Some(notice) = &view.cloud.docker.notice {
                out.push(notice.text.clone());
            }
            if let Some(form) = &view.cloud.docker.form {
                out.extend(form.error.clone());
            }
            if let Some(removal) = &view.cloud.docker.removal {
                out.push(removal.docker_id.clone());
                if let pages::cloud_docker::RemoveStage::Failed(error) = &removal.stage {
                    out.push(error.clone());
                }
            }
            out.extend(view.fix_hint());
            out.extend(view.cloud.backends.fix_error.clone());
            if view.cloud.docker.form.is_some()
                && let Some(inputs) = &view.cloud.docker.inputs
            {
                for input in [&inputs.docker_id, &inputs.pat] {
                    let input = input.read(cx);
                    // What the input draws: stars for a masked one.
                    out.push(if input.is_secret() {
                        "*".repeat(input.content.chars().count())
                    } else {
                        input.content.to_string()
                    });
                }
            }
            out.join("\n")
        })
        .unwrap()
}

#[gpui::test]
fn the_token_never_appears_in_the_rendered_page(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    // Positive control: the scan sees the page's own text.
    click(visual, "docker-add-button");
    type_into(&window, visual, false, "octo-dev");
    type_into(&window, visual, true, PAT);
    let typed = page_text(&window, visual);
    assert!(typed.contains("octo-dev"), "the scan must see typed text");
    assert!(
        typed.contains("****"),
        "the masked token draws stars: {typed}"
    );
    assert!(!typed.contains(PAT), "the typed token is on the page");

    world.hub.set(Reply::Rejected);
    click(visual, "docker-form-save");
    let failed = page_text(&window, visual);
    assert!(failed.contains("Wrong Docker ID or token"), "{failed}");
    assert!(!failed.contains(PAT), "the token is in the error state");

    world.hub.set(Reply::Sandbox);
    click(visual, "docker-form-save");
    let saved = page_text(&window, visual);
    assert!(saved.contains("Cloud Sandboxes access"), "{saved}");
    assert!(!saved.contains(PAT), "the token is on the page after Save");
}

/// The summary of the only listed account.
fn first_summary(
    window: &WindowHandle<SettingsView>,
    visual: &mut VisualTestContext,
) -> monocode_core::docker_accounts::DockerAccountSummary {
    window
        .update(visual, |view, _, _| match &view.cloud.docker.list {
            pages::cloud_docker::AccountList::Ready(rows) => rows[0].summary.clone(),
            _ => panic!("not loaded"),
        })
        .unwrap()
}

fn busy(window: &WindowHandle<SettingsView>, visual: &mut VisualTestContext) -> bool {
    window
        .update(visual, |view, _, _| view.cloud.docker.busy)
        .unwrap()
}

#[gpui::test]
fn a_running_check_blocks_every_other_action_and_ends_clean(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    let id = first_summary(&window, visual).id;
    let calls = world.hub.calls.lock().unwrap().len();

    // A check starts and has not been polled yet.
    window
        .update(visual, |view, _, cx| view.docker_check(id.clone(), cx))
        .unwrap();
    assert!(busy(&window, visual));
    // Add and Save are blocked: no form opens, nothing is sent to Docker.
    window
        .update(visual, |view, window, cx| {
            view.open_docker_form(pages::cloud_docker::FormKind::Add, window, cx);
            assert!(view.cloud.docker.form.is_none(), "a form opened while busy");
            view.docker_save(cx);
            view.docker_remove_start(id.clone(), "octo-dev".into(), cx);
            assert!(view.cloud.docker.removal.is_none(), "removal while busy");
            // A second check does not start either.
            view.docker_check(id.clone(), cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(
        world.hub.calls.lock().unwrap().len(),
        calls + 1,
        "exactly one check reached Docker"
    );
    assert!(
        !busy(&window, visual),
        "busy is released when the check ends"
    );
    assert!(notice(&window, visual).is_some_and(|(ok, _)| ok));
}

#[gpui::test]
fn a_running_save_is_not_replaced_by_a_check(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    let id = first_summary(&window, visual).id;
    click(visual, "docker-replace-octo-dev");
    type_into(&window, visual, true, PAT2);
    let calls = world.hub.calls.lock().unwrap().len();

    window
        .update(visual, |view, _, cx| {
            view.docker_save(cx);
            assert!(view.cloud.docker.busy);
            // Before the fix this overwrote (cancelled) the save and left busy stuck.
            view.docker_check(id.clone(), cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(
        world.hub.calls.lock().unwrap().len(),
        calls + 1,
        "only the save reached Docker"
    );
    assert!(!busy(&window, visual));
    assert_eq!(
        notice(&window, visual).map(|(_, text)| text),
        Some("Checked: token works and has Cloud Sandboxes access".into()),
        "the save finished and reported"
    );
    assert!(!has(visual, "docker-form"));
    // The buttons are usable again.
    click(visual, "docker-check-octo-dev");
    assert_eq!(world.hub.calls.lock().unwrap().len(), calls + 2);
}

#[gpui::test]
fn a_form_opened_during_a_save_does_not_take_its_outcome(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    let id = first_summary(&window, visual).id;
    click(visual, "docker-add-button");
    type_into(&window, visual, false, "second");
    type_into(&window, visual, true, PAT2);
    window
        .update(visual, |view, window, cx| {
            view.docker_save(cx);
            // Opening the Replace form for the other row is refused while the save runs.
            view.open_docker_form(
                pages::cloud_docker::FormKind::Replace {
                    id: id.clone(),
                    docker_id: "octo-dev".into(),
                },
                window,
                cx,
            );
            assert_eq!(
                view.cloud
                    .docker
                    .form
                    .as_ref()
                    .map(|form| form.kind.clone()),
                Some(pages::cloud_docker::FormKind::Add)
            );
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(account_ids(&window, visual), ["octo-dev", "second"]);
}

#[gpui::test]
fn a_stale_save_never_clears_the_token_typed_in_a_newer_form(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    let summary = first_summary(&window, visual);

    // A newer form with a token being typed.
    click(visual, "docker-add-button");
    type_into(&window, visual, false, "second");
    type_into(&window, visual, true, PAT2);
    // The answer of a save from before this form was opened arrives.
    window
        .update(visual, |view, _, cx| {
            let old = view.cloud.docker.epoch - 1;
            view.docker_saved(old, Ok(summary.clone()), cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(
        pat_field(&window, visual),
        PAT2,
        "the typed token was wiped"
    );
    assert!(has(visual, "docker-form"), "the newer form was closed");
    assert!(notice(&window, visual).is_none());

    // The same for a stale error: it is not shown under the newer form.
    window
        .update(visual, |view, _, cx| {
            let old = view.cloud.docker.epoch - 1;
            view.docker_saved(old, Err("old failure".into()), cx);
        })
        .unwrap();
    assert_eq!(form_error(&window, visual), None);
}

#[gpui::test]
fn leaving_the_page_during_a_save_drops_its_result_but_refreshes_the_list(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    click(visual, "docker-add-button");
    type_into(&window, visual, false, "octo-dev");
    type_into(&window, visual, true, PAT);
    window
        .update(visual, |view, _, cx| {
            view.docker_save(cx);
            view.leave_cloud(cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert!(!busy(&window, visual));
    assert!(notice(&window, visual).is_none(), "a stale result reported");
    assert!(
        window
            .update(visual, |view, _, _| view.cloud.docker.form.is_none())
            .unwrap()
    );
    // The account was saved all the same, so the list shows it.
    assert_eq!(account_ids(&window, visual), ["octo-dev"]);
    assert_eq!(pat_field(&window, visual), "");
}

#[gpui::test]
fn a_removal_result_for_another_row_is_ignored(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    add_account(&window, visual, "second", PAT2);
    click(visual, "docker-remove-octo-dev");
    let other = window
        .update(visual, |view, _, _| match &view.cloud.docker.list {
            pages::cloud_docker::AccountList::Ready(rows) => rows
                .iter()
                .find(|row| row.summary.docker_id == "second")
                .unwrap()
                .summary
                .id
                .clone(),
            _ => panic!("not loaded"),
        })
        .unwrap();
    window
        .update(visual, |view, _, cx| {
            let epoch = view.cloud.docker.epoch;
            view.cloud.docker.busy = true;
            view.docker_removed(epoch, other.clone(), Ok(Ok(())), cx);
            let removal = view.cloud.docker.removal.as_ref().expect("still open");
            assert_eq!(removal.docker_id, "octo-dev");
            assert!(matches!(
                removal.stage,
                pages::cloud_docker::RemoveStage::Confirm
            ));
            view.cloud.docker.busy = true;
            view.docker_removed(epoch, other, Err("boom".into()), cx);
            assert!(matches!(
                view.cloud.docker.removal.as_ref().unwrap().stage,
                pages::cloud_docker::RemoveStage::Confirm
            ));
            assert!(!view.cloud.docker.busy);
        })
        .unwrap();
}

#[gpui::test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "the gh install fix (brew install gh) exists on macOS only"
)]
fn fix_says_why_when_the_program_cannot_be_found(cx: &mut TestAppContext) {
    let world = World::new();
    world.gh.set(Err("gh is not installed"));
    world.hide("brew");
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    click(visual, "cloud-fix-codespaces");
    assert!(!has(visual, "fix-terminal"), "nothing can run");
    let error = window
        .update(visual, |view, _, _| view.cloud.backends.fix_error.clone())
        .unwrap()
        .expect("the user is told why Fix did nothing");
    assert!(
        error.contains("`brew`") && error.contains("brew install gh"),
        "{error}"
    );
    // Checking again clears it, and with brew found Fix opens the terminal.
    world.missing.lock().unwrap().clear();
    click(visual, "cloud-check-codespaces");
    assert!(
        window
            .update(visual, |view, _, _| view.cloud.backends.fix_error.is_none())
            .unwrap()
    );
    click(visual, "cloud-fix-codespaces");
    assert!(has(visual, "fix-terminal"));
}

#[gpui::test]
fn the_fix_hint_names_both_commands_when_both_backends_need_fixing(cx: &mut TestAppContext) {
    let world = World::new();
    world.gh.set(Ok(AUTH_NO_SCOPE));
    let window = open(&world, cx);
    let hint = window
        .update(cx, |view, _, _| view.fix_hint())
        .unwrap()
        .expect("both need fixing");
    assert!(
        hint.contains("gh auth refresh -h github.com -s codespace"),
        "{hint}"
    );
    assert!(hint.contains("sbx --cloud setup ssh"), "{hint}");

    // Only one: its command alone.
    world.set_up_ssh();
    window
        .update(cx, |view, _, cx| view.refresh_backends(cx))
        .unwrap();
    settle(cx);
    let hint = window
        .update(cx, |view, _, _| view.fix_hint())
        .unwrap()
        .unwrap();
    assert!(
        hint.contains("gh auth refresh") && !hint.contains("sbx"),
        "{hint}"
    );
}

#[gpui::test]
fn a_finished_save_cannot_be_pressed_again_into_a_validation_error(cx: &mut TestAppContext) {
    let world = World::new();
    let window = open(&world, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.refresh().unwrap();
    add_account(&window, visual, "octo-dev", PAT);
    let line = notice(&window, visual);
    assert!(line.as_ref().is_some_and(|(ok, _)| *ok));
    let calls = world.hub.calls.lock().unwrap().len();
    // Enter or a second Save press on the emptied inputs does nothing.
    window
        .update(visual, |view, _, cx| view.docker_save(cx))
        .unwrap();
    visual.run_until_parked();
    assert_eq!(notice(&window, visual), line);
    assert_eq!(world.hub.calls.lock().unwrap().len(), calls);
}
