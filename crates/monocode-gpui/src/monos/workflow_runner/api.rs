//! The seam between a run and the app: what a run asks of its host, and the values that cross it.
use super::Fut;
use monocode_core::mono::{
    habits,
    workflows::{command::CommandResult, model::WorkflowRun},
};

/// The waits a run uses; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// How often a watched turn is looked at.
    pub poll_ms: u64,
    /// How long the Mono has to answer a decision request.
    pub ask_timeout_ms: u64,
    /// How long a node's session may wait for an approval.
    pub approval_wait_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            poll_ms: 2_000,
            ask_timeout_ms: 10 * 60_000,
            approval_wait_ms: habits::APPROVAL_WAIT_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Completed,
    Failed,
    Cancelled,
}

/// How a turn ended (`ControlOutcome`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub status: Status,
    pub text: String,
    pub error: Option<String>,
}

impl Outcome {
    pub(crate) fn completed(text: impl Into<String>) -> Self {
        Self {
            status: Status::Completed,
            text: text.into(),
            error: None,
        }
    }
    pub(crate) fn failed(error: impl Into<String>) -> Self {
        Self {
            status: Status::Failed,
            text: String::new(),
            error: Some(error.into()),
        }
    }
    pub(crate) fn cancelled(error: impl Into<String>) -> Self {
        Self {
            status: Status::Cancelled,
            text: String::new(),
            error: Some(error.into()),
        }
    }
}

/// A session a node asks for (`SessionRequest`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRequest {
    /// One of the Mono's project paths.
    pub project: String,
    pub prompt: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `workspaceMode: "worktree"`: a new worktree for the session.
    pub new_worktree: bool,
    pub worktree_cwd: Option<String>,
    pub parent_session_id: Option<String>,
    pub folder_id: Option<String>,
    pub folder_name: Option<String>,
}

pub(crate) struct StartedSession {
    /// The persisted session id, as run files store it.
    pub session_id: String,
    pub folder_id: Option<String>,
    /// Resolves when the first turn settles.
    pub settled: Fut<Outcome>,
}

/// A session the report lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionInfo {
    pub session_id: String,
    pub cwd: String,
    pub title: String,
    pub harness: String,
    pub model: Option<String>,
}

/// An approval a node's session is waiting on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApprovalAsk {
    pub mono_id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub run_id: String,
    pub session_id: String,
}

/// A run's report for the Mono's chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Posted {
    pub mono_id: String,
    pub workflow_id: String,
    pub workflow_name: String,
    pub run_id: String,
    pub title: String,
    pub text: String,
    /// The sessions the run started, in the order of its nodes.
    pub launches: Vec<SessionInfo>,
}

/// Everything a run needs from the app (`MonoWorkflowHost`).
pub(crate) trait WorkflowHost {
    /// Starts a visible project session owned by the Mono, like `sessions.start`, without a
    /// completion report to the Mono.
    fn start_session(
        &self,
        mono_id: &str,
        req: SessionRequest,
    ) -> Fut<Result<StartedSession, String>>;
    /// Sends a follow-up turn to an existing session; resolves when it settles.
    fn send(&self, session_id: &str, prompt: String) -> Fut<Outcome>;
    fn stop(&self, session_id: &str) -> Fut<Result<(), String>>;
    /// The last thing the session said this turn (empty when it said nothing).
    fn final_reply(&self, session_id: &str) -> String;
    /// Whether the session is waiting on the user (an approval or a question).
    fn waiting_approval(&self, session_id: &str) -> bool;
    /// Shows what the session waits on to the user. Called again while it waits.
    fn ask_approval(&self, ask: ApprovalAsk);
    /// Stops showing a session's approvals.
    fn end_approvals(&self, session_id: &str);
    /// The run is done with the session: whatever the host kept to watch it can go.
    fn release_session(&self, session_id: &str);
    fn session_info(&self, session_id: &str) -> Option<SessionInfo>;
    /// Runs a shell command; `id` names it for `cancel_command` while it runs.
    fn run_command(
        &self,
        cwd: String,
        command: String,
        timeout_ms: u64,
        id: String,
    ) -> Fut<Result<CommandResult, String>>;
    /// Kills the running command `id`; false when no such command runs (yet).
    fn cancel_command(&self, id: &str) -> Fut<Result<bool, String>>;
    /// A hidden Mono-context turn; resolves with its final reply.
    fn ask_mono(&self, mono_id: &str, prompt: String) -> Fut<Result<String, String>>;
    fn post(&self, report: Posted);
    /// Resolves a `{{inputs.x}}` value to one of the Mono's project paths.
    fn project_path(&self, mono_id: &str, input: &str) -> Option<String>;
    /// The working directory of a session (its worktree, else its project).
    fn worktree_of(&self, session_id: &str) -> Option<String>;
    /// Writes the run into the workflow's run history.
    fn save_run(&self, mono_id: &str, run: WorkflowRun) -> Fut<Result<(), String>>;
}
