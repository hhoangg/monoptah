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
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

#[cfg(test)]
mod tests;

/// The `emoji.json` version this build reads and writes.
pub const VERSION: u32 = 1;
const FILE: &str = "emoji.json";
const MAX_EMOTION_NAME: usize = 32;

/// The default emotions: name and description. The id of a default emotion is its name.
pub const DEFAULT_EMOTIONS: [(&str, &str); 13] = [
    ("happy", "Glad, pleased, things went well"),
    ("celebrate", "A task finished, something shipped, a win"),
    ("laugh", "Something funny, a light joke"),
    ("love", "Thanks, warmth, appreciation"),
    ("sad", "Bad news, a failure, something lost"),
    ("angry", "Frustration with a bug or a broken tool"),
    ("thinking", "Investigating, weighing options"),
    ("surprised", "Unexpected result, wow"),
    ("approve", "Agree, ok, looks good"),
    ("cool", "Confident, smooth, under control"),
    ("clever", "A neat trick or a smart fix"),
    ("nervous", "Risky change, worry, deadline"),
    ("chill", "Relaxed, nothing urgent"),
];

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

/// How often a Mono may add an emoji.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Frequency {
    Rarely,
    #[default]
    Sometimes,
    Often,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Emotion {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Emoji {
    /// The stored file name inside the pack folder, `<hash>.<ext>`.
    pub file: String,
    /// Full SHA-256 of the image bytes, as hex.
    pub hash: String,
    pub name: String,
    pub format: Format,
    /// An emotion id, or none for Unsorted.
    pub emotion: Option<String>,
}

impl Default for Emoji {
    fn default() -> Self {
        Self {
            file: String::new(),
            hash: String::new(),
            name: String::new(),
            format: Format::Png,
            emotion: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// The model label used for the sorting, or none when imported without sorting.
    pub sorted_by: Option<String>,
    pub emoji: Vec<Emoji>,
}

impl Default for Pack {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            sorted_by: None,
            emoji: Vec::new(),
        }
    }
}

/// The content of `emoji.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Library {
    pub version: u32,
    pub enabled: bool,
    pub frequency: Frequency,
    pub emotions: Vec<Emotion>,
    pub packs: Vec<Pack>,
}

fn default_emotions() -> Vec<Emotion> {
    DEFAULT_EMOTIONS
        .iter()
        .map(|(name, description)| Emotion {
            id: (*name).to_owned(),
            name: (*name).to_owned(),
            description: (*description).to_owned(),
        })
        .collect()
}

impl Default for Library {
    fn default() -> Self {
        Self {
            version: VERSION,
            enabled: false,
            frequency: Frequency::Sometimes,
            emotions: default_emotions(),
            packs: Vec::new(),
        }
    }
}

/// A library read from disk. A `read_only` one came from a newer version of the app and
/// must not be written back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub library: Library,
    pub read_only: bool,
}

/// The Mono's emoji folder. `root` is `mono::root(store)`. Nothing here is reachable
/// through `files::resolve_file`, so the Mono's own CLI cannot read these files.
pub fn dir(root: &Path, mono_id: &str) -> Result<PathBuf, String> {
    if !super::files::valid_id(mono_id) {
        return Err("Not a Mono id".into());
    }
    Ok(root.join(mono_id).join("emoji"))
}

/// The last path component only, so a hand-edited name cannot climb out of its folder.
fn component(name: &str) -> &std::ffi::OsStr {
    Path::new(name).file_name().unwrap_or_default()
}

/// The folder of one pack: `<dir>/packs/<pack id>`.
pub fn pack_dir(dir: &Path, pack_id: &str) -> PathBuf {
    dir.join("packs").join(component(pack_id))
}

/// The stored image of an emoji.
pub fn image_path(dir: &Path, pack: &Pack, emoji: &Emoji) -> PathBuf {
    pack_dir(dir, &pack.id).join(component(&emoji.file))
}

/// Remove every `.staging-*` folder: an import that was not confirmed.
pub fn clean_staging(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let staging = entry.file_name().to_string_lossy().starts_with(".staging-");
        if staging && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn disk_version(text: &str) -> Result<u32, String> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("{FILE} is not readable: {e}"))?;
    Ok(value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .map_or(VERSION, |v| u32::try_from(v).unwrap_or(u32::MAX)))
}

/// Read the library. A missing file gives the default. A file that cannot be parsed is an
/// error and is left as it is. A newer version comes back read-only.
pub fn load(root: &Path, mono_id: &str) -> Result<Loaded, String> {
    let dir = dir(root, mono_id)?;
    clean_staging(&dir);
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Loaded {
                library: Library::default(),
                read_only: false,
            });
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if disk_version(&text)? > VERSION {
        // A newer app wrote it: show what we understand, never write it back.
        return Ok(Loaded {
            library: serde_json::from_str(&text).unwrap_or_default(),
            read_only: true,
        });
    }
    let library =
        serde_json::from_str(&text).map_err(|e| format!("{FILE} is not readable: {e}"))?;
    Ok(Loaded {
        library,
        read_only: false,
    })
}

