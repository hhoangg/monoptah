//! App API for codespaces (`codespaces.*` actions and `sessions.start` with a `target`).
//!
//! One service shared by the Operator dispatcher and the Mono dispatcher so both behave the
//! same. It is a thin layer over [`CloudSessions`] and [`crate::codespaces`]: every call here
//! BLOCKS (gh, and for sessions.start minutes of codespace creation or boot), so callers run
//! it on a worker thread. Nothing here wakes a codespace except preparing a session target the
//! caller explicitly asked for; listings only read cached state.
//!
//! Secret values never pass through this module: `secrets_status` reports set/missing only.
use crate::cloud_sessions::{
    CloudBinding, CloudSessions, CloudState, CodespaceChoice, Host, SessionTarget, Target,
};
use crate::codespaces::{self, GhRunner};
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

type Database = Arc<Mutex<Store>>;

static SHARED: Mutex<Option<Arc<CloudApi>>> = Mutex::new(None);

pub struct CloudApi {
    gh: Arc<dyn GhRunner>,
    host: Arc<dyn Host>,
    cloud: CloudSessions,
}

/// The codespace lifecycle as it is reported to agents.
pub fn state_id(state: CloudState) -> String {
    state.label().to_lowercase()
}

/// `target` of a session: `{kind:"local"}` or `{kind:"codespace",codespace,state}`.
pub fn target_json(target: Option<&SessionTarget>) -> Value {
    match target {
        None => json!({"kind":"local"}),
        Some(t) => json!({"kind":"codespace","codespace":t.codespace,"state":state_id(t.state)}),
    }
}

/// Adds `target` to a session object, and `codespaceState` for a cloud session.
pub fn annotate(session: &mut Value, target: Option<&SessionTarget>) {
    session["target"] = target_json(target);
    if let Some(t) = target {
        session["codespaceState"] = json!(state_id(t.state));
    }
}

/// The parsed, already validated `target` of sessions.start (see `mono::api::validate`).
pub fn target_from_input(input: &Value) -> Option<Target> {
    let target = input.get("target")?;
    let text = |key: &str| target.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Target {
        codespace: match text("codespace")?.as_str() {
            "new" => CodespaceChoice::New,
            name => CodespaceChoice::Named(name.into()),
        },
        branch: text("branch"),
        machine: text("machine"),
    })
}

/// True when session `id` is a cloud session bound to the local `project`.
pub fn is_cloud_session_of(store: &Store, id: i64, project: &str) -> bool {
    let key = crate::files::path_key(project);
    store.remote_session(id).ok().flatten().is_some_and(|s| {
        s.project.codespace.is_some()
            && s.project
                .local_project
                .as_deref()
                .is_some_and(|p| crate::files::path_key(p) == key)
    })
}

/// Error text for a start that failed after its codespace was created.
pub fn created_note(codespace: &str, error: &str) -> String {
    format!(
        "{error} (codespace {codespace} was created and kept; retry with target {{\"codespace\":\"{codespace}\"}} or remove it with codespaces.delete)"
    )
}

fn current_branch(project: &str) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(["-C", project, "rev-parse", "--abbrev-ref", "HEAD"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("Cannot run git: {e}"))?;
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || branch.is_empty() || branch == "HEAD" {
        return Err(
            "Provide branch: the project has no current branch to create a codespace from".into(),
        );
    }
    Ok(branch)
}

impl CloudApi {
    pub fn new(gh: Arc<dyn GhRunner>, host: Arc<dyn Host>, store: Database) -> Self {
        Self {
            cloud: CloudSessions::new(gh.clone(), host.clone(), store),
            gh,
            host,
        }
    }
    /// Real `gh` and the app's default connections.
    pub fn system(store: Database) -> Result<Self, String> {
        let gh: Arc<dyn GhRunner> = Arc::new(codespaces::SystemGh);
        let host: Arc<dyn Host> = Arc::new(crate::remote::default_connections()?);
        Ok(Self::new(gh, host, store))
    }
    /// The process-wide service (it owns the codespace state cache). `install` replaces it.
    pub fn shared(store: &Database) -> Result<Arc<Self>, String> {
        let mut shared = SHARED.lock().map_err(|_| "Codespace service unavailable")?;
        if let Some(api) = shared.as_ref() {
            return Ok(api.clone());
        }
        let api = Arc::new(Self::system(store.clone())?);
        *shared = Some(api.clone());
        Ok(api)
    }
    /// Replaces the shared service (tests and embedders with their own seams).
    pub fn install(api: Arc<Self>) {
        if let Ok(mut shared) = SHARED.lock() {
            *shared = Some(api);
        }
    }

