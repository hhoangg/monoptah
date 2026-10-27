use super::*;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::store::Store;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
pub(super) struct Setup {
    pub(super) window: WindowHandle<Shell>,
    pub(super) manager: Entity<Monos>,
    pub(super) id: String,
    pub(super) project: String,
    pub(super) session: i64,
    pub(super) root: PathBuf,
    pub(super) store: Database,
}

#[gpui::test]
fn missing_document_uses_error_cards_without_moving_content(cx: &mut TestAppContext) {
    let s = setup(cx);
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    let body = visual.debug_bounds("main-pane-frame").unwrap();
    s.manager.update(visual, |m, cx| {
        m.failed_reports.insert("failed-report".into());
        m.open_document("deleted-document", cx);
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert_eq!(
        s.manager
            .read_with(visual, |m, _| m.error.clone())
            .as_deref(),
        Some("Document unavailable. It may have been deleted.")
    );
    let error = visual.debug_bounds("mono-error").unwrap();
    let viewport = s
        .window
        .update(visual, |_, w, _| w.viewport_size())
        .unwrap();
    assert_eq!(error.top(), px(12.));
    assert_eq!(error.right(), viewport.width - px(12.));
    assert_eq!(error.size.width, px(360.));
    assert_eq!(visual.debug_bounds("main-pane-frame").unwrap(), body);

    let close = visual.debug_bounds("mono-error-close").unwrap();
    visual.simulate_click(close.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("mono-error").is_none());
    s.manager.read_with(visual, |m, _| {
        assert!(m.error.is_none());
        assert!(m.failed_reports.is_empty());
        assert!(
            m.settings.is_some(),
            "dismissing an error keeps settings open"
        );
        assert_eq!(m.active.as_deref(), Some(s.id.as_str()));
    });
}

#[gpui::test]
fn saved_desktop_mono_opens_in_actual_manager_without_replacing_its_conversation(
    cx: &mut TestAppContext,
) {
    let db = Store::open_monocode(std::path::Path::new(":memory:")).unwrap();
    let session = db.create("codex", "/saved-project").unwrap();
    let original = db.persisted_session_id(session.id).unwrap();
    let settings = Settings {
        model: Some("gpt-6.1-sol".into()),
        access: AccessMode::Auto,
        ..Default::default()
    };
    db.configure_agent(session.id, "codex", &settings).unwrap();
    let mut roster = Roster::default();
    roster.monos.push(Mono {
        id: "desktop-original-mono".into(),
        session_id: Some(session.id),
        name: "Big Boss".into(),
        mascot: "ghost".into(),
        color: 0xe25a9e,
        projects: vec!["/saved-project".into()],
        ..Default::default()
    });
    roster.save(&db).unwrap();
    let summaries = db.sessions().unwrap();
    let store = Arc::new(Mutex::new(db));
    let database = store.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(database, summaries, "/saved-project".into(), None, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let manager = window
        .update(cx, |shell, _, _| shell.mono_manager())
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            manager.update(cx, |m, cx| m.open("desktop-original-mono", window, cx));
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        manager.update(cx, |m, cx| {
            m.ensure_chat("desktop-original-mono", cx);
            let mono = m.current().unwrap();
            assert_eq!(mono.id, "desktop-original-mono");
            assert_eq!(mono.session_id, Some(session.id));
            assert_eq!(mono.name, "Big Boss");
            assert_eq!(mono.mascot, "ghost");
            let chat = m.chats[&mono.id].read(cx);
            assert_eq!(chat.summary.id, session.id);
            assert_eq!(chat.summary.provider, "codex");
            assert_eq!(chat.settings.model.as_deref(), Some("gpt-6.1-sol"));
            assert_eq!(m.chats.len(), 1);
        });
    });
    let db = locked(&store).unwrap();
    assert_eq!(db.sessions().unwrap().len(), 1);
    assert_eq!(db.persisted_session_id(session.id).unwrap(), original);
}

#[gpui::test]
fn settings_reset_restores_only_mono_soul_and_name(cx: &mut TestAppContext) {
    let s = setup(cx);
    mono::files::save(&s.root, &s.id, None, "SOUL.md", "# Custom soul", None).unwrap();
    mono::files::save(&s.root, &s.id, None, "MEMORY.md", "Keep this memory", None).unwrap();
    let before = cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.roster.get_mut(&s.id).unwrap().name = "Custom name".into();
            m.save(cx);
            m.roster.get(&s.id).unwrap().clone()
        })
    });
    cx.run_until_parked();
    let _reset = cx.update(|cx| s.manager.update(cx, |m, cx| m.reset_defaults(&s.id, cx)));
    cx.run_until_parked();
    let roster = Roster::load(&locked(&s.store).unwrap()).unwrap();
    let after = roster.get(&s.id).unwrap();
    assert!(after.name.is_empty());
    assert_eq!(after.session_id, before.session_id);
    assert_eq!(after.projects, before.projects);
    assert_eq!(after.habit_sessions, before.habit_sessions);
    let files = mono::files::load(&s.root, &s.id, None).unwrap();
    assert_eq!(files.soul.as_deref(), Some(mono::DEFAULT_SOUL));
    assert_eq!(files.memory, "Keep this memory");
}

#[gpui::test]
fn detail_pages_slide_over_the_overview_and_back_without_recreating_content(
    cx: &mut TestAppContext,
) {
    for kind in ["markdown", "html", "session"] {
        cx.update(|cx| cx.set_reduce_motion(true));
        let s = setup(cx);
        let session = s
            .window
            .update(cx, |shell, _, _| shell.workspace.active.unwrap())
            .unwrap();
        s.window
            .update(cx, |_, _, cx| {
                s.manager.update(cx, |m, cx| {
                    m.settings = None;
                    m.overview_tab = OverviewTab::Documents;
                    m.roster
                        .get_mut(&s.id)
                        .unwrap()
                        .documents
                        .push(mono::Document {
                            id: "slide-document".into(),
                            kind: kind.into(),
                            title: "Slide document".into(),
                            summary: String::new(),
                            body: "# Document\n\nKeep this reader alive.".into(),
                            created_at: 1,
                            updated_at: 1,
                        });
                    cx.notify();
                });
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
        visual.run_until_parked();
        visual.update(|_, cx| cx.set_reduce_motion(false));
        let action = if kind == "session" {
            json!({"type":"session", "sessionId":session})
        } else {
            json!({"type":"document", "documentId":"slide-document"})
        };
        s.window
            .update(visual, |_, _, cx| {
                s.manager
                    .update(cx, |m, cx| m.card_action(&s.id, &action, cx))
            })
            .unwrap();
        visual.run_until_parked();
        let page: &'static str = Box::leak(
            format!(
                "mono-detail-page-{}",
                if kind == "session" {
                    format!("session:{session}")
                } else {
                    "document:slide-document".into()
                }
            )
            .into_boxed_str(),
        );
        let advance = |visual: &mut gpui::VisualTestContext, ms| {
            let details = s.manager.read_with(visual, |m, _| m.details.clone());
            details.update(visual, |d, cx| d.elapsed(Duration::from_millis(ms), cx));
            visual
                .update_window(s.window.into(), |_, w, cx| {
                    w.simulate_next_frame(cx);
                    w.draw(cx).clear(cx);
                })
                .unwrap();
        };
        advance(visual, 70);
        let stack = visual.debug_bounds("mono-detail-stack").unwrap();
        let incoming = visual.debug_bounds(page).unwrap();
        assert!(incoming.left() > stack.left() && incoming.left() < stack.right());
        assert_eq!(
            incoming.size, stack.size,
            "sliding pages keep their full layout size"
        );
        assert!(visual.debug_bounds("mono-detail-base").unwrap().left() < stack.left());
        advance(visual, 1000);
        assert_eq!(visual.debug_bounds(page).unwrap().left(), stack.left());
        let reader = s.manager.read_with(visual, |m, cx| match kind {
            "html" => m.html_documents["slide-document"].entity_id(),
            "session" => m
                .owner
                .upgrade()
                .unwrap()
                .read(cx)
                .mono_session_entity(session)
                .unwrap()
                .entity_id(),
            _ => m.documents["slide-document"].entity_id(),
        });
        assert_eq!(
            s.manager
                .read_with(visual, |m, cx| m.native_detail_content(cx)),
            kind == "html"
        );
        let back = visual.debug_bounds("mono-page-back").unwrap().center();
        visual.simulate_click(back, Default::default());
        visual.run_until_parked();
        advance(visual, 70);
        let outgoing = visual
            .debug_bounds(page)
            .expect("Back keeps the outgoing page mounted");
        assert!(outgoing.left() > stack.left() && outgoing.left() < stack.right());
        advance(visual, 1000);
        assert!(visual.debug_bounds(page).is_none());
        assert_eq!(
            visual.debug_bounds("mono-detail-base").unwrap().left(),
            stack.left()
        );
        assert!(
            !s.manager
                .read_with(visual, |m, cx| m.native_detail_content(cx))
        );

        visual.update(|_, cx| cx.set_reduce_motion(true));
        s.window
            .update(visual, |_, _, cx| {
                s.manager
                    .update(cx, |m, cx| m.card_action(&s.id, &action, cx))
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds(page).unwrap().left(), stack.left());
        s.manager.read_with(visual, |m, cx| {
            let reopened = match kind {
                "html" => m.html_documents["slide-document"].entity_id(),
                "session" => m
                    .owner
                    .upgrade()
                    .unwrap()
                    .read(cx)
                    .mono_session_entity(session)
                    .unwrap()
                    .entity_id(),
                _ => m.documents["slide-document"].entity_id(),
            };
            assert_eq!(reader, reopened);
        });
        visual.update(|_, cx| cx.set_reduce_motion(false));
        s.window
            .update(visual, |_, w, cx| {
                s.manager.update(cx, |m, cx| {
                    m.close(cx);
                    m.open(&s.id, w, cx);
                });
            })
            .unwrap();
        visual.run_until_parked();
        assert!(
            visual.debug_bounds(page).is_none(),
            "returning to a Mono must not replay an old page's exit"
        );
        assert_eq!(
            visual.debug_bounds("mono-detail-base").unwrap().left(),
            stack.left()
        );
    }
}
pub(super) fn setup(cx: &mut TestAppContext) -> Setup {
    let dir = std::env::temp_dir().join(format!("native-monos-{}", mono::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.to_string_lossy().to_string();
    let store = Arc::new(Mutex::new(Store::open(&dir.join("native.sqlite")).unwrap()));
    let db = store.clone();
    let cwd = project.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| Shell::new(db, Vec::new(), cwd, None, cx))
        })
        .unwrap()
    });
    cx.run_until_parked();
    let manager = window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_open = true;
            cx.notify();
            shell.mono_manager()
        })
        .unwrap();
    let handle: gpui::AnyWindowHandle = window.into();
    handle
        .update(cx, |_, w, cx| {
            manager.update(cx, |m, cx| m.create(vec![project.clone()], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    let (id, session, root) = cx.update(|cx| {
        let m = manager.read(cx);
        let mono = m.current().unwrap();
        (mono.id.clone(), mono.session_id.unwrap(), m.root.clone())
    });
    Setup {
        window,
        manager,
        id,
        project,
        session,
        root,
        store,
    }
}
pub(super) fn call(
    s: &Setup,
    request: &str,
    action: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> mpsc::Receiver<Result<Value, String>> {
    call_from(s, s.session, request, action, input, cx)
}
pub(super) fn call_from(
    s: &Setup,
    session: i64,
    request: &str,
    action: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> mpsc::Receiver<Result<Value, String>> {
    let (reply, rx) = mpsc::channel();
    let call = mono::bridge::Call {
        session,
        request_id: request.into(),
        action: action.into(),
        input,
        reply,
    };
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.app_call(call, w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    rx
}
#[gpui::test]
fn settings_match_react_dialog_and_left_navigation_geometry(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let dialog = visual.debug_bounds("mono-settings-dialog").unwrap();
    let nav = visual.debug_bounds("mono-settings-navigation").unwrap();
    let viewport = s
        .window
        .update(visual, |_, w, _| w.viewport_size())
        .unwrap();
    assert_eq!(
        dialog.size.width + px(2.),
        px(880.).min(viewport.width - px(24.))
    );
    assert!((dialog.center().x - viewport.width / 2.).abs() <= px(1.));
    assert_eq!(nav.size.width, px(192.));
    for section in ["General", "Soul", "Memory"] {
        let row = visual
            .debug_bounds(Box::leak(
                format!("mono-settings-{section}").into_boxed_str(),
            ))
            .unwrap();
        let icon = visual
            .debug_bounds(Box::leak(
                format!("mono-settings-icon-{section}").into_boxed_str(),
            ))
            .unwrap();
        let label = visual
            .debug_bounds(Box::leak(
                format!("mono-settings-label-{section}").into_boxed_str(),
            ))
            .unwrap();
        assert_eq!(row.size.height, px(32.));
        assert_eq!(icon.left() - row.left(), px(10.));
        assert_eq!(label.left() - icon.right(), px(10.));
    }
    assert_eq!(
        visual.debug_bounds("mono-look").unwrap().size,
        gpui::size(px(80.), px(80.))
    );
    assert_eq!(
        visual.debug_bounds("mono-name-field").unwrap().size,
        gpui::size(px(256.), px(32.))
    );
}

#[gpui::test]
fn settings_color_popover_follows_its_mascot_when_the_window_resizes(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| {
        crate::shell::bind_keys(cx);
        crate::popover::bind(cx);
    });
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let face = visual.debug_bounds("mono-look").unwrap();
    visual.simulate_click(face.center(), Default::default());
    visual.run_until_parked();
    for size in [
        gpui::size(px(1300.), px(900.)),
        gpui::size(px(1050.), px(760.)),
    ] {
        visual.simulate_resize(size);
        visual.run_until_parked();
        let face = visual.debug_bounds("mono-look").unwrap();
        let picker = visual.debug_bounds("mono-settings-look-picker").unwrap();
        assert!((picker.center().x - face.center().x).abs() <= px(1.));
        assert!(picker.top() >= face.bottom());
    }
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.update(|_, cx| {
        let m = s.manager.read(cx);
        assert!(m.picker.is_none());
        assert!(m.settings.is_some());
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    visual.update(|_, cx| assert!(s.manager.read(cx).settings.is_none()));
}

#[gpui::test]
fn memory_editor_saves_with_enter_and_stays_open_for_the_next_fact(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(input::bind);
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = Some(SettingsSection::Memory);
                m.memory_adding = true;
                cx.notify();
            })
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.window
        .update(visual, |_, w, cx| {
            let field = s.manager.read(cx).fields["memory-fact"].clone();
            w.focus(&field.focus_handle(cx), cx);
        })
        .unwrap();
    visual.simulate_input("Keep UI aligned with React.");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert!(
        mono::files::read(&s.root, &s.id, None, "MEMORY.md")
            .unwrap()
            .text
            .unwrap()
            .contains("Keep UI aligned with React.")
    );
    visual.update(|_, cx| {
        let m = s.manager.read(cx);
        assert!(m.memory_adding);
        assert!(m.fields["memory-fact"].read(cx).content.is_empty());
    });
}

#[gpui::test]
fn overview_activity_returns_to_its_previous_tab_and_uses_the_shared_reveal(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    s.window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.select_overview(OverviewTab::Documents, cx);
                assert!(m.overview_reveal.is_some());
                m.select_overview(OverviewTab::Activity, cx);
                assert!(m.overview_previous == OverviewTab::Documents);
                m.select_overview(m.overview_previous, cx);
                assert!(m.overview_tab == OverviewTab::Documents);
                cx.set_reduce_motion(true);
                assert!(
                    m.overview_reveal
                        .as_ref()
                        .unwrap()
                        .progress(OverviewTab::Documents, window, cx)
                        .is_none()
                );
            });
        })
        .unwrap();
}

#[gpui::test]
fn floating_chat_opens_after_releasing_the_manager_and_shares_the_runtime(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.open_floating(s.id.clone(), cx))
    });
    cx.run_until_parked();
    cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.open_floating(s.id.clone(), cx))
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let manager = s.manager.read(cx);
        assert_eq!(manager.floating.len(), 1);
        assert_eq!(manager.chats.len(), 1);
        assert!(manager.chats.contains_key(&s.id));
        // The panel owns the chat; the workspace draws a placeholder rather
        // than the same transcript in a second window.
        assert!(manager.floating_shows(&s.id, cx));
    });
    cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.hide_floating(&s.id.clone(), cx))
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(!s.manager.read(cx).floating_shows(&s.id, cx)));
}

