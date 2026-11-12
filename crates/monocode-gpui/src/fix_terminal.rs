//! A small dialog with an embedded terminal that runs one fix command, such as
//! `gh auth login` or `sbx --cloud setup ssh`. Shared by Settings > Cloud and the project
//! Cloud settings dialog. The caller owns the entity, shows it with [`dialog`] (or its own
//! modal), and re-checks what the command was fixing when it emits [`FixEvent::Exited`].
//!
//! A token the command needs goes only into [`FixCommand::env`]: the environment of the
//! launched program. It is never part of the arguments, the command line shown in the dialog
//! or any event.
use crate::{
    cloud_dialog_ui::{button, code_box, muted},
    terminal::{Terminal, TerminalEvent},
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, IntoElement,
    KeyDownEvent, MouseButton, Render, Role, StyleRefinement, Subscription, Window, div,
    prelude::*, px,
};
use monocode_core::git_credentials::Secret;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// What to run. `program` is launched as given (resolve it with the GUI binary lookup first,
/// because an app started from the Dock has a short `PATH`).
#[derive(Debug)]
pub(crate) struct FixCommand {
    /// The dialog title, chosen by the caller.
    pub title: String,
    pub program: OsString,
    pub args: Vec<String>,
    /// Environment variables for the program only. Values are secrets: never shown, never
    /// part of an event, and not in `Debug` output.
    pub env: Vec<(String, Secret)>,
}

impl FixCommand {
    pub(crate) fn run(
        title: impl Into<String>,
        program: impl Into<OsString>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            title: title.into(),
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
        }
    }

    /// A command written as a line such as `gh auth login` (words split on whitespace; no
    /// quoting). `resolve` finds the program, falling back to the bare word.
    pub(crate) fn from_line(
        title: impl Into<String>,
        line: &str,
        resolve: impl Fn(&str) -> Option<PathBuf>,
    ) -> Option<Self> {
        let mut words = line.split_whitespace();
        let name = words.next()?;
        let program = resolve(name).map_or_else(|| OsString::from(name), PathBuf::into_os_string);
        Some(Self::run(title, program, words))
    }

    pub(crate) fn with_env(mut self, name: impl Into<String>, value: Secret) -> Self {
        self.env.push((name.into(), value));
        self
    }

    /// The command line as shown: the program's file name and its arguments. Never the
    /// environment.
    pub(crate) fn line(&self) -> String {
        let program = Path::new(&self.program)
            .file_name()
            .unwrap_or(&self.program)
            .to_string_lossy()
            .into_owned();
        std::iter::once(program)
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// What the dialog tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FixEvent {
    /// The command ended. `success` is true for exit code 0. Re-check now.
    Exited { success: bool },
    /// The user closed the dialog (Close, the x, or Escape). The terminal is already
    /// stopped; the owner drops the entity.
    Close,
}

pub(crate) struct FixTerminal {
    title: String,
    line: String,
    terminal: Entity<Terminal>,
    _terminal_events: Subscription,
    focus: FocusHandle,
    /// `None` while the command runs.
    outcome: Option<Outcome>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Succeeded,
    Failed(Option<String>),
}

impl EventEmitter<FixEvent> for FixTerminal {}
impl Focusable for FixTerminal {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FixTerminal {
    /// Starts the command in a terminal whose directory is `cwd` (home when it is missing).
    pub(crate) fn new(command: FixCommand, cwd: &Path, cx: &mut Context<Self>) -> Self {
        let line = command.line();
        let env_set = command
            .env
            .iter()
            .map(|(name, value)| (name.clone(), OsString::from(value.as_str())))
            .collect();
        let args = command.args.iter().map(OsString::from).collect();
        let program = command.program.clone();
        let terminal = cx.new(|cx| Terminal::launch(cwd, program, args, env_set, Vec::new(), cx));
        let events = cx.subscribe(&terminal, Self::on_terminal_event);
        Self {
            title: command.title,
            line,
            terminal,
            _terminal_events: events,
            focus: cx.focus_handle(),
            outcome: None,
        }
    }

    fn on_terminal_event(
        &mut self,
        _: Entity<Terminal>,
        event: &TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        let TerminalEvent::Exited { code, error, .. } = event else {
            return;
        };
        if self.outcome.is_some() {
            return;
        }
        let success = *code == Some(0) && error.is_none();
        self.outcome = Some(if success {
            Outcome::Succeeded
        } else {
            Outcome::Failed(error.clone())
        });
        cx.emit(FixEvent::Exited { success });
        cx.notify();
    }

    /// The embedded terminal has the keyboard: Escape belongs to the program, not the dialog.
    pub(crate) fn terminal_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.terminal
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    }

