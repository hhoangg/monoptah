//! `commands.list` and `commands.run`: the `/` menu of a session composer for a paired web
//! device. The list comes from `monocode_core::slash_commands`, the table the native composer
//! uses too, and every command runs through the function the desktop's own menu calls.
use super::*;
use monocode_core::slash_commands::{self, Context as MenuContext};

/// The most skills one menu carries, and the longest description of each, so the answer stays
/// small for a project with thousands of skills.
const MAX_SKILLS: usize = 300;
const MAX_DETAIL: usize = 200;

/// Why a command is not offered before a session exists: the new-session page applies Plan,
/// Orchestrator and Draft as a prefix of its first message, as the desktop composer does.
fn new_session_context(provider: Kind, read_only: bool) -> MenuContext {
    let blocked = [
        ("operator", "Start the session first, then turn on Operator"),
        ("btw", "Available once the session has a finished turn"),
        ("compact", "Available once the session has a finished turn"),
        ("add-to-folder", "Available once the session exists"),
    ]
    .into_iter()
    .map(|(id, reason)| (id.to_owned(), reason.to_owned()))
    .collect();
    MenuContext {
        provider: Some(provider),
        read_only,
        blocked,
        active: Vec::new(),
    }
}

/// The sensitive mode a message starts with (`/operator ...`, `/orchestrator ...`), which the
/// desktop composer turns on when the message is sent. `commands.run` audits these modes; a
/// message that carries the prefix is the same switch by another road, so it is audited too.
pub(super) fn leading_sensitive_mode(params: &Value) -> Option<&'static str> {
    let text = params["text"].as_str()?;
    ["operator", "orchestrator"]
        .into_iter()
        .find(|mode| monocode_core::composer::consume_command(text, mode).is_some())
}

impl Shell {
    /// A `conversation.send` or `session.start` whose text begins with `/operator` or
    /// `/orchestrator` switches that mode on, so it needs Control (the app does not rely on
    /// the host) and writes the same audit line as `commands.run`.
    pub(super) fn mobile_guard_prefixed_mode(&self, request: &Request) -> Result<(), Failure> {
        let Some(mode) = leading_sensitive_mode(&request.params) else {
            return Ok(());
        };
        let denied = request.device.permission != policy::Permission::Control;
        let conversation = string(&request.params, "conversationId");
        audit::record_slash_command_with(
            &request.device,
            &audit::SlashCommand {
                fingerprint: &self.mobile_fingerprint(&request.device.id),
                conversation_id: conversation.as_deref(),
                command: mode,
                enabled: Some(true),
                command_id: Some(&request.command_id),
                transport: request.device.kind.into(),
                outcome: if denied {
                    audit::Outcome::Denied(policy::Code::ReadOnly)
                } else {
                    audit::Outcome::Ok
                },
            },
        );
        if denied {
            return Err(policy::Code::ReadOnly.failure());
        }
        Ok(())
    }

    /// What a menu needs from the conversation or the project it is for, or `None` when the
    /// desktop shows no `/` menu there (a Mono chat, a remote project).
    fn mobile_menu_inputs(
        &mut self,
        request: &Request,
        cx: &mut Context<Self>,
    ) -> Result<Option<(String, MenuContext)>, Failure> {
        let read_only = request.device.permission != policy::Permission::Control;
        if let Some(conversation) = string(&request.params, "conversationId") {
            let Target::Session(id) = self.mobile_target(&conversation, &request.device, cx)?
            else {
                return Ok(None);
            };
            let view = self
                .ensure_session(id, cx)
                .ok_or("This conversation is still opening on desktop; try again")?;
            let chat = view.read(cx);
            if chat.settings.mono.is_some() || chat.summary.cwd.starts_with("remote://") {
                return Ok(None);
            }
            return Ok(Some((
                chat.summary.cwd.clone(),
                chat.mobile_menu_context(read_only, cx),
            )));
        }
        let cwd = self.mobile_project_path(
            &string(&request.params, "projectId").unwrap_or_default(),
            &request.device,
            cx,
        )?;
        if cwd.starts_with("remote://") {
            return Ok(None);
        }
        let kind = string(&request.params, "harness")
            .as_deref()
            .and_then(Kind::parse)
            .unwrap_or_else(|| self.mobile_defaults(&cwd, cx).0);
        Ok(Some((cwd, new_session_context(kind, read_only))))
    }

