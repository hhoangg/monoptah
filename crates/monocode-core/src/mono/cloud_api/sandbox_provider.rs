//! Docker Sandboxes behind the cloud app API.
//!
//! A sandbox exists only together with its first session (the clone and provisioning run inside
//! the session start flow), so `cloud.create` is not supported. Sandbox names carry no repo: an
//! instance belongs to a project when one of the project's cloud sessions (or pending start) is
//! bound to it.
use super::provider::{
    current_branch, option_text, CloudInstance, CloudKindSpec, CloudProvider, CloudStart,
};
use crate::cloud_sessions::docker_config::{self, valid_ref};
use crate::cloud_sessions::{CloudBackendKind, CloudRef, CloudSessions, CloudState};
use crate::sandboxes::{self, Prerequisites};
use crate::store::{RemoteSession, Store};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Static descriptor of the sandbox backend.
pub struct SandboxSpec;

impl CloudKindSpec for SandboxSpec {
    fn kind(&self) -> CloudBackendKind {
        CloudBackendKind::DockerSandbox
    }
    fn instance_name(&self, name: &str) -> Result<(), String> {
        sandboxes::validate_sandbox_name(name).or_else(|name_error| {
            if name.starts_with("sbx_") {
                sandboxes::validate_sandbox_id(name)
            } else {
                Err(name_error)
            }
        })
    }
    fn start_options(&self) -> &'static [&'static str] {
        &["branch", "cpus", "memory", "ttl"]
    }
    fn resume_without_session_advice(&self, _: &str) -> String {
        // sessions.start on an existing sandbox needs a ready session, so it cannot help here.
        "Its sessions have not finished starting or failed to start: use Retry on one of them in the app.".into()
    }
    fn check_start_option(&self, key: &str, value: &Value) -> Result<(), String> {
        let bad = || format!("target.{key} is not a valid {key}");
        match key {
            "branch" => value
                .as_str()
                .filter(|b| valid_ref(b))
                .map(|_| ())
                .ok_or_else(bad),
            "cpus" => {
                let cpus = value
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(bad)?;
                docker_config::check_cpus(cpus).map_err(|e| format!("target.cpus: {e}"))
            }
            "memory" => docker_config::check_memory(&lowered(value).ok_or_else(bad)?)
                .map_err(|e| format!("target.memory: {e}")),
            "ttl" => docker_config::check_ttl(&lowered(value).ok_or_else(bad)?)
                .map_err(|e| format!("target.ttl: {e}")),
            _ => Err(bad()),
        }
    }
}

fn lowered(value: &Value) -> Option<String> {
    value.as_str().map(|s| s.trim().to_lowercase())
}

/// The Docker accounts to ask (`None` is the machine login) with the bound sandboxes of each.
type Asked = Vec<(Option<String>, Vec<String>)>;

/// What `sbx ls` answered across accounts.
#[derive(Default)]
struct Listing {
    listed: Vec<sandboxes::SandboxInfo>,
    /// Accounts that could not be listed: the bound sandboxes in them and why.
    failed: Vec<(Vec<String>, String)>,
}
impl Listing {
    /// Every failure, one text.
    fn errors(&self) -> String {
        let mut texts: Vec<&str> = self.failed.iter().map(|(_, e)| e.as_str()).collect();
        texts.dedup();
        texts.join("; ")
    }
}

pub struct SandboxProvider {
    cloud: Arc<CloudSessions>,
    store: Arc<Mutex<Store>>,
}

