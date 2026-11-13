//! Import of an emoji pack from an untrusted zip file. See "Zip import security" in
//! `docs/native/mono-emoji.md`.
//!
//! An entry path is never used as a filesystem destination. Every kept file is written as
//! `<first 16 hex of sha256>.<ext>` directly inside a fresh `.staging-<uuid>` folder.

use super::Format;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_zip_bytes: u64,
    pub max_entries: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_dimension: u32,
    pub max_frames: usize,
    /// Upper bound for `width * height * frames` of one image, with the logical screen or
    /// canvas size. GPUI decodes every frame of a GIF or animated WebP at that size and keeps
    /// all of them as RGBA, so this is the memory the display needs divided by four.
    pub max_decoded_pixels: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_zip_bytes: 200 * 1024 * 1024,
            max_entries: 500,
            max_file_bytes: 10 * 1024 * 1024,
            max_total_bytes: 200 * 1024 * 1024,
            max_dimension: 1024,
            max_frames: 300,
            // About 64 MB of RGBA.
            max_decoded_pixels: 16_777_216,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    UnsafePath,
    Symlink,
    Hidden,
    MacMetadata,
    NestedArchive,
    Encrypted,
    NotImage,
    TooLarge,
    BadImage,
    TooManyPixels,
    TooManyFrames,
    Duplicate,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            SkipReason::UnsafePath => "unsafe path",
            SkipReason::Symlink => "symbolic link",
            SkipReason::Hidden => "hidden file",
            SkipReason::MacMetadata => "macOS metadata",
            SkipReason::NestedArchive => "nested archive",
            SkipReason::Encrypted => "encrypted",
            SkipReason::NotImage => "not a PNG, JPG, GIF or WebP image",
            SkipReason::TooLarge => "file too large",
            SkipReason::BadImage => "damaged image",
            SkipReason::TooManyPixels => "image too large",
            SkipReason::TooManyFrames => "too many frames",
            SkipReason::Duplicate => "duplicate",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub entry: String,
    pub reason: SkipReason,
}

#[derive(Clone, Debug)]
pub struct StagedImage {
    pub hash: String,
    pub file: String,
    pub name: String,
    pub format: Format,
    pub animated: bool,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct Staged {
    pub dir: PathBuf,
    pub source_name: String,
    pub entries: usize,
    pub images: Vec<StagedImage>,
    pub skipped: Vec<Skipped>,
}

const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "jar", "cbz",
];

/// Detects the format from magic bytes only.
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::Webp)
    } else {
        None
    }
}

fn is_archive_magic(b: &[u8]) -> bool {
    b.starts_with(b"PK\x03\x04")
        || b.starts_with(b"PK\x05\x06")
        || b.starts_with(b"Rar!\x1a\x07")
        || b.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C])
        || b.starts_with(&[0x1F, 0x8B])
        || (b.len() >= 262 && &b[257..262] == b"ustar")
        || b.starts_with(b"BZh")
        || b.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00])
        || b.starts_with(&[0x28, 0xB5, 0x2F, 0xFD])
}

/// True for Unicode format characters (general category Cf) that can reorder or hide text:
/// soft hyphen, bidi controls, zero-width characters, joiners, word joiner, BOM, interlinear
/// annotation, invisible operators and the tag block.
fn is_format_char(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// Composes the text (NFC) and drops format characters and all combining marks. Dropping
/// every mark (after composing, so common accented letters survive) is simpler and safer than
/// limiting runs: it also stops stacked "zalgo" text.
fn strip_invisible(text: &str) -> String {
    text.nfc()
        .filter(|c| !is_format_char(*c) && !is_combining_mark(*c))
        .collect()
}

/// A display name for an entry or a zip file: last path component, extension removed,
/// numeric emoji.gg prefix removed, only letters, digits, space, `-`, `_` and `.`, at most
/// 64 chars. Falls back to "emoji".
pub fn sanitize_name(entry_name: &str) -> String {
    let clean = strip_invisible(entry_name);
    let last = clean.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = match last.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => last,
    };
    let digits = stem.chars().take_while(|c| c.is_ascii_digit()).count();
    let stem = if digits > 0 && stem[digits..].starts_with('-') && stem.len() > digits + 1 {
        &stem[digits + 1..]
    } else {
        stem
    };
    let kept: String = stem
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    let collapsed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(64).collect();
    let out = truncated.trim().to_string();
    if out.is_empty() {
        "emoji".to_string()
    } else {
        out
    }
}

