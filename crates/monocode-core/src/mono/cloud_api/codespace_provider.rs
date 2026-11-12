//! GitHub Codespaces behind the cloud app API.
use super::provider::{
    current_branch, option_text, CloudInstance, CloudKindSpec, CloudProvider, CloudStart,
};
use crate::cloud_sessions::{
    ensure_before_send, CloudBackendKind, CloudBinding, CloudRef, CloudSessions, CloudState,
    CodespaceChoice, Host, Target,
};
use crate::codespaces::{self, GhRunner};
use crate::store::RemoteSession;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Valid codespace names are letters, digits and hyphens (they become `gh` argv).
fn codespace_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn branch_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 250
        && !value.starts_with('-')
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
}
fn machine_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Static descriptor of the codespace backend.
pub struct CodespaceSpec;

impl CloudKindSpec for CodespaceSpec {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::Codespaces
    }
    fn instance_name(&self, name: &str) -> Result<(), String> {
        if codespace_name(name) {
            Ok(())
        } else {
            Err("a codespace name has letters, digits and hyphens only".into())
        }
    }
    fn start_options(&self) -> &'static [&'static str] {
        &["branch", "machine"]
    }
    fn resume_without_session_advice(&self, name: &str) -> String {
        format!(
            "Use sessions.start with target {{\"kind\":\"codespace\",\"instance\":\"{name}\"}} to reconnect through a new session."
        )
    }
    fn check_start_option(&self, key: &str, value: &Value) -> Result<(), String> {
        self.check_create_option(key, value)
            .map_err(|_| format!("target.{key} is not a valid {key} name"))
    }
    fn supports_standalone_create(&self) -> bool {
        true
    }
    fn create_options(&self) -> &'static [&'static str] {
        &["branch", "machine", "displayName"]
    }
    fn check_create_option(&self, key: &str, value: &Value) -> Result<(), String> {
        let text = value.as_str();
        match key {
            "branch" if !text.is_some_and(branch_name) => {
                Err("branch is not a valid branch name".into())
            }
            "machine" if !text.is_some_and(machine_name) => {
                Err("machine is not a valid machine type name".into())
            }
            "displayName"
                if text.is_none_or(|d| {
                    d.trim().is_empty() || d.chars().count() > 48 || d.chars().any(char::is_control)
                }) =>
            {
                Err("displayName must be 1 to 48 characters".into())
            }
            _ => Ok(()),
        }
    }
}

pub struct CodespaceProvider {
    gh: Arc<dyn GhRunner>,
    host: Arc<dyn Host>,
    cloud: Arc<CloudSessions>,
}

impl CodespaceProvider {
    pub fn new(gh: Arc<dyn GhRunner>, host: Arc<dyn Host>, cloud: Arc<CloudSessions>) -> Self {
        Self { gh, host, cloud }
    }
    /// The project's repository, after gh is known to be signed in with the codespace scope.
    fn scoped_repo(&self, project: &str) -> Result<String, String> {
        codespaces::auth_status(&*self.gh)?.require_codespace_scope()?;
        self.cloud.repo_for(project)
    }
    /// The requested branch, else the project's cloud settings base branch, else the branch
    /// checked out in the project folder.
    fn default_branch(&self, project: &str, requested: Option<&str>) -> Result<String, String> {
        if let Some(branch) = requested {
            return Ok(branch.to_owned());
        }
        let base = self.cloud.codespace_project_config(project).base_ref;
        if base.is_empty() {
            current_branch(project, "codespace")
        } else {
            Ok(base)
        }
    }
    fn target(
        &self,
        project: &str,
        instance: Option<&str>,
        options: &Map<String, Value>,
    ) -> Result<Target, String> {
        match instance {
            Some(name) => {
                if !options.is_empty() {
                    return Err("Options only apply when creating a codespace".into());
                }
                Ok(Target {
                    codespace: CodespaceChoice::Named(name.into()),
                    branch: None,
                    machine: None,
                    idle_timeout_minutes: None,
                    devcontainer_path: None,
                    claude_account: None,
                    claude_account_pinned: false,
                    work_branch: None,
                })
            }
            None => Ok(Target {
                codespace: CodespaceChoice::New,
                branch: Some(self.default_branch(project, option_text(options, "branch"))?),
                machine: option_text(options, "machine").map(str::to_owned),
                idle_timeout_minutes: None,
                devcontainer_path: None,
                claude_account: None,
                claude_account_pinned: false,
                work_branch: None,
            }),
        }
    }
}

