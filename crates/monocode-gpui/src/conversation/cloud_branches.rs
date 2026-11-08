//! The branches offered by the cloud panel's Base branch (Docker Sandbox) and Branch
//! (Codespace) rows. The session twins a local project, so the list comes from that checkout's
//! remote-tracking branches of the remote that is the session's repository: the one whose URL
//! matches the saved Docker config's `repo_url`, or `origin` for a codespace. Reading them is
//! local git only (no network) and runs off the UI thread. A `git ls-remote --heads` refresh
//! follows when git can reach the remote without asking anything (no terminal prompt, a short
//! timeout); if it fails the local list stays, silently. When no local remote matches the
//! repository the panel keeps its free-text field. Nothing here is written to the project.
use super::cloud_target::*;
use super::*;
use monocode_core::cloud_sessions::docker_config;
use monocode_core::git_credentials::normalize_origin;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long `git ls-remote` may take before the local list is kept as it is.
const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(10);

/// Which local remote is the session's repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RepoMatch {
    /// The remote whose URL is this one (Docker: the saved config's `repo_url`).
    Url(String),
    /// The remote named `origin` (Codespace).
    Origin,
}

/// The remote-tracking branches of one remote, newest commit first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteBranches {
    pub remote: String,
    /// The matched remote's URL, what `ls-remote` is pointed at (never the name).
    pub url: String,
    pub branches: Vec<String>,
}

/// Reads the local remote-tracking branches of `project` (blocking, no network). `None` when no
/// remote matches.
pub(crate) type LocalBranches =
    Arc<dyn Fn(&str, &RepoMatch) -> Option<RemoteBranches> + Send + Sync>;
/// Lists the branch names the repository at a URL has, `git ls-remote --heads` (blocking, network).
pub(crate) type RemoteHeads = Arc<dyn Fn(&str, &str) -> Result<Vec<String>, String> + Send + Sync>;

/// Where the panel gets its branches. The default runs git; tests hand in fakes.
#[derive(Clone)]
pub(crate) struct BranchSource {
    pub local: LocalBranches,
    pub heads: RemoteHeads,
}
impl Default for BranchSource {
    fn default() -> Self {
        Self {
            local: Arc::new(local_remote_branches),
            heads: Arc::new(remote_heads),
        }
    }
}

/// What a draft knows about the branches of its repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BranchList {
    /// Being read: the free-text field stays meanwhile.
    Loading,
    /// A local remote is the repository: the row opens a picker over `branches`.
    Ready {
        remote: String,
        branches: Vec<String>,
    },
    /// No local remote is the repository (or git could not be read): free text.
    Unmatched,
}

/// A row of the panel that picks a branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudRefField {
    /// Docker: the base branch.
    Base,
    /// Codespace: the branch the codespace is created on.
    Branch,
}
impl CloudRefField {
    pub(super) fn index(self) -> usize {
        self as usize
    }
}

/// The open branch picker of a panel row.
pub(super) struct CloudRefPicker {
    pub field: CloudRefField,
    pub picker: Entity<GitPicker>,
    pub _events: Subscription,
}

/// A typed codespace branch: there is no validator for it, but it must not read as an option.
fn codespace_branch_ok(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('-') && !name.contains(char::is_whitespace)
}

/// `host/owner/repo`, lowercase and without `.git`, of an https, ssh or scp-style git URL.
pub(crate) fn repo_key(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest.to_owned(),
        // scp-like `git@github.com:owner/repo.git`
        None => {
            let (host, path) = url.split_once(':')?;
            if host.contains('/') {
                return None;
            }
            format!("{host}/{path}")
        }
    };
    let rest = rest
        .rsplit_once('@')
        .map_or(rest.as_str(), |(_, rest)| rest);
    let (host, path) = rest.split_once('/')?;
    let host = host.split(':').next().unwrap_or(host);
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && !path.is_empty()).then(|| format!("{host}/{path}").to_lowercase())
}

/// The key a Docker config's `repo_url` has, through the same normalisation the config gets.
fn config_key(repo_url: &str) -> Option<String> {
    let (_, origin, path) = normalize_origin(repo_url).ok()?;
    repo_key(&format!("{origin}/{path}"))
}

