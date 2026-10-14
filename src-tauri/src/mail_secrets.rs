//! Where AWS key pairs live: on the `mail_credential` row, with the secret
//! encrypted by AES-256-GCM under a random per-install key kept in `mail.key`
//! next to the database. The access key id is not a secret and is stored as is.
//!
//! This replaces the OS keychain, whose prompts (every launch of an unsigned
//! build) made the app unusable. Key pairs a keychain-era install saved are read
//! from it once, on first use, and moved onto the row.
//!
//! The file protects against a copied or backed-up database, not against
//! something already running as the user, which could read the key file too.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aes_gcm::aead::{Aead, KeyInit, OsRng, Payload};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rusqlite::Connection;
use tauri::{AppHandle, Manager};

use crate::mail::{KeyPair, SecretStore};
use crate::mail_store;
use crate::session_store::SessionStore;

const FORMAT_PREFIX: &str = "v1:";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const KEY_FILE: &str = "mail.key";

type RawKey = [u8; KEY_LEN];
/// A row's `(access_key_id, encrypted secret)`; either is `None` until written.
type StoredKeys = (Option<String>, Option<String>);

/// The row and the key id are authenticated with the ciphertext, so a secret
/// copied onto another row, or paired with another access key id, will not open.
fn aad(credential_id: i64, access_key_id: &str) -> String {
    format!("{credential_id}:{access_key_id}")
}

fn seal(
    key: &RawKey,
    credential_id: i64,
    access_key_id: &str,
    plaintext: &str,
) -> Result<String, String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let aad = aad(credential_id, access_key_id);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext.as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "Could not encrypt the AWS key.".to_string())?;
    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    Ok(format!("{FORMAT_PREFIX}{}", STANDARD.encode(blob)))
}

/// `None` for anything that is not a value `seal` produced under this key for
/// this row and key id: a different key, row or id, a damaged value, an unknown
/// format version.
fn open(key: &RawKey, credential_id: i64, access_key_id: &str, stored: &str) -> Option<String> {
    let blob = STANDARD.decode(stored.strip_prefix(FORMAT_PREFIX)?).ok()?;
    if blob.len() <= NONCE_LEN {
        return None;
    }
    let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let aad = aad(credential_id, access_key_id);
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .ok()?;
    String::from_utf8(plaintext).ok()
}

/// The per-install key. It is never logged or put in an error message.
pub(crate) struct KeyFile {
    path: Result<PathBuf, String>,
}

impl KeyFile {
    /// `dir` is the app data directory, which holds `monocode.db` too. The dev
    /// build has its own, so it has its own key.
    pub(crate) fn new(dir: Result<PathBuf, String>) -> Self {
        Self {
            path: dir.map(|dir| dir.join(KEY_FILE)),
        }
    }

    fn path(&self) -> Result<&Path, String> {
        self.path.as_deref().map_err(|error| error.clone())
    }

    /// `None` when there is no key yet. A file that is not exactly a key is an
    /// error and is left alone: replacing it would strand every stored secret.
    fn load(&self) -> Result<Option<RawKey>, String> {
        match std::fs::read(self.path()?) {
            Ok(bytes) => RawKey::try_from(bytes.as_slice()).map(Some).map_err(|_| {
                "The mail key file is not valid, so saved AWS keys cannot be read.".to_string()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Could not read the mail key file.".to_string()),
        }
    }

    fn load_or_create(&self) -> Result<RawKey, String> {
        if let Some(key) = self.load()? {
            return Ok(key);
        }
        let path = self.path()?;
        let fail = |_| "Could not create the mail key file.".to_string();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let key: RawKey = Aes256Gcm::generate_key(OsRng).into();
        // Written in full to a private temp file, then linked into place: the
        // link fails if the key exists, so two racing creators cannot clobber
        // each other and nobody reads a half-written key.
        let temp = path.with_file_name(format!("{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let written = options.open(&temp).and_then(|mut file| {
            file.write_all(&key)?;
            file.sync_all()
        });
        let linked = written.and_then(|()| std::fs::hard_link(&temp, path));
        let _ = std::fs::remove_file(&temp);
        match linked {
            Ok(()) => Ok(key),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => self
                .load()?
                .ok_or_else(|| "Could not create the mail key file.".to_string()),
            Err(error) => Err(fail(error)),
        }
    }
}

/// How the database is reached. The app's handle takes the session store's lock
/// per call; tests hand in a store directly.
pub(crate) trait StoreHost: Send + Sync {
    fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<R>)
        -> Result<R, String>;
}

impl StoreHost for SessionStore {
    fn with_conn<R>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<R>,
    ) -> Result<R, String> {
        f(&*self.lock_conn()?).map_err(|e| e.to_string())
    }
}

