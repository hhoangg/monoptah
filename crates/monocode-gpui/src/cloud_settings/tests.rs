use super::*;
use crate::cloud_settings::model::{self, Context as ValidateContext, SandboxForm};
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{
        docker_config::{self, DockerProjectConfig},
        project_cloud::CodespaceProjectConfig,
        readiness::{Id, State},
    },
    codespaces::GhOutput,
    docker_accounts::DockerAccountStatus,
    git_credentials::{GitOutput, GitPlatform},
    sandboxes::SbxOutput,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) const JWT: &str = "jwt-secret-value-0123456789";
const OAUTH: &str = "CLAUDE_CODE_OAUTH_TOKEN";
const AUTH_OK: &str = "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const MACHINES: &str = r#"{"total_count":3,"machines":[
 {"name":"basicLinux32gb","display_name":"2-core, 8 GB","operating_system":"linux","storage_in_bytes":34359738368,"memory_in_bytes":8589934592,"cpus":2},
 {"name":"standardLinux32gb","display_name":"4-core, 16 GB","operating_system":"linux","storage_in_bytes":34359738368,"memory_in_bytes":17179869184,"cpus":4},
 {"name":"premiumLinux","display_name":"8-core, 32 GB","operating_system":"linux","storage_in_bytes":68719476736,"memory_in_bytes":34359738368,"cpus":8}]}"#;

