//! AES-256-GCM sealing shared by every credential stored in the native store
//! database. The key lives in `credentials.key` next to the database, never in
//! the database itself. Callers choose the table and the associated data (AAD)
//! that binds a ciphertext to its owner; this module owns the key file and the
//! encrypt/decrypt primitives.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Key, Nonce,
};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

pub(crate) const KEY_FILE: &str = "credentials.key";
pub(crate) const KEY_LEN: usize = 32;
pub(crate) const NONCE_LEN: usize = 12;

/// Why a key operation failed. Callers map these to their own user-facing text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyError {
    /// An existing key could not be read or has the wrong length.
    Unreadable,
    /// A key could not be written.
    SaveFailed,
}

pub(crate) fn read_key(path: &Path) -> Result<[u8; KEY_LEN], KeyError> {
    let bytes = std::fs::read(path).map_err(|_| KeyError::Unreadable)?;
    <[u8; KEY_LEN]>::try_from(bytes.as_slice()).map_err(|_| KeyError::Unreadable)
}

/// Write a new key to a private temp file next to `path`.
pub(crate) fn write_temp_key(path: &Path, key: &[u8; KEY_LEN]) -> Result<PathBuf, KeyError> {
    use std::io::Write;
    let dir = path.parent().ok_or(KeyError::SaveFailed)?;
    std::fs::create_dir_all(dir).map_err(|_| KeyError::SaveFailed)?;
    let temporary = dir.join(format!(".{KEY_FILE}.{}", super::request_id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&temporary)
        .and_then(|mut file| file.write_all(key).and_then(|_| file.sync_all()));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err(KeyError::SaveFailed);
    }
    Ok(temporary)
}

/// Create the key file once, privately and atomically: write a private temp
/// file, then hard-link it into place. A racing creator loses the link and
/// reads the winner's key, and no reader ever sees a partial file.
pub(crate) fn create_key(path: &Path) -> Result<[u8; KEY_LEN], KeyError> {
    let key: [u8; KEY_LEN] = rand::random();
    let temporary = write_temp_key(path, &key)?;
    let linked = std::fs::hard_link(&temporary, path);
    let _ = std::fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(key),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read_key(path),
        Err(_) => Err(KeyError::SaveFailed),
    }
}

/// What a save finds at the key path. Only a file that is absent may be
/// created; anything else unexpected must never be overwritten.
pub(crate) enum KeyState {
    Valid([u8; KEY_LEN]),
    Missing,
    WrongLength,
}
pub(crate) fn inspect_key(path: &Path) -> Result<KeyState, KeyError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(<[u8; KEY_LEN]>::try_from(bytes.as_slice())
            .map(KeyState::Valid)
            .unwrap_or(KeyState::WrongLength)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(KeyState::Missing),
        Err(_) => Err(KeyError::SaveFailed),
    }
}

/// Encrypt with a fresh random nonce.
pub(crate) fn encrypt(
    key: &[u8; KEY_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<([u8; NONCE_LEN], Vec<u8>), ()> {
    let nonce: [u8; NONCE_LEN] = rand::random();
    let ciphertext = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| ())?;
    Ok((nonce, ciphertext))
}

/// Decrypt and authenticate. Fails on a wrong key, nonce, AAD or any flipped byte.
pub(crate) fn decrypt(
    key: &[u8; KEY_LEN],
    aad: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, ()> {
    if nonce.len() != NONCE_LEN {
        return Err(());
    }
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| ())
}

/// Path of the key file for the store database `database`.
pub(crate) fn key_path(database: &Path) -> Option<PathBuf> {
    Some(database.parent()?.join(KEY_FILE))
}

/// Seal `plaintext` under the key next to `database`, creating the key only when
/// the file is absent. A damaged key is never replaced here.
pub(crate) fn seal(
    database: &Path,
    aad: &[u8],
    plaintext: &[u8],
) -> Result<([u8; NONCE_LEN], Vec<u8>), String> {
    const FAILED: &str = "Could not save the credential.";
    const DAMAGED: &str = "The key file for saved credentials is damaged. Restore credentials.key, or remove the saved credentials, then try again.";
    let path = key_path(database).ok_or(FAILED)?;
    let key = Zeroizing::new(match inspect_key(&path).map_err(|_| FAILED)? {
        KeyState::Valid(key) => key,
        KeyState::Missing => create_key(&path).map_err(|_| FAILED)?,
        KeyState::WrongLength => return Err(DAMAGED.into()),
    });
    encrypt(&key, aad, plaintext).map_err(|_| FAILED.into())
}

/// Open a value sealed by [`seal`].
pub(crate) fn open(
    database: &Path,
    aad: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, String> {
    const UNREADABLE: &str = "Could not read the saved credential. Save it again.";
    let path = key_path(database).ok_or(UNREADABLE)?;
    let key = Zeroizing::new(read_key(&path).map_err(|_| UNREADABLE)?);
    decrypt(&key, aad, nonce, ciphertext).map_err(|_| UNREADABLE.into())
}

#[cfg(test)]
mod tests;