impl StoreHost for AppHandle {
    fn with_conn<R>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<R>,
    ) -> Result<R, String> {
        self.try_state::<SessionStore>()
            .ok_or_else(|| "The session store is not available.".to_string())?
            .with_conn(f)
    }
}

impl<T: StoreHost> StoreHost for std::sync::Arc<T> {
    fn with_conn<R>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<R>,
    ) -> Result<R, String> {
        (**self).with_conn(f)
    }
}

/// Where keychain-era installs saved their key pairs. Only read, and cleared
/// after a key pair has been moved; tests use a fake so `cargo test` never
/// touches a real keychain.
pub(crate) trait LegacyKeys: Send + Sync {
    fn read(&self, credential_id: i64) -> Result<Option<KeyPair>, String>;
    fn delete(&self, credential_id: i64) -> Result<(), String>;
}

pub(crate) struct LegacyKeychain {
    service: String,
}

/// The entry names carry the database row id, and the dev build has its own
/// database whose ids restart at 1. A shared service would let a dev read or
/// delete the installed app's key pair, so the service is the running app's
/// identifier. The production identifier is `com.monoptah.desktop`, the string
/// this used to be hard-coded to, so installed apps keep their entries.
pub(crate) fn keychain_service(app_identifier: &str) -> String {
    app_identifier.to_string()
}

impl LegacyKeychain {
    pub(crate) fn new(app_identifier: &str) -> Self {
        Self {
            service: keychain_service(app_identifier),
        }
    }

    fn entry(&self, credential_id: i64, field: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(
            &self.service,
            &format!("mail.aws.cred.{credential_id}.{field}"),
        )
        .map_err(|_| "Could not access the system credential store.".to_string())
    }

    fn read_entry(&self, credential_id: i64, field: &str) -> Result<Option<String>, String> {
        match self.entry(credential_id, field)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err("Could not read the AWS key from the system credential store.".into()),
        }
    }

    fn delete_entry(&self, credential_id: i64, field: &str) -> Result<(), String> {
        match self.entry(credential_id, field)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err("Could not remove the AWS key from the system credential store.".into()),
        }
    }
}

impl LegacyKeys for LegacyKeychain {
    fn read(&self, credential_id: i64) -> Result<Option<KeyPair>, String> {
        let id = self.read_entry(credential_id, "access_key_id")?;
        let secret = self.read_entry(credential_id, "secret_access_key")?;
        Ok(match (id, secret) {
            (Some(access_key_id), Some(secret_access_key)) => Some(KeyPair {
                access_key_id,
                secret_access_key,
            }),
            _ => None,
        })
    }

    fn delete(&self, credential_id: i64) -> Result<(), String> {
        let id = self.delete_entry(credential_id, "access_key_id");
        let secret = self.delete_entry(credential_id, "secret_access_key");
        id.and(secret)
    }
}

/// Key pairs on the credential row. Each call takes the database lock only for
/// its own statement, never across the legacy keychain, which can block on a
/// system prompt while every other command waits on that lock.
pub(crate) struct DbSecrets<H: StoreHost> {
    host: H,
    key_file: KeyFile,
    legacy: Box<dyn LegacyKeys>,
    /// Credential ids whose keychain migration has been tried this run, with
    /// whatever outcome. A denied prompt or a half-present pair must not prompt
    /// again on every sync tick. Held for the whole attempt, so concurrent gets
    /// of one id make one keychain read; the database lock is only ever taken
    /// inside it, never the other way round.
    migrated: Mutex<HashSet<i64>>,
}

impl<H: StoreHost> DbSecrets<H> {
    pub(crate) fn new(host: H, key_file: KeyFile, legacy: Box<dyn LegacyKeys>) -> Self {
        Self {
            host,
            key_file,
            legacy,
            migrated: Mutex::new(HashSet::new()),
        }
    }

