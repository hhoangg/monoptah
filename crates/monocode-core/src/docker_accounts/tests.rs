use super::*;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const PAT: &str = concat!("dckr_pat_", "exampleTokenValue1234567890");
const PAT2: &str = concat!("dckr_pat_", "secondTokenValue0987654321");

fn b64(text: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text)
}
/// A JWT whose payload is `payload` (the signature is junk).
fn jwt_with(payload: &str) -> String {
    format!(
        "{}.{}.{}",
        b64(r#"{"alg":"RS256"}"#),
        b64(payload),
        "c2lnbmF0dXJl"
    )
}
fn jwt(exp: u64, nonce: u64) -> String {
    jwt_with(&format!(
        r#"{{"sub":"u","scope":"repo:read sandbox:use","exp":{exp},"iat":1,"n":{nonce}}}"#
    ))
}

type Reply = Arc<dyn Fn(&str, &str, usize) -> Result<String, ExchangeError> + Send + Sync>;
struct FakeHub {
    calls: AtomicUsize,
    delay_ms: u64,
    reply: Mutex<Reply>,
}
impl FakeHub {
    fn new(reply: Reply) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
            reply: Mutex::new(reply),
        })
    }
    fn slow(reply: Reply) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            delay_ms: 80,
            reply: Mutex::new(reply),
        })
    }
    fn set(&self, reply: Reply) {
        *self.reply.lock().unwrap() = reply;
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
impl DockerHub for FakeHub {
    fn exchange(&self, docker_id: &str, pat: &str) -> Result<Secret, ExchangeError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
        }
        let reply = self.reply.lock().unwrap().clone();
        reply(docker_id, pat, n).map(Secret::new)
    }
}
fn ok_hub() -> Arc<FakeHub> {
    FakeHub::new(Arc::new(|_, _, n| Ok(jwt(10_000, n as u64))))
}

struct Fixture {
    root: tempfile::TempDir,
    _store: crate::store::Store,
    accounts: DockerAccounts,
    database: PathBuf,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("monocode.db");
    let store = crate::store::Store::open(&database).unwrap();
    let accounts = DockerAccounts::for_store(&store).unwrap();
    Fixture {
        root,
        _store: store,
        accounts,
        database,
    }
}
fn clock(now: &Arc<AtomicU64>) -> Clock {
    let now = now.clone();
    Arc::new(move || now.load(Ordering::SeqCst))
}
fn access(f: &Fixture, hub: Arc<FakeHub>, now: &Arc<AtomicU64>) -> DockerAccess {
    DockerAccess::new(f.database.clone(), hub, clock(now))
}
fn now_at(t: u64) -> Arc<AtomicU64> {
    Arc::new(AtomicU64::new(t))
}

#[test]
fn round_trip_add_list_get_replace_check_remove() {
    let f = fixture();
    let hub = ok_hub();
    assert!(f.accounts.list().unwrap().is_empty());
    let added = f.accounts.add("  Alice ", PAT, hub.as_ref()).unwrap();
    assert_eq!(added.docker_id, "Alice");
    assert_eq!(added.status, DockerAccountStatus::Valid);
    assert_eq!(added.expires_at, None);
    assert!(added.checked_at.is_some());
    assert_eq!(f.accounts.list().unwrap(), vec![added.clone()]);
    assert_eq!(f.accounts.get(&added.id).unwrap(), added);
    let replaced = f
        .accounts
        .replace_token(&added.id, PAT2, hub.as_ref())
        .unwrap();
    assert_eq!(replaced.id, added.id);
    assert_eq!(
        f.accounts.secret(&added.id).unwrap().unwrap().1.as_str(),
        PAT2
    );
    let checked = f.accounts.check(&added.id, hub.as_ref()).unwrap();
    assert_eq!(checked.status, DockerAccountStatus::Valid);
    f.accounts.remove(&added.id).unwrap();
    assert!(f.accounts.list().unwrap().is_empty());
    assert_eq!(f.accounts.get(&added.id).unwrap_err(), NOT_FOUND);
    f.accounts.remove(&added.id).unwrap();
}

