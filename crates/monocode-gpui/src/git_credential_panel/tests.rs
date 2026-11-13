use super::*;
use gpui::TestAppContext;
use monocode_core::{
    git_credentials::{AccessCheck, AccessStatus, GitOutput, Secret},
    store::Store,
};
use std::{path::PathBuf, sync::Mutex};

const TOKEN: &str = "ghp_exampleTokenValue1234567890";
const REPO: &str = "https://github.com/acme/alpha";

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Records each call and answers with a fixed stderr.
struct FakeGit {
    calls: Mutex<Vec<Vec<String>>>,
    success: bool,
    stderr: String,
}
impl FakeGit {
    fn new(success: bool, stderr: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            success,
            stderr: stderr.into(),
        })
    }
}
impl GitRunner for FakeGit {
    fn run(
        &self,
        args: &[String],
        _: &[(String, String)],
        token: &Secret,
    ) -> Result<GitOutput, String> {
        assert_eq!(token.as_str(), TOKEN);
        self.calls.lock().unwrap().push(args.to_vec());
        Ok(GitOutput {
            success: self.success,
            stderr: self.stderr.clone(),
        })
    }
}

fn store() -> (Scratch, Database, GitCredentials) {
    let dir = std::env::temp_dir().join(format!(
        "monoptah-git-panel-{}",
        monocode_core::mono::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir.join("monocode.db")).unwrap();
    let credentials = GitCredentials::for_store(&store).unwrap();
    (Scratch(dir), Arc::new(Mutex::new(store)), credentials)
}

#[test]
fn check_results_map_to_lines_and_never_echo_the_token() {
    for status in [
        AccessStatus::AuthFailed,
        AccessStatus::NotFound,
        AccessStatus::Network,
    ] {
        let line = model::check_line(
            Ok(AccessCheck {
                status,
                message: format!("failed with {TOKEN}"),
            }),
            TOKEN,
        );
        assert!(!line.ok);
        assert!(!line.text.contains(TOKEN), "{}", line.text);
    }
    let ok = model::check_line(
        Ok(AccessCheck {
            status: AccessStatus::ReadOk,
            message: "Read access works. Write access is not checked.".into(),
        }),
        TOKEN,
    );
    assert!(ok.ok && ok.text.contains("Write access is not checked"));
    let local = model::check_line(Err(format!("boom {TOKEN}")), TOKEN);
    assert!(!local.ok && !local.text.contains(TOKEN));
}

#[test]
fn repo_problem_names_a_host_mismatch_or_a_bad_url() {
    assert!(model::repo_problem(REPO, GitPlatform::GitHub).is_none());
    assert!(model::repo_problem(REPO, GitPlatform::GitLab).is_some());
    assert!(model::repo_problem("git@github.com:a/b.git", GitPlatform::GitHub).is_some());
}

#[test]
fn checking_an_unsaved_token_stores_nothing() {
    let (dir, _db, credentials) = store();
    let before = std::fs::read(dir.0.join("monocode.db")).unwrap();
    let git = FakeGit::new(true, "");
    let line = model::check_unsaved(
        git.as_ref(),
        GitPlatform::GitHub,
        "me",
        Secret::new(TOKEN.into()),
        REPO,
    );
    assert!(line.ok, "{line:?}");
    assert_eq!(git.calls.lock().unwrap().len(), 1);
    assert!(credentials.list().unwrap().is_empty());
    assert_eq!(std::fs::read(dir.0.join("monocode.db")).unwrap(), before);

    let denied = FakeGit::new(false, "fatal: Authentication failed");
    let line = model::check_unsaved(
        denied.as_ref(),
        GitPlatform::GitHub,
        "me",
        Secret::new(TOKEN.into()),
        REPO,
    );
    assert!(!line.ok);
    assert!(credentials.list().unwrap().is_empty());
}

#[test]
fn a_rejected_save_does_not_echo_the_token() {
    let (_dir, _db, credentials) = store();
    let error =
        model::save_credential(&credentials, "", GitPlatform::GitHub, "me", TOKEN).unwrap_err();
    assert!(!error.contains(TOKEN));
    let error =
        model::save_credential(&credentials, "Work", GitPlatform::GitHub, "", TOKEN).unwrap_err();
    assert!(error.contains("username"), "{error}");
    // GitLab needs no username.
    assert!(
        model::save_credential(&credentials, "Lab", GitPlatform::GitLab, "", "glpat-abc123")
            .is_ok()
    );
    assert_eq!(credentials.list().unwrap()[0].username, "oauth2");
}

fn open(cx: &mut TestAppContext, db: &Database, git: Arc<FakeGit>) -> Entity<GitCredentialPanel> {
    let env = Env {
        store: db.clone(),
        git,
    };
    cx.new(|cx| GitCredentialPanel::new("alpha".into(), REPO.into(), None, env, cx))
}

fn type_into(
    dialog: &Entity<GitCredentialPanel>,
    cx: &mut TestAppContext,
    label: &str,
    username: &str,
    token: &str,
) {
    dialog.update(cx, |d, cx| {
        let (l, u, t) = (d.label.clone(), d.username.clone(), d.token.clone());
        l.update(cx, |i, cx| i.set_text(label.into(), cx));
        u.update(cx, |i, cx| i.set_text(username.into(), cx));
        t.update(cx, |i, cx| i.set_text(token.into(), cx));
    });
}

fn token_text(dialog: &Entity<GitCredentialPanel>, cx: &mut TestAppContext) -> String {
    dialog.read_with(cx, |d, cx| d.token.read(cx).content.to_string())
}

#[gpui::test]
fn saving_stores_the_profile_empties_the_token_and_reports_it(cx: &mut TestAppContext) {
    let (_dir, db, credentials) = store();
    let dialog = open(cx, &db, FakeGit::new(true, ""));
    let chosen = Arc::new(Mutex::new(Vec::new()));
    let seen = chosen.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(&dialog, move |_, event: &Event, _| {
            if let Event::Chosen(id) = event {
                seen.lock().unwrap().push(id.clone());
            }
        })
    });
    type_into(&dialog, cx, "Work", "me", TOKEN);
    assert_eq!(token_text(&dialog, cx), TOKEN);
    dialog.update(cx, |d, cx| d.save(cx));
    cx.run_until_parked();
    let list = credentials.list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(chosen.lock().unwrap().as_slice(), [list[0].id.clone()]);
    assert_eq!(token_text(&dialog, cx), "", "the token is not kept");
    dialog.read_with(cx, |d, cx| {
        for input in [&d.label, &d.username, &d.token] {
            assert!(!input.read(cx).content.contains(TOKEN));
        }
        assert!(d.error.is_none());
    });
}

