//! A Mono's Emoji settings page: the on/off switch and frequency, the packs, and the board of
//! emotions. See `docs/native/mono-emoji.md`.
//!
//! The loaded library of each Mono lives in `Monos::emoji` as an `Arc<Library>` and is edited
//! in place with `Arc::make_mut`, so a chat that holds an older `Arc` keeps a stable snapshot.
//! Every edit marks the Mono dirty and one serialized background writer saves the latest state.
use super::emoji_import::{self, ImportDialog};
use super::panels::{card, setting, switch_setting, switch_track};
use super::*;
use crate::{
    attachment_tray::{DropHooks, NO_FILE_DRAGGED, bind_drop},
    cloud_dialog_ui::alert,
    cloud_mark::CloudTip,
    popover::{Choice, ChoiceEvent, Choices, Popover},
};
use gpui::{CursorStyle, Div, ObjectFit, PromptLevel, Render, Stateful};
use monocode_core::mono::emoji::{self, Emoji, Emotion, Format, Frequency, Library, Pack};
use std::{path::Path, sync::Arc};

pub(super) const SUBTITLE: &str =
    "Let this Mono add an emoji from its packs to a reply when it fits the mood.";
pub(super) const SWITCH_HELP: &str =
    "Off by default. Never in code, commit messages, PR text or reports.";
pub(super) const READ_ONLY: &str = "These emoji settings were saved by a newer version of Monoptah. They are shown here but cannot be changed.";
const SELECT_PRIORITY: usize = 90;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectKind {
    Frequency,
    Filter,
}

#[derive(Clone, PartialEq)]
pub(super) enum SelectPick {
    Frequency(Frequency),
    Pack(Option<String>),
}

pub(super) struct OpenSelect {
    kind: SelectKind,
    choices: Choices<SelectPick>,
}

/// The page's own state; the libraries themselves are in `Monos::emoji`.
pub(super) struct EmojiUi {
    loading: HashSet<String>,
    pub(super) read_only: HashSet<String>,
    pub(super) errors: HashMap<String, String>,
    /// Monos whose library changed since it was last written.
    dirty: HashSet<String>,
    saving: bool,
    /// Show only the emoji of this pack on the board.
    pub(super) filter: Option<String>,
    /// Why the name being typed into an emotion card was refused, by emotion id.
    pub(super) name_errors: HashMap<String, String>,
    wired: HashSet<gpui::EntityId>,
    focus_name: Option<String>,
    select: Option<OpenSelect>,
    frequency_anchor: AnchorSlot,
    filter_anchor: AnchorSlot,
    pub(super) drop_over: bool,
    pub(super) import: Option<ImportDialog>,
    pub(super) sorter: emoji_import::SorterFactory,
    pub(super) next_token: u64,
}

impl EmojiUi {
    #[cfg(test)]
    pub(super) fn select_is_open(&self) -> bool {
        self.select.is_some()
    }

    pub(super) fn new() -> Self {
        Self {
            loading: HashSet::new(),
            read_only: HashSet::new(),
            errors: HashMap::new(),
            dirty: HashSet::new(),
            saving: false,
            filter: None,
            name_errors: HashMap::new(),
            wired: HashSet::new(),
            focus_name: None,
            select: None,
            frequency_anchor: AnchorSlot::default(),
            filter_anchor: AnchorSlot::default(),
            drop_over: false,
            import: None,
            sorter: emoji_import::default_sorter(),
            next_token: 0,
        }
    }
}

/// What travels with the pointer while an emoji chip is dragged.
#[derive(Clone)]
struct DraggedEmoji {
    mono: String,
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
            .child(picture(&self.path, 36., "emoji-dragged".into()))
    }
}

/// An emoji image, `side` points square. The id lets a GIF animate.
pub(super) fn picture(path: &Path, side: f32, id: SharedString) -> impl IntoElement + use<> {
    crate::image_memory::thumbnail(path.to_path_buf())
        .object_fit(ObjectFit::Contain)
        .size(px(side))
        .id(id)
}

