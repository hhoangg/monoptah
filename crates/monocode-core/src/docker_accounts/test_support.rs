//! Test fixtures for code that acts as a Docker account: a fake Docker Hub that hands out
//! distinct, recognisable JWTs, and a guard that installs a [`DockerAccess`] over a temp
//! database as the process-wide instance (serialised, because that instance is global).
use super::*;
use base64::Engine;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Serialises tests that read or replace the process-wide instance.
pub(super) static GLOBAL: Mutex<()> = Mutex::new(());

const PAT: &str = concat!("dckr_pat_", "exampleTokenValue1234567890");

/// Far in the future, so a cached JWT is never refreshed by the clock.
const EXP: u64 = 4_000_000_000;

/// Hands out a different JWT on every exchange and keeps every one it issued.
#[derive(Default)]
pub(crate) struct CountingHub {
    calls: AtomicUsize,
    reject: AtomicBool,
    issued: Mutex<Vec<String>>,
}
impl CountingHub {
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    /// Every JWT issued so far, oldest first.
    pub(crate) fn issued(&self) -> Vec<String> {
        self.issued.lock().unwrap().clone()
    }
    /// Makes every later exchange answer 401, as a revoked token does.
    pub(crate) fn reject_from_now_on(&self) {
        self.reject.store(true, Ordering::SeqCst);
    }
    pub(crate) fn last(&self) -> String {
        self.issued().pop().expect("no JWT was issued")
    }
}
impl DockerHub for CountingHub {
    fn exchange(&self, docker_id: &str, _pat: &str) -> Result<Secret, ExchangeError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.reject.load(Ordering::SeqCst) {
            return Err(ExchangeError::Rejected(401));
        }
        let b64 = |text: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text);
        let jwt = format!(
            "{}.{}.{}",
            b64(r#"{"alg":"RS256"}"#),
            b64(&format!(
                r#"{{"sub":"{docker_id}","scope":"repo:read sandbox:use","exp":{EXP},"iat":1,"n":{n}}}"#
            )),
            "c2lnbmF0dXJl"
        );
        self.issued.lock().unwrap().push(jwt.clone());
        Ok(Secret::new(jwt))
    }
}

/// A temp credentials database with a [`DockerAccess`] installed as the shared instance
/// for as long as the value lives.
pub(crate) struct Installed {
    pub(crate) hub: Arc<CountingHub>,
    pub(crate) accounts: DockerAccounts,
    _store: crate::store::Store,
    _root: tempfile::TempDir,
    previous: Option<Arc<DockerAccess>>,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl Installed {
    pub(crate) fn new() -> Self {
        let lock = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("monocode.db");
        let store = crate::store::Store::open(&database).unwrap();
        let accounts = DockerAccounts::for_store(&store).unwrap();
        let hub = Arc::new(CountingHub::default());
        let access = Arc::new(DockerAccess::new(database, hub.clone(), system_clock()));
        let previous = replace_shared_for_test(Some(access));
        Self {
            hub,
            accounts,
            _store: store,
            _root: root,
            previous,
            _lock: lock,
        }
    }
    /// Saves an account for `docker_id` and returns its id.
    pub(crate) fn add(&self, docker_id: &str) -> String {
        self.accounts
            .add(docker_id, PAT, self.hub.as_ref())
            .unwrap()
            .id
    }
}
impl Drop for Installed {
    fn drop(&mut self) {
        replace_shared_for_test(self.previous.take());
    }
}

/// Holds the global lock with NO shared instance, for tests of the "not available" path.
pub(crate) struct Uninstalled {
    previous: Option<Arc<DockerAccess>>,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl Uninstalled {
    pub(crate) fn new() -> Self {
        let lock = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        Self {
            previous: replace_shared_for_test(None),
            _lock: lock,
        }
    }
}
impl Drop for Uninstalled {
    fn drop(&mut self) {
        replace_shared_for_test(self.previous.take());
    }
}