#[gpui::test]
fn a_failed_save_keeps_the_form_and_shows_a_clean_error(cx: &mut TestAppContext) {
    let (_dir, db, credentials) = store();
    let dialog = open(cx, &db, FakeGit::new(true, ""));
    type_into(&dialog, cx, "", "me", TOKEN);
    dialog.update(cx, |d, cx| d.save(cx));
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        let error = d.error.clone().unwrap();
        assert!(!error.contains(TOKEN));
    });
    assert!(credentials.list().unwrap().is_empty());

    type_into(&dialog, cx, "Work", "me", "");
    dialog.update(cx, |d, cx| d.save(cx));
    dialog.read_with(cx, |d, _| {
        assert_eq!(d.error.as_deref(), Some("Paste the access token."));
    });
}

#[gpui::test]
fn check_access_runs_in_the_background_and_shows_the_result(cx: &mut TestAppContext) {
    let (_dir, db, credentials) = store();
    let git = FakeGit::new(true, "");
    let dialog = open(cx, &db, git.clone());
    type_into(&dialog, cx, "Work", "me", TOKEN);
    dialog.update(cx, |d, cx| d.check_access(cx));
    dialog.read_with(cx, |d, _| {
        assert!(d.checking, "the check is pending, not run inline")
    });
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert!(!d.checking);
        let line = d.check.clone().unwrap();
        assert!(line.ok);
        assert!(!line.text.contains(TOKEN));
    });
    assert_eq!(git.calls.lock().unwrap().len(), 1);
    assert!(
        credentials.list().unwrap().is_empty(),
        "checking does not save"
    );
    assert_eq!(token_text(&dialog, cx), TOKEN);
}

#[gpui::test]
fn removing_a_profile_asks_to_confirm_first(cx: &mut TestAppContext) {
    let (_dir, db, credentials) = store();
    let id = credentials
        .save(
            "Work",
            GitPlatform::GitHub,
            "https://github.com",
            "me",
            TOKEN,
        )
        .unwrap();
    let env = Env {
        store: db.clone(),
        git: FakeGit::new(true, ""),
    };
    let dialog =
        cx.new(|cx| GitCredentialPanel::new("alpha".into(), REPO.into(), Some(id), env, cx));
    cx.run_until_parked();
    dialog.update(cx, |d, cx| d.remove(cx));
    cx.run_until_parked();
    assert_eq!(
        credentials.list().unwrap().len(),
        1,
        "first press only asks"
    );
    dialog.update(cx, |d, cx| d.remove(cx));
    cx.run_until_parked();
    assert!(credentials.list().unwrap().is_empty());
    dialog.read_with(cx, |d, _| {
        assert!(d.selected.is_none() && d.profiles.is_empty())
    });
}

#[gpui::test]
fn a_check_result_for_a_selection_that_changed_is_dropped(cx: &mut TestAppContext) {
    let (_dir, db, _credentials) = store();
    let dialog = open(cx, &db, FakeGit::new(true, ""));
    type_into(&dialog, cx, "Work", "me", TOKEN);
    dialog.update(cx, |d, cx| d.check_access(cx));
    // The platform changes while the check runs.
    dialog.update(cx, |d, cx| d.set_platform(GitPlatform::GitLab, cx));
    cx.run_until_parked();
    dialog.read_with(cx, |d, _| {
        assert!(d.check.is_none(), "{:?}", d.check);
        assert!(!d.checking);
    });
}
