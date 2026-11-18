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
use zeroize::{Zeroize, Zeroizing};

/// DDL for the sealed identity table, shared with the native store schema.
pub(crate) const MOBILE_IDENTITY_TABLE: &str = "CREATE TABLE IF NOT EXISTS mobile_identity(
  account TEXT PRIMARY KEY, nonce BLOB NOT NULL, ciphertext BLOB NOT NULL);";

const SERVICE: &str = "com.monocode.mobile";
const MISSING: &str =
    "Desktop mobile identity is missing. Remove paired devices before creating a new identity.";
pub(crate) const UNAVAILABLE: &str = "Desktop mobile identity storage is unavailable.";
const UNREADABLE: &str = "Cannot read desktop mobile identity: the saved identity is damaged or its key file is unreadable.";
/// A web row that is damaged or whose key file is unreadable; the caller can
/// treat it as a reinstall.
pub(crate) const WEB_UNREADABLE: &str = "Cannot read the web link identity: the saved identity is damaged or its key file is unreadable.";
const WEB_SAVE_FAILED: &str = "Could not save the web link identity.";
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

/// The row key of the web link identity for this config file. It differs from
/// `account_for`, so the two rows never share a key.
pub(crate) fn web_account_for(config_path: &Path) -> String {
    format!("web:{}", config_path.display())
}

/// A different binding from the mobile row's, so a row of one kind can never
/// be opened as the other.
fn web_associated_data(account: &str) -> Vec<u8> {
    format!("monoptah-web-identity-v1\0{account}").into_bytes()
}

/// What the desktop keeps of its web link pairing. The keys and the token live
/// only in the sealed row, never in `mobile-access.json`.
#[allow(dead_code)] // Read by the web link session once it lands.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct WebIdentity {
    pub(crate) static_private: [u8; 32],
    pub(crate) static_public: [u8; 32],
    pub(crate) desktop_id: String,
    pub(crate) desktop_token: String,
    pub(crate) relay_url: String,
    /// The web account's handle, shown in Settings. Rows sealed before it
    /// existed have none.
    pub(crate) account_handle: Option<String>,
}

impl std::fmt::Debug for WebIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebIdentity")
            .field("static_private", &"<redacted>")
            .field("static_public", &self.static_public)
            .field("desktop_id", &self.desktop_id)
            .field("desktop_token", &"<redacted>")
            .field("relay_url", &self.relay_url)
            .field("account_handle", &self.account_handle)
            .finish()
    }
}

impl Drop for WebIdentity {
    fn drop(&mut self) {
        self.static_private.zeroize();
        self.desktop_token.zeroize();
    }
}

/// The JSON form inside the seal: keys as base64.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebIdentityJson {
    static_private: String,
    static_public: String,
    desktop_id: String,
    desktop_token: String,
    relay_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_handle: Option<String>,
}

impl Drop for WebIdentityJson {
    fn drop(&mut self) {
        self.static_private.zeroize();
        self.desktop_token.zeroize();
    }
}

fn encode_web(identity: &WebIdentity) -> Result<Zeroizing<Vec<u8>>, String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let json = WebIdentityJson {
        static_private: URL_SAFE_NO_PAD.encode(identity.static_private),
        static_public: URL_SAFE_NO_PAD.encode(identity.static_public),
        desktop_id: identity.desktop_id.clone(),
        desktop_token: identity.desktop_token.clone(),
        relay_url: identity.relay_url.clone(),
        account_handle: identity.account_handle.clone(),
    };
    serde_json::to_vec(&json)
        .map(Zeroizing::new)
        .map_err(|_| WEB_SAVE_FAILED.to_owned())
}

fn decode_web(bytes: &[u8]) -> Result<WebIdentity, String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let json: WebIdentityJson =
        serde_json::from_slice(bytes).map_err(|_| WEB_UNREADABLE.to_owned())?;
    let key = |text: &str| -> Result<[u8; 32], String> {
        let mut bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(text)
                .map_err(|_| WEB_UNREADABLE.to_owned())?,
        );
        let key = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| WEB_UNREADABLE.to_owned());
        bytes.zeroize();
        key
    };
    Ok(WebIdentity {
        static_private: key(&json.static_private)?,
        static_public: key(&json.static_public)?,
        desktop_id: json.desktop_id.clone(),
        desktop_token: json.desktop_token.clone(),
        relay_url: json.relay_url.clone(),
        account_handle: json.account_handle.clone(),
    })
}

/// The saved web identity. `Ok(None)` means no row; a damaged row or an
/// unreadable key file is `WEB_UNREADABLE`.
#[allow(dead_code)] // Used through `MobileHost::load_web_identity`.
pub(crate) fn load_web(database: &Path, account: &str) -> Result<Option<WebIdentity>, String> {
    let conn = open_existing(database)?;
    let Some(row) = read_row(&conn, account)? else {
        return Ok(None);
    };
    let plain = sealed::open(database, &web_associated_data(account), &row.0, &row.1)
        .map_err(|_| WEB_UNREADABLE.to_owned())?;
    decode_web(&plain).map(Some)
}

/// Seals the web identity, replacing any earlier one.
#[allow(dead_code)] // Used through `MobileHost::save_web_identity`.
pub(crate) fn save_web(
    database: &Path,
    account: &str,
    identity: &WebIdentity,
) -> Result<(), String> {
    let conn = open_existing(database)?;
    conn.execute_batch(MOBILE_IDENTITY_TABLE)
        .map_err(|_| WEB_SAVE_FAILED.to_owned())?;
    let plain = encode_web(identity)?;
    let (nonce, ciphertext) = sealed::seal(database, &web_associated_data(account), &plain)
        .map_err(|_| WEB_SAVE_FAILED.to_owned())?;
    conn.execute(
        "INSERT OR REPLACE INTO mobile_identity(account, nonce, ciphertext) VALUES(?1, ?2, ?3)",
        params![account, nonce.as_slice(), ciphertext],
    )
    .map_err(|_| WEB_SAVE_FAILED.to_owned())?;
    Ok(())
}

/// Removes the web identity row. Removing a row that is not there succeeds.
pub(crate) fn wipe_web(database: &Path, account: &str) -> Result<(), String> {
    let conn = open_existing(database)?;
    match conn.execute(
        "DELETE FROM mobile_identity WHERE account=?1",
        params![account],
    ) {
        Ok(_) => Ok(()),
        // A database that predates the table has nothing to wipe.
        Err(e) if e.to_string().contains("no such table") => Ok(()),
        Err(_) => Err(UNAVAILABLE.into()),
    }
}

#[cfg(test)]
pub(crate) mod tests;
