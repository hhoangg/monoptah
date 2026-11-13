//! The app Claude account a codespace's Host runs under.
//!
//! A project can choose an app Claude account for its codespaces (Cloud settings). The
//! account's credential then reaches the codespace's Host through the same stdin prelude a
//! sandbox uses ([`crate::cloud_sessions::docker_start::ClaudeAuth::prelude`]), prepended to
//! the shared bootstrap script. Because a codespace re-runs that bootstrap from
//! [`super::Connections`] on every pairing, wake, reconnect and Host upgrade, and a run
//! WITHOUT the prelude would put the Host back on the Codespaces secret, the account is
//! resolved inside `Connections` for every codespace bootstrap, through [`CodespaceClaude`].
//!
//! Nothing here is written anywhere: the credential lives in these values only for the
//! length of one job, and is masked from every error that job reports.
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The Claude credential to deliver to one codespace.
pub struct ClaudeDelivery {
    /// The app account id (never a secret).
    pub account: String,
    /// Shell text for the top of the bootstrap script. Travels on stdin only.
    pub prelude: Zeroizing<String>,
    /// The secret inside the prelude, kept to mask it in errors.
    pub secret: Zeroizing<String>,
}

impl ClaudeDelivery {
    /// Identifies what was delivered without holding the secret: the account and a digest of
    /// its current token, so Replace token on the same account counts as a change too.
    pub(crate) fn fingerprint(&self) -> String {
        let digest = Sha256::digest(self.secret.as_bytes());
        let mut text = format!("{}:", self.account);
        for byte in &digest[..8] {
            text.push_str(&format!("{byte:02x}"));
        }
        text
    }
}

/// The fingerprint of an optional delivery; `None` is the Codespaces secret.
pub(crate) fn fingerprint_of(delivery: Option<&ClaudeDelivery>) -> Option<String> {
    delivery.map(ClaudeDelivery::fingerprint)
}

/// The account id inside a fingerprint (`<id>:<digest>`; the digest is empty for a delivery
/// known from the persisted hint only).
pub(crate) fn account_of(fingerprint: &str) -> &str {
    fingerprint
        .rsplit_once(':')
        .map_or(fingerprint, |(id, _)| id)
}

/// Whether two fingerprints (`None`: the Codespaces secret) stand for the same delivery. A
/// fingerprint without a digest only knows the account, so it matches on the id alone.
pub(crate) fn same_delivery(wanted: Option<&str>, delivered: Option<&str>) -> bool {
    match (wanted, delivered) {
        (None, None) => true,
        (Some(wanted), Some(delivered)) => {
            wanted == delivered
                || (account_of(wanted) == account_of(delivered)
                    && (wanted.ends_with(':') || delivered.ends_with(':')))
        }
        _ => false,
    }
}

/// Where a codespace's Claude account comes from. Injectable; the app installs
/// [`crate::cloud_sessions::claude_pin::StoreCodespaceClaude`]. Called from worker threads
/// only (it reads the sealed credential).
pub trait CodespaceClaude: Send + Sync {
    /// The id of the account codespace `name` must run under, WITHOUT reading its credential
    /// (cheap: a connected codespace compares it with the delivered one before it decrypts
    /// anything). `Err` as in [`Self::resolve`]. The default resolves.
    fn account_id(&self, codespace: &str) -> Result<Option<String>, String> {
        Ok(self.resolve(codespace)?.map(|delivery| delivery.account))
    }

    /// The credential codespace `name` must run under: `Ok(None)` when it follows the
    /// Codespaces secret. `Err` when the chosen account cannot be used (deleted, no token);
    /// the caller must fail rather than fall back to the secret.
    fn resolve(&self, codespace: &str) -> Result<Option<ClaudeDelivery>, String>;

    /// The credential of app account `account`, as codespace `codespace` would run under it
    /// (used to re-deliver the account a Host already runs under when a change cannot be
    /// applied yet). `Err` when that account can no longer be used.
    fn resolve_account(
        &self,
        codespace: &str,
        account: &str,
    ) -> Result<Option<ClaudeDelivery>, String>;

    /// The id of the account the Host of `codespace` was last bootstrapped with, kept across
    /// app restarts (an id only, never a credential). The default keeps nothing.
    fn delivered_hint(&self, codespace: &str) -> Option<String> {
        let _ = codespace;
        None
    }

    /// Records the account (`None`: the Codespaces secret) the Host of `codespace` was just
    /// bootstrapped with, see [`Self::delivered_hint`]. Best effort.
    fn note_delivered(&self, codespace: &str, account: Option<&str>) {
        let _ = (codespace, account);
    }
}

/// Masks the credential, and the whole prelude it came in, in text that may be shown.
pub(crate) fn mask(text: &str, delivery: Option<&ClaudeDelivery>) -> String {
    match delivery {
        Some(delivery) => mask_parts(text, &delivery.prelude, &delivery.secret),
        None => text.to_owned(),
    }
}

/// [`mask`] for a prelude and secret held apart from a [`ClaudeDelivery`].
pub(crate) fn mask_parts(text: &str, prelude: &str, secret: &str) -> String {
    let text = if prelude.is_empty() {
        text.to_owned()
    } else {
        text.replace(prelude, "[prelude]")
    };
    super::host_upgrade::scrub(&text, secret)
}
