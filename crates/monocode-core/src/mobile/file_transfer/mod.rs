//! `file.prepare`: a remote device opens a file through an end-to-end encrypted object in R2
//! instead of streaming its content through the relay.
//!
//! 1. The device asks the desktop for a file (a path, or an asset of an html page).
//! 2. The desktop decides (`mentioned_file::transfer`: Control may read any readable file,
//!    Read only inside the shared projects, the deny-list and symlink rules for everyone) and
//!    hashes the content (SHA-256).
//! 3. It derives the object name from a keyed hash of the content (HMAC under a secret of this desktop) and the key id,
//!    never from the path or the bare content hash,
//!    asks the server whether that object exists and, when it does not, seals the file with a
//!    fresh random key (chunked AES-256-GCM, see [`crypto`]) and uploads the ciphertext.
//! 4. It answers over the encrypted relay channel with the object name, the size and the key.
//!    The device downloads the ciphertext from the server and decrypts it in the browser.
//!
//! A changed file has a new hash, so stale content is never shown; an object the server deleted
//! (a day after it was written) is uploaded again. A second device asking for the same content
//! gets the key kept in the [`keystore::KeyStore`] and no upload happens.
pub mod crypto;
pub mod keystore;

use crate::mobile::policy::{Code, Failure};
use crypto::{hash_reader, object_hash, sealed_len, ObjectKey, SealReader, MAX_PLAINTEXT_BYTES};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    sync::{Arc, LazyLock, Mutex},
};

/// One lock per content hash being published: a second open of the same content (another device, a reopen while the
/// first upload still runs) waits for the first and then finds the object, instead of uploading it a second time.
static FLIGHTS: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// How long a second open of the same content waits for the first one's upload. The host waits 5.5 minutes for the
/// whole call and an upload may take 4 (`UPLOAD_TIMEOUT`), so this plus the upload fits; past it the call is `busy`
/// and the web asks again.
pub const FLIGHT_WAIT: std::time::Duration = std::time::Duration::from_secs(60);

fn flight(hash: &str) -> Arc<Mutex<()>> {
    let mut flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
    // Nobody holds or waits on an entry whose only owner is the map.
    flights.retain(|_, lock| Arc::strong_count(lock) > 1);
    flights.entry(hash.to_owned()).or_default().clone()
}

/// What the server answers to the desktop's object calls.
#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    /// No answer.
    Unreachable,
    /// A non-2xx answer.
    Http(u16),
    /// The body could not be read or was cut short.
    Failed,
}

/// The server's object store as the desktop sees it (the Worker in front of R2). A trait so the
/// rules are tested without a network.
pub trait ObjectStore {
    /// The size of the stored object, or `None` when there is none.
    fn head(&self, object: &str) -> Result<Option<u64>, StoreError>;
    /// Stores `len` bytes read from `body`. An object that already exists is left as it is.
    fn put(&self, object: &str, len: u64, body: &mut dyn Read) -> Result<(), StoreError>;
}

/// The Worker, over the desktop's bearer token.
pub struct HttpStore<'a> {
    pub base: &'a str,
    pub token: &'a str,
}

impl ObjectStore for HttpStore<'_> {
    fn head(&self, object: &str) -> Result<Option<u64>, StoreError> {
        crate::mobile::web_link::desktop_api::transfer_head(self.base, self.token, object)
    }

    fn put(&self, object: &str, len: u64, body: &mut dyn Read) -> Result<(), StoreError> {
        crate::mobile::web_link::desktop_api::transfer_put(self.base, self.token, object, len, body)
    }
}

/// The web server's base URL and this desktop's bearer token, or `None` when it is not paired.
pub fn endpoint(host: &crate::mobile::MobileHost) -> Option<(String, String)> {
    crate::mobile::notify::push_target(host)
}

/// The content to publish.
pub enum Source {
    /// A file already checked and opened by the policy (read again from its start).
    File(std::fs::File),
    /// Bytes made in memory (the sanitized html of a page).
    Bytes(Vec<u8>),
}

impl Source {
    /// The size the content has now (it is hashed again by [`publish`]).
    pub fn len_hint(&self) -> u64 {
        match self {
            Source::File(file) => file.metadata().map_or(0, |meta| meta.len()),
            Source::Bytes(bytes) => bytes.len() as u64,
        }
    }

