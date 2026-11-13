//! The Emoji settings page and the import dialog. The model is never called: sorting runs through
//! a fake runner, or falls back to file names when none is installed.
use super::workflows_ui::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{ExternalPaths, FileDropEvent, TestAppContext, VisualTestContext};
use monocode_core::mono::emoji::{
    self, Emoji, Emotion, Format, Library, Pack,
    sort::{self, Source},
    test_support,
};
use std::sync::Arc;

/// Answers every batch with fixed emotions, in the order the images are attached.
struct Fake(Vec<&'static str>);

impl sort::Runner for Fake {
    fn ask(
        &self,
        _prompt: String,
        images: Vec<PathBuf>,
        _cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<String, String> {
        let pairs = (0..images.len())
            .map(|i| format!("\"e{:02}.png\": \"{}\"", i + 1, self.0[i % self.0.len()]))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!("{{{pairs}}}"))
    }
}

fn install_fake(s: &Setup, answers: Vec<&'static str>, cx: &mut VisualTestContext) {
    s.manager.update(cx, |m, _| {
        m.emoji_ui.sorter = Arc::new(move |_| {
            Ok(Some(
                Arc::new(Fake(answers.clone())) as Arc<dyn sort::Runner>
            ))
        });
    });
}

fn emoji_dir(s: &Setup) -> PathBuf {
    emoji::dir(&s.root, &s.id).unwrap()
}

fn disk(s: &Setup) -> Library {
    emoji::load(&s.root, &s.id).unwrap().library
}

fn entry(file: &str, name: &str, format: Format, emotion: Option<&str>) -> Emoji {
    Emoji {
        file: file.into(),
        hash: format!("hash-{file}"),
        name: name.into(),
        format,
        emotion: emotion.map(str::to_owned),
    }
}

/// A pack with real images on disk, saved before the page is opened.
fn seed(s: &Setup, library: &mut Library, id: &str, emoji: Vec<Emoji>) {
    let folder = emoji::pack_dir(&emoji_dir(s), id);
    std::fs::create_dir_all(&folder).unwrap();
    for (n, e) in emoji.iter().enumerate() {
        let bytes = match e.format {
            Format::Gif => test_support::gif(2),
            _ => test_support::png(4, 4, n as u8 * 30),
        };
        std::fs::write(folder.join(&e.file), bytes).unwrap();
    }
    library.packs.push(Pack {
        id: id.into(),
        name: id.into(),
        enabled: true,
        sorted_by: Some("Fake model".into()),
        emoji,
    });
    emoji::save(&s.root, &s.id, library).unwrap();
}

fn open(cx: &mut TestAppContext) -> (Setup, VisualTestContext) {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let visual = VisualTestContext::from_window(s.window.into(), cx);
    (s, visual)
}

/// Opens the Emoji page after the library was seeded.
fn show(_s: &Setup, visual: &mut VisualTestContext) {
    visual.run_until_parked();
    click(visual, "mono-settings-Emoji");
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn seeded_page(cx: &mut TestAppContext, emoji: Vec<Emoji>) -> (Setup, VisualTestContext, Library) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let mut library = Library::default();
    seed(&s, &mut library, "pack-seed", emoji);
    show(&s, &mut visual);
    (s, visual, library)
}

fn three() -> Vec<Emoji> {
    vec![
        entry("a.png", "alpha", Format::Png, Some("happy")),
        entry("b.png", "bravo", Format::Png, Some("happy")),
        entry("c.gif", "charlie", Format::Gif, None),
    ]
}

fn library_of(s: &Setup, visual: &VisualTestContext) -> Library {
    s.manager
        .read_with(visual, |m, _| (*m.emoji[&s.id].1).clone())
}

fn dialog_open(s: &Setup, visual: &VisualTestContext) -> bool {
    s.manager
        .read_with(visual, |m, _| m.emoji_ui.import.is_some())
}

fn scroll_page(visual: &mut VisualTestContext, dy: f32) {
    let at = visual.debug_bounds("mono-emoji-scroll").unwrap().center();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-dy))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn make_zip(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emoji-zips-{}", mono::new_id()));
    let zip = dir.join(name);
    test_support::zip(
        &zip,
        &[
            ("smile.png", &test_support::png(4, 4, 10)),
            ("cry.png", &test_support::png(4, 4, 70)),
            ("grin.png", &test_support::png(4, 4, 130)),
            ("party.gif", &test_support::gif(2)),
            ("readme.txt", b"not an image"),
        ],
    );
    zip
}