pub(crate) struct FakeGh {
    /// Listing machine types fails while set.
    machines_fail: AtomicBool,
    /// `gh secret set` fails while set.
    secret_fail: AtomicBool,
    pub(crate) calls: Mutex<Vec<Vec<String>>>,
    /// `(key, stdin)` of every `gh secret set`.
    pub(crate) secret_sets: Mutex<Vec<(String, String)>>,
}
impl FakeGh {
    fn new(machines_fail: bool) -> Arc<Self> {
        Arc::new(Self {
            machines_fail: AtomicBool::new(machines_fail),
            secret_fail: AtomicBool::new(false),
            calls: Mutex::new(Vec::new()),
            secret_sets: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn secret_calls(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|args| args.first().map(String::as_str) == Some("secret"))
            .count()
    }
}
impl GhRunner for FakeGh {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<GhOutput, String> {
        self.calls.lock().unwrap().push(args.to_vec());
        let ok = |stdout: &str| {
            Ok(GhOutput {
                success: true,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        };
        match args.first().map(String::as_str) {
            Some("auth") => ok(AUTH_OK),
            Some("api") if args.iter().any(|a| a.contains("/codespaces/machines")) => {
                if self.machines_fail.load(Ordering::SeqCst) {
                    Err("HTTP 500".into())
                } else {
                    ok(MACHINES)
                }
            }
            Some("api") if args.iter().any(|a| a.ends_with("/repositories")) => {
                ok(r#"[{"repositories":[]}]"#)
            }
            Some("api") => ok(r#"[{"secrets":[]}]"#),
            Some("secret") => {
                let key = args.get(2).cloned().unwrap_or_default();
                if self.secret_fail.load(Ordering::SeqCst) {
                    return Ok(GhOutput {
                        success: false,
                        stdout: String::new(),
                        stderr: "HTTP 422".into(),
                    });
                }
                self.secret_sets
                    .lock()
                    .unwrap()
                    .push((key, stdin.unwrap_or_default().to_owned()));
                ok("")
            }
            other => panic!("unexpected gh call {other:?}"),
        }
    }
}

#[derive(Default)]
pub(crate) struct FakeSbx {
    pub(crate) calls: Mutex<Vec<Option<String>>>,
}
impl SbxRunner for FakeSbx {
    fn run(&self, _: &[String], _: Option<&str>) -> Result<SbxOutput, String> {
        self.run_as(None, &[], None)
    }
    fn run_as(
        &self,
        account: Option<&str>,
        _: &[String],
        _: Option<&str>,
    ) -> Result<SbxOutput, String> {
        self.calls.lock().unwrap().push(account.map(str::to_owned));
        Ok(SbxOutput {
            success: true,
            stdout: "[]".into(),
            stderr: String::new(),
        })
    }
}

#[derive(Default)]
pub(crate) struct FakeGit {
    pub(crate) calls: Mutex<usize>,
}
impl GitRunner for FakeGit {
    fn run(
        &self,
        _: &[String],
        _: &[(String, String)],
        _: &monocode_core::git_credentials::Secret,
    ) -> Result<GitOutput, String> {
        *self.calls.lock().unwrap() += 1;
        Ok(GitOutput {
            success: true,
            stderr: String::new(),
        })
    }
}

pub(crate) fn https_origin(_: &Path) -> Option<String> {
    Some("https://github.com/acme/alpha.git".into())
}
fn ssh_origin(_: &Path) -> Option<String> {
    Some("git@github.com:acme/alpha.git".into())
}
fn no_claude() -> Option<PathBuf> {
    None
}
fn accounts(_: &Store) -> Result<Vec<DockerAccountSummary>, String> {
    let account = |id: &str, docker_id: &str, status| DockerAccountSummary {
        id: id.into(),
        docker_id: docker_id.into(),
        status,
        checked_at: None,
        expires_at: None,
        created_at: 1,
    };
    Ok(vec![
        account("acct-1", "alice", DockerAccountStatus::Valid),
        account("acct-2", "bob", DockerAccountStatus::Revoked),
    ])
}
fn docker_env(account: Option<&str>) -> Result<Option<(&'static str, Secret)>, DockerAuthError> {
    Ok(account.map(|_| ("DOCKER_ACCESS_TOKEN", Secret::new(JWT.into()))))
}

pub(crate) struct Scratch(pub(crate) PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "monoptah-cloud-settings-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) struct Fixture {
    _data: Scratch,
    home: Scratch,
    pub(crate) project_dir: Scratch,
    pub(crate) store: Database,
    pub(crate) gh: Arc<FakeGh>,
    pub(crate) sbx: Arc<FakeSbx>,
    pub(crate) git: Arc<FakeGit>,
}

pub(crate) fn project_of(fixture: &Fixture) -> String {
    fixture.project_dir.0.to_string_lossy().into_owned()
}

impl Fixture {
    pub(crate) fn new() -> Self {
        Self::with(false, true)
    }

    pub(crate) fn with(machines_fail: bool, ssh_alias: bool) -> Self {
        let data = Scratch::new();
        let home = Scratch::new();
        std::fs::create_dir_all(home.0.join(".ssh")).unwrap();
        if ssh_alias {
            std::fs::write(home.0.join(".ssh/config"), "Host sbx_cloud\n  HostName x\n").unwrap();
        }
        let project_dir = Scratch::new();
        std::fs::create_dir_all(project_dir.0.join(".devcontainer")).unwrap();
        std::fs::write(
            project_dir.0.join(".devcontainer/devcontainer.json"),
            r#"{"hostRequirements":{"cpus":4,"memory":"16gb"},
                "features":{"ghcr.io/devcontainers/features/sshd:1":{}}}"#,
        )
        .unwrap();
        let store = Store::open(&data.0.join("monocode.db")).unwrap();
        Self {
            _data: data,
            home,
            project_dir,
            store: Arc::new(Mutex::new(store)),
            gh: FakeGh::new(machines_fail),
            sbx: Arc::new(FakeSbx::default()),
            git: Arc::new(FakeGit::default()),
        }
    }

    pub(crate) fn env(&self, origin: fn(&Path) -> Option<String>) -> Env {
        Env {
            gh: self.gh.clone(),
            sbx: self.sbx.clone(),
            git: self.git.clone(),
            store: self.store.clone(),
            home: Some(self.home.0.clone()),
            origin,
            claude: no_claude,
            docker_accounts: accounts,
            docker_env,
            resolve: Arc::new(|name| Some(PathBuf::from("/opt/tools").join(name))),
            claude_accounts: vec![
                AccountChoice {
                    id: "claude-1".into(),
                    label: "work@company".into(),
                    kind: "setup-token",
                },
                AccountChoice {
                    id: "claude-2".into(),
                    label: "Team API key".into(),
                    kind: "API key",
                },
            ],
        }
    }

    pub(crate) fn open(
        &self,
        cx: &mut TestAppContext,
        backend: Option<DefaultBackend>,
        origin: fn(&Path) -> Option<String>,
    ) -> Entity<CloudSettings> {
        let (project, env) = (project_of(self), self.env(origin));
        let dialog = cx.new(|cx| CloudSettings::new(project, "alpha".into(), backend, env, cx));
        cx.run_until_parked();
        dialog
    }

    pub(crate) fn load(&self) -> ProjectCloud {
        project_cloud::load(&self.store.lock().unwrap(), &project_of(self)).unwrap()
    }
}

fn saved_sandbox() -> DockerProjectConfig {
    DockerProjectConfig {
        repo_url: "https://gitlab.com/g/r".into(),
        credential_id: Some("cred-1".into()),
        base_ref: "dev".into(),
        devcontainer_path: ".devcontainer/web/devcontainer.json".into(),
        cpus: 3,
        memory: "6g".into(),
        ttl: "90m".into(),
        platform: Some("linux/amd64".into()),
        claude_account: Some("claude-1".into()),
        docker_account: Some("acct-1".into()),
    }
}

fn summary(id: &str, platform: GitPlatform) -> GitCredentialSummary {
    GitCredentialSummary {
        id: id.into(),
        label: format!("profile {id}"),
        platform,
        origin: platform.origin(),
        username: "me".into(),
        created_at: 1,
    }
}

fn claude_accounts_fixture() -> Vec<AccountChoice> {
    vec![AccountChoice {
        id: "claude-1".into(),
        label: "work".into(),
        kind: "setup-token",
    }]
}

fn context<'a>(
    credentials: &'a [GitCredentialSummary],
    claude: &'a [AccountChoice],
    baseline: &'a SandboxForm,
    saved: bool,
) -> ValidateContext<'a> {
    ValidateContext {
        credentials,
        credentials_loaded: true,
        claude_accounts: claude,
        docker_accounts: None,
        sandbox_saved: saved,
        sandbox_baseline: baseline,
        sandbox_wanted: false,
    }
}

// ------------------------------------------------------------------ model

#[test]
fn prefill_takes_an_https_origin_on_a_supported_host_only() {
    for url in [
        "https://github.com/acme/alpha.git",
        "https://gitlab.com/group/sub/alpha",
    ] {
        assert_eq!(model::prefill_repo(Some(url)), url);
    }
    for url in [
        "git@github.com:acme/alpha.git",
        "ssh://git@github.com/acme/alpha.git",
        "http://github.com/acme/alpha",
        "https://bitbucket.org/acme/alpha",
        "https://gitlab.example.com/acme/alpha",
        "https://user:pw@github.com/acme/alpha",
        "",
    ] {
        assert_eq!(model::prefill_repo(Some(url)), "", "{url}");
    }
    assert_eq!(model::prefill_repo(None), "");
}

#[test]
fn an_existing_sandbox_config_opens_and_saves_back_unchanged() {
    // Migration: nothing is rewritten by opening and saving the dialog.
    let saved = ProjectCloud {
        default_backend: None,
        codespace: CodespaceProjectConfig::default(),
        sandbox: Some(saved_sandbox()),
    };
    let form = Form::initial(&saved, Some("https://github.com/other/other"));
    assert_eq!(form.sandbox.repo_url, "https://gitlab.com/g/r");
    assert_eq!(form.sandbox.claude.as_deref(), Some("claude-1"));
    assert_eq!(form.sandbox.credential_id.as_deref(), Some("cred-1"));
    assert_eq!(
        (
            form.sandbox.cpus,
            form.sandbox.memory.as_str(),
            form.sandbox.ttl.as_str()
        ),
        (3, "6g", "90m")
    );
    let credentials = [summary("cred-1", GitPlatform::GitLab)];
    let claude = claude_accounts_fixture();
    let baseline = form.sandbox.clone();
    let out = model::validate(&form, &context(&credentials, &claude, &baseline, true)).unwrap();
    assert_eq!(out.sandbox, Some(saved_sandbox()));
    assert_eq!(out.default_backend, None);
    // The values that are not presets stay selectable and selected.
    let sizes = model::size_options(3, "6g");
    assert_eq!(sizes.last(), Some(&(3, "6g".to_owned())));
    assert_eq!(sizes.len(), 6);
    let runtimes = model::runtime_options("90m");
    assert_eq!(runtimes.last().map(String::as_str), Some("90m"));
    assert_eq!(model::runtime_label("90m"), "90 minutes, then stop");
    assert_eq!(model::idle_options(45).last(), Some(&45));
    assert_eq!(model::idle_label(45), "After 45 minutes");
    // A preset is not duplicated.
    assert_eq!(model::size_options(2, "4g").len(), 5);
    assert_eq!(model::runtime_options("2h").len(), 4);
}

#[test]
fn the_labels_read_naturally() {
    assert_eq!(model::size_label(1, "2g"), "1 CPU, 2 GB");
    assert_eq!(model::size_label(16, "32g"), "16 CPUs, 32 GB");
    assert_eq!(model::size_label(2, "512m"), "2 CPUs, 512 MB");
    assert_eq!(model::idle_label(30), "After 30 minutes");
    assert_eq!(model::idle_label(60), "After 1 hour");
    assert_eq!(model::idle_label(240), "After 4 hours");
    assert_eq!(model::runtime_label("1h"), "1 hour, then stop");
    assert_eq!(model::runtime_label("8h"), "8 hours, then stop");
}

#[test]
fn the_codespace_part_round_trips() {
    let saved = ProjectCloud {
        default_backend: Some(DefaultBackend::Codespace),
        codespace: CodespaceProjectConfig {
            base_ref: "main".into(),
            devcontainer_path: ".devcontainer/web/devcontainer.json".into(),
            machine: Some("standardLinux32gb".into()),
            idle_timeout_minutes: 120,
            claude_account: Some("claude-1".into()),
        },
        sandbox: None,
    };
    let form = Form::initial(&saved, None);
    let claude = claude_accounts_fixture();
    let baseline = form.sandbox.clone();
    let out = model::validate(&form, &context(&[], &claude, &baseline, false)).unwrap();
    assert_eq!(out, saved);
}

#[test]
fn a_sandbox_never_configured_and_untouched_writes_nothing() {
    let saved = ProjectCloud::default();
    let mut form = Form::initial(&saved, Some("https://github.com/acme/alpha"));
    let claude = claude_accounts_fixture();
    let baseline = form.sandbox.clone();
    let ctx = context(&[], &claude, &baseline, false);
    assert_eq!(model::validate(&form, &ctx).unwrap().sandbox, None);
    // Editing it makes it a configuration.
    form.sandbox.cpus = 4;
    form.sandbox.memory = "8g".into();
    let out = model::validate(&form, &ctx).unwrap();
    assert_eq!(out.sandbox.map(|s| s.cpus), Some(4));
    // So does choosing Docker Sandbox as the default.
    let mut form = Form::initial(&saved, Some("https://github.com/acme/alpha"));
    form.default_backend = Some(DefaultBackend::Sandbox);
    assert!(model::validate(&form, &ctx).unwrap().sandbox.is_some());
    // And an empty URL is then the field's error.
    let mut empty = Form::initial(&saved, None);
    empty.default_backend = Some(DefaultBackend::Sandbox);
    let error = model::validate(&empty, &ctx).unwrap_err();
    assert_eq!(
        (error.backend, error.field),
        (DefaultBackend::Sandbox, Field::Repo)
    );
}

#[test]
fn validation_errors_land_under_the_right_field() {
    let claude = claude_accounts_fixture();
    let at = |edit: fn(&mut Form), creds: &[GitCredentialSummary]| {
        let mut form = Form::initial(
            &ProjectCloud::default(),
            Some("https://github.com/acme/alpha"),
        );
        form.default_backend = Some(DefaultBackend::Sandbox);
        edit(&mut form);
        let baseline = form.sandbox.clone();
        let error = model::validate(&form, &context(creds, &claude, &baseline, false)).unwrap_err();
        (error.backend, error.field)
    };
    let sb = DefaultBackend::Sandbox;
    let cs = DefaultBackend::Codespace;
    assert_eq!(
        at(
            |f| f.sandbox.repo_url = "git@github.com:a/b.git".into(),
            &[]
        ),
        (sb, Field::Repo)
    );
    assert_eq!(
        at(|f| f.sandbox.repo_url = String::new(), &[]),
        (sb, Field::Repo)
    );
    assert_eq!(
        at(
            |f| f.sandbox.repo_url = "https://tok@github.com/acme/alpha".into(),
            &[]
        ),
        (sb, Field::Repo)
    );
    assert_eq!(
        at(|f| f.sandbox.devcontainer = "/etc/passwd".into(), &[]),
        (sb, Field::Devcontainer)
    );
    assert_eq!(
        at(|f| f.sandbox.base_ref = "a..b".into(), &[]),
        (sb, Field::Base)
    );
    assert_eq!(at(|f| f.sandbox.cpus = 99, &[]), (sb, Field::Size));
    assert_eq!(
        at(|f| f.sandbox.ttl = "99h".into(), &[]),
        (sb, Field::Runtime)
    );
    assert_eq!(
        at(|f| f.sandbox.credential_id = Some("gone".into()), &[]),
        (sb, Field::Credential)
    );
    assert_eq!(
        at(
            |f| f.sandbox.credential_id = Some("c1".into()),
            &[summary("c1", GitPlatform::GitLab)]
        ),
        (sb, Field::Credential)
    );
    assert_eq!(
        at(|f| f.sandbox.claude = Some("missing".into()), &[]),
        (sb, Field::Claude)
    );
    assert_eq!(
        at(|f| f.codespace.base_ref = "a..b".into(), &[]),
        (cs, Field::Base)
    );
    assert_eq!(
        at(|f| f.codespace.devcontainer = "/etc/passwd".into(), &[]),
        (cs, Field::Devcontainer)
    );
    assert_eq!(at(|f| f.codespace.idle = 3, &[]), (cs, Field::Idle));
    assert_eq!(
        at(|f| f.codespace.machine = Some("bad name".into()), &[]),
        (cs, Field::Machine)
    );
    assert_eq!(
        at(|f| f.codespace.claude = Some("missing".into()), &[]),
        (cs, Field::Claude)
    );
    // A good credential for the host passes.
    let mut form = Form::initial(
        &ProjectCloud::default(),
        Some("https://github.com/acme/alpha"),
    );
    form.default_backend = Some(DefaultBackend::Sandbox);
    form.sandbox.credential_id = Some("c1".into());
    let baseline = form.sandbox.clone();
    let credentials = [summary("c1", GitPlatform::GitHub)];
    assert!(model::validate(&form, &context(&credentials, &claude, &baseline, false)).is_ok());
}

#[test]
fn a_docker_account_that_no_longer_exists_is_refused() {
    let claude = claude_accounts_fixture();
    let mut form = Form::initial(
        &ProjectCloud::default(),
        Some("https://github.com/acme/alpha"),
    );
    form.default_backend = Some(DefaultBackend::Sandbox);
    form.sandbox.docker_account = Some("gone".into());
    let baseline = form.sandbox.clone();
    let listed = accounts(&Store::open(Path::new(":memory:")).unwrap()).unwrap();
    let mut ctx = context(&[], &claude, &baseline, false);
    ctx.docker_accounts = Some(&listed);
    let error = model::validate(&form, &ctx).unwrap_err();
    assert_eq!(error.field, Field::DockerAccount);
    form.sandbox.docker_account = Some("acct-2".into());
    assert!(model::validate(&form, &ctx).is_ok());
}

#[test]
fn only_credentials_for_the_repository_host_are_offered() {
    let all = [
        summary("a", GitPlatform::GitHub),
        summary("b", GitPlatform::GitLab),
    ];
    let ids = |url: &str| -> Vec<String> {
        model::matching_credentials(&all, url)
            .into_iter()
            .map(|c| c.id)
            .collect()
    };
    assert_eq!(ids("https://github.com/o/r"), ["a"]);
    assert_eq!(ids("https://gitlab.com/o/r"), ["b"]);
    assert_eq!(ids("not a url"), ["a", "b"]);
}

#[test]
fn a_size_below_the_devcontainer_warns_but_stays_selectable() {
    let info = DevcontainerInfo {
        host_cpus: Some(4),
        host_memory_mib: Some(16 * 1024),
        ..Default::default()
    };
    assert_eq!(
        model::requirement_hint(&info).as_deref(),
        Some("Asks for 4 CPUs and 16 GB")
    );
    assert!(model::size_warning(2, Some(4096), &info).is_some());
    assert!(
        model::size_warning(4, Some(4096), &info).is_some(),
        "memory alone"
    );
    assert!(model::size_warning(8, Some(16 * 1024), &info).is_none());
    assert!(model::size_warning(1, Some(1024), &DevcontainerInfo::default()).is_none());
    assert_eq!(model::requirement_hint(&DevcontainerInfo::default()), None);
    // Every size stays in the list.
    assert_eq!(model::size_options(1, "2g").len(), 5);
}

#[test]
fn the_machine_the_devcontainer_asks_for_is_marked() {
    let machine = |name: &str, display: &str, cpus: u32, gb: u64| MachineType {
        name: name.into(),
        display_name: display.into(),
        cpus,
        memory_in_bytes: gb * 1024 * 1024 * 1024,
        ..Default::default()
    };
    let machines = [
        machine("basicLinux32gb", "2-core, 8 GB", 2, 8),
        machine("standardLinux32gb", "4-core, 16 GB", 4, 16),
        machine("premiumLinux", "8-core, 32 GB", 8, 32),
    ];
    let info = DevcontainerInfo {
        host_cpus: Some(4),
        host_memory_mib: Some(16 * 1024),
        ..Default::default()
    };
    let labels: Vec<_> = model::machine_options(&machines, &info)
        .into_iter()
        .map(|o| o.label)
        .collect();
    assert_eq!(
        labels,
        [
            "2-core, 8 GB",
            "4-core, 16 GB (devcontainer)",
            "8-core, 32 GB"
        ]
    );
    // No requirement, no mark.
    let plain = model::machine_options(&machines, &DevcontainerInfo::default());
    assert!(plain.iter().all(|o| !o.label.contains("devcontainer")));
    // None selects the machine a codespace starts on, a saved one while it is allowed.
    assert_eq!(
        model::selected_machine(&machines, None).as_deref(),
        Some("basicLinux32gb")
    );
    assert_eq!(
        model::selected_machine(&machines, Some("premiumLinux")).as_deref(),
        Some("premiumLinux")
    );
    // A saved machine the repository no longer allows is not offered; the smallest allowed one
    // is selected instead and the dialog says so.
    let allowed = &machines[1..];
    assert_eq!(
        model::unavailable_machine(allowed, Some("basicLinux32gb")),
        Some("basicLinux32gb")
    );
    assert_eq!(
        model::unavailable_machine(allowed, Some("premiumLinux")),
        None
    );
    assert_eq!(model::unavailable_machine(allowed, None), None);
    assert_eq!(
        model::selected_machine(allowed, Some("basicLinux32gb")).as_deref(),
        Some("standardLinux32gb")
    );
}

#[test]
fn a_docker_account_label_names_a_token_that_needs_replacing() {
    let listed = accounts(&Store::open(Path::new(":memory:")).unwrap()).unwrap();
    assert_eq!(model::docker_account_label(&listed[0]), "alice");
    assert_eq!(
        model::docker_account_label(&listed[1]),
        "bob (token needs replacing)"
    );
}

#[test]
fn paths_show_the_home_folder_as_a_tilde() {
    let home = Path::new("/Users/me");
    assert_eq!(
        model::display_path("/Users/me/work/app", Some(home)),
        "~/work/app"
    );
    assert_eq!(model::display_path("/Users/me", Some(home)), "~");
    assert_eq!(model::display_path("/srv/app", Some(home)), "/srv/app");
    assert_eq!(model::display_path("/srv/app", None), "/srv/app");
}

// ------------------------------------------------------------------ dialog

fn items(dialog: &CloudSettings, backend: DefaultBackend) -> Vec<Item> {
    match &dialog.ready[ix(backend)] {
        Ready::Done(items) => items.clone(),
        _ => panic!("the checklist is not done"),
    }
}

#[gpui::test]
fn it_opens_on_the_given_backend_else_the_projects_default(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let on = |dialog: &Entity<CloudSettings>, cx: &mut TestAppContext| {
        dialog.read_with(cx, |d, _| d.tab)
    };
    let dialog = fixture.open(cx, None, https_origin);
    assert_eq!(
        on(&dialog, cx),
        DefaultBackend::Codespace,
        "no sandbox saved"
    );
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    assert_eq!(on(&dialog, cx), DefaultBackend::Sandbox);
    // A saved sandbox makes it the default.
    docker_config::save(
        &fixture.store.lock().unwrap(),
        &project_of(&fixture),
        &DockerProjectConfig {
            repo_url: "https://github.com/acme/alpha".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let dialog = fixture.open(cx, None, https_origin);
    assert_eq!(on(&dialog, cx), DefaultBackend::Sandbox);
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    assert_eq!(on(&dialog, cx), DefaultBackend::Codespace);
}

#[gpui::test]
fn the_codespace_checklist_is_computed_when_the_tab_opens(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.read_with(cx, |d, _| {
        let list = items(d, DefaultBackend::Codespace);
        assert!(readiness::all_ok(&list), "{list:?}");
        assert!(list.iter().any(|i| i.id == Id::Sshd));
        // The other tab is not checked until it is shown.
        assert!(matches!(d.ready[1], Ready::NotStarted));
    });
    assert_eq!(*fixture.git.calls.lock().unwrap(), 0);
    assert!(fixture.sbx.calls.lock().unwrap().is_empty());
}

#[gpui::test]
fn the_sandbox_checklist_is_computed_on_first_switch(cx: &mut TestAppContext) {
    let fixture = Fixture::with(false, false);
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| d.switch_tab(DefaultBackend::Sandbox, None, cx));
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        let list = items(d, DefaultBackend::Sandbox);
        let ssh = list.iter().find(|i| i.id == Id::Ssh).unwrap();
        assert_eq!(ssh.state, State::Problem);
        assert_eq!(ssh.fix, Some(Fix::Command(readiness::SBX_SSH_COMMAND)));
        let repo = list.iter().find(|i| i.id == Id::Repository).unwrap();
        assert_eq!(repo.state, State::Ok, "the public check ran");
    });
    assert_eq!(*fixture.git.calls.lock().unwrap(), 1);
    assert_eq!(*fixture.sbx.calls.lock().unwrap(), [None]);
    // Switching back and forth does not repeat the checks.
    dialog.update(cx, |d, cx| {
        d.switch_tab(DefaultBackend::Codespace, None, cx);
        d.switch_tab(DefaultBackend::Sandbox, None, cx);
    });
    cx.run_until_parked();
    assert_eq!(fixture.sbx.calls.lock().unwrap().len(), 1);
}

/// A finished Sandbox checklist whose Docker account line carries `fix`.
fn with_account_problem(dialog: &mut CloudSettings, fix: Option<Fix>) {
    dialog.ready[1] = Ready::Done(vec![Item {
        id: Id::DockerAccount,
        state: State::Problem,
        text: "Docker account alice: token revoked or expired.".into(),
        fix,
    }]);
}

#[gpui::test]
fn a_docker_account_that_is_not_available_points_at_the_picker_not_at_settings(
    cx: &mut TestAppContext,
) {
    // No Docker account service is installed in tests: the account cannot give a token.
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.pick(Pick::DockerAccount(Some("acct-2".into())), cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        let list = items(d, DefaultBackend::Sandbox);
        let account = list.iter().find(|i| i.id == Id::DockerAccount).unwrap();
        assert_eq!(account.state, State::Problem);
        assert_eq!(account.fix, None, "replacing the token would not help");
        assert!(
            account.text.contains("Docker account list below"),
            "{}",
            account.text
        );
        assert!(!account.text.contains("sbx login"), "{}", account.text);
        assert!(!account.text.contains(JWT));
    });
}

#[gpui::test]
fn replace_token_opens_settings_at_once_when_nothing_is_edited(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        with_account_problem(d, Some(Fix::ReplaceToken));
        d.run_fix(DefaultBackend::Sandbox, Fix::ReplaceToken, cx)
    });
    assert_eq!(events.take(), ["replace-token"]);
}

