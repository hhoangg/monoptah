//! Sorting emoji into emotions with the Mono's own model.
//!
//! Images are shrunk to small PNG thumbnails, sent in batches through one-off text calls,
//! and the JSON replies are parsed into a suggestion per item. When the model cannot read
//! images (or a call fails) the items are suggested from their file names instead.
//! The caller always lets the user review the suggestion; nothing here writes metadata.

use super::Format;
use crate::provider;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Images sent per model call.
pub const BATCH: usize = 16;
/// Thumbnails fit inside this many pixels on each side.
pub const THUMB_PX: u32 = 96;
const MAX_INPUT_BYTES: u64 = 10 * 1024 * 1024;

/// An emotion the user can sort into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmotionRef {
    pub name: String,
    pub description: String,
}

/// One emoji to sort. `key` is the caller's id, such as the stored file name.
#[derive(Clone, Debug)]
pub struct Item {
    pub key: String,
    pub name: String,
    pub path: PathBuf,
    pub format: Format,
}

/// Where the suggestions came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Model { label: String },
    Names { reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Debug)]
pub struct Sorted {
    /// Item key to emotion name; `None` means unsorted.
    pub suggestion: HashMap<String, Option<String>>,
    pub source: Source,
}

/// The seam between sorting and the model. Tests and the UI can supply their own.
pub trait Runner: Send + Sync {
    fn ask(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        cancel: Arc<AtomicBool>,
    ) -> Result<String, String>;
}

/// Runs each call as a one-off text request with images on the Mono's provider and model.
pub struct ProviderRunner {
    pub request: provider::Request,
}

impl Runner for ProviderRunner {
    fn ask(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        cancel: Arc<AtomicBool>,
    ) -> Result<String, String> {
        let mut request = self.request.clone();
        request.prompt = prompt;
        provider::run_text_with_images(request, images, cancel)
    }
}

/// Whether a provider has a working one-off text call. The model catalog has no per-model
/// image flag, so a failing call is the real test and falls back to file names.
pub fn supports_images(kind: provider::Kind) -> bool {
    crate::session_title::supports(kind)
}

/// Shrink `path` to fit `THUMB_PX` and write it as a PNG at `out`. Animated images give
/// their first frame.
pub fn thumbnail(path: &Path, out: &Path) -> Result<(), String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?
        .len();
    if size > MAX_INPUT_BYTES {
        return Err("Image is larger than 10 MB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let image = image::load_from_memory(&bytes).map_err(|e| format!("Cannot decode image: {e}"))?;
    let small = image.resize(THUMB_PX, THUMB_PX, image::imageops::FilterType::Triangle);
    small
        .save_with_format(out, image::ImageFormat::Png)
        .map_err(|e| format!("Cannot write thumbnail: {e}"))
}

/// The instruction for one batch. `labels` are in the order the images are attached.
pub fn prompt(emotions: &[EmotionRef], labels: &[String]) -> String {
    let mut text = String::from(
        "Look at each attached image. Each one is a small emoji. For every image choose the \
         single best matching emotion from this list, or \"unsorted\" when none fits:\n",
    );
    for emotion in emotions {
        text.push_str(&format!("- {}: {}\n", emotion.name, emotion.description));
    }
    text.push_str("\nThe images are attached in this order, with these labels:\n");
    for (i, label) in labels.iter().enumerate() {
        text.push_str(&format!("{}. {label}\n", i + 1));
    }
    let example = labels
        .iter()
        .map(|l| format!("\"{l}\": \"<emotion or unsorted>\""))
        .collect::<Vec<_>>()
        .join(", ");
    text.push_str(&format!(
        "\nReply with ONLY strict JSON, one key per label and no other text: {{{example}}}"
    ));
    text
}

/// Read the model's reply. Missing labels, unknown emotions, "unsorted" and non-string
/// values give `None`. Fails only when no JSON object can be parsed.
pub fn parse_reply(
    reply: &str,
    labels: &[String],
    emotions: &[EmotionRef],
) -> Result<HashMap<String, Option<String>>, String> {
    let object = first_object(reply).ok_or("The reply had no JSON object")?;
    Ok(labels
        .iter()
        .map(|label| {
            let emotion = object.get(label).and_then(|v| v.as_str()).and_then(|v| {
                let v = v.trim();
                emotions
                    .iter()
                    .find(|e| e.name.trim().eq_ignore_ascii_case(v))
                    .map(|e| e.name.clone())
            });
            (label.clone(), emotion)
        })
        .collect())
}

fn first_object(reply: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut from = 0;
    while let Some(offset) = reply[from..].find('{') {
        let start = from + offset;
        if let Some(Ok(serde_json::Value::Object(map))) =
            serde_json::Deserializer::from_str(&reply[start..])
                .into_iter::<serde_json::Value>()
                .next()
        {
            return Some(map);
        }
        from = start + 1;
    }
    None
}

const SYNONYMS: &[(&str, &[&str])] = &[
    ("happy", &["smile", "happy", "yay", "blush", "glad", "yea"]),
    (
        "celebrate",
        &["party", "triumph", "win", "tada", "rich", "celebrate"],
    ),
    ("laugh", &["lol", "lmao", "laugh", "haha", "kek"]),
    ("love", &["heart", "love", "hug", "kiss"]),
    (
        "sad",
        &["sad", "cry", "crying", "depressed", "tear", "hang", "sob"],
    ),
    ("angry", &["angry", "mad", "rage", "terminator", "wick"]),
    (
        "thinking",
        &["hmm", "think", "thinking", "genius", "ponder"],
    ),
    (
        "surprised",
        &["wow", "shock", "mindblown", "surprised", "omg"],
    ),
    (
        "approve",
        &["ok", "okay", "plusone", "yes", "thumbs", "point", "approve"],
    ),
    (
        "cool",
        &["cool", "cig", "smoking", "wine", "sunglasses", "musk"],
    ),
    (
        "clever",
        &["wizard", "magician", "clever", "smart", "brain"],
    ),
    ("nervous", &["fomo", "nervous", "sweat", "worry", "scared"]),
    (
        "chill",
        &["camp", "chill", "relax", "taco", "chai", "coffee", "sleep"],
    ),
];

/// Words of a display name: lowercase, split on non-alphanumerics, camel case and
/// letter/digit boundaries.
fn tokens(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        let split = match prev {
            None => false,
            Some(p) => {
                !c.is_alphanumeric()
                    || (p.is_lowercase() && c.is_uppercase())
                    || (p.is_alphanumeric() && p.is_alphabetic() != c.is_alphabetic())
            }
        };
        if split && !word.is_empty() {
            out.push(std::mem::take(&mut word).to_lowercase());
        }
        if c.is_alphanumeric() {
            word.push(c);
        }
        prev = Some(c);
    }
    if !word.is_empty() {
        out.push(word.to_lowercase());
    }
    out
}

