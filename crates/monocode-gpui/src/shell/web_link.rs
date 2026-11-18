//! The web link of the shell: the bridge between the core web link and the
//! workspace. It runs the Settings > Web app actions on the background
//! executor, polls a pairing until the browser answers, and watches for
//! devices that wait for confirmation while Settings is closed.
use super::*;
use crate::notifications::platform;
use crate::settings::{SettingsEvent, WebPolicyFlag};
use gpui::{Global, Task};
use monocode_core::mobile::{
    WebStatus,
    host::MobileHost,
    policy::{self, PolicyFlags},
    web_link::{PairPoll, PairTicket, PendingDevice, attached_host},
};
use std::{
    collections::HashSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// How often the watcher reads the web status.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// What the bridge remembers between frames. A GPUI global, so the Shell
/// struct needs no field for it.
#[derive(Default)]
struct WebBridge {
    /// The code and link on show while the browser has not answered.
    pairing: Option<PairTicket>,
    notice: Option<String>,
    poll: Option<Task<()>>,
    watcher: Option<Task<()>>,
    /// Fingerprints already announced, so each one raises one notification.
    seen: HashSet<String>,
    /// Devices and requests waiting for the user, for the Connections badge.
    badge: usize,
    /// Bumped whenever a pairing is cancelled or Settings closes, so a ticket
    /// that arrives late is dropped instead of reopening the dialog.
    pairing_generation: u64,
}

impl Global for WebBridge {}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The name the web server shows for this desktop.
fn machine_name() -> String {
    ["HOSTNAME", "COMPUTERNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "Desktop".into())
}

/// The pending devices whose fingerprint was not announced yet.
fn fresh_pending<'a>(seen: &HashSet<String>, status: &'a WebStatus) -> Vec<&'a PendingDevice> {
    status
        .pending
        .iter()
        .filter(|device| !seen.contains(&device.fingerprint))
        .collect()
}

/// Takes one watcher reading into the bridge. Returns the devices to announce
/// and whether the badge count changed (the caller repaints then).
fn take_reading(bridge: &mut WebBridge, status: &WebStatus) -> (Vec<(String, String)>, bool) {
    let fresh = fresh_pending(&bridge.seen, status)
        .into_iter()
        .map(|device| (device.name.clone(), device.fingerprint.clone()))
        .collect();
    bridge.seen = status
        .pending
        .iter()
        .map(|device| device.fingerprint.clone())
        .collect();
    let badge = status.pending.len() + status.requests.len();
    let changed = bridge.badge != badge;
    bridge.badge = badge;
    (fresh, changed)
}

/// What one poll answer means for the pairing on show: `None` keeps polling,
/// `Some(notice)` ends it (with that notice, if any). Every end that is not an
/// approval also makes the shell cancel the host's pairing.
fn poll_step(polled: &Result<PairPoll, String>) -> Option<Option<String>> {
    match polled {
        Ok(PairPoll::Pending) => None,
        Ok(PairPoll::Approved { .. }) => Some(None),
        Ok(PairPoll::Denied) => Some(Some("Pairing was denied in the browser.".to_owned())),
        Ok(PairPoll::Expired) => Some(Some("The pairing code expired.".to_owned())),
        Err(error) => Some(Some(format!("Web app: {error}"))),
    }
}

/// (id, name) pairs of the projects and the Monos the host publishes.
pub(super) type CatalogChoices = (Vec<(String, String)>, Vec<(String, String)>);