/// Write the library atomically. Refuses when the file on disk is newer or unreadable.
pub fn save(root: &Path, mono_id: &str, library: &Library) -> Result<(), String> {
    let dir = dir(root, mono_id)?;
    let path = dir.join(FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            if disk_version(&text)? > VERSION {
                return Err("The emoji settings come from a newer version of the app".into());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut library = library.clone();
    library.version = VERSION;
    let text = serde_json::to_string_pretty(&library).map_err(|e| e.to_string())?;
    super::files::write_atomic(&path, &text)
}

fn short_id(prefix: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}", &id[..8])
}

fn unique_name(wanted: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(wanted) {
        return wanted.to_owned();
    }
    (2..)
        .map(|n| format!("{wanted} {n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| wanted.to_owned())
}

impl Library {
    fn emotion_index(&self, id: &str) -> Option<usize> {
        self.emotions.iter().position(|e| e.id == id)
    }

    /// The id of the emotion called `name`, compared case-insensitively.
    pub fn emotion_id(&self, name: &str) -> Option<&str> {
        let name = name.trim().to_lowercase();
        self.emotions
            .iter()
            .find(|e| e.name.to_lowercase() == name)
            .map(|e| e.id.as_str())
    }

    /// Add an emotion called "new emotion" (made unique with a number); returns its id.
    pub fn add_emotion(&mut self) -> String {
        let name = unique_name("new emotion", |name| self.emotion_id(name).is_some());
        let id = short_id("emo");
        self.emotions.push(Emotion {
            id: id.clone(),
            name,
            description: String::new(),
        });
        id
    }

    pub fn rename_emotion(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("An emotion needs a name".into());
        }
        if name.chars().count() > MAX_EMOTION_NAME {
            return Err(format!(
                "An emotion name has at most {MAX_EMOTION_NAME} characters"
            ));
        }
        if name.contains(['[', ']', '\n', '\r']) {
            return Err("An emotion name cannot contain [ ] or a line break".into());
        }
        let index = self.emotion_index(id).ok_or("No such emotion")?;
        let lowered = name.to_lowercase();
        let clash = self
            .emotions
            .iter()
            .enumerate()
            .any(|(i, e)| i != index && e.name.to_lowercase() == lowered);
        if clash {
            return Err("Another emotion has this name".into());
        }
        self.emotions[index].name = name.to_owned();
        Ok(())
    }

    pub fn describe_emotion(&mut self, id: &str, text: &str) {
        if let Some(index) = self.emotion_index(id) {
            self.emotions[index].description = text.trim().to_owned();
        }
    }

    /// Remove an emotion. Its emoji become Unsorted.
    pub fn delete_emotion(&mut self, id: &str) {
        self.emotions.retain(|e| e.id != id);
        for emoji in self.packs.iter_mut().flat_map(|p| p.emoji.iter_mut()) {
            if emoji.emotion.as_deref() == Some(id) {
                emoji.emotion = None;
            }
        }
    }

    /// Move one emoji to an emotion, or to Unsorted with `None`. An unknown emotion id
    /// counts as Unsorted.
    pub fn move_emoji(&mut self, pack_id: &str, file: &str, emotion: Option<&str>) {
        let emotion = emotion
            .filter(|id| self.emotion_index(id).is_some())
            .map(str::to_owned);
        if let Some(emoji) = self
            .packs
            .iter_mut()
            .find(|p| p.id == pack_id)
            .and_then(|p| p.emoji.iter_mut().find(|e| e.file == file))
        {
            emoji.emotion = emotion;
        }
    }

    pub fn set_pack_enabled(&mut self, pack_id: &str, enabled: bool) {
        if let Some(pack) = self.packs.iter_mut().find(|p| p.id == pack_id) {
            pack.enabled = enabled;
        }
    }

    /// Drop a pack from the library and delete its folder (only inside `<dir>/packs/`).
    pub fn remove_pack(&mut self, dir: &Path, pack_id: &str) -> Result<(), String> {
        if !super::files::valid_id(pack_id) {
            return Err("Not a pack id".into());
        }
        let index = self
            .packs
            .iter()
            .position(|p| p.id == pack_id)
            .ok_or("No such pack")?;
        let folder = pack_dir(dir, pack_id);
        match std::fs::remove_dir_all(&folder) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", folder.display())),
        }
        self.packs.remove(index);
        Ok(())
    }

    /// Restore the default emotions. An emoji keeps its emotion when the name of its
    /// current emotion is one of the defaults; the rest become Unsorted.
    pub fn reset_emotions(&mut self) {
        let names: HashMap<String, String> = self
            .emotions
            .iter()
            .map(|e| (e.id.clone(), e.name.to_lowercase()))
            .collect();
        let defaults = default_emotions();
        for emoji in self.packs.iter_mut().flat_map(|p| p.emoji.iter_mut()) {
            let name = emoji.emotion.as_ref().and_then(|id| names.get(id));
            emoji.emotion = name
                .and_then(|name| defaults.iter().find(|d| d.name == *name))
                .map(|d| d.id.clone());
        }
        self.emotions = defaults;
    }

    /// Add a pack (enabled) and return its new id. The display name is made unique.
    pub fn add_pack(&mut self, name: &str, sorted_by: Option<String>, emoji: Vec<Emoji>) -> String {
        let wanted = match name.trim() {
            "" => "Emoji pack",
            trimmed => trimmed,
        };
        let name = unique_name(wanted, |candidate| {
            let lowered = candidate.to_lowercase();
            self.packs.iter().any(|p| p.name.to_lowercase() == lowered)
        });
        let id = loop {
            let id = short_id("pack");
            if self.packs.iter().all(|p| p.id != id) {
                break id;
            }
        };
        self.packs.push(Pack {
            id: id.clone(),
            name,
            enabled: true,
            sorted_by,
            emoji,
        });
        id
    }

    /// Set the emotion of the emoji named in `map` (file to emotion name). Names match
    /// case-insensitively; an unknown name or `None` means Unsorted. Files that are not in
    /// the map keep their emotion.
    pub fn apply_sorting(
        &mut self,
        pack_id: &str,
        map: &HashMap<String, Option<String>>,
        sorted_by: Option<String>,
    ) {
        let resolved: HashMap<&String, Option<String>> = map
            .iter()
            .map(|(file, name)| {
                let id = name
                    .as_deref()
                    .and_then(|name| self.emotion_id(name))
                    .map(str::to_owned);
                (file, id)
            })
            .collect();
        let Some(pack) = self.packs.iter_mut().find(|p| p.id == pack_id) else {
            return;
        };
        for emoji in &mut pack.emoji {
            if let Some(id) = resolved.get(&emoji.file) {
                emoji.emotion = id.clone();
            }
        }
        pack.sorted_by = sorted_by;
    }

    /// The content hashes of every emoji in every pack, for de-duplicating an import.
    pub fn known_hashes(&self) -> HashSet<String> {
        self.packs
            .iter()
            .flat_map(|p| p.emoji.iter().map(|e| e.hash.clone()))
            .collect()
    }

    /// Emotions with at least one emoji in an enabled pack, in order.
    pub fn usable_emotions(&self) -> Vec<&Emotion> {
        self.emotions
            .iter()
            .filter(|emotion| {
                self.packs.iter().filter(|p| p.enabled).any(|p| {
                    p.emoji
                        .iter()
                        .any(|e| e.emotion.as_deref() == Some(&emotion.id))
                })
            })
            .collect()
    }

    /// The emoji of the emotion called `emotion_name` in enabled packs, ordered by pack id
    /// and then file so a pick is stable.
    pub fn candidates(&self, emotion_name: &str) -> Vec<(&Pack, &Emoji)> {
        let Some(id) = self.emotion_id(emotion_name) else {
            return Vec::new();
        };
        let mut found: Vec<(&Pack, &Emoji)> = self
            .packs
            .iter()
            .filter(|p| p.enabled)
            .flat_map(|p| {
                p.emoji
                    .iter()
                    .filter(|e| e.emotion.as_deref() == Some(id))
                    .map(move |e| (p, e))
            })
            .collect();
        found.sort_by(|a, b| (&a.0.id, &a.1.file).cmp(&(&b.0.id, &b.1.file)));
        found
    }
}

/// How many emoji of each format a pack holds, in a fixed order, skipping empty formats.
pub fn format_counts(pack: &Pack) -> Vec<(Format, usize)> {
    [Format::Png, Format::Gif, Format::Jpeg, Format::Webp]
        .into_iter()
        .filter_map(|format| {
            let count = pack.emoji.iter().filter(|e| e.format == format).count();
            (count > 0).then_some((format, count))
        })
        .collect()
}

/// The `<emoji>` block of the Mono context, or none when the feature is off or no emotion
/// has a picture to show.
pub fn context_block(library: &Library) -> Option<String> {
    if !library.enabled {
        return None;
    }
    let usable = library.usable_emotions();
    if usable.is_empty() {
        return None;
    }
    let list = usable
        .iter()
        .map(|e| match e.description.trim() {
            "" => format!("- {}", e.name),
            description => format!("- {}: {description}", e.name),
        })
        .collect::<Vec<_>>()
        .join("\n");
    let frequency = match library.frequency {
        Frequency::Rarely => {
            "Rarely: at most one emoji, in about one reply in five, only for a clear mood."
        }
        Frequency::Sometimes => {
            "Sometimes: at most one emoji per reply, in about one reply in three."
        }
        Frequency::Often => "Often: one emoji in most conversational replies, never more than two.",
    };
    Some(format!(
        "<emoji>\nYou may add a picture emoji to a conversational reply when it fits the mood. Write `:emo[<emotion>]` inline or on its own line; the app shows a picture. Emotions you can use:\n{list}\n\n{frequency}\nNever inside code, code blocks, commands, commit messages, PR or issue text, reports, files you write or tool input.\n</emoji>"
    ))
}