    fn scoped_repo(&self, project: &str) -> Result<String, String> {
        codespaces::auth_status(&*self.gh)?.require_codespace_scope()?;
        self.cloud.repo_for(project)
    }
    /// Refuses names that are not codespaces of the project's repository.
    fn owned(&self, repo: &str, name: &str) -> Result<(), String> {
        if codespaces::list(&*self.gh, repo)?
            .iter()
            .any(|c| c.name == name)
        {
            Ok(())
        } else {
            Err(format!(
                "Codespace {name} is not a codespace of {repo}; run codespaces.list"
            ))
        }
    }

    /// `codespaces.list`: the codespaces of the project's GitHub repository.
    pub fn list(&self, project: &str) -> Result<Value, String> {
        let repo = self.scoped_repo(project)?;
        let rows: Vec<Value> = codespaces::list(&*self.gh, &repo)?
            .into_iter()
            .map(|c| {
                json!({
                    "name":c.name,
                    "displayName":c.display_name,
                    "state":CloudState::from_gh(&c.state).label().to_lowercase(),
                    "ghState":c.state,
                    "machine":c.machine_name,
                    "branch":c.git_status.map(|g| g.git_ref),
                    "lastUsedAt":c.last_used_at,
                })
            })
            .collect();
        Ok(json!({"repo":repo,"codespaces":rows}))
    }