    /// Stops the program. Called by [`FixEvent::Close`] and by owners that drop the dialog.
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.terminal.read(cx).kill();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.shutdown(cx);
        cx.emit(FixEvent::Close);
    }

    #[cfg(test)]
    pub(crate) fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    #[cfg(test)]
    pub(crate) fn command_line(&self) -> &str {
        &self.line
    }

    fn status(&self) -> String {
        match &self.outcome {
            None => "Running. Follow the steps in the terminal; this dialog stays open until you close it."
                .to_owned(),
            Some(Outcome::Succeeded) => "Finished. You can close this dialog.".to_owned(),
            Some(Outcome::Failed(Some(error))) => format!("Could not run the command: {error}"),
            Some(Outcome::Failed(None)) => {
                "The command ended with an error. The output is above.".to_owned()
            }
        }
    }
}

impl Render for FixTerminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let failed = matches!(self.outcome, Some(Outcome::Failed(_)));
        div()
            .id("fix-terminal")
            .debug_selector(|| "fix-terminal".into())
            .role(Role::Dialog)
            .aria_label(self.title.clone())
            .track_focus(&self.focus)
            .relative()
            .min_h_0()
            .flex()
            .flex_col()
            .text_color(content(1.))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && !this.terminal_focused(window, cx) {
                    this.close(cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_4()
                    .pt_3()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .pt(px(2.))
                            .text_size(px(16.))
                            .line_height(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.title.clone()),
                    )
                    .child(
                        icon_button("fix-terminal-close-x", "Close", "close")
                            .size(px(28.))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .child(
                div()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(muted("Running this command:"))
                    .child(
                        code_box(&self.line)
                            .id("fix-terminal-command")
                            .debug_selector(|| "fix-terminal-command".into()),
                    )
                    .child(
                        div()
                            .id("fix-terminal-pane")
                            .debug_selector(|| "fix-terminal-pane".into())
                            .h(px(280.))
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .overflow_hidden()
                            .child(
                                self.terminal
                                    .clone()
                                    .cached(StyleRefinement::default().size_full()),
                            ),
                    )
                    .child(
                        div()
                            .id("fix-terminal-status")
                            .debug_selector(|| "fix-terminal-status".into())
                            .role(if failed { Role::Alert } else { Role::Status })
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(if failed {
                                gpui::rgb(0xf87171).into()
                            } else {
                                content(0.55)
                            })
                            .child(self.status()),
                    )
                    .child(
                        div().flex().justify_end().child(
                            button("fix-terminal-close", "Close", false)
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        ),
                    ),
            )
    }
}

