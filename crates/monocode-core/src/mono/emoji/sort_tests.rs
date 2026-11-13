use super::*;
use std::sync::Mutex;

const DEFAULTS: [&str; 13] = [
    "happy",
    "celebrate",
    "laugh",
    "love",
    "sad",
    "angry",
    "thinking",
    "surprised",
    "approve",
    "cool",
    "clever",
    "nervous",
    "chill",
];

fn emotions() -> Vec<EmotionRef> {
    DEFAULTS
        .iter()
        .map(|n| EmotionRef {
            name: (*n).into(),
            description: format!("Feeling {n}"),
        })
        .collect()
}

fn labels(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("e{i:02}.png")).collect()
}

fn items(dir: &Path, count: usize) -> Vec<Item> {
    (0..count)
        .map(|i| {
            let path = dir.join(format!("src{i}.png"));
            image::RgbaImage::from_pixel(200, 120, image::Rgba([i as u8, 10, 20, 255]))
                .save(&path)
                .unwrap();
            Item {
                key: format!("k{i}"),
                name: if i == 0 {
                    "pepeheart".into()
                } else {
                    format!("thing{i}")
                },
                path,
                format: Format::Png,
            }
        })
        .collect()
}

fn not_cancelled() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<(String, Vec<PathBuf>)>>,
    fail: bool,
}

impl Runner for Fake {
    fn ask(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        _cancel: Arc<AtomicBool>,
    ) -> Result<String, String> {
        for image in &images {
            let (w, h) = image::image_dimensions(image).unwrap();
            assert!(w <= THUMB_PX && h <= THUMB_PX);
            assert_eq!(
                image::ImageReader::open(image)
                    .unwrap()
                    .with_guessed_format()
                    .unwrap()
                    .format(),
                Some(image::ImageFormat::Png)
            );
        }
        let names: Vec<String> = images
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        self.calls.lock().unwrap().push((prompt, images));
        if self.fail {
            return Err("model refused".into());
        }
        let body = names
            .iter()
            .map(|n| format!("\"{n}\": \"Happy\""))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!("{{{body}}}"))
    }
}

#[test]
fn prompt_lists_every_emotion_and_label() {
    let text = prompt(&emotions(), &labels(3));
    for name in DEFAULTS {
        assert!(text.contains(name), "{name}");
    }
    assert!(text.contains("Feeling sad"));
    for label in labels(3) {
        assert!(text.contains(&label));
    }
    assert!(text.contains("unsorted"));
    assert!(text.contains("JSON"));
}

#[test]
fn parse_reply_handles_fences_prose_and_casing() {
    let l = labels(2);
    let got = parse_reply(
        "Sure!\n```json\n{\"e01.png\": \" HAPPY \", \"e02.png\": \"Love\"}\n```\nDone.",
        &l,
        &emotions(),
    )
    .unwrap();
    assert_eq!(got["e01.png"].as_deref(), Some("happy"));
    assert_eq!(got["e02.png"].as_deref(), Some("love"));
}

#[test]
fn parse_reply_maps_unknown_missing_and_non_string_to_none() {
    let l = labels(4);
    let got = parse_reply(
        r#"{"e01.png": "wat", "e02.png": "unsorted", "e03.png": 7, "extra.png": "sad"}"#,
        &l,
        &emotions(),
    )
    .unwrap();
    assert_eq!(got.len(), 4);
    assert!(got.values().all(|v| v.is_none()));
    assert!(!got.contains_key("extra.png"));
}

#[test]
fn parse_reply_skips_a_stray_brace_and_fails_on_garbage() {
    let l = labels(1);
    let got = parse_reply("{oops} then {\"e01.png\": \"sad\"}", &l, &emotions()).unwrap();
    assert_eq!(got["e01.png"].as_deref(), Some("sad"));
    assert!(parse_reply("no json here", &l, &emotions()).is_err());
    assert!(parse_reply("{broken", &l, &emotions()).is_err());
}

#[test]
fn names_match_the_real_pack_words() {
    let e = emotions();
    for (name, want) in [
        ("pepeheart", Some("love")),
        ("crying", Some("sad")),
        ("lol", Some("laugh")),
        ("pepeokay", Some("approve")),
        ("cigpepe", Some("cool")),
        ("PepeSmile", Some("happy")),
        ("hmm2", Some("thinking")),
        ("urgay", None),
    ] {
        assert_eq!(suggest_by_name(name, &e).as_deref(), want, "{name}");
    }
}

