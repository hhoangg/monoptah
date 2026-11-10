//! The two-phase upgrade of a cloud Host (design 1.2 H2, rule D2).
//!
//! Phase 1 is the ordinary connect bootstrap: it starts the installed Host, whatever its
//! version. Phase 2 runs here, after the tunnel is open: read what the Host is and whether
//! anything runs on it, and replace it with the pinned Host when it is older or foreign.
//!
//! * A turn is never killed. A Host with a running (or waiting) session is left alone, the
//!   machine is marked `pending_host_upgrade` and the next connect that finds it idle upgrades.
//! * A failure (download blocked, checksum mismatch, a Host that does not come back) keeps
//!   whatever Host is there, says so as a warning, and is never fatal to the connect.
//! * Versions only decide this upgrade (rule D3). Features gate on capability strings.
//! * It only runs inside a user-initiated connect or start flow, never from a background path:
//!   it needs the tunnel that flow just opened, and it may run ssh.
//!
//! The decision lives in [`upgrade_if_needed`], over the [`UpgradeEnv`] seam, so it is tested
//! with a fake Host and a fake ssh runner.
use super::host_info::HostInfo;
use super::host_release::HOST_VERSION;

/// Shown while the upgrade script runs.
pub const UPDATING: &str = "Updating Monoptah Host…";

/// The bare `X.Y.Z` of the pinned Host.
pub fn pinned_version() -> &'static str {
    HOST_VERSION.split('+').next().unwrap_or(HOST_VERSION)
}

/// Shown while an upgrade waits for the running turn to end.
pub fn deferred_message() -> String {
    format!(
        "Monoptah Host {} will be installed when the current turn ends",
        pinned_version()
    )
}

/// Credential text a sandbox upgrade needs: the SAME prelude the connect step uses (the
/// restarted Host must have the Claude credential in its environment) and the secret inside
/// it, so nothing that comes back can leak it.
#[derive(Clone, Copy)]
pub struct SandboxAuth<'a> {
    pub prelude: &'a str,
    pub secret: &'a str,
}

/// What [`upgrade_if_needed`] did. `Failed` and `Deferred` are warnings, not errors: the
/// connect goes on with the Host that is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The Host is the pinned one or newer (a fork Host never goes down), or it could not be read.
    Current,
    /// Replaced; carries what the restarted Host reports.
    Upgraded(HostInfo),
    /// A session is running: `pending_host_upgrade` is set and the next idle connect upgrades.
    Deferred,
    /// The old Host is kept. Carries the user-facing warning.
    Failed(String),
}

/// Whether an installed Host must be replaced by the pin: it is not the fork's flavor
/// (whatever its number), has no version, or its `X.Y.Z` is lower than the pin's. A
/// pre-release is lower than its release. The twin of `is_older_or_foreign` in
/// `remote_bootstrap.sh`, which re-checks it on the machine.
pub fn is_older_or_foreign(info: &HostInfo, pin: &str) -> bool {
    if !info.is_monoptah() {
        return true;
    }
    let Some(installed) = info.version.as_deref() else {
        return true;
    };
    let (Some((a, a_pre)), Some((b, b_pre))) = (parse(installed), parse(pin)) else {
        return true;
    };
    if a != b {
        return a < b;
    }
    // Same X.Y.Z: a pre-release is older than a release, and two different pre-releases are
    // replaced (the pin is a release in practice).
    a_pre.is_some() && (b_pre.is_none() || a_pre != b_pre)
}

fn parse(version: &str) -> Option<([u64; 3], Option<String>)> {
    let core = version.split('+').next()?;
    let (numbers, pre) = match core.split_once('-') {
        Some((numbers, pre)) => (numbers, Some(pre.to_owned())),
        None => (core, None),
    };
    let mut parts = numbers.split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *slot = part.parse().ok()?;
    }
    parts.next().is_none().then_some((out, pre))
}

