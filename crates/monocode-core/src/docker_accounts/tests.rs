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
        f.accounts.secret(&added.id).unwrap().unwrap().2.as_str(),
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
    let rejected = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected(401))));
    assert_eq!(
        f.accounts.add("alice", PAT, rejected.as_ref()).unwrap_err(),
        "Wrong Docker ID or token, or the token is revoked or expired."
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
    assert_eq!(
        f.accounts.add("alice", PAT, status.as_ref()).unwrap_err(),
        "Docker Hub answered HTTP 503. Try again later."
    );
    let malformed = FakeHub::new(Arc::new(|_, _, _| Ok("not-a-jwt".into())));
    assert_eq!(
        f.accounts
            .add("alice", PAT, malformed.as_ref())
            .unwrap_err(),
        "Docker Hub sent an unexpected answer. Try again later."
    );
    let unusable = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Malformed)));
    assert_eq!(
        f.accounts.add("alice", PAT, unusable.as_ref()).unwrap_err(),
        "Docker Hub sent an unexpected answer. Try again later."
    );
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
    let rejected = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected(401))));
    assert!(f
        .accounts
        .replace_token(&a.id, PAT2, rejected.as_ref())
        .is_err());
    assert_eq!(f.accounts.secret(&a.id).unwrap().unwrap().2.as_str(), PAT);
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
    let rejected = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected(401))));
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
    assert_eq!(DockerAccountStatus::Valid.label(), "Cloud Sandboxes access");
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
    let hub = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected(401))));
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
    // Each exchange waits until BOTH are inside the hub. If one lock covered both
    // accounts the second could never arrive, the wait times out and the test fails.
    let arrived = Arc::new((Mutex::new(0usize), std::sync::Condvar::new()));
    let gate = arrived.clone();
    let hub = FakeHub::new(Arc::new(move |_, _, n| {
        let (count, cv) = &*gate;
        let mut count = count.lock().unwrap();
        *count += 1;
        cv.notify_all();
        let (count, wait) = cv
            .wait_timeout_while(count, TIMEOUT, |count| *count < 2)
            .unwrap();
        if wait.timed_out() || *count < 2 {
            return Err(ExchangeError::Network("not concurrent".into()));
        }
        Ok(jwt(100_000, n as u64))
    }));
    let access = Arc::new(access(&f, hub.clone(), &now_at(1)));
    let handles: Vec<_> = [a.id.clone(), b.id.clone()]
        .into_iter()
        .map(|id| {
            let access = access.clone();
            std::thread::spawn(move || access.access_token(&id))
        })
        .collect();
    for h in handles {
        h.join().unwrap().expect("both refreshes ran at once");
    }
    assert_eq!(hub.calls(), 2);
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
        FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Rejected(401)))),
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

/// Serializes tests that touch the process-wide slot and restores it afterwards.
use super::test_support::GLOBAL as SHARED_LOCK;
struct SharedGuard {
    previous: Option<Arc<DockerAccess>>,
    _lock: MutexGuard<'static, ()>,
}
impl SharedGuard {
    fn install(access: Arc<DockerAccess>) -> Self {
        let lock = lock(&SHARED_LOCK);
        Self {
            previous: replace_shared_for_test(Some(access)),
            _lock: lock,
        }
    }
}
impl Drop for SharedGuard {
    fn drop(&mut self) {
        replace_shared_for_test(self.previous.take());
    }
}

#[test]
fn shared_slot_can_be_replaced() {
    let f = fixture();
    let one = Arc::new(access(&f, ok_hub(), &now_at(1)));
    let two = Arc::new(access(&f, ok_hub(), &now_at(1)));
    let guard = SharedGuard::install(one.clone());
    assert!(Arc::ptr_eq(&shared().unwrap(), &one));
    install(two.clone());
    assert!(Arc::ptr_eq(&shared().unwrap(), &two));
    let previous = guard.previous.clone();
    drop(guard);
    assert_eq!(
        shared().map(|a| Arc::as_ptr(&a)),
        previous.map(|a| Arc::as_ptr(&a)),
        "the guard restores the earlier instance"
    );
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !done() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        std::thread::yield_now();
    }
}
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn slot_of(access: &DockerAccess, id: &str) -> Arc<Slot> {
    lock(&access.cache).get(id).cloned().expect("slot exists")
}