#[gpui::test]
fn replace_token_asks_before_discarding_unsaved_edits(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        with_account_problem(d, Some(Fix::ReplaceToken));
        d.set_text("sb_base", "release", cx);
        d.run_fix(DefaultBackend::Sandbox, Fix::ReplaceToken, cx);
        assert!(d.confirm_replace, "the question is shown");
    });
    assert!(events.take().is_empty(), "nothing is discarded yet");
    // Keep editing: the question goes and the edit is still there.
    dialog.update(cx, |d, cx| {
        assert!(d.dismiss_transient(None, cx));
        assert!(!d.confirm_replace);
        assert_eq!(d.text("sb_base", cx), "release");
    });
    assert!(events.take().is_empty());
    // Ask again and agree.
    dialog.update(cx, |d, cx| {
        d.run_fix(DefaultBackend::Sandbox, Fix::ReplaceToken, cx);
        assert!(d.confirm_replace);
        d.run_fix(DefaultBackend::Sandbox, Fix::ReplaceToken, cx);
    });
    assert_eq!(events.take(), ["replace-token"]);
    // Typed secrets count as unsaved edits too.
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.secrets
            .as_ref()
            .unwrap()
            .update(cx, |v, cx| v.set_field(OAUTH, "sk-ant-oat-x", cx));
        assert!(d.dirty(cx));
        d.run_fix(DefaultBackend::Codespace, Fix::ReplaceToken, cx);
        assert!(d.confirm_replace);
    });
    assert!(events.take().is_empty());
}

