//! The shared emoji library page (Settings > Monos > Emoji) and its import dialog. The model is
//! never called: sorting runs through a fake runner, or falls back to file names.
use super::emoji_tests::{data_dir, disk, emoji_dir, entry, library_of, open, reload, seed, store};
use super::workflows_ui::tests::click;
use super::*;
use crate::emoji_library::{EmojiLibraryView, ItemState, SortParts, names_notice};
use crate::monos::tests::Setup;
use gpui::{ExternalPaths, FileDropEvent, TestAppContext, VisualTestContext};
use monocode_core::mono::emoji::{
    self, Emotion, Format, Library,
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

/// Opens Settings > Monos > Emoji in a window tall enough to show the whole page.
fn show(s: &Setup, visual: &mut VisualTestContext) {
    reload(s, visual);
    s.manager.update(visual, |m, cx| {
        m.settings = None;
        cx.notify();
    });
    visual.simulate_resize(gpui::size(px(1300.), px(3400.)));
    s.window
        .update(visual, |shell, window, cx| {
            shell.open_emoji_library_for_test(window, cx)
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn view(s: &Setup, visual: &VisualTestContext) -> gpui::Entity<EmojiLibraryView> {
    s.window
        .read_with(visual, |shell, cx| {
            shell
                .settings_view()
                .and_then(|v| v.read(cx).emoji_library_view())
        })
        .unwrap()
        .expect("the library page exists")
}

fn install_fake(s: &Setup, answers: Vec<&'static str>, visual: &mut VisualTestContext) {
    view(s, visual).update(visual, |v, _| {
        v.set_sorter(Arc::new(move |_| {
            Ok(Some(
                Arc::new(Fake(answers.clone())) as Arc<dyn sort::Runner>
            ))
        }));
    });
}

fn seeded_page(
    cx: &mut TestAppContext,
    emoji: Vec<emoji::Emoji>,
) -> (Setup, VisualTestContext, Library) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let mut library = Library::default();
    seed(&s, &mut library, "pack-seed", emoji);
    show(&s, &mut visual);
    (s, visual, library)
}

fn three() -> Vec<emoji::Emoji> {
    vec![
        entry("a.png", "alpha", Format::Png, Some("happy")),
        entry("b.png", "bravo", Format::Png, Some("happy")),
        entry("c.gif", "charlie", Format::Gif, None),
    ]
}

fn dialog_open(s: &Setup, visual: &VisualTestContext) -> bool {
    view(s, visual).read_with(visual, |v, _| v.dialog().is_some())
}

/// A folder for test zips that is removed when the test's thread ends.
struct ZipDir(PathBuf);

impl Drop for ZipDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

thread_local! {
    static ZIP_DIRS: std::cell::RefCell<Vec<ZipDir>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn zip_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emoji-zips-{}", mono::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    ZIP_DIRS.with(|dirs| dirs.borrow_mut().push(ZipDir(dir.clone())));
    dir
}

fn make_zip(name: &str) -> PathBuf {
    let zip = zip_dir().join(name);
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
    click(visual, "emoji-lib-import");
    assert!(visual.did_prompt_for_paths());
    visual.simulate_path_prompt_response(move |_| Some(zips.clone()));
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(dialog_open(s, visual), "the import dialog opens");
}

/// The reason of the names fallback of the zip under review.
fn names_reason(s: &Setup, visual: &VisualTestContext) -> String {
    view(s, visual).read_with(visual, |v, _| {
        let step = v.dialog().unwrap().step.as_ref().unwrap();
        match &step.review.as_ref().unwrap().source {
            Source::Names { reason } => reason.clone(),
            Source::Model { label } => panic!("sorted by {label}"),
        }
    })
}

/// Lets typed text rest long enough to be written, then runs the write.
fn settle(visual: &mut VisualTestContext) {
    visual
        .executor()
        .advance_clock(crate::emoji_library::SAVE_DELAY + std::time::Duration::from_millis(50));
    visual.run_until_parked();
}

fn staging_dirs(s: &Setup) -> Vec<PathBuf> {
    crate::emoji_library::wait_for_discards();
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

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[gpui::test]
fn the_library_page_is_reachable_from_settings_monos(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    show(&s, &mut visual);
    let section = s
        .window
        .read_with(&visual, |shell, cx| {
            shell.settings_view().map(|v| v.read(cx).section)
        })
        .unwrap();
    assert_eq!(section, Some(crate::settings::Section::Monos));
    for selector in [
        "settings-emoji-library",
        "emoji-lib",
        "emoji-lib-import",
        "emoji-lib-pack-summary",
        "emoji-lib-drop-hint",
        "emoji-lib-board",
        "emoji-lib-filter",
        "emoji-lib-reset",
        "emoji-lib-add-emotion",
        "emoji-lib-emotion-happy",
        "emoji-lib-emotion-unsorted",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // Packs have no switch any more: a Mono chooses its packs.
    assert!(visual.debug_bounds("emoji-lib-enabled").is_none());
    assert_eq!(library_of(&s, &visual).emotions.len(), 13);
    // Nothing is written until something changes.
    assert!(!emoji_dir(&s).join("emoji.json").exists());
}

#[gpui::test]
fn a_newer_file_is_shown_read_only_and_never_written(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let dir = emoji_dir(&s);
    std::fs::create_dir_all(&dir).unwrap();
    let newer = "{\"version\": 99}";
    std::fs::write(dir.join("emoji.json"), newer).unwrap();
    show(&s, &mut visual);
    assert!(visual.debug_bounds("emoji-lib-read-only").is_some());
    click(&mut visual, "emoji-lib-add-emotion");
    visual.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(dir.join("emoji.json")).unwrap(),
        newer
    );
    assert_eq!(library_of(&s, &visual).emotions.len(), 13);
}

#[gpui::test]
fn a_zip_is_unpacked_sorted_reviewed_and_saved_into_the_shared_library(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy", "sad", "happy", "celebrate"], &mut visual);
    let zip = make_zip("pepe-pack.zip");
    import(&s, &mut visual, vec![zip]);
    for selector in [
        "emoji-lib-import-dialog",
        "emoji-lib-import-model",
        "emoji-lib-step-unpacked",
        "emoji-lib-step-kept",
        "emoji-lib-step-sorting",
        "emoji-lib-review",
        "emoji-lib-review-happy",
        "emoji-lib-review-chill",
        "emoji-lib-review-unsorted",
        "emoji-lib-import-use",
        "emoji-lib-import-skip",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(visual.debug_bounds("emoji-lib-import-notice").is_none());
    assert!(visual.debug_bounds("emoji-lib-import-subtitle").is_some());
    let label = view(&s, &visual).read_with(&visual, |v, _| {
        let model = v.dialog().unwrap().model().clone();
        crate::pickers::cached_model_name(model.kind, model.model.as_deref()).to_string()
    });
    // Nothing is saved before the user decides.
    assert!(!emoji_dir(&s).join("emoji.json").exists());
    assert_eq!(staging_dirs(&s).len(), 1);
    click(&mut visual, "emoji-lib-import-use");
    visual.run_until_parked();
    assert!(!dialog_open(&s, &visual), "the queue is done");
    let saved = disk(&s);
    assert_eq!(saved.packs.len(), 1);
    let pack = &saved.packs[0];
    assert_eq!(pack.name, "pepe-pack");
    assert_eq!(pack.emoji.len(), 4);
    assert_eq!(
        pack.sorted_by.as_deref(),
        Some(label.as_str()),
        "the pack records the model that sorted it"
    );
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
    // The library is under the app data folder, shared by every Mono.
    assert_eq!(emoji_dir(&s), data_dir(&s).join("emoji"));
    assert!(staging_dirs(&s).is_empty());
    assert_eq!(library_of(&s, &visual), saved);
    let id = pack.id.clone();
    assert!(
        visual
            .debug_bounds(leak(format!("emoji-lib-pack-{id}")))
            .is_some()
    );
}

#[gpui::test]
fn importing_without_sorting_leaves_everything_unsorted(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("plain.zip")]);
    click(&mut visual, "emoji-lib-import-skip");
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
    click(&mut visual, "emoji-lib-import-cancel");
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
    let zip = zip_dir().join("names.zip");
    test_support::zip(
        &zip,
        &[
            ("happy-face.png", &test_support::png(4, 4, 10)),
            ("so-sad.png", &test_support::png(4, 4, 70)),
            ("mystery.png", &test_support::png(4, 4, 130)),
        ],
    );
    import(&s, &mut visual, vec![zip]);
    assert!(visual.debug_bounds("emoji-lib-import-notice").is_some());
    assert!(visual.debug_bounds("emoji-lib-import-unsorted").is_some());
    let label = view(&s, &visual).read_with(&visual, |v, _| {
        let model = v.dialog().unwrap().model().clone();
        crate::pickers::cached_model_name(model.kind, model.model.as_deref()).to_string()
    });
    assert_eq!(
        names_reason(&s, &visual),
        format!("{label} cannot sort by image"),
        "a provider that cannot read images keeps its wording"
    );
    assert_eq!(
        names_notice(&names_reason(&s, &visual)),
        format!("{label} cannot sort by image, so emoji were suggested from their file names.")
    );
    click(&mut visual, "emoji-lib-import-use");
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
    let dir = zip_dir();
    let broken = dir.join("broken.zip");
    std::fs::write(&broken, b"this is not a zip").unwrap();
    let note = dir.join("notes.txt");
    std::fs::write(&note, b"hello").unwrap();
    import(&s, &mut visual, vec![note, broken, make_zip("good.zip")]);
    // The text file is skipped and the broken zip fails; the good one reaches the review.
    click(&mut visual, "emoji-lib-import-use");
    visual.run_until_parked();
    assert_eq!(disk(&s).packs.len(), 1);
    // The dialog stays to report what did not import.
    assert!(dialog_open(&s, &visual));
    let states = view(&s, &visual).read_with(&visual, |v, _| {
        v.dialog()
            .unwrap()
            .queue
            .iter()
            .map(|i| i.state.clone())
            .collect::<Vec<_>>()
    });
    assert!(matches!(states[0], ItemState::Skipped(_)));
    assert!(matches!(states[1], ItemState::Failed(_)));
    assert_eq!(states[2], ItemState::Saved);
    click(&mut visual, "emoji-lib-import-close");
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
    let spot = visual.debug_bounds("emoji-lib-drop-hint").unwrap().center();
    let paths = ExternalPaths(vec![zip, notes].into());
    visual.simulate_event(FileDropEvent::Entered {
        position: spot,
        paths,
    });
    visual.simulate_event(FileDropEvent::Pending { position: spot });
    visual.refresh().unwrap();
    assert!(view(&s, &visual).read_with(&visual, |v, _| v.drop_over()));
    visual.simulate_event(FileDropEvent::Submit { position: spot });
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(dialog_open(&s, &visual));
    assert!(!view(&s, &visual).read_with(&visual, |v, _| v.drop_over()));
    let (names, states) = view(&s, &visual).read_with(&visual, |v, _| {
        let d = v.dialog().unwrap();
        (
            d.queue.iter().map(|i| i.name.clone()).collect::<Vec<_>>(),
            d.queue.iter().map(|i| i.state.clone()).collect::<Vec<_>>(),
        )
    });
    assert_eq!(names, ["dropped.zip", "notes.txt"]);
    assert!(
        matches!(states[1], ItemState::Skipped(_)),
        "the text file is reported"
    );
    click(&mut visual, "emoji-lib-import-cancel");
    assert!(staging_dirs(&s).is_empty());
}

#[gpui::test]
fn dragging_a_chip_to_another_emotion_moves_it(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    let chip: &'static str = "emoji-lib-chip-pack-seed-a.png";
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
        .debug_bounds("emoji-lib-emotion-celebrate")
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
    click(&mut visual, "emoji-lib-delete-happy");
    assert!(
        visual.pending_prompt().is_some(),
        "a non empty emotion asks first"
    );
    visual.simulate_prompt_answer("Cancel");
    visual.run_until_parked();
    assert!(disk(&s).emotions.iter().any(|e| e.id == "happy"));
    click(&mut visual, "emoji-lib-delete-happy");
    visual.simulate_prompt_answer("Delete");
    visual.run_until_parked();
    let saved = disk(&s);
    assert!(saved.emotions.iter().all(|e| e.id != "happy"));
    assert_eq!(saved.emotions.len(), 12);
    assert!(saved.packs[0].emoji.iter().all(|e| e.emotion.is_none()));
    // An empty emotion goes without asking.
    click(&mut visual, "emoji-lib-delete-celebrate");
    assert!(visual.pending_prompt().is_none());
    visual.run_until_parked();
    assert_eq!(disk(&s).emotions.len(), 11);
}

#[gpui::test]
fn an_emotion_can_be_added_and_renamed_and_a_bad_name_shows_an_error(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    click(&mut visual, "emoji-lib-add-emotion");
    visual.refresh().unwrap();
    let id = disk(&s)
        .emotions
        .iter()
        .find(|e| e.name == "new emotion")
        .map(|e| e.id.clone())
        .expect("the emotion is added and saved");
    let field = view(&s, &visual).read_with(&visual, |v, _| {
        v.field_for_test(&format!("emoji-name:{id}"))
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
            .debug_bounds(leak(format!("emoji-lib-name-error-{id}")))
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
    settle(&mut visual);
    assert_eq!(name(&s, &visual).as_deref(), Some("joyful"));
    assert!(
        visual
            .debug_bounds(leak(format!("emoji-lib-name-error-{id}")))
            .is_none()
    );
    let saved = disk(&s);
    assert_eq!(
        saved.emotions.iter().find(|e| e.id == id).unwrap().name,
        "joyful"
    );
    // The description is saved too.
    let description = view(&s, &visual).read_with(&visual, |v, _| {
        v.field_for_test(&format!("emoji-desc:{id}"))
    });
    description.update(&mut visual, |f, cx| f.set_text("Pure delight".into(), cx));
    visual.run_until_parked();
    settle(&mut visual);
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
fn removing_a_pack_asks_deletes_its_folder_and_drops_it_from_every_mono(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let mut library = Library::default();
    seed(
        &s,
        &mut library,
        "pack-seed",
        vec![entry("a.png", "alpha", Format::Png, Some("happy"))],
    );
    seed(
        &s,
        &mut library,
        "pack-other",
        vec![entry("b.png", "bravo", Format::Png, Some("sad"))],
    );
    // A second Mono, so the pack is chosen by two of them.
    let handle: gpui::AnyWindowHandle = s.window.into();
    let project = s.project.clone();
    handle
        .update(&mut visual, |_, w, cx| {
            s.manager
                .update(cx, |m, cx| m.create(vec![project.clone()], w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    let ids: Vec<String> = s.manager.read_with(&visual, |m, _| {
        m.roster.monos.iter().map(|m| m.id.clone()).collect()
    });
    assert_eq!(ids.len(), 2);
    s.manager.update(&mut visual, |m, cx| {
        for mono in &mut m.roster.monos {
            mono.emoji.enabled = true;
            mono.emoji.packs = vec!["pack-seed".into(), "pack-other".into()];
        }
        m.save(cx);
    });
    visual.run_until_parked();
    show(&s, &mut visual);
    let folder = emoji::pack_dir(&emoji_dir(&s), "pack-seed");
    assert!(folder.is_dir());
    click(&mut visual, "emoji-lib-remove-pack-seed");
    visual.simulate_prompt_answer("Cancel");
    visual.run_until_parked();
    assert!(folder.is_dir());
    assert_eq!(disk(&s).packs.len(), 2);
    click(&mut visual, "emoji-lib-remove-pack-seed");
    visual.simulate_prompt_answer("Remove");
    visual.run_until_parked();
    assert!(!folder.exists());
    assert_eq!(disk(&s).packs.len(), 1);
    assert_eq!(library_of(&s, &visual).packs.len(), 1);
    // Both Monos lost the id, in memory and in the saved roster.
    s.manager.read_with(&visual, |m, _| {
        for id in &ids {
            assert_eq!(m.roster.get(id).unwrap().emoji.packs, ["pack-other"]);
        }
    });
    let db = locked(&s.store).unwrap();
    let saved = mono::Roster::load(&db).unwrap();
    for id in &ids {
        assert_eq!(saved.get(id).unwrap().emoji.packs, ["pack-other"]);
        assert!(saved.get(id).unwrap().emoji.enabled);
    }
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
    click(&mut visual, "emoji-lib-reset");
    visual.simulate_prompt_answer("Cancel");
    visual.run_until_parked();
    assert!(disk(&s).emotions.iter().any(|e| e.id == "emo-zany"));
    click(&mut visual, "emoji-lib-reset");
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
            .debug_bounds("emoji-lib-chip-pack-one-a.png")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("emoji-lib-chip-pack-two-b.png")
            .is_some()
    );
    click(&mut visual, "emoji-lib-filter");
    visual.refresh().unwrap();
    assert!(view(&s, &visual).read_with(&visual, |v, _| v.select_is_open()));
    click(&mut visual, "menu-action-2");
    visual.refresh().unwrap();
    assert!(
        visual
            .debug_bounds("emoji-lib-chip-pack-one-a.png")
            .is_none()
    );
    assert!(
        visual
            .debug_bounds("emoji-lib-chip-pack-two-b.png")
            .is_some()
    );
    assert_eq!(
        view(&s, &visual)
            .read_with(&visual, |v, _| v.filter())
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
    click(&mut visual, "emoji-lib-resort-pack-seed");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(dialog_open(&s, &visual));
    assert!(visual.debug_bounds("emoji-lib-step-unpacked").is_none());
    assert!(visual.debug_bounds("emoji-lib-import-skip").is_none());
    assert!(visual.debug_bounds("emoji-lib-review-love").is_some());
    // Nothing changes until the user agrees.
    assert_eq!(names_in(&disk(&s), "happy"), ["alpha", "bravo"]);
    click(&mut visual, "emoji-lib-import-use");
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
    store(&s, &visual).update(&mut visual, |store, cx| {
        for text in ["one", "two", "three", "four"] {
            store
                .edit(cx, |l| {
                    l.describe_emotion("happy", text);
                    l.describe_emotion("sad", text);
                    Ok(())
                })
                .unwrap();
        }
    });
    visual.run_until_parked();
    let saved = disk(&s);
    assert_eq!(saved.emotions[0].description, "four");
    assert_eq!(saved.emotions[4].description, "four");
}

fn desc_field(
    s: &Setup,
    visual: &VisualTestContext,
    emotion: &str,
) -> gpui::Entity<crate::input::Input> {
    view(s, visual).read_with(visual, |v, _| {
        v.field_for_test(&format!("emoji-desc:{emotion}"))
    })
}

#[gpui::test]
fn typing_a_description_waits_before_it_is_saved(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    let field = desc_field(&s, &visual, "happy");
    field.update(&mut visual, |f, cx| f.set_text("Typing".into(), cx));
    visual.run_until_parked();
    field.update(&mut visual, |f, cx| f.set_text("Typing on".into(), cx));
    visual.run_until_parked();
    assert_ne!(
        disk(&s).emotions[0].description,
        "Typing on",
        "nothing is written while typing"
    );
    assert_eq!(library_of(&s, &visual).emotions[0].description, "Typing on");
    settle(&mut visual);
    assert_eq!(disk(&s).emotions[0].description, "Typing on");
}

#[gpui::test]
fn flushing_a_field_writes_what_was_typed_at_once(cx: &mut TestAppContext) {
    // Leaving a field calls this; GPUI's test window does not deliver the focus-out event.
    let (s, mut visual, _) = seeded_page(cx, three());
    let field = desc_field(&s, &visual, "happy");
    field.update(&mut visual, |f, cx| f.set_text("Quick".into(), cx));
    visual.run_until_parked();
    assert_ne!(disk(&s).emotions[0].description, "Quick");
    store(&s, &visual).update(&mut visual, |store, cx| store.flush(cx));
    visual.run_until_parked();
    assert_eq!(disk(&s).emotions[0].description, "Quick");
}

#[gpui::test]
fn an_edit_made_while_a_read_is_pending_is_not_overwritten(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    store(&s, &visual).update(&mut visual, |store, cx| {
        store.load(true, cx);
        store
            .edit(cx, |l| {
                l.describe_emotion("happy", "Kept");
                Ok(())
            })
            .unwrap();
    });
    visual.run_until_parked();
    assert_eq!(
        library_of(&s, &visual).emotions[0].description,
        "Kept",
        "the read did not undo it"
    );
    assert_eq!(disk(&s).emotions[0].description, "Kept", "and it was saved");
}

#[gpui::test]
fn a_read_refused_while_the_library_is_busy_runs_when_it_is_free(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    let field = desc_field(&s, &visual, "happy");
    field.update(&mut visual, |f, cx| f.set_text("Busy".into(), cx));
    let store = store(&s, &visual);
    store.update(&mut visual, |store, cx| store.load(true, cx));
    assert_eq!(
        store.read_with(&visual, |store, _| store.pending_read()),
        Some(true),
        "the read waits for the library"
    );
    settle(&mut visual);
    assert_eq!(
        store.read_with(&visual, |store, _| store.pending_read()),
        None,
        "it was started once the write was done"
    );
    assert_eq!(library_of(&s, &visual).emotions[0].description, "Busy");
}

#[gpui::test]
fn the_first_read_cleans_stale_folders_and_a_later_one_leaves_an_open_import(
    cx: &mut TestAppContext,
) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    let dir = emoji_dir(&s);
    std::fs::create_dir_all(dir.join(".staging-old")).unwrap();
    std::fs::create_dir_all(dir.join("sort-old")).unwrap();
    // As if the app had just started: the library has not been read yet.
    let store = store(&s, &visual);
    store.update(&mut visual, |store, _| store.forget_cleaned());
    store.update(&mut visual, |store, cx| store.load(true, cx));
    visual.run_until_parked();
    assert!(!dir.join(".staging-old").exists());
    assert!(!dir.join("sort-old").exists());
    // Another read, such as the one a turn makes, takes nothing away.
    std::fs::create_dir_all(dir.join(".staging-open")).unwrap();
    store.update(&mut visual, |store, cx| store.load(true, cx));
    visual.run_until_parked();
    assert!(dir.join(".staging-open").exists());
}

#[gpui::test]
fn only_the_chip_under_the_pointer_plays_its_gif(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    let selector = "emoji-lib-chip-pack-seed-c.gif";
    let chip = visual.debug_bounds(selector).expect("the chip is drawn");
    assert_eq!(
        view(&s, &visual).read_with(&visual, |v, _| v.hovered_chip()),
        None
    );
    visual.simulate_mouse_move(chip.center(), None, gpui::Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        view(&s, &visual).read_with(&visual, |v, _| v.hovered_chip()),
        Some(selector.to_string())
    );
    visual.simulate_mouse_move(
        gpui::point(px(1.), px(1.)),
        None,
        gpui::Modifiers::default(),
    );
    visual.run_until_parked();
    assert_eq!(
        view(&s, &visual).read_with(&visual, |v, _| v.hovered_chip()),
        None
    );
}

/// Records the parts the factory is given and answers that there is no model.
fn record_parts(s: &Setup, cx: &mut VisualTestContext) -> Arc<std::sync::Mutex<Vec<SortParts>>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    view(s, cx).update(cx, |v, _| {
        v.set_sorter(Arc::new(move |parts| {
            sink.lock().unwrap().push(parts);
            Ok(None)
        }));
    });
    seen
}

#[gpui::test]
fn the_dialog_starts_on_the_model_of_the_mono_whose_settings_were_open(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    // The Mono runs on a model that is not the app default (Codex), so only remembering it
    // when its settings open can make the dialog start on it.
    visual.update(|_, cx| {
        let mut prefs = cx
            .try_global::<crate::settings::Preferences>()
            .cloned()
            .unwrap_or_default();
        prefs.default_provider = Some("codex".into());
        cx.set_global(prefs);
    });
    s.manager
        .update(&mut visual, |m, cx| {
            m.chats[&s.id].update(cx, |chat, cx| {
                chat.mono_config(
                    Kind::Claude,
                    Some("claude-test".into()),
                    std::collections::HashMap::new(),
                    cx,
                )
            })
        })
        .unwrap();
    store(&s, &visual).update(&mut visual, |store, _| store.forget_model());
    // The Mono's settings are open on the Emoji page: its model is remembered.
    click(&mut visual, "mono-settings-Emoji");
    let (kind, model, account) = s.manager.read_with(&visual, |m, cx| {
        let chat = m.chats[&s.id].read(cx);
        (
            Kind::from_id(&chat.summary.provider),
            chat.mono_model(),
            chat.settings.account.clone(),
        )
    });
    assert_eq!(kind, Kind::Claude);
    assert_eq!(model.as_deref(), Some("claude-test"));
    show(&s, &mut visual);
    let seen = record_parts(&s, &mut visual);
    import(&s, &mut visual, vec![make_zip("parts.zip")]);
    let dialog = view(&s, &visual).read_with(&visual, |v, _| v.dialog().unwrap().model().clone());
    assert_eq!(dialog.kind, kind);
    assert_eq!(dialog.model, model);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "one sorting run asked for a runner");
    let parts = &seen[0];
    assert_eq!(parts.kind, kind);
    assert_eq!(parts.model, model);
    assert_eq!(parts.settings.account.id, account.id);
    assert_eq!(parts.cwd, data_dir(&s).to_string_lossy());
    assert!(parts.settings.text_only);
    assert_eq!(
        parts.settings.access,
        monocode_core::agent::AccessMode::ReadOnly
    );
}

#[gpui::test]
fn without_a_mono_model_the_app_default_provider_and_model_are_used(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    show(&s, &mut visual);
    store(&s, &visual).update(&mut visual, |store, _| store.forget_model());
    visual.update(|_, cx| {
        let mut prefs = cx
            .try_global::<crate::settings::Preferences>()
            .cloned()
            .unwrap_or_default();
        prefs.default_provider = Some("claude".into());
        prefs
            .default_models
            .insert("claude".into(), "claude-opus-5-5".into());
        cx.set_global(prefs);
    });
    let seen = record_parts(&s, &mut visual);
    import(&s, &mut visual, vec![make_zip("default.zip")]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].kind, Kind::Claude);
    assert_eq!(seen[0].model.as_deref(), Some("claude-opus-5-5"));
}

#[gpui::test]
fn picking_another_model_sorts_the_open_zip_again_with_it(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    show(&s, &mut visual);
    store(&s, &visual).update(&mut visual, |store, _| store.forget_model());
    let seen = record_parts(&s, &mut visual);
    import(&s, &mut visual, vec![make_zip("again.zip")]);
    assert_eq!(seen.lock().unwrap().len(), 1);
    // The shared model list opens under the dialog's model button.
    click(&mut visual, "emoji-lib-import-model");
    visual.refresh().unwrap();
    let list = view(&s, &visual)
        .read_with(&visual, |v, _| v.model_list())
        .expect("the shared model list opens");
    list.update(&mut visual, |_, cx| {
        cx.emit(crate::pickers::ModelPicked(
            Kind::Claude,
            Some("claude-test".into()),
        ));
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "the zip was sorted again");
        assert_eq!(seen[1].kind, Kind::Claude);
        assert_eq!(seen[1].model.as_deref(), Some("claude-test"));
    }
    let model = view(&s, &visual).read_with(&visual, |v, _| v.dialog().unwrap().model().clone());
    assert_eq!(model.kind, Kind::Claude);
    assert_eq!(model.model.as_deref(), Some("claude-test"));
    assert!(
        view(&s, &visual)
            .read_with(&visual, |v, _| v.model_list())
            .is_none(),
        "the list closes after a pick"
    );
    // An event of the first run, arriving late, changes nothing.
    let library_view = view(&s, &visual);
    visual.update(|window, cx| {
        library_view.update(cx, |v, cx| v.stale_event_for_test(window, cx));
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    let state =
        view(&s, &visual).read_with(&visual, |v, _| v.dialog().unwrap().queue[0].state.clone());
    assert_eq!(state, ItemState::Review, "a stale run's event is dropped");
    // The new sorting can be reviewed and saved.
    assert!(visual.debug_bounds("emoji-lib-review").is_some());
    click(&mut visual, "emoji-lib-import-skip");
    visual.run_until_parked();
    assert_eq!(disk(&s).packs.len(), 1);
}

#[gpui::test]
fn a_re_sort_uses_the_picked_model_too(cx: &mut TestAppContext) {
    let (s, mut visual, _) = seeded_page(cx, three());
    store(&s, &visual).update(&mut visual, |store, _| store.forget_model());
    let seen = record_parts(&s, &mut visual);
    click(&mut visual, "emoji-lib-resort-pack-seed");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    click(&mut visual, "emoji-lib-import-model");
    visual.refresh().unwrap();
    let list = view(&s, &visual)
        .read_with(&visual, |v, _| v.model_list())
        .unwrap();
    list.update(&mut visual, |_, cx| {
        cx.emit(crate::pickers::ModelPicked(
            Kind::Cursor,
            Some("cursor-test".into()),
        ));
    });
    visual.run_until_parked();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].kind, Kind::Cursor);
    assert_eq!(seen[1].model.as_deref(), Some("cursor-test"));
}

#[gpui::test]
fn the_notice_says_why_the_model_was_not_asked(cx: &mut TestAppContext) {
    // A model that cannot be started: the error is the reason.
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    view(&s, &visual).update(&mut visual, |v, _| {
        v.set_sorter(Arc::new(|_| Err("The CLI is missing.".into())));
    });
    import(&s, &mut visual, vec![make_zip("a.zip")]);
    let label = view(&s, &visual).read_with(&visual, |v, _| {
        let model = v.dialog().unwrap().model().clone();
        crate::pickers::cached_model_name(model.kind, model.model.as_deref()).to_string()
    });
    assert_eq!(
        names_notice(&names_reason(&s, &visual)),
        format!(
            "{label} is not available: The CLI is missing, so emoji were suggested from their file names."
        )
    );
}

#[test]
fn the_dialog_texts_do_not_name_a_mono() {
    use crate::emoji_library::dialog_texts::{sorting_line, subtitle};
    assert_eq!(
        subtitle(false, "", 1),
        "1 file \u{b7} into your emoji library"
    );
    assert_eq!(
        subtitle(false, "", 3),
        "3 files \u{b7} into your emoji library"
    );
    assert_eq!(subtitle(true, "Pepe", 1), "Pepe \u{b7} your emoji library");
    assert_eq!(
        sorting_line("Opus 5.5", 3, 12),
        "Sorting into emotions with Opus 5.5... 3 of 12"
    );
}

#[gpui::test]
fn the_size_of_a_picked_file_is_read_off_the_main_thread(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("sized.zip")]);
    let size = view(&s, &visual).read_with(&visual, |v, _| v.dialog().unwrap().queue[0].size);
    assert!(size > 0);
}

#[gpui::test]
fn a_failed_write_after_an_import_removes_the_new_pack_and_its_folder(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("doomed.zip")]);
    // A folder where the file belongs makes every write fail.
    std::fs::create_dir_all(emoji_dir(&s).join("emoji.json")).unwrap();
    click(&mut visual, "emoji-lib-import-use");
    visual.run_until_parked();
    assert!(
        library_of(&s, &visual).packs.is_empty(),
        "the pack is not kept"
    );
    let packs = emoji_dir(&s).join("packs");
    let left = std::fs::read_dir(&packs).map_or(0, |rd| rd.count());
    assert_eq!(left, 0, "no folder is left behind");
    let error = store(&s, &visual)
        .read_with(&visual, |store, _| store.error().map(str::to_owned))
        .expect("the error is shown");
    assert!(
        error.contains("Could not save the emoji library"),
        "{error}"
    );
    assert!(staging_dirs(&s).is_empty());
}

fn close_settings(s: &Setup, visual: &mut VisualTestContext) {
    s.window
        .update(visual, |shell, window, cx| {
            shell.close_settings_for_test(window, cx)
        })
        .unwrap();
}

#[gpui::test]
fn closing_settings_while_saving_still_lands_the_pack(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("late.zip")]);
    // Press "Use this sorting" and close Settings before the save has run.
    // Both happen inside one update, so nothing runs between the accept and the close.
    let page = view(&s, &visual);
    s.window
        .update(&mut visual, |shell, window, cx| {
            page.update(cx, |v, cx| v.accept_for_test(window, cx));
            shell.close_settings_for_test(window, cx);
        })
        .unwrap();
    visual.run_until_parked();
    let saved = disk(&s);
    assert_eq!(saved.packs.len(), 1, "the pack is in the library file");
    for e in &saved.packs[0].emoji {
        assert!(emoji::image_path(&emoji_dir(&s), &saved.packs[0], e).is_file());
    }
    assert_eq!(library_of(&s, &visual), saved);
    assert!(staging_dirs(&s).is_empty());
    let folders = std::fs::read_dir(emoji_dir(&s).join("packs"))
        .unwrap()
        .count();
    assert_eq!(folders, 1, "no orphan pack folder");
}