    fn read_row(&self, credential_id: i64) -> Result<Option<StoredKeys>, String> {
        self.host
            .with_conn(|conn| mail_store::credential_keys(conn, credential_id))
    }

    /// `None` when the secret cannot be decrypted: no key file, a different
    /// key, a damaged value.
    fn decrypt(
        &self,
        credential_id: i64,
        access_key_id: String,
        encrypted: &str,
    ) -> Result<Option<KeyPair>, String> {
        let Some(key) = self.key_file.load()? else {
            return Ok(None);
        };
        Ok(
            open(&key, credential_id, &access_key_id, encrypted).map(|secret_access_key| KeyPair {
                access_key_id,
                secret_access_key,
            }),
        )
    }

    /// Writes the pair; `false` when the row is gone, or (with `only_if_empty`)
    /// already holds one.
    fn write(
        &self,
        credential_id: i64,
        keys: &KeyPair,
        only_if_empty: bool,
    ) -> Result<bool, String> {
        let key = self.key_file.load_or_create()?;
        let encrypted = seal(
            &key,
            credential_id,
            &keys.access_key_id,
            &keys.secret_access_key,
        )?;
        self.host.with_conn(|conn| {
            mail_store::set_credential_keys(
                conn,
                credential_id,
                &keys.access_key_id,
                &encrypted,
                only_if_empty,
            )
        })
    }

    /// What the row holds now, without any keychain lookup.
    fn current(&self, credential_id: i64) -> Result<Option<KeyPair>, String> {
        match self.read_row(credential_id)? {
            Some((Some(access_key_id), Some(encrypted))) => {
                self.decrypt(credential_id, access_key_id, &encrypted)
            }
            _ => Ok(None),
        }
    }

    /// Moves a keychain-era key pair onto the row. The keychain copy goes only
    /// once the row holds one, and a failure to delete it is not an error. The
    /// keychain is asked at most once per credential per run.
    fn migrate(&self, credential_id: i64) -> Result<Option<KeyPair>, String> {
        let mut migrated = self.migrated.lock().unwrap_or_else(|e| e.into_inner());
        if !migrated.insert(credential_id) {
            // Tried already, here or by a get that held the lock first: take
            // what the row holds now.
            return self.current(credential_id);
        }
        // A keychain that cannot be read (denied, unavailable) is the same as
        // one with nothing in it: the credential has no key and must be re-entered.
        let Ok(Some(keys)) = self.legacy.read(credential_id) else {
            return Ok(None);
        };
        if self.write(credential_id, &keys, true)? {
            let _ = self.legacy.delete(credential_id);
            return Ok(Some(keys));
        }
        // Someone saved a key (or deleted the row) since the first look.
        self.current(credential_id)
    }
}

impl<H: StoreHost> SecretStore for DbSecrets<H> {
    fn put(&self, credential_id: i64, keys: &KeyPair) -> Result<(), String> {
        if self.write(credential_id, keys, false)? {
            Ok(())
        } else {
            Err("That credential no longer exists.".into())
        }
    }

