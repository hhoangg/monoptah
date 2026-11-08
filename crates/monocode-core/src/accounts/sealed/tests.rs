use super::*;

fn db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("monocode.db");
    (dir, database)
}

#[test]
fn round_trip_and_key_is_private() {
    let (dir, database) = db();
    let (nonce, ciphertext) = seal(&database, b"aad", b"secret").unwrap();
    assert!(!ciphertext.windows(6).any(|w| w == b"secret"));
    let plain = open(&database, b"aad", &nonce, &ciphertext).unwrap();
    assert_eq!(plain.as_slice(), b"secret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(KEY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn wrong_aad_or_flipped_byte_is_rejected() {
    let (_dir, database) = db();
    let (nonce, mut ciphertext) = seal(&database, b"aad", b"secret").unwrap();
    assert!(open(&database, b"other", &nonce, &ciphertext).is_err());
    let mut bad_nonce = nonce;
    bad_nonce[0] ^= 1;
    assert!(open(&database, b"aad", &bad_nonce, &ciphertext).is_err());
    assert!(open(&database, b"aad", &nonce[..5], &ciphertext).is_err());
    ciphertext[0] ^= 1;
    assert!(open(&database, b"aad", &nonce, &ciphertext).is_err());
}

#[test]
fn a_damaged_key_is_never_replaced() {
    let (dir, database) = db();
    let key = dir.path().join(KEY_FILE);
    std::fs::write(&key, b"short").unwrap();
    assert!(seal(&database, b"aad", b"secret").is_err());
    assert_eq!(std::fs::read(&key).unwrap(), b"short");
    assert!(open(&database, b"aad", &[0; NONCE_LEN], b"x").is_err());
}

#[test]
fn the_key_is_reused_across_seals() {
    let (_dir, database) = db();
    let (n1, c1) = seal(&database, b"a", b"one").unwrap();
    let (n2, c2) = seal(&database, b"b", b"two").unwrap();
    assert_eq!(open(&database, b"a", &n1, &c1).unwrap().as_slice(), b"one");
    assert_eq!(open(&database, b"b", &n2, &c2).unwrap().as_slice(), b"two");
}