#[test]
fn stale_rejection_does_not_overwrite_a_replaced_token_status() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    let release_rx = Mutex::new(release_rx);
    // The exchange of the OLD pat blocks until released, then answers "rejected".
    let hub = FakeHub::new(Arc::new(move |_, pat, _| {
        assert_eq!(pat, PAT);
        started_tx.lock().unwrap().send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(TIMEOUT)
            .expect("released in time");
        Err(ExchangeError::Rejected(401))
    }));
    let access = Arc::new(access(&f, hub, &now_at(1)));
    let worker = {
        let access = access.clone();
        let id = a.id.clone();
        std::thread::spawn(move || access.access_token(&id))
    };
    started_rx.recv_timeout(TIMEOUT).expect("exchange started");
    let replaced = f
        .accounts
        .replace_token(&a.id, PAT2, ok_hub().as_ref())
        .unwrap();
    assert_eq!(replaced.status, DockerAccountStatus::Valid);
    release_tx.send(()).unwrap();
    assert!(matches!(
        worker.join().unwrap(),
        Err(DockerAuthError::Revoked { .. })
    ));
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::Valid,
        "a stale rejection of the old token must not mark the new one revoked"
    );
}

#[test]
fn stale_check_result_does_not_overwrite_a_replaced_token_status() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    let release_rx = Mutex::new(release_rx);
    let hub = FakeHub::new(Arc::new(move |_, _, _| {
        started_tx.lock().unwrap().send(()).unwrap();
        release_rx.lock().unwrap().recv_timeout(TIMEOUT).unwrap();
        Err(ExchangeError::Rejected(401))
    }));
    let accounts = Arc::new(DockerAccounts::at(f.database.clone()));
    let worker = {
        let (accounts, id) = (accounts.clone(), a.id.clone());
        std::thread::spawn(move || accounts.check(&id, hub.as_ref()))
    };
    started_rx.recv_timeout(TIMEOUT).expect("check started");
    f.accounts
        .replace_token(&a.id, PAT2, ok_hub().as_ref())
        .unwrap();
    release_tx.send(()).unwrap();
    worker.join().unwrap().unwrap();
    assert_eq!(
        f.accounts.get(&a.id).unwrap().status,
        DockerAccountStatus::Valid
    );
}

#[test]
fn failures_are_coalesced_too() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let started_tx = Mutex::new(started_tx);
    let release_rx = Mutex::new(release_rx);
    let hub = FakeHub::new(Arc::new(move |_, _, _| {
        started_tx.lock().unwrap().send(()).unwrap();
        release_rx.lock().unwrap().recv_timeout(TIMEOUT).unwrap();
        Err(ExchangeError::Status(503))
    }));
    let access = Arc::new(access(&f, hub.clone(), &now_at(1)));
    let spawn = |access: Arc<DockerAccess>, id: String| {
        std::thread::spawn(move || access.access_token(&id))
    };
    let first = spawn(access.clone(), a.id.clone());
    started_rx.recv_timeout(TIMEOUT).expect("exchange started");
    let slot = slot_of(&access, &a.id);
    let rest: Vec<_> = (0..4)
        .map(|_| spawn(access.clone(), a.id.clone()))
        .collect();
    wait_until("four callers queued", || {
        slot.queued.load(Ordering::SeqCst) == 4
    });
    release_tx.send(()).unwrap();
    let mut results = vec![first.join().unwrap()];
    results.extend(rest.into_iter().map(|h| h.join().unwrap()));
    assert_eq!(hub.calls(), 1, "one exchange for the whole burst");
    for result in results {
        assert_eq!(
            result.unwrap_err(),
            DockerAuthError::Status {
                docker_id: "alice".into(),
                code: 503
            }
        );
    }
    // A caller arriving after the burst starts a new attempt.
    release_tx.send(()).unwrap();
    assert!(access.access_token(&a.id).is_err());
    assert_eq!(hub.calls(), 2);
}

