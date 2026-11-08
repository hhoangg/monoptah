//! "RUN ON CLOUD" in the new-session composer: a manual way to put a fresh session in a
//! Codespace or a Docker Sandbox. Choosing a target in the Workspace menu does not create
//! anything. "Create and open" asks the shell to (see `shell/cloud_create.rs`), which makes the
//! session at once, queues the composer text as its first prompt and follows the start. Send
//! never provisions on its own: while a target is chosen and nothing is created it is disabled.
use super::*;
use monocode_core::cloud_sessions::docker_config::{self, DockerProjectConfig};

/// Shown as the hint and the send button's label while a cloud target waits for "Create and open".
pub(crate) const CREATE_FIRST: &str = "Create and open first";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudTarget {
    Codespace,
    DockerSandbox,
}
impl CloudTarget {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Codespace => "Codespace",
            Self::DockerSandbox => "Docker Sandbox",
        }
    }
    fn note(self) -> &'static str {
        match self {
            Self::Codespace => {
                "Creates a GitHub codespace (billed to your GitHub account). Your prompt is queued until it is ready."
            }
            Self::DockerSandbox => {
                "Creates paid cloud compute in your Docker account. Your prompt is queued until the sandbox is ready."
            }
        }
    }
}

/// What "Create and open" asks the shell for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudCreate {
    pub target: CloudTarget,
    /// The local project the session twins.
    pub project: String,
    /// Docker: the new feature branch. Codespace: the branch the codespace is created on.
    pub branch: String,
    /// Docker only: the saved project config.
    pub config: Option<DockerProjectConfig>,
    /// The composer text, queued as the first prompt.
    pub prompt: Option<String>,
    /// Which press asked; the answer only applies to the draft that is still waiting for it.
    pub request_id: u64,
}

/// Emitted by "Create and open".
pub(crate) struct CreateCloudSession(pub CloudCreate);
/// Emitted when the Docker Sandbox row is picked and the project has no saved config.
pub(crate) struct OpenDockerSetup(pub String);
impl EventEmitter<CreateCloudSession> for Conversation {}
impl EventEmitter<OpenDockerSetup> for Conversation {}

pub(super) struct CloudDraft {
    pub target: CloudTarget,
    /// Docker: the feature branch, editable.
    pub branch: Entity<input::Input>,
    /// The saved config shown in the cloud row (Docker).
    pub config: Option<DockerProjectConfig>,
    pub creating: bool,
    pub error: Option<String>,
    /// The request in flight (or the last one).
    pub request: u64,
    /// A codespace this draft already created: creating again would bill a second one.
    pub created: Option<String>,
    _events: Vec<Subscription>,
}

/// The codespace named by a start error that happened after it was created
/// (`created_note`), if any.
pub(crate) fn created_codespace(error: &str) -> Option<&str> {
    let rest = error.split_once("(codespace ")?.1;
    let (name, tail) = rest.split_once(" was created and kept")?;
    (!name.is_empty() && !tail.is_empty() && !name.contains(' ')).then_some(name)
}

/// `4g` -> `4 GiB`, `512m` -> `512 MiB`; anything else as saved.
pub(crate) fn memory_label(memory: &str) -> String {
    for (suffix, unit) in [('g', "GiB"), ('m', "MiB")] {
        if let Some(digits) = memory.strip_suffix(suffix)
            && !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
        {
            return format!("{digits} {unit}");
        }
    }
    memory.to_owned()
}

/// `2 vCPU / 4 GiB · 2h then stop`.
pub(crate) fn compute_label(config: &DockerProjectConfig) -> String {
    format!(
        "{} vCPU / {} \u{b7} {} then stop",
        config.cpus,
        memory_label(&config.memory),
        config.ttl
    )
}

fn short_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() ^ (d.as_secs() as u32));
    format!("{:06x}", nanos & 0xff_ffff)
}

