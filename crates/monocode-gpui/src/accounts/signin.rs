use super::*;
use crate::theme::*;
use gpui::{AnyElement, FontWeight, Role, SharedString, div, px};

/// Shown in code mode when the CLI gave no usable code page link. Nothing was
/// opened, and the CLI opens nothing in this mode.
const NO_CODE_PAGE: &str = "The code page link is not available. Use browser sign-in or try again.";
/// Code mode: the app, not the CLI, opens the hosted code page, once per attempt.
fn open_once(mode: accounts::SignInMode, prompt: &mut CodePrompt, cx: &mut Context<Accounts>) {
    if mode == accounts::SignInMode::Code
        && !prompt.opened
        && let Some(url) = &prompt.url
    {
        prompt.opened = true;
        cx.open_url(url);
    }
}

impl Accounts {
    /// The CLI reported progress on attempt `attempt`: show or update the code
    /// entry, or surface a code it refused. Events from an older attempt are ignored.
    pub(super) fn sign_in_progress(
        &mut self,
        attempt: &Arc<AtomicBool>,
        sender: std::sync::mpsc::Sender<String>,
        event: accounts::AuthProgress,
        cx: &mut Context<Self>,
    ) {
        let Some(signin) = &mut self.signin else {
            return;
        };
        if !Arc::ptr_eq(&signin.cancel, attempt) || signin.complete || signin.error.is_some() {
            return;
        }
        let mode = signin.mode;
        match event {
            accounts::AuthProgress::CodePrompt { url } => match &mut signin.code {
                Some(prompt) => {
                    // The CLI prints two authorize URLs; only the hosted code
                    // page may be kept or opened, never the localhost one.
                    if let Some(url) = url.as_deref().and_then(accounts::manual_sign_in_url) {
                        prompt.url = Some(url);
                    }
                    open_once(mode, prompt, cx);
                }
                None => {
                    let input = cx.new(|cx| {
                        let mut input = Input::field(
                            "Verification code",
                            "Paste the code from your browser",
                            content(0.25),
                            cx,
                        );
                        input.set_secret(true);
                        input.set_max_chars(2100);
                        input
                    });
                    let submit = cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
                        this.submit_code(cx)
                    });
                    let changed = cx.subscribe(&input, |this, _, _: &input::Changed, _| {
                        if let Some(prompt) = this.signin.as_mut().and_then(|s| s.code.as_mut()) {
                            prompt.interactions += 1;
                        }
                    });
                    let url = url.as_deref().and_then(accounts::manual_sign_in_url);
                    let mut prompt = CodePrompt {
                        error: (mode == accounts::SignInMode::Code && url.is_none())
                            .then(|| NO_CODE_PAGE.into()),
                        url,
                        opened: false,
                        interactions: 0,
                        input,
                        sender,
                        checking: false,
                        focused: false,
                        _submit: submit,
                        _changed: changed,
                    };
                    open_once(mode, &mut prompt, cx);
                    signin.code = Some(prompt);
                }
            },
            accounts::AuthProgress::CodeRejected(message) => {
                if let Some(prompt) = &mut signin.code {
                    prompt.checking = false;
                    prompt.interactions += 1;
                    prompt.error = Some(message);
                }
            }
        }
        cx.notify();
    }
    /// Send the pasted code to the running CLI. The field is emptied at once;
    /// the code is never kept, logged or copied.
    pub(crate) fn submit_code(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.signin.as_mut().and_then(|s| s.code.as_mut()) else {
            return;
        };
        if prompt.checking || prompt.input.read(cx).is_composing() {
            return;
        }
        prompt.interactions += 1;
        match prompt.input.update(cx, |input, cx| input.take(cx)) {
            None => prompt.error = Some("Paste the code shown in your browser.".into()),
            Some(value) => {
                if prompt.sender.send(value).is_ok() {
                    prompt.error = None;
                    prompt.checking = true;
                } else {
                    prompt.error = Some("Sign-in already ended. Start it again.".into());
                }
            }
        }
        cx.notify();
    }
    /// A code-mode sign-in is running: the browser is not what it waits for.
    pub(super) fn code_mode_running(&self) -> bool {
        self.signin.as_ref().is_some_and(|s| {
            s.mode == accounts::SignInMode::Code && !s.complete && s.error.is_none()
        })
    }
    /// Open the hosted code page again. Checked once more here, so a localhost
    /// callback URL can never be opened even if one were stored by mistake.
    pub(super) fn reopen_sign_in_page(&self, cx: &mut Context<Self>) {
        if let Some(url) = self
            .signin
            .as_ref()
            .and_then(|s| s.code.as_ref())
            .and_then(|p| p.url.as_deref())
            .and_then(accounts::manual_sign_in_url)
        {
            cx.open_url(&url);
        }
    }
    /// The code entry for a sign-in that is waiting on a pasted verification
    /// code: one helper for the Settings dialog, the inline add-account editor
    /// and the footer panel. `prefix` keeps element ids unique per surface.
    pub(super) fn code_entry(
        &self,
        prefix: &'static str,
        with_cancel: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self.signin.as_ref()?.code.as_ref()?;
        let id = |name: &str| SharedString::from(format!("{prefix}-code-{name}"));
        let checking = prompt.checking;
        let has_url = prompt.url.is_some();
        let code_mode = self
            .signin
            .as_ref()
            .is_some_and(|s| s.mode == accounts::SignInMode::Code);
        let link = |id: SharedString, label: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(content(0.55))
                .cursor_pointer()
                .hover(|s| s.text_color(content(0.9)))
                .child(label)
        };
        Some(
            div()
                .id(id("entry"))
                .role(Role::Group)
                .aria_label("Verification code")
                .w_full()
                .mt_3()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(content(0.55))
                        .child(if code_mode {
                            "Sign in in your browser, then paste the code it shows here."
                        } else {
                            "Browser showing a code? Paste it here to finish signing in."
                        }),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .h_8()
                                .min_w_0()
                                .flex_1()
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .rounded_lg()
                                .bg(content(0.04))
                                .border_1()
                                .border_color(content(0.1))
                                .text_size(px(12.))
                                .when(checking, |d| d.opacity(0.55))
                                .child(div().min_w_0().flex_1().child(prompt.input.clone())),
                        )
                        .child(
                            crate::settings::widgets::primary_button(
                                id("submit"),
                                if checking {
                                    "Checking code…"
                                } else {
                                    "Submit"
                                },
                                checking,
                            )
                            .h_8()
                            .px(px(12.))
                            .rounded_lg()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .child(if checking {
                                "Checking code…"
                            } else {
                                "Submit"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.submit_code(cx))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .when(has_url, |d| {
                            d.child(link(id("reopen"), "Open sign-in page again").on_click(
                                cx.listener(|this, _, _, cx| this.reopen_sign_in_page(cx)),
                            ))
                        })
                        .when(with_cancel, |d| {
                            d.child(link(id("cancel"), "Cancel").on_click(cx.listener(
                                |this, _, window, cx| {
                                    if this.has_modal_auth() {
                                        this.close_auth(window, cx);
                                    } else {
                                        this.cancel_auth(cx);
                                    }
                                },
                            )))
                        }),
                )
                .children(prompt.error.clone().map(|error| {
                    div()
                        .id(id("error"))
                        .role(Role::Alert)
                        .text_size(px(10.))
                        .line_height(px(16.))
                        .text_color(gpui::rgb(0xf87171))
                        .child(error)
                }))
                .into_any_element(),
        )
    }
    /// The code entry for the inline add-account editor, when its sign-in has one.
    pub(super) fn inline_code_entry(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.signin
            .as_ref()
            .filter(|s| s.adding.is_some())
            .and_then(|_| self.code_entry("account-editor", true, cx))
    }
    pub(crate) fn has_modal_auth(&self) -> bool {
        self.signin
            .as_ref()
            .is_some_and(|s| s.surface == Surface::Settings)
    }
    #[cfg(test)]
    pub(crate) fn open_modal_signin_for_test(&mut self, cx: &mut Context<Self>) {
        self.signin = Some(SignIn {
            kind: Kind::Codex,
            profile: Profile::default(),
            action: AuthAction::SignIn,
            mode: accounts::SignInMode::Browser,
            surface: Surface::Settings,
            cancel: Arc::new(AtomicBool::new(false)),
            complete: false,
            error: None,
            adding: None,
            started: Instant::now(),
            code: None,
        });
        cx.notify();
    }
    pub(crate) fn has_footer_auth(&self, kind: Kind, id: &str) -> bool {
        self.footer_signin(kind, id).is_some()
    }
    fn footer_signin(&self, kind: Kind, id: &str) -> Option<&SignIn> {
        self.signin.as_ref().filter(|s| {
            s.surface == Surface::Footer
                && s.kind == kind
                && s.profile.id == id
                && s.action == AuthAction::SignIn
                && s.adding.is_none()
        })
    }
    /// ProviderSignInPanel.tsx, embedded in the usage popover.
    pub(crate) fn footer_auth_panel(
        &self,
        kind: Kind,
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if kind == Kind::Claude && self.uses_claude_token(id) {
            return div().p_4().flex().flex_col().gap_3()
                .child("This account uses a token or API key. If Claude Code reports an authentication error, replace it in Accounts.")
                .child(crate::settings::widgets::secondary_button("manage-claude-token", "Manage sign-in", false, self.working)
                    .child("Manage sign-in")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(AccountEvent::Manage))))
                .into_any_element();
        }
        let signin = self.footer_signin(kind, id);
        let complete = signin.is_some_and(|s| s.complete);
        let error = signin.and_then(|s| s.error.clone());
        let running = signin.is_some_and(|s| !s.complete && s.error.is_none());
        let auth_running = self
            .signin
            .as_ref()
            .is_some_and(|s| !s.complete && s.error.is_none());
        let disabled = auth_running || complete || self.working;
        let label: SharedString = if running {
            if self.code_mode_running() {
                "Waiting for code…".into()
            } else {
                "Waiting for browser…".into()
            }
        } else if complete {
            "Signed in".into()
        } else {
            format!("Sign in to {}", kind.label()).into()
        };
        let click_id = id.to_owned();
        let keyboard_id = id.to_owned();
        let mut button_text = shell_fill();
        button_text.a = 1.;
        div()
            .id("footer-provider-signin-panel")
            .role(Role::Status)
            .aria_label(if complete {
                "Signed in"
            } else {
                "Authentication required"
            })
            .min_h(px(272.))
            .px(px(20.))
            .py(px(24.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_color(content(1.))
            .child(
                div()
                    .size(px(64.))
                    .flex_shrink_0()
                    .rounded(px(16.))
                    .bg(content(0.06))
                    .border_1()
                    .border_color(content(0.08))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(provider_icon(kind.id(), 36.)),
            )
            .child(
                div()
                    .mt(px(14.))
                    .text_size(px(15.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_center()
                    .child(if complete {
                        format!("Signed in to {}", kind.label())
                    } else {
                        "Authentication required".into()
                    }),
            )
            .child(
                div()
                    .mt_1()
                    .max_w(px(224.))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(content(0.45))
                    .text_center()
                    .child(if complete {
                        "You can retry your last message now.".into()
                    } else {
                        format!("Sign in to continue using {}.", kind.label())
                    }),
            )
            .child(
                div()
                    .id("footer-provider-auth-action")
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .track_focus(&self.dialog_focus)
                    .tab_index(0)
                    .mt_4()
                    .h(px(32.))
                    .px(px(14.))
                    .rounded_lg()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .bg(content(1.))
                    .text_color(button_text)
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .when(disabled, |d| d.opacity(0.55))
                    .when(!disabled, |d| {
                        d.cursor_pointer().hover(|s| s.bg(content(0.85)))
                    })
                    .when(running, |d| {
                        d.child(crate::pulse::looping(
                            "footer-provider-auth-spinner",
                            std::time::Duration::from_secs(1),
                            crate::pulse::Rate::Smooth,
                            move |delta| {
                                icon("refresh-cw", 14.)
                                    .text_color(button_text)
                                    .with_transformation(gpui::Transformation::rotate(
                                        gpui::percentage(delta),
                                    ))
                            },
                        ))
                    })
                    .when(complete, |d| {
                        d.child(icon("check", 14.).text_color(button_text))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !disabled {
                            this.begin_footer_auth(kind, &click_id, cx);
                        }
                    }))
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        if !disabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.begin_footer_auth(kind, &keyboard_id, cx);
                            cx.stop_propagation();
                        }
                    })),
            )
            .when(
                super::token::offers_code_sign_in(kind)
                    && !auth_running
                    && !complete
                    && !self.working,
                |d| {
                    let code_id = id.to_owned();
                    let key_id = id.to_owned();
                    d.child(
                        div()
                            .id("footer-provider-auth-with-code")
                            .role(Role::Button)
                            .aria_label("Sign in with code")
                            .tab_index(0)
                            .mt(px(8.))
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(4.))
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(content(0.5))
                            .cursor_pointer()
                            .hover(|s| s.text_color(content(0.8)))
                            .child("Sign in with code")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.begin_footer_auth_with(
                                    kind,
                                    &code_id,
                                    accounts::SignInMode::Code,
                                    cx,
                                );
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        this.begin_footer_auth_with(
                                            kind,
                                            &key_id,
                                            accounts::SignInMode::Code,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    }
                                },
                            )),
                    )
                },
            )
            .children(signin.and_then(|_| self.code_entry("footer-provider-signin", false, cx)))
            // The CLI cannot see a closed browser tab; let the user give up
            // on a waiting sign-in (from any surface) and start again.
            .when(auth_running && !complete, |d| {
                d.child(
                    div()
                        .id("footer-provider-auth-cancel")
                        .role(Role::Button)
                        .aria_label("Cancel sign-in")
                        .mt(px(8.))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(content(0.5))
                        .cursor_pointer()
                        .hover(|s| s.text_color(content(0.8)))
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_auth(cx))),
                )
            })
            .children(error.map(|error| {
                div()
                    .id("footer-provider-signin-error")
                    .role(Role::Alert)
                    .mt(px(10.))
                    .max_w(px(240.))
                    .text_size(px(10.))
                    .line_height(px(16.))
                    .text_color(gpui::rgb(0xf87171))
                    .text_center()
                    .child(error)
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn settings_add_signin_stays_inline_after_completion(cx: &mut gpui::TestAppContext) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let view =
            cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx));
        view.update(cx, |accounts, cx| {
            accounts.signin = Some(SignIn {
                kind: Kind::Codex,
                profile: Profile::default(),
                action: AuthAction::SignIn,
                mode: accounts::SignInMode::Browser,
                surface: Surface::InlineSettings,
                cancel: Arc::new(AtomicBool::new(false)),
                complete: false,
                error: None,
                adding: None,
                started: std::time::Instant::now(),
                code: None,
            });
            assert!(!accounts.has_modal_auth());
            accounts.signin.as_mut().unwrap().complete = true;
            assert!(!accounts.has_modal_auth());
            assert!(!accounts.has_footer_auth(Kind::Codex, accounts::DEFAULT));
            let _ = cx;
        });
    }
    #[gpui::test]
    fn footer_signin_never_opens_a_modal_or_steals_window_focus(cx: &mut gpui::TestAppContext) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx))
            })
            .unwrap()
        });
        let previous = window
            .update(cx, |accounts, window, cx| {
                let previous = cx.focus_handle();
                window.focus(&previous, cx);
                accounts.signin = Some(SignIn {
                    kind: Kind::Codex,
                    profile: Profile::default(),
                    action: AuthAction::SignIn,
                    mode: accounts::SignInMode::Browser,
                    surface: Surface::Footer,
                    cancel: Arc::new(AtomicBool::new(false)),
                    complete: false,
                    error: None,
                    adding: None,
                    started: std::time::Instant::now(),
                    code: None,
                });
                cx.notify();
                previous
            })
            .unwrap();
        for (complete, error) in [(false, None), (false, Some("Retry sign-in")), (true, None)] {
            window
                .update(cx, |accounts, _, cx| {
                    let signin = accounts.signin.as_mut().unwrap();
                    signin.complete = complete;
                    signin.error = error.map(str::to_owned);
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            window
                .update(cx, |accounts, window, cx| {
                    assert!(previous.is_focused(window));
                    assert!(!accounts.dialog_presented);
                    assert!(!accounts.has_modal_auth());
                    assert!(accounts.has_footer_auth(Kind::Codex, accounts::DEFAULT));
                    assert!(!accounts.has_footer_auth(Kind::Claude, accounts::DEFAULT));
                    assert!(!accounts.has_footer_auth(Kind::Codex, "another-account"));
                    let _ = accounts.footer_auth_panel(Kind::Codex, accounts::DEFAULT, cx);
                })
                .unwrap();
        }
        // The existing Settings presentation still owns its dialog focus.
        window
            .update(cx, |accounts, _, cx| {
                accounts.signin.as_mut().unwrap().surface = Surface::Settings;
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
        window
            .update(cx, |accounts, window, _| {
                assert!(accounts.dialog_presented);
                assert!(accounts.dialog_focus.is_focused(window));
            })
            .unwrap();
    }
    #[gpui::test]
    fn returning_to_the_app_resets_a_sign_in_the_browser_abandoned(cx: &mut gpui::TestAppContext) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx))
            })
            .unwrap()
        });
        activate(&window, cx);
        let waiting = |complete| SignIn {
            kind: Kind::Claude,
            profile: Profile::default(),
            action: AuthAction::SignIn,
            mode: accounts::SignInMode::Browser,
            surface: Surface::Footer,
            cancel: Arc::new(AtomicBool::new(false)),
            complete,
            error: None,
            adding: None,
            started: std::time::Instant::now() - std::time::Duration::from_secs(30),
            code: None,
        };
        window
            .update(cx, |accounts, window, cx| {
                accounts.signin = Some(waiting(false));
                accounts.app_reactivated(window, cx);
            })
            .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        window
            .update(cx, |accounts, _, _| {
                assert!(accounts.signin.is_none(), "an abandoned sign-in resets");
                assert!(!accounts.has_footer_auth(Kind::Claude, accounts::DEFAULT));
            })
            .unwrap();
        // A sign-in that finished while the user was away is left alone.
        window
            .update(cx, |accounts, window, cx| {
                accounts.signin = Some(waiting(true));
                accounts.app_reactivated(window, cx);
            })
            .unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(6));
        cx.run_until_parked();
        window
            .update(cx, |accounts, _, _| assert!(accounts.signin.is_some()))
            .unwrap();
    }

    fn code_sign_in(surface: Surface, adding: Option<Account>) -> (SignIn, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let signin = SignIn {
            kind: Kind::Claude,
            profile: Profile::default(),
            action: AuthAction::SignIn,
            mode: accounts::SignInMode::Browser,
            surface,
            cancel: cancel.clone(),
            complete: false,
            error: None,
            adding,
            started: std::time::Instant::now() - std::time::Duration::from_secs(30),
            code: None,
        };
        (signin, cancel)
    }
    const MANUAL: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=fake-client&response_type=code&redirect_uri=https%3A%2F%2Fplatform.example.test%2Foauth%2Fcode%2Fcallback&code_challenge=fake-challenge&state=fake-state";
    const MANUAL_OTHER: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=fake-client&response_type=code&redirect_uri=https%3A%2F%2Fplatform.example.test%2Foauth%2Fcode%2Fcallback&code_challenge=fake-challenge&state=other-state";
    const AUTOMATIC: &str = "https://claude.com/cai/oauth/authorize?client_id=fake-client&response_type=code&redirect_uri=http%3A%2F%2Flocalhost%3A54321%2Fcallback&code_challenge=fake-challenge&state=fake-state";
    /// A window whose sign-in (in `mode`) just received the CLI's code prompt
    /// carrying `urls` one after another.
    fn prompted(
        cx: &mut gpui::TestAppContext,
        mode: accounts::SignInMode,
        urls: &[Option<&str>],
    ) -> gpui::WindowHandle<Accounts> {
        let window = code_window(cx);
        let (sender, _received) = std::sync::mpsc::channel();
        window
            .update(cx, |accounts, _, cx| {
                let (mut signin, cancel) = code_sign_in(Surface::Footer, None);
                signin.mode = mode;
                accounts.signin = Some(signin);
                for url in urls {
                    accounts.sign_in_progress(
                        &cancel,
                        sender.clone(),
                        accounts::AuthProgress::CodePrompt {
                            url: url.map(str::to_owned),
                        },
                        cx,
                    );
                }
            })
            .unwrap();
        window
    }
    fn prompt_state(
        window: &gpui::WindowHandle<Accounts>,
        cx: &mut gpui::TestAppContext,
    ) -> (Option<String>, bool, Option<String>) {
        window
            .update(cx, |accounts, _, _| {
                let prompt = accounts.signin.as_ref().unwrap().code.as_ref().unwrap();
                (prompt.url.clone(), prompt.opened, prompt.error.clone())
            })
            .unwrap()
    }
    #[gpui::test]
    fn code_mode_opens_the_manual_url_once(cx: &mut gpui::TestAppContext) {
        let window = prompted(cx, accounts::SignInMode::Code, &[Some(MANUAL)]);
        assert_eq!(cx.opened_url().as_deref(), Some(MANUAL));
        let (url, opened, error) = prompt_state(&window, cx);
        assert_eq!((url.as_deref(), opened, error), (Some(MANUAL), true, None));
        // A repeated prompt, even with another link, opens nothing more.
        let (sender, _received) = std::sync::mpsc::channel();
        window
            .update(cx, |accounts, _, cx| {
                let cancel = accounts.signin.as_ref().unwrap().cancel.clone();
                accounts.sign_in_progress(
                    &cancel,
                    sender,
                    accounts::AuthProgress::CodePrompt {
                        url: Some(MANUAL_OTHER.into()),
                    },
                    cx,
                );
            })
            .unwrap();
        assert_eq!(cx.opened_url().as_deref(), Some(MANUAL));
    }
    #[gpui::test]
    fn code_mode_never_opens_the_localhost_url(cx: &mut gpui::TestAppContext) {
        for url in [Some(AUTOMATIC), None] {
            let window = prompted(cx, accounts::SignInMode::Code, &[url]);
            assert_eq!(cx.opened_url(), None);
            let (url, opened, error) = prompt_state(&window, cx);
            assert!(url.is_none() && !opened);
            assert_eq!(error.as_deref(), Some(NO_CODE_PAGE));
        }
    }
    #[gpui::test]
    fn browser_mode_opens_nothing_from_the_app(cx: &mut gpui::TestAppContext) {
        let window = prompted(cx, accounts::SignInMode::Browser, &[Some(MANUAL)]);
        assert_eq!(cx.opened_url(), None);
        let (url, opened, error) = prompt_state(&window, cx);
        assert_eq!((url.as_deref(), opened, error), (Some(MANUAL), false, None));
        // The localhost URL is dropped, not offered.
        let window = prompted(cx, accounts::SignInMode::Browser, &[Some(AUTOMATIC)]);
        assert_eq!(prompt_state(&window, cx).0, None);
        assert_eq!(cx.opened_url(), None);
    }
    #[gpui::test]
    fn open_sign_in_page_again_opens_the_manual_url_and_refuses_localhost(
        cx: &mut gpui::TestAppContext,
    ) {
        for mode in [accounts::SignInMode::Browser, accounts::SignInMode::Code] {
            let window = prompted(cx, mode, &[Some(MANUAL)]);
            window
                .update(cx, |accounts, _, cx| {
                    // Code mode already opened it: reopening shows it again.
                    accounts.reopen_sign_in_page(cx);
                })
                .unwrap();
            assert_eq!(cx.opened_url().as_deref(), Some(MANUAL));
            // Even a stored localhost URL is refused at open time.
            window
                .update(cx, |accounts, _, cx| {
                    let prompt = accounts.signin.as_mut().unwrap().code.as_mut().unwrap();
                    prompt.url = Some(AUTOMATIC.into());
                    accounts.reopen_sign_in_page(cx);
                })
                .unwrap();
            assert_eq!(cx.opened_url().as_deref(), Some(MANUAL));
        }
    }
    fn new_account() -> Account {
        Book::default().add(Kind::Claude, "Work").unwrap()
    }
    #[gpui::test]
    fn code_prompt_renders_on_every_surface_and_submit_sends_then_clears(
        cx: &mut gpui::TestAppContext,
    ) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx))
            })
            .unwrap()
        });
        let (sender, received) = std::sync::mpsc::channel();
        for (surface, adding) in [
            (Surface::Settings, None),
            (Surface::InlineSettings, Some(new_account())),
            (Surface::Footer, None),
            (Surface::Footer, Some(new_account())),
        ] {
            window
                .update(cx, |accounts, _, cx| {
                    let (signin, cancel) = code_sign_in(surface, adding.clone());
                    accounts.signin = Some(signin);
                    assert!(accounts.code_entry("t", true, cx).is_none());
                    accounts.sign_in_progress(
                        &cancel,
                        sender.clone(),
                        accounts::AuthProgress::CodePrompt {
                            url: Some("https://example.test/signin".into()),
                        },
                        cx,
                    );
                    // A stale attempt cannot touch this one.
                    accounts.sign_in_progress(
                        &Arc::new(AtomicBool::new(false)),
                        sender.clone(),
                        accounts::AuthProgress::CodeRejected("stale".into()),
                        cx,
                    );
                    assert!(
                        accounts
                            .signin
                            .as_ref()
                            .unwrap()
                            .code
                            .as_ref()
                            .unwrap()
                            .error
                            .is_none()
                    );
                    assert!(accounts.code_entry("t", true, cx).is_some());
                    match (surface, adding.is_some()) {
                        (Surface::Footer, false) => {
                            let _ = accounts.footer_auth_panel(Kind::Claude, accounts::DEFAULT, cx);
                        }
                        (Surface::Settings, _) => assert!(accounts.has_modal_auth()),
                        _ => assert!(accounts.inline_code_entry(cx).is_some()),
                    }
                    cx.notify();
                })
                .unwrap();
            cx.refresh().unwrap();
            window
                .update(cx, |accounts, _, cx| {
                    let input = accounts
                        .signin
                        .as_ref()
                        .unwrap()
                        .code
                        .as_ref()
                        .unwrap()
                        .input
                        .clone();
                    // Nothing to send yet.
                    accounts.submit_code(cx);
                    assert!(received.try_recv().is_err());
                    assert!(
                        accounts
                            .signin
                            .as_ref()
                            .unwrap()
                            .code
                            .as_ref()
                            .unwrap()
                            .error
                            .is_some()
                    );
                    input.update(cx, |input, cx| {
                        assert!(input.is_secret());
                        input.set_text("fake-code#fake-state".into(), cx)
                    });
                    accounts.submit_code(cx);
                    assert_eq!(received.try_recv().unwrap(), "fake-code#fake-state");
                    assert!(input.read(cx).content.is_empty());
                    let prompt = accounts.signin.as_ref().unwrap().code.as_ref().unwrap();
                    assert!(prompt.checking && prompt.error.is_none());
                    // Disabled while checking.
                    input.update(cx, |input, cx| input.set_text("fake-b#fake-c".into(), cx));
                    accounts.submit_code(cx);
                    assert!(received.try_recv().is_err());
                    let cancel = accounts.signin.as_ref().unwrap().cancel.clone();
                    accounts.sign_in_progress(
                        &cancel,
                        sender.clone(),
                        accounts::AuthProgress::CodeRejected("Copy the whole code".into()),
                        cx,
                    );
                    let prompt = accounts.signin.as_ref().unwrap().code.as_ref().unwrap();
                    assert!(!prompt.checking);
                    assert_eq!(prompt.error.as_deref(), Some("Copy the whole code"));
                })
                .unwrap();
            cx.refresh().unwrap();
        }
    }
    fn code_window(cx: &mut gpui::TestAppContext) -> gpui::WindowHandle<Accounts> {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx))
            })
            .unwrap()
        });
        activate(&window, cx);
        window
    }
    fn activate(window: &gpui::WindowHandle<Accounts>, cx: &mut gpui::TestAppContext) {
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |_, window, _| assert!(window.is_window_active()))
            .unwrap();
    }
    /// Start a sign-in showing the code field, then report the app returning.
    fn reactivate_with_prompt(
        window: &gpui::WindowHandle<Accounts>,
        cx: &mut gpui::TestAppContext,
        typed: bool,
        checking: bool,
    ) {
        let (sender, _received) = std::sync::mpsc::channel();
        window
            .update(cx, |accounts, window, cx| {
                let (signin, cancel) = code_sign_in(Surface::Footer, None);
                accounts.signin = Some(signin);
                accounts.sign_in_progress(
                    &cancel,
                    sender,
                    accounts::AuthProgress::CodePrompt { url: None },
                    cx,
                );
                let prompt = accounts.signin.as_mut().unwrap().code.as_mut().unwrap();
                prompt.checking = checking;
                if typed {
                    prompt
                        .input
                        .update(cx, |input, cx| input.set_text("fake-code#fake".into(), cx));
                }
                accounts.app_reactivated(window, cx);
            })
            .unwrap();
    }
    fn advance(cx: &mut gpui::TestAppContext, secs: u64) {
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(secs));
        cx.run_until_parked();
    }
    fn has_signin(window: &gpui::WindowHandle<Accounts>, cx: &mut gpui::TestAppContext) -> bool {
        window
            .update(cx, |accounts, _, _| accounts.signin.is_some())
            .unwrap()
    }
    #[gpui::test]
    fn returning_to_the_app_gives_a_code_prompt_a_long_grace_then_resets_an_idle_one(
        cx: &mut gpui::TestAppContext,
    ) {
        let window = code_window(cx);
        // (text typed into the field, a code already sent and being checked)
        for (typed, checking, survives) in [
            (false, false, false),
            (true, false, true),
            (false, true, true),
        ] {
            reactivate_with_prompt(&window, cx, typed, checking);
            advance(cx, 6);
            assert!(has_signin(&window, cx), "survives the short grace");
            advance(cx, CODE_PROMPT_GRACE.as_secs() + 1);
            assert_eq!(
                has_signin(&window, cx),
                survives,
                "typed={typed} checking={checking}"
            );
        }
    }
    #[gpui::test]
    fn a_later_return_to_the_app_voids_the_earlier_reset_timer(cx: &mut gpui::TestAppContext) {
        let window = code_window(cx);
        reactivate_with_prompt(&window, cx, false, false);
        advance(cx, 60);
        window
            .update(cx, |accounts, window, cx| {
                accounts.app_reactivated(window, cx)
            })
            .unwrap();
        // The first timer is due at 90 s: it must do nothing.
        advance(cx, 40);
        assert!(has_signin(&window, cx));
        // Leaving the app voids the second one too.
        window
            .update(cx, |accounts, _, _| accounts.app_deactivated())
            .unwrap();
        advance(cx, CODE_PROMPT_GRACE.as_secs() + 1);
        assert!(has_signin(&window, cx));
    }
    #[gpui::test]
    fn a_focused_code_field_keeps_the_sign_in_alive(cx: &mut gpui::TestAppContext) {
        let window = code_window(cx);
        reactivate_with_prompt(&window, cx, false, false);
        window
            .update(cx, |accounts, window, cx| {
                let handle = accounts
                    .signin
                    .as_ref()
                    .unwrap()
                    .code
                    .as_ref()
                    .unwrap()
                    .input
                    .read(cx)
                    .focus_handle(cx);
                window.focus(&handle, cx);
            })
            .unwrap();
        advance(cx, CODE_PROMPT_GRACE.as_secs() + 1);
        assert!(has_signin(&window, cx));
    }
    #[gpui::test]
    fn inline_add_account_editor_renders_the_code_prompt(cx: &mut gpui::TestAppContext) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), Book::default(), store, cx))
            })
            .unwrap()
        });
        let (sender, _received) = std::sync::mpsc::channel();
        window
            .update(cx, |accounts, window, cx| {
                accounts.start_edit(Kind::Claude, None, Surface::Settings, window, cx);
                let (signin, cancel) = code_sign_in(Surface::InlineSettings, Some(new_account()));
                accounts.signin = Some(signin);
                assert!(accounts.inline_code_entry(cx).is_none());
                accounts.sign_in_progress(
                    &cancel,
                    sender,
                    accounts::AuthProgress::CodePrompt { url: None },
                    cx,
                );
                assert!(accounts.inline_code_entry(cx).is_some());
                // The editor row is built with the entry in it.
                let _ = accounts.settings_rows(cx);
                cx.notify();
            })
            .unwrap();
        cx.refresh().unwrap();
    }

    #[gpui::test]
    fn a_rejected_code_keeps_the_sign_in_the_user_is_using(cx: &mut gpui::TestAppContext) {
        let window = code_window(cx);
        reactivate_with_prompt(&window, cx, false, false);
        advance(cx, 60);
        // The user submits and the CLI refuses: the field is empty and unfocused.
        let (sender, _received) = std::sync::mpsc::channel();
        window
            .update(cx, |accounts, _, cx| {
                let cancel = accounts.signin.as_ref().unwrap().cancel.clone();
                accounts.sign_in_progress(
                    &cancel,
                    sender,
                    accounts::AuthProgress::CodeRejected("Copy the whole code".into()),
                    cx,
                );
            })
            .unwrap();
        // The timer started at the return to the app fires now.
        advance(cx, 40);
        assert!(has_signin(&window, cx), "an active sign-in is not reset");
    }
    /// The mode chosen in the UI is stored on the sign-in at once. Every attempt
    /// is cancelled inside the same `update`, before any queued task can run,
    /// and a cancelled attempt never starts the CLI (core checks the flag
    /// first), so this never runs a real login.
    #[gpui::test]
    fn each_sign_in_choice_stores_its_mode(cx: &mut gpui::TestAppContext) {
        use accounts::SignInMode::{Browser, Code};
        let window = code_window(cx);
        window
            .update(cx, |accounts, window, cx| {
                let mode = |accounts: &Accounts| accounts.signin.as_ref().map(|s| s.mode);
                for (surface, code) in [(Surface::Footer, true), (Surface::Footer, false)] {
                    accounts.start_edit(Kind::Claude, None, surface, window, cx);
                    accounts
                        .editor
                        .as_ref()
                        .unwrap()
                        .input
                        .update(cx, |input, cx| input.set_text("Work".into(), cx));
                    accounts.submit_footer_editor(code, cx);
                    assert_eq!(mode(accounts), Some(if code { Code } else { Browser }));
                    accounts.cancel_auth(cx);
                }
                // The Settings editor follows the picked sign-in option.
                for (method, expected) in [
                    (AuthMethod::BrowserCode, Code),
                    (AuthMethod::Browser, Browser),
                ] {
                    accounts.start_edit(Kind::Claude, None, Surface::Settings, window, cx);
                    accounts
                        .editor
                        .as_ref()
                        .unwrap()
                        .input
                        .update(cx, |input, cx| input.set_text("Work".into(), cx));
                    accounts.pick_auth(method, window, cx);
                    accounts.submit_editor(cx);
                    assert_eq!(mode(accounts), Some(expected));
                    accounts.cancel_auth(cx);
                }
                // Existing-account footer sign-in.
                accounts.begin_footer_auth_with(Kind::Claude, accounts::DEFAULT, Code, cx);
                assert_eq!(mode(accounts), Some(Code));
                accounts.cancel_auth(cx);
                accounts.begin_footer_auth(Kind::Claude, accounts::DEFAULT, cx);
                assert_eq!(mode(accounts), Some(Browser));
                accounts.cancel_auth(cx);
            })
            .unwrap();
    }
}