#[gpui::test]
fn a_result_for_a_changed_state_is_dropped(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.check_ready(DefaultBackend::Sandbox, cx);
        // The dialog moves on (closing bumps every generation) before the answer arrives.
        d.shutdown(cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert!(
            matches!(d.ready[1], Ready::Checking),
            "the stale answer is ignored"
        )
    });
}

#[gpui::test]
fn the_fix_for_ssh_carries_the_account_token_only_in_the_environment(cx: &mut TestAppContext) {
    let fixture = Fixture::with(false, false);
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.pick(Pick::DockerAccount(Some("acct-1".into())), cx)
    });
    cx.run_until_parked();
    let commands = commands_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.run_fix(
            DefaultBackend::Sandbox,
            Fix::Command(readiness::SBX_SSH_COMMAND),
            cx,
        );
        // The token is fetched off the UI thread: nothing is emitted synchronously, and a
        // second click meanwhile is ignored.
        assert!(d.fix_pending);
        d.run_fix(
            DefaultBackend::Sandbox,
            Fix::Command(readiness::SBX_SSH_COMMAND),
            cx,
        );
    });
    assert!(commands.borrow().is_empty());
    cx.run_until_parked();
    let taken = std::mem::take(&mut *commands.borrow_mut());
    assert_eq!(taken.len(), 1);
    let command = &taken[0];
    assert_eq!(command.line(), "sbx --cloud setup ssh");
    assert_eq!(
        command
            .env
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect::<Vec<_>>(),
        [("DOCKER_ACCESS_TOKEN", JWT)]
    );
    assert!(!command.line().contains(JWT));
    assert!(!command.args.iter().any(|a| a.contains(JWT)));
    assert!(!format!("{command:?}").contains(JWT));
    dialog.read_with(cx, |d, cx| {
        assert!(!d.fix_pending);
        for select in [Select::DockerAccount, Select::Claude, Select::Size] {
            assert!(!d.current_label(select, cx).contains(JWT));
        }
    });
    // Without an account the machine's sign-in is used: no token at all.
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    let commands = commands_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.run_fix(
            DefaultBackend::Sandbox,
            Fix::Command(readiness::SBX_SSH_COMMAND),
            cx,
        )
    });
    let taken = std::mem::take(&mut *commands.borrow_mut());
    assert_eq!(taken.len(), 1);
    assert!(taken[0].env.is_empty());
}

#[gpui::test]
fn a_gh_fix_runs_the_exact_command_and_checks_again_when_it_ends(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    let commands = commands_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.run_fix(
            DefaultBackend::Codespace,
            Fix::Command(readiness::GH_REFRESH_COMMAND),
            cx,
        )
    });
    let taken = std::mem::take(&mut *commands.borrow_mut());
    assert_eq!(
        taken[0].line(),
        "gh auth refresh -h github.com -s codespace"
    );
    assert_eq!(taken[0].program, std::ffi::OsString::from("/opt/tools/gh"));
    let before = fixture.gh.calls.lock().unwrap().len();
    // Another fix cannot start while this one runs.
    dialog.update(cx, |d, cx| {
        d.run_fix(
            DefaultBackend::Codespace,
            Fix::Command(readiness::GH_LOGIN_COMMAND),
            cx,
        )
    });
    assert!(commands.borrow().is_empty());
    dialog.update(cx, |d, cx| d.fix_finished(cx));
    cx.run_until_parked();
    assert!(
        fixture.gh.calls.lock().unwrap().len() > before,
        "checked again"
    );
    dialog.read_with(cx, |d, _| assert!(d.fix_backend.is_none()));
}

