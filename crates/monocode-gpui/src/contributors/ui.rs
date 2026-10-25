//! Small widgets shared by the Contributors page and its tabs: the segmented
//! control, switch and select of the settings primitives, free of
//! `SettingsView` so any entity can use them.
use crate::{
    popover::{Align, Anchor, AnchorSlot, Popover, Side, popover},
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, FocusHandle, Role, SharedString, Stateful, Window, div, prelude::*,
    px, rgb,
};

pub(super) const RED: u32 = 0xf87171;

/// `border-red-400/30 bg-red-400/5 text-red-400` at the given alpha.
pub(super) fn red(alpha: f32) -> gpui::Hsla {
    let mut color: gpui::Hsla = rgb(RED).into();
    color.a = alpha;
    color
}

/// `amber-400`, the warning tone.
pub(super) fn amber(alpha: f32) -> gpui::Hsla {
    let mut color: gpui::Hsla = rgb(0xfbbf24).into();
    color.a = alpha;
    color
}

/// Segmented: equal-width options in a hairline frame.
pub(super) fn segmented<T: 'static>(
    id: &'static str,
    label: &'static str,
    value: &str,
    options: &[(&'static str, &'static str)],
    cx: &mut Context<T>,
    change: impl Fn(&mut T, &'static str, &mut Context<T>) + Clone + 'static,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("segmented-{id}")))
        .role(Role::RadioGroup)
        .aria_label(label)
        .grid()
        .grid_cols(options.len() as u16)
        .flex_shrink_0()
        .gap(px(2.))
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .p(px(2.))
        .text_size(px(12.))
        .line_height(px(16.))
        .children(options.iter().map(|&(option, text)| {
            let selected = value == option;
            let change = change.clone();
            div()
                .id(SharedString::from(format!("segmented-{id}-{option}")))
                .debug_selector(move || format!("contributors-{id}-{option}"))
                .role(Role::RadioButton)
                .aria_label(text)
                .aria_selected(selected)
                .min_w_0()
                .flex()
                .justify_center()
                .whitespace_nowrap()
                .rounded(px(5.))
                .px(px(10.))
                .py_1()
                .cursor_pointer()
                .map(|d| {
                    if selected {
                        d.bg(content(0.1)).text_color(content(1.))
                    } else {
                        d.text_color(content(0.5))
                            .hover(|s| s.text_color(content(1.)))
                    }
                })
                .child(text)
                .on_click(cx.listener(move |this, _, _, cx| change(this, option, cx)))
        }))
}

/// Toggle: a 36x20 switch with a 16px knob.
pub(super) fn toggle<T: 'static>(
    id: &'static str,
    label: &'static str,
    on: bool,
    cx: &mut Context<T>,
    change: impl Fn(&mut T, bool, &mut Context<T>) + 'static,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("toggle-{id}")))
        .debug_selector(move || format!("contributors-toggle-{id}"))
        .role(Role::Switch)
        .aria_label(label)
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .relative()
        .h(px(20.))
        .w(px(36.))
        .flex_shrink_0()
        .rounded_full()
        .bg(if on { accent_color() } else { content(0.2) })
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(px(if on { 18. } else { 2. }))
                .size(px(16.))
                .rounded_full()
                .bg(rgb(0xffffff)),
        )
        .on_click(cx.listener(move |this, _, _, cx| change(this, !on, cx)))
}

/// A one-paragraph tooltip on the app's surface color.
pub(super) struct Tip(pub SharedString);
impl gpui::Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut background = shell_fill();
        background.a = 1.;
        div()
            .max_w(px(320.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(background)
            .border_1()
            .border_color(content(0.12))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.85))
            .child(self.0.clone())
    }
}

/// Checkbox: a 14px box, its label and a small hint after it (a status type).
pub(super) fn checkbox<T: 'static>(
    id: String,
    label: SharedString,
    hint: Option<SharedString>,
    checked: bool,
    disabled: bool,
    cx: &mut Context<T>,
    change: impl Fn(&mut T, bool, &mut Context<T>) + 'static,
) -> Stateful<Div> {
    let selector = id.clone();
    div()
        .id(SharedString::from(id))
        .debug_selector(move || selector.clone())
        .role(Role::CheckBox)
        .aria_label(label.clone())
        .aria_toggled(if checked {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .text_size(px(12.))
        .text_color(content(if disabled { 0.4 } else { 0.8 }))
        .child(
            div()
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .border_1()
                .border_color(if checked {
                    accent_color()
                } else {
                    content(0.3)
                })
                .when(checked, |d| {
                    d.bg(accent_color())
                        .child(icon("check", 10.).text_color(gpui::white()))
                }),
        )
        .child(div().min_w_0().truncate().child(label))
        .children(hint.map(|hint| {
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(content(0.4))
                .child(hint)
        }))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| change(this, !checked, cx)))
        })
}

