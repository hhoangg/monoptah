//! UI invariants around the host-owned conversation and the next local draft.
use super::*;
use gpui::TestAppContext;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Mutex;

fn conversation(cx: &mut TestAppContext) -> Entity<Conversation> {
    let store = monocode_core::store::Store::open(Path::new(":memory:")).unwrap();
    let mut summary = store.create("codex", "/test-project").unwrap();
    summary.cwd = "remote://remote-ui-tests/home/me/project".into();
    cx.update(|cx| {
        cx.new(|cx| {
            let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
            view.loaded = true;
            view.remote_state.online = true;
            view.remote_state.capabilities =
                ["sessions.draft", "sessions.plan", "attachments.upload"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
            view.input.update(cx, |input, cx| {
                input.set_text("Keep this next message".into(), cx)
            });
            view
        })
    })
}

#[gpui::test]
fn offline_pending_and_busy_guards_preserve_typed_message(cx: &mut TestAppContext) {
    let view = conversation(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            for (loaded, online, pending, busy, expected) in [
                (
                    false,
                    true,
                    false,
                    false,
                    "Wait for this remote conversation",
                ),
                (true, false, false, false, "machine is offline"),
                (true, true, true, false, "command needs confirmation"),
                (true, true, false, true, "remote turn to finish"),
            ] {
                view.loaded = loaded;
                view.remote_state.online = online;
                view.remote_state.pending = pending;
                view.busy = busy;
                view.send(cx);
                assert!(view.status.contains(expected), "{}", view.status);
                assert_eq!(view.input.read(cx).content, "Keep this next message");
                assert!(view.follow_ups.state.messages.is_empty());
                assert!(view.transcript.read(cx).source_rows().is_empty());
            }
            view.busy = false;
        })
    });
}

#[gpui::test]
fn remote_capability_and_command_guards_keep_composer_and_attachments(cx: &mut TestAppContext) {
    let view = conversation(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.remote_state.capabilities.clear();
            view.attachments = vec![PathBuf::from("/home/me/design.png")];
            assert!(view.remote_send_guard(cx));
            assert!(view.status.contains("attachments"));
            assert_eq!(view.attachments.len(), 1);
            view.attachments.clear();
            view.settings.plan = true;
            assert!(view.remote_send_guard(cx));
            assert!(view.status.contains("Plan mode"));
            view.settings.plan = false;
            view.follow_ups.draft_selected = true;
            assert!(view.remote_send_guard(cx));
            assert!(view.status.contains("save drafts"));
            view.follow_ups.draft_selected = false;
            for command in [
                "/operator delegate this",
                "/orchestrate a task",
                "/add-to-folder name",
            ] {
                view.input
                    .update(cx, |input, cx| input.set_text(command.into(), cx));
                assert!(view.remote_send_guard(cx));
                assert_eq!(view.input.read(cx).content.as_ref(), command);
            }
        })
    });
}

#[gpui::test]
fn restoring_host_saved_draft_preserves_the_next_typed_message(cx: &mut TestAppContext) {
    let view = conversation(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.install_remote_draft(&json!({"session": {"blocks": [
            {"id": "saved-remote-draft", "role": "user", "draft": true, "text": "Saved on the host"}
        ]}}), cx);
            let draft = view.follow_ups.state.draft.as_ref().unwrap();
            assert_eq!(draft.id, "saved-remote-draft");
            assert_eq!(draft.text, "Saved on the host");
            assert_eq!(view.input.read(cx).content, "Keep this next message");
            view.install_remote_draft(&json!({"session": {"blocks": []}}), cx);
            assert!(view.follow_ups.state.draft.is_none());
            assert_eq!(view.input.read(cx).content, "Keep this next message");
        })
    });
}

#[gpui::test]
fn dropping_remote_conversation_does_not_cancel_its_host_turn(cx: &mut TestAppContext) {
    let view = conversation(cx);
    let cancel = cx.update(|cx| view.read(cx).cancel.clone());
    cx.update(|_| drop(view));
    cx.run_until_parked();
    assert!(!cancel.load(Ordering::Relaxed));

    let local = conversation(cx);
    let local_cancel = cx.update(|cx| {
        local.update(cx, |view, _| {
            view.summary.cwd = "/local/project".into();
            view.cancel.clone()
        })
    });
    cx.update(|_| drop(local));
    cx.run_until_parked();
    assert!(local_cancel.load(Ordering::Relaxed));
}

struct HostFixture {
    client: Arc<monocode_core::remote::Connections>,
    store: Database,
    summary: monocode_core::store::Session,
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    stop: Arc<AtomicBool>,
    server: Option<std::thread::JoinHandle<()>>,
    directory: PathBuf,
    /// A running host turn goes idle after this many syncs.
    idle_after: Arc<std::sync::atomic::AtomicUsize>,
    /// From this sync on (0: never) the host reports a title it chose itself.
    rename_at: Arc<std::sync::atomic::AtomicUsize>,
}
impl HostFixture {
    fn new(running: bool, saved_draft: bool) -> Self {
        use std::io::{BufRead, Read, Write};
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let environment = format!("remote-ui-{}-{nonce}", std::process::id());
        let directory = std::env::temp_dir().join(&environment);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let descriptor = json!({"protocolVersion": 1, "environmentId": environment, "name": "UI test host", "providers": ["codex"], "capabilities": ["sessions.draft", "sessions.plan", "attachments.upload", "sessions.sync"]});
        let snapshot = json!({"projectId": "project", "revision": 1, "status": if running {"running"} else {"idle"}, "runId": if running {json!("host-run")} else {serde_json::Value::Null}, "updatedAt": 42,
            "session": {"id": "host-session", "title": "Conversation on host", "harness": "codex", "model": "codex:gpt-test", "runtimeMode": "supervised", "modelSettings": {}, "blocks": if saved_draft { json!([{ "id": "host-saved-draft", "role": "user", "draft": true, "text": "Saved host draft" }]) } else {json!([])} }
        });
        let initial = snapshot.clone();
        let requests = Arc::new(Mutex::new(vec![]));
        let received = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let idle_after = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let idles = idle_after.clone();
        let rename_at = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let renames = rename_at.clone();
        let server = std::thread::spawn(move || {
            let mut snapshot = snapshot;
            let mut syncs = 0;
            while !done.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("host accept: {error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                received.lock().unwrap().push(request.clone());
                let result = match request["method"].as_str().unwrap() {
                    "environment.describe" => descriptor.clone(),
                    "sessions.sync" => {
                        syncs += 1;
                        let renamed = renames.load(Ordering::Relaxed);
                        if renamed > 0 && syncs >= renamed {
                            snapshot["session"]["title"] = json!("Named by the host");
                        }
                        if running && syncs > idles.load(Ordering::Relaxed) {
                            snapshot["status"] = json!("idle");
                            snapshot["runId"] = serde_json::Value::Null;
                            snapshot["revision"] = json!(2);
                        }
                        json!({"kind": "snapshot", "value": snapshot})
                    }
                    "commands.dispatch" => {
                        let command = &request["params"];
                        json!({"commandId": command["commandId"], "sessionId": "host-session", "revision": 2})
                    }
                    _ => json!({}),
                };
                let body = json!({"result": result}).to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let client = Arc::new(monocode_core::remote::Connections::new(&directory));
        client.connect(String::new(), url, "a".repeat(43)).unwrap();
        let store = monocode_core::store::Store::open(Path::new(":memory:")).unwrap();
        let project = store
            .remember_remote_project(
                &environment,
                &json!({"id": "project", "cwd": "/home/me/project", "name": "project"}),
            )
            .unwrap();
        let summary = store.register_remote_summary(&project, &initial).unwrap();
        Self {
            client,
            store: Arc::new(Mutex::new(store)),
            summary,
            requests,
            stop,
            server: Some(server),
            directory,
            idle_after,
            rename_at,
        }
    }
    fn conversation(&self, cx: &mut TestAppContext) -> Entity<Conversation> {
        cx.update(|cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(self.summary.clone(), self.store.clone(), cx);
                view.remote_state.client = Some(self.client.clone());
                view.input.update(cx, |input, cx| {
                    input.set_text("Keep this next message".into(), cx)
                });
                view.load_remote(cx);
                view
            })
        })
    }
    fn wait(&self, view: &Entity<Conversation>, cx: &mut TestAppContext, settled: bool) {
        for _ in 0..1000 {
            cx.run_until_parked();
            if cx.update(|cx| {
                let view = view.read(cx);
                view.loaded && (!settled || (!view.busy && !view.remote_state.refreshing))
            }) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        cx.update(|cx| {
            panic!(
                "remote conversation did not settle: {}",
                view.read(cx).status
            )
        });
    }
}
impl Drop for HostFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let result = server.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[gpui::test]
fn snapshot_load_restores_host_draft_without_replacing_unsent_composer(cx: &mut TestAppContext) {
    let host = HostFixture::new(false, true);
    let view = host.conversation(cx);
    host.wait(&view, cx, true);
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.remote_state.online);
        assert_eq!(view.summary.title, "Conversation on host");
        assert_eq!(
            view.follow_ups.state.draft.as_ref().unwrap().text,
            "Saved host draft"
        );
        assert_eq!(view.input.read(cx).content, "Keep this next message");
    });
    assert!(
        !host
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r["method"] == "commands.dispatch")
    );
}

