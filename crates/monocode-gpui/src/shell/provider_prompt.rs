//! NewSessionProviderDialog.tsx and newSessionProviderPrompt.ts: a terminal
//! session gives no way to switch provider afterwards, so when the default
//! provider opens as one, a new session asks which provider to use first.
use super::*;
use monocode_core::surface::{Surface, new_session_surface};

const TITLE: &str = "Choose a provider";
const DESCRIPTION: &str = "For this session only";
const HINT: &str =
    "The default provider opens as a terminal session, which can't switch providers afterwards.";

/// The providers the model picker would show for this project: not hidden for it,
/// and installed once that is known (`None` while the CLIs are not probed yet, so
/// the list does not shrink and jump when they are found).
pub(super) fn new_session_providers(hidden: &[String], installed: Option<&[Kind]>) -> Vec<Kind> {
    Kind::ALL
        .into_iter()
        .filter(|kind| {
            !hidden.iter().any(|id| id == kind.id())
                && installed.is_none_or(|installed| installed.contains(kind))
        })
        .collect()
}

/// A new session should ask which provider to use when the default one opens as
/// a terminal. With fewer than two providers (`available`, as listed by
/// [`new_session_providers`]) there is nothing to choose, and a project on
/// another machine opens a chat whatever the preference says.
pub(super) fn should_prompt_for_provider(
    cwd: &str,
    default_kind: Kind,
    surfaces: &HashMap<String, Surface>,
    available: &[Kind],
) -> bool {
    let preferred = surfaces.get(default_kind.id()).copied().unwrap_or_default();
    new_session_surface(default_kind, cwd, preferred).is_tui() && available.len() >= 2
}

/// Move the highlight one row, wrapping like the model picker menus.
pub(super) fn move_highlight(index: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    (index as i64 + delta as i64).rem_euclid(len as i64) as usize
}

pub(super) struct Prompt {
    cwd: String,
    /// The folder the new session joins, for a request made from a folder row.
    folder: Option<String>,
    highlighted: usize,
    focus: FocusHandle,
    restore: Option<FocusHandle>,
}

impl Shell {
    /// How a new session of `kind` opens: the Settings preference, as a chat when
    /// the database cannot store a terminal session.
    pub(super) fn provider_surface(&self, kind: Kind) -> Surface {
        if !self.surface_supported {
            return Surface::Chat;
        }
        self.workspace
            .preferences
            .provider_surfaces
            .get(kind.id())
            .copied()
            .unwrap_or_default()
    }

    fn prompt_providers(&self) -> Vec<Kind> {
        let hidden = self
            .workspace
            .preferences
            .hidden_for(&self.workspace.project);
        new_session_providers(&hidden, self.installed_providers.as_deref())
    }

    pub(super) fn should_prompt_for_provider(&self) -> bool {
        let none = HashMap::new();
        let surfaces = if self.surface_supported {
            &self.workspace.preferences.provider_surfaces
        } else {
            &none
        };
        should_prompt_for_provider(
            &self.workspace.project,
            self.default_kind(),
            surfaces,
            &self.prompt_providers(),
        )
    }