fn source_name_of(zip_path: &Path) -> String {
    let file = zip_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = sanitize_name(&file);
    let stripped = name
        .strip_suffix("-emojigg-pack")
        .map(str::trim)
        .unwrap_or(&name);
    if stripped.is_empty() {
        "emoji".to_string()
    } else {
        stripped.to_string()
    }
}

fn parts(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.split(|b| *b == b'/' || *b == b'\\')
}

/// True when the raw entry name could escape a folder or name an absolute location.
fn is_unsafe_path(raw: &[u8]) -> bool {
    if raw.contains(&0) {
        return true;
    }
    if matches!(raw.first(), Some(b'/') | Some(b'\\')) {
        return true;
    }
    if raw.len() >= 2 && raw[0].is_ascii_alphabetic() && raw[1] == b':' {
        return true;
    }
    parts(raw).any(|part| part == b"..")
}

fn has_hidden_component(raw: &[u8]) -> bool {
    parts(raw).any(|part| part.first() == Some(&b'.'))
}

fn has_macosx_component(raw: &[u8]) -> bool {
    parts(raw).any(|part| part == b"__MACOSX")
}

fn has_archive_extension(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => ARCHIVE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

struct Inspected {
    width: u32,
    height: u32,
    animated: bool,
}

/// `width * height * frames` must stay within the decoded pixel budget (checked arithmetic).
fn check_budget(width: u32, height: u32, frames: usize, limits: &Limits) -> Result<(), SkipReason> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|p| p.checked_mul(frames as u64))
        .ok_or(SkipReason::TooManyPixels)?;
    if pixels > limits.max_decoded_pixels {
        return Err(SkipReason::TooManyPixels);
    }
    Ok(())
}

/// Skips the data sub-blocks that start at `pos`, returns the position after the terminator.
fn skip_gif_sub_blocks(b: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = usize::from(*b.get(pos)?);
        pos = pos.checked_add(1)?;
        if len == 0 {
            return Some(pos);
        }
        pos = pos.checked_add(len)?;
    }
}

/// Walks the GIF block structure without decompressing any pixel data. Returns the logical
/// screen size and the frame count. A frame that does not fit inside the screen is rejected
/// (the gif crate does not check this by default, and decoding cost then scales with the frame
/// size rather than the screen size). The frame cap and the pixel budget are enforced while
/// walking, so a hostile file stops early.
fn scan_gif(b: &[u8], limits: &Limits) -> Result<(u32, u32, usize), SkipReason> {
    let bad = SkipReason::BadImage;
    if b.len() < 13 {
        return Err(bad);
    }
    let u16_at = |at: usize| -> Option<u32> {
        Some(u32::from(u16::from_le_bytes([
            *b.get(at)?,
            *b.get(at + 1)?,
        ])))
    };
    let width = u16_at(6).ok_or(bad)?;
    let height = u16_at(8).ok_or(bad)?;
    if width == 0 || height == 0 {
        return Err(bad);
    }
    if width > limits.max_dimension || height > limits.max_dimension {
        return Err(SkipReason::TooManyPixels);
    }
    let mut pos = 13usize;
    if b[10] & 0x80 != 0 {
        pos += 3 * (1usize << ((b[10] & 7) + 1));
    }
    let mut frames = 0usize;
    loop {
        match *b.get(pos).ok_or(bad)? {
            0x3B => break,
            0x21 => pos = skip_gif_sub_blocks(b, pos + 2).ok_or(bad)?,
            0x2C => {
                let left = u16_at(pos + 1).ok_or(bad)?;
                let top = u16_at(pos + 3).ok_or(bad)?;
                let fw = u16_at(pos + 5).ok_or(bad)?;
                let fh = u16_at(pos + 7).ok_or(bad)?;
                let flags = *b.get(pos + 9).ok_or(bad)?;
                if fw == 0 || fh == 0 || left + fw > width || top + fh > height {
                    return Err(bad);
                }
                pos += 10;
                if flags & 0x80 != 0 {
                    pos += 3 * (1usize << ((flags & 7) + 1));
                }
                // LZW minimum code size, then the data sub-blocks.
                pos = skip_gif_sub_blocks(b, pos + 1).ok_or(bad)?;
                frames += 1;
                if frames > limits.max_frames {
                    return Err(SkipReason::TooManyFrames);
                }
                check_budget(width, height, frames, limits)?;
            }
            _ => return Err(bad),
        }
    }
    if frames == 0 {
        return Err(bad);
    }
    Ok((width, height, frames))
}