/// The GPUI test executor runs the remote observer inline, so the host itself
/// scripts the turn: it renames the conversation on sync 3 and goes idle on 4.
#[gpui::test]
fn a_host_title_change_reaches_the_busy_view_without_a_rename_request(cx: &mut TestAppContext) {
    let host = HostFixture::new(true, false);
    host.idle_after.store(3, Ordering::Relaxed);
    host.rename_at.store(3, Ordering::Relaxed);
    let view = host.conversation(cx);
    let seen = Arc::new(Mutex::new(Vec::<(bool, String)>::new()));
    let log = seen.clone();
    let _changes = cx.update(|cx| {
        cx.subscribe(&view, move |view, _: &SessionChanged, cx| {
            let view = view.read(cx);
            log.lock()
                .unwrap()
                .push((view.busy, view.summary.title.to_string()));
        })
    });
    host.wait(&view, cx, true);
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.summary.title, "Named by the host");
        assert_eq!(view.display_title(), "Named by the host");
    });
    let seen = seen.lock().unwrap();
    assert!(
        seen.contains(&(true, "Named by the host".to_owned())),
        "chrome must be told while the turn is still running: {seen:?}"
    );
    let requests = host.requests.lock().unwrap();
    assert!(
        !requests.iter().any(|r| matches!(
            r["method"].as_str(),
            Some("sessions.update" | "commands.dispatch")
        )),
        "a title from the host must never be written back: {requests:?}"
    );
    drop(requests);
    assert_eq!(
        host.store
            .lock()
            .unwrap()
            .remote_session(host.summary.id)
            .unwrap()
            .unwrap()
            .summary
            .title,
        "Named by the host"
    );
}

#[gpui::test]
fn opening_running_host_reattaches_without_dispatching_another_turn(cx: &mut TestAppContext) {
    let host = HostFixture::new(true, false);
    let view = host.conversation(cx);
    host.wait(&view, cx, true);
    cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.remote_state.online);
        assert_eq!(view.input.read(cx).content, "Keep this next message");
        assert_eq!(
            view.remote_state.snapshot.as_ref().unwrap()["status"],
            "idle"
        );
        assert!(!view.cancel.load(Ordering::Relaxed));
    });
    let requests = host.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .filter(|r| r["method"] == "sessions.sync")
            .count()
            >= 2
    );
    assert!(!requests.iter().any(|r| r["method"] == "commands.dispatch"));
}

/// A session started through the app API (a cloud session) keeps its first prompt as a local
/// draft; a remote conversation never reads local follow-ups, so the prompt used to be dropped
/// and the session sat idle. It must be dispatched to the host as an ordinary prompt.
#[gpui::test]
fn a_remote_session_started_through_the_app_dispatches_its_first_prompt(cx: &mut TestAppContext) {
    let host = HostFixture::new(false, false);
    {
        let store = host.store.lock().unwrap();
        store
            .prepare_app_prompt(host.summary.id, "start-1", "Do the first thing")
            .unwrap();
    }
    let view = host.conversation(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let settings = monocode_core::agent::Settings {
                model: Some("codex:gpt-test".into()),
                ..Default::default()
            };
            view.install_app_launch(settings, true, cx)
        })
    });
    let dispatched = || {
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "commands.dispatch")
            .cloned()
            .collect::<Vec<_>>()
    };
    for _ in 0..10000 {
        cx.run_until_parked();
        if dispatched().iter().any(|r| r["params"]["type"] == "send") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    host.wait(&view, cx, true);
    let sent = dispatched();
    assert!(
        sent.iter()
            .any(|r| r["params"]["type"] == "send" && r["params"]["text"] == "Do the first thing"),
        "{sent:?}"
    );
    // The local copy is consumed, so the session does not also report a stale draft.
    assert!(
        host.store
            .lock()
            .unwrap()
            .follow_ups(host.summary.id)
            .unwrap()
            .draft
            .is_none()
    );
}

/// The machine being offline when the API launch is installed must not strand the first prompt in
/// the invisible local draft: it is sent once the next load succeeds.
#[gpui::test]
fn an_offline_app_launch_sends_its_first_prompt_after_the_next_successful_load(
    cx: &mut TestAppContext,
) {
    let host = HostFixture::new(false, false);
    host.store
        .lock()
        .unwrap()
        .prepare_app_prompt(host.summary.id, "start-2", "Do it later")
        .unwrap();
    let view = host.conversation(cx);
    host.wait(&view, cx, true);
    let settings = monocode_core::agent::Settings {
        model: Some("codex:gpt-test".into()),
        ..Default::default()
    };
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.remote_state.online = false;
            view.install_app_launch(settings, true, cx)
        })
    });
    let dispatches = || {
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "commands.dispatch" && r["params"]["type"] == "send")
            .count()
    };
    cx.run_until_parked();
    assert_eq!(dispatches(), 0);
    assert!(
        host.store
            .lock()
            .unwrap()
            .follow_ups(host.summary.id)
            .unwrap()
            .draft
            .is_some(),
        "the draft is kept until it is sent"
    );
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_remote_snapshot(cx)));
    for _ in 0..10000 {
        cx.run_until_parked();
        if dispatches() > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    host.wait(&view, cx, true);
    assert_eq!(dispatches(), 1, "{:?}", host.requests.lock().unwrap());
    // A later refresh drops the local copy of the sent prompt.
    cx.update(|cx| view.update(cx, |view, cx| view.refresh_remote_snapshot(cx)));
    host.wait(&view, cx, true);
    assert!(
        host.store
            .lock()
            .unwrap()
            .follow_ups(host.summary.id)
            .unwrap()
            .draft
            .is_none()
    );
}

/// An API draft on a remote session lands in the composer, where the user can see it.
#[gpui::test]
fn an_api_draft_on_a_remote_session_is_shown_in_the_composer(cx: &mut TestAppContext) {
    let host = HostFixture::new(false, false);
    host.store
        .lock()
        .unwrap()
        .prepare_app_prompt(host.summary.id, "start-3", "Review this")
        .unwrap();
    let view = host.conversation(cx);
    host.wait(&view, cx, true);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            view.install_app_launch(monocode_core::agent::Settings::default(), false, cx);
            assert_eq!(view.input.read(cx).content, "Review this");
        })
    });
    assert_eq!(
        host.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r["method"] == "commands.dispatch")
            .count(),
        0
    );
}