#[gpui::test]
fn machines_are_listed_off_the_ui_thread_and_a_failure_keeps_a_text_fallback(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.read_with(cx, |d, cx| {
        assert!(matches!(&d.machines, Machines::Loaded(list) if list.len() == 3));
        // The devcontainer asks for 4 CPUs and 16 GB: the 4-core machine is marked.
        let labels: Vec<_> = d
            .options(Select::Machine, cx)
            .into_iter()
            .map(|o| o.label)
            .collect();
        assert_eq!(labels[1], "4-core, 16 GB (devcontainer)");
        // None shows the machine a codespace starts on.
        assert_eq!(d.current_label(Select::Machine, cx), "2-core, 8 GB");
        // No machine and no Claude account chosen: the machine default and the secret.
        let typed = d.typed(cx);
        assert_eq!(typed.codespace.machine, None);
        assert_eq!(typed.codespace.claude, None);
        assert_eq!(
            d.current_label(Select::Claude, cx),
            model::CODESPACE_SECRET_LABEL
        );
        // The default machine is below the devcontainer's: warned, still selectable.
        assert!(d.machine_warning(cx).is_some());
    });
    dialog.update(cx, |d, cx| {
        d.pick(Pick::Machine(Some("standardLinux32gb".into())), cx)
    });
    dialog.read_with(cx, |d, cx| assert!(d.machine_warning(cx).is_none()));

    let failing = Fixture::with(true, true);
    let dialog = failing.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        assert!(matches!(d.machines, Machines::Failed(_)));
        d.set_text("machine", "premiumLinux", cx);
        d.save(cx);
    });
    let saved = failing.load();
    assert_eq!(saved.codespace.machine.as_deref(), Some("premiumLinux"));
}

#[gpui::test]
fn the_default_shown_is_the_default_stored_when_the_save_adds_the_sandbox(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    let shown = dialog.update(cx, |d, cx| {
        // No sandbox is stored: Codespaces is what the select shows. The user fills the
        // Docker tab and leaves the select alone.
        let shown = d.effective_default();
        assert_eq!(shown, DefaultBackend::Codespace);
        d.switch_tab(DefaultBackend::Sandbox, None, cx);
        d.set_text("repo", "https://github.com/acme/other", cx);
        d.save(cx);
        assert!(d.error.is_none(), "{:?}", d.error);
        shown
    });
    let saved = fixture.load();
    assert!(saved.sandbox.is_some(), "the sandbox was stored");
    assert_eq!(saved.effective_default_backend(), shown);
}

#[gpui::test]
fn switching_the_segment_keeps_both_backends_edits_and_save_writes_both(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.set_text("cs_base", "release", cx);
        d.pick(Pick::Idle(120), cx);
        d.switch_tab(DefaultBackend::Sandbox, None, cx);
        d.set_text("repo", "https://github.com/acme/other", cx);
        d.set_text("sb_base", "dev", cx);
        d.pick(Pick::Size(4, "8g".into()), cx);
        d.pick(Pick::Runtime("4h".into()), cx);
        d.pick(Pick::DockerAccount(Some("acct-1".into())), cx);
        d.pick(Pick::Claude(Some("claude-1".into())), cx);
        d.switch_tab(DefaultBackend::Codespace, None, cx);
        // The first backend's edits survived the trip.
        assert_eq!(d.text("cs_base", cx), "release");
        assert_eq!(d.form.codespace.idle, 120);
        d.pick(Pick::Claude(Some("claude-2".into())), cx);
        d.pick(Pick::Default(DefaultBackend::Sandbox), cx);
    });
    cx.run_until_parked();
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| d.save(cx));
    assert_eq!(events.take(), ["saved", "close"]);
    let saved = fixture.load();
    assert_eq!(saved.default_backend, Some(DefaultBackend::Sandbox));
    assert_eq!(saved.codespace.base_ref, "release");
    assert_eq!(saved.codespace.idle_timeout_minutes, 120);
    assert_eq!(saved.codespace.claude_account.as_deref(), Some("claude-2"));
    let sandbox = saved.sandbox.expect("the sandbox part was saved");
    assert_eq!(sandbox.repo_url, "https://github.com/acme/other");
    assert_eq!(sandbox.base_ref, "dev");
    assert_eq!(
        (sandbox.cpus, sandbox.memory.as_str(), sandbox.ttl.as_str()),
        (4, "8g", "4h")
    );
    assert_eq!(sandbox.docker_account.as_deref(), Some("acct-1"));
    assert_eq!(sandbox.claude_account.as_deref(), Some("claude-1"));
}

#[gpui::test]
fn cancel_writes_nothing_and_an_untouched_sandbox_stays_unconfigured(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.set_text("cs_base", "release", cx);
        d.switch_tab(DefaultBackend::Sandbox, None, cx);
    });
    cx.run_until_parked();
    // Closed without saving: nothing was written.
    dialog.update(cx, |d, cx| d.shutdown(cx));
    assert_eq!(fixture.load(), ProjectCloud::default());
    // Saving with the sandbox tab only looked at does not create a docker entry.
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.switch_tab(DefaultBackend::Sandbox, None, cx);
        d.switch_tab(DefaultBackend::Codespace, None, cx);
        d.set_text("cs_base", "release", cx);
        d.save(cx);
        assert!(d.error.is_none(), "{:?}", d.error);
    });
    let saved = fixture.load();
    assert_eq!(saved.codespace.base_ref, "release");
    assert!(saved.sandbox.is_none());
    assert_eq!(saved.default_backend, None, "not chosen, so not saved");
}

#[gpui::test]
fn opening_from_the_composer_on_docker_sandbox_saves_the_prefilled_sandbox(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        assert_eq!(d.text("repo", cx), "https://github.com/acme/alpha.git");
        d.save(cx);
        assert!(d.error.is_none(), "{:?}", d.error);
    });
    let saved = fixture.load();
    assert_eq!(
        saved.sandbox.map(|s| s.repo_url).as_deref(),
        Some("https://github.com/acme/alpha.git")
    );
}

#[gpui::test]
fn a_save_error_shows_under_its_field_on_its_tab(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.set_text("repo", "git@github.com:acme/alpha.git", cx);
        d.switch_tab(DefaultBackend::Codespace, None, cx);
        d.pick(Pick::Default(DefaultBackend::Sandbox), cx);
        d.save(cx);
        let error = d.error.as_ref().expect("the sandbox URL is refused");
        assert_eq!(
            (error.backend, error.field),
            (DefaultBackend::Sandbox, Field::Repo)
        );
        assert_eq!(
            d.tab,
            DefaultBackend::Sandbox,
            "the tab with the error is shown"
        );
        assert!(d.error_for(DefaultBackend::Sandbox, Field::Repo).is_some());
        assert!(
            d.error_for(DefaultBackend::Codespace, Field::Repo)
                .is_none()
        );
    });
    assert_eq!(
        fixture.load(),
        ProjectCloud::default(),
        "nothing was written"
    );
}

#[gpui::test]
fn the_repository_is_prefilled_from_an_https_origin_only_and_never_over_typing(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), ssh_origin);
    dialog.read_with(cx, |d, cx| assert_eq!(d.text("repo", cx), ""));

    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.read_with(cx, |d, cx| {
        assert_eq!(d.text("repo", cx), "https://github.com/acme/alpha.git")
    });

    // Typed before the origin arrives: kept.
    let (project, env) = (project_of(&fixture), fixture.env(https_origin));
    let dialog = cx.new(|cx| {
        CloudSettings::new(
            project,
            "alpha".into(),
            Some(DefaultBackend::Sandbox),
            env,
            cx,
        )
    });
    dialog.update(cx, |d, cx| {
        d.set_text("repo", "https://github.com/me/typed", cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, cx| {
        assert_eq!(d.text("repo", cx), "https://github.com/me/typed")
    });

    // A saved config is not replaced by the origin.
    docker_config::save(
        &fixture.store.lock().unwrap(),
        &project_of(&fixture),
        &DockerProjectConfig {
            repo_url: "https://gitlab.com/g/r".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.read_with(cx, |d, cx| {
        assert_eq!(d.text("repo", cx), "https://gitlab.com/g/r")
    });
}

#[gpui::test]
fn changing_the_repository_host_drops_a_credential_for_the_old_one(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.credentials = vec![summary("c1", GitPlatform::GitHub)];
        d.form.sandbox.credential_id = Some("c1".into());
        d.set_text("repo", "https://github.com/acme/other", cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.form.sandbox.credential_id.as_deref(), Some("c1"))
    });
    dialog.update(cx, |d, cx| d.set_text("repo", "https://gitlab.com/g/r", cx));
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| assert_eq!(d.form.sandbox.credential_id, None));
}

#[gpui::test]
fn the_git_choices_are_the_saved_credentials_for_the_host_plus_none(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.credentials = vec![
            summary("a", GitPlatform::GitHub),
            summary("b", GitPlatform::GitLab),
        ];
        d.pick(Pick::Git(Some("a".into())), cx);
    });
    dialog.read_with(cx, |d, cx| {
        let options = d.options(Select::Git, cx);
        let labels: Vec<_> = options
            .iter()
            .map(|o| (o.label.as_str(), o.selected))
            .collect();
        assert_eq!(
            labels,
            [
                ("profile a (github.com, saved credential)", true),
                ("None (read-only clone)", false)
            ]
        );
    });
}