/// Counts the top-level `ANMF` chunks of a WebP file without decoding anything.
fn count_webp_frames(b: &[u8]) -> usize {
    let mut pos = 12usize;
    let mut frames = 0usize;
    while let Some(head) = b.get(pos..pos + 8) {
        let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        if &head[0..4] == b"ANMF" {
            frames += 1;
        }
        pos = match pos
            .checked_add(8)
            .and_then(|p| p.checked_add(size + (size & 1)))
        {
            Some(next) => next,
            None => break,
        };
    }
    frames
}

fn inspect(bytes: &[u8], format: Format, limits: &Limits) -> Result<Inspected, SkipReason> {
    use image::AnimationDecoder;
    let image_format = match format {
        Format::Png => image::ImageFormat::Png,
        Format::Jpeg => image::ImageFormat::Jpeg,
        Format::Gif => image::ImageFormat::Gif,
        Format::Webp => image::ImageFormat::WebP,
    };
    let (width, height, scanned_frames) = if format == Format::Gif {
        scan_gif(bytes, limits)?
    } else {
        let (w, h) = image::ImageReader::with_format(Cursor::new(bytes), image_format)
            .into_dimensions()
            .map_err(|_| SkipReason::BadImage)?;
        (w, h, 1)
    };
    if width == 0 || height == 0 {
        return Err(SkipReason::BadImage);
    }
    if width > limits.max_dimension || height > limits.max_dimension {
        return Err(SkipReason::TooManyPixels);
    }
    // Frames are iterated, never collected. Counting stops at the frame cap plus one and at
    // the pixel budget, using the logical screen or canvas size (what GPUI allocates per frame).
    let count_frames = |frames: image::Frames<'_>| -> Result<usize, SkipReason> {
        let mut n = 0usize;
        for frame in frames {
            frame.map_err(|_| SkipReason::BadImage)?;
            n += 1;
            if n > limits.max_frames {
                return Err(SkipReason::TooManyFrames);
            }
            check_budget(width, height, n, limits)?;
        }
        Ok(n)
    };
    let animated = match format {
        Format::Gif => {
            check_budget(width, height, scanned_frames, limits)?;
            let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                .map_err(|_| SkipReason::BadImage)?;
            count_frames(decoder.into_frames())? > 1
        }
        Format::Webp => {
            let decoder = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|_| SkipReason::BadImage)?;
            if decoder.has_animation() {
                let declared = count_webp_frames(bytes);
                if declared > limits.max_frames {
                    return Err(SkipReason::TooManyFrames);
                }
                check_budget(width, height, declared.max(1), limits)?;
                count_frames(decoder.into_frames())?;
                true
            } else {
                decode_static(bytes, image_format, limits)?;
                false
            }
        }
        _ => {
            decode_static(bytes, image_format, limits)?;
            false
        }
    };
    Ok(Inspected {
        width,
        height,
        animated,
    })
}

/// Fully decodes a static image (already capped by `max_dimension`), so a valid header over a
/// damaged body is rejected.
fn decode_static(
    bytes: &[u8],
    format: image::ImageFormat,
    limits: &Limits,
) -> Result<(), SkipReason> {
    let (w, h) = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| SkipReason::BadImage)?;
    check_budget(w, h, 1, limits)?;
    image::ImageReader::with_format(Cursor::new(bytes), format)
        .decode()
        .map(|_| ())
        .map_err(|_| SkipReason::BadImage)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const EOCD_SIG: &[u8; 4] = b"PK\x05\x06";
