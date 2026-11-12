use super::*;
use crate::{
    codespaces::GhOutput,
    git_credentials::{GitOutput, Secret},
    sandboxes::SbxOutput,
};
use std::sync::Mutex;

const AUTH_OK: &str = "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const AUTH_NO_SCOPE: &str =
    "github.com\n  \u{2713} Logged in to github.com account octo (keyring)\n  - Token scopes: 'repo'\n";

struct Gh(Result<&'static str, &'static str>);
impl GhRunner for Gh {
    fn run(&self, _: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        self.0
            .map(|stdout| GhOutput {
                success: true,
                stdout: stdout.into(),
                stderr: String::new(),
            })
            .map_err(str::to_owned)
    }
}

struct Sbx {
    result: Result<SbxOutput, String>,
}
impl SbxRunner for Sbx {
    fn run(&self, _: &[String], _: Option<&str>) -> Result<SbxOutput, String> {
        self.result.clone()
    }
}
fn sbx_ok() -> Sbx {
    Sbx {
        result: Ok(SbxOutput {
            success: true,
            stdout: "[]".into(),
            stderr: String::new(),
        }),
    }
}

#[derive(Default)]
struct Git {
    stderr: Mutex<String>,
    calls: Mutex<Vec<(Vec<String>, String)>>,
}
impl GitRunner for Git {
    fn run(
        &self,
        args: &[String],
        _: &[(String, String)],
        token: &Secret,
    ) -> Result<GitOutput, String> {
        self.calls
            .lock()
            .unwrap()
            .push((args.to_vec(), token.as_str().to_owned()));
        let stderr = self.stderr.lock().unwrap().clone();
        Ok(GitOutput {
            success: stderr.is_empty(),
            stderr,
        })
    }
}

fn home(alias: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".ssh")).unwrap();
    if alias {
        std::fs::write(
            dir.path().join(".ssh/config"),
            "Host sbx_cloud\n  HostName x\n",
        )
        .unwrap();
    }
    dir
}

fn project(devcontainer: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    if let Some(text) = devcontainer {
        std::fs::create_dir_all(dir.path().join(".devcontainer")).unwrap();
        std::fs::write(dir.path().join(".devcontainer/devcontainer.json"), text).unwrap();
    }
    dir
}

const SSHD: &str = r#"{"features":{"ghcr.io/devcontainers/features/sshd:1":{}}}"#;

fn by(items: &[Item], id: Id) -> &Item {
    items.iter().find(|i| i.id == id).unwrap()
}

#[test]
fn repositories_are_compared_across_url_forms() {
    assert!(same_repository(
        "git@github.com:Acme/Alpha.git",
        "https://github.com/acme/alpha"
    ));
    assert!(same_repository(
        "ssh://git@github.com/acme/alpha.git",
        "https://github.com/acme/alpha.git"
    ));
    assert!(!same_repository(
        "https://github.com/acme/alpha",
        "https://gitlab.com/acme/alpha"
    ));
    assert!(!same_repository(
        "https://github.com/acme/alpha",
        "https://github.com/acme/beta"
    ));
    assert!(!same_repository("nonsense", "nonsense"));
}

#[test]
fn a_ready_codespace_project_passes_every_line() {
    let dir = project(Some(SSHD));
    let items = codespace(
        &Gh(Ok(AUTH_OK)),
        dir.path(),
        Some("git@github.com:acme/alpha.git"),
        "",
    );
    assert!(all_ok(&items), "{items:?}");
    assert_eq!(items.len(), 4);
    assert!(by(&items, Id::Gh).text.contains("octo"));
}