#[test]
fn synonyms_apply_only_when_the_emotion_exists() {
    let only_sad: Vec<EmotionRef> = emotions().into_iter().filter(|e| e.name == "sad").collect();
    assert_eq!(suggest_by_name("pepeheart", &only_sad), None);
    assert_eq!(suggest_by_name("crying", &only_sad).as_deref(), Some("sad"));
    let custom = vec![EmotionRef {
        name: "Spooky".into(),
        description: "Ghostly things".into(),
    }];
    assert_eq!(
        suggest_by_name("spooky_cat", &custom).as_deref(),
        Some("Spooky")
    );
    assert_eq!(
        suggest_by_name("a ghostly one", &custom).as_deref(),
        Some("Spooky")
    );
}

#[test]
fn thumbnail_shrinks_to_png_and_refuses_bad_input() {
    let dir = tempfile::tempdir().unwrap();
    let src = items(dir.path(), 1).remove(0).path;
    let out = dir.path().join("t.png");
    thumbnail(&src, &out).unwrap();
    let (w, h) = image::image_dimensions(&out).unwrap();
    assert_eq!((w, h), (96, 58));
    let junk = dir.path().join("junk.png");
    std::fs::write(&junk, b"not an image").unwrap();
    assert!(thumbnail(&junk, &out).is_err());
    let big = dir.path().join("big.png");
    std::fs::write(&big, vec![0u8; 10 * 1024 * 1024 + 1]).unwrap();
    assert!(thumbnail(&big, &out).unwrap_err().contains("10 MB"));
}

#[test]
fn sort_sends_batches_of_sixteen_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let list = items(dir.path(), 20);
    let fake = Fake::default();
    let mut seen = Vec::new();
    let sorted = sort(
        &list,
        &emotions(),
        Some(&fake),
        "Codex",
        &scratch,
        &not_cancelled(),
        |p| seen.push(p),
    )
    .unwrap();
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1.len(), 16);
    assert_eq!(calls[1].1.len(), 4);
    assert!(calls[0].0.contains("e16.png") && !calls[1].0.contains("e05.png"));
    assert!(calls[1].0.contains("e01.png"));
    assert_eq!(
        seen,
        vec![
            Progress {
                done: 16,
                total: 20
            },
            Progress {
                done: 20,
                total: 20
            }
        ]
    );
    assert_eq!(
        sorted.source,
        Source::Model {
            label: "Codex".into()
        }
    );
    assert_eq!(sorted.suggestion.len(), 20);
    assert!(sorted
        .suggestion
        .values()
        .all(|v| v.as_deref() == Some("happy")));
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn sort_falls_back_to_names_when_the_first_call_fails() {
    let dir = tempfile::tempdir().unwrap();
    let scratch = dir.path().join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    let list = items(dir.path(), 3);
    let fake = Fake {
        fail: true,
        ..Default::default()
    };
    let sorted = sort(
        &list,
        &emotions(),
        Some(&fake),
        "Codex",
        &scratch,
        &not_cancelled(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        sorted.source,
        Source::Names {
            reason: "Codex could not sort by image, so emoji were suggested from their file names."
                .into()
        }
    );
    assert_eq!(sorted.suggestion["k0"].as_deref(), Some("love"));
    assert_eq!(sorted.suggestion["k1"], None);
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn sort_without_a_runner_uses_names() {
    let dir = tempfile::tempdir().unwrap();
    let list = items(dir.path(), 2);
    let sorted = sort(
        &list,
        &emotions(),
        None,
        "Pi",
        dir.path(),
        &not_cancelled(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        sorted.source,
        Source::Names {
            reason: "Pi cannot sort by image".into()
        }
    );
    assert_eq!(sorted.suggestion["k0"].as_deref(), Some("love"));
}

#[test]
fn sort_stops_when_cancelled_and_a_bad_image_is_only_unsorted() {
    let dir = tempfile::tempdir().unwrap();
    let list = items(dir.path(), 2);
    let fake = Fake::default();
    let cancel = Arc::new(AtomicBool::new(true));
    let err = sort(
        &list,
        &emotions(),
        Some(&fake),
        "Codex",
        dir.path(),
        &cancel,
        |_| {},
    )
    .err()
    .unwrap();
    assert_eq!(err, "Cancelled");
    assert!(fake.calls.lock().unwrap().is_empty());

    std::fs::write(&list[1].path, b"broken").unwrap();
    let sorted = sort(
        &list,
        &emotions(),
        Some(&fake),
        "Codex",
        dir.path(),
        &not_cancelled(),
        |_| {},
    )
    .unwrap();
    assert_eq!(sorted.suggestion["k0"].as_deref(), Some("happy"));
    assert_eq!(sorted.suggestion["k1"], None);
    assert_eq!(fake.calls.lock().unwrap()[0].1.len(), 1);
}

#[test]
fn supports_images_follows_the_one_off_text_providers() {
    assert!(supports_images(provider::Kind::Codex));
}
