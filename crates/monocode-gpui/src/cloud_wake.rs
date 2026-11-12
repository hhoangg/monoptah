//! Waking the instance of a cloud session for a send, whatever the backend.
//!
//! Shared by the Operator and Mono dispatchers and the conversation view; the rule itself is
//! `monocode_core::mono::cloud_api::should_wake` so the GUI can reuse it. The history of a cloud
//! session cannot load while its instance has no live connection (background work never wakes
//! or reconnects), so a send would wait forever. A send therefore calls `CloudApi::wake` for
//! every state except Unavailable, for a codespace and for a Docker sandbox alike (a sandbox is
//! resumed and its relay, Git credential and Claude environment restored); it is a no-op when
//! already connected. Sending is an explicit act, so the origin (a Mono, the app API or the
//! user's own hand) does not matter. The wake runs in the background and the view reloads when
//! it is done, which also flushes a send the Mono queued. Drafts and background polls never come
//! here.
use crate::{conversation::Conversation, runtime::Database};
use gpui::{AsyncApp, Entity};
use monocode_core::cloud_sessions::CloudBackendKind;
use monocode_core::mono::cloud_api::{CloudApi, should_wake};

/// Starts (re)connecting the instance of cloud session `id` for a send, unless it is
/// Unavailable or the session is local. Returns whether a wake was started; never waits for it.
/// The wake is the view's own `reconnect_cloud`: one in-flight flag with the Reconnect button,
/// progress on the status line, and a failure lands in the view's connection-error path (so a
/// send a Mono queued is not left waiting silently). The blocking work stays on the background
/// executor.
pub(crate) async fn wake_for_send(
    store: Database,
    id: i64,
    view: Entity<Conversation>,
    cx: &mut AsyncApp,
) -> bool {
    let wake = cx
        .background_executor()
        .spawn(async move {
            let target = CloudApi::shared(&store).ok()?.target_of(id)?;
            Some(should_wake(target.state))
        })
        .await
        .unwrap_or(false);
    wake && view.update(cx, |v, cx| v.reconnect_cloud(cx))
}

/// Reconnects the instance of cloud session `id` for the conversation view's "Reconnect" button,
/// a send typed in the session or a click on a file link in its transcript, and waits for it. `Ok(false)` means nothing was done (local
/// session or the instance is Unavailable). Runs gh, sbx and the SSH bootstrap on the background
/// executor. A sandbox reports its steps on `progress` and honors `cancel`; a codespace wake has
/// neither.
pub(crate) async fn reconnect(
    store: Database,
    id: i64,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: async_channel::Sender<String>,
    cx: &mut AsyncApp,
) -> Result<bool, String> {
    cx.background_executor()
        .spawn(async move {
            reconnect_blocking(&store, id, &cancel, &mut |line| {
                let _ = progress.send_blocking(line.to_string());
            })
        })
        .await
}