/// A small square icon button (period arrows, refresh, More).
pub(super) fn icon_button(
    id: &'static str,
    label: &'static str,
    path: &'static str,
    side: f32,
    disabled: bool,
) -> Stateful<Div> {
    icon_button_with(
        id,
        label,
        icon(path, side)
            .text_color(content(if disabled { 0.35 } else { 0.55 }))
            .into_any_element(),
        disabled,
    )
}

/// An icon button with any glyph (a spinner while it works).
pub(super) fn icon_button_with(
    id: &'static str,
    label: &'static str,
    glyph: AnyElement,
    disabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded_md()
        .p(px(6.))
        .child(glyph)
        .when(disabled, |d| d.opacity(0.5))
        .when(!disabled, |d| {
            d.cursor_pointer().hover(|s| s.bg(content(0.08)))
        })
}

/// What a select keeps between frames: whether its list is open, the row the
/// keyboard is on and the trigger's place for the popover.
pub(crate) struct Dropdown {
    pub open: bool,
    pub active: usize,
    pub anchor: AnchorSlot,
    pub list_focus: FocusHandle,
}
impl Dropdown {
    pub(crate) fn new(cx: &mut gpui::App) -> Self {
        Self {
            open: false,
            active: 0,
            anchor: AnchorSlot::default(),
            list_focus: cx.focus_handle(),
        }
    }
}

/// Select: a bordered trigger; the list is `select_list`, drawn by the owner
/// at the end of its element tree so it sits above everything.
#[allow(clippy::too_many_arguments)]
pub(super) fn select_trigger<T: 'static>(
    id: &'static str,
    label: &'static str,
    text: SharedString,
    dropdown: &Dropdown,
    cx: &mut Context<T>,
    state: fn(&mut T) -> &mut Dropdown,
    selected_index: usize,
) -> Stateful<Div> {
    let open = dropdown.open;
    div()
        .id(SharedString::from(format!("select-{id}")))
        .debug_selector(move || format!("contributors-select-{id}"))
        .role(Role::ComboBox)
        .aria_label(format!("{label}: {text}"))
        .aria_expanded(open)
        .relative()
        .max_w(px(208.))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .bg(content(0.05))
        .px_2()
        .py_1()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(content(1.))
        .cursor_pointer()
        .hover(|s| s.border_color(content(0.2)))
        .child(dropdown.anchor.probe())
        .child(div().min_w_0().truncate().child(text))
        .child(icon("chevron-down-hi", 14.).text_color(content(0.5)))
        .on_click(cx.listener(move |this, _, window, cx| {
            let dropdown = state(this);
            dropdown.open = !dropdown.open;
            dropdown.active = selected_index;
            if dropdown.open {
                let focus = dropdown.list_focus.clone();
                window.focus(&focus, cx);
            }
            cx.notify();
        }))
}

