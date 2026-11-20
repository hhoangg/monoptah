use super::*;
use crate::mobile::terminal::tests::FakeSpawner;
use crate::mobile::web_link::{desktop_api::ServerGrant, noise};
use std::sync::atomic::Ordering;

const PUBLIC: [u8; 32] = [9; 32];

/// Runs `future` while holding the policy settings lock, so no other test
/// replaces the process-wide settings (and with them the toggle) meanwhile.
fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    let _guard = policy::TEST_SETTINGS_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn encode_id(path: &str) -> String {
    path.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

struct World {
    // The runtime lives inside the host; the host drops first.
    host: MobileHost,
    spawner: Arc<FakeSpawner>,
    dir: tempfile::TempDir,
    project: String,
    device: String,
    peer: String,
    relay: String,
}

fn world(permission: Permission) -> World {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let project = encode_id(&root.canonicalize().unwrap().to_string_lossy());
    let host = MobileHost::open(dir.path().join("mobile-access.json"))
        .unwrap()
        .0
        // No wait between two codes: these tests ask for many.
        .with_terminal_limits(terminal::Limits {
            code_gap_ms: 0,
            ..terminal::Limits::default()
        });
    host.set_flags_for_test(true, true);
    let spawner = Arc::new(FakeSpawner::default());
    host.set_shell_spawner(spawner.clone());
    let device = format!("term-{}", uuid::Uuid::new_v4());
    let relay = uuid::Uuid::new_v4().to_string();
    let peer = noise::fingerprint(&PUBLIC);
    host.add_device_for_test(Device {
        id: device.clone(),
        name: "Laptop".into(),
        fingerprint: peer.clone(),
        push: None,
        kind: DeviceKind::Web,
        permission,
        projects: None,
        monos: None,
        server_device_id: Some(relay.clone()),
        created_at: None,
        last_seen: None,
    });
    host.inner.lock().unwrap().catalog =
        json!({"projects":[{"id": project, "name": "Proj"}],"monos":[],"sessions":[]});
    World {
        host,
        spawner,
        dir,
        project,
        device,
        peer,
        relay,
    }
}

impl World {
    async fn call(&self, method: &str, params: Value) -> Result<Value, Failure> {
        self.host
            .handle_web(
                &self.peer,
                json!({"version":VERSION,"id":"c1","method":method,"params":params}),
            )
            .await
    }

    fn allow(&self) {
        self.host
            .web_set_terminal_allowed(&self.device, true)
            .unwrap();
    }

    fn code(&self) -> String {
        self.host.terminal_status().codes[0].code.clone()
    }

    async fn open_with(&self, extra: Value) -> Result<String, Failure> {
        let mut params = json!({"projectId": self.project, "cols": 80, "rows": 24});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let step = self.call("terminal.open", params.clone()).await?;
        let challenge = step["challengeId"].as_str().unwrap().to_owned();
        params["challengeId"] = json!(challenge);
        params["code"] = json!(self.code());
        let opened = self.call("terminal.open", params).await?;
        Ok(opened["terminalId"].as_str().unwrap().to_owned())
    }

    async fn open(&self) -> String {
        self.open_with(json!({})).await.unwrap()
    }

    fn kills(&self) -> Vec<usize> {
        self.spawner
            .shells
            .lock()
            .unwrap()
            .iter()
            .map(|shell| shell.kills.load(Ordering::SeqCst))
            .collect()
    }

    fn grants(&self, state: &str, permission: Permission) -> ServerGrant {
        ServerGrant {
            device_id: self.relay.clone(),
            device_name: "Laptop".into(),
            public_key: URL_SAFE_NO_PAD.encode(PUBLIC),
            fingerprint: self.peer.clone(),
            state: state.into(),
            requested: None,
            permission: Some(permission),
        }
    }
}

fn code_of(failure: &Failure) -> &str {
    failure.code.as_deref().unwrap_or("")
}

const METHODS: [&str; 5] = [
    "terminal.open",
    "terminal.read",
    "terminal.write",
    "terminal.resize",
    "terminal.close",
];

#[test]
fn every_terminal_call_is_refused_until_the_desktop_allows_that_device() {
    run(async {
        let w = world(Permission::Control);
        for method in METHODS {
            let failure = w
                .call(
                    method,
                    json!({"projectId": w.project, "terminalId": "x", "cols": 80, "rows": 24}),
                )
                .await
                .unwrap_err();
            assert_eq!(code_of(&failure), "disabled", "{method}");
        }
        assert!(
            w.host.terminal_status().codes.is_empty(),
            "no code was shown"
        );
        assert!(w.spawner.shells.lock().unwrap().is_empty());
    });
}

#[test]
fn a_read_only_device_is_refused_and_cannot_be_allowed() {
    run(async {
        let w = world(Permission::Read);
        let error = w
            .host
            .web_set_terminal_allowed(&w.device, true)
            .unwrap_err();
        assert!(error.contains("control"), "{error}");
        assert!(!policy::terminal_allowed(&w.device));
        // Even if the toggle were somehow set, the device is still read-only.
        policy::set_terminal_allowed(&w.device, true);
        let failure = w
            .call(
                "terminal.open",
                json!({"projectId": w.project, "cols": 80, "rows": 24}),
            )
            .await
            .unwrap_err();
        assert_eq!(code_of(&failure), "read_only");
        policy::set_terminal_allowed(&w.device, false);
        assert!(w.host.web_set_terminal_allowed("nobody", true).is_err());
    });
}

#[test]
fn a_quic_phone_never_reaches_the_terminal_and_unknown_methods_fail_closed() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        let failure = w.call("terminal.exec", json!({})).await.unwrap_err();
        assert_eq!(code_of(&failure), "unsupported");
        // The phone path answers with text only, and refuses.
        let mut phone = w.host.inner.lock().unwrap().config.devices[0].clone();
        phone.id = "phone-row".into();
        phone.kind = DeviceKind::Phone;
        phone.fingerprint = "phone-fingerprint".into();
        w.host.add_device_for_test(phone);
        let refused = w
            .host
            .handle(
                "phone-fingerprint",
                json!({"version":VERSION,"id":"c","method":"terminal.open","params":{"projectId":w.project,"cols":80,"rows":24}}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused, policy::Code::Forbidden.message());
        w.host.set_flags_for_test(true, true);
    });
}

