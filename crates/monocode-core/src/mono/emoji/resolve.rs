//! Which picture a `:emo[<emotion>]` token shows, in one place: the native transcript and the
//! picks sent to phones and the web both ask here, so they always agree.

use super::{image_path, token, Emoji, Format, Library, Pack};
use std::path::Path;

/// The most pictures sent with one message.
pub const MAX_PICKS: usize = 64;

/// The emoji a token shows: its candidate in the `selected` packs, chosen by a seed of the
/// session, the raw message source before the token (`raw[..start]`) and the token's index
/// among the tokens outside code. Also the emotion's display name. None when the name is
/// empty or unknown, or the emotion has no picture there. The file is not checked.
pub fn pick_token<'a>(
    library: &'a Library,
    selected: &[String],
    session: &str,
    raw: &str,
    start: usize,
    index: usize,
    name: &str,
) -> Option<(&'a Pack, &'a Emoji, String)> {
    let seed = token::seed_for(session, &raw[..start.min(raw.len())], index);
    let (pack, emoji) = token::pick(library, selected, name, seed)?;
    let emotion = library
        .emotions
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .map_or_else(|| name.to_owned(), |e| e.name.clone());
    Some((pack, emoji, emotion))
}

/// The emoji with content hash `hash` in one of the `selected` packs, if any.
pub fn find_by_hash<'a>(
    library: &'a Library,
    selected: &[String],
    hash: &str,
) -> Option<(&'a Pack, &'a Emoji)> {
    library
        .packs
        .iter()
        .filter(|pack| selected.contains(&pack.id))
        .find_map(|pack| {
            pack.emoji
                .iter()
                .find(|emoji| emoji.hash == hash)
                .map(|emoji| (pack, emoji))
        })
}

/// One resolved picture of a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// Byte offset in the stripped text where the picture goes.
    pub at: usize,
    pub emotion: String,
    pub name: String,
    pub hash: String,
    pub format: Format,
    /// Drawn large (alone on its line) rather than inline.
    pub large: bool,
}

/// The stripped text of `raw` (no tokens) and the pictures its tokens resolve to, as native
/// shows them. `raw` is the message source as native lays it out: with a token still
/// streaming in already cut off. `streaming` says the message is still being written. A
/// token with no picture (unknown or empty emotion, none in the selected packs, the file
/// gone, as `exists` says) has no entry. `exists` is the caller's cached file check, so
/// laying out a message makes no per-token filesystem call. At most [`MAX_PICKS`].
pub fn resolve(
    library: &Library,
    dir: &Path,
    selected: &[String],
    session: &str,
    raw: &str,
    streaming: bool,
    exists: &dyn Fn(&Path) -> bool,
) -> (String, Vec<Resolved>) {
    let (text, marks) = token::strip_with_marks(raw);
    if marks.is_empty() {
        return (text, Vec::new());
    }
    let tokens = token::outside_code(raw);
    let large = token::large_flags(raw, streaming);
    let mut picks = Vec::new();
    for (index, found) in tokens.iter().enumerate() {
        if picks.len() >= MAX_PICKS {
            break;
        }
        let Some((pack, emoji, emotion)) = pick_token(
            library,
            selected,
            session,
            raw,
            found.range.start,
            index,
            &found.name,
        ) else {
            continue;
        };
        // A file that is gone draws nothing.
        if !exists(&image_path(dir, pack, emoji)) {
            continue;
        }
        picks.push(Resolved {
            at: marks.get(index).copied().unwrap_or(text.len()),
            emotion,
            name: emoji.name.clone(),
            hash: emoji.hash.clone(),
            format: emoji.format,
            large: large.get(index).copied().unwrap_or(false),
        });
    }
    (text, picks)
}