/// "Import zip..." answered with `zips`, then the unpack and sort jobs settle.
fn import(s: &Setup, visual: &mut VisualTestContext, zips: Vec<PathBuf>) {
    click(visual, "mono-emoji-import");
    assert!(visual.did_prompt_for_paths());
    visual.simulate_path_prompt_response(move |_| Some(zips.clone()));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(dialog_open(s, visual), "the import dialog opens");
}

fn staging_dirs(s: &Setup) -> Vec<PathBuf> {
    std::fs::read_dir(emoji_dir(s))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with(".staging-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn names_in(library: &Library, emotion: &str) -> Vec<String> {
    let mut names: Vec<String> = library
        .packs
        .iter()
        .flat_map(|p| p.emoji.iter())
        .filter(|e| e.emotion.as_deref() == Some(emotion))
        .map(|e| e.name.clone())
        .collect();
    names.sort();
    names
}

#[gpui::test]
fn the_emoji_page_is_the_fourth_settings_page(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    click(&mut visual, "mono-settings-Emoji");
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("mono-emoji-page").is_some());
    assert!(visual.debug_bounds("mono-emoji-enabled").is_some());
    assert!(visual.debug_bounds("mono-emoji-frequency").is_some());
    assert!(visual.debug_bounds("mono-emoji-import").is_some());
    assert!(visual.debug_bounds("mono-emoji-board").is_some());
    assert!(
        s.manager
            .read_with(&visual, |m, _| m.settings == Some(SettingsSection::Emoji))
    );
    // Nothing is written until something changes.
    assert!(!emoji_dir(&s).join("emoji.json").exists());
}

#[gpui::test]
fn the_switch_and_the_frequency_are_saved(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    assert!(!library_of(&s, &visual).enabled);
    click(&mut visual, "mono-emoji-enabled");
    visual.run_until_parked();
    assert!(disk(&s).enabled);
    click(&mut visual, "mono-emoji-frequency");
    visual.refresh().unwrap();
    assert!(
        s.manager
            .read_with(&visual, |m, _| m.emoji_ui.select_is_open()),
        "the list opens"
    );
    click(&mut visual, "menu-action-2");
    visual.run_until_parked();
    let saved = disk(&s);
    assert_eq!(saved.frequency, emoji::Frequency::Often);
    assert!(saved.enabled);
    click(&mut visual, "mono-emoji-enabled");
    visual.run_until_parked();
    assert!(!disk(&s).enabled);
}

#[gpui::test]
fn a_newer_file_is_shown_read_only_and_never_written(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let dir = emoji_dir(&s);
    std::fs::create_dir_all(&dir).unwrap();
    let newer = "{\"version\": 99, \"enabled\": false}";
    std::fs::write(dir.join("emoji.json"), newer).unwrap();
    show(&s, &mut visual);
    assert!(visual.debug_bounds("mono-emoji-read-only").is_some());
    click(&mut visual, "mono-emoji-enabled");
    visual.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(dir.join("emoji.json")).unwrap(),
        newer
    );
    assert!(!library_of(&s, &visual).enabled);
}