impl SandboxProvider {
    pub fn new(cloud: Arc<CloudSessions>, store: Arc<Mutex<Store>>) -> Self {
        Self { cloud, store }
    }
    /// Names of the sandboxes the project's cloud sessions (pending starts included) use.
    fn bound_names(&self, project: &str) -> HashSet<String> {
        self.cloud
            .cloud_sessions_for(project)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.cloud.kind == CloudBackendKind::DockerSandbox)
            .map(|s| s.cloud.name)
            .collect()
    }
    /// The Docker accounts to ask when listing the project's sandboxes, each with the bound
    /// sandboxes that live in it. A sandbox lives in the account that created it, so each
    /// bound sandbox adds ITS account (which may differ from the project's current choice, or
    /// from each other). With nothing bound it is the project's own account, with no names.
    /// Without any account this is one `None` entry: one `sbx ls` as the machine login, as
    /// before. `Err` when the account of a sandbox could not be read (never taken for the
    /// machine login, which would report the sandbox as gone).
    fn accounts_to_ask(&self, project: &str, bound: &HashSet<String>) -> Result<Asked, String> {
        let mut asked: Asked = Vec::new();
        let mut names: Vec<&String> = bound.iter().collect();
        names.sort();
        for name in names {
            let account = self.cloud.try_sandbox_account(name)?;
            match asked.iter_mut().find(|(known, _)| *known == account) {
                Some((_, bound)) => bound.push(name.clone()),
                None => asked.push((account, vec![name.clone()])),
            }
        }
        if asked.is_empty() {
            asked.push((
                self.config(project)
                    .ok()
                    .flatten()
                    .and_then(|config| config.docker_account),
                Vec::new(),
            ));
        }
        asked.sort();
        Ok(asked)
    }
    /// `sbx ls` once per account in `asked`. The sandboxes of the accounts that answered are
    /// returned together with the accounts that did not, each with the error and the bound
    /// sandboxes in it. A failed account is NOT "nothing listed": its sandboxes are unknown,
    /// never gone.
    fn list_as(&self, asked: &Asked) -> Result<Listing, String> {
        let (runner, _) = self.cloud.sandbox_tools()?;
        let mut listing = Listing::default();
        for (account, names) in asked {
            match sandboxes::list(&sandboxes::as_account(&*runner, account.as_deref())) {
                Ok(found) => {
                    for info in found {
                        // A sandbox is listed by the one account it lives in; never twice.
                        if !listing
                            .listed
                            .iter()
                            .any(|seen| seen.id == info.id && seen.name == info.name)
                        {
                            listing.listed.push(info);
                        }
                    }
                }
                Err(error) => listing.failed.push((names.clone(), error)),
            }
        }
        Ok(listing)
    }
    fn config(&self, project: &str) -> Result<Option<docker_config::DockerProjectConfig>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "Native session store unavailable")?;
        docker_config::load(&store, project)
    }
}

/// `4096` MiB as `4g`, `512` as `512m`.
fn memory_text(mib: u64) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) {
        format!("{}g", mib / 1024)
    } else {
        format!("{mib}m")
    }
}

