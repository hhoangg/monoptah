//! The provider's session surface: which paths open a terminal session and which
//! never do. Fixtures shared with the tests of the paths that must stay chats
//! live here (`providerSurface.ts`, `newSession` / `sessionSurface.test.ts`).
use super::*;
use gpui::{TestAppContext, WindowHandle};

const PROJECT: &str = "/work/monocode";

/// Points every provider's CLI at a file that is not there, so a test that
/// submits a turn cannot run a real CLI (`MONOCODE_<ID>_BINARY` wins discovery).
pub(crate) struct NoClis(Vec<(String, Option<std::ffi::OsString>)>);
impl NoClis {
    pub(crate) fn new() -> Self {
        let saved = Kind::ALL
            .into_iter()
            .map(|kind| {
                let key = format!("MONOCODE_{}_BINARY", kind.id().to_uppercase());
                let old = std::env::var_os(&key);
                // SAFETY: gpui tests run with one test thread (`--test-threads=1`).
                unsafe { std::env::set_var(&key, "/nonexistent/monoptah-test-cli") };
                (key, old)
            })
            .collect();
        Self(saved)
    }
}
impl Drop for NoClis {
    fn drop(&mut self) {
        for (key, old) in self.0.drain(..) {
            // SAFETY: as in `new`.
            unsafe {
                match old {
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
    }
}

/// Every provider opens new sessions as a terminal: a path that still makes a
/// chat is pinned, not defaulted.
pub(crate) fn all_terminal(shell: &mut Shell) {
    for kind in Kind::ALL {
        shell
            .workspace
            .preferences
            .provider_surfaces
            .insert(kind.id().into(), Surface::Tui);
    }
    assert_eq!(shell.provider_surface(Kind::Claude), Surface::Tui);
    assert_eq!(shell.provider_surface(Kind::Codex), Surface::Tui);
}

pub(crate) fn known(shell: &Shell) -> HashSet<i64> {
    shell.session_summaries.keys().copied().collect()
}

/// The sessions that appeared since `before`.
pub(crate) fn since(shell: &Shell, before: &HashSet<i64>) -> Vec<i64> {
    let mut ids: Vec<_> = known(shell).difference(before).copied().collect();
    ids.sort();
    ids
}

/// A conversation in the shell, a chat in the store, and no terminal anywhere.
pub(crate) fn assert_chat(shell: &Shell, id: i64) {
    assert!(shell.sessions.contains_key(&id), "{id} has a conversation");
    assert!(
        !shell.tui_sessions.contains_key(&id),
        "{id} has no terminal"
    );
    assert_eq!(shell.session_summaries[&id].surface, Surface::Chat);
    assert_eq!(
        locked(&shell.store).unwrap().session_surface(id).unwrap(),
        Surface::Chat
    );
}

fn args(shell: &Shell, id: i64, cx: &App) -> Vec<String> {
    shell.tui_sessions[&id]
        .read(cx)
        .terminal()
        .unwrap()
        .read(cx)
        .launched()
        .unwrap()
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn window(cx: &mut TestAppContext) -> (WindowHandle<Shell>, i64, i64) {
    crate::shell::tests::window(cx)
}

#[gpui::test]
fn a_new_session_opens_as_its_providers_preferred_surface(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let before = window
        .update(cx, |shell, _, cx| {
            let before = known(shell);
            shell
                .workspace
                .preferences
                .provider_surfaces
                .insert("claude".into(), Surface::Tui);
            shell.create(Kind::Claude, cx);
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, cx| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            let id = ids[0];
            assert_eq!(shell.session_summaries[&id].surface, Surface::Tui);
            assert_eq!(
                locked(&shell.store).unwrap().session_surface(id).unwrap(),
                Surface::Tui
            );
            assert!(shell.tui_sessions.contains_key(&id));
            assert!(!shell.sessions.contains_key(&id), "never a conversation");
            assert_eq!(shell.workspace.active, Some(id));
            // It launched the provider's own CLI, not a chat turn.
            assert!(args(shell, id, cx).contains(&"--session-id".to_string()));
        })
        .unwrap();
    // Another provider keeps its own preference: codex has none, so a chat.
    let before = window
        .update(cx, |shell, _, cx| {
            let before = known(shell);
            shell.create(Kind::Codex, cx);
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            assert_chat(shell, ids[0]);
        })
        .unwrap();
}

#[gpui::test]
fn a_new_session_in_a_folder_follows_the_default_providers_surface(cx: &mut TestAppContext) {
    let (window, existing, _) = window(cx);
    let before = window
        .update(cx, |shell, _, cx| {
            shell.workspace.preferences.default_provider = Some("claude".into());
            shell
                .workspace
                .preferences
                .provider_surfaces
                .insert("claude".into(), Surface::Tui);
            shell
                .groups
                .entry(PROJECT.into())
                .or_default()
                .create_folder(&[existing], "folder-1".into());
            let before = known(shell);
            shell.create_in_folder("folder-1".into(), cx);
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            assert_eq!(shell.session_summaries[&ids[0]].surface, Surface::Tui);
            assert!(
                shell.groups[PROJECT]
                    .folders
                    .iter()
                    .any(|folder| folder.session_ids.contains(&ids[0]))
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_split_follows_the_preference_of_the_focused_sessions_provider(cx: &mut TestAppContext) {
    let (window, _, claude) = window(cx);
    let before = window
        .update(cx, |shell, window, cx| {
            shell.open(claude, window, cx);
            assert_eq!(shell.workspace.active, Some(claude));
            shell
                .workspace
                .preferences
                .provider_surfaces
                .insert("claude".into(), Surface::Tui);
            let before = known(shell);
            shell.split_session(SplitDir::Right, cx);
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            let id = ids[0];
            assert_eq!(shell.session_summaries[&id].surface, Surface::Tui);
            assert_eq!(shell.session_summaries[&id].provider, "claude");
            assert!(shell.tui_sessions.contains_key(&id));
            assert_eq!(
                shell.workspace.tab_of(id),
                shell.workspace.tab_of(claude),
                "it opened beside the focused pane"
            );
        })
        .unwrap();
}

#[gpui::test]
fn the_new_session_command_skips_the_prompt_when_one_provider_is_installed(
    cx: &mut TestAppContext,
) {
    let (window, _, _) = window(cx);
    let before = window
        .update(cx, |shell, window, cx| {
            shell.workspace.preferences.default_provider = Some("claude".into());
            shell
                .workspace
                .preferences
                .provider_surfaces
                .insert("claude".into(), Surface::Tui);
            shell.installed_providers = Some(vec![Kind::Claude]);
            let before = known(shell);
            shell.request_new_session(window, cx);
            assert!(shell.provider_prompt.is_none());
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            assert_eq!(shell.session_summaries[&ids[0]].surface, Surface::Tui);
        })
        .unwrap();
}

#[gpui::test]
fn a_database_that_cannot_store_terminals_opens_chats_and_never_asks(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let before = window
        .update(cx, |shell, window, cx| {
            shell.workspace.preferences.default_provider = Some("claude".into());
            shell
                .workspace
                .preferences
                .provider_surfaces
                .insert("claude".into(), Surface::Tui);
            shell.installed_providers = Some(Kind::ALL.to_vec());
            assert!(shell.should_prompt_for_provider());
            assert!(!shell.settings_context(cx).no_terminal_sessions);
            shell.surface_supported = false;
            assert!(shell.settings_context(cx).no_terminal_sessions);
            assert_eq!(shell.provider_surface(Kind::Claude), Surface::Chat);
            assert!(!shell.should_prompt_for_provider());
            let before = known(shell);
            shell.request_new_session(window, cx);
            assert!(shell.provider_prompt.is_none());
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            assert_chat(shell, ids[0]);
        })
        .unwrap();
}

#[gpui::test]
fn the_support_probe_reads_the_database(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.surface_supported = false;
            shell.probe_surface_support(cx);
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| assert!(shell.surface_supported))
        .unwrap();
}

#[gpui::test]
fn the_apps_own_quick_compose_is_a_chat_whatever_the_preference(cx: &mut TestAppContext) {
    let (window, _, _) = window(cx);
    let before = window
        .update(cx, |shell, _, cx| {
            all_terminal(shell);
            let before = known(shell);
            shell.create_chat(Kind::Codex, cx);
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            assert_chat(shell, ids[0]);
        })
        .unwrap();
}

#[gpui::test]
fn a_handoff_or_second_opinion_is_a_chat_whatever_the_preference(cx: &mut TestAppContext) {
    use crate::response_actions::Transfer;
    use monocode_core::response::{Target, TransferKind};
    let _clis = NoClis::new();
    let (window, _, _) = window(cx);
    let before = window
        .update(cx, |shell, _, cx| {
            all_terminal(shell);
            let before = known(shell);
            shell.transfer_response(
                Transfer {
                    kind: TransferKind::Handoff,
                    target: Target {
                        provider: "claude".into(),
                        model: None,
                        effort: None,
                        settings: Default::default(),
                    },
                    from: "codex".into(),
                    turn: Vec::new(),
                    through: Vec::new(),
                    cwd: PROJECT.into(),
                    title: "Handoff".into(),
                    settings: Default::default(),
                },
                cx,
            );
            before
        })
        .unwrap();
    cx.run_until_parked();
    window
        .update(cx, |shell, _, _| {
            let ids = since(shell, &before);
            assert_eq!(ids.len(), 1);
            assert_chat(shell, ids[0]);
            assert_eq!(shell.session_summaries[&ids[0]].provider, "claude");
        })
        .unwrap();
}

/// Every `.rs` file under the crate's `src/`, as (path from `src/`, source).
fn crate_sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((name, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

/// Whether a file holds only test code, by name: `tests.rs`, `*_tests.rs`, or
/// anything under a `tests/` folder.
fn is_test_file(name: &str) -> bool {
    let file = name.rsplit('/').next().unwrap_or(name);
    file == "tests.rs" || file.ends_with("_tests.rs") || name.contains("/tests/")
}

#[test]
fn only_the_user_facing_paths_may_create_a_terminal_session() {
    // The terminal is opened by `create_with_surface` alone (`Store::create` is a
    // chat). Every non-test site that calls it is listed with the reason it may;
    // a new one anywhere in the crate fails here until it is added on purpose,
    // after deciding the app's own paths stay chats.
    let allowed: [(&str, usize); 3] = [
        // create_session_target (New session, tab, folder row) and split_session.
        ("shell.rs", 2),
        // Inbox "Start work", kept a terminal only when the CLI takes a first prompt.
        ("shell/app_views.rs", 1),
        // An automation run follows the provider's surface when the CLI takes a first prompt.
        ("shell/automation_runs.rs", 1),
    ];
    let sources = crate_sources();
    assert!(sources.len() > 50, "the scan found the crate's sources");
    let production = |source: &str| {
        source
            .split("\nmod tests {")
            .next()
            .unwrap_or_default()
            .matches("create_with_surface(")
            .count()
    };
    for (name, source) in &sources {
        if is_test_file(name) {
            continue;
        }
        let expected = allowed
            .iter()
            .find(|(allowed, _)| allowed == name)
            .map_or(0, |(_, count)| *count);
        assert_eq!(
            production(source),
            expected,
            "{name}: sessions the app drives itself (mobile, app control, accounts, orchestration, quick composer, Monos) stay chats"
        );
    }
    for (name, _) in allowed {
        assert!(
            sources.iter().any(|(file, _)| file == name),
            "{name} is listed but missing"
        );
    }
}