#[gpui::test]
fn a_zip_is_unpacked_sorted_reviewed_and_saved(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy", "sad", "happy", "celebrate"], &mut visual);
    let zip = make_zip("pepe-pack.zip");
    import(&s, &mut visual, vec![zip]);
    for selector in [
        "mono-emoji-import-dialog",
        "mono-emoji-step-unpacked",
        "mono-emoji-step-kept",
        "mono-emoji-step-sorting",
        "mono-emoji-review",
        "mono-emoji-review-happy",
        "mono-emoji-import-use",
        "mono-emoji-import-skip",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(visual.debug_bounds("mono-emoji-import-notice").is_none());
    // Nothing is saved before the user decides.
    assert!(!emoji_dir(&s).join("emoji.json").exists());
    assert_eq!(staging_dirs(&s).len(), 1);
    click(&mut visual, "mono-emoji-import-use");
    visual.run_until_parked();
    assert!(!dialog_open(&s, &visual), "the queue is done");
    let saved = disk(&s);
    assert_eq!(saved.packs.len(), 1);
    let pack = &saved.packs[0];
    assert_eq!(pack.name, "pepe-pack");
    assert_eq!(pack.emoji.len(), 4);
    let label = s.manager.read_with(&visual, |m, cx| {
        m.chats[&s.id].read(cx).mono_model_label().to_string()
    });
    assert_eq!(pack.sorted_by.as_deref(), Some(label.as_str()));
    assert_eq!(names_in(&saved, "happy"), ["grin", "smile"]);
    assert_eq!(names_in(&saved, "sad"), ["cry"]);
    assert_eq!(names_in(&saved, "celebrate"), ["party"]);
    for e in &pack.emoji {
        assert!(
            emoji::image_path(&emoji_dir(&s), pack, e).is_file(),
            "{} is on disk",
            e.file
        );
    }
    assert!(staging_dirs(&s).is_empty());
    assert_eq!(library_of(&s, &visual), saved);
    let id = pack.id.clone();
    assert!(
        visual
            .debug_bounds(Box::leak(format!("mono-emoji-pack-{id}").into_boxed_str()))
            .is_some()
    );
}

#[gpui::test]
fn importing_without_sorting_leaves_everything_unsorted(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("plain.zip")]);
    click(&mut visual, "mono-emoji-import-skip");
    visual.run_until_parked();
    assert!(!dialog_open(&s, &visual));
    let saved = disk(&s);
    assert_eq!(saved.packs.len(), 1);
    assert_eq!(saved.packs[0].sorted_by, None);
    assert_eq!(saved.packs[0].emoji.len(), 4);
    assert!(saved.packs[0].emoji.iter().all(|e| e.emotion.is_none()));
    assert!(staging_dirs(&s).is_empty());
}

#[gpui::test]
fn cancel_saves_nothing_and_leaves_no_staging_folder(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(
        &s,
        &mut visual,
        vec![make_zip("one.zip"), make_zip("two.zip")],
    );
    assert_eq!(staging_dirs(&s).len(), 1);
    click(&mut visual, "mono-emoji-import-cancel");
    visual.run_until_parked();
    assert!(!dialog_open(&s, &visual));
    assert!(staging_dirs(&s).is_empty());
    assert!(!emoji_dir(&s).join("emoji.json").exists());
    assert!(library_of(&s, &visual).packs.is_empty());
    // The second zip never started: Cancel stops the whole queue.
    assert!(!emoji_dir(&s).join("packs").exists());
}