impl Conversation {
    /// The shell tells whether the project is a GitHub repository (Codespace needs one).
    pub(crate) fn set_offers_codespace(&mut self, offers: bool, cx: &mut Context<Self>) {
        if self.cloud_offers_codespace != offers {
            self.cloud_offers_codespace = offers;
            cx.notify();
        }
    }

    pub(crate) fn cloud_target(&self) -> Option<CloudTarget> {
        self.cloud_draft.as_ref().map(|draft| draft.target)
    }

    /// A create is running: the target and the workspace cannot change under it.
    pub(crate) fn cloud_creating(&self) -> bool {
        self.cloud_draft
            .as_ref()
            .is_some_and(|draft| draft.creating)
    }

    /// A cloud target is chosen and nothing is created yet: the send button stays disabled.
    pub(crate) fn cloud_create_pending(&self) -> bool {
        self.cloud_draft.is_some()
    }

    /// Whether the Workspace menu can offer a cloud target for this session.
    fn cloud_menu_applies(&self) -> bool {
        !self.summary.has_messages && !self.is_remote()
    }

    /// The "RUN ON CLOUD" section of the Workspace menu, empty when nothing applies.
    pub(super) fn cloud_menu_items(&self) -> Vec<MenuItem> {
        if !self.cloud_menu_applies() {
            return Vec::new();
        }
        let mut rows = Vec::new();
        if self.cloud_offers_codespace {
            rows.push(
                MenuAction::new("workspace:cloud-codespace", "Codespace")
                    .icon("cloud-upload")
                    .checked(self.cloud_target() == Some(CloudTarget::Codespace))
                    .into(),
            );
        }
        let configured = self.docker_config().is_some();
        let mut docker = MenuAction::new("workspace:cloud-docker", "Docker Sandbox")
            .icon("cloud-upload")
            .checked(self.cloud_target() == Some(CloudTarget::DockerSandbox));
        if !configured {
            docker = docker.description("Set up first");
        }
        rows.push(docker.into());
        let mut items = vec![MenuItem::Separator, MenuItem::Header("Run on cloud".into())];
        items.extend(rows);
        items
    }

    /// The saved config, read from the store once per menu opening, not per render.
    fn docker_config(&self) -> Option<DockerProjectConfig> {
        self.docker_config_cache
            .borrow_mut()
            .get_or_insert_with(|| {
                locked(&self.store).ok().and_then(|store| {
                    docker_config::load(&store, &self.summary.cwd)
                        .ok()
                        .flatten()
                })
            })
            .clone()
    }

    pub(super) fn forget_docker_config(&self) {
        self.docker_config_cache.take();
    }

    /// Handles the cloud rows of the Workspace menu; false for any other id.
    pub(crate) fn cloud_menu_choice(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        match id {
            "workspace:cloud-codespace" => {
                if self.cloud_menu_applies()
                    && !self.cloud_creating()
                    && self.cloud_offers_codespace
                {
                    self.choose_cloud_target(CloudTarget::Codespace, None, cx);
                }
                true
            }
            "workspace:cloud-docker" => {
                if self.cloud_menu_applies() && !self.cloud_creating() {
                    self.forget_docker_config();
                    match self.docker_config() {
                        Some(config) => {
                            self.choose_cloud_target(CloudTarget::DockerSandbox, Some(config), cx)
                        }
                        None => cx.emit(OpenDockerSetup(self.summary.cwd.clone())),
                    }
                }
                true
            }
            // Local checkouts mean Local.
            "workspace:current" | "workspace:new" => {
                // Not while a create runs: it would send locally and queue the same prompt.
                if self.cloud_creating() {
                    return true;
                }
                self.clear_cloud_target(cx);
                false
            }
            _ => false,
        }
    }

    pub(super) fn clear_cloud_target(&mut self, cx: &mut Context<Self>) {
        if self.cloud_creating() {
            return;
        }
        if self.cloud_draft.take().is_some() {
            cx.notify();
        }
    }

