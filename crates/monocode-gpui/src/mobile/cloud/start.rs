//! Start options and the remote start's `target` check (H-13, H-11).
//!
//! `cloud.startOptions` answers from data that costs nothing and changes
//! nothing: the project's origin and the local remote-tracking branches (local
//! git, no network), whether `gh` and `sbx` are installed and the sandbox ssh
//! alias exists (a file read), the codespace machine list when an earlier
//! desktop call already cached it, and the fixed sandbox size presets. It
//! never runs `gh` or `sbx`, never creates and never wakes anything, so
//! `ready` means "set up as far as the desktop can tell without calling out":
//! a missing sign-in still shows up as a failed start.
//!
//! `session.start` with a `target` is checked here ([`start_target`]) and run
//! by the shell (`shell/mobile_host/cloud_start.rs`). The client picks from
//! these lists; the kit, template, image, size numbers and time limits are
//! never the client's to set.
use crate::cloud_checks::Sbx;
use crate::settings::cloud_env::{CloudServices, Resolver};
use crate::shell::Shell;
use gpui::{App, Context};
use monocode_core::cloud_sessions::{CloudBackendKind, CloudSessions};
use monocode_core::mobile::cloud_policy::{self, StartInstance, StartKind, StartTarget};
use monocode_core::mobile::policy::{Code, Failure};
use monocode_core::mobile::{DeviceContext, DeviceKind};
use monocode_core::mono::cloud_api::{CloudTarget, InstanceChoice};
use monocode_core::sandboxes::SHAPES;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
mod tests;

/// Most branches one reply lists.
const MAX_BRANCHES: usize = 200;
/// Longest branch name one reply lists.
const MAX_BRANCH_CHARS: usize = 200;

/// A `session.start` that goes to the cloud: the checked `target`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudStartPlan {
    pub target: StartTarget,
}

impl CloudStartPlan {
    /// The backend this start asks for.
    pub(crate) fn kind(&self) -> CloudBackendKind {
        match self.target.kind {
            StartKind::Codespace => CloudBackendKind::Codespaces,
            StartKind::Sandbox => CloudBackendKind::DockerSandbox,
        }
    }

    /// The target the app API's `begin_start` takes. The kit stays on the
    /// desktop's saved preference (`ApiKit::Default` can only narrow it), and
    /// no account is pinned, so the project's own choice applies.
    pub(crate) fn cloud_target(&self) -> Result<CloudTarget, Code> {
        let instance = match &self.target.instance {
            StartInstance::New => InstanceChoice::New,
            StartInstance::Named(name) => InstanceChoice::Named(name.clone()),
        };
        let mut target = CloudTarget::new(self.kind(), instance);
        if let Some(branch) = &self.target.branch {
            target.options.insert("branch".into(), json!(branch));
        }
        if let Some(machine) = &self.target.machine {
            target.options.insert("machine".into(), json!(machine));
        }
        if let Some(size) = &self.target.size {
            let (cpus, memory) = shape_of(size).ok_or(Code::Forbidden)?;
            target.options.insert("cpus".into(), json!(cpus));
            target.options.insert("memory".into(), json!(memory));
        }
        Ok(target)
    }
}

/// The id of a sandbox size preset: `2cpu-4g`.
fn size_id(cpus: u8, memory: &str) -> String {
    format!("{cpus}cpu-{memory}")
}

/// The `(cpus, memory)` of a preset id, only for one the cloud offers.
fn shape_of(id: &str) -> Option<(u8, &'static str)> {
    SHAPES
        .iter()
        .find(|(cpus, memory)| size_id(*cpus, memory) == id)
        .copied()
}

fn listed_branch(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_BRANCH_CHARS
        && !name.starts_with('-')
        && !name.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// What `cloud.startOptions` lists. Plain data, so the checks are unit tested.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartOptions {
    pub codespace_ready: bool,
    pub sandbox_ready: bool,
    pub branches: Vec<String>,
    /// `(id, name)` of the allowed codespace machines.
    pub machines: Vec<(String, String)>,
    /// `(id, name)` of the sandbox presets.
    pub sizes: Vec<(String, String)>,
}

