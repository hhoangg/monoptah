//! Updating and restarting the app remotely: the `app.update` Mono action and the
//! `desktop.update` and `desktop.updateStatus` relay methods of the web app.
//!
//! The rules (what counts as an update, when to ask for confirmation, how a result reads) are
//! in `monocode_core::app_update::remote`. This file gathers what they judge against (the
//! signed feed, the sessions that are running), answers the caller first, and only then starts
//! the existing in-app updater, so the caller always hears back before the app restarts. A
//! request is written to the store before the restart; the next launch compares the running
//! version with it and posts the result.
use super::*;
use crate::{
    monos::Monos,
    notifications::{platform, updater},
};
use gpui::{Global, Task};
use monocode_core::app_update::{
    Installation, Update,
    remote::{self, Decision, Mode, Offer, Outcome, Pending, Request, RunningSession, Version},
};
use serde_json::{Value, json};
use std::sync::Arc;

/// Where the updater is reached. The app uses the signed feed and the in-app updater; tests
/// put their own in `Override` so nothing is downloaded, installed or restarted.
pub(crate) trait Backend: Send + Sync {
    fn current(&self) -> Version;
    /// Looks the feed up. Blocking: runs off the UI thread.
    fn check(&self) -> Result<Found, String>;
    /// Starts installing what `check` found. `failed` is told why when it ends before the
    /// restart. Returns at once.
    fn install(
        &self,
        found: Found,
        failed: updater::InstallFailed,
        cx: &mut App,
    ) -> Result<(), String>;
}

/// What a feed lookup found.
pub(crate) struct Found {
    pub offer: Option<Offer>,
    /// The verified package, for the real updater. Empty in tests.
    package: Option<(Update, Installation)>,
}

struct Feed;
impl Backend for Feed {
    fn current(&self) -> Version {
        Version::parse(updater::version()).unwrap_or_else(|_| Version::new(0, 0, 0))
    }
    fn check(&self) -> Result<Found, String> {
        let found = updater::check_feed()?;
        Ok(Found {
            offer: found.as_ref().map(|(update, _)| Offer {
                version: update.version.clone(),
                notes: update.notes.clone(),
            }),
            package: found,
        })
    }
    fn install(
        &self,
        found: Found,
        failed: updater::InstallFailed,
        cx: &mut App,
    ) -> Result<(), String> {
        let (update, installation) = found.package.ok_or("No update package was found")?;
        updater::install_remote(update, installation, failed, cx)
    }
}

/// Replaces the backend (tests).
#[cfg(test)]
pub(crate) struct Override(pub Arc<dyn Backend>);
#[cfg(test)]
impl Global for Override {}

fn backend(cx: &App) -> Arc<dyn Backend> {
    #[cfg(test)]
    if let Some(over) = cx.try_global::<Override>() {
        return over.0.clone();
    }
    let _ = cx;
    Arc::new(Feed)
}

/// What the app remembers between requests. Only one install runs at a time.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) installing: bool,
    /// Why the last install ended before the restart; shown by the status until the next try.
    last_failure: Option<String>,
    /// The desktop notices raised, for tests (the OS bridge has no observer).
    #[cfg(test)]
    pub(crate) notices: Vec<String>,
    /// The pushes asked for (chat, text), for tests.
    #[cfg(test)]
    pub(crate) pushes: Vec<(String, String)>,
}
impl Global for State {}

/// Who asked.
#[derive(Clone, Debug)]
pub(crate) enum Origin {
    /// A Mono, from its chat session `session`.
    Mono {
        id: String,
        name: String,
        session: i64,
    },
    /// A paired web device or phone.
    Device { name: String },
}
impl Origin {
    fn kind(&self) -> &'static str {
        match self {
            Origin::Mono { .. } => "mono",
            Origin::Device { .. } => "device",
        }
    }
    fn name(&self) -> &str {
        match self {
            Origin::Mono { name, .. } | Origin::Device { name } => name,
        }
    }
}

/// The answer to the caller, and what starts once it was sent.
pub(crate) struct Reply {
    pub value: Value,
    pub begin: Option<Begin>,
}
pub(crate) struct Begin {
    found: Found,
    pending: Pending,
}

impl Begin {
    /// The install can no longer start (the window is gone): forget the request, so nothing
    /// stays recorded or locked for an update that never began.
    pub(crate) fn abandon(self, store: &Database, cx: &mut App) {
        cx.default_global::<State>().installing = false;
        let _ = locked(store).and_then(|store| clear_if_same(&store, &self.pending));
    }
}

/// Clears the stored request only when it is still `pending`, never another request's.
fn clear_if_same(store: &monocode_core::store::Store, pending: &Pending) -> Result<(), String> {
    if Pending::load(store)?.as_ref() == Some(pending) {
        Pending::clear(store)?;
    }
    Ok(())
}