#[gpui::test]
fn floating_chat_leaves_the_workspace_selection_alone(cx: &mut TestAppContext) {
    let s = setup(cx);
    let other = cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            let before = m.active.clone();
            let id = m.create_floating(Vec::new(), cx);
            assert_eq!(
                m.active, before,
                "a panel's new Mono stays out of the workspace"
            );
            m.open_floating(id.clone(), cx);
            assert_eq!(m.active, before);
            id
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let manager = s.manager.read(cx);
        assert!(manager.floating_shows(&other, cx));
        assert!(!manager.floating_shows(&s.id, cx));
    });
}

#[gpui::test]
fn note_actions_preserve_tags_when_updating_the_body(cx: &mut TestAppContext) {
    let s = setup(cx);
    let note = call(
        &s,
        "note-create",
        "notes.write",
        json!({"title":"QA note","body":"First","tags":["work","mono"]}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    let updated = call(
        &s,
        "note-update",
        "notes.write",
        json!({"id":note["id"],"title":"QA note","body":"Second"}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert_eq!(updated["tags"], json!(["work", "mono"]));
    assert_eq!(updated["body"], "Second");
}
#[gpui::test]
fn identity_and_context_editors_survive_switches_and_save_to_their_own_files(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.settings = Some(SettingsSection::Soul);
            let mono = m.current().unwrap().clone();
            let _ = m.soul_page(&mono, cx);
            let key = format!("file:{}:SOUL.md", s.id);
            m.fields[&key].update(cx, |f, cx| {
                f.set_text("# Soul\nKeep replies concise.\n".into(), cx)
            });
            m.save_open_file(cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(
        mono::files::read(&s.root, &s.id, None, "SOUL.md")
            .unwrap()
            .text
            .as_deref(),
        Some("# Soul\nKeep replies concise.\n")
    );
    let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
    assert_eq!(saved.monos.len(), 1);
    assert_eq!(saved.monos[0].session_id, Some(s.session));
    assert!(saved.hidden_sessions.contains(&s.session));
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager
                .update(cx, |m, cx| m.create(vec![s.project.clone()], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            let mono = m.current().unwrap().clone();
            assert_ne!(mono.id, s.id);
            assert_ne!(mono.mascot, m.roster.get(&s.id).unwrap().mascot);
            m.settings = Some(SettingsSection::Soul);
            let _ = m.soul_page(&mono, cx);
            let key = format!("file:{}:SOUL.md", mono.id);
            assert!(
                !m.fields[&key]
                    .read(cx)
                    .content
                    .contains("Keep replies concise")
            );
        })
    });
}
#[gpui::test]
fn app_receipt_lookup_does_not_wait_for_storage_on_the_ui_thread(cx: &mut TestAppContext) {
    let s = setup(cx);
    let store = s.store.clone();
    let (held, acquired) = mpsc::sync_channel(0);
    let (release, released) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _guard = store.lock().unwrap();
        held.send(()).unwrap();
        // A regression must fail rather than hang the whole test suite.
        released.recv_timeout(Duration::from_secs(2)).is_ok()
    });
    acquired.recv().unwrap();
    let (reply, response) = mpsc::channel();
    let call = mono::bridge::Call {
        session: s.session,
        request_id: "contended-lookup".into(),
        action: "sessions.list".into(),
        input: json!({}),
        reply,
    };
    s.window
        .update(cx, |_, window, cx| {
            s.manager
                .update(cx, |manager, cx| manager.app_call(call, window, cx));
        })
        .unwrap();
    assert!(response.try_recv().is_err());
    let _ = release.send(());
    assert!(
        holder.join().unwrap(),
        "app request blocked the UI waiting for the store"
    );
    cx.run_until_parked();
    assert!(response.try_recv().unwrap().is_ok());
}

#[gpui::test]
fn durable_app_receipts_survive_cache_eviction_and_invalid_receipts_do_not_repeat_mutations(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let input = json!({"name":"Digest","instructions":"Summarize changes","schedule":{"kind":"daily","time":"09:00"}});
    let first = call(&s, "durable-habit", "habits.add", input.clone(), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    s.manager.update(cx, |m, _| m.app_results.clear());
    let retry = call(&s, "durable-habit", "habits.add", input, cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(first, retry);
    assert_eq!(habits::load(&s.root, &s.id).unwrap().len(), 1);
    assert!(
        call(
            &s,
            "durable-habit",
            "habits.add",
            json!({"name":"different"}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_err()
    );
    let key = format!(
        "mono-app-receipt:{}:{}",
        s.session,
        mono::files::content_hash("corrupt-habit")
    );
    locked(&s.store)
        .unwrap()
        .save_setting(&key, "invalid receipt")
        .unwrap();
    let error = call(&s, "corrupt-habit", "habits.add", json!({"name":"Another","instructions":"Summarize changes","schedule":{"kind":"daily","time":"09:00"}}), cx).try_recv().unwrap().unwrap_err();
    assert!(error.contains("Could not read app result"));
    assert_eq!(habits::load(&s.root, &s.id).unwrap().len(), 1);
}

#[gpui::test]
fn app_actions_are_project_scoped_and_mutation_retries_are_idempotent(cx: &mut TestAppContext) {
    let s = setup(cx);
    assert!(
        call(
            &s,
            "outside",
            "sessions.list",
            json!({"project":"/not-assigned"}),
            cx
        )
        .try_recv()
        .unwrap()
        .is_err()
    );
    let input = json!({"name":"Morning digest","instructions":"Summarize changes without editing","schedule":{"kind":"daily","time":"09:00"}});
    let first = call(&s, "habit", "habits.add", input.clone(), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    let retry = call(&s, "habit", "habits.add", input, cx)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(first, retry);
    assert_eq!(habits::load(&s.root, &s.id).unwrap().len(), 1);
    assert!(
        call(&s, "habit", "habits.add", json!({"name":"different"}), cx)
            .try_recv()
            .unwrap()
            .is_err()
    );
    let start=call(&s,"draft","sessions.start",json!({"project":s.project,"prompt":"Review the project","title":"Review draft","draft":true}),cx).try_recv().unwrap().unwrap();
    let child = start["sessionId"].as_i64().unwrap();
    cx.update(|cx| {
        let m = s.manager.read(cx);
        assert_eq!(m.roster.get(&s.id).unwrap().launches.len(), 1);
        let owner = m.owner.upgrade().unwrap();
        let view = owner.read(cx).mono_session_entity(child).unwrap();
        assert!(view.read(cx).has_saved_draft());
        assert_eq!(
            locked(&s.store)
                .unwrap()
                .follow_ups(child)
                .unwrap()
                .draft
                .unwrap()
                .text,
            "Review the project"
        );
        assert!(!view.read(cx).busy);
        assert!(m.hidden_session(child));
    });
}
#[gpui::test]
fn sidebar_hidden_launches_still_list_in_the_mono_panel_after_a_restart(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = call(
        &s,
        "hidden-launch",
        "sessions.start",
        json!({"project":s.project,"prompt":"Review quietly","title":"Quiet review","draft":true}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    let child = started["sessionId"].as_i64().unwrap();
    let owner = cx.update(|cx| s.manager.read(cx).owner.upgrade().unwrap());
    cx.update(|cx| {
        assert!(s.manager.read(cx).hidden_session(child));
        owner.update(cx, |shell, _| shell.mono_forget_session(child));
        assert!(
            !owner
                .read(cx)
                .mono_session_views(cx)
                .iter()
                .any(|v| v.id == child)
        );
    });
    s.manager.update(cx, |m, cx| m.configure_chat(&s.id, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let views = owner.read(cx).mono_session_views(cx);
        let view = views
            .iter()
            .find(|v| v.id == child)
            .expect("the launch lists again");
        assert!(view.title.contains("Quiet review"), "{}", view.title);
        assert!(
            owner
                .update(cx, |shell, cx| shell.mono_ensure_session(child, cx))
                .is_some()
        );
        assert!(s.manager.read(cx).hidden_session(child));
    });
}
#[gpui::test]
fn mono_app_launch_visibility_can_be_toggled_and_survives_roster_reload(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = call(
        &s,
        "visibility-draft",
        "sessions.start",
        json!({"project":s.project,"prompt":"Review quietly","draft":true}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    let child = started["sessionId"].as_i64().unwrap();
    cx.update(|cx| assert!(s.manager.read(cx).hidden_session(child)));
    s.manager
        .update(cx, |m, cx| m.sidebar_preferences(&s.id, true, true, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(!s.manager.read(cx).hidden_session(child));
        assert!(s.manager.read(cx).is_mono_session(s.session));
    });
    assert!(
        mono::Roster::load(&locked(&s.store).unwrap())
            .unwrap()
            .get(&s.id)
            .unwrap()
            .show_started_sessions_in_sidebar
    );
    s.manager
        .update(cx, |m, cx| m.sidebar_preferences(&s.id, false, false, cx));
    cx.run_until_parked();
    cx.update(|cx| assert!(s.manager.read(cx).hidden_session(child)));
    assert!(
        !mono::Roster::load(&locked(&s.store).unwrap())
            .unwrap()
            .get(&s.id)
            .unwrap()
            .show_started_sessions_in_sidebar
    );
}
#[gpui::test]
fn queued_messages_are_saved_before_dispatch_and_do_not_run_while_blocked(cx: &mut TestAppContext) {
    let s = setup(cx);
    let message = mono::new_id();
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            m.roster.get_mut(&s.id).unwrap().outbox.push(Message {
                sent_at: None,
                id: message.clone(),
                text: "Keep this queued".into(),
                internal: false,
                error: Some("Retry required".into()),
                attachments: Vec::new(),
            });
            m.save(cx);
            m.configure_chat(&s.id, cx);
        })
    });
    cx.run_until_parked();
    let saved = Roster::load(&locked(&s.store).unwrap()).unwrap();
    assert_eq!(saved.monos[0].outbox[0].id, message);
    assert_eq!(
        locked(&s.store)
            .unwrap()
            .load(s.session)
            .unwrap()
            .rows
            .len(),
        0
    );
}

#[gpui::test]
fn chat_composer_keeps_attach_under_the_box_and_opens_the_model_list(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.dismiss_settings(w, cx));
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let field = visual.debug_bounds("mono-composer").unwrap();
    let send = visual.debug_bounds("mono-send").unwrap();
    let attach = visual.debug_bounds("mono-attach").unwrap();
    let model = visual.debug_bounds("mono-model").unwrap();
    // MonoComposer with controls: send inside the box, attach and the model
    // on the row beneath it, the model at its right end.
    assert!(field.contains(&send.center()));
    assert!(attach.top() >= field.bottom());
    assert!(model.top() >= field.bottom());
    assert!(model.left() > attach.right());
    assert_eq!(send.size, gpui::size(px(26.), px(26.)));
    assert_eq!(attach.size, gpui::size(px(26.), px(26.)));
    let chat = visual.update(|_, cx| s.manager.read(cx).chat().unwrap());
    handle
        .update(visual, |_, w, cx| {
            chat.update(cx, |c, cx| c.toggle_model_menu(w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    visual.update(|_, cx| {
        let (models, menu) = chat.read(cx).mono_menus_open();
        assert!(models, "the trigger opens the model list itself");
        assert!(!menu, "not the session composer's settings menu");
    });
}

#[gpui::test]
fn details_tabs_share_the_session_titlebar_in_both_layouts(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::shell::bind_keys(cx);
    });
    let session = s
        .window
        .update(cx, |shell, w, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.focus_active(w, cx);
                cx.notify();
            });
            shell.workspace.active.unwrap()
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    for classic in [false, true] {
        s.window
            .update(visual, |shell, _, cx| {
                shell.workspace.new_layout_boxy = classic;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let panel = visual.debug_bounds("right-panel-frame").unwrap();
        let tabs = visual.debug_bounds("mono-overview-tabs").unwrap();
        let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
        let title_tabs = visual.debug_bounds("title-tab-strip").unwrap();
        assert!((tabs.top() - title_tabs.top()).abs() <= px(0.5));
        assert_eq!(tabs.size.height, px(TITLE_BAR_HEIGHT));
        assert_eq!(
            tabs.left(),
            panel.left() + px(if classic { 6. } else { 7. })
        );
        let activity = visual.debug_bounds("mono-activity").unwrap();
        let search = visual.debug_bounds("titlebar-file-search").unwrap();
        assert_eq!(search.left() - activity.right(), px(6.));
        assert_eq!(toggle.left() - search.right(), px(4.));
        assert!(tabs.bottom() <= panel.top() + px(if classic { TITLE_BAR_HEIGHT } else { 0. }));
        assert_eq!(
            visual.debug_bounds("mono-session-list").unwrap().top(),
            panel.top() + px(if classic { TITLE_BAR_HEIGHT } else { 0. })
        );
        assert!(
            visual.debug_bounds("mono-overview-toggle").is_none(),
            "only the shell owns the panel toggle"
        );
        assert!(title_tabs.right() <= tabs.left());
        s.window
            .update(visual, |shell, w, cx| shell.open(session, w, cx))
            .unwrap();
        visual.run_until_parked();
        if !s
            .window
            .update(visual, |shell, _, _| shell.workspace.right_panel_open)
            .unwrap()
        {
            let button = visual.debug_bounds("toggle-right-panel").unwrap();
            visual.simulate_click(button.center(), Default::default());
            visual.run_until_parked();
        }
        assert_eq!(
            visual.debug_bounds("toggle-right-panel").unwrap(),
            toggle,
            "the toggle stays put when changing tab types"
        );
        let selector = if classic {
            "right-panel-explorer"
        } else {
            "project-tabs-separator"
        };
        assert_eq!(
            visual.debug_bounds(selector).unwrap().center().y,
            tabs.center().y
        );
        s.window
            .update(visual, |_, w, cx| {
                s.manager.update(cx, |m, cx| m.open(&s.id, w, cx))
            })
            .unwrap();
        visual.run_until_parked();
    }
}

#[gpui::test]
fn titlebar_details_tabs_switch_pages_and_keep_activity_and_habit_actions(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 400.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                cx.notify();
            })
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    for (selector, tab) in [
        ("mono-overview-Documents", OverviewTab::Documents),
        ("mono-overview-Habits", OverviewTab::Habits),
    ] {
        let button = visual.debug_bounds(selector).unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        assert!(s.manager.read_with(visual, |m, _| m.overview_tab == tab));
        assert!(visual.debug_bounds(selector).unwrap().size.width > px(26.));
    }
    let add = visual.debug_bounds("mono-new-habit").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| matches!(m.route, Some(Route::NewHabit)))
    );
    assert!(visual.debug_bounds("mono-overview-tabs").is_some());
    let sessions = visual.debug_bounds("mono-overview-Sessions").unwrap();
    visual.simulate_click(sessions.center(), Default::default());
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.route.is_none()
        && m.overview_tab == OverviewTab::Sessions));
    for expected in [OverviewTab::Activity, OverviewTab::Sessions] {
        let activity = visual.debug_bounds("mono-activity").unwrap();
        visual.simulate_click(activity.center(), Default::default());
        visual.run_until_parked();
        assert!(
            s.manager
                .read_with(visual, |m, _| m.overview_tab == expected)
        );
    }
}

#[gpui::test]
fn titlebar_divider_resizes_mono_details_and_closed_panels_keep_the_corner(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |shell, _, cx| {
            // The titlebar divider belongs to the rounded frame.
            shell.workspace.new_layout_boxy = false;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                cx.notify();
            })
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    for width in [220., 280., 400., 500.] {
        s.window
            .update(visual, |shell, _, cx| {
                shell.workspace.right_panel_width = width;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("project-tabs-separator")
                .unwrap()
                .left(),
            visual.debug_bounds("right-panel-frame").unwrap().left()
        );
        let activity = visual.debug_bounds("mono-activity").unwrap();
        let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
        assert!(
            activity.right() < toggle.left(),
            "Mono controls fit the session sidebar's minimum width"
        );
    }
    let start = visual
        .debug_bounds("titlebar-right-panel-resize")
        .unwrap()
        .center();
    let end = start - gpui::point(px(40.), px(0.));
    visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Default::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Default::default());
    visual.run_until_parked();
    s.window
        .update(visual, |shell, _, _| {
            assert_eq!(shell.workspace.right_panel_width, 540.);
        })
        .unwrap();
    assert_eq!(
        visual
            .debug_bounds("project-tabs-separator")
            .unwrap()
            .left(),
        visual.debug_bounds("right-panel-frame").unwrap().left()
    );
    for classic in [false, true] {
        s.window
            .update(visual, |shell, _, cx| {
                shell.workspace.new_layout_boxy = classic;
                shell.git.update(cx, |g, cx| g.set_project("~".into(), cx));
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let open = visual.debug_bounds("toggle-right-panel").unwrap();
        visual.simulate_click(open.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("mono-overview-tabs").is_none());
        assert!(visual.debug_bounds("mono-overview-panel").is_none());
        assert_eq!(visual.debug_bounds("toggle-right-panel").unwrap(), open);
        visual.simulate_click(open.center(), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("mono-overview-tabs").is_some());
    }
}

#[gpui::test]
fn opening_a_session_card_uses_the_shared_sidebar_state(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let session = s
        .window
        .update(cx, |shell, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                cx.notify();
            });
            shell.workspace.active.unwrap()
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let toggle = visual.debug_bounds("toggle-right-panel").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("right-panel-frame").is_none());
    s.window
        .update(visual, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.card_action(&s.id, &json!({"type":"session", "sessionId":session}), cx);
            })
        })
        .unwrap();
    visual.run_until_parked();
    let details = visual.debug_bounds("right-panel-frame").unwrap();
    assert!(s.manager.read_with(
        visual,
        |m, _| matches!(m.route, Some(Route::Session(id)) if id == session)
    ));
    s.window
        .update(visual, |shell, w, cx| {
            assert!(shell.workspace.right_panel_open);
            shell.open(session, w, cx);
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(visual.debug_bounds("right-panel-frame").unwrap(), details);
}

#[gpui::test]
fn long_habit_draft_keeps_fields_and_schedule_inside_the_scrolling_page(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        input::bind(cx);
        crate::popover::bind(cx);
    });
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 220.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.route = Some(Route::NewHabit);
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.fields["habit-name"].update(cx, |f, cx| {
            f.set_text(
                "A long but valid habit name that fills the narrow panel".into(),
                cx,
            )
        });
        m.fields["habit-instructions"].update(cx, |f, cx| {
            f.set_text(
                "Report useful updates on the current project.\n".repeat(65),
                cx,
            )
        });
        m.fields["habit-kind"].update(cx, |f, cx| f.set_text("weekly".into(), cx));
        cx.notify();
    });
    visual.run_until_parked();
    let body = visual.debug_bounds("mono-habit-editor").unwrap();
    let name = visual.debug_bounds("mono-habit-name-field").unwrap();
    let schedule = visual.debug_bounds("mono-habit-schedule").unwrap();
    let prompt = visual.debug_bounds("mono-habit-prompt-field").unwrap();
    let create = visual.debug_bounds("mono-habit-save").unwrap();
    assert!(name.size.height >= px(36.));
    assert!(schedule.top() >= name.bottom());
    assert!(prompt.top() > schedule.bottom());
    assert!(prompt.size.height >= px(300.));
    assert!(prompt.size.height <= px(328.));
    for bounds in [name, schedule, prompt] {
        assert!(bounds.left() >= body.left());
        assert!(bounds.right() <= body.right() + px(1.));
    }
    assert!(create.bottom() <= body.top());
    let day = visual.debug_bounds("mono-habit-weekday-picker").unwrap();
    assert!(day.left() >= schedule.left());
    assert!(day.right() <= schedule.right() + px(1.));
    visual.simulate_click(day.center(), Default::default());
    visual.run_until_parked();
    // Escape returns focus to the trigger; arrow keys reopen the list with
    // the current weekday highlighted, and Enter retains that selection.
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_none()));
    visual.simulate_keystrokes("down");
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_some()));
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("habit-weekday", cx)),
        "1"
    );
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_none()));
    visual.simulate_click(day.center(), Default::default());
    visual.run_until_parked();
    let saturday = visual.debug_bounds("menu-action-6").unwrap();
    visual.simulate_click(saturday.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("habit-weekday", cx)),
        "6"
    );
    // Invalid schedules keep the full draft in place instead of navigating away.
    s.manager.update(visual, |m, cx| {
        m.fields["habit-time"].update(cx, |f, cx| f.set_text("25:00".into(), cx));
    });
    visual.run_until_parked();
    visual.simulate_click(create.center(), Default::default());
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| m.route == Some(Route::NewHabit))
    );
    assert!(s.manager.read_with(visual, |m, cx| {
        m.fields["habit-instructions"].read(cx).content.len() > 2000
    }));
    assert!(habits::load(&s.root, &s.id).unwrap().is_empty());
    s.manager.update(visual, |m, cx| {
        m.fields["habit-time"].update(cx, |f, cx| f.set_text("09:00".into(), cx));
    });
    visual.run_until_parked();
    visual.simulate_click(create.center(), Default::default());
    visual.run_until_parked();
    let saved = habits::load(&s.root, &s.id).unwrap();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].instructions.len() > 2000);
    assert_eq!(saved[0].schedule.schedule_kind, "weekly");
    assert_eq!(saved[0].schedule.day_of_week, 6);
    assert!(s.manager.read_with(visual, |m, _| m.route.is_none()));
}

