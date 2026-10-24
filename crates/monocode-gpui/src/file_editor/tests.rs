use super::*;
use gpui::{EntityInputHandler, TestAppContext};

#[gpui::test]
fn replace_is_one_undo_and_autosave_formats_with_original_encoding(cx: &mut TestAppContext) {
    let path = std::env::temp_dir().join(format!(
        "monocode-editor-parity-{}.json",
        std::process::id()
    ));
    let original = "\u{feff}{\"word\":\"hello\",\"other\":\"hello\"}\r\n";
    std::fs::write(&path, original).unwrap();
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx
                .new(|cx| ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx));
            cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), git, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |e, w, cx| {
            e.open_search(w, cx);
            e.replace_open = true;
            e.search.update(cx, |i, cx| i.set_text("hello".into(), cx));
            e.replacement
                .update(cx, |i, cx| i.set_text("world".into(), cx));
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |e, _, cx| {
            assert_eq!(e.matches.len(), 2);
            e.replace_matches(true, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    window
        .update(cx, |e, w, cx| {
            assert!(e.dirty);
            e.dismiss_search(w, cx);
            w.dispatch_action(Box::new(input::Undo), cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |e, w, cx| {
            assert!(!e.dirty);
            e.open_search(w, cx);
            e.find_matches(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |e, _, cx| {
            assert_eq!(e.matches.len(), 2);
            e.replace_matches(true, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| cx.update_global::<crate::settings::Preferences, _>(|p, _| p.autosave = true));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(999));
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "\u{feff}{ \"word\": \"world\", \"other\": \"world\" }\r\n"
    );
    window
        .update(cx, |e, _, _| assert!(!e.dirty && !e.saving))
        .unwrap();
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn autosave_refuses_external_edits_even_before_the_file_poll(cx: &mut TestAppContext) {
    let path = std::env::temp_dir().join(format!(
        "monocode-autosave-conflict-{}.txt",
        std::process::id()
    ));
    std::fs::write(&path, "original\n").unwrap();
    cx.update(|cx| {
        cx.set_global(crate::settings::Preferences {
            autosave: true,
            ..Default::default()
        })
    });
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx
                .new(|cx| ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx));
            cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), git, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |e, w, cx| {
            e.input
                .update(cx, |i, cx| i.replace_text_in_range(None, "draft ", w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    std::fs::write(&path, "external\n").unwrap();
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    window
        .update(cx, |e, _, _| {
            assert!(e.dirty);
            assert!(e.error.as_ref().unwrap().contains("changed on disk"));
        })
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
    std::fs::remove_file(path).unwrap();
}

#[gpui::test]
fn saving_a_clean_file_still_applies_the_format_on_save_preference(cx: &mut TestAppContext) {
    let path =
        std::env::temp_dir().join(format!("monocode-clean-format-{}.json", std::process::id()));
    std::fs::write(&path, "{\"value\":1}").unwrap();
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let git = cx
                .new(|cx| ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx));
            cx.new(|cx| FileEditor::new(path.to_string_lossy().into_owned(), git, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    window
        .update(cx, |e, _, cx| {
            assert!(!e.dirty);
            e.save(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{ \"value\": 1 }\n"
    );
    std::fs::remove_file(path).unwrap();
}

mod blame {
    use super::*;
    use crate::file_editor::blame::State;
    use monocode_core::scm::BlameHunk;

    fn hunk(sha: &str, start: usize, count: usize) -> BlameHunk {
        BlameHunk {
            sha: format!("{sha:0<40}"),
            short_sha: sha.into(),
            author: format!("author-{sha}"),
            author_time: 0,
            summary: "summary".into(),
            uncommitted: false,
            start_line: start,
            line_count: count,
        }
    }

    /// A shown, clean editor on a file inside its project. Tests feed hunks
    /// to `receive_blame`; `request_blame` never runs git under `cfg(test)`.
    fn shown(
        cx: &mut TestAppContext,
        prefs: crate::settings::Preferences,
    ) -> (gpui::WindowHandle<FileEditor>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("monocode-blame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "{}.txt",
            cx.test_function_name().unwrap_or("blame")
        ));
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        cx.update(|cx| cx.set_global(prefs));
        let project = dir.to_string_lossy().into_owned();
        let file = path.to_string_lossy().into_owned();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let git = cx.new(|cx| ProjectGit::new(project, cx));
                cx.new(|cx| FileEditor::new(file, git, cx))
            })
            .unwrap()
        });
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                e.visible = true;
                e.sync_blame_options(cx);
            })
            .unwrap();
        (window, path)
    }
    fn state_of(e: &FileEditor, cx: &App) -> &'static str {
        match e.input.read(cx).editor_blame_state() {
            State::Loading => "loading",
            State::Absent => "absent",
            State::Lines { .. } => "lines",
        }
    }

    #[gpui::test]
    fn an_answer_applies_only_to_the_request_and_document_it_was_for(cx: &mut TestAppContext) {
        let (window, path) = shown(cx, crate::settings::Preferences::default());
        window
            .update(cx, |e, _, cx| {
                let text = e.input.read(cx).content.clone();
                e.request_blame(cx);
                let current = e.blame_generation;
                // An older request's answer is dropped.
                e.receive_blame(current - 1, text.clone(), Ok(vec![hunk("aaa", 1, 3)]), cx);
                assert_eq!(state_of(e, cx), "loading");
                e.receive_blame(current, text.clone(), Ok(vec![hunk("aaa", 1, 3)]), cx);
                assert_eq!(state_of(e, cx), "lines");
                // Untracked, binary, oversized and failing files have none.
                e.request_blame(cx);
                e.receive_blame(e.blame_generation, text.clone(), Ok(vec![]), cx);
                assert_eq!(state_of(e, cx), "absent");
                e.request_blame(cx);
                e.receive_blame(e.blame_generation, text.clone(), Err("boom".into()), cx);
                assert_eq!(state_of(e, cx), "absent");
                // Unsaved edits: blame describes the saved file, so it waits.
                e.request_blame(cx);
                let before = e.blame_generation;
                e.dirty = true;
                e.receive_blame(before, text.clone(), Ok(vec![hunk("aaa", 1, 3)]), cx);
                assert_eq!(state_of(e, cx), "absent");
                let generation = e.blame_generation;
                e.request_blame(cx);
                assert_eq!(e.blame_generation, generation, "no fetch while dirty");
                // The buffer changed under an answer: ask again.
                e.dirty = false;
                e.request_blame(cx);
                let asked = e.blame_generation;
                e.receive_blame(asked, "stale".into(), Ok(vec![hunk("aaa", 1, 3)]), cx);
                assert_eq!(state_of(e, cx), "absent");
                assert_eq!(e.blame_generation, asked + 1);
            })
            .unwrap();
        std::fs::remove_file(path).ok();
    }

    #[gpui::test]
    fn nothing_is_requested_when_hidden_or_when_both_modes_are_off(cx: &mut TestAppContext) {
        let off = crate::settings::Preferences {
            editor_inline_blame: false,
            ..Default::default()
        };
        let (window, path) = shown(cx, off);
        window
            .update(cx, |e, _, cx| {
                let start = e.blame_generation;
                e.request_blame(cx);
                assert_eq!(e.blame_generation, start, "both modes off");
                assert_eq!(e.input.read(cx).editor_blame_options(), (false, false));
                cx.update_global::<crate::settings::Preferences, _>(|p, _| {
                    p.editor_blame_column = true
                });
                e.sync_blame(cx);
                assert_eq!(e.input.read(cx).editor_blame_options(), (false, true));
                assert_eq!(e.blame_generation, start + 1, "turning a mode on fetches");
                e.visible = false;
                e.request_blame(cx);
                assert_eq!(e.blame_generation, start + 1, "hidden editors wait");
                assert!(e.blame_stale);
                e.visible = true;
                cx.update_global::<crate::settings::Preferences, _>(|p, _| {
                    p.editor_blame_column = false
                });
                e.sync_blame(cx);
                assert_eq!(state_of(e, cx), "loading", "off drops the blame");
            })
            .unwrap();
        std::fs::remove_file(path).ok();
    }

    #[gpui::test]
    fn the_defaults_are_inline_on_and_column_off(cx: &mut TestAppContext) {
        let (window, path) = shown(cx, crate::settings::Preferences::default());
        window
            .update(cx, |e, _, cx| {
                assert_eq!(e.input.read(cx).editor_blame_options(), (true, false));
            })
            .unwrap();
        std::fs::remove_file(path).ok();
    }

    #[gpui::test]
    fn review_tabs_and_files_outside_the_project_have_no_blame(cx: &mut TestAppContext) {
        let (window, path) = shown(cx, crate::settings::Preferences::default());
        window
            .update(cx, |e, _, cx| {
                e.show_diff = true;
                e.sync_blame_options(cx);
                assert_eq!(e.input.read(cx).editor_blame_options(), (false, false));
                e.show_diff = false;
                e.path = "/elsewhere/file.txt".into();
                assert!(e.blame_target().is_none());
            })
            .unwrap();
        std::fs::remove_file(path).ok();
    }

    #[gpui::test]
    fn a_hidden_editor_fetches_once_when_shown_and_not_when_nothing_changed(
        cx: &mut TestAppContext,
    ) {
        let (window, path) = shown(cx, crate::settings::Preferences::default());
        window
            .update(cx, |e, _, cx| {
                let text = e.input.read(cx).content.clone();
                e.request_blame(cx);
                e.receive_blame(e.blame_generation, text, Ok(vec![hunk("aaa", 1, 3)]), cx);
                assert!(e.blame_loaded && !e.blame_stale);
                // Hidden: a change only marks the blame stale.
                e.visible = false;
                let generation = e.blame_generation;
                e.request_blame(cx);
                assert_eq!(e.blame_generation, generation);
                assert!(e.blame_stale);
                // Shown again, it catches up once.
                e.visible = true;
                e.catch_up_blame(cx);
                assert_eq!(e.blame_generation, generation + 1);
                assert!(!e.blame_stale);
                // Shown again with nothing changed: no fetch.
                let text = e.input.read(cx).content.clone();
                e.receive_blame(e.blame_generation, text, Ok(vec![hunk("aaa", 1, 3)]), cx);
                e.catch_up_blame(cx);
                assert_eq!(e.blame_generation, generation + 1);
            })
            .unwrap();
        std::fs::remove_file(path).ok();
    }
}