#[gpui::test]
fn the_claude_picker_offers_the_codespaces_secret_only_on_codespaces(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.read_with(cx, |d, cx| {
        let labels: Vec<_> = d
            .options(Select::Claude, cx)
            .into_iter()
            .map(|o| o.label)
            .collect();
        assert_eq!(
            labels,
            [
                "Codespaces secret (CLAUDE_CODE_OAUTH_TOKEN)",
                "work@company",
                "Team API key"
            ]
        );
        assert!(
            d.secrets.is_some(),
            "the secrets block shows with the secret selected"
        );
    });
    dialog.update(cx, |d, cx| {
        d.pick(Pick::Claude(Some("claude-1".into())), cx)
    });
    dialog.read_with(cx, |d, cx| {
        assert_eq!(d.current_label(Select::Claude, cx), "work@company");
        assert_eq!(d.typed(cx).codespace.claude.as_deref(), Some("claude-1"));
    });
    dialog.update(cx, |d, cx| d.switch_tab(DefaultBackend::Sandbox, None, cx));
    dialog.read_with(cx, |d, cx| {
        let labels: Vec<_> = d
            .options(Select::Claude, cx)
            .into_iter()
            .map(|o| o.label)
            .collect();
        assert_eq!(labels, ["None", "work@company", "Team API key"]);
        assert_eq!(d.current_label(Select::Claude, cx), "None");
        assert_eq!(d.typed(cx).sandbox.claude, None);
    });
    dialog.update(cx, |d, cx| {
        d.pick(Pick::DockerAccount(Some("acct-1".into())), cx)
    });
    dialog.read_with(cx, |d, cx| {
        let typed = d.typed(cx);
        assert_eq!(d.current_label(Select::Size, cx), "2 CPUs, 4 GB");
        assert_eq!(typed.sandbox.claude, None);
        assert_eq!(typed.sandbox.docker_account.as_deref(), Some("acct-1"));
        assert_eq!(d.current_label(Select::DockerAccount, cx), "alice");
        let docker: Vec<_> = d
            .options(Select::DockerAccount, cx)
            .into_iter()
            .map(|o| o.label)
            .collect();
        assert_eq!(
            docker,
            [
                "Machine sign-in (sbx login)",
                "alice",
                "bob (token needs replacing)"
            ]
        );
    });
}

#[gpui::test]
fn the_secrets_block_is_only_built_when_it_is_needed(cx: &mut TestAppContext) {
    // Opening on the sandbox makes no gh call at all.
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.read_with(cx, |d, _| assert!(d.secrets.is_none()));
    assert!(fixture.gh.calls.lock().unwrap().is_empty());
    dialog.update(cx, |d, cx| {
        d.switch_tab(DefaultBackend::Codespace, None, cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| assert!(d.secrets.is_some()));
    assert!(!fixture.gh.calls.lock().unwrap().is_empty());
}

#[gpui::test]
fn closing_ends_the_secrets_view_and_answers_in_flight_are_ignored(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.load_machines(cx);
        d.shutdown(cx);
        assert!(d.secrets.is_none());
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| assert!(matches!(d.machines, Machines::Loading)));
}

#[gpui::test]
fn nothing_secret_is_in_the_text_the_dialog_draws_or_reports(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.pick(Pick::DockerAccount(Some("acct-1".into())), cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, cx| {
        let mut text: Vec<String> = items(d, DefaultBackend::Sandbox)
            .into_iter()
            .map(|i| i.text)
            .collect();
        text.extend(
            [Select::DockerAccount, Select::Claude, Select::Size]
                .into_iter()
                .map(|s| d.current_label(s, cx)),
        );
        text.extend(
            [
                Select::Default,
                Select::Size,
                Select::Runtime,
                Select::DockerAccount,
                Select::Claude,
                Select::Git,
            ]
            .into_iter()
            .flat_map(|s| d.options(s, cx).into_iter().map(|o| o.label)),
        );
        text.extend(d.error.iter().map(|e| e.message.clone()));
        for line in text {
            assert!(!line.contains(JWT) && !line.contains("dckr_pat_"), "{line}");
        }
    });
}

fn no_origin(_: &Path) -> Option<String> {
    None
}

#[gpui::test]
fn the_codespace_repository_row_tells_not_read_yet_from_no_origin(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (project, env) = (project_of(&fixture), fixture.env(no_origin));
    let dialog = cx.new(|cx| CloudSettings::new(project, "alpha".into(), None, env, cx));
    // The origin is read off the UI thread: until then it is not "no origin".
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.codespace_repo_text(), "Reading the project's origin...")
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.codespace_repo_text(), "The project has no git origin.")
    });
    let dialog = fixture.open(cx, None, https_origin);
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.codespace_repo_text(), "github.com/acme/alpha")
    });
}