#[gpui::test]
fn interval_habit_editor_builds_an_anchored_schedule_and_saves_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        input::bind(cx);
        crate::popover::bind(cx);
    });
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 320.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.route = Some(Route::NewHabit);
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    // Other kinds carry no interval fields and have no hours picker.
    assert!(visual.debug_bounds("mono-habit-hours-picker").is_none());
    s.manager.update(visual, |m, cx| {
        let daily = m.habit_schedule(cx);
        assert_eq!((daily.interval_hours, daily.interval_from), (None, None));
        m.fields["habit-name"].update(cx, |f, cx| f.set_text("Every few hours".into(), cx));
        m.fields["habit-instructions"].update(cx, |f, cx| f.set_text("Check builds.".into(), cx));
        m.fields["habit-kind"].update(cx, |f, cx| f.set_text("interval".into(), cx));
        m.fields["habit-time"].update(cx, |f, cx| f.set_text("08:30".into(), cx));
        cx.notify();
    });
    visual.run_until_parked();
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let draft = s.manager.read_with(visual, |m, cx| m.habit_schedule(cx));
    assert_eq!(draft.schedule_kind, "interval");
    assert_eq!(draft.interval_hours, Some(4));
    assert_eq!(draft.interval_from.as_deref(), Some(today.as_str()));
    assert_eq!(draft.time, "08:30");
    assert_eq!(draft.label(), "Every 4 hours from 08:30");
    let schedule = visual.debug_bounds("mono-habit-schedule").unwrap();
    let hours = visual.debug_bounds("mono-habit-hours-picker").unwrap();
    assert!(hours.left() >= schedule.left());
    assert!(hours.right() <= schedule.right() + px(1.));
    visual.simulate_click(hours.center(), Default::default());
    visual.run_until_parked();
    let six = visual.debug_bounds("menu-action-6").unwrap();
    visual.simulate_click(six.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, cx| m.value("habit-hours", cx)),
        "6"
    );
    let create = visual.debug_bounds("mono-habit-save").unwrap();
    visual.simulate_click(create.center(), Default::default());
    visual.run_until_parked();
    let saved = habits::load(&s.root, &s.id).unwrap();
    assert_eq!(saved.len(), 1);
    let schedule = &saved[0].schedule;
    assert_eq!(schedule.schedule_kind, "interval");
    assert_eq!(schedule.interval_hours, Some(6));
    assert_eq!(schedule.interval_from.as_deref(), Some(today.as_str()));
    assert_eq!(schedule.time, "08:30");
    assert_eq!(schedule.label(), "Every 6 hours from 08:30");
    assert!(saved[0].next_run_at > monocode_core::time::now_ms());
    assert!(s.manager.read_with(visual, |m, _| m.route.is_none()));
}

#[gpui::test]
fn existing_interval_habit_keeps_its_hours_time_and_anchor_through_page_actions(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let mut h = Habit::new(
        "Interval".into(),
        "Check builds.".into(),
        habits::Schedule {
            schedule_kind: "interval".into(),
            time: "08:30".into(),
            interval_hours: Some(1),
            ..Default::default()
        },
        monocode_core::time::now_ms(),
    )
    .unwrap();
    h.id = "interval-keep".into();
    let before = h.schedule.clone();
    assert_eq!(before.label(), "Every hour from 08:30");
    habits::update(&s.root, &s.id, |all| {
        all.push(h.clone());
        Ok(())
    })
    .unwrap();
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.habits = vec![h.clone()];
                m.route = Some(Route::Habit(h.id.clone()));
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    // Pause and resume rewrite the file and recompute the next run.
    for _ in 0..2 {
        let toggle = visual.debug_bounds("mono-habit-toggle").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
    }
    let saved = habits::load(&s.root, &s.id).unwrap();
    assert_eq!(saved[0].schedule, before);
    assert!(saved[0].enabled);
    assert!(saved[0].next_run_at > monocode_core::time::now_ms());
}

#[gpui::test]
fn habit_detail_preserves_long_content_history_and_actions_without_edit_fields(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let mut h = Habit::new(
        "A long habit title that should truncate before the action buttons".into(),
        "Review changes and report anything that needs attention.".into(),
        habits::Schedule::default(),
        monocode_core::time::now_ms(),
    )
    .unwrap();
    h.id = "layout-history".into();
    h.runs.push(habits::Run {
        at: monocode_core::time::now_ms() - 60_000,
        duration_ms: 72_000,
        outcome: "posted".into(),
        report: Some("**Result**\n\nThe project is ready to review.".into()),
        error: None,
    });
    habits::update(&s.root, &s.id, |all| {
        all.push(h.clone());
        Ok(())
    })
    .unwrap();
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 280.;
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.habits = vec![h.clone()];
                m.route = Some(Route::Habit(h.id.clone()));
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(
        s.manager
            .read_with(visual, |m, _| !m.fields.contains_key("habit-name"))
    );
    let body = visual.debug_bounds("mono-habit-editor").unwrap();
    let description = visual.debug_bounds("mono-habit-instructions").unwrap();
    let row_id = format!("mono-habit-history-{}-{}", h.id, h.runs[0].at);
    let row = visual
        .debug_bounds(Box::leak(row_id.into_boxed_str()))
        .unwrap();
    assert!(row.top() > description.bottom());
    assert!(description.right() <= body.right());
    for selector in ["mono-habit-run", "mono-habit-toggle", "mono-habit-delete"] {
        let action = visual.debug_bounds(selector).unwrap();
        assert!(action.bottom() <= body.top());
        assert!(action.right() <= body.right());
    }
    visual.simulate_click(row.center(), Default::default());
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds(Box::leak(
                format!("mono-habit-report-{}-{}", h.id, h.runs[0].at).into_boxed_str()
            ))
            .is_some()
    );
    let toggle = visual.debug_bounds("mono-habit-toggle").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert!(!habits::load(&s.root, &s.id).unwrap()[0].enabled);
    assert!(
        s.manager
            .read_with(visual, |m, _| m.route == Some(Route::Habit(h.id.clone())))
    );
    let remove = visual.debug_bounds("mono-habit-delete").unwrap();
    visual.simulate_click(remove.center(), Default::default());
    visual.run_until_parked();
    assert!(habits::load(&s.root, &s.id).unwrap().is_empty());
    assert!(s.manager.read_with(visual, |m, _| m.route.is_none()));
}

#[gpui::test]
fn new_habit_click_transfers_composer_focus_after_the_animated_page_mounts(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(false));
    s.window
        .update(cx, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.route = None;
                m.focus_active(window, cx);
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.window
        .update(visual, |_, window, cx| {
            s.manager.read_with(cx, |m, cx| {
                assert!(
                    m.chat()
                        .unwrap()
                        .read(cx)
                        .input
                        .focus_handle(cx)
                        .is_focused(window)
                );
            });
        })
        .unwrap();
    let advance_focus_frame = |visual: &mut gpui::VisualTestContext| {
        visual
            .update_window(s.window.into(), |_, window, cx| {
                // Use the untyped window context: callbacks and drawing may
                // update the root Shell, so it must not already be borrowed.
                // Parking executor tasks does not deliver platform frame callbacks.
                // Flush the requested next frame before drawing its focused field.
                assert!(window.simulate_next_frame(cx) > 0);
                window.draw(cx).clear(cx);
            })
            .unwrap();
        visual.run_until_parked();
    };
    // Follow the actual header action, with the old composer still focused.
    let add = visual.debug_bounds("mono-new-habit").unwrap();
    visual.simulate_click(add.center(), Default::default());
    visual.run_until_parked();
    advance_focus_frame(visual);
    s.window
        .update(visual, |_, window, cx| {
            s.manager.read_with(cx, |m, cx| {
                assert_eq!(m.route, Some(Route::NewHabit));
                assert!(m.fields["habit-name"].focus_handle(cx).is_focused(window));
                assert!(
                    !m.chat()
                        .unwrap()
                        .read(cx)
                        .input
                        .focus_handle(cx)
                        .is_focused(window)
                );
            });
        })
        .unwrap();
    let page = "mono-detail-page-new-habit";
    assert!(visual.debug_bounds(page).is_some());
    // Complete the real page transition, then leave/reopen through its controls.
    for reopen in [false, true] {
        visual
            .update_window(s.window.into(), |_, window, cx| {
                let details = s.manager.read(cx).details.clone();
                details.update(cx, |v, cx| v.elapsed(std::time::Duration::from_secs(1), cx));
                window.draw(cx).clear(cx);
            })
            .unwrap();
        let back = visual.debug_bounds("mono-page-back").unwrap();
        visual.simulate_click(back.center(), Default::default());
        visual.run_until_parked();
        assert!(s.manager.read_with(visual, |m, _| m.route.is_none()));
        if !reopen {
            let add = visual.debug_bounds("mono-new-habit").unwrap();
            visual.simulate_click(add.center(), Default::default());
            visual.run_until_parked();
            advance_focus_frame(visual);
            s.window
                .update(visual, |_, window, cx| {
                    s.manager.read_with(cx, |m, cx| {
                        assert!(m.fields["habit-name"].focus_handle(cx).is_focused(window))
                    });
                })
                .unwrap();
        }
    }
}

