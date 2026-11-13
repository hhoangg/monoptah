use super::*;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

fn rgba(w: u32, h: u32, seed: u8) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([seed, (x * 40) as u8, (y * 40) as u8, 255])
    })
}

fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba(w, h, seed).as_raw(), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn jpeg() -> Vec<u8> {
    let mut out = Vec::new();
    let rgb = image::DynamicImage::ImageRgba8(rgba(4, 4, 7)).to_rgb8();
    image::codecs::jpeg::JpegEncoder::new(&mut out)
        .write_image(rgb.as_raw(), 4, 4, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn gif(frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
        let list = (0..frames).map(|i| image::Frame::new(rgba(2, 2, i as u8 * 50)));
        enc.encode_frames(list).unwrap();
    }
    out
}

fn webp() -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut out)
        .write_image(rgba(3, 3, 9).as_raw(), 3, 3, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn opts() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Stored)
}

fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, data) in entries {
        w.start_file(*name, opts()).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap();
}

struct Env {
    tmp: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            tmp: tempfile::tempdir().unwrap(),
        }
    }
    fn zip(&self) -> PathBuf {
        self.tmp
            .path()
            .join("zips")
            .join("830563-Cats-emojigg-pack.zip")
    }
    fn emoji(&self) -> PathBuf {
        self.tmp.path().join("emoji")
    }
    fn build(&self, entries: &[(&str, &[u8])]) {
        std::fs::create_dir_all(self.zip().parent().unwrap()).unwrap();
        make_zip(&self.zip(), entries);
    }
    fn run(&self, limits: &Limits) -> Result<Staged, String> {
        self.run_with(&HashSet::new(), limits, &AtomicBool::new(false))
    }
    fn run_with(
        &self,
        existing: &HashSet<String>,
        limits: &Limits,
        cancel: &AtomicBool,
    ) -> Result<Staged, String> {
        stage(
            &self.zip(),
            &self.emoji(),
            existing,
            limits,
            cancel,
            |_, _| {},
        )
    }
    fn staging_dirs(&self) -> Vec<PathBuf> {
        match std::fs::read_dir(self.emoji()) {
            Ok(rd) => rd.map(|e| e.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn reasons(s: &Staged) -> Vec<(String, SkipReason)> {
    s.skipped
        .iter()
        .map(|k| (k.entry.clone(), k.reason))
        .collect()
}

fn files_of(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(root).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(files_of(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn happy_path_stages_all_four_formats_only_inside_staging() {
    let env = Env::new();
    let (p, j, g, w) = (png(2, 2, 1), jpeg(), gif(2), webp());
    env.build(&[
        ("830563-happy cat.png", &p),
        ("sub/dir/photo.jpg", &j),
        ("anim.gif", &g),
        ("sticker.webp", &w),
    ]);
    let calls = std::cell::RefCell::new(Vec::new());
    let staged = stage(
        &env.zip(),
        &env.emoji(),
        &HashSet::new(),
        &Limits::default(),
        &AtomicBool::new(false),
        |d, t| calls.borrow_mut().push((d, t)),
    )
    .unwrap();
    assert_eq!(*calls.borrow(), vec![(1, 4), (2, 4), (3, 4), (4, 4)]);
    assert_eq!(staged.entries, 4);
    assert_eq!(staged.source_name, "Cats");
    assert!(staged.skipped.is_empty());
    assert_eq!(staged.images.len(), 4);
    let by_name = |n: &str| staged.images.iter().find(|i| i.name == n).unwrap();
    assert_eq!(by_name("happy cat").format, Format::Png);
    assert_eq!(
        (by_name("happy cat").width, by_name("happy cat").height),
        (2, 2)
    );
    assert_eq!(by_name("photo").format, Format::Jpeg);
    assert!(by_name("anim").animated);
    assert_eq!(by_name("anim").format, Format::Gif);
    assert_eq!(by_name("sticker").format, Format::Webp);
    assert!(!by_name("sticker").animated);
    for img in &staged.images {
        assert_eq!(img.hash.len(), 64);
        assert_eq!(
            img.file,
            format!("{}.{}", &img.hash[..16], img.format.extension())
        );
        assert!(staged.dir.join(&img.file).is_file());
    }
    // Nothing was written outside the staging folder, and only hash-named files inside it.
    let all = files_of(env.tmp.path());
    let zip = env.zip();
    for f in all {
        if f == zip {
            continue;
        }
        assert_eq!(f.parent().unwrap(), staged.dir, "stray file {f:?}");
    }
    assert_eq!(files_of(&staged.dir).len(), 4);
}

#[test]
fn skips_each_unsafe_or_ignored_entry_with_its_reason() {
    let env = Env::new();
    let good = png(2, 2, 2);
    let mut inner = Vec::new();
    {
        let mut w = zip::ZipWriter::new(Cursor::new(&mut inner));
        w.start_file("a.png", opts()).unwrap();
        w.write_all(&png(2, 2, 3)).unwrap();
        w.finish().unwrap();
    }
    let g = gif(1);
    env.build(&[
        ("../evil.png", &good),
        ("/abs.png", &good),
        ("C:\\x.png", &good),
        ("a\\..\\..\\b.png", &good),
        ("nul\0.png", &good),
        ("__MACOSX/._a.png", &good),
        ("x/__MACOSX/a.png", &good),
        (".hidden.png", &good),
        ("dir/.x/a.png", &good),
        ("inner.zip", &inner),
        ("pic.png", &inner),
        ("fake.png", b"just text"),
        ("vector.svg", b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
        ("real.txt", &g),
        ("broken.png", &good[..20]),
        ("keep.png", &good),
    ]);
    let s = env.run(&Limits::default()).unwrap();
    let r = reasons(&s);
    let has = |name: &str, reason: SkipReason| {
        assert!(
            r.iter().any(|(n, rs)| n == name && *rs == reason),
            "{name} {reason:?} missing in {r:?}"
        );
    };
    has("../evil.png", SkipReason::UnsafePath);
    has("/abs.png", SkipReason::UnsafePath);
    has("C:\\x.png", SkipReason::UnsafePath);
    has("a\\..\\..\\b.png", SkipReason::UnsafePath);
    assert!(r
        .iter()
        .any(|(n, rs)| n.starts_with("nul") && *rs == SkipReason::UnsafePath));
    has("__MACOSX/._a.png", SkipReason::MacMetadata);
    has("x/__MACOSX/a.png", SkipReason::MacMetadata);
    has(".hidden.png", SkipReason::Hidden);
    has("dir/.x/a.png", SkipReason::Hidden);
    has("inner.zip", SkipReason::NestedArchive);
    has("pic.png", SkipReason::NestedArchive);
    has("fake.png", SkipReason::NotImage);
    has("vector.svg", SkipReason::NotImage);
    has("broken.png", SkipReason::BadImage);
    // Extension is ignored: a GIF named .txt is kept as a GIF.
    let names: Vec<_> = s
        .images
        .iter()
        .map(|i| (i.name.as_str(), i.format))
        .collect();
    assert!(names.contains(&("real", Format::Gif)), "{names:?}");
    assert!(names.contains(&("keep", Format::Png)));
    assert_eq!(s.images.len(), 2);
    for k in &s.skipped {
        assert!(!k.reason.label().is_empty());
    }
}

#[test]
fn symlink_directory_and_encrypted_entries_are_skipped() {
    let env = Env::new();
    std::fs::create_dir_all(env.zip().parent().unwrap()).unwrap();
    let good = png(2, 2, 4);
    let mut w = zip::ZipWriter::new(std::fs::File::create(env.zip()).unwrap());
    w.add_directory("folder/", opts()).unwrap();
    w.add_symlink("link.png", "/etc/passwd", opts().unix_permissions(0o777))
        .unwrap();
    w.start_file("secret.png", opts()).unwrap();
    w.write_all(&good).unwrap();
    w.start_file("ok.png", opts()).unwrap();
    w.write_all(&good).unwrap();
    w.finish().unwrap();
    // Set the "encrypted" flag on secret.png in its local and central headers.
    let mut bytes = std::fs::read(env.zip()).unwrap();
    let name = b"secret.png";
    let mut patched = 0;
    for i in 0..bytes.len().saturating_sub(46 + name.len()) {
        if bytes[i..].starts_with(b"PK\x03\x04") && bytes[i + 30..].starts_with(name) {
            bytes[i + 6] |= 1;
            patched += 1;
        }
        if bytes[i..].starts_with(b"PK\x01\x02") && bytes[i + 46..].starts_with(name) {
            bytes[i + 8] |= 1;
            patched += 1;
        }
    }
    assert_eq!(patched, 2);
    std::fs::write(env.zip(), bytes).unwrap();
    let s = env.run(&Limits::default()).unwrap();
    let r = reasons(&s);
    assert!(
        r.contains(&("link.png".to_string(), SkipReason::Symlink)),
        "{r:?}"
    );
    assert!(
        r.contains(&("secret.png".to_string(), SkipReason::Encrypted)),
        "{r:?}"
    );
    assert!(!r.iter().any(|(n, _)| n.starts_with("folder")));
    assert_eq!(s.images.len(), 1);
}

#[test]
fn nested_archive_magic_bytes_are_detected() {
    let env = Env::new();
    let mut tar = vec![0u8; 512];
    tar[257..262].copy_from_slice(b"ustar");
    env.build(&[
        ("a.png", b"Rar!\x1a\x07\x00 rest"),
        ("b.png", &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0, 0]),
        ("c.png", &[0x1F, 0x8B, 8, 0, 0, 0]),
        ("d.png", &tar),
        ("e.png", b"BZh91AY"),
        ("f.png", &[0xFD, b'7', b'z', b'X', b'Z', 0, 1]),
        ("g.png", &[0x28, 0xB5, 0x2F, 0xFD, 0, 0]),
    ]);
    let s = env.run(&Limits::default()).unwrap();
    assert_eq!(s.skipped.len(), 7);
    assert!(s
        .skipped
        .iter()
        .all(|k| k.reason == SkipReason::NestedArchive));
}

#[test]
fn too_many_entries_errors_and_extracts_nothing() {
    let env = Env::new();
    std::fs::create_dir_all(env.zip().parent().unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(std::fs::File::create(env.zip()).unwrap());
    for i in 0..501 {
        w.start_file(format!("{i}.txt"), opts()).unwrap();
    }
    w.finish().unwrap();
    let err = env.run(&Limits::default()).unwrap_err();
    assert!(err.contains("too many files"), "{err}");
    assert!(env.staging_dirs().is_empty());
}

#[test]
fn per_file_limit_uses_bytes_read_not_the_header() {
    let env = Env::new();
    std::fs::create_dir_all(env.zip().parent().unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(std::fs::File::create(env.zip()).unwrap());
    // A deflated 11 MB entry of zeros: tiny on disk, over the default 10 MB per-file cap.
    w.start_file(
        "bomb.png",
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )
    .unwrap();
    let chunk = vec![0u8; 1024 * 1024];
    for _ in 0..11 {
        w.write_all(&chunk).unwrap();
    }
    w.finish().unwrap();
    assert!(std::fs::metadata(env.zip()).unwrap().len() < 100 * 1024);
    let s = env.run(&Limits::default()).unwrap();
    assert_eq!(
        reasons(&s),
        vec![("bomb.png".to_string(), SkipReason::TooLarge)]
    );
    assert!(s.images.is_empty());
    discard(s);
}

#[test]
fn total_limit_stops_the_import_and_removes_staging() {
    let env = Env::new();
    std::fs::create_dir_all(env.zip().parent().unwrap()).unwrap();
    let mut w = zip::ZipWriter::new(std::fs::File::create(env.zip()).unwrap());
    let chunk = vec![0u8; 1024 * 1024];
    for i in 0..4 {
        w.start_file(
            format!("z{i}.bin"),
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
        )
        .unwrap();
        for _ in 0..9 {
            w.write_all(&chunk).unwrap();
        }
    }
    w.finish().unwrap();
    let limits = Limits {
        max_total_bytes: 20 * 1024 * 1024,
        ..Limits::default()
    };
    let err = env.run(&limits).unwrap_err();
    assert!(err.contains("too large"), "{err}");
    assert!(env.staging_dirs().is_empty());
}

#[test]
fn zip_larger_than_limit_errors_before_reading() {
    let env = Env::new();
    env.build(&[("a.png", &png(2, 2, 1))]);
    let limits = Limits {
        max_zip_bytes: 10,
        ..Limits::default()
    };
    let err = env.run(&limits).unwrap_err();
    assert!(err.contains("too large"), "{err}");
    assert!(!env.emoji().exists());
}

#[test]
fn not_a_zip_errors() {
    let env = Env::new();
    std::fs::create_dir_all(env.zip().parent().unwrap()).unwrap();
    std::fs::write(env.zip(), b"this is definitely not a zip file at all").unwrap();
    let err = env.run(&Limits::default()).unwrap_err();
    assert!(err.contains("not a valid zip"), "{err}");
    assert!(env.staging_dirs().is_empty());
}

#[test]
fn duplicates_within_zip_and_against_existing_are_skipped() {
    let env = Env::new();
    let (a, b) = (png(2, 2, 1), png(2, 2, 2));
    env.build(&[("a.png", &a), ("copy.png", &a), ("b.png", &b)]);
    let s = env.run(&Limits::default()).unwrap();
    assert_eq!(s.images.len(), 2);
    assert_eq!(
        reasons(&s),
        vec![("copy.png".to_string(), SkipReason::Duplicate)]
    );
    let b_hash = s
        .images
        .iter()
        .find(|i| i.name == "b")
        .unwrap()
        .hash
        .clone();
    discard(s);
    let existing: HashSet<String> = [b_hash].into_iter().collect();
    let s = env
        .run_with(&existing, &Limits::default(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(s.images.len(), 1);
    assert_eq!(s.images[0].name, "a");
    assert!(s
        .skipped
        .iter()
        .any(|k| k.entry == "b.png" && k.reason == SkipReason::Duplicate));
}

#[test]
fn dimension_cap_skips_large_images() {
    let env = Env::new();
    env.build(&[("big.png", &png(8, 2, 1)), ("small.png", &png(4, 4, 2))]);
    let limits = Limits {
        max_dimension: 4,
        ..Limits::default()
    };
    let s = env.run(&limits).unwrap();
    assert_eq!(
        reasons(&s),
        vec![("big.png".to_string(), SkipReason::TooManyPixels)]
    );
    assert_eq!(s.images.len(), 1);
}

#[test]
fn frame_cap_skips_long_animations() {
    let env = Env::new();
    env.build(&[
        ("long.gif", &gif(3)),
        ("short.gif", &gif(2)),
        ("one.gif", &gif(1)),
    ]);
    let limits = Limits {
        max_frames: 2,
        ..Limits::default()
    };
    let s = env.run(&limits).unwrap();
    assert_eq!(
        reasons(&s),
        vec![("long.gif".to_string(), SkipReason::TooManyFrames)]
    );
    let one = s.images.iter().find(|i| i.name == "one").unwrap();
    assert!(!one.animated);
    assert!(
        s.images
            .iter()
            .find(|i| i.name == "short")
            .unwrap()
            .animated
    );
}

#[test]
fn cancel_removes_staging() {
    let env = Env::new();
    env.build(&[("a.png", &png(2, 2, 1)), ("b.png", &png(2, 2, 2))]);
    let cancel = AtomicBool::new(false);
    let err = stage(
        &env.zip(),
        &env.emoji(),
        &HashSet::new(),
        &Limits::default(),
        &cancel,
        |done, _| {
            if done == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap_err();
    assert_eq!(err, "Cancelled");
    assert!(env.staging_dirs().is_empty());
}

#[test]
fn commit_moves_atomically_and_refuses_bad_targets() {
    let env = Env::new();
    env.build(&[("a.png", &png(2, 2, 1))]);
    let s = env.run(&Limits::default()).unwrap();
    let file = s.images[0].file.clone();
    for bad in ["", "UPPER", "a/b", "..", "a_b", &"a".repeat(65)] {
        assert!(commit(&s, &env.emoji(), bad).is_err(), "{bad}");
    }
    assert!(s.dir.is_dir());
    let target = commit(&s, &env.emoji(), "cats-1").unwrap();
    assert_eq!(target, env.emoji().join("packs").join("cats-1"));
    assert!(target.join(&file).is_file());
    assert!(!s.dir.exists());
    // A second import cannot take the same id.
    let s2 = env.run(&Limits::default()).unwrap();
    assert!(commit(&s2, &env.emoji(), "cats-1").is_err());
    assert!(s2.dir.is_dir());
    // A staging folder from elsewhere is refused.
    let other = env.tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let fake = Staged {
        dir: other,
        source_name: String::new(),
        entries: 0,
        images: vec![],
        skipped: vec![],
    };
    assert!(commit(&fake, &env.emoji(), "ok").is_err());
}

#[test]
fn discard_removes_staging() {
    let env = Env::new();
    env.build(&[("a.png", &png(2, 2, 1))]);
    let s = env.run(&Limits::default()).unwrap();
    let dir = s.dir.clone();
    assert!(dir.is_dir());
    discard(s);
    assert!(!dir.exists());
}

#[test]
fn sanitize_name_cases() {
    assert_eq!(sanitize_name("830563-happy cat.png"), "happy cat");
    assert_eq!(sanitize_name("a/b/c/dir\\pic.gif"), "pic");
    assert_eq!(
        sanitize_name("  spaced   out \t name.png"),
        "spaced out name"
    );
    assert_eq!(sanitize_name("we<ird>:*?\"|name\u{7}.png"), "weirdname");
    assert_eq!(
        sanitize_name("snake_case-kebab.v2.png"),
        "snake_case-kebab.v2"
    );
    assert_eq!(sanitize_name("12345.png"), "12345");
    assert_eq!(sanitize_name("***.png"), "emoji");
    assert_eq!(sanitize_name(""), "emoji");
    assert_eq!(sanitize_name("dir/"), "emoji");
    let long = format!("{}.png", "\u{e9}".repeat(100));
    assert_eq!(sanitize_name(&long).chars().count(), 64);
}

#[test]
fn source_name_strips_prefix_and_pack_suffix() {
    assert_eq!(
        source_name_of(Path::new("/x/830563-Cats-emojigg-pack.zip")),
        "Cats"
    );
    assert_eq!(source_name_of(Path::new("/x/My pack.zip")), "My pack");
    assert_eq!(source_name_of(Path::new("/x/-emojigg-pack.zip")), "emoji");
}

#[test]
fn sniff_cases() {
    assert_eq!(sniff(&png(2, 2, 1)), Some(Format::Png));
    assert_eq!(sniff(&jpeg()), Some(Format::Jpeg));
    assert_eq!(sniff(b"GIF87a.."), Some(Format::Gif));
    assert_eq!(sniff(b"GIF89a.."), Some(Format::Gif));
    assert_eq!(sniff(&webp()), Some(Format::Webp));
    assert_eq!(sniff(b"RIFF\0\0\0\0WAVE"), None);
    assert_eq!(sniff(b"RIFF\0\0\0\0WEB"), None);
    assert_eq!(sniff(b"BM......"), None);
    assert_eq!(sniff(b"<svg>"), None);
    assert_eq!(sniff(b""), None);
}
