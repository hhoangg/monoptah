//! This month's Codespaces usage, read from GitHub's billing summary through `gh`.
//!
//! `gh api /users/<login>/settings/billing/usage/summary` needs a token with the `user` scope.
//! The response lists one item per product and sku (`Codespaces` / `codespaces_compute_d4`,
//! `codespaces_storage`, `actions_linux`, ...) with gross, discount and net quantities and
//! amounts. The included allowance is NOT in it, so [`CodespacesUsage::estimated_hours_left`]
//! is an estimate for the two personal plans whose Codespaces allowance GitHub documents (see
//! [`FREE_CORE_HOURS`] and [`PRO_CORE_HOURS`]). Codespaces has its own allowance, in core-hours
//! and GB-months, separate from the Actions minutes.
//!
//! Every `gh` call goes through the injectable [`GhRunner`]. Nothing here is secret. Callers
//! reach it through a [`UsageCache`] so the panel never asks GitHub more than once per
//! [`CACHE_TTL`] unless the user presses refresh.
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{argv, valid_segment, GhOutput, GhRunner};

/// How long a successful answer is reused for.
pub const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a failure is remembered: re-opening the panel right after "no scope" does not
/// spawn `gh` again, while an explicit refresh always does.
const FAILURE_TTL: Duration = Duration::from_secs(60);

/// GitHub's authoritative page; the numbers here are only a convenience copy.
pub const BILLING_SUMMARY_URL: &str = "https://github.com/settings/billing/summary";
/// What fixes [`UsageProblem::MissingScope`].
pub const SCOPE_FIX_COMMAND: &str = "gh auth refresh -h github.com -s user";

/// ESTIMATE ONLY. The Codespaces compute included each month with a personal account, in
/// core-hours (a 4-core machine used for 1 hour is 4 core-hours), as documented by GitHub:
/// 120 on Free, 180 on Pro. Actions minutes are a separate allowance and do not draw on it.
/// The billing API does not return these figures and they have not been checked against it;
/// GitHub changes them, and other plans (Team, Enterprise, ...) get no estimate.
pub const FREE_CORE_HOURS: f64 = 120.0;
pub const PRO_CORE_HOURS: f64 = 180.0;
/// The included Codespaces storage, in GB-months (15 on Free, 20 on Pro). Same caveat.
pub const FREE_STORAGE_GB_MONTHS: f64 = 15.0;
pub const PRO_STORAGE_GB_MONTHS: f64 = 20.0;
/// Codespaces compute price per core-hour (a 4-core machine is `pricePerUnit` 0.36 an hour).
/// Turns the compute dollars into core-hours, the unit of the allowance.
pub const USD_PER_CORE_HOUR: f64 = 0.09;

/// This month's Codespaces usage for the signed-in user.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodespacesUsage {
    pub year: Option<i32>,
    pub month: Option<u32>,
    /// Hours of the machines, `codespaces_compute_*` `grossQuantity` (a 4-core machine for an
    /// hour is 1 here). Not the unit of the allowance: see `compute_core_hours`.
    pub compute_machine_hours: f64,
    /// Core-hours of `codespaces_compute_*`: the gross dollars over [`USD_PER_CORE_HOUR`]. This
    /// is what GitHub's billing page counts against the allowance.
    pub compute_core_hours: f64,
    /// Gigabyte-months of Codespaces storage.
    pub storage_gb_months: f64,
    /// All Codespaces items, in dollars: before discount, the discount, and what is billed.
    pub gross: f64,
    pub discount: f64,
    pub net: f64,
    /// The user's plan from `gh api user` (`pro`, `free`, ...), lower case.
    pub plan: Option<String>,
}

impl CodespacesUsage {
    /// Anything is billed to the account this month.
    pub fn is_billed(&self) -> bool {
        self.net > 0.004
    }

    /// The included compute in core-hours, only for the plans whose allowance is known.
    pub fn included_core_hours(&self) -> Option<f64> {
        match self.plan.as_deref() {
            Some("pro") => Some(PRO_CORE_HOURS),
            Some("free") => Some(FREE_CORE_HOURS),
            _ => None,
        }
    }