/// "29 PNG, 3 GIF" for a list of formats, in a fixed order.
pub(super) fn format_list(formats: impl IntoIterator<Item = Format>) -> Vec<(Format, usize)> {
    let formats: Vec<Format> = formats.into_iter().collect();
    [Format::Png, Format::Gif, Format::Jpeg, Format::Webp]
        .into_iter()
        .filter_map(|format| {
            let count = formats.iter().filter(|f| **f == format).count();
            (count > 0).then_some((format, count))
        })
        .collect()
}

pub(super) fn format_text(counts: &[(Format, usize)]) -> String {
    counts
        .iter()
        .map(|(format, count)| format!("{count} {}", format.label()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn frequency_label(frequency: Frequency) -> &'static str {
    match frequency {
        Frequency::Rarely => "Rarely",
        Frequency::Sometimes => "Sometimes",
        Frequency::Often => "Often",
    }
}

fn pack_counts(pack: &Pack) -> String {
    let counts = emoji::format_counts(pack);
    if counts.is_empty() {
        "0 emoji".into()
    } else {
        format!(
            "{} emoji ({})",
            pack.emoji.len(),
            format_text(&counts.into_iter().collect::<Vec<_>>())
        )
    }
}

impl Monos {
    /// The Emoji page opened: read the library again, unless an edit or an import is under way.
    pub(super) fn open_emoji_page(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.active.clone() {
            self.load_emoji(&id, true, cx);
        }
    }

    /// Reads a Mono's library in the background. `force` reads again even when one is loaded.
    pub(super) fn load_emoji(&mut self, id: &str, force: bool, cx: &mut Context<Self>) {
        let ui = &self.emoji_ui;
        if ui.loading.contains(id)
            || (!force && self.emoji.contains_key(id))
            || ui.dirty.contains(id)
            || ui.saving
            || ui.import.is_some()
        {
            return;
        }
        let Ok(dir) = emoji::dir(&self.root, id) else {
            self.emoji_ui
                .errors
                .insert(id.into(), "Not a Mono id".into());
            return;
        };
        self.emoji_ui.loading.insert(id.into());
        let (root, mono) = (self.root.clone(), id.to_string());
        let work = {
            let mono = mono.clone();
            cx.background_executor()
                .spawn(async move { emoji::load(&root, &mono) })
        };
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.emoji_ui.loading.remove(&mono);
                match result {
                    // An edit or an import that began while this was reading wins.
                    Ok(_)
                        if this.emoji_ui.dirty.contains(&mono)
                            || this.emoji_ui.import.is_some() => {}
                    Ok(loaded) => {
                        this.emoji
                            .insert(mono.clone(), (dir, Arc::new(loaded.library)));
                        if loaded.read_only {
                            this.emoji_ui.read_only.insert(mono.clone());
                        } else {
                            this.emoji_ui.read_only.remove(&mono);
                        }
                        this.emoji_ui.errors.remove(&mono);
                        this.emoji_changed(&mono, cx);
                    }
                    Err(error) => {
                        this.emoji_ui.errors.insert(mono, error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Mono's library changed, or was loaded or saved.
    pub(super) fn emoji_changed(&mut self, mono_id: &str, cx: &mut Context<Self>) {
        // Wire to Conversation::set_mono_emoji: pass this Mono's `(dir, Arc<Library>)` from
        // `self.emoji` to its chat (`self.chats`) so the transcript can draw `:emo[...]`.
        let _ = mono_id;
        cx.notify();
    }

    /// Changes a Mono's library in place and queues it for writing. An `Err` leaves the library
    /// as it was and writes nothing. Nothing is edited while the file is read-only.
    pub(super) fn emoji_edit<R>(
        &mut self,
        mono: &str,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Library) -> Result<R, String>,
    ) -> Result<R, String> {
        if self.emoji_ui.read_only.contains(mono) {
            return Err(READ_ONLY.into());
        }
        let Some((_, library)) = self.emoji.get_mut(mono) else {
            return Err("The emoji settings are not loaded yet".into());
        };
        let value = edit(Arc::make_mut(library))?;
        self.emoji_ui.dirty.insert(mono.into());
        self.emoji_pump(cx);
        cx.notify();
        Ok(value)
    }

    /// Writes the next changed library, one write at a time. A change made during a write
    /// marks the Mono dirty again, so the latest state is always written last.
    fn emoji_pump(&mut self, cx: &mut Context<Self>) {
        if self.emoji_ui.saving {
            return;
        }
        let Some(mono) = self.emoji_ui.dirty.iter().next().cloned() else {
            return;
        };
        self.emoji_ui.dirty.remove(&mono);
        let Some((_, library)) = self.emoji.get(&mono) else {
            return;
        };
        let snapshot = library.clone();
        self.emoji_ui.saving = true;
        let root = self.root.clone();
        let work = {
            let mono = mono.clone();
            cx.background_executor()
                .spawn(async move { emoji::save(&root, &mono, &snapshot) })
        };
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.emoji_ui.saving = false;
                match result {
                    Ok(()) => {
                        this.emoji_ui.errors.remove(&mono);
                        this.emoji_changed(&mono, cx);
                    }
                    Err(error) => {
                        this.emoji_ui.errors.insert(
                            mono.clone(),
                            format!("Could not save the emoji settings. {error}"),
                        );
                    }
                }
                this.emoji_pump(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn emoji_ask(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        title: &str,
        detail: &str,
        yes: &'static str,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        let prompt = window.prompt(
            PromptLevel::Warning,
            title,
            Some(detail),
            &["Cancel", yes],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if prompt.await.ok() == Some(1) {
                let _ = this.update(cx, then);
            }
        })
        .detach();
    }

    fn emoji_open_select(&mut self, kind: SelectKind, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.emoji_ui.select.take() {
            open.choices.close(window, cx);
            if open.kind == kind {
                cx.notify();
                return;
            }
        }
        let Some(mono) = self.active.clone() else {
            return;
        };
        let Some((_, library)) = self.emoji.get(&mono) else {
            return;
        };
        let (label, choices, anchor) = match kind {
            SelectKind::Frequency => (
                "How often",
                [Frequency::Rarely, Frequency::Sometimes, Frequency::Often]
                    .into_iter()
                    .map(|f| Choice {
                        value: SelectPick::Frequency(f),
                        label: frequency_label(f).into(),
                        checked: library.frequency == f,
                    })
                    .collect::<Vec<_>>(),
                self.emoji_ui.frequency_anchor.clone(),
            ),
            SelectKind::Filter => {
                let filter = self.emoji_ui.filter.clone();
                let mut choices = vec![Choice {
                    value: SelectPick::Pack(None),
                    label: "All packs".into(),
                    checked: filter.is_none(),
                }];
                choices.extend(library.packs.iter().map(|p| Choice {
                    value: SelectPick::Pack(Some(p.id.clone())),
                    label: p.name.clone().into(),
                    checked: filter.as_deref() == Some(&p.id),
                }));
                ("Pack", choices, self.emoji_ui.filter_anchor.clone())
            }
        };
        let width = anchor
            .get()
            .map_or(200., |b| b.size.width.as_f32().max(160.));
        let choices = Choices::open(
            label,
            choices,
            width,
            window,
            cx,
            |this, event, window, cx| {
                if let Some(open) = this.emoji_ui.select.take() {
                    open.choices.close(window, cx);
                }
                if let ChoiceEvent::Picked(pick) = event {
                    this.emoji_pick(pick, cx);
                }
                cx.notify();
            },
        );
        self.emoji_ui.select = Some(OpenSelect { kind, choices });
        cx.notify();
    }

    fn emoji_pick(&mut self, pick: SelectPick, cx: &mut Context<Self>) {
        match pick {
            SelectPick::Frequency(frequency) => {
                if let Some(mono) = self.active.clone() {
                    let _ = self.emoji_edit(&mono, cx, |l| {
                        l.frequency = frequency;
                        Ok(())
                    });
                }
            }
            SelectPick::Pack(pack) => self.emoji_ui.filter = pack,
        }
    }

    pub(super) fn emoji_select_popover(&self, cx: &App) -> Option<Popover> {
        let open = self.emoji_ui.select.as_ref()?;
        let anchor = match open.kind {
            SelectKind::Frequency => &self.emoji_ui.frequency_anchor,
            SelectKind::Filter => &self.emoji_ui.filter_anchor,
        };
        Some(open.choices.popover(anchor, SELECT_PRIORITY, cx))
    }

    fn emoji_select_trigger(
        &self,
        id: &'static str,
        text: String,
        kind: SelectKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let anchor = match kind {
            SelectKind::Frequency => &self.emoji_ui.frequency_anchor,
            SelectKind::Filter => &self.emoji_ui.filter_anchor,
        };
        div()
            .id(id)
            .debug_selector(move || id.into())
            .role(Role::Button)
            .aria_label(text.clone())
            .relative()
            .h(px(28.))
            .min_w(px(120.))
            .max_w(px(220.))
            .px_2p5()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .text_size(px(12.))
            .text_color(content(0.85))
            .when(enabled, |d| {
                d.cursor_pointer().hover(|s| s.bg(content(0.06))).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.emoji_open_select(kind, window, cx)
                    }),
                )
            })
            .when(!enabled, |d| d.opacity(0.4))
            .child(anchor.probe())
            .child(div().min_w_0().truncate().child(text))
            .child(icon("chevron-down", 12.).text_color(content(0.5)))
    }

    fn emoji_text_changed(
        &mut self,
        description: bool,
        mono: &str,
        emotion: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self
            .emoji
            .get(mono)
            .and_then(|(_, l)| l.emotions.iter().find(|e| e.id == emotion))
        else {
            return;
        };
        if description {
            if current.description != text.trim() {
                let (id, text) = (emotion.to_owned(), text.to_owned());
                let _ = self.emoji_edit(mono, cx, move |l| {
                    l.describe_emotion(&id, &text);
                    Ok(())
                });
            }
        } else if current.name == text.trim() {
            self.emoji_ui.name_errors.remove(emotion);
            cx.notify();
        } else {
            let (id, name) = (emotion.to_owned(), text.to_owned());
            match self.emoji_edit(mono, cx, move |l| l.rename_emotion(&id, &name)) {
                Ok(()) => {
                    self.emoji_ui.name_errors.remove(emotion);
                }
                Err(error) => {
                    self.emoji_ui.name_errors.insert(emotion.into(), error);
                }
            }
            cx.notify();
        }
    }

    /// The text field of an emotion card, kept in step with the library while it is not being
    /// edited and wired to save what is typed.
    fn emoji_field(
        &mut self,
        description: bool,
        mono: &str,
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
        let key = format!("emoji-{kind}:{mono}:{}", emotion.id);
        let field = self.field(&key, label, text.clone(), false, cx);
        if self.emoji_ui.wired.insert(field.entity_id()) {
            let (mono, id) = (mono.to_owned(), emotion.id.clone());
            self.observations.push(cx.subscribe(
                &field,
                move |this, input, _: &input::Changed, cx| {
                    let text = input.read(cx).content.to_string();
                    this.emoji_text_changed(description, &mono, &id, &text, cx);
                },
            ));
            let id = emotion.id.clone();
            self.observations.push(cx.on_blur(
                &field.focus_handle(cx),
                window,
                move |this, _, cx| {
                    // A refused name goes back to the saved one when the field is left.
                    if this.emoji_ui.name_errors.remove(&id).is_some() {
                        cx.notify();
                    }
                },
            ));
        }
        let editing = field.focus_handle(cx).is_focused(window)
            || (!description && self.emoji_ui.name_errors.contains_key(&emotion.id));
        if !editing && *field.read(cx).content != *text.as_str() {
            field.update(cx, |f, cx| f.set_text(text, cx));
        }
        field
    }

    #[allow(clippy::too_many_arguments)]
    fn emoji_chip(
        &self,
        mono: &str,
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
        let id = format!("mono-emoji-{scope}-{}-{}", pack.id, emoji.file);
        let selector = id.clone();
        let payload = DraggedEmoji {
            mono: mono.into(),
            pack: pack.id.clone(),
            file: emoji.file.clone(),
            path: path.clone(),
        };
        let _ = cx;
        div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || selector.clone())
            .aria_label(tip.clone())
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
            .child(picture(&path, side, format!("{id}-image").into()))
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

    pub(super) fn emoji_page(
        &mut self,
        mono: &Mono,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = mono.id.clone();
        self.load_emoji(&id, false, cx);
        let loaded = self
            .emoji
            .get(&id)
            .map(|(dir, library)| (dir.clone(), library.clone()));
        let read_only = self.emoji_ui.read_only.contains(&id);
        let editable = !read_only;
        let importing = self.emoji_ui.import.is_some();
        let mut sections: Vec<AnyElement> = vec![
            div()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(content(0.55))
                .child(SUBTITLE)
                .into_any_element(),
        ];
        if read_only {
            sections.push(
                div()
                    .id("mono-emoji-read-only")
                    .debug_selector(|| "mono-emoji-read-only".into())
                    .rounded_lg()
                    .bg(content(0.05))
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.7))
                    .child(READ_ONLY)
                    .into_any_element(),
            );
        }
        if let Some(error) = self.emoji_ui.errors.get(&id) {
            sections.push(alert("mono-emoji-error", error.clone()).into_any_element());
        }
        match loaded {
            None => {
                if !self.emoji_ui.errors.contains_key(&id) {
                    sections.push(
                        div()
                            .px_2()
                            .py_6()
                            .text_center()
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child("Loading\u{2026}")
                            .into_any_element(),
                    );
                }
            }
            Some((dir, library)) => {
                sections.push(self.emoji_settings_card(&id, &library, editable, cx));
                sections.push(self.emoji_packs_card(&id, &dir, &library, editable, importing, cx));
                sections.push(self.emoji_board(&id, &dir, &library, editable, window, cx));
            }
        }
        let over = self.emoji_ui.drop_over;
        let page = bind_drop(
            div()
                .id("mono-emoji-page")
                .debug_selector(|| "mono-emoji-page".into())
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .relative(),
            DropHooks {
                over: |this: &Self| this.emoji_ui.drop_over,
                set_over: |this, over, cx| {
                    if this.emoji_ui.drop_over != over {
                        this.emoji_ui.drop_over = over;
                        cx.notify();
                    }
                },
                drop: |this, paths, window: &mut Window, cx| {
                    if paths.is_empty() {
                        this.emoji_ui.errors.insert(
                            this.active.clone().unwrap_or_default(),
                            NO_FILE_DRAGGED.into(),
                        );
                        cx.notify();
                    } else {
                        this.emoji_import_paths(paths, window, cx);
                    }
                },
            },
            cx,
        );
        page.child(Self::settings_page_header("Emoji"))
            .child(
                div()
                    .id("mono-emoji-scroll")
                    .debug_selector(|| "mono-emoji-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(
                        sections
                            .into_iter()
                            .map(|section| div().flex_shrink_0().child(section)),
                    ),
            )
            .when(over, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded_lg()
                        .border_2()
                        .border_color(accent_color().opacity(0.6))
                        .bg(accent_color().opacity(0.06)),
                )
            })
            .into_any_element()
    }

    fn emoji_settings_card(
        &mut self,
        mono: &str,
        library: &Library,
        editable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = mono;
        let toggle = switch_track(
            "mono-emoji-enabled",
            "Use emoji in replies",
            library.enabled,
        )
        .debug_selector(|| "mono-emoji-enabled".into())
        .when(editable, |d| {
            d.cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                if let Some(mono) = this.active.clone() {
                    let _ = this.emoji_edit(&mono, cx, |l| {
                        l.enabled = !l.enabled;
                        Ok(())
                    });
                }
            }))
        })
        .when(!editable, |d| d.opacity(0.4));
        let frequency = self.emoji_select_trigger(
            "mono-emoji-frequency",
            frequency_label(library.frequency).into(),
            SelectKind::Frequency,
            editable,
            cx,
        );
        card()
            .child(switch_setting(
                "Use emoji in replies",
                SWITCH_HELP,
                false,
                toggle.into_any_element(),
            ))
            .child(div().h(px(1.)).bg(content(0.05)))
            .child(setting("How often", None, frequency.into_any_element()))
            .into_any_element()
    }

    fn emoji_packs_card(
        &mut self,
        mono: &str,
        dir: &Path,
        library: &Library,
        editable: bool,
        importing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total: usize = library.packs.iter().map(|p| p.emoji.len()).sum();
        let formats = format_list(
            library
                .packs
                .iter()
                .flat_map(|p| p.emoji.iter().map(|e| e.format)),
        );
        let summary = if library.packs.is_empty() {
            "No packs yet".to_string()
        } else {
            format!(
                "{} \u{b7} {total} emoji \u{b7} {}",
                plural(library.packs.len(), "pack", "packs"),
                formats
                    .iter()
                    .map(|(f, _)| f.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let can_import = editable && !importing;
        let header = div()
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
                            .child("Packs"),
                    )
                    .child(
                        div()
                            .id("mono-emoji-pack-summary")
                            .debug_selector(|| "mono-emoji-pack-summary".into())
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child(summary),
                    ),
            )
            .child(
                crate::modal::disabled(
                    crate::modal::ghost_button("mono-emoji-import", "Import zip...")
                        .debug_selector(|| "mono-emoji-import".into())
                        .border_1()
                        .border_color(content(0.1)),
                    !can_import,
                )
                .when(can_import, |d| {
                    d.on_click(cx.listener(|this, _, window, cx| this.emoji_pick_zips(window, cx)))
                }),
            );
        let rows: Vec<AnyElement> = library
            .packs
            .iter()
            .map(|pack| self.emoji_pack_row(mono, dir, pack, editable, importing, cx))
            .collect();
        let over = self.emoji_ui.drop_over;
        card()
            .child(header)
            .children(rows)
            .child(
                div()
                    .id("mono-emoji-drop-hint")
                    .debug_selector(|| "mono-emoji-drop-hint".into())
                    .m_3()
                    .px_4()
                    .py_4()
                    .rounded_lg()
                    .border_1()
                    .border_dashed()
                    .border_color(if over { accent_color() } else { content(0.15) })
                    .text_center()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(content(0.55))
                    .child("Drop one or more .zip files here, or Import zip... to pick several at once. A pack that is turned off keeps its sorting but Mono does not use it."),
            )
            .into_any_element()
    }

    fn emoji_pack_row(
        &self,
        mono: &str,
        dir: &Path,
        pack: &Pack,
        editable: bool,
        importing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pid = pack.id.clone();
        let thumbs = pack.emoji.iter().take(5).map(|e| {
            picture(
                &emoji::image_path(dir, pack, e),
                22.,
                format!("mono-emoji-thumb-{}-{}", pack.id, e.file).into(),
            )
        });
        let pill = if !pack.enabled {
            ("turned off".to_string(), false)
        } else if let Some(model) = &pack.sorted_by {
            (format!("sorted by {model}"), true)
        } else {
            ("not sorted".to_string(), false)
        };
        let busy = !editable || importing;
        let action = |id: String, label: &'static str, danger: bool| {
            let selector = id.clone();
            crate::modal::disabled(
                crate::modal::ghost_button(SharedString::from(id), label)
                    .debug_selector(move || selector.clone())
                    .h(px(26.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .when(danger, |d| d.text_color(content(0.6))),
                busy,
            )
        };
        let resort_click = {
            let (mono, pid) = (mono.to_owned(), pid.clone());
            cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                this.emoji_resort(&mono, &pid, window, cx)
            })
        };
        let remove_click = {
            let (mono, pid, name) = (mono.to_owned(), pid.clone(), pack.name.clone());
            cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                let (mono, pid) = (mono.clone(), pid.clone());
                this.emoji_ask(
                    window,
                    cx,
                    "Remove this pack?",
                    &format!("\"{name}\" and its emoji are deleted from this Mono. Turn the pack off instead to keep them."),
                    "Remove",
                    move |this, cx| this.emoji_remove_pack(&mono, &pid, cx),
                );
            })
        };
        let toggle_click = {
            let (mono, pid) = (mono.to_owned(), pid.clone());
            cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                let on = this
                    .emoji
                    .get(&mono)
                    .and_then(|(_, l)| l.packs.iter().find(|p| p.id == pid))
                    .is_some_and(|p| p.enabled);
                let pid = pid.clone();
                let _ = this.emoji_edit(&mono, cx, move |l| {
                    l.set_pack_enabled(&pid, !on);
                    Ok(())
                });
            })
        };
        let resort = action(format!("mono-emoji-resort-{pid}"), "Re-sort", false)
            .when(!busy && !pack.emoji.is_empty(), |d| {
                d.on_click(resort_click)
            });
        let remove = action(format!("mono-emoji-remove-{pid}"), "Remove", true)
            .when(!busy, |d| d.on_click(remove_click));
        let toggle = switch_track(
            SharedString::from(format!("mono-emoji-pack-switch-{pid}")),
            format!("Use {}", pack.name),
            pack.enabled,
        )
        .debug_selector({
            let pid = pid.clone();
            move || format!("mono-emoji-pack-switch-{pid}")
        })
        .when(editable, |d| d.cursor_pointer().on_click(toggle_click))
        .when(!editable, |d| d.opacity(0.4));
        div()
            .id(SharedString::from(format!("mono-emoji-pack-{pid}")))
            .debug_selector({
                let pid = pid.clone();
                move || format!("mono-emoji-pack-{pid}")
            })
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_t_1()
            .border_color(content(0.05))
            .child(div().flex().items_center().gap(px(2.)).children(thumbs))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .truncate()
                                    .child(pack.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(12.))
                                    .text_color(content(0.45))
                                    .child(format!("\u{b7} {}", pack_counts(pack))),
                            ),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("mono-emoji-pill-{pid}")))
                    .debug_selector({
                        let pid = pid.clone();
                        move || format!("mono-emoji-pill-{pid}")
                    })
                    .flex_shrink_0()
                    .px_2()
                    .rounded_full()
                    .border_1()
                    .border_color(if pill.1 {
                        accent_color().opacity(0.4)
                    } else {
                        content(0.1)
                    })
                    .text_size(px(11.))
                    .line_height(px(18.))
                    .text_color(if pill.1 {
                        accent_color()
                    } else {
                        content(0.55)
                    })
                    .child(pill.0),
            )
            .child(resort)
            .child(remove)
            .child(toggle)
            .into_any_element()
    }

    fn emoji_remove_pack(&mut self, mono: &str, pack_id: &str, cx: &mut Context<Self>) {
        let Some((dir, library)) = self.emoji.get(mono) else {
            return;
        };
        // Remove the folder off the main thread on a copy; the live library follows once it is gone.
        let (dir, mut copy) = (dir.clone(), (**library).clone());
        let (mono, pack_id) = (mono.to_owned(), pack_id.to_owned());
        let work = {
            let pack_id = pack_id.clone();
            cx.background_executor()
                .spawn(async move { copy.remove_pack(&dir, &pack_id) })
        };
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        if this.emoji_ui.filter.as_deref() == Some(pack_id.as_str()) {
                            this.emoji_ui.filter = None;
                        }
                        let _ = this.emoji_edit(&mono, cx, |l| {
                            l.packs.retain(|p| p.id != pack_id);
                            Ok(())
                        });
                    }
                    Err(error) => {
                        this.emoji_ui.errors.insert(mono, error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn emoji_board(
        &mut self,
        mono: &str,
        dir: &Path,
        library: &Library,
        editable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = self
            .emoji_ui
            .filter
            .clone()
            .filter(|id| library.packs.iter().any(|p| &p.id == id));
        let filter_text = filter
            .as_ref()
            .and_then(|id| library.packs.iter().find(|p| &p.id == id))
            .map_or("All packs".to_string(), |p| p.name.clone());
        let trigger = self.emoji_select_trigger(
            "mono-emoji-filter",
            filter_text,
            SelectKind::Filter,
            true,
            cx,
        );
        let reset = crate::modal::disabled(
            crate::modal::ghost_button("mono-emoji-reset", "Reset to defaults")
                .debug_selector(|| "mono-emoji-reset".into())
                .h(px(28.))
                .px(px(10.))
                .text_size(px(12.)),
            !editable,
        )
        .when(editable, |d| {
            d.on_click(cx.listener(|this, _, window, cx| {
                let Some(mono) = this.active.clone() else {
                    return;
                };
                this.emoji_ask(
                    window,
                    cx,
                    "Reset emotions to defaults?",
                    "The 13 default emotions come back. Emoji whose emotion is one of them keep it; the rest go to Unsorted.",
                    "Reset",
                    move |this, cx| {
                        this.emoji_ui.name_errors.clear();
                        let _ = this.emoji_edit(&mono, cx, |l| {
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
                self.emoji_card(
                    mono,
                    dir,
                    library,
                    Some(emotion),
                    filter.as_deref(),
                    editable,
                    window,
                    cx,
                )
            })
            .collect();
        cards.push(self.emoji_card(
            mono,
            dir,
            library,
            None,
            filter.as_deref(),
            editable,
            window,
            cx,
        ));
        cards.push(
            div()
                .id("mono-emoji-add-emotion")
                .debug_selector(|| "mono-emoji-add-emotion".into())
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
                            let Some(mono) = this.active.clone() else {
                                return;
                            };
                            if let Ok(id) = this.emoji_edit(&mono, cx, |l| Ok(l.add_emotion())) {
                                this.emoji_ui.focus_name = Some(id);
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
                                    .child("Drag emoji to move them. The description tells Mono when to use the emotion."),
                            ),
                    )
                    .child(trigger)
                    .child(reset),
            )
            .child(
                div()
                    .id("mono-emoji-board")
                    .debug_selector(|| "mono-emoji-board".into())
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
    fn emoji_card(
        &mut self,
        mono: &str,
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
            .map(|(p, e)| self.emoji_chip(mono, dir, p, e, 36., editable, "chip", cx))
            .collect();
        let head = match emotion {
            Some(emotion) => {
                let name = self.emoji_field(false, mono, emotion, window, cx);
                if self.emoji_ui.focus_name.as_deref() == Some(emotion.id.as_str()) {
                    self.emoji_ui.focus_name = None;
                    window.focus(&name.focus_handle(cx), cx);
                    name.update(cx, |f, cx| {
                        let end = f.content.len();
                        f.select_range(0..end, cx)
                    });
                }
                name.update(cx, |f, _| f.set_read_only(!editable));
                let (mono_id, eid) = (mono.to_owned(), emotion.id.clone());
                let delete = div()
                    .id(SharedString::from(format!("mono-emoji-delete-{eid}")))
                    .debug_selector({
                        let eid = eid.clone();
                        move || format!("mono-emoji-delete-{eid}")
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
                                let (mono, eid) = (mono_id.clone(), eid.clone());
                                if count == 0 {
                                    this.emoji_delete_emotion(&mono, &eid, cx);
                                } else {
                                    this.emoji_ask(
                                        window,
                                        cx,
                                        "Delete this emotion?",
                                        &format!(
                                            "Its {} will move to Unsorted.",
                                            plural(count, "emoji", "emoji")
                                        ),
                                        "Delete",
                                        move |this, cx| this.emoji_delete_emotion(&mono, &eid, cx),
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
                let description = self.emoji_field(true, mono, emotion, window, cx);
                description.update(cx, |f, _| f.set_read_only(!editable));
                let error = self.emoji_ui.name_errors.get(&emotion.id).cloned();
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(error.map(|message| {
                        alert(format!("mono-emoji-name-error-{}", emotion.id), message)
                    }))
                    .child(div().h(px(22.)).child(description))
                    .into_any_element()
            }
            None => div()
                .px_1()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child("Never used by Mono")
                .into_any_element(),
        };
        let mono_id = mono.to_owned();
        div()
            .id(SharedString::from(format!("mono-emoji-emotion-{key}")))
            .debug_selector({
                let key = key.clone();
                move || format!("mono-emoji-emotion-{key}")
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
                if dragged.mono == mono_id {
                    let (pack, file, target) =
                        (dragged.pack.clone(), dragged.file.clone(), target.clone());
                    let _ = this.emoji_edit(&mono_id, cx, move |l| {
                        l.move_emoji(&pack, &file, target.as_deref());
                        Ok(())
                    });
                }
            }))
            .child(head)
            .child(second)
            .child(
                div()
                    .id(SharedString::from(format!("mono-emoji-chips-{key}")))
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(chips),
            )
            .into_any_element()
    }

    fn emoji_delete_emotion(&mut self, mono: &str, emotion: &str, cx: &mut Context<Self>) {
        let id = emotion.to_owned();
        self.emoji_ui.name_errors.remove(emotion);
        let _ = self.emoji_edit(mono, cx, move |l| {
            l.delete_emotion(&id);
            Ok(())
        });
    }
}