impl Shell {
    /// The chats that are working now: what a restart would interrupt. `except` is the
    /// session of the caller, which is working because it is asking.
    pub(super) fn remote_update_running(
        &self,
        except: Option<i64>,
        cx: &App,
    ) -> Vec<RunningSession> {
        let terminals = self.tui_sessions.iter().filter_map(|(id, tui)| {
            let summary = self.session_summaries.get(id)?;
            (tui.read(cx).busy() && Some(*id) != except).then(|| RunningSession {
                id: id.to_string(),
                title: monocode_core::session_title::display_title(
                    &summary.title,
                    &summary.provider,
                )
                .to_owned(),
            })
        });
        let chats = self
            .notification_chats(cx)
            .into_iter()
            .filter_map(|view| {
                let view = view.read(cx);
                (view.responding() && Some(view.summary.id) != except).then(|| RunningSession {
                    id: view.summary.id.to_string(),
                    title: monocode_core::session_title::display_title(
                        &view.summary.title,
                        &view.summary.provider,
                    )
                    .to_owned(),
                })
            })
            .collect::<Vec<_>>();
        chats.into_iter().chain(terminals).collect()
    }

    /// Answers a `check` or an `install` request. For an install, the returned reply says what
    /// will happen; pass it to [`Shell::remote_update_begin`] after it was sent.
    pub(crate) fn remote_update(
        &mut self,
        origin: Origin,
        request: Request,
        cx: &mut Context<Self>,
    ) -> Task<Result<Reply, String>> {
        let backend = backend(cx);
        let except = match &origin {
            Origin::Mono { session, .. } => Some(*session),
            Origin::Device { .. } => None,
        };
        let running = self.remote_update_running(except, cx);
        if request.mode == Mode::Install {
            // A request to install is always told to the person at the computer.
            self.remote_update_notice(
                &format!("{} asked to update and restart Monoptah.", origin.name()),
                cx,
            );
        }
        let store = self.store.clone();
        let work = cx.background_executor().spawn({
            let backend = backend.clone();
            async move { backend.check() }
        });
        cx.spawn(async move |this, cx| {
            let checked = work.await;
            let current = backend.current();
            let decided = this.update(cx, |_, cx| -> Result<Reply, String> {
                let state = cx.default_global::<State>();
                match request.mode {
                    Mode::Check => {
                        let (offer, error) = match &checked {
                            Ok(found) => (found.offer.clone(), None),
                            Err(error) => (None, Some(error.as_str())),
                        };
                        let mut value = remote::status(&current, offer.as_ref(), &running, error);
                        if let Some(failure) = &state.last_failure {
                            value["lastFailure"] = json!(failure);
                        }
                        Ok(Reply { value, begin: None })
                    }
                    Mode::Install => {
                        let found =
                            checked.map_err(|e| format!("Could not read the update feed: {e}"))?;
                        let decision =
                            remote::decide(&current, found.offer.as_ref(), &running, request.force);
                        let Decision::Proceed { from, to } = &decision else {
                            return Ok(Reply {
                                value: decision.reply(),
                                begin: None,
                            });
                        };
                        // Checked and taken in this one step, after the feed answered: two
                        // requests cannot both pass.
                        if state.installing {
                            return Err("An update is already in progress".into());
                        }
                        state.installing = true;
                        state.last_failure = None;
                        let pending = Pending {
                            from: from.clone(),
                            to: to.clone(),
                            kind: origin.kind().into(),
                            requester: origin.name().into(),
                            chat: match &origin {
                                Origin::Mono { id, .. } => Some(id.clone()),
                                Origin::Device { .. } => None,
                            },
                            requested_at: monocode_core::time::now_ms(),
                        };
                        // Written before anything starts, so a restart is never unexplained.
                        if let Err(error) = locked(&store).and_then(|store| pending.save(&store)) {
                            cx.default_global::<State>().installing = false;
                            return Err(format!("Could not record the update request: {error}"));
                        }
                        Ok(Reply {
                            value: decision.reply(),
                            begin: Some(Begin { found, pending }),
                        })
                    }
                }
            });
            decided.map_err(|e| e.to_string())?
        })
    }

    /// Starts the install of an accepted request: call it once the reply was sent. The result
    /// is posted when the install fails (here) or after the restart (next launch).
    pub(crate) fn remote_update_begin(&mut self, begin: Begin, cx: &mut Context<Self>) {
        let Begin { found, pending } = begin;
        let shell = cx.weak_entity();
        let failed_pending = pending.clone();
        let failed: updater::InstallFailed = Box::new(move |error, cx| {
            let _ = shell.update(cx, |shell, cx| {
                shell.finish_remote_update(&failed_pending, Outcome::Failed(error), cx)
            });
        });
        if let Err(error) = backend(cx).install(found, failed, cx) {
            self.finish_remote_update(&pending, Outcome::Failed(error), cx);
        }
    }