#[gpui::test]
fn sidebar_mono_only_marks_unread_replies_and_swaps_the_mark_for_options_on_hover(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.close(cx);
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    let unread_selector: &'static str =
        Box::leak(format!("mono-rail-unread-{}", s.id).into_boxed_str());
    let options_selector: &'static str =
        Box::leak(format!("mono-rail-options-{}", s.id).into_boxed_str());
    let row_selector: &'static str = Box::leak(format!("mono-rail-{}", s.id).into_boxed_str());
    let away = gpui::point(px(5.), px(5.));
    visual.simulate_mouse_move(away, None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    let row = visual.debug_bounds(row_selector).unwrap();
    assert!(visual.debug_bounds(unread_selector).is_none());
    assert!(visual.debug_bounds(options_selector).is_none());
    // Check the painted output too: an idle Mono used to paint a gray
    // status dot here, even with no unread reply and no pointer over it.
    s.window
        .update(visual, |_, window, _| {
            let scale = window.scale_factor();
            assert!(!window.painted_quads().iter().any(|quad| {
                let bounds = quad.bounds;
                bounds.left().0 >= f32::from(row.right() - px(28.)) * scale
                    && bounds.right().0 <= f32::from(row.right() - px(4.)) * scale
                    && bounds.top().0 >= f32::from(row.top()) * scale
                    && bounds.bottom().0 <= f32::from(row.bottom()) * scale
                    && (3. * scale..=8. * scale).contains(&bounds.size.width.0)
                    && (3. * scale..=8. * scale).contains(&bounds.size.height.0)
            }));
        })
        .unwrap();

    let post = |role, text: &str, visual: &mut gpui::VisualTestContext| {
        s.window
            .update(visual, |_, _, cx| {
                let chat = s.manager.read(cx).chats[&s.id].clone();
                chat.update(cx, |v, cx| v.mono_post(role, text.into(), cx))
                    .unwrap();
            })
            .unwrap();
        visual.run_until_parked();
        visual.refresh().unwrap();
    };
    post("user", "My question", visual);
    assert!(visual.debug_bounds(unread_selector).is_none());
    post("assistant", "A new reply", visual);
    let unread = visual
        .debug_bounds(unread_selector)
        .expect("an unseen assistant reply has an accent circle");
    assert_eq!(unread.size, gpui::size(px(6.), px(6.)));
    assert!(visual.debug_bounds(options_selector).is_none());
    let before_hover = visual.debug_bounds(row_selector).unwrap();
    visual.simulate_mouse_move(before_hover.center(), None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds(unread_selector).is_none(),
        "options replace the unread circle on hover"
    );
    let options = visual.debug_bounds(options_selector).unwrap();
    assert_eq!(options.size, gpui::size(px(24.), px(24.)));
    assert_eq!(options.center(), unread.center());
    assert_eq!(visual.debug_bounds(row_selector).unwrap(), before_hover);
    visual.simulate_click(options.center(), Default::default());
    visual.run_until_parked();
    s.manager.read_with(visual, |m, _| {
        assert!(m.menu.is_some(), "the hover action opens Mono options");
        assert!(m.active.is_none(), "options must not open the Mono");
    });

    s.window
        .update(visual, |_, window, cx| {
            s.manager.update(cx, |m, cx| {
                m.menu = None;
                m.open(&s.id, window, cx);
            });
        })
        .unwrap();
    visual.simulate_mouse_move(away, None, Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds(unread_selector).is_none());
    s.window
        .update(visual, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                assert_eq!(
                    m.roster.get(&s.id).unwrap().seen_rows,
                    m.chats[&s.id].read(cx).mono_rows(cx).len()
                );
                m.close(cx);
            });
        })
        .unwrap();
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(
        visual.debug_bounds(unread_selector).is_none(),
        "a reply stays read when the Mono closes"
    );
}

#[gpui::test]
fn sidebar_right_click_opens_mono_menu_outside_the_mono_and_edits_its_look(
    cx: &mut TestAppContext,
) {
    cx.update(input::bind);
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    // Leave the Mono, as when a session is open: only the rail remains.
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.active = None;
                cx.notify();
            })
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let row = s
        .manager
        .read_with(visual, |m, _| m.rail_bounds[&s.id].get())
        .expect("the sidebar lists the Mono");
    let at = row.center();
    visual.simulate_event(gpui::MouseDownEvent {
        position: at,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
        first_mouse: false,
    });
    visual.simulate_event(gpui::MouseUpEvent {
        position: at,
        button: MouseButton::Right,
        modifiers: Default::default(),
        click_count: 1,
    });
    visual.run_until_parked();
    // MonoMenu opens beside the pointer with ten mascots and nine presets
    // plus the custom swatch, all in single rows.
    let first = visual
        .debug_bounds("mono-mascot-0")
        .expect("right-click opens the Mono's menu");
    let last = visual.debug_bounds("mono-mascot-9").unwrap();
    assert_eq!(first.top(), last.top());
    assert_eq!(first.size.height, px(32.));
    assert!(first.left() >= at.x);
    let custom = visual.debug_bounds("mono-custom-color").unwrap();
    assert_eq!(
        visual.debug_bounds("mono-color-0").unwrap().top(),
        custom.top()
    );
    let pink = visual.debug_bounds("mono-color-4").unwrap().center();
    visual.simulate_click(pink, Default::default());
    visual.run_until_parked();
    let color = s
        .manager
        .read_with(visual, |m, _| m.roster.get(&s.id).unwrap().color);
    assert_eq!(color, mono::COLORS_MONO[4]);
    // Enter in the name field commits and closes the menu.
    let name = visual.debug_bounds("mono-options-name").unwrap();
    visual.simulate_click(name.center(), Default::default());
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    s.manager
        .read_with(visual, |m, _| assert!(m.menu.is_none()));
    visual.refresh().unwrap();
    visual
        .update_window(s.window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
    assert!(visual.debug_bounds("mono-mascot-0").is_none());
}

#[gpui::test]
fn sessions_a_mono_makes_are_chats_whatever_the_preference(cx: &mut TestAppContext) {
    let s = setup(cx);
    s.window
        .update(cx, |shell, _, _| {
            crate::shell::surface_tests::all_terminal(shell)
        })
        .unwrap();
    // A new Mono's own conversation.
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager
                .update(cx, |m, cx| m.create(vec![s.project.clone()], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    let second = cx.update(|cx| s.manager.read(cx).current().unwrap().session_id.unwrap());
    assert_ne!(second, s.session);
    // A session a Mono starts for a project.
    let start = call(
        &s,
        "start",
        "sessions.start",
        json!({"project":s.project,"prompt":"Review the project","draft":true}),
        cx,
    )
    .try_recv()
    .unwrap()
    .unwrap();
    let child = start["sessionId"].as_i64().unwrap();
    for id in [second, child] {
        assert_eq!(
            locked(&s.store).unwrap().session_surface(id).unwrap(),
            monocode_core::surface::Surface::Chat,
            "{id}"
        );
    }
    cx.update(|cx| {
        let owner = s.manager.read(cx).owner.upgrade().unwrap();
        let shell = owner.read(cx);
        assert!(shell.mono_session_entity(child).is_some());
    });
}

// sessions.start: folders, parent sessions and the no-commit worktree check
// (ports of agentApp.test.ts's folder, parent and "no commits" cases).
fn start(s: &Setup, request: &str, input: Value, cx: &mut TestAppContext) -> Result<Value, String> {
    call(s, request, "sessions.start", input, cx)
        .try_recv()
        .unwrap()
}
pub(super) fn session_count(s: &Setup) -> usize {
    locked(&s.store).unwrap().sessions().unwrap().len()
}
pub(super) fn groups_of(
    s: &Setup,
    cx: &mut TestAppContext,
) -> monocode_core::session_groups::Groups {
    cx.update(|cx| {
        s.manager
            .read(cx)
            .owner
            .upgrade()
            .unwrap()
            .read(cx)
            .mono_groups(&s.project)
    })
}
pub(super) fn edit_mono(s: &Setup, cx: &mut TestAppContext, edit: impl FnOnce(&mut mono::Mono)) {
    cx.update(|cx| {
        s.manager.update(cx, |m, cx| {
            edit(m.roster.get_mut(&s.id).unwrap());
            m.save(cx);
        })
    });
}
fn seed_folder(s: &Setup, cx: &mut TestAppContext, id: &str, name: &str, sessions: &[i64]) {
    let mut groups = groups_of(s, cx);
    let folder = groups.create_folder(sessions, id.into()).unwrap();
    groups.rename_folder(&folder, name);
    cx.update(|cx| {
        let owner = s.manager.read(cx).owner.upgrade().unwrap();
        owner.update(cx, |o, cx| {
            o.mono_apply_groups(s.project.clone(), groups, cx)
        });
    });
}
fn visible_launches(s: &Setup, cx: &mut TestAppContext) {
    edit_mono(s, cx, |m| m.show_started_sessions_in_sidebar = true);
}
fn launch_count(s: &Setup, cx: &mut TestAppContext) -> usize {
    cx.update(|cx| s.manager.read(cx).roster.get(&s.id).unwrap().launches.len())
}
fn git_init(dir: &str) {
    let out = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
}
pub(super) fn started_link(s: &Setup, child: i64) -> Option<i64> {
    locked(&s.store)
        .unwrap()
        .started_links()
        .unwrap()
        .get(&child)
        .copied()
}

#[gpui::test]
fn worktree_start_in_a_project_without_commits_is_rejected_before_anything_exists(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    git_init(&s.project);
    visible_launches(&s, cx);
    let before = session_count(&s);
    let error = start(
        &s,
        "no-commit",
        json!({"project":s.project,"prompt":"Add a file","draft":true,"workspaceMode":"worktree","folderName":"Tickets"}),
        cx,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "This project has no commits yet; make an initial commit before using workspaceMode worktree"
    );
    assert_eq!(session_count(&s), before, "no orphan session row");
    assert_eq!(launch_count(&s, cx), 0);
    assert!(groups_of(&s, cx).folders.is_empty());
    // The current checkout still starts.
    let started = start(
        &s,
        "current",
        json!({"project":s.project,"prompt":"Add a file","draft":true,"workspaceMode":"current"}),
        cx,
    )
    .unwrap();
    assert!(started["sessionId"].as_i64().is_some());
    assert_eq!(session_count(&s), before + 1);
}

#[gpui::test]
fn failed_worktree_start_leaves_no_session_row(cx: &mut TestAppContext) {
    let s = setup(cx);
    let before = session_count(&s);
    // Not a repository: the worktree cannot be created.
    assert!(
        start(
            &s,
            "worktree",
            json!({"project":s.project,"prompt":"x","draft":true,"workspaceMode":"worktree"}),
            cx,
        )
        .is_err()
    );
    assert_eq!(session_count(&s), before);
    assert_eq!(launch_count(&s, cx), 0);
    // An unknown worktreeCwd is refused before the row exists as well.
    let error = start(
        &s,
        "cwd",
        json!({"project":s.project,"prompt":"x","draft":true,"worktreeCwd":"/not/a/worktree"}),
        cx,
    )
    .unwrap_err();
    assert!(!error.is_empty());
    assert_eq!(session_count(&s), before);
}

#[gpui::test]
fn start_files_into_an_existing_folder_without_renaming_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    visible_launches(&s, cx);
    seed_folder(&s, cx, "f1", "Ticket 1", &[9001]);
    let result = start(
        &s,
        "dev",
        json!({"project":s.project,"prompt":"Fix","draft":true,"folderId":"f1","folderName":"Something else"}),
        cx,
    )
    .unwrap();
    let child = result["sessionId"].as_i64().unwrap();
    assert_eq!(result["folderId"], "f1");
    assert_eq!(result["folderName"], "Ticket 1");
    let groups = groups_of(&s, cx);
    assert_eq!(groups.folders.len(), 1);
    assert_eq!(groups.folders[0].name, "Ticket 1");
    assert_eq!(groups.folders[0].session_ids, vec![9001, child]);
    assert_eq!(
        locked(&s.store).unwrap().groups(&s.project).unwrap(),
        groups,
        "the folder is saved"
    );
}

#[gpui::test]
fn start_creates_a_folder_for_an_unknown_id_with_a_name_and_the_mono_color(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    visible_launches(&s, cx);
    edit_mono(&s, cx, |m| m.color = mono::COLORS_MONO[2]);
    let result = start(
        &s,
        "dev",
        json!({"project":s.project,"prompt":"Fix","draft":true,"folderId":"gone","folderName":"  Ticket 2 "}),
        cx,
    )
    .unwrap();
    let child = result["sessionId"].as_i64().unwrap();
    let groups = groups_of(&s, cx);
    assert_eq!(groups.folders.len(), 1);
    let folder = &groups.folders[0];
    assert_eq!(folder.name, "Ticket 2");
    assert_eq!(folder.session_ids, vec![child]);
    assert_eq!(
        folder.color_index,
        Some(3),
        "the palette position the Mono default folder uses"
    );
    assert_eq!(result["folderId"], folder.id.as_str());
    assert_eq!(result["folderName"], "Ticket 2");
    assert_ne!(folder.id, "gone");
}

#[gpui::test]
fn start_rejects_an_unknown_folder_id_alone_before_creating_a_session(cx: &mut TestAppContext) {
    let s = setup(cx);
    visible_launches(&s, cx);
    let before = session_count(&s);
    let error = start(
        &s,
        "dev",
        json!({"project":s.project,"prompt":"Fix","draft":true,"folderId":"gone"}),
        cx,
    )
    .unwrap_err();
    assert_eq!(error, "Folder was not found in this project");
    assert_eq!(session_count(&s), before);
    assert_eq!(launch_count(&s, cx), 0);
}

#[gpui::test]
fn launches_sharing_a_folder_name_never_merge(cx: &mut TestAppContext) {
    let s = setup(cx);
    visible_launches(&s, cx);
    for request in ["a", "b"] {
        start(
            &s,
            request,
            json!({"project":s.project,"prompt":"Fix","draft":true,"folderName":"Ticket 7 (2026-10-21)"}),
            cx,
        )
        .unwrap();
    }
    let groups = groups_of(&s, cx);
    assert_eq!(groups.folders.len(), 2);
    assert!(
        groups
            .folders
            .iter()
            .all(|f| f.name == "Ticket 7 (2026-10-21)")
    );
    assert_ne!(groups.folders[0].id, groups.folders[1].id);
}

#[gpui::test]
fn start_rejects_bad_folder_fields_before_creating_a_session(cx: &mut TestAppContext) {
    let s = setup(cx);
    visible_launches(&s, cx);
    let before = session_count(&s);
    for fields in [
        json!({"folderName":"n".repeat(101)}),
        json!({"folderName":""}),
        json!({"folderName":"   "}),
        json!({"folderName":5}),
        json!({"folderId":5}),
    ] {
        let mut input = json!({"project":s.project,"prompt":"Fix","draft":true});
        input
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let error = start(&s, "dev", input, cx).unwrap_err();
        assert!(error.contains("must be a non-empty string"), "{error}");
    }
    assert_eq!(session_count(&s), before);
    assert!(groups_of(&s, cx).folders.is_empty());
}

#[gpui::test]
fn a_launch_hidden_from_the_sidebar_is_validated_but_not_filed(cx: &mut TestAppContext) {
    let s = setup(cx);
    // show_started_sessions_in_sidebar is off by default.
    let result = start(
        &s,
        "dev",
        json!({"project":s.project,"prompt":"Fix","draft":true,"folderName":"Ticket"}),
        cx,
    )
    .unwrap();
    assert!(groups_of(&s, cx).folders.is_empty());
    assert!(result.get("folderId").is_none());
    assert!(result.get("folderName").is_none());
    let before = session_count(&s);
    let error = start(
        &s,
        "dev2",
        json!({"project":s.project,"prompt":"Fix","draft":true,"folderId":"gone"}),
        cx,
    )
    .unwrap_err();
    assert_eq!(error, "Folder was not found in this project");
    assert_eq!(session_count(&s), before);
}

#[gpui::test]
fn the_mono_default_folder_stays_when_no_folder_field_is_given(cx: &mut TestAppContext) {
    let s = setup(cx);
    edit_mono(&s, cx, |m| {
        m.show_started_sessions_in_sidebar = true;
        m.use_sidebar_folders = true;
    });
    let result = start(
        &s,
        "dev",
        json!({"project":s.project,"prompt":"Fix","draft":true}),
        cx,
    )
    .unwrap();
    let child = result["sessionId"].as_i64().unwrap();
    let groups = groups_of(&s, cx);
    assert_eq!(groups.folders.len(), 1);
    assert_eq!(groups.folders[0].session_ids, vec![child]);
    assert!(result.get("folderId").is_none());
    assert!(result.get("folderName").is_none());
}

#[gpui::test]
fn start_under_a_parent_records_the_link_and_files_into_the_parent_folder(cx: &mut TestAppContext) {
    let s = setup(cx);
    visible_launches(&s, cx);
    let dev = start(
        &s,
        "dev",
        json!({"project":s.project,"prompt":"Fix","draft":true}),
        cx,
    )
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    seed_folder(&s, cx, "f1", "Ticket 1", &[dev]);
    let review = start(
        &s,
        "rev",
        json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":dev,"folderId":"f1"}),
        cx,
    )
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    assert_eq!(started_link(&s, review), Some(dev));
    assert_eq!(started_link(&s, dev), None);
    assert_eq!(groups_of(&s, cx).folders[0].session_ids, vec![dev, review]);
    assert_eq!(launch_count(&s, cx), 2);
    cx.update(|cx| {
        let owner = s.manager.read(cx).owner.upgrade().unwrap();
        assert_eq!(owner.read(cx).mono_started_by(review), Some(dev));
    });
    // The id may also be sent as numeric text.
    let again = start(
        &s,
        "rev2",
        json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":dev.to_string()}),
        cx,
    )
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    assert_eq!(started_link(&s, again), Some(dev));
}

#[gpui::test]
fn start_rejects_a_parent_this_mono_did_not_start(cx: &mut TestAppContext) {
    let s = setup(cx);
    let user_session = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    // Another Mono's launch.
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| {
            s.manager
                .update(cx, |m, cx| m.create(vec![s.project.clone()], w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    let other_launch = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            let other = m.roster.monos.iter_mut().find(|x| x.id != s.id).unwrap();
            other.launches.push(mono::Launch {
                session_id: other_launch,
                cwd: s.project.clone(),
                title: "x".into(),
                harness: "codex".into(),
                model: None,
                origin_turn: 0,
                notify_on_complete: false,
                each: false,
                completed: false,
                dismissed: false,
                visible: false,
            });
        })
    });
    let before = session_count(&s);
    for parent in [user_session, s.session, 987_654, other_launch] {
        let error = start(
            &s,
            &format!("rev-{parent}"),
            json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":parent}),
            cx,
        )
        .unwrap_err();
        assert_eq!(
            error, "parentSessionId must be a session this Mono started",
            "{parent}"
        );
    }
    assert_eq!(session_count(&s), before);
    assert_eq!(launch_count(&s, cx), 0);
}