#[test]
fn exchange_receives_the_docker_id_and_pat() {
    let f = fixture();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let hub = FakeHub::new(Arc::new(move |id, pat, n| {
        sink.lock().unwrap().push((id.to_owned(), pat.to_owned()));
        Ok(jwt(10_000, n as u64))
    }));
    f.accounts.add("alice", PAT, hub.as_ref()).unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![("alice".to_owned(), PAT.to_owned())]
    );
}

#[test]
fn duplicate_docker_id_is_refused_case_insensitively() {
    let f = fixture();
    let hub = ok_hub();
    f.accounts.add("Alice", PAT, hub.as_ref()).unwrap();
    let calls = hub.calls();
    let error = f.accounts.add("aLICE", PAT2, hub.as_ref()).unwrap_err();
    assert!(
        error.contains("already added; use Replace token"),
        "{error}"
    );
    assert_eq!(hub.calls(), calls, "no exchange for a duplicate");
    assert_eq!(f.accounts.list().unwrap().len(), 1);
}

#[test]
fn input_validation() {
    let f = fixture();
    let hub = ok_hub();
    for bad in [
        "",
        " ",
        "a",
        "has space",
        "semi;colon",
        &"x".repeat(65),
        "a/b",
    ] {
        assert!(f.accounts.add(bad, PAT, hub.as_ref()).is_err(), "{bad:?}");
    }
    for bad in [
        "",
        "hunter2password",
        "ghp_notDocker",
        "dckr_pat_two words",
        "dckr_pat_line\nbreak",
    ] {
        assert!(
            f.accounts.add("alice", bad, hub.as_ref()).is_err(),
            "{bad:?}"
        );
    }
    assert_eq!(
        f.accounts
            .add("alice", "hunter2password", hub.as_ref())
            .unwrap_err(),
        "Use a personal access token (dckr_pat_...), not your Docker password."
    );
    assert_eq!(hub.calls(), 0);
    f.accounts
        .add("al", "dckr_oat_orgToken123", hub.as_ref())
        .unwrap();
    f.accounts.add(&"x".repeat(64), PAT, hub.as_ref()).unwrap();
}

#[test]
fn rejected_and_missing_scope_and_network_at_add() {
    let f = fixture();
    let rejected = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected)));
    assert_eq!(
        f.accounts.add("alice", PAT, rejected.as_ref()).unwrap_err(),
        "Docker Hub rejected this Docker ID and token. Check both, and that the token is not revoked or expired."
    );
    let no_scope = FakeHub::new(Arc::new(|_, _, _| {
        Ok(jwt_with(r#"{"scope":"repo:read","exp":9999999999}"#))
    }));
    assert_eq!(
        f.accounts.add("alice", PAT, no_scope.as_ref()).unwrap_err(),
        "This token has no Cloud Sandboxes access. Create a token with \"Allow Cloud Sandboxes access\" ticked."
    );
    let network = FakeHub::new(Arc::new(|_, _, _| {
        Err(ExchangeError::Network("Dns Failed".into()))
    }));
    assert_eq!(
        f.accounts.add("alice", PAT, network.as_ref()).unwrap_err(),
        "Could not reach Docker Hub (Dns Failed). Check your connection and try again."
    );
    let status = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Status(503))));
    assert!(f
        .accounts
        .add("alice", PAT, status.as_ref())
        .unwrap_err()
        .contains("HTTP 503"));
    let malformed = FakeHub::new(Arc::new(|_, _, _| Ok("not-a-jwt".into())));
    assert!(f
        .accounts
        .add("alice", PAT, malformed.as_ref())
        .unwrap_err()
        .contains("unexpected answer"));
    assert!(f.accounts.list().unwrap().is_empty());
}

#[test]
fn replace_token_checks_input_and_scope_and_keeps_old_token_on_failure() {
    let f = fixture();
    let hub = ok_hub();
    let a = f.accounts.add("alice", PAT, hub.as_ref()).unwrap();
    assert!(f
        .accounts
        .replace_token(&a.id, "nope", hub.as_ref())
        .is_err());
    let rejected = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected)));
    assert!(f
        .accounts
        .replace_token(&a.id, PAT2, rejected.as_ref())
        .is_err());
    assert_eq!(f.accounts.secret(&a.id).unwrap().unwrap().1.as_str(), PAT);
    assert_eq!(
        f.accounts
            .replace_token("missing", PAT2, hub.as_ref())
            .unwrap_err(),
        NOT_FOUND
    );
}

