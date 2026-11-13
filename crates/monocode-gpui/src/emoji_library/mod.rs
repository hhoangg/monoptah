//! The one emoji library of the app, shown under Settings > Monos > Emoji. Every Mono picks
//! the packs it may use from it in its own settings. See `docs/native/mono-emoji.md`.
//!
//! - `store`: the library itself, its debounced serialized writer and its events.
//! - `view`: the packs card and the page-level plumbing (drop, selects, prompts).
//! - `board`: the board of emotions.
//! - `import`: the "Import emoji packs" dialog with its model picker.
use crate::theme::*;
use gpui::{AnyElement, Div, ObjectFit, SharedString, div, prelude::*, px};
use monocode_core::mono::emoji::{self, Format, Pack};
use std::path::Path;

mod board;
mod import;
mod store;
mod view;

pub(crate) use store::{EmojiEvent, EmojiStore, SortModel, get, install};
pub(crate) use view::EmojiLibraryView;

#[cfg(test)]
pub(crate) use import::{ItemState, SortParts, dialog_texts, names_notice};
#[cfg(test)]
pub(crate) use store::SAVE_DELAY;

/// An emoji image, `side` points square. A GIF shows its first frame; with an `animate` id it
/// plays, and then GPUI redraws the whole view on every frame, so only a hovered chip gets one.
pub(crate) fn picture(path: &Path, side: f32, animate: Option<SharedString>) -> AnyElement {
    let image = crate::image_memory::thumbnail(path.to_path_buf())
        .object_fit(ObjectFit::Contain)
        .size(px(side));
    match animate {
        Some(id) => image.id(id).into_any_element(),
        None => image.into_any_element(),
    }
}

/// "29 PNG, 3 GIF" for a list of formats, in a fixed order.
fn format_list(formats: impl IntoIterator<Item = Format>) -> Vec<(Format, usize)> {
    let formats: Vec<Format> = formats.into_iter().collect();
    [Format::Png, Format::Gif, Format::Jpeg, Format::Webp]
        .into_iter()
        .filter_map(|format| {
            let count = formats.iter().filter(|f| **f == format).count();
            (count > 0).then_some((format, count))
        })
        .collect()
}

fn format_text(counts: &[(Format, usize)]) -> String {
    counts
        .iter()
        .map(|(format, count)| format!("{count} {}", format.label()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// "32 emoji (29 PNG, 3 GIF)".
pub(crate) fn pack_counts(pack: &Pack) -> String {
    let counts = emoji::format_counts(pack);
    if counts.is_empty() {
        "0 emoji".into()
    } else {
        format!("{} emoji ({})", pack.emoji.len(), format_text(&counts))
    }
}

/// A rounded, bordered card.
pub(crate) fn card() -> Div {
    div()
        .rounded_xl()
        .border_1()
        .border_color(content(0.04))
        .overflow_hidden()
        .bg(content(0.02))
        .flex()
        .flex_col()
}
