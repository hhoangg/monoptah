//! The web link of the shell: the bridge between the core web link and the
//! workspace. It runs the Settings > Web app actions on the background
//! executor, polls a pairing until the browser answers, and watches for
//! devices that wait for confirmation while Settings is closed.
use super::*;
use crate::notifications::platform;
use crate::settings::SettingsEvent;
use gpui::{Global, Task};
use monocode_core::mobile::{
    WebStatus,
    host::MobileHost,
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
        let bridge = cx.default_global::<WebBridge>();
        let fresh: Vec<(String, String)> = fresh_pending(&bridge.seen, status)
            .into_iter()
            .map(|device| (device.name.clone(), device.fingerprint.clone()))
            .collect();
        bridge.seen = status
            .pending
            .iter()
            .map(|device| device.fingerprint.clone())
            .collect();
        bridge.badge = status.pending.len() + status.requests.len();
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
            SettingsEvent::WebAnswerRequest { device_id, approve } => {
                let (device_id, approve) = (device_id.clone(), *approve);
                self.web_action("Request", cx, move |host| {
                    host.web_answer_request(&device_id, approve)
                });
            }
            _ => {}
        }
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
        cx.default_global::<WebBridge>().notice = None;
        let name = machine_name();
        let task = cx
            .background_executor()
            .spawn(async move { host.web_pair_start(&host_url, &name) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(ticket) => this.web_pair_started(ticket, cx),
                Err(error) => this.web_set_notice(Some(format!("Web app: {error}")), cx),
            });
        })
        .detach();
    }

    /// Shows the code and polls until approved, denied, expired or cancelled.
    fn web_pair_started(&mut self, ticket: PairTicket, cx: &mut Context<Self>) {
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
        let notice = match polled {
            Ok(PairPoll::Pending) => return false,
            Ok(PairPoll::Approved { .. }) => None,
            Ok(PairPoll::Denied) => Some("Pairing was denied in the browser.".to_owned()),
            Ok(PairPoll::Expired) => Some("The pairing code expired.".to_owned()),
            Err(error) => Some(format!("Web app: {error}")),
        };
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
        if !showing {
            return;
        }
        if let Some(host) = attached_host() {
            cx.background_executor()
                .spawn(async move { host.web_pair_cancel() })
                .detach();
        }
        self.sync_settings(cx);
        cx.notify();
    }
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
}