    fn reader(&mut self) -> std::io::Result<Box<dyn Read + '_>> {
        match self {
            Source::File(file) => {
                file.seek(SeekFrom::Start(0))?;
                Ok(Box::new(file.take(MAX_PLAINTEXT_BYTES + 1)))
            }
            Source::Bytes(bytes) => Ok(Box::new(std::io::Cursor::new(bytes.as_slice()))),
        }
    }
}

/// What a device needs to fetch and open the object.
#[derive(Debug)]
pub struct Published {
    pub hash: String,
    pub size: u64,
    pub object: String,
    pub key: ObjectKey,
    /// The server already held the object: nothing was uploaded.
    pub reused: bool,
}

impl Published {
    /// The part of the `file.prepare` answer that belongs to the transfer.
    pub fn to_json(&self) -> Value {
        json!({
            "hash": self.hash,
            "size": self.size,
            "object": self.object,
            "key": self.key.key_base64().as_str(),
            "nonce": self.key.nonce_prefix_base64(),
            "chunkBytes": crypto::CHUNK_BYTES,
            "reused": self.reused,
        })
    }
}

fn store_failure(error: StoreError) -> Failure {
    match error {
        StoreError::Http(413) => Code::TooLarge.failure(),
        StoreError::Http(429) => Code::Busy.failure(),
        StoreError::Http(401 | 403) => {
            Failure::from("The web server refused this desktop. Pair it again.")
        }
        _ => Failure::from("Could not send the file to the web server"),
    }
}

/// Hashes `source`, then makes sure the server holds it sealed under a key this desktop keeps.
pub fn publish(
    source: &mut Source,
    store: &dyn ObjectStore,
    keys: &keystore::KeyStore,
) -> Result<Published, Failure> {
    publish_waiting(source, store, keys, FLIGHT_WAIT)
}

/// [`publish`] with its own limit for the wait behind an upload of the same content.
pub fn publish_waiting(
    source: &mut Source,
    store: &dyn ObjectStore,
    keys: &keystore::KeyStore,
    flight_wait: std::time::Duration,
) -> Result<Published, Failure> {
    let read_failed = |_| Failure::from("Could not read the file");
    let (digest, size) = hash_reader(source.reader().map_err(read_failed)?, MAX_PLAINTEXT_BYTES)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidData {
                Code::TooLarge.failure()
            } else {
                Failure::from("Could not read the file")
            }
        })?;
    let hash = crypto::hex(&digest);
    let secret = keys
        .name_secret()
        .map_err(|_| Failure::from("Could not keep the file key"))?;
    let name = object_hash(&secret, &hash);
    let lock = flight(&hash);
    let deadline = std::time::Instant::now() + flight_wait;
    let _flight = loop {
        match lock.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(Code::Busy.failure());
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }
    };
    let mut generated = false;
    let mut key = match keys.get(&hash) {
        Some(key) => key,
        None => {
            generated = true;
            fresh_key(keys, &hash)?
        }
    };
    loop {
        let object = format!("{name}.{}", key.key_id());
        let present = store.head(&object).map_err(store_failure)?;
        let reused = present == Some(sealed_len(size));
        if reused || present.is_none() {
            if !reused {
                let reader = source.reader().map_err(read_failed)?;
                let mut sealed = SealReader::new(reader, key.clone(), size, digest);
                store
                    .put(&object, sealed_len(size), &mut sealed)
                    .map_err(store_failure)?;
            }
            return Ok(Published {
                hash,
                size,
                object,
                key,
                reused: reused && !generated,
            });
        }
        // The object exists under this key id with another size: it is not ours to trust.
        // Start again under a key of its own.
        key = fresh_key(keys, &hash)?;
        generated = true;
    }
}

fn fresh_key(keys: &keystore::KeyStore, hash: &str) -> Result<ObjectKey, Failure> {
    let key = ObjectKey::random().map_err(Failure::from)?;
    keys.put(hash, &key)
        .map_err(|_| Failure::from("Could not keep the file key"))?;
    Ok(key)
}

#[cfg(test)]
mod tests;