impl Shell {
    /// Starts the watcher that announces devices waiting for confirmation.
    /// Call right after `init_mobile_host`, which registers the host.
    pub(super) fn init_web_link(&mut self, cx: &mut Context<Self>) {
        // GPUI's deterministic executor cannot be woken by the link's threads.
        if cx.is_test() {
            return;
        }
        let watcher = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                let Some(host) = attached_host() else {
                    continue;
                };
                let status = cx
                    .background_executor()
                    .spawn(async move { host.web_status() })
                    .await;
                if this
                    .update(cx, |this, cx| this.web_status_watched(&status, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        cx.default_global::<WebBridge>().watcher = Some(watcher);
    }

    /// One watcher tick: one system notification per new pending device
    /// while Settings is closed, and the badge count.
    fn web_status_watched(&mut self, status: &WebStatus, cx: &mut Context<Self>) {
        let notify = self.settings.is_none() && self.workspace.preferences.notifications;
        let sounds = self.workspace.preferences.sounds;
        let (fresh, changed) = take_reading(cx.default_global::<WebBridge>(), status);
        if changed {
            cx.notify();
        }
        if !notify {
            return;
        }
        for (name, fingerprint) in fresh {
            let tag: String = fingerprint.chars().take(16).collect();
            // The receiver is dropped: delivery is best effort.
            let _ = platform::show(platform::Notification {
                tag: format!("web-pending-{tag}"),
                subtitle: "Web app".into(),
                body: format!("Web app: {name} asks for access"),
                sound: sounds,
            });
        }
    }

    /// How many devices and requests wait for the user, for the Connections
    /// section's badge.
    pub(super) fn web_badge(&self, cx: &App) -> usize {
        cx.try_global::<WebBridge>()
            .map_or(0, |bridge| bridge.badge)
    }

    /// The (projects, monos) the host publishes, as (id, name) pairs.
    pub(super) fn web_catalog_choices(&self) -> CatalogChoices {
        attached_host()
            .map(|host| host.catalog_choices())
            .unwrap_or_default()
    }

    pub(super) fn web_status(&self) -> Option<WebStatus> {
        attached_host().map(|host| host.web_status())
    }

    pub(super) fn web_pairing(&self, cx: &App) -> Option<PairTicket> {
        cx.try_global::<WebBridge>()
            .and_then(|bridge| bridge.pairing.clone())
    }

    pub(super) fn web_notice(&self, cx: &App) -> Option<String> {
        cx.try_global::<WebBridge>()
            .and_then(|bridge| bridge.notice.clone())
    }

    fn web_set_notice(&mut self, notice: Option<String>, cx: &mut Context<Self>) {
        cx.default_global::<WebBridge>().notice = notice;
        self.sync_settings(cx);
        cx.notify();
    }

    /// Routes the Web app events of the Settings page.
    pub(super) fn web_settings_event(&mut self, event: &SettingsEvent, cx: &mut Context<Self>) {
        match event {
            SettingsEvent::WebPair { host_url } => self.web_pair(host_url.clone(), cx),
            SettingsEvent::WebCancelPair => self.web_cancel_pair(cx),
            SettingsEvent::WebUnpair => {
                self.web_cancel_pair(cx);
                self.web_action("Unpair", cx, |host| host.web_unpair());
            }
            SettingsEvent::WebDenyDevice(fingerprint) => {
                let fingerprint = fingerprint.clone();
                self.web_action("Deny", cx, move |host| host.web_deny(&fingerprint));
            }
            SettingsEvent::WebSetPermission {
                device_id,
                permission,
            } => {
                let (device_id, permission) = (device_id.clone(), *permission);
                self.web_action("Permission", cx, move |host| {
                    let device = host
                        .web_status()
                        .devices
                        .into_iter()
                        .find(|device| device.id == device_id)
                        .ok_or("That device is no longer paired")?;
                    host.set_device_access(&device_id, permission, device.projects, device.monos)
                        .map(|_| ())
                });
            }
            SettingsEvent::WebConfirmDevice {
                fingerprint,
                permission,
                projects,
                monos,
            } => {
                let (fingerprint, permission) = (fingerprint.clone(), *permission);
                let (projects, monos) = (projects.clone(), monos.clone());
                self.web_action("Confirm", cx, move |host| {
                    host.web_confirm(&fingerprint, permission, projects, monos)
                        .map(|_| ())
                });
            }
            SettingsEvent::PhoneSetProjects {
                device_id,
                projects,
            } => {
                let (device_id, projects) = (device_id.clone(), projects.clone());
                self.web_action("Projects", cx, move |host| {
                    let device = host
                        .status()
                        .devices
                        .into_iter()
                        .find(|device| device.id == device_id)
                        .ok_or("That device is no longer paired")?;
                    host.set_device_access(&device_id, device.permission, projects, device.monos)
                        .map(|_| ())
                });
            }
            SettingsEvent::WebRevokeDevice(device_id) => {
                let device_id = device_id.clone();
                self.web_action("Revoke", cx, move |host| host.web_revoke(&device_id));
            }
            SettingsEvent::WebSetPolicyFlag { flag, on } => {
                self.web_set_policy_flag(*flag, *on, cx);
            }
            SettingsEvent::WebAnswerRequest { device_id, approve } => {
                let (device_id, approve) = (device_id.clone(), *approve);
                self.web_action("Request", cx, move |host| {
                    host.web_answer_request(&device_id, approve)
                });
            }
            _ => {}
        }
    }

    /// Switches a remote-action toggle. The process flags change at once (a
    /// device is judged by them on its next call), the file is written off the
    /// UI thread, and the connected web devices are told. Saves run one at a
    /// time, each writing the flags as they are when it runs, so the last save
    /// always holds the latest state. A failed save fails closed: the flag
    /// ends OFF, whichever way it was being switched, so the page never grants
    /// more than what was saved, and the notice says so.
    fn web_set_policy_flag(&mut self, flag: WebPolicyFlag, on: bool, cx: &mut Context<Self>) {
        let Some(path) = self
            .data_dir
            .as_ref()
            .map(|dir| dir.join(policy::SETTINGS_FILE))
        else {
            self.web_set_notice(
                Some("Web app: the settings folder is unavailable.".into()),
                cx,
            );
            return;
        };
        policy::set_flags(with_flag(policy::flags(), flag, on));
        self.sync_settings(cx);
        let task = cx.background_executor().spawn(async move {
            let _one_at_a_time = SAVING.lock().unwrap_or_else(|e| e.into_inner());
            // One retry: a busy disk or a short-lived lock should not cost the setting.
            policy::save_settings(&path, &policy::settings())
                .or_else(|_| policy::save_settings(&path, &policy::settings()))
        });
        cx.spawn(async move |this, cx| {
            let saved = task.await;
            let _ = this.update(cx, |this, cx| {
                let notice = saved.err().map(|error| {
                    // Only this flag moves: another toggle may have changed meanwhile.
                    policy::set_flags(with_flag(policy::flags(), flag, false));
                    eprintln!("web app: could not save the remote-action settings: {error}");
                    format!(
                        "Web app: could not save the setting ({error}). It stays off for now, but the change is not saved and will revert when the app restarts."
                    )
                });
                monocode_core::mobile::web_link::resend_state();
                this.web_set_notice(notice, cx);
            });
        })
        .detach();
    }

    /// Runs a blocking host call off the UI thread, then shows its error on
    /// the Web app page and reads the status again.
    fn web_action(
        &mut self,
        label: &'static str,
        cx: &mut Context<Self>,
        work: impl FnOnce(MobileHost) -> Result<(), String> + Send + 'static,
    ) {
        let Some(host) = attached_host() else {
            self.web_set_notice(Some("The web app is not available.".into()), cx);
            return;
        };
        let task = cx.background_executor().spawn(async move { work(host) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let notice = result
                    .err()
                    .map(|error| format!("Web app: {label}: {error}"));
                this.web_set_notice(notice, cx);
            });
        })
        .detach();
    }