impl CloudProvider for SandboxProvider {
    fn spec(&self) -> &'static dyn CloudKindSpec {
        &SandboxSpec
    }
    fn relevant(&self, project: &str) -> bool {
        self.cloud
            .project_uses(project, CloudBackendKind::DockerSandbox)
            || self.config(project).is_ok_and(|config| config.is_some())
    }
    fn setup_hint(&self) -> &'static str {
        "sandbox: set the project up for Docker Sandbox in the app"
    }

    fn list(&self, project: &str) -> Result<Vec<CloudInstance>, String> {
        let bound = self.bound_names(project);
        let listing = self.list_as(&self.accounts_to_ask(project, &bound)?)?;
        // Nothing could be listed: that is an error, not an empty list.
        if listing.listed.is_empty() && !listing.failed.is_empty() {
            return Err(listing.errors());
        }
        let mut instances: Vec<CloudInstance> = listing
            .listed
            .into_iter()
            .filter(|info| bound.contains(&info.name))
            .map(|info| {
                let mut details = Map::new();
                if info.cpus > 0 {
                    details.insert("cpus".into(), json!(info.cpus));
                }
                if info.memory_mib > 0 {
                    details.insert("memory".into(), json!(memory_text(info.memory_mib)));
                }
                if let Some(expires) = info.expires_at.clone().filter(|e| !e.is_empty()) {
                    details.insert("expiresAt".into(), json!(expires));
                }
                CloudInstance {
                    cloud: CloudRef::sandbox(&info.name),
                    display_name: info.name.clone(),
                    state: sandboxes::display_state(&info.state),
                    raw_state: info.state,
                    last_used_at: None,
                    details,
                }
            })
            .collect();
        // The sandboxes of an account that could not be listed are reported as unknown with
        // the reason, never left out as if they were gone.
        for (names, error) in &listing.failed {
            for name in names {
                if instances.iter().any(|i| i.cloud.name == *name) {
                    continue;
                }
                let mut details = Map::new();
                details.insert("error".into(), json!(error));
                instances.push(CloudInstance {
                    cloud: CloudRef::sandbox(name),
                    display_name: name.clone(),
                    state: CloudState::Unknown,
                    raw_state: "unknown".into(),
                    last_used_at: None,
                    details,
                });
            }
        }
        Ok(instances)
    }

    /// `{ready, installed, loggedIn, sshConfigured, message?, fixCommand?, configured,
    /// repoUrl?}`. `loggedIn` is null when sbx could not be asked.
    fn status(&self, project: &str) -> Result<Value, String> {
        let (runner, home) = self.cloud.sandbox_tools()?;
        let config = self.config(project);
        // As the project's Docker account when it names one.
        let account = match &config {
            Ok(Some(config)) => config.docker_account.as_deref(),
            _ => None,
        };
        let prerequisites = sandboxes::diagnose_as(&*runner, &home, account);
        let (installed, logged_in) = match &prerequisites {
            Prerequisites::SbxMissing => (false, json!(false)),
            Prerequisites::NotLoggedIn(_) | Prerequisites::DockerAccount(_) => (true, json!(false)),
            Prerequisites::Unreachable(_) => (true, Value::Null),
            Prerequisites::SshAliasMissing | Prerequisites::Ready => (true, json!(true)),
        };
        let mut result = json!({
            "ready":prerequisites.is_ready(),
            "installed":installed,
            "loggedIn":logged_in,
            "sshConfigured":sandboxes::ssh_alias_configured(&home),
            "configured":matches!(config, Ok(Some(_))),
        });
        if !prerequisites.is_ready() {
            result["message"] = json!(prerequisites.message());
        }
        if let Some(command) = prerequisites.fix_command() {
            result["fixCommand"] = json!(command);
        }
        match config {
            Ok(Some(config)) => result["repoUrl"] = json!(config.repo_url),
            Ok(None) => {}
            Err(error) => result["configError"] = json!(error),
        }
        Ok(result)
    }

    /// An instance is the project's when one of the project's cloud sessions is bound to it. A
    /// sandbox id is mapped to its name through `sbx ls`.
    fn resolve_instance(&self, project: &str, instance: &str) -> Result<String, String> {
        let bound = self.bound_names(project);
        let name = if sandboxes::validate_sandbox_id(instance).is_ok() {
            let listing = self.list_as(&self.accounts_to_ask(project, &bound)?)?;
            match listing.listed.iter().find(|info| info.id == instance) {
                Some(info) => info.name.clone(),
                // Not found among the accounts that answered, but some did not: it may live
                // in one of those, so do not say it does not exist.
                None if !listing.failed.is_empty() => {
                    return Err(format!(
                        "Sandbox {instance} could not be looked up: {}",
                        listing.errors()
                    ))
                }
                None => return Err(format!("Sandbox {instance} was not found; run cloud.list")),
            }
        } else {
            instance.to_owned()
        };
        if bound.contains(&name) {
            Ok(name)
        } else {
            Err(format!(
                "Sandbox {name} is not used by a session of this project; run cloud.list"
            ))
        }
    }

    /// New: the project's Docker config (with optional `cpus`, `memory`, `ttl` overrides) and
    /// `branch` (default: the current branch) through `begin_sandbox_start`. Existing: another
    /// ready session on the sandbox, see `CloudSessions::attach_session`.
    fn start(
        &self,
        project: &str,
        harness: &str,
        instance: Option<&str>,
        options: &Map<String, Value>,
        kit: bool,
    ) -> Result<CloudStart, String> {
        if let Some(instance) = instance {
            if !options.is_empty() {
                return Err("Options only apply when creating a sandbox".into());
            }
            let cloud = CloudRef::sandbox(self.resolve_instance(project, instance)?);
            let session = self.cloud.attach_session(project, &cloud, harness)?;
            let state = self.cloud.cloud_state_of(&cloud);
            return Ok(CloudStart::Attached(session, cloud, state));
        }
        let mut config = self.config(project)?.ok_or(
            "Docker Sandbox is not set up for this project. Set it up in the project's Docker Sandbox settings first.",
        )?;
        if let Some(cpus) = options.get("cpus").and_then(Value::as_u64) {
            config.cpus = u32::try_from(cpus).map_err(|_| "cpus is too large")?;
        }
        if let Some(memory) = option_text(options, "memory") {
            config.memory = memory.to_owned();
        }
        if let Some(ttl) = option_text(options, "ttl") {
            config.ttl = ttl.to_owned();
        }
        let config = config.validated()?;
        let branch = match option_text(options, "branch") {
            Some(branch) => branch.to_owned(),
            None => current_branch(project, "sandbox")?,
        };
        let (session, record) = self
            .cloud
            .begin_sandbox_start_with_kit(project, &config, &branch, harness, kit)?;
        Ok(CloudStart::Pending(session, record))
    }

    /// Reconnects the sandbox (resuming it when stopped) through the session. BLOCKS for
    /// minutes.
    fn wake(&self, session: &RemoteSession) -> Result<(), String> {
        self.cloud
            .reconnect_sandbox(session.summary.id, &mut |_| {}, &AtomicBool::new(false))
    }
}
