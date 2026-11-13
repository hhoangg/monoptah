//! A Mono's Emoji settings page: the switch, how often, and which packs of the shared emoji
//! library this Mono may use. The library itself (packs, emotions, import) is managed in
//! Settings > Monos > Emoji; see `crate::emoji_library` and `docs/native/mono-emoji.md`.
//!
//! What a Mono keeps is `Mono::emoji` in the roster, so `configure_chat` and `save` carry it.
//! The library is shared: when it changes, or a Mono's switch or selection does, the chat is
//! given the library plus the Mono's selected pack ids again.
use super::panels::{card, setting, switch_setting, switch_track};
use super::*;
use crate::emoji_library::{EmojiEvent, SortModel, pack_counts, picture};
use crate::transcript::emoji::MonoEmoji;
use crate::{
    cloud_dialog_ui::alert,
    popover::{Choice, ChoiceEvent, Choices, Popover},
};
use gpui::{Div, Stateful};
use monocode_core::mono::emoji::{self, Frequency, Library, MonoEmoji as MonoSettings};

pub(super) const SUBTITLE: &str =
    "Let this Mono add an emoji from the packs you choose to a reply when it fits the mood.";
pub(super) const SWITCH_HELP: &str =
    "Off by default. Never in code, commit messages, PR text or reports.";
pub(super) const NEW_PACKS: &str = "New packs are not turned on for this Mono automatically.";
const SELECT_PRIORITY: usize = 90;

/// The page's own state; the library is in `Monos::emoji_store`.
pub(super) struct EmojiUi {
    select: Option<Choices<Frequency>>,
    frequency_anchor: AnchorSlot,
    /// The render key of what was last handed to each Mono's chat.
    pushed: HashMap<String, u64>,
}

impl EmojiUi {
    #[cfg(test)]
    pub(super) fn select_is_open(&self) -> bool {
        self.select.is_some()
    }

    pub(super) fn new() -> Self {
        Self {
            select: None,
            frequency_anchor: AnchorSlot::default(),
            pushed: HashMap::new(),
        }
    }
}

