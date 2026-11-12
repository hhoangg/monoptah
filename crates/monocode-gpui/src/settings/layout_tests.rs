use super::*;
use gpui::{
    Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions, point,
    size,
};

fn window(section: Section, cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1200.), px(800.)),
                ))),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut view = SettingsView::new(
                        section,
                        Preferences::default(),
                        SettingsContext::default(),
                        cx,
                    );
                    view.discovery_task = None;
                    view.discovery_loading = false;
                    view
                })
            },
        )
        .unwrap()
    });
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
}

#[gpui::test]
fn workspace_style_is_only_offered_under_the_new_layout(cx: &mut TestAppContext) {
    let window = window(Section::Appearance, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    for classic in [false, true, false] {
        window
            .update(visual, |view, _, cx| {
                view.ctx.classic = classic;
                view.ctx.new_layout_boxy = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(
            visual
                .debug_bounds("settings-new-layout-style-boxy")
                .is_some(),
            !classic
        );
        assert_eq!(
            visual
                .debug_bounds("settings-new-layout-style-rounded")
                .is_some(),
            !classic
        );
        assert!(
            visual
                .debug_bounds("settings-collapsed-project-rail-hidden")
                .is_none()
        );
    }
}

#[gpui::test]
fn skills_document_has_its_own_full_height_pane_and_compact_actions(cx: &mut TestAppContext) {
    let window = window(Section::Skills, cx);
    window
        .update(cx, |view, _, cx| {
            let path = std::path::PathBuf::from("/project/.agents/skills/review/SKILL.md");
            view.discovery.skills = vec![data::Skill {
                name: "review".into(),
                description: "Review changes".into(),
                path: path.clone(),
                scope: "project".into(),
                provider: "agents".into(),
            }];
            view.skill_preview = Some(path);
            view.skill_preview_text = Some(Ok(skill_document::DocumentData::new(format!(
                "---\nname: review\n---\n# Review\n\n{}",
                "Instructions.\n\n".repeat(150)
            ))));
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    let body = visual.debug_bounds("skill-document").unwrap();
    assert_eq!(left.size.height, px(800.));
    assert_eq!(right.size.height, px(800.));
    assert!(right.left() >= left.right());
    assert!(body.bottom() <= right.bottom());
    assert!(body.size.height > px(600.));
    assert!(visual.debug_bounds("transcript").is_none());
    for id in [
        "skill-preview-action",
        "skill-copy-action",
        "skill-reveal-action",
    ] {
        assert_eq!(
            visual.debug_bounds(id).unwrap().size,
            size(px(20.), px(20.))
        );
    }
    let source = visual
        .debug_bounds("settings-skill-document-source")
        .unwrap();
    assert!(source.size.width < px(100.));
    visual.simulate_click(source.center(), Default::default());
    visual.run_until_parked();
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.skill_source_paths.len(), 1)
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    // Deliver the frame that applies the measured Skills container breakpoint.
    visual.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    visual.run_until_parked();
    let left = visual.debug_bounds("skills-list-scroll").unwrap();
    let right = visual.debug_bounds("skill-preview-pane").unwrap();
    assert!(
        right.top() >= left.bottom(),
        "list={left:?}, preview={right:?}"
    );
    assert_eq!(left.size.width, px(700.));
    assert_eq!(right.size.width, px(700.));
    assert!(visual.debug_bounds("skill-document").unwrap().size.height > px(200.));
}

#[gpui::test]
fn mono_mascot_is_beside_name_and_sidebar_switch_stays_visible(cx: &mut TestAppContext) {
    let window = window(Section::Monos, cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.monos = vec![MonoSetting {
                id: "layout-mono".into(),
                name: "Big Boss".into(),
                mascot: "ghost".into(),
                color: 0xee99bb,
                projects: vec!["monocode".into(), "mono-web".into()],
                show: false,
                folders: false,
            }];
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let mascot = visual.debug_bounds("settings-mono-mascot").unwrap();
    let switch = visual
        .debug_bounds("settings-toggle-mono-sidebar-show")
        .unwrap();
    let reset = visual.debug_bounds("settings-mono-reset").unwrap();
    assert_eq!(mascot.size, size(px(16.), px(16.)));
    assert_eq!(switch.size, size(px(36.), px(20.)));
    let name = visual.debug_bounds("settings-mono-name").unwrap();
    assert!(mascot.right() < name.left());
    assert!(mascot.left() < px(200.));
    assert!(switch.left() > px(800.));
    assert!(switch.right() <= reset.left());
    assert!(reset.right() <= px(1200.));
    assert!((mascot.top() - switch.top()).abs() < px(4.));
    assert!(
        visual
            .debug_bounds("settings-toggle-mono-sidebar-folders")
            .is_none()
    );
    visual.simulate_click(reset.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    window
        .update(visual, |view, window, cx| {
            view.close_management(window, cx);
            cx.notify();
        })
        .unwrap();
    visual.simulate_resize(size(px(700.), px(800.)));
    visual.run_until_parked();
    let switch = visual
        .debug_bounds("settings-toggle-mono-sidebar-show")
        .unwrap();
    assert_eq!(switch.size, size(px(36.), px(20.)));
    assert!(switch.right() <= px(700.));
}

#[gpui::test]
fn each_provider_chooses_the_surface_its_new_sessions_open_in(cx: &mut TestAppContext) {
    use monocode_core::{provider::Kind, surface::Surface};
    let window = window(Section::Providers, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    // Search lands on the first provider row.
    assert!(
        visual
            .debug_bounds("settings-row-session-surface")
            .is_some()
    );
    for kind in Kind::ALL {
        for option in ["chat", "tui"] {
            let selector: &'static str = Box::leak(
                format!("settings-session-surface-{}-{option}", kind.id()).into_boxed_str(),
            );
            assert!(visual.debug_bounds(selector).is_some(), "{selector}");
        }
    }
    let prefs = |visual: &mut VisualTestContext| {
        window
            .update(visual, |view, _, _| view.prefs.provider_surfaces.clone())
            .unwrap()
    };
    assert!(
        prefs(visual).is_empty(),
        "nothing is saved until one is picked"
    );
    let tui = visual
        .debug_bounds("settings-session-surface-claude-tui")
        .unwrap();
    visual.simulate_click(tui.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(prefs(visual).get("claude"), Some(&Surface::Tui));
    assert_eq!(
        prefs(visual).get("codex"),
        None,
        "other providers keep theirs"
    );
    let chat = visual
        .debug_bounds("settings-session-surface-claude-chat")
        .unwrap();
    visual.simulate_click(chat.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(prefs(visual).get("claude"), Some(&Surface::Chat));
}

#[gpui::test]
fn the_surface_control_gives_way_to_a_hint_when_terminals_cannot_be_stored(
    cx: &mut TestAppContext,
) {
    let window = window(Section::Providers, cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.no_terminal_sessions = true;
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    assert!(
        visual
            .debug_bounds("settings-session-surface-claude-tui")
            .is_none()
    );
    assert!(
        visual
            .debug_bounds("settings-session-surface-claude-chat")
            .is_none()
    );
    // The row stays, so search still finds it and shows why.
    assert!(
        visual
            .debug_bounds("settings-row-session-surface")
            .is_some()
    );
}

fn tree(path: &str, branch: Option<&str>, locked: bool) -> data::Tree {
    data::Tree {
        tree: monocode_core::git::Worktree {
            path: path.into(),
            branch: branch.map(Into::into),
            ..Default::default()
        },
        locked,
        dirty: Some(false),
        unpushed: 0,
    }
}

fn worktree_window(cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    let window = window(Section::Worktrees, cx);
    window
        .update(cx, |view, _, cx| {
            view.selected_project = "/repo".into();
            view.discovery.worktrees.main = "/repo".into();
            view.discovery.worktrees.trees = vec![
                tree("/wt/a", Some("a"), false),
                tree("/wt/locked", Some("locked"), true),
                tree("/wt/detached", None, false),
                tree("/wt/b", Some("b"), false),
            ];
            cx.notify();
        })
        .unwrap();
    window
}

#[gpui::test]
fn worktrees_select_all_skips_locked_and_detached_trees(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let click_select_all = |visual: &mut VisualTestContext| {
        let bounds = visual.debug_bounds("select-all-trees").unwrap();
        visual.simulate_click(bounds.center(), Default::default());
    };
    click_select_all(visual);
    window
        .update(visual, |view, _, _| {
            let mut got = view.selected_trees.iter().cloned().collect::<Vec<_>>();
            got.sort();
            assert_eq!(got, ["/wt/a", "/wt/b"]);
        })
        .unwrap();
    // Blocked rows ignore clicks.
    let locked = visual.debug_bounds("select-tree-1").unwrap();
    visual.simulate_click(locked.center(), Default::default());
    // A selected row can be cleared on its own.
    let a = visual.debug_bounds("select-tree-0").unwrap();
    visual.simulate_click(a.center(), Default::default());
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.selected_trees.len(), 1);
            assert!(view.selected_trees.contains("/wt/b"));
        })
        .unwrap();
    click_select_all(visual);
    click_select_all(visual);
    window
        .update(visual, |view, _, _| assert!(view.selected_trees.is_empty()))
        .unwrap();
}

#[gpui::test]
fn selection_follows_the_discovered_worktrees(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    window
        .update(cx, |view, _, cx| {
            view.selected_trees = ["/wt/a", "/wt/b", "/wt/gone"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            view.discovery
                .worktrees
                .trees
                .retain(|t| t.tree.path != "/wt/b");
            view.prune_selected_trees();
            assert_eq!(view.selected_trees.len(), 1);
            assert!(view.selected_trees.contains("/wt/a"));
            view.select_section(Section::Archive, cx);
            assert!(view.selected_trees.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn bulk_delete_runs_one_remove_event_at_a_time_and_keeps_failures_selected(
    cx: &mut TestAppContext,
) {
    let window = worktree_window(cx);
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let view = window.root(visual).unwrap();
    let log = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &SettingsEvent, _| {
            if let SettingsEvent::RemoveWorktree {
                cwd,
                path,
                delete_sessions,
            } = event
            {
                log.borrow_mut()
                    .push((cwd.clone(), path.clone(), *delete_sessions));
            }
        })
        .detach();
    });
    window
        .update(visual, |view, _, cx| {
            view.selected_trees = ["/wt/a", "/wt/b"].iter().map(|s| s.to_string()).collect();
            cx.notify();
        })
        .unwrap();
    visual.run_until_parked();
    let button = visual.debug_bounds("delete-selected-trees").unwrap();
    visual.simulate_click(button.center(), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    window
        .update(visual, |view, _, cx| view.submit_management(cx))
        .unwrap();
    visual.run_until_parked();
    assert_eq!(events.borrow().len(), 1);
    assert_eq!(events.borrow()[0], ("/repo".into(), "/wt/a".into(), false));
    // Busy: a second submit does not start another delete.
    window
        .update(visual, |view, _, cx| view.submit_management(cx))
        .unwrap();
    visual.run_until_parked();
    assert_eq!(events.borrow().len(), 1);
    // The first fails; the run continues with the second.
    window
        .update(visual, |view, _, cx| {
            view.management_finished(Err("in use".into()), Some("/wt/a"), cx)
        })
        .unwrap();
    visual.run_until_parked();
    assert_eq!(events.borrow().len(), 2);
    assert_eq!(events.borrow()[1].1, "/wt/b");
    window
        .update(visual, |view, _, cx| {
            view.management_finished(Ok(()), Some("/wt/b"), cx)
        })
        .unwrap();
    // Checked before the refresh lands: the test folders don't exist on disk.
    window
        .update(visual, |view, _, _| {
            let editor = view.management.as_ref().expect("summary stays open");
            assert!(!editor.busy);
            assert_eq!(editor.bulk.as_ref().unwrap().failures().len(), 1);
            assert!(view.selected_trees.contains("/wt/a"));
            assert!(!view.selected_trees.contains("/wt/b"));
        })
        .unwrap();
    assert!(visual.debug_bounds("settings-management-dialog").is_some());
    assert_eq!(events.borrow().len(), 2);
}

#[gpui::test]
fn bulk_delete_resets_the_project_and_closes_when_everything_succeeds(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    window
        .update(cx, |view, window, cx| {
            view.selected_project = "/wt/a/sub".into();
            view.selected_trees = ["/wt/a"].iter().map(|s| s.to_string()).collect();
            let chosen = vec![view.discovery.worktrees.trees[0].clone()];
            view.open_management(management::Target::DeleteTrees(chosen), window, cx);
            view.submit_management(cx);
            view.management_finished(Ok(()), Some("/wt/a"), cx);
            assert_eq!(view.selected_project, "/repo");
            assert!(view.management.is_none());
            assert!(view.selected_trees.is_empty());
        })
        .unwrap();
}

fn record_removals(
    window: WindowHandle<SettingsView>,
    visual: &mut VisualTestContext,
) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = events.clone();
    let view = window.root(visual).unwrap();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &SettingsEvent, _| {
            if let SettingsEvent::RemoveWorktree { path, .. } = event {
                log.borrow_mut().push(path.clone());
            }
        })
        .detach();
    });
    events
}

#[gpui::test]
fn bulk_delete_ignores_results_for_another_worktree(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let events = record_removals(window, visual);
    window
        .update(visual, |view, window, cx| {
            let chosen = vec![
                view.discovery.worktrees.trees[0].clone(),
                view.discovery.worktrees.trees[3].clone(),
            ];
            view.selected_trees = ["/wt/a", "/wt/b"].iter().map(|s| s.to_string()).collect();
            view.open_management(management::Target::DeleteTrees(chosen), window, cx);
            view.submit_management(cx);
            // A stale result, a result for an item that is still queued, and one
            // with no path at all must not move the run on.
            view.management_finished(Ok(()), Some("/wt/old"), cx);
            view.management_finished(Ok(()), Some("/wt/b"), cx);
            view.management_finished(Ok(()), None, cx);
            let editor = view.management.as_ref().unwrap();
            assert!(editor.busy);
            let bulk = editor.bulk.as_ref().unwrap();
            assert_eq!(bulk.current().unwrap().path, "/wt/a");
            assert!(bulk.deleted().is_empty());
            assert_eq!(view.selected_trees.len(), 2);
        })
        .unwrap();
    assert_eq!(*events.borrow(), ["/wt/a"]);
    window
        .update(visual, |view, _, cx| {
            view.management_finished(Ok(()), Some("/wt/a"), cx)
        })
        .unwrap();
    assert_eq!(*events.borrow(), ["/wt/a", "/wt/b"]);
}

#[gpui::test]
fn single_delete_ignores_results_for_another_worktree(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    window
        .update(cx, |view, window, cx| {
            let tree = view.discovery.worktrees.trees[0].clone();
            view.open_management(management::Target::DeleteTree(tree), window, cx);
            // Not submitted yet: a result can't close the dialog.
            view.management_finished(Ok(()), Some("/wt/a"), cx);
            assert!(view.management.is_some());
            view.submit_management(cx);
            view.management_finished(Ok(()), Some("/wt/b"), cx);
            view.management_finished(Err("late".into()), None, cx);
            let editor = view.management.as_ref().unwrap();
            assert!(editor.busy && editor.error.is_none());
            view.management_finished(Ok(()), Some("/wt/a"), cx);
            assert!(view.management.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn results_without_a_path_still_finish_other_forms(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    window
        .update(cx, |view, window, cx| {
            view.open_management(management::Target::Mcp, window, cx);
            view.management_finished(Err("bad".into()), None, cx);
            assert_eq!(
                view.management.as_ref().unwrap().error.as_deref(),
                Some("bad")
            );
            view.management_finished(Ok(()), None, cx);
            assert!(view.management.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn a_running_delete_blocks_leaving_the_section(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    window
        .update(cx, |view, window, cx| {
            assert!(!view.delete_in_flight());
            let tree = view.discovery.worktrees.trees[0].clone();
            view.open_management(management::Target::DeleteTree(tree), window, cx);
            assert!(!view.delete_in_flight(), "open but not started");
            view.submit_management(cx);
            assert!(view.delete_in_flight());
            view.select_section(Section::Archive, cx);
            assert_eq!(view.section, Section::Worktrees);
            assert!(view.management.is_some());
            view.management_finished(Ok(()), Some("/wt/a"), cx);
            assert!(!view.delete_in_flight());
            view.select_section(Section::Archive, cx);
            assert_eq!(view.section, Section::Archive);
        })
        .unwrap();
}

#[gpui::test]
fn the_dialog_title_reports_a_finished_run_with_failures(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    window
        .update(cx, |view, window, cx| {
            let chosen = vec![
                view.discovery.worktrees.trees[0].clone(),
                view.discovery.worktrees.trees[3].clone(),
            ];
            view.open_management(management::Target::DeleteTrees(chosen), window, cx);
            assert_eq!(view.management_title(), "Delete 2 worktrees?");
            view.submit_management(cx);
            assert_eq!(view.management_title(), "Delete 2 worktrees?");
            view.management_finished(Ok(()), Some("/wt/a"), cx);
            view.management_finished(Err("in use".into()), Some("/wt/b"), cx);
            assert_eq!(view.management_title(), "1 of 2 worktrees deleted");
        })
        .unwrap();
}

#[gpui::test]
fn switching_project_in_the_picker_clears_the_selection(cx: &mut TestAppContext) {
    let window = worktree_window(cx);
    window
        .update(cx, |view, _, cx| {
            view.ctx.projects = vec![
                ("/repo".into(), "repo".into()),
                ("/other".into(), "other".into()),
            ];
            view.selected_trees = ["/wt/a"].iter().map(|s| s.to_string()).collect();
            cx.notify();
        })
        .unwrap();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let trigger = visual
        .debug_bounds("settings-select-settings-project")
        .unwrap();
    visual.simulate_click(trigger.center(), Default::default());
    visual.run_until_parked();
    let option = visual
        .debug_bounds("settings-option-settings-project-1")
        .unwrap();
    visual.simulate_click(option.center(), Default::default());
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.selected_project, "/other");
            assert!(view.selected_trees.is_empty());
        })
        .unwrap();
}

fn cloud_window(cx: &mut TestAppContext) -> WindowHandle<SettingsView> {
    let window = window(Section::General, cx);
    cx.update(|cx| {
        cx.set_global(crate::cloud_kit_ui::KitScanner(
            crate::cloud_kit_ui::fixture::scan,
        ))
    });
    window
        .update(cx, |view, _, cx| {
            view.selected_project = "/work/app".into();
            view.select_section(Section::Cloud, cx);
        })
        .unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    window
}

fn click(visual: &mut VisualTestContext, selector: &'static str) {
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    visual.simulate_click(bounds.center(), Default::default());
    visual.run_until_parked();
    visual.refresh().unwrap();
}

#[gpui::test]
fn the_cloud_page_checks_everything_compatible_and_refuses_local_only_items(
    cx: &mut TestAppContext,
) {
    let window = cloud_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();

    // Defaults: nothing stored, the compatible skill is checked.
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.cloud_kit.selection, None);
            assert!(matches!(view.cloud.load, pages::CloudLoad::Ready(_)));
        })
        .unwrap();
    assert!(
        visual
            .debug_bounds("cloud-kit-check-skill:user:review")
            .is_some()
    );

    // Unchecking turns the defaults into an explicit list of ids.
    click(visual, "cloud-kit-row-skill:user:review");
    window
        .update(visual, |view, _, _| {
            let selection = view.prefs.cloud_kit.selection.clone().unwrap();
            assert!(!selection.contains("skill:user:review"));
            assert!(selection.contains("skill:user:deploy"));
            assert!(selection.contains("plugin:fmt@market"));
        })
        .unwrap();

    // MCP tab: a local-only server is drawn, with a reason, and clicking it changes nothing.
    click(visual, "settings-cloud-kit-tab-mcp");
    let before = window
        .update(visual, |view, _, _| view.prefs.cloud_kit.clone())
        .unwrap();
    assert!(visual.debug_bounds("cloud-kit-row-mcp:user:mine").is_some());
    click(visual, "cloud-kit-row-mcp:user:mine");
    click(visual, "cloud-kit-check-mcp:user:mine");
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.cloud_kit, before);
            let selection = view.prefs.cloud_kit.selection.as_ref().unwrap();
            assert!(!selection.contains("mcp:user:mine"));
            assert!(
                selection.contains("mcp:user:github"),
                "secrets are on by default"
            );
            assert!(
                selection.contains("mcp:user:linear"),
                "OAuth-style servers are on by default"
            );
        })
        .unwrap();

    // A selectable server toggles.
    click(visual, "cloud-kit-row-mcp:user:github");
    window
        .update(visual, |view, _, _| {
            let selection = view.prefs.cloud_kit.selection.as_ref().unwrap();
            assert!(!selection.contains("mcp:user:github"));
            // What is stored is ids only.
            let saved = serde_json::to_string(&view.prefs).unwrap();
            assert!(!saved.contains(crate::cloud_kit_ui::fixture::FAKE_SECRET));
        })
        .unwrap();

    // Back to the defaults.
    click(visual, "cloud-kit-reset");
    window
        .update(visual, |view, _, _| {
            assert_eq!(view.prefs.cloud_kit.selection, None);
        })
        .unwrap();
}

#[gpui::test]
fn the_instructions_switch_shows_the_source_and_what_stays_behind(cx: &mut TestAppContext) {
    let window = cloud_window(cx);
    cx.update(|cx| {
        cx.set_global(crate::cloud_kit_ui::KitScanner(
            crate::cloud_kit_ui::fixture::scan_with_instructions,
        ))
    });
    window
        .update(cx, |view, _, cx| view.refresh_cloud(cx))
        .unwrap();
    cx.run_until_parked();
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();

    // On by default, and the import that stays behind is listed.
    window
        .update(visual, |view, _, _| {
            assert!(view.prefs.cloud_kit.instructions)
        })
        .unwrap();
    assert!(
        visual
            .debug_bounds("cloud-kit-instructions-warning-0")
            .is_some()
    );
    click(visual, "settings-toggle-cloud-kit-instructions");
    window
        .update(visual, |view, _, _| {
            assert!(!view.prefs.cloud_kit.instructions)
        })
        .unwrap();
    click(visual, "settings-toggle-cloud-kit-instructions");
    window
        .update(visual, |view, _, _| {
            assert!(view.prefs.cloud_kit.instructions)
        })
        .unwrap();
}

#[gpui::test]
fn the_instructions_switch_is_disabled_without_a_claude_md(cx: &mut TestAppContext) {
    let window = cloud_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    click(visual, "settings-toggle-cloud-kit-instructions");
    window
        .update(visual, |view, _, _| {
            assert!(view.prefs.cloud_kit.instructions)
        })
        .unwrap();
}

#[gpui::test]
fn the_instructions_switch_follows_the_preference_while_the_scan_has_not_answered(
    cx: &mut TestAppContext,
) {
    let window = cloud_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    window
        .update(visual, |view, _, cx| {
            view.cloud.load = pages::CloudLoad::Loading;
            cx.notify();
        })
        .unwrap();
    visual.refresh().unwrap();
    // Not disabled: a click turns the preference off, and nothing claims there is no CLAUDE.md.
    click(visual, "settings-toggle-cloud-kit-instructions");
    window
        .update(visual, |view, _, _| {
            assert!(!view.prefs.cloud_kit.instructions)
        })
        .unwrap();
}

#[gpui::test]
fn the_cloud_notice_shows_once_and_is_stored_as_acknowledged(cx: &mut TestAppContext) {
    let window = cloud_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("cloud-kit-notice").is_none());

    // First time on: the notice opens, and the kit stays off until it is accepted.
    click(visual, "settings-toggle-cloud-kit-enabled");
    assert!(visual.debug_bounds("cloud-kit-notice").is_some());
    window
        .update(visual, |view, _, _| {
            assert!(!view.prefs.cloud_kit.enabled);
            assert!(!view.prefs.cloud_kit.notice_acknowledged);
        })
        .unwrap();

    // Cancel leaves everything as it was.
    click(visual, "cloud-kit-notice-cancel");
    assert!(visual.debug_bounds("cloud-kit-notice").is_none());
    window
        .update(visual, |view, _, _| assert!(!view.prefs.cloud_kit.enabled))
        .unwrap();

    click(visual, "settings-toggle-cloud-kit-enabled");
    click(visual, "cloud-kit-notice-accept");
    assert!(visual.debug_bounds("cloud-kit-notice").is_none());
    window
        .update(visual, |view, _, _| {
            assert!(view.prefs.cloud_kit.enabled);
            assert!(view.prefs.cloud_kit.notice_acknowledged);
        })
        .unwrap();

    // Off and on again: no second notice.
    click(visual, "settings-toggle-cloud-kit-enabled");
    window
        .update(visual, |view, _, _| assert!(!view.prefs.cloud_kit.enabled))
        .unwrap();
    click(visual, "settings-toggle-cloud-kit-enabled");
    assert!(visual.debug_bounds("cloud-kit-notice").is_none());
    window
        .update(visual, |view, _, _| assert!(view.prefs.cloud_kit.enabled))
        .unwrap();
}

#[gpui::test]
fn the_cloud_page_has_a_refresh_button_and_stays_when_preferences_change(cx: &mut TestAppContext) {
    let window = cloud_window(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.refresh().unwrap();
    assert!(visual.debug_bounds("refresh-cloud").is_some());

    let on = Preferences::default();
    window
        .update(visual, |view, _, cx| view.set_preferences(&on, cx))
        .unwrap();
    window
        .update(visual, |view, _, cx| {
            assert_eq!(view.section, Section::Cloud);
            view.set_preferences(&Preferences::default(), cx);
            assert_eq!(
                view.section,
                Section::Cloud,
                "the page has no flag any more"
            );
        })
        .unwrap();
}

/// The DEFAULT badge marks the account new sessions use, and the pin button on
/// another account makes that one the default and moves the badge to it.
#[gpui::test]
fn the_default_badge_follows_the_account_chosen_with_the_pin_button(cx: &mut TestAppContext) {
    use monocode_core::{accounts::DEFAULT, provider::Kind};
    let _clis = crate::shell::surface_tests::NoClis::new();
    let dir = std::env::temp_dir().join(format!(
        "native-settings-accounts-{}",
        monocode_core::mono::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = std::sync::Arc::new(std::sync::Mutex::new(
        monocode_core::store::Store::open(&dir.join("native.sqlite")).unwrap(),
    ));
    cx.update(|cx| crate::accounts::install(store, cx));
    let account = cx.update(|cx| {
        crate::accounts::get(cx).unwrap().update(cx, |center, _| {
            center.book.add(Kind::Claude, "Token").unwrap().id
        })
    });
    let window = window(Section::Providers, cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    let badge = |id: &str| format!("account-default-badge-claude-{id}");
    let pin = |id: &str| format!("account-set-default-claude-{id}");
    let has = |visual: &mut VisualTestContext, selector: String| {
        visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .is_some()
    };
    // The CLI login is the default until another account is chosen.
    assert!(has(visual, badge(DEFAULT)));
    assert!(!has(visual, pin(DEFAULT)));
    assert!(!has(visual, badge(&account)));
    let button = visual
        .debug_bounds(Box::leak(pin(&account).into_boxed_str()))
        .unwrap();
    visual.simulate_click(button.center(), Default::default());
    visual.run_until_parked();
    let preferred = visual.update(|_, cx| {
        crate::accounts::get(cx)
            .unwrap()
            .read(cx)
            .book
            .preferred(Kind::Claude)
    });
    assert_eq!(preferred, account);
    assert!(has(visual, badge(&account)));
    assert!(!has(visual, badge(DEFAULT)));
    // The CLI login can be made the default again.
    assert!(has(visual, pin(DEFAULT)));
    let _ = std::fs::remove_dir_all(dir);
}
