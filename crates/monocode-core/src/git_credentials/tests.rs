use super::*;
use std::sync::Mutex;

const TOKEN: &str = "ghp_exampleTokenValue1234567890";

struct Fixture {
    _root: tempfile::TempDir,
    _store: crate::store::Store,
    creds: GitCredentials,
    database: PathBuf,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("monocode.db");
    let store = crate::store::Store::open(&database).unwrap();
    let creds = GitCredentials::for_store(&store).unwrap();
    Fixture {
        _root: root,
        _store: store,
        creds,
        database,
    }
}
fn save_github(f: &Fixture) -> String {
    f.creds
        .save(
            "Work",
            GitPlatform::GitHub,
            "https://github.com",
            "octocat",
            TOKEN,
        )
        .unwrap()
}

/// Arguments, environment, whether the askpass file existed, its path.
type Call = (Vec<String>, Vec<(String, String)>, bool, Option<String>);

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Call>>,
    stderr: String,
    success: bool,
}
impl GitRunner for Fake {
    fn run(
        &self,
        args: &[String],
        env: &[(String, String)],
        token: &Secret,
    ) -> Result<GitOutput, String> {
        assert_eq!(token.as_str(), TOKEN);
        let askpass = env
            .iter()
            .find(|(k, _)| k == "GIT_ASKPASS")
            .map(|(_, v)| PathBuf::from(v));
        let exists = askpass.as_ref().is_some_and(|p| p.exists());
        #[cfg(unix)]
        if let Some(path) = &askpass {
            use std::os::unix::fs::PermissionsExt;
            let dir = path.parent().unwrap();
            assert_eq!(
                std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            let script = std::fs::read_to_string(path).unwrap();
            assert!(!script.contains(TOKEN));
        }
        self.calls.lock().unwrap().push((
            args.to_vec(),
            env.to_vec(),
            exists,
            askpass.map(|p| p.to_string_lossy().into_owned()),
        ));
        Ok(GitOutput {
            success: self.success,
            stderr: self.stderr.clone(),
        })
    }
}

#[test]
fn origin_normalisation_table() {
    let ok = [
        ("https://github.com/o/r", GitPlatform::GitHub, "o/r"),
        ("https://github.com/o/r.git", GitPlatform::GitHub, "o/r"),
        ("https://GitHub.com/o/r/", GitPlatform::GitHub, "o/r"),
        ("https://gitlab.com/g/r", GitPlatform::GitLab, "g/r"),
        (
            "https://gitlab.com/g/sub/r.git",
            GitPlatform::GitLab,
            "g/sub/r",
        ),
    ];
    for (url, platform, path) in ok {
        let (p, origin, repo) = normalize_origin(url).unwrap_or_else(|e| panic!("{url}: {e}"));
        assert_eq!((p, repo.as_str()), (platform, path), "{url}");
        assert_eq!(origin, platform.origin());
    }
    let bad = [
        "",
        "http://github.com/o/r",
        "ssh://git@github.com/o/r.git",
        "git@github.com:o/r.git",
        "/tmp/repo",
        "file:///tmp/repo",
        "https://user:pw@github.com/o/r",
        "https://token@github.com/o/r",
        "https://github.com/o/r?x=1",
        "https://github.com/o/r#frag",
        "https://github.com/o/../r",
        "https://github.com/o/%2e%2e/r",
        "https://github.com/o",
        "https://github.com/o/r/extra",
        "https://github.com:8443/o/r",
        "https://bitbucket.org/o/r",
        "https://github.com.evil.com/o/r",
        "https://github.com/o/ r",
        "https://github.com/o/-r",
    ];
    for url in bad {
        assert!(normalize_origin(url).is_err(), "{url:?} should be rejected");
    }
}

#[test]
fn save_list_remove_secret_round_trip() {
    let f = fixture();
    assert!(f.creds.list().unwrap().is_empty());
    let id = save_github(&f);
    let gitlab = f
        .creds
        .save(
            "Lab",
            GitPlatform::GitLab,
            "https://gitlab.com/",
            "",
            "glpat-abc",
        )
        .unwrap();
    let list = f.creds.list().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].username, "octocat");
    assert_eq!(list[1].username, "oauth2");
    assert_eq!(f.creds.secret(&id).unwrap().as_str(), TOKEN);
    assert_eq!(f.creds.secret(&gitlab).unwrap().as_str(), "glpat-abc");
    f.creds.remove(&id).unwrap();
    assert_eq!(f.creds.list().unwrap().len(), 1);
    assert!(f.creds.secret(&id).is_err());
    f.creds.remove(&id).unwrap();
}