/// What the chat draws: whether it is on, the emotions, and where each picture of the selected
/// packs sits. A description, the frequency or a pack the Mono does not use is not part of it.
fn render_key(library: &Library, settings: &MonoSettings) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    settings.enabled.hash(&mut hasher);
    if settings.enabled {
        for emotion in &library.emotions {
            (&emotion.id, &emotion.name).hash(&mut hasher);
        }
        for pack in library.packs.iter().filter(|p| settings.uses(&p.id)) {
            pack.id.hash(&mut hasher);
            for emoji in &pack.emoji {
                (&emoji.file, &emoji.name, emoji.format, &emoji.emotion).hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

fn frequency_label(frequency: Frequency) -> &'static str {
    match frequency {
        Frequency::Rarely => "Rarely",
        Frequency::Sometimes => "Sometimes",
        Frequency::Often => "Often",
    }
}

impl Monos {
    /// Follows the shared library: pushes it to the chats, and drops a removed pack from every
    /// Mono's selection.
    pub(super) fn watch_emoji_library(&mut self, cx: &mut Context<Self>) {
        let store = self.emoji_store.clone();
        self.observations.push(cx.subscribe(
            &store,
            |this, _, event: &EmojiEvent, cx| match event {
                EmojiEvent::Changed => this.emoji_changed(cx),
                EmojiEvent::PackRemoved(pack) => this.emoji_pack_removed(pack, cx),
            },
        ));
        self.observations
            .push(cx.observe(&store, |_, _, cx| cx.notify()));
    }

    /// Remembers the model of the Mono whose settings are open, for the emoji import dialog.
    pub(super) fn note_settings_model(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.chat() else {
            return;
        };
        let chat = chat.read(cx);
        let model = SortModel {
            kind: Kind::from_id(&chat.summary.provider),
            model: chat.mono_model(),
            account: chat.settings.account.clone(),
        };
        self.emoji_store
            .update(cx, |store, _| store.remember_model(model));
    }

    /// A pack left the library: no Mono keeps its id. Ids that are gone are ignored at use time
    /// anyway; this keeps the roster tidy.
    fn emoji_pack_removed(&mut self, pack: &str, cx: &mut Context<Self>) {
        let mut touched = Vec::new();
        for mono in &mut self.roster.monos {
            if mono.emoji.uses(pack) {
                mono.emoji.set_pack(pack, false);
                touched.push(mono.id.clone());
            }
        }
        if touched.is_empty() {
            return;
        }
        self.save(cx);
        // The library's own change follows and is the one push the chats get.
        for id in touched {
            if self.chats.contains_key(&id) {
                self.configure_chat_with(&id, false, cx);
            }
        }
        cx.notify();
    }

    /// The library changed, or was loaded: hand it to the chats whose picture of it changed.
    /// A chat lays out every message again, so it is only told when something it draws changed.
    pub(super) fn emoji_changed(&mut self, cx: &mut Context<Self>) {
        let library = self.emoji_store.read(cx).library().clone();
        let stale: Vec<String> = self
            .roster
            .monos
            .iter()
            .filter(|mono| {
                Some(&render_key(&library, &mono.emoji)) != self.emoji_ui.pushed.get(&mono.id)
            })
            .map(|mono| mono.id.clone())
            .collect();
        for id in stale {
            self.push_emoji_to_views(&id, cx);
        }
        cx.notify();
    }

    /// The conversations that show this Mono's emoji: its chat, and the transcripts of the habit
    /// runs of this Mono that are open.
    pub(super) fn emoji_views(&self, id: &str) -> Vec<Entity<Conversation>> {
        self.chats
            .get(id)
            .cloned()
            .into_iter()
            .chain(
                self.running
                    .values()
                    .filter(|run| run.mono == id)
                    .map(|run| run.view.clone()),
            )
            .collect()
    }

    fn push_emoji_to_views(&mut self, id: &str, cx: &mut Context<Self>) {
        for view in self.emoji_views(id) {
            self.push_emoji(id, &view, cx);
        }
    }

    /// Gives a conversation that runs in this Mono's mode the shared library and the Mono's
    /// packs, or takes emoji away when the Mono's switch is off. Nothing is changed while the
    /// library is not loaded yet; the library's `Changed` follows the load.
    pub(super) fn push_emoji(
        &mut self,
        mono_id: &str,
        chat: &Entity<Conversation>,
        cx: &mut Context<Self>,
    ) {
        let Some(mono) = self.roster.get(mono_id) else {
            return;
        };
        let settings = mono.emoji.clone();
        let (library, dir, loaded) = {
            let store = self.emoji_store.read(cx);
            (
                store.library().clone(),
                store.dir().to_path_buf(),
                store.loaded(),
            )
        };
        if settings.enabled && !loaded {
            return;
        }
        self.emoji_ui
            .pushed
            .insert(mono_id.into(), render_key(&library, &settings));
        let emoji = settings
            .enabled
            .then(|| MonoEmoji::new(library, dir, settings.packs));
        chat.update(cx, |chat, cx| chat.set_mono_emoji(emoji, cx));
    }

    /// Changes the open Mono's emoji settings, saves the roster and tells its chat.
    pub(super) fn emoji_update(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut MonoSettings),
    ) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let Some(mono) = self.roster.get_mut(&id) else {
            return;
        };
        change(&mut mono.emoji);
        self.save(cx);
        // The chat and the open habit runs all get the push, so the shared key stays true.
        self.configure_chat_with(&id, false, cx);
        self.push_emoji_to_views(&id, cx);
        cx.notify();
    }

    /// "Manage emoji library...": Settings > Monos > Emoji. The Mono's settings close so the
    /// library is not hidden behind them.
    fn emoji_manage(&mut self, cx: &mut Context<Self>) {
        self.save_open_file(cx);
        self.settings = None;
        cx.emit(ShowEmojiLibrary);
        cx.notify();
    }

    fn emoji_open_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.emoji_ui.select.take() {
            open.close(window, cx);
            cx.notify();
            return;
        }
        let Some(frequency) = self.current().map(|m| m.emoji.frequency) else {
            return;
        };
        let choices = [Frequency::Rarely, Frequency::Sometimes, Frequency::Often]
            .into_iter()
            .map(|f| Choice {
                value: f,
                label: frequency_label(f).into(),
                checked: frequency == f,
            })
            .collect::<Vec<_>>();
        let width = self
            .emoji_ui
            .frequency_anchor
            .get()
            .map_or(200., |b| b.size.width.as_f32().max(160.));
        self.emoji_ui.select = Some(Choices::open(
            "How often",
            choices,
            width,
            window,
            cx,
            |this, event, window, cx| {
                if let Some(open) = this.emoji_ui.select.take() {
                    open.close(window, cx);
                }
                if let ChoiceEvent::Picked(frequency) = event {
                    this.emoji_update(cx, |e| e.frequency = frequency);
                }
                cx.notify();
            },
        ));
        cx.notify();
    }

    pub(super) fn emoji_select_popover(&self, cx: &App) -> Option<Popover> {
        let open = self.emoji_ui.select.as_ref()?;
        Some(open.popover(&self.emoji_ui.frequency_anchor, SELECT_PRIORITY, cx))
    }

    fn emoji_select_trigger(&self, text: String, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("mono-emoji-frequency")
            .debug_selector(|| "mono-emoji-frequency".into())
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
            .cursor_pointer()
            .hover(|s| s.bg(content(0.06)))
            .on_click(cx.listener(|this, _, window, cx| this.emoji_open_select(window, cx)))
            .child(self.emoji_ui.frequency_anchor.probe())
            .child(div().min_w_0().truncate().child(text))
            .child(icon("chevron-down", 12.).text_color(content(0.5)))
    }

    pub(super) fn emoji_page(
        &mut self,
        mono: &Mono,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let settings = mono.emoji.clone();
        let (library, dir, loaded, error) = {
            let store = self.emoji_store.read(cx);
            (
                store.library().clone(),
                store.dir().to_path_buf(),
                store.loaded(),
                store.error().map(str::to_owned),
            )
        };
        let mut sections: Vec<AnyElement> = vec![
            div()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(content(0.55))
                .child(SUBTITLE)
                .into_any_element(),
        ];
        if let Some(error) = error {
            sections.push(alert("mono-emoji-error", error).into_any_element());
        }
        sections.push(self.emoji_settings_card(&settings, cx));
        if loaded {
            sections.push(self.emoji_packs_card(&settings, &library, &dir, cx));
        } else {
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
        div()
            .id("mono-emoji-page")
            .debug_selector(|| "mono-emoji-page".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(Self::settings_page_header("Emoji"))
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
            .into_any_element()
    }

    fn emoji_settings_card(
        &mut self,
        settings: &MonoSettings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle = switch_track(
            "mono-emoji-enabled",
            "Use emoji in replies",
            settings.enabled,
        )
        .debug_selector(|| "mono-emoji-enabled".into())
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| {
            this.emoji_update(cx, |e| e.enabled = !e.enabled);
        }));
        let frequency = self.emoji_select_trigger(frequency_label(settings.frequency).into(), cx);
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
        settings: &MonoSettings,
        library: &Library,
        dir: &std::path::Path,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chosen = library
            .packs
            .iter()
            .filter(|p| settings.uses(&p.id))
            .count();
        let summary = if library.packs.is_empty() {
            "No packs yet".to_string()
        } else {
            format!(
                "{chosen} of {} packs used by this Mono",
                library.packs.len()
            )
        };
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
                crate::modal::ghost_button("mono-emoji-manage", "Manage emoji library...")
                    .debug_selector(|| "mono-emoji-manage".into())
                    .border_1()
                    .border_color(content(0.1))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.emoji_manage(cx))),
            );
        let rows: Vec<AnyElement> = library
            .packs
            .iter()
            .map(|pack| self.emoji_pack_row(settings, dir, pack, cx))
            .collect();
        let body: AnyElement = if rows.is_empty() {
            div()
                .id("mono-emoji-empty")
                .debug_selector(|| "mono-emoji-empty".into())
                .px_4()
                .py_5()
                .border_t_1()
                .border_color(content(0.05))
                .text_center()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(content(0.55))
                .child("The emoji library has no packs yet. Import emoji packs there, then choose here which ones this Mono may use.")
                .into_any_element()
        } else {
            div().flex().flex_col().children(rows).into_any_element()
        };
        card()
            .child(header)
            .child(body)
            .child(
                div()
                    .id("mono-emoji-new-packs")
                    .debug_selector(|| "mono-emoji-new-packs".into())
                    .px_4()
                    .py_2p5()
                    .border_t_1()
                    .border_color(content(0.05))
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(NEW_PACKS),
            )
            .into_any_element()
    }

    fn emoji_pack_row(
        &self,
        settings: &MonoSettings,
        dir: &std::path::Path,
        pack: &emoji::Pack,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pid = pack.id.clone();
        let on = settings.uses(&pid);
        let thumbs = pack
            .emoji
            .iter()
            .take(5)
            .map(|e| picture(&emoji::image_path(dir, pack, e), 22., None));
        let toggle = switch_track(
            SharedString::from(format!("mono-emoji-use-{pid}")),
            format!("Use {}", pack.name),
            on,
        )
        .debug_selector({
            let pid = pid.clone();
            move || format!("mono-emoji-use-{pid}")
        })
        .cursor_pointer()
        .on_click({
            let pid = pid.clone();
            cx.listener(move |this, _, _, cx| {
                let pid = pid.clone();
                this.emoji_update(cx, move |e| {
                    let now = e.uses(&pid);
                    e.set_pack(&pid, !now);
                });
            })
        });
        div()
            .id(SharedString::from(format!("mono-emoji-pack-{pid}")))
            .debug_selector(move || format!("mono-emoji-pack-{pid}"))
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
            .child(toggle)
            .into_any_element()
    }
}
