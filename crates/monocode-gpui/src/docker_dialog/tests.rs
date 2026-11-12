use super::*;
use gpui::TestAppContext;
use monocode_core::{
    git_credentials::{GitOutput, GitPlatform, Secret},
    sandboxes::{Prerequisites, SbxOutput},
    store::Store,
};
use std::sync::Mutex;

#[derive(Default)]
struct FakeSbx {
    calls: Mutex<Vec<Vec<String>>>,
    logged_out: bool,
}
impl SbxRunner for FakeSbx {
    fn run(
        &self,
        args: &[String],
        _: Option<&str>,
    ) -> Result<monocode_core::sandboxes::SbxOutput, String> {
        self.calls.lock().unwrap().push(args.to_vec());
        Ok(SbxOutput {
            success: !self.logged_out,
            stdout: "[]".into(),
            stderr: if self.logged_out {
                "not logged in, run sbx login".into()
            } else {
                String::new()
            },
        })
    }
}

#[derive(Default)]
struct FakeGit {
    calls: Mutex<usize>,
}
impl GitRunner for FakeGit {
    fn run(&self, _: &[String], _: &[(String, String)], _: &Secret) -> Result<GitOutput, String> {
        *self.calls.lock().unwrap() += 1;
        Ok(GitOutput {
            success: true,
            stderr: String::new(),
        })
    }
}

/// A scratch directory removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "monoptah-docker-dialog-{}",
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

struct Fixture {
    _dir: Scratch,
    home: Scratch,
    store: Database,
    sbx: Arc<FakeSbx>,
    git: Arc<FakeGit>,
}

fn fixture(logged_out: bool) -> Fixture {
    let dir = Scratch::new();
    let home = Scratch::new();
    std::fs::create_dir_all(home.0.join(".ssh")).unwrap();
    std::fs::write(home.0.join(".ssh/config"), "Host sbx_cloud\n  HostName x\n").unwrap();
    let store = Store::open(&dir.0.join("monocode.db")).unwrap();
    Fixture {
        _dir: dir,
        home,
        store: Arc::new(Mutex::new(store)),
        sbx: Arc::new(FakeSbx {
            logged_out,
            ..Default::default()
        }),
        git: Arc::new(FakeGit::default()),
    }
}

impl Fixture {
    fn env(&self, origin: fn(&Path) -> Option<String>) -> Env {
        Env {
            sbx: self.sbx.clone(),
            git: self.git.clone(),
            store: self.store.clone(),
            home: Some(self.home.0.to_path_buf()),
            origin,
            accounts: vec![AccountChoice {
                id: "acct-1".into(),
                label: "Work".into(),
                kind: "setup-token",
            }],
        }
    }
}