#[test]
fn list_and_database_never_expose_the_secret() {
    let f = fixture();
    save_github(&f);
    let json = serde_json::to_string(&f.creds.list().unwrap()).unwrap();
    assert!(!json.contains(TOKEN));
    for name in ["monocode.db", "monocode.db-wal"] {
        let path = f.database.with_file_name(name);
        if let Ok(bytes) = std::fs::read(path) {
            assert!(!bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()));
        }
    }
}

#[test]
fn save_validates_input() {
    let f = fixture();
    let save = |label: &str, platform, origin: &str, user: &str, token: &str| {
        f.creds.save(label, platform, origin, user, token)
    };
    assert!(save("", GitPlatform::GitHub, "https://github.com", "u", TOKEN).is_err());
    assert!(save("x", GitPlatform::GitHub, "https://github.com", "", TOKEN).is_err());
    assert!(save("x", GitPlatform::GitHub, "https://github.com", "u", "").is_err());
    assert!(save("x", GitPlatform::GitHub, "https://github.com", "u", "a b").is_err());
    assert!(save("x", GitPlatform::GitHub, "https://github.com", "u", "a\nb").is_err());
    assert!(save("x", GitPlatform::GitHub, "https://gitlab.com", "u", TOKEN).is_err());
    assert!(save(
        "x",
        GitPlatform::GitHub,
        "https://github.com",
        "u\nv",
        TOKEN
    )
    .is_err());
    assert!(f.creds.list().unwrap().is_empty());
}

#[test]
fn a_row_moved_to_another_identity_fails_to_decrypt() {
    let f = fixture();
    let id = save_github(&f);
    let conn = Connection::open(&f.database).unwrap();
    conn.execute(
        "UPDATE git_credentials SET username='mallory' WHERE id=?1",
        [&id],
    )
    .unwrap();
    assert!(f.creds.secret(&id).is_err());
}

// The askpass helper is unix-only (see `check_with`).
#[cfg(unix)]
#[test]
fn check_access_keeps_the_token_out_of_argv_and_cleans_up() {
    let f = fixture();
    let id = save_github(&f);
    let runner = Fake {
        success: true,
        ..Default::default()
    };
    let check = f
        .creds
        .check_access(&runner, &id, "https://github.com/o/r.git")
        .unwrap();
    assert_eq!(check.status, AccessStatus::ReadOk);
    assert!(check.message.contains("Write access is not verified"));
    let calls = runner.calls.lock().unwrap();
    let (args, env, existed, askpass) = &calls[0];
    assert!(args.iter().all(|a| !a.contains(TOKEN)));
    assert!(args.iter().any(|a| a == "ls-remote"));
    assert!(args.contains(&"https://github.com/o/r.git".to_owned()));
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(get("GIT_TERMINAL_PROMPT"), Some("0"));
    assert_eq!(
        get(TOKEN_ENV),
        None,
        "the token travels as a Secret, not in the env list"
    );
    assert_eq!(get("GIT_CONFIG_GLOBAL"), Some("/dev/null"));
    assert_eq!(get("GIT_CONFIG_NOSYSTEM"), Some("1"));
    assert_eq!(get("GIT_ALLOW_PROTOCOL"), Some("https"));
    assert!(args
        .windows(2)
        .any(|w| w == ["-c", "http.followRedirects=false"]));
    assert!(args.windows(2).any(|w| w == ["-c", "credential.helper="]));
    assert_eq!(get(USERNAME_ENV), Some("octocat"));
    assert!(*existed);
    assert!(!PathBuf::from(askpass.as_ref().unwrap()).exists());
    assert!(!PathBuf::from(askpass.as_ref().unwrap())
        .parent()
        .unwrap()
        .exists());
}