impl StartOptions {
    /// The wire result (`CloudStartOptionsResultSchema`). Empty lists are left
    /// out: the web then offers no choice for them.
    pub(crate) fn to_value(&self) -> Value {
        let mut value = json!({
            "backends": [
                {"kind": "codespace", "ready": self.codespace_ready},
                {"kind": "sandbox", "ready": self.sandbox_ready},
            ],
        });
        let pairs = |list: &[(String, String)]| -> Vec<Value> {
            list.iter()
                .map(|(id, name)| json!({"id": id, "name": name}))
                .collect()
        };
        if !self.branches.is_empty() {
            value["branches"] = json!(self.branches);
        }
        if !self.machines.is_empty() {
            value["machines"] = json!(pairs(&self.machines));
        }
        if !self.sizes.is_empty() {
            value["sizes"] = json!(pairs(&self.sizes));
        }
        value
    }

    /// Checks the values of a start against what was offered. Values outside
    /// the lists are `forbidden`; an instance the project does not use is
    /// `unsupported_target`; options never go with an existing instance.
    pub(crate) fn check(&self, target: &StartTarget, used: &[String]) -> Result<(), Code> {
        match &target.instance {
            StartInstance::Named(name) => {
                if target.branch.is_some() || target.machine.is_some() || target.size.is_some() {
                    return Err(Code::Forbidden);
                }
                if !used.contains(name) {
                    return Err(Code::UnsupportedTarget);
                }
                Ok(())
            }
            StartInstance::New => {
                if target
                    .branch
                    .as_ref()
                    .is_some_and(|branch| !self.branches.contains(branch))
                {
                    return Err(Code::Forbidden);
                }
                if target
                    .machine
                    .as_ref()
                    .is_some_and(|machine| !self.machines.iter().any(|(id, _)| id == machine))
                {
                    return Err(Code::Forbidden);
                }
                if target
                    .size
                    .as_ref()
                    .is_some_and(|size| !self.sizes.iter().any(|(id, _)| id == size))
                {
                    return Err(Code::Forbidden);
                }
                Ok(())
            }
        }
    }
}

/// The sandbox presets, smallest first.
fn size_presets() -> Vec<(String, String)> {
    SHAPES
        .iter()
        .map(|(cpus, memory)| {
            (
                size_id(*cpus, memory),
                crate::cloud_presets::size_label(u32::from(*cpus), memory),
            )
        })
        .collect()
}

/// Everything a read of the options needs, owned so it can run off the UI
/// thread. Building it is cheap and calls nothing.
pub(crate) struct OptionsProbe {
    project: String,
    cloud: Arc<CloudSessions>,
    resolve: Resolver,
    home: Option<PathBuf>,
}

impl OptionsProbe {
    pub(crate) fn new(project: String, cloud: Arc<CloudSessions>, cx: &App) -> Self {
        let services = CloudServices::get(cx);
        Self {
            project,
            cloud,
            resolve: services.resolve,
            home: services.home,
        }
    }

    /// Reads the options. Blocking (a few local git commands): call it from a
    /// worker. Calls no `gh` and no `sbx`, creates and wakes nothing.
    pub(crate) fn run(&self) -> StartOptions {
        // Local git: the project's origin and its remote-tracking branches.
        let repo = self.cloud.repo_for(&self.project).ok();
        let gh_installed = (self.resolve)("gh").is_some();
        let sbx = Sbx::check(&*self.resolve, self.home.as_deref());
        let mut branches = crate::conversation::cloud_branches::local_remote_branches(
            &self.project,
            &crate::conversation::cloud_branches::RepoMatch::Origin,
        )
        .map(|found| found.branches)
        .unwrap_or_default();
        branches.retain(|name| listed_branch(name));
        branches.truncate(MAX_BRANCHES);
        let machines = repo
            .as_deref()
            .and_then(|repo| self.cloud.cached_machines(repo, None))
            .unwrap_or_default()
            .into_iter()
            .map(|machine| {
                let name = if machine.display_name.is_empty() {
                    machine.name.clone()
                } else {
                    machine.display_name
                };
                (machine.name, name)
            })
            .collect();
        StartOptions {
            codespace_ready: repo.is_some() && gh_installed,
            sandbox_ready: sbx.installed && sbx.ssh_set_up,
            branches,
            machines,
            sizes: size_presets(),
        }
    }
}