/// Guess an emotion from a display name. The first emotion (in list order) that matches
/// wins: by its own name, a long word of its description, or a built-in synonym. A key
/// of three or more letters also matches inside a longer word ("pepeheart").
pub fn suggest_by_name(name: &str, emotions: &[EmotionRef]) -> Option<String> {
    let words = tokens(name);
    let joined: String = words.concat();
    let hits =
        |key: &str| words.iter().any(|w| w == key) || (key.len() >= 3 && joined.contains(key));
    for emotion in emotions {
        let key = emotion.name.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        if hits(&key) {
            return Some(emotion.name.clone());
        }
        let described = emotion
            .description
            .split(|c: char| !c.is_alphanumeric())
            .map(str::to_lowercase)
            .filter(|w| w.len() >= 4)
            .any(|w| words.contains(&w));
        if described {
            return Some(emotion.name.clone());
        }
        if let Some((_, keys)) = SYNONYMS.iter().find(|(n, _)| *n == key) {
            if keys.iter().any(|k| hits(k)) {
                return Some(emotion.name.clone());
            }
        }
    }
    None
}

fn names(items: &[Item], emotions: &[EmotionRef], reason: String) -> Sorted {
    Sorted {
        suggestion: items
            .iter()
            .map(|i| (i.key.clone(), suggest_by_name(&i.name, emotions)))
            .collect(),
        source: Source::Names { reason },
    }
}

/// Removes the scratch subdirectory when dropped, also on error.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Suggest an emotion for every item. A runner of `None`, or a failure of the first
/// batch, suggests from file names for all items. A later failed batch leaves its items
/// unsorted. Thumbnails are made in a fresh subfolder of `scratch`, removed at the end.
pub fn sort(
    items: &[Item],
    emotions: &[EmotionRef],
    runner: Option<&dyn Runner>,
    model_label: &str,
    scratch: &Path,
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(Progress),
) -> Result<Sorted, String> {
    let Some(runner) = runner else {
        return Ok(names(
            items,
            emotions,
            format!("{model_label} cannot sort by image"),
        ));
    };
    let total = items.len();
    let mut suggestion = HashMap::new();
    for (index, batch) in items.chunks(BATCH).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let dir = scratch.join(format!("sort-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create scratch folder: {e}"))?;
        let _guard = Scratch(dir.clone());
        let mut labels = Vec::new();
        let mut images = Vec::new();
        let mut placed: Vec<(&Item, Option<String>)> = Vec::new();
        for (n, item) in batch.iter().enumerate() {
            let label = format!("e{:02}.png", n + 1);
            let out = dir.join(&label);
            if thumbnail(&item.path, &out).is_ok() {
                labels.push(label.clone());
                images.push(out);
                placed.push((item, Some(label)));
            } else {
                placed.push((item, None));
            }
        }
        let mut parsed = HashMap::new();
        if !images.is_empty() {
            let reply = runner.ask(prompt(emotions, &labels), images, cancel.clone());
            match reply.and_then(|r| parse_reply(&r, &labels, emotions)) {
                Ok(map) => parsed = map,
                Err(_) if cancel.load(Ordering::Relaxed) => return Err("Cancelled".into()),
                Err(_) if index == 0 => {
                    return Ok(names(
                        items,
                        emotions,
                        format!(
                            "{model_label} could not sort by image, so emoji were suggested from their file names."
                        ),
                    ));
                }
                Err(_) => {}
            }
        }
        for (item, label) in placed {
            let emotion = label.and_then(|l| parsed.get(&l).cloned().flatten());
            suggestion.insert(item.key.clone(), emotion);
        }
        progress(Progress {
            done: ((index + 1) * BATCH).min(total),
            total,
        });
    }
    Ok(Sorted {
        suggestion,
        source: Source::Model {
            label: model_label.into(),
        },
    })
}

#[cfg(test)]
#[path = "sort_tests.rs"]
mod tests;