/// [`reconnect`] for a worker thread: BLOCKS, for minutes. It goes through the shared
/// [`CloudApi`], whose in-flight set is the one the Reconnect button uses, so two callers never
/// run the sandbox flow of the same sandbox at once.
pub(crate) fn reconnect_blocking(
    store: &Database,
    id: i64,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<bool, String> {
    let api = CloudApi::shared(store)?;
    match api.target_of(id) {
        // User-initiated by construction: this is the Reconnect button, a send typed in the
        // session's own view or a click on a file link in its transcript.
        Some(target) if target.cloud.kind == CloudBackendKind::DockerSandbox => {
            if !should_wake(target.state) {
                return Ok(false);
            }
            api.reconnect_sandbox(id, progress, cancel).map(|()| true)
        }
        Some(target) if should_wake(target.state) => api.wake(id, store).map(|()| true),
        _ => Ok(false),
    }
}

/// Doubles for tests of the dispatchers: a git project with a GitHub origin, a fake `gh`
/// and a fake machine host, installed as the shared cloud service.
#[cfg(test)]
pub(crate) mod fakes {
    use super::Database;
    use monocode_core::{
        cloud_sessions::{
            CloudRef, Host,
            backend::docker::{SandboxShell, ShellOutput},
            docker_start::{ClaudeAuth, SandboxSecrets},
        },
        codespaces::{GhOutput, GhRunner},
        mono::cloud_api::CloudApi,
        remote::{Machine, SandboxTarget},
        sandboxes::{SbxOutput, SbxRunner},
    };
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex, atomic::AtomicBool};

    pub(crate) const AUTH_OK: &str = "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
    pub(crate) const AUTH_NO_SCOPE: &str =
        "github.com\n  ✓ Logged in to github.com account me (keyring)\n  - Token scopes: 'repo'\n";
    const LIST: &str = r#"[{"name":"octo-run","displayName":"run","state":"Shutdown","repository":"octo/widgets"}]"#;

    /// `state` is what `gh codespace view` reports (`Shutdown`, `Available`, ...).
    pub(crate) struct Gh {
        pub(crate) auth: &'static str,
        pub(crate) state: &'static str,
        pub(crate) calls: Mutex<Vec<String>>,
    }
    impl GhRunner for Gh {
        fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
            let line = args.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            if line.contains("/codespaces/machines") {
                // GitHub not reachable: the static default machine stays.
                return Ok(GhOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: "HTTP 404".into(),
                });
            }
            let view = format!(r#"{{"state":"{}"}}"#, self.state);
            let stdout = if line.contains("auth status") {
                self.auth
            } else if line.contains("codespace list") {
                LIST
            } else if line.contains("codespace view") {
                &view
            } else if line.contains("api --method POST") {
                r#"{"name":"octo-new","state":"Provisioning"}"#
            } else if line.contains("codespace stop")
                || line.contains("codespace delete")
                || line.contains("codespace ssh")
            {
                ""
            } else if line.contains("secrets") {
                r#"{"secrets":[]}"#
            } else {
                panic!("unexpected gh call: {line}")
            };
            Ok(GhOutput {
                success: true,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        }
    }
    /// Records what it is asked to do; the codespace `octo-run` is its only machine, plus the
    /// sandbox [`SANDBOX`] when the second field is set.
    pub(crate) struct FakeHost(
        pub(crate) Mutex<Vec<String>>,
        pub(crate) bool,
        /// Makes `ensure` (the codespace wake) fail.
        pub(crate) AtomicBool,
    );
    /// The name of the fake Docker sandbox.
    pub(crate) const SANDBOX: &str = "box-1";
    fn sandbox_machine() -> Machine {
        Machine {
            id: format!("m-sbx-{SANDBOX}"),
            name: SANDBOX.into(),
            endpoint: format!("sandbox://{SANDBOX}"),
            environment_id: format!("env-{SANDBOX}"),
            ssh: None,
            codespace: None,
            sandbox: Some(SandboxTarget {
                id: "sbx_001abc".into(),
                name: SANDBOX.into(),
                workspace: "/home/agent/workspaces/widgets".into(),
                relay_port: 47200,
                config: None,
                mode: Default::default(),
                docker_account: None,
            }),
        }
    }
    fn machine(name: &str) -> Machine {
        Machine {
            id: format!("m-{name}"),
            name: name.into(),
            endpoint: format!("codespace://{name}"),
            environment_id: format!("env-{name}"),
            ssh: None,
            codespace: Some(name.into()),
            sandbox: None,
        }
    }
    impl Host for FakeHost {
        fn machines(&self) -> Result<Vec<Machine>, String> {
            let mut machines = vec![machine("octo-run")];
            if self.1 {
                machines.push(sandbox_machine());
            }
            Ok(machines)
        }
        fn connect(
            &self,
            name: &str,
            _: &str,
            _: &AtomicBool,
            _: &mut dyn FnMut(&str),
        ) -> Result<Machine, String> {
            Ok(machine(name))
        }
        fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("ensure {id}"));
            if self.2.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("ssh refused".into());
            }
            Ok(())
        }
        fn request(&self, id: &str, method: &str, params: Value) -> Result<Value, String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("request {id} {method}"));
            Ok(json!({"id":"project-1","cwd":params["cwd"]}))
        }
        fn disconnect(&self, id: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("disconnect {id}"));
            Ok(())
        }
    }

    /// A fake `sbx` with the one sandbox [`SANDBOX`] in state `state`; records every call.
    pub(crate) struct Sbx {
        pub(crate) state: Mutex<String>,
        pub(crate) calls: Mutex<Vec<String>>,
    }
    impl SbxRunner for Sbx {
        fn run(&self, args: &[String], _: Option<&str>) -> Result<SbxOutput, String> {
            self.calls.lock().unwrap().push(args.join(" "));
            let state = self.state.lock().unwrap().clone();
            let item = json!({"id":"sbx_001abc","name":SANDBOX,"agent":"shell","state":state,"cpus":4,"memory_mib":8192,"expires_at":"2030-01-01T00:00:00Z"});
            let ok = |text: String| {
                Ok(SbxOutput {
                    success: true,
                    stdout: text,
                    stderr: String::new(),
                })
            };
            match args.get(2).map(String::as_str).unwrap_or("") {
                "ls" => ok(json!([item]).to_string()),
                "inspect" => ok(item.to_string()),
                "stop" | "rm" => ok(String::new()),
                other => panic!("unexpected sbx call: {other}"),
            }
        }
    }
    /// The ssh seam: tests of the dispatchers never reach a sandbox over ssh, so it refuses
    /// and counts what it was asked.
    #[derive(Default)]
    pub(crate) struct Shell(pub(crate) Mutex<usize>);
    impl SandboxShell for Shell {
        fn run(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &mut dyn FnMut(&str),
            _: &dyn Fn() -> bool,
            _: std::time::Duration,
        ) -> Result<ShellOutput, String> {
            *self.0.lock().unwrap() += 1;
            Err("unexpected ssh".into())
        }
    }
    struct NoSecrets;
    impl SandboxSecrets for NoSecrets {
        fn claude_auth(&self, _: Option<&str>) -> Result<Option<ClaudeAuth>, String> {
            Ok(None)
        }
        fn git_payload(&self, _: &str, _: &str) -> Result<zeroize::Zeroizing<String>, String> {
            Err("no credentials in tests".into())
        }
    }
    /// A session of `project` bound to the fake sandbox, as the sandbox start flow leaves it.
    pub(crate) fn sandbox_session(
        store: &Database,
        project: &str,
    ) -> monocode_core::store::Session {
        let store = store.lock().unwrap();
        let remote = store
            .remember_cloud_ref_project(
                &format!("env-{SANDBOX}"),
                &json!({"id":"project-1","cwd":"/workspaces/widgets"}),
                project,
                &CloudRef::sandbox(SANDBOX),
            )
            .unwrap();
        store.create("claude", &remote.key).unwrap()
    }

    /// The cloud service is process-wide (`CloudApi::install`): tests using it run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());
    /// A scratch directory removed on drop; it also holds the serial lock.
    pub(crate) struct Dir(
        pub(crate) std::path::PathBuf,
        #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
    );
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    /// Holds the serial lock for a test that builds its own project.
    pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// A git project whose origin is `github.com/octo/widgets`.
    pub(crate) fn github_project() -> Dir {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = Dir(
            std::env::temp_dir().join(format!(
                "monoptah-cloud-api-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
            serial,
        );
        std::fs::create_dir_all(&dir.0).unwrap();
        for args in [
            vec!["init", "-q", "-b", "trunk"],
            vec!["remote", "add", "origin", "https://github.com/octo/widgets"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&dir.0)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        dir
    }
    /// Installs the shared cloud service over the fakes.
    pub(crate) fn install(
        store: Database,
        auth: &'static str,
        state: &'static str,
    ) -> (Arc<Gh>, Arc<FakeHost>) {
        let gh = Arc::new(Gh {
            auth,
            state,
            calls: Mutex::new(vec![]),
        });
        let host = Arc::new(FakeHost(Mutex::new(vec![]), false, AtomicBool::new(false)));
        CloudApi::install(Arc::new(CloudApi::new(gh.clone(), host.clone(), store)));
        (gh, host)
    }
    /// [`install`] plus the Docker sandbox [`SANDBOX`] in `sandbox_state` (`running`,
    /// `stopped`, ...); its machine is connected and the ssh seam refuses.
    pub(crate) fn install_with_sandbox(
        store: Database,
        auth: &'static str,
        state: &'static str,
        sandbox_state: &str,
    ) -> (Arc<Gh>, Arc<FakeHost>, Arc<Sbx>, Arc<Shell>) {
        let gh = Arc::new(Gh {
            auth,
            state,
            calls: Mutex::new(vec![]),
        });
        let host = Arc::new(FakeHost(Mutex::new(vec![]), true, AtomicBool::new(false)));
        let sbx = Arc::new(Sbx {
            state: Mutex::new(sandbox_state.into()),
            calls: Mutex::new(vec![]),
        });
        let shell = Arc::new(Shell::default());
        CloudApi::install(Arc::new(CloudApi::with_sandboxes(
            gh.clone(),
            host.clone(),
            store,
            sbx.clone(),
            shell.clone(),
            Arc::new(NoSecrets),
        )));
        (gh, host, sbx, shell)
    }
}