/// After an app restart a stored API prompt that never went out is shown in the composer with a
/// notice, and is not sent on its own.
#[gpui::test]
fn a_stored_app_prompt_is_recovered_into_the_composer_after_a_restart(cx: &mut TestAppContext) {
    let host = HostFixture::new(false, false);
    host.store
        .lock()
        .unwrap()
        .prepare_app_prompt(host.summary.id, "start-4", "Never sent")
        .unwrap();
    let view = host.conversation(cx);
    host.wait(&view, cx, true);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            // The fixture seeds the composer; a fresh app has it empty.
            assert_eq!(view.input.read(cx).content, "Keep this next message");
        })
    });
    let fresh = {
        host.store
            .lock()
            .unwrap()
            .prepare_app_prompt(host.summary.id, "start-5", "Never sent")
            .unwrap();
        cx.update(|cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(host.summary.clone(), host.store.clone(), cx);
                view.remote_state.client = Some(host.client.clone());
                view.load_remote(cx);
                view
            })
        })
    };
    host.wait(&fresh, cx, true);
    cx.update(|cx| {
        let view = fresh.read(cx);
        assert_eq!(view.input.read(cx).content, "Never sent");
        assert!(view.status.contains("not sent yet"), "{}", view.status);
    });
    assert!(
        host.store
            .lock()
            .unwrap()
            .follow_ups(host.summary.id)
            .unwrap()
            .draft
            .is_none()
    );
    assert!(
        !host
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r["method"] == "commands.dispatch")
    );
}

/// A remote view never queues a prompt into local follow-ups while stopping: the host would never
/// get it.
#[gpui::test]
fn a_remote_view_does_not_queue_into_local_follow_ups_while_stopping(cx: &mut TestAppContext) {
    let view = conversation(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.stopping = true;
            view.launch_prompt("later".into(), cx);
            assert!(view.follow_ups.state.messages.is_empty());
            assert!(view.status.contains("Wait"), "{}", view.status);
        })
    });
}

/// A freshly launched remote session has no stored messages until its first prompt is journaled;
/// loading it must not turn off the in-memory flag that keeps it in the sidebar.
#[gpui::test]
fn loading_a_blank_launched_remote_session_keeps_it_listable(cx: &mut TestAppContext) {
    let host = HostFixture::new(false, false);
    let blank = {
        let store = host.store.lock().unwrap();
        let project = store
            .remote_projects()
            .unwrap()
            .into_values()
            .next()
            .unwrap();
        store.create("codex", &project.key).unwrap()
    };
    let mut summary = blank.clone();
    summary.has_messages = true;
    let view = cx.update(|cx| {
        cx.new(|cx| {
            let mut view = Conversation::new(summary, host.store.clone(), cx);
            view.remote_state.client = Some(host.client.clone());
            view.load_remote(cx);
            view
        })
    });
    host.wait(&view, cx, true);
    cx.update(|cx| assert!(view.read(cx).summary.has_messages));
}

#[gpui::test]
fn explicit_retry_reuses_pending_command_and_preserves_new_composer_and_attachment(
    cx: &mut TestAppContext,
) {
    let host = HostFixture::new(false, false);
    let view = host.conversation(cx);
    host.wait(&view, cx, true);
    {
        let store = host.store.lock().unwrap();
        let mut remote = store.remote_session(host.summary.id).unwrap().unwrap();
        remote.outbox.push(json!({"type": "send", "commandId": "original-accepted-command", "sessionId": "host-session", "text": "Earlier message"}));
        store.save_remote_session(&remote).unwrap();
    }
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.remote_state.pending = true;
            view.attachments = vec![PathBuf::from("/unsent/new-attachment.png")];
            view.retry_remote(cx);
            assert_eq!(view.input.read(cx).content, "Keep this next message");
            assert_eq!(
                view.attachments,
                [PathBuf::from("/unsent/new-attachment.png")]
            );
        })
    });
    host.wait(&view, cx, true);
    let requests = host.requests.lock().unwrap();
    let dispatched: Vec<_> = requests
        .iter()
        .filter(|r| r["method"] == "commands.dispatch")
        .collect();
    assert_eq!(dispatched.len(), 1);
    assert_eq!(
        dispatched[0]["params"]["commandId"],
        "original-accepted-command"
    );
    assert_eq!(dispatched[0]["params"]["text"], "Earlier message");
    assert!(!requests.iter().any(|r| r["method"] == "attachments.upload"));
    drop(requests);
    cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.input.read(cx).content, "Keep this next message");
        assert_eq!(view.attachments.len(), 1);
    });
}

#[gpui::test]
fn rejected_message_restore_preserves_new_draft_then_restores_original_text_and_files(
    cx: &mut TestAppContext,
) {
    let view = conversation(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
        view.remote_state.rejected = Some(json!({
            "error": "Host rejected request: permission denied",
            "commands": [
                {"type": "configure", "commandId": "configure-original"},
                {"type": "send", "commandId": "send-original", "text": "Original rejected message"}
            ],
            "uploads": [{"commandId": "send-original", "path": "/original/rejected.png"}]
        }));
        view.attachments = vec![PathBuf::from("/new/unsent.png")];
        view.restore_remote_message(cx);
        assert_eq!(view.input.read(cx).content, "Keep this next message");
        assert_eq!(view.attachments, [PathBuf::from("/new/unsent.png")]);
        assert!(view.status.contains("Clear the composer"));

        view.input.update(cx, |input, cx| input.set_text(String::new(), cx));
        view.attachments.clear();
        view.restore_remote_message(cx);
        assert_eq!(view.input.read(cx).content, "Original rejected message");
        assert_eq!(view.attachments, [PathBuf::from("/original/rejected.png")]);
    })
    });
}

#[gpui::test]
fn ambiguous_legacy_requests_block_send_retry_and_restore_without_consuming_draft(
    cx: &mut TestAppContext,
) {
    let view = conversation(cx);
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.remote_state.rejected = Some(json!({
                "legacyRecovery": true,
                "error": "Open the original desktop to recover older pending requests.",
                "commands": [{"type": "send", "text": "Ambiguous original message"}]
            }));
            view.remote_state.pending = true;
            view.send(cx);
            view.retry_remote(cx);
            view.restore_remote_message(cx);
            assert_eq!(
                view.status,
                "Open the original desktop to recover older pending requests."
            );
            assert_eq!(view.input.read(cx).content, "Keep this next message");
            assert!(!view.busy);
            assert!(view.settings.remote_action.is_none());
            assert!(view.transcript.read(cx).source_rows().is_empty());
        })
    });
}

/// After an app restart a cloud session has no live tunnel and its history never loads in the
/// background; a send (and the Reconnect button) must wake the codespace off the UI thread.
#[gpui::test]
fn sending_to_an_unloaded_cloud_session_reconnects_its_codespace(cx: &mut TestAppContext) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    use monocode_core::cloud_sessions::{CodespaceChoice, Target};
    let dir = github_project();
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let (_gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let api = monocode_core::mono::cloud_api::CloudApi::shared(&store).unwrap();
    let binding = api
        .prepare(
            dir.0.to_str().unwrap(),
            Target {
                codespace: CodespaceChoice::Named("octo-run".into()),
                branch: None,
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap();
    let summary = api.create_session(&binding, "claude").unwrap();
    host.0.lock().unwrap().clear();
    let view = cx.update(|cx| {
        cx.new(|cx| {
            let mut view = Conversation::new(summary, store.clone(), cx);
            view.loaded = false;
            view.remote_state.online = false;
            // The reload that follows a wake is not under test (it would open real connections).
            view.remote_state.refreshing = true;
            view.input
                .update(cx, |input, cx| input.set_text("Reply with PONG".into(), cx));
            view
        })
    });
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.send(cx);
            assert!(view.status.contains("Reconnecting"), "{}", view.status);
            assert!(view.remote_state.waking);
            // A second send while waking does not start another wake.
            view.send(cx);
            assert_eq!(view.input.read(cx).content, "Reply with PONG");
        })
    });
    cx.run_until_parked();
    let calls = host.0.lock().unwrap().clone();
    assert_eq!(
        calls.iter().filter(|c| *c == "ensure m-octo-run").count(),
        1,
        "{calls:?}"
    );
    cx.update(|cx| view.update(cx, |view, _| assert!(!view.remote_state.waking)));
}