#[test]
fn check_updates_status_and_network_failure_keeps_it() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let rejected = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected)));
    let after = f.accounts.check(&a.id, rejected.as_ref()).unwrap();
    assert_eq!(after.status, DockerAccountStatus::Revoked);
    assert_eq!(
        DockerAccountStatus::Revoked.label(),
        "Token revoked or expired"
    );
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::Revoked
    );
    let network = FakeHub::new(Arc::new(|_, _, _| {
        Err(ExchangeError::Network("timed out".into()))
    }));
    let error = f.accounts.check(&a.id, network.as_ref()).unwrap_err();
    assert!(
        error.contains("Could not reach Docker Hub (timed out)"),
        "{error}"
    );
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::Revoked
    );
    let no_scope = FakeHub::new(Arc::new(|_, _, _| Ok(jwt_with(r#"{"exp":1}"#))));
    let after = f.accounts.check(&a.id, no_scope.as_ref()).unwrap();
    assert_eq!(after.status, DockerAccountStatus::MissingScope);
    assert_eq!(after.status.label(), "Missing Cloud Sandboxes access");
    let after = f.accounts.check(&a.id, ok_hub().as_ref()).unwrap();
    assert_eq!(after.status, DockerAccountStatus::Valid);
    assert_eq!(
        f.accounts.check("missing", ok_hub().as_ref()).unwrap_err(),
        NOT_FOUND
    );
}

#[test]
fn status_text_round_trips() {
    for s in [
        DockerAccountStatus::Valid,
        DockerAccountStatus::Revoked,
        DockerAccountStatus::MissingScope,
        DockerAccountStatus::Unchecked,
    ] {
        assert_eq!(DockerAccountStatus::parse(s.as_str()), Some(s));
    }
    assert_eq!(DockerAccountStatus::parse("bogus"), None);
}

#[test]
fn claims_parsing() {
    let c = parse_claims(&jwt_with(
        r#"{"sub":"alice","scope":"repo:read sandbox:use","exp":200,"iat":100}"#,
    ))
    .unwrap();
    assert_eq!(
        (c.exp, c.iat, c.sub.as_deref()),
        (Some(200), Some(100), Some("alice"))
    );
    assert_eq!(c.scopes, vec!["repo:read", "sandbox:use"]);
    assert!(c.has_sandbox_access());

    let c = parse_claims(&jwt_with(r#"{"scope":["repo:read","sandboxes"],"exp":5}"#)).unwrap();
    assert!(c.has_sandbox_access());
    let c = parse_claims(&jwt_with(r#"{"scope":["repo:read"]}"#)).unwrap();
    assert!(!c.has_sandbox_access());
    let c = parse_claims(&jwt_with(r#"{"scope":"repo:read my-sandbox"}"#)).unwrap();
    assert!(!c.has_sandbox_access(), "must start with sandbox");

    let c = parse_claims(&jwt_with(r#"{"scope":"sandbox:use"}"#)).unwrap();
    assert_eq!((c.exp, c.iat, c.sub), (None, None, None));
    assert!(parse_claims(&jwt_with("{}")).unwrap().scopes.is_empty());

    for bad in [
        "",
        "abc",
        "a.b",
        "a.!!!.c",
        &format!("a.{}.c", b64("[1]")),
        &format!("a.{}.c", b64("nope")),
    ] {
        assert!(parse_claims(bad).is_none(), "{bad}");
    }
}

#[test]
fn cache_hit_refresh_and_invalidate() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let now = now_at(1_000);
    let hub = FakeHub::new(Arc::new(|_, _, n| Ok(jwt(1_000 + 900, n as u64))));
    let access = access(&f, hub.clone(), &now);

    let first = access.access_token(&a.id).unwrap();
    assert_eq!(hub.calls(), 1);
    let again = access.access_token(&a.id).unwrap();
    assert_eq!(again.as_str(), first.as_str());
    assert_eq!(hub.calls(), 1, "cache hit");

    now.store(1_000 + 839, Ordering::SeqCst); // 61 s left
    access.access_token(&a.id).unwrap();
    assert_eq!(hub.calls(), 1);
    now.store(1_000 + 840, Ordering::SeqCst); // 60 s left: refresh
    let refreshed = access.access_token(&a.id).unwrap();
    assert_eq!(hub.calls(), 2);
    assert_ne!(refreshed.as_str(), first.as_str());

    access.invalidate(&a.id);
    access.access_token(&a.id).unwrap();
    assert_eq!(hub.calls(), 3);
    access.invalidate("never-seen");
}

#[test]
fn token_without_exp_is_never_reused() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let hub = FakeHub::new(Arc::new(|_, _, _| {
        Ok(jwt_with(r#"{"scope":"sandbox:use"}"#))
    }));
    let access = access(&f, hub.clone(), &now_at(5));
    access.access_token(&a.id).unwrap();
    access.access_token(&a.id).unwrap();
    assert_eq!(hub.calls(), 2);
}

#[test]
fn access_token_errors_and_status_updates() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let now = now_at(1);
    let hub = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected)));
    let access = access(&f, hub.clone(), &now);
    let error = access.access_token(&a.id).unwrap_err();
    assert_eq!(
        error,
        DockerAuthError::Revoked {
            docker_id: "alice".into()
        }
    );
    assert_eq!(
        error.to_string(),
        "Docker account alice: token revoked or expired. Replace it in Settings > Cloud."
    );
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::Revoked
    );

    hub.set(Arc::new(|_, _, _| {
        Ok(jwt_with(r#"{"scope":"repo:read","exp":99999}"#))
    }));
    let error = access.access_token(&a.id).unwrap_err();
    assert!(matches!(error, DockerAuthError::MissingScope { .. }));
    assert!(error.to_string().contains("alice"));
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::MissingScope
    );

    hub.set(Arc::new(|_, _, _| {
        Err(ExchangeError::Network("timed out".into()))
    }));
    let error = access.access_token(&a.id).unwrap_err();
    assert!(matches!(error, DockerAuthError::Network { .. }));
    assert!(error.to_string().contains("alice") && error.to_string().contains("timed out"));
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::MissingScope,
        "a network failure keeps the status"
    );

    hub.set(Arc::new(|_, _, n| Ok(jwt(99_999, n as u64))));
    access.access_token(&a.id).unwrap();
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::Valid
    );

    assert_eq!(
        access.access_token("nope").unwrap_err(),
        DockerAuthError::UnknownAccount
    );
}

#[test]
fn concurrent_refreshes_of_one_account_are_coalesced() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let hub = FakeHub::slow(Arc::new(|_, _, n| Ok(jwt(100_000, n as u64))));
    let access = Arc::new(access(&f, hub.clone(), &now_at(1)));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let access = access.clone();
            let id = a.id.clone();
            std::thread::spawn(move || access.access_token(&id).unwrap().as_str().to_owned())
        })
        .collect();
    let tokens: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(hub.calls(), 1);
    assert!(tokens.iter().all(|t| t == &tokens[0]));
}

#[test]
fn different_accounts_refresh_independently() {
    let f = fixture();
    let ok = ok_hub();
    let a = f.accounts.add("alice", PAT, ok.as_ref()).unwrap();
    let b = f.accounts.add("bob", PAT2, ok.as_ref()).unwrap();
    // Each exchange takes 80 ms; if one lock covered both accounts the pair would take
    // 160 ms or more, so overlapping is observed through the peak concurrency.
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (l, p) = (live.clone(), peak.clone());
    let hub = FakeHub::new(Arc::new(move |_, _, n| {
        let now = l.fetch_add(1, Ordering::SeqCst) + 1;
        p.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(80));
        l.fetch_sub(1, Ordering::SeqCst);
        Ok(jwt(100_000, n as u64))
    }));
    let access = Arc::new(access(&f, hub.clone(), &now_at(1)));
    let handles: Vec<_> = [a.id.clone(), b.id.clone()]
        .into_iter()
        .map(|id| {
            let access = access.clone();
            std::thread::spawn(move || access.access_token(&id).unwrap())
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(hub.calls(), 2);
    assert_eq!(peak.load(Ordering::SeqCst), 2, "exchanges overlapped");
}

#[test]
fn secrets_never_appear_in_errors_debug_or_the_database() {
    let f = fixture();
    let secret_jwt = jwt(100_000, 7);
    let leaked = secret_jwt.clone();
    let hub = FakeHub::new(Arc::new(move |_, _, _| Ok(leaked.clone())));
    let a = f.accounts.add("alice", PAT, hub.as_ref()).unwrap();
    assert!(!format!("{a:?}").contains(PAT));
    let token = DockerAccess::new(f.database.clone(), hub.clone(), clock(&now_at(1)))
        .access_token(&a.id)
        .unwrap();
    assert_eq!(format!("{token:?}"), "Secret(***)");

    let mut texts = Vec::new();
    let failing: Vec<Arc<FakeHub>> = vec![
        FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected))),
        FakeHub::new(Arc::new(|_, _, _| {
            Err(ExchangeError::Network(format!("boom {PAT}")))
        })),
        FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Malformed))),
        FakeHub::new(Arc::new(|_, _, _| Ok(jwt_with(r#"{"scope":"x"}"#)))),
    ];
    for h in &failing {
        if let Err(e) = f.accounts.add("carol", PAT, h.as_ref()) {
            texts.push(e);
        }
        if let Err(e) = f.accounts.check(&a.id, h.as_ref()) {
            texts.push(e);
        }
        let access = access(&f, h.clone(), &now_at(1));
        if let Err(e) = access.access_token(&a.id) {
            texts.push(format!("{e} {e:?}"));
        }
    }
    for text in &texts {
        assert!(!text.contains(PAT) && !text.contains(&secret_jwt), "{text}");
    }
    assert!(!texts.is_empty());

    for name in ["monocode.db", "monocode.db-wal"] {
        if let Ok(bytes) = std::fs::read(f.root.path().join(name)) {
            assert!(
                !bytes.windows(PAT.len()).any(|w| w == PAT.as_bytes()),
                "{name}"
            );
            assert!(
                !bytes
                    .windows(secret_jwt.len())
                    .any(|w| w == secret_jwt.as_bytes()),
                "{name}"
            );
        }
    }
}

#[test]
fn list_and_remove_never_decrypt() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    std::fs::remove_file(f.root.path().join(sealed::KEY_FILE)).unwrap();
    assert_eq!(f.accounts.list().unwrap().len(), 1);
    assert_eq!(f.accounts.get(&a.id).unwrap().docker_id, "alice");
    assert!(f.accounts.secret(&a.id).is_err());
    f.accounts.remove(&a.id).unwrap();
    assert!(f.accounts.list().unwrap().is_empty());
}

#[test]
fn ciphertext_is_bound_to_id_and_docker_id() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let conn = Connection::open(&f.database).unwrap();
    conn.execute(
        "UPDATE docker_accounts SET docker_id='mallory' WHERE id=?1",
        [&a.id],
    )
    .unwrap();
    assert!(f.accounts.secret(&a.id).is_err());
}

#[test]
fn missing_database_or_table_is_empty_and_never_created() {
    let root = tempfile::tempdir().unwrap();
    let missing = DockerAccounts::at(root.path().join("none.db"));
    assert!(missing.list().unwrap().is_empty());
    missing.remove("x").unwrap();
    assert!(missing.add("alice", PAT, ok_hub().as_ref()).is_err());
    assert!(!root.path().join("none.db").exists());
    assert!(!root.path().join(sealed::KEY_FILE).exists());

    let bare = root.path().join("bare.db");
    Connection::open(&bare).unwrap();
    let accounts = DockerAccounts::at(bare);
    assert!(accounts.list().unwrap().is_empty());
    accounts.remove("x").unwrap();
    assert_eq!(accounts.get("x").unwrap_err(), NOT_FOUND);
}

#[test]
fn shared_slot_can_be_replaced() {
    let f = fixture();
    let one = Arc::new(access(&f, ok_hub(), &now_at(1)));
    let two = Arc::new(access(&f, ok_hub(), &now_at(1)));
    install(one.clone());
    assert!(Arc::ptr_eq(&shared().unwrap(), &one));
    install(two.clone());
    assert!(Arc::ptr_eq(&shared().unwrap(), &two));
}