    /// `codespaces.create {branch?,machine?,displayName?}`: create and wait until available.
    pub fn create(
        &self,
        project: &str,
        input: &Value,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Value, String> {
        let repo = self.scoped_repo(project)?;
        let text = |key: &str| input.get(key).and_then(Value::as_str);
        let branch = match text("branch") {
            Some(branch) => branch.to_owned(),
            None => current_branch(project)?,
        };
        let name = codespaces::create(
            &*self.gh,
            &repo,
            &branch,
            text("machine"),
            text("displayName"),
            progress,
        )?;
        codespaces::wait_ready(&*self.gh, &name)?;
        self.cloud.invalidate(&name);
        Ok(json!({"codespace":name,"repo":repo,"branch":branch,"state":"available"}))
    }

    /// `codespaces.stop {name}`.
    pub fn stop(&self, project: &str, name: &str) -> Result<Value, String> {
        let repo = self.scoped_repo(project)?;
        self.owned(&repo, name)?;
        self.cloud.stop_codespace(name)?;
        Ok(json!({"codespace":name,"stopped":true}))
    }

    /// `codespaces.delete {name}`: irreversible; also forgets the bindings of its sessions.
    pub fn delete(&self, project: &str, name: &str) -> Result<Value, String> {
        let repo = self.scoped_repo(project)?;
        self.owned(&repo, name)?;
        self.cloud.delete_codespace(name)?;
        Ok(json!({"codespace":name,"deleted":true}))
    }

    /// `codespaces.secretsStatus`: gh auth, the secrets the project's devcontainer asks for and
    /// `CLAUDE_CODE_OAUTH_TOKEN`, each as set / missing / notForRepo. Never values.
    pub fn secrets_status(&self, project: &str) -> Result<Value, String> {
        let auth = codespaces::auth_status(&*self.gh)?;
        let repo = self.cloud.repo_for(project).ok();
        let wanted = codespaces::devcontainer_secrets(Path::new(project));
        let mut result = json!({
            "auth":auth,
            "repo":repo,
            "devcontainerSecrets":wanted,
            "secrets":[],
        });
        let Some(repo) = repo else {
            result["note"] = json!("The project's origin is not a GitHub repository.");
            return Ok(result);
        };
        if !auth.logged_in || !auth.has_codespace_scope {
            result["note"] = json!(auth
                .require_codespace_scope()
                .err()
                .unwrap_or_else(|| "Sign in with gh auth login first.".into()));
            return Ok(result);
        }
        let keys: Vec<String> = wanted.iter().map(|s| s.key.clone()).collect();
        result["secrets"] = json!(codespaces::secrets_status(&*self.gh, &repo, &keys)?);
        Ok(result)
    }

    /// Prepares the codespace for `sessions.start`: creating one needs a branch (default: the
    /// project's current branch). BLOCKS for minutes when creating or booting.
    ///
    /// A codespace created here is never deleted on failure (it may hold the user's work and
    /// billing stops by itself when it idles). When preparing fails after it was created, the
    /// error names it so the caller retries with `target {codespace:"<name>"}` instead of
    /// creating a second one.
    pub fn prepare(
        &self,
        project: &str,
        mut target: Target,
        progress: &mut dyn FnMut(&str),
    ) -> Result<CloudBinding, String> {
        let creating = target.codespace == CodespaceChoice::New;
        if creating && target.branch.is_none() {
            target.branch = Some(current_branch(project)?);
        }
        let before = creating.then(|| self.codespace_names(project)).flatten();
        match self.cloud.prepare_target(project, target, progress) {
            Ok(binding) => {
                // The state cache may still say Shutdown/Starting for a reused codespace.
                self.cloud.invalidate(&binding.codespace);
                Ok(binding)
            }
            Err(error) => {
                let created = before.and_then(|before| {
                    self.codespace_names(project)?
                        .into_iter()
                        .find(|name| !before.contains(name))
                });
                Err(match created {
                    Some(name) => created_note(&name, &error),
                    None => error,
                })
            }
        }
    }
    fn codespace_names(&self, project: &str) -> Option<Vec<String>> {
        let repo = self.cloud.repo_for(project).ok()?;
        Some(
            codespaces::list(&*self.gh, &repo)
                .ok()?
                .into_iter()
                .map(|c| c.name)
                .collect(),
        )
    }
    /// A blank session on the binding (Claude only).
    pub fn create_session(
        &self,
        binding: &CloudBinding,
        harness: &str,
    ) -> Result<crate::store::Session, String> {
        self.cloud.create_session(binding, harness)
    }

    /// Cloud sessions of `project` with their cached codespace state. Never wakes anything.
    pub fn session_targets(&self, project: &str) -> HashMap<i64, SessionTarget> {
        let mut states: HashMap<String, CloudState> = HashMap::new();
        self.cloud
            .cloud_sessions_for(project)
            .unwrap_or_default()
            .into_iter()
            .map(|s| {
                let state = *states
                    .entry(s.codespace.clone())
                    .or_insert_with(|| self.cloud.cloud_state(&s.codespace));
                (
                    s.summary.id,
                    SessionTarget {
                        codespace: s.codespace,
                        state,
                    },
                )
            })
            .collect()
    }
    /// Wakes the codespace of a cloud session (no-op when connected or not a cloud session).
    /// BLOCKS for minutes; used when a send finds the session offline.
    pub fn wake(&self, session_id: i64, store: &Database) -> Result<(), String> {
        let session = store
            .lock()
            .map_err(|_| "Native session store unavailable")?
            .remote_session(session_id)?;
        let Some(session) = session else {
            return Ok(());
        };
        crate::cloud_sessions::ensure_before_send(
            &*self.host,
            &session,
            &std::sync::atomic::AtomicBool::new(false),
            &mut |_| {},
        )
    }
    /// One session's target, `None` for a local session. Never wakes anything.
    pub fn target_of(&self, session_id: i64) -> Option<SessionTarget> {
        self.cloud.target_for_session(session_id)
    }
}

#[cfg(test)]
mod tests;