#[gpui::test]
fn editing_the_repository_or_a_devcontainer_path_marks_that_checklist_changed(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| d.switch_tab(DefaultBackend::Sandbox, None, cx));
    cx.run_until_parked();
    let gh_calls = fixture.gh.calls.lock().unwrap().len();
    let sbx_calls = fixture.sbx.calls.lock().unwrap().len();
    dialog.read_with(cx, |d, _| assert_eq!(d.ready_stale, [false, false]));
    // A Codespaces devcontainer path changes the Codespaces checklist only.
    dialog.update(cx, |d, cx| {
        d.set_text("cs_dev", ".devcontainer/other.json", cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.ready_stale, [true, false]);
        assert!(
            matches!(d.ready[0], Ready::Done(_)),
            "the old answer stays shown"
        );
    });
    // The repository and the sandbox devcontainer path change the Sandbox checklist.
    dialog.update(cx, |d, cx| {
        d.set_text("repo", "https://github.com/acme/other", cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| assert_eq!(d.ready_stale, [true, true]));
    dialog.update(cx, |d, cx| {
        d.check_ready(DefaultBackend::Sandbox, cx);
        d.set_text("sb_dev", ".devcontainer/x.json", cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert!(
            matches!(d.ready[1], Ready::Changed),
            "an answer for the old text is dropped"
        );
    });
    // No command ran for a keystroke: only the one explicit check did.
    assert_eq!(fixture.gh.calls.lock().unwrap().len(), gh_calls);
    assert_eq!(fixture.sbx.calls.lock().unwrap().len(), sbx_calls + 1);
    // Check again brings both back.
    dialog.update(cx, |d, cx| {
        d.check_ready(DefaultBackend::Sandbox, cx);
        d.check_ready(DefaultBackend::Codespace, cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.ready_stale, [false, false]);
        assert!(matches!(d.ready[0], Ready::Done(_)) && matches!(d.ready[1], Ready::Done(_)));
    });
    // Fields no check reads do not mark anything.
    dialog.update(cx, |d, cx| {
        d.set_text("cs_base", "dev", cx);
        d.set_text("sb_base", "dev", cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| assert_eq!(d.ready_stale, [false, false]));
}

#[gpui::test]
fn a_saved_credential_blocks_save_until_the_credential_list_is_read(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    docker_config::save(
        &fixture.store.lock().unwrap(),
        &project_of(&fixture),
        &DockerProjectConfig {
            repo_url: "https://github.com/acme/alpha".into(),
            credential_id: Some("cred-1".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let (project, env) = (project_of(&fixture), fixture.env(https_origin));
    let dialog = cx.new(|cx| {
        CloudSettings::new(
            project,
            "alpha".into(),
            Some(DefaultBackend::Sandbox),
            env,
            cx,
        )
    });
    let events = events_of(&dialog, cx);
    // The list is still being read (the read runs off the UI thread).
    dialog.update(cx, |d, cx| {
        assert!(!d.credentials_loaded);
        d.save(cx);
        let error = d.error.as_ref().expect("save is refused");
        assert_eq!(error.field, Field::Credential);
        assert!(
            error
                .message
                .starts_with("Still reading the saved Git credentials"),
            "{}",
            error.message
        );
    });
    assert!(events.take().is_empty());
    assert_eq!(
        fixture.load().sandbox.unwrap().credential_id.as_deref(),
        Some("cred-1"),
        "nothing was rewritten"
    );
    // Without a saved credential there is nothing to wait for.
    let fresh = Fixture::new();
    let (project, env) = (project_of(&fresh), fresh.env(https_origin));
    let dialog = cx.new(|cx| {
        CloudSettings::new(
            project,
            "alpha".into(),
            Some(DefaultBackend::Sandbox),
            env,
            cx,
        )
    });
    dialog.update(cx, |d, cx| {
        d.set_text("repo", "https://github.com/acme/alpha", cx);
        d.save(cx);
        assert!(d.error.is_none(), "{:?}", d.error);
    });
}

#[gpui::test]
fn a_general_save_error_keeps_the_tab_the_user_is_on(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.show_error(
            FieldError {
                backend: DefaultBackend::Sandbox,
                field: Field::General,
                message: "The database is locked.".into(),
            },
            cx,
        );
        assert_eq!(d.tab, DefaultBackend::Codespace);
        assert!(d.error.is_some());
        // A field error still brings its tab forward.
        d.show_error(
            FieldError {
                backend: DefaultBackend::Sandbox,
                field: Field::Repo,
                message: "x".into(),
            },
            cx,
        );
        assert_eq!(d.tab, DefaultBackend::Sandbox);
    });
    // And through a real save: secrets that cannot be saved (no GitHub origin) are a general
    // error, reported on the tab the user is looking at.
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), no_origin);
    dialog.update(cx, |d, cx| {
        d.secrets
            .as_ref()
            .unwrap()
            .update(cx, |v, cx| v.set_field(OAUTH, "sk-ant-oat-x", cx));
        d.switch_tab(DefaultBackend::Sandbox, None, cx);
        d.save(cx);
        let error = d.error.as_ref().expect("the secrets could not be saved");
        assert_eq!(error.field, Field::General);
        assert_eq!(d.tab, DefaultBackend::Sandbox);
    });
}

#[gpui::test]
fn a_picked_machine_survives_a_failed_reload(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.pick(Pick::Machine(Some("standardLinux32gb".into())), cx)
    });
    // GitHub stops answering; the list is read again.
    fixture.gh.machines_fail.store(true, Ordering::SeqCst);
    dialog.update(cx, |d, cx| d.load_machines(cx));
    cx.run_until_parked();
    dialog.update(cx, |d, cx| {
        assert!(matches!(d.machines, Machines::Failed(_)));
        assert_eq!(
            d.typed(cx).codespace.machine.as_deref(),
            Some("standardLinux32gb"),
            "the text fallback starts from the pick"
        );
        assert_eq!(d.text("machine", cx), "standardLinux32gb");
        d.save(cx);
    });
    assert_eq!(
        fixture.load().codespace.machine.as_deref(),
        Some("standardLinux32gb")
    );
}

#[gpui::test]
fn the_machines_are_listed_for_the_base_branch_and_a_saved_one_that_is_gone_is_replaced(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    // A saved machine the repository does not list (a devcontainer on another branch rules it
    // out) is not offered; the smallest listed one is selected and the dialog says so.
    dialog.update(cx, |d, cx| {
        d.pick(Pick::Machine(Some("legacyLinux".into())), cx);
        d.set_text("cs_base", "feature/big", cx);
        d.load_machines(cx);
    });
    cx.run_until_parked();
    let asked = fixture.gh.calls.lock().unwrap().clone();
    assert!(
        asked.iter().any(|call| call
            .iter()
            .any(|a| a.ends_with("/codespaces/machines?ref=feature%2Fbig"))),
        "{asked:?}"
    );
    dialog.read_with(cx, |d, cx| {
        let labels: Vec<_> = d
            .options(Select::Machine, cx)
            .into_iter()
            .map(|o| o.label)
            .collect();
        assert_eq!(labels.len(), 3, "{labels:?}");
        assert!(labels.iter().all(|label| !label.contains("legacyLinux")));
        assert_eq!(d.current_label(Select::Machine, cx), "2-core, 8 GB");
        assert!(d.machine_warning(cx).is_some());
    });
}

#[gpui::test]
fn leaving_the_codespaces_secret_ends_a_running_setup_token(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let env = Env {
        claude: || Some(PathBuf::from("/usr/bin/true")),
        ..fixture.env(https_origin)
    };
    let (project, env) = (project_of(&fixture), env);
    let dialog = cx.new(|cx| {
        CloudSettings::new(
            project,
            "alpha".into(),
            Some(DefaultBackend::Codespace),
            env,
            cx,
        )
    });
    cx.run_until_parked();
    let secrets = dialog.read_with(cx, |d, _| d.secrets.clone().expect("the secrets block"));
    secrets.update(cx, |view, cx| view.generate_token(cx));
    let terminal = secrets.read_with(cx, |view, _| view.terminal_entity().expect("running"));
    dialog.update(cx, |d, cx| {
        d.pick(Pick::Claude(Some("claude-1".into())), cx)
    });
    cx.run_until_parked();
    secrets.read_with(cx, |view, _| assert!(!view.generating()));
    // The command prints a token as it ends: it must not become the secret.
    let token = format!("sk-ant-oat01-{}", "a".repeat(90));
    cx.update(|cx| {
        terminal.update(cx, |_, cx| {
            cx.emit(crate::terminal::TerminalEvent::Exited {
                code: Some(0),
                error: None,
                tail: format!("\n{token}\n"),
            })
        })
    });
    cx.run_until_parked();
    assert_eq!(fixture.gh.secret_calls(), 0, "nothing was saved");
    dialog.read_with(cx, |d, cx| assert_eq!(d.secret_text(cx), ""));
}

#[gpui::test]
fn reading_the_credentials_again_drops_the_answer_of_an_older_read(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.credentials_loaded = false;
        // Two reads in flight; the dialog closes before either answers.
        d.reload_credentials(Some("c1".into()), cx);
        d.reload_credentials(None, cx);
        assert_eq!(d.credentials_gen, 3);
        d.shutdown(cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert!(!d.credentials_loaded, "no stale answer was applied");
        assert_eq!(d.form.sandbox.credential_id, None);
    });
    // Superseded: the pick asked for by the first read lands with the newest answer.
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.reload_credentials(Some("c1".into()), cx);
        d.reload_credentials(None, cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.form.sandbox.credential_id.as_deref(), Some("c1"))
    });
}

#[gpui::test]
fn the_devcontainer_is_read_off_the_ui_thread_and_only_for_its_own_fields(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.read_with(cx, |d, _| assert_eq!(d.infos[0].host_cpus, Some(4)));
    let generation = dialog.read_with(cx, |d, _| d.infos_gen);
    // Typing elsewhere reads nothing.
    dialog.update(cx, |d, cx| {
        d.set_text("cs_base", "release", cx);
        d.set_text("sb_base", "release", cx);
        d.set_text("machine", "x", cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| assert_eq!(d.infos_gen, generation));
    // A new path is read in the background: the old hint stays until the answer is in.
    dialog.update(cx, |d, cx| d.set_text("cs_dev", "missing.json", cx));
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.infos[0].host_cpus, Some(4), "not read on the UI thread")
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.infos_gen, generation + 1);
        assert_eq!(d.infos[0].host_cpus, None);
    });
}

#[gpui::test]
fn the_footer_save_also_saves_the_typed_codespaces_secrets(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.secrets
            .as_ref()
            .unwrap()
            .update(cx, |v, cx| v.set_field(OAUTH, "  sk-ant-oat-footer ", cx));
        d.set_text("cs_base", "release", cx);
        d.save(cx);
        // The secret goes off the UI thread: the settings are not stored before it landed.
        assert!(d.saving);
    });
    assert!(events.take().is_empty());
    assert_eq!(fixture.load(), ProjectCloud::default());
    cx.run_until_parked();
    assert_eq!(events.take(), ["saved", "close"]);
    assert_eq!(
        *fixture.gh.secret_sets.lock().unwrap(),
        [(OAUTH.to_owned(), "sk-ant-oat-footer".to_owned())],
        "the value reached gh over stdin, trimmed"
    );
    assert!(
        fixture
            .gh
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|argv| !argv.iter().any(|a| a.contains("sk-ant-oat-footer"))),
        "never in argv"
    );
    assert_eq!(fixture.load().codespace.base_ref, "release");
    dialog.read_with(cx, |d, cx| {
        assert_eq!(d.secret_text(cx), "", "the field is emptied")
    });
}