/// Picks the remote for `want` among `(name, url)` pairs: `origin` first when several match.
pub(super) fn pick_remote(remotes: &[(String, String)], want: &RepoMatch) -> Option<String> {
    // A name that could be read as an option never reaches git's argv.
    let remotes: Vec<(String, String)> = remotes
        .iter()
        .filter(|(name, _)| !name.starts_with('-'))
        .cloned()
        .collect();
    let remotes = remotes.as_slice();
    match want {
        RepoMatch::Origin => remotes
            .iter()
            .any(|(name, _)| name == "origin")
            .then(|| "origin".to_owned()),
        RepoMatch::Url(url) => {
            let key = config_key(url)?;
            let matching: Vec<&str> = remotes
                .iter()
                .filter(|(_, remote)| repo_key(remote).as_deref() == Some(key.as_str()))
                .map(|(name, _)| name.as_str())
                .collect();
            // `origin` wins a tie when it is one of the matches.
            matching
                .iter()
                .find(|name| **name == "origin")
                .or(matching.first())
                .map(|name| (*name).to_owned())
        }
    }
}

/// `remote.<name>.url\n<url>\0` entries of `git config -z --get-regexp`, so names with spaces
/// work.
pub(super) fn parse_remotes(config: &str) -> Vec<(String, String)> {
    config
        .split('\0')
        .filter_map(|entry| {
            let (key, url) = entry.split_once('\n')?;
            let name = key.strip_prefix("remote.")?.strip_suffix(".url")?;
            (!name.is_empty()).then(|| (name.to_owned(), url.trim().to_owned()))
        })
        .collect()
}

/// `refs/remotes/<remote>/<name>` lines to names, without the remote's `HEAD`.
fn parse_tracking(remote: &str, refs: &str) -> Vec<String> {
    let prefix = format!("refs/remotes/{remote}/");
    refs.lines()
        .filter_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .filter(|name| !name.is_empty() && *name != "HEAD")
        .map(str::to_owned)
        .collect()
}

/// `<sha>\trefs/heads/<name>` lines of `git ls-remote --heads`.
pub(super) fn parse_heads(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once('\t')?.1.trim().strip_prefix("refs/heads/"))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The local list, as far as the remote still has the branches, then the ones only it has.
pub(crate) fn merge_heads(local: &[String], heads: Vec<String>) -> Vec<String> {
    let mut extra = heads.clone();
    extra.sort();
    let mut merged: Vec<String> = local
        .iter()
        .filter(|name| heads.contains(name))
        .cloned()
        .collect();
    merged.extend(extra.into_iter().filter(|name| !local.contains(name)));
    merged
}

fn git_command(cwd: &str, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null());
    monocode_core::process::configure_child(&mut command);
    command
}

fn git_output(cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = git_command(cwd, args)
        .output()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

/// The real [`LocalBranches`]: `git config --get-regexp` for the remotes, then `for-each-ref`
/// over the matching one's tracking branches.
fn local_remote_branches(project: &str, want: &RepoMatch) -> Option<RemoteBranches> {
    let config = git_output(
        project,
        &["config", "-z", "--get-regexp", r"^remote\..*\.url$"],
    )
    .ok()?;
    let remotes = parse_remotes(&config);
    let remote = pick_remote(&remotes, want)?;
    let url = remotes
        .iter()
        .find(|(name, _)| *name == remote)
        .map(|(_, url)| url.clone())?;
    let pattern = format!("refs/remotes/{remote}/");
    let refs = git_output(
        project,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)",
            &pattern,
        ],
    )
    .ok()?;
    Some(RemoteBranches {
        branches: parse_tracking(&remote, &refs),
        remote,
        url,
    })
}

