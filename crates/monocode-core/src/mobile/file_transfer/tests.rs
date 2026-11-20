use super::crypto::{
    chunk_count, hash_reader, hex, hmac_sha256, object_hash, sealed_len, ObjectKey, SealReader,
    CHUNK_BYTES, MAX_PLAINTEXT_BYTES, TAG_BYTES,
};
use super::keystore::{KeyStore, KEY_TTL};
use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Mutex;

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

fn seal(key: &ObjectKey, plain: &[u8]) -> Vec<u8> {
    let mut sealed = Vec::new();
    SealReader::new(
        Cursor::new(plain.to_vec()),
        key.clone(),
        plain.len() as u64,
        digest(plain),
    )
    .read_to_end(&mut sealed)
    .unwrap();
    sealed
}

#[test]
fn every_size_round_trips_and_has_the_announced_sealed_length() {
    let key = ObjectKey::random().unwrap();
    for len in [
        0,
        1,
        CHUNK_BYTES - 1,
        CHUNK_BYTES,
        CHUNK_BYTES + 1,
        3 * CHUNK_BYTES + 5,
    ] {
        let plain = pattern(len);
        let sealed = seal(&key, &plain);
        assert_eq!(sealed.len() as u64, sealed_len(len as u64), "len {len}");
        assert_eq!(
            key.open_all(&sealed).as_deref(),
            Some(plain.as_slice()),
            "len {len}"
        );
    }
    assert_eq!(chunk_count(0), 1);
    assert_eq!(chunk_count(CHUNK_BYTES as u64), 1);
    assert_eq!(chunk_count(CHUNK_BYTES as u64 + 1), 2);
}

#[test]
fn truncation_reordering_and_tampering_are_detected() {
    let key = ObjectKey::random().unwrap();
    let step = CHUNK_BYTES + TAG_BYTES as usize;
    let plain = pattern(3 * CHUNK_BYTES + 10);
    let sealed = seal(&key, &plain);
    // A dropped final chunk: the one before it is not marked final.
    assert!(key.open_all(&sealed[..3 * step]).is_none());
    // A dropped first chunk.
    assert!(key.open_all(&sealed[step..]).is_none());
    // Two chunks swapped.
    let mut swapped = sealed.clone();
    let (a, b) = (sealed[..step].to_vec(), sealed[step..2 * step].to_vec());
    swapped[..step].copy_from_slice(&b);
    swapped[step..2 * step].copy_from_slice(&a);
    assert!(key.open_all(&swapped).is_none());
    // One flipped bit.
    let mut flipped = sealed.clone();
    flipped[step + 7] ^= 1;
    assert!(key.open_all(&flipped).is_none());
    // Another key.
    assert!(ObjectKey::random().unwrap().open_all(&sealed).is_none());
    // Appended bytes.
    let mut longer = sealed.clone();
    longer.extend([0u8; 20]);
    assert!(key.open_all(&longer).is_none());
}

#[test]
fn a_chunk_is_not_valid_as_the_final_one_nor_at_another_index() {
    let key = ObjectKey::random().unwrap();
    let sealed = key.seal_chunk(0, false, b"data").unwrap();
    assert!(key.open_chunk(0, true, &sealed).is_none());
    assert!(key.open_chunk(1, false, &sealed).is_none());
    assert_eq!(
        key.open_chunk(0, false, &sealed).as_deref(),
        Some(&b"data"[..])
    );
    // The same plaintext at two indexes never gives the same ciphertext (unique nonces).
    assert_ne!(sealed, key.seal_chunk(1, false, b"data").unwrap());
}

#[test]
fn keys_are_random_and_never_printed() {
    let (a, b) = (ObjectKey::random().unwrap(), ObjectKey::random().unwrap());
    assert_ne!(*a.key_base64(), *b.key_base64());
    assert_ne!(a.key_id(), b.key_id());
    assert_eq!(a.key_id().len(), 16);
    let shown = format!("{a:?}");
    assert!(!shown.contains(a.key_base64().as_str()));
    // The key id is a hash, not the key.
    assert!(!a.key_base64().contains(&a.key_id()));
}

#[test]
fn a_file_that_changes_while_it_is_sealed_fails_instead_of_being_stored() {
    let key = ObjectKey::random().unwrap();
    let plain = pattern(CHUNK_BYTES + 100);
    let announced = digest(&plain);
    let len = plain.len() as u64;
    // Same length, other content.
    let mut other = plain.clone();
    other[10] ^= 0xff;
    let mut out = Vec::new();
    let error = SealReader::new(Cursor::new(other), key.clone(), len, announced)
        .read_to_end(&mut out)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    // Grown.
    let mut grown = plain.clone();
    grown.push(1);
    assert!(
        SealReader::new(Cursor::new(grown), key.clone(), len, announced)
            .read_to_end(&mut Vec::new())
            .is_err()
    );
    // Shrunk.
    assert!(SealReader::new(
        Cursor::new(plain[..plain.len() - 1].to_vec()),
        key,
        len,
        announced
    )
    .read_to_end(&mut Vec::new())
    .is_err());
}