/// A centred modal around `view` with a scrim over the whole window, for hosts that have no
/// modal of their own. The owner handles [`FixEvent::Close`].
pub(crate) fn dialog(view: &Entity<FixTerminal>, window: &Window) -> AnyElement {
    let overlay = div()
        .absolute()
        .inset_0()
        .occlude()
        .child(crate::modal::scrim("fix-terminal-scrim"))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id("fix-terminal-card")
                .w(px(560.))
                .max_w_full()
                .max_h(px(window.viewport_size().height.as_f32() - 32.))
                .relative()
                .rounded(px(crate::modal::RADIUS))
                .border_1()
                .border_color(content(crate::modal::BORDER))
                .child(crate::modal::surface(crate::modal::RADIUS - 1.))
                .child(view.clone()),
        );
    crate::modal::window_overlay(window, overlay).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::{cell::RefCell, rc::Rc};

    const TOKEN: &str = "ghp_exampleSecretTokenValue123456";

    fn resolve(name: &str) -> Option<PathBuf> {
        Some(PathBuf::from("/opt/tools").join(name))
    }
    fn nothing(_: &str) -> Option<PathBuf> {
        None
    }

    struct Host {
        fix: Entity<FixTerminal>,
    }
    impl Render for Host {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            dialog(&self.fix, window)
        }
    }

    fn open(
        command: FixCommand,
        cx: &mut TestAppContext,
    ) -> (gpui::WindowHandle<Host>, Rc<RefCell<Vec<FixEvent>>>) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = events.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_, cx| {
                let fix = cx.new(|cx| FixTerminal::new(command, Path::new("/tmp"), cx));
                let sink = sink.clone();
                cx.subscribe(&fix, move |_, event: &FixEvent, _| {
                    sink.borrow_mut().push(event.clone())
                })
                .detach();
                cx.new(|_| Host { fix })
            })
            .unwrap()
        });
        cx.run_until_parked();
        cx.refresh().unwrap();
        (window, events)
    }

    fn fix_of(window: &gpui::WindowHandle<Host>, cx: &mut TestAppContext) -> Entity<FixTerminal> {
        window.read_with(cx, |host, _| host.fix.clone()).unwrap()
    }

    #[test]
    fn a_line_is_split_and_the_program_resolved() {
        let command = FixCommand::from_line(
            "Sign in",
            "gh auth refresh -h github.com -s codespace",
            resolve,
        )
        .unwrap();
        assert_eq!(command.program, OsString::from("/opt/tools/gh"));
        assert_eq!(
            command.args,
            ["auth", "refresh", "-h", "github.com", "-s", "codespace"]
        );
        // The line shows the program's name, not the path it resolved to.
        assert_eq!(command.line(), "gh auth refresh -h github.com -s codespace");
        let bare = FixCommand::from_line("Set up", "sbx --cloud setup ssh", nothing).unwrap();
        assert_eq!(bare.program, OsString::from("sbx"));
        assert!(FixCommand::from_line("Empty", "   ", resolve).is_none());
    }

    #[test]
    fn the_token_is_in_the_environment_only() {
        let command = FixCommand::run("Sign in", "/bin/tool", ["login"])
            .with_env("TOOL_TOKEN", Secret::new(TOKEN.into()));
        assert!(!command.line().contains(TOKEN));
        assert!(!format!("{command:?}").contains(TOKEN));
        assert!(!command.args.iter().any(|a| a.contains(TOKEN)));
    }

    #[gpui::test]
    fn it_launches_the_command_with_the_environment_and_shows_the_line(cx: &mut TestAppContext) {
        let command = FixCommand::run("Set up ssh", "/bin/true", ["--cloud", "setup", "ssh"])
            .with_env("TOOL_TOKEN", Secret::new(TOKEN.into()));
        let (window, _) = open(command, cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        let fix = fix_of(&window, visual);
        fix.update(visual, |fix, cx| {
            assert_eq!(fix.command_line(), "true --cloud setup ssh");
            fix.terminal().update(cx, |terminal, _| {
                let spec = terminal.launched().expect("the launch is recorded");
                assert_eq!(spec.program, OsString::from("/bin/true"));
                assert_eq!(spec.args, ["--cloud", "setup", "ssh"].map(OsString::from));
                assert_eq!(
                    spec.env_set,
                    [("TOOL_TOKEN".to_owned(), OsString::from(TOKEN))]
                );
            });
        });
        assert!(visual.debug_bounds("fix-terminal-command").is_some());
        assert!(visual.debug_bounds("fix-terminal-pane").is_some());
        assert!(visual.debug_bounds("fix-terminal-close").is_some());
    }

    #[gpui::test]
    fn exit_is_reported_once_with_the_outcome(cx: &mut TestAppContext) {
        let (window, events) = open(FixCommand::run("Fix", "/bin/true", ["x"]), cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let fix = fix_of(&window, visual);
        fix.update(visual, |fix, cx| {
            fix.terminal().update(cx, |t, cx| {
                t.fake_started(cx);
                t.fake_exit(Some(0), cx);
                t.fake_exit(Some(1), cx);
            });
        });
        visual.run_until_parked();
        assert_eq!(*events.borrow(), [FixEvent::Exited { success: true }]);
        fix.update(visual, |fix, _| {
            assert_eq!(fix.status(), "Finished. You can close this dialog.")
        });

        let (window, events) = open(FixCommand::run("Fix", "/bin/false", ["x"]), cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let fix = fix_of(&window, visual);
        fix.update(visual, |fix, cx| {
            fix.terminal().update(cx, |t, cx| t.fake_exit(Some(1), cx));
        });
        visual.run_until_parked();
        assert_eq!(*events.borrow(), [FixEvent::Exited { success: false }]);
    }

    #[gpui::test]
    fn a_program_that_cannot_start_is_a_failure_with_a_reason(cx: &mut TestAppContext) {
        let (window, events) = open(FixCommand::run("Fix", "/no/such/tool", ["x"]), cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        let fix = fix_of(&window, visual);
        fix.update(visual, |fix, cx| {
            fix.terminal()
                .update(cx, |t, cx| t.fake_spawn_error("No such file", cx));
        });
        visual.run_until_parked();
        fix.update(visual, |fix, _| {
            assert!(fix.status().contains("No such file"));
        });
        assert_eq!(*events.borrow(), [FixEvent::Exited { success: false }]);
    }

    #[gpui::test]
    fn close_button_and_escape_close_unless_the_terminal_has_focus(cx: &mut TestAppContext) {
        let (window, events) = open(FixCommand::run("Fix", "/bin/true", ["x"]), cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.refresh().unwrap();
        // Escape with the dialog focused closes it.
        let fix = fix_of(&window, visual);
        visual.update(|window, cx| window.focus(&fix.focus_handle(cx), cx));
        visual.simulate_keystrokes("escape");
        assert_eq!(*events.borrow(), [FixEvent::Close]);
        events.borrow_mut().clear();
        // Escape with the terminal focused is the program's.
        let terminal = fix.read_with(visual, |fix, _| fix.terminal().clone());
        visual.update(|window, cx| {
            let handle = terminal.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        });
        visual.simulate_keystrokes("escape");
        assert!(events.borrow().is_empty());
        // The Close button always closes.
        let bounds = visual.debug_bounds("fix-terminal-close").unwrap();
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(*events.borrow(), [FixEvent::Close]);
    }
}