/// The open list of a select as a popover. `pick` gets the chosen value;
/// Escape closes the list and nothing else.
#[allow(clippy::too_many_arguments)]
pub(super) fn select_list<T: 'static>(
    id: &'static str,
    label: &'static str,
    value: &str,
    options: Vec<(String, SharedString)>,
    dropdown: &Dropdown,
    cx: &mut Context<T>,
    state: fn(&mut T) -> &mut Dropdown,
    owner_focus: FocusHandle,
    pick: impl Fn(&mut T, String, &mut Window, &mut Context<T>) + Clone + 'static,
) -> Popover {
    let selected = options.iter().position(|(v, _)| v == value);
    let active = dropdown.active;
    let values: Vec<String> = options.iter().map(|(v, _)| v.clone()).collect();
    let count = values.len();
    let weak = cx.weak_entity();
    let dismiss_focus = owner_focus.clone();
    let key_pick = pick.clone();
    let key_focus = owner_focus.clone();
    let rows = options
        .into_iter()
        .enumerate()
        .map(|(ix, (option, text))| {
            let is_selected = Some(ix) == selected;
            let pick = pick.clone();
            let focus = owner_focus.clone();
            div()
                .id(SharedString::from(format!("select-{id}-{ix}")))
                .debug_selector(move || format!("contributors-option-{id}-{ix}"))
                .role(Role::ListBoxOption)
                .aria_label(text.clone())
                .aria_selected(is_selected)
                .w_full()
                .flex()
                .items_center()
                .gap_2()
                .rounded_lg()
                .px_2()
                .py(px(6.))
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(content(1.))
                .cursor_pointer()
                .map(|d| {
                    if ix == active || is_selected {
                        d.bg(content(0.1))
                    } else {
                        d.hover(|s| s.bg(content(0.05)))
                    }
                })
                .child(div().min_w_0().flex_1().truncate().child(text))
                .when(is_selected, |d| {
                    d.child(icon("check-hi", 14.).text_color(content(1.)))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    state(this).open = false;
                    pick(this, option.clone(), window, cx);
                    window.focus(&focus, cx);
                    cx.notify();
                }))
        })
        .collect::<Vec<_>>();
    popover(
        SharedString::from(format!("select-popover-{id}")),
        Anchor::Slot(dropdown.anchor.clone()),
    )
    .side(Side::Bottom)
    .align(Align::End)
    .width(280.)
    .max_height(320.)
    .padding(4.)
    .on_dismiss(move |window, cx| {
        let _ = weak.update(cx, |this, cx| {
            state(this).open = false;
            window.focus(&dismiss_focus, cx);
            cx.notify();
        });
    })
    .child(
        div()
            .id(SharedString::from(format!("select-list-{id}")))
            .role(Role::ListBox)
            .aria_label(label)
            .track_focus(&dropdown.list_focus)
            .flex()
            .flex_col()
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    let last = count.saturating_sub(1);
                    match event.keystroke.key.as_str() {
                        "down" => state(this).active = (state(this).active + 1).min(last),
                        "up" => state(this).active = state(this).active.saturating_sub(1),
                        "home" => state(this).active = 0,
                        "end" => state(this).active = last,
                        "enter" | "tab" => {
                            let at = state(this).active;
                            state(this).open = false;
                            if let Some(value) = values.get(at) {
                                key_pick(this, value.clone(), window, cx);
                            }
                            window.focus(&key_focus, cx);
                        }
                        "escape" => {
                            state(this).open = false;
                            window.focus(&key_focus, cx);
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .overflow_y_scroll()
            .children(rows),
    )
}

/// A red banner with room for an action: `text` and the caller's buttons.
pub(super) fn alert(id: &'static str, text: String, actions: Vec<AnyElement>) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .role(Role::Alert)
        .aria_label(text.clone())
        .mb_4()
        .flex()
        .items_start()
        .gap(px(10.))
        .rounded_xl()
        .border_1()
        .border_color(red(0.3))
        .bg(red(0.05))
        .px(px(14.))
        .py(px(10.))
        .text_size(px(12.))
        .text_color(rgb(RED))
        .child(icon("alert-circle", 14.).mt(px(2.)).text_color(rgb(RED)))
        .child(div().min_w_0().flex_1().line_height(px(19.5)).child(text))
        .children(actions)
}

/// The grey of a person with no color (`FALLBACK_COLOR` of StatsTab.tsx).
const FALLBACK_COLOR: u32 = 0x8a8f98;

/// `#rrggbb` as a color; anything else is the neutral grey.
pub(super) fn hex_color(hex: &str) -> gpui::Hsla {
    let digits = hex.strip_prefix('#').unwrap_or(hex);
    let value = (digits.len() == 6)
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten();
    rgb(value.unwrap_or(FALLBACK_COLOR)).into()
}

/// "Ana Bui" -> "AB", "ana" -> "A", "" -> "?" (`initialsOf` of Avatar.tsx).
pub(super) fn initials_of(name: &str) -> String {
    let name = name.split('@').next().unwrap_or("");
    let mut words = name
        .split(|c: char| c.is_whitespace() || matches!(c, '.' | '_' | '-'))
        .filter(|word| !word.is_empty());
    let first = words.next().and_then(|word| word.chars().next());
    let last = words.next_back().and_then(|word| word.chars().next());
    let text: String = first
        .into_iter()
        .chain(last)
        .collect::<String>()
        .to_uppercase();
    if text.is_empty() { "?".into() } else { text }
}

#[derive(Clone, Copy)]
pub(super) enum AvatarSize {
    /// 24px, the table rows.
    Sm,
    /// 28px.
    #[allow(dead_code)] // Task 6.9 (podium) and 6.10 (person cards).
    Md,
    /// 44px.
    #[allow(dead_code)] // Task 6.9 (podium).
    Lg,
}

/// A round badge with the person's initials on their color (`Avatar.tsx`).
pub(super) fn avatar(name: &str, color: &str, size: AvatarSize) -> Div {
    let (side, text) = match size {
        AvatarSize::Sm => (24., 10.),
        AvatarSize::Md => (28., 11.),
        AvatarSize::Lg => (44., 16.),
    };
    div()
        .size(px(side))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(hex_color(color))
        .text_size(px(text))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(rgb(0xffffff))
        .child(initials_of(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_use_the_first_letters_of_the_first_and_last_word() {
        assert_eq!(initials_of("Hoang Hoang"), "HH");
        assert_eq!(initials_of("ana maria de souza"), "AS");
    }

    #[test]
    fn initials_use_one_letter_for_a_single_word_and_ignore_an_email_domain() {
        assert_eq!(initials_of("ana"), "A");
        assert_eq!(initials_of("ana.lee@example.com"), "AL");
    }

    #[test]
    fn initials_fall_back_to_a_question_mark_for_nothing_usable() {
        assert_eq!(initials_of(""), "?");
        assert_eq!(initials_of("  "), "?");
        assert_eq!(initials_of("@example.com"), "?");
    }

    #[test]
    fn a_color_is_read_from_hex_and_falls_back_to_grey() {
        assert_eq!(hex_color("#e3a008"), gpui::Hsla::from(rgb(0xe3a008)));
        assert_eq!(hex_color("#8a8f98"), hex_color("nonsense"));
        assert_eq!(hex_color("#12"), hex_color(""));
    }
}