#[cfg(unix)]
#[test]
fn the_askpass_helper_answers_only_the_exact_origin() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_askpass(dir.path(), "https://github.com", "octocat").unwrap();
    let run = |prompt: &str| {
        let out = std::process::Command::new(&script)
            .arg(prompt)
            .env(USERNAME_ENV, "octocat")
            .env(TOKEN_ENV, TOKEN)
            .output()
            .unwrap();
        (out.status.success(), String::from_utf8(out.stdout).unwrap())
    };
    assert_eq!(
        run("Username for 'https://github.com': "),
        (true, "octocat\n".into())
    );
    assert_eq!(
        run("Password for 'https://github.com': "),
        (true, format!("{TOKEN}\n"))
    );
    assert_eq!(
        run("Password for 'https://octocat@github.com': "),
        (true, format!("{TOKEN}\n"))
    );
    for prompt in [
        "Password for 'https://evil.com': ",
        "Password for 'https://octocat@evil.com': ",
        "Password for 'https://mallory@github.com': ",
        "Password for 'https://github.com.evil.com': ",
        "Username for 'https://evil.com': ",
        "Password for 'ssh://git@github.com': ",
        "Enter passphrase for key '/home/x/.ssh/id_rsa': ",
        "",
    ] {
        assert_eq!(run(prompt), (false, String::new()), "{prompt:?}");
    }
}

#[test]
fn secrets_do_not_print() {
    let secret = Secret::new(TOKEN.into());
    assert!(!format!("{secret:?}").contains(TOKEN));
}

// The askpass helper is unix-only (see `check_with`).
#[cfg(unix)]
#[test]
fn git_gets_the_normalised_url_not_the_raw_input() {
    let f = fixture();
    let id = save_github(&f);
    let runner = Fake {
        success: true,
        ..Default::default()
    };
    f.creds
        .check_access(&runner, &id, "  https://GitHub.com/o/r/  ")
        .unwrap();
    f.creds
        .check_access(&runner, &id, "https://github.com/o/r.git")
        .unwrap();
    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls[0].0.last().unwrap(), "https://github.com/o/r");
    assert_eq!(calls[1].0.last().unwrap(), "https://github.com/o/r.git");
    assert_eq!(
        clone_url("https://gitlab.com/g/s/r.git").unwrap(),
        "https://gitlab.com/g/s/r.git"
    );
    for bad in [
        "HTTP://github.com/o/r",
        "https://github.com/o/./r",
        "https://github.com/o/x/../r",
        "https://github.com/o/r/..",
        " ftp://github.com/o/r",
    ] {
        assert!(normalize_origin(bad).is_err(), "{bad}");
    }
}

// The askpass helper is unix-only (see `check_with`).
#[cfg(unix)]
#[test]
fn check_access_maps_failures_and_redacts() {
    let f = fixture();
    let id = save_github(&f);
    let case = |stderr: &str| {
        let runner = Fake {
            stderr: stderr.into(),
            ..Default::default()
        };
        f.creds
            .check_access(&runner, &id, "https://github.com/o/r")
            .unwrap()
    };
    assert_eq!(
        case("remote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/o/r/'").status,
        AccessStatus::AuthFailed
    );
    assert_eq!(
        case("fatal: could not read Username for 'https://github.com': terminal prompts disabled")
            .status,
        AccessStatus::AuthFailed
    );
    assert_eq!(
        case(
            "remote: Repository not found.\nfatal: repository 'https://github.com/o/r/' not found"
        )
        .status,
        AccessStatus::NotFound
    );
    let network = case(&format!(
        "fatal: unable to access: Could not resolve host: github.com {TOKEN}"
    ));
    assert_eq!(network.status, AccessStatus::Network);
    assert!(!network.message.contains(TOKEN));
}

#[test]
fn check_access_refuses_a_credential_for_another_host_or_id() {
    let f = fixture();
    let id = save_github(&f);
    let runner = Fake::default();
    assert!(f
        .creds
        .check_access(&runner, &id, "https://gitlab.com/g/r")
        .is_err());
    assert!(f
        .creds
        .check_access(&runner, "missing", "https://github.com/o/r")
        .is_err());
    assert!(runner.calls.lock().unwrap().is_empty());
}

