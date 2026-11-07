//! Reconnecting the codespace of a cloud session for a user-initiated send.
//!
//! Shared by the Operator and Mono dispatchers; the rule itself is
//! `monocode_core::mono::cloud_api::should_wake` so the GUI can reuse it. The history of a cloud
//! session cannot load while its codespace has no live connection (background work never wakes
//! or reconnects), so a send would wait forever. A send therefore calls `ensure_codespace`
//! (through `CloudApi::wake`) for every codespace state except Unavailable; it is a no-op when
//! already connected. The wake runs in the background and the view reloads when it is done, which
//! also flushes a send the Mono queued. Drafts and background polls never come here.
use crate::{conversation::Conversation, runtime::Database};
use gpui::{AsyncApp, Entity};
use monocode_core::mono::cloud_api::{CloudApi, should_wake};

/// Starts (re)connecting the codespace of cloud session `id` for a send, unless it is
/// Unavailable or the session is local. Returns whether a wake was started; never waits for it.
pub(crate) async fn wake_for_send(
    store: Database,
    id: i64,
    view: Entity<Conversation>,
    cx: &mut AsyncApp,
) -> bool {
    let probe = store.clone();
    let stopped = cx
        .background_executor()
        .spawn(async move {
            let target = CloudApi::shared(&probe).ok()?.target_of(id)?;
            Some(should_wake(target.state))
        })
        .await
        .unwrap_or(false);
    if stopped {
        let wake = cx
            .background_executor()
            .spawn(async move { CloudApi::shared(&store)?.wake(id, &store) });
        cx.spawn(async move |cx| {
            if wake.await.is_ok() {
                view.update(cx, |v, cx| v.load(cx));
            }
        })
        .detach();
    }
    stopped
}

/// Reconnects the codespace of cloud session `id` for a send or an explicit "Reconnect" from the
/// conversation view and waits for it. `Ok(false)` means nothing was done (local session or the
/// codespace is Unavailable). Runs gh and the SSH bootstrap on the background executor.
pub(crate) async fn reconnect(store: Database, id: i64, cx: &mut AsyncApp) -> Result<bool, String> {
    cx.background_executor()
        .spawn(async move {
            let api = CloudApi::shared(&store)?;
            match api.target_of(id) {
                Some(target) if should_wake(target.state) => api.wake(id, &store).map(|()| true),
                _ => Ok(false),
            }
        })
        .await
}

/// Doubles for tests of the dispatchers: a git project with a GitHub origin, a fake `gh`
/// and a fake machine host, installed as the shared cloud service.
#[cfg(test)]
pub(crate) mod fakes {
    use super::Database;
    use monocode_core::{
        cloud_sessions::Host,
        codespaces::{GhOutput, GhRunner},
        mono::cloud_api::CloudApi,
        remote::Machine,
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
    /// Records what it is asked to do; the codespace `octo-run` is its only machine.
    pub(crate) struct FakeHost(pub(crate) Mutex<Vec<String>>);
    fn machine(name: &str) -> Machine {
        Machine {
            id: format!("m-{name}"),
            name: name.into(),
            endpoint: format!("codespace://{name}"),
            environment_id: format!("env-{name}"),
            ssh: None,
            codespace: Some(name.into()),
        }
    }
    impl Host for FakeHost {
        fn machines(&self) -> Result<Vec<Machine>, String> {
            Ok(vec![machine("octo-run")])
        }
        fn connect(&self, name: &str, _: &str, _: &mut dyn FnMut(&str)) -> Result<Machine, String> {
            Ok(machine(name))
        }
        fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
            self.0.lock().unwrap().push(format!("ensure {id}"));
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
        let host = Arc::new(FakeHost(Mutex::new(vec![])));
        CloudApi::install(Arc::new(CloudApi::new(gh.clone(), host.clone(), store)));
        (gh, host)
    }
}