#[gpui::test]
fn without_a_model_the_dialog_suggests_from_file_names_and_says_so(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    let dir = std::env::temp_dir().join(format!("emoji-zips-{}", mono::new_id()));
    let zip = dir.join("names.zip");
    test_support::zip(
        &zip,
        &[
            ("happy-face.png", &test_support::png(4, 4, 10)),
            ("so-sad.png", &test_support::png(4, 4, 70)),
            ("mystery.png", &test_support::png(4, 4, 130)),
        ],
    );
    import(&s, &mut visual, vec![zip]);
    assert!(visual.debug_bounds("mono-emoji-import-notice").is_some());
    assert!(visual.debug_bounds("mono-emoji-import-unsorted").is_some());
    let names = s.manager.read_with(&visual, |m, _| {
        matches!(
            m.emoji_ui
                .import
                .as_ref()
                .unwrap()
                .step
                .as_ref()
                .unwrap()
                .review
                .as_ref()
                .unwrap()
                .source,
            Source::Names { .. }
        )
    });
    assert!(names);
    click(&mut visual, "mono-emoji-import-use");
    visual.run_until_parked();
    let saved = disk(&s);
    assert_eq!(
        saved.packs[0].sorted_by, None,
        "a names suggestion is not model sorting"
    );
    assert_eq!(names_in(&saved, "happy"), ["happy-face"]);
    assert_eq!(names_in(&saved, "sad"), ["so-sad"]);
    assert!(saved.packs[0].emoji.iter().any(|e| e.emotion.is_none()));
}