impl CloudProvider for CodespaceProvider {
    fn spec(&self) -> &'static dyn CloudKindSpec {
        &CodespaceSpec
    }
    fn relevant(&self, project: &str) -> bool {
        self.cloud
            .project_uses(project, CloudBackendKind::Codespaces)
            || self.cloud.repo_for(project).is_ok()
    }
    fn setup_hint(&self) -> &'static str {
        "codespace: give the project a GitHub origin"
    }

    fn list(&self, project: &str) -> Result<Vec<CloudInstance>, String> {
        let repo = self.scoped_repo(project)?;
        Ok(codespaces::list(&*self.gh, &repo)?
            .into_iter()
            .map(|c| {
                let mut details = Map::new();
                if let Some(branch) = c.git_status.map(|g| g.git_ref).filter(|b| !b.is_empty()) {
                    details.insert("branch".into(), json!(branch));
                }
                if !c.machine_name.is_empty() {
                    details.insert("machine".into(), json!(c.machine_name));
                }
                CloudInstance {
                    cloud: CloudRef::codespace(&c.name),
                    display_name: if c.display_name.is_empty() {
                        c.name.clone()
                    } else {
                        c.display_name.clone()
                    },
                    state: CloudState::from_gh(&c.state),
                    raw_state: c.state,
                    last_used_at: Some(c.last_used_at).filter(|t| !t.is_empty()),
                    details,
                }
            })
            .collect())
    }

    /// gh auth, the secrets the project's devcontainer asks for and `CLAUDE_CODE_OAUTH_TOKEN`,
    /// each as set / missing / notForRepo (never values), plus `ready`.
    fn status(&self, project: &str) -> Result<Value, String> {
        let auth = codespaces::auth_status(&*self.gh)?;
        let repo = self.cloud.repo_for(project).ok();
        let wanted = codespaces::devcontainer_secrets(Path::new(project));
        let mut result = json!({
            "ready":false,
            "auth":auth,
            "repo":repo,
            "devcontainerSecrets":wanted,
            "secrets":[],
        });
        // A project that chooses an app Claude account does not need the Codespaces secret:
        // the account's credential is delivered to the codespace's Host at connect time.
        let account = self.cloud.codespace_project_config(project).claude_account;
        if let Some(account) = &account {
            result["claudeAccount"] = json!(self.cloud.claude_account_label(account));
            result["secretRequired"] = json!(false);
        }
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
        let mut secrets = json!(codespaces::secrets_status(&*self.gh, &repo, &keys)?);
        if account.is_some() {
            for secret in secrets.as_array_mut().into_iter().flatten() {
                if secret["key"] == "CLAUDE_CODE_OAUTH_TOKEN" {
                    secret["optional"] = json!(true);
                }
            }
        }
        result["secrets"] = secrets;
        result["ready"] = json!(true);
        Ok(result)
    }

    /// Creates a codespace and waits until it is available.
    fn create(
        &self,
        project: &str,
        input: &Value,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Value, String> {
        let repo = self.scoped_repo(project)?;
        let text = |key: &str| input.get(key).and_then(Value::as_str);
        let branch = self.default_branch(project, text("branch"))?;
        let config = self.cloud.codespace_project_config(project);
        // A machine named in the request is used as is. None named: the project's saved machine
        // while the repository allows it on that branch, else the smallest allowed machine.
        let machine = match text("machine").map(str::trim).filter(|m| !m.is_empty()) {
            Some(machine) => machine.to_owned(),
            None => self
                .cloud
                .pick_machine(&repo, Some(&branch), config.machine.as_deref()),
        };
        let name = codespaces::create(
            &*self.gh,
            &repo,
            &branch,
            &codespaces::CreateOptions {
                machine: Some(&machine),
                idle_timeout_minutes: Some(config.idle_timeout_minutes),
                devcontainer_path: Some(config.devcontainer_path.as_str()),
            },
            text("displayName"),
            progress,
        )?;
        codespaces::wait_ready(&*self.gh, &name)?;
        self.cloud.invalidate(&name);
        Ok(json!({
            "kind":CloudBackendKind::Codespaces.api_name(),
            "instance":name,
            "repo":repo,
            "branch":branch,
            "state":"available",
        }))
    }

    /// Refuses names that are not codespaces of the project's repository.
    fn resolve_instance(&self, project: &str, instance: &str) -> Result<String, String> {
        let repo = self.scoped_repo(project)?;
        if codespaces::list(&*self.gh, &repo)?
            .iter()
            .any(|c| c.name == instance)
        {
            Ok(instance.to_owned())
        } else {
            Err(format!(
                "Codespace {instance} is not a codespace of {repo}; run cloud.list"
            ))
        }
    }

    /// A new codespace is requested, or an existing one connected (even while stopped); the
    /// session is pending either way.
    fn start(
        &self,
        project: &str,
        harness: &str,
        instance: Option<&str>,
        options: &Map<String, Value>,
        kit: bool,
        account: Option<&str>,
    ) -> Result<CloudStart, String> {
        let mut target = self.target(project, instance, options)?;
        if let Some(account) = account {
            // The account belongs to the instance: only a new one takes it.
            if instance.is_some() {
                return Err(super::super::api::ACCOUNT_EXISTING_INSTANCE.into());
            }
            target.claude_account = Some(account.to_owned());
            target.claude_account_pinned = true;
        }
        let (session, record) = self
            .cloud
            .begin_start_with_kit(project, target, harness, kit)?;
        Ok(CloudStart::Pending(session, record))
    }

    /// A codespace created here is never deleted on failure; the error names it so the caller
    /// retries with `instance:"<name>"` instead of creating a second one.
    fn prepare(
        &self,
        project: &str,
        instance: Option<&str>,
        options: &Map<String, Value>,
        progress: &mut dyn FnMut(&str),
    ) -> Result<CloudBinding, String> {
        let target = self.target(project, instance, options)?;
        let binding = self.cloud.prepare_target(project, target, progress)?;
        // The state cache may still say Shutdown/Starting for a reused codespace.
        self.cloud.invalidate(&binding.codespace);
        Ok(binding)
    }

    fn wake(&self, session: &RemoteSession) -> Result<(), String> {
        ensure_before_send(&*self.host, session, &AtomicBool::new(false), &mut |_| {})
    }
}
