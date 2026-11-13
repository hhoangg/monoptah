//! The board of emotions: one card per emotion plus Unsorted, with the emoji as draggable chips.
use super::{EmojiLibraryView, card, picture, plural};
use crate::{
    cloud_dialog_ui::alert,
    cloud_mark::CloudTip,
    input::{self, Input},
    theme::*,
};
use gpui::{
    AnyElement, Context, CursorStyle, Entity, Focusable, Render, Role, SharedString, Window, div,
    prelude::*, px,
};
use monocode_core::mono::emoji::{self, Emoji, Emotion, Format, Library, Pack};
use std::path::{Path, PathBuf};

/// What travels with the pointer while an emoji chip is dragged.
#[derive(Clone)]
struct DraggedEmoji {
    pack: String,
    file: String,
    path: PathBuf,
}

impl Render for DraggedEmoji {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size(px(44.))
            .rounded_lg()
            .border_1()
            .border_color(accent_color().opacity(0.6))
            .bg(floating_fill())
            .flex()
            .items_center()
            .justify_center()
            .child(picture(&self.path, 36., None))
    }
}

impl EmojiLibraryView {
    fn text_changed(
        &mut self,
        description: bool,
        emotion: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self
            .store
            .read(cx)
            .library()
            .emotions
            .iter()
            .find(|e| e.id == emotion)
            .cloned()
        else {
            return;
        };
        let store = self.store.clone();
        if description {
            if current.description != text.trim() {
                let (id, text) = (emotion.to_owned(), text.to_owned());
                let _ = store.update(cx, |s, cx| {
                    s.edit_typed(cx, move |l| {
                        l.describe_emotion(&id, &text);
                        Ok(())
                    })
                });
            }
        } else if current.name == text.trim() {
            self.name_errors.remove(emotion);
            cx.notify();
        } else {
            let (id, name) = (emotion.to_owned(), text.to_owned());
            match store.update(cx, |s, cx| {
                s.edit_typed(cx, move |l| l.rename_emotion(&id, &name))
            }) {
                Ok(()) => {
                    self.name_errors.remove(emotion);
                }
                Err(error) => {
                    self.name_errors.insert(emotion.into(), error);
                }
            }
            cx.notify();
        }
    }