fn https_origin(_: &Path) -> Option<String> {
    Some("https://github.com/acme/alpha.git".into())
}
fn ssh_origin(_: &Path) -> Option<String> {
    Some("git@github.com:acme/alpha.git".into())
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

fn form(repo: &str) -> Form {
    Form::initial(None, Some(repo))
}

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
fn defaults_match_the_agreed_config() {
    let form = form("https://github.com/acme/alpha");
    let config = form.to_config(None);
    assert_eq!(config.devcontainer_path, ".devcontainer/devcontainer.json");
    assert_eq!((config.cpus, config.memory.as_str()), (2, "4g"));
    assert_eq!(config.ttl, "2h");
    assert_eq!(config.credential_id, None);
}

#[test]
fn a_saved_config_comes_back_into_the_form() {
    let saved = DockerProjectConfig {
        repo_url: "https://gitlab.com/g/r".into(),
        credential_id: Some("c1".into()),
        base_ref: "dev".into(),
        cpus: 4,
        memory: "8g".into(),
        ttl: "4h".into(),
        claude_account: Some("acct-1".into()),
        ..Default::default()
    };
    let form = Form::initial(Some(&saved), Some("https://github.com/other/other"));
    assert_eq!(form.repo_url, "https://gitlab.com/g/r");
    assert_eq!((form.compute, form.runtime), (1, 2));
    assert_eq!(form.to_config(Some(&saved)), saved);
}

#[test]
fn the_dialog_keeps_the_saved_docker_account() {
    let saved = DockerProjectConfig {
        repo_url: "https://github.com/acme/alpha".into(),
        docker_account: Some("docker-acct-1".into()),
        ..Default::default()
    };
    let form = Form::initial(Some(&saved), None);
    assert_eq!(
        form.to_config(Some(&saved)).docker_account.as_deref(),
        Some("docker-acct-1")
    );
    assert_eq!(form.to_config(None).docker_account, None);
}

#[test]
fn validation_errors_land_under_the_right_field() {
    let accounts = [];
    let check = |edit: fn(&mut Form), creds: &[GitCredentialSummary]| {
        let mut form = form("https://github.com/acme/alpha");
        edit(&mut form);
        model::validate(&form, creds, &accounts, None)
            .unwrap_err()
            .field
    };
    assert_eq!(
        check(|f| f.repo_url = "git@github.com:a/b.git".into(), &[]),
        Field::Repo
    );
    assert_eq!(check(|f| f.repo_url = String::new(), &[]), Field::Repo);
    assert_eq!(
        check(
            |f| f.repo_url = "https://tok@github.com/acme/alpha".into(),
            &[]
        ),
        Field::Repo
    );
    assert_eq!(
        check(|f| f.devcontainer = "/etc/passwd".into(), &[]),
        Field::Devcontainer
    );
    assert_eq!(check(|f| f.base_ref = "a..b".into(), &[]), Field::Base);
    assert_eq!(
        check(|f| f.credential_id = Some("gone".into()), &[]),
        Field::Credential
    );
    assert_eq!(
        check(
            |f| f.credential_id = Some("c1".into()),
            &[summary("c1", GitPlatform::GitLab)]
        ),
        Field::Credential
    );
    let ok = {
        let mut form = form("https://github.com/acme/alpha");
        form.credential_id = Some("c1".into());
        model::validate(
            &form,
            &[summary("c1", GitPlatform::GitHub)],
            &accounts,
            None,
        )
    };
    assert!(ok.is_ok(), "{ok:?}");
    let mut with_account = form("https://github.com/acme/alpha");
    with_account.account = Some("missing".into());
    assert!(model::validate(&with_account, &[], &accounts, None).is_err());
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
fn access_shows_the_exact_command_to_run() {
    let fixture = fixture(true);
    let out = model::check_access(fixture.sbx.as_ref(), Some(&fixture.home.0), None);
    assert!(matches!(out, Prerequisites::NotLoggedIn(_)));
    let access = Access::Done(out);
    assert_eq!(access.fix(), Some("sbx login"));
    assert!(!access.is_ready());
    assert_eq!(
        Access::Done(Prerequisites::SshAliasMissing).fix(),
        Some("sbx --cloud setup ssh")
    );
    let ready = fixture_ready();
    assert!(ready.is_ready() && ready.fix().is_none());
    assert!(ready.line().starts_with("Ready"));
}

fn fixture_ready() -> Access {
    let fixture = fixture(false);
    Access::Done(model::check_access(
        fixture.sbx.as_ref(),
        Some(&fixture.home.0),
        None,
    ))
}

#[gpui::test]
fn the_dialog_checks_access_off_the_ui_thread_and_prefills_from_https(cx: &mut TestAppContext) {
    let fixture = fixture(false);
    let dialog =
        cx.new(|cx| DockerDialog::new("/p".into(), "alpha".into(), fixture.env(https_origin), cx));
    cx.run_until_parked();
    dialog.read_with(cx, |dialog, cx| {
        assert!(dialog.access.is_ready());
        assert_eq!(
            dialog.field_text(Field::Repo, cx),
            "https://github.com/acme/alpha.git"
        );
    });
    let calls = fixture.sbx.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].iter().any(|a| a == "ls"), "{:?}", calls[0]);
    assert!(
        calls[0]
            .iter()
            .all(|a| !["create", "run", "exec", "login", "ssh"].contains(&a.as_str()))
    );
}

#[gpui::test]
fn an_ssh_origin_is_not_prefilled(cx: &mut TestAppContext) {
    let fixture = fixture(false);
    let dialog =
        cx.new(|cx| DockerDialog::new("/p".into(), "alpha".into(), fixture.env(ssh_origin), cx));
    cx.run_until_parked();
    dialog.read_with(cx, |dialog, cx| {
        assert_eq!(dialog.field_text(Field::Repo, cx), "")
    });
}

#[gpui::test]
fn save_validates_and_writes_the_config_without_any_cloud_call(cx: &mut TestAppContext) {
    let fixture = fixture(false);
    let dialog =
        cx.new(|cx| DockerDialog::new("/p".into(), "alpha".into(), fixture.env(https_origin), cx));
    cx.run_until_parked();
    let sbx_before = fixture.sbx.calls.lock().unwrap().len();

    // A bad URL is a field error and stores nothing.
    dialog.update(cx, |dialog, cx| {
        dialog.set_fields("git@github.com:acme/alpha.git", cx);
        dialog.save(cx);
        assert_eq!(dialog.error.as_ref().map(|e| e.field), Some(Field::Repo));
    });
    assert!(
        docker_config::load(&fixture.store.lock().unwrap(), "/p")
            .unwrap()
            .is_none()
    );

    dialog.update(cx, |dialog, cx| {
        dialog.set_fields("https://github.com/acme/alpha", cx);
        dialog.form.account = Some("acct-1".into());
        dialog.form.compute = 1;
        dialog.save(cx);
        assert!(dialog.error.is_none());
    });
    let saved = docker_config::load(&fixture.store.lock().unwrap(), "/p")
        .unwrap()
        .unwrap();
    assert_eq!(saved.repo_url, "https://github.com/acme/alpha");
    assert_eq!((saved.cpus, saved.memory.as_str()), (4, "8g"));
    assert_eq!(saved.claude_account.as_deref(), Some("acct-1"));
    cx.run_until_parked();
    assert_eq!(fixture.sbx.calls.lock().unwrap().len(), sbx_before);
    assert_eq!(*fixture.git.calls.lock().unwrap(), 0);
}

