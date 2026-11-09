use super::*;
use std::{collections::HashMap, ffi::OsStr, process::Command};

struct Fixture {
    root: tempfile::TempDir,
    _store: crate::store::Store,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = crate::store::Store::open(&root.path().join("monocode.db")).unwrap();
        Self {
            root,
            _store: store,
        }
    }
    fn profile(&self, id: &str) -> Profile {
        Profile::new(id, self.root.path())
    }
    fn db(&self) -> Connection {
        Connection::open(self.root.path().join("monocode.db")).unwrap()
    }
    fn key(&self) -> PathBuf {
        self.root.path().join(KEY_FILE)
    }
    fn rows(&self) -> i64 {
        self.db()
            .query_row(
                "SELECT count(*) FROM provider_account_credentials",
                [],
                |r| r.get(0),
            )
            .unwrap()
    }
}

const TOKEN: &str = "sk-ant-oat01-fixture-first";

#[test]
fn round_trip_is_encrypted_and_account_scoped() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    let personal = fx.profile("personal");
    account.save_claude_token(TOKEN).unwrap();
    assert!(fx.profile("work").uses_claude_token().unwrap());
    assert!(!personal.uses_claude_token().unwrap());
    assert_eq!(account.claude_token().unwrap().as_deref(), Some(TOKEN));
    assert_eq!(personal.claude_token().unwrap(), None);
    let (kind, nonce, ciphertext): (String, Vec<u8>, Vec<u8>) = fx
        .db()
        .query_row(
            "SELECT kind, nonce, ciphertext FROM provider_account_credentials",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "setup-token");
    assert_eq!(nonce.len(), NONCE_LEN);
    assert!(!String::from_utf8_lossy(&ciphertext).contains(TOKEN));
    assert!(!format!("{account:?}").contains(TOKEN));
    assert!(!serde_json::to_string(&account).unwrap().contains(TOKEN));
    // A fresh nonce per save, even for the same secret.
    account.save_claude_token(TOKEN).unwrap();
    let again: Vec<u8> = fx
        .db()
        .query_row("SELECT nonce FROM provider_account_credentials", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_ne!(nonce, again);
    // The key sits in its own file, never inside the database.
    let key = std::fs::read(fx.key()).unwrap();
    assert_eq!(key.len(), KEY_LEN);
    let database = std::fs::read(fx.root.path().join("monocode.db")).unwrap();
    assert!(!database.windows(KEY_LEN).any(|w| w == key.as_slice()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(fx.key()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn a_row_copied_to_another_account_fails_to_decrypt() {
    let fx = Fixture::new();
    fx.profile("work").save_claude_token(TOKEN).unwrap();
    fx.db()
        .execute(
            "INSERT INTO provider_account_credentials
             SELECT provider, 'thief', kind, nonce, ciphertext FROM provider_account_credentials",
            [],
        )
        .unwrap();
    let thief = fx.profile("thief");
    assert!(thief.uses_claude_token().unwrap());
    assert_eq!(thief.claude_token().unwrap_err(), UNAVAILABLE);
    assert!(thief.env(Kind::Claude).is_err());
    // Relabelling the kind of a row breaks it as well.
    fx.db()
        .execute("UPDATE provider_account_credentials SET kind='api-key'", [])
        .unwrap();
    assert_eq!(fx.profile("work").claude_token().unwrap_err(), UNAVAILABLE);
}

#[test]
fn missing_or_corrupt_key_fails_closed_but_never_blocks_removal() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    account.save_claude_token(TOKEN).unwrap();
    std::fs::remove_file(fx.key()).unwrap();
    assert_eq!(account.env(Kind::Claude).unwrap_err(), UNAVAILABLE);
    assert_eq!(
        account.claude_credential().unwrap(),
        Some(ClaudeCredential::SetupToken)
    );
    std::fs::write(fx.key(), b"short").unwrap();
    assert_eq!(account.env(Kind::Claude).unwrap_err(), UNAVAILABLE);
    // A wrong but well-formed key authenticates nothing either.
    std::fs::write(fx.key(), [7u8; KEY_LEN]).unwrap();
    assert_eq!(account.env(Kind::Claude).unwrap_err(), UNAVAILABLE);
    // Replacing recovers; so does removing.
    account.save_claude_token("sk-ant-oat01-recovered").unwrap();
    assert!(account.env(Kind::Claude).is_ok());
    std::fs::remove_file(fx.key()).unwrap();
    account.remove_claude_token().unwrap();
    assert_eq!(fx.rows(), 0);
    assert!(account.env(Kind::Claude).is_ok());
}

#[test]
fn key_creation_is_race_safe() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(KEY_FILE);
    let first = create_key(&path).unwrap();
    let second = create_key(&path).unwrap();
    assert_eq!(first, second);
    assert_eq!(read_key(&path).unwrap(), first);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn cross_kind_replace_is_one_atomic_upsert() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    account.save_claude_token("sk-ant-oat01-old").unwrap();
    account
        .save_claude_credential(ClaudeCredential::ApiKey, "sk-ant-api03-new")
        .unwrap();
    assert_eq!(fx.rows(), 1);
    assert_eq!(
        account.claude_credential().unwrap(),
        Some(ClaudeCredential::ApiKey)
    );
    assert_eq!(
        account.claude_token().unwrap().as_deref(),
        Some("sk-ant-api03-new")
    );
    // Invalid input and a failing write both leave the previous row intact.
    assert!(account.save_claude_token("not a token").is_err());
    fx.db()
        .execute_batch(
            "CREATE TRIGGER refuse BEFORE UPDATE ON provider_account_credentials
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
    let error = account.save_claude_token("sk-ant-oat01-never").unwrap_err();
    assert!(!error.contains("never"));
    assert_eq!(
        account.claude_credential().unwrap(),
        Some(ClaudeCredential::ApiKey)
    );
    assert_eq!(
        account.claude_token().unwrap().as_deref(),
        Some("sk-ant-api03-new")
    );
}

#[test]
fn removing_the_account_or_switching_to_browser_deletes_the_row() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    let personal = fx.profile("personal");
    account.save_claude_token(TOKEN).unwrap();
    personal.save_claude_token("sk-ant-oat01-personal").unwrap();
    account.remove_claude_token().unwrap();
    assert!(!account.uses_claude_token().unwrap());
    assert_eq!(fx.rows(), 1);
    account.remove_claude_token().unwrap();
    account.save_claude_token(TOKEN).unwrap();
    let dir = account.directory(Kind::Claude).unwrap().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    account.remove_credentials(Kind::Claude).unwrap();
    assert!(!dir.exists());
    assert_eq!(fx.rows(), 1);
    assert_eq!(
        personal.claude_token().unwrap().as_deref(),
        Some("sk-ant-oat01-personal")
    );
    // Without a database or table there is nothing to remove.
    let empty = tempfile::tempdir().unwrap();
    Profile::new("work", empty.path())
        .remove_claude_token()
        .unwrap();
}

#[test]
fn environment_injects_the_right_variable_and_keeps_the_rest_removed() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    account.save_claude_token(TOKEN).unwrap();
    let (set, remove) = account.env(Kind::Claude).unwrap();
    assert!(set
        .iter()
        .any(|(k, v)| k == "CLAUDE_CODE_OAUTH_TOKEN" && v.as_os_str() == OsStr::new(TOKEN)));
    assert!(!remove.contains(&"CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(remove.contains(&"ANTHROPIC_API_KEY"));
    assert!(remove.contains(&"ANTHROPIC_AUTH_TOKEN"));
    let mut command = Command::new("claude");
    for key in ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"] {
        command.env(key, "ambient-fixture");
    }
    account.apply(Kind::Claude, &mut command).unwrap();
    let env: HashMap<_, _> = command.get_envs().collect();
    assert_eq!(
        env[OsStr::new("CLAUDE_CODE_OAUTH_TOKEN")],
        Some(OsStr::new(TOKEN))
    );
    assert_eq!(env[OsStr::new("ANTHROPIC_API_KEY")], None);

    account
        .save_claude_credential(ClaudeCredential::ApiKey, "sk-ant-api03-fixture")
        .unwrap();
    let (set, remove) = account.env(Kind::Claude).unwrap();
    assert!(set
        .iter()
        .any(|(k, v)| k == "ANTHROPIC_API_KEY"
            && v.as_os_str() == OsStr::new("sk-ant-api03-fixture")));
    assert!(!set.iter().any(|(k, _)| k == "CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(!remove.contains(&"ANTHROPIC_API_KEY"));
    assert!(remove.contains(&"CLAUDE_CODE_OAUTH_TOKEN"));

    // Other accounts and the default profile never receive the secret.
    for profile in [fx.profile("other"), Profile::default()] {
        let (set, _) = profile.env(Kind::Claude).unwrap();
        assert!(!set
            .iter()
            .any(|(k, _)| k == "ANTHROPIC_API_KEY" || k == "CLAUDE_CODE_OAUTH_TOKEN"));
    }
    account.remove_claude_token().unwrap();
    assert!(account
        .env(Kind::Claude)
        .unwrap()
        .1
        .contains(&"CLAUDE_CODE_OAUTH_TOKEN"));
}

#[test]
fn the_registered_database_is_used_wherever_it_lives() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let database = elsewhere.path().join("custom-name.db");
    let _store = crate::store::Store::open(&database).unwrap();
    // Profiles are keyed by data directory, so register that directory.
    register_database(root.path(), &database);
    let account = Profile::new("work", root.path());
    account.save_claude_token(TOKEN).unwrap();
    assert!(elsewhere.path().join(KEY_FILE).exists());
    assert!(!root.path().join(KEY_FILE).exists());
    assert_eq!(account.claude_token().unwrap().as_deref(), Some(TOKEN));
}

#[test]
fn saving_needs_a_named_account_and_an_existing_database() {
    assert!(Profile::default().save_claude_token(TOKEN).is_err());
    let empty = tempfile::tempdir().unwrap();
    let account = Profile::new("work", empty.path());
    // No store was opened for this directory.
    assert!(account.save_claude_token(TOKEN).is_err());
    // A registered but not yet created database is not created by a save.
    register_database(empty.path(), &empty.path().join("monocode.db"));
    assert!(account.save_claude_token(TOKEN).is_err());
    assert!(!empty.path().join("monocode.db").exists());
    assert!(!account.uses_claude_token().unwrap());
    assert!(account.env(Kind::Claude).is_ok());
}

#[test]
fn api_key_validation_accepts_wrapping_and_never_echoes_input() {
    assert_eq!(
        normalize_claude_api_key(" sk-ant-api03-fix\n ture_1 ").unwrap(),
        "sk-ant-api03-fixture_1"
    );
    for value in [
        "",
        "sk-ant-api",
        "sk-ant-api03-",
        " sk-ant-api03-\n",
        " sk-ant-api \n",
        "sk-ant-oat01-token",
        "export ANTHROPIC_API_KEY=sk-ant-api03-private",
        "sk-ant-api03-abc;xyz",
        &format!("sk-ant-api03-{}", "a".repeat(9000)),
    ] {
        let error = normalize_claude_api_key(value).unwrap_err();
        // The bare prefix is quoted by the guidance itself, so it is exempt.
        assert!(
            value.trim().is_empty()
                || value.trim() == "sk-ant-api"
                || !error.contains(value.trim())
        );
    }
    assert!(normalize_claude_api_key("sk-ant-api0").is_ok());
}

#[test]
fn token_accounts_do_not_infer_browser_identity_or_fetch_usage() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    account.save_claude_token("sk-ant-oat01-fixture").unwrap();
    let dir = account.directory(Kind::Claude).unwrap().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".claude.json"),
        r#"{"oauthAccount":{"emailAddress":"old@example.test"}}"#,
    )
    .unwrap();
    assert!(account.identity(Kind::Claude).is_none());
    let error = crate::usage::fetch_for(Kind::Claude, &account, None).unwrap_err();
    assert!(!error.unavailable);
    assert!(error.message.contains("unavailable"));
    account.remove_claude_token().unwrap();
    assert_eq!(
        account.identity(Kind::Claude).unwrap().email.as_deref(),
        Some("old@example.test")
    );
}