#[gpui::test]
fn start_rejects_a_parent_outside_the_project_or_already_a_child(cx: &mut TestAppContext) {
    let s = setup(cx);
    let dev = start(
        &s,
        "dev",
        json!({"project":s.project,"prompt":"Fix","draft":true}),
        cx,
    )
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    let review = start(
        &s,
        "rev",
        json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":dev}),
        cx,
    )
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    let before = session_count(&s);
    let error = start(
        &s,
        "deep",
        json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":review}),
        cx,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "parentSessionId cannot be a session started under another one; a child of a child is not allowed"
    );
    // A launch whose session no longer exists in the project is not listed.
    locked(&s.store).unwrap().delete(dev).unwrap();
    let error = start(
        &s,
        "gone",
        json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":dev}),
        cx,
    )
    .unwrap_err();
    assert_eq!(error, "Session was not found in this project");
    assert_eq!(session_count(&s), before - 1);
}

#[gpui::test]
fn start_rejects_a_malformed_parent_id(cx: &mut TestAppContext) {
    let s = setup(cx);
    let before = session_count(&s);
    for parent in [json!(0), json!(-1), json!(""), json!("abc"), json!(true)] {
        let error = start(
            &s,
            "rev",
            json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":parent}),
            cx,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "parentSessionId must be a session ID (a positive number)"
        );
    }
    assert_eq!(session_count(&s), before);
}

fn launch_of(session: i64, cwd: &str) -> mono::Launch {
    mono::Launch {
        session_id: session,
        cwd: cwd.into(),
        title: "x".into(),
        harness: "codex".into(),
        model: None,
        origin_turn: 0,
        notify_on_complete: false,
        each: false,
        completed: false,
        dismissed: false,
        visible: false,
    }
}

#[test]
fn only_a_link_to_another_launch_makes_a_child() {
    // The shared (Tauri) database links `app-<chat>-<id>` launches to the Mono chat (1).
    let launches = [launch_of(10, "/p"), launch_of(11, "/p")];
    let links = std::collections::HashMap::from([(10, 1), (11, 10), (12, 11)]);
    assert!(
        !host::is_child_launch(&links, &launches, 10),
        "linked to the chat"
    );
    assert!(
        host::is_child_launch(&links, &launches, 11),
        "linked to a launch"
    );
    assert!(!host::is_child_launch(&links, &launches, 99), "no link");
}

#[gpui::test]
fn a_launch_linked_to_the_mono_chat_is_a_valid_parent(cx: &mut TestAppContext) {
    let s = setup(cx);
    let direct = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    // How a Tauri Mono's direct launch looks once persisted.
    locked(&s.store)
        .unwrap()
        .save_started_link(direct, s.session)
        .unwrap();
    edit_mono(&s, cx, |m| {
        m.launches.push(launch_of(direct, &m.projects[0].clone()))
    });
    let review = start(
        &s,
        "rev",
        json!({"project":s.project,"prompt":"Review","draft":true,"parentSessionId":direct}),
        cx,
    )
    .unwrap()["sessionId"]
        .as_i64()
        .unwrap();
    assert_eq!(started_link(&s, review), Some(direct));
}

// Completion delivery (ports of monoCompletionBatches.test.ts onto the launch
// flags: native batches are derived from `Launch`, there is no watch/accept/discard).
fn pending(session: i64, origin: u64, each: bool) -> mono::Launch {
    mono::Launch {
        session_id: session,
        title: format!("s{session}"),
        origin_turn: origin,
        notify_on_complete: true,
        each,
        ..launch_of(session, "/p")
    }
}
const TURN: u64 = (7 << 32) | 1;
fn plan(
    launches: &[mono::Launch],
    source_busy: bool,
    running: &[i64],
) -> Vec<(host::Batch, Vec<i64>)> {
    host::ready_batches(launches, |_| source_busy, |id| !running.contains(&id))
        .into_iter()
        .map(|(batch, sessions)| (batch, sessions.into_iter().map(|(id, _)| id).collect()))
        .collect()
}

#[test]
fn a_group_waits_for_its_turn_and_every_child_then_reports_all_in_launch_order() {
    let launches = [
        pending(1, TURN, false),
        pending(2, TURN, false),
        pending(3, TURN, false),
    ];
    // A fast child stopped, but the launching turn is still active.
    assert!(plan(&launches, true, &[2, 3]).is_empty());
    // The turn ended while siblings still run.
    assert!(plan(&launches, false, &[2]).is_empty());
    assert_eq!(
        plan(&launches, false, &[]),
        [(host::Batch::Group(TURN), vec![1, 2, 3])]
    );
}

#[test]
fn a_single_session_group_reports_alone() {
    assert_eq!(
        plan(&[pending(9, TURN, false)], false, &[]),
        [(host::Batch::Group(TURN), vec![9])]
    );
}

#[test]
fn later_turns_and_other_callers_do_not_wait_for_a_slow_earlier_group() {
    let later = (7 << 32) | 2;
    let other = (8 << 32) | 1;
    let launches = [
        pending(1, TURN, false),
        pending(2, TURN, false),
        pending(3, later, false),
        pending(4, other, false),
    ];
    let plan = host::ready_batches(&launches, |_| false, |id| id != 1);
    let batches: Vec<_> = plan.iter().map(|(b, _)| *b).collect();
    assert_eq!(
        batches,
        [host::Batch::Group(later), host::Batch::Group(other)]
    );
}

#[test]
fn a_dismissed_session_is_dropped_without_losing_the_others() {
    let mut launches = [pending(1, TURN, false), pending(2, TURN, false)];
    launches[0].dismissed = true;
    assert_eq!(
        plan(&launches, false, &[]),
        [(host::Batch::Group(TURN), vec![2])]
    );
    launches[1].dismissed = true;
    assert!(plan(&launches, false, &[]).is_empty());
}

#[test]
fn a_session_without_notification_or_already_reported_is_not_waited_for() {
    let mut launches = [pending(1, TURN, false), pending(2, TURN, false)];
    launches[0].notify_on_complete = false;
    launches[1].completed = true;
    assert!(plan(&launches, false, &[]).is_empty());
}

#[test]
fn finishing_a_group_marks_only_that_group_and_is_idempotent() {
    let mut launches = [
        pending(1, TURN, false),
        pending(2, TURN, true),
        pending(3, (7 << 32) | 2, false),
    ];
    host::finish_batch(&mut launches, host::Batch::Group(TURN));
    host::finish_batch(&mut launches, host::Batch::Group(TURN));
    assert_eq!(
        launches.iter().map(|l| l.completed).collect::<Vec<_>>(),
        [true, false, false],
        "an each launch sharing the turn is not part of the group"
    );
    assert!(plan(&launches[1..], true, &[3]).len() == 1);
}

#[test]
fn an_each_session_reports_as_soon_as_it_stops_without_the_turn_or_siblings() {
    let launches = [pending(1, TURN, false), pending(2, TURN, true)];
    // The launching turn is still active and the group sibling is running.
    assert_eq!(
        plan(&launches, true, &[1]),
        [(host::Batch::Each(2), vec![2])]
    );
    // The group follows on its own once the turn ends and the sibling stops.
    assert_eq!(
        plan(&launches[..1], false, &[]),
        [(host::Batch::Group(TURN), vec![1])]
    );
}

#[test]
fn an_each_session_still_running_or_dismissed_reports_nothing() {
    let mut launches = [pending(2, TURN, true)];
    assert!(plan(&launches, false, &[2]).is_empty());
    launches[0].dismissed = true;
    assert!(plan(&launches, false, &[]).is_empty());
}

#[test]
fn each_sessions_report_separately_and_once() {
    let mut launches = [
        pending(1, TURN, false),
        pending(2, TURN, true),
        pending(3, TURN, true),
    ];
    assert_eq!(
        plan(&launches, true, &[1]),
        [
            (host::Batch::Each(2), vec![2]),
            (host::Batch::Each(3), vec![3])
        ]
    );
    host::finish_batch(&mut launches, host::Batch::Each(2));
    host::finish_batch(&mut launches, host::Batch::Each(2));
    assert_eq!(
        plan(&launches, true, &[1]),
        [(host::Batch::Each(3), vec![3])]
    );
    assert!(!launches[2].completed && !launches[0].completed);
}

#[test]
fn a_large_group_keeps_every_session_with_bounded_results() {
    let results: Vec<_> = (0..40)
        .map(|i| (i, format!("worker-{i}"), "x".repeat(20_000)))
        .collect();
    let message = host::completion_message(host::Batch::Group(TURN), &results);
    assert!(message.len() < 60_000, "{}", message.len());
    for i in 0..40 {
        assert!(message.contains(&format!("Session {i}: worker-{i}")));
    }
    assert!(message.contains("[truncated]"));
    // One session keeps the full per-session cap.
    let one = host::completion_message(
        host::Batch::Each(1),
        &[(1, "big".into(), "y".repeat(12_000))],
    );
    assert!(!one.contains("[truncated]"));
    assert!(one.contains("Session 1: big"));
}

#[test]
fn group_and_each_messages_say_what_to_do() {
    let results = [(1, "a".to_string(), "done".to_string())];
    let group = host::completion_message(host::Batch::Group(TURN), &results);
    let each = host::completion_message(host::Batch::Each(1), &results);
    assert!(group.starts_with("[Delegated work completed]"));
    assert!(group.contains("All sessions in this group have stopped"));
    assert!(each.starts_with("[Delegated work completed]"));
    assert!(each.contains("This session has stopped"));
    assert!(!each.contains("group"));
    assert!(each.ends_with("Session 1: a\ndone"));
}

#[test]
fn a_chat_caller_keeps_its_turn_and_a_habit_run_reports_each_to_the_chat() {
    let each = json!({"completionDelivery":"each"});
    let group = json!({"completionDelivery":"group"});
    // Mono chat 7, turn 3.
    assert_eq!(
        host::report_to(7, 3, None, &json!({})),
        ((7 << 32) | 3, false)
    );
    assert_eq!(host::report_to(7, 3, None, &group), ((7 << 32) | 3, false));
    assert_eq!(host::report_to(7, 3, None, &each), ((7 << 32) | 3, true));
    // Habit run 21 of the Mono whose chat is 7: always each, to the chat.
    for input in [json!({}), group.clone(), each.clone()] {
        assert_eq!(host::report_to(21, 5, Some(7), &input), (7 << 32, true));
    }
}

#[test]
fn what_a_run_started_is_named_in_its_post() {
    assert_eq!(
        scheduler::run_post(Some("Fixed it."), 3).as_deref(),
        Some("Fixed it.")
    );
    assert_eq!(scheduler::run_post(None, 0), None);
    assert_eq!(
        scheduler::run_post(None, 1).as_deref(),
        Some("Started a session.")
    );
    assert_eq!(
        scheduler::run_post(None, 4).as_deref(),
        Some("Started 4 sessions.")
    );
}

#[test]
fn the_habit_run_prompt_sends_session_reports_to_the_chat() {
    let prompt = scheduler::habit_prompt("Triage", "Look at CI");
    assert!(prompt.starts_with("[Scheduled habit: Triage]\nLook at CI"));
    assert!(prompt.contains("NOTHING_TO_REPORT"));
    assert!(prompt.contains("report to your chat, not to this run"));
    assert!(prompt.contains("do not wait for them or poll"));
    assert!(prompt.contains("Mention in your reply what you started"));
    assert!(!prompt.contains("belongs to this habit"));
}

pub(super) fn launches_of(s: &Setup, cx: &mut TestAppContext) -> Vec<mono::Launch> {
    cx.update(|cx| {
        s.manager
            .read(cx)
            .roster
            .get(&s.id)
            .unwrap()
            .launches
            .clone()
    })
}
/// A habit run of the Mono, driven through its chat view (nothing is sent).
fn fake_run(s: &Setup, cx: &mut TestAppContext) -> i64 {
    let run = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            let view = m.chats.get(&s.id).unwrap().clone();
            m.running.insert(
                run,
                RunningHabit {
                    mono: s.id.clone(),
                    habit: "h1".into(),
                    view,
                    started: monocode_core::time::now_ms(),
                    launched: monocode_core::time::now_ms(),
                    failure: None,
                    waiting_since: None,
                    cards: Vec::new(),
                    sessions: Vec::new(),
                    created: Vec::new(),
                    stop: None,
                },
            );
        })
    });
    run
}
fn run_sessions(s: &Setup, run: i64, cx: &mut TestAppContext) -> Vec<i64> {
    cx.update(|cx| {
        s.manager
            .read(cx)
            .running
            .get(&run)
            .map(|r| r.sessions.clone())
    })
    .unwrap_or_default()
}
/// A launch that reports back needs a submitted prompt (an unsent draft cannot notify). Tests that
/// start one hold a `NoClis` so no provider runs.
fn notifying(extra: Value) -> Value {
    let mut input = json!({"prompt":"Review","notifyOnComplete":true});
    for (key, value) in extra.as_object().unwrap() {
        input[key] = value.clone();
    }
    input
}

