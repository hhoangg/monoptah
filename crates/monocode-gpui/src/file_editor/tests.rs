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

#[gpui::test]
fn hunk_actions_are_refused_while_a_cloud_session_is_in_view(cx: &mut TestAppContext) {
    let path = std::env::temp_dir().join(format!("monocode-gate-{}.txt", std::process::id()));
    std::fs::write(&path, "one\n").unwrap();
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
    crate::git_gate::set(crate::git_gate::Gate::local_of("octo-abc"));
    window
        .update(cx, |editor, _, cx| {
            editor.apply_change(true, cx);
            assert_eq!(editor.error.as_deref(), Some(crate::git_gate::REFUSED));
            editor.error = None;
            editor.apply_change(false, cx);
            assert_eq!(editor.error.as_deref(), Some(crate::git_gate::REFUSED));
        })
        .unwrap();
    crate::git_gate::set(crate::git_gate::Gate::default());
    let _ = std::fs::remove_file(&path);
}

/// A file on another machine offers neither Open externally nor Reveal: both act on this
/// machine's disk. A local file that fails to open still offers them.
#[gpui::test]
fn an_unreadable_remote_file_hides_open_externally_and_reveal(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
    let mut shown = Vec::new();
    for path in [
        "/nonexistent/monocode-missing.txt",
        "remote://env-octo-abc/workspaces/widgets/a.rs",
    ] {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let git = cx.new(|cx| ProjectGit::new("/nonexistent".into(), cx));
                cx.new(|cx| FileEditor::new(path.into(), git, cx))
            })
            .unwrap()
        });
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        // Whatever the read did, the error state is what this view draws.
        window
            .update(visual, |editor, _, cx| {
                editor.loading = false;
                editor.unsupported = None;
                editor.error = Some("Could not open".into());
                cx.notify();
            })
            .unwrap();
        visual.refresh().unwrap();
        shown.push((
            visual.debug_bounds("open-file-externally").is_some(),
            visual.debug_bounds("reveal-file").is_some(),
        ));
    }
    assert_eq!(shown, [(true, true), (false, false)]);
}

mod cloud_conflicts {
    use super::*;
    use monocode_core::files::remote_write::HOST_CHANGED;
    use std::sync::Mutex;

    const PATH: &str = "remote://env/work/repo/a.txt";

    fn doc(text: &str, read_only: bool) -> TextDocument {
        TextDocument {
            text: text.into(),
            crlf: false,
            cr: false,
            bom: false,
            disk: text.as_bytes().to_vec(),
            read_only: read_only.then(|| document::REMOTE_READ_ONLY.to_owned()),
        }
    }