#[test]
fn hashing_refuses_more_than_the_limit() {
    assert!(hash_reader(Cursor::new(vec![0u8; 11]), 10).is_err());
    let (hash, len) = hash_reader(Cursor::new(b"abc".to_vec()), 10).unwrap();
    assert_eq!(len, 3);
    assert_eq!(
        hex(&hash),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn the_key_store_keeps_a_key_by_hash_with_owner_only_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let store = KeyStore::new(dir.path());
    let key = ObjectKey::random().unwrap();
    assert!(store.get("aa").is_none());
    store.put_at("aa", &key, 1_000).unwrap();
    let back = store.get_at("aa", 1_001).unwrap();
    assert_eq!(*back.key_base64(), *key.key_base64());
    assert_eq!(back.nonce_prefix_base64(), key.nonce_prefix_base64());
    assert_eq!(back.key_id(), key.key_id());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(super::keystore::FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "group and others have no access");
    }
    // Stale after the TTL, and pruned by the next write.
    assert!(store.get_at("aa", 1_000 + KEY_TTL.as_secs()).is_none());
    store.put_at("bb", &key, 1_000 + KEY_TTL.as_secs()).unwrap();
    let text = std::fs::read_to_string(dir.path().join(super::keystore::FILE_NAME)).unwrap();
    assert!(!text.contains("\"aa\""));
    store.forget("bb");
    assert!(store.get_at("bb", 1_000 + KEY_TTL.as_secs()).is_none());
}

#[test]
fn a_damaged_key_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(super::keystore::FILE_NAME), "not json").unwrap();
    let store = KeyStore::new(dir.path());
    assert!(store.get("aa").is_none());
    store.put("aa", &ObjectKey::random().unwrap()).unwrap();
    assert!(store.get("aa").is_some());
}

/// An in-memory server that keeps what it is given and never overwrites.
#[derive(Default)]
struct FakeStore {
    objects: Mutex<HashMap<String, Vec<u8>>>,
    puts: Mutex<u32>,
    heads: Mutex<u32>,
}

impl ObjectStore for FakeStore {
    fn head(&self, object: &str) -> Result<Option<u64>, StoreError> {
        *self.heads.lock().unwrap() += 1;
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(object)
            .map(|o| o.len() as u64))
    }

    fn put(&self, object: &str, len: u64, body: &mut dyn Read) -> Result<(), StoreError> {
        let mut data = Vec::new();
        body.read_to_end(&mut data)
            .map_err(|_| StoreError::Failed)?;
        if data.len() as u64 != len {
            return Err(StoreError::Failed);
        }
        *self.puts.lock().unwrap() += 1;
        self.objects
            .lock()
            .unwrap()
            .entry(object.to_owned())
            .or_insert(data);
        Ok(())
    }
}

fn bytes_source(plain: &[u8]) -> Source {
    Source::Bytes(plain.to_vec())
}

#[test]
fn publishing_names_the_object_by_content_never_by_path_and_dedupes() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(dir.path());
    let store = FakeStore::default();
    let plain = pattern(CHUNK_BYTES + 3);
    let first = publish(&mut bytes_source(&plain), &store, &keys).unwrap();
    assert!(!first.reused);
    assert_eq!(first.hash, hex(&digest(&plain)));
    // The name is a keyed hash of the content, not the content hash itself.
    let (name, key_id) = first.object.split_once('.').unwrap();
    assert_ne!(name, first.hash);
    assert_eq!(key_id, first.key.key_id());
    assert_eq!(name, object_hash(&keys.name_secret().unwrap(), &first.hash));
    assert_eq!(*store.puts.lock().unwrap(), 1);
    // What the server holds opens with the key and nothing else.
    let held = store
        .objects
        .lock()
        .unwrap()
        .get(&first.object)
        .cloned()
        .unwrap();
    assert_eq!(first.key.open_all(&held).unwrap(), plain);
    assert!(
        !held.windows(8).any(|w| w == &plain[100..108]),
        "ciphertext, not plaintext"
    );
    // A second device opens the same content: same key, no upload.
    let second = publish(&mut bytes_source(&plain), &store, &keys).unwrap();
    assert!(second.reused);
    assert_eq!(second.object, first.object);
    assert_eq!(*second.key.key_base64(), *first.key.key_base64());
    assert_eq!(*store.puts.lock().unwrap(), 1);
}

