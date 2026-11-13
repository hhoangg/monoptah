//! Settings > Cloud > Cost control: stop a cloud workspace that nothing needs any more. It only
//! ever stops (the work is kept and the next send resumes it); it never deletes.
use crate::settings::SettingsView;
use gpui::{AnyElement, Context};
use monocode_core::cloud_sessions::{
    CloudBackendKind,
    auto_stop::{DEFAULT_SANDBOX_IDLE_MINUTES, IDLE_CHOICES},
};

// These are Monoptah's own rule (it stops the workspace). They are worded apart from the
// per-project "Stop when idle" of the cloud settings, which is GitHub's codespace timeout.
const SANDBOX_IDLE: &str = "Auto-stop sandboxes after inactivity";
const CODESPACE_IDLE: &str = "Auto-stop codespaces after inactivity";
const SANDBOX_UNUSED: &str = "Auto-stop a sandbox when its last session is gone";
const CODESPACE_UNUSED: &str = "Auto-stop a codespace when its last session is gone";

/// `off`, then each [`IDLE_CHOICES`] in minutes.
const OPTIONS: [(&str, &str); 5] = [
    ("off", "Off"),
    ("5", "5 min"),
    ("10", "10 min"),
    ("15", "15 min"),
    ("30", "30 min"),
];

/// The segmented value of an idle setting.
fn option_of(minutes: Option<u32>) -> String {
    minutes.map_or_else(|| "off".to_owned(), |m| m.to_string())
}

/// The idle setting a segmented value stands for.
fn minutes_of(option: &str) -> Option<u32> {
    option
        .parse()
        .ok()
        .filter(|minutes| IDLE_CHOICES.contains(minutes))
}

impl SettingsView {
    /// The "Stop idle workspaces" group: one idle time and one "when unused" switch per backend.
    pub(in crate::settings) fn auto_stop_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let prefs = self.prefs.cloud_auto_stop;
        let sandbox = prefs.policy(CloudBackendKind::DockerSandbox);
        let codespace = prefs.policy(CloudBackendKind::Codespaces);
        let rows = vec![
            self.row(
                Some("cloud-auto-stop-sandbox"),
                true,
                SANDBOX_IDLE,
                Some(
                    format!(
                        "A sandbox bills per runtime second until its runtime limit ends it. Monoptah stops it after this long with no turn running, nothing queued or starting, no typing and no terminal open on it. Default {DEFAULT_SANDBOX_IDLE_MINUTES} min."
                    )
                    .into(),
                ),
                vec![self.segmented(
                    "cloud-auto-stop-sandbox",
                    SANDBOX_IDLE,
                    &option_of(sandbox.idle_minutes),
                    &OPTIONS,
                    cx,
                    |this, value, cx| {
                        this.update(cx, |p| {
                            p.cloud_auto_stop.sandbox.idle_minutes = minutes_of(value)
                        })
                    },
                )],
            ),
            self.row(
                Some("cloud-auto-stop-sandbox-unused"),
                false,
                SANDBOX_UNUSED,
                Some(
                    "Gone means archived or deleted. Waits a few minutes first. A sandbox you kept running when archiving, or removed from Monoptah, is left alone."
                        .into(),
                ),
                vec![self.toggle(
                    "cloud-auto-stop-sandbox-unused",
                    SANDBOX_UNUSED,
                    sandbox.stop_when_unused,
                    false,
                    cx,
                    |this, on, cx| {
                        this.update(cx, |p| p.cloud_auto_stop.sandbox.stop_when_unused = on)
                    },
                )],
            ),
            self.row(
                Some("cloud-auto-stop-codespace"),
                false,
                CODESPACE_IDLE,
                Some(
                    "Off by default: GitHub already idles a codespace out after its own timeout (Stop when idle in the cloud settings of a project). Turn this on to have Monoptah stop it sooner, by the same rules as sandboxes."
                        .into(),
                ),
                vec![self.segmented(
                    "cloud-auto-stop-codespace",
                    CODESPACE_IDLE,
                    &option_of(codespace.idle_minutes),
                    &OPTIONS,
                    cx,
                    |this, value, cx| {
                        this.update(cx, |p| {
                            p.cloud_auto_stop.codespace.idle_minutes = minutes_of(value)
                        })
                    },
                )],
            ),
            self.row(
                Some("cloud-auto-stop-codespace-unused"),
                false,
                CODESPACE_UNUSED,
                Some(
                    "A codespace you kept running when archiving, or removed from Monoptah, is left alone."
                        .into(),
                ),
                vec![self.toggle(
                    "cloud-auto-stop-codespace-unused",
                    CODESPACE_UNUSED,
                    codespace.stop_when_unused,
                    false,
                    cx,
                    |this, on, cx| {
                        this.update(cx, |p| p.cloud_auto_stop.codespace.stop_when_unused = on)
                    },
                )],
            ),
        ];
        self.group(
            Some("cloud-auto-stop"),
            false,
            "Cost control",
            Some(
                "Monoptah stops a workspace that is no longer needed, and only stops it: nothing is deleted and your work is kept. Send a message in its session to resume it. A turn that is running, a queued message, an unsaved copy or an open terminal keeps a workspace running. Workspaces without a Monoptah session are never stopped here; use Cloud workspaces for those."
                    .into(),
            ),
            None,
            rows,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Preferences;

    #[test]
    fn the_segments_round_trip_the_idle_choices() {
        assert_eq!(option_of(None), "off");
        assert_eq!(minutes_of("off"), None);
        for minutes in IDLE_CHOICES {
            assert_eq!(minutes_of(&option_of(Some(minutes))), Some(minutes));
            assert!(
                OPTIONS
                    .iter()
                    .any(|(value, _)| *value == minutes.to_string())
            );
        }
        assert_eq!(minutes_of("7"), None);
    }

    #[test]
    fn preferences_default_to_sandbox_on_codespace_off_and_survive_old_files() {
        let prefs = Preferences::default().cloud_auto_stop;
        assert_eq!(
            prefs.sandbox.idle_minutes,
            Some(DEFAULT_SANDBOX_IDLE_MINUTES)
        );
        assert_eq!(prefs.codespace.idle_minutes, None);
        let old: Preferences = serde_json::from_str(r#"{"sounds":false}"#).unwrap();
        assert_eq!(old.cloud_auto_stop, prefs);
        let mut changed = Preferences::default();
        changed.cloud_auto_stop.sandbox.idle_minutes = Some(30);
        changed.cloud_auto_stop.codespace.stop_when_unused = true;
        let loaded: Preferences =
            serde_json::from_str(&serde_json::to_string(&changed).unwrap()).unwrap();
        assert_eq!(loaded.cloud_auto_stop, changed.cloud_auto_stop);
    }
}