#[gpui::test]
fn closing_settings_removes_the_staging_folder_of_an_open_import(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("review.zip")]);
    assert_eq!(staging_dirs(&s).len(), 1);
    close_settings(&s, &mut visual);
    visual.run_until_parked();
    assert!(
        staging_dirs(&s).is_empty(),
        "the review's staging folder goes"
    );
    assert!(library_of(&s, &visual).packs.is_empty());
}

#[gpui::test]
fn closing_settings_while_unpacking_leaves_no_staging_folder(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    let zip = make_zip("early.zip");
    // Start and close inside one update: the unpack job has not run, and its staging folder
    // appears after the page is gone.
    let page = view(&s, &visual);
    s.window
        .update(&mut visual, |shell, window, cx| {
            page.update(cx, |v, cx| v.import_for_test(vec![zip], window, cx));
            assert!(page.read(cx).dialog().is_some());
            shell.close_settings_for_test(window, cx);
        })
        .unwrap();
    drop(page);
    visual.run_until_parked();
    assert!(staging_dirs(&s).is_empty());
    assert!(library_of(&s, &visual).packs.is_empty());
}

#[gpui::test]
fn a_commit_that_fails_after_the_page_closed_is_reported_next_time(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("broken.zip")]);
    // A file where the packs folder should be makes the commit fail.
    std::fs::write(emoji_dir(&s).join("packs"), b"not a folder").unwrap();
    let page = view(&s, &visual);
    s.window
        .update(&mut visual, |shell, window, cx| {
            page.update(cx, |v, cx| v.accept_for_test(window, cx));
            shell.close_settings_for_test(window, cx);
        })
        .unwrap();
    drop(page);
    visual.run_until_parked();
    let error = store(&s, &visual).read_with(&visual, |st, _| st.error().map(str::to_owned));
    assert!(error.is_some(), "the store remembers the failed import");
    assert!(staging_dirs(&s).is_empty());
    assert!(library_of(&s, &visual).packs.is_empty());
}

