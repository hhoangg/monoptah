//! Settings > Cloud > Backends: a checklist per backend (GitHub Codespaces, Docker Sandbox)
//! with Check again and Fix. Checks run on the background executor when the section opens
//! and on Check again. Fix opens the shared fix terminal and checks again when it exits.
use crate::{
    cloud_checks::{Gh, Sbx, gh_state},
    fix_terminal::{FixCommand, FixEvent, FixTerminal},
    settings::{
        SettingsView,
        cloud_env::CloudServices,
        widgets::{note, secondary_button},
    },
    theme::*,
};
use gpui::{
    AnyElement, Context, Entity, Focusable, Role, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use monocode_core::codespaces;

#[derive(Default)]
pub(in crate::settings) struct BackendsState {
    pub(in crate::settings) codespaces: Option<Gh>,
    pub(in crate::settings) sandbox: Option<Sbx>,
    pub(in crate::settings) checking: bool,
    /// Counts checks so a slow answer to an older one is dropped.
    pub(in crate::settings) generation: u64,
    pub(in crate::settings) task: Option<Task<()>>,
    pub(in crate::settings) fix: Option<OpenFix>,
    /// Why Fix could not start.
    pub(in crate::settings) fix_error: Option<String>,
}

pub(in crate::settings) struct OpenFix {
    pub(in crate::settings) view: Entity<FixTerminal>,
    _events: Subscription,
}

/// Which backend a button belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Codespaces,
    Sandbox,
}

impl Backend {
    fn key(self) -> &'static str {
        match self {
            Backend::Codespaces => "codespaces",
            Backend::Sandbox => "sandbox",
        }
    }
}

/// A check mark or an exclamation mark with its text.
pub(in crate::settings) fn chip(selector: &'static str, ok: bool, text: String) -> AnyElement {
    let color = if ok { success_color() } else { warning_color() };
    div()
        .id(selector)
        .debug_selector(move || selector.into())
        .role(Role::Status)
        .aria_label(text.clone())
        .flex()
        .items_center()
        .gap(px(5.))
        .text_size(px(12.))
        .text_color(content(0.8))
        .child(
            div()
                .size(px(14.))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(color)
                .text_color(color)
                .text_size(px(9.))
                .font_weight(gpui::FontWeight::BOLD)
                .flex()
                .items_center()
                .justify_center()
                .child(if ok { "\u{2713}" } else { "!" }),
        )
        .child(text)
        .into_any_element()
}

#[cfg(test)]
impl BackendsState {
    pub(in crate::settings) fn fix_view(&self) -> Option<Entity<FixTerminal>> {
        self.fix.as_ref().map(|open| open.view.clone())
    }
}

