//! The emoji library page: the packs card, the board of emotions and the import dialog. One
//! entity, rendered under Settings > Monos > Emoji, so a GIF that plays on hover redraws this
//! view only.
use super::{
    EmojiStore, card,
    import::{ImportDialog, ModelPicker, SorterFactory, default_sorter},
    pack_counts, picture, plural,
    store::READ_ONLY,
};
use crate::{
    attachment_tray::{DropHooks, NO_FILE_DRAGGED, bind_drop},
    cloud_dialog_ui::alert,
    input::Input,
    popover::{AnchorSlot, Choice, ChoiceEvent, Choices, Popover},
    theme::*,
};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, PromptLevel, Render, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use monocode_core::mono::emoji::{self, Pack};
use std::collections::{HashMap, HashSet};

const SELECT_PRIORITY: usize = 90;
pub(super) const SUBTITLE: &str = "Packs and emotions are shared by every Mono. Each Mono chooses which packs it may use in its own Emoji settings.";

pub(crate) struct EmojiLibraryView {
    pub(super) store: Entity<EmojiStore>,
    pub(super) focus: FocusHandle,
    /// The chip under the pointer: only its GIF plays.
    pub(super) hovered_chip: Option<String>,
    /// Show only the emoji of this pack on the board.
    pub(super) filter: Option<String>,
    /// Why the name being typed into an emotion card was refused, by emotion id.
    pub(super) name_errors: HashMap<String, String>,
    pub(super) fields: HashMap<String, Entity<Input>>,
    pub(super) wired: HashSet<gpui::EntityId>,
    pub(super) focus_name: Option<String>,
    select: Option<Choices<Option<String>>>,
    pub(super) filter_anchor: AnchorSlot,
    pub(super) drop_over: bool,
    /// An error of this page that the store does not know, such as a drag without a file.
    pub(super) error: Option<String>,
    pub(super) import: Option<ImportDialog>,
    pub(super) picker: Option<ModelPicker>,
    pub(super) model_anchor: AnchorSlot,
    pub(super) sorter: SorterFactory,
    pub(super) next_token: u64,
    /// Shared with the store: true while the import dialog is open.
    pub(super) import_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(super) subscriptions: Vec<Subscription>,
}

