//! The webview-facing mail commands. Logic lives in plain functions that take
//! the store and the AWS seams, so tests run without Tauri, a keychain or a
//! network; the `#[tauri::command]` wrappers only fetch managed state.
//!
//! No command returns a secret. A credential is described by its masked key id
//! (`AKIA••••3K7Q`), and the key pair itself lives in the OS keychain.

use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::aws_s3::{S3Api, S3Client, S3ErrorKind};
use crate::aws_ses::{SesApi, SesClient, SesErrorKind};
use crate::aws_sigv4::Credentials;
use crate::mail_store::{
    self, AccountInput, CredentialInput, MailAccount, MailCredential, MailIdentity, MailMailbox,
    MailSyncState, MailboxInput, Page, ThreadDetail, ThreadQuery, ThreadRow,
};
use crate::mail_sync::{self, ClientFactory, SyncReport, SyncService, TauriEvents};
use crate::session_store::{now_millis, SessionStore};

const KEYCHAIN_SERVICE: &str = "com.monoptah.desktop";
const PROVIDER: &str = "aws";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// How often the daily identity refresh looks for sources that are due.
const IDENTITY_TICK: Duration = Duration::from_secs(30 * 60);
const IDENTITY_FIRST_DELAY: Duration = Duration::from_secs(60);
const MAX_PREFIX_PAGES: usize = 20;
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_IMAGE_REDIRECTS: usize = 3;
const IMAGE_TIMEOUT: Duration = Duration::from_secs(15);

// ── keychain ───────────────────────────────────────────────────────

/// No `Debug` and no `Serialize`: neither half may reach a log or a command
/// result by accident.
pub(crate) struct KeyPair {
    pub access_key_id: String,
    pub secret_access_key: String,
}

/// Where AWS key pairs live. The app uses the OS keychain; tests use memory, so
/// `cargo test` never needs (or touches) a real keychain.
pub(crate) trait SecretStore: Send + Sync {
    fn put(&self, credential_id: i64, keys: &KeyPair) -> Result<(), String>;
    fn get(&self, credential_id: i64) -> Result<Option<KeyPair>, String>;
    fn remove(&self, credential_id: i64) -> Result<(), String>;
}

pub(crate) struct Keychain;

fn entry(credential_id: i64, field: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(
        KEYCHAIN_SERVICE,
        &format!("mail.aws.cred.{credential_id}.{field}"),
    )
    .map_err(|_| "Could not access the system credential store.".to_string())
}

fn read_entry(credential_id: i64, field: &str) -> Result<Option<String>, String> {
    match entry(credential_id, field)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err("Could not read the AWS key from the system credential store.".into()),
    }
}

fn write_entry(credential_id: i64, field: &str, value: &str) -> Result<(), String> {
    entry(credential_id, field)?
        .set_password(value)
        .map_err(|_| "Could not save the AWS key to the system credential store.".to_string())
}

fn delete_entry(credential_id: i64, field: &str) -> Result<(), String> {
    match entry(credential_id, field)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("Could not remove the AWS key from the system credential store.".into()),
    }
}

impl SecretStore for Keychain {
    fn put(&self, credential_id: i64, keys: &KeyPair) -> Result<(), String> {
        let previous = read_entry(credential_id, "access_key_id")?;
        write_entry(credential_id, "access_key_id", &keys.access_key_id)?;
        if let Err(error) = write_entry(credential_id, "secret_access_key", &keys.secret_access_key)
        {
            // Never leave a new id sitting beside the old secret.
            let _ = match previous {
                Some(old) => write_entry(credential_id, "access_key_id", &old),
                None => delete_entry(credential_id, "access_key_id"),
            };
            return Err(error);
        }
        Ok(())
    }

    fn get(&self, credential_id: i64) -> Result<Option<KeyPair>, String> {
        let id = read_entry(credential_id, "access_key_id")?;
        let secret = read_entry(credential_id, "secret_access_key")?;
        Ok(match (id, secret) {
            (Some(access_key_id), Some(secret_access_key)) => Some(KeyPair {
                access_key_id,
                secret_access_key,
            }),
            _ => None,
        })
    }

    fn remove(&self, credential_id: i64) -> Result<(), String> {
        let id = delete_entry(credential_id, "access_key_id");
        let secret = delete_entry(credential_id, "secret_access_key");
        id.and(secret)
    }
}

// ── AWS clients ────────────────────────────────────────────────────

/// Builds the S3 and SES clients of a source. S3 uses the source's S3
/// credential (falling back to its SES one); SES always uses the SES one.
pub(crate) trait AwsClients: Send + Sync {
    fn s3(&self, account: &MailAccount) -> Result<Box<dyn S3Api + Send>, String>;
    fn ses(&self, account: &MailAccount) -> Result<Box<dyn SesApi + Send>, String>;
}

pub(crate) struct LiveClients {
    secrets: Arc<dyn SecretStore>,
}

impl LiveClients {
    fn credentials(&self, credential_id: i64) -> Result<Credentials, String> {
        match self.secrets.get(credential_id)? {
            Some(keys) => Ok(Credentials {
                access_key_id: keys.access_key_id,
                secret_access_key: keys.secret_access_key,
            }),
            None => Err("The AWS key for this source is missing from the system credential store. Enter it again in Mail settings.".into()),
        }
    }
}

impl AwsClients for LiveClients {
    fn s3(&self, account: &MailAccount) -> Result<Box<dyn S3Api + Send>, String> {
        let credential = account.s3_credential_id.unwrap_or(account.credential_id);
        Ok(Box::new(S3Client::new(
            self.credentials(credential)?,
            &account.s3_region,
            &account.bucket,
        )))
    }

    fn ses(&self, account: &MailAccount) -> Result<Box<dyn SesApi + Send>, String> {
        Ok(Box::new(SesClient::new(
            self.credentials(account.credential_id)?,
            &account.ses_region,
        )))
    }
}

impl ClientFactory for LiveClients {
    fn client(&self, account: &MailAccount) -> Result<Box<dyn S3Api + Send>, String> {
        self.s3(account)
    }
}

pub(crate) struct MailState {
    secrets: Arc<dyn SecretStore>,
    clients: Arc<dyn AwsClients>,
    factory: Arc<dyn ClientFactory>,
    sync: Arc<SyncService>,
}

pub fn init(app: &AppHandle) {
    let secrets: Arc<dyn SecretStore> = Arc::new(Keychain);
    let live = Arc::new(LiveClients {
        secrets: secrets.clone(),
    });
    let sync = Arc::new(SyncService::default());
    app.manage(MailState {
        secrets,
        clients: live.clone(),
        factory: live.clone(),
        sync: sync.clone(),
    });
    mail_sync::spawn_scheduler(app.clone(), sync, live.clone());
    spawn_identity_refresh(app.clone(), live);
}

/// The sync scheduler's loop is closed over `sync_due`, so identities get their
/// own slow loop instead of a hook in it. Sources refresh once a day; the tick
/// only finds out which are due.
fn spawn_identity_refresh(app: AppHandle, clients: Arc<dyn AwsClients>) {
    std::thread::spawn(move || {
        std::thread::sleep(IDENTITY_FIRST_DELAY);
        loop {
            let store = app.state::<SessionStore>();
            refresh_due_identities(&store, &*clients, now_millis());
            std::thread::sleep(IDENTITY_TICK);
        }
    });
}

fn db(error: rusqlite::Error) -> String {
    error.to_string()
}

// ── credentials ────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialUpsert {
    pub label: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub default_region: Option<String>,
}

fn valid_region(region: &str) -> bool {
    (2..=32).contains(&region.len())
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !region.starts_with('-')
        && !region.ends_with('-')
}

fn valid_bucket(bucket: &str) -> bool {
    let bytes = bucket.as_bytes();
    (3..=63).contains(&bytes.len())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'.')
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
}

/// Messages never echo the input: a mistyped secret must not land in a log.
fn validate_keys(input: &CredentialUpsert) -> Result<(), String> {
    let id = input.access_key_id.trim();
    if !(16..=128).contains(&id.len())
        || !id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err("That does not look like an AWS access key id.".into());
    }
    if id.starts_with("ASIA") {
        return Err("Temporary session keys are not supported. Use an IAM user access key.".into());
    }
    let secret = input.secret_access_key.trim();
    if !(20..=128).contains(&secret.len()) || secret.chars().any(char::is_whitespace) {
        return Err("That does not look like an AWS secret access key.".into());
    }
    if input.label.trim().is_empty() {
        return Err("Give the credential a name.".into());
    }
    if let Some(region) = input.default_region.as_deref() {
        if !valid_region(region) {
            return Err(format!("'{region}' is not an AWS region."));
        }
    }
    Ok(())
}