impl SettingsView {
    /// Checks both backends on the background executor.
    pub(in crate::settings) fn refresh_backends(&mut self, cx: &mut Context<Self>) {
        self.cloud.backends.generation += 1;
        let generation = self.cloud.backends.generation;
        self.cloud.backends.checking = true;
        self.cloud.backends.fix_error = None;
        let services = CloudServices::get(cx);
        let work = cx.background_executor().spawn(async move {
            let gh = gh_state(codespaces::auth_status(&*services.gh));
            let sbx = Sbx::check(&*services.resolve, services.home.as_deref());
            (gh, sbx)
        });
        self.cloud.backends.task = Some(cx.spawn(async move |this, cx| {
            let (gh, sbx) = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.cloud.backends.generation != generation {
                    return;
                }
                this.cloud.backends.codespaces = Some(gh);
                this.cloud.backends.sandbox = Some(sbx);
                this.cloud.backends.checking = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn start_fix(&mut self, backend: Backend, window: &mut Window, cx: &mut Context<Self>) {
        self.close_fix(cx);
        self.cloud.backends.fix_error = None;
        let (title, line) = match backend {
            Backend::Codespaces => match &self.cloud.backends.codespaces {
                Some(gh @ Gh::Missing) => ("Install the GitHub CLI", gh.fix()),
                Some(gh @ Gh::NotLoggedIn) => ("Sign in to GitHub", gh.fix()),
                Some(gh @ Gh::MissingScope(_)) => ("Add the codespace scope", gh.fix()),
                _ => return,
            },
            Backend::Sandbox => match &self.cloud.backends.sandbox {
                Some(sbx) => ("Set up ssh for Docker Sandbox", sbx.fix()),
                None => return,
            },
        };
        let Some(line) = line else {
            return;
        };
        let services = CloudServices::get(cx);
        let program = line.split_whitespace().next().unwrap_or_default();
        if (services.resolve)(program).is_none() {
            self.cloud.backends.fix_error = Some(format!(
                "Fix cannot run: `{program}` was not found on this Mac. Run `{line}` yourself in a terminal, then press Check again."
            ));
            cx.notify();
            return;
        }
        let Some(command) = FixCommand::from_line(title, line, &*services.resolve) else {
            return;
        };
        let cwd = services.home.unwrap_or_else(std::env::temp_dir);
        let view = cx.new(|cx| FixTerminal::new(command, &cwd, cx));
        let events = cx.subscribe(&view, |this, _, event: &FixEvent, cx| match event {
            FixEvent::Exited { .. } => this.refresh_backends(cx),
            FixEvent::Close => this.close_fix(cx),
        });
        window.focus(&view.focus_handle(cx), cx);
        self.cloud.backends.fix = Some(OpenFix {
            view,
            _events: events,
        });
        cx.notify();
    }

    /// Stops the fix command and drops its dialog.
    pub(in crate::settings) fn close_fix(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.cloud.backends.fix.take() {
            open.view.update(cx, |view, cx| view.shutdown(cx));
            cx.notify();
        }
    }

    /// Escape inside the fix dialog: the program owns it while the terminal has focus.
    pub(in crate::settings) fn dismiss_fix(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = &self.cloud.backends.fix else {
            return false;
        };
        if !open.view.read(cx).terminal_focused(window, cx) {
            self.close_fix(cx);
            window.focus(&self.focus, cx);
        }
        true
    }

    pub(in crate::settings) fn fix_overlay(&mut self, window: &mut Window) -> Option<AnyElement> {
        let open = self.cloud.backends.fix.as_ref()?;
        Some(crate::fix_terminal::dialog(&open.view, window))
    }

    pub(in crate::settings) fn backends_group(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let checking = self.cloud.backends.checking;
        let codespaces_row = self.backend_row(
            Backend::Codespaces,
            "GitHub Codespaces",
            self.codespace_chips(),
            self.cloud
                .backends
                .codespaces
                .as_ref()
                .is_some_and(|gh| gh.fix().is_some()),
            checking,
            cx,
        );
        let sandbox_row = self.backend_row(
            Backend::Sandbox,
            "Docker Sandbox",
            self.sandbox_chips(),
            self.cloud
                .backends
                .sandbox
                .is_some_and(|sbx| sbx.fix().is_some()),
            checking,
            cx,
        );
        let mut rows = vec![codespaces_row, sandbox_row];
        if let Some(hint) = self.fix_hint() {
            rows.push(note(hint));
        }
        if let Some(error) = self.cloud.backends.fix_error.clone() {
            rows.push(note(error));
        }
        self.group(
            Some("cloud-backends"),
            true,
            "Backends",
            Some("Sign-ins this Mac uses to create cloud workspaces".into()),
            None,
            rows,
        )
    }

    /// What Fix will run, so the user knows before pressing it. Both commands when both
    /// backends need fixing.
    pub(in crate::settings) fn fix_hint(&self) -> Option<String> {
        let codespaces = self.cloud.backends.codespaces.as_ref().and_then(Gh::fix);
        let sandbox = self.cloud.backends.sandbox.and_then(|sbx| sbx.fix());
        Some(match (codespaces, sandbox) {
            (Some(gh), Some(sbx)) => format!(
                "Fix runs `{gh}` for GitHub Codespaces and `{sbx}` for Docker Sandbox, each in a terminal, then checks again."
            ),
            (Some(line), None) | (None, Some(line)) => {
                format!("Fix runs `{line}` in a terminal, then checks again.")
            }
            (None, None) => return None,
        })
    }

    fn codespace_chips(&self) -> Vec<AnyElement> {
        let Some(gh) = &self.cloud.backends.codespaces else {
            return vec![pending_chip()];
        };
        match gh {
            Gh::Missing => vec![chip("cloud-chip-gh", false, "gh not installed".into())],
            Gh::NotLoggedIn => vec![chip("cloud-chip-gh", false, "gh not signed in".into())],
            Gh::Failed(error) => vec![chip(
                "cloud-chip-gh",
                false,
                format!("gh check failed: {error}"),
            )],
            Gh::MissingScope(user) => vec![
                chip("cloud-chip-gh", true, signed_in(user.as_deref())),
                chip(
                    "cloud-chip-gh-scope",
                    false,
                    "codespace scope missing".into(),
                ),
            ],
            Gh::Ready(user) => vec![
                chip("cloud-chip-gh", true, signed_in(user.as_deref())),
                chip("cloud-chip-gh-scope", true, "codespace scope".into()),
            ],
        }
    }

    fn sandbox_chips(&self) -> Vec<AnyElement> {
        let Some(sbx) = self.cloud.backends.sandbox else {
            return vec![pending_chip()];
        };
        let mut chips = vec![chip(
            "cloud-chip-sbx",
            sbx.installed,
            if sbx.installed {
                "sbx installed".into()
            } else {
                "sbx not installed".into()
            },
        )];
        chips.push(chip(
            "cloud-chip-sbx-ssh",
            sbx.ssh_set_up,
            if sbx.ssh_set_up {
                "ssh set up".into()
            } else {
                "ssh not set up".into()
            },
        ));
        chips.push(
            div()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child("Sign-in uses the Docker accounts below, not sbx login")
                .into_any_element(),
        );
        chips
    }

    fn backend_row(
        &mut self,
        backend: Backend,
        label: &'static str,
        chips: Vec<AnyElement>,
        fixable: bool,
        checking: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = backend.key();
        let check = secondary_button(
            SharedString::from(format!("cloud-check-{key}")),
            "Check again",
            false,
            checking,
        )
        .debug_selector(move || format!("cloud-check-{key}"))
        .child("Check again")
        .when(!checking, |d| {
            d.on_click(cx.listener(|this, _, _, cx| this.refresh_backends(cx)))
        });
        let fix = fixable.then(|| {
            secondary_button(
                SharedString::from(format!("cloud-fix-{key}")),
                "Fix",
                false,
                false,
            )
            .debug_selector(move || format!("cloud-fix-{key}"))
            .child("Fix")
            .on_click(cx.listener(move |this, _, window, cx| this.start_fix(backend, window, cx)))
        });
        let mut controls = vec![
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x_4()
                .gap_y_1()
                .children(chips)
                .into_any_element(),
        ];
        controls.extend(fix.map(IntoElement::into_any_element));
        controls.push(check.into_any_element());
        self.row(None, backend == Backend::Codespaces, label, None, controls)
    }
}

fn signed_in(user: Option<&str>) -> String {
    match user {
        Some(user) => format!("gh signed in as {user}"),
        None => "gh signed in".into(),
    }
}

fn pending_chip() -> AnyElement {
    div()
        .id("cloud-chip-checking")
        .debug_selector(|| "cloud-chip-checking".into())
        .text_size(px(12.))
        .text_color(content(0.45))
        .child("Checking...")
        .into_any_element()
}