    fn get(&self, credential_id: i64) -> Result<Option<KeyPair>, String> {
        match self.read_row(credential_id)? {
            None => Ok(None),
            Some((Some(access_key_id), Some(encrypted))) => {
                self.decrypt(credential_id, access_key_id, &encrypted)
            }
            // Only a row that exists and holds no key can reach the keychain,
            // so only an un-migrated row can prompt.
            Some(_) => self.migrate(credential_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_sigv4::test_keys::{KEY_ID, OTHER_KEY_ID, SECRET};
    use crate::mail_store::CredentialInput;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("monoptah-mail-key-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn key_file(&self) -> KeyFile {
            KeyFile::new(Ok(self.0.clone()))
        }

        fn key_path(&self) -> PathBuf {
            self.0.join(KEY_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Default)]
    struct FakeLegacy {
        entries: Mutex<HashMap<i64, (String, String)>>,
        reads: AtomicUsize,
        deletes: AtomicUsize,
        fail_read: AtomicBool,
        fail_delete: AtomicBool,
        /// Holds each read open, as a system prompt would.
        read_delay_ms: AtomicUsize,
    }

    impl FakeLegacy {
        fn with(id: i64, key_id: &str, secret: &str) -> Arc<Self> {
            let fake = Arc::new(Self::default());
            fake.entries
                .lock()
                .unwrap()
                .insert(id, (key_id.into(), secret.into()));
            fake
        }
    }

    impl LegacyKeys for Arc<FakeLegacy> {
        fn read(&self, id: i64) -> Result<Option<KeyPair>, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let delay = self.read_delay_ms.load(Ordering::SeqCst) as u64;
            std::thread::sleep(std::time::Duration::from_millis(delay));
            if self.fail_read.load(Ordering::SeqCst) {
                return Err("keychain denied".into());
            }
            Ok(self.entries.lock().unwrap().get(&id).map(|(a, s)| KeyPair {
                access_key_id: a.clone(),
                secret_access_key: s.clone(),
            }))
        }

        fn delete(&self, id: i64) -> Result<(), String> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err("keychain denied".into());
            }
            self.entries.lock().unwrap().remove(&id);
            Ok(())
        }
    }

    fn pair(key_id: &str, secret: &str) -> KeyPair {
        KeyPair {
            access_key_id: key_id.into(),
            secret_access_key: secret.into(),
        }
    }

    fn row(store: &SessionStore, label: &str) -> i64 {
        let conn = store.lock_conn().unwrap();
        mail_store::upsert_credential(
            &conn,
            &CredentialInput {
                provider: "aws".into(),
                label: label.into(),
                access_key_masked: mail_store::mask_access_key_id(KEY_ID),
                default_region: None,
            },
        )
        .unwrap()
    }

    fn raw_row(store: &SessionStore, id: i64) -> (Option<String>, Option<String>) {
        let conn = store.lock_conn().unwrap();
        mail_store::credential_keys(&conn, id).unwrap().unwrap()
    }

    struct Fixture {
        dir: TempDir,
        store: Arc<SessionStore>,
        legacy: Arc<FakeLegacy>,
        secrets: DbSecrets<Arc<SessionStore>>,
    }

    fn fixture_with(legacy: Arc<FakeLegacy>) -> Fixture {
        let dir = TempDir::new();
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let secrets = DbSecrets::new(store.clone(), dir.key_file(), Box::new(legacy.clone()));
        Fixture {
            dir,
            store,
            legacy,
            secrets,
        }
    }

    fn fixture() -> Fixture {
        fixture_with(Arc::new(FakeLegacy::default()))
    }

    #[test]
    fn a_secret_round_trips_and_is_not_stored_in_the_clear() {
        let key = [7u8; KEY_LEN];
        let sealed = seal(&key, 1, KEY_ID, SECRET).unwrap();
        assert!(sealed.starts_with("v1:"));
        assert!(!sealed.contains(SECRET));
        assert_eq!(open(&key, 1, KEY_ID, &sealed).as_deref(), Some(SECRET));
        // A fresh nonce per write: the same secret never seals to the same text.
        assert_ne!(seal(&key, 1, KEY_ID, SECRET).unwrap(), sealed);
    }

    #[test]
    fn a_secret_is_bound_to_its_row_and_access_key_id() {
        let key = [7u8; KEY_LEN];
        let sealed = seal(&key, 1, KEY_ID, SECRET).unwrap();
        assert_eq!(open(&key, 1, KEY_ID, &sealed).as_deref(), Some(SECRET));
        assert!(open(&key, 2, KEY_ID, &sealed).is_none());
        assert!(open(&key, 1, OTHER_KEY_ID, &sealed).is_none());
        // The two halves of the context cannot be shifted across the separator.
        assert!(open(&key, 11, "AKIA", &seal(&key, 1, "1AKIA", SECRET).unwrap()).is_none());
    }