    /// Reports what became of a request: the chat that asked, the computer and the Control
    /// devices. Clears the stored request.
    pub(crate) fn finish_remote_update(
        &mut self,
        pending: &Pending,
        outcome: Outcome,
        cx: &mut Context<Self>,
    ) {
        let state = cx.default_global::<State>();
        state.installing = false;
        if matches!(outcome, Outcome::Failed(_)) {
            state.last_failure = Some(outcome.message(pending));
        }
        let store = self.store.clone();
        let stored = pending.clone();
        let clearing = cx
            .background_executor()
            .spawn(async move { locked(&store).and_then(|store| clear_if_same(&store, &stored)) });
        cx.spawn(async move |_, _| {
            if let Err(error) = clearing.await {
                eprintln!("Could not clear the remote update request: {error}");
            }
        })
        .detach();
        let text = outcome.message(pending);
        if let (Some(chat), Some(monos)) = (&pending.chat, self.monos.clone()) {
            monos.update(cx, |monos: &mut Monos, cx| {
                monos.post_update_result(chat, &text, cx)
            });
        }
        self.remote_update_notice(&text, cx);
        self.push_update_result(pending, &text, cx);
    }

    /// Tells the Control devices how the update ended: the web ones through the web server and
    /// the phones through Expo. Both need a chat to open, so only an update a Mono asked for
    /// is pushed (the web server refuses a notice with neither a session nor a Mono); the
    /// device that asked sees the result in its settings screen.
    fn push_update_result(&mut self, pending: &Pending, text: &str, cx: &mut Context<Self>) {
        use monocode_core::mobile::notify;
        let Some(chat) = pending.chat.as_deref() else {
            return;
        };
        let name = self
            .monos
            .as_ref()
            .and_then(|monos| monos.read(cx).roster.get(chat).map(|mono| mono.label()))
            .unwrap_or_else(|| "Monoptah".into());
        #[cfg(test)]
        cx.default_global::<State>()
            .pushes
            .push((chat.to_owned(), text.to_owned()));
        notify::notify_web(&notify::WebNotice {
            category: "monoMessages",
            session_id: None,
            mono_id: Some(chat),
            project: &name,
            title: &name,
            preview: Some(text),
            control_only: true,
        });
        let notice = notify::Notice {
            title: name,
            subtitle: None,
            body: crate::notifications::delivery::clip_body(text),
            data: json!({"monoId": chat}),
        };
        self.send_mobile_notice("monoMessages", notice, true, cx);
    }

    /// At launch: judges a request the previous run left by the version running now.
    pub(super) fn consume_remote_update(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store).and_then(|store| Pending::load(&store)) });
        cx.spawn(async move |this, cx| {
            if let Ok(Some(pending)) = work.await {
                let outcome = remote::resolve(&pending, updater::version());
                let _ = this.update(cx, |this, cx| {
                    this.finish_remote_update(&pending, outcome, cx)
                });
            }
        })
        .detach();
    }

    /// A banner on this computer, whatever the notification settings of sessions say: someone
    /// remote changed the app, so the person here is always told.
    fn remote_update_notice(&mut self, body: &str, cx: &mut Context<Self>) {
        self.remote_notice("monocode-remote-update", "Remote update", body, cx);
    }

    /// The banner behind [`Self::remote_update_notice`], also used for other
    /// actions a remote device took on this computer.
    pub(super) fn remote_notice(
        &mut self,
        tag: &str,
        subtitle: &str,
        body: &str,
        _cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        _cx.default_global::<State>().notices.push(body.to_owned());
        let sequence = monocode_core::time::now_ms();
        let _ = platform::show(platform::Notification {
            tag: format!("{tag}:{sequence}"),
            subtitle: subtitle.into(),
            body: crate::notifications::delivery::clip_body(body),
            sound: self.workspace.preferences.sounds,
        });
    }
}

/// A backend that downloads and installs nothing (tests).
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::{cell::RefCell, sync::Mutex};

    thread_local! {
        /// The failure hook of the install a test started, to fire it by hand.
        pub(crate) static FAILED: RefCell<Option<updater::InstallFailed>> = const { RefCell::new(None) };
    }

    pub(crate) struct Fake {
        pub current: Version,
        pub offer: Option<Offer>,
        pub feed_error: Option<String>,
        pub install_error: Option<String>,
        /// The version of every install started.
        pub installs: Mutex<Vec<String>>,
    }
    impl Fake {
        pub(crate) fn new(current: &str, latest: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                current: Version::parse(current).unwrap(),
                offer: latest.map(|version| Offer {
                    version: Version::parse(version).unwrap(),
                    notes: "Better things".into(),
                }),
                feed_error: None,
                install_error: None,
                installs: Mutex::new(Vec::new()),
            })
        }
    }
    impl Backend for Fake {
        fn current(&self) -> Version {
            self.current.clone()
        }
        fn check(&self) -> Result<Found, String> {
            match &self.feed_error {
                Some(error) => Err(error.clone()),
                None => Ok(Found {
                    offer: self.offer.clone(),
                    package: None,
                }),
            }
        }
        fn install(
            &self,
            found: Found,
            failed: updater::InstallFailed,
            _: &mut App,
        ) -> Result<(), String> {
            if let Some(error) = &self.install_error {
                return Err(error.clone());
            }
            let version = found
                .offer
                .map(|o| o.version.to_string())
                .unwrap_or_default();
            self.installs.lock().unwrap().push(version);
            FAILED.with(|slot| *slot.borrow_mut() = Some(failed));
            Ok(())
        }
    }
}