#[test]
fn the_two_step_open_shows_a_code_only_on_the_desktop_and_a_shell_echoes_through_the_relay_calls() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        let step = w
            .call(
                "terminal.open",
                json!({"projectId": w.project, "cols": 80, "rows": 24}),
            )
            .await
            .unwrap();
        // The answer holds the challenge, never the code.
        assert_eq!(step["codeLength"], 6);
        assert_eq!(step["attempts"], 3);
        let wire = step.to_string();
        assert!(!wire.contains(&w.code()), "the code stays on the desktop");
        assert!(w.spawner.shells.lock().unwrap().is_empty(), "no shell yet");
        // A wrong code is retyped, a right one opens.
        let params = |code: &str| json!({"projectId": w.project, "cols": 80, "rows": 24, "challengeId": step["challengeId"], "code": code});
        let wrong = if w.code() == "AAAAAA" {
            "BBBBBB"
        } else {
            "AAAAAA"
        };
        let failure = w.call("terminal.open", params(wrong)).await.unwrap_err();
        assert_eq!(code_of(&failure), "reauth_failed");
        let right = w.code();
        let opened = w.call("terminal.open", params(&right)).await.unwrap();
        let id = opened["terminalId"].as_str().unwrap().to_owned();
        assert_eq!(opened["cwd"], "Proj");
        assert_eq!(opened["idleTimeoutMs"], 900_000);
        assert_eq!(opened["maxDurationMs"], 28_800_000);
        let cwd = w.spawner.cwds.lock().unwrap()[0].clone();
        assert!(cwd.ends_with("proj"), "{cwd:?}");
        // The code was single use.
        let replay = w.call("terminal.open", params(&right)).await.unwrap_err();
        assert_eq!(code_of(&replay), "reauth_required");
        w.call(
            "terminal.write",
            json!({"terminalId": id, "data": "bHMNCg=="}),
        )
        .await
        .unwrap();
        w.call(
            "terminal.resize",
            json!({"terminalId": id, "cols": 120, "rows": 40}),
        )
        .await
        .unwrap();
        let read = w
            .call(
                "terminal.read",
                json!({"terminalId": id, "from": 0, "waitMs": 0}),
            )
            .await
            .unwrap();
        assert_eq!(read["data"], "bHMNCg==");
        assert_eq!(read["next"], 4);
        assert_eq!(read["exited"], Value::Null);
        w.call("terminal.close", json!({"terminalId": id}))
            .await
            .unwrap();
        assert_eq!(w.kills(), [1]);
        let after = w
            .call(
                "terminal.read",
                json!({"terminalId": id, "from": 4, "waitMs": 0}),
            )
            .await
            .unwrap();
        assert_eq!(after["exited"]["reason"], "closed");
    });
}