pub(crate) fn credential_upsert(
    store: &SessionStore,
    secrets: &dyn SecretStore,
    input: CredentialUpsert,
) -> Result<MailCredential, String> {
    validate_keys(&input)?;
    let label = input.label.trim().to_string();
    let keys = KeyPair {
        access_key_id: input.access_key_id.trim().to_string(),
        secret_access_key: input.secret_access_key.trim().to_string(),
    };
    let (previous, id) = {
        let conn = store.lock_conn()?;
        let previous = mail_store::list_credentials(&conn)
            .map_err(db)?
            .into_iter()
            .find(|c| c.provider == PROVIDER && c.label == label);
        let id = mail_store::upsert_credential(
            &conn,
            &CredentialInput {
                provider: PROVIDER.into(),
                label,
                access_key_masked: mail_store::mask_access_key_id(&keys.access_key_id),
                default_region: input.default_region.clone(),
            },
        )
        .map_err(db)?;
        (previous, id)
    };
    // The keychain call happens without the database lock: it can block on a
    // system prompt, and every other command waits on that lock.
    if let Err(error) = secrets.put(id, &keys) {
        let conn = store.lock_conn()?;
        match previous {
            Some(old) => {
                let _ = mail_store::upsert_credential(
                    &conn,
                    &CredentialInput {
                        provider: old.provider,
                        label: old.label,
                        access_key_masked: old.access_key_masked,
                        default_region: old.default_region,
                    },
                );
            }
            None => {
                let _ = mail_store::delete_credential(&conn, id);
            }
        }
        return Err(error);
    }
    let conn = store.lock_conn()?;
    mail_store::get_credential(&conn, id)
        .map_err(db)?
        .ok_or_else(|| "The credential was saved but could not be read back.".to_string())
}

pub(crate) fn credential_delete(
    store: &SessionStore,
    secrets: &dyn SecretStore,
    id: i64,
) -> Result<(), String> {
    // One acquisition for check, secret and row. Released between them, a source
    // saved in the gap would make the row delete refuse after the secret was
    // already gone, leaving a source that cannot authenticate. The keychain call
    // can block on a prompt while the lock is held; for a rare, user-initiated
    // delete that is the price of not stranding a secret. (A tombstone flag
    // would avoid it but needs a schema change this task does not own.)
    let conn = store.lock_conn()?;
    // Refuses a credential in use before anything is touched.
    if !mail_store::check_credential_unused(&conn, id).map_err(db)? {
        return Err("That credential no longer exists.".into());
    }
    // Secret before row: if the keychain fails the row survives, so the UI can
    // retry. The other order leaves a secret nothing points at.
    secrets.remove(id)?;
    mail_store::delete_credential(&conn, id).map_err(db)?;
    Ok(())
}

// ── sources and mailboxes ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    #[serde(flatten)]
    pub account: MailAccount,
    /// Epoch milliseconds; `None` until identities have been fetched once.
    pub identities_refreshed_at: Option<i64>,
    pub mailboxes: Vec<MailMailbox>,
}

fn source_row(conn: &rusqlite::Connection, account: MailAccount) -> Result<SourceRow, String> {
    Ok(SourceRow {
        identities_refreshed_at: mail_store::identities_refreshed_at(conn, account.id)
            .map_err(db)?,
        mailboxes: mail_store::list_mailboxes(conn, Some(account.id)).map_err(db)?,
        account,
    })
}

pub(crate) fn sources(store: &SessionStore) -> Result<Vec<SourceRow>, String> {
    let conn = store.lock_conn()?;
    mail_store::list_accounts(&conn)
        .map_err(db)?
        .into_iter()
        .map(|account| source_row(&conn, account))
        .collect()
}

fn validate_account(conn: &rusqlite::Connection, input: &mut AccountInput) -> Result<(), String> {
    if input.provider != PROVIDER {
        return Err(format!(
            "Mail provider '{}' is not supported.",
            input.provider
        ));
    }
    input.label = input.label.trim().to_string();
    if input.label.is_empty() {
        return Err("Give the source a name.".into());
    }
    input.bucket = input.bucket.trim().to_string();
    for (what, region) in [("SES", &input.ses_region), ("S3", &input.s3_region)] {
        if !valid_region(region) {
            return Err(format!("'{region}' is not a valid {what} region."));
        }
    }
    if !valid_bucket(&input.bucket) {
        return Err(format!("'{}' is not a valid S3 bucket name.", input.bucket));
    }
    if !["on_demand", "always"].contains(&input.attach_policy.as_str()) {
        return Err("Attachment policy must be on_demand or always.".into());
    }
    if !["ask", "always", "never"].contains(&input.image_policy.as_str()) {
        return Err("Image policy must be ask, always or never.".into());
    }
    for id in std::iter::once(input.credential_id).chain(input.s3_credential_id) {
        if mail_store::get_credential(conn, id).map_err(db)?.is_none() {
            return Err("That credential no longer exists.".into());
        }
    }
    input.default_from = input
        .default_from
        .take()
        .map(|from| from.trim().to_string())
        .filter(|from| !from.is_empty());
    if let Some((id, label)) = mail_store::find_account_by_key(
        conn,
        input.credential_id,
        &input.ses_region,
        &input.bucket,
        &input.s3_region,
        input.id,
    )
    .map_err(db)?
    {
        // A create that matches an existing source updates it, as the store
        // does; only an edit that would collide with a different one is an error.
        if input.id.is_some() {
            return Err(format!(
                "Source '{label}' (#{id}) already uses this key, region and bucket."
            ));
        }
    }
    Ok(())
}

pub(crate) fn source_upsert(
    store: &SessionStore,
    mut input: AccountInput,
) -> Result<SourceRow, String> {
    let conn = store.lock_conn()?;
    validate_account(&conn, &mut input)?;
    let id = mail_store::upsert_account(&conn, &input).map_err(db)?;
    let account = mail_store::get_account(&conn, id)
        .map_err(db)?
        .ok_or_else(|| "The source was saved but could not be read back.".to_string())?;
    source_row(&conn, account)
}

/// Returns the ids of the removed messages so the caller can delete their
/// attachment files.
pub(crate) fn source_delete(store: &SessionStore, id: i64) -> Result<Vec<i64>, String> {
    let conn = store.lock_conn()?;
    let tx = conn.unchecked_transaction().map_err(db)?;
    let messages = mail_store::message_ids_in_account(&tx, id).map_err(db)?;
    if !mail_store::delete_account(&tx, id).map_err(db)? {
        return Err("That source no longer exists.".into());
    }
    tx.commit().map_err(db)?;
    Ok(messages)
}

pub(crate) fn mailboxes_upsert(
    store: &SessionStore,
    account_id: i64,
    rows: Vec<MailboxInput>,
) -> Result<Vec<MailMailbox>, String> {
    let rows: Vec<MailboxInput> = rows
        .into_iter()
        .map(|mut row| {
            row.address = row.address.trim().to_string();
            row.s3_prefix = row.s3_prefix.trim().to_string();
            if row.display_name.trim().is_empty() {
                row.display_name = row
                    .address
                    .split('@')
                    .next()
                    .unwrap_or_default()
                    .to_string();
            }
            row
        })
        .collect();
    if let Some(row) = rows.iter().find(|r| r.s3_prefix.starts_with('/')) {
        return Err(format!(
            "Prefix '{}' must not start with '/'.",
            row.s3_prefix
        ));
    }
    let conn = store.lock_conn()?;
    if mail_store::get_account(&conn, account_id)
        .map_err(db)?
        .is_none()
    {
        return Err("That source no longer exists.".into());
    }
    mail_store::upsert_mailboxes(&conn, account_id, &rows).map_err(db)?;
    mail_store::list_mailboxes(&conn, Some(account_id)).map_err(db)
}

pub(crate) fn mailboxes_delete(store: &SessionStore, id: i64) -> Result<Vec<i64>, String> {
    let conn = store.lock_conn()?;
    let tx = conn.unchecked_transaction().map_err(db)?;
    let messages = mail_store::message_ids_in_mailbox(&tx, id).map_err(db)?;
    if !mail_store::delete_mailbox(&tx, id).map_err(db)? {
        return Err("That mailbox no longer exists.".into());
    }
    tx.commit().map_err(db)?;
    Ok(messages)
}

// ── identities ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityRow {
    pub account_id: i64,
    #[serde(flatten)]
    pub identity: MailIdentity,
}

pub(crate) fn identities(
    store: &SessionStore,
    account_id: Option<i64>,
) -> Result<Vec<IdentityRow>, String> {
    let conn = store.lock_conn()?;
    let ids: Vec<i64> = match account_id {
        Some(id) => vec![id],
        None => mail_store::list_accounts(&conn)
            .map_err(db)?
            .into_iter()
            .map(|a| a.id)
            .collect(),
    };
    let mut rows = Vec::new();
    for account_id in ids {
        for identity in mail_store::list_identities(&conn, account_id).map_err(db)? {
            rows.push(IdentityRow {
                account_id,
                identity,
            });
        }
    }
    Ok(rows)
}

/// Stores fetched identities unless the source was repointed at another
/// credential or SES region while the request was in flight: those identities
/// would describe the wrong account.
fn store_identities(
    store: &SessionStore,
    fetched_for: &MailAccount,
    identities: &[MailIdentity],
) -> Result<(), String> {
    let conn = store.lock_conn()?;
    let current = mail_store::get_account(&conn, fetched_for.id).map_err(db)?;
    match current {
        Some(now)
            if now.credential_id == fetched_for.credential_id
                && now.ses_region == fetched_for.ses_region =>
        {
            mail_store::replace_identities(&conn, now.id, identities).map_err(db)
        }
        _ => Ok(()),
    }
}

