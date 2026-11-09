use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct FakeKeychain {
    value: Mutex<Option<String>>,
    pub(crate) denied: Mutex<bool>,
    pub(crate) fail_delete: Mutex<bool>,
    pub(crate) gets: AtomicUsize,
    pub(crate) deletes: AtomicUsize,
}
impl FakeKeychain {
    pub(crate) fn with(value: Option<String>) -> Self {
        Self {
            value: Mutex::new(value),
            ..Self::default()
        }
    }
    fn has_value(&self) -> bool {
        self.value.lock().unwrap().is_some()
    }
}
impl IdentityKeychain for FakeKeychain {
    fn get(&self) -> Result<Option<Zeroizing<String>>, String> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if *self.denied.lock().unwrap() {
            return Err("User denied access".into());
        }
        Ok(self.value.lock().unwrap().clone().map(Zeroizing::new))
    }
    fn delete(&self) -> Result<(), String> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        if *self.fail_delete.lock().unwrap() {
            return Err("delete failed".into());
        }
        *self.value.lock().unwrap() = None;
        Ok(())
    }
}

const ACCOUNT: &str = "desktop:/tmp/mobile-access.json";

fn database() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    Connection::open(&path).unwrap();
    (dir, path)
}

fn rows(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row("SELECT count(*) FROM mobile_identity", [], |r| r.get(0))
        .unwrap_or_default()
}

fn fingerprint(identity: &Identity) -> String {
    identity.fingerprint().unwrap()
}

#[test]
fn sealed_round_trip_and_tamper_detection() {
    let (_dir, path) = database();
    let conn = Connection::open(&path).unwrap();
    write_verified(&conn, &path, ACCOUNT, "{\"a\":1}").unwrap();
    let row = read_row(&conn, ACCOUNT).unwrap().unwrap();
    assert_eq!(
        open_row(&path, ACCOUNT, &row).unwrap().as_slice(),
        b"{\"a\":1}"
    );
    assert!(open_row(&path, "desktop:/other", &row).is_err());
    let mut flipped = row.clone();
    flipped.1[0] ^= 1;
    assert!(open_row(&path, ACCOUNT, &flipped).is_err());
}

#[test]
fn write_verified_returns_the_existing_row_without_overwriting() {
    let (_dir, path) = database();
    let conn = Connection::open(&path).unwrap();
    let a = serde_json::to_string(&Identity::generate().unwrap()).unwrap();
    let b = serde_json::to_string(&Identity::generate().unwrap()).unwrap();
    assert_ne!(a, b);
    write_verified(&conn, &path, ACCOUNT, &a).unwrap();
    let stored = write_verified(&conn, &path, ACCOUNT, &b).unwrap();
    assert_eq!(stored.as_slice(), a.as_bytes());
    assert_eq!(rows(&path), 1);
    let row = read_row(&conn, ACCOUNT).unwrap().unwrap();
    assert_eq!(
        open_row(&path, ACCOUNT, &row).unwrap().as_slice(),
        a.as_bytes()
    );
}

#[test]
fn migrates_keychain_identity_keeping_fingerprint() {
    let (_dir, path) = database();
    let original = Identity::generate().unwrap();
    let keychain = FakeKeychain::with(Some(serde_json::to_string(&original).unwrap()));
    let loaded = load(Some(&path), ACCOUNT, true, &keychain).unwrap();
    assert_eq!(fingerprint(&loaded), fingerprint(&original));
    assert_eq!(rows(&path), 1);
    assert_eq!(keychain.deletes.load(Ordering::SeqCst), 1);
    assert!(!keychain.has_value());
    // The second load comes from the database and never asks the keychain.
    let again = load(Some(&path), ACCOUNT, true, &keychain).unwrap();
    assert_eq!(fingerprint(&again), fingerprint(&original));
    assert_eq!(keychain.gets.load(Ordering::SeqCst), 1);
}