/// Checks a read of the options and returns what is needed to run it off the
/// UI thread: the device limit, the device's selection and the project.
pub(crate) fn prepare_options(
    shell: &Shell,
    device: &DeviceContext,
    params: &Value,
    cx: &App,
) -> Result<OptionsProbe, Failure> {
    if device.kind != DeviceKind::Web {
        return Err(Code::Unsupported.failure());
    }
    cloud_policy::allow_read(&device.id)?;
    let id = params["projectId"].as_str().unwrap_or_default();
    let project = shell.mobile_start_project(id, device, cx)?;
    Ok(OptionsProbe::new(project, shell.mobile_cloud_service(), cx))
}

/// `cloud.startOptions {projectId}`, answered in place (the host uses
/// [`options_deferred`], which runs the read off the UI thread).
pub(super) fn options(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    cx: &mut Context<Shell>,
) -> Result<Value, Failure> {
    let probe = prepare_options(shell, device, params, cx)?;
    Ok(probe.run().to_value())
}

/// `cloud.startOptions` for the host: the checks run now, the local git reads
/// on a worker, and `finish` gets the answer from there.
pub(super) fn options_deferred(
    shell: &mut Shell,
    device: &DeviceContext,
    params: &Value,
    finish: impl FnOnce(&mut Shell, Result<Value, Failure>) + 'static,
    cx: &mut Context<Shell>,
) {
    let probe = match prepare_options(shell, device, params, cx) {
        Ok(probe) => probe,
        Err(error) => return finish(shell, Err(error)),
    };
    let work = cx
        .background_executor()
        .spawn(async move { probe.run().to_value() });
    cx.spawn(async move |this, cx| {
        let value = work.await;
        let _ = this.update(cx, |this, _| finish(this, Ok(value)));
    })
    .detach();
}

/// Checks the `target` of a session start once its project (`cwd`) is
/// resolved. `Ok(None)` for a local start (no `target`), `Ok(Some(plan))` for a
/// cloud start the shell then runs. Only what needs no outside read is judged
/// here; the values are checked against the options before anything is
/// created.
pub(super) fn start_target(
    shell: &Shell,
    device: &DeviceContext,
    cwd: &str,
    params: &Value,
    _cx: &App,
) -> Result<Option<CloudStartPlan>, Failure> {
    let Some(target) = params.get("target").filter(|target| !target.is_null()) else {
        return Ok(None);
    };
    // Phones keep today's local start; the cloud is the web's.
    if device.kind != DeviceKind::Web {
        return Err(Code::Unsupported.failure());
    }
    let target = cloud_policy::validate_start_target(target).map_err(Code::failure)?;
    // A cloud session is Claude's; it has no draft, worktree, attachment or Mono.
    if params
        .get("harness")
        .is_some_and(|harness| !harness.is_null() && harness.as_str() != Some("claude"))
    {
        return Err(Code::UnsupportedTarget.failure());
    }
    let present = |key: &str| params.get(key).is_some_and(|value| !value.is_null());
    if params["draft"] == true
        || present("worktreePath")
        || present("workspaceMode")
        || present("monoId")
        || params["attachments"]
            .as_array()
            .is_some_and(|files| !files.is_empty())
    {
        return Err(Code::UnsupportedTarget.failure());
    }
    let plan = CloudStartPlan { target };
    if let StartInstance::Named(name) = &plan.target.instance
        && !shell
            .mobile_cloud_instances(cwd, plan.kind())
            .contains(name)
    {
        return Err(Code::UnsupportedTarget.failure());
    }
    Ok(Some(plan))
}