    fn web_pair(&mut self, host_url: String, cx: &mut Context<Self>) {
        let Some(host) = attached_host() else {
            self.web_set_notice(Some("The web app is not available.".into()), cx);
            return;
        };
        let bridge = cx.default_global::<WebBridge>();
        bridge.notice = None;
        let generation = bridge.pairing_generation;
        let name = machine_name();
        let task = cx
            .background_executor()
            .spawn(async move { host.web_pair_start(&host_url, &name) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(ticket) => this.web_pair_started(ticket, generation, cx),
                Err(error) => this.web_set_notice(Some(format!("Web app: {error}")), cx),
            });
        })
        .detach();
    }

    /// Shows the code and polls until approved, denied, expired or cancelled.
    fn web_pair_started(&mut self, ticket: PairTicket, generation: u64, cx: &mut Context<Self>) {
        if cx.default_global::<WebBridge>().pairing_generation != generation {
            // Cancelled or closed while the code was being fetched: forget it.
            self.web_forget_pairing(cx);
            return;
        }
        let interval = Duration::from_secs(ticket.interval.max(1));
        let expires_at = ticket.expires_at;
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                let Some(host) = attached_host() else {
                    break;
                };
                if unix_now() >= expires_at {
                    let _ = this.update(cx, |this, cx| {
                        this.web_forget_pairing(cx);
                        this.web_pair_finished(Some("The pairing code expired.".into()), cx)
                    });
                    break;
                }
                let polled = cx
                    .background_executor()
                    .spawn(async move { host.web_pair_poll() })
                    .await;
                match this.update(cx, |this, cx| this.web_pair_polled(polled, cx)) {
                    Ok(false) => {}
                    _ => break,
                }
            }
        });
        let bridge = cx.default_global::<WebBridge>();
        bridge.pairing = Some(ticket);
        bridge.notice = None;
        bridge.poll = Some(poll);
        self.sync_settings(cx);
        cx.notify();
    }

    /// True when the pairing is over.
    fn web_pair_polled(
        &mut self,
        polled: Result<PairPoll, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if cx.default_global::<WebBridge>().pairing.is_none() {
            return true;
        }
        let Some(notice) = poll_step(&polled) else {
            return false;
        };
        if !matches!(polled, Ok(PairPoll::Approved { .. })) {
            self.web_forget_pairing(cx);
        }
        self.web_pair_finished(notice, cx);
        true
    }

    fn web_pair_finished(&mut self, notice: Option<String>, cx: &mut Context<Self>) {
        let bridge = cx.default_global::<WebBridge>();
        bridge.pairing = None;
        bridge.poll = None;
        bridge.notice = notice;
        self.sync_settings(cx);
        cx.notify();
    }

    /// Closes the code and tells the host to forget the pairing.
    pub(super) fn web_cancel_pair(&mut self, cx: &mut Context<Self>) {
        let bridge = cx.default_global::<WebBridge>();
        let showing = bridge.pairing.take().is_some();
        bridge.poll = None;
        // A ticket still being fetched is dropped when it arrives.
        bridge.pairing_generation += 1;
        self.web_forget_pairing(cx);
        if !showing {
            return;
        }
        self.sync_settings(cx);
        cx.notify();
    }

    /// Tells the host to forget its pairing (off the UI thread).
    fn web_forget_pairing(&mut self, cx: &mut Context<Self>) {
        if let Some(host) = attached_host() {
            cx.background_executor()
                .spawn(async move { host.web_pair_cancel() })
                .detach();
        }
    }
}

