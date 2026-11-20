//! The encryption of a transferred file: AES-256-GCM in independent chunks.
//!
//! The object stored in R2 is the plain concatenation of the sealed chunks, so the browser can
//! fetch any chunk with a range request and decrypt it alone:
//!
//! - plaintext chunk `i` is [`CHUNK_BYTES`] long (the last one is shorter, or exactly full);
//! - its nonce is the 8 random bytes of [`ObjectKey::nonce_prefix`] followed by `i` as a
//!   big-endian `u32`, so no nonce repeats under one key (the key is random per object, so it is
//!   never reused across objects either);
//! - its associated data is `"mt1"`, `i` as a big-endian `u32` and one byte that is `1` for the
//!   final chunk: a chunk moved to another position, a dropped tail and a chunk taken for the
//!   final one all fail the tag check;
//! - an empty file is one empty final chunk (a bare tag).
//!
//! The key and the nonce prefix travel only inside the end-to-end encrypted relay channel. The
//! server sees ciphertext and its length.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::TryRngCore;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use zeroize::Zeroizing;

/// Plaintext bytes in every chunk but the last.
pub const CHUNK_BYTES: usize = 1024 * 1024;
/// The GCM tag every sealed chunk carries.
pub const TAG_BYTES: u64 = 16;
/// The random part of every nonce.
pub const NONCE_PREFIX_BYTES: usize = 8;
/// The largest file the transfer takes (the Worker accepts its sealed size, see
/// `web/worker/src/modules/transfers`). Documented in `web/docs/security.md`.
pub const MAX_PLAINTEXT_BYTES: u64 = 64 * 1024 * 1024;

const AAD_TAG: &[u8; 3] = b"mt1";

/// The number of chunks a file of `plain` bytes is sealed in (at least one).
pub fn chunk_count(plain: u64) -> u64 {
    plain.div_ceil(CHUNK_BYTES as u64).max(1)
}

/// The size of the stored object for a file of `plain` bytes.
pub fn sealed_len(plain: u64) -> u64 {
    plain + chunk_count(plain) * TAG_BYTES
}

/// The random key and nonce prefix of one object.
#[derive(Clone)]
pub struct ObjectKey {
    key: Zeroizing<[u8; 32]>,
    nonce_prefix: [u8; NONCE_PREFIX_BYTES],
}

impl std::fmt::Debug for ObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectKey")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl ObjectKey {
    /// A fresh key from the operating system's random source.
    pub fn random() -> Result<Self, String> {
        let mut key = Zeroizing::new([0u8; 32]);
        let mut nonce_prefix = [0u8; NONCE_PREFIX_BYTES];
        let mut os = rand::rngs::OsRng;
        os.try_fill_bytes(key.as_mut_slice())
            .and_then(|()| os.try_fill_bytes(&mut nonce_prefix))
            .map_err(|_| "No secure random source".to_owned())?;
        Ok(Self { key, nonce_prefix })
    }

    pub fn from_parts(key: [u8; 32], nonce_prefix: [u8; NONCE_PREFIX_BYTES]) -> Self {
        Self {
            key: Zeroizing::new(key),
            nonce_prefix,
        }
    }

    /// From the base64 text the key store keeps, or `None` when it is not a key.
    pub fn from_base64(key: &str, nonce_prefix: &str) -> Option<Self> {
        let key: [u8; 32] = STANDARD.decode(key).ok()?.try_into().ok()?;
        let nonce_prefix: [u8; NONCE_PREFIX_BYTES] =
            STANDARD.decode(nonce_prefix).ok()?.try_into().ok()?;
        Some(Self::from_parts(key, nonce_prefix))
    }

    pub fn key_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(STANDARD.encode(self.key.as_slice()))
    }

    pub fn nonce_prefix_base64(&self) -> String {
        STANDARD.encode(self.nonce_prefix)
    }

    /// 16 hex characters that name this key inside an object name. It is a hash of the key, so
    /// it does not give the key away, and a re-upload under a new key gets a new object name.
    pub fn key_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"monoptah-transfer-key-id");
        hasher.update(self.key.as_slice());
        hex(&hasher.finalize()[..8])
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(self.key.as_slice()).expect("a 32 byte key")
    }

    fn nonce(&self, index: u32) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..NONCE_PREFIX_BYTES].copy_from_slice(&self.nonce_prefix);
        nonce[NONCE_PREFIX_BYTES..].copy_from_slice(&index.to_be_bytes());
        nonce
    }

    fn aad(index: u32, last: bool) -> [u8; 8] {
        let mut aad = [0u8; 8];
        aad[..3].copy_from_slice(AAD_TAG);
        aad[3..7].copy_from_slice(&index.to_be_bytes());
        aad[7] = u8::from(last);
        aad
    }

    /// Seals one chunk.
    pub fn seal_chunk(&self, index: u32, last: bool, plain: &[u8]) -> Result<Vec<u8>, String> {
        self.cipher()
            .encrypt(
                Nonce::from_slice(&self.nonce(index)),
                Payload {
                    msg: plain,
                    aad: &Self::aad(index, last),
                },
            )
            .map_err(|_| "Could not encrypt the file".to_owned())
    }

    /// Opens one chunk, or `None` when it was altered, moved or is not the final one it claims.
    pub fn open_chunk(&self, index: u32, last: bool, sealed: &[u8]) -> Option<Vec<u8>> {
        self.cipher()
            .decrypt(
                Nonce::from_slice(&self.nonce(index)),
                Payload {
                    msg: sealed,
                    aad: &Self::aad(index, last),
                },
            )
            .ok()
    }

    /// Opens a whole object (tests and tools; the browser does the same chunk by chunk).
    pub fn open_all(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        let step = CHUNK_BYTES + TAG_BYTES as usize;
        let count = sealed.len().div_ceil(step).max(1);
        let mut out = Vec::with_capacity(sealed.len());
        for index in 0..count {
            let from = index * step;
            let to = (from + step).min(sealed.len());
            let last = index + 1 == count;
            out.extend(self.open_chunk(u32::try_from(index).ok()?, last, sealed.get(from..to)?)?);
        }
        Some(out)
    }
}