    fn choose_cloud_target(
        &mut self,
        target: CloudTarget,
        config: Option<DockerProjectConfig>,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &mut self.cloud_draft
            && draft.target == target
            && !draft.creating
        {
            draft.config = config;
            cx.notify();
            return;
        }
        if self
            .cloud_draft
            .as_ref()
            .is_some_and(|draft| draft.creating)
        {
            return;
        }
        let default = format!("monoptah/{}", short_id());
        let branch = cx.new(|cx| {
            let mut field = input::Input::field("Branch", "monoptah/feature", content(0.35), cx);
            field.set_font_size(12.);
            field.set_text(default, cx);
            field
        });
        let events = vec![cx.subscribe(&branch, |_, _, _: &input::Changed, cx| cx.notify())];
        // The cloud session starts from the project's own checkout, not a worktree.
        self.settings.worktree_cwd = None;
        self.settings.new_worktree = false;
        self.settings.worktree_removed = false;
        self.save_settings(cx);
        self.refresh_git(cx);
        self.cloud_draft = Some(CloudDraft {
            target,
            branch,
            config,
            creating: false,
            error: None,
            request: 0,
            created: None,
            _events: events,
        });
        cx.notify();
    }

    /// The base branch the cloud session starts from.
    fn cloud_base(&self) -> String {
        match self.cloud_draft.as_ref() {
            Some(CloudDraft {
                target: CloudTarget::DockerSandbox,
                config: Some(config),
                ..
            }) if !config.base_ref.trim().is_empty() => config.base_ref.trim().to_owned(),
            Some(CloudDraft {
                target: CloudTarget::DockerSandbox,
                ..
            }) => "default branch".into(),
            _ => self
                .branch
                .clone()
                .unwrap_or_else(|| "default branch".into()),
        }
    }

    /// The workspace trigger's text while a cloud target is chosen.
    pub(super) fn cloud_workspace_label(&self) -> Option<String> {
        self.cloud_target()
            .map(|target| format!("\u{2601} {}", target.label()))
    }

    /// The branch trigger's text while a cloud target is chosen.
    pub(super) fn cloud_base_label(&self) -> Option<String> {
        self.cloud_draft
            .is_some()
            .then(|| format!("base {}", self.cloud_base()))
    }

    /// "Create and open": asks the shell to create the resource and the session.
    pub(crate) fn create_cloud(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = &self.cloud_draft else {
            return;
        };
        if draft.creating || draft.created.is_some() {
            return;
        }
        let target = draft.target;
        let fail = |this: &mut Self, error: &str, cx: &mut Context<Self>| {
            if let Some(draft) = &mut this.cloud_draft {
                draft.error = Some(error.to_owned());
            }
            cx.notify();
        };
        if !self.attachments.is_empty() {
            fail(
                self,
                "Attachments cannot be queued while the session starts. Remove them first.",
                cx,
            );
            return;
        }
        let (branch, config) = match target {
            // Same source as the header picker: HEAD of the project folder (a worktree's branch
            // would not exist on the remote). The machine stays the default: the picker's choice
            // comes from a blocking `gh` listing, which does not belong on the UI thread.
            CloudTarget::Codespace => match git_head(Path::new(&self.summary.cwd)) {
                Some((_, true)) => {
                    fail(
                        self,
                        "HEAD is detached. Check out a branch to start a codespace on it.",
                        cx,
                    );
                    return;
                }
                Some((branch, false)) => (branch, None),
                None => (String::new(), None),
            },
            CloudTarget::DockerSandbox => {
                let branch = draft.branch.read(cx).content.trim().to_owned();
                if branch.is_empty() {
                    fail(self, "Enter a branch name for the new session.", cx);
                    return;
                }
                self.forget_docker_config();
                match self.docker_config() {
                    Some(config) => (branch, Some(config)),
                    None => {
                        fail(self, "Set up Docker Sandbox for this project first.", cx);
                        return;
                    }
                }
            }
        };
        if branch.is_empty() {
            fail(self, "There is no branch to start the codespace on.", cx);
            return;
        }
        let prompt = self.input.read(cx).content.trim().to_owned();
        self.cloud_request_seq += 1;
        let request_id = self.cloud_request_seq;
        let request = CloudCreate {
            request_id,
            target,
            project: self.summary.cwd.clone(),
            branch,
            config,
            prompt: (!prompt.is_empty()).then_some(prompt),
        };
        if let Some(draft) = &mut self.cloud_draft {
            draft.creating = true;
            draft.request = request_id;
            draft.error = None;
        }
        cx.emit(CreateCloudSession(request));
        cx.notify();
    }