/// A Docker sandbox session is reconnected only by a user action in its own view, through
/// `CloudApi::reconnect_sandbox`; the codespace wake (`ensure`) is never used for it, and a
/// Mono or app API send (`wake_for_send`) wakes nothing.
#[gpui::test]
fn a_sandbox_session_reconnects_through_the_sandbox_path_and_background_sends_wake_nothing(
    cx: &mut TestAppContext,
) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    use monocode_core::cloud_sessions::CloudRef;
    let dir = github_project();
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let (gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let summary = {
        let guard = store.lock().unwrap();
        let project = guard
            .remember_cloud_ref_project(
                "env-box",
                &serde_json::json!({"id":"project-1","cwd":"/workspaces/widgets"}),
                dir.0.to_str().unwrap(),
                &CloudRef::sandbox("monoptah-fix-1234"),
            )
            .unwrap();
        guard.create("claude", &project.key).unwrap()
    };
    let id = summary.id;
    host.0.lock().unwrap().clear();
    gh.calls.lock().unwrap().clear();
    let view = cx.update(|cx| {
        cx.new(|cx| {
            let mut view = Conversation::new(summary, store.clone(), cx);
            assert!(view.is_cloud_session());
            assert_eq!(view.cloud_noun(), Some("sandbox"));
            view.loaded = false;
            view.remote_state.online = false;
            view.remote_state.refreshing = true;
            view
        })
    });
    // A Mono or app API send: no wake of any kind.
    let woke = cx.update(|cx| {
        let store = store.clone();
        let view = view.clone();
        cx.spawn(async move |cx| crate::cloud_wake::wake_for_send(store, id, view, cx).await)
    });
    cx.run_until_parked();
    assert!(!cx.foreground_executor().block_test(woke));
    // The user's Reconnect: sandbox wording, and the sandbox path (not configured in this
    // fake service, so it says so) instead of the codespace wake.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            assert!(view.reconnect_cloud(true, cx));
            assert!(
                view.status.contains("Reconnecting the sandbox"),
                "{}",
                view.status
            );
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        view.update(cx, |view, _| {
            assert!(!view.remote_state.waking);
            assert!(view.status.contains("sandbox"), "{}", view.status);
        })
    });
    assert!(
        host.0.lock().unwrap().is_empty(),
        "{:?}",
        host.0.lock().unwrap()
    );
    assert!(gh.calls.lock().unwrap().is_empty());
}

/// A send that is not the user's own hand (Mono, workflow, app API) reaches the same guard and
/// must not resume a sandbox; the composer's own submit does.
#[gpui::test]
fn only_a_user_submit_reconnects_a_stopped_sandbox_view(cx: &mut TestAppContext) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    use monocode_core::cloud_sessions::CloudRef;
    let dir = github_project();
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let (gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let summary = {
        let guard = store.lock().unwrap();
        let project = guard
            .remember_cloud_ref_project(
                "env-box",
                &serde_json::json!({"id":"project-1","cwd":"/workspaces/widgets"}),
                dir.0.to_str().unwrap(),
                &CloudRef::sandbox("monoptah-fix-1234"),
            )
            .unwrap();
        guard.create("claude", &project.key).unwrap()
    };
    host.0.lock().unwrap().clear();
    gh.calls.lock().unwrap().clear();
    let view = cx.update(|cx| {
        cx.new(|cx| {
            let mut view = Conversation::new(summary, store.clone(), cx);
            view.loaded = true;
            view.remote_state.online = false;
            view.remote_state.refreshing = true;
            view.input
                .update(cx, |input, cx| input.set_text("hello".into(), cx));
            view
        })
    });
    // Mono external send, workflow send and app API send all end in send / draft / app prompt.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.send(cx);
            assert_eq!(view.status, super::remote::SANDBOX_STOPPED);
            assert!(!view.remote_state.waking);
            view.status.clear();
            view.remote_save_draft(cx);
            assert_eq!(view.status, super::remote::SANDBOX_STOPPED);
            assert!(!view.remote_state.waking);
            view.status.clear();
            view.send_remote_app_prompt(cx);
            assert_eq!(view.status, super::remote::SANDBOX_STOPPED);
            assert!(!view.remote_state.waking);
            assert!(!view.reconnect_cloud(false, cx));
            assert!(!view.remote_state.waking);
        })
    });
    cx.run_until_parked();
    assert!(host.0.lock().unwrap().is_empty());
    assert!(gh.calls.lock().unwrap().is_empty());
    // The user's own submit reconnects.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            let input = view.input.clone();
            input.update(cx, |_, cx| cx.emit(crate::input::Submitted));
        })
    });
    cx.update(|cx| {
        view.update(cx, |view, _| {
            assert!(view.remote_state.waking, "{}", view.status);
            assert!(
                view.status.contains("Reconnecting the sandbox"),
                "{}",
                view.status
            );
        })
    });
}

// ------------------------------------------------- sandbox reconnect banner

const RECREATED: &str =
    "The sandbox container was recreated, so this session's history is gone. Start a new session.";

/// A window with the conversation of a stored cloud session of `cloud`.
fn cloud_conversation(
    cx: &mut TestAppContext,
    cloud: monocode_core::cloud_sessions::CloudRef,
) -> gpui::WindowHandle<Conversation> {
    cx.update(|cx| cx.set_reduce_motion(true));
    let store = monocode_core::store::Store::open(Path::new(":memory:")).unwrap();
    let project = store
        .remember_cloud_ref_project(
            "env-box",
            &serde_json::json!({"id":"project-1","cwd":"/workspaces/widgets"}),
            "/work/widgets",
            &cloud,
        )
        .unwrap();
    let mut summary = store.create("claude", &project.key).unwrap();
    summary.has_messages = true;
    cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    })
}