#[test]
fn keychain_delete_failure_is_not_fatal() {
    let (_dir, path) = database();
    let original = Identity::generate().unwrap();
    let keychain = FakeKeychain::with(Some(serde_json::to_string(&original).unwrap()));
    *keychain.fail_delete.lock().unwrap() = true;
    let loaded = load(Some(&path), ACCOUNT, true, &keychain).unwrap();
    assert_eq!(fingerprint(&loaded), fingerprint(&original));
    assert_eq!(rows(&path), 1);
}

#[test]
fn denied_keychain_errors_writes_nothing_and_retries() {
    let (_dir, path) = database();
    let original = Identity::generate().unwrap();
    let keychain = FakeKeychain::with(Some(serde_json::to_string(&original).unwrap()));
    *keychain.denied.lock().unwrap() = true;
    let error = load(Some(&path), ACCOUNT, false, &keychain).err().unwrap();
    assert!(error.starts_with("Cannot read desktop mobile identity: "));
    assert_eq!(rows(&path), 0);
    assert!(!path.parent().unwrap().join(sealed::KEY_FILE).exists());
    *keychain.denied.lock().unwrap() = false;
    let loaded = load(Some(&path), ACCOUNT, false, &keychain).unwrap();
    assert_eq!(fingerprint(&loaded), fingerprint(&original));
    assert_eq!(rows(&path), 1);
}

#[test]
fn invalid_keychain_value_is_an_error() {
    let (_dir, path) = database();
    let keychain = FakeKeychain::with(Some("not json".into()));
    assert!(load(Some(&path), ACCOUNT, false, &keychain).is_err());
    assert_eq!(rows(&path), 0);
    assert_eq!(keychain.deletes.load(Ordering::SeqCst), 0);
}

#[test]
fn paired_devices_without_any_store_do_not_generate() {
    let (_dir, path) = database();
    let keychain = FakeKeychain::default();
    let error = load(Some(&path), ACCOUNT, true, &keychain).err().unwrap();
    assert_eq!(error, MISSING);
    assert_eq!(rows(&path), 0);
    assert!(!path.parent().unwrap().join(sealed::KEY_FILE).exists());
}

#[test]
fn generates_and_persists_when_nothing_exists() {
    let (_dir, path) = database();
    let keychain = FakeKeychain::default();
    let first = load(Some(&path), ACCOUNT, false, &keychain).unwrap();
    assert_eq!(rows(&path), 1);
    assert!(!keychain.has_value());
    let second = load(Some(&path), ACCOUNT, false, &keychain).unwrap();
    assert_eq!(fingerprint(&first), fingerprint(&second));
    assert_eq!(keychain.gets.load(Ordering::SeqCst), 1);
}

#[test]
fn damaged_key_with_a_row_errors_and_never_regenerates() {
    let (_dir, path) = database();
    let keychain = FakeKeychain::default();
    load(Some(&path), ACCOUNT, false, &keychain).unwrap();
    let key = path.parent().unwrap().join(sealed::KEY_FILE);
    std::fs::write(&key, b"short").unwrap();
    assert!(load(Some(&path), ACCOUNT, false, &keychain).is_err());
    assert_eq!(std::fs::read(&key).unwrap(), b"short");
    std::fs::remove_file(&key).unwrap();
    assert!(load(Some(&path), ACCOUNT, false, &keychain).is_err());
    assert!(!key.exists());
    assert_eq!(rows(&path), 1);
    assert_eq!(keychain.gets.load(Ordering::SeqCst), 1);
}

#[test]
fn undecryptable_row_errors() {
    let (_dir, path) = database();
    let keychain = FakeKeychain::default();
    load(Some(&path), ACCOUNT, false, &keychain).unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE mobile_identity SET ciphertext=zeroblob(40)", [])
        .unwrap();
    assert!(load(Some(&path), ACCOUNT, false, &keychain).is_err());
    assert_eq!(keychain.gets.load(Ordering::SeqCst), 1);
}

#[test]
fn unavailable_database_errors_without_generating() {
    let keychain = FakeKeychain::default();
    assert!(load(None, ACCOUNT, false, &keychain).is_err());
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.db");
    assert!(load(Some(&missing), ACCOUNT, false, &keychain).is_err());
    assert!(!missing.exists());
    assert_eq!(keychain.gets.load(Ordering::SeqCst), 0);
}