#[test]
fn bad_terminal_parameters_are_refused() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        for params in [
            json!({"projectId": w.project, "cols": 5, "rows": 24}),
            json!({"projectId": w.project, "cols": 80, "rows": 1}),
            json!({"projectId": w.project, "cols": 501, "rows": 24}),
            json!({"projectId": w.project, "rows": 24}),
            json!({"cols": 80, "rows": 24}),
            json!({"projectId": w.project, "cols": 80, "rows": 24, "subdir": 5}),
        ] {
            assert!(
                w.call("terminal.open", params.clone()).await.is_err(),
                "{params}"
            );
        }
        let id = w.open().await;
        for params in [
            json!({"terminalId": id, "data": "!!!not base64"}),
            json!({"terminalId": id}),
            json!({"terminalId": id, "data": "x".repeat(terminal::MAX_INPUT_BYTES * 2 + 4)}),
        ] {
            assert!(w.call("terminal.write", params).await.is_err());
        }
        assert!(w
            .call(
                "terminal.resize",
                json!({"terminalId": id, "cols": 0, "rows": 0})
            )
            .await
            .is_err());
    });
}

#[test]
fn the_folder_must_be_a_project_or_inside_one() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        // A folder inside the project is fine, and shows in the label.
        let id = w.open_with(json!({"subdir": "src"})).await.unwrap();
        assert!(!id.is_empty());
        let cwd = w.spawner.cwds.lock().unwrap()[0].clone();
        assert!(cwd.ends_with("proj/src"), "{cwd:?}");
        for subdir in ["../x", "/etc", "src/../../outside", "..", "missing"] {
            let failure = w.open_with(json!({"subdir": subdir})).await.unwrap_err();
            assert!(
                failure.error.contains("folder"),
                "{subdir}: {}",
                failure.error
            );
        }
        // A project the desktop never published is not found.
        let unknown = w
            .call(
                "terminal.open",
                json!({"projectId": "%2Fetc", "cols": 80, "rows": 24}),
            )
            .await
            .unwrap_err();
        assert_eq!(code_of(&unknown), "not_found");
    });
}

#[cfg(unix)]
#[test]
fn a_symlink_cannot_lead_out_of_the_project() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        let outside = w.dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, w.dir.path().join("proj/src/escape")).unwrap();
        let failure = w
            .open_with(json!({"subdir": "src/escape"}))
            .await
            .unwrap_err();
        assert!(
            failure.error.contains("inside the project"),
            "{}",
            failure.error
        );
    });
}

#[test]
fn a_project_the_device_does_not_share_is_refused() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.host
            .set_device_access(
                &w.device,
                DeviceKind::Web,
                Permission::Control,
                Some(vec!["another-project".into()]),
                None,
            )
            .unwrap();
        let failure = w
            .call(
                "terminal.open",
                json!({"projectId": w.project, "cols": 80, "rows": 24}),
            )
            .await
            .unwrap_err();
        assert_eq!(code_of(&failure), "not_selected");
    });
}

#[test]
fn turning_the_toggle_off_kills_the_shell_and_refuses_the_next_call() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        let id = w.open().await;
        w.host.web_set_terminal_allowed(&w.device, false).unwrap();
        assert_eq!(w.kills(), [1]);
        let failure = w
            .call("terminal.write", json!({"terminalId": id, "data": "YQ=="}))
            .await
            .unwrap_err();
        assert_eq!(code_of(&failure), "disabled");
        assert!(w.host.terminal_status().open.is_empty());
    });
}