#[gpui::test]
fn a_chat_launch_reports_under_its_turn_as_a_group_unless_asked_for_each(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let s = setup(cx);
    for (request, extra) in [
        ("plain", json!({})),
        ("group", json!({"completionDelivery":"group"})),
        ("each", json!({"completionDelivery":"each"})),
    ] {
        let mut input = notifying(extra);
        input["project"] = json!(s.project);
        start(&s, request, input, cx).unwrap();
    }
    let launches = launches_of(&s, cx);
    assert_eq!(
        launches.iter().map(|l| l.each).collect::<Vec<_>>(),
        [false, false, true]
    );
    assert!(launches.iter().all(|l| l.notify_on_complete));
    assert!(
        launches
            .iter()
            .all(|l| (l.origin_turn >> 32) as i64 == s.session)
    );
    let mut bad = notifying(json!({"completionDelivery":"sometimes"}));
    bad["project"] = json!(s.project);
    assert_eq!(
        start(&s, "bad", bad, cx).unwrap_err(),
        "completionDelivery must be \"group\" or \"each\""
    );
    assert_eq!(launch_count(&s, cx), 3);
}

#[gpui::test]
fn a_habit_runs_sessions_report_each_to_the_chat_and_the_run_notes_them(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let s = setup(cx);
    let run = fake_run(&s, cx);
    for (request, extra) in [
        ("plain", json!({})),
        ("group", json!({"completionDelivery":"group"})),
    ] {
        let mut input = notifying(extra);
        input["project"] = json!(s.project);
        call_from(&s, run, request, "sessions.start", input, cx)
            .try_recv()
            .unwrap()
            .unwrap();
    }
    let launches = launches_of(&s, cx);
    assert_eq!(launches.len(), 2);
    for launch in &launches {
        assert!(launch.each, "forced each");
        assert_eq!(
            launch.origin_turn,
            (s.session as u64) << 32,
            "origin is the chat"
        );
    }
    assert_eq!(
        run_sessions(&s, run, cx),
        launches.iter().map(|l| l.session_id).collect::<Vec<_>>()
    );
}

#[gpui::test]
fn a_run_timeout_stops_the_run_but_not_the_sessions_it_started(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, cx);
    let child = locked(&s.store)
        .unwrap()
        .create("codex", &s.project)
        .unwrap()
        .id;
    // Both shapes: reporting to the chat, and the old "belongs to the run" origin.
    // Not notifying, so the tick sends no report (that would start a provider turn).
    edit_mono(&s, cx, |m| {
        m.launches.push(mono::Launch {
            origin_turn: (m.session_id.unwrap() as u64) << 32,
            notify_on_complete: false,
            ..pending(child, 0, true)
        });
        m.launches.push(mono::Launch {
            origin_turn: (run as u64) << 32,
            notify_on_complete: false,
            ..pending(child + 1000, 0, false)
        });
    });
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            let run = m.running.get_mut(&run).unwrap();
            run.sessions.push(child);
            run.started = 0;
        })
    });
    let handle: gpui::AnyWindowHandle = s.window.into();
    handle
        .update(cx, |_, w, cx| s.manager.update(cx, |m, cx| m.tick(w, cx)))
        .unwrap();
    cx.run_until_parked();
    assert!(
        cx.update(|cx| s.manager.read(cx).running.is_empty()),
        "the run ended"
    );
    for launch in launches_of(&s, cx) {
        assert!(!launch.dismissed, "{} was dismissed", launch.session_id);
    }
    let rows = locked(&s.store).unwrap().load(s.session).unwrap().rows;
    assert!(
        rows.iter().any(|r| r.text.ends_with("Started a session.")),
        "{:?}",
        rows.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
}

#[gpui::test]
fn a_run_ends_without_waiting_for_the_sessions_it_started(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, cx);
    // Unsent drafts start no provider; an unsent draft cannot ask to be monitored, so the
    // launches are marked as monitored afterwards.
    let mut input = json!({"prompt":"Review","draft":true});
    input["project"] = json!(s.project);
    call_from(&s, run, "one", "sessions.start", input.clone(), cx)
        .try_recv()
        .unwrap()
        .unwrap();
    input["prompt"] = json!("Review again");
    call_from(&s, run, "two", "sessions.start", input, cx)
        .try_recv()
        .unwrap()
        .unwrap();
    edit_mono(&s, cx, |m| {
        for launch in &mut m.launches {
            launch.notify_on_complete = true;
        }
    });
    // Both are still running (held busy, so no provider starts) yet the run is done.
    for launch in launches_of(&s, cx) {
        cx.update(|cx| {
            let view = s
                .manager
                .read(cx)
                .owner
                .upgrade()
                .unwrap()
                .read(cx)
                .mono_session_entity(launch.session_id)
                .unwrap();
            view.update(cx, |v, _| v.busy = true);
        });
    }
    cx.update(|cx| s.manager.update(cx, |m, cx| m.habit_changed(run, cx)));
    cx.run_until_parked();
    assert!(cx.update(|cx| s.manager.read(cx).running.is_empty()));
    assert!(
        launches_of(&s, cx)
            .iter()
            .all(|l| !l.completed && !l.dismissed)
    );
    let rows = locked(&s.store).unwrap().load(s.session).unwrap().rows;
    assert!(
        rows.iter().any(|r| r.text.ends_with("Started 2 sessions.")),
        "{:?}",
        rows.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
}

// Task 5.5: the habit Stop sheet and what a stopped run does.

#[test]
fn the_stop_sheet_offers_the_three_choices() {
    assert_eq!(
        habit_stop::CHOICES,
        ["Stop run only", "Stop run and its sessions", "Keep running"]
    );
    let text = habit_stop::stop_text("PR check");
    assert_eq!(
        text,
        "Stop \"PR check\"? Sessions it already started can keep running."
    );
    assert_eq!(
        habit_stop::finished_text("PR check"),
        "\"PR check\" already finished, so there was nothing to stop."
    );
}

#[test]
fn keep_running_is_the_cancel_button_so_escape_keeps_the_run() {
    let buttons = habit_stop::buttons();
    assert_eq!(
        buttons
            .iter()
            .map(|b| b.label().to_string())
            .collect::<Vec<_>>(),
        habit_stop::CHOICES
    );
    assert_eq!(
        buttons.iter().map(|b| b.is_cancel()).collect::<Vec<_>>(),
        [false, false, true]
    );
    let cancel = buttons.iter().position(|b| b.is_cancel());
    assert_eq!(habit_stop::mode_for(cancel), None);
}

#[test]
fn a_stop_asked_before_the_turn_works_ends_it_once_it_does() {
    use habit_stop::{StopMode, stop_pending};
    let stop = Some(StopMode::Run);
    let (none, failed) = (None, Some("x".to_string()));
    assert!(stop_pending(stop, &none, true));
    assert!(!stop_pending(stop, &none, false), "not working yet");
    assert!(!stop_pending(stop, &failed, true), "already ending");
    assert!(!stop_pending(None, &none, true), "nothing asked");
}

#[test]
fn each_button_maps_to_a_stop_mode_and_keeping_or_closing_to_none() {
    use habit_stop::{StopMode, mode_for};
    assert_eq!(mode_for(Some(0)), Some(StopMode::Run));
    assert_eq!(mode_for(Some(1)), Some(StopMode::All));
    assert_eq!(mode_for(Some(2)), None);
    // Closing the sheet some other way is also keeping it running.
    assert_eq!(mode_for(Some(9)), None);
    assert_eq!(mode_for(None), None);
}

fn seed_habit(s: &Setup) {
    let mut h = Habit::new(
        "PR check".into(),
        "Look at open PRs".into(),
        habits::Schedule::default(),
        monocode_core::time::now_ms(),
    )
    .unwrap();
    h.id = "h1".into();
    h.running_since = Some(monocode_core::time::now_ms());
    habits::update(&s.root, &s.id, |all| {
        all.push(h.clone());
        Ok(())
    })
    .unwrap();
}
fn stop_of(s: &Setup, run: i64, cx: &mut TestAppContext) -> Option<habit_stop::StopMode> {
    cx.update(|cx| s.manager.read(cx).running.get(&run).and_then(|r| r.stop))
}
fn choose(s: &Setup, answer: Option<usize>, cx: &mut TestAppContext) -> habit_stop::StopChoice {
    cx.update(|cx| {
        s.manager
            .update(cx, |m, cx| m.choose_stop("h1", answer, cx))
    })
}

#[gpui::test]
fn the_chosen_stop_is_requested_and_keeping_requests_nothing(cx: &mut TestAppContext) {
    use habit_stop::{StopChoice, StopMode};
    let s = setup(cx);
    let run = fake_run(&s, cx);
    assert_eq!(choose(&s, Some(2), cx), StopChoice::Kept);
    assert_eq!(choose(&s, None, cx), StopChoice::Kept);
    assert_eq!(stop_of(&s, run, cx), None);
    assert_eq!(choose(&s, Some(1), cx), StopChoice::Requested);
    assert_eq!(stop_of(&s, run, cx), Some(StopMode::All));
}

#[gpui::test]
fn it_does_not_ask_again_once_a_stop_was_requested(cx: &mut TestAppContext) {
    use habit_stop::StopMode;
    let s = setup(cx);
    let run = fake_run(&s, cx);
    let wants =
        |cx: &mut TestAppContext| cx.update(|cx| s.manager.read(cx).wants_stop_prompt("h1"));
    assert!(wants(cx));
    choose(&s, Some(0), cx);
    assert!(!wants(cx));
    // A later choice cannot change the request.
    choose(&s, Some(1), cx);
    assert_eq!(stop_of(&s, run, cx), Some(StopMode::Run));
}

#[gpui::test]
fn it_tells_the_user_instead_when_the_run_ended_while_they_were_choosing(cx: &mut TestAppContext) {
    use habit_stop::StopChoice;
    let s = setup(cx);
    // No run at all: nothing to stop and nothing recorded.
    assert_eq!(choose(&s, Some(1), cx), StopChoice::Finished);
    assert!(!cx.update(|cx| s.manager.read(cx).habit_stopping("h1")));
    // Keeping it running says nothing, even with no run.
    assert_eq!(choose(&s, Some(2), cx), StopChoice::Kept);
    // The run ended after the sheet opened.
    let run = fake_run(&s, cx);
    cx.update(|cx| s.manager.update(cx, |m, _| m.running.remove(&run)));
    assert_eq!(choose(&s, Some(0), cx), StopChoice::Finished);
}

#[gpui::test]
fn a_stop_asked_while_the_turn_is_idle_waits_for_it_to_work(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, cx);
    choose(&s, Some(0), cx);
    cx.update(|cx| s.manager.update(cx, |m, cx| m.enforce_stop(run, cx)));
    // Idle: the request stands, nothing is failed yet.
    cx.update(|cx| {
        let r = &s.manager.read(cx).running[&run];
        assert!(r.stop.is_some() && r.failure.is_none());
    });
}

#[gpui::test]
fn it_reports_whether_a_stop_is_pending(cx: &mut TestAppContext) {
    let s = setup(cx);
    let pending = |cx: &mut TestAppContext| cx.update(|cx| s.manager.read(cx).habit_stopping("h1"));
    assert!(!pending(cx));
    fake_run(&s, cx);
    assert!(!pending(cx));
    choose(&s, Some(0), cx);
    assert!(pending(cx));
}

/// A run the user stopped while it was working: `apply_stop` with `busy`.
fn user_stop(s: &Setup, run: i64, mode: habit_stop::StopMode, cx: &mut TestAppContext) {
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            let r = m.running.get_mut(&run).unwrap();
            habit_stop::apply_stop(&mut r.stop, &mut r.failure, mode, true)
        })
    });
}
/// A session started through the host as `caller`; the run notes it as created.
fn child(s: &Setup, caller: i64, request: &str, cx: &mut TestAppContext) -> i64 {
    let mut input = json!({"prompt":"Review","draft":true,"notifyOnComplete":false});
    input["project"] = json!(s.project);
    call_from(s, caller, request, "sessions.start", input, cx)
        .try_recv()
        .unwrap()
        .unwrap()["sessionId"]
        .as_i64()
        .unwrap()
}
/// A session the run only wrote to (the chat started it).
fn messaged(s: &Setup, run: i64, request: &str, cx: &mut TestAppContext) -> i64 {
    let id = child(s, s.session, request, cx);
    cx.update(|cx| {
        s.manager
            .update(cx, |m, _| m.note_run_session(run, id, false))
    });
    id
}
fn finish_run(s: &Setup, run: i64, cx: &mut TestAppContext) {
    cx.update(|cx| s.manager.update(cx, |m, cx| m.habit_changed(run, cx)));
    cx.run_until_parked();
    assert!(cx.update(|cx| s.manager.read(cx).running.is_empty()));
}
pub(super) fn chat_rows(s: &Setup) -> Vec<String> {
    locked(&s.store)
        .unwrap()
        .load(s.session)
        .unwrap()
        .rows
        .into_iter()
        .map(|r| r.text)
        .collect()
}
fn stored_habit(s: &Setup) -> Habit {
    habits::load(&s.root, &s.id).unwrap().remove(0)
}
fn dismissed(s: &Setup, id: i64, cx: &mut TestAppContext) -> bool {
    launches_of(s, cx)
        .iter()
        .find(|l| l.session_id == id)
        .unwrap()
        .dismissed
}

#[gpui::test]
fn stop_run_only_ends_the_run_keeps_its_sessions_and_says_where_they_are(cx: &mut TestAppContext) {
    let s = setup(cx);
    seed_habit(&s);
    let run = fake_run(&s, cx);
    let (a, b) = (child(&s, run, "a", cx), child(&s, run, "b", cx));
    user_stop(&s, run, habit_stop::StopMode::Run, cx);
    finish_run(&s, run, cx);
    assert!(!dismissed(&s, a, cx) && !dismissed(&s, b, cx));
    let rows = chat_rows(&s);
    assert!(
        rows.iter()
            .any(|r| r.ends_with("Stopped. Started 2 sessions that keep running.")),
        "{rows:?}"
    );
    let habit = stored_habit(&s);
    assert_eq!(habit.last_outcome.as_deref(), Some("failed"));
    assert_eq!(habit.last_error.as_deref(), Some("Stopped by the user."));
}

#[gpui::test]
fn stopping_a_run_that_started_no_sessions_posts_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    seed_habit(&s);
    let run = fake_run(&s, cx);
    user_stop(&s, run, habit_stop::StopMode::Run, cx);
    finish_run(&s, run, cx);
    assert!(!chat_rows(&s).iter().any(|r| r.contains("Habit report")));
    let habit = stored_habit(&s);
    assert_eq!(habit.last_outcome.as_deref(), Some("failed"));
    assert_eq!(habit.last_error.as_deref(), Some("Stopped by the user."));
}

#[gpui::test]
fn a_single_kept_session_is_named_in_the_singular_and_a_messaged_one_is_not_counted(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    seed_habit(&s);
    let run = fake_run(&s, cx);
    child(&s, run, "a", cx);
    messaged(&s, run, "r", cx);
    user_stop(&s, run, habit_stop::StopMode::Run, cx);
    finish_run(&s, run, cx);
    assert!(
        chat_rows(&s)
            .iter()
            .any(|r| r.ends_with("Stopped. Started 1 session that keeps running.")),
        "{:?}",
        chat_rows(&s)
    );
}

