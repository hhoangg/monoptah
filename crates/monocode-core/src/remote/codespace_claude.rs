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