#[test]
fn a_changed_file_gets_a_new_object() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(dir.path());
    let store = FakeStore::default();
    let one = publish(&mut bytes_source(b"version one"), &store, &keys).unwrap();
    let two = publish(&mut bytes_source(b"version two"), &store, &keys).unwrap();
    assert_ne!(one.hash, two.hash);
    assert_ne!(one.object, two.object);
    assert_ne!(*one.key.key_base64(), *two.key.key_base64());
    assert_eq!(*store.puts.lock().unwrap(), 2);
}

#[test]
fn an_object_the_server_deleted_is_uploaded_again_under_the_kept_key() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(dir.path());
    let store = FakeStore::default();
    let first = publish(&mut bytes_source(b"report"), &store, &keys).unwrap();
    store.objects.lock().unwrap().clear();
    let again = publish(&mut bytes_source(b"report"), &store, &keys).unwrap();
    assert!(!again.reused);
    assert_eq!(again.object, first.object);
    assert_eq!(*store.puts.lock().unwrap(), 2);
}

#[test]
fn a_lost_key_means_a_new_key_and_a_new_object_next_to_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(dir.path());
    let store = FakeStore::default();
    let first = publish(&mut bytes_source(b"report"), &store, &keys).unwrap();
    keys.forget(&first.hash);
    let again = publish(&mut bytes_source(b"report"), &store, &keys).unwrap();
    assert!(!again.reused);
    assert_ne!(again.object, first.object);
    assert_eq!(store.objects.lock().unwrap().len(), 2);
    // The new key opens the new object.
    let held = store
        .objects
        .lock()
        .unwrap()
        .get(&again.object)
        .cloned()
        .unwrap();
    assert_eq!(again.key.open_all(&held).unwrap(), b"report");
}

#[test]
fn a_file_over_the_cap_is_refused_before_anything_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(dir.path());
    let store = FakeStore::default();
    let big = tempfile::tempfile().unwrap();
    big.set_len(MAX_PLAINTEXT_BYTES + 1).unwrap();
    let failure = publish(&mut Source::File(big), &store, &keys).unwrap_err();
    assert_eq!(
        crate::mobile::policy::code_of_failure(&failure),
        Some(Code::TooLarge)
    );
    assert_eq!(*store.heads.lock().unwrap(), 0);
    assert_eq!(*store.puts.lock().unwrap(), 0);
}

#[test]
fn a_file_exactly_at_the_cap_is_accepted_by_the_hash_step() {
    let big = tempfile::tempfile().unwrap();
    big.set_len(MAX_PLAINTEXT_BYTES).unwrap();
    let (_, len) = hash_reader(&big, MAX_PLAINTEXT_BYTES).unwrap();
    assert_eq!(len, MAX_PLAINTEXT_BYTES);
}

#[test]
fn a_server_refusal_becomes_a_coded_failure() {
    struct Refuses(u16);
    impl ObjectStore for Refuses {
        fn head(&self, _: &str) -> Result<Option<u64>, StoreError> {
            Err(StoreError::Http(self.0))
        }
        fn put(&self, _: &str, _: u64, _: &mut dyn Read) -> Result<(), StoreError> {
            Err(StoreError::Http(self.0))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(dir.path());
    let code = |status| {
        let failure = publish(&mut bytes_source(b"x"), &Refuses(status), &keys).unwrap_err();
        crate::mobile::policy::code_of_failure(&failure)
    };
    assert_eq!(code(413), Some(Code::TooLarge));
    assert_eq!(code(429), Some(Code::Busy));
    assert_eq!(code(500), None);
}

#[test]
fn the_reply_carries_the_key_and_no_path() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::new(dir.path());
    let store = FakeStore::default();
    let published = publish(&mut bytes_source(b"hello"), &store, &keys).unwrap();
    let json = published.to_json();
    assert_eq!(json["size"], 5);
    assert_eq!(json["chunkBytes"], CHUNK_BYTES);
    assert_eq!(json["object"], published.object.as_str());
    assert!(json["key"].as_str().is_some_and(|key| key.len() == 44));
    assert!(json["nonce"].as_str().is_some());
    assert!(json.get("path").is_none());
}

/// The same object the browser tests open (`web/app/src/features/filePreview/transfer/decrypt.test.ts`): key bytes
/// 1 to 32, nonce prefix bytes 200 to 207, one chunk.
const WEB_VECTOR_PLAIN: &str = "Hello from the desktop, sealed in R2.\n";
const WEB_VECTOR_SEALED_BASE64: &str =
    "cfviAsesskP7d6Fggrd6l4QMavUXjLw4b+jUas5ZJy9SLC+GDzt6IDzx6wk5MIxIXUywDxjm";

#[test]
fn the_web_vector_opens() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let key = ObjectKey::from_parts(
        std::array::from_fn(|i| i as u8 + 1),
        std::array::from_fn(|i| i as u8 + 200),
    );
    let sealed = seal(&key, WEB_VECTOR_PLAIN.as_bytes());
    assert_eq!(STANDARD.encode(&sealed), WEB_VECTOR_SEALED_BASE64);
    let held = STANDARD.decode(WEB_VECTOR_SEALED_BASE64).unwrap();
    assert_eq!(key.open_all(&held).unwrap(), WEB_VECTOR_PLAIN.as_bytes());
}

