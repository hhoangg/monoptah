//! Where the desktop's mobile identity lives. It is sealed (AES-256-GCM) in the
//! native store database, like the other saved credentials, so reading it never
//! touches the OS keychain. An ad-hoc signed app gets a new code identity on
//! every build, and each new identity makes the keychain ask for a password.
//! The keychain is read at most once, to migrate an identity saved there by an
//! earlier version, and the entry is removed after the sealed copy is verified.
use crate::accounts::sealed;
use monocode_mobile_link::Identity;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// DDL for the sealed identity table, shared with the native store schema.
pub(crate) const MOBILE_IDENTITY_TABLE: &str = "CREATE TABLE IF NOT EXISTS mobile_identity(
  account TEXT PRIMARY KEY, nonce BLOB NOT NULL, ciphertext BLOB NOT NULL);";

const SERVICE: &str = "com.monocode.mobile";
const MISSING: &str =
    "Desktop mobile identity is missing. Remove paired devices before creating a new identity.";
const UNAVAILABLE: &str = "Desktop mobile identity storage is unavailable.";
const UNREADABLE: &str = "Cannot read desktop mobile identity: the saved identity is damaged or its key file is unreadable.";
const SAVE_FAILED: &str = "Could not save the desktop mobile identity.";

/// The legacy home of the identity. Only read and deleted, never written.
pub(crate) trait IdentityKeychain: Send + Sync {
    /// `Ok(None)` means there is no entry. `Err` carries the reason a read failed.
    fn get(&self) -> Result<Option<Zeroizing<String>>, String>;
    fn delete(&self) -> Result<(), String>;
}

/// The OS keychain entry the earlier desktop app used.
pub(crate) struct SystemKeychain {
    account: String,
}
impl SystemKeychain {
    pub(crate) fn new(account: String) -> Self {
        Self { account }
    }
    fn entry(&self) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, &self.account).map_err(|e| e.to_string())
    }
}
impl IdentityKeychain for SystemKeychain {
    fn get(&self) -> Result<Option<Zeroizing<String>>, String> {
        match self.entry()?.get_password() {
            Ok(value) => Ok(Some(Zeroizing::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn delete(&self) -> Result<(), String> {
        self.entry()?.delete_credential().map_err(|e| e.to_string())
    }
}

/// The row key, and the binding that ties a ciphertext to this config file.
pub(crate) fn account_for(config_path: &Path) -> String {
    format!("desktop:{}", config_path.display())
}

fn associated_data(account: &str) -> Vec<u8> {
    format!("monoptah-mobile-identity-v1\0{account}").into_bytes()
}

/// Open an existing database without creating it.
fn open_existing(database: &Path) -> Result<Connection, String> {
    if !database.exists() {
        return Err(UNAVAILABLE.into());
    }
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|_| UNAVAILABLE.to_owned())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| UNAVAILABLE.to_owned())?;
    Ok(conn)
}

type Row = (Vec<u8>, Vec<u8>);

fn read_row(conn: &Connection, account: &str) -> Result<Option<Row>, String> {
    match conn
        .query_row(
            "SELECT nonce, ciphertext FROM mobile_identity WHERE account=?1",
            params![account],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
    {
        Ok(row) => Ok(row),
        // A database that predates the table has no identity.
        Err(e) if e.to_string().contains("no such table") => Ok(None),
        Err(_) => Err(UNAVAILABLE.into()),
    }
}

fn open_row(database: &Path, account: &str, row: &Row) -> Result<Zeroizing<Vec<u8>>, String> {
    sealed::open(database, &associated_data(account), &row.0, &row.1)
        .map_err(|_| UNREADABLE.to_owned())
}

/// Seal `json` and store it unless a row already exists (another process may
/// have won the race), then read the row back and return the bytes actually
/// stored. Those may differ from `json`; the caller must use them.
fn write_verified(
    conn: &Connection,
    database: &Path,
    account: &str,
    json: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    conn.execute_batch(MOBILE_IDENTITY_TABLE)
        .map_err(|_| SAVE_FAILED.to_owned())?;
    let (nonce, ciphertext) = sealed::seal(database, &associated_data(account), json.as_bytes())
        .map_err(|_| SAVE_FAILED.to_owned())?;
    conn.execute(
        "INSERT OR IGNORE INTO mobile_identity(account, nonce, ciphertext) VALUES(?1, ?2, ?3)",
        params![account, nonce.as_slice(), ciphertext],
    )
    .map_err(|_| SAVE_FAILED.to_owned())?;
    let stored = read_row(conn, account)?.ok_or_else(|| SAVE_FAILED.to_owned())?;
    open_row(database, account, &stored)
}

fn parse_identity(bytes: &[u8]) -> Result<Identity, String> {
    serde_json::from_slice(bytes).map_err(|_| "Saved desktop mobile identity is invalid".to_owned())
}

/// Find, migrate or create the identity. Never regenerates over a row that
/// exists, and never creates an identity it cannot persist.
pub(crate) fn load(
    database: Option<&Path>,
    account: &str,
    paired: bool,
    keychain: &dyn IdentityKeychain,
) -> Result<Identity, String> {
    let database: PathBuf = database.ok_or(UNAVAILABLE)?.into();
    let conn = open_existing(&database)?;
    if let Some(row) = read_row(&conn, account)? {
        let plain = open_row(&database, account, &row)?;
        return parse_identity(&plain);
    }
    match keychain.get() {
        Ok(Some(value)) => {
            parse_identity(value.as_bytes())?;
            let stored = write_verified(&conn, &database, account, &value)?;
            let identity = parse_identity(&stored)?;
            // Delete the keychain entry only when the sealed copy is exactly
            // the keychain value; otherwise another identity won the race.
            if stored.as_slice() == value.as_bytes() {
                let _ = keychain.delete();
            }
            Ok(identity)
        }
        Ok(None) => {
            if paired {
                return Err(MISSING.into());
            }
            let generated = Identity::generate()?;
            let json =
                Zeroizing::new(serde_json::to_string(&generated).map_err(|e| e.to_string())?);
            let stored = write_verified(&conn, &database, account, &json)?;
            // If another process stored first, its identity is the one to use.
            parse_identity(&stored)
        }
        Err(error) => Err(format!("Cannot read desktop mobile identity: {error}")),
    }
}

#[cfg(test)]
pub(crate) mod tests;
