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
use std::{cell::RefCell, rc::Rc, sync::Mutex};

pub(crate) const JWT: &str = "jwt-secret-value-0123456789";
const AUTH_OK: &str = "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const MACHINES: &str = r#"{"total_count":3,"machines":[
 {"name":"basicLinux32gb","display_name":"2-core, 8 GB","operating_system":"linux","storage_in_bytes":34359738368,"memory_in_bytes":8589934592,"cpus":2},
 {"name":"standardLinux32gb","display_name":"4-core, 16 GB","operating_system":"linux","storage_in_bytes":34359738368,"memory_in_bytes":17179869184,"cpus":4},
 {"name":"premiumLinux","display_name":"8-core, 32 GB","operating_system":"linux","storage_in_bytes":68719476736,"memory_in_bytes":34359738368,"cpus":8}]}"#;

pub(crate) struct FakeGh {
    machines_fail: bool,
    pub(crate) calls: Mutex<Vec<Vec<String>>>,
}
impl FakeGh {
    fn new(machines_fail: bool) -> Arc<Self> {
        Arc::new(Self {
            machines_fail,
            calls: Mutex::new(Vec::new()),
        })
    }
}
impl GhRunner for FakeGh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
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
            Some("api") if args.iter().any(|a| a.ends_with("/codespaces/machines")) => {
                if self.machines_fail {
                    Err("HTTP 500".into())
                } else {
                    ok(MACHINES)
                }
            }
            Some("api") => ok(r#"[{"secrets":[]}]"#),
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
    assert_eq!(model::idle_label(45), "45 minutes");
    // A preset is not duplicated.
    assert_eq!(model::size_options(2, "4g").len(), 5);
    assert_eq!(model::runtime_options("2h").len(), 4);
}

#[test]
fn the_labels_read_naturally() {
    assert_eq!(model::size_label(1, "2g"), "1 CPU, 2 GB");
    assert_eq!(model::size_label(16, "32g"), "16 CPUs, 32 GB");
    assert_eq!(model::size_label(2, "512m"), "2 CPUs, 512 MB");
    assert_eq!(model::idle_label(30), "30 minutes");
    assert_eq!(model::idle_label(60), "1 hour");
    assert_eq!(model::idle_label(240), "4 hours");
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
fn the_summary_line_follows_the_backend_shown() {
    assert_eq!(
        model::summary_line(
            DefaultBackend::Sandbox,
            "4 CPUs, 8 GB",
            "work",
            Some("alice"),
            true
        ),
        "New cloud sessions use Docker Sandbox \u{b7} 4 CPUs, 8 GB \u{b7} Claude work \u{b7} Docker alice \u{b7} Claude setup on"
    );
    assert_eq!(
        model::summary_line(
            DefaultBackend::Codespace,
            "4-core, 16 GB",
            "Codespaces secret",
            None,
            false
        ),
        "New cloud sessions use Codespaces \u{b7} 4-core, 16 GB \u{b7} Claude Codespaces secret \u{b7} Claude setup off"
    );
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
    let labels: Vec<_> = model::machine_options(&machines, None, &info)
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
    // No requirement, no mark. A saved machine the list lacks stays selectable.
    let plain = model::machine_options(&machines, Some("legacy"), &DevcontainerInfo::default());
    assert!(plain.iter().all(|o| !o.label.contains("devcontainer")));
    assert_eq!(plain.last().map(|o| o.name.as_str()), Some("legacy"));
    // None selects the machine a codespace starts on.
    assert_eq!(
        model::selected_machine(&machines, None).as_deref(),
        Some("basicLinux32gb")
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
    dialog.update(cx, |d, cx| d.switch_tab(DefaultBackend::Sandbox, cx));
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
        d.switch_tab(DefaultBackend::Codespace, cx);
        d.switch_tab(DefaultBackend::Sandbox, cx);
    });
    cx.run_until_parked();
    assert_eq!(fixture.sbx.calls.lock().unwrap().len(), 1);
}

