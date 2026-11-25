//! The permanent retirement entry in Settings > General.
use super::*;
use gpui::{
    Bounds, Render, TestAppContext, VisualTestContext, WindowBounds, WindowOptions, point, size,
};

struct Preview(Entity<SettingsView>);

impl Render for Preview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(self.0.clone())
    }
}

#[gpui::test]
fn general_always_shows_the_zptah_entry(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(2800.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    Preview(cx.new(|cx| {
                        SettingsView::new(
                            Section::General,
                            Preferences::default(),
                            SettingsContext::default(),
                            cx,
                        )
                    }))
                })
            },
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for selector in [
        "settings-retirement-get",
        "settings-retirement-migrate",
        "settings-retirement-copy",
        "settings-retirement-command",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // The dismiss buttons belong to the banner only.
    assert!(visual.debug_bounds("retirement-dismiss").is_none());
}