    /// What the fake Host saw: ("save" | "overwrite", text).
    type Log = Arc<Mutex<Vec<(&'static str, String)>>>;

    /// A Host whose file was changed by the agent: `save` is refused, `overwrite` lands.
    fn io(log: &Log, cloud: &str) -> conflict::FileIo {
        let (save_log, overwrite_log) = (log.clone(), log.clone());
        let cloud = cloud.to_owned();
        conflict::FileIo {
            save: Arc::new(move |_, _, text| {
                save_log.lock().unwrap().push(("save", text.into()));
                Err(format!("Command failed: {HOST_CHANGED}"))
            }),
            overwrite: Arc::new(move |_, _, text| {
                overwrite_log
                    .lock()
                    .unwrap()
                    .push(("overwrite", text.into()));
                Ok(text.as_bytes().to_vec())
            }),
            read: Arc::new(move |_| Ok(Document::Text(doc(&cloud, false)))),
        }
    }

    fn editor(
        cx: &mut TestAppContext,
        document: TextDocument,
        io: conflict::FileIo,
    ) -> gpui::WindowHandle<FileEditor> {
        cx.update(|cx| cx.set_global(crate::settings::Preferences::default()));
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                let git = cx.new(|cx| {
                    ProjectGit::new(std::env::temp_dir().to_string_lossy().into_owned(), cx)
                });
                cx.new(|cx| {
                    let mut e = FileEditor::new_deferred(PATH.into(), git, cx);
                    let text = document.text.clone();
                    e.input.update(cx, |i, cx| i.reload_file(text, false, cx));
                    e.document = Some(document);
                    e.io = io;
                    e.sync_read_only(cx);
                    e
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        window
    }

    fn type_draft(cx: &mut TestAppContext, window: &gpui::WindowHandle<FileEditor>) {
        window
            .update(cx, |e, w, cx| {
                e.input
                    .update(cx, |i, cx| i.replace_text_in_range(None, "draft ", w, cx))
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_refused_save_opens_the_dialog_and_keeps_the_text(cx: &mut TestAppContext) {
        let log = Log::default();
        let window = editor(cx, doc("one\n", false), io(&log, "agent\n"));
        type_draft(cx, &window);
        window.update(cx, |e, _, cx| e.save(cx)).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                let conflict = e.conflict.as_ref().expect("the dialog is open");
                assert_eq!(conflict.mine, "draft one\n");
                assert!(matches!(&conflict.cloud, conflict::Cloud::Text(d) if d.text == "agent\n"));
                // Nothing was replaced: the draft is still there and still unsaved.
                assert!(e.dirty && !e.saving);
                assert_eq!(e.input.read(cx).content.as_ref(), "draft one\n");
                assert!(e.input.read(cx).is_read_only());
                // A second save does not run while the dialog is open.
                e.save(cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(*log.lock().unwrap(), [("save", "draft one\n".to_owned())]);
    }

    #[gpui::test]
    fn cancel_keeps_the_draft_and_unlocks_the_editor(cx: &mut TestAppContext) {
        let log = Log::default();
        let window = editor(cx, doc("one\n", false), io(&log, "agent\n"));
        type_draft(cx, &window);
        window.update(cx, |e, _, cx| e.save(cx)).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                e.cancel_conflict(cx);
                assert!(e.conflict.is_none() && e.dirty);
                assert_eq!(e.input.read(cx).content.as_ref(), "draft one\n");
                assert!(!e.input.read(cx).is_read_only());
            })
            .unwrap();
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[gpui::test]
    fn keep_mine_overwrites_and_adopts_what_was_written(cx: &mut TestAppContext) {
        let log = Log::default();
        let window = editor(cx, doc("one\n", false), io(&log, "agent\n"));
        type_draft(cx, &window);
        window.update(cx, |e, _, cx| e.save(cx)).unwrap();
        cx.run_until_parked();
        window.update(cx, |e, _, cx| e.keep_mine(cx)).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                assert!(e.conflict.is_none() && !e.dirty && !e.saving);
                assert!(e.error.is_none());
                assert_eq!(e.input.read(cx).content.as_ref(), "draft one\n");
                let doc = e.document.as_ref().unwrap();
                assert_eq!(doc.disk, b"draft one\n");
                assert!(!e.input.read(cx).is_read_only());
            })
            .unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            [
                ("save", "draft one\n".to_owned()),
                ("overwrite", "draft one\n".to_owned())
            ]
        );
    }

    #[gpui::test]
    fn take_cloud_reloads_the_editor_with_the_cloud_text(cx: &mut TestAppContext) {
        let log = Log::default();
        let window = editor(cx, doc("one\n", false), io(&log, "agent\n"));
        type_draft(cx, &window);
        window.update(cx, |e, _, cx| e.save(cx)).unwrap();
        cx.run_until_parked();
        // Until the user chooses, the draft is intact.
        window
            .update(cx, |e, _, cx| {
                assert_eq!(e.input.read(cx).content.as_ref(), "draft one\n");
                e.take_cloud(cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                assert!(e.conflict.is_none() && !e.dirty);
                assert_eq!(e.input.read(cx).content.as_ref(), "agent\n");
                assert_eq!(e.document.as_ref().unwrap().disk, b"agent\n");
            })
            .unwrap();
        // Take cloud never writes.
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[gpui::test]
    fn a_deleted_cloud_file_offers_only_keep_mine(cx: &mut TestAppContext) {
        let log = Log::default();
        let mut gone = io(&log, "");
        gone.read = Arc::new(|_| Err("ENOENT: no such file".into()));
        let window = editor(cx, doc("one\n", false), gone);
        type_draft(cx, &window);
        window.update(cx, |e, _, cx| e.save(cx)).unwrap();
        cx.run_until_parked();
        window
            .update(cx, |e, _, cx| {
                assert!(matches!(
                    e.conflict.as_ref().unwrap().cloud,
                    conflict::Cloud::Unavailable(_)
                ));
                e.take_cloud(cx);
                assert!(e.conflict.is_some() && e.dirty);
                assert_eq!(e.input.read(cx).content.as_ref(), "draft one\n");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_file_over_one_megabyte_is_read_only_and_cannot_be_saved(cx: &mut TestAppContext) {
        let log = Log::default();
        let window = editor(cx, doc("big\n", true), io(&log, "x"));
        window
            .update(cx, |e, w, cx| {
                assert!(e.input.read(cx).is_read_only());
                e.input
                    .update(cx, |i, cx| i.replace_text_in_range(None, "typed", w, cx));
                assert_eq!(e.input.read(cx).content.as_ref(), "big\n");
                e.save(cx);
                assert!(!e.saving);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(log.lock().unwrap().is_empty());
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("read-only-banner").is_some());
        window
            .update(visual, |e, _, _| {
                assert_eq!(
                    e.document.as_ref().unwrap().read_only.as_deref(),
                    Some(document::REMOTE_READ_ONLY)
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_agent_working_notice_follows_the_running_signal(cx: &mut TestAppContext) {
        let log = Log::default();
        let window = editor(cx, doc("one\n", false), io(&log, "agent\n"));
        let visual = &mut gpui::VisualTestContext::from_window(window.into(), cx);
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("agent-working-banner").is_none());
        crate::git_gate::set(crate::git_gate::Gate::running_in(&["env"]));
        window.update(visual, |_, _, cx| cx.notify()).unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("agent-working-banner").is_some());
        // The compare dialog carries the same notice.
        window
            .update(visual, |e, w, cx| {
                e.input
                    .update(cx, |i, cx| i.replace_text_in_range(None, "d", w, cx));
                e.show_conflict("d".into(), conflict::Cloud::Unavailable("gone".into()), cx);
            })
            .unwrap();
        visual.refresh().unwrap();
        assert!(visual.debug_bounds("conflict-agent-working").is_some());
        assert!(visual.debug_bounds("conflict-keep-mine").is_some());
        crate::git_gate::set(crate::git_gate::Gate::default());
    }
}
