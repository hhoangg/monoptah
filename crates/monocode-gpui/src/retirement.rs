//! The retirement notice: Monoptah Native has moved to zptah and is no longer maintained.
//!
//! One card is drawn in two places: a dismissible banner over the shell (see
//! `shell/retirement.rs`) and a permanent entry in Settings > General. Both read the constants
//! and the card from here, so the copy, the links and the command exist once.
use crate::{settings::widgets::secondary_button, theme::*};
use gpui::{AnyElement, App, ClipboardItem, Div, FontWeight, SharedString, div, prelude::*, px};

/// Where the latest zptah release is published.
pub(crate) const GET_URL: &str = "https://github.com/hhoangg/zptah/releases/latest";
/// The migration section of the Monoptah README.
pub(crate) const MIGRATE_URL: &str = "https://github.com/hhoangg/monoptah#migrate-to-zptah";
/// The Homebrew command that installs zptah (macOS, Apple Silicon).
pub(crate) const BREW_COMMAND: &str = "brew install --cask hhoangg/zptah/zptah";
pub(crate) const TITLE: &str = "Monoptah has moved to zptah";
pub(crate) const MESSAGE: &str = "Monoptah is no longer maintained and will not receive future updates. Install zptah to keep getting fixes and new features.";
/// What the updater says after a check, in place of a bare "latest version" line.
pub(crate) const UPDATER_RETIRED: &str =
    "Monoptah is retired and will not receive future updates. Get zptah to keep up to date.";

/// Whether the banner is drawn: shown unless it was dismissed for good or for this session.
pub(crate) fn banner_visible(dismissed_for_good: bool, dismissed_for_session: bool) -> bool {
    !dismissed_for_good && !dismissed_for_session
}

pub(crate) fn open_get(cx: &mut App) {
    cx.open_url(GET_URL);
}

pub(crate) fn open_migrate(cx: &mut App) {
    cx.open_url(MIGRATE_URL);
}

pub(crate) fn copy_command(cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(BREW_COMMAND.to_owned()));
}

/// The card: title, message, the copyable command and the Get / How to migrate / Copy buttons.
/// `prefix` namespaces the element ids; `extra` is a row of buttons the caller adds (the
/// banner's dismiss buttons).
pub(crate) fn card(prefix: &'static str, extra: Vec<AnyElement>) -> Div {
    let id = move |name: &str| SharedString::from(format!("{prefix}-{name}"));
    let button = move |name: &'static str, label: &'static str| {
        let selector = id(name);
        secondary_button(selector.clone(), label, false, false)
            .debug_selector(move || selector.to_string())
            .child(label)
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(0.95))
                .child(TITLE),
        )
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(19.5))
                .text_color(content(0.6))
                .child(MESSAGE),
        )
        .child({
            let selector = id("command");
            div()
                .debug_selector(move || selector.to_string())
                .px_2()
                .py_1()
                .rounded_md()
                .bg(content(0.06))
                .font_family(crate::fonts::mono())
                .text_size(px(12.))
                .text_color(content(0.85))
                .child(BREW_COMMAND)
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(button("get", "Get zptah").on_click(|_, _, cx| open_get(cx)))
                .child(button("migrate", "How to migrate").on_click(|_, _, cx| open_migrate(cx)))
                .child(button("copy", "Copy command").on_click(|_, _, cx| copy_command(cx))),
        )
        .when(!extra.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .children(extra),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_is_shown_by_default() {
        assert!(banner_visible(false, false));
    }

    #[test]
    fn dismissing_for_the_session_hides_it_until_the_next_launch() {
        assert!(!banner_visible(false, true));
        // A new launch starts with the session flag cleared.
        assert!(banner_visible(false, false));
    }

    #[test]
    fn dont_show_again_hides_it_for_good() {
        assert!(!banner_visible(true, false));
        assert!(!banner_visible(true, true));
    }

    #[test]
    fn the_links_and_command_point_at_zptah() {
        assert_eq!(GET_URL, "https://github.com/hhoangg/zptah/releases/latest");
        assert!(MIGRATE_URL.ends_with("#migrate-to-zptah"));
        assert_eq!(BREW_COMMAND, "brew install --cask hhoangg/zptah/zptah");
    }

    #[gpui::test]
    fn copy_puts_the_brew_command_on_the_clipboard(cx: &mut gpui::TestAppContext) {
        cx.update(copy_command);
        let text = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(text.as_deref(), Some(BREW_COMMAND));
    }

    #[gpui::test]
    fn the_links_open_through_the_platform(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            open_get(cx);
            open_migrate(cx);
        });
        assert_eq!(cx.opened_url().as_deref(), Some(MIGRATE_URL));
        cx.update(open_get);
        assert_eq!(cx.opened_url().as_deref(), Some(GET_URL));
    }
}