impl Drop for EmojiLibraryView {
    fn drop(&mut self) {
        // The page goes away with Settings: stop what the dialog started and let reads run.
        if let Some(dialog) = &self.import {
            dialog.stop();
        }
        self.import_flag
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

impl EmojiLibraryView {
    pub(crate) fn new(store: Entity<EmojiStore>, cx: &mut Context<Self>) -> Self {
        let watch = cx.observe(&store, |_, _, cx| cx.notify());
        // The page reads the library again when it is made, so a file changed outside the app
        // shows. The read waits while an edit, a write or an import is under way.
        store.update(cx, |store, cx| store.load(true, cx));
        let import_flag = store.read(cx).import_flag();
        Self {
            store,
            focus: cx.focus_handle(),
            hovered_chip: None,
            filter: None,
            name_errors: HashMap::new(),
            fields: HashMap::new(),
            wired: HashSet::new(),
            focus_name: None,
            select: None,
            filter_anchor: AnchorSlot::default(),
            drop_over: false,
            error: None,
            import: None,
            picker: None,
            model_anchor: AnchorSlot::default(),
            sorter: default_sorter(),
            next_token: 0,
            import_flag,
            subscriptions: vec![watch],
        }
    }

    /// Escape: closes what is open on this page, innermost first. False when nothing was.
    pub(crate) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.picker.take().is_some() {
            cx.notify();
            return true;
        }
        if let Some(select) = self.select.take() {
            select.close(window, cx);
            cx.notify();
            return true;
        }
        if self.import.is_some() {
            self.import_cancel(window, cx);
            return true;
        }
        false
    }

    #[cfg(test)]
    pub(crate) fn dialog(&self) -> Option<&ImportDialog> {
        self.import.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_sorter(&mut self, sorter: SorterFactory) {
        self.sorter = sorter;
    }

    /// The open model list of the import dialog.
    #[cfg(test)]
    pub(crate) fn model_list(&self) -> Option<Entity<crate::pickers::ModelList>> {
        self.picker.as_ref().map(|p| p.list.clone())
    }

    #[cfg(test)]
    pub(crate) fn hovered_chip(&self) -> Option<String> {
        self.hovered_chip.clone()
    }

    #[cfg(test)]
    pub(crate) fn drop_over(&self) -> bool {
        self.drop_over
    }

    #[cfg(test)]
    pub(crate) fn filter(&self) -> Option<String> {
        self.filter.clone()
    }

    /// The text field of an emotion card, by `emoji-name:<id>` or `emoji-desc:<id>`.
    #[cfg(test)]
    pub(crate) fn field_for_test(&self, key: &str) -> Entity<Input> {
        self.fields[key].clone()
    }

    #[cfg(test)]
    pub(crate) fn select_is_open(&self) -> bool {
        self.select.is_some()
    }

    /// Changes the library through the store. Errors (read-only, not loaded) are shown by the
    /// page itself, so they are dropped here.
    pub(super) fn edit<R>(
        &self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut emoji::Library) -> Result<R, String>,
    ) -> Result<R, String> {
        self.store
            .clone()
            .update(cx, |store, cx| store.edit(cx, edit))
    }

    pub(super) fn ask(
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

    fn open_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.select.take() {
            open.close(window, cx);
            cx.notify();
            return;
        }
        let library = self.store.read(cx).library().clone();
        let mut choices = vec![Choice {
            value: None,
            label: "All packs".into(),
            checked: self.filter.is_none(),
        }];
        choices.extend(library.packs.iter().map(|p| Choice {
            value: Some(p.id.clone()),
            label: p.name.clone().into(),
            checked: self.filter.as_deref() == Some(&p.id),
        }));
        let width = self
            .filter_anchor
            .get()
            .map_or(200., |b| b.size.width.as_f32().max(160.));
        self.select = Some(Choices::open(
            "Pack",
            choices,
            width,
            window,
            cx,
            |this, event, window, cx| {
                if let Some(open) = this.select.take() {
                    open.close(window, cx);
                }
                if let ChoiceEvent::Picked(pack) = event {
                    this.filter = pack;
                }
                cx.notify();
            },
        ));
        cx.notify();
    }

    fn filter_popover(&self, cx: &App) -> Option<Popover> {
        let open = self.select.as_ref()?;
        Some(open.popover(&self.filter_anchor, SELECT_PRIORITY, cx))
    }

    pub(super) fn filter_trigger(
        &self,
        text: String,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id("emoji-lib-filter")
            .debug_selector(|| "emoji-lib-filter".into())
            .role(gpui::Role::Button)
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
            .cursor_pointer()
            .hover(|s| s.bg(content(0.06)))
            .on_click(cx.listener(|this, _, window, cx| this.open_filter(window, cx)))
            .child(self.filter_anchor.probe())
            .child(div().min_w_0().truncate().child(text))
            .child(icon("chevron-down", 12.).text_color(content(0.5)))
    }

    fn packs_card(&mut self, editable: bool, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let library = store.library().clone();
        let dir = store.dir().to_path_buf();
        let importing = self.import.is_some();
        let total: usize = library.packs.iter().map(|p| p.emoji.len()).sum();
        let formats = super::format_list(
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
                            .id("emoji-lib-pack-summary")
                            .debug_selector(|| "emoji-lib-pack-summary".into())
                            .text_size(px(12.))
                            .text_color(content(0.45))
                            .child(summary),
                    ),
            )
            .child(
                crate::modal::disabled(
                    crate::modal::ghost_button("emoji-lib-import", "Import zip...")
                        .debug_selector(|| "emoji-lib-import".into())
                        .border_1()
                        .border_color(content(0.1)),
                    !can_import,
                )
                .when(can_import, |d| {
                    d.on_click(cx.listener(|this, _, window, cx| this.pick_zips(window, cx)))
                }),
            );
        let rows: Vec<AnyElement> = library
            .packs
            .iter()
            .map(|pack| self.pack_row(&dir, pack, editable, importing, cx))
            .collect();
        let over = self.drop_over;
        card()
            .child(header)
            .children(rows)
            .child(
                div()
                    .id("emoji-lib-drop-hint")
                    .debug_selector(|| "emoji-lib-drop-hint".into())
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
                    .child("Drop one or more .zip files here, or Import zip... to pick several at once."),
            )
            .into_any_element()
    }