#[test]
fn invalidate_forgets_a_remembered_failure() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let hub = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Status(500))));
    let access = access(&f, hub.clone(), &now_at(1));
    assert!(access.access_token(&a.id).is_err());
    access.invalidate(&a.id);
    assert!(lock(&slot_of(&access, &a.id).state).last.is_none());
    assert!(access.access_token(&a.id).is_err());
    assert_eq!(hub.calls(), 2);
}

#[test]
fn replace_token_and_remove_invalidate_the_shared_cache() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let hub = ok_hub();
    let access = Arc::new(access(&f, hub.clone(), &now_at(1)));
    let _guard = SharedGuard::install(access.clone());
    access.access_token(&a.id).unwrap();
    let slot = slot_of(&access, &a.id);
    assert!(lock(&slot.state).cached.is_some());
    f.accounts
        .replace_token(&a.id, PAT2, ok_hub().as_ref())
        .unwrap();
    assert!(lock(&slot.state).cached.is_none(), "replace drops the JWT");
    access.access_token(&a.id).unwrap();
    assert!(lock(&slot.state).cached.is_some());
    f.accounts.remove(&a.id).unwrap();
    assert!(lock(&slot.state).cached.is_none(), "remove drops the JWT");
    assert_eq!(
        access.access_token(&a.id).unwrap_err(),
        DockerAuthError::UnknownAccount
    );
}

#[test]
fn a_cache_hit_never_outlives_a_removed_or_replaced_row() {
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let hub = ok_hub();
    // Not installed as the shared instance: only the row check can protect it.
    let access = access(&f, hub.clone(), &now_at(1));
    access.access_token(&a.id).unwrap();
    access.access_token(&a.id).unwrap();
    assert_eq!(hub.calls(), 1, "cache hit");
    f.accounts
        .replace_token(&a.id, PAT2, ok_hub().as_ref())
        .unwrap();
    access.access_token(&a.id).unwrap();
    assert_eq!(hub.calls(), 2, "a replaced token is exchanged again");
    f.accounts.remove(&a.id).unwrap();
    assert_eq!(
        access.access_token(&a.id).unwrap_err(),
        DockerAuthError::UnknownAccount
    );
    assert_eq!(hub.calls(), 2);
}

#[test]
fn error_copy_for_status_and_unexpected_answers() {
    assert_eq!(ExchangeError::Rejected(401).detail(), "HTTP 401");
    assert_eq!(ExchangeError::Rejected(403).detail(), "HTTP 403");
    let f = fixture();
    let a = f.accounts.add("alice", PAT, ok_hub().as_ref()).unwrap();
    let status = FakeHub::new(Arc::new(|_, _, _| Err(ExchangeError::Status(502))));
    assert_eq!(
        f.accounts.check(&a.id, status.as_ref()).unwrap_err(),
        "Docker Hub answered HTTP 502. Try again later."
    );
    let access = access(&f, status, &now_at(1));
    assert_eq!(
        access.access_token(&a.id).unwrap_err().to_string(),
        "Docker account alice: Docker Hub answered HTTP 502. Try again later."
    );
    let junk = FakeHub::new(Arc::new(|_, _, _| Ok("junk".into())));
    let error = DockerAccess::new(f.database.clone(), junk, clock(&now_at(1)))
        .access_token(&a.id)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Docker account alice: Docker Hub sent an unexpected answer. Try again later."
    );
    assert!(!matches!(error, DockerAuthError::Network { .. }));
}

#[test]
fn a_concurrent_add_of_the_same_docker_id_hits_the_unique_index() {
    let f = fixture();
    let database = f.database.clone();
    // While the exchange runs, another window adds the same Docker ID (other casing).
    let hub = FakeHub::new(Arc::new(move |_, _, n| {
        let conn = Connection::open(&database).unwrap();
        conn.execute(
            "INSERT INTO docker_accounts(id,docker_id,nonce,ciphertext,status,created_at)
             VALUES('other','ALICE',x'00',x'00','valid',1)",
            [],
        )
        .unwrap();
        Ok(jwt(10_000, n as u64))
    }));
    let error = f.accounts.add("alice", PAT, hub.as_ref()).unwrap_err();
    assert_eq!(
        error,
        "Docker account alice is already added; use Replace token."
    );
    assert_eq!(f.accounts.list().unwrap().len(), 1);

    let conn = Connection::open(&f.database).unwrap();
    let direct = conn.execute(
        "INSERT INTO docker_accounts(id,docker_id,nonce,ciphertext,status,created_at)
         VALUES('x','aLiCe',x'00',x'00','valid',1)",
        [],
    );
    assert!(direct.is_err(), "the index is case-insensitive");
}