#[gpui::test]
fn stop_run_and_its_sessions_stops_only_the_ones_it_created(cx: &mut TestAppContext) {
    let s = setup(cx);
    seed_habit(&s);
    let run = fake_run(&s, cx);
    let a = child(&s, run, "a", cx);
    let reviewer = messaged(&s, run, "r", cx);
    user_stop(&s, run, habit_stop::StopMode::All, cx);
    finish_run(&s, run, cx);
    assert!(dismissed(&s, a, cx), "its report is dropped");
    // The dismissal is saved, so a restart cannot deliver the stopped session's report.
    let saved = mono::Roster::load(&locked(&s.store).unwrap()).unwrap();
    let launch = |id| {
        saved
            .get(&s.id)
            .unwrap()
            .launches
            .iter()
            .find(|l| l.session_id == id)
            .unwrap()
            .dismissed
    };
    assert!(launch(a) && !launch(reviewer));
    assert!(
        !dismissed(&s, reviewer, cx),
        "a messaged session is not its to stop"
    );
    assert!(!chat_rows(&s).iter().any(|r| r.contains("Habit report")));
    assert_eq!(
        stored_habit(&s).last_error.as_deref(),
        Some("Stopped by the user.")
    );
}

#[gpui::test]
fn it_says_which_sessions_would_not_stop(cx: &mut TestAppContext) {
    let s = setup(cx);
    seed_habit(&s);
    let run = fake_run(&s, cx);
    let a = child(&s, run, "a", cx);
    // A Mono chat is never stopped from a habit.
    let chat = s.session;
    cx.update(|cx| {
        s.manager
            .update(cx, |m, _| m.note_run_session(run, chat, true))
    });
    edit_mono(&s, cx, |m| {
        m.launches.push(mono::Launch {
            notify_on_complete: false,
            ..pending(chat, 0, true)
        })
    });
    let c = child(&s, run, "c", cx);
    user_stop(&s, run, habit_stop::StopMode::All, cx);
    finish_run(&s, run, cx);
    assert!(dismissed(&s, a, cx) && dismissed(&s, c, cx));
    assert!(!dismissed(&s, chat, cx));
    let expected = format!("Could not stop 1 session: {chat}.");
    assert!(
        chat_rows(&s).iter().any(|r| r.ends_with(&expected)),
        "{:?}",
        chat_rows(&s)
    );
}

#[gpui::test]
fn a_run_that_finished_as_the_user_chose_still_stops_its_sessions(cx: &mut TestAppContext) {
    let s = setup(cx);
    seed_habit(&s);
    let run = fake_run(&s, cx);
    let a = child(&s, run, "a", cx);
    // The turn was already over when the choice came: not busy, so no failure is recorded.
    cx.update(|cx| {
        s.manager.update(cx, |m, _| {
            let r = m.running.get_mut(&run).unwrap();
            habit_stop::apply_stop(
                &mut r.stop,
                &mut r.failure,
                habit_stop::StopMode::All,
                false,
            )
        })
    });
    finish_run(&s, run, cx);
    assert!(dismissed(&s, a, cx));
    assert_ne!(
        stored_habit(&s).last_error.as_deref(),
        Some("Stopped by the user.")
    );
}

#[test]
fn a_stop_while_working_fails_the_run_and_one_after_it_ended_does_not() {
    use habit_stop::{StopMode, apply_stop};
    let run = |busy| {
        let (mut stop, mut failure) = (None, None);
        apply_stop(&mut stop, &mut failure, StopMode::Run, busy);
        (stop, failure)
    };
    let (stop, failure) = run(true);
    assert_eq!(stop, Some(StopMode::Run));
    assert_eq!(failure.as_deref(), Some("Stopped by the user."));
    let (stop, failure) = run(false);
    assert_eq!(stop, Some(StopMode::Run));
    assert_eq!(failure, None);
    // An earlier failure (a timeout) is kept.
    let (mut stop, mut failure) = (None, Some("Habit timed out".to_string()));
    apply_stop(&mut stop, &mut failure, StopMode::All, true);
    assert_eq!(failure.as_deref(), Some("Habit timed out"));
    // A request already made stands.
    apply_stop(&mut stop, &mut failure, StopMode::Run, true);
    assert_eq!(stop, Some(StopMode::All));
}

#[test]
fn the_post_says_what_a_stopped_or_finished_run_left_behind() {
    use habit_stop::{StopMode, post_text};
    let post = |report, mode, stopped, created, remaining, stuck: &[i64]| {
        post_text(report, mode, stopped, created, remaining, stuck)
    };
    assert_eq!(
        post(None, Some(StopMode::Run), true, 2, 2, &[]).as_deref(),
        Some("Stopped. Started 2 sessions that keep running.")
    );
    assert_eq!(
        post(None, Some(StopMode::Run), true, 1, 1, &[]).as_deref(),
        Some("Stopped. Started 1 session that keeps running.")
    );
    assert_eq!(post(None, Some(StopMode::Run), true, 0, 0, &[]), None);
    assert_eq!(post(None, Some(StopMode::All), true, 2, 0, &[]), None);
    assert_eq!(
        post(None, Some(StopMode::All), true, 3, 1, &[7]).as_deref(),
        Some("Could not stop 1 session: 7.")
    );
    assert_eq!(
        post(None, Some(StopMode::All), true, 3, 2, &[7, 9]).as_deref(),
        Some("Could not stop 2 sessions: 7, 9.")
    );
    // Not stopped by the user: the report, or what is left running.
    assert_eq!(
        post(Some("CI is red."), None, false, 1, 1, &[]).as_deref(),
        Some("CI is red.")
    );
    assert_eq!(
        post(None, Some(StopMode::All), false, 1, 1, &[]).as_deref(),
        Some("Started a session.")
    );
    assert_eq!(post(None, None, false, 0, 0, &[]), None);
    assert_eq!(
        post(Some("CI is red."), Some(StopMode::All), false, 2, 1, &[4]).as_deref(),
        Some("CI is red. Could not stop 1 session: 4.")
    );
}

#[gpui::test]
fn a_new_run_starts_without_a_stop_request(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, cx);
    assert_eq!(stop_of(&s, run, cx), None);
    assert!(cx.update(|cx| s.manager.read(cx).running[&run].created.is_empty()));
}

#[gpui::test]
fn every_follow_up_is_counted_but_only_created_sessions_are_started(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, cx);
    let note = |session: i64, created: bool, cx: &mut TestAppContext| {
        cx.update(|cx| {
            s.manager
                .update(cx, |m, _| m.note_run_session(run, session, created))
        })
    };
    note(10, false, cx);
    note(11, true, cx);
    // A follow-up to a session it created keeps it created; repeats are not double counted.
    note(11, false, cx);
    note(10, false, cx);
    let (sessions, created) = cx.update(|cx| {
        let r = &s.manager.read(cx).running[&run];
        (r.sessions.clone(), r.created.clone())
    });
    assert_eq!(sessions, [10, 11]);
    assert_eq!(created, [11]);
    // Sessions of a caller that is not a habit run are ignored.
    cx.update(|cx| {
        s.manager
            .update(cx, |m, _| m.note_run_session(run + 1, 12, true))
    });
    assert!(cx.update(|cx| s.manager.read(cx).running[&run].sessions.len() == 2));
}

#[gpui::test]
fn a_session_the_run_started_is_created_and_reported_with_it(cx: &mut TestAppContext) {
    let _clis = crate::shell::surface_tests::NoClis::new();
    let s = setup(cx);
    let run = fake_run(&s, cx);
    let mut input = notifying(json!({}));
    input["project"] = json!(s.project);
    call_from(&s, run, "one", "sessions.start", input, cx)
        .try_recv()
        .unwrap()
        .unwrap();
    let (sessions, created) = cx.update(|cx| {
        let r = &s.manager.read(cx).running[&run];
        (r.sessions.clone(), r.created.clone())
    });
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions, created);
}

#[gpui::test]
fn the_stop_button_shows_only_while_the_habit_runs(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    seed_habit(&s);
    let h = stored_habit(&s);
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.habits = vec![h.clone()];
                m.route = Some(Route::Habit(h.id.clone()));
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-habit-run").is_some());
    assert!(visual.debug_bounds("mono-habit-stop").is_none());
    let run = fake_run(&s, visual);
    visual.update(|_, cx| s.manager.update(cx, |_, cx| cx.notify()));
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-habit-stop").is_some());
    // Once asked, the button stays but is disabled ("Stopping...").
    user_stop(&s, run, habit_stop::StopMode::Run, visual);
    visual.update(|_, cx| s.manager.update(cx, |_, cx| cx.notify()));
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-habit-stop").is_some());
    assert!(visual.update(|_, cx| s.manager.read(cx).habit_stopping("h1")));
}

mod reset_flow {
    use super::*;
    use monocode_core::{
        mono::provider_files::{self, retire_session},
        provider::Kind,
    };

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    const C: &str = "33333333-3333-4333-8333-333333333333";

    /// The old conversation's title. Ids are reused once the newest row is deleted,
    /// so the old row is told apart by what it holds, not by its id.
    const OLD: &str = "old conversation";

    /// Every call a cleanup runner got: the requests, and whether the old row was already gone.
    type Calls = Arc<Mutex<Vec<(Vec<(Kind, Vec<String>)>, bool)>>>;

    /// The Mono's chat on `provider`, with `current` as its provider session and `retired` rotated out.
    fn conversation(s: &Setup, provider: &str, current: Option<&str>, retired: &[(&str, &str)]) {
        let db = locked(&s.store).unwrap();
        db.title(s.session, OLD).unwrap();
        db.change_provider(s.session, provider).unwrap();
        if let Some(id) = current {
            db.provider_id(s.session, id).unwrap();
        }
        for (harness, id) in retired {
            retire_session(&db, s.session, harness, id).unwrap();
        }
    }
    /// Replaces the cleanup with a recorder that reports `problems`.
    fn recorder(s: &Setup, problems: Vec<String>, cx: &mut TestAppContext) -> Calls {
        let calls: Calls = Default::default();
        let log = calls.clone();
        let store = s.store.clone();
        cx.update(|cx| {
            s.manager.update(cx, |m, _| {
                m.cleanup = Arc::new(move |_, requests| {
                    let gone = !old_exists_in(&store);
                    log.lock().unwrap().push((
                        requests
                            .iter()
                            .map(|r| (r.provider(), r.ids().to_vec()))
                            .collect(),
                        gone,
                    ));
                    problems.clone()
                });
            })
        });
        calls
    }
    fn ask(s: &Setup, cx: &mut TestAppContext) {
        let handle: gpui::AnyWindowHandle = s.window.into();
        handle
            .update(cx, |_, w, cx| {
                s.manager.update(cx, |m, cx| m.request_reset(&s.id, w, cx))
            })
            .unwrap();
        cx.run_until_parked();
    }
    fn confirm(s: &Setup, cx: &mut TestAppContext) {
        let handle: gpui::AnyWindowHandle = s.window.into();
        handle
            .update(cx, |_, w, cx| {
                s.manager.update(cx, |m, cx| m.confirm_reset(w, cx))
            })
            .unwrap();
        cx.run_until_parked();
    }
    fn session_of(s: &Setup, cx: &mut TestAppContext) -> Option<i64> {
        cx.update(|cx| s.manager.read(cx).roster.get(&s.id).unwrap().session_id)
    }
    fn old_exists_in(store: &Database) -> bool {
        locked(store)
            .unwrap()
            .sessions()
            .unwrap()
            .iter()
            .any(|session| session.title == OLD)
    }
    fn old_exists(s: &Setup) -> bool {
        old_exists_in(&s.store)
    }
    fn rows(s: &Setup) -> Vec<i64> {
        locked(&s.store)
            .unwrap()
            .sessions()
            .unwrap()
            .iter()
            .map(|session| session.id)
            .collect()
    }
    fn dialog<T>(
        s: &Setup,
        cx: &mut TestAppContext,
        read: impl FnOnce(&reset::ResetDialog) -> T,
    ) -> Option<T> {
        cx.update(|cx| s.manager.read(cx).reset.as_ref().map(read))
    }

    #[test]
    fn the_dialog_copy_follows_the_desktop_app() {
        assert_eq!(reset::title("Broski"), "Reset Broski's conversation?");
        assert_eq!(
            reset::BODY,
            "All messages in this Mono's conversation will be deleted and any active reply will be stopped. This can't be undone."
        );
        assert_eq!(reset::KEPT, "Its soul, memory and habits will be kept.");
        let files = provider_files::mono_provider_files("claude", Some(A), &[]);
        assert_eq!(
            reset::option_label(&files),
            "Also delete the Claude Code session files on this computer"
        );
        assert_eq!(reset::option_note(&files), "They can't be recovered.");
        let cursor = provider_files::mono_provider_files("cursor", Some(A), &[]);
        assert_eq!(
            reset::option_note(&cursor),
            "Monoptah can't delete Cursor session files yet."
        );
        let mixed = provider_files::mono_provider_files(
            "cursor",
            Some(A),
            &[provider_files::provider_session_ref("claude", B).unwrap()],
        );
        assert_eq!(
            reset::option_note(&mixed),
            "They can't be recovered. Files from the current Cursor session are not covered."
        );
        assert_eq!(
            reset::failure("Storage unavailable"),
            "Could not reset the conversation. Storage unavailable"
        );
    }