    #[test]
    fn a_ciphertext_moved_to_another_row_reads_as_no_key() {
        let f = fixture();
        let a = row(&f.store, "a");
        let b = row(&f.store, "b");
        f.secrets.put(a, &pair(KEY_ID, SECRET)).unwrap();
        let (_, moved) = raw_row(&f.store, a);
        f.store
            .lock_conn()
            .unwrap()
            .execute(
                "UPDATE mail_credential SET access_key_id = ?2, secret_access_key = ?3 WHERE id = ?1",
                rusqlite::params![b, KEY_ID, moved],
            )
            .unwrap();
        assert!(f.secrets.get(b).unwrap().is_none());
        // Same row, different access key id.
        f.store
            .lock_conn()
            .unwrap()
            .execute(
                "UPDATE mail_credential SET access_key_id = ?2 WHERE id = ?1",
                rusqlite::params![a, OTHER_KEY_ID],
            )
            .unwrap();
        assert!(f.secrets.get(a).unwrap().is_none());
    }

    #[test]
    fn a_wrong_key_or_damaged_value_opens_to_nothing() {
        let key = [7u8; KEY_LEN];
        let sealed = seal(&key, 1, KEY_ID, SECRET).unwrap();
        assert!(open(&[8u8; KEY_LEN], 1, KEY_ID, &sealed).is_none());

        let mut raw = STANDARD.decode(&sealed["v1:".len()..]).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(open(&key, 1, KEY_ID, &format!("v1:{}", STANDARD.encode(&raw))).is_none());
        raw.truncate(NONCE_LEN);
        assert!(open(&key, 1, KEY_ID, &format!("v1:{}", STANDARD.encode(&raw))).is_none());
        assert!(open(&key, 1, KEY_ID, "v1:").is_none());
        assert!(open(&key, 1, KEY_ID, "v1:not base64!").is_none());
        assert!(open(&key, 1, KEY_ID, &sealed.replacen("v1:", "v2:", 1)).is_none());
        assert!(open(&key, 1, KEY_ID, SECRET).is_none());
    }