#[gpui::test]
fn a_bad_zip_is_reported_and_the_queue_moves_on(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    let dir = std::env::temp_dir().join(format!("emoji-zips-{}", mono::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let broken = dir.join("broken.zip");
    std::fs::write(&broken, b"this is not a zip").unwrap();
    let note = dir.join("notes.txt");
    std::fs::write(&note, b"hello").unwrap();
    import(&s, &mut visual, vec![note, broken, make_zip("good.zip")]);
    // The text file is skipped and the broken zip fails; the good one reaches the review.
    click(&mut visual, "mono-emoji-import-use");
    visual.run_until_parked();
    assert_eq!(disk(&s).packs.len(), 1);
    // The dialog stays to report what did not import.
    assert!(dialog_open(&s, &visual));
    let states = s.manager.read_with(&visual, |m, _| {
        m.emoji_ui
            .import
            .as_ref()
            .unwrap()
            .queue
            .iter()
            .map(|i| i.state.clone())
            .collect::<Vec<_>>()
    });
    assert!(matches!(states[0], emoji_import::ItemState::Skipped(_)));
    assert!(matches!(states[1], emoji_import::ItemState::Failed(_)));
    assert_eq!(states[2], emoji_import::ItemState::Saved);
    click(&mut visual, "mono-emoji-import-close");
    assert!(!dialog_open(&s, &visual));
    assert!(staging_dirs(&s).is_empty());
}

#[gpui::test]
fn dropping_a_zip_on_the_page_starts_the_import(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    let zip = make_zip("dropped.zip");
    let notes = zip.with_file_name("notes.txt");
    std::fs::write(&notes, b"x").unwrap();
    let spot = visual
        .debug_bounds("mono-emoji-drop-hint")
        .unwrap()
        .center();
    let paths = ExternalPaths(vec![zip, notes].into());
    visual.simulate_event(FileDropEvent::Entered {
        position: spot,
        paths,
    });
    visual.simulate_event(FileDropEvent::Pending { position: spot });
    visual.refresh().unwrap();
    assert!(s.manager.read_with(&visual, |m, _| m.emoji_ui.drop_over));
    visual.simulate_event(FileDropEvent::Submit { position: spot });
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(dialog_open(&s, &visual));
    assert!(!s.manager.read_with(&visual, |m, _| m.emoji_ui.drop_over));
    let (names, states) = s.manager.read_with(&visual, |m, _| {
        let d = m.emoji_ui.import.as_ref().unwrap();
        (
            d.queue.iter().map(|i| i.name.clone()).collect::<Vec<_>>(),
            d.queue.iter().map(|i| i.state.clone()).collect::<Vec<_>>(),
        )
    });
    assert_eq!(names, ["dropped.zip", "notes.txt"]);
    assert!(
        matches!(states[1], emoji_import::ItemState::Skipped(_)),
        "the text file is reported"
    );
    click(&mut visual, "mono-emoji-import-cancel");
    assert!(staging_dirs(&s).is_empty());
}

#[gpui::test]
fn dragging_a_chip_to_another_emotion_moves_it(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    let chip: &'static str = "mono-emoji-chip-pack-seed-a.png";
    let from = visual
        .debug_bounds(chip)
        .expect("the chip is drawn")
        .center();
    visual.simulate_mouse_down(from, gpui::MouseButton::Left, Default::default());
    visual.simulate_mouse_move(
        from + gpui::point(px(8.), px(0.)),
        Some(gpui::MouseButton::Left),
        Default::default(),
    );
    visual.run_until_parked();
    let to = visual
        .debug_bounds("mono-emoji-emotion-celebrate")
        .unwrap()
        .center();
    visual.simulate_mouse_move(to, Some(gpui::MouseButton::Left), Default::default());
    visual.run_until_parked();
    visual.simulate_mouse_up(to, gpui::MouseButton::Left, Default::default());
    visual.run_until_parked();
    let saved = disk(&s);
    assert_eq!(names_in(&saved, "celebrate"), ["alpha"]);
    assert_eq!(names_in(&saved, "happy"), ["bravo"]);
}

#[gpui::test]
fn deleting_an_emotion_with_emoji_asks_and_moves_them_to_unsorted(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    click(&mut visual, "mono-emoji-delete-happy");
    assert!(
        visual.pending_prompt().is_some(),
        "a non empty emotion asks first"
    );
    visual.simulate_prompt_answer("Cancel");
    visual.run_until_parked();
    assert!(disk(&s).emotions.iter().any(|e| e.id == "happy"));
    click(&mut visual, "mono-emoji-delete-happy");
    visual.simulate_prompt_answer("Delete");
    visual.run_until_parked();
    let saved = disk(&s);
    assert!(saved.emotions.iter().all(|e| e.id != "happy"));
    assert_eq!(saved.emotions.len(), 12);
    assert!(saved.packs[0].emoji.iter().all(|e| e.emotion.is_none()));
    // An empty emotion goes without asking.
    click(&mut visual, "mono-emoji-delete-celebrate");
    assert!(visual.pending_prompt().is_none());
    visual.run_until_parked();
    assert_eq!(disk(&s).emotions.len(), 11);
}

#[gpui::test]
fn an_emotion_can_be_added_and_renamed_and_a_bad_name_shows_an_error(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    scroll_page(&mut visual, 2000.);
    click(&mut visual, "mono-emoji-add-emotion");
    visual.refresh().unwrap();
    let id = disk(&s)
        .emotions
        .iter()
        .find(|e| e.name == "new emotion")
        .map(|e| e.id.clone())
        .expect("the emotion is added and saved");
    let field = s.manager.read_with(&visual, |m, _| {
        m.fields[&format!("emoji-name:{}:{id}", s.id)].clone()
    });
    assert!(
        visual.update(|window, cx| field.focus_handle(cx).is_focused(window)),
        "the new name is focused"
    );
    // A name that another emotion has is refused and the old name stays.
    field.update(&mut visual, |f, cx| f.set_text("HAPPY".into(), cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("mono-emoji-name-error-{id}").into_boxed_str()
            ))
            .is_some()
    );
    let name = |s: &Setup, visual: &VisualTestContext| {
        library_of(s, visual)
            .emotions
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.name.clone())
    };
    assert_eq!(name(&s, &visual).as_deref(), Some("new emotion"));
    // A good name is kept and saved, and the error goes.
    field.update(&mut visual, |f, cx| f.set_text("joyful".into(), cx));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(name(&s, &visual).as_deref(), Some("joyful"));
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("mono-emoji-name-error-{id}").into_boxed_str()
            ))
            .is_none()
    );
    let saved = disk(&s);
    assert_eq!(
        saved.emotions.iter().find(|e| e.id == id).unwrap().name,
        "joyful"
    );
    // The description is saved too.
    let description = s.manager.read_with(&visual, |m, _| {
        m.fields[&format!("emoji-desc:{}:{id}", s.id)].clone()
    });
    description.update(&mut visual, |f, cx| f.set_text("Pure delight".into(), cx));
    visual.run_until_parked();
    assert_eq!(
        disk(&s)
            .emotions
            .iter()
            .find(|e| e.id == id)
            .unwrap()
            .description,
        "Pure delight"
    );
}

