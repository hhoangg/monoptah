//! Background session names never join or rewrite the user's provider history.
use super::*;
use monocode_core::session_title::display_title;

impl Conversation {
    /// Tauri's sessionDisplayTitle: the harness prefix belongs to persistence,
    /// while every conversation headline shows only the user's title.
    pub(crate) fn display_title(&self) -> String {
        display_title(&self.summary.title, &self.summary.provider).replace('\n', " ")
    }
    pub(crate) fn rename(&mut self, title: String, cx: &mut Context<Self>) {
        self.cancel_title_generation();
        self.summary.title = title;
        cx.emit(SessionChanged);
        cx.notify();
    }
    pub(super) fn cancel_title_generation(&mut self) {
        if let Some(cancel) = self.title_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    pub(super) fn generate_title(&mut self, message: &str, cx: &mut Context<Self>) {
        if self.summary.has_messages
            || self.settings.mono.is_some()
            || self.summary.title
                != format!(
                    "New {} session",
                    Kind::from_id(&self.summary.provider).label()
                )
        {
            return;
        }
        self.cancel_title_generation();
        let placeholder = self.summary.title.clone();
        let seed = message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(42)
            .collect::<String>();
        if seed.is_empty() {
            return;
        }
        self.summary.title = seed.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.title_cancel = Some(cancel.clone());
        let kind = Kind::from_id(&self.summary.provider);
        let cwd = self
            .settings
            .worktree_cwd
            .clone()
            .unwrap_or_else(|| self.summary.cwd.clone());
        let message = message.to_owned();
        let account = self.settings.account.clone();
        let id = self.summary.id;
        let store = self.store.clone();
        let expected = seed.clone();
        let flag = cancel.clone();
        let work = cx.background_executor().spawn(async move {
            // Persist the fallback before generation so a fast response cannot
            // race its initial write, and never overwrite a saved manual name.
            if !locked(&store)
                .and_then(|s| s.replace_title(id, &placeholder, &expected))
                .unwrap_or(false)
            {
                return None;
            }
            // GPUI unit tests use synthetic conversations. Live-provider checks
            // are opt-in; the core runner is tested with isolated CLI fixtures.
            if cfg!(test) {
                None
            } else {
                monocode_core::session_title::generate_for(
                    kind,
                    Path::new(&cwd),
                    &message,
                    account,
                    flag,
                )
            }
        });
        cx.spawn(async move |this, cx| {
            let generated = work.await;
            let _ = this.update(cx, |this, cx| {
                if !cancel.load(Ordering::Relaxed)
                    && let Some(generated) = generated
                {
                    this.apply_generated_title(&seed, generated.title, cx);
                }
            });
        })
        .detach();
    }
    pub(super) fn apply_generated_title(
        &mut self,
        seed: &str,
        title: String,
        cx: &mut Context<Self>,
    ) {
        if self.summary.title != seed {
            return;
        }
        self.summary.title = title.clone();
        let store = self.store.clone();
        let id = self.summary.id;
        let seed = seed.to_owned();
        cx.background_executor()
            .spawn(async move {
                let _ = locked(&store).and_then(|s| s.replace_title(id, &seed, &title));
            })
            .detach();
        cx.emit(SessionChanged);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presentation_removes_only_the_sessions_own_canonical_provider_prefix() {
        assert_eq!(display_title("codex · Fix sidebar", "codex"), "Fix sidebar");
        assert_eq!(
            display_title("claude · Fix sidebar", "claude"),
            "Fix sidebar"
        );
        assert_eq!(
            display_title("claude · Notes about Claude", "codex"),
            "claude · Notes about Claude"
        );
        assert_eq!(
            display_title("My codex · notes", "codex"),
            "My codex · notes"
        );
        assert_eq!(
            display_title("New feature proposal", "codex"),
            "New feature proposal"
        );
        for title in ["codex", "Codex", "New Codex session"] {
            assert_eq!(display_title(title, "codex"), "New session");
        }
    }
    #[gpui::test]
    fn late_generated_titles_preserve_manual_names_and_conversation_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let store = Arc::new(std::sync::Mutex::new(
            monocode_core::store::Store::open(Path::new(":memory:")).unwrap(),
        ));
        let summary = locked(&store).unwrap().create("codex", "/tmp").unwrap();
        let id = summary.id;
        let view = cx.new(|cx| Conversation::new(summary, store.clone(), cx));
        view.update(cx, |v, cx| {
            v.summary.title = "The first prompt".into();
            v.apply_generated_title("The first prompt", "Improve session filtering".into(), cx);
        });
        cx.run_until_parked();
        view.update(cx, |v, cx| {
            assert_eq!(v.summary.title, "Improve session filtering");
            let cancelled = Arc::new(AtomicBool::new(false));
            v.title_cancel = Some(cancelled.clone());
            v.rename("My session name".into(), cx);
            assert!(cancelled.load(Ordering::Relaxed));
            v.apply_generated_title(
                "Improve session filtering",
                "Late generated title".into(),
                cx,
            );
            assert_eq!(v.summary.title, "My session name");
        });
        assert!(locked(&store).unwrap().load(id).unwrap().rows.is_empty());
    }
}
