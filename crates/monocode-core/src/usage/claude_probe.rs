//! Manual quota check for `claude setup-token` accounts.
//!
//! Anthropic's usage endpoint needs the `user:profile` scope, which a setup
//! token lacks. The Messages API does accept it and reports the subscription
//! windows in `anthropic-ratelimit-unified-*` headers, so a manual refresh
//! sends one tiny message and reads them. That spends a little quota, so it
//! runs only when the user asks; every other refresh returns the last reading.
use super::{now_ms, FetchError, Snapshot, Window};
use crate::{accounts::Profile, provider::Kind};
use std::{path::Path, time::Duration};

const URL: &str = "https://api.anthropic.com/v1/messages";
/// The cheapest model the setup token is known to accept.
pub const MODEL: &str = "claude-haiku-4-5";
/// Last reading, kept beside the account's Claude config. Never holds a secret.
const CACHE: &str = ".monoptah-usage-probe.json";
pub(super) const IDLE: &str =
    "Usage is unavailable automatically for setup tokens. Refresh to check it with one tiny message.";

#[cfg(test)]
thread_local! {
    pub(super) static TEST_URL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}
fn url() -> String {
    #[cfg(test)]
    if let Some(url) = TEST_URL.with(|u| u.borrow().clone()) {
        return url;
    }
    URL.into()
}

pub(super) fn fetch(profile: &Profile, manual: bool) -> Result<Snapshot, FetchError> {
    let cache = profile
        .directory(Kind::Claude)
        .map_err(FetchError::failed)?
        .map(|dir| dir.join(CACHE));
    if !manual {
        return cache
            .as_deref()
            .zip(profile.claude_setup_stamp())
            .and_then(|(path, stamp)| load(path, &stamp))
            .ok_or_else(|| FetchError::failed(IDLE));
    }
    let (token, stamp) = profile
        .claude_setup_token_stamped()
        .map_err(FetchError::failed)?
        .ok_or_else(|| FetchError::failed(IDLE))?;
    let snapshot = probe(&url(), &token)?;
    if let Some(path) = cache {
        save(&path, &stamp, &snapshot);
    }
    Ok(snapshot)
}

fn probe(url: &str, token: &str) -> Result<Snapshot, FetchError> {
    let body = serde_json::json!({
        "model": MODEL,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "."}],
    });
    let result = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .build()
        .post(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .set("User-Agent", "claude-code/2.1.0")
        .send_string(&body.to_string());
    match result {
        Ok(response) => parse_headers(|name| response.header(name).map(str::to_owned)),
        // A 429 at the subscription limit still reports the windows.
        Err(ureq::Error::Status(status, response)) => {
            if status == 429 {
                if let Ok(snapshot) = parse_headers(|name| response.header(name).map(str::to_owned))
                {
                    return Ok(snapshot);
                }
            }
            Err(status_error(status))
        }
        Err(_) => Err(FetchError::failed(
            "Could not reach Claude to check usage. Try again.",
        )),
    }
}

fn status_error(status: u16) -> FetchError {
    FetchError::failed(match status {
        401 => {
            "Claude rejected the setup token. Run claude setup-token and save the new one.".into()
        }
        403 => "This setup token is not allowed to check usage.".into(),
        429 => "Claude is rate limiting this account. Try again in a few minutes.".into(),
        status => format!("Claude usage check failed ({status})."),
    })
}

fn parse_headers(header: impl Fn(&str) -> Option<String>) -> Result<Snapshot, FetchError> {
    let windows: Vec<Window> = [("5h", 300), ("7d", 10_080)]
        .into_iter()
        .filter_map(|(name, minutes)| {
            let used = header(&format!("anthropic-ratelimit-unified-{name}-utilization"))?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())?;
            let resets_at = header(&format!("anthropic-ratelimit-unified-{name}-reset"))
                .and_then(|s| s.trim().parse::<i64>().ok())
                .map(|seconds| seconds.saturating_mul(1000));
            Some(Window {
                used_percent: (used * 100.).clamp(0., 100.),
                minutes,
                resets_at,
            })
        })
        .collect();
    if windows.is_empty() {
        return Err(FetchError::failed(
            "Claude did not report usage for this account.",
        ));
    }
    Ok(Snapshot {
        windows,
        updated_at: now_ms(),
        manual: true,
        ..Default::default()
    })
}

/// The cached reading with the credential it was taken with. A probe that
/// outlives a token change can write after `forget`; the stamp lets `load`
/// reject it. Holds no secret: the stamp is the credential row's nonce.
#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    stamp: Vec<u8>,
    snapshot: Snapshot,
}

fn load(path: &Path, stamp: &[u8]) -> Option<Snapshot> {
    let cached: Cached = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    (cached.stamp == stamp && !cached.snapshot.windows.is_empty()).then_some(Snapshot {
        manual: true,
        ..cached.snapshot
    })
}
/// Drop the last reading when the account's credential changes, so a new
/// token or an API key never shows the old token's quota.
pub(crate) fn forget(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(CACHE));
}
fn save(path: &Path, stamp: &[u8], snapshot: &Snapshot) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cached = Cached {
        stamp: stamp.to_vec(),
        snapshot: snapshot.clone(),
    };
    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests;