    #[gpui::test]
    fn asking_shows_the_box_checked_for_claude_code_and_names_every_provider(
        cx: &mut TestAppContext,
    ) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[("codex", B)]);
        ask(&s, cx);
        let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
        visual.run_until_parked();
        let (label, checked, supported) = dialog(&s, visual, |d| {
            let files = d.files.as_ref().unwrap();
            (files.label.clone(), d.delete_files, files.supported)
        })
        .unwrap();
        assert_eq!(label, "Claude Code and Codex");
        assert!(checked && supported);
        assert!(visual.debug_bounds("mono-reset-dialog").is_some());
        assert!(visual.debug_bounds("mono-reset-files").is_some());
        // Nothing happens until the user confirms.
        assert_eq!(session_of(&s, visual), Some(s.session));
        assert!(old_exists(&s));
    }

    #[gpui::test]
    fn clicking_the_box_toggles_it_and_cancel_changes_nothing(cx: &mut TestAppContext) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[]);
        let calls = recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
        visual.run_until_parked();
        let checkbox = visual.debug_bounds("mono-reset-files").unwrap();
        visual.simulate_click(checkbox.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(dialog(&s, visual, |d| d.delete_files), Some(false));
        let cancel = visual.debug_bounds("mono-reset-cancel").unwrap();
        visual.simulate_click(cancel.center(), Default::default());
        visual.run_until_parked();
        assert!(dialog(&s, visual, |_| ()).is_none());
        assert_eq!(session_of(&s, visual), Some(s.session));
        assert!(old_exists(&s));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn resetting_with_the_box_ticked_deletes_the_row_then_cleans_current_and_retired_files(
        cx: &mut TestAppContext,
    ) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[("claude", B), ("codex", C)]);
        let calls = recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        confirm(&s, cx);
        cx.run_until_parked();

        assert!(dialog(&s, cx, |_| ()).is_none());
        assert!(!old_exists(&s), "the old row is deleted");
        let fresh = session_of(&s, cx).expect("a fresh conversation");
        assert!(rows(&s).contains(&fresh));
        let hidden = cx.update(|cx| s.manager.read(cx).roster.hidden_sessions.clone());
        assert_eq!(
            hidden.iter().filter(|&&id| id == fresh).count(),
            1,
            "the new conversation is hidden once, even when it reuses the old id"
        );
        let mut calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (mut requests, gone) = calls.remove(0);
        requests.sort_by_key(|(kind, _)| kind.id());
        assert_eq!(
            requests,
            vec![
                (Kind::Claude, vec![A.to_string(), B.to_string()]),
                (Kind::Codex, vec![C.to_string()]),
            ]
        );
        assert!(gone, "the cleanup runs after the row is gone");
        assert!(
            locked(&s.store)
                .unwrap()
                .setting(&format!("mono-retired:{}", s.session))
                .unwrap()
                .is_none()
        );
        assert_eq!(cx.update(|cx| s.manager.read(cx).error.clone()), None);
    }

    #[gpui::test]
    fn resetting_with_the_box_unticked_still_deletes_the_row_but_keeps_the_files(
        cx: &mut TestAppContext,
    ) {
        let s = setup(cx);
        conversation(&s, "codex", Some(A), &[("codex", B)]);
        let calls = recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        cx.update(|cx| {
            s.manager
                .update(cx, |m, _| m.reset.as_mut().unwrap().delete_files = false)
        });
        confirm(&s, cx);
        cx.run_until_parked();

        assert!(!old_exists(&s));
        assert!(session_of(&s, cx).is_some());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn cleanup_problems_become_a_notice_after_the_reset(cx: &mut TestAppContext) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[]);
        recorder(&s, vec!["one".into(), "two".into()], cx);
        ask(&s, cx);
        confirm(&s, cx);
        cx.run_until_parked();

        assert!(!old_exists(&s), "problems never undo the reset");
        assert_eq!(
            cx.update(|cx| s.manager.read(cx).error.clone()).as_deref(),
            Some(provider_files::problem_message("Claude Code", 2).as_str())
        );
    }

    #[gpui::test]
    fn a_harness_without_deletable_files_offers_a_disabled_box(cx: &mut TestAppContext) {
        let s = setup(cx);
        conversation(&s, "cursor", Some(A), &[]);
        let calls = recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
        visual.run_until_parked();
        assert_eq!(dialog(&s, visual, |d| d.delete_files), Some(false));
        let checkbox = visual.debug_bounds("mono-reset-files").unwrap();
        visual.simulate_click(checkbox.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(dialog(&s, visual, |d| d.delete_files), Some(false));
        confirm(&s, visual);
        visual.run_until_parked();
        assert!(!old_exists(&s));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn a_failed_deletion_keeps_the_conversation_and_says_why(cx: &mut TestAppContext) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[]);
        let calls = recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        // A poisoned store lock makes the deletion fail.
        let store = s.store.clone();
        let _ = std::thread::spawn(move || {
            let _guard = store.lock().unwrap();
            panic!("poison the store");
        })
        .join();
        confirm(&s, cx);
        cx.run_until_parked();
        s.store.clear_poison();

        assert_eq!(session_of(&s, cx), Some(s.session));
        assert!(old_exists(&s));
        let (busy, error) = dialog(&s, cx, |d| (d.busy, d.error.clone())).unwrap();
        assert!(!busy, "the dialog can be used again");
        assert!(error.is_some_and(|e| e.starts_with("Could not reset the conversation. ")));
        assert!(
            calls.lock().unwrap().is_empty(),
            "files stay when the row stays"
        );
    }

    #[gpui::test]
    fn a_mono_without_a_saved_conversation_resets_to_a_fresh_one(cx: &mut TestAppContext) {
        let s = setup(cx);
        let calls = recorder(&s, Vec::new(), cx);
        cx.update(|cx| {
            s.manager.update(cx, |m, _| {
                let mono = m.roster.get_mut(&s.id).unwrap();
                mono.session_id = None;
                mono.unavailable_session = Some("missing".into());
            })
        });
        ask(&s, cx);
        assert!(dialog(&s, cx, |d| d.files.is_none()).unwrap());
        confirm(&s, cx);
        cx.run_until_parked();
        let unavailable = cx.update(|cx| {
            s.manager
                .read(cx)
                .roster
                .get(&s.id)
                .unwrap()
                .unavailable_session
                .clone()
        });
        assert_eq!(unavailable, None);
        assert!(session_of(&s, cx).is_some());
        assert!(calls.lock().unwrap().is_empty());
    }
    #[gpui::test]
    fn a_reply_that_will_not_stop_keeps_the_conversation_and_says_so(cx: &mut TestAppContext) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[]);
        let calls = recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        // A turn that never reports back stays stopping.
        cx.update(|cx| {
            let chat = s.manager.read(cx).chats[&s.id].clone();
            chat.update(cx, |chat, cx| {
                chat.busy = true;
                chat.mono_stop(cx);
            });
        });
        confirm(&s, cx);
        assert!(
            old_exists(&s),
            "nothing is deleted while the reply may still write"
        );
        cx.executor().advance_clock(Duration::from_secs(11));
        cx.run_until_parked();

        assert!(old_exists(&s));
        assert_eq!(session_of(&s, cx), Some(s.session));
        let (busy, error) = dialog(&s, cx, |d| (d.busy, d.error.clone())).unwrap();
        assert!(!busy);
        assert_eq!(
            error.as_deref(),
            Some("Could not reset the conversation. The reply did not stop in time. Try again.")
        );
        assert!(calls.lock().unwrap().is_empty());
    }
    #[gpui::test]
    fn queued_messages_and_reports_wait_for_the_fresh_conversation(cx: &mut TestAppContext) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[]);
        recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        let queued = mono::Message {
            id: "queued".into(),
            text: "do it later".into(),
            sent_at: None,
            internal: false,
            // Never sent, so no provider process is started by this test.
            error: Some("not sent".into()),
            attachments: Vec::new(),
        };
        let handle: gpui::AnyWindowHandle = s.window.into();
        handle
            .update(cx, |_, w, cx| {
                s.manager.update(cx, |m, cx| {
                    let mono = m.roster.get_mut(&s.id).unwrap();
                    mono.outbox.push(queued);
                    mono.pending_reports.push("Habit finished.".into());
                    m.confirm_reset(w, cx);
                    // The reset is deleting the old row: nothing may reach it now.
                    m.dispatch(&s.id, cx);
                    let mono = m.roster.get(&s.id).unwrap();
                    assert_eq!(mono.pending_reports, vec!["Habit finished.".to_string()]);
                    assert_eq!(mono.outbox.len(), 1);
                });
            })
            .unwrap();
        cx.run_until_parked();

        assert!(!old_exists(&s));
        let outbox = cx.update(|cx| s.manager.read(cx).roster.get(&s.id).unwrap().outbox.len());
        assert_eq!(
            outbox, 1,
            "the queued message is kept for the new conversation"
        );
        let posted = cx.update(|cx| {
            let m = s.manager.read(cx);
            let pending = m.roster.get(&s.id).unwrap().pending_reports.clone();
            let rows = m.chats[&s.id]
                .read(cx)
                .transcript
                .read(cx)
                .source_rows()
                .to_vec();
            (pending, rows.iter().any(|r| r.text == "Habit finished."))
        });
        assert!(
            posted.1 || posted.0 == vec!["Habit finished.".to_string()],
            "the report is delivered to, or still queued for, the new conversation"
        );
    }
    #[gpui::test]
    fn the_notice_names_the_dialogs_provider_when_the_record_vanished(cx: &mut TestAppContext) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[]);
        let calls = recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        let mut db = locked(&s.store).unwrap();
        db.delete(s.session).unwrap();
        // An id retired by an earlier rotation is all that is left of it.
        retire_session(&db, s.session, "claude", B).unwrap();
        drop(db);
        confirm(&s, cx);
        cx.run_until_parked();

        assert!(
            calls.lock().unwrap().is_empty(),
            "nothing is deleted without the record"
        );
        assert_eq!(
            cx.update(|cx| s.manager.read(cx).error.clone()).as_deref(),
            Some(provider_files::problem_message("Claude Code", 1).as_str())
        );
    }
    #[gpui::test]
    fn a_report_held_back_by_a_failed_reset_is_delivered_to_the_kept_conversation(
        cx: &mut TestAppContext,
    ) {
        let s = setup(cx);
        conversation(&s, "claude", Some(A), &[]);
        recorder(&s, Vec::new(), cx);
        ask(&s, cx);
        // The row cannot be deleted, while the store keeps working for the report.
        let path = locked(&s.store).unwrap().database_path().to_path_buf();
        let blocked = std::process::Command::new("sqlite3")
            .arg(&path)
            .arg("CREATE TRIGGER keep_rows BEFORE DELETE ON native_sessions BEGIN SELECT RAISE(ABORT, 'kept'); END;")
            .status();
        if !blocked.is_ok_and(|status| status.success()) {
            eprintln!("skipped: the sqlite3 command is not available");
            return;
        }
        let handle: gpui::AnyWindowHandle = s.window.into();
        handle
            .update(cx, |_, w, cx| {
                s.manager.update(cx, |m, cx| {
                    m.roster
                        .get_mut(&s.id)
                        .unwrap()
                        .pending_reports
                        .push("Habit finished.".into());
                    m.confirm_reset(w, cx);
                    m.dispatch(&s.id, cx);
                    assert_eq!(m.roster.get(&s.id).unwrap().pending_reports.len(), 1);
                });
            })
            .unwrap();
        cx.run_until_parked();

        assert_eq!(session_of(&s, cx), Some(s.session));
        let (busy, pending, posted) = cx.update(|cx| {
            let m = s.manager.read(cx);
            let rows = m.chats[&s.id]
                .read(cx)
                .transcript
                .read(cx)
                .source_rows()
                .to_vec();
            (
                m.reset.as_ref().unwrap().busy,
                m.roster.get(&s.id).unwrap().pending_reports.clone(),
                rows.iter().any(|r| r.text == "Habit finished."),
            )
        });
        assert!(!busy);
        assert!(pending.is_empty() && posted, "{pending:?} posted={posted}");
    }
}

fn workflow_input(name: &str) -> Value {
    json!({"workflow": {
        "name": name,
        "nodes": [{"id": "trigger", "type": "manual"}, {"id": "report", "type": "report"}],
        "edges": [{"from": "trigger", "to": "report"}]
    }})
}

fn saved_workflows(s: &Setup) -> Vec<monocode_core::mono::workflows::model::Workflow> {
    monocode_core::mono::workflows::store::load_workflows(
        &monocode_core::mono::workflows::store::MonoDir::new(&s.root, &s.id),
    )
    .unwrap()
    .workflows
}

#[gpui::test]
fn workflow_actions_work_from_the_mono_chat_and_signal_the_runner(cx: &mut TestAppContext) {
    let s = setup(cx);
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    let ask = |request: &str, action: &str, input: Value, cx: &mut TestAppContext| {
        call(&s, request, action, input, cx).try_recv().unwrap()
    };
    let listed = ask("list-0", "workflows.list", json!({}), cx).unwrap();
    assert_eq!(listed, json!({"workflows": []}));
    let added = ask("add-1", "workflows.add", workflow_input("Digest"), cx).unwrap();
    let id = added["id"].as_str().unwrap().to_string();
    assert_eq!(added["enabled"], false);
    assert_eq!(added["issues"], json!([]));
    // A retry with the same request id returns the same result and adds nothing.
    let retry = ask("add-1", "workflows.add", workflow_input("Digest"), cx).unwrap();
    assert_eq!(retry, added);
    assert_eq!(saved_workflows(&s).len(), 1);
    assert!(!saved_workflows(&s)[0].enabled);
    // The guide and the field checks come through the same path.
    let guide = ask("guide-1", "workflows.guide", json!({}), cx).unwrap();
    assert!(guide["guide"].as_str().unwrap().contains("workflow"));
    let refused = ask("bogus-1", "workflows.add", json!({"bogus": 1}), cx).unwrap_err();
    assert_eq!(refused, "workflows.add does not take bogus");
    let refused = ask("shape-1", "workflows.add", json!({"workflow": "x"}), cx).unwrap_err();
    assert_eq!(refused, "workflow must be an object");
    // run needs a resumed workflow, then only writes its request and wakes the runner.
    let refused = ask("run-1", "workflows.run", json!({"id": id}), cx).unwrap_err();
    assert_eq!(refused, "Resume the workflow before running it.");
    assert!(!signals.take_check_now());
    ask("resume-1", "workflows.resume", json!({"id": id}), cx).unwrap();
    ask("run-2", "workflows.run", json!({"id": id}), cx).unwrap();
    // The runner is woken, so the request is acted on at once and not on the next look: the
    // run it asked for was claimed and is in the run history.
    assert!(!saved_workflows(&s)[0].run_requested);
    let runs = monocode_core::mono::workflows::store::load_runs(
        &monocode_core::mono::workflows::store::MonoDir::new(&s.root, &s.id),
        &id,
    )
    .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].trigger,
        monocode_core::mono::workflows::model::RunTrigger::Manual
    );
    assert!(!signals.take_check_now(), "the look answered the request");
    // stop and the running flag come from what the runner reports.
    let refused = ask("stop-1", "workflows.stop", json!({"id": id}), cx).unwrap_err();
    assert_eq!(refused, "That workflow is not running");
    signals.set_running(&id, true);
    let listed = ask("list-1", "workflows.list", json!({}), cx).unwrap();
    assert_eq!(listed["workflows"][0]["running"], true);
    ask("stop-2", "workflows.stop", json!({"id": id}), cx).unwrap();
    // Taken at once as well (nothing real runs, so there is nothing to stop).
    assert!(!saved_workflows(&s)[0].stop_requested);
    assert!(!signals.take_check_now());
    let refused = ask("remove-1", "workflows.remove", json!({"id": id}), cx).unwrap_err();
    assert_eq!(refused, "Stop the workflow before removing it");
    signals.set_running(&id, false);
    ask("remove-2", "workflows.remove", json!({"id": id}), cx).unwrap();
    assert!(saved_workflows(&s).is_empty());
}

#[gpui::test]
fn a_habit_run_cannot_manage_workflows(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = fake_run(&s, cx);
    for (action, input) in [
        ("workflows.list", json!({})),
        ("workflows.guide", json!({})),
        ("workflows.add", workflow_input("From a habit")),
    ] {
        let result = call_from(&s, run, action, action, input, cx)
            .try_recv()
            .unwrap();
        assert_eq!(
            result.unwrap_err(),
            "Only a Mono's own conversation can manage its workflows",
            "{action}"
        );
    }
    assert!(saved_workflows(&s).is_empty());
    // A session that holds no capability at all gets no answer from this Mono.
    let outsider = call_from(&s, run + 1000, "x", "workflows.list", json!({}), cx)
        .try_recv()
        .unwrap();
    assert_eq!(
        outsider.unwrap_err(),
        "This Mono no longer owns an active app capability"
    );
}

#[gpui::test]
fn habits_and_documents_search_like_sessions(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let habit = |id: &str, name: &str| {
        let mut h = Habit::new(
            name.into(),
            "Do the work.".into(),
            habits::Schedule::default(),
            monocode_core::time::now_ms(),
        )
        .unwrap();
        h.id = id.into();
        h
    };
    let document = |id: &str, title: &str| mono::Document {
        id: id.into(),
        kind: "markdown".into(),
        title: title.into(),
        summary: String::new(),
        body: String::new(),
        created_at: 1,
        updated_at: 1,
    };
    s.window
        .update(cx, |_, _, cx| {
            s.manager.update(cx, |m, cx| {
                m.settings = None;
                m.overview_tab = OverviewTab::Habits;
                m.habits = vec![
                    habit("digest", "Morning digest"),
                    habit("report", "Weekly report"),
                ];
                let documents = &mut m.roster.get_mut(&s.id).unwrap().documents;
                documents.push(document("plan", "Launch plan"));
                documents.push(document("summary", "Release summary"));
                cx.notify();
            });
        })
        .unwrap();
    let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let search = |visual: &mut gpui::VisualTestContext, key: &str, text: &str| {
        s.manager.update(visual, |m, cx| {
            m.fields[key].update(cx, |f, cx| f.set_text(text.into(), cx));
        });
        visual.run_until_parked();
    };
    assert!(visual.debug_bounds("mono-habit-report").is_some());
    search(visual, "mono-habit-search", "DIGEST");
    assert!(visual.debug_bounds("mono-habit-digest").is_some());
    assert!(visual.debug_bounds("mono-habit-report").is_none());

    s.manager.update(visual, |m, cx| {
        m.select_overview(OverviewTab::Documents, cx)
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-document-plan").is_some());
    search(visual, "mono-document-search", "release");
    assert!(visual.debug_bounds("mono-document-plan").is_none());
    assert!(visual.debug_bounds("mono-document-summary").is_some());
}
