//! The retirement banner: a card over the bottom right corner of the window that says Monoptah
//! has moved to zptah. It never blocks the app. "Dismiss" hides it for this session and "Don't
//! show again" is saved in the workspace. The card itself is `crate::retirement::card`.
use super::*;
use crate::{retirement, settings::widgets::secondary_button};

impl Shell {
    pub(super) fn retirement_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !retirement::banner_visible(
            self.workspace.retirement_notice_dismissed,
            self.retirement_session_dismissed,
        ) {
            return None;
        }
        let dismiss = secondary_button("retirement-dismiss", "Dismiss", false, false)
            .debug_selector(|| "retirement-dismiss".into())
            .child("Dismiss")
            .on_click(cx.listener(|this, _, _, cx| {
                this.retirement_session_dismissed = true;
                cx.notify();
            }))
            .into_any_element();
        let never = secondary_button("retirement-never", "Don't show again", false, false)
            .debug_selector(|| "retirement-never".into())
            .child("Don't show again")
            .on_click(cx.listener(|this, _, _, cx| {
                this.workspace.retirement_notice_dismissed = true;
                this.persist(cx);
                cx.notify();
            }))
            .into_any_element();
        // The wrapper has no hitbox, so only the occluding frame takes clicks and scrolls.
        Some(
            div()
                .id("retirement-banner")
                .debug_selector(|| "retirement-banner".into())
                .role(Role::Status)
                .absolute()
                .left(px(12.))
                .right(px(12.))
                .bottom(px(44.))
                .flex()
                .justify_end()
                .child(
                    crate::notifications::surface::glass_frame()
                        .occlude()
                        .w(px(420.))
                        .max_w_full()
                        .bg(content(0.1))
                        .p_4()
                        .child(retirement::card("retirement", vec![dismiss, never])),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};

    fn banner_drawn(visual: &mut VisualTestContext) -> bool {
        visual.refresh().unwrap();
        visual.run_until_parked();
        visual.debug_bounds("retirement-banner").is_some()
    }

    fn click(visual: &mut VisualTestContext, selector: &'static str) {
        let bounds = visual.debug_bounds(selector).expect("drawn");
        visual.simulate_click(bounds.center(), Default::default());
        visual.run_until_parked();
    }

    fn open(cx: &mut TestAppContext, dismissed: bool) -> WindowHandle<Shell> {
        let (window, _, _) = super::super::tests::window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.workspace.retirement_notice_dismissed = dismissed;
                shell.retirement_session_dismissed = false;
                cx.notify();
            })
            .unwrap();
        window
    }

    #[gpui::test]
    fn the_banner_shows_and_dismiss_hides_it_for_the_session(cx: &mut TestAppContext) {
        let window = open(cx, false);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(banner_drawn(visual));
        click(visual, "retirement-dismiss");
        assert!(!banner_drawn(visual));
        window
            .update(visual, |shell, _, _| {
                assert!(shell.retirement_session_dismissed);
                assert!(!shell.workspace.retirement_notice_dismissed);
            })
            .unwrap();
    }

    #[gpui::test]
    fn dont_show_again_sets_the_flag_and_saves_it(cx: &mut TestAppContext) {
        let window = open(cx, false);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(banner_drawn(visual));
        click(visual, "retirement-never");
        assert!(!banner_drawn(visual));
        window
            .update(visual, |shell, _, _| {
                assert!(shell.workspace.retirement_notice_dismissed);
                let saved = serde_json::to_string(&shell.workspace).unwrap();
                let loaded: Workspace = serde_json::from_str(&saved).unwrap();
                assert!(loaded.retirement_notice_dismissed);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_saved_flag_draws_no_banner(cx: &mut TestAppContext) {
        let window = open(cx, true);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        assert!(!banner_drawn(visual));
    }
}