/// Policy saves run one at a time.
static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `flags` with `flag` set to `on`.
fn with_flag(mut flags: PolicyFlags, flag: WebPolicyFlag, on: bool) -> PolicyFlags {
    match flag {
        WebPolicyFlag::GitPush => flags.remote_git_push = on,
        WebPolicyFlag::CloudStart => flags.remote_cloud_start = on,
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(fingerprint: &str) -> PendingDevice {
        PendingDevice {
            fingerprint: fingerprint.into(),
            name: "Browser".into(),
            server_device_id: "srv".into(),
            online: true,
        }
    }

    /// The policy flags are process-wide: these tests run one at a time.
    static POLICY: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monoptah-policy-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The toggle changes the live flags at once, is written to
    /// `mobile-policy.json` with the deny patterns kept, and a reload carries
    /// it. Both toggles stay independent.
    #[gpui::test]
    fn a_policy_toggle_is_saved_and_a_reload_carries_it(cx: &mut gpui::TestAppContext) {
        let _serial = POLICY.lock().unwrap_or_else(|e| e.into_inner());
        let (window, _, _) = crate::shell::tests::window(cx);
        let dir = scratch_dir("save");
        policy::install_settings(policy::PolicySettings {
            deny_patterns: [("/work/p".to_owned(), vec!["*.pem".to_owned()])].into(),
            ..Default::default()
        });
        assert_eq!(policy::flags(), PolicyFlags::default(), "off by default");
        let event = |flag, on| SettingsEvent::WebSetPolicyFlag { flag, on };
        window
            .update(cx, |shell, _, cx| {
                shell.data_dir = Some(dir.clone());
                shell.web_settings_event(&event(WebPolicyFlag::CloudStart, true), cx);
                assert!(policy::flags().remote_cloud_start);
                assert!(!policy::flags().remote_git_push);
            })
            .unwrap();
        cx.run_until_parked();
        let saved = policy::load_settings(&dir.join(policy::SETTINGS_FILE));
        assert!(saved.flags.remote_cloud_start && !saved.flags.remote_git_push);
        assert_eq!(saved.deny_patterns["/work/p"], ["*.pem"]);
        window
            .update(cx, |shell, _, cx| {
                shell.web_settings_event(&event(WebPolicyFlag::GitPush, true), cx);
                shell.web_settings_event(&event(WebPolicyFlag::CloudStart, false), cx);
            })
            .unwrap();
        cx.run_until_parked();
        let saved = policy::load_settings(&dir.join(policy::SETTINGS_FILE));
        assert!(saved.flags.remote_git_push && !saved.flags.remote_cloud_start);
        assert_eq!(saved, policy::settings());
        policy::install_settings(Default::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A save that fails puts the flag back and says why.
    #[gpui::test]
    fn a_failed_save_reverts_the_flag_and_shows_a_notice(cx: &mut gpui::TestAppContext) {
        let _serial = POLICY.lock().unwrap_or_else(|e| e.into_inner());
        let (window, _, _) = crate::shell::tests::window(cx);
        let dir = scratch_dir("fail");
        policy::install_settings(Default::default());
        window
            .update(cx, |shell, _, cx| {
                shell.data_dir = Some(dir.join("missing"));
                shell.web_settings_event(
                    &SettingsEvent::WebSetPolicyFlag {
                        flag: WebPolicyFlag::GitPush,
                        on: true,
                    },
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(policy::flags(), PolicyFlags::default());
        window
            .update(cx, |shell, _, cx| {
                let notice = shell.web_notice(cx).expect("a notice");
                assert!(notice.contains("could not save"), "{notice}");
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M1: turning a flag OFF whose save fails leaves it OFF (never back on),
    /// and the notice says it stays off.
    #[gpui::test]
    fn a_failed_save_never_leaves_a_flag_on(cx: &mut gpui::TestAppContext) {
        let _serial = POLICY.lock().unwrap_or_else(|e| e.into_inner());
        let (window, _, _) = crate::shell::tests::window(cx);
        let dir = scratch_dir("closed");
        policy::install_settings(policy::PolicySettings {
            flags: PolicyFlags {
                remote_cloud_start: true,
                remote_git_push: true,
                ..Default::default()
            },
            ..Default::default()
        });
        window
            .update(cx, |shell, _, cx| {
                shell.data_dir = Some(dir.join("missing"));
                shell.web_settings_event(
                    &SettingsEvent::WebSetPolicyFlag {
                        flag: WebPolicyFlag::CloudStart,
                        on: false,
                    },
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        let flags = policy::flags();
        assert!(!flags.remote_cloud_start, "off stays off");
        assert!(flags.remote_git_push, "another flag is not touched");
        window
            .update(cx, |shell, _, cx| {
                let notice = shell.web_notice(cx).expect("a notice");
                assert!(notice.contains("could not save"), "{notice}");
                assert!(notice.contains("stays off"), "{notice}");
                assert!(notice.contains("revert when the app restarts"), "{notice}");
            })
            .unwrap();
        policy::install_settings(Default::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_unannounced_pending_devices_are_fresh() {
        let status = WebStatus {
            pending: vec![pending("aa"), pending("bb")],
            ..Default::default()
        };
        let seen: HashSet<String> = ["aa".to_owned()].into();
        let fresh = fresh_pending(&seen, &status);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].fingerprint, "bb");
        assert!(fresh_pending(&HashSet::from(["aa".into(), "bb".into()]), &status).is_empty());
    }

    #[test]
    fn a_changed_badge_count_asks_for_a_repaint() {
        let mut bridge = WebBridge::default();
        let mut status = WebStatus {
            pending: vec![pending("aa")],
            ..Default::default()
        };
        let (fresh, changed) = take_reading(&mut bridge, &status);
        assert!(changed && fresh.len() == 1 && bridge.badge == 1);
        let (fresh, changed) = take_reading(&mut bridge, &status);
        assert!(!changed && fresh.is_empty());
        status.pending.clear();
        let (_, changed) = take_reading(&mut bridge, &status);
        assert!(changed && bridge.badge == 0);
    }

    #[test]
    fn a_failed_poll_ends_the_pairing_and_only_a_pending_one_keeps_it() {
        assert_eq!(poll_step(&Ok(PairPoll::Pending)), None);
        assert_eq!(
            poll_step(&Ok(PairPoll::Approved {
                account: "a".into()
            })),
            Some(None)
        );
        assert!(matches!(poll_step(&Ok(PairPoll::Denied)), Some(Some(_))));
        assert!(matches!(poll_step(&Ok(PairPoll::Expired)), Some(Some(_))));
        let ended = poll_step(&Err("The web server answered 404".into()));
        assert_eq!(
            ended,
            Some(Some("Web app: The web server answered 404".into()))
        );
    }
}