pub(crate) fn refresh_identities(
    store: &SessionStore,
    clients: &dyn AwsClients,
    account: &MailAccount,
) -> Result<usize, String> {
    let fetched = clients
        .ses(account)?
        .list_identities()
        .map_err(|e| e.message)?;
    store_identities(store, account, &fetched)?;
    Ok(fetched.len())
}

/// Refreshes every enabled source whose identities are older than a day, or
/// were never fetched. One source failing does not stop the others.
pub(crate) fn refresh_due_identities(
    store: &SessionStore,
    clients: &dyn AwsClients,
    now_ms: i64,
) -> Vec<(i64, Result<usize, String>)> {
    let due: Vec<MailAccount> = {
        let Ok(conn) = store.lock_conn() else {
            return Vec::new();
        };
        let Ok(accounts) = mail_store::list_accounts(&conn) else {
            return Vec::new();
        };
        accounts
            .into_iter()
            .filter(|a| a.enabled)
            .filter(|a| {
                mail_store::identities_refreshed_at(&conn, a.id)
                    .ok()
                    .flatten()
                    .is_none_or(|at| now_ms - at >= DAY_MS)
            })
            .collect()
    };
    due.iter()
        .map(|a| (a.id, refresh_identities(store, clients, a)))
        .collect()
}

// ── source test ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub ok: bool,
    /// The IAM action the check needs.
    pub action: &'static str,
    /// Machine-readable failure class, e.g. `accessDenied` or `wrongRegion`.
    pub kind: Option<&'static str>,
    /// Names the source; for a wrong region it names the bucket's real one.
    pub message: Option<String>,
}

impl CheckResult {
    fn pass(action: &'static str) -> Self {
        Self {
            ok: true,
            action,
            kind: None,
            message: None,
        }
    }

    fn fail(action: &'static str, source: &str, kind: &'static str, message: &str) -> Self {
        Self {
            ok: false,
            action,
            kind: Some(kind),
            message: Some(format!("{source}: {message}")),
        }
    }
}

/// One report per source. There is deliberately no combined verdict: three
/// healthy domains and one broken one must not read as "connected".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub account_id: i64,
    pub label: String,
    pub s3: CheckResult,
    pub ses_identities: CheckResult,
    pub ses_account: CheckResult,
    pub identity_count: Option<usize>,
    /// `Some(true)` while SES only delivers to verified addresses.
    pub sandbox: Option<bool>,
}

fn s3_kind(kind: S3ErrorKind) -> &'static str {
    match kind {
        S3ErrorKind::AccessDenied => "accessDenied",
        S3ErrorKind::NoSuchBucket => "noSuchBucket",
        S3ErrorKind::NoSuchKey => "noSuchKey",
        S3ErrorKind::InvalidCredentials => "invalidCredentials",
        S3ErrorKind::WrongRegion => "wrongRegion",
        S3ErrorKind::ClockSkew => "clockSkew",
        S3ErrorKind::Network => "network",
        S3ErrorKind::Other => "other",
    }
}

fn ses_kind(kind: SesErrorKind) -> &'static str {
    match kind {
        SesErrorKind::AccessDenied => "accessDenied",
        SesErrorKind::InvalidCredentials => "invalidCredentials",
        SesErrorKind::ClockSkew => "clockSkew",
        SesErrorKind::Network => "network",
        SesErrorKind::Other => "other",
    }
}

const ACTION_LIST_BUCKET: &str = "s3:ListBucket";
const ACTION_LIST_IDENTITIES: &str = "ses:ListEmailIdentities";
const ACTION_GET_ACCOUNT: &str = "ses:GetAccount";

pub(crate) fn source_test(
    store: &SessionStore,
    clients: &dyn AwsClients,
    id: i64,
) -> Result<TestReport, String> {
    let account = {
        let conn = store.lock_conn()?;
        mail_store::get_account(&conn, id)
            .map_err(db)?
            .ok_or_else(|| "That source no longer exists.".to_string())?
    };
    let label = account.label.clone();
    let mut report = TestReport {
        account_id: id,
        label: label.clone(),
        s3: CheckResult::pass(ACTION_LIST_BUCKET),
        ses_identities: CheckResult::pass(ACTION_LIST_IDENTITIES),
        ses_account: CheckResult::pass(ACTION_GET_ACCOUNT),
        identity_count: None,
        sandbox: None,
    };

    match clients.s3(&account) {
        Err(message) => {
            report.s3 = CheckResult::fail(ACTION_LIST_BUCKET, &label, "noKey", &message)
        }
        Ok(s3) => {
            if let Err(error) = s3.list_objects_v2("", Some("/"), None) {
                report.s3 = CheckResult::fail(
                    ACTION_LIST_BUCKET,
                    &label,
                    s3_kind(error.kind),
                    &error.message,
                );
            }
        }
    }

    match clients.ses(&account) {
        Err(message) => {
            report.ses_identities =
                CheckResult::fail(ACTION_LIST_IDENTITIES, &label, "noKey", &message);
            report.ses_account = CheckResult::fail(ACTION_GET_ACCOUNT, &label, "noKey", &message);
        }
        Ok(ses) => {
            match ses.list_identities() {
                Ok(found) => {
                    // A passing test refreshes the cache, so fixing an IAM
                    // policy and pressing Test is enough to unlock sending.
                    store_identities(store, &account, &found)?;
                    report.identity_count = Some(found.len());
                }
                Err(error) => {
                    report.ses_identities = CheckResult::fail(
                        ACTION_LIST_IDENTITIES,
                        &label,
                        ses_kind(error.kind),
                        &error.message,
                    );
                }
            }
            match ses.get_account() {
                Ok(info) => report.sandbox = Some(info.sandbox),
                Err(error) => {
                    report.ses_account = CheckResult::fail(
                        ACTION_GET_ACCOUNT,
                        &label,
                        ses_kind(error.kind),
                        &error.message,
                    );
                }
            }
        }
    }
    Ok(report)
}

/// Top-level prefixes of the bucket (`owner/`, `seller/`), so the settings page
/// can offer them instead of asking the user to type.
pub(crate) fn detect_prefixes(
    store: &SessionStore,
    clients: &dyn AwsClients,
    id: i64,
) -> Result<Vec<String>, String> {
    let account = {
        let conn = store.lock_conn()?;
        mail_store::get_account(&conn, id)
            .map_err(db)?
            .ok_or_else(|| "That source no longer exists.".to_string())?
    };
    let s3 = clients.s3(&account)?;
    let mut prefixes = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..MAX_PREFIX_PAGES {
        let page = s3
            .list_objects_v2("", Some("/"), token.as_deref())
            .map_err(|e| format!("{}: {}", account.label, e.message))?;
        prefixes.extend(page.common_prefixes);
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    prefixes.sort();
    prefixes.dedup();
    Ok(prefixes)
}

// ── status ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxStatus {
    #[serde(flatten)]
    pub mailbox: MailMailbox,
    pub unread: i64,
    pub sync: Option<MailSyncState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatus {
    #[serde(flatten)]
    pub account: MailAccount,
    /// Masked ids only.
    pub credential: Option<MailCredential>,
    pub s3_credential: Option<MailCredential>,
    pub identities_refreshed_at: Option<i64>,
    pub mailboxes: Vec<MailboxStatus>,
    pub unread: i64,
    /// Epoch milliseconds of the newest finished sync across the mailboxes.
    pub last_sync_at: Option<i64>,
    /// The first mailbox error, so the sidebar can badge the source.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailStatus {
    pub sources: Vec<SourceStatus>,
    /// Over enabled sources only.
    pub unread_total: i64,
    pub sync_interval_secs: u64,
}

pub(crate) fn status(store: &SessionStore, sync_interval_secs: u64) -> Result<MailStatus, String> {
    let conn = store.lock_conn()?;
    let credentials = mail_store::list_credentials(&conn).map_err(db)?;
    let unread = mail_store::unread_by_mailbox(&conn).map_err(db)?;
    let find = |id: i64| credentials.iter().find(|c| c.id == id).cloned();
    let mut sources = Vec::new();
    let mut unread_total = 0;
    for account in mail_store::list_accounts(&conn).map_err(db)? {
        let mut mailboxes = Vec::new();
        for mailbox in mail_store::list_mailboxes(&conn, Some(account.id)).map_err(db)? {
            mailboxes.push(MailboxStatus {
                unread: unread.get(&mailbox.id).copied().unwrap_or(0),
                sync: mail_store::get_sync_state(&conn, mailbox.id).map_err(db)?,
                mailbox,
            });
        }
        let source_unread: i64 = mailboxes.iter().map(|m| m.unread).sum();
        if account.enabled {
            unread_total += source_unread;
        }
        sources.push(SourceStatus {
            credential: find(account.credential_id),
            s3_credential: account.s3_credential_id.and_then(find),
            identities_refreshed_at: mail_store::identities_refreshed_at(&conn, account.id)
                .map_err(db)?,
            unread: source_unread,
            last_sync_at: mailboxes
                .iter()
                .filter_map(|m| m.sync.as_ref()?.last_finished_at)
                .max(),
            last_error: mailboxes
                .iter()
                .find_map(|m| m.sync.as_ref()?.last_error.clone()),
            mailboxes,
            account,
        });
    }
    Ok(MailStatus {
        sources,
        unread_total,
        sync_interval_secs,
    })
}

// ── sync ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SyncScope {
    All,
    Source { id: i64 },
    Mailbox { id: i64 },
}