/// HMAC-SHA256 (RFC 2104) of `message` under `key`.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let inner_pad: Vec<u8> = padded.iter().map(|b| b ^ 0x36).collect();
    let outer_pad: Vec<u8> = padded.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha256::new();
    inner.update(&inner_pad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(&outer_pad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// The object name part that stands for a content hash: HMAC-SHA256 of it under a secret that never leaves
/// this desktop. Anyone who sees the bucket's key list (the server, Cloudflare) cannot test whether a known file
/// was opened, which the bare content hash would let them do.
pub fn object_hash(secret: &[u8], content_hash_hex: &str) -> String {
    hex(&hmac_sha256(secret, content_hash_hex.as_bytes()))
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn read_up_to(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// SHA-256 and length of everything `reader` yields, refusing more than `limit` bytes.
pub fn hash_reader(mut reader: impl Read, limit: u64) -> io::Result<([u8; 32], u64)> {
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        total += n as u64;
        if total > limit {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "file too large"));
        }
        hasher.update(&buffer[..n]);
    }
    Ok((hasher.finalize().into(), total))
}

/// Reads plaintext and yields the sealed object, one chunk of memory at a time. Before it emits
/// the final chunk it checks that the plaintext had the length and SHA-256 that were announced:
/// a file that changed since it was hashed fails the upload instead of being stored under the
/// wrong content hash.
pub struct SealReader<R: Read> {
    inner: R,
    key: ObjectKey,
    expected_len: u64,
    expected_hash: [u8; 32],
    hasher: Sha256,
    read_total: u64,
    index: u32,
    carry: Option<u8>,
    done: bool,
    out: Vec<u8>,
    out_at: usize,
}

impl<R: Read> SealReader<R> {
    pub fn new(inner: R, key: ObjectKey, expected_len: u64, expected_hash: [u8; 32]) -> Self {
        Self {
            inner,
            key,
            expected_len,
            expected_hash,
            hasher: Sha256::new(),
            read_total: 0,
            index: 0,
            carry: None,
            done: false,
            out: Vec::new(),
            out_at: 0,
        }
    }

    fn changed() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "The file changed while it was read",
        )
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let mut plain = vec![0u8; CHUNK_BYTES];
        let mut have = 0;
        if let Some(byte) = self.carry.take() {
            plain[0] = byte;
            have = 1;
        }
        have += read_up_to(&mut self.inner, &mut plain[have..])?;
        plain.truncate(have);
        let mut last = have < CHUNK_BYTES;
        if !last {
            let mut probe = [0u8; 1];
            if read_up_to(&mut self.inner, &mut probe)? == 0 {
                last = true;
            } else {
                self.carry = Some(probe[0]);
            }
        }
        // The probe byte is counted but not hashed: it opens the next chunk.
        self.read_total += have as u64;
        if self.read_total + u64::from(self.carry.is_some()) > self.expected_len {
            return Err(Self::changed());
        }
        self.hasher.update(&plain);
        if last {
            let digest: [u8; 32] = self.hasher.clone().finalize().into();
            if self.read_total != self.expected_len || digest != self.expected_hash {
                return Err(Self::changed());
            }
            self.done = true;
        }
        self.out = self
            .key
            .seal_chunk(self.index, last, &plain)
            .map_err(io::Error::other)?;
        self.out_at = 0;
        self.index = self
            .index
            .checked_add(1)
            .ok_or_else(|| io::Error::other("file too large"))?;
        Ok(())
    }
}

impl<R: Read> Read for SealReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.out_at >= self.out.len() {
            if self.done {
                return Ok(0);
            }
            self.next_chunk()?;
        }
        let n = buffer.len().min(self.out.len() - self.out_at);
        buffer[..n].copy_from_slice(&self.out[self.out_at..self.out_at + n]);
        self.out_at += n;
        Ok(n)
    }
}