    #[test]
    fn the_key_file_is_created_private_and_then_reused() {
        let dir = TempDir::new();
        let file = dir.key_file();
        assert!(file.load().unwrap().is_none());
        let first = file.load_or_create().unwrap();
        assert_eq!(std::fs::read(dir.key_path()).unwrap(), first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.key_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(file.load_or_create().unwrap(), first);
        assert_eq!(file.load().unwrap(), Some(first));
        // No temp file is left beside it.
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn a_malformed_key_file_is_an_error_and_is_left_untouched() {
        let dir = TempDir::new();
        std::fs::write(dir.key_path(), b"short").unwrap();
        let file = dir.key_file();
        assert!(file.load().is_err());
        assert!(file.load_or_create().is_err());
        assert_eq!(std::fs::read(dir.key_path()).unwrap(), b"short");
    }

    #[test]
    fn an_unresolvable_data_dir_is_an_error() {
        let file = KeyFile::new(Err("no data dir".into()));
        assert!(file.load().is_err());
        assert!(file.load_or_create().is_err());
    }

    #[test]
    fn put_then_get_returns_the_pair_and_the_row_holds_only_ciphertext() {
        let f = fixture();
        let id = row(&f.store, "a");
        assert!(f.secrets.get(id).unwrap().is_none());
        f.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        let got = f.secrets.get(id).unwrap().unwrap();
        assert_eq!(got.access_key_id, KEY_ID);
        assert_eq!(got.secret_access_key, SECRET);

        let (stored_id, stored_secret) = raw_row(&f.store, id);
        assert_eq!(stored_id.as_deref(), Some(KEY_ID));
        let stored_secret = stored_secret.unwrap();
        assert!(stored_secret.starts_with("v1:"));
        assert!(!stored_secret.contains(SECRET));
    }

    #[test]
    fn put_overwrites_the_previous_pair() {
        let f = fixture();
        let id = row(&f.store, "a");
        f.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        f.secrets
            .put(id, &pair(OTHER_KEY_ID, "another-secret-value-0123456789"))
            .unwrap();
        let got = f.secrets.get(id).unwrap().unwrap();
        assert_eq!(got.access_key_id, OTHER_KEY_ID);
        assert_eq!(got.secret_access_key, "another-secret-value-0123456789");
    }

    #[test]
    fn put_for_a_missing_row_is_an_error_and_get_finds_nothing() {
        let f = fixture();
        assert!(f.secrets.put(99, &pair(KEY_ID, SECRET)).is_err());
        assert!(f.secrets.get(99).unwrap().is_none());
        // A row that does not exist never reaches the keychain.
        assert_eq!(f.legacy.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn deleting_the_credential_takes_its_secret_with_it() {
        let f = fixture();
        let id = row(&f.store, "a");
        f.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        crate::mail::credential_delete(&f.store, id).unwrap();
        assert!(f.secrets.get(id).unwrap().is_none());
        let conn = f.store.lock_conn().unwrap();
        assert!(mail_store::credential_keys(&conn, id).unwrap().is_none());
    }

    #[test]
    fn a_missing_key_file_leaves_the_credential_without_a_key() {
        let f = fixture();
        let id = row(&f.store, "a");
        f.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        std::fs::remove_file(f.dir.key_path()).unwrap();
        assert!(f.secrets.get(id).unwrap().is_none());
        // Reading never invents a key: that would hide the loss.
        assert!(!f.dir.key_path().exists());
    }

    #[test]
    fn a_different_key_or_damaged_ciphertext_reads_as_no_key() {
        let f = fixture();
        let id = row(&f.store, "a");
        f.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        std::fs::write(f.dir.key_path(), [9u8; KEY_LEN]).unwrap();
        assert!(f.secrets.get(id).unwrap().is_none());

        let other = row(&f.store, "b");
        f.secrets.put(other, &pair(KEY_ID, SECRET)).unwrap();
        f.store
            .lock_conn()
            .unwrap()
            .execute(
                "UPDATE mail_credential SET secret_access_key = 'v1:AAAA' WHERE id = ?1",
                [other],
            )
            .unwrap();
        assert!(f.secrets.get(other).unwrap().is_none());
        // Nothing here consults the keychain: the row has a (broken) key.
        assert_eq!(f.legacy.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_malformed_key_file_fails_get_and_put_without_replacing_it() {
        let f = fixture();
        let id = row(&f.store, "a");
        f.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        std::fs::write(f.dir.key_path(), b"garbage").unwrap();
        assert!(f.secrets.get(id).is_err());
        assert!(f.secrets.put(id, &pair(KEY_ID, SECRET)).is_err());
        assert_eq!(std::fs::read(f.dir.key_path()).unwrap(), b"garbage");
    }

    #[test]
    fn a_keychain_era_pair_moves_onto_the_row_and_out_of_the_keychain() {
        let f = fixture_with(FakeLegacy::with(1, KEY_ID, SECRET));
        let id = row(&f.store, "a");
        assert_eq!(id, 1);

        let got = f.secrets.get(id).unwrap().unwrap();
        assert_eq!(got.access_key_id, KEY_ID);
        assert_eq!(got.secret_access_key, SECRET);
        assert!(f.legacy.entries.lock().unwrap().is_empty());
        let (stored_id, stored_secret) = raw_row(&f.store, id);
        assert_eq!(stored_id.as_deref(), Some(KEY_ID));
        assert!(stored_secret.unwrap().starts_with("v1:"));

        // The second read is served from the row alone.
        let reads = f.legacy.reads.load(Ordering::SeqCst);
        let again = f.secrets.get(id).unwrap().unwrap();
        assert_eq!(again.secret_access_key, SECRET);
        assert_eq!(f.legacy.reads.load(Ordering::SeqCst), reads);
    }

    #[test]
    fn a_failed_keychain_delete_does_not_lose_the_migrated_pair() {
        let f = fixture_with(FakeLegacy::with(1, KEY_ID, SECRET));
        f.legacy.fail_delete.store(true, Ordering::SeqCst);
        let id = row(&f.store, "a");
        assert!(f.secrets.get(id).unwrap().is_some());
        assert_eq!(f.legacy.deletes.load(Ordering::SeqCst), 1);
        assert_eq!(
            f.secrets.get(id).unwrap().unwrap().secret_access_key,
            SECRET
        );
    }

    #[test]
    fn no_keychain_entries_means_no_key() {
        let f = fixture();
        let id = row(&f.store, "a");
        assert!(f.secrets.get(id).unwrap().is_none());
        assert_eq!(f.legacy.reads.load(Ordering::SeqCst), 1);
        // Nothing was written, and no key file was made for a lookup.
        assert_eq!(raw_row(&f.store, id), (None, None));
        assert!(!f.dir.key_path().exists());
    }

    #[test]
    fn an_unreadable_keychain_means_no_key_and_nothing_is_written() {
        let f = fixture_with(FakeLegacy::with(1, KEY_ID, SECRET));
        f.legacy.fail_read.store(true, Ordering::SeqCst);
        let id = row(&f.store, "a");
        assert!(f.secrets.get(id).unwrap().is_none());
        assert_eq!(raw_row(&f.store, id), (None, None));
        assert_eq!(f.legacy.entries.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_denied_or_empty_keychain_is_asked_once_per_run() {
        // Denied.
        let f = fixture_with(FakeLegacy::with(1, KEY_ID, SECRET));
        f.legacy.fail_read.store(true, Ordering::SeqCst);
        let id = row(&f.store, "a");
        for _ in 0..3 {
            assert!(f.secrets.get(id).unwrap().is_none());
        }
        assert_eq!(f.legacy.reads.load(Ordering::SeqCst), 1);

        // Nothing there (or only half of a pair, which reads as nothing).
        let g = fixture();
        let id = row(&g.store, "a");
        for _ in 0..3 {
            assert!(g.secrets.get(id).unwrap().is_none());
        }
        assert_eq!(g.legacy.reads.load(Ordering::SeqCst), 1);

        // A key entered afterwards is still read from the row.
        g.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        assert_eq!(
            g.secrets.get(id).unwrap().unwrap().secret_access_key,
            SECRET
        );
        assert_eq!(g.legacy.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_gets_of_an_unmigrated_credential_ask_the_keychain_once() {
        let f = fixture_with(FakeLegacy::with(1, KEY_ID, SECRET));
        f.legacy.read_delay_ms.store(100, Ordering::SeqCst);
        let id = row(&f.store, "a");
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| f.secrets.get(id).unwrap()))
                .collect();
            for handle in handles {
                let got = handle.join().unwrap().unwrap();
                assert_eq!(got.secret_access_key, SECRET);
            }
        });
        assert_eq!(f.legacy.reads.load(Ordering::SeqCst), 1);
        assert_eq!(f.legacy.deletes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_pair_saved_after_the_first_look_is_not_overwritten_by_the_migration() {
        let f = fixture_with(FakeLegacy::with(1, KEY_ID, SECRET));
        let id = row(&f.store, "a");
        // The row gains a key after `get` saw it empty and before it wrote.
        let key = f.dir.key_file().load_or_create().unwrap();
        let newer = seal(&key, id, OTHER_KEY_ID, "newer-secret-value-0123456789").unwrap();
        {
            let conn = f.store.lock_conn().unwrap();
            assert!(
                mail_store::set_credential_keys(&conn, id, OTHER_KEY_ID, &newer, false).unwrap()
            );
            assert!(!mail_store::set_credential_keys(&conn, id, KEY_ID, "v1:old", true).unwrap());
        }
        let got = f.secrets.get(id).unwrap().unwrap();
        assert_eq!(got.access_key_id, OTHER_KEY_ID);
    }

    #[test]
    fn credential_listings_never_carry_the_key_pair() {
        let f = fixture();
        let id = row(&f.store, "a");
        f.secrets.put(id, &pair(KEY_ID, SECRET)).unwrap();
        let conn = f.store.lock_conn().unwrap();
        let listed = mail_store::list_credentials(&conn).unwrap();
        let json = serde_json::to_string(&listed).unwrap();
        assert!(!json.contains(SECRET));
        assert!(!json.contains(KEY_ID));
        assert!(!json.contains("v1:"));
        assert!(!format!("{listed:?}").contains(SECRET));
        let one = mail_store::get_credential(&conn, id).unwrap().unwrap();
        assert!(!serde_json::to_string(&one).unwrap().contains(SECRET));
    }

    #[test]
    fn the_keychain_service_follows_the_app_identifier_and_production_keeps_its_old_one() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let production = conf["identifier"].as_str().unwrap();
        // The string every installed app already wrote its AWS keys under.
        assert_eq!(keychain_service(production), "com.monoptah.desktop");
        let dev: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.dev.conf.json")).unwrap();
        assert_ne!(
            keychain_service(dev["identifier"].as_str().unwrap()),
            keychain_service(production)
        );
    }
}