/// `Mailbox` is wider than it sounds: it syncs the whole source that owns the
/// mailbox, because the sync service works per source. Pressing sync on
/// `owner@` therefore also rewrites the sync state, last-sync time and error
/// badge of `support@` and every other mailbox in that source. A per-mailbox
/// control must not promise otherwise.
fn scope_account(store: &SessionStore, scope: SyncScope) -> Result<Option<i64>, String> {
    match scope {
        SyncScope::All => Ok(None),
        SyncScope::Source { id } => Ok(Some(id)),
        SyncScope::Mailbox { id } => {
            let conn = store.lock_conn()?;
            mail_store::mailbox_account_id(&conn, id)
                .map(Some)
                .map_err(|_| "That mailbox no longer exists.".to_string())
        }
    }
}

// ── reading ────────────────────────────────────────────────────────

pub(crate) fn thread_detail(store: &SessionStore, thread_id: i64) -> Result<ThreadDetail, String> {
    let conn = store.lock_conn()?;
    mail_store::thread_detail(&conn, thread_id)
        .map_err(db)?
        .ok_or_else(|| "That conversation no longer exists.".to_string())
}

pub(crate) fn raw_message(
    store: &SessionStore,
    clients: &dyn AwsClients,
    message_id: i64,
) -> Result<String, String> {
    let (account, key) = {
        let conn = store.lock_conn()?;
        let location = mail_store::message_location(&conn, message_id)
            .map_err(db)?
            .ok_or_else(|| "That message no longer exists.".to_string())?;
        let key = location
            .s3_key
            .ok_or_else(|| "This message has no stored source.".to_string())?;
        let account = mail_store::get_account(&conn, location.account_id)
            .map_err(db)?
            .ok_or_else(|| "That source no longer exists.".to_string())?;
        (account, key)
    };
    let raw = clients
        .s3(&account)?
        .get_object(&key)
        .map_err(|e| format!("{}: {}", account.label, e.message))?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// A name that is safe to create on any platform: one path component, no
/// control or reserved characters, no leading dot.
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !":*?\"<>|".contains(*c))
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let name: String = trimmed.chars().take(100).collect();
    if name.is_empty() {
        "attachment".to_string()
    } else {
        name
    }
}

fn extract_part(raw: &[u8], part_index: usize) -> Result<Vec<u8>, String> {
    let gone = || "The attachment is no longer in the message.".to_string();
    let message = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or_else(gone)?;
    if !message.attachments.contains(&(part_index as u32)) {
        return Err(gone());
    }
    Ok(message
        .parts
        .get(part_index)
        .ok_or_else(gone)?
        .contents()
        .to_vec())
}

/// The attachment on disk, fetched from S3 the first time. `root` is
/// `<app data>/mail/attachments`.
pub(crate) fn ensure_attachment(
    store: &SessionStore,
    clients: &dyn AwsClients,
    root: &Path,
    attachment_id: i64,
) -> Result<PathBuf, String> {
    let (attachment, stored, account, key) = {
        let conn = store.lock_conn()?;
        let (attachment, stored) = mail_store::get_attachment(&conn, attachment_id)
            .map_err(db)?
            .ok_or_else(|| "That attachment no longer exists.".to_string())?;
        let location = mail_store::message_location(&conn, attachment.message_id)
            .map_err(db)?
            .ok_or_else(|| "That message no longer exists.".to_string())?;
        let account = mail_store::get_account(&conn, location.account_id)
            .map_err(db)?
            .ok_or_else(|| "That source no longer exists.".to_string())?;
        (attachment, stored, account, location.s3_key)
    };
    if let Some(path) = stored.map(PathBuf::from) {
        if path.starts_with(root) && path.is_file() {
            return Ok(path);
        }
    }
    let key = key.ok_or_else(|| "This message has no stored source.".to_string())?;
    let raw = clients
        .s3(&account)?
        .get_object(&key)
        .map_err(|e| format!("{}: {}", account.label, e.message))?;
    let bytes = extract_part(&raw, attachment.part_index as usize)?;
    let dir = root.join(attachment.message_id.to_string());
    std::fs::create_dir_all(&dir)
        .map_err(|_| "Could not create the attachment folder.".to_string())?;
    let path = dir.join(format!(
        "{}-{}",
        attachment.part_index,
        safe_file_name(&attachment.filename)
    ));
    std::fs::write(&path, bytes).map_err(|_| "Could not save the attachment.".to_string())?;
    let conn = store.lock_conn()?;
    mail_store::set_attachment_path(&conn, attachment_id, Some(&path.to_string_lossy()))
        .map_err(db)?;
    Ok(path)
}

fn remove_attachment_dirs(root: &Path, message_ids: &[i64]) {
    for id in message_ids {
        let _ = std::fs::remove_dir_all(root.join(id.to_string()));
    }
}

fn attachments_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("mail")
        .join("attachments"))
}

// ── remote images ──────────────────────────────────────────────────