#[test]
fn an_existing_key_is_never_replaced_after_a_transient_error() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    account.save_claude_token(TOKEN).unwrap();
    let original = std::fs::read(fx.key()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(fx.key(), std::fs::Permissions::from_mode(0o000)).unwrap();
        // Skip when the process can read it anyway (for example as root).
        if std::fs::read(fx.key()).is_err() {
            assert!(account.save_claude_token("sk-ant-oat01-other").is_err());
            assert_eq!(account.env(Kind::Claude).unwrap_err(), UNAVAILABLE);
        }
        std::fs::set_permissions(fx.key(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_eq!(std::fs::read(fx.key()).unwrap(), original);
    assert_eq!(account.claude_token().unwrap().as_deref(), Some(TOKEN));
}

#[test]
fn a_damaged_key_is_not_replaced_while_other_accounts_depend_on_it() {
    let fx = Fixture::new();
    fx.profile("work").save_claude_token(TOKEN).unwrap();
    fx.profile("other")
        .save_claude_token("sk-ant-oat01-other")
        .unwrap();
    std::fs::write(fx.key(), b"short").unwrap();
    let error = fx
        .profile("work")
        .save_claude_token("sk-ant-oat01-new")
        .unwrap_err();
    assert_eq!(error, DAMAGED_KEY);
    assert_eq!(std::fs::read(fx.key()).unwrap(), b"short");
    // The previous row is untouched too.
    assert_eq!(fx.rows(), 2);
    // Once the others are removed, the damaged key is replaced.
    fx.profile("other").remove_claude_token().unwrap();
    fx.profile("work")
        .save_claude_token("sk-ant-oat01-new")
        .unwrap();
    assert_eq!(std::fs::read(fx.key()).unwrap().len(), KEY_LEN);
    assert_eq!(
        fx.profile("work").claude_token().unwrap().as_deref(),
        Some("sk-ant-oat01-new")
    );
}

#[test]
fn a_damaged_key_is_not_replaced_while_git_credentials_depend_on_it() {
    let fx = Fixture::new();
    fx.db()
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS git_credentials(
               id TEXT PRIMARY KEY, label TEXT NOT NULL, platform TEXT NOT NULL,
               origin TEXT NOT NULL, username TEXT NOT NULL, nonce BLOB NOT NULL,
               ciphertext BLOB NOT NULL, created_at INTEGER NOT NULL);
             INSERT INTO git_credentials VALUES('a','l','github','https://github.com','u',x'00',x'00',0);",
        )
        .unwrap();
    std::fs::write(fx.key(), b"short").unwrap();
    let error = fx.profile("work").save_claude_token(TOKEN).unwrap_err();
    assert_eq!(error, DAMAGED_KEY);
    assert_eq!(std::fs::read(fx.key()).unwrap(), b"short");
}

#[test]
fn a_failed_save_leaves_a_damaged_key_in_place() {
    let fx = Fixture::new();
    let account = fx.profile("work");
    account.save_claude_token(TOKEN).unwrap();
    std::fs::write(fx.key(), b"short").unwrap();
    fx.db()
        .execute_batch(
            "CREATE TRIGGER refuse BEFORE UPDATE ON provider_account_credentials
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
    assert!(account.save_claude_token("sk-ant-oat01-new").is_err());
    assert_eq!(std::fs::read(fx.key()).unwrap(), b"short");
    // No temp key files are left behind.
    assert!(std::fs::read_dir(fx.root.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".credentials.key.")));
}

#[test]
fn account_list_reports_kinds_without_secrets_paths_or_emails() {
    use crate::accounts::{list_value, Book};
    let fx = Fixture::new();
    let mut book = Book::default();
    let token = book.add(Kind::Claude, "Token account").unwrap();
    let key = book.add(Kind::Claude, "Key account").unwrap();
    let plain = book.add(Kind::Claude, "Browser login").unwrap();
    let codex = book.add(Kind::Codex, "Codex work").unwrap();
    fx.profile(&token.id).save_claude_token(TOKEN).unwrap();
    fx.profile(&key.id)
        .save_claude_credential(ClaudeCredential::ApiKey, "sk-ant-api03-fixture-key")
        .unwrap();
    book.set_preferred(Kind::Claude, &key.id).unwrap();
    let kind_of = |id: &str| fx.profile(id).claude_credential().ok().flatten();
    let value = list_value(&book, Some(Kind::Claude), &kind_of);
    let rows = value["accounts"].as_array().unwrap();
    let row = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(row(&token.id)["authKind"], "setupToken");
    assert_eq!(row(&key.id)["authKind"], "apiKey");
    assert_eq!(row(&plain.id)["authKind"], "cli");
    assert_eq!(row("default")["authKind"], "cli");
    assert_eq!(row(&key.id)["isDefault"], true);
    assert_eq!(row("default")["isDefault"], false);
    assert!(rows.iter().all(|r| r["provider"] == "claude"));
    for r in rows {
        let mut names: Vec<_> = r.as_object().unwrap().keys().cloned().collect();
        names.sort();
        assert_eq!(names, ["authKind", "id", "isDefault", "label", "provider"]);
    }
    // No filter lists every account-capable provider; codex is always "cli".
    let all = list_value(&book, None, &kind_of);
    let all_rows = all["accounts"].as_array().unwrap();
    assert!(all_rows.iter().any(|r| r["provider"] == "devin"));
    let codex_row = all_rows
        .iter()
        .find(|r| r["id"] == codex.id.as_str())
        .unwrap();
    assert_eq!(codex_row["authKind"], "cli");
    let text = all.to_string();
    for forbidden in [
        TOKEN,
        "sk-ant-api03-fixture-key",
        fx.root.path().to_str().unwrap(),
        "@",
    ] {
        assert!(!text.contains(forbidden), "leaked {forbidden}");
    }
}