#[test]
fn float_exp_is_accepted() {
    let c = parse_claims(&jwt_with(
        r#"{"scope":"sandbox:use","exp":1700000000.0,"iat":1699999000.5}"#,
    ))
    .unwrap();
    assert_eq!((c.exp, c.iat), (Some(1_700_000_000), Some(1_699_999_000)));
    let c = parse_claims(&jwt_with(r#"{"exp":-5.0,"iat":"x"}"#)).unwrap();
    assert_eq!((c.exp, c.iat), (None, None));
}

#[test]
fn the_token_reply_is_parsed_without_keeping_a_plain_copy() {
    let reply: TokenReply = serde_json::from_str(r#"{"access_token":"abc.def.ghi"}"#).unwrap();
    assert_eq!(
        reply.access_token.unwrap().into_inner().as_str(),
        "abc.def.ghi"
    );
    let none: TokenReply = serde_json::from_str("{}").unwrap();
    assert!(none.access_token.is_none());
}

#[test]
fn install_system_needs_a_data_directory_and_never_replaces_an_installed_instance() {
    let _lock = lock(&SHARED_LOCK);
    let previous = replace_shared_for_test(None);
    // An in-memory store has no data directory: nothing is installed.
    let memory = crate::store::Store::open(std::path::Path::new(":memory:")).unwrap();
    install_system(&memory);
    assert!(shared().is_none());
    // A store on disk installs the instance...
    let f = fixture();
    install_system(&f._store);
    let first = shared().expect("installed");
    // ...and a second call keeps it (a test double, or an earlier start, stays in place).
    install_system(&f._store);
    assert!(Arc::ptr_eq(&shared().unwrap(), &first));
    // It reads the store's credentials database: a saved account is known to it.
    let hub = ok_hub();
    let added = f.accounts.add("alice", PAT, hub.as_ref()).unwrap();
    assert_eq!(first.docker_id(&added.id).as_deref(), Some("alice"));
    assert_eq!(first.docker_id("nope"), None);
    replace_shared_for_test(previous);
}

#[test]
fn env_for_names_an_unusable_account_instead_of_falling_back() {
    let f = fixture();
    let hub = ok_hub();
    let a = f.accounts.add("alice", PAT, hub.as_ref()).unwrap();
    let now = now_at(1);
    let access = Arc::new(access(&f, hub.clone(), &now));
    let guard = SharedGuard::install(access.clone());
    // No account: nothing, and no exchange.
    let calls = hub.calls();
    assert!(env_for(None).unwrap().is_none());
    assert_eq!(hub.calls(), calls);
    // An account: the variable and a JWT.
    let (name, token) = env_for(Some(&a.id)).unwrap().unwrap();
    assert_eq!(name, "DOCKER_ACCESS_TOKEN");
    assert!(parse_claims(token.as_str()).is_some());
    // The same account twice reuses the cached JWT.
    let calls = hub.calls();
    env_for(Some(&a.id)).unwrap();
    assert_eq!(hub.calls(), calls);
    // A removed one is an error with its own text.
    assert_eq!(
        env_for(Some("gone")).unwrap_err(),
        DockerAuthError::UnknownAccount
    );
    // The revoked-after-retry error names the Docker ID and carries no token.
    let rejected = access.rejected(&a.id);
    assert_eq!(
        rejected.to_string(),
        "Docker account alice: token revoked or expired. Replace it in Settings > Cloud."
    );
    assert_eq!(access.rejected("gone"), DockerAuthError::UnknownAccount);
    drop(guard);
    // Without an installed instance the account is not silently dropped.
    let _lock = lock(&SHARED_LOCK);
    let previous = replace_shared_for_test(None);
    let error = env_for(Some(&a.id)).unwrap_err();
    assert_eq!(error, DockerAuthError::NotAvailable);
    assert!(error.to_string().contains("not available"));
    replace_shared_for_test(previous);
    assert!(is_rejection("error: DOCKER_ACCESS_TOKEN was rejected"));
    assert!(!is_rejection("error: not logged in"));
}

#[test]
fn apply_env_sets_the_variable_on_the_command_and_never_adds_an_argument() {
    let f = fixture();
    let hub = ok_hub();
    let a = f.accounts.add("alice", PAT, hub.as_ref()).unwrap();
    let now = now_at(1);
    let _guard = SharedGuard::install(Arc::new(access(&f, hub.clone(), &now)));
    let mut with = std::process::Command::new("sbx");
    apply_env(&mut with, Some(&a.id)).unwrap();
    let value = with
        .get_envs()
        .find(|(k, _)| *k == "DOCKER_ACCESS_TOKEN")
        .and_then(|(_, v)| v)
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap();
    assert!(parse_claims(&value).is_some());
    assert_eq!(with.get_args().count(), 0);
    let mut without = std::process::Command::new("sbx");
    apply_env(&mut without, None).unwrap();
    assert_eq!(without.get_envs().count(), 0);
    assert_eq!(without.get_args().count(), 0);
    let error = apply_env(&mut std::process::Command::new("sbx"), Some("gone")).unwrap_err();
    assert!(error.contains("no longer exists"));
}

#[test]
fn invalidate_if_rejected_drops_the_token_only_for_a_failed_run_that_says_so() {
    use super::test_support::Installed;
    let installed = Installed::new();
    let id = installed.add("alice");
    let jwt = || env_for(Some(&id)).unwrap().unwrap().1.as_str().to_owned();
    let first = jwt();
    let rejected = "Error: DOCKER_ACCESS_TOKEN was rejected";
    // No account, a successful run, or another failure: nothing is dropped.
    assert!(!invalidate_if_rejected(None, false, rejected));
    assert!(!invalidate_if_rejected(Some(&id), true, rejected));
    assert!(!invalidate_if_rejected(
        Some(&id),
        false,
        "Connection refused"
    ));
    assert_eq!(jwt(), first);
    // A failed run whose output says the JWT was rejected: the next one is fresh.
    assert!(invalidate_if_rejected(Some(&id), false, rejected));
    assert_ne!(jwt(), first);
}

#[test]
fn a_terminals_ssh_exit_255_drops_the_token_and_other_exits_do_not() {
    use super::test_support::Installed;
    let installed = Installed::new();
    let id = installed.add("alice");
    let jwt = || env_for(Some(&id)).unwrap().unwrap().1.as_str().to_owned();
    let first = jwt();
    for code in [Some(0), Some(1), Some(130), None] {
        assert!(!invalidate_after_ssh_exit(Some(&id), code));
    }
    assert!(!invalidate_after_ssh_exit(None, Some(255)));
    assert_eq!(jwt(), first);
    assert!(invalidate_after_ssh_exit(Some(&id), Some(255)));
    assert_ne!(jwt(), first);
}

#[test]
fn building_the_cloud_service_never_installs_the_real_docker_hub_over_its_database() {
    use super::test_support::Uninstalled;
    let _none = Uninstalled::new();
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        crate::store::Store::open(&dir.path().join("monocode.db")).unwrap(),
    ));
    struct NoHost;
    impl crate::cloud_sessions::Host for NoHost {
        fn machines(&self) -> Result<Vec<crate::remote::Machine>, String> {
            Ok(vec![])
        }
        fn connect(
            &self,
            _: &str,
            _: &str,
            _: &std::sync::atomic::AtomicBool,
            _: &mut dyn FnMut(&str),
        ) -> Result<crate::remote::Machine, String> {
            Err("no host".into())
        }
        fn ensure(
            &self,
            _: &str,
            _: &std::sync::atomic::AtomicBool,
            _: &mut dyn FnMut(&str),
        ) -> Result<(), String> {
            Ok(())
        }
        fn request(
            &self,
            _: &str,
            _: &str,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, String> {
            Err("no host".into())
        }
        fn disconnect(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    let _service = crate::cloud_sessions::CloudSessions::new(
        std::sync::Arc::new(crate::codespaces::SystemGh),
        std::sync::Arc::new(NoHost),
        store.clone(),
    )
    .with_system_sandboxes(store);
    assert!(shared().is_none(), "the app installs it once, at startup");
}