    /// Cmd+T and the New buttons: ask which provider first when the default one
    /// opens as a terminal, else open the default straight away.
    pub(super) fn request_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_prompt_for_provider() {
            self.open_provider_prompt(None, window, cx);
        } else {
            self.create(self.default_kind(), cx);
        }
    }

    /// The same from a folder's New session row; the session joins the folder.
    pub(super) fn request_new_session_in_folder(
        &mut self,
        folder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.should_prompt_for_provider() {
            self.open_provider_prompt(Some(folder), window, cx);
        } else {
            self.create_in_folder(folder, cx);
        }
    }

    /// A second request must not drop the folder the first one carried.
    pub(super) fn open_provider_prompt(
        &mut self,
        folder: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_stale_provider_prompt(window, cx);
        if self.provider_prompt.is_some() {
            return;
        }
        self.close_menu(cx);
        self.close_preview(cx);
        // The CLIs may have been installed since the last look.
        self.probe_installed_providers(cx);
        let providers = self.prompt_providers();
        let focus = cx.focus_handle();
        let restore = window.focused(cx);
        window.focus(&focus, cx);
        self.provider_prompt = Some(Prompt {
            cwd: self.workspace.project.clone(),
            folder,
            highlighted: providers
                .iter()
                .position(|kind| *kind == self.default_kind())
                .unwrap_or(0),
            focus,
            restore,
        });
        cx.notify();
    }

    /// The rows on show, each with the surface a session of it opens in.
    pub(super) fn provider_prompt_rows(&self) -> Vec<(Kind, Surface)> {
        self.prompt_providers()
            .into_iter()
            .map(|kind| (kind, self.provider_surface(kind)))
            .collect()
    }

    pub(super) fn close_provider_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prompt) = self.provider_prompt.take() {
            if let Some(restore) = prompt.restore {
                window.focus(&restore, cx);
            } else {
                self.focus_active(window, cx);
            }
        }
        cx.notify();
    }

    fn prompt_is_stale(&self) -> bool {
        self.provider_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.cwd != self.workspace.project)
    }

    /// The prompt belongs to the project it was opened in: once the project
    /// changes it closes the way Escape closes it, giving the keyboard back.
    pub(super) fn drop_stale_provider_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.prompt_is_stale() {
            self.close_provider_prompt(window, cx);
        }
    }

    /// The same for a caller with no window (`persist`): no focus to give back.
    pub(super) fn forget_stale_provider_prompt(&mut self) {
        if self.prompt_is_stale() {
            self.provider_prompt = None;
        }
    }

    fn pick_provider(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.provider_prompt.as_ref().map(|p| p.folder.clone()) else {
            return;
        };
        self.close_provider_prompt(window, cx);
        self.create_session_target(kind, false, folder, self.provider_surface(kind), cx);
    }

    fn provider_prompt_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.provider_prompt_rows();
        let Some(prompt) = self.provider_prompt.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            key @ ("down" | "up") => {
                let delta = if key == "down" { 1 } else { -1 };
                prompt.highlighted = move_highlight(prompt.highlighted, rows.len(), delta);
            }
            "enter" => {
                if let Some(&(kind, _)) = rows.get(prompt.highlighted) {
                    self.pick_provider(kind, window, cx);
                }
            }
            "escape" => self.close_provider_prompt(window, cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Where the CLIs are looked for is the picker's: an installed CLI, or one
    /// still unknown. Resolving scans the disk, so it happens in the background.
    pub(super) fn probe_installed_providers(&mut self, cx: &mut Context<Self>) {
        if cfg!(test) {
            return;
        }
        let work = cx.background_executor().spawn(async {
            Kind::ALL
                .into_iter()
                .filter(|kind| monocode_core::provider::resolve(*kind).is_some())
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let installed = work.await;
            let _ = this.update(cx, |this, cx| {
                this.installed_providers = Some(installed);
                cx.notify();
            });
        })
        .detach();
    }

    /// Whether the database can store a terminal session, asked once in the
    /// background; until it answers, creation falls back to a chat by itself.
    pub(super) fn probe_surface_support(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let work = cx
            .background_executor()
            .spawn(async move { locked(&store).map(|store| store.surface_supported()) });
        cx.spawn(async move |this, cx| {
            let supported = work.await.unwrap_or(true);
            let _ = this.update(cx, |this, cx| {
                if this.surface_supported != supported {
                    this.surface_supported = supported;
                    this.sync_settings(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn provider_prompt_modal(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let prompt = self
            .provider_prompt
            .as_ref()
            .filter(|prompt| prompt.cwd == self.workspace.project)?;
        let rows = self.provider_prompt_rows();
        let highlighted = prompt.highlighted.min(rows.len().saturating_sub(1));
        let viewport = window.viewport_size();
        let width = 360f32.min(viewport.width.as_f32() - 24.);
        let anchor = Anchor::Point(gpui::point(
            px((viewport.width.as_f32() - width) / 2.),
            px((viewport.height.as_f32() * 0.16).max(16.)),
        ));
        let weak = cx.weak_entity();
        let list = div()
            .id("provider-prompt-list")
            .role(Role::ListBox)
            .aria_label("Provider")
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p_2()
            .text_size(px(12.))
            .child(
                div()
                    .px(px(10.))
                    .pb_1()
                    .text_size(px(11.))
                    .text_color(content(0.55))
                    .child(HINT),
            )
            .children(rows.iter().enumerate().map(|(ix, &(kind, surface))| {
                let badge = if surface.is_tui() { "TUI" } else { "Chat" };
                let id = kind.id();
                div()
                    .id(("provider-prompt-row", ix))
                    .debug_selector(move || format!("provider-prompt-row-{id}"))
                    .role(Role::ListBoxOption)
                    .aria_selected(ix == highlighted)
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .px(px(10.))
                    .py_2()
                    .cursor_pointer()
                    .when(ix == highlighted, |d| d.bg(content(0.1)))
                    .hover(|s| s.bg(content(0.08)))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && let Some(prompt) = this.provider_prompt.as_mut() {
                            prompt.highlighted = ix;
                            cx.notify();
                        }
                    }))
                    .child(provider_icon(id, 16.))
                    .child(div().min_w_0().flex_1().truncate().child(kind.label()))
                    .child(
                        div()
                            .debug_selector(move || format!("provider-prompt-badge-{id}-{badge}"))
                            .rounded(px(4.))
                            .bg(content(0.1))
                            .px(px(6.))
                            .py(px(2.))
                            .text_size(px(10.))
                            .text_color(content(0.6))
                            .child(badge),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.pick_provider(kind, window, cx)
                        }),
                    )
            }));
        let card = div()
            .id("provider-prompt")
            .debug_selector(|| "provider-prompt".into())
            .role(Role::Dialog)
            .aria_label(TITLE)
            .track_focus(&prompt.focus)
            .on_key_down(
                cx.listener(|this, event, window, cx| this.provider_prompt_key(event, window, cx)),
            )
            .flex()
            .flex_col()
            .min_h_0()
            .text_color(content(1.))
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(TITLE),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.55))
                            .child(DESCRIPTION),
                    ),
            )
            .child(list);
        Some(
            div()
                .absolute()
                .size_full()
                .child(crate::modal::scrim("provider-prompt-scrim"))
                .child(
                    popover("provider-prompt-modal", anchor)
                        .modal_surface(self.workspace.boxy_layout())
                        .width(width)
                        .max_height(viewport.height.as_f32() * 0.7)
                        .gap(0.)
                        .padding(0.)
                        .priority(80)
                        .on_dismiss(move |window, cx| {
                            let _ =
                                weak.update(cx, |this, cx| this.close_provider_prompt(window, cx));
                        })
                        .child(card),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};
    use monocode_core::surface::Surface;

    const PROJECT: &str = "/work/monocode";

    fn surfaces(entries: &[(Kind, Surface)]) -> HashMap<String, Surface> {
        entries
            .iter()
            .map(|(kind, surface)| (kind.id().to_owned(), *surface))
            .collect()
    }

    // newSessionProviderPrompt.test.ts: shouldPromptForProvider
    #[test]
    fn it_does_not_prompt_when_the_default_provider_opens_as_a_chat() {
        let all = new_session_providers(&[], None);
        let chat = surfaces(&[(Kind::Claude, Surface::Chat)]);
        assert!(!should_prompt_for_provider(
            PROJECT,
            Kind::Claude,
            &chat,
            &all
        ));
        assert!(!should_prompt_for_provider(
            PROJECT,
            Kind::Claude,
            &HashMap::new(),
            &all
        ));
    }

    #[test]
    fn it_prompts_when_the_default_provider_opens_as_a_terminal() {
        let all = new_session_providers(&[], None);
        let tui = surfaces(&[(Kind::Claude, Surface::Tui)]);
        assert!(should_prompt_for_provider(
            PROJECT,
            Kind::Claude,
            &tui,
            &all
        ));
    }

    #[test]
    fn it_only_looks_at_the_default_providers_surface() {
        let all = new_session_providers(&[], None);
        let tui = surfaces(&[(Kind::Codex, Surface::Tui)]);
        assert!(!should_prompt_for_provider(
            PROJECT,
            Kind::Claude,
            &tui,
            &all
        ));
    }

    #[test]
    fn it_does_not_prompt_in_a_remote_project_where_a_terminal_becomes_a_chat() {
        let all = new_session_providers(&[], None);
        let tui = surfaces(&[(Kind::Claude, Surface::Tui)]);
        assert!(!should_prompt_for_provider(
            "remote://env/repo",
            Kind::Claude,
            &tui,
            &all
        ));
    }

    // shouldPromptForProvider with few providers
    fn tui() -> HashMap<String, Surface> {
        surfaces(&[(Kind::Claude, Surface::Tui)])
    }

    #[test]
    fn it_does_not_prompt_when_no_provider_is_installed() {
        let none = new_session_providers(&[], Some(&[]));
        assert!(!should_prompt_for_provider(
            PROJECT,
            Kind::Claude,
            &tui(),
            &none
        ));
    }

    #[test]
    fn it_does_not_prompt_when_only_one_provider_is_installed() {
        let one = new_session_providers(&[], Some(&[Kind::Claude]));
        assert!(!should_prompt_for_provider(
            PROJECT,
            Kind::Claude,
            &tui(),
            &one
        ));
    }

    #[test]
    fn it_prompts_when_two_providers_are_installed() {
        let two = new_session_providers(&[], Some(&[Kind::Claude, Kind::Codex]));
        assert!(should_prompt_for_provider(
            PROJECT,
            Kind::Claude,
            &tui(),
            &two
        ));
    }

    // newSessionProviders
    #[test]
    fn every_provider_is_listed_before_availability_is_probed() {
        assert_eq!(new_session_providers(&[], None), Kind::ALL.to_vec());
    }

    #[test]
    fn only_installed_providers_are_kept_once_probed() {
        assert_eq!(
            new_session_providers(&[], Some(&[Kind::Codex])),
            vec![Kind::Codex]
        );
    }

    #[test]
    fn a_provider_hidden_for_the_project_is_excluded() {
        let hidden = vec!["codex".to_owned()];
        assert!(!new_session_providers(&hidden, None).contains(&Kind::Codex));
        assert!(!new_session_providers(&hidden, Some(&Kind::ALL)).contains(&Kind::Codex));
        assert!(new_session_providers(&hidden, None).contains(&Kind::Claude));
    }

    // moveHighlight
    #[test]
    fn the_highlight_moves_down_and_up() {
        assert_eq!(move_highlight(0, 3, 1), 1);
        assert_eq!(move_highlight(2, 3, -1), 1);
    }

    #[test]
    fn the_highlight_wraps_at_both_ends() {
        assert_eq!(move_highlight(2, 3, 1), 0);
        assert_eq!(move_highlight(0, 3, -1), 2);
    }

    #[test]
    fn the_highlight_stays_at_zero_for_an_empty_list() {
        assert_eq!(move_highlight(0, 0, 1), 0);
        assert_eq!(move_highlight(0, 0, -1), 0);
    }

    // NewSessionProviderDialog.test.ts
    fn window(cx: &mut TestAppContext, tui: &[Kind]) -> WindowHandle<Shell> {
        let (window, _, _) = crate::shell::tests::window(cx);
        window
            .update(cx, |shell, _, _| {
                let prefs = &mut shell.workspace.preferences;
                prefs.default_provider = Some("claude".into());
                for kind in tui {
                    prefs
                        .provider_surfaces
                        .insert(kind.id().into(), Surface::Tui);
                }
                shell.installed_providers = Some(Kind::ALL.to_vec());
            })
            .unwrap();
        window
    }

    fn open(window: WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, window, cx| {
                shell.request_new_session(window, cx)
            })
            .unwrap();
        cx.run_until_parked();
    }

    fn rows(window: WindowHandle<Shell>, cx: &mut TestAppContext) -> Vec<(Kind, Surface)> {
        window
            .update(cx, |shell, _, _| shell.provider_prompt_rows())
            .unwrap()
    }

    #[gpui::test]
    fn the_dialog_excludes_a_provider_hidden_for_the_project(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        window
            .update(cx, |shell, _, _| {
                shell
                    .workspace
                    .preferences
                    .project_providers
                    .entry(PROJECT.into())
                    .or_default()
                    .hidden
                    .push("codex".into());
            })
            .unwrap();
        open(window, cx);
        let kinds: Vec<_> = rows(window, cx).into_iter().map(|(kind, _)| kind).collect();
        assert!(!kinds.contains(&Kind::Codex));
        assert!(kinds.contains(&Kind::Claude));
    }

    #[gpui::test]
    fn the_dialog_badges_each_provider_with_its_surface(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        window
            .update(cx, |shell, _, _| {
                shell
                    .workspace
                    .preferences
                    .provider_surfaces
                    .insert("codex".into(), Surface::Chat);
            })
            .unwrap();
        open(window, cx);
        let rows = rows(window, cx);
        assert!(rows.contains(&(Kind::Claude, Surface::Tui)));
        assert!(rows.contains(&(Kind::Codex, Surface::Chat)));
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(
            visual
                .debug_bounds("provider-prompt-badge-claude-TUI")
                .is_some()
        );
        assert!(
            visual
                .debug_bounds("provider-prompt-badge-codex-Chat")
                .is_some()
        );
    }

    #[gpui::test]
    fn the_dialog_copy_is_the_web_apps(cx: &mut TestAppContext) {
        assert_eq!(TITLE, "Choose a provider");
        assert_eq!(DESCRIPTION, "For this session only");
        assert_eq!(
            HINT,
            "The default provider opens as a terminal session, which can't switch providers afterwards."
        );
        let window = window(cx, &[Kind::Claude]);
        open(window, cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("provider-prompt").is_some());
    }

    #[gpui::test]
    fn arrow_down_then_enter_picks_the_second_provider(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        open(window, cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        // The default provider is highlighted first.
        window
            .update(visual, |shell, _, _| {
                let prompt = shell.provider_prompt.as_ref().unwrap();
                assert_eq!(
                    shell.provider_prompt_rows()[prompt.highlighted].0,
                    Kind::Claude
                );
                assert_eq!(prompt.highlighted, 0);
            })
            .unwrap();
        visual.simulate_keystrokes("down enter");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.provider_prompt.is_none());
                let id = shell.workspace.active.unwrap();
                assert_eq!(shell.session_summaries[&id].provider, "codex");
                // Codex is a chat; a different row would be a terminal.
                assert_eq!(shell.session_summaries[&id].surface, Surface::Chat);
            })
            .unwrap();
    }

    #[gpui::test]
    fn up_from_the_first_row_wraps_to_the_last_provider(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        open(window, cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("up enter");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                let id = shell.workspace.active.unwrap();
                // Up from the first row wraps to the last: the last provider, a chat.
                assert_eq!(
                    shell.session_summaries[&id].provider,
                    Kind::ALL.last().unwrap().id()
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn clicking_a_row_picks_that_provider_with_its_surface(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude, Kind::Codex]);
        open(window, cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let row = visual.debug_bounds("provider-prompt-row-codex").unwrap();
        visual.simulate_click(row.center(), Default::default());
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.provider_prompt.is_none());
                let id = shell.workspace.active.unwrap();
                assert_eq!(shell.session_summaries[&id].provider, "codex");
                assert_eq!(shell.session_summaries[&id].surface, Surface::Tui);
            })
            .unwrap();
    }

    #[gpui::test]
    fn escape_closes_the_dialog_without_a_session(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        let before = window
            .update(cx, |shell, _, _| shell.session_summaries.len())
            .unwrap();
        open(window, cx);
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        window
            .update(visual, |shell, _, _| {
                assert!(shell.provider_prompt.is_none());
                assert_eq!(shell.session_summaries.len(), before);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_dialog_is_dropped_when_the_project_changes(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        open(window, cx);
        window
            .update(cx, |shell, window, cx| {
                assert!(shell.provider_prompt.is_some());
                shell.select_project("/work/another-project".into(), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| assert!(shell.provider_prompt.is_none()))
            .unwrap();
    }

    #[gpui::test]
    fn changing_project_closes_the_dialog_and_gives_focus_back(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        open(window, cx);
        window
            .update(cx, |shell, window, cx| {
                let focus = shell.provider_prompt.as_ref().unwrap().focus.clone();
                assert!(focus.is_focused(window), "the dialog holds the keyboard");
                shell.select_project("/work/another-project".into(), window, cx);
                // Dropped by the close path, not left holding the keyboard.
                assert!(shell.provider_prompt.is_none());
                assert!(!focus.is_focused(window));
            })
            .unwrap();
    }

    #[gpui::test]
    fn opening_a_session_of_another_project_closes_the_dialog_and_gives_focus_back(
        cx: &mut TestAppContext,
    ) {
        let window = window(cx, &[Kind::Claude]);
        let other = window
            .update(cx, |shell, _, _| {
                *shell
                    .session_summaries
                    .iter()
                    .find(|(_, summary)| summary.cwd != PROJECT)
                    .map(|(id, _)| id)
                    .unwrap()
            })
            .unwrap();
        open(window, cx);
        window
            .update(cx, |shell, window, cx| {
                let focus = shell.provider_prompt.as_ref().unwrap().focus.clone();
                shell.open(other, window, cx);
                assert!(shell.provider_prompt.is_none());
                assert!(!focus.is_focused(window));
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_stale_dialog_does_not_block_opening_one_for_the_new_project(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        open(window, cx);
        window
            .update(cx, |shell, window, cx| {
                // The project changed by a path that has not persisted yet.
                shell.workspace.project = "/work/another-project".into();
                shell.open_provider_prompt(None, window, cx);
                let prompt = shell.provider_prompt.as_ref().expect("a new dialog");
                assert_eq!(prompt.cwd, "/work/another-project");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_default_that_opens_as_a_chat_creates_the_session_without_asking(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Codex]);
        let before = window
            .update(cx, |shell, _, _| shell.session_summaries.len())
            .unwrap();
        open(window, cx);
        window
            .update(cx, |shell, _, _| {
                assert!(shell.provider_prompt.is_none());
                assert_eq!(shell.session_summaries.len(), before + 1);
                let id = shell.workspace.active.unwrap();
                assert_eq!(shell.session_summaries[&id].provider, "claude");
                assert_eq!(shell.session_summaries[&id].surface, Surface::Chat);
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_second_request_keeps_the_first_dialog(cx: &mut TestAppContext) {
        let window = window(cx, &[Kind::Claude]);
        window
            .update(cx, |shell, window, cx| {
                shell.open_provider_prompt(Some("folder-a".into()), window, cx);
                shell.open_provider_prompt(Some("folder-b".into()), window, cx);
                assert_eq!(
                    shell.provider_prompt.as_ref().unwrap().folder.as_deref(),
                    Some("folder-a")
                );
            })
            .unwrap();
    }
}