#[test]
fn the_toggle_is_saved_on_the_desktop_and_a_new_host_starts_from_the_saved_file_only() {
    run(async {
        let w = world(Permission::Control);
        assert!(!policy::terminal_allowed(&w.device), "default off");
        w.allow();
        let saved = policy::load_settings(&w.dir.path().join(policy::SETTINGS_FILE));
        assert!(saved.terminal_devices.contains(&w.device));
        w.host.web_set_terminal_allowed(&w.device, false).unwrap();
        let saved = policy::load_settings(&w.dir.path().join(policy::SETTINGS_FILE));
        assert!(!saved.terminal_devices.contains(&w.device));
    });
}

#[test]
fn revoking_a_device_kills_its_shell_and_forgets_its_toggle() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        w.host.web_revoke(&w.device).unwrap();
        assert_eq!(w.kills(), [1]);
        assert!(!policy::terminal_allowed(&w.device));
        assert!(w.host.terminal_status().open.is_empty());
    });
}

#[test]
fn a_device_that_leaves_loses_its_shell() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        w.call("device.leave", json!({})).await.unwrap();
        assert_eq!(w.kills(), [1]);
        assert!(!policy::terminal_allowed(&w.device));
    });
}

#[test]
fn a_grant_the_server_revokes_kills_the_shell() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        let generation = w.host.web_link().generation();
        w.host
            .apply_server_grants(&[w.grants("revoked", Permission::Control)], generation);
        assert_eq!(w.kills(), [1]);
        assert!(w.host.terminal_status().open.is_empty());
    });
}

#[test]
fn a_grant_the_server_lowers_to_read_only_kills_the_shell() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        let generation = w.host.web_link().generation();
        // The server asks for less than the row has: the desktop applies it.
        let mut lowered = w.grants("confirmed", Permission::Control);
        lowered.requested = Some(Permission::Read);
        w.host.apply_server_grants(&[lowered], generation);
        assert_eq!(w.kills(), [1]);
    });
}

#[test]
fn nothing_in_the_grant_sync_can_switch_the_toggle() {
    run(async {
        let w = world(Permission::Control);
        let generation = w.host.web_link().generation();
        // A server that lists the device as confirmed with Control, asks for
        // more, and adds a pending device: the toggle stays exactly as it was.
        let mut asking = w.grants("confirmed", Permission::Control);
        asking.requested = Some(Permission::Control);
        let stranger = ServerGrant {
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: "Stranger".into(),
            public_key: URL_SAFE_NO_PAD.encode([5u8; 32]),
            fingerprint: noise::fingerprint(&[5u8; 32]),
            state: "pending".into(),
            requested: Some(Permission::Control),
            permission: None,
        };
        w.host
            .apply_server_grants(&[asking.clone(), stranger.clone()], generation);
        assert!(!policy::terminal_allowed(&w.device), "off stays off");
        assert!(!policy::terminal_allowed("Stranger"));
        w.allow();
        w.host.apply_server_grants(&[asking, stranger], generation);
        assert!(policy::terminal_allowed(&w.device), "on stays on");
        assert!(policy::terminal_allowed(&w.device));
    });
}

#[test]
fn the_desktop_lowering_a_device_or_narrowing_its_projects_kills_the_shell() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        // Still shared: nothing happens.
        w.host
            .set_device_access(
                &w.device,
                DeviceKind::Web,
                Permission::Control,
                Some(vec![w.project.clone()]),
                None,
            )
            .unwrap();
        assert_eq!(w.kills(), [0]);
        w.host
            .set_device_access(
                &w.device,
                DeviceKind::Web,
                Permission::Control,
                Some(vec!["other".into()]),
                None,
            )
            .unwrap();
        assert_eq!(w.kills(), [1]);
        w.open_after_widen().await;
        w.host
            .set_device_access(&w.device, DeviceKind::Web, Permission::Read, None, None)
            .unwrap();
        assert_eq!(w.kills(), [1, 1]);
    });
}