    /// The included storage in GB-months, for the same plans.
    pub fn included_storage_gb_months(&self) -> Option<f64> {
        match self.plan.as_deref() {
            Some("pro") => Some(PRO_STORAGE_GB_MONTHS),
            Some("free") => Some(FREE_STORAGE_GB_MONTHS),
            _ => None,
        }
    }

    /// Core-hours of the included compute not used yet.
    pub fn core_hours_left(&self) -> Option<f64> {
        Some((self.included_core_hours()? - self.compute_core_hours).max(0.))
    }

    /// ESTIMATE: hours of a machine with `cores` cores that the included core-hours still
    /// cover. `None` for plans whose allowance is unknown or when the machine's size is unknown.
    pub fn estimated_hours_left(&self, cores: u32) -> Option<f64> {
        if cores == 0 {
            return None;
        }
        Some(self.core_hours_left()? / f64::from(cores))
    }
}

/// Why the usage could not be read; every case is a short hint, never a blocker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageProblem {
    GhMissing,
    NotLoggedIn,
    /// gh says the token lacks a scope (`user`): the one case `SCOPE_FIX_COMMAND` fixes.
    MissingScope,
    /// A bare 403/404: the token may be fine, the account or token just cannot use this
    /// endpoint (not on GitHub's enhanced billing, a fine-grained token, an organisation
    /// policy). Refreshing the scope does not help, so no command is offered.
    NotAvailable,
    /// GitHub's API rate limit.
    RateLimited,
    Offline,
    Failed(String),
}

impl UsageProblem {
    /// One short muted line for the panel.
    pub fn hint(&self) -> String {
        match self {
            Self::GhMissing => "Usage unavailable: GitHub CLI (gh) is not installed".into(),
            Self::NotLoggedIn => "Usage unavailable: run `gh auth login`".into(),
            Self::MissingScope => {
                format!("Usage needs the `user` scope: run `{SCOPE_FIX_COMMAND}`")
            }
            Self::NotAvailable => "Usage not available for this account".into(),
            Self::RateLimited => "Usage unavailable: GitHub rate limit reached, try later".into(),
            Self::Offline => "Usage unavailable: can't reach GitHub".into(),
            Self::Failed(why) if why.is_empty() => "Usage unavailable".into(),
            Self::Failed(why) => format!("Usage unavailable: {why}"),
        }
    }
}

fn classify(out: &GhOutput) -> UsageProblem {
    let text = format!("{}\n{}", out.stderr, out.stdout);
    let lower = text.to_lowercase();
    if lower.contains("gh auth login")
        || lower.contains("gh_token")
        || lower.contains("bad credentials")
        || lower.contains("http 401")
    {
        return UsageProblem::NotLoggedIn;
    }
    // `gh` says `This API operation needs the "user" scope` when (and only when) the token
    // lacks it. A bare 403/404 is not that: the same answer comes from an account that cannot
    // use the endpoint, and telling those users to refresh the scope never ends.
    if lower.contains("rate limit") {
        return UsageProblem::RateLimited;
    }
    if lower.contains("scope") {
        return UsageProblem::MissingScope;
    }
    if lower.contains("http 403")
        || lower.contains("http 404")
        || lower.contains("not found")
        || lower.contains("forbidden")
        || lower.contains("not accessible")
    {
        return UsageProblem::NotAvailable;
    }
    if [
        "error connecting",
        "dial tcp",
        "no such host",
        "could not resolve",
        "timed out",
        "timeout",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return UsageProblem::Offline;
    }
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    UsageProblem::Failed(first.chars().take(80).collect())
}

fn api(runner: &dyn GhRunner, path: &str) -> Result<String, UsageProblem> {
    let args = argv(&["api", path]);
    match runner.run(&args, None) {
        Ok(out) if out.success => Ok(out.stdout),
        Ok(out) => Err(classify(&out)),
        Err(error) if error.contains("not installed") => Err(UsageProblem::GhMissing),
        Err(error) => Err(UsageProblem::Failed(error.chars().take(80).collect())),
    }
}