#[gpui::test]
fn a_recreated_or_deleted_sandbox_offers_archive_and_new_session_not_reconnect(
    cx: &mut TestAppContext,
) {
    use monocode_core::cloud_sessions::CloudRef;
    for error in [RECREATED, "Sandbox was deleted. Start a new session."] {
        let window = cloud_conversation(cx, CloudRef::sandbox("monoptah-fix-1234"));
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = seen.clone();
        let view = window.root(cx).unwrap();
        let _subscription = cx.update(|cx| {
            cx.subscribe(
                &view,
                move |_, event: &crate::cloud_start_view::CloudStartAction, _| {
                    log.borrow_mut().push(event.action)
                },
            )
        });
        window
            .update(cx, |view, _, cx| {
                view.remote_state.online = false;
                view.remote_state.error = Some(error.into());
                cx.notify();
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("remote-reconnect").is_none(), "{error}");
        assert!(visual.debug_bounds("remote-retry").is_none(), "{error}");
        for (id, action) in [
            (
                "remote-archive-session",
                crate::cloud_start_view::StartAction::ArchiveSession,
            ),
            (
                "remote-new-session",
                crate::cloud_start_view::StartAction::NewSession,
            ),
        ] {
            let bounds = visual.debug_bounds(id).unwrap_or_else(|| panic!("{id}"));
            visual.simulate_click(bounds.center(), Default::default());
            assert_eq!(seen.borrow().last(), Some(&action), "{id}");
        }
    }
}

#[gpui::test]
fn another_sandbox_error_and_every_codespace_error_keep_reconnect_and_retry(
    cx: &mut TestAppContext,
) {
    use monocode_core::cloud_sessions::CloudRef;
    for (cloud, error) in [
        (
            CloudRef::sandbox("monoptah-fix-1234"),
            "Could not reach sbx",
        ),
        (CloudRef::codespace("octo-abc"), RECREATED),
    ] {
        let window = cloud_conversation(cx, cloud);
        window
            .update(cx, |view, _, cx| {
                view.remote_state.online = false;
                view.remote_state.error = Some(error.into());
                cx.notify();
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("remote-reconnect").is_some(), "{error}");
        assert!(visual.debug_bounds("remote-retry").is_some(), "{error}");
        assert!(visual.debug_bounds("remote-archive-session").is_none());
    }
}

#[gpui::test]
fn the_first_sandbox_reconnect_attempt_shows_progress_and_cancel(cx: &mut TestAppContext) {
    use monocode_core::cloud_sessions::CloudRef;
    // A sandbox: the banner appears while the first attempt runs, with Cancel only.
    let window = cloud_conversation(cx, CloudRef::sandbox("monoptah-fix-1234"));
    window
        .update(cx, |view, _, cx| {
            view.remote_state.online = false;
            view.remote_state.waking = true;
            view.remote_state.wake_cancel =
                Some(Arc::new(std::sync::atomic::AtomicBool::new(false)));
            view.status = "Starting the container".into();
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("remote-reconnect-cancel").is_some());
    assert!(visual.debug_bounds("remote-retry").is_none());
    window
        .update(visual, |view, _, cx| {
            assert!(view.sandbox_reconnecting());
            view.cancel_cloud_reconnect(cx);
            let cancelled = view
                .remote_state
                .wake_cancel
                .as_ref()
                .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed));
            assert!(cancelled);
        })
        .unwrap();
    // A codespace wake with no error keeps showing no banner, as before.
    let window = cloud_conversation(cx, CloudRef::codespace("octo-abc"));
    window
        .update(cx, |view, _, cx| {
            view.remote_state.online = false;
            view.remote_state.waking = true;
            cx.notify();
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("remote-reconnect-cancel").is_none());
}

// ------------------------------------------------------ stopped workspace banner

fn banner_shown(cx: &mut gpui::VisualTestContext) -> bool {
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.debug_bounds("cloud-resume").is_some()
}

#[gpui::test]
fn the_stopped_banner_shows_only_while_the_workspace_is_stopped(cx: &mut TestAppContext) {
    use monocode_core::cloud_sessions::CloudRef;
    for (cloud, words) in [
        (CloudRef::codespace("octo-abc"), "Codespace stopped"),
        (CloudRef::sandbox("monoptah-fix-1234"), "Sandbox stopped"),
    ] {
        let window = cloud_conversation(cx, cloud);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(!banner_shown(visual), "{words}: running");
        window
            .update(visual, |view, _, cx| view.set_cloud_stopped(true, cx))
            .unwrap();
        assert!(banner_shown(visual), "{words}: stopped");
        window
            .update(visual, |view, _, cx| {
                assert!(view.cloud_stopped_banner());
                // An error is shown by its own banner, with Reconnect and Retry.
                view.remote_state.error = Some("Could not reach sbx".into());
                cx.notify();
            })
            .unwrap();
        assert!(!banner_shown(visual), "{words}: error banner instead");
        assert!(visual.debug_bounds("remote-reconnect").is_some());
        window
            .update(visual, |view, _, cx| {
                view.remote_state.error = None;
                view.set_cloud_stopped(false, cx);
            })
            .unwrap();
        assert!(!banner_shown(visual), "{words}: resumed");
    }
    // A local session never shows it, whatever the shell says.
    let store = monocode_core::store::Store::open(Path::new(":memory:")).unwrap();
    let mut summary = store.create("claude", "/work/widgets").unwrap();
    summary.has_messages = true;
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    window
        .update(visual, |view, _, cx| view.set_cloud_stopped(true, cx))
        .unwrap();
    assert!(!banner_shown(visual));
}

#[gpui::test]
fn the_stopped_banner_text_names_the_workspace(cx: &mut TestAppContext) {
    use monocode_core::cloud_sessions::CloudRef;
    for (cloud, words) in [
        (CloudRef::codespace("octo-abc"), "Codespace stopped"),
        (CloudRef::sandbox("monoptah-fix-1234"), "Sandbox stopped"),
    ] {
        let window = cloud_conversation(cx, cloud);
        window
            .update(cx, |view, _, cx| view.set_cloud_stopped(true, cx))
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        assert!(banner_shown(visual));
        // While it reconnects the banner keeps a disabled "Resuming..." button and Cancel.
        window
            .update(visual, |view, _, cx| {
                view.remote_state.waking = true;
                view.remote_state.wake_cancel =
                    Some(Arc::new(std::sync::atomic::AtomicBool::new(false)));
                view.status = "Starting".into();
                cx.notify();
            })
            .unwrap();
        // No second Resume to press: the button stays, disabled, and says so.
        assert!(banner_shown(visual));
        assert!(visual.debug_bounds("remote-reconnect-cancel").is_some());
        let text = window
            .update(visual, |view, _, _| (view.status.clone(), words))
            .unwrap();
        assert_eq!(text.0, "Starting");
    }
}

/// Resume is the user's click: the codespace wake runs once for it and never without it.
#[gpui::test]
fn clicking_resume_wakes_the_codespace_once_and_nothing_runs_without_a_click(
    cx: &mut TestAppContext,
) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    use monocode_core::cloud_sessions::{CodespaceChoice, Target};
    cx.update(|cx| cx.set_reduce_motion(true));
    let dir = github_project();
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let (_gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let api = monocode_core::mono::cloud_api::CloudApi::shared(&store).unwrap();
    let binding = api
        .prepare(
            dir.0.to_str().unwrap(),
            Target {
                codespace: CodespaceChoice::Named("octo-run".into()),
                branch: None,
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap();
    let mut summary = api.create_session(&binding, "claude").unwrap();
    summary.has_messages = true;
    host.0.lock().unwrap().clear();
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, store.clone(), cx);
                view.loaded = true;
                view.remote_state.refreshing = true;
                view.set_cloud_stopped(true, cx);
                view
            })
        })
        .unwrap()
    });
    let resumed = Arc::new(Mutex::new(0));
    let count = resumed.clone();
    let view = window.root(cx).unwrap();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, _: &super::CloudResumed, _| {
            *count.lock().unwrap() += 1
        })
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    assert!(banner_shown(visual));
    // Rendering and waiting wake nothing.
    visual.run_until_parked();
    assert!(host.0.lock().unwrap().is_empty());
    let button = visual.debug_bounds("cloud-resume").unwrap();
    visual.simulate_click(button.center(), Default::default());
    visual.run_until_parked();
    let calls = host.0.lock().unwrap().clone();
    assert_eq!(
        calls.iter().filter(|c| *c == "ensure m-octo-run").count(),
        1,
        "{calls:?}"
    );
    assert_eq!(*resumed.lock().unwrap(), 1);
    window
        .update(visual, |view, _, _| {
            assert!(!view.remote_state.waking);
            assert!(!view.cloud_stopped);
        })
        .unwrap();
    assert!(!banner_shown(visual));
}