    /// The shell finished (or refused) the creation requested by [`Self::create_cloud`]. `Ok`
    /// says whether the composer text moved into the new session's queue.
    pub(crate) fn cloud_create_finished(
        &mut self,
        request_id: u64,
        result: Result<bool, String>,
        cx: &mut Context<Self>,
    ) {
        // A stale answer (the draft was replaced) must not touch the composer.
        if !self
            .cloud_draft
            .as_ref()
            .is_some_and(|draft| draft.creating && draft.request == request_id)
        {
            return;
        }
        match result {
            Ok(consumed) => {
                if consumed {
                    self.input
                        .update(cx, |input, cx| input.set_text(String::new(), cx));
                }
                // The resource exists: never offer a second "Create and open" for it.
                self.cloud_draft = None;
            }
            Err(error) => {
                if let Some(draft) = &mut self.cloud_draft {
                    draft.creating = false;
                    draft.created = created_codespace(&error).map(str::to_owned);
                    draft.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    /// The compact row between the composer box and the workspace bar.
    pub(super) fn cloud_target_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = self.cloud_draft.as_ref()?;
        let compute = match (draft.target, &draft.config) {
            (CloudTarget::DockerSandbox, Some(config)) => compute_label(config),
            _ => format!(
                "{} \u{b7} billed to your GitHub account",
                monocode_core::codespaces::DEFAULT_MACHINE
            ),
        };
        let creating = draft.creating;
        let created = draft.created.clone();
        let locked_button = creating || created.is_some();
        Some(
            div()
                .id("cloud-target-row")
                .debug_selector(|| "cloud-target-row".into())
                .role(Role::Group)
                .aria_label(format!("Run on {}", draft.target.label()))
                .mx(px(15.))
                .mt(px(8.))
                .p(px(10.))
                .rounded_lg()
                .bg(content(0.04))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .child(icon("cloud-upload", 14.).text_color(content(0.55)))
                        .child(
                            div()
                                .id("cloud-target-compute")
                                .debug_selector(|| "cloud-target-compute".into())
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_color(content(0.8))
                                .child(compute),
                        ),
                )
                .when(draft.target == CloudTarget::DockerSandbox, |d| {
                    d.child(
                        div()
                            .h(px(28.))
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.03))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .font_family("Menlo")
                            .child(draft.branch.clone()),
                    )
                })
                .child(
                    div()
                        .id("cloud-target-note")
                        .text_size(px(11.))
                        .line_height(px(15.))
                        .text_color(content(0.5))
                        .child(draft.target.note()),
                )
                .children(draft.error.clone().map(|error| {
                    div()
                        .id("cloud-target-error")
                        .debug_selector(|| "cloud-target-error".into())
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .line_height(px(17.))
                        .text_color(gpui::rgb(0xf87171))
                        .child(error)
                }))
                .child(
                    div().flex().justify_end().child(
                        crate::modal::disabled(
                            crate::modal::primary_button(
                                "cloud-target-create",
                                if creating {
                                    "Creating\u{2026}".to_owned()
                                } else if let Some(name) = &created {
                                    format!("Already created: {name}")
                                } else {
                                    "Create and open".to_owned()
                                },
                            ),
                            locked_button,
                        )
                        .debug_selector(|| "cloud-target-create".into())
                        .on_click(cx.listener(|this, _, _, cx| this.create_cloud(cx))),
                    ),
                )
                .into_any_element(),
        )
    }
}