#[gpui::test]
fn removing_a_pack_asks_and_deletes_its_folder(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    let folder = emoji::pack_dir(&emoji_dir(&s), "pack-seed");
    assert!(folder.is_dir());
    click(&mut visual, "mono-emoji-remove-pack-seed");
    visual.simulate_prompt_answer("Cancel");
    visual.run_until_parked();
    assert!(folder.is_dir());
    assert_eq!(disk(&s).packs.len(), 1);
    click(&mut visual, "mono-emoji-remove-pack-seed");
    visual.simulate_prompt_answer("Remove");
    visual.run_until_parked();
    assert!(!folder.exists());
    assert!(disk(&s).packs.is_empty());
    assert!(library_of(&s, &visual).packs.is_empty());
}

#[gpui::test]
fn a_pack_can_be_turned_off_and_keeps_its_sorting(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    click(&mut visual, "mono-emoji-pack-switch-pack-seed");
    visual.run_until_parked();
    let saved = disk(&s);
    assert!(!saved.packs[0].enabled);
    assert_eq!(saved.packs[0].sorted_by.as_deref(), Some("Fake model"));
    assert_eq!(names_in(&saved, "happy"), ["alpha", "bravo"]);
    assert!(
        saved.usable_emotions().is_empty(),
        "Mono does not use a pack that is off"
    );
    click(&mut visual, "mono-emoji-pack-switch-pack-seed");
    visual.run_until_parked();
    assert!(disk(&s).packs[0].enabled);
}

#[gpui::test]
fn reset_to_defaults_asks_and_restores_the_thirteen_emotions(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let mut library = Library::default();
    library.emotions.retain(|e| e.id != "sad");
    library.emotions.push(Emotion {
        id: "emo-zany".into(),
        name: "zany".into(),
        description: "Wild".into(),
    });
    seed(
        &s,
        &mut library,
        "pack-seed",
        vec![
            entry("a.png", "alpha", Format::Png, Some("happy")),
            entry("b.png", "bravo", Format::Png, Some("emo-zany")),
        ],
    );
    show(&s, &mut visual);
    click(&mut visual, "mono-emoji-reset");
    visual.simulate_prompt_answer("Cancel");
    visual.run_until_parked();
    assert!(disk(&s).emotions.iter().any(|e| e.id == "emo-zany"));
    click(&mut visual, "mono-emoji-reset");
    visual.simulate_prompt_answer("Reset");
    visual.run_until_parked();
    let saved = disk(&s);
    assert_eq!(saved.emotions.len(), 13);
    assert!(saved.emotions.iter().any(|e| e.id == "sad"));
    assert_eq!(names_in(&saved, "happy"), ["alpha"]);
    assert!(
        saved.packs[0]
            .emoji
            .iter()
            .any(|e| e.name == "bravo" && e.emotion.is_none())
    );
}

#[gpui::test]
fn the_pack_filter_shows_one_pack_and_keeps_the_counts(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let mut library = Library::default();
    seed(
        &s,
        &mut library,
        "pack-one",
        vec![entry("a.png", "alpha", Format::Png, Some("happy"))],
    );
    seed(
        &s,
        &mut library,
        "pack-two",
        vec![entry("b.png", "bravo", Format::Png, Some("happy"))],
    );
    show(&s, &mut visual);
    assert!(
        visual
            .debug_bounds("mono-emoji-chip-pack-one-a.png")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("mono-emoji-chip-pack-two-b.png")
            .is_some()
    );
    click(&mut visual, "mono-emoji-filter");
    visual.refresh().unwrap();
    click(&mut visual, "menu-action-2");
    visual.refresh().unwrap();
    assert!(
        visual
            .debug_bounds("mono-emoji-chip-pack-one-a.png")
            .is_none()
    );
    assert!(
        visual
            .debug_bounds("mono-emoji-chip-pack-two-b.png")
            .is_some()
    );
    assert_eq!(
        s.manager
            .read_with(&visual, |m, _| m.emoji_ui.filter.clone())
            .as_deref(),
        Some("pack-two")
    );
    // Filtering is only a view: nothing is written.
    assert_eq!(disk(&s), library);
}

