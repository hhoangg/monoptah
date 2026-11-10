//! The two-phase upgrade of a cloud Host (design 1.2 H2, rule D2).
//!
//! Phase 1 is the ordinary connect bootstrap: it starts the installed Host, whatever its
//! version. Phase 2 runs here, after the tunnel is open: read what the Host is and whether
//! anything runs on it, and replace it with the pinned Host when it is older or foreign.
//!
//! * A turn is never killed. A Host with a running (or waiting) session is left alone, the
//!   machine is marked `pending_host_upgrade` and the next connect that finds it idle upgrades.
//!   Downloading takes a minute, so the upgrade is two steps: the package is staged while the
//!   Host runs untouched, then sends to the machine are held back, the Host is checked for
//!   activity AGAIN, and only then is it swapped. A turn that started in between aborts the swap.
//! * A Host that fails to start after the swap is rolled back by the script (previous runtime
//!   and launcher restored and restarted); the connect then goes on with the old Host.
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
        "Monoptah Host {} will be installed the next time this workspace connects",
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
    /// The old Host is kept (or the true state is told). Carries the user-facing warning.
    Failed(String),
    /// The start was cancelled between steps. Nothing was swapped.
    Cancelled,
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

/// Why a swap failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapError {
    /// The new Host did not start and the previous one was put back and is running.
    Restored(String),
    /// Anything else, including a failed restore: the state of the Host is not known.
    Failed(String),
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
    /// The start this runs in was cancelled.
    fn cancelled(&self) -> bool;
    /// Phase A: download and verify the pinned package next to the running Host, which is
    /// not touched. `Ok(false)`: the Host is already current, nothing was staged.
    fn stage(&self) -> Result<bool, String>;
    /// Drops a staged package.
    fn discard(&self);
    /// Holds back (`true`) or releases (`false`) every request that could start a turn on
    /// this machine, so none starts during the swap.
    fn hold_sends(&self, held: bool);
    /// Phase B: installs the staged package and restarts the Host.
    fn swap(&self) -> Result<(), SwapError>;
    /// Opens a fresh tunnel (the Host restarted under the old one) and describes the Host.
    fn reconnect(&self) -> Result<HostInfo, String>;
    /// The machine is a Docker Sandbox, whose network policy can block the download.
    fn is_sandbox(&self) -> bool;
}

/// Releases the send hold however the upgrade ends.
struct Held<'a>(&'a dyn UpgradeEnv);
impl<'a> Held<'a> {
    fn new(env: &'a dyn UpgradeEnv) -> Self {
        env.hold_sends(true);
        Self(env)
    }
}
impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.0.hold_sends(false);
    }
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
    if env.cancelled() {
        return Outcome::Cancelled;
    }
    // A Host that cannot be asked is treated as busy: a turn is never killed.
    if env.busy().unwrap_or(true) {
        return defer(env, progress);
    }
    progress(UPDATING);
    match env.stage() {
        Ok(true) => {}
        Ok(false) => {
            env.set_pending(false);
            return Outcome::Current;
        }
        // The running Host was not touched.
        Err(error) => return failed(env, progress, &error),
    }
    if env.cancelled() {
        env.discard();
        return Outcome::Cancelled;
    }
    // The download took a while and a turn may have started since. Hold sends back FIRST, then
    // look again: anything that started before the hold shows up here, anything after is refused.
    let _held = Held::new(env);
    if env.busy().unwrap_or(true) {
        env.discard();
        return defer(env, progress);
    }
    if env.cancelled() {
        env.discard();
        return Outcome::Cancelled;
    }
    // From here on nothing stops the swap: it finishes or rolls back by itself.
    match env.swap() {
        Ok(()) => {}
        Err(SwapError::Restored(error)) => {
            // The old Host runs again; the tunnel must reach it.
            return match env.reconnect() {
                Ok(_) => failed(env, progress, &error),
                Err(after) => {
                    let text = format!(
                        "Monoptah Host could not be updated and the previous Host does not answer: {}",
                        clip(&after)
                    );
                    progress(&text);
                    Outcome::Failed(text)
                }
            };
        }
        Err(SwapError::Failed(error)) => {
            let _ = env.reconnect();
            let text = format!(
                "Monoptah Host could not be updated and the Host may not be running: {}",
                clip(&error)
            );
            progress(&text);
            return Outcome::Failed(text);
        }
    }
    match env.reconnect() {
        Ok(after) if !is_older_or_foreign(&after, HOST_VERSION) => {
            env.set_pending(false);
            Outcome::Upgraded(after)
        }
        Ok(_) => failed(env, progress, "The Host still reports the old version."),
        Err(error) => failed(env, progress, &error),
    }
}