    /// The text field of an emotion card, kept in step with the library while it is not being
    /// edited and wired to save what is typed.
    fn field(
        &mut self,
        description: bool,
        emotion: &Emotion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Input> {
        let (kind, label, text) = if description {
            (
                "desc",
                "When should Mono use it?",
                emotion.description.clone(),
            )
        } else {
            ("name", "Emotion name", emotion.name.clone())
        };
        let key = format!("emoji-{kind}:{}", emotion.id);
        let field = match self.fields.get(&key) {
            Some(field) => field.clone(),
            None => {
                let field = cx.new(|cx| {
                    let mut f = Input::field(label, label, content(0.35), cx);
                    f.set_font_size(13.);
                    f.set_placeholder(label, cx);
                    f.set_text(text.clone(), cx);
                    f
                });
                self.fields.insert(key, field.clone());
                field
            }
        };
        if self.wired.insert(field.entity_id()) {
            let id = emotion.id.clone();
            self.subscriptions.push(cx.subscribe(
                &field,
                move |this, input, _: &input::Changed, cx| {
                    let text = input.read(cx).content.to_string();
                    this.text_changed(description, &id, &text, cx);
                },
            ));
            let id = emotion.id.clone();
            self.subscriptions.push(cx.on_blur(
                &field.focus_handle(cx),
                window,
                move |this, _, cx| {
                    this.store.clone().update(cx, |store, cx| store.flush(cx));
                    // A refused name goes back to the saved one when the field is left.
                    if this.name_errors.remove(&id).is_some() {
                        cx.notify();
                    }
                },
            ));
        }
        let editing = field.focus_handle(cx).is_focused(window)
            || (!description && self.name_errors.contains_key(&emotion.id));
        if !editing && *field.read(cx).content != *text.as_str() {
            field.update(cx, |f, cx| f.set_text(text, cx));
        }
        field
    }

    #[allow(clippy::too_many_arguments)]
    fn chip(
        &self,
        dir: &Path,
        pack: &Pack,
        emoji: &Emoji,
        side: f32,
        draggable: bool,
        scope: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = emoji::image_path(dir, pack, emoji);
        let tip: SharedString = format!("{} \u{b7} {}", emoji.name, emoji.format.label()).into();
        let id = format!("emoji-lib-{scope}-{}-{}", pack.id, emoji.file);
        let selector = id.clone();
        let payload = DraggedEmoji {
            pack: pack.id.clone(),
            file: emoji.file.clone(),
            path: path.clone(),
        };
        // Only the chip under the pointer plays its GIF.
        let playing =
            emoji.format == Format::Gif && self.hovered_chip.as_deref() == Some(id.as_str());
        let hover_id = id.clone();
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || selector.clone())
            .aria_label(tip.clone())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let current = this.hovered_chip.as_deref() == Some(hover_id.as_str());
                if *hovered && !current {
                    this.hovered_chip = Some(hover_id.clone());
                    cx.notify();
                } else if !*hovered && current {
                    this.hovered_chip = None;
                    cx.notify();
                }
            }))
            .relative()
            .size(px(side + 8.))
            .flex_shrink_0()
            .rounded_lg()
            .border_1()
            .border_color(content(0.08))
            .bg(content(0.03))
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.border_color(accent_color().opacity(0.7)))
            .child(picture(
                &path,
                side,
                playing.then(|| format!("{id}-image").into()),
            ))
            .when(emoji.format == Format::Gif, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(-4.))
                        .bottom(px(-4.))
                        .px(px(3.))
                        .rounded(px(4.))
                        .bg(content(0.2))
                        .text_size(px(9.))
                        .line_height(px(12.))
                        .text_color(content(0.9))
                        .child("GIF"),
                )
            })
            .tooltip(move |_, cx| cx.new(|_| CloudTip(tip.clone())).into())
            .when(draggable, |d| {
                d.cursor(CursorStyle::OpenHand)
                    .on_drag(payload, |drag, _, _, cx| cx.new(|_| drag.clone()))
            })
            .into_any_element()
    }

    pub(super) fn board(
        &mut self,
        editable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (dir, library) = {
            let store = self.store.read(cx);
            (store.dir().to_path_buf(), store.library().clone())
        };
        let filter = self
            .filter
            .clone()
            .filter(|id| library.packs.iter().any(|p| &p.id == id));
        let filter_text = filter
            .as_ref()
            .and_then(|id| library.packs.iter().find(|p| &p.id == id))
            .map_or("All packs".to_string(), |p| p.name.clone());
        let trigger = self.filter_trigger(filter_text, cx);
        let reset = crate::modal::disabled(
            crate::modal::ghost_button("emoji-lib-reset", "Reset to defaults")
                .debug_selector(|| "emoji-lib-reset".into())
                .h(px(28.))
                .px(px(10.))
                .text_size(px(12.)),
            !editable,
        )
        .when(editable, |d| {
            d.on_click(cx.listener(|this, _, window, cx| {
                this.ask(
                    window,
                    cx,
                    "Reset emotions to defaults?",
                    "The 13 default emotions come back. Emoji whose emotion is one of them keep it; the rest go to Unsorted.",
                    "Reset",
                    move |this, cx| {
                        this.name_errors.clear();
                        let _ = this.edit(cx, |l| {
                            l.reset_emotions();
                            Ok(())
                        });
                    },
                );
            }))
        });
        let mut cards: Vec<AnyElement> = library
            .emotions
            .iter()
            .map(|emotion| {
                self.emotion_card(
                    &dir,
                    &library,
                    Some(emotion),
                    filter.as_deref(),
                    editable,
                    window,
                    cx,
                )
            })
            .collect();
        cards.push(self.emotion_card(
            &dir,
            &library,
            None,
            filter.as_deref(),
            editable,
            window,
            cx,
        ));
        cards.push(
            div()
                .id("emoji-lib-add-emotion")
                .debug_selector(|| "emoji-lib-add-emotion".into())
                .role(Role::Button)
                .aria_label("Add emotion")
                .flex_1()
                .min_w_0()
                .min_h(px(120.))
                .rounded_lg()
                .border_1()
                .border_dashed()
                .border_color(content(0.15))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(content(0.55))
                .when(editable, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.border_color(accent_color()).text_color(content(1.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Ok(id) = this.edit(cx, |l| Ok(l.add_emotion())) {
                                this.focus_name = Some(id);
                            }
                        }))
                })
                .when(!editable, |d| d.opacity(0.4))
                .child("+ Add emotion")
                .into_any_element(),
        );
        let mut rows = Vec::new();
        let mut cards = cards.into_iter();
        loop {
            let row: Vec<AnyElement> = cards.by_ref().take(3).collect();
            if row.is_empty() {
                break;
            }
            let fill = 3 - row.len();
            rows.push(
                div()
                    .flex()
                    .gap_2p5()
                    .children(row)
                    .children((0..fill).map(|_| div().flex_1().min_w_0())),
            );
        }
        card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("Emotions"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(content(0.45))
                                    .child("Drag emoji to move them. The description tells a Mono when to use the emotion."),
                            ),
                    )
                    .child(trigger)
                    .child(reset),
            )
            .child(
                div()
                    .id("emoji-lib-board")
                    .debug_selector(|| "emoji-lib-board".into())
                    .px_3()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_2p5()
                    .children(rows),
            )
            .into_any_element()
    }

    /// One card of the board: an emotion, or Unsorted when `emotion` is none.
    #[allow(clippy::too_many_arguments)]
    fn emotion_card(
        &mut self,
        dir: &Path,
        library: &Library,
        emotion: Option<&Emotion>,
        filter: Option<&str>,
        editable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let target = emotion.map(|e| e.id.clone());
        let key = target.clone().unwrap_or_else(|| "unsorted".into());
        let wanted = &target;
        let count = library
            .packs
            .iter()
            .map(|p| p.emoji.iter().filter(|e| e.emotion == *wanted).count())
            .sum::<usize>();
        let chips: Vec<AnyElement> = library
            .packs
            .iter()
            .filter(|p| filter.is_none_or(|id| id == p.id))
            .flat_map(|p| {
                p.emoji
                    .iter()
                    .filter(move |e| e.emotion == *wanted)
                    .map(move |e| (p, e))
            })
            .map(|(p, e)| self.chip(dir, p, e, 36., editable, "chip", cx))
            .collect();
        let head = match emotion {
            Some(emotion) => {
                let name = self.field(false, emotion, window, cx);
                if self.focus_name.as_deref() == Some(emotion.id.as_str()) {
                    self.focus_name = None;
                    window.focus(&name.focus_handle(cx), cx);
                    name.update(cx, |f, cx| {
                        let end = f.content.len();
                        f.select_range(0..end, cx)
                    });
                }
                name.update(cx, |f, _| f.set_read_only(!editable));
                let eid = emotion.id.clone();
                let delete = div()
                    .id(SharedString::from(format!("emoji-lib-delete-{eid}")))
                    .debug_selector({
                        let eid = eid.clone();
                        move || format!("emoji-lib-delete-{eid}")
                    })
                    .role(Role::Button)
                    .aria_label(format!("Delete {}", emotion.name))
                    .size(px(22.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .when(editable, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(content(0.08)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let eid = eid.clone();
                                if count == 0 {
                                    this.delete_emotion(&eid, cx);
                                } else {
                                    this.ask(
                                        window,
                                        cx,
                                        "Delete this emotion?",
                                        &format!(
                                            "Its {} will move to Unsorted.",
                                            plural(count, "emoji", "emoji")
                                        ),
                                        "Delete",
                                        move |this, cx| this.delete_emotion(&eid, cx),
                                    );
                                }
                            }))
                    })
                    .when(!editable, |d| d.opacity(0.4))
                    .child(icon("trash", 12.).text_color(content(0.45)));
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().h(px(24.)).child(name))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(11.))
                            .text_color(content(0.4))
                            .child(count.to_string()),
                    )
                    .child(delete)
                    .into_any_element()
            }
            None => div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_1()
                        .px_1()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child("Unsorted"),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(count.to_string()),
                )
                .into_any_element(),
        };
        let second = match emotion {
            Some(emotion) => {
                let description = self.field(true, emotion, window, cx);
                description.update(cx, |f, _| f.set_read_only(!editable));
                let error = self.name_errors.get(&emotion.id).cloned();
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(error.map(|message| {
                        alert(format!("emoji-lib-name-error-{}", emotion.id), message)
                    }))
                    .child(div().h(px(22.)).child(description))
                    .into_any_element()
            }
            None => div()
                .px_1()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child("Never used by a Mono")
                .into_any_element(),
        };
        div()
            .id(SharedString::from(format!("emoji-lib-emotion-{key}")))
            .debug_selector({
                let key = key.clone();
                move || format!("emoji-lib-emotion-{key}")
            })
            .flex_1()
            .min_w_0()
            .min_h(px(120.))
            .p_2p5()
            .rounded_lg()
            .border_1()
            .when(emotion.is_none(), |d| d.border_dashed())
            .border_color(content(0.08))
            .bg(content(0.03))
            .flex()
            .flex_col()
            .gap_2()
            .drag_over::<DraggedEmoji>(move |style, _, _, _| {
                style
                    .border_color(accent_color())
                    .bg(accent_color().opacity(0.08))
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedEmoji, _, cx| {
                cx.stop_propagation();
                let (pack, file, target) =
                    (dragged.pack.clone(), dragged.file.clone(), target.clone());
                let _ = this.edit(cx, move |l| {
                    l.move_emoji(&pack, &file, target.as_deref());
                    Ok(())
                });
            }))
            .child(head)
            .child(second)
            .child(
                div()
                    .id(SharedString::from(format!("emoji-lib-chips-{key}")))
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(chips),
            )
            .into_any_element()
    }

    fn delete_emotion(&mut self, emotion: &str, cx: &mut Context<Self>) {
        let id = emotion.to_owned();
        self.name_errors.remove(emotion);
        let _ = self.edit(cx, move |l| {
            l.delete_emotion(&id);
            Ok(())
        });
    }
}