const ZIP64_LOCATOR_SIG: &[u8; 4] = b"PK\x06\x07";
const ZIP64_EOCD_SIG: &[u8; 4] = b"PK\x06\x06";
/// The end of central directory record is 22 bytes plus a comment of at most 65535 bytes.
const EOCD_WINDOW: u64 = 65535 + 22;

fn too_many_files(count: u64, limits: &Limits) -> String {
    format!(
        "The zip has too many files ({count}, the limit is {})",
        limits.max_entries
    )
}

/// Reads the declared entry count from the last end of central directory record, before the
/// zip library parses the whole central directory. Only the declared count is judged: the
/// zip library still validates the structure.
fn precheck_entry_count(
    file: &mut std::fs::File,
    size: u64,
    limits: &Limits,
) -> Result<(), String> {
    let not_zip = |why: &str| format!("This is not a valid zip file: {why}");
    let tail_len = size.min(EOCD_WINDOW);
    let tail_start = size - tail_len;
    file.seek(SeekFrom::Start(tail_start))
        .map_err(|e| format!("Cannot read the zip file: {e}"))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail)
        .map_err(|e| format!("Cannot read the zip file: {e}"))?;
    let Some(at) = tail
        .windows(4)
        .rposition(|w| w == EOCD_SIG)
        .filter(|at| at + 22 <= tail.len())
    else {
        return Err(not_zip("no end of central directory"));
    };
    let mut declared = u64::from(u16::from_le_bytes([tail[at + 10], tail[at + 11]]));
    if declared == 0xFFFF && at >= 20 && &tail[at - 20..at - 16] == ZIP64_LOCATOR_SIG {
        let loc = &tail[at - 20..at];
        let offset = u64::from_le_bytes(loc[8..16].try_into().expect("8 bytes"));
        if offset.checked_add(56).is_some_and(|end| end <= size) {
            let mut rec = [0u8; 56];
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(&mut rec))
                .map_err(|e| format!("Cannot read the zip file: {e}"))?;
            if &rec[0..4] == ZIP64_EOCD_SIG {
                declared = u64::from_le_bytes(rec[32..40].try_into().expect("8 bytes"));
            }
        }
    }
    if declared > limits.max_entries as u64 {
        return Err(too_many_files(declared, limits));
    }
    Ok(())
}

/// Wraps the zip file: counts every byte read against a budget and stops when cancelled, so
/// a crafted archive cannot keep the parser busy (many end of central directory candidates,
/// overlapping entries) for long.
struct GuardedFile<'a> {
    file: std::fs::File,
    remaining: u64,
    cancel: &'a AtomicBool,
    over_budget: &'a AtomicBool,
}

impl Read for GuardedFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            self.over_budget.store(true, Ordering::Relaxed);
            return Err(std::io::Error::other("read budget exceeded"));
        }
        let cap = buf
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let n = self.file.read(&mut buf[..cap])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

impl Seek for GuardedFile<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

/// Removes the staging folder on drop unless `keep` was called, so an error, a cancel or a
/// panic cannot leave it behind.
struct StagingGuard {
    dir: PathBuf,
    armed: bool,
}

impl StagingGuard {
    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

type Archive<'a> = zip::ZipArchive<GuardedFile<'a>>;

/// Reads the zip, validates every entry and writes the images that pass into a new
/// `.staging-<uuid>` folder inside `emoji_dir`. Any error removes the staging folder.
pub fn stage(
    zip_path: &Path,
    emoji_dir: &Path,
    existing_hashes: &HashSet<String>,
    limits: &Limits,
    cancel: &AtomicBool,
    progress: impl FnMut(usize, usize),
) -> Result<Staged, String> {
    let cancelled = || "Cancelled".to_string();
    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let size = std::fs::metadata(zip_path)
        .map_err(|e| format!("Cannot read the zip file: {e}"))?
        .len();
    if size > limits.max_zip_bytes {
        return Err("The zip file is too large to import".to_string());
    }
    let mut file =
        std::fs::File::open(zip_path).map_err(|e| format!("Cannot open the zip file: {e}"))?;
    precheck_entry_count(&mut file, size, limits)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("Cannot read the zip file: {e}"))?;
    let over_budget = AtomicBool::new(false);
    let guarded = GuardedFile {
        file,
        // Honest archives read each byte about once; the slack covers headers and re-reads.
        remaining: size.saturating_mul(2).saturating_add(1024 * 1024),
        cancel,
        over_budget: &over_budget,
    };
    let mut archive = zip::ZipArchive::new(guarded).map_err(|e| {
        if cancel.load(Ordering::Relaxed) {
            cancelled()
        } else if over_budget.load(Ordering::Relaxed) {
            "The zip is too complex to import".to_string()
        } else {
            format!("This is not a valid zip file: {e}")
        }
    })?;
    if archive.len() > limits.max_entries {
        return Err(too_many_files(archive.len() as u64, limits));
    }
    std::fs::create_dir_all(emoji_dir)
        .map_err(|e| format!("Cannot create the emoji folder: {e}"))?;
    let dir = emoji_dir.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).map_err(|e| format!("Cannot create the staging folder: {e}"))?;
    let guard = StagingGuard {
        dir: dir.clone(),
        armed: true,
    };
    let staged = stage_into(
        &mut archive,
        &dir,
        zip_path,
        existing_hashes,
        limits,
        cancel,
        &over_budget,
        progress,
    )?;
    guard.keep();
    Ok(staged)
}