/// A sandbox is resumed through the same user-initiated path as the Reconnect button.
#[gpui::test]
fn clicking_resume_on_a_sandbox_uses_the_user_reconnect_path(cx: &mut TestAppContext) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    use monocode_core::cloud_sessions::CloudRef;
    cx.update(|cx| cx.set_reduce_motion(true));
    let dir = github_project();
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let (gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let summary = {
        let guard = store.lock().unwrap();
        let project = guard
            .remember_cloud_ref_project(
                "env-box",
                &serde_json::json!({"id":"project-1","cwd":"/workspaces/widgets"}),
                dir.0.to_str().unwrap(),
                &CloudRef::sandbox("monoptah-fix-1234"),
            )
            .unwrap();
        let mut summary = guard.create("claude", &project.key).unwrap();
        summary.has_messages = true;
        summary
    };
    host.0.lock().unwrap().clear();
    gh.calls.lock().unwrap().clear();
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, store.clone(), cx);
                view.loaded = true;
                view.remote_state.refreshing = true;
                view.set_cloud_stopped(true, cx);
                view
            })
        })
        .unwrap()
    });
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    assert!(banner_shown(visual));
    let button = visual.debug_bounds("cloud-resume").unwrap();
    visual.simulate_click(button.center(), Default::default());
    visual.run_until_parked();
    // The codespace wake is never used for a sandbox.
    assert!(host.0.lock().unwrap().is_empty());
    assert!(gh.calls.lock().unwrap().is_empty());
    window
        .update(visual, |view, _, _| {
            // The sandbox path ran (this fake service has no sandbox backend, so it says so).
            assert!(!view.remote_state.waking);
            assert!(view.status.contains("sandbox"), "{}", view.status);
        })
        .unwrap();
}

/// A stopped codespace session with the fake wake services, `text` typed in the composer.
fn stopped_codespace(
    cx: &mut TestAppContext,
    text: &str,
) -> (
    gpui::WindowHandle<Conversation>,
    Arc<crate::cloud_wake::fakes::FakeHost>,
) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    use monocode_core::cloud_sessions::{CodespaceChoice, Target};
    cx.update(|cx| cx.set_reduce_motion(true));
    let dir = github_project();
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let (_gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let api = monocode_core::mono::cloud_api::CloudApi::shared(&store).unwrap();
    let binding = api
        .prepare(
            dir.0.to_str().unwrap(),
            Target {
                codespace: CodespaceChoice::Named("octo-run".into()),
                branch: None,
                machine: None,
            },
            &mut |_| {},
        )
        .unwrap();
    let mut summary = api.create_session(&binding, "claude").unwrap();
    summary.has_messages = true;
    host.0.lock().unwrap().clear();
    let text = text.to_owned();
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, store.clone(), cx);
                view.loaded = true;
                view.remote_state.refreshing = true;
                // The refresh after a resume reaches the (fake) host and succeeds.
                view.remote_state.refresh_stub = Some(Ok(()));
                view.set_cloud_stopped(true, cx);
                view.input.update(cx, |input, cx| input.set_text(text, cx));
                view
            })
        })
        .unwrap()
    });
    (window, host)
}

fn ensures(host: &Arc<crate::cloud_wake::fakes::FakeHost>) -> usize {
    host.0
        .lock()
        .unwrap()
        .iter()
        .filter(|c| *c == "ensure m-octo-run")
        .count()
}

#[gpui::test]
fn the_send_button_is_a_resume_pill_only_while_the_workspace_is_stopped(cx: &mut TestAppContext) {
    use monocode_core::cloud_sessions::CloudRef;
    for (cloud, noun) in [
        (CloudRef::codespace("octo-abc"), "codespace"),
        (CloudRef::sandbox("monoptah-fix-1234"), "sandbox"),
    ] {
        let window = cloud_conversation(cx, cloud);
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        window
            .update(visual, |view, _, _| {
                assert!(view.cloud_resume_tip().is_none())
            })
            .unwrap();
        window
            .update(visual, |view, _, cx| view.set_cloud_stopped(true, cx))
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
        window
            .update(visual, |view, _, _| {
                // Usable with or without a prompt.
                let (tip, usable) = view.cloud_resume_tip().unwrap();
                assert_eq!(tip.as_ref(), format!("Resume {noun} and send"));
                assert!(usable);
                view.remote_state.waking = true;
                let (tip, usable) = view.cloud_resume_tip().unwrap();
                assert_eq!(tip.as_ref(), format!("Resuming {noun}\u{2026}"));
                assert!(!usable);
                view.remote_state.waking = false;
            })
            .unwrap();
        let send = visual.debug_bounds("send-or-stop").expect("the pill shows");
        // The pill is wider than the square resting button.
        assert!(send.size.width > gpui::px(40.), "{:?}", send.size);
        window
            .update(visual, |view, _, cx| view.set_cloud_stopped(false, cx))
            .unwrap();
        window
            .update(visual, |view, _, _| {
                assert!(view.cloud_resume_tip().is_none())
            })
            .unwrap();
    }
}

/// Presses Enter in the composer (the user's own submit).
fn submit(window: gpui::WindowHandle<Conversation>, cx: &mut TestAppContext) {
    window
        .update(cx, |view, _, cx| {
            view.input
                .update(cx, |_, cx| cx.emit(crate::input::Submitted))
        })
        .unwrap();
}

fn launched(window: gpui::WindowHandle<Conversation>, cx: &mut TestAppContext) -> Vec<String> {
    window
        .update(cx, |view, _, _| view.launched.clone())
        .unwrap()
}

/// Marks the stopped session as already loaded and online, so the resume can finish its send
/// without a real host (the fake host answers `ensure` only).
fn loaded_and_online(window: gpui::WindowHandle<Conversation>, cx: &mut TestAppContext) {
    window
        .update(cx, |view, _, _| {
            view.remote_state.online = true;
            view.remote_state.refreshing = false;
        })
        .unwrap();
}

#[gpui::test]
fn enter_with_text_while_stopped_wakes_once_and_sends_once_after_connect(cx: &mut TestAppContext) {
    let (window, host) = stopped_codespace(cx, "fix it");
    loaded_and_online(window, cx);
    for _ in 0..3 {
        submit(window, cx);
    }
    cx.run_until_parked();
    assert_eq!(ensures(&host), 1, "{:?}", host.0.lock().unwrap());
    assert_eq!(launched(window, cx), vec!["fix it".to_owned()]);
    window
        .update(cx, |view, _, cx| {
            assert!(!view.cloud_stopped);
            // The message stays queued until the runtime reports the command journaled. The
            // fake host cannot take the turn, so it is still there (failed, never lost); the
            // composer was emptied at once.
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            let id = view.follow_ups.state.messages[0].id.clone();
            view.follow_up_delivered(&id, cx);
            assert!(view.follow_ups.state.messages.is_empty());
            assert_eq!(view.input.read(cx).content.as_ref(), "");
        })
        .unwrap();
}

#[gpui::test]
fn enter_with_text_while_stopped_moves_the_prompt_into_the_queue_before_the_resume_ends(
    cx: &mut TestAppContext,
) {
    let (window, host) = stopped_codespace(cx, "fix it");
    // Not online yet: the queue keeps the prompt while the wake runs.
    submit(window, cx);
    window
        .update(cx, |view, _, cx| {
            let queued: Vec<_> = view
                .follow_ups
                .state
                .messages
                .iter()
                .map(|m| (m.text.clone(), m.steer))
                .collect();
            assert_eq!(queued, vec![("fix it".to_owned(), false)]);
            assert_eq!(view.input.read(cx).content.as_ref(), "");
            assert!(view.remote_state.waking);
            // The queue holds still until the host is back.
            view.pump_follow_ups(cx);
        })
        .unwrap();
    assert!(launched(window, cx).is_empty());
    assert_eq!(ensures(&host), 0, "the wake runs on a worker");
    cx.run_until_parked();
    assert_eq!(ensures(&host), 1);
    // Reconnected, but the history load did not come back online in this fixture.
    window
        .update(cx, |view, _, cx| {
            assert!(view.follow_ups.state.messages.len() <= 1);
            view.remote_state.online = true;
            view.pump_follow_ups(cx);
            view.pump_follow_ups(cx);
        })
        .unwrap();
    assert_eq!(launched(window, cx), vec!["fix it".to_owned()]);
}

