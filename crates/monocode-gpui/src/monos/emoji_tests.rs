//! The Emoji page of a Mono's settings (switch, frequency, packs, the manage button) and how its
//! choices reach the chat. The shared library page and the import dialog are tested in
//! `emoji_library_tests.rs`.
use super::workflows_ui::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::emoji::{self, Emoji, Format, Library, Pack, test_support};

pub(super) fn data_dir(s: &Setup) -> PathBuf {
    s.root.parent().unwrap().to_path_buf()
}

pub(super) fn emoji_dir(s: &Setup) -> PathBuf {
    emoji::dir(&data_dir(s))
}

pub(super) fn disk(s: &Setup) -> Library {
    emoji::load(&emoji_dir(s)).unwrap().library
}

pub(super) fn entry(file: &str, name: &str, format: Format, emotion: Option<&str>) -> Emoji {
    Emoji {
        file: file.into(),
        hash: format!("hash-{file}"),
        name: name.into(),
        format,
        emotion: emotion.map(str::to_owned),
    }
}

/// Adds a pack with real images on disk to `library` and writes the library file. The app has
/// to read it afterwards (`reload`).
pub(super) fn seed(s: &Setup, library: &mut Library, id: &str, emoji: Vec<Emoji>) {
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
        sorted_by: Some("Fake model".into()),
        emoji,
    });
    emoji::save(&emoji_dir(s), library).unwrap();
}

pub(super) fn open(cx: &mut TestAppContext) -> (Setup, VisualTestContext) {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let visual = VisualTestContext::from_window(s.window.into(), cx);
    (s, visual)
}

pub(super) fn store(
    s: &Setup,
    visual: &VisualTestContext,
) -> Entity<crate::emoji_library::EmojiStore> {
    s.manager.read_with(visual, |m, _| m.emoji_store.clone())
}

/// The app reads the library file again, as after a start.
pub(super) fn reload(s: &Setup, visual: &mut VisualTestContext) {
    let store = store(s, visual);
    store.update(visual, |store, cx| store.load(true, cx));
    visual.run_until_parked();
}

pub(super) fn library_of(s: &Setup, visual: &VisualTestContext) -> Library {
    (*store(s, visual).read_with(visual, |store, _| store.library().clone())).clone()
}

