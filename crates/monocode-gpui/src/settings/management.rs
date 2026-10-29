//! React Settings forms and their asynchronous actions.
use super::widgets::{note, secondary_button};
use super::*;
use monocode_core::provider::Kind;

#[derive(Clone)]
pub(super) enum Target {
    Cli(Kind),
    Mcp,
    Skill,
    Worktree,
    DeleteTree(data::Tree),
    DeleteTrees(Vec<data::Tree>),
    ResetMono(super::MonoSetting),
}
pub(super) struct Editor {
    pub target: Target,
    pub name: Entity<Input>,
    pub body: Entity<Input>,
    pub provider: String,
    pub scope: String,
    pub base: String,
    pub existing: bool,
    pub delete_sessions: bool,
    pub bulk: Option<bulk_delete::BulkDelete>,
    pub busy: bool,
    pub error: Option<String>,
    pub editing: bool,
    pub inspection: Option<monocode_core::cli::Inspection>,
    previous: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}
impl SettingsView {
    pub(super) fn open_management(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.management.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        self.management_generation += 1;
        let (label, placeholder) = match target {
            Target::Cli(_) => ("CLI path", "Auto-detected path"),
            Target::Mcp => ("Name (optional with mcpServers)", "my-server"),
            Target::Skill => ("Skill name", "skill-name"),
            _ => ("New branch name", "feature/my-task"),
        };
        let name = cx.new(|cx| {
            let mut input = Input::field(label, placeholder, content(0.35), cx);
            input.set_font_size(12.);
            input
        });
        if let Target::Cli(kind) = target {
            name.update(cx, |input, cx| {
                input.set_text(
                    self.prefs
                        .binary_paths
                        .get(kind.id())
                        .cloned()
                        .unwrap_or_default(),
                    cx,
                )
            });
        }
        if matches!(target, Target::Skill) {
            name.update(cx, |input, cx| {
                input.set_font_size(13.);
                input.set_text(monocode_core::skills::slug_name(&self.skill_query), cx);
            });
        }
        let body=cx.new(|cx|Input::multiline_field("JSON configuration", "{\"mcpServers\":{\"my-server\":{\"command\":\"npx\",\"args\":[\"-y\",\"example-mcp\"]}}}",cx));
        let subscriptions = vec![
            cx.subscribe(&name, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&body, |_, _, _: &input::Changed, cx| cx.notify()),
            cx.subscribe(&name, |this, _, _: &input::Submitted, cx| {
                this.submit_management(cx)
            }),
        ];
        let previous = window.focused(cx);
        if matches!(
            target,
            Target::Cli(_) | Target::DeleteTree(_) | Target::DeleteTrees(_) | Target::ResetMono(_)
        ) {
            window.focus(&self.focus, cx);
        } else {
            window.focus(&name.read(cx).focus_handle(cx), cx);
        }
        let provider = if self.mcp_filter == "all" {
            "claude".into()
        } else {
            self.mcp_filter.clone()
        };
        let scope = if matches!(target, Target::Skill) {
            if self.selected_project.is_empty() || self.selected_project == "~" {
                "user"
            } else {
                "project"
            }
        } else if matches!(provider.as_str(), "claude_desktop" | "codex") {
            "user"
        } else if provider == "claude" {
            "local"
        } else {
            "project"
        }
        .into();
        self.management = Some(Editor {
            target: target.clone(),
            name,
            body,
            provider,
            scope,
            base: "HEAD".into(),
            existing: false,
            delete_sessions: false,
            bulk: None,
            busy: false,
            error: None,
            editing: false,
            inspection: None,
            previous,
            _subscriptions: subscriptions,
        });
        if let Target::Cli(kind) = target {
            self.inspect_cli(kind, false, cx);
        }
        cx.notify();
    }
    pub(super) fn close_management(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.management.as_ref().is_some_and(|e| e.busy) {
            return;
        }
        self.management_generation += 1;
        if let Some(focus) = self.management.take().and_then(|e| e.previous) {
            window.focus(&focus, cx);
        }
        self.select_open = None;
        cx.notify();
    }
    fn inspect_cli(&mut self, kind: Kind, save: bool, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.management else {
            return;
        };
        let draft = editor.name.read(cx).content.trim().to_owned();
        editor.busy = true;
        editor.error = None;
        let generation = self.management_generation;
        let work = cx
            .background_executor()
            .spawn(async move { monocode_core::cli::inspect(kind, Some(&draft)) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.management_generation != generation {
                    return;
                }
                if let Some(editor) = &mut this.management {
                    editor.busy = false;
                    match result {
                        Ok(inspection) => {
                            editor.inspection = Some(inspection);
                            if save {
                                let value = editor.name.read(cx).content.trim().to_owned();
                                this.update(cx, |p| {
                                    if value.is_empty() {
                                        p.binary_paths.remove(kind.id());
                                    } else {
                                        p.binary_paths.insert(kind.id().into(), value);
                                    }
                                });
                                this.management = None;
                            }
                        }
                        Err(e) => editor.error = Some(e),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    /// True while a worktree delete is running, so Settings must stay open to
    /// receive its result.
    pub(crate) fn delete_in_flight(&self) -> bool {
        self.management.as_ref().is_some_and(|e| {
            e.busy && matches!(e.target, Target::DeleteTree(_) | Target::DeleteTrees(_))
        })
    }
    /// Starts deleting `path` as the Delete button would, for tests in other modules.
    #[cfg(test)]
    pub(crate) fn start_delete_for_test(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tree = data::Tree {
            tree: monocode_core::git::Worktree {
                path: path.into(),
                branch: Some("topic".into()),
                ..Default::default()
            },
            locked: false,
            dirty: Some(false),
            unpushed: 0,
        };
        self.open_management(Target::DeleteTree(tree), window, cx);
        self.submit_management(cx);
    }
    /// Whether a finished action belongs to the form that is open. Worktree
    /// deletes carry their path and must match the one in flight; every other
    /// action has no path.
    fn accepts_result(&self, path: Option<&str>) -> bool {
        let Some(editor) = &self.management else {
            return false;
        };
        match &editor.target {
            Target::DeleteTree(tree) => editor.busy && path == Some(tree.tree.path.as_str()),
            Target::DeleteTrees(_) => editor
                .bulk
                .as_ref()
                .and_then(|b| b.current())
                .is_some_and(|item| path == Some(item.path.as_str())),
            _ => path.is_none(),
        }
    }
    pub(super) fn management_title(&self) -> String {
        let Some(editor) = &self.management else {
            return String::new();
        };
        match &editor.target {
            Target::Cli(kind) => format!("{} CLI details", kind.label()),
            Target::Mcp => "Add MCP server".into(),
            Target::Worktree => "Create worktree".into(),
            Target::DeleteTree(_) => "Delete worktree?".into(),
            Target::DeleteTrees(trees) => match &editor.bulk {
                Some(bulk) if bulk.finished() && !bulk.failures().is_empty() => {
                    bulk_delete::result_title(bulk.deleted().len(), bulk.total())
                }
                _ => format!("Delete {} worktrees?", trees.len()),
            },
            Target::ResetMono(mono) => format!("Reset {} to its defaults?", mono.name),
            Target::Skill => String::new(),
        }
    }
    /// `path` is the worktree a delete result is about; results of other
    /// actions pass `None`. A result that does not match the open form is ignored.
    pub(crate) fn management_finished(
        &mut self,
        result: Result<(), String>,
        path: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts_result(path) {
            return;
        }
        if self
            .management
            .as_ref()
            .is_some_and(|e| e.bulk.as_ref().is_some_and(|b| b.running()))
        {
            self.bulk_delete_finished(result, cx);
            return;
        }
        if let Some(editor) = &mut self.management {
            editor.busy = false;
            match result {
                Ok(()) => {
                    if let Some(editor) = &self.management
                        && let Target::DeleteTree(tree) = &editor.target
                        && monocode_core::files::is_equal_or_inside(
                            &self.selected_project,
                            &tree.tree.path,
                        )
                    {
                        self.selected_project = self.discovery.worktrees.main.clone();
                    }
                    self.management = None;
                    self.refresh(cx);
                }
                Err(e) => editor.error = Some(e),
            }
        }
        cx.notify();
    }
    /// Drops ticked worktrees that are no longer in the list or can't be deleted.
    pub(super) fn prune_selected_trees(&mut self) {
        let paths = bulk_delete::selectable_paths(&self.discovery.worktrees.trees);
        bulk_delete::prune(&mut self.selected_trees, &paths);
    }
    /// Result of one worktree in a bulk delete. Each one goes through the same
    /// `RemoveWorktree` event as a single delete; this decides what comes next.
    fn bulk_delete_finished(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.management else {
            return;
        };
        let Some(bulk) = &mut editor.bulk else {
            return;
        };
        let Some(done) = bulk.current().cloned() else {
            return;
        };
        let ok = result.is_ok();
        let next = bulk.record(result);
        if ok {
            self.selected_trees.remove(&done.path);
            if monocode_core::files::is_equal_or_inside(&self.selected_project, &done.path) {
                self.selected_project = self.discovery.worktrees.main.clone();
            }
        }
        let cwd = self.selected_project.clone();
        let Some(editor) = &mut self.management else {
            return;
        };
        if let Some(next) = next {
            cx.emit(SettingsEvent::RemoveWorktree {
                cwd,
                path: next.path,
                delete_sessions: editor.delete_sessions,
            });
            cx.notify();
            return;
        }
        editor.busy = false;
        if editor
            .bulk
            .as_ref()
            .is_some_and(|b| b.failures().is_empty())
        {
            self.management = None;
        }
        self.refresh(cx);
        cx.notify();
    }
    pub(super) fn submit_management(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &mut self.management else {
            return;
        };
        if editor.busy {
            return;
        }
        if let Target::Cli(kind) = editor.target {
            self.inspect_cli(kind, true, cx);
            return;
        }
        let name = editor.name.read(cx).content.trim().to_owned();
        let body = editor.body.read(cx).content.to_string();
        let cwd = self.selected_project.clone();
        let target = editor.target.clone();
        let provider = editor.provider.clone();
        let scope = editor.scope.clone();
        let base = editor.base.clone();
        let existing = editor.existing;
        if matches!(target, Target::Worktree | Target::Skill) && name.is_empty() {
            return;
        }
        editor.busy = true;
        editor.error = None;
        if let Target::ResetMono(mono) = target {
            cx.emit(SettingsEvent::ResetMono(mono.id));
            cx.notify();
            return;
        }
        if let Target::DeleteTrees(trees) = target {
            let mut bulk =
                bulk_delete::BulkDelete::new(trees.iter().map(bulk_delete::item).collect());
            let first = bulk.start();
            editor.bulk = Some(bulk);
            match first {
                Some(first) => cx.emit(SettingsEvent::RemoveWorktree {
                    cwd,
                    path: first.path,
                    delete_sessions: editor.delete_sessions,
                }),
                None => editor.busy = false,
            }
            cx.notify();
            return;
        }
        if let Target::DeleteTree(tree) = target {
            cx.emit(SettingsEvent::RemoveWorktree {
                cwd,
                path: tree.tree.path,
                delete_sessions: editor.delete_sessions,
            });
            cx.notify();
            return;
        }
        let generation = self.management_generation;
        let work = cx.background_executor().spawn(async move {
            match target {
                Target::Mcp => monocode_core::mcp::mcp_add(cwd, provider, scope, name, body),
                Target::Skill => {
                    monocode_core::skills::create_blank(&cwd, &name, scope == "user").map(|_| ())
                }
                Target::Worktree => monocode_core::git::create_project_worktree(
                    std::path::Path::new(&cwd),
                    &name,
                    Some(&base),
                    existing,
                )
                .map(|_| ()),
                _ => Ok(()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.management_generation == generation {
                    this.management_finished(result, None, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn management_form(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(e) = &self.management else {
            return div().into_any_element();
        };
        let target = e.target.clone();
        let name = e.name.clone();
        let body = e.body.clone();
        let busy = e.busy;
        name.update(cx, |input, _| input.set_read_only(busy));
        body.update(cx, |input, _| input.set_read_only(busy));
        let error = e.error.clone();
        let provider = e.provider.clone();
        let scope = e.scope.clone();
        let existing = e.existing;
        let base = e.base.clone();
        let deleting = e.delete_sessions;
        let bulk = e.bulk.as_ref().map(|b| {
            (
                b.running(),
                b.finished(),
                b.position(),
                b.total(),
                b.deleted().len(),
                b.failures().to_vec(),
            )
        });
        let editing = e.editing;
        let inspection = e.inspection.clone();
        let mut rows = Vec::new();
        match &target {
            Target::ResetMono(mono) => {
                let default = monocode_core::mono::Mono {
                    name: String::new(),
                    mascot: mono.mascot.clone(),
                    ..Default::default()
                }
                .label();
                rows.push(note(format!("Its soul goes back to the default and its name to {default}. Changes to its soul can't be recovered.")));
                rows.push(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .child("Its conversation, projects, memory and habits will be kept.")
                        .into_any_element(),
                );
            }
            Target::Cli(kind) => {
                let overridden = self.prefs.binary_paths.contains_key(kind.id());
                let pending = monocode_core::cli::configured(*kind)
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    != self.prefs.binary_paths.get(kind.id()).cloned();
                rows.push(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .text_size(px(12.))
                        .child(format!("{} CLI", kind.label()))
                        .child(
                            div()
                                .text_size(px(10.))
                                .text_color(content(0.5))
                                .child(format!(
                                    "Global path · {}",
                                    if error.is_some() {
                                        "Needs attention"
                                    } else if pending {
                                        "Restart required"
                                    } else if overridden {
                                        "Configured"
                                    } else {
                                        "Auto-detected"
                                    }
                                )),
                        )
                        .into_any_element(),
                );
                if editing {
                    rows.push(form_field("CLI path", name, 32.));
                    rows.push(note("Enter the absolute path to the CLI executable. Changes apply after restarting Monoptah."));
                } else {
                    rows.push(
                        div()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .bg(content(0.03))
                            .text_size(px(10.))
                            .child(
                                div().font_family(crate::fonts::mono()).child(
                                    inspection
                                        .as_ref()
                                        .map(|i| i.path.display().to_string())
                                        .unwrap_or_else(|| {
                                            if busy {
                                                "Checking the selected CLI…".into()
                                            } else {
                                                "CLI could not be resolved".into()
                                            }
                                        }),
                                ),
                            )
                            .child(div().mt_1().text_color(content(0.4)).child(
                                inspection.map(|i| i.version).unwrap_or_else(|| {
                                    if busy {
                                        "Checking version…".into()
                                    } else {
                                        "Retry to check this CLI".into()
                                    }
                                }),
                            ))
                            .into_any_element(),
                    );
                }
            }
            Target::Mcp => {
                let available = self.scroll.bounds().size.width.as_f32();
                let column = ((if available > 0. {
                    available.min(560.)
                } else {
                    560.
                }) - 46.)
                    / 2.;
                rows.push(note(
                    "Paste a server configuration and choose where to add it.",
                ));
                rows.push(
                    div()
                        .flex()
                        .gap_3()
                        .child(labeled_control(
                            "Provider",
                            self.select(
                                "mcp-provider",
                                "Provider",
                                &provider,
                                [
                                    ("claude", "Claude Code"),
                                    ("claude_desktop", "Claude Desktop"),
                                    ("codex", "Codex"),
                                    ("cursor", "Cursor"),
                                    ("opencode", "OpenCode"),
                                ]
                                .into_iter()
                                .map(|(id, label)| (id.into(), label.into(), None))
                                .collect(),
                                Some(column),
                                cx,
                                |this, v, cx| {
                                    if let Some(e) = &mut this.management {
                                        e.provider = v;
                                        e.scope = match e.provider.as_str() {
                                            "claude" => "local",
                                            "codex" | "claude_desktop" => "user",
                                            _ => "project",
                                        }
                                        .into();
                                    }
                                    cx.notify();
                                },
                            ),
                        ))
                        .child(labeled_control(
                            "Scope",
                            self.select(
                                "mcp-scope",
                                "Scope",
                                &scope,
                                scopes(&provider)
                                    .iter()
                                    .map(|s| {
                                        (
                                            s.to_string(),
                                            SharedString::from(match *s {
                                                "local" => "Local",
                                                "project" => "Project",
                                                _ => "User",
                                            }),
                                            None,
                                        )
                                    })
                                    .collect(),
                                Some(column),
                                cx,
                                |this, v, cx| {
                                    if let Some(e) = &mut this.management {
                                        e.scope = v;
                                    }
                                    cx.notify();
                                },
                            ),
                        ))
                        .into_any_element(),
                );
                rows.push(form_field("Name (optional with mcpServers)", name, 32.));
                rows.push(form_field("JSON configuration", body, 140.));
                rows.push(note("Paste one entry from an mcpServers block, or a single server object with a name above."));
            }
            Target::Skill => {
                rows.push(form_field("Skill name", name, 32.));
                rows.push(self.segmented(
                    "skill-scope",
                    "Skill scope",
                    &scope,
                    &[("project", "Project"), ("user", "Personal")],
                    cx,
                    |this, v, cx| {
                        if let Some(e) = &mut this.management {
                            e.scope = v.into();
                        }
                        cx.notify();
                    },
                ));
                rows.push(note(
                    "Creates a starter SKILL.md in .agents/skills. Edit it in your file editor.",
                ));
            }
            Target::Worktree => {
                rows.push(note(format!("An independent working copy of {}. Existing uncommitted changes stay in their current working copy.",self.selected_project)));
                rows.push(self.select(
                    "worktree-type",
                    "Branch type",
                    if existing { "existing" } else { "new" },
                    vec![
                        ("new".into(), "Create a new branch".into(), None),
                        (
                            "existing".into(),
                            "Use an existing local branch".into(),
                            None,
                        ),
                    ],
                    None,
                    cx,
                    |this, v, cx| {
                        if let Some(e) = &mut this.management {
                            e.existing = v == "existing";
                            e.name.update(cx, |i, cx| i.set_text(String::new(), cx));
                        }
                        cx.notify();
                    },
                ));
                if existing {
                    let selected = name.read(cx).content.to_string();
                    rows.push(
                        self.select(
                            "worktree-branch",
                            "Existing branch",
                            &selected,
                            self.discovery
                                .worktrees
                                .branches
                                .iter()
                                .filter(|b| b.remote.is_none())
                                .map(|b| (b.name.clone(), b.name.clone().into(), None))
                                .collect(),
                            None,
                            cx,
                            |this, v, cx| {
                                if let Some(e) = &mut this.management {
                                    e.name.update(cx, |i, cx| i.set_text(v, cx));
                                }
                                cx.notify();
                            },
                        ),
                    );
                } else {
                    rows.push(form_field("New branch name", name, 36.));
                    let mut options = vec![("HEAD".into(), "Current commit".into(), None)];
                    options.extend(self.discovery.worktrees.branches.iter().map(|b| {
                        let label = b
                            .remote
                            .as_ref()
                            .map(|r| format!("{r}/{}", b.name))
                            .unwrap_or_else(|| b.name.clone());
                        (label.clone(), label.into(), None)
                    }));
                    rows.push(self.select(
                        "worktree-base",
                        "Start from",
                        &base,
                        options,
                        None,
                        cx,
                        |this, v, cx| {
                            if let Some(e) = &mut this.management {
                                e.base = v;
                            }
                            cx.notify();
                        },
                    ));
                }
                rows.push(note(format!(
                    "Created in {}",
                    self.discovery.worktrees.default_root.display()
                )));
            }
            Target::DeleteTrees(trees) => {
                let count = trees
                    .iter()
                    .map(|tree| {
                        self.ctx
                            .sessions
                            .iter()
                            .filter(|(_, p, _)| {
                                monocode_core::files::is_equal_or_inside(p, &tree.tree.path)
                            })
                            .count()
                    })
                    .sum::<usize>();
                let (dirty, unknown) = bulk_delete::split_dirty(trees);
                let unpushed = trees.iter().map(|t| t.unpushed).sum::<usize>();
                if let Some((running, finished, position, total, deleted, failures)) = &bulk {
                    if *running {
                        rows.push(
                            div()
                                .id("bulk-delete-progress")
                                .role(Role::Status)
                                .text_size(px(12.))
                                .child(format!("Deleting {position} of {total}"))
                                .into_any_element(),
                        );
                    } else if *finished {
                        rows.push(
                            div()
                                .id("bulk-delete-summary")
                                .role(Role::Alert)
                                .text_size(px(12.))
                                .text_color(gpui::rgb(0xf87171))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(format!(
                                    "Deleted {deleted} of {total}. {} could not be deleted and stay selected:",
                                    failures.len()
                                ))
                                .children(failures.iter().map(|f| {
                                    format!("{} ({}): {}", f.item.label, f.item.path, f.error)
                                }))
                                .into_any_element(),
                        );
                    }
                } else {
                    rows.push(note(format!(
                        "This permanently deletes {} working copies and everything inside them.",
                        trees.len()
                    )));
                    let mut consequences = div()
                        .rounded_lg()
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.05))
                        .p_3()
                        .text_size(px(12.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .children(trees.iter().map(|t| t.tree.path.clone()));
                    if count > 0 {
                        consequences = consequences.child(format!(
                            "{count} sessions using these worktrees are {}",
                            if deleting {
                                "permanently deleted."
                            } else {
                                "kept. Select a branch or worktree to continue them."
                            }
                        ));
                    }
                    if !dirty.is_empty() {
                        consequences = consequences.child(
                            div().text_color(gpui::rgb(0xfbbf24)).child(format!(
                                "All uncommitted and untracked changes are discarded in: {}.",
                                dirty.join(", ")
                            )),
                        );
                    }
                    if !unknown.is_empty() {
                        consequences = consequences.child(
                            div().text_color(gpui::rgb(0xfbbf24)).child(format!(
                                "Status unknown (changes may be discarded): {}.",
                                unknown.join(", ")
                            )),
                        );
                    }
                    consequences = consequences.child("Their branches and commits are kept.");
                    if unpushed > 0 {
                        consequences = consequences.child(format!(
                            "{unpushed} unpublished commits stay on the branches."
                        ));
                    }
                    rows.push(consequences.into_any_element());
                    if count > 0 {
                        rows.push(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .p_3()
                                .rounded_lg()
                                .border_1()
                                .border_color(content(0.1))
                                .text_size(px(12.5))
                                .child("Also delete associated sessions")
                                .child(self.toggle(
                                    "delete-worktree-sessions",
                                    "Also delete associated sessions",
                                    deleting,
                                    busy,
                                    cx,
                                    |this, on, cx| {
                                        if let Some(e) = &mut this.management {
                                            e.delete_sessions = on;
                                        }
                                        cx.notify();
                                    },
                                ))
                                .into_any_element(),
                        );
                    }
                }
            }
            Target::DeleteTree(tree) => {
                let count = self
                    .ctx
                    .sessions
                    .iter()
                    .filter(|(_, p, _)| {
                        monocode_core::files::is_equal_or_inside(p, &tree.tree.path)
                    })
                    .count();
                rows.push(note(
                    "This permanently deletes the working copy and everything inside it.",
                ));
                let mut consequences = div()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.1))
                    .bg(content(0.05))
                    .p_3()
                    .text_size(px(12.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(tree.tree.path.clone());
                if count > 0 {
                    consequences = consequences.child(format!(
                        "{count} sessions using this worktree are {}",
                        if deleting {
                            "permanently deleted."
                        } else {
                            "kept. Select a branch or worktree to continue them."
                        }
                    ));
                }
                if tree.dirty != Some(false) {
                    consequences = consequences.child(
                        div()
                            .text_color(gpui::rgb(0xfbbf24))
                            .child("All uncommitted and untracked changes here are discarded."),
                    );
                }
                consequences = consequences.child(format!(
                    "The {} branch and its commits are kept.",
                    tree.tree.branch.as_deref().unwrap_or("worktree")
                ));
                if tree.unpushed > 0 {
                    consequences = consequences.child(format!(
                        "{} unpublished commits stay on the branch.",
                        tree.unpushed
                    ));
                }
                rows.push(consequences.into_any_element());
                if count > 0 {
                    rows.push(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(content(0.1))
                            .text_size(px(12.5))
                            .child("Also delete associated sessions")
                            .child(self.toggle(
                                "delete-worktree-sessions",
                                "Also delete associated sessions",
                                deleting,
                                busy,
                                cx,
                                |this, on, cx| {
                                    if let Some(e) = &mut this.management {
                                        e.delete_sessions = on;
                                    }
                                    cx.notify();
                                },
                            ))
                            .into_any_element(),
                    );
                }
            }
        }
        if let Some(error) = error {
            rows.push(
                div()
                    .id("management-error")
                    .role(Role::Alert)
                    .text_size(px(11.))
                    .text_color(gpui::rgb(0xf87171))
                    .child(error)
                    .into_any_element(),
            );
        }
        let mut actions = div().flex().items_center().justify_end().gap_2();
        if let Target::Cli(kind) = target {
            if !editing {
                if self.management.as_ref().is_some_and(|e| e.error.is_some()) {
                    actions =
                        actions.child(
                            secondary_button("retry-cli", "Retry CLI inspection", false, busy)
                                .child("Retry")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.inspect_cli(kind, false, cx)
                                })),
                        );
                }
                let path = self
                    .management
                    .as_ref()
                    .and_then(|e| e.inspection.as_ref())
                    .map(|i| i.path.clone());
                actions = actions
                    .child(
                        secondary_button("reveal-cli", "Open CLI location", false, path.is_none())
                            .child("Open location")
                            .on_click(move |_, _, cx| {
                                if let Some(path) = &path {
                                    cx.reveal_path(path);
                                }
                            }),
                    )
                    .child(
                        secondary_button("edit-cli", "Edit CLI path", false, busy)
                            .child("Edit path")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(e) = &mut this.management {
                                    e.editing = true;
                                    window.focus(&e.name.read(cx).focus_handle(cx), cx);
                                }
                                cx.notify();
                            })),
                    );
                rows.push(actions.into_any_element());
                return div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(rows)
                    .into_any_element();
            } else if self.prefs.binary_paths.contains_key(kind.id()) {
                actions = actions.child(
                    secondary_button("auto-cli", "Use auto-detected path", false, busy)
                        .child("Use auto-detected path")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(e) = &mut this.management {
                                e.name.update(cx, |i, cx| i.set_text(String::new(), cx));
                            }
                            this.inspect_cli(kind, true, cx);
                        })),
                );
            }
        }
        let label = match target {
            Target::Cli(_) => "Save path",
            Target::Mcp => {
                if busy {
                    "Adding…"
                } else {
                    "Add server"
                }
            }
            Target::Skill => {
                if busy {
                    "Creating…"
                } else {
                    "Create skill"
                }
            }
            Target::Worktree => "Create worktree",
            Target::ResetMono(_) => {
                if busy {
                    "Resetting…"
                } else {
                    "Reset Mono"
                }
            }
            Target::DeleteTree(_) => {
                if deleting {
                    "Delete worktree and sessions"
                } else {
                    "Delete worktree"
                }
            }
            Target::DeleteTrees(_) => "",
        };
        let label = match &target {
            Target::DeleteTrees(trees) if busy => format!("Deleting {} worktrees…", trees.len()),
            Target::DeleteTrees(trees) if deleting => {
                format!("Delete {} worktrees and sessions", trees.len())
            }
            Target::DeleteTrees(trees) => format!("Delete {} worktrees", trees.len()),
            _ => label.to_owned(),
        };
        let bulk_done = bulk.as_ref().is_some_and(|b| b.1 && !b.0);
        actions = actions
            .child(
                crate::modal::disabled(
                    crate::modal::ghost_button(
                        "cancel-management",
                        if bulk_done { "Close" } else { "Cancel" },
                    ),
                    busy,
                )
                .on_click(cx.listener(|this, _, window, cx| this.close_management(window, cx))),
            )
            .when(!bulk_done, |actions| {
                actions.child(
                    crate::modal::disabled(
                        if matches!(
                            target,
                            Target::DeleteTree(_) | Target::DeleteTrees(_) | Target::ResetMono(_)
                        ) {
                            crate::modal::danger_button("submit-management", label)
                        } else {
                            crate::modal::primary_button("submit-management", label)
                        },
                        busy,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.submit_management(cx))),
                )
            });
        rows.push(actions.into_any_element());
        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(rows)
            .into_any_element()
    }
    pub(super) fn management_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let target = self.management.as_ref()?.target.clone();
        if matches!(target, Target::Skill) {
            return None;
        }
        let title = self.management_title();
        let form = self.management_form(cx);
        if let Target::Cli(kind) = target {
            let slot = self
                .binary_anchors
                .borrow()
                .get(&kind)
                .cloned()
                .unwrap_or_default();
            return Some(
                popover("cli-details", Anchor::Slot(slot))
                    .side(Side::Bottom)
                    .align(Align::Start)
                    .width(440.)
                    .on_dismiss({
                        let weak = cx.weak_entity();
                        move |window, cx| {
                            let _ = weak.update(cx, |this, cx| this.close_management(window, cx));
                        }
                    })
                    .child(div().p_3().child(form))
                    .into_any_element(),
            );
        }
        let overlay = div()
            .absolute()
            .inset_0()
            .occlude()
            .child(crate::modal::scrim("settings-management-scrim"))
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .id("settings-management-dialog")
                    .debug_selector(|| "settings-management-dialog".into())
                    .role(Role::Dialog)
                    .aria_label(title.clone())
                    .w(px(if matches!(target, Target::Mcp) {
                        560.
                    } else {
                        448.
                    }))
                    .max_w_full()
                    .relative()
                    .rounded(px(crate::modal::RADIUS))
                    .border_1()
                    .border_color(content(crate::modal::BORDER))
                    .child(crate::modal::surface(crate::modal::RADIUS - 1.))
                    .child(
                        div()
                            .p_4()
                            .border_b_1()
                            .border_color(content(0.1))
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(title)
                            .child(
                                secondary_button(
                                    "close-management",
                                    "Close dialog",
                                    false,
                                    self.management.as_ref().is_some_and(|e| e.busy),
                                )
                                .border_0()
                                .size_6()
                                .p_0()
                                .justify_center()
                                .child(icon("x", 14.))
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.close_management(window, cx),
                                )),
                            ),
                    )
                    .child(div().p_4().child(form)),
            );
        Some(crate::modal::window_overlay(window, overlay).into_any_element())
    }
}
fn scopes(provider: &str) -> &'static [&'static str] {
    match provider {
        "claude" => &["local", "project", "user"],
        "codex" | "claude_desktop" => &["user"],
        _ => &["project", "user"],
    }
}
fn form_field(label: &str, input: Entity<Input>, height: f32) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(content(0.65))
        .child(label.to_owned())
        .child(
            div()
                .h(px(height))
                .px(px(10.))
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.04))
                .when(label == "JSON configuration", |d| {
                    d.font_family(crate::fonts::mono())
                })
                .child(input),
        )
        .into_any_element()
}

fn labeled_control(label: &str, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .text_size(px(12.))
        .text_color(content(0.65))
        .child(label.to_owned())
        .child(control)
        .into_any_element()
}