#[gpui::test]
fn enter_on_an_empty_composer_resumes_nothing_but_the_pill_does(cx: &mut TestAppContext) {
    let (window, host) = stopped_codespace(cx, "");
    loaded_and_online(window, cx);
    submit(window, cx);
    cx.run_until_parked();
    assert_eq!(ensures(&host), 0);
    window
        .update(cx, |view, _, _| assert!(view.cloud_stopped))
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let send = visual.debug_bounds("send-or-stop").unwrap();
    visual.simulate_click(send.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(ensures(&host), 1, "{:?}", host.0.lock().unwrap());
    assert!(launched(window, visual).is_empty());
}

#[gpui::test]
fn attachments_only_while_stopped_ask_for_a_prompt(cx: &mut TestAppContext) {
    let (window, host) = stopped_codespace(cx, "");
    window
        .update(cx, |view, _, _| view.attachments.push("/tmp/a.png".into()))
        .unwrap();
    submit(window, cx);
    cx.run_until_parked();
    assert_eq!(ensures(&host), 0);
    window
        .update(cx, |view, _, _| {
            assert!(
                view.status.contains("Attachments need a prompt"),
                "{}",
                view.status
            )
        })
        .unwrap();
}

#[gpui::test]
fn a_sandbox_resume_with_text_goes_through_the_user_path_and_never_the_codespace_wake(
    cx: &mut TestAppContext,
) {
    use crate::cloud_wake::fakes::{AUTH_OK, github_project, install};
    use monocode_core::cloud_sessions::CloudRef;
    cx.update(|cx| cx.set_reduce_motion(true));
    let dir = github_project();
    let store: Database = Arc::new(Mutex::new(
        monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
    ));
    let (gh, host) = install(store.clone(), AUTH_OK, "Shutdown");
    let summary = {
        let guard = store.lock().unwrap();
        let project = guard
            .remember_cloud_ref_project(
                "env-box",
                &serde_json::json!({"id":"project-1","cwd":"/workspaces/widgets"}),
                dir.0.to_str().unwrap(),
                &CloudRef::sandbox("monoptah-fix-1234"),
            )
            .unwrap();
        let mut summary = guard.create("claude", &project.key).unwrap();
        summary.has_messages = true;
        summary
    };
    host.0.lock().unwrap().clear();
    gh.calls.lock().unwrap().clear();
    let window = cx.update(|cx| {
        crate::input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, store.clone(), cx);
                view.loaded = true;
                view.remote_state.online = true;
                view.set_cloud_stopped(true, cx);
                view.input
                    .update(cx, |input, cx| input.set_text("fix it".into(), cx));
                view
            })
        })
        .unwrap()
    });
    // A send that is not the user's (Mono, API) wakes and sends nothing.
    window.update(cx, |view, _, cx| view.send(cx)).unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.status, super::remote::SANDBOX_STOPPED);
            assert!(!view.remote_state.waking);
            assert_eq!(view.input.read(cx).content.as_ref(), "fix it");
            assert!(view.follow_ups.state.messages.is_empty());
        })
        .unwrap();
    assert!(host.0.lock().unwrap().is_empty() && gh.calls.lock().unwrap().is_empty());
    // The user's Enter takes the sandbox path; this fake has no sandbox backend, so the resume
    // fails: nothing is sent and the prompt stays in the queue.
    submit(window, cx);
    cx.run_until_parked();
    assert!(host.0.lock().unwrap().is_empty());
    assert!(gh.calls.lock().unwrap().is_empty());
    assert!(launched(window, cx).is_empty());
    window
        .update(cx, |view, _, cx| {
            assert!(!view.remote_state.waking);
            assert_eq!(view.input.read(cx).content.as_ref(), "");
            let queued: Vec<_> = view
                .follow_ups
                .state
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect();
            assert_eq!(queued, vec!["fix it"]);
            assert!(view.status.contains("sandbox"), "{}", view.status);
        })
        .unwrap();
}

#[gpui::test]
fn a_failed_resume_keeps_the_prompt_in_the_queue_and_shows_the_error(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "fix it");
    window
        .update(cx, |view, _, cx| {
            view.queue_composer_prompt(true, cx).unwrap();
            view.remote_connection_error("Could not reach the codespace", cx);
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            assert_eq!(view.follow_ups.state.messages[0].text, "fix it");
            assert_eq!(
                view.remote_state.error.as_deref(),
                Some("Could not reach the codespace")
            );
            // Offline: the queue sends nothing, now or on a later pump.
            view.pump_follow_ups(cx);
        })
        .unwrap();
    assert!(launched(window, cx).is_empty());
    cx.run_until_parked();
    // Back online (a successful Retry or Reconnect): the same prompt goes out, once.
    window
        .update(cx, |view, _, cx| {
            view.remote_state.online = true;
            view.cloud_stopped = false;
            view.pump_follow_ups(cx);
        })
        .unwrap();
    assert_eq!(launched(window, cx), vec!["fix it".to_owned()]);
}

#[gpui::test]
fn a_running_turn_found_after_the_resume_leaves_the_prompt_waiting_in_the_queue(
    cx: &mut TestAppContext,
) {
    let (window, _host) = stopped_codespace(cx, "fix it");
    window
        .update(cx, |view, _, cx| {
            view.queue_composer_prompt(true, cx).unwrap();
            view.remote_state.online = true;
            view.cloud_stopped = false;
            // The load attached a turn that was already running.
            view.busy = true;
            view.pump_follow_ups(cx);
            assert_eq!(view.follow_ups.state.messages.len(), 1);
        })
        .unwrap();
    assert!(launched(window, cx).is_empty());
}

#[gpui::test]
fn a_still_stopped_workspace_does_not_drain_the_queue(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "fix it");
    window
        .update(cx, |view, _, cx| {
            view.queue_composer_prompt(true, cx).unwrap();
            view.remote_state.online = true;
            assert!(view.cloud_stopped);
            view.pump_follow_ups(cx);
            view.remote_state.waking = true;
            view.cloud_stopped = false;
            view.pump_follow_ups(cx);
            assert_eq!(view.follow_ups.state.messages.len(), 1);
        })
        .unwrap();
    assert!(launched(window, cx).is_empty());
}

#[gpui::test]
fn a_send_that_is_not_the_users_never_auto_sends_after_a_resume(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "fix it");
    window
        .update(cx, |view, _, cx| {
            assert!(!view.user_send);
            // The codespace keeps its existing wake path, but no prompt is queued for later.
            view.send(cx);
            assert!(view.follow_ups.state.messages.is_empty());
        })
        .unwrap();
    cx.run_until_parked();
    assert!(launched(window, cx).is_empty());
}

