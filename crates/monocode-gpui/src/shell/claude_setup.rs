//! The Claude setup block of the cloud panel in the shell (design 3.2.4): what the workspace
//! got, one health line per MCP server, whether the local setup changed since, and the manual
//! "Update Claude setup" action, which is the only refresh there is.
//!
//! The stored result is read once per workspace on a worker (it scans the local setup to
//! detect a change) and kept in memory; a finished start, an update or a different workspace
//! asks for a fresh read. The paint path only reads the cache.
use super::*;
use crate::panel_root::{PanelRoot, SetupStrip};
use monocode_core::cloud_sessions::claude_setup::SetupView;
use std::{cell::RefCell, rc::Rc, sync::atomic::AtomicBool};

/// What the panel knows about one workspace's Claude setup.
#[derive(Default)]
pub(super) struct SetupSlot {
    pub(super) view: Option<SetupView>,
    pub(super) requested: bool,
    pub(super) updating: bool,
    pub(super) error: Option<String>,
}

pub(super) type Setups = Rc<RefCell<HashMap<String, SetupSlot>>>;

impl Shell {
    /// The block for the workspace of the panels, `None` when nothing was ever sent there. The
    /// first call for a workspace starts the read; nothing here touches the disk.
    pub(super) fn claude_setup_strip(
        &self,
        root: &PanelRoot,
        cx: &mut Context<Self>,
    ) -> Option<SetupStrip> {
        let cloud = root.cloud()?;
        let id = self
            .workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane))?;
        let key = cloud.key();
        let already_requested = {
            let mut slots = self.claude_setups.borrow_mut();
            let slot = slots.entry(key.clone()).or_default();
            std::mem::replace(&mut slot.requested, true)
        };
        if !already_requested {
            self.read_claude_setup(id, key, cx);
            return None;
        }
        let slots = self.claude_setups.borrow();
        let slot = slots.get(&key)?;
        let connected = self.panel_link(cx) == crate::panel_root::Link::Connected;
        slot.view
            .as_ref()
            .map(|view| SetupStrip::of(view, slot.updating, slot.error.clone(), connected))
    }

    /// Reads the stored result on a worker and shows it.
    fn read_claude_setup(&self, id: i64, key: String, cx: &mut Context<Self>) {
        let cloud = self.cloud.clone();
        let setups = self.claude_setups.clone();
        let work = cx
            .background_executor()
            .spawn(async move { cloud.claude_setup_view(id) });
        cx.spawn(async move |this, cx| {
            let view = work.await;
            let _ = this.update(cx, |_, cx| {
                setups.borrow_mut().entry(key).or_default().view = view;
                cx.notify();
            });
        })
        .detach();
    }

    /// A start finished or the workspace is gone: read again when it is next in view.
    pub(super) fn forget_claude_setup(&mut self, key: &str) {
        self.claude_setups.borrow_mut().remove(key);
    }

    /// "Update Claude setup": the user's one-way copy of the local setup, credentials included.
    /// Runs on a worker (it uploads); the answer is a fresh block or the error text.
    pub(super) fn update_claude_setup(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .workspace
            .active
            .map(|pane| self.workspace.session_for_pane(pane))
        else {
            return;
        };
        let Some(cloud_ref) = self.panel_root(cx).cloud().cloned() else {
            return;
        };
        let key = cloud_ref.key();
        {
            let mut slots = self.claude_setups.borrow_mut();
            let slot = slots.entry(key.clone()).or_default();
            if slot.updating {
                return;
            }
            slot.updating = true;
            slot.error = None;
        }
        let cloud = self.cloud.clone();
        let setups = self.claude_setups.clone();
        let work = cx.background_executor().spawn(async move {
            let updated = cloud.update_claude_setup(id, &AtomicBool::new(false), &mut |_| {});
            (updated.map(|_| ()), cloud.claude_setup_view(id))
        });
        cx.spawn(async move |this, cx| {
            let (result, view) = work.await;
            let _ = this.update(cx, |_, cx| {
                let mut slots = setups.borrow_mut();
                let slot = slots.entry(key).or_default();
                slot.updating = false;
                slot.error = result.err();
                if view.is_some() {
                    slot.view = view;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