#[test]
fn each_codespace_problem_is_named_with_its_fix() {
    let dir = project(Some(r#"{"features":{}}"#));
    let items = codespace(&Gh(Ok(AUTH_NO_SCOPE)), dir.path(), None, "");
    assert_eq!(
        by(&items, Id::Gh).fix,
        Some(Fix::Command(GH_REFRESH_COMMAND))
    );
    assert_eq!(by(&items, Id::Origin).state, State::Problem);
    assert!(by(&items, Id::Origin).text.contains("no git origin"));
    assert_eq!(by(&items, Id::Devcontainer).state, State::Ok);
    assert_eq!(by(&items, Id::Sshd).state, State::Problem);

    let items = codespace(
        &Gh(Err("gh is not installed")),
        dir.path(),
        Some("https://gitlab.com/acme/alpha"),
        "",
    );
    assert_eq!(by(&items, Id::Gh).fix, GH_INSTALL_COMMAND.map(Fix::Command));
    if cfg!(target_os = "macos") {
        assert_eq!(GH_INSTALL_COMMAND, Some("brew install gh"));
    } else {
        // No command the app can name: the line says what to install and offers no Fix.
        assert_eq!(by(&items, Id::Gh).fix, None);
        assert_eq!(by(&items, Id::Gh).text, "Install the GitHub CLI (gh)");
    }
    assert!(by(&items, Id::Origin).text.contains("not on github.com"));

    let none = project(None);
    let items = codespace(
        &Gh(Ok("You are not logged into any GitHub hosts.")),
        none.path(),
        Some("https://github.com/acme/alpha"),
        "",
    );
    assert_eq!(by(&items, Id::Gh).fix, Some(Fix::Command(GH_LOGIN_COMMAND)));
    assert_eq!(by(&items, Id::Devcontainer).state, State::Problem);
    assert_eq!(by(&items, Id::Sshd).state, State::Unknown);
}

fn query<'a>(
    project_root: &'a Path,
    home: &'a Path,
    origin: Option<&'a str>,
    account: Option<&'a str>,
    credential: Option<&'a str>,
) -> SandboxQuery<'a> {
    SandboxQuery {
        project_root,
        origin,
        home: Some(home),
        docker_account: account,
        repo_url: "https://github.com/acme/alpha",
        credential_id: credential,
        devcontainer_path: "",
    }
}

#[test]
fn a_ready_sandbox_project_with_a_public_repository_checks_without_a_token() {
    let (home, dir) = (home(true), project(Some(SSHD)));
    let git = Git::default();
    let items = sandbox(
        &sbx_ok(),
        &git,
        None,
        &query(
            dir.path(),
            home.path(),
            Some("git@github.com:acme/alpha.git"),
            None,
            None,
        ),
    );
    assert!(all_ok(&items), "{items:?}");
    assert!(by(&items, Id::DockerAccount).text.contains("sbx login"));
    let calls = git.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.iter().any(|a| a == "ls-remote"));
    assert_eq!(calls[0].1, "", "no token is sent");
}

#[test]
fn the_devcontainer_is_read_locally_only_when_the_origin_is_the_same_repository() {
    let (home, dir) = (home(true), project(Some(SSHD)));
    let git = Git::default();
    let other = sandbox(
        &sbx_ok(),
        &git,
        None,
        &query(
            dir.path(),
            home.path(),
            Some("https://github.com/acme/other"),
            None,
            None,
        ),
    );
    let item = by(&other, Id::Devcontainer);
    assert_eq!(item.state, State::Unknown);
    assert!(item.text.contains("checked when the sandbox starts"));
    let none = sandbox(
        &sbx_ok(),
        &git,
        None,
        &query(dir.path(), home.path(), None, None, None),
    );
    assert_eq!(by(&none, Id::Devcontainer).state, State::Unknown);
    // Same repository but no file in the checkout.
    let empty = project(None);
    let missing = sandbox(
        &sbx_ok(),
        &git,
        None,
        &query(
            empty.path(),
            home.path(),
            Some("https://github.com/acme/alpha.git"),
            None,
            None,
        ),
    );
    assert_eq!(by(&missing, Id::Devcontainer).state, State::Problem);
}

#[test]
fn a_missing_ssh_alias_offers_the_setup_command_and_nothing_else_fails() {
    let (home, dir) = (home(false), project(None));
    let items = sandbox(
        &sbx_ok(),
        &Git::default(),
        None,
        &query(dir.path(), home.path(), None, None, None),
    );
    assert_eq!(by(&items, Id::DockerAccount).state, State::Ok);
    assert_eq!(by(&items, Id::Ssh).fix, Some(Fix::Command(SBX_SSH_COMMAND)));
}

#[test]
fn a_docker_account_that_cannot_give_a_token_points_at_the_account_picker() {
    let (home, dir) = (home(true), project(None));
    let items = sandbox(
        &sbx_ok(),
        &Git::default(),
        None,
        &query(dir.path(), home.path(), None, Some("no-such-account"), None),
    );
    let account = by(&items, Id::DockerAccount);
    assert_eq!(account.state, State::Problem);
    // No account service here: replacing a token would not help, the picker is the way out.
    assert_eq!(account.fix, None);
    assert!(
        account.text.contains("Docker account list below"),
        "{}",
        account.text
    );
    assert!(!account.text.contains("sbx login"), "{}", account.text);
    // ssh is not judged while the account fails.
    assert_eq!(by(&items, Id::Ssh).state, State::Unknown);
}