#[gpui::test]
fn every_remote_notice_variant_sits_above_the_composer(cx: &mut TestAppContext) {
    use monocode_core::cloud_sessions::CloudRef;
    type Setup = fn(&mut Conversation);
    let sandbox = || CloudRef::sandbox("monoptah-fix-1234");
    let cases: Vec<(&str, CloudRef, Setup, &str)> = vec![
        (
            "error",
            CloudRef::codespace("octo-abc"),
            |v| v.remote_state.error = Some("The host request did not complete.".into()),
            "remote-retry",
        ),
        (
            "rejected",
            CloudRef::codespace("octo-abc"),
            |v| {
                v.remote_state.error = Some("Rejected".into());
                v.remote_state.rejected = Some("text".into());
            },
            "remote-retry",
        ),
        (
            "pending",
            CloudRef::codespace("octo-abc"),
            |v| v.remote_state.pending = true,
            "remote-retry",
        ),
        (
            "stopped",
            CloudRef::codespace("octo-abc"),
            |v| v.cloud_stopped = true,
            "cloud-resume",
        ),
        (
            "reconnecting",
            sandbox(),
            |v| {
                v.remote_state.waking = true;
                v.remote_state.wake_cancel =
                    Some(Arc::new(std::sync::atomic::AtomicBool::new(false)));
            },
            "remote-reconnect-cancel",
        ),
        (
            "dead",
            sandbox(),
            |v| v.remote_state.error = Some(RECREATED.into()),
            "remote-archive-session",
        ),
    ];
    for (name, cloud, setup, button) in cases {
        let window = cloud_conversation(cx, cloud);
        window
            .update(cx, |view, _, cx| {
                view.remote_state.online = false;
                setup(view);
                cx.notify();
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        let notice = visual
            .debug_bounds("remote-notice")
            .unwrap_or_else(|| panic!("{name}: no notice"));
        let composer = visual.debug_bounds("composer-box").expect("composer");
        assert!(
            notice.bottom() <= composer.top(),
            "{name}: {notice:?} {composer:?}"
        );
        let action = visual
            .debug_bounds(button)
            .unwrap_or_else(|| panic!("{name}: no {button}"));
        // The actions are inside the card and right of the title.
        let title = visual.debug_bounds("remote-notice-title").expect("title");
        assert!(action.left() > title.left(), "{name}");
        assert!(action.right() <= notice.right(), "{name}");
    }
}

// ------------------------------------------------------ resume ordering and queue safety

#[gpui::test]
fn a_refresh_that_began_before_a_reconnect_is_ignored(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "");
    window
        .update(cx, |view, _, cx| {
            view.cloud_stopped = false;
            view.remote_state.online = true;
            view.remote_state.refreshing = false;
            // The old tunnel accepts but the stopped host never answers: a timeout.
            view.remote_state.refresh_stub = Some(Err("The host request did not complete.".into()));
            view.refresh_remote_snapshot(cx);
            assert!(view.remote_state.refreshing);
            // The reconnect starts while that request is still in flight.
            view.remote_state.epoch += 1;
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(view.remote_state.online);
            assert!(view.remote_state.error.is_none());
            // No newer refresh took over, so the stale one released the flag.
            assert!(!view.remote_state.refreshing);
        })
        .unwrap();
}

#[gpui::test]
fn a_stale_refresh_does_not_keep_the_flag_when_no_newer_refresh_runs_and_yields_to_one_that_does(
    cx: &mut TestAppContext,
) {
    let (window, _host) = stopped_codespace(cx, "");
    window
        .update(cx, |view, _, cx| {
            view.cloud_stopped = false;
            view.remote_state.refreshing = false;
            view.remote_state.refresh_stub = Some(Ok(()));
            view.refresh_remote_snapshot(cx);
            // The reconnect failed or was cancelled: epoch moved, nothing new started.
            view.remote_state.epoch += 1;
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(!view.remote_state.refreshing);
            // A newer refresh in flight keeps its flag when an older answer lands.
            view.refresh_remote_snapshot(cx);
            view.remote_state.epoch += 1;
            view.remote_state.refreshing = false;
            view.refresh_remote_snapshot(cx);
            assert!(view.remote_state.refreshing);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |view, _, _| assert!(!view.remote_state.refreshing))
        .unwrap();
}

#[gpui::test]
fn the_clock_skips_a_stopped_or_waking_workspace(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "");
    window
        .update(cx, |view, _, _| {
            view.history_unloaded = false;
            assert!(view.cloud_stopped);
            assert!(!view.background_refresh_due());
            view.cloud_stopped = false;
            assert!(view.background_refresh_due());
            view.remote_state.waking = true;
            assert!(!view.background_refresh_due());
        })
        .unwrap();
}

#[gpui::test]
fn a_resume_whose_fresh_refresh_fails_sends_nothing_and_keeps_the_prompt(cx: &mut TestAppContext) {
    let (window, host) = stopped_codespace(cx, "fix it");
    window
        .update(cx, |view, _, _| {
            view.remote_state.online = true;
            view.remote_state.refreshing = false;
            view.remote_state.refresh_stub = Some(Err("The host request did not complete.".into()));
        })
        .unwrap();
    submit(window, cx);
    cx.run_until_parked();
    assert_eq!(ensures(&host), 1);
    assert!(launched(window, cx).is_empty());
    window
        .update(cx, |view, _, _| {
            assert!(!view.remote_state.online);
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            assert!(view.remote_state.error.is_some());
        })
        .unwrap();
}

#[gpui::test]
fn the_drained_prompt_stays_queued_while_it_is_being_sent(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "fix it");
    let id = window
        .update(cx, |view, _, cx| {
            view.cloud_stopped = false;
            view.remote_state.online = true;
            view.queue_composer_prompt(true, cx).unwrap()
        })
        .unwrap();
    // The save comes back and the queue pumps; the worker has not run yet.
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.launched, vec!["fix it".to_owned()]);
            // Not removed at launch: only the runtime's receipt does that.
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            assert_eq!(view.settings.mono_message_id, None);
            // A failure before the command was journaled keeps the prompt with its error.
            view.fail_follow_up(&id, "boom".into(), cx);
            assert_eq!(view.follow_ups.state.messages.len(), 1);
            assert_eq!(
                view.follow_ups.state.messages[0].error.as_deref(),
                Some("boom")
            );
            assert!(view.follow_ups.state.paused);
        })
        .unwrap();
}

#[gpui::test]
fn retrying_a_remote_queue_sends_no_continue_prompt(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "");
    window
        .update(cx, |view, _, cx| {
            view.cloud_stopped = false;
            view.remote_state.online = false;
            view.input
                .update(cx, |input, cx| input.set_text("fix it".into(), cx));
            let id = view.queue_composer_prompt(true, cx).unwrap();
            view.fail_follow_up(&id, "boom".into(), cx);
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.remote_state.online = true;
            view.resume_queue(cx);
            assert!(!view.follow_ups.state.paused);
            assert!(view.follow_ups.state.messages[0].error.is_none());
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, _| {
            // The queued prompt itself goes out; the synthetic continue text never does.
            assert_eq!(view.launched, vec!["fix it".to_owned()]);
        })
        .unwrap();
}

#[gpui::test]
fn a_blocked_head_message_pauses_the_queue_with_a_retry(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "");
    window
        .update(cx, |view, _, cx| {
            view.cloud_stopped = false;
            view.remote_state.online = true;
            view.remote_state.capabilities.clear();
            view.attachments = vec![PathBuf::from("/tmp/a.png")];
            view.input
                .update(cx, |input, cx| input.set_text("see this".into(), cx));
            view.queue_composer_prompt(true, cx).unwrap();
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.launched.is_empty());
            assert!(view.follow_ups.state.paused);
            assert!(view.follow_ups.state.messages[0].error.is_some());
            // Retry clears it; the capability is still missing, so it blocks again, visibly.
            view.resume_queue(cx);
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, _| {
            assert!(view.follow_ups.state.paused);
            assert!(view.follow_ups.state.messages[0].error.is_some());
            assert!(view.launched.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn queued_messages_of_a_remote_session_offer_no_steer(cx: &mut TestAppContext) {
    let (window, _host) = stopped_codespace(cx, "fix it");
    submit(window, cx);
    let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    window
        .update(visual, |view, _, _| {
            assert!(!view.follow_ups.state.messages.is_empty() || !view.launched.is_empty())
        })
        .unwrap();
    assert!(visual.debug_bounds("queue-steer").is_none());
}