    /// `commands.list` (a read): the rows of the `/` menu for a conversation, or for a new
    /// session in a project. A device that may only read gets every row disabled.
    pub(super) fn mobile_commands_list(&mut self, request: Request, cx: &mut Context<Self>) {
        let inputs = match self.mobile_menu_inputs(&request, cx) {
            Ok(Some(inputs)) => inputs,
            Ok(None) => return request.reply(Ok(json!({"commands": []}))),
            Err(error) => return request.reply(Err(error)),
        };
        let (cwd, context) = inputs;
        let disabled = cx
            .try_global::<crate::settings::Preferences>()
            .map(|preferences| preferences.disabled_skill_paths.clone())
            .unwrap_or_default();
        let work = cx.background_executor().spawn(async move {
            monocode_core::skills::list_skills(cwd, Some(disabled)).unwrap_or_default()
        });
        cx.spawn(async move |_, _| {
            let skills = work.await;
            let mut entries = slash_commands::list(&context, &skills);
            let builtin = entries
                .iter()
                .filter(|entry| entry.kind == slash_commands::EntryKind::Builtin)
                .count();
            entries.truncate(builtin + MAX_SKILLS);
            for entry in &mut entries {
                if entry.detail.chars().count() > MAX_DETAIL {
                    entry.detail = entry.detail.chars().take(MAX_DETAIL).collect();
                }
            }
            request.reply(Ok(json!({ "commands": entries })));
        })
        .detach();
    }

    /// `commands.run` (Control): runs one command of the menu on an existing session. Plan,
    /// Operator and Orchestrator switch the mode of the next message; Draft saves `text`
    /// without starting the agent; a side question opens on the desktop; Compact context
    /// starts at once. Skills need no call: they are text the device puts in its message.
    pub(super) fn mobile_run_command(
        &mut self,
        device: &DeviceContext,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, Failure> {
        // The host already refuses a read-only device; the app does not rely on it.
        if device.permission != policy::Permission::Control {
            return Err(policy::Code::ReadOnly.failure());
        }
        let conversation = string(params, "conversationId").unwrap_or_default();
        let target = self.mobile_target(&conversation, device, cx)?;
        let command = string(params, "command").unwrap_or_default();
        if command == "add-to-folder" {
            return Err("Choose the folder in the web app".into());
        }
        if !Conversation::mobile_runnable(&command) {
            return Err(policy::Code::Unsupported.failure());
        }
        let Target::Session(id) = target else {
            return Err(policy::Code::UnsupportedTarget.failure());
        };
        let view = self
            .ensure_session(id, cx)
            .ok_or("This conversation is still opening on desktop; try again")?;
        if view.read(cx).settings.mono.is_some() {
            return Err(policy::Code::UnsupportedTarget.failure());
        }
        let enabled = params["enabled"].as_bool();
        let text = || message_text(params);
        let reply = json!({"command": command});
        match command.as_str() {
            "plan" | "operator" | "orchestrator" => {
                let on = view.update(cx, |chat, cx| {
                    chat.load(cx);
                    chat.mobile_set_mode(&command, enabled, cx)
                })?;
                Ok(json!({"command": command, "active": on}))
            }
            "draft" => {
                let text = text()?;
                view.update(cx, |chat, cx| chat.mobile_save_draft(text, cx))?;
                self.publish_mobile_catalog(cx);
                Ok(reply)
            }
            "btw" => {
                let text = text()?;
                view.update(cx, |chat, cx| chat.mobile_side_question(text, cx))?;
                Ok(json!({
                    "command": command,
                    "message": "The side question opened on the desktop. The answer shows there.",
                }))
            }
            _ => {
                view.update(cx, |chat, cx| chat.mobile_compact(cx))?;
                Ok(reply)
            }
        }
    }

    /// One audit line per `commands.run`, every outcome. Only the id of the command and the
    /// mode switch are written, never the text of a message.
    pub(super) fn mobile_audit_command(&self, request: &Request, result: &Result<Value, Failure>) {
        let outcome = match result {
            Ok(_) => audit::Outcome::Ok,
            Err(error) => {
                policy::code_of_failure(error).map_or(audit::Outcome::Error, audit::Outcome::Denied)
            }
        };
        let conversation = string(&request.params, "conversationId");
        let command = string(&request.params, "command").unwrap_or_default();
        audit::record_slash_command_with(
            &request.device,
            &audit::SlashCommand {
                fingerprint: &self.mobile_fingerprint(&request.device.id),
                conversation_id: conversation.as_deref(),
                command: &command,
                enabled: request.params["enabled"].as_bool(),
                command_id: Some(&request.command_id),
                transport: request.device.kind.into(),
                outcome,
            },
        );
    }
}