/// The real [`RemoteHeads`]: `git ls-remote --heads -- <url>` that can never ask anything. No
/// terminal prompt, no controlling tty (so ssh cannot open /dev/tty), no askpass program, ssh in
/// batch mode unless the user configured their own ssh command, and a timeout. It works only
/// where git's credential helper or an ssh agent answers by itself.
fn remote_heads(project: &str, url: &str) -> Result<Vec<String>, String> {
    let mut command = git_command(project, &["ls-remote", "--heads", "--", url]);
    command.env("SSH_ASKPASS_REQUIRE", "never");
    let own_ssh = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || git_output(project, &["config", "--get", "core.sshCommand"])
            .is_ok_and(|value| !value.trim().is_empty());
    if !own_ssh {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let text = run_with_timeout(command, LS_REMOTE_TIMEOUT)?;
    Ok(parse_heads(&text))
}

/// Runs `command` in its own session and process group and returns its stdout. When it outlives
/// `timeout` the whole group is killed, so helpers it started (ssh, git-remote-https) do not
/// linger.
pub(super) fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<String, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no Rust state in the forked child.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not run Git: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("No output from Git")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                #[cfg(unix)]
                // SAFETY: the child called setsid, so its pid is its process group id.
                unsafe {
                    libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("git ls-remote timed out".into());
            }
        }
    };
    let text = reader.join().map_err(|_| "git ls-remote failed")?;
    if status.success() {
        Ok(text)
    } else {
        Err("git ls-remote failed".into())
    }
}

impl Conversation {
    /// Replaces the branch source (tests hand in fakes).
    #[cfg(test)]
    pub(crate) fn set_branch_source(&mut self, source: BranchSource) {
        self.branch_source = source;
    }

    /// Reads the repository's branches off the UI thread: the local tracking branches first,
    /// then the `ls-remote` refresh. The answers only apply to the draft that asked.
    pub(super) fn load_cloud_branches(&mut self, request: u64, cx: &mut Context<Self>) {
        let Some(draft) = &self.cloud_draft else {
            return;
        };
        let want = match (draft.target, &draft.config) {
            (CloudTarget::Codespace, _) => RepoMatch::Origin,
            (CloudTarget::DockerSandbox, Some(config)) => RepoMatch::Url(config.repo_url.clone()),
            (CloudTarget::DockerSandbox, None) => {
                if let Some(draft) = &mut self.cloud_draft {
                    draft.branches = BranchList::Unmatched;
                }
                return;
            }
        };
        let project = self.summary.cwd.clone();
        let source = self.branch_source.clone();
        cx.spawn(async move |this, cx| {
            let local = {
                let (source, project) = (source.clone(), project.clone());
                cx.background_executor()
                    .spawn(async move { (source.local)(&project, &want) })
                    .await
            };
            let Some(local) = local else {
                let _ = this.update(cx, |this, cx| this.apply_local_branches(request, None, cx));
                return;
            };
            let (remote, url, tracked) = (
                local.remote.clone(),
                local.url.clone(),
                local.branches.clone(),
            );
            let applied = this.update(cx, |this, cx| {
                this.apply_local_branches(request, Some(local), cx)
            });
            if !applied.unwrap_or(false) {
                return;
            }
            let heads = cx
                .background_executor()
                .spawn(async move { (source.heads)(&project, &url) })
                .await;
            // A failed refresh keeps the local list, silently.
            let Ok(heads) = heads else {
                return;
            };
            let merged = merge_heads(&tracked, heads);
            let _ = this.update(cx, |this, cx| {
                this.apply_refreshed_branches(request, remote, merged, cx)
            });
        })
        .detach();
    }

    /// The local read finished. Applies only to the draft that asked; returns whether it did and
    /// found a repository.
    pub(super) fn apply_local_branches(
        &mut self,
        request: u64,
        local: Option<RemoteBranches>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(draft) = self
            .cloud_draft
            .as_mut()
            .filter(|draft| draft.branches_request == request)
        else {
            return false;
        };
        let found = local.is_some();
        draft.branches = match local {
            Some(local) => BranchList::Ready {
                remote: local.remote,
                branches: local.branches,
            },
            None => BranchList::Unmatched,
        };
        cx.notify();
        found
    }

    /// The `ls-remote` refresh finished; the open picker follows. Ignored for another draft.
    pub(super) fn apply_refreshed_branches(
        &mut self,
        request: u64,
        remote: String,
        branches: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self
            .cloud_draft
            .as_mut()
            .filter(|draft| draft.branches_request == request)
        else {
            return;
        };
        draft.branches = BranchList::Ready {
            remote,
            branches: branches.clone(),
        };
        if let Some(open) = &self.cloud_picker {
            let names = self.cloud_ref_names(&branches);
            open.picker
                .update(cx, |picker, cx| picker.set_refs(names, cx));
        }
        cx.notify();
    }