#[gpui::test]
fn showing_another_settings_page_cancels_the_import(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("hidden.zip")]);
    let settings = s
        .window
        .read_with(&visual, |shell, _| shell.settings_view())
        .unwrap()
        .unwrap();
    settings.update(&mut visual, |v, cx| {
        v.select_section(crate::settings::Section::General, cx)
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    use gpui::Focusable;
    let inside = visual.update(|window, cx| {
        settings
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    });
    assert!(inside, "keys still reach the settings view");
    assert!(!dialog_open(&s, &visual), "no dialog runs hidden");
    assert!(staging_dirs(&s).is_empty());
    assert!(library_of(&s, &visual).packs.is_empty());
}

#[gpui::test]
fn after_the_dialog_closes_keys_still_reach_the_settings_view(cx: &mut TestAppContext) {
    use gpui::Focusable;
    let (s, mut visual) = open(cx);
    show(&s, &mut visual);
    install_fake(&s, vec!["happy"], &mut visual);
    import(&s, &mut visual, vec![make_zip("focus.zip")]);
    click(&mut visual, "emoji-lib-import-cancel");
    visual.run_until_parked();
    let settings = s
        .window
        .read_with(&visual, |shell, _| shell.settings_view())
        .unwrap()
        .unwrap();
    let inside = visual.update(|window, cx| {
        settings
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    });
    assert!(inside, "the focus stays inside the settings view");
}

#[gpui::test]
fn removing_a_pack_lays_the_chat_out_once(cx: &mut TestAppContext) {
    use super::emoji_tests::{chat_transcript, rebuilds};
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    let mut library = Library::default();
    seed(
        &s,
        &mut library,
        "pack-seed",
        vec![entry("a.png", "alpha", Format::Png, Some("happy"))],
    );
    reload(&s, &mut visual);
    s.manager.update(&mut visual, |m, cx| {
        m.emoji_update(cx, |e| {
            e.enabled = true;
            e.packs = vec!["pack-seed".into()];
        });
    });
    let _ = chat_transcript(&s, &mut visual);
    assert!(visual.debug_bounds("mono-emoji-happy").is_some());
    show(&s, &mut visual);
    let before = rebuilds(&s, &visual);
    click(&mut visual, "emoji-lib-remove-pack-seed");
    visual.simulate_prompt_answer("Remove");
    visual.run_until_parked();
    assert_eq!(rebuilds(&s, &visual), before + 1, "one push, not two");
    assert!(s.manager.read_with(&visual, |m, _| {
        m.roster.get(&s.id).unwrap().emoji.packs.is_empty()
    }));
}