/// The Mono's own settings page, after the library was seeded.
fn show(s: &Setup, visual: &mut VisualTestContext) {
    reload(s, visual);
    click(visual, "mono-settings-Emoji");
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn two_packs(s: &Setup, visual: &mut VisualTestContext) {
    let mut library = Library::default();
    seed(
        s,
        &mut library,
        "pack-a",
        vec![
            entry("a.png", "alpha", Format::Png, Some("happy")),
            entry("b.gif", "bravo", Format::Gif, Some("sad")),
        ],
    );
    seed(
        s,
        &mut library,
        "pack-b",
        vec![entry("c.png", "charlie", Format::Png, Some("love"))],
    );
    show(s, visual);
}

pub(super) fn mono_emoji(s: &Setup, visual: &VisualTestContext) -> emoji::MonoEmoji {
    s.manager
        .read_with(visual, |m, _| m.roster.get(&s.id).unwrap().emoji.clone())
}

/// What the roster holds on disk (the database), not in memory.
fn saved_mono_emoji(s: &Setup) -> emoji::MonoEmoji {
    let db = locked(&s.store).unwrap();
    mono::Roster::load(&db)
        .unwrap()
        .get(&s.id)
        .unwrap()
        .emoji
        .clone()
}

#[gpui::test]
fn the_mono_page_keeps_the_switch_the_frequency_and_the_pack_choice(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    two_packs(&s, &mut visual);
    for selector in [
        "mono-emoji-page",
        "mono-emoji-enabled",
        "mono-emoji-frequency",
        "mono-emoji-manage",
        "mono-emoji-new-packs",
        "mono-emoji-pack-pack-a",
        "mono-emoji-pack-pack-b",
        "mono-emoji-use-pack-a",
        "mono-emoji-use-pack-b",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // The library is not managed here any more.
    for selector in [
        "mono-emoji-import",
        "mono-emoji-board",
        "emoji-lib-import",
        "emoji-lib-board",
    ] {
        assert!(visual.debug_bounds(selector).is_none(), "{selector}");
    }
    assert!(
        s.manager
            .read_with(&visual, |m, _| m.settings == Some(SettingsSection::Emoji))
    );
    assert_eq!(
        emoji::MonoEmoji::default(),
        mono_emoji(&s, &visual),
        "nothing is on until the user turns it on"
    );
}

#[test]
fn the_texts_of_the_page_say_what_is_not_automatic() {
    assert_eq!(
        emoji_ui::NEW_PACKS,
        "New packs are not turned on for this Mono automatically."
    );
    assert_eq!(
        emoji_ui::SWITCH_HELP,
        "Off by default. Never in code, commit messages, PR text or reports."
    );
}

#[gpui::test]
fn the_switch_and_the_frequency_are_saved_on_the_mono(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    show(&s, &mut visual);
    assert!(!mono_emoji(&s, &visual).enabled);
    click(&mut visual, "mono-emoji-enabled");
    assert!(mono_emoji(&s, &visual).enabled);
    assert!(saved_mono_emoji(&s).enabled, "the roster was written");
    click(&mut visual, "mono-emoji-frequency");
    visual.refresh().unwrap();
    assert!(
        s.manager
            .read_with(&visual, |m, _| m.emoji_ui.select_is_open()),
        "the list opens"
    );
    click(&mut visual, "menu-action-2");
    visual.run_until_parked();
    let saved = saved_mono_emoji(&s);
    assert_eq!(saved.frequency, emoji::Frequency::Often);
    assert!(saved.enabled);
    click(&mut visual, "mono-emoji-enabled");
    assert!(!saved_mono_emoji(&s).enabled);
    // The library file is not touched by a Mono's choices.
    assert!(!emoji_dir(&s).join("emoji.json").exists());
}

#[gpui::test]
fn packs_are_chosen_per_mono_and_a_new_pack_is_not_selected(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    two_packs(&s, &mut visual);
    click(&mut visual, "mono-emoji-use-pack-a");
    assert_eq!(mono_emoji(&s, &visual).packs, ["pack-a"]);
    assert_eq!(saved_mono_emoji(&s).packs, ["pack-a"]);
    click(&mut visual, "mono-emoji-use-pack-b");
    assert_eq!(saved_mono_emoji(&s).packs, ["pack-a", "pack-b"]);
    click(&mut visual, "mono-emoji-use-pack-a");
    assert_eq!(saved_mono_emoji(&s).packs, ["pack-b"]);
    // A pack that joins the library later is not turned on for the Mono.
    let store = store(&s, &visual);
    store.update(&mut visual, |store, cx| {
        store
            .add_pack(
                Pack {
                    id: "pack-new".into(),
                    name: "pack-new".into(),
                    sorted_by: None,
                    emoji: vec![entry("n.png", "newbie", Format::Png, None)],
                },
                cx,
            )
            .unwrap();
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("mono-emoji-use-pack-new").is_some());
    assert_eq!(mono_emoji(&s, &visual).packs, ["pack-b"]);
    assert_eq!(saved_mono_emoji(&s).packs, ["pack-b"]);
}

#[gpui::test]
fn an_empty_library_shows_the_empty_state_with_the_manage_button(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    show(&s, &mut visual);
    assert!(visual.debug_bounds("mono-emoji-empty").is_some());
    assert!(visual.debug_bounds("mono-emoji-manage").is_some());
    assert!(visual.debug_bounds("mono-emoji-use-pack-a").is_none());
}

#[gpui::test]
fn the_manage_button_closes_the_mono_settings_and_opens_the_library_page(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    two_packs(&s, &mut visual);
    click(&mut visual, "mono-emoji-manage");
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        s.manager.read_with(&visual, |m, _| m.settings.is_none()),
        "the Mono's settings do not stay behind the page"
    );
    let section = s
        .window
        .read_with(&visual, |shell, cx| {
            shell.settings_view().map(|v| v.read(cx).section)
        })
        .unwrap();
    assert_eq!(section, Some(crate::settings::Section::Monos));
    assert!(visual.debug_bounds("settings-emoji-library").is_some());
    assert!(visual.debug_bounds("emoji-lib").is_some());
    assert!(visual.debug_bounds("emoji-lib-import").is_some());
}

fn chat_transcript(
    s: &Setup,
    visual: &mut VisualTestContext,
) -> gpui::Entity<crate::transcript::TranscriptView> {
    use monocode_core::transcript::Row;
    let chat = s.manager.read_with(visual, |m, _| m.chats[&s.id].clone());
    // A Mono chat shows its transcript once the session has messages.
    chat.update(visual, |c, cx| {
        c.summary.has_messages = true;
        cx.notify();
    });
    let transcript = chat.read_with(visual, |c, _| c.transcript.clone());
    let row = |role: &str, text: &str| Row {
        role: role.into(),
        text: text.into(),
        first: true,
        sent_at: None,
    };
    transcript.update(visual, |t, cx| {
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
    transcript
}

#[gpui::test]
fn the_chat_draws_emoji_tokens_only_for_the_selected_packs(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    two_packs(&s, &mut visual);
    click(&mut visual, "mono-emoji-enabled");
    let transcript = chat_transcript(&s, &mut visual);
    assert_eq!(
        transcript.read_with(&visual, |t, _| t.source_rows().len()),
        2,
        "the patched rows are still there"
    );
    assert!(
        visual.debug_bounds("mono-emoji-happy").is_none(),
        "no pack is selected, so nothing is drawn"
    );
    // The emotion has an emoji only in pack-a: selecting pack-b changes nothing.
    click(&mut visual, "mono-emoji-use-pack-b");
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("mono-emoji-happy").is_none());
    click(&mut visual, "mono-emoji-use-pack-a");
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds("mono-emoji-happy").is_some(),
        "selecting the pack draws the picture"
    );
    click(&mut visual, "mono-emoji-use-pack-a");
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds("mono-emoji-happy").is_none(),
        "deselecting it removes the picture"
    );
    click(&mut visual, "mono-emoji-use-pack-a");
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("mono-emoji-happy").is_some());
    click(&mut visual, "mono-emoji-enabled");
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds("mono-emoji-happy").is_none(),
        "switching it off removes the picture"
    );
}