fn unescape_attr(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Whether `url` is one the sanitiser blocked in this message. Only those may
/// be fetched, so the command is not a general-purpose proxy for the webview.
fn is_blocked_url(body_html: &str, url: &str) -> bool {
    let mut rest = body_html;
    while let Some(start) = rest.find(mail_store::BLOCKED_IMAGE_ATTR) {
        rest = &rest[start + mail_store::BLOCKED_IMAGE_ATTR.len()..];
        let Some(end) = rest.find('"') else { break };
        if unescape_attr(&rest[..end]) == url {
            return true;
        }
        rest = &rest[end..];
    }
    false
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                // 100.64.0.0/10, carrier-grade NAT
                || (a == 100 && (64..128).contains(&b))
                || a == 0)
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_ip(IpAddr::V4(v4)),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

/// A sender chooses these URLs, so the fetch must not reach the user's own
/// network. Every address the host resolves to has to be public. The check is
/// made on the name and again on each redirect hop; a resolver that answers
/// differently a moment later (DNS rebinding) is not covered.
fn guard_url(url: &url::Url) -> Result<(), String> {
    let refuse = || "That image address is not allowed.".to_string();
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() {
        return Err(refuse());
    }
    let host = url.host_str().ok_or_else(refuse)?.to_ascii_lowercase();
    if host == "localhost"
        || [".localhost", ".local", ".internal", ".localdomain"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
    {
        return Err(refuse());
    }
    let port = url.port_or_known_default().ok_or_else(refuse)?;
    let addresses: Vec<_> = (host.trim_matches(['[', ']']), port)
        .to_socket_addrs()
        .map_err(|_| "Could not look up the image host.".to_string())?
        .collect();
    if addresses.is_empty() || !addresses.iter().all(|a| is_public_ip(a.ip())) {
        return Err(refuse());
    }
    Ok(())
}

fn fetch_remote_image(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let agent = ureq::AgentBuilder::new()
        .timeout(IMAGE_TIMEOUT)
        .redirects(0)
        .build();
    let mut current =
        url::Url::parse(url).map_err(|_| "That image address is not valid.".to_string())?;
    for _ in 0..=MAX_IMAGE_REDIRECTS {
        guard_url(&current)?;
        let response = match agent
            .get(current.as_str())
            .set("Accept", "image/*")
            .set("User-Agent", "Monoptah")
            .call()
        {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) if (300..400).contains(&status) => response,
            Err(ureq::Error::Status(status, _)) => {
                return Err(format!("The image request failed ({status})."))
            }
            Err(_) => return Err("Could not fetch the image.".into()),
        };
        if (300..400).contains(&response.status()) {
            let location = response
                .header("Location")
                .ok_or_else(|| "The image redirect has no destination.".to_string())?;
            current = current
                .join(location)
                .map_err(|_| "The image redirect is not valid.".to_string())?;
            continue;
        }
        let content_type = response
            .header("Content-Type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        // SVG is a document that can carry script and further fetches.
        if !content_type.starts_with("image/") || content_type.starts_with("image/svg") {
            return Err("That address is not a displayable image.".into());
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Could not read the image.".to_string())?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("That image is too large to show.".into());
        }
        return Ok(bytes);
    }
    Err("The image redirected too many times.".into())
}

pub(crate) fn remote_image(
    store: &SessionStore,
    message_id: i64,
    url: &str,
) -> Result<Vec<u8>, String> {
    {
        let conn = store.lock_conn()?;
        let location = mail_store::message_location(&conn, message_id)
            .map_err(db)?
            .ok_or_else(|| "That message no longer exists.".to_string())?;
        let policy = mail_store::get_account(&conn, location.account_id)
            .map_err(db)?
            .map(|a| a.image_policy);
        if policy.as_deref() == Some("never") {
            return Err("Remote images are turned off for this source.".into());
        }
        if !location
            .body_html
            .as_deref()
            .is_some_and(|html| is_blocked_url(html, url))
        {
            return Err("That image is not part of this message.".into());
        }
    }
    fetch_remote_image(url)
}

// ── commands ───────────────────────────────────────────────────────

/// Runs blocking network work off the async runtime's threads, with the managed
/// state fetched inside so nothing borrowed crosses the thread boundary.
async fn blocking<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&SessionStore, &MailState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = app.state::<SessionStore>();
        let mail = app.state::<MailState>();
        work(&store, &mail)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(async)]
pub fn mail_status(
    store: State<'_, SessionStore>,
    mail: State<'_, MailState>,
) -> Result<MailStatus, String> {
    status(&store, mail.sync.interval_secs())
}

#[tauri::command(async)]
pub fn mail_credentials(store: State<'_, SessionStore>) -> Result<Vec<MailCredential>, String> {
    let conn = store.lock_conn()?;
    mail_store::list_credentials(&conn).map_err(db)
}

#[tauri::command(async)]
pub fn mail_credential_upsert(
    store: State<'_, SessionStore>,
    mail: State<'_, MailState>,
    input: CredentialUpsert,
) -> Result<MailCredential, String> {
    credential_upsert(&store, &*mail.secrets, input)
}

#[tauri::command(async)]
pub fn mail_credential_delete(
    store: State<'_, SessionStore>,
    mail: State<'_, MailState>,
    id: i64,
) -> Result<(), String> {
    credential_delete(&store, &*mail.secrets, id)
}

#[tauri::command(async)]
pub fn mail_sources(store: State<'_, SessionStore>) -> Result<Vec<SourceRow>, String> {
    sources(&store)
}

#[tauri::command(async)]
pub fn mail_source_upsert(
    store: State<'_, SessionStore>,
    source: AccountInput,
) -> Result<SourceRow, String> {
    source_upsert(&store, source)
}

#[tauri::command(async)]
pub fn mail_source_delete(
    app: AppHandle,
    store: State<'_, SessionStore>,
    id: i64,
) -> Result<(), String> {
    let messages = source_delete(&store, id)?;
    remove_attachment_dirs(&attachments_root(&app)?, &messages);
    Ok(())
}

#[tauri::command]
pub async fn mail_source_test(app: AppHandle, id: i64) -> Result<TestReport, String> {
    blocking(app, move |store, mail| {
        source_test(store, &*mail.clients, id)
    })
    .await
}

#[tauri::command]
pub async fn mail_source_detect_prefixes(app: AppHandle, id: i64) -> Result<Vec<String>, String> {
    blocking(app, move |store, mail| {
        detect_prefixes(store, &*mail.clients, id)
    })
    .await
}

#[tauri::command(async)]
pub fn mail_mailboxes_upsert(
    store: State<'_, SessionStore>,
    account_id: i64,
    rows: Vec<MailboxInput>,
) -> Result<Vec<MailMailbox>, String> {
    mailboxes_upsert(&store, account_id, rows)
}

#[tauri::command(async)]
pub fn mail_mailboxes_delete(
    app: AppHandle,
    store: State<'_, SessionStore>,
    id: i64,
) -> Result<(), String> {
    let messages = mailboxes_delete(&store, id)?;
    remove_attachment_dirs(&attachments_root(&app)?, &messages);
    Ok(())
}

/// Syncs one source or all of them. A `mailbox` scope syncs that mailbox's whole
/// source (see [`scope_account`]); the reports are per source.
#[tauri::command]
pub async fn mail_sync_now(app: AppHandle, scope: SyncScope) -> Result<Vec<SyncReport>, String> {
    blocking(app.clone(), move |store, mail| {
        let account_id = scope_account(store, scope)?;
        Ok(mail
            .sync
            .sync_now(store, &*mail.factory, &TauriEvents(app), account_id))
    })
    .await
}

/// The sync interval is a webview setting (`monocode.mail.syncIntervalSeconds`)
/// that Rust cannot read, so the webview pushes it at startup and on change.
#[tauri::command]
pub fn mail_sync_interval_set(mail: State<'_, MailState>, secs: u64) {
    mail.sync.set_interval_secs(secs);
}

#[tauri::command(async)]
pub fn mail_identities(
    store: State<'_, SessionStore>,
    account_id: Option<i64>,
) -> Result<Vec<IdentityRow>, String> {
    identities(&store, account_id)
}

#[tauri::command(async)]
pub fn mail_threads(
    store: State<'_, SessionStore>,
    query: ThreadQuery,
) -> Result<Page<ThreadRow>, String> {
    let conn = store.lock_conn()?;
    mail_store::list_threads(&conn, &query).map_err(db)
}

#[tauri::command(async)]
pub fn mail_thread(store: State<'_, SessionStore>, thread_id: i64) -> Result<ThreadDetail, String> {
    thread_detail(&store, thread_id)
}

#[tauri::command]
pub async fn mail_message_raw(app: AppHandle, message_id: i64) -> Result<String, String> {
    blocking(app, move |store, mail| {
        raw_message(store, &*mail.clients, message_id)
    })
    .await
}

#[tauri::command(async)]
pub fn mail_mark(
    store: State<'_, SessionStore>,
    thread_ids: Vec<i64>,
    read: bool,
) -> Result<(), String> {
    let conn = store.lock_conn()?;
    mail_store::set_threads_read(&conn, &thread_ids, read).map_err(db)
}

#[tauri::command(async)]
pub fn mail_star(
    store: State<'_, SessionStore>,
    thread_ids: Vec<i64>,
    starred: bool,
) -> Result<(), String> {
    let conn = store.lock_conn()?;
    mail_store::set_threads_starred(&conn, &thread_ids, starred).map_err(db)
}

/// Downloads the attachment if needed and returns where it is. Opening it is
/// the webview's call, through the same default-app path other files use.
#[tauri::command]
pub async fn mail_attachment_open(app: AppHandle, attachment_id: i64) -> Result<String, String> {
    let root = attachments_root(&app)?;
    blocking(app, move |store, mail| {
        ensure_attachment(store, &*mail.clients, &root, attachment_id)
            .map(|path| path.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
pub async fn mail_attachment_bytes(
    app: AppHandle,
    attachment_id: i64,
) -> Result<tauri::ipc::Response, String> {
    let root = attachments_root(&app)?;
    let bytes = blocking(app, move |store, mail| {
        let path = ensure_attachment(store, &*mail.clients, &root, attachment_id)?;
        std::fs::read(path).map_err(|_| "Could not read the attachment.".to_string())
    })
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn mail_remote_image(
    app: AppHandle,
    message_id: i64,
    url: String,
) -> Result<tauri::ipc::Response, String> {
    let bytes = blocking(app, move |store, _| remote_image(store, message_id, &url)).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_s3::{DeleteOutcome, ObjectPage, S3Error};
    use crate::aws_ses::{SesAccount, SesError};
    use crate::mail_mime::parse_mail;
    use crate::mail_store::{ListedObject, ThreadCursor, ThreadScope};
    use crate::mail_thread::ingest_message;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    const KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";
    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    #[derive(Default)]
    struct MemSecrets {
        map: Mutex<HashMap<i64, (String, String)>>,
        fail: AtomicBool,
        fail_remove: AtomicBool,
    }

    impl SecretStore for MemSecrets {
        fn put(&self, id: i64, keys: &KeyPair) -> Result<(), String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("keychain unavailable".into());
            }
            self.map.lock().unwrap().insert(
                id,
                (keys.access_key_id.clone(), keys.secret_access_key.clone()),
            );
            Ok(())
        }
        fn get(&self, id: i64) -> Result<Option<KeyPair>, String> {
            Ok(self.map.lock().unwrap().get(&id).map(|(a, s)| KeyPair {
                access_key_id: a.clone(),
                secret_access_key: s.clone(),
            }))
        }
        fn remove(&self, id: i64) -> Result<(), String> {
            if self.fail_remove.load(Ordering::SeqCst) {
                return Err("keychain unavailable".into());
            }
            self.map.lock().unwrap().remove(&id);
            Ok(())
        }
    }

    struct FakeS3 {
        list: Result<ObjectPage, S3Error>,
        objects: HashMap<String, Vec<u8>>,
    }

    impl S3Api for FakeS3 {
        fn list_objects_v2(
            &self,
            _prefix: &str,
            _delimiter: Option<&str>,
            token: Option<&str>,
        ) -> Result<ObjectPage, S3Error> {
            let mut page = self.list.clone()?;
            // Two pages when a token is expected, to prove the loop follows it.
            if page.next_token.is_some() && token.is_some() {
                page.next_token = None;
                page.common_prefixes = vec!["seller/".into()];
            }
            Ok(page)
        }
        fn get_object(&self, key: &str) -> Result<Vec<u8>, S3Error> {
            self.objects.get(key).cloned().ok_or(S3Error {
                kind: S3ErrorKind::NoSuchKey,
                code: None,
                message: "gone".into(),
            })
        }
        fn put_object(&self, _: &str, _: &[u8], _: &str) -> Result<(), S3Error> {
            Ok(())
        }
        fn delete_objects(&self, _: &[String]) -> Vec<DeleteOutcome> {
            Vec::new()
        }
    }

    struct FakeSes {
        identities: Vec<MailIdentity>,
        sandbox: bool,
        denied: bool,
    }

    impl SesApi for FakeSes {
        fn list_identities(&self) -> Result<Vec<MailIdentity>, SesError> {
            if self.denied {
                return Err(SesError {
                    kind: SesErrorKind::AccessDenied,
                    message: "Access denied. The key pair needs ses:ListEmailIdentities.".into(),
                });
            }
            Ok(self.identities.clone())
        }
        fn get_account(&self) -> Result<SesAccount, SesError> {
            Ok(SesAccount {
                sandbox: self.sandbox,
                sending_enabled: true,
            })
        }
    }

    struct FakeClients {
        list: Result<ObjectPage, S3Error>,
        objects: HashMap<String, Vec<u8>>,
        /// Verified domains per source id.
        verified: HashMap<i64, Vec<&'static str>>,
        ses_denied: bool,
        no_key: bool,
        refreshes: Mutex<Vec<i64>>,
    }

    impl Default for FakeClients {
        fn default() -> Self {
            Self {
                list: Ok(ObjectPage {
                    objects: Vec::new(),
                    common_prefixes: vec!["owner/".into()],
                    next_token: None,
                }),
                objects: HashMap::new(),
                verified: HashMap::new(),
                ses_denied: false,
                no_key: false,
                refreshes: Mutex::new(Vec::new()),
            }
        }
    }

    impl AwsClients for FakeClients {
        fn s3(&self, _: &MailAccount) -> Result<Box<dyn S3Api + Send>, String> {
            if self.no_key {
                return Err("The AWS key for this source is missing.".into());
            }
            Ok(Box::new(FakeS3 {
                list: self.list.clone(),
                objects: self.objects.clone(),
            }))
        }
        fn ses(&self, account: &MailAccount) -> Result<Box<dyn SesApi + Send>, String> {
            if self.no_key {
                return Err("The AWS key for this source is missing.".into());
            }
            self.refreshes.lock().unwrap().push(account.id);
            Ok(Box::new(FakeSes {
                identities: self
                    .verified
                    .get(&account.id)
                    .into_iter()
                    .flatten()
                    .map(|d| MailIdentity {
                        identity: d.to_string(),
                        kind: "domain".into(),
                        status: "verified".into(),
                    })
                    .collect(),
                sandbox: true,
                denied: self.ses_denied,
            }))
        }
    }

    fn keys(label: &str, key_id: &str) -> CredentialUpsert {
        CredentialUpsert {
            label: label.into(),
            access_key_id: key_id.into(),
            secret_access_key: SECRET.into(),
            default_region: Some("us-east-1".into()),
        }
    }

    fn credential(store: &SessionStore, secrets: &MemSecrets, label: &str) -> MailCredential {
        credential_upsert(store, secrets, keys(label, KEY_ID)).unwrap()
    }

    fn account_input(credential_id: i64, label: &str, region: &str, bucket: &str) -> AccountInput {
        AccountInput {
            id: None,
            provider: "aws".into(),
            credential_id,
            s3_credential_id: None,
            label: label.into(),
            ses_region: region.into(),
            bucket: bucket.into(),
            s3_region: region.into(),
            default_from: None,
            attach_policy: "on_demand".into(),
            image_policy: "ask".into(),
            enabled: true,
            sort_order: 0,
        }
    }

    fn mailbox_input(address: &str, prefix: &str) -> MailboxInput {
        MailboxInput {
            address: address.into(),
            s3_prefix: prefix.into(),
            display_name: String::new(),
            color: None,
            sort_order: 0,
        }
    }

    fn setup() -> (SessionStore, MemSecrets) {
        (
            SessionStore::open_in_memory().unwrap(),
            MemSecrets::default(),
        )
    }

    fn raw_mail(subject: &str, id: &str, body: &str) -> Vec<u8> {
        format!(
            "From: Ann <ann@example.org>\r\nTo: owner@byptah.com\r\nSubject: {subject}\r\n\
             Message-ID: <{id}@example.org>\r\nDate: Tue, 01 Oct 2024 10:00:00 +0000\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    fn ingest(store: &SessionStore, mailbox_id: i64, raw: &[u8], key: &str) -> (i64, i64) {
        let conn = store.lock_conn().unwrap();
        let object = mail_store::insert_object(
            &conn,
            mailbox_id,
            &ListedObject {
                s3_key: key,
                etag: "e",
                size_bytes: raw.len() as i64,
                s3_modified: Some(1),
            },
            1,
        )
        .unwrap();
        let mail = parse_mail(raw).unwrap();
        let done = ingest_message(&conn, mailbox_id, "inbound", Some(object), &mail, 1).unwrap();
        (done.message_id, done.thread_id)
    }

    fn one_source(store: &SessionStore, secrets: &MemSecrets) -> (SourceRow, MailMailbox) {
        let cred = credential(store, secrets, "personal");
        let source = source_upsert(
            store,
            account_input(cred.id, "byptah (us-east-1)", "us-east-1", "byptah-mail"),
        )
        .unwrap();
        let boxes = mailboxes_upsert(
            store,
            source.account.id,
            vec![mailbox_input("owner@byptah.com", "owner/")],
        )
        .unwrap();
        (source, boxes[0].clone())
    }

    #[test]
    fn status_and_credentials_carry_no_secret() {
        let (store, secrets) = setup();
        let (_, mailbox) = one_source(&store, &secrets);
        ingest(
            &store,
            mailbox.id,
            &raw_mail("Hi", "a1", "hello"),
            "owner/a1",
        );
        let json = format!(
            "{}{}{}",
            serde_json::to_string(&status(&store, 3600).unwrap()).unwrap(),
            serde_json::to_string(&{
                let conn = store.lock_conn().unwrap();
                mail_store::list_credentials(&conn).unwrap()
            })
            .unwrap(),
            serde_json::to_string(&sources(&store).unwrap()).unwrap(),
        );
        assert!(json.contains("AKIA\u{2022}\u{2022}\u{2022}\u{2022}MPLE"));
        assert!(!json.contains(SECRET));
        assert!(!json.contains(KEY_ID));
        assert!(!json.contains("wJalr"));
        let parsed: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&status(&store, 3600).unwrap()).unwrap())
                .unwrap();
        assert_eq!(parsed["unreadTotal"], 1);
        assert_eq!(parsed["sources"][0]["mailboxes"][0]["unread"], 1);
    }

    #[test]
    fn credential_validation_never_echoes_input() {
        let (store, secrets) = setup();
        let mut bad = keys("x", KEY_ID);
        bad.secret_access_key = "short secret".into();
        let error = credential_upsert(&store, &secrets, bad).unwrap_err();
        assert!(!error.contains("short secret"));
        let mut session = keys("x", "ASIAIOSFODNN7EXAMPLE");
        session.default_region = None;
        assert!(credential_upsert(&store, &secrets, session)
            .unwrap_err()
            .contains("Temporary"));
    }

    #[test]
    fn a_failed_keychain_write_rolls_the_row_back() {
        let (store, secrets) = setup();
        secrets.fail.store(true, Ordering::SeqCst);
        assert!(credential_upsert(&store, &secrets, keys("a", KEY_ID)).is_err());
        let conn = store.lock_conn().unwrap();
        assert!(mail_store::list_credentials(&conn).unwrap().is_empty());
        drop(conn);

        secrets.fail.store(false, Ordering::SeqCst);
        let first = credential(&store, &secrets, "a");
        secrets.fail.store(true, Ordering::SeqCst);
        let rotated = keys("a", "AKIAJ5Q2ZPLMNBVCXZ9K");
        assert!(credential_upsert(&store, &secrets, rotated).is_err());
        let conn = store.lock_conn().unwrap();
        let kept = mail_store::get_credential(&conn, first.id)
            .unwrap()
            .unwrap();
        assert_eq!(kept.access_key_masked, first.access_key_masked);
    }

    #[test]
    fn deleting_a_used_credential_is_refused_and_names_the_source() {
        let (store, secrets) = setup();
        let (source, _) = one_source(&store, &secrets);
        let id = source.account.credential_id;
        let error = credential_delete(&store, &secrets, id).unwrap_err();
        assert!(error.contains("byptah (us-east-1)"), "{error}");
        assert!(secrets.get(id).unwrap().is_some());

        source_delete(&store, source.account.id).unwrap();
        credential_delete(&store, &secrets, id).unwrap();
        assert!(secrets.get(id).unwrap().is_none());
    }

    #[test]
    fn a_failed_secret_removal_keeps_the_credential_row() {
        let (store, secrets) = setup();
        let cred = credential(&store, &secrets, "spare");
        secrets.fail_remove.store(true, Ordering::SeqCst);
        assert!(credential_delete(&store, &secrets, cred.id).is_err());
        let conn = store.lock_conn().unwrap();
        assert!(mail_store::get_credential(&conn, cred.id)
            .unwrap()
            .is_some());
        drop(conn);
        secrets.fail_remove.store(false, Ordering::SeqCst);
        credential_delete(&store, &secrets, cred.id).unwrap();
        assert!(secrets.get(cred.id).unwrap().is_none());
    }

    #[test]
    fn a_used_credential_keeps_its_secret_when_delete_is_refused() {
        let (store, secrets) = setup();
        let (source, _) = one_source(&store, &secrets);
        let id = source.account.credential_id;
        assert!(credential_delete(&store, &secrets, id).is_err());
        assert!(secrets.get(id).unwrap().is_some());
    }

    /// A thread with one visible message and one `gone` one.
    fn thread_with_a_gone_message(store: &SessionStore, secrets: &MemSecrets) -> (i64, i64, i64) {
        let (_, mailbox) = one_source(store, secrets);
        let (visible, thread) = ingest(store, mailbox.id, &raw_mail("Hi", "g1", "one"), "owner/g1");
        let raw = String::from_utf8(raw_mail("Re: Hi", "g2", "two"))
            .unwrap()
            .replace("Subject:", "In-Reply-To: <g1@example.org>\r\nSubject:");
        let (gone, thread2) = ingest(store, mailbox.id, raw.as_bytes(), "owner/g2");
        assert_eq!(thread, thread2);
        let conn = store.lock_conn().unwrap();
        conn.execute(
            "UPDATE mail_message SET state = 'gone' WHERE id = ?1",
            [gone],
        )
        .unwrap();
        (visible, gone, thread)
    }

    // Covers `thread_message_rows`' and the recount's `state != 'gone'` filters:
    // each assertion fails if its filter is removed.
    #[test]
    fn marking_read_clears_the_badge_despite_an_unread_gone_message() {
        let (store, secrets) = setup();
        let (_, _, thread) = thread_with_a_gone_message(&store, &secrets);
        {
            let conn = store.lock_conn().unwrap();
            mail_store::set_threads_read(&conn, &[thread], true).unwrap();
            // set_threads_read leaves the gone row alone, so it is still unread
            // and only the recount's filter keeps it out of the badge.
            let gone_unread: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM mail_message WHERE state = 'gone' AND is_read = 0",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(gone_unread, 1);
        }
        let detail = thread_detail(&store, thread).unwrap();
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.thread.unread_count, 0);
    }

    #[test]
    fn marking_unread_counts_only_the_messages_that_can_be_opened() {
        let (store, secrets) = setup();
        let (visible, gone, thread) = thread_with_a_gone_message(&store, &secrets);
        let conn = store.lock_conn().unwrap();
        conn.execute(
            "UPDATE mail_message SET is_read = 1 WHERE id = ?1",
            [visible],
        )
        .unwrap();
        conn.execute("UPDATE mail_message SET is_read = 0 WHERE id = ?1", [gone])
            .unwrap();
        mail_store::set_threads_read(&conn, &[thread], false).unwrap();
        let unread: i64 = conn
            .query_row(
                "SELECT unread_count FROM mail_thread WHERE id = ?1",
                [thread],
                |row| row.get(0),
            )
            .unwrap();
        // One visible message, so a badge of 2 would be one nobody can clear.
        assert_eq!(unread, 1);
        let rows = mail_store::thread_message_rows(&conn, thread).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn mailbox_scope_resolves_to_the_owning_source() {
        let (store, secrets) = setup();
        let (source, mailbox) = one_source(&store, &secrets);
        let other_cred = credential(&store, &secrets, "second");
        let other = source_upsert(
            &store,
            account_input(other_cred.id, "acme", "eu-west-1", "acme-mail"),
        )
        .unwrap();
        assert_eq!(scope_account(&store, SyncScope::All).unwrap(), None);
        assert_eq!(
            scope_account(
                &store,
                SyncScope::Source {
                    id: other.account.id
                }
            )
            .unwrap(),
            Some(other.account.id)
        );
        assert_eq!(
            scope_account(&store, SyncScope::Mailbox { id: mailbox.id }).unwrap(),
            Some(source.account.id)
        );
        assert!(scope_account(&store, SyncScope::Mailbox { id: 9999 }).is_err());
    }

    #[test]
    fn a_duplicate_address_across_sources_names_the_holder() {
        let (store, secrets) = setup();
        let (_, _) = one_source(&store, &secrets);
        let cred = credential(&store, &secrets, "second");
        let other = source_upsert(
            &store,
            account_input(cred.id, "acme (eu-west-1)", "eu-west-1", "acme-mail"),
        )
        .unwrap();
        let error = mailboxes_upsert(
            &store,
            other.account.id,
            vec![mailbox_input("Owner@Byptah.com", "owner/")],
        )
        .unwrap_err();
        assert!(error.contains("byptah (us-east-1)"), "{error}");
    }

    #[test]
    fn the_same_domain_is_sendable_only_where_it_is_verified() {
        let (store, secrets) = setup();
        let cred = credential(&store, &secrets, "personal");
        let a =
            source_upsert(&store, account_input(cred.id, "A", "us-east-1", "bucket-a")).unwrap();
        let b =
            source_upsert(&store, account_input(cred.id, "B", "eu-west-1", "bucket-b")).unwrap();
        mailboxes_upsert(
            &store,
            a.account.id,
            vec![mailbox_input("owner@byptah.com", "owner/")],
        )
        .unwrap();
        mailboxes_upsert(
            &store,
            b.account.id,
            vec![mailbox_input("support@byptah.com", "support/")],
        )
        .unwrap();
        let mut clients = FakeClients::default();
        clients.verified.insert(a.account.id, vec!["byptah.com"]);
        clients.verified.insert(b.account.id, vec![]);

        // replace_identities stamps the real clock, so the test measures from it.
        let start = now_millis();
        let results = refresh_due_identities(&store, &clients, start);
        assert!(results.iter().all(|(_, r)| r.is_ok()));
        let conn = store.lock_conn().unwrap();
        let sendable = |address: &str| {
            mail_store::list_mailboxes(&conn, None)
                .unwrap()
                .into_iter()
                .find(|m| m.address == address)
                .unwrap()
                .is_sendable
        };
        assert!(sendable("owner@byptah.com"));
        assert!(!sendable("support@byptah.com"));
        drop(conn);

        // Fresh identities are not fetched again until a day has passed.
        let before = clients.refreshes.lock().unwrap().len();
        refresh_due_identities(&store, &clients, start + DAY_MS - 1);
        assert_eq!(clients.refreshes.lock().unwrap().len(), before);
        refresh_due_identities(&store, &clients, start + DAY_MS + 1_000);
        assert_eq!(clients.refreshes.lock().unwrap().len(), before + 2);
    }

    #[test]
    fn repointing_a_source_clears_its_identities() {
        let (store, secrets) = setup();
        let (source, _) = one_source(&store, &secrets);
        let mut clients = FakeClients::default();
        clients
            .verified
            .insert(source.account.id, vec!["byptah.com"]);
        refresh_identities(&store, &clients, &source.account).unwrap();
        assert_eq!(
            identities(&store, Some(source.account.id)).unwrap().len(),
            1
        );

        let mut edit = account_input(
            source.account.credential_id,
            "renamed",
            "us-east-1",
            "byptah-mail",
        );
        edit.id = Some(source.account.id);
        let kept = source_upsert(&store, edit.clone()).unwrap();
        assert!(kept.identities_refreshed_at.is_some());

        edit.ses_region = "eu-west-1".into();
        let moved = source_upsert(&store, edit).unwrap();
        assert_eq!(moved.identities_refreshed_at, None);
        assert!(identities(&store, Some(source.account.id))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn source_test_reports_each_check_and_names_the_real_region() {
        let (store, secrets) = setup();
        let (source, _) = one_source(&store, &secrets);
        let mut clients = FakeClients {
            list: Err(S3Error {
                kind: S3ErrorKind::WrongRegion,
                code: Some("PermanentRedirect".into()),
                message: "The bucket is in region eu-west-1. Change this source's region to match."
                    .into(),
            }),
            ses_denied: true,
            ..Default::default()
        };
        let report = source_test(&store, &clients, source.account.id).unwrap();
        assert!(!report.s3.ok);
        assert_eq!(report.s3.kind, Some("wrongRegion"));
        assert_eq!(report.s3.action, "s3:ListBucket");
        let message = report.s3.message.unwrap();
        assert!(message.contains("eu-west-1") && message.contains("byptah (us-east-1)"));
        assert!(!report.ses_identities.ok);
        assert_eq!(report.ses_identities.action, "ses:ListEmailIdentities");
        assert!(report
            .ses_identities
            .message
            .unwrap()
            .contains("byptah (us-east-1)"));
        assert!(report.ses_account.ok);
        assert_eq!(report.sandbox, Some(true));

        clients.list = FakeClients::default().list;
        clients.ses_denied = false;
        clients
            .verified
            .insert(source.account.id, vec!["byptah.com"]);
        let report = source_test(&store, &clients, source.account.id).unwrap();
        assert!(report.s3.ok && report.ses_identities.ok);
        assert_eq!(report.identity_count, Some(1));

        clients.no_key = true;
        let report = source_test(&store, &clients, source.account.id).unwrap();
        assert_eq!(report.s3.kind, Some("noKey"));
    }

    #[test]
    fn detect_prefixes_follows_pages_and_dedupes() {
        let (store, secrets) = setup();
        let (source, _) = one_source(&store, &secrets);
        let clients = FakeClients {
            list: Ok(ObjectPage {
                objects: Vec::new(),
                common_prefixes: vec!["owner/".into(), "owner/".into()],
                next_token: Some("t".into()),
            }),
            ..Default::default()
        };
        let found = detect_prefixes(&store, &clients, source.account.id).unwrap();
        assert_eq!(found, vec!["owner/".to_string(), "seller/".to_string()]);
    }

    #[test]
    fn listing_hides_gone_and_archived_and_pages_by_cursor() {
        let (store, secrets) = setup();
        let (_, mailbox) = one_source(&store, &secrets);
        let mut threads = Vec::new();
        for i in 0..4 {
            let raw = raw_mail(&format!("Subject {i}"), &format!("m{i}"), "body");
            threads.push(ingest(&store, mailbox.id, &raw, &format!("owner/{i}")).1);
        }
        let conn = store.lock_conn().unwrap();
        // One thread whose object vanished, one the user archived.
        conn.execute(
            "UPDATE mail_message SET state = 'gone' WHERE thread_id = ?1",
            [threads[0]],
        )
        .unwrap();
        conn.execute(
            "UPDATE mail_message SET state = 'archived' WHERE thread_id = ?1",
            [threads[1]],
        )
        .unwrap();
        let listed = |scope, limit, cursor| {
            mail_store::list_threads(
                &conn,
                &ThreadQuery {
                    scope,
                    cursor,
                    limit,
                },
            )
            .unwrap()
        };
        let all = listed(ThreadScope::All, None, None);
        let ids: Vec<i64> = all.rows.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&threads[0]) && !ids.contains(&threads[1]));

        // Tie on timestamp: the keyset must still walk both rows exactly once.
        let first = listed(
            ThreadScope::Mailbox {
                mailbox_id: mailbox.id,
            },
            Some(1),
            None,
        );
        assert_eq!(first.rows.len(), 1);
        let cursor: ThreadCursor = first.next_cursor.unwrap();
        let second = listed(
            ThreadScope::Mailbox {
                mailbox_id: mailbox.id,
            },
            Some(1),
            Some(cursor),
        );
        assert_eq!(second.rows.len(), 1);
        assert_ne!(first.rows[0].id, second.rows[0].id);
        assert!(second.next_cursor.is_none());

        conn.execute("UPDATE mail_account SET enabled = 0", [])
            .unwrap();
        assert!(listed(ThreadScope::All, None, None).rows.is_empty());
        assert_eq!(
            listed(
                ThreadScope::Mailbox {
                    mailbox_id: mailbox.id
                },
                None,
                None
            )
            .rows
            .len(),
            2
        );
    }

    #[test]
    fn mark_and_star_update_threads_and_messages() {
        let (store, secrets) = setup();
        let (_, mailbox) = one_source(&store, &secrets);
        let (_, thread) = ingest(&store, mailbox.id, &raw_mail("Hi", "a1", "x"), "owner/a1");
        {
            let conn = store.lock_conn().unwrap();
            mail_store::set_threads_read(&conn, &[thread], true).unwrap();
            mail_store::set_threads_starred(&conn, &[thread], true).unwrap();
        }
        let detail = thread_detail(&store, thread).unwrap();
        assert_eq!(detail.thread.unread_count, 0);
        assert!(detail.thread.is_starred);
        assert!(detail.messages[0].is_read && detail.messages[0].is_starred);
        assert_eq!(detail.image_policy, "ask");
        assert!(thread_detail(&store, 9999).is_err());
    }

    #[test]
    fn deleting_a_mailbox_leaves_no_search_rows_behind() {
        let (store, secrets) = setup();
        let (_, mailbox) = one_source(&store, &secrets);
        ingest(
            &store,
            mailbox.id,
            &raw_mail("Quarterly", "q1", "zanzibar"),
            "owner/q1",
        );
        let hits = |store: &SessionStore| -> i64 {
            let conn = store.lock_conn().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM mail_message_fts WHERE mail_message_fts MATCH 'zanzibar'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(hits(&store), 1);
        let removed = mailboxes_delete(&store, mailbox.id).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(hits(&store), 0);
    }

    #[test]
    fn attachments_are_fetched_once_and_saved_under_a_safe_name() {
        let (store, secrets) = setup();
        let (_, mailbox) = one_source(&store, &secrets);
        let raw = "From: a@example.org\r\nTo: owner@byptah.com\r\nSubject: File\r\n\
             Message-ID: <f1@example.org>\r\nDate: Tue, 01 Oct 2024 10:00:00 +0000\r\n\
             MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
             --b\r\nContent-Type: text/plain\r\n\r\nsee attached\r\n\
             --b\r\nContent-Type: application/octet-stream\r\n\
             Content-Disposition: attachment; filename=\"../../evil.sh\"\r\n\
             Content-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--b--\r\n";
        let (message, thread) = ingest(&store, mailbox.id, raw.as_bytes(), "owner/f1");
        let detail = thread_detail(&store, thread).unwrap();
        let attachment = &detail.messages[0].attachments[0];
        assert!(!attachment.downloaded);

        let mut clients = FakeClients::default();
        clients
            .objects
            .insert("owner/f1".into(), raw.as_bytes().to_vec());
        let root =
            std::env::temp_dir().join(format!("monoptah-mail-test-{}", uuid::Uuid::new_v4()));
        let path = ensure_attachment(&store, &clients, &root, attachment.id).unwrap();
        assert!(path.starts_with(&root));
        assert_eq!(path.file_name().unwrap().to_string_lossy(), "2-evil.sh");
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        assert!(thread_detail(&store, thread).unwrap().messages[0].attachments[0].downloaded);

        // Second call uses the file: no S3 object is needed any more.
        clients.objects.clear();
        assert_eq!(
            ensure_attachment(&store, &clients, &root, attachment.id).unwrap(),
            path
        );
        remove_attachment_dirs(&root, &[message]);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn raw_message_is_refetched_from_s3() {
        let (store, secrets) = setup();
        let (_, mailbox) = one_source(&store, &secrets);
        let raw = raw_mail("Hi", "r1", "body");
        let (message, _) = ingest(&store, mailbox.id, &raw, "owner/r1");
        let mut clients = FakeClients::default();
        clients.objects.insert("owner/r1".into(), raw.clone());
        assert_eq!(
            raw_message(&store, &clients, message).unwrap().as_bytes(),
            raw.as_slice()
        );
    }

    #[test]
    fn remote_images_are_limited_to_blocked_urls_on_public_hosts() {
        let html = r#"<img src="" data-mono-src="https://cdn.example.com/a.png?x=1&amp;y=2">"#;
        assert!(is_blocked_url(
            html,
            "https://cdn.example.com/a.png?x=1&y=2"
        ));
        assert!(!is_blocked_url(html, "https://cdn.example.com/other.png"));
        assert!(!is_blocked_url(html, ""));

        let parse = |s: &str| url::Url::parse(s).unwrap();
        for private in [
            "http://127.0.0.1/a.png",
            "http://10.0.0.5/a.png",
            "http://192.168.1.1/a.png",
            "http://169.254.169.254/latest",
            "http://[::1]/a.png",
            "http://[::ffff:127.0.0.1]/a.png",
            "http://localhost/a.png",
            "http://printer.local/a.png",
            "file:///etc/passwd",
            "https://user@8.8.8.8/a.png",
        ] {
            assert!(guard_url(&parse(private)).is_err(), "{private}");
        }
        assert!(guard_url(&parse("https://8.8.8.8/a.png")).is_ok());
    }

    #[test]
    fn remote_image_refuses_when_the_source_turns_images_off() {
        let (store, secrets) = setup();
        let (source, mailbox) = one_source(&store, &secrets);
        let html = r#"<p><img src="" data-mono-src="https://8.8.8.8/a.png"></p>"#;
        let raw = String::from(
            "From: a@example.org\r\nTo: owner@byptah.com\r\nSubject: Img\r\nMessage-ID: <i1@example.org>\r\n\
             Date: Tue, 01 Oct 2024 10:00:00 +0000\r\nContent-Type: text/plain\r\n\r\nx\r\n"
        );
        let (message, _) = ingest(&store, mailbox.id, raw.as_bytes(), "owner/i1");
        {
            let conn = store.lock_conn().unwrap();
            conn.execute("UPDATE mail_message SET body_html = ?1", [html])
                .unwrap();
        }
        let error = remote_image(&store, message, "https://evil.example/x.png").unwrap_err();
        assert!(error.contains("not part of this message"));
        let mut edit = account_input(
            source.account.credential_id,
            "x",
            "us-east-1",
            "byptah-mail",
        );
        edit.id = Some(source.account.id);
        edit.image_policy = "never".into();
        source_upsert(&store, edit).unwrap();
        let error = remote_image(&store, message, "https://8.8.8.8/a.png").unwrap_err();
        assert!(error.contains("turned off"));
    }

    #[test]
    fn source_input_is_validated() {
        let (store, secrets) = setup();
        let cred = credential(&store, &secrets, "personal");
        for (region, bucket) in [("us-east-1/../x", "ok-bucket"), ("us-east-1", "Bad_Bucket")] {
            let input = account_input(cred.id, "x", region, bucket);
            assert!(source_upsert(&store, input).is_err(), "{region} {bucket}");
        }
        let mut missing = account_input(9999, "x", "us-east-1", "ok-bucket");
        assert!(source_upsert(&store, missing.clone()).is_err());
        missing.credential_id = cred.id;
        let first = source_upsert(&store, missing.clone()).unwrap();
        // Same key, region and bucket created again updates, not duplicates.
        let again = source_upsert(&store, missing).unwrap();
        assert_eq!(first.account.id, again.account.id);
    }

    #[test]
    fn file_names_cannot_escape_the_folder() {
        assert_eq!(safe_file_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_file_name("C:\\x\\..\\a?b.txt"), "ab.txt");
        assert_eq!(safe_file_name("..."), "attachment");
        assert_eq!(safe_file_name(""), "attachment");
        assert_eq!(safe_file_name(&"a".repeat(300)).len(), 100);
    }
}