#[gpui::test]
fn re_sorting_a_pack_replaces_its_sorting_without_unpacking(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    install_fake(&s, vec!["love"], &mut visual);
    click(&mut visual, "mono-emoji-resort-pack-seed");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(dialog_open(&s, &visual));
    assert!(visual.debug_bounds("mono-emoji-step-unpacked").is_none());
    assert!(visual.debug_bounds("mono-emoji-import-skip").is_none());
    assert!(visual.debug_bounds("mono-emoji-review-love").is_some());
    // Nothing changes until the user agrees.
    assert_eq!(names_in(&disk(&s), "happy"), ["alpha", "bravo"]);
    click(&mut visual, "mono-emoji-import-use");
    visual.run_until_parked();
    assert!(!dialog_open(&s, &visual));
    let saved = disk(&s);
    assert_eq!(names_in(&saved, "love"), ["alpha", "bravo", "charlie"]);
    assert_eq!(saved.packs.len(), 1);
    assert!(saved.packs[0].sorted_by.is_some());
    assert!(staging_dirs(&s).is_empty());
}

#[gpui::test]
fn rapid_edits_are_written_one_at_a_time_and_the_last_wins(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    let id = s.id.clone();
    s.manager.update(&mut visual, |m, cx| {
        for (n, frequency) in [
            emoji::Frequency::Rarely,
            emoji::Frequency::Often,
            emoji::Frequency::Rarely,
            emoji::Frequency::Often,
        ]
        .into_iter()
        .enumerate()
        {
            m.emoji_edit(&id, cx, |l| {
                l.frequency = frequency;
                l.enabled = n % 2 == 0;
                Ok(())
            })
            .unwrap();
        }
    });
    visual.run_until_parked();
    let saved = disk(&s);
    assert_eq!(saved.frequency, emoji::Frequency::Often);
    assert!(!saved.enabled);
}

#[gpui::test]
fn the_chat_draws_emoji_tokens_while_the_switch_is_on(cx: &mut TestAppContext) {
    use monocode_core::transcript::Row;
    let (s, mut visual, _) = seeded_page(cx, three());
    click(&mut visual, "mono-emoji-enabled");
    visual.run_until_parked();
    assert!(library_of(&s, &visual).enabled);
    let chat = s.manager.read_with(&visual, |m, _| m.chats[&s.id].clone());
    // A Mono chat shows its transcript once the session has messages.
    chat.update(&mut visual, |c, cx| {
        c.summary.has_messages = true;
        cx.notify();
    });
    let transcript = chat.read_with(&visual, |c, _| c.transcript.clone());
    let row = |role: &str, text: &str| Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    };
    transcript.update(&mut visual, |t, cx| {
        t.patch(
            0,
            vec![row("user", "hi"), row("assistant", "Nice :emo[happy] work")],
            cx,
        )
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    let rows = transcript.read_with(&visual, |t, _| t.source_rows().len());
    assert_eq!(rows, 2, "the patched rows are still there");
    assert!(
        visual.debug_bounds("mono-emoji-happy").is_some(),
        "the chat draws the picture"
    );
    click(&mut visual, "mono-emoji-enabled");
    visual.run_until_parked();
    assert!(!library_of(&s, &visual).enabled);
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds("mono-emoji-happy").is_none(),
        "switching it off removes the picture"
    );
}