/// Everything [`upgrade_if_needed`] touches outside itself.
pub trait UpgradeEnv {
    /// What the Host on the other end of the open tunnel reports now.
    fn host_info(&self) -> Result<HostInfo, String>;
    /// Whether any session on the Host runs or waits for input (`sessions.list` of every
    /// project). An error means "cannot tell", which is treated as busy.
    fn busy(&self) -> Result<bool, String>;
    fn pending(&self) -> bool;
    fn set_pending(&self, pending: bool);
    /// Runs the upgrade script on the machine (a codespace through `gh codespace ssh`, a
    /// sandbox through the container exec with the connect step's prelude).
    fn install(&self) -> Result<(), String>;
    /// Opens a fresh tunnel (the Host restarted under the old one) and describes the new Host.
    fn reconnect(&self) -> Result<HostInfo, String>;
    /// The machine is a Docker Sandbox, whose network policy can block the download.
    fn is_sandbox(&self) -> bool;
}

/// Phase 2. `progress` hears the status lines ("Updating Monoptah Host…", the deferred text,
/// the warning). Never fails.
pub fn upgrade_if_needed(env: &dyn UpgradeEnv, progress: &mut dyn FnMut(&str)) -> Outcome {
    let Ok(info) = env.host_info() else {
        // Cannot tell what the Host is: leave it alone. The connect already worked.
        return Outcome::Current;
    };
    if !is_older_or_foreign(&info, HOST_VERSION) {
        if env.pending() {
            env.set_pending(false);
        }
        return Outcome::Current;
    }
    // A Host that cannot be asked is treated as busy: a turn is never killed.
    if env.busy().unwrap_or(true) {
        env.set_pending(true);
        progress(&deferred_message());
        return Outcome::Deferred;
    }
    progress(UPDATING);
    if let Err(error) = env.install() {
        return failed(env, progress, &error);
    }
    // The Host restarted under the old tunnel (reusing the tunnel-invalidation path).
    match env.reconnect() {
        Ok(after) if !is_older_or_foreign(&after, HOST_VERSION) => {
            env.set_pending(false);
            Outcome::Upgraded(after)
        }
        Ok(_) => failed(env, progress, "The Host still reports the old version."),
        Err(error) => failed(env, progress, &error),
    }
}

fn failed(env: &dyn UpgradeEnv, progress: &mut dyn FnMut(&str), error: &str) -> Outcome {
    let warning = warning(error, env.is_sandbox());
    progress(&warning);
    Outcome::Failed(warning)
}

/// The sandbox network policy hint, from the hosts the bootstrap downloads from.
pub const POLICY_HINT: &str = "sbx policy allow network github.com,**.githubusercontent.com";

