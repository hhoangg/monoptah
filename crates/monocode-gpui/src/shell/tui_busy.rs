//! Claude's hooks report a turn starting or ending by dropping a file in the
//! spool (`monocode_core::tui_hook`). One watcher per app drains it every
//! `POLL` and hands the events to every window; a window applies those for the
//! terminal sessions it has open and drops the rest. The spool is only touched
//! on the background executor. Busy is never saved.
use super::*;
use gpui::{Global, WeakEntity};
use monocode_core::tui_hook::{self, HookEvent, MAX_EVENT_AGE};
use std::{path::PathBuf, time::Duration, time::SystemTime};

pub(super) const POLL: Duration = Duration::from_millis(300);

/// The windows that may own a terminal session.
struct Watcher {
    shells: Vec<WeakEntity<Shell>>,
}
impl Global for Watcher {}

/// Adds a window to the app's watcher, starting it for the first one. The
/// spool is the first window's: every window of an app shares one data folder.
pub(super) fn register(spool: PathBuf, shell: WeakEntity<Shell>, cx: &mut App) {
    if cx.has_global::<Watcher>() {
        cx.global_mut::<Watcher>().shells.push(shell);
        return;
    }
    cx.set_global(Watcher {
        shells: vec![shell],
    });
    cx.spawn(async move |cx| {
        // Whatever a run before this one left behind is not this run's turn.
        let boot = spool.clone();
        cx.background_executor()
            .spawn(async move { tui_hook::prune(&boot, MAX_EVENT_AGE) })
            .await;
        loop {
            cx.background_executor().timer(POLL).await;
            let dir = spool.clone();
            let events = cx
                .background_executor()
                .spawn(async move { tui_hook::drain(&dir, SystemTime::now()) })
                .await;
            if !events.is_empty() {
                cx.update(|cx| deliver(&events, cx));
            }
        }
    })
    .detach();
}

fn deliver(events: &[HookEvent], cx: &mut App) {
    let watcher = cx.global_mut::<Watcher>();
    watcher.shells.retain(|shell| shell.upgrade().is_some());
    for shell in watcher.shells.clone() {
        let _ = shell.update(cx, |shell, cx| shell.apply_tui_hooks(events, cx));
    }
}

impl Shell {
    /// Busy or idle for the open terminal sessions whose conversation the events
    /// name. Others are not this window's.
    fn apply_tui_hooks(&mut self, events: &[HookEvent], cx: &mut Context<Self>) {
        for event in events {
            let owners: Vec<_> = self
                .tui_sessions
                .values()
                .filter(|tui| {
                    tui.read(cx).provider_id() == Some(event.provider_session_id.as_str())
                })
                .cloned()
                .collect();
            for tui in owners {
                tui.update(cx, |tui, cx| tui.set_busy(event.busy, cx));
            }
        }
    }

    /// Joins the app's watcher. A store without a data folder has no spool.
    pub(super) fn watch_tui_hooks(&self, cx: &mut Context<Self>) {
        let dir = locked(&self.store)
            .ok()
            .and_then(|store| store.data_dir().map(tui_hook::spool_dir));
        if let Some(dir) = dir {
            register(dir, cx.weak_entity(), cx);
        }
    }
}
