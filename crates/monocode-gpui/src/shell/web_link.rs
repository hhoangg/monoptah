//! The web link of the shell: the bridge between the core web link and the
//! workspace. It runs the Settings > Web app actions on the background
//! executor, polls a pairing until the browser answers, and watches for
//! devices that wait for confirmation while Settings is closed.
use super::*;
use crate::notifications::{platform, surface::glass_frame};
use crate::settings::{SettingsEvent, WebPolicyFlag};
use gpui::{Global, Task};
use monocode_core::mobile::{
    AccessChange, DeviceKind, WebStatus,
    host::MobileHost,
    policy::{self, PolicyFlags},
    terminal::TerminalStatus,
    web_link::{PairPoll, PairTicket, PendingDevice, attached_host},
};
use std::{
    collections::HashSet,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// How often the watcher reads the web status. One second, so a code for the
/// web terminal is on screen soon after the browser asks for it.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

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
    /// Access changes already announced, by (time, device id, text).
    seen_changes: HashSet<(u64, String, String)>,
    /// Devices and requests waiting for the user, for the Connections badge.
    badge: usize,
    /// Bumped whenever a pairing is cancelled or Settings closes, so a ticket
    /// that arrives late is dropped instead of reopening the dialog.
    pairing_generation: u64,
    /// Open web terminals and the codes to read out, for the corner panel.
    terminals: TerminalStatus,
    /// Codes already announced with a notification.
    announced_codes: HashSet<String>,
}

impl Global for WebBridge {}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The name the web server shows for this desktop: the same one phone pairing uses.
fn machine_name() -> String {
    monocode_core::mobile::host::name()
}

/// A code on screen, by device, project and the code itself.
fn code_key(code: &monocode_core::mobile::terminal::PendingCode) -> String {
    format!(
        "{}\u{0}{}\u{0}{}",
        code.device_name, code.project, code.code
    )
}

/// "ABC234" as "ABC 234", easier to read out.
fn group_code(code: &str) -> String {
    let half = code.len() / 2;
    format!("{} {}", &code[..half], &code[half..])
}

/// The pending devices whose fingerprint was not announced yet.
fn fresh_pending<'a>(seen: &HashSet<String>, status: &'a WebStatus) -> Vec<&'a PendingDevice> {
    status
        .pending
        .iter()
        .filter(|device| !seen.contains(&device.fingerprint))
        .collect()
}

/// The access changes that were not announced yet.
fn fresh_changes<'a>(
    seen: &HashSet<(u64, String, String)>,
    status: &'a WebStatus,
) -> Vec<&'a AccessChange> {
    status
        .access_changes
        .iter()
        .filter(|change| !seen.contains(&change_key(change)))
        .collect()
}

fn change_key(change: &AccessChange) -> (u64, String, String) {
    (change.at, change.device_id.clone(), change.text.clone())
}

/// (pending devices, access changes) that may raise a system notification.
/// A pending device is announced only while Settings is closed; an access
/// change also while it is open, since the owner did not make it. Both follow
/// the notifications preference.
fn notify_gates(settings_open: bool, notifications: bool) -> (bool, bool) {
    (!settings_open && notifications, notifications)
}

/// The notification text of an access change.
fn access_change_body(change: &AccessChange) -> String {
    format!("Web app: {}", change.text)
}

