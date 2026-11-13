//! Per-Mono picture emoji: the packs a user imported, the emotions they are sorted into,
//! and the settings that let a Mono use them in replies. See `docs/native/mono-emoji.md`.
//!
//! Layout, inside the Mono's folder:
//! `emoji/emoji.json` (metadata), `emoji/packs/<pack id>/<hash>.<ext>` (images) and
//! `emoji/.staging-<uuid>/` (an import that is not confirmed yet).

pub mod import;
pub mod sort;
pub mod token;

use serde::{Deserialize, Serialize};

/// An image format an emoji may have. Detected from magic bytes, never from the extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl Format {
    /// The label shown to the user, such as "PNG".
    pub fn label(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPG",
            Format::Gif => "GIF",
            Format::Webp => "WebP",
        }
    }

    /// The extension used for the stored file.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Gif => "gif",
            Format::Webp => "webp",
        }
    }
}