fn defer(env: &dyn UpgradeEnv, progress: &mut dyn FnMut(&str)) -> Outcome {
    env.set_pending(true);
    progress(&deferred_message());
    Outcome::Deferred
}

fn failed(env: &dyn UpgradeEnv, progress: &mut dyn FnMut(&str), error: &str) -> Outcome {
    let warning = warning(error, env.is_sandbox());
    progress(&warning);
    Outcome::Failed(warning)
}

fn clip(text: &str) -> String {
    text.trim()
        .trim_start_matches("SSH setup failed:")
        .trim()
        .chars()
        .take(300)
        .collect()
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

/// The warning shown when the upgrade failed and the current Host is kept. A download that
/// looks refused in a sandbox adds the `sbx policy allow` hint.
pub fn warning(error: &str, sandbox: bool) -> String {
    let detail = clip(error);
    let mut text =
        format!("Monoptah Host could not be updated, the current Host is kept: {detail}");
    if sandbox && looks_blocked(&detail) {
        text.push_str(&format!(
            ". The download may be blocked by the sandbox network policy; allow it with `{POLICY_HINT}`"
        ));
    }
    text
}

/// Masks `secret` and anything that looks like a credential (the same redactors the sandbox
/// start uses) in text that may be shown or stored.
pub fn scrub(text: &str, secret: &str) -> String {
    let mut out = crate::sandboxes::redact(text);
    if secret.len() >= 4 {
        out = crate::git_credentials::redact(&out, secret);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

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
        /// One answer per `busy()` call; the last one repeats.
        busy: RefCell<VecDeque<Result<bool, String>>>,
        pending: Cell<bool>,
        cancel: Cell<bool>,
        /// The cancel flag flips when the package is staged.
        cancel_after_stage: Cell<bool>,
        stage: Result<bool, String>,
        swap: Result<(), SwapError>,
        /// What the Host reports once it restarted (or came back as the old one).
        after: Result<HostInfo, String>,
        sandbox: bool,
        held: Cell<bool>,
        /// `held` at the moment of each swap.
        held_at_swap: RefCell<Vec<bool>>,
        discards: Cell<usize>,
        reconnects: Cell<usize>,
        order: RefCell<Vec<&'static str>>,
    }
    impl Fake {
        fn new(info: HostInfo) -> Self {
            Self {
                info: RefCell::new(Ok(info)),
                busy: RefCell::new(VecDeque::from([Ok(false)])),
                pending: Cell::new(false),
                cancel: Cell::new(false),
                cancel_after_stage: Cell::new(false),
                stage: Ok(true),
                swap: Ok(()),
                after: Ok(info_pinned()),
                sandbox: false,
                held: Cell::new(false),
                held_at_swap: RefCell::new(vec![]),
                discards: Cell::new(0),
                reconnects: Cell::new(0),
                order: RefCell::new(vec![]),
            }
        }
        fn busy_answers(&self, answers: Vec<Result<bool, String>>) {
            *self.busy.borrow_mut() = answers.into();
        }
        fn swaps(&self) -> usize {
            self.held_at_swap.borrow().len()
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
            self.order.borrow_mut().push("busy");
            let mut answers = self.busy.borrow_mut();
            if answers.len() > 1 {
                answers.pop_front().unwrap()
            } else {
                answers.front().cloned().unwrap()
            }
        }
        fn pending(&self) -> bool {
            self.pending.get()
        }
        fn set_pending(&self, pending: bool) {
            self.pending.set(pending);
        }
        fn cancelled(&self) -> bool {
            self.cancel.get()
        }
        fn stage(&self) -> Result<bool, String> {
            self.order.borrow_mut().push("stage");
            if self.cancel_after_stage.get() {
                self.cancel.set(true);
            }
            self.stage.clone()
        }
        fn discard(&self) {
            self.discards.set(self.discards.get() + 1);
            self.order.borrow_mut().push("discard");
        }
        fn hold_sends(&self, held: bool) {
            self.held.set(held);
            self.order
                .borrow_mut()
                .push(if held { "hold" } else { "release" });
        }
        fn swap(&self) -> Result<(), SwapError> {
            self.held_at_swap.borrow_mut().push(self.held.get());
            self.order.borrow_mut().push("swap");
            self.swap.clone()
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
    fn an_older_idle_host_is_staged_checked_again_swapped_and_the_tunnel_reopened() {
        let fake = Fake::new(info("upstream", None));
        let (outcome, lines) = run(&fake);
        assert_eq!(outcome, Outcome::Upgraded(info_pinned()));
        assert_eq!(
            *fake.order.borrow(),
            [
                "busy",
                "stage",
                "hold",
                "busy",
                "swap",
                "reconnect",
                "release"
            ]
        );
        // The swap ran with sends held, and they are released afterwards.
        assert_eq!(*fake.held_at_swap.borrow(), [true]);
        assert!(!fake.held.get());
        assert_eq!(lines, [UPDATING]);
        assert!(!fake.pending.get());
    }

    #[test]
    fn a_turn_that_starts_while_the_package_downloads_aborts_the_swap_and_sets_the_flag() {
        let fake = Fake::new(info("monoptah", Some("1.0.0")));
        // Idle before the download, running after it.
        fake.busy_answers(vec![Ok(false), Ok(true)]);
        let (outcome, lines) = run(&fake);
        assert_eq!(outcome, Outcome::Deferred);
        assert_eq!(fake.swaps(), 0, "a turn is never killed");
        assert_eq!(fake.reconnects.get(), 0);
        assert_eq!(fake.discards.get(), 1, "the staged package is dropped");
        assert!(fake.pending.get());
        assert!(!fake.held.get(), "the hold is released");
        assert_eq!(
            lines.last().map(String::as_str),
            Some("Monoptah Host 1.1.0 will be installed the next time this workspace connects")
        );
        // The next connect finds it idle.
        fake.busy_answers(vec![Ok(false)]);
        let (outcome, _) = run(&fake);
        assert_eq!(outcome, Outcome::Upgraded(info_pinned()));
        assert!(!fake.pending.get(), "the flag is cleared");
    }

    #[test]
    fn an_older_host_with_a_running_turn_is_deferred_before_anything_is_downloaded() {
        let fake = Fake::new(info("monoptah", Some("1.0.0")));
        fake.busy_answers(vec![Ok(true)]);
        let (outcome, lines) = run(&fake);
        assert_eq!(outcome, Outcome::Deferred);
        assert!(!fake.order.borrow().contains(&"stage"));
        assert_eq!(fake.swaps(), 0);
        assert!(fake.pending.get());
        assert_eq!(
            lines,
            ["Monoptah Host 1.1.0 will be installed the next time this workspace connects"]
        );
    }

    #[test]
    fn a_host_that_cannot_be_asked_if_it_is_busy_is_not_touched_at_either_check() {
        for answers in [
            vec![Err("Host is busy".to_string())],
            vec![Ok(false), Err("Host is busy".to_string())],
        ] {
            let fake = Fake::new(info("upstream", None));
            fake.busy_answers(answers);
            let (outcome, _) = run(&fake);
            assert_eq!(outcome, Outcome::Deferred);
            assert_eq!(fake.swaps(), 0);
            assert!(fake.pending.get());
        }
    }

    #[test]
    fn a_newer_fork_host_is_not_touched_and_a_stale_flag_is_cleared() {
        let fake = Fake::new(info("monoptah", Some("1.2.0")));
        fake.pending.set(true);
        let (outcome, lines) = run(&fake);
        assert_eq!(outcome, Outcome::Current);
        assert!(fake.order.borrow().is_empty());
        assert!(lines.is_empty());
        assert!(!fake.pending.get());
    }

    #[test]
    fn an_upstream_host_at_a_higher_version_is_upgraded() {
        let fake = Fake::new(info("upstream", Some("9.0.0")));
        let (outcome, _) = run(&fake);
        assert!(matches!(outcome, Outcome::Upgraded(_)));
        assert_eq!(fake.swaps(), 1);
    }

    #[test]
    fn a_host_that_became_current_meanwhile_is_not_swapped() {
        let mut fake = Fake::new(info("upstream", None));
        fake.stage = Ok(false);
        let (outcome, _) = run(&fake);
        assert_eq!(outcome, Outcome::Current);
        assert_eq!(fake.swaps(), 0);
    }

    #[test]
    fn a_failed_download_keeps_the_old_host_warns_and_is_not_fatal() {
        let mut fake = Fake::new(info("upstream", None));
        fake.stage = Err("SSH setup failed: Monoptah Host package checksum mismatch.".into());
        let (outcome, lines) = run(&fake);
        let Outcome::Failed(warning) = outcome else {
            panic!("expected a warning");
        };
        assert!(warning.starts_with("Monoptah Host could not be updated"));
        assert!(warning.contains("checksum mismatch"));
        assert!(!warning.contains("sbx policy"), "{warning}");
        assert_eq!(fake.swaps(), 0, "the running Host was never touched");
        assert_eq!(fake.reconnects.get(), 0);
        assert_eq!(lines.last(), Some(&warning));
    }

    #[test]
    fn a_blocked_sandbox_download_names_the_network_policy() {
        let mut fake = Fake::new(info("upstream", None));
        fake.sandbox = true;
        fake.stage = Err("SSH setup failed: curl: (6) Could not resolve host: github.com".into());
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
    fn a_new_host_that_fails_to_start_is_rolled_back_and_the_old_one_is_reached_again() {
        let mut fake = Fake::new(info("monoptah", Some("1.0.0")));
        fake.swap = Err(SwapError::Restored(
            "SSH setup failed: Monoptah Host update failed; the previous Host was restored.".into(),
        ));
        fake.after = Ok(info("monoptah", Some("1.0.0")));
        let (outcome, lines) = run(&fake);
        let Outcome::Failed(warning) = outcome else {
            panic!("expected a warning");
        };
        // The old Host really is kept, so saying so is true.
        assert!(warning.contains("the current Host is kept"), "{warning}");
        assert_eq!(
            fake.reconnects.get(),
            1,
            "the tunnel is reopened to the old Host"
        );
        assert_eq!(lines.last(), Some(&warning));
        assert!(!fake.held.get());
    }

    #[test]
    fn when_the_rollback_itself_failed_the_true_state_is_told() {
        let mut fake = Fake::new(info("monoptah", Some("1.0.0")));
        fake.swap = Err(SwapError::Failed(
            "Monoptah Host update failed and the previous Host could not be restored.".into(),
        ));
        fake.after = Err("Machine is unreachable".into());
        let (outcome, _) = run(&fake);
        let Outcome::Failed(warning) = outcome else {
            panic!("expected a warning");
        };
        assert!(!warning.contains("is kept"), "{warning}");
        assert!(warning.contains("may not be running"), "{warning}");
        // A restored Host that does not answer is not called kept either.
        let mut fake = Fake::new(info("monoptah", Some("1.0.0")));
        fake.swap = Err(SwapError::Restored("restored".into()));
        fake.after = Err("Machine is unreachable".into());
        let Outcome::Failed(warning) = run(&fake).0 else {
            panic!("expected a warning");
        };
        assert!(warning.contains("does not answer"), "{warning}");
    }

    #[test]
    fn a_host_that_does_not_come_back_or_stays_old_after_a_good_swap_is_a_warning_too() {
        let mut fake = Fake::new(info("upstream", None));
        fake.after = Err("Machine is unreachable".into());
        assert!(matches!(run(&fake).0, Outcome::Failed(_)));
        let mut fake = Fake::new(info("upstream", None));
        fake.after = Ok(info("upstream", None));
        assert!(matches!(run(&fake).0, Outcome::Failed(_)));
    }

    #[test]
    fn a_cancelled_start_stops_between_steps_and_never_mid_swap() {
        // Before anything.
        let fake = Fake::new(info("upstream", None));
        fake.cancel.set(true);
        assert_eq!(run(&fake).0, Outcome::Cancelled);
        assert!(!fake.order.borrow().contains(&"stage"));
        // After the download: the staged package is dropped, nothing is swapped.
        let fake = Fake::new(info("upstream", None));
        fake.cancel_after_stage.set(true);
        assert_eq!(run(&fake).0, Outcome::Cancelled);
        assert_eq!(fake.swaps(), 0);
        assert_eq!(fake.discards.get(), 1);
        assert!(!fake.held.get());
    }

    #[test]
    fn an_unreadable_host_is_left_alone() {
        let fake = Fake::new(info("upstream", None));
        *fake.info.borrow_mut() = Err("gone".into());
        assert_eq!(run(&fake).0, Outcome::Current);
        assert!(fake.order.borrow().is_empty());
    }

    #[test]
    fn scrub_masks_the_secret_and_anything_that_looks_like_a_credential() {
        assert_eq!(
            scrub("a mysecret-1 b mysecret-1", "mysecret-1"),
            "a [redacted] b [redacted]"
        );
        assert_eq!(scrub("token sk-ant-oat01-abcdef", ""), "token [redacted]");
        assert_eq!(scrub("plain", ""), "plain");
    }
}