/// Words in a download failure that mean the sandbox's network policy refused the host.
fn looks_blocked(error: &str) -> bool {
    let lower = error.to_lowercase();
    [
        "could not resolve",
        "couldn't resolve",
        "connection refused",
        "connection reset",
        "403",
        "forbidden",
        "proxy",
        "blocked",
        "unavailable",
        "timed out",
        "curl:",
        "wget:",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

/// The warning shown when the upgrade failed, e.g. `Monoptah Host could not be updated
/// (the old Host is kept): ...`. A download that looks refused in a sandbox adds the
/// `sbx policy allow` hint.
pub fn warning(error: &str, sandbox: bool) -> String {
    let detail: String = error
        .trim()
        .trim_start_matches("SSH setup failed:")
        .trim()
        .chars()
        .take(300)
        .collect();
    let mut text =
        format!("Monoptah Host could not be updated, the current Host is kept: {detail}");
    if sandbox && looks_blocked(&detail) {
        text.push_str(&format!(
            ". The download may be blocked by the sandbox network policy; allow it with `{POLICY_HINT}`"
        ));
    }
    text
}

/// Removes every occurrence of `secret` from `text`.
pub fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_owned()
    } else {
        text.replace(secret, "[hidden]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn info(flavor: &str, version: Option<&str>) -> HostInfo {
        HostInfo {
            flavor: flavor.into(),
            version: version.map(Into::into),
            capabilities: Default::default(),
        }
    }

    #[test]
    fn older_foreign_and_unparsable_hosts_upgrade_current_and_newer_forks_do_not() {
        let pin = "1.1.0+monoptah";
        assert!(is_older_or_foreign(&info("upstream", None), pin));
        // Foreign always upgrades, even at a higher number.
        assert!(is_older_or_foreign(&info("upstream", Some("9.9.9")), pin));
        assert!(is_older_or_foreign(&info("monoptah", None), pin));
        assert!(is_older_or_foreign(&info("monoptah", Some("1.0.9")), pin));
        assert!(is_older_or_foreign(&info("monoptah", Some("0.9.0")), pin));
        assert!(is_older_or_foreign(&info("monoptah", Some("garbage")), pin));
        assert!(is_older_or_foreign(
            &info("monoptah", Some("1.1.0-rc.1")),
            pin
        ));
        assert!(!is_older_or_foreign(&info("monoptah", Some("1.1.0")), pin));
        assert!(!is_older_or_foreign(
            &info("monoptah", Some("1.1.0+monoptah")),
            pin
        ));
        assert!(!is_older_or_foreign(&info("monoptah", Some("1.1.1")), pin));
        assert!(!is_older_or_foreign(&info("monoptah", Some("2.0.0")), pin));
        assert!(!is_older_or_foreign(&info("monoptah", Some("1.10.0")), pin));
    }

    /// A fake Host plus a fake ssh runner.
    struct Fake {
        info: RefCell<Result<HostInfo, String>>,
        busy: Result<bool, String>,
        pending: Cell<bool>,
        /// What the runner answers; `Err` is a failed download or checksum mismatch.
        install: Result<(), String>,
        /// What the Host reports once it restarted.
        after: Result<HostInfo, String>,
        sandbox: bool,
        installs: Cell<usize>,
        reconnects: Cell<usize>,
        order: RefCell<Vec<&'static str>>,
    }
    impl Fake {
        fn new(info: HostInfo) -> Self {
            Self {
                info: RefCell::new(Ok(info)),
                busy: Ok(false),
                pending: Cell::new(false),
                install: Ok(()),
                after: Ok(info_pinned()),
                sandbox: false,
                installs: Cell::new(0),
                reconnects: Cell::new(0),
                order: RefCell::new(vec![]),
            }
        }
    }
    fn info_pinned() -> HostInfo {
        info("monoptah", Some(pinned_version()))
    }
    impl UpgradeEnv for Fake {
        fn host_info(&self) -> Result<HostInfo, String> {
            self.info.borrow().clone()
        }
        fn busy(&self) -> Result<bool, String> {
            self.busy.clone()
        }
        fn pending(&self) -> bool {
            self.pending.get()
        }
        fn set_pending(&self, pending: bool) {
            self.pending.set(pending);
        }
        fn install(&self) -> Result<(), String> {
            self.installs.set(self.installs.get() + 1);
            self.order.borrow_mut().push("install");
            self.install.clone()
        }
        fn reconnect(&self) -> Result<HostInfo, String> {
            self.reconnects.set(self.reconnects.get() + 1);
            self.order.borrow_mut().push("reconnect");
            self.after.clone()
        }
        fn is_sandbox(&self) -> bool {
            self.sandbox
        }
    }
    fn run(fake: &Fake) -> (Outcome, Vec<String>) {
        let mut lines = vec![];
        let outcome = upgrade_if_needed(fake, &mut |line| lines.push(line.to_owned()));
        (outcome, lines)
    }

    #[test]
    fn an_older_idle_host_is_upgraded_and_then_the_tunnel_is_reopened() {
        let fake = Fake::new(info("upstream", None));
        let (outcome, lines) = run(&fake);
        assert_eq!(outcome, Outcome::Upgraded(info_pinned()));
        assert_eq!(*fake.order.borrow(), ["install", "reconnect"]);
        assert_eq!(lines, [UPDATING]);
        assert!(!fake.pending.get());
    }

    #[test]
    fn an_older_host_with_a_running_turn_is_deferred_and_the_next_idle_connect_upgrades() {
        let mut fake = Fake::new(info("monoptah", Some("1.0.0")));
        fake.busy = Ok(true);
        let (outcome, lines) = run(&fake);
        assert_eq!(outcome, Outcome::Deferred);
        assert_eq!(fake.installs.get(), 0, "a turn is never killed");
        assert_eq!(fake.reconnects.get(), 0);
        assert!(fake.pending.get());
        assert_eq!(
            lines,
            ["Monoptah Host 1.1.0 will be installed when the current turn ends"]
        );
        // The next connect finds it idle.
        fake.busy = Ok(false);
        let (outcome, _) = run(&fake);
        assert_eq!(outcome, Outcome::Upgraded(info_pinned()));
        assert_eq!(fake.installs.get(), 1);
        assert!(!fake.pending.get(), "the flag is cleared");
    }

    #[test]
    fn a_host_that_cannot_be_asked_if_it_is_busy_is_not_touched() {
        let mut fake = Fake::new(info("upstream", None));
        fake.busy = Err("Host is busy".into());
        let (outcome, _) = run(&fake);
        assert_eq!(outcome, Outcome::Deferred);
        assert_eq!(fake.installs.get(), 0);
        assert!(fake.pending.get());
    }

    #[test]
    fn a_newer_fork_host_is_not_touched_and_a_stale_flag_is_cleared() {
        let fake = Fake::new(info("monoptah", Some("1.2.0")));
        fake.pending.set(true);
        let (outcome, lines) = run(&fake);
        assert_eq!(outcome, Outcome::Current);
        assert_eq!(fake.installs.get(), 0);
        assert!(lines.is_empty());
        assert!(!fake.pending.get());
    }

    #[test]
    fn an_upstream_host_at_a_higher_version_is_upgraded() {
        let fake = Fake::new(info("upstream", Some("9.0.0")));
        let (outcome, _) = run(&fake);
        assert!(matches!(outcome, Outcome::Upgraded(_)));
        assert_eq!(fake.installs.get(), 1);
    }

    #[test]
    fn a_failed_download_keeps_the_old_host_warns_and_is_not_fatal() {
        let mut fake = Fake::new(info("upstream", None));
        fake.install = Err("SSH setup failed: Monoptah Host package checksum mismatch.".into());
        let (outcome, lines) = run(&fake);
        let Outcome::Failed(warning) = outcome else {
            panic!("expected a warning");
        };
        assert!(warning.starts_with("Monoptah Host could not be updated"));
        assert!(warning.contains("checksum mismatch"));
        assert!(!warning.contains("sbx policy"), "{warning}");
        assert_eq!(fake.reconnects.get(), 0, "the old Host and tunnel stay");
        assert_eq!(lines.last(), Some(&warning));
    }

    #[test]
    fn a_blocked_sandbox_download_names_the_network_policy() {
        let mut fake = Fake::new(info("upstream", None));
        fake.sandbox = true;
        fake.install = Err("SSH setup failed: curl: (6) Could not resolve host: github.com".into());
        let (outcome, _) = run(&fake);
        let Outcome::Failed(warning) = outcome else {
            panic!("expected a warning");
        };
        assert!(warning.contains("sandbox network policy"), "{warning}");
        assert!(
            warning.contains("sbx policy allow network github.com"),
            "{warning}"
        );
    }

    #[test]
    fn a_host_that_does_not_come_back_or_stays_old_is_a_warning_too() {
        let mut fake = Fake::new(info("upstream", None));
        fake.after = Err("Machine is unreachable".into());
        assert!(matches!(run(&fake).0, Outcome::Failed(_)));
        let mut fake = Fake::new(info("upstream", None));
        fake.after = Ok(info("upstream", None));
        assert!(matches!(run(&fake).0, Outcome::Failed(_)));
    }

    #[test]
    fn an_unreadable_host_is_left_alone() {
        let fake = Fake::new(info("upstream", None));
        *fake.info.borrow_mut() = Err("gone".into());
        assert_eq!(run(&fake).0, Outcome::Current);
        assert_eq!(fake.installs.get(), 0);
    }

    #[test]
    fn scrub_removes_the_secret_wherever_it_appears() {
        assert_eq!(scrub("a sk-1 b sk-1", "sk-1"), "a [hidden] b [hidden]");
        assert_eq!(scrub("a", ""), "a");
    }
}