static ORIGIN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn counted_origin(_: &Path) -> Option<String> {
    ORIGIN_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Some("https://github.com/acme/counted".into())
}

#[gpui::test]
fn the_origin_is_read_after_construction_and_never_over_typed_input(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = fixture(false);
    let dialog = cx
        .new(|cx| DockerDialog::new("/p".into(), "alpha".into(), fixture.env(counted_origin), cx));
    assert_eq!(ORIGIN_CALLS.load(SeqCst), 0, "not read in the constructor");
    dialog.update(cx, |d, cx| d.set_fields("https://github.com/me/typed", cx));
    cx.run_until_parked();
    assert_eq!(ORIGIN_CALLS.load(SeqCst), 1);
    dialog.read_with(cx, |d, cx| {
        assert_eq!(d.field_text(Field::Repo, cx), "https://github.com/me/typed");
    });
}

static SAVED_ORIGIN_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn saved_origin(_: &Path) -> Option<String> {
    SAVED_ORIGIN_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Some("https://github.com/acme/counted".into())
}

#[gpui::test]
fn a_saved_config_is_not_replaced_by_the_origin(cx: &mut TestAppContext) {
    use std::sync::atomic::Ordering::SeqCst;
    let fixture = fixture(false);
    let saved = DockerProjectConfig {
        repo_url: "https://gitlab.com/g/r".into(),
        ..Default::default()
    };
    docker_config::save(&fixture.store.lock().unwrap(), "/p", &saved).unwrap();
    let before = SAVED_ORIGIN_CALLS.load(SeqCst);
    let dialog =
        cx.new(|cx| DockerDialog::new("/p".into(), "alpha".into(), fixture.env(saved_origin), cx));
    cx.run_until_parked();
    assert_eq!(
        SAVED_ORIGIN_CALLS.load(SeqCst),
        before,
        "no need to read it"
    );
    dialog.read_with(cx, |d, cx| {
        assert_eq!(d.field_text(Field::Repo, cx), "https://gitlab.com/g/r");
    });
}

#[gpui::test]
fn the_access_check_asks_as_the_saved_docker_account_and_never_advises_sbx_login(
    cx: &mut TestAppContext,
) {
    let fixture = fixture(true);
    let saved = DockerProjectConfig {
        repo_url: "https://github.com/acme/alpha.git".into(),
        docker_account: Some("acct-1".into()),
        ..Default::default()
    };
    docker_config::save(&fixture.store.lock().unwrap(), "/p", &saved).unwrap();
    let dialog =
        cx.new(|cx| DockerDialog::new("/p".into(), "alpha".into(), fixture.env(https_origin), cx));
    cx.run_until_parked();
    dialog.read_with(cx, |dialog, _| {
        // No Docker accounts are installed in this test, so the account cannot give a token:
        // that is the answer, not the machine login's "run sbx login".
        assert!(!dialog.access.is_ready());
        assert_eq!(dialog.access.fix(), None);
        assert!(
            dialog.access.line().contains("not available"),
            "{}",
            dialog.access.line()
        );
        assert!(!dialog.access.line().contains("sbx login"));
    });
    assert!(
        fixture.sbx.calls.lock().unwrap().is_empty(),
        "sbx was not run as the machine login instead"
    );
}

#[gpui::test]
fn changing_the_repository_host_drops_a_credential_for_the_old_one(cx: &mut TestAppContext) {
    let fixture = fixture(false);
    let dialog =
        cx.new(|cx| DockerDialog::new("/p".into(), "alpha".into(), fixture.env(https_origin), cx));
    cx.run_until_parked();
    dialog.update(cx, |d, cx| {
        d.credentials = vec![summary("c1", GitPlatform::GitHub)];
        d.form.credential_id = Some("c1".into());
        d.set_fields("https://github.com/acme/other", cx);
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.form.credential_id.as_deref(), Some("c1"))
    });
    dialog.update(cx, |d, cx| d.set_fields("https://gitlab.com/g/r", cx));
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| assert_eq!(d.form.credential_id, None));
}