/// Takes the access changes of one reading: returns the ones to announce.
fn take_changes(bridge: &mut WebBridge, status: &WebStatus) -> Vec<AccessChange> {
    let fresh = fresh_changes(&bridge.seen_changes, status)
        .into_iter()
        .cloned()
        .collect();
    bridge.seen_changes = status.access_changes.iter().map(change_key).collect();
    fresh
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
                let (status, terminals) = cx
                    .background_executor()
                    .spawn(async move { (host.web_status(), host.terminal_status()) })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.web_terminals_watched(terminals, cx);
                        this.web_status_watched(&status, cx);
                    })
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
        let (notify, notify_changes) = notify_gates(
            self.settings.is_some(),
            self.workspace.preferences.notifications,
        );
        let sounds = self.workspace.preferences.sounds;
        let (fresh, changed) = take_reading(cx.default_global::<WebBridge>(), status);
        let widened = take_changes(cx.default_global::<WebBridge>(), status);
        if changed || !widened.is_empty() {
            cx.notify();
        }
        if notify_changes {
            for change in widened {
                let tag: String = change.device_id.chars().take(16).collect();
                let _ = platform::show(platform::Notification {
                    tag: format!("web-access-{tag}-{}", change.at),
                    subtitle: "Web app".into(),
                    body: access_change_body(&change),
                    sound: sounds,
                });
            }
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

    /// One watcher tick for the web terminal: keeps what the corner panel
    /// shows and raises one notification per new code (without the code: it
    /// is read in the app's window).
    fn web_terminals_watched(&mut self, terminals: TerminalStatus, cx: &mut Context<Self>) {
        let notifications = self.workspace.preferences.notifications;
        let sounds = self.workspace.preferences.sounds;
        let bridge = cx.default_global::<WebBridge>();
        let fresh: Vec<_> = terminals
            .codes
            .iter()
            .filter(|code| bridge.announced_codes.insert(code_key(code)))
            .cloned()
            .collect();
        bridge
            .announced_codes
            .retain(|key| terminals.codes.iter().any(|code| &code_key(code) == key));
        let changed = bridge.terminals != terminals;
        bridge.terminals = terminals;
        if changed {
            cx.notify();
        }
        if notifications {
            for code in fresh {
                let _ = platform::show(platform::Notification {
                    tag: "web-terminal-code".into(),
                    subtitle: "Web terminal".into(),
                    body: format!(
                        "{} asks for a shell in {}. Open Monoptah to read the code.",
                        code.device_name, code.project
                    ),
                    sound: sounds,
                });
            }
        }
    }

    /// The Kill button of an open web terminal.
    fn web_kill_terminal(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(host) = attached_host() else {
            return;
        };
        let killed = id.clone();
        cx.background_executor()
            .spawn(async move {
                host.terminal_kill(&id);
            })
            .detach();
        // The next tick drops it from the panel; hide it now.
        cx.default_global::<WebBridge>()
            .terminals
            .open
            .retain(|open| open.id != killed);
        cx.notify();
    }

    /// The corner panel: a code to read out to the browser, and every open
    /// web terminal with its device name and a Kill button. On top of the
    /// workspace, whatever page is open.
    pub(super) fn web_terminal_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = cx.try_global::<WebBridge>()?.terminals.clone();
        if status.open.is_empty() && status.codes.is_empty() {
            return None;
        }
        let mut stack = div()
            .absolute()
            .top(px(44.))
            .right(px(16.))
            .w(px(340.))
            .flex()
            .flex_col()
            .gap_2();
        for code in &status.codes {
            stack = stack.child(
                glass_frame()
                    .id(SharedString::from(format!("web-terminal-code-{}", code.device_name)))
                    .role(Role::Status)
                    .debug_selector(|| "web-terminal-code".into())
                    .occlude()
                    .p_3()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Web terminal request"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(12.))
                            .text_color(content(0.7))
                            .child(format!(
                                "{} wants a shell in {}. Type this code in the browser:",
                                code.device_name, code.project
                            )),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_size(px(28.))
                            .font_weight(FontWeight::BOLD)
                            .debug_selector(|| "web-terminal-code-text".into())
                            .child(group_code(&code.code)),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(11.))
                            .text_color(content(0.5))
                            .child(format!(
                                "Works once, for {} more seconds. If you did not ask for this, ignore it.",
                                code.expires_in_ms / 1000
                            )),
                    ),
            );
        }
        for open in &status.open {
            let id = open.id.clone();
            let label = format!(
                "Web terminal open: {} ({}){}",
                open.device_name,
                open.project,
                if open.detached { ", disconnected" } else { "" }
            );
            stack = stack.child(
                glass_frame()
                    .id(SharedString::from(format!("web-terminal-open-{id}")))
                    .role(Role::Status)
                    .debug_selector(|| "web-terminal-open".into())
                    .occlude()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(12.))
                            .child(label),
                    )
                    .child(
                        crate::modal::danger_ghost_button(
                            SharedString::from(format!("web-terminal-kill-{id}")),
                            "Kill",
                        )
                        .debug_selector(|| "web-terminal-kill".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.web_kill_terminal(id.clone(), cx);
                        })),
                    ),
            );
        }
        Some(gpui::deferred(stack).with_priority(110).into_any_element())
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
                    host.set_device_access(
                        &device_id,
                        DeviceKind::Web,
                        permission,
                        device.projects,
                        device.monos,
                    )
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
                        .find(|device| device.id == device_id && device.kind == DeviceKind::Phone)
                        .ok_or("That device is no longer paired")?;
                    host.set_device_access(
                        &device_id,
                        DeviceKind::Phone,
                        device.permission,
                        projects,
                        device.monos,
                    )
                    .map(|_| ())
                });
            }
            SettingsEvent::WebSetSelection {
                device_id,
                projects,
                monos,
            } => {
                let (device_id, projects, monos) =
                    (device_id.clone(), projects.clone(), monos.clone());
                self.web_action("Projects", cx, move |host| {
                    let device = host
                        .web_status()
                        .devices
                        .into_iter()
                        .find(|device| device.id == device_id && device.kind == DeviceKind::Web)
                        .ok_or("That device is no longer paired")?;
                    host.set_device_access(
                        &device_id,
                        DeviceKind::Web,
                        device.permission,
                        projects,
                        monos,
                    )
                    .map(|_| ())
                });
            }
            SettingsEvent::WebRevokeDevice(device_id) => {
                let device_id = device_id.clone();
                self.web_action("Revoke", cx, move |host| host.web_revoke(&device_id));
            }
            SettingsEvent::WebSetTerminalAllowed { device_id, on } => {
                let (device_id, on) = (device_id.clone(), *on);
                self.web_action("Terminal", cx, move |host| {
                    host.web_set_terminal_allowed(&device_id, on)
                });
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
            // One retry (inside): a busy disk or a short-lived lock should not
            // cost the setting. The core's save shares its lock with the web
            // terminal switch, so neither writes over the other.
            policy::persist_settings(&path)
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
                // Open watches drop an `approvals` or `cloud` field that is now off.
                this.push_mobile_watches(cx);
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
        let task = cx
            .background_executor()
            .spawn(async move { host.web_pair_start(&host_url, &machine_name()) });
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
        WebPolicyFlag::Approvals => flags.remote_approvals = on,
        WebPolicyFlag::PushContent => flags.show_push_content = on,
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_shown_in_two_groups_and_keyed_by_device_project_and_code() {
        assert_eq!(group_code("ABC234"), "ABC 234");
        let code = |device: &str, code: &str| monocode_core::mobile::terminal::PendingCode {
            device_name: device.into(),
            project: "Proj".into(),
            code: code.into(),
            expires_in_ms: 1,
        };
        assert_ne!(
            code_key(&code("A", "ABC234")),
            code_key(&code("B", "ABC234"))
        );
        assert_ne!(
            code_key(&code("A", "ABC234")),
            code_key(&code("A", "ABC235"))
        );
        // The countdown is not part of the key: one code, one notification.
        let mut later = code("A", "ABC234");
        later.expires_in_ms = 40_000;
        assert_eq!(code_key(&code("A", "ABC234")), code_key(&later));
    }

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

    /// The approvals toggle defaults off, is saved like the others, and a
    /// change re-sends the state (`resend_state` runs after the save).
    #[gpui::test]
    fn the_approvals_toggle_defaults_off_and_is_saved(cx: &mut gpui::TestAppContext) {
        let _serial = POLICY.lock().unwrap_or_else(|e| e.into_inner());
        let (window, _, _) = crate::shell::tests::window(cx);
        let dir = scratch_dir("approvals");
        policy::install_settings(Default::default());
        assert!(!policy::flags().remote_approvals, "off by default");
        assert!(!policy::approvals_enabled());
        window
            .update(cx, |shell, _, cx| {
                shell.data_dir = Some(dir.clone());
                shell.web_settings_event(
                    &SettingsEvent::WebSetPolicyFlag {
                        flag: WebPolicyFlag::Approvals,
                        on: true,
                    },
                    cx,
                );
                assert!(policy::flags().remote_approvals);
                assert!(policy::approvals_enabled());
            })
            .unwrap();
        cx.run_until_parked();
        let saved = policy::load_settings(&dir.join(policy::SETTINGS_FILE));
        assert!(saved.flags.remote_approvals);
        assert!(!saved.flags.remote_git_push && !saved.flags.remote_cloud_start);
        policy::install_settings(Default::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The content toggle defaults on and its change is saved per desktop.
    #[gpui::test]
    fn the_push_content_toggle_defaults_on_and_is_saved(cx: &mut gpui::TestAppContext) {
        let _serial = POLICY.lock().unwrap_or_else(|e| e.into_inner());
        let (window, _, _) = crate::shell::tests::window(cx);
        let dir = scratch_dir("push-content");
        policy::install_settings(Default::default());
        assert!(policy::flags().show_push_content, "on by default");
        assert!(
            policy::load_settings(&dir.join(policy::SETTINGS_FILE))
                .flags
                .show_push_content,
            "an old or missing file keeps it on"
        );
        window
            .update(cx, |shell, _, cx| {
                shell.data_dir = Some(dir.clone());
                shell.web_settings_event(
                    &SettingsEvent::WebSetPolicyFlag {
                        flag: WebPolicyFlag::PushContent,
                        on: false,
                    },
                    cx,
                );
                assert!(!policy::flags().show_push_content);
            })
            .unwrap();
        cx.run_until_parked();
        let saved = policy::load_settings(&dir.join(policy::SETTINGS_FILE));
        assert!(!saved.flags.show_push_content);
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
    fn an_access_change_notifies_with_settings_open_but_not_when_notifications_are_off() {
        assert_eq!(notify_gates(true, true), (false, true));
        assert_eq!(notify_gates(false, true), (true, true));
        assert_eq!(notify_gates(true, false), (false, false));
        assert_eq!(notify_gates(false, false), (false, false));
    }

    #[test]
    fn each_access_change_is_announced_once_with_its_text() {
        let change = |at: u64, text: &str| AccessChange {
            at,
            device_id: "dev".into(),
            device_name: "Work laptop".into(),
            text: text.into(),
        };
        let mut bridge = WebBridge::default();
        let mut status = WebStatus {
            access_changes: vec![change(1, "Work laptop now has access to Alpha")],
            ..Default::default()
        };
        let first = take_changes(&mut bridge, &status);
        assert_eq!(first.len(), 1);
        assert_eq!(
            access_change_body(&first[0]),
            "Web app: Work laptop now has access to Alpha"
        );
        assert!(take_changes(&mut bridge, &status).is_empty());
        status.access_changes.push(change(
            2,
            "Work laptop now has access to all projects, including new ones",
        ));
        let next = take_changes(&mut bridge, &status);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].at, 2);
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