    /// Whether `field` picks from the repository's branches rather than taking free text.
    pub(super) fn cloud_picks(&self, field: CloudRefField) -> bool {
        self.cloud_draft.as_ref().is_some_and(|draft| {
            matches!(draft.branches, BranchList::Ready { .. })
                && match field {
                    CloudRefField::Base => draft.target == CloudTarget::DockerSandbox,
                    CloudRefField::Branch => draft.target == CloudTarget::Codespace,
                }
        })
    }

    /// The text field behind a picking row.
    fn cloud_ref_input(&self, field: CloudRefField) -> Option<Entity<input::Input>> {
        self.cloud_draft.as_ref().map(|draft| match field {
            CloudRefField::Base => draft.base.clone(),
            CloudRefField::Branch => draft.branch.clone(),
        })
    }

    /// The value shown on a picking row.
    pub(super) fn cloud_ref_value(&self, field: CloudRefField, cx: &gpui::App) -> String {
        let text = self
            .cloud_ref_input(field)
            .map(|input| input.read(cx).content.trim().to_owned())
            .unwrap_or_default();
        match (field, text.is_empty()) {
            (CloudRefField::Base, true) => "Default branch".into(),
            (CloudRefField::Branch, true) => "Choose a branch".into(),
            _ => text,
        }
    }

    /// The names the picker offers: Docker only takes refs the config validation accepts.
    fn cloud_ref_names(&self, branches: &[String]) -> Vec<String> {
        let docker = self.cloud_target() == Some(CloudTarget::DockerSandbox);
        branches
            .iter()
            .filter(|name| !docker || docker_config::valid_ref(name))
            .cloned()
            .collect()
    }

    pub(super) fn cloud_ref_menu_open(&self, field: CloudRefField) -> bool {
        self.cloud_picker
            .as_ref()
            .is_some_and(|open| open.field == field)
    }

    /// The row was pressed: opens the branch picker anchored to it, or closes it.
    pub(super) fn toggle_cloud_ref_picker(
        &mut self,
        field: CloudRefField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_creating() {
            return;
        }
        let was_open = self.cloud_ref_menu_open(field);
        self.close_cloud_picker();
        if was_open {
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
            return;
        }
        let Some(draft) = &self.cloud_draft else {
            return;
        };
        let BranchList::Ready { remote, branches } = draft.branches.clone() else {
            return;
        };
        self.menu = None;
        self.git_picker = None;
        self.git_events = None;
        let names = self.cloud_ref_names(&branches);
        let selected = self
            .cloud_ref_input(field)
            .map(|input| input.read(cx).content.trim().to_owned())
            .filter(|text| !text.is_empty());
        let default_entry = field == CloudRefField::Base;
        let footer = format!("Branches of {remote}");
        let accept: fn(&str) -> bool = if self.cloud_target() == Some(CloudTarget::DockerSandbox) {
            docker_config::valid_ref
        } else {
            codespace_branch_ok
        };
        let picker =
            cx.new(|cx| GitPicker::refs(names, selected, default_entry, Some(footer), accept, cx));
        let events = cx.subscribe_in(
            &picker,
            window,
            move |this, _, event: &GitPicked, window, cx| {
                if let GitPicked::Ref(name) = event {
                    this.cloud_ref_picked(field, name.clone(), cx);
                }
                this.close_cloud_picker();
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            },
        );
        window.focus(&picker.read(cx).focus(cx), cx);
        self.cloud_picker = Some(CloudRefPicker {
            field,
            picker,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_cloud_picker(&mut self) -> bool {
        self.cloud_picker.take().is_some()
    }

    /// Applies a pick to the session's override: the same text the free field would hold, so
    /// the request is built and validated as before. `None` is the project's default branch.
    pub(super) fn cloud_ref_picked(
        &mut self,
        field: CloudRefField,
        name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_creating() {
            return;
        }
        let name = name.unwrap_or_default();
        let ok = if self.cloud_target() == Some(CloudTarget::DockerSandbox) {
            docker_config::valid_ref(&name)
        } else {
            codespace_branch_ok(&name)
        };
        if !name.is_empty() && !ok {
            return;
        }
        let Some(input) = self.cloud_ref_input(field) else {
            return;
        };
        if field == CloudRefField::Base
            && let Some(draft) = &mut self.cloud_draft
        {
            draft.base_text = name.clone();
        }
        input.update(cx, |input, cx| input.set_text(name, cx));
        cx.notify();
    }
}