#[test]
fn hmac_sha256_matches_the_rfc_4231_vectors() {
    assert_eq!(
        hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_eq!(
        hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    // A key longer than the block is hashed first (test case 6).
    assert_eq!(
        hex(&hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

#[test]
fn object_names_depend_on_a_secret_of_this_desktop_and_are_stable_on_it() {
    let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (KeyStore::new(one.path()), KeyStore::new(two.path()));
    let content = hex(&digest(b"the same file"));
    let name_a = object_hash(&a.name_secret().unwrap(), &content);
    // Stable across calls and across a new instance on the same folder (a restart).
    assert_eq!(name_a, object_hash(&a.name_secret().unwrap(), &content));
    assert_eq!(
        name_a,
        object_hash(&KeyStore::new(one.path()).name_secret().unwrap(), &content)
    );
    // Another desktop (or anyone who only knows the content) gets another name.
    assert_ne!(name_a, object_hash(&b.name_secret().unwrap(), &content));
    assert_ne!(name_a, content);
    // Keys put later do not lose the secret.
    a.put(&content, &ObjectKey::random().unwrap()).unwrap();
    assert_eq!(name_a, object_hash(&a.name_secret().unwrap(), &content));
}

#[test]
fn concurrent_puts_on_the_shared_store_lose_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let hashes: Vec<String> = (0..24).map(|i| hex(&digest(&[i as u8]))).collect();
    let path = dir.path();
    std::thread::scope(|scope| {
        for hash in &hashes {
            scope.spawn(move || {
                let store = KeyStore::shared(path);
                store.put(hash, &ObjectKey::random().unwrap()).unwrap();
            });
        }
    });
    let store = KeyStore::shared(dir.path());
    for hash in &hashes {
        assert!(store.get(hash).is_some(), "lost {hash}");
    }
    // One instance per folder.
    assert!(std::sync::Arc::ptr_eq(
        &store,
        &KeyStore::shared(dir.path())
    ));
}

/// A server that takes its time to store, so two uploads of one content would overlap.
struct SlowStore(FakeStore);

impl ObjectStore for SlowStore {
    fn head(&self, object: &str) -> Result<Option<u64>, StoreError> {
        self.0.head(object)
    }
    fn put(&self, object: &str, len: u64, body: &mut dyn Read) -> Result<(), StoreError> {
        std::thread::sleep(std::time::Duration::from_millis(150));
        self.0.put(object, len, body)
    }
}

#[test]
fn concurrent_prepares_of_the_same_content_upload_it_once() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::shared(dir.path());
    let store = SlowStore(FakeStore::default());
    let plain = pattern(CHUNK_BYTES + 5);
    let published: Vec<Published> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| scope.spawn(|| publish(&mut bytes_source(&plain), &store, &keys).unwrap()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(*store.0.puts.lock().unwrap(), 1);
    assert_eq!(published.iter().filter(|p| !p.reused).count(), 1);
    assert!(published.iter().all(|p| p.object == published[0].object));
}

#[test]
fn a_second_open_waits_a_bounded_time_for_the_first_upload_and_then_is_busy() {
    let dir = tempfile::tempdir().unwrap();
    let keys = KeyStore::shared(dir.path());
    let store = FakeStore::default();
    let plain = pattern(100);
    // Another call is uploading this very content.
    let hash = hex(&digest(&plain));
    let busy_lock = flight(&hash);
    let held = busy_lock.lock().unwrap();
    let started = std::time::Instant::now();
    let failure = publish_waiting(
        &mut bytes_source(&plain),
        &store,
        &keys,
        std::time::Duration::from_millis(200),
    )
    .unwrap_err();
    assert_eq!(
        crate::mobile::policy::code_of_failure(&failure),
        Some(Code::Busy)
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(200));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(*store.puts.lock().unwrap(), 0);
    // Once it is done, the same call goes through.
    drop(held);
    assert!(publish_waiting(&mut bytes_source(&plain), &store, &keys, FLIGHT_WAIT).is_ok());
    // The wait fits inside the host's reply wait together with an upload.
    assert!(
        FLIGHT_WAIT + std::time::Duration::from_secs(4 * 60)
            < std::time::Duration::from_secs(5 * 60 + 30)
    );
}