#[gpui::test]
fn a_refused_secret_keeps_the_dialog_open_and_stores_nothing(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    fixture.gh.secret_fail.store(true, Ordering::SeqCst);
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.secrets
            .as_ref()
            .unwrap()
            .update(cx, |v, cx| v.set_field(OAUTH, "sk-ant-oat-refused", cx));
        d.set_text("cs_base", "release", cx);
        d.switch_tab(DefaultBackend::Sandbox, None, cx);
        d.switch_tab(DefaultBackend::Codespace, None, cx);
        d.save(cx);
    });
    cx.run_until_parked();
    assert!(events.take().is_empty(), "the dialog stays open");
    assert_eq!(
        fixture.load(),
        ProjectCloud::default(),
        "nothing was stored"
    );
    dialog.read_with(cx, |d, cx| {
        let error = d.error.as_ref().expect("the error is shown");
        assert_eq!(error.field, Field::General);
        assert!(error.message.contains(OAUTH), "{}", error.message);
        assert!(!error.message.contains("sk-ant-oat-refused"));
        assert!(!d.saving);
        assert_eq!(d.secret_text(cx), "sk-ant-oat-refused", "kept for a retry");
    });
    // The retry works once GitHub accepts it.
    fixture.gh.secret_fail.store(false, Ordering::SeqCst);
    dialog.update(cx, |d, cx| d.save(cx));
    cx.run_until_parked();
    assert_eq!(events.take(), ["saved", "close"]);
    assert_eq!(fixture.load().codespace.base_ref, "release");
}

#[gpui::test]
fn typed_secrets_that_cannot_be_saved_are_reported_not_dropped(cx: &mut TestAppContext) {
    // gh answers, but the project has no GitHub origin: the secrets cannot be saved.
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), no_origin);
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.secrets
            .as_ref()
            .unwrap()
            .update(cx, |v, cx| v.set_field(OAUTH, "sk-ant-oat-lost", cx));
        d.save(cx);
        let error = d.error.as_ref().expect("reported");
        assert_eq!(error.field, Field::General);
        assert!(
            error.message.contains("cannot be saved"),
            "{}",
            error.message
        );
    });
    cx.run_until_parked();
    assert!(events.take().is_empty());
    assert_eq!(fixture.gh.secret_calls(), 0);
}

#[gpui::test]
fn the_secrets_block_asks_gh_nothing_the_ready_check_did_not(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let _dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    let auth_calls = fixture
        .gh
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|argv| argv.first().map(String::as_str) == Some("auth"))
        .count();
    assert_eq!(auth_calls, 1, "one `gh auth status` for the whole dialog");
}

#[gpui::test]
fn save_without_edits_keeps_a_stored_config_byte_for_byte(cx: &mut TestAppContext) {
    // Migration: a project saved by the old Docker Sandbox dialog and one that used only
    // the Codespaces secret open and save without a change.
    let full = DockerProjectConfig {
        credential_id: None,
        ..saved_sandbox()
    };
    let fixture = Fixture::new();
    docker_config::save(&fixture.store.lock().unwrap(), &project_of(&fixture), &full).unwrap();
    let raw = |fixture: &Fixture, key: &str| fixture.store.lock().unwrap().setting(key).unwrap();
    let before = raw(&fixture, docker_config::SETTING);
    assert!(before.is_some());
    assert_eq!(raw(&fixture, project_cloud::SETTING), None);
    let dialog = fixture.open(cx, None, https_origin);
    dialog.update(cx, |d, cx| {
        assert_eq!(
            d.tab,
            DefaultBackend::Sandbox,
            "a saved sandbox is the default"
        );
        d.save(cx);
        assert!(d.error.is_none(), "{:?}", d.error);
    });
    cx.run_until_parked();
    assert_eq!(
        raw(&fixture, docker_config::SETTING),
        before,
        "byte-identical"
    );
    assert_eq!(fixture.gh.secret_calls(), 0, "no secret was set");
    assert_eq!(
        raw(&fixture, project_cloud::SETTING),
        None,
        "no cloud-projects.v1 default was written"
    );

    // A credential that no longer exists is the one documented change: it is dropped, the
    // rest stays as stored.
    let other = Fixture::new();
    let with_credential = saved_sandbox();
    assert_eq!(with_credential.credential_id.as_deref(), Some("cred-1"));
    docker_config::save(
        &other.store.lock().unwrap(),
        &project_of(&other),
        &with_credential,
    )
    .unwrap();
    let dialog = other.open(cx, None, https_origin);
    dialog.update(cx, |d, cx| {
        d.save(cx);
        assert!(d.error.is_none(), "{:?}", d.error);
    });
    cx.run_until_parked();
    let expected = Fixture::new();
    docker_config::save(&expected.store.lock().unwrap(), &project_of(&other), &full).unwrap();
    assert_eq!(
        raw(&other, docker_config::SETTING),
        raw(&expected, docker_config::SETTING),
        "only credentialId is gone"
    );

    // A codespace project that used only the old secret: nothing is invented.
    let codespace = Fixture::new();
    let dialog = codespace.open(cx, None, https_origin);
    dialog.update(cx, |d, cx| {
        assert_eq!(d.tab, DefaultBackend::Codespace);
        d.save(cx);
        assert!(d.error.is_none(), "{:?}", d.error);
    });
    cx.run_until_parked();
    assert_eq!(codespace.gh.secret_calls(), 0, "no secret was set");
    assert_eq!(
        raw(&codespace, docker_config::SETTING),
        None,
        "no docker entry"
    );
    assert_eq!(
        raw(&codespace, project_cloud::SETTING),
        None,
        "no cloud-projects.v1 default was written"
    );
    assert_eq!(codespace.load(), ProjectCloud::default());
}

#[gpui::test]
fn the_ready_chip_names_the_docker_account_and_the_subtitle_matches_the_mockup(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    dialog.update(cx, |d, cx| {
        d.pick(Pick::DockerAccount(Some("acct-1".into())), cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        let account = ready_item(d, DefaultBackend::Sandbox, Id::DockerAccount);
        // The item itself is the core's; the chip names the Docker ID.
        assert_eq!(
            d.chip_text(&Item {
                state: State::Ok,
                ..account
            }),
            "Docker account alice"
        );
    });
    assert_eq!(
        READY_NOTE,
        "Checked when this tab opens. Every start checks sign-in and the Docker account again."
    );
}

fn ready_item(dialog: &CloudSettings, backend: DefaultBackend, id: Id) -> Item {
    items(dialog, backend)
        .into_iter()
        .find(|i| i.id == id)
        .unwrap()
}

// ------------------------------------------------------------------ helpers

/// The events the dialog emitted, as short names.
#[derive(Clone, Default)]
struct Seen(Rc<RefCell<Vec<&'static str>>>);
impl Seen {
    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut self.0.borrow_mut())
    }
}

fn events_of(dialog: &Entity<CloudSettings>, cx: &mut TestAppContext) -> Seen {
    let seen = Seen::default();
    let sink = seen.clone();
    cx.update(|cx| {
        cx.subscribe(dialog, move |_, event: &Event, _| {
            sink.0.borrow_mut().push(match event {
                Event::Close => "close",
                Event::Saved => "saved",
                Event::Resized => "resized",
                Event::Fix(_) => "fix",
                Event::ReplaceDockerToken => "replace-token",
            })
        })
        .detach();
    });
    seen
}

/// The fix commands the dialog handed out.
fn commands_of(
    dialog: &Entity<CloudSettings>,
    cx: &mut TestAppContext,
) -> Rc<RefCell<Vec<FixCommand>>> {
    let commands: Rc<RefCell<Vec<FixCommand>>> = Default::default();
    let sink = commands.clone();
    cx.update(|cx| {
        cx.subscribe(dialog, move |_, event: &Event, _| {
            if let Event::Fix(slot) = event
                && let Some(command) = slot.take()
            {
                sink.borrow_mut().push(command);
            }
        })
        .detach();
    });
    commands
}

#[test]
fn the_helper_captions_under_the_form_fields_are_gone() {
    // The captions were literals in these two files; a caption that is not in the source
    // cannot be rendered.
    let source = [include_str!("mod.rs"), include_str!("model.rs")].concat();
    for caption in [
        "Empty means",
        "HTTPS, GitHub or GitLab.com.",
        "Relative to the repository root",
        "Creates and runs this project's sandboxes",
        "Also signs in gh for pull requests",
        "Applies from the next start or reconnect. Running sessions keep the account",
        "No Claude account with a setup-token or API key",
        "Type one, or leave empty for",
        "Same fields, same order for both backends",
        "One Claude picker for both backends",
    ] {
        assert!(!source.contains(caption), "{caption}");
    }
    // The one line the spec requires, once, for the whole Credentials group.
    assert_eq!(
        source
            .matches("applies from the next start or reconnect")
            .count(),
        1
    );
    assert_eq!(
        model::ACCOUNT_CHANGE_NOTE,
        "Changing an account applies from the next start or reconnect; running sessions keep theirs."
    );
    assert_eq!(source.matches("ACCOUNT_CHANGE_NOTE").count(), 2);
}