#[test]
fn payload_format() {
    let f = fixture();
    let id = save_github(&f);
    let payload = f
        .creds
        .credential_payload(&id, "https://github.com/o/r.git")
        .unwrap();
    assert_eq!(
        payload.as_str(),
        format!(
            "protocol=https\nhost=github.com\npath=o/r.git\nusername=octocat\npassword={TOKEN}\n\n"
        )
    );
    let plain = f
        .creds
        .credential_payload(&id, "https://github.com/o/r")
        .unwrap();
    assert!(plain.contains("path=o/r\nusername"));
    assert!(f
        .creds
        .credential_payload(&id, "https://gitlab.com/g/r")
        .is_err());
}

#[test]
fn redact_replaces_every_occurrence() {
    assert_eq!(
        redact("a SECRET b SECRET", "SECRET"),
        "a [redacted] b [redacted]"
    );
    assert_eq!(redact("keep", ""), "keep");
}

#[test]
fn usernames_must_be_shell_safe() {
    let f = fixture();
    for user in ["u'v", "$(id)", "a b", "u\nv", "`id`", "a\"b"] {
        assert!(f
            .creds
            .save("x", GitPlatform::GitHub, "https://github.com", user, TOKEN)
            .is_err());
    }
}

#[test]
fn saving_without_a_database_creates_no_key_file() {
    let root = tempfile::tempdir().unwrap();
    let creds = GitCredentials::at(root.path().join("monocode.db"));
    assert!(creds
        .save("x", GitPlatform::GitHub, "https://github.com", "u", TOKEN)
        .is_err());
    assert!(!root.path().join("credentials.key").exists());
    assert!(!root.path().join("monocode.db").exists());
}

// The askpass helper is unix-only (see `check_with`).
#[cfg(unix)]
#[test]
fn check_token_checks_an_unsaved_token_without_touching_the_database() {
    let root = tempfile::tempdir().unwrap();
    let runner = Fake {
        success: true,
        ..Default::default()
    };
    let check = check_token(
        &runner,
        GitPlatform::GitHub,
        "octocat",
        Secret::new(TOKEN.into()),
        "https://github.com/o/r.git",
    )
    .unwrap();
    assert_eq!(check.status, AccessStatus::ReadOk);
    let calls = runner.calls.lock().unwrap();
    let (args, env, existed, askpass) = &calls[0];
    assert!(args.iter().all(|a| !a.contains(TOKEN)));
    assert!(env.iter().all(|(_, v)| !v.contains(TOKEN)));
    assert!(args.iter().any(|a| a == "ls-remote"));
    assert!(*existed);
    assert!(!PathBuf::from(askpass.as_ref().unwrap()).exists());
    // Nothing was created next to where a database would live.
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

// The askpass helper is unix-only (see `check_with`).
#[cfg(unix)]
#[test]
fn check_token_validates_like_save_and_rejects_a_host_mismatch() {
    let runner = Fake {
        success: true,
        ..Default::default()
    };
    let run = |platform, username: &str, token: &str, url: &str| {
        check_token(&runner, platform, username, Secret::new(token.into()), url)
    };
    let url = "https://github.com/o/r";
    assert!(
        run(GitPlatform::GitHub, "", TOKEN, url).is_err(),
        "GitHub needs a username"
    );
    assert!(run(GitPlatform::GitHub, "me", "", url).is_err());
    assert!(run(GitPlatform::GitHub, "me", "two words", url).is_err());
    assert!(run(GitPlatform::GitHub, "me", TOKEN, "https://gitlab.com/g/r").is_err());
    assert!(run(GitPlatform::GitHub, "me", TOKEN, "git@github.com:o/r.git").is_err());
    assert!(runner.calls.lock().unwrap().is_empty(), "nothing ran");
    assert!(run(GitPlatform::GitLab, "", TOKEN, "https://gitlab.com/g/r").is_ok());
}