#[test]
fn sbx_problems_map_to_the_account_line() {
    let (home, dir) = (home(true), project(None));
    let check = |result: Result<SbxOutput, String>| {
        sandbox(
            &Sbx { result },
            &Git::default(),
            None,
            &query(dir.path(), home.path(), None, None, None),
        )
    };
    let missing = check(Err("sbx is not installed".into()));
    assert!(by(&missing, Id::DockerAccount)
        .text
        .contains("not installed"));
    let logged_out = check(Ok(SbxOutput {
        success: false,
        stdout: String::new(),
        stderr: "not logged in, run sbx login".into(),
    }));
    assert_eq!(
        by(&logged_out, Id::DockerAccount).fix,
        Some(Fix::Command(SBX_LOGIN_COMMAND))
    );
    let down = check(Err("connection refused".into()));
    assert_eq!(by(&down, Id::DockerAccount).state, State::Problem);
}

#[test]
fn an_unreadable_repository_reports_the_git_answer_without_any_token() {
    let (home, dir) = (home(true), project(None));
    let git = Git::default();
    *git.stderr.lock().unwrap() = "fatal: Authentication failed for 'https://github.com/'".into();
    let items = sandbox(
        &sbx_ok(),
        &git,
        None,
        &query(dir.path(), home.path(), None, None, None),
    );
    let repo = by(&items, Id::Repository);
    assert_eq!(repo.state, State::Problem);
    assert!(repo.text.contains("needs a credential"), "{}", repo.text);
    *git.stderr.lock().unwrap() = "fatal: repository 'https://github.com/x/y/' not found".into();
    let items = sandbox(
        &sbx_ok(),
        &git,
        None,
        &query(dir.path(), home.path(), None, None, None),
    );
    assert!(by(&items, Id::Repository).text.contains("not found"));
}

#[test]
fn a_chosen_credential_that_is_not_available_is_a_problem_not_a_public_check() {
    let (home, dir) = (home(true), project(None));
    let git = Git::default();
    let items = sandbox(
        &sbx_ok(),
        &git,
        None,
        &query(dir.path(), home.path(), None, None, Some("c1")),
    );
    assert_eq!(by(&items, Id::Repository).state, State::Problem);
    assert!(git.calls.lock().unwrap().is_empty());
}

#[test]
fn an_empty_repository_url_is_asked_for() {
    let (home, dir) = (home(true), project(None));
    let mut q = query(dir.path(), home.path(), None, None, None);
    q.repo_url = "  ";
    let items = sandbox(&sbx_ok(), &Git::default(), None, &q);
    assert!(by(&items, Id::Repository).text.contains("repository URL"));
}

#[test]
fn only_a_rejected_docker_token_offers_replace_token() {
    use crate::docker_accounts::DockerAuthError as E;
    let docker_id = || "alice".to_owned();
    let fix_of = |error: E| {
        let item = account_item(&Prerequisites::DockerAccount(error.to_string()), Some("a1"));
        assert_eq!(item.state, State::Problem);
        item
    };
    // The token is the problem: replace it.
    for error in [
        E::Revoked {
            docker_id: docker_id(),
        },
        E::MissingScope {
            docker_id: docker_id(),
        },
    ] {
        assert_eq!(fix_of(error).fix, Some(Fix::ReplaceToken));
    }
    // sbx itself rejected the token.
    let rejected = account_item(
        &Prerequisites::DockerAccount(
            "Docker Sandboxes did not accept this Docker account's token. Replace it in Settings > Cloud."
                .into(),
        ),
        Some("a1"),
    );
    assert_eq!(rejected.fix, Some(Fix::ReplaceToken));
    // Hub or network trouble: replacing the token would not help, only Check again.
    for error in [
        E::Network {
            docker_id: docker_id(),
            detail: "timeout".into(),
        },
        E::Status {
            docker_id: docker_id(),
            code: 503,
        },
        E::Unexpected {
            docker_id: docker_id(),
        },
        E::Storage("The database is locked.".into()),
    ] {
        let item = fix_of(error);
        assert_eq!(item.fix, None, "{}", item.text);
        assert!(!item.text.contains("Settings > Cloud"), "{}", item.text);
    }
    // The account is gone or accounts cannot be read: the dialog's own picker is the way out.
    for error in [E::UnknownAccount, E::NotAvailable] {
        let item = fix_of(error);
        assert_eq!(item.fix, None, "{}", item.text);
        assert!(
            item.text.contains("Docker account list below"),
            "{}",
            item.text
        );
    }
}