#[gpui::test]
fn a_docker_account_that_cannot_give_a_token_offers_replace_token(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Sandbox), https_origin);
    let events = events_of(&dialog, cx);
    dialog.update(cx, |d, cx| {
        d.pick(Pick::DockerAccount(Some("acct-2".into())), cx)
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        let list = items(d, DefaultBackend::Sandbox);
        let account = list.iter().find(|i| i.id == Id::DockerAccount).unwrap();
        assert_eq!(account.state, State::Problem);
        assert_eq!(account.fix, Some(Fix::ReplaceToken));
        assert!(!account.text.contains("sbx login"), "{}", account.text);
        assert!(!account.text.contains(JWT));
    });
    dialog.update(cx, |d, cx| {
        d.run_fix(DefaultBackend::Sandbox, Fix::ReplaceToken, cx)
    });
    assert_eq!(events.take(), ["replace-token"]);
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
        assert!(!d.summary(cx).contains(JWT));
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
        assert_eq!(
            d.summary(cx),
            "New cloud sessions use Codespaces \u{b7} 2-core, 8 GB \u{b7} Claude Codespaces secret \u{b7} Claude setup off"
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
fn switching_the_segment_keeps_both_backends_edits_and_save_writes_both(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.set_text("cs_base", "release", cx);
        d.pick(Pick::Idle(120), cx);
        d.switch_tab(DefaultBackend::Sandbox, cx);
        d.set_text("repo", "https://github.com/acme/other", cx);
        d.set_text("sb_base", "dev", cx);
        d.pick(Pick::Size(4, "8g".into()), cx);
        d.pick(Pick::Runtime("4h".into()), cx);
        d.pick(Pick::DockerAccount(Some("acct-1".into())), cx);
        d.pick(Pick::Claude(Some("claude-1".into())), cx);
        d.switch_tab(DefaultBackend::Codespace, cx);
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
        d.switch_tab(DefaultBackend::Sandbox, cx);
    });
    cx.run_until_parked();
    // Closed without saving: nothing was written.
    dialog.update(cx, |d, cx| d.shutdown(cx));
    assert_eq!(fixture.load(), ProjectCloud::default());
    // Saving with the sandbox tab only looked at does not create a docker entry.
    let dialog = fixture.open(cx, Some(DefaultBackend::Codespace), https_origin);
    dialog.update(cx, |d, cx| {
        d.switch_tab(DefaultBackend::Sandbox, cx);
        d.switch_tab(DefaultBackend::Codespace, cx);
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
        d.switch_tab(DefaultBackend::Codespace, cx);
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
        assert_eq!(
            d.summary(cx),
            "New cloud sessions use Codespaces \u{b7} 2-core, 8 GB \u{b7} Claude work@company \u{b7} Claude setup off"
        );
    });
    dialog.update(cx, |d, cx| d.switch_tab(DefaultBackend::Sandbox, cx));
    dialog.read_with(cx, |d, cx| {
        let labels: Vec<_> = d
            .options(Select::Claude, cx)
            .into_iter()
            .map(|o| o.label)
            .collect();
        assert_eq!(labels, ["None", "work@company", "Team API key"]);
        assert!(d.summary(cx).contains("Claude None"));
    });
    dialog.update(cx, |d, cx| {
        d.pick(Pick::DockerAccount(Some("acct-1".into())), cx)
    });
    dialog.read_with(cx, |d, cx| {
        assert_eq!(
            d.summary(cx),
            "New cloud sessions use Docker Sandbox \u{b7} 2 CPUs, 4 GB \u{b7} Claude None \u{b7} Docker alice \u{b7} Claude setup off"
        );
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
    dialog.update(cx, |d, cx| d.switch_tab(DefaultBackend::Codespace, cx));
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
        let mut text = vec![d.summary(cx)];
        text.extend(
            items(d, DefaultBackend::Sandbox)
                .into_iter()
                .map(|i| i.text),
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
                Event::ManageCredentials { .. } => "manage",
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