#[test]
fn lowering_below_control_clears_the_toggle_so_raising_back_does_not_restore_it() {
    run(async {
        // The desktop lowers the device.
        let w = world(Permission::Control);
        w.allow();
        w.host
            .set_device_access(&w.device, DeviceKind::Web, Permission::Read, None, None)
            .unwrap();
        assert!(!policy::terminal_allowed(&w.device), "cleared at once");
        let saved = policy::load_settings(&w.dir.path().join(policy::SETTINGS_FILE));
        assert!(!saved.terminal_devices.contains(&w.device), "and saved");
        w.host
            .set_device_access(&w.device, DeviceKind::Web, Permission::Control, None, None)
            .unwrap();
        let refused = w
            .call(
                "terminal.open",
                json!({"projectId": w.project, "cols": 80, "rows": 24}),
            )
            .await;
        assert!(refused.is_err(), "no terminal until the toggle is on again");
        w.allow();
        assert!(w.open_with(json!({})).await.is_ok());

        // The server lowers it.
        let w = world(Permission::Control);
        w.allow();
        let generation = w.host.web_link().generation();
        let mut lowered = w.grants("confirmed", Permission::Control);
        lowered.requested = Some(Permission::Read);
        w.host.apply_server_grants(&[lowered], generation);
        assert!(!policy::terminal_allowed(&w.device));
        let saved = policy::load_settings(&w.dir.path().join(policy::SETTINGS_FILE));
        assert!(!saved.terminal_devices.contains(&w.device));
    });
}

impl World {
    async fn open_after_widen(&self) {
        self.host
            .set_device_access(
                &self.device,
                DeviceKind::Web,
                Permission::Control,
                None,
                None,
            )
            .unwrap();
        self.open().await;
    }
}

#[test]
fn the_tick_ends_a_shell_whose_device_row_vanished_by_any_other_path() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        w.host
            .inner
            .lock()
            .unwrap()
            .config
            .devices
            .retain(|device| device.id != w.device);
        w.host.terminals().tick();
        assert_eq!(w.kills(), [1]);
    });
}

#[test]
fn the_tick_ends_a_shell_when_the_web_link_is_switched_off() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        w.host.inner.lock().unwrap().config.web_enabled = false;
        w.host.terminals().tick();
        assert_eq!(w.kills(), [1]);
    });
}

#[test]
fn shutting_the_desktop_down_kills_every_shell() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        w.open().await;
        w.open().await;
        w.host.shutdown();
        assert_eq!(w.kills(), [1, 1]);
    });
}

#[test]
fn the_kill_button_ends_the_shell_it_names() {
    run(async {
        let w = world(Permission::Control);
        w.allow();
        let id = w.open().await;
        let shown = w.host.terminal_status().open;
        assert_eq!((shown.len(), shown[0].device_name.as_str()), (1, "Laptop"));
        assert!(w.host.terminal_kill(&shown[0].id));
        assert_eq!(w.kills(), [1]);
        let ended = w
            .call(
                "terminal.read",
                json!({"terminalId": id, "from": 0, "waitMs": 0}),
            )
            .await
            .unwrap();
        assert_eq!(ended["exited"]["reason"], "killed");
        assert_eq!(w.host.terminal_kill_all(), 0);
    });
}

#[test]
fn opening_and_closing_are_audited_with_the_reason_and_reads_are_not() {
    use crate::mobile::audit;
    run(async {
        let log = tempfile::tempdir().unwrap();
        let _turn = audit::test_log(log.path());
        let w = world(Permission::Control);
        w.allow();
        let id = w.open().await;
        w.call(
            "terminal.read",
            json!({"terminalId": id, "from": 0, "waitMs": 0}),
        )
        .await
        .unwrap();
        w.host.terminal_kill(&id);
        let lines = audit::read_recent(log.path(), 100);
        let mine: Vec<_> = lines
            .iter()
            .filter(|line| line["deviceId"] == w.device.as_str())
            .collect();
        let events: Vec<String> = mine
            .iter()
            .map(|line| line["event"].as_str().unwrap_or("call").to_owned())
            .collect();
        for wanted in [
            "terminal_toggle",
            "terminal_challenge",
            "terminal_open",
            "terminal_close",
        ] {
            assert!(
                events.iter().any(|event| event == wanted),
                "{wanted} in {events:?}"
            );
        }
        let closed = mine
            .iter()
            .find(|line| line["event"] == "terminal_close")
            .unwrap();
        assert_eq!(closed["detail"], "killed");
        assert_eq!(closed["transport"], "web");
        // No parameters, no output and no code ever reach the log.
        let text = serde_json::to_string(&lines).unwrap();
        assert!(!text.contains("terminal.read"), "polls are not written");
    });
}