#[allow(clippy::too_many_arguments)]
fn stage_into(
    archive: &mut Archive<'_>,
    dir: &Path,
    zip_path: &Path,
    existing_hashes: &HashSet<String>,
    limits: &Limits,
    cancel: &AtomicBool,
    over_budget: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Result<Staged, String> {
    let total = archive.len();
    let mut images: Vec<StagedImage> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut total_read: u64 = 0;
    for index in 0..total {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        let outcome = stage_entry(
            archive,
            index,
            dir,
            existing_hashes,
            limits,
            &mut seen,
            &mut total_read,
        );
        // A cancel or an exhausted read budget during the entry is a hard stop, not a skip.
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        if over_budget.load(Ordering::Relaxed) {
            return Err("The zip is too complex to import".to_string());
        }
        match outcome? {
            Outcome::Ignore => {}
            Outcome::Skip(entry, reason) => skipped.push(Skipped { entry, reason }),
            Outcome::Keep(image) => images.push(image),
        }
        progress(index + 1, total);
    }
    Ok(Staged {
        dir: dir.to_path_buf(),
        source_name: source_name_of(zip_path),
        entries: total,
        images,
        skipped,
    })
}

enum Outcome {
    Ignore,
    Skip(String, SkipReason),
    Keep(StagedImage),
}

fn stage_entry(
    archive: &mut Archive<'_>,
    index: usize,
    dir: &Path,
    existing_hashes: &HashSet<String>,
    limits: &Limits,
    seen: &mut HashSet<String>,
    total_read: &mut u64,
) -> Result<Outcome, String> {
    // Classify by name and metadata first, through the raw reader (no decompression).
    let (shown, display, pre_skip) = {
        let entry = archive
            .by_index_raw(index)
            .map_err(|e| format!("The zip is damaged: {e}"))?;
        let raw = entry.name_raw().to_vec();
        if entry.is_dir() || raw.ends_with(b"/") || raw.ends_with(b"\\") {
            return Ok(Outcome::Ignore);
        }
        let lossy = String::from_utf8_lossy(&raw).into_owned();
        let skip = if is_unsafe_path(&raw) {
            Some(SkipReason::UnsafePath)
        } else if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            Some(SkipReason::Symlink)
        } else if has_macosx_component(&raw) {
            Some(SkipReason::MacMetadata)
        } else if has_hidden_component(&raw) {
            Some(SkipReason::Hidden)
        } else if entry.encrypted() {
            Some(SkipReason::Encrypted)
        } else if has_archive_extension(&lossy) {
            Some(SkipReason::NestedArchive)
        } else {
            None
        };
        let shown: String = strip_invisible(&lossy)
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .take(120)
            .collect();
        (shown, sanitize_name(&lossy), skip)
    };
    if let Some(reason) = pre_skip {
        return Ok(Outcome::Skip(shown, reason));
    }

    // Read the bytes actually present, capped at the limit plus one. Header sizes are not
    // trusted.
    let mut buf: Vec<u8> = Vec::new();
    {
        let entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => return Ok(Outcome::Skip(shown, SkipReason::BadImage)),
        };
        let read = entry.take(limits.max_file_bytes + 1).read_to_end(&mut buf);
        *total_read += buf.len() as u64;
        if *total_read > limits.max_total_bytes {
            return Err("The zip is too large once unpacked".to_string());
        }
        if read.is_err() && buf.len() as u64 <= limits.max_file_bytes {
            return Ok(Outcome::Skip(shown, SkipReason::BadImage));
        }
    }
    if buf.len() as u64 > limits.max_file_bytes {
        return Ok(Outcome::Skip(shown, SkipReason::TooLarge));
    }
    if is_archive_magic(&buf) {
        return Ok(Outcome::Skip(shown, SkipReason::NestedArchive));
    }
    let Some(format) = sniff(&buf) else {
        return Ok(Outcome::Skip(shown, SkipReason::NotImage));
    };
    // A decoder panic on hostile bytes must only lose this entry.
    let inspected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inspect(&buf, format, limits)
    }))
    .unwrap_or(Err(SkipReason::BadImage));
    let info = match inspected {
        Ok(info) => info,
        Err(reason) => return Ok(Outcome::Skip(shown, reason)),
    };
    let hash = hex(&Sha256::digest(&buf));
    if existing_hashes.contains(&hash) || !seen.insert(hash.clone()) {
        return Ok(Outcome::Skip(shown, SkipReason::Duplicate));
    }
    let file = format!("{}.{}", &hash[..16], format.extension());
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(&file))
        .map_err(|e| format!("Cannot write the staged image: {e}"))?;
    out.write_all(&buf)
        .map_err(|e| format!("Cannot write the staged image: {e}"))?;
    Ok(Outcome::Keep(StagedImage {
        hash,
        file,
        name: display,
        format,
        animated: info.animated,
        width: info.width,
        height: info.height,
        bytes: buf.len() as u64,
    }))
}

