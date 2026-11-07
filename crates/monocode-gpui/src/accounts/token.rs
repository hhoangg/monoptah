//! Native user-controlled Claude credentials (setup-token or API key). Secrets
//! live only in the masked editor and, encrypted, in the app database, never
//! in account metadata.
use super::*;
use crate::popover::{Anchor, Menu, MenuAction, MenuEvent, MenuStyle, menu_popover};
use gpui::{Window, div, px};
use monocode_core::accounts::ClaudeCredential;

/// The open "sign-in options" dropdown and its event subscription.
pub(super) struct AuthMenu {
    menu: Entity<Menu>,
    _events: Subscription,
}

impl AuthMethod {
    /// The secret kind this selection stores, if any.
    fn credential(self) -> Option<ClaudeCredential> {
        match self {
            Self::SetupToken => Some(ClaudeCredential::SetupToken),
            Self::ApiKey => Some(ClaudeCredential::ApiKey),
            Self::Keep | Self::Browser => None,
        }
    }
    fn placeholder(self) -> &'static str {
        match self {
            Self::ApiKey => "Paste Anthropic API key",
            _ => "Paste token from `claude setup-token`",
        }
    }
    fn hint(self) -> &'static str {
        match self {
            Self::ApiKey => "Paste an Anthropic API key. Stored encrypted in the app database.",
            _ => {
                "Run claude setup-token in a terminal and paste the token. Stored encrypted in the app database."
            }
        }
    }
    pub(super) fn button_label(self, adding: bool) -> &'static str {
        match (adding, self) {
            (true, Self::SetupToken) => "Add with token",
            (true, Self::ApiKey) => "Add with API key",
            (true, _) => "Sign in and add",
            (false, Self::Browser) => "Save and sign in",
            (false, Self::SetupToken) => "Save token",
            (false, Self::ApiKey) => "Save API key",
            (false, Self::Keep) => "Save",
        }
    }
}