#[gpui::test]
fn a_pack_id_the_library_does_not_have_is_ignored(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    two_packs(&s, &mut visual);
    let id = s.id.clone();
    s.manager.update(&mut visual, |m, cx| {
        m.emoji_update(cx, |e| {
            e.enabled = true;
            e.packs = vec!["pack-gone".into(), "pack-a".into()];
        });
    });
    let _ = chat_transcript(&s, &mut visual);
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("mono-emoji-happy").is_some());
    assert!(
        s.manager
            .read_with(&visual, |m, _| m.roster.get(&id).is_some())
    );
}

fn rebuilds(s: &Setup, visual: &VisualTestContext) -> usize {
    s.manager.read_with(visual, |m, cx| {
        m.chats[&s.id].read(cx).transcript.read(cx).emoji_rebuilds
    })
}

#[gpui::test]
fn a_description_never_rebuilds_the_chat_but_a_pack_choice_does(cx: &mut TestAppContext) {
    let (s, mut visual) = open(cx);
    visual.run_until_parked();
    two_packs(&s, &mut visual);
    click(&mut visual, "mono-emoji-use-pack-a");
    click(&mut visual, "mono-emoji-enabled");
    let before = rebuilds(&s, &visual);
    assert!(before >= 1, "turning it on is drawn: the chat was told");
    let store = store(&s, &visual);
    store.update(&mut visual, |store, cx| {
        store
            .edit_typed(cx, |l| {
                l.describe_emotion("happy", "Typing on");
                Ok(())
            })
            .unwrap();
    });
    visual.run_until_parked();
    visual
        .executor()
        .advance_clock(crate::emoji_library::SAVE_DELAY + std::time::Duration::from_millis(50));
    visual.run_until_parked();
    assert_eq!(disk(&s).emotions[0].description, "Typing on");
    assert_eq!(
        rebuilds(&s, &visual),
        before,
        "a description is not drawn, so the chat is not laid out again"
    );
    // Another pack's pictures are not drawn either.
    store.update(&mut visual, |store, cx| {
        store
            .edit(cx, |l| {
                l.move_emoji("pack-b", "c.png", Some("celebrate"));
                Ok(())
            })
            .unwrap();
    });
    visual.run_until_parked();
    assert_eq!(rebuilds(&s, &visual), before);
    // A change the chat draws still reaches it.
    click(&mut visual, "mono-emoji-use-pack-b");
    visual.run_until_parked();
    assert_eq!(rebuilds(&s, &visual), before + 1);
}