fn valid_pack_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_staging_dir(dir: &Path) -> bool {
    dir.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with(".staging-"))
}

/// Publishes the staged images as `emoji_dir/packs/<pack_id>`. The target is created with
/// `create_dir` first (exclusive: it fails when the id is taken, so two commits cannot both
/// win), then each file is renamed into it and the staging folder is removed. When a move
/// fails the files already moved are put back and the target is removed, so the staging
/// folder stays complete for `discard`.
pub fn commit(staged: &Staged, emoji_dir: &Path, pack_id: &str) -> Result<PathBuf, String> {
    if !valid_pack_id(pack_id) {
        return Err("The pack id is not valid".to_string());
    }
    if staged.images.is_empty() {
        return Err("There are no images to save".to_string());
    }
    if staged.dir.parent() != Some(emoji_dir) || !is_staging_dir(&staged.dir) {
        return Err("The staging folder is not inside the emoji folder".to_string());
    }
    // Staged is public, so refuse file names that are not a plain name.
    if staged
        .images
        .iter()
        .any(|i| Path::new(&i.file).file_name() != Some(std::ffi::OsStr::new(&i.file)))
    {
        return Err("A staged file name is not valid".to_string());
    }
    let packs = emoji_dir.join("packs");
    std::fs::create_dir_all(&packs).map_err(|e| format!("Cannot create the packs folder: {e}"))?;
    let target = packs.join(pack_id);
    std::fs::create_dir(&target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            "A pack with this id already exists".to_string()
        } else {
            format!("Cannot save the pack: {e}")
        }
    })?;
    let mut moved: Vec<&str> = Vec::new();
    for image in &staged.images {
        if let Err(e) = std::fs::rename(staged.dir.join(&image.file), target.join(&image.file)) {
            for done in moved {
                let _ = std::fs::rename(target.join(done), staged.dir.join(done));
            }
            let _ = std::fs::remove_dir_all(&target);
            return Err(format!("Cannot save the pack: {e}"));
        }
        moved.push(&image.file);
    }
    let _ = std::fs::remove_dir_all(&staged.dir);
    Ok(target)
}

/// Removes the staging folder. Does nothing unless the folder is a `.staging-*` folder.
/// Errors are ignored.
pub fn discard(staged: Staged) {
    if is_staging_dir(&staged.dir) {
        let _ = std::fs::remove_dir_all(&staged.dir);
    }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
