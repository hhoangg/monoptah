//! The per-object keys of transferred files, kept on this desktop by content hash.
//!
//! When a second device opens a file that is already in R2, the desktop sends it the same key
//! instead of uploading again, so the key must outlive one request. The file lives next to the
//! other desktop data, is written atomically with owner-only permissions (no Keychain: it must
//! work on Linux too), and holds nothing the server could use: the keys open objects the server
//! deletes after a day. Entries older than [`KEY_TTL`] are dropped. The key never reaches a log.
use super::crypto::ObjectKey;
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

/// The server deletes an object a day after it was written (an R2 lifecycle rule that runs once
/// a day), so a key older than this opens nothing.
pub const KEY_TTL: std::time::Duration = std::time::Duration::from_secs(3 * 24 * 60 * 60);
/// Most keys kept; the oldest go first.
pub const MAX_KEYS: usize = 2048;
pub const FILE_NAME: &str = "file-transfer-keys.json";

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    key: String,
    nonce: String,
    at: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Document {
    version: u32,
    /// Base64 of the 32 byte secret that turns a content hash into an object name (never sent anywhere).
    #[serde(default)]
    secret: Option<String>,
    entries: BTreeMap<String, Entry>,
}

pub struct KeyStore {
    path: PathBuf,
    lock: Mutex<()>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl KeyStore {
    /// The one store of `dir` for the whole process: every request of the app shares its lock, so two files opened
    /// at once cannot overwrite each other's entry (each write reads, changes and replaces the file).
    pub fn shared(dir: &Path) -> Arc<Self> {
        static STORES: LazyLock<Mutex<HashMap<PathBuf, Arc<KeyStore>>>> =
            LazyLock::new(|| Mutex::new(HashMap::new()));
        let mut stores = STORES.lock().unwrap_or_else(|e| e.into_inner());
        stores
            .entry(dir.to_path_buf())
            .or_insert_with(|| Arc::new(Self::new(dir)))
            .clone()
    }

    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
            lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Document {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, document: &Document) -> Result<(), String> {
        let dir = self.path.parent().ok_or("No data folder")?;
        let text = serde_json::to_string(document).map_err(|_| "Could not save keys")?;
        // A temporary file starts owner-only (0600) on unix and is renamed over the old one.
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|_| "Could not save keys")?;
        temp.write_all(text.as_bytes())
            .and_then(|()| temp.flush())
            .map_err(|_| "Could not save keys")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o600));
        }
        temp.persist(&self.path)
            .map_err(|_| "Could not save keys".to_owned())?;
        Ok(())
    }

    /// The key kept for content `hash` (lowercase hex), when it is still fresh.
    pub fn get(&self, hash: &str) -> Option<ObjectKey> {
        self.get_at(hash, now_secs())
    }

    pub(crate) fn get_at(&self, hash: &str, now: u64) -> Option<ObjectKey> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let entry = self.load().entries.get(hash).cloned()?;
        if now.saturating_sub(entry.at) >= KEY_TTL.as_secs() {
            return None;
        }
        ObjectKey::from_base64(&entry.key, &entry.nonce)
    }

    /// Keeps `key` for content `hash`, dropping what is stale or over the cap.
    pub fn put(&self, hash: &str, key: &ObjectKey) -> Result<(), String> {
        self.put_at(hash, key, now_secs())
    }

    pub(crate) fn put_at(&self, hash: &str, key: &ObjectKey, now: u64) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut document = self.load();
        document.version = 1;
        document
            .entries
            .retain(|_, entry| now.saturating_sub(entry.at) < KEY_TTL.as_secs());
        document.entries.insert(
            hash.to_owned(),
            Entry {
                key: key.key_base64().to_string(),
                nonce: key.nonce_prefix_base64(),
                at: now,
            },
        );
        while document.entries.len() > MAX_KEYS {
            let oldest = document
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.at)
                .map(|(hash, _)| hash.clone());
            match oldest {
                Some(hash) => document.entries.remove(&hash),
                None => break,
            };
        }
        self.save(&document)
    }

    /// The secret behind object names, made on first use and kept with the keys.
    pub fn name_secret(&self) -> Result<Zeroizing<Vec<u8>>, String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut document = self.load();
        if let Some(bytes) = document
            .secret
            .as_deref()
            .and_then(|text| STANDARD.decode(text).ok())
            .filter(|bytes| bytes.len() == 32)
        {
            return Ok(Zeroizing::new(bytes));
        }
        let mut bytes = Zeroizing::new(vec![0u8; 32]);
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| "No secure random source".to_owned())?;
        document.version = 1;
        document.secret = Some(STANDARD.encode(bytes.as_slice()));
        self.save(&document)?;
        Ok(bytes)
    }

    /// Forgets the key of `hash` (it did not open what the server holds).
    pub fn forget(&self, hash: &str) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut document = self.load();
        if document.entries.remove(hash).is_some() {
            let _ = self.save(&document);
        }
    }
}