    fn pack_row(
        &self,
        dir: &std::path::Path,
        pack: &Pack,
        editable: bool,
        importing: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pid = pack.id.clone();
        let thumbs = pack
            .emoji
            .iter()
            .take(5)
            .map(|e| picture(&emoji::image_path(dir, pack, e), 22., None));
        let pill = match &pack.sorted_by {
            Some(model) => (format!("sorted by {model}"), true),
            None => ("not sorted".to_string(), false),
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
            let pid = pid.clone();
            cx.listener(move |this, _: &gpui::ClickEvent, window, cx| this.resort(&pid, window, cx))
        };
        let remove_click = {
            let (pid, name) = (pid.clone(), pack.name.clone());
            cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                let pid = pid.clone();
                this.ask(
                    window,
                    cx,
                    "Remove this pack?",
                    &format!("\"{name}\" and its emoji are deleted from the library, and no Mono can use it any more."),
                    "Remove",
                    move |this, cx| this.remove_pack(&pid, cx),
                );
            })
        };
        let resort = action(format!("emoji-lib-resort-{pid}"), "Re-sort", false)
            .when(!busy && !pack.emoji.is_empty(), |d| {
                d.on_click(resort_click)
            });
        let remove = action(format!("emoji-lib-remove-{pid}"), "Remove", true)
            .when(!busy, |d| d.on_click(remove_click));
        div()
            .id(SharedString::from(format!("emoji-lib-pack-{pid}")))
            .debug_selector({
                let pid = pid.clone();
                move || format!("emoji-lib-pack-{pid}")
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
                    .id(SharedString::from(format!("emoji-lib-pill-{pid}")))
                    .debug_selector({
                        let pid = pid.clone();
                        move || format!("emoji-lib-pill-{pid}")
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
            .into_any_element()
    }

    fn remove_pack(&mut self, pack_id: &str, cx: &mut Context<Self>) {
        if self.filter.as_deref() == Some(pack_id) {
            self.filter = None;
        }
        let pack_id = pack_id.to_owned();
        self.store
            .clone()
            .update(cx, |store, cx| store.remove_pack(&pack_id, cx));
    }
}

impl Focusable for EmojiLibraryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for EmojiLibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (loaded, read_only, store_error) = {
            let store = self.store.read(cx);
            (
                store.loaded(),
                store.read_only(),
                store.error().map(str::to_owned),
            )
        };
        let editable = !read_only;
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
                    .id("emoji-lib-read-only")
                    .debug_selector(|| "emoji-lib-read-only".into())
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
        for error in [self.error.clone(), store_error.clone()]
            .into_iter()
            .flatten()
        {
            sections.push(alert("emoji-lib-error", error).into_any_element());
        }
        if loaded {
            sections.push(self.packs_card(editable, cx));
            sections.push(self.board(editable, window, cx));
        } else if store_error.is_none() {
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
        let over = self.drop_over;
        bind_drop(
            div()
                .id("emoji-lib")
                .debug_selector(|| "emoji-lib".into())
                .track_focus(&self.focus)
                .relative()
                .flex()
                .flex_col()
                .gap_3(),
            DropHooks {
                over: |this: &Self| this.drop_over,
                set_over: |this, over, cx| {
                    if this.drop_over != over {
                        this.drop_over = over;
                        cx.notify();
                    }
                },
                drop: |this, paths, window: &mut Window, cx| {
                    if paths.is_empty() {
                        this.error = Some(NO_FILE_DRAGGED.into());
                        cx.notify();
                    } else {
                        this.import_paths(paths, window, cx);
                    }
                },
            },
            cx,
        )
        .children(sections)
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
        .children(self.import_modal(window, cx))
        .children(self.model_popover(cx))
        .children(self.filter_popover(cx))
    }
}