/// A JSON number, or a number written as text; anything else is absent.
fn number(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|n| n.is_finite())
}

/// Parse the billing summary. Unknown products, skus and units are ignored; a missing field
/// counts as zero. `plan` is carried through as given.
pub fn parse_summary(text: &str, plan: Option<String>) -> Result<CodespacesUsage, UsageProblem> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| UsageProblem::Failed("unexpected billing response".into()))?;
    let Some(root) = value.as_object() else {
        return Err(UsageProblem::Failed("unexpected billing response".into()));
    };
    let mut usage = CodespacesUsage {
        year: number(root.get("timePeriod").and_then(|p| p.get("year"))).map(|n| n as i32),
        month: number(root.get("timePeriod").and_then(|p| p.get("month"))).map(|n| n as u32),
        plan: plan.map(|p| p.to_lowercase()),
        ..CodespacesUsage::default()
    };
    let items = root
        .get("usageItems")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    for item in items {
        let text = |key: &str| {
            item.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
        };
        let (product, sku, unit) = (text("product"), text("sku"), text("unitType"));
        let amount = |key: &str| number(item.get(key)).unwrap_or(0.);
        let codespaces = product == "codespaces" || sku.starts_with("codespaces_");
        let compute = sku.starts_with("codespaces_compute");
        if codespaces {
            usage.gross += amount("grossAmount");
            usage.discount += amount("discountAmount");
            usage.net += amount("netAmount");
            if compute {
                usage.compute_machine_hours += amount("grossQuantity");
                usage.compute_core_hours += amount("grossAmount") / USD_PER_CORE_HOUR;
            } else if sku.contains("storage") && (unit.is_empty() || unit.contains("gigabyte")) {
                usage.storage_gb_months += amount("grossQuantity");
            }
        }
    }
    Ok(usage)
}

/// Read this month's usage of the user `gh` is signed in as: two quick `gh api` calls.
pub fn fetch(runner: &dyn GhRunner) -> Result<CodespacesUsage, UsageProblem> {
    let user: serde_json::Value = serde_json::from_str(&api(runner, "user")?)
        .map_err(|_| UsageProblem::Failed("unexpected user response".into()))?;
    let login = user.get("login").and_then(|v| v.as_str()).unwrap_or("");
    if !valid_segment(login) {
        return Err(UsageProblem::NotLoggedIn);
    }
    let plan = user
        .get("plan")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_owned);
    let text = api(
        runner,
        &format!("/users/{login}/settings/billing/usage/summary"),
    )?;
    parse_summary(&text, plan)
}

struct Cached {
    at: Instant,
    result: Result<CodespacesUsage, UsageProblem>,
}

/// One answer kept for the app session. Concurrent callers wait for the one fetch in flight.
#[derive(Default)]
pub struct UsageCache {
    slot: Mutex<Option<Cached>>,
}

impl UsageCache {
    /// The cached answer while fresh, else a new fetch. `force` (an explicit refresh) always
    /// fetches. Blocking: call it on a worker.
    pub fn get(&self, runner: &dyn GhRunner, force: bool) -> Result<CodespacesUsage, UsageProblem> {
        self.get_at(runner, force, Instant::now())
    }

    fn get_at(
        &self,
        runner: &dyn GhRunner,
        force: bool,
        now: Instant,
    ) -> Result<CodespacesUsage, UsageProblem> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = slot.as_ref().filter(|_| !force) {
            let ttl = if cached.result.is_ok() {
                CACHE_TTL
            } else {
                FAILURE_TTL
            };
            if now.saturating_duration_since(cached.at) < ttl {
                return cached.result.clone();
            }
        }
        let result = fetch(runner);
        *slot = Some(Cached {
            at: now,
            result: result.clone(),
        });
        result
    }
}

#[cfg(test)]
mod tests;