impl Accounts {
    /// True for the editor of a named Claude account, which offers the
    /// sign-in options menu. The default account and other providers do not.
    pub(super) fn has_auth_options(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|e| e.kind == Kind::Claude && e.id.as_deref() != Some(accounts::DEFAULT))
    }
    /// Whether the primary button can be pressed, beyond the working state.
    pub(super) fn editor_ready(&self, cx: &App) -> bool {
        let Some(editor) = &self.editor else {
            return false;
        };
        !editor.input.read(cx).content.trim().is_empty()
            && (editor.auth.credential().is_none()
                || !editor.token.read(cx).content.trim().is_empty())
    }
    pub(super) fn select_auth(&mut self, auth: AuthMethod, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.auth = auth;
        editor.menu = None;
        editor.token.update(cx, |input, cx| {
            input.set_text(String::new(), cx);
            input.set_placeholder(auth.placeholder(), cx);
        });
        self.error = None;
        cx.notify();
    }
    /// Select a sign-in method and focus the field that needs input next:
    /// the secret for token and key modes, otherwise the account name.
    pub(super) fn pick_auth(
        &mut self,
        auth: AuthMethod,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_auth(auth, cx);
        if let Some(editor) = &self.editor {
            let field = if auth.credential().is_some() {
                &editor.token
            } else {
                &editor.input
            };
            window.focus(&field.read(cx).focus_handle(cx), cx);
        }
    }
    pub(super) fn toggle_auth_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.menu.take().is_some() {
            cx.notify();
            return;
        }
        let mut items = Vec::new();
        if editor.id.is_some() {
            items.push((AuthMethod::Keep, "Keep current sign-in"));
        }
        items.extend([
            (AuthMethod::Browser, "Sign in with browser"),
            (AuthMethod::SetupToken, "Use setup-token"),
            (AuthMethod::ApiKey, "Use API key"),
        ]);
        let current = editor.auth;
        let actions = items
            .iter()
            .map(|(method, label)| {
                MenuAction::new(format!("{method:?}"), *label)
                    .checked(*method == current)
                    .into()
            })
            .collect();
        let menu = cx.new(|cx| {
            Menu::new("Sign-in options", actions, window, cx)
                .width(190.)
                .style(MenuStyle::Git)
        });
        let events = cx.subscribe_in(
            &menu,
            window,
            move |this, _, event: &MenuEvent, window, cx| {
                if let Some(editor) = &mut this.editor {
                    editor.menu = None;
                }
                if let MenuEvent::Picked(id, _) = event
                    && let Some((method, _)) =
                        items.iter().find(|(m, _)| format!("{m:?}") == id.as_ref())
                {
                    this.pick_auth(*method, window, cx);
                } else if let Some(editor) = &this.editor {
                    // Dismissed: return focus to the name field the menu took it from.
                    window.focus(&editor.input.read(cx).focus_handle(cx), cx);
                }
                cx.notify();
            },
        );
        editor.menu = Some(AuthMenu {
            menu,
            _events: events,
        });
        cx.notify();
    }
    pub(super) fn auth_menu_popover(&self, cx: &App) -> Option<crate::popover::Popover> {
        let editor = self.editor.as_ref()?;
        let open = editor.menu.as_ref()?;
        Some(
            menu_popover(&open.menu, Anchor::Slot(editor.anchor.clone()), cx)
                .align(crate::popover::Align::End),
        )
    }
    /// The hint and masked entry shown below the name row for secret modes.
    pub(super) fn secret_row(&self) -> Option<gpui::AnyElement> {
        use crate::theme::*;
        let editor = self.editor.as_ref()?;
        editor.auth.credential()?;
        Some(
            div()
                .px_4()
                .pb_2()
                .border_b_1()
                .border_color(content(0.05))
                .child(
                    div()
                        .h_8()
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.04))
                        .text_size(px(12.))
                        .when(self.working, |d| d.opacity(0.5))
                        .child(div().min_w_0().flex_1().child(editor.token.clone())),
                )
                .child(
                    div()
                        .mt_1()
                        .px(px(2.))
                        .text_size(px(10.))
                        .text_color(content(0.4))
                        .child(editor.auth.hint()),
                )
                .into_any_element(),
        )
    }
    /// Cancel for the row editor. Saving a credential cannot be interrupted.
    pub(super) fn cancel_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving_credential
            || (self.working && self.signin.is_none() && self.has_auth_options())
        {
            return;
        }
        // Only cancel a sign-in this editor started; another account's keeps running.
        let own = self
            .editor
            .as_ref()
            .zip(self.signin.as_ref())
            .is_some_and(|(editor, signin)| {
                signin.kind == editor.kind
                    && match &editor.id {
                        Some(id) => signin.profile.id == *id,
                        None => signin.adding.is_some(),
                    }
            });
        if self.working && own {
            self.cancel_auth(cx);
        } else if own {
            self.signin = None;
        }
        if let Some(focus) = self.editor.take().and_then(|e| e.previous_focus) {
            window.focus(&focus, cx);
        }
        self.error = None;
        cx.notify();
    }
    pub(super) fn submit_claude_editor(&mut self, cx: &mut Context<Self>) {
        let editor = self.editor.as_ref().unwrap();
        let label = editor.input.read(cx).content.to_string();
        if label.trim().is_empty() {
            return;
        }
        if editor.auth == AuthMethod::Keep {
            let mut book = self.book.clone();
            match book.rename(Kind::Claude, editor.id.as_deref().unwrap(), &label) {
                Ok(()) => self.persist(book, cx),
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                }
            }
            return;
        }
        let credential = editor.auth.credential();
        let token_mode = credential.is_some();
        if token_mode && editor.token.read(cx).content.trim().is_empty() {
            return;
        }
        // A browser sign-in cannot start while another account's is running:
        // refuse before renaming or changing anything.
        if !token_mode
            && self.signin.as_ref().is_some_and(|s| {
                !s.complete
                    && s.error.is_none()
                    && (editor.id.is_none() || Some(s.profile.id.as_str()) != editor.id.as_deref())
            })
        {
            self.error = Some("Finish or cancel the other sign-in first.".into());
            cx.notify();
            return;
        }
        // Validate before creating an account or changing its credentials.
        let token = if let Some(kind) = credential {
            let value = editor.token.read(cx).content.to_string();
            let checked = match kind {
                ClaudeCredential::SetupToken => accounts::normalize_claude_token(&value),
                ClaudeCredential::ApiKey => accounts::normalize_claude_api_key(&value),
            };
            match checked {
                Ok(token) => Some(token),
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let mut book = self.book.clone();
        let adding = editor.id.is_none();
        let account = if let Some(id) = &editor.id {
            if let Err(error) = book.rename(Kind::Claude, id, &label) {
                self.error = Some(error);
                cx.notify();
                return;
            }
            book.list(Kind::Claude)
                .into_iter()
                .find(|a| a.id == *id)
                .unwrap()
        } else {
            match book.add(Kind::Claude, &label) {
                Ok(account) => account,
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
            }
        };
        if !token_mode && adding {
            self.begin_auth(
                Kind::Claude,
                &account.id.clone(),
                AuthAction::SignIn,
                Some(account),
                cx,
            );
            return;
        }
        let profile = self.profile(&account.id);
        let store = self.store.clone();
        // Only a change to an existing credential (or a switch away from one)
        // affects running turns; a plain rename or first browser login does not.
        let had_credential = !adding && self.uses_claude_token(&account.id);
        let replacing = !adding && (token_mode || had_credential);
        // A sign-in of this account is moot once its authentication changes;
        // an unrelated account's sign-in must keep running.
        if token_mode || had_credential {
            let same = self
                .signin
                .as_ref()
                .is_some_and(|s| s.kind == Kind::Claude && s.profile.id == account.id);
            if same && let Some(signin) = self.signin.take() {
                signin.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.working = true;
        self.saving_credential = token_mode || had_credential;
        self.error = None;
        editor.token.update(cx, |input, cx| {
            input.set_text(String::new(), cx);
            input.set_read_only(true);
        });
        editor
            .input
            .update(cx, |input, _| input.set_read_only(true));
        if replacing {
            // Stop old turns before replacement/removal; never change a running
            // process's authentication underneath it.
            cx.emit(AccountEvent::Removing(Kind::Claude, account.id.clone()));
        }
        let work = cx.background_executor().spawn(async move {
            if replacing {
                accounts::stop_processes(Kind::Claude, &profile);
            }
            let result = (|| {
                if let (Some(token), Some(kind)) = (token, credential) {
                    profile.save_claude_credential(kind, &token)?;
                } else {
                    // Idempotent, so it runs even when the cache saw nothing.
                    profile.remove_claude_token()?;
                }
                locked(&store)?.save_provider_accounts(&book)?;
                Ok(book)
            })();
            if result.is_err() && adding {
                let _ = profile.remove_credentials(Kind::Claude);
            }
            let usable = replacing && result.is_err() && profile.env(Kind::Claude).is_ok();
            (result, usable)
        });
        cx.spawn(async move |this, cx| {
            let (result, usable): (Result<Book, String>, bool) = work.await;
            let _ = this.update(cx, |this, cx| {
                this.working = false;
                this.saving_credential = false;
                if let Some(editor) = &this.editor {
                    editor
                        .input
                        .update(cx, |input, _| input.set_read_only(false));
                    editor
                        .token
                        .update(cx, |input, _| input.set_read_only(false));
                }
                match result {
                    Ok(book) => {
                        this.book = book;
                        this.editor = None;
                        monocode_core::catalog::invalidate(
                            Kind::Claude,
                            &this.profile(&account.id),
                        );
                        this.invalidate(Kind::Claude, &account.id, cx);
                        cx.emit(AccountEvent::Updated);
                        if token_mode {
                            cx.emit(AccountEvent::Authenticated(Kind::Claude, account.id));
                        } else {
                            // If this sign-in is cancelled or fails, tell
                            // conversations the account still works.
                            if !adding {
                                this.pending_auth = Some(account.id.clone());
                            }
                            let previous = this.signin.as_ref().map(|s| s.cancel.clone());
                            this.begin_auth(
                                Kind::Claude,
                                &account.id,
                                AuthAction::SignIn,
                                None,
                                cx,
                            );
                            let started = this.signin.as_ref().is_some_and(|s| {
                                !previous.as_ref().is_some_and(|p| Arc::ptr_eq(p, &s.cancel))
                            });
                            if !started {
                                this.restore_pending_auth(cx);
                                this.error = Some("Could not start the browser sign-in.".into());
                            }
                        }
                    }
                    Err(error) => {
                        this.invalidate(Kind::Claude, &account.id, cx);
                        if usable {
                            cx.emit(AccountEvent::Authenticated(Kind::Claude, account.id));
                        }
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_labels_follow_the_selection() {
        use AuthMethod::*;
        let adding: Vec<_> = [Browser, SetupToken, ApiKey]
            .map(|m| m.button_label(true))
            .into();
        assert_eq!(
            adding,
            ["Sign in and add", "Add with token", "Add with API key"]
        );
        let editing: Vec<_> = [Keep, Browser, SetupToken, ApiKey]
            .map(|m| m.button_label(false))
            .into();
        assert_eq!(
            editing,
            ["Save", "Save and sign in", "Save token", "Save API key"]
        );
    }

    #[test]
    fn hints_describe_storage_without_the_os_credential_store() {
        use AuthMethod::*;
        assert_eq!(
            SetupToken.hint(),
            "Run claude setup-token in a terminal and paste the token. Stored encrypted in the app database."
        );
        assert_eq!(
            ApiKey.hint(),
            "Paste an Anthropic API key. Stored encrypted in the app database."
        );
    }

    #[gpui::test]
    fn defaults_selection_and_secret_clearing(cx: &mut gpui::TestAppContext) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let mut book = Book::default();
        let work = book.add(Kind::Claude, "Work").unwrap();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), book, store, cx))
            })
            .unwrap()
        });
        window
            .update(cx, |accounts, window, cx| {
                accounts.start_edit(
                    Kind::Claude,
                    Some(work.id.clone()),
                    Surface::Settings,
                    window,
                    cx,
                );
                assert_eq!(accounts.editor.as_ref().unwrap().auth, AuthMethod::Keep);
                assert!(accounts.has_auth_options());
                accounts.pick_auth(AuthMethod::ApiKey, window, cx);
                let token = accounts.editor.as_ref().unwrap().token.clone();
                // Choosing a secret mode moves focus to the secret input.
                assert!(token.read(cx).focus_handle(cx).is_focused(window));
                assert_eq!(token.read(cx).placeholder(), "Paste Anthropic API key");
                token.update(cx, |input, cx| input.set_text("sk-ant-api03-x".into(), cx));
                assert!(accounts.editor_ready(cx));
                accounts.select_auth(AuthMethod::SetupToken, cx);
                assert!(token.read(cx).content.is_empty());
                assert!(!accounts.editor_ready(cx));
                assert_eq!(
                    token.read(cx).placeholder(),
                    "Paste token from `claude setup-token`"
                );
                accounts.pick_auth(AuthMethod::Keep, window, cx);
                let name = accounts.editor.as_ref().unwrap().input.clone();
                assert!(name.read(cx).focus_handle(cx).is_focused(window));
                assert!(accounts.editor_ready(cx));
                accounts.editor = None;
                accounts.start_edit(
                    Kind::Claude,
                    Some(accounts::DEFAULT.into()),
                    Surface::Settings,
                    window,
                    cx,
                );
                assert!(!accounts.has_auth_options());
            })
            .unwrap();
    }

    #[gpui::test]
    fn claude_setup_uses_masked_entry_and_rejects_invalid_tokens_before_saving(
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
        window
            .update(cx, |accounts, window, cx| {
                accounts.start_edit(Kind::Claude, None, Surface::Settings, window, cx);
                let editor = accounts.editor.as_mut().unwrap();
                assert_eq!(editor.auth, AuthMethod::Browser); // Existing browser flow is the default.
                assert!(editor.token.read(cx).is_secret());
                editor.auth = AuthMethod::ApiKey;
                editor
                    .input
                    .update(cx, |input, cx| input.set_text("Work".into(), cx));
                editor.token.update(cx, |input, cx| {
                    input.set_text("sk-ant-oat01-private;fixture".into(), cx)
                });
                accounts.submit_editor(cx);
                assert!(!accounts.working);
                assert!(!accounts.error.as_ref().unwrap().contains("private;fixture"));
                assert_eq!(accounts.book.list(Kind::Claude).len(), 1);
                assert!(accounts.signin.is_none());
                let cancel = Arc::new(AtomicBool::new(false));
                accounts.signin = Some(SignIn {
                    kind: Kind::Claude,
                    profile: Profile::default(),
                    action: AuthAction::SignIn,
                    surface: Surface::InlineSettings,
                    cancel: cancel.clone(),
                    complete: false,
                    error: None,
                    // The sign-in of the account this editor is adding.
                    adding: Some(Book::default().add(Kind::Claude, "Adding").unwrap()),
                    started: Instant::now(),
                });
                accounts.working = true;
                accounts.cancel_editor(window, cx);
                assert!(cancel.load(Ordering::Relaxed));
                assert!(!accounts.working);
                assert!(accounts.signin.is_none());
                assert!(accounts.editor.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn another_accounts_sign_in_blocks_browser_mode_and_survives_cancel(
        cx: &mut gpui::TestAppContext,
    ) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let mut book = Book::default();
        let first = book.add(Kind::Claude, "First").unwrap();
        let second = book.add(Kind::Claude, "Second").unwrap();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| Accounts::new(PathBuf::from("/fixture"), book, store, cx))
            })
            .unwrap()
        });
        window
            .update(cx, |accounts, window, cx| {
                let cancel = Arc::new(AtomicBool::new(false));
                accounts.signin = Some(SignIn {
                    kind: Kind::Claude,
                    profile: accounts.profile(&first.id),
                    action: AuthAction::SignIn,
                    surface: Surface::InlineSettings,
                    cancel: cancel.clone(),
                    complete: false,
                    error: None,
                    adding: None,
                    started: Instant::now(),
                });
                accounts.start_edit(
                    Kind::Claude,
                    Some(second.id.clone()),
                    Surface::Settings,
                    window,
                    cx,
                );
                accounts.select_auth(AuthMethod::Browser, cx);
                accounts
                    .editor
                    .as_ref()
                    .unwrap()
                    .input
                    .update(cx, |input, cx| input.set_text("Renamed".into(), cx));
                accounts.submit_editor(cx);
                assert_eq!(
                    accounts.error.as_deref(),
                    Some("Finish or cancel the other sign-in first.")
                );
                assert!(!accounts.working);
                let label = |accounts: &Accounts| {
                    accounts
                        .book
                        .list(Kind::Claude)
                        .into_iter()
                        .find(|a| a.id == second.id)
                        .unwrap()
                        .label
                };
                assert_eq!(label(accounts), "Second");
                // Cancelling this editor leaves the other account's sign-in alone.
                accounts.cancel_editor(window, cx);
                assert!(accounts.editor.is_none());
                assert!(accounts.signin.is_some());
                assert!(!cancel.load(Ordering::Relaxed));
            })
            .unwrap();
    }
}
