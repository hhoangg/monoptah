//! A Mono's Browser settings page: switch, prerequisites, Chrome profile, extension, token,
//! connection test, safety rules and the audit log. Design:
//! `docs/superpowers/specs/2026-11-21-mono-browser-design.md`.
//!
//! What a Mono keeps is `Mono::browser` in the roster (no secret). The extension token is
//! sealed in `mono_secrets` (`MonoBrowserStore`); the page only ever shows its last four
//! characters. Anything that would touch Chrome, System Settings or the MCP server goes
//! through [`BrowserOps`], which tests replace.
use super::panels::{card, heading, setting, switch_setting, switch_track};
use super::*;
use crate::{
    cloud_dialog_ui::alert,
    popover::{Choice, ChoiceEvent, Choices, Popover},
};
use gpui::{Div, Stateful};
use monocode_core::mono::browser::recipe::manage::DryRunReport;
use monocode_core::mono::browser::{
    self, AuditEntry, BrowserTrust, Check, ChromeProfile, MonoBrowser, MonoBrowserStore, PrereqEnv,
    Prerequisites,
};
use std::sync::Arc;

/// Stops the Mono's browser server (off the UI thread: ending a process can wait a moment).
fn stop_server(id: &str) {
    let id = id.to_owned();
    std::thread::spawn(move || browser::server::servers().stop(&id));
}

/// The selector id of a trust level's row (tests click it).
pub(super) fn trust_selector(trust: BrowserTrust) -> &'static str {
    match trust {
        BrowserTrust::ReadOnly => "mono-browser-trust-read-only",
        BrowserTrust::AskRisky => "mono-browser-trust-ask-risky",
        BrowserTrust::Autonomous => "mono-browser-trust-autonomous",
    }
}
const TRUST_SUBTITLE: &str = "How far this Mono may act in your Chrome. Change it any time; it applies from the next message.";

/// One level of the trust selector: a radio dot, the level's name and what it means.
fn trust_row(trust: BrowserTrust, selected: bool, cx: &mut Context<Monos>) -> AnyElement {
    let selector = trust_selector(trust);
    div()
        .id(selector)
        .debug_selector(move || selector.into())
        .role(Role::RadioButton)
        .aria_label(trust.label())
        .aria_selected(selected)
        .flex()
        .items_start()
        .gap_3()
        .px_3()
        .py_2()
        .cursor_pointer()
        .hover(|s| s.bg(content(0.04)))
        .on_click(cx.listener(move |this, _, _, cx| this.browser_set_trust(trust, cx)))
        .child(
            div()
                .mt(px(3.))
                .size(px(14.))
                .flex_shrink_0()
                .rounded_full()
                .border_1()
                .border_color(if selected {
                    accent_color()
                } else {
                    content(0.3)
                })
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |d| {
                    d.child(div().size(px(8.)).rounded_full().bg(accent_color()))
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(content(0.9))
                        .child(trust.label()),
                )
                .child(note(format!("{selector}-text"), trust.explanation(), 0.5)),
        )
        .into_any_element()
}

/// The Full Disk Access pane of System Settings. The scheme has a dot: `x-apple.systempreferences`.
#[cfg(any(target_os = "macos", test))]
pub(super) const FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
const SELECT_PRIORITY: usize = 90;
const AUDIT_ROWS: usize = 20;
pub(super) const SUBTITLE: &str = "Let this Mono use your own Chrome, with the sessions you are signed in to, through Playwright MCP. Off by default.";

/// What touches the outside world. The real one opens Chrome and starts the MCP server.
pub(super) trait BrowserOps: Send + Sync {
    fn open_in_profile(&self, env: &PrereqEnv, profile_dir: &str, url: &str) -> Result<(), String>;
    fn open_full_disk_access(&self);
    fn probe(&self, runtime: &browser::Runtime, env: &PrereqEnv) -> Result<(), String>;
    /// A recipe's dry run through the Mono's running gate (see `Gate::dry_run_recipe`). It
    /// blocks until a run in progress ends, so it is called off the UI thread.
    fn dry_run_recipe(&self, mono_id: &str, recipe_id: &str) -> Result<DryRunReport, String>;
}
struct RealOps;
impl BrowserOps for RealOps {
    fn open_in_profile(&self, env: &PrereqEnv, profile_dir: &str, url: &str) -> Result<(), String> {
        browser::open_in_profile(env, profile_dir, url)
    }
    fn open_full_disk_access(&self) {
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open")
            .arg(FULL_DISK_ACCESS_URL)
            .spawn();
    }
    fn probe(&self, runtime: &browser::Runtime, env: &PrereqEnv) -> Result<(), String> {
        let mut transport = browser::ProcessTransport::spawn(runtime, env)?;
        browser::probe(&mut transport)
    }
    fn dry_run_recipe(&self, mono_id: &str, recipe_id: &str) -> Result<DryRunReport, String> {
        browser::server::servers()
            .gate(mono_id)
            .ok_or_else(|| {
                "This Mono's browser is not running yet. Send the Mono a message that uses the browser, then try the dry run again.".to_owned()
            })?
            .dry_run_recipe(recipe_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TestState {
    Idle,
    Running,
    Passed,
    Failed(String),
}

/// The page's own state; the setting itself is on the Mono.
pub(super) struct BrowserUi {
    pub(super) ops: Arc<dyn BrowserOps>,
    pub(super) env: Option<PrereqEnv>,
    pub(super) prereq: Option<Prerequisites>,
    pub(super) profiles: Vec<ChromeProfile>,
    /// Chrome's profile list could not be read: `profiles` holds only the saved profile.
    pub(super) profiles_unreadable: bool,
    pub(super) installed: bool,
    pub(super) hint: Option<String>,
    /// The Mono `hint` was read for. The 2 second install poll must not decrypt the token again.
    pub(super) hint_mono: Option<String>,
    pub(super) replacing: bool,
    pub(super) test: TestState,
    pub(super) audit: Vec<AuditEntry>,
    pub(super) recipes: super::recipes_ui::RecipesUi,
    pub(super) select: Option<Choices<String>>,
    pub(super) profile_anchor: AnchorSlot,
    pub(super) watching: bool,
    pub(super) error: Option<String>,
}
impl BrowserUi {
    pub(super) fn new() -> Self {
        Self {
            ops: Arc::new(RealOps),
            // Tests start with Chrome and Node absent, never the real ones.
            env: cfg!(test).then(|| PrereqEnv {
                chrome_binaries: Vec::new(),
                user_data_dir: PathBuf::new(),
                search_path: String::new(),
            }),
            prereq: None,
            profiles: Vec::new(),
            profiles_unreadable: false,
            installed: false,
            hint: None,
            hint_mono: None,
            replacing: false,
            test: TestState::Idle,
            audit: Vec::new(),
            recipes: Default::default(),
            select: None,
            profile_anchor: AnchorSlot::default(),
            watching: false,
            error: None,
        }
    }
    fn env(&mut self) -> PrereqEnv {
        self.env.get_or_insert_with(PrereqEnv::detect).clone()
    }
    #[cfg(test)]
    pub(super) fn use_fakes(&mut self, ops: Arc<dyn BrowserOps>, env: PrereqEnv) {
        self.ops = ops;
        self.env = Some(env);
    }
    #[cfg(test)]
    pub(super) fn test_state(&self) -> TestState {
        self.test.clone()
    }
    #[cfg(test)]
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    #[cfg(test)]
    pub(super) fn select_is_open(&self) -> bool {
        self.select.is_some()
    }
}

pub(super) fn action(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    crate::modal::ghost_button(id, label)
        .debug_selector(move || id.into())
        .border_1()
        .border_color(content(0.1))
        .cursor_pointer()
}
pub(super) fn note(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    alpha: f32,
) -> Stateful<Div> {
    let id = id.into();
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(content(alpha))
        .child(text.into())
}
pub(super) fn ago(ts_ms: u64) -> String {
    let seconds = monocode_core::time::now_ms().saturating_sub(ts_ms) / 1000;
    match seconds {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

impl Monos {
    pub(super) fn browser_store(&self) -> Result<MonoBrowserStore, String> {
        let store = locked(&self.store)?;
        MonoBrowserStore::for_store(&store)
    }

    /// Reads what the page shows from disk: prerequisites, profiles, the extension, the token
    /// hint and the audit log. Then keeps the Mono's `ready` flag true to it.
    pub(super) fn browser_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let env = self.browser_ui.env();
        let (profile_dir, profile_name) = self
            .roster
            .get(&id)
            .and_then(|m| m.browser.as_ref())
            .map(|b| (b.profile_dir.clone(), b.profile_name.clone()))
            .unwrap_or_default();
        let ui = &mut self.browser_ui;
        ui.prereq = Some(browser::check_prerequisites(&env));
        let listed = browser::read_local_state(&env)
            .ok()
            .and_then(|text| browser::parse_local_state(&text).ok());
        ui.profiles_unreadable = listed.is_none();
        ui.profiles = listed.unwrap_or_default();
        // The saved profile is always offered, even when Chrome's profile list cannot be read
        // (Full Disk Access missing), so the picker still opens and shows the current choice.
        if browser::valid_profile_dir(&profile_dir)
            && !ui.profiles.iter().any(|p| p.dir == profile_dir)
        {
            ui.profiles.push(ChromeProfile {
                dir: profile_dir.clone(),
                name: if profile_name.is_empty() {
                    profile_dir.clone()
                } else {
                    profile_name
                },
                email: String::new(),
                avatar_icon: String::new(),
                picture: None,
            });
        }
        ui.installed = !profile_dir.is_empty()
            && browser::extension_installed(&env.user_data_dir, &profile_dir);
        let store = self.browser_store().ok();
        let ui = &mut self.browser_ui;
        if ui.hint_mono.as_deref() != Some(&id) {
            ui.hint = None;
        }
        ui.hint = match &store {
            Some(s) if s.has_token_row(&id) => ui.hint.take().or_else(|| s.token_hint(&id)),
            _ => None,
        };
        ui.hint_mono = Some(id.clone());
        // The note of a failed launch goes once its cause is fixed: a token is saved and Chrome
        // and node are there. A later launch sets it again if something else is wrong.
        if ui.hint.is_some() && ui.prereq.as_ref().is_some_and(Prerequisites::can_attach) {
            browser::server::servers().set_attach_error(&id, None);
        }
        ui.audit = store
            .as_ref()
            .map(|s| s.recent(&id, AUDIT_ROWS))
            .unwrap_or_default();
        self.recipes_refresh();
        self.browser_sync(cx);
    }

    /// `ready` is the page's own summary: Chrome, node, a profile and a token. A launch does not
    /// read it. Full Disk Access is not part of it: it only lets the page list other profiles.
    fn browser_sync(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let ready = self
            .browser_ui
            .prereq
            .as_ref()
            .is_some_and(Prerequisites::can_attach)
            && self.browser_ui.hint.is_some();
        let Some(setting) = self.roster.get_mut(&id).and_then(|m| m.browser.as_mut()) else {
            cx.notify();
            return;
        };
        let ready = ready && !setting.profile_dir.is_empty();
        if setting.ready != ready {
            setting.ready = ready;
            self.save(cx);
            self.configure_chat_with(&id, false, cx);
        }
        cx.notify();
    }

    pub(super) fn browser_edit(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut MonoBrowser),
    ) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let Some(mono) = self.roster.get_mut(&id) else {
            return;
        };
        let setting = mono.browser.get_or_insert_with(MonoBrowser::default);
        let before = setting.clone();
        change(setting);
        setting.normalize();
        // The server starts again with the new setting on the next run, with a new tab group.
        // A change that does not shape the server (the trust level) keeps it and its group; the
        // running gate takes the new level at once.
        if before.server_key() != setting.server_key() {
            stop_server(&id);
        }
        browser::server::servers().update_policy(&id, setting.policy());
        self.save(cx);
        self.configure_chat_with(&id, false, cx);
        self.browser_ui.error = None;
        cx.notify();
    }

    /// A level row was chosen.
    pub(super) fn browser_set_trust(&mut self, trust: BrowserTrust, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.trust = trust);
    }

    /// The "Show agent actions on the page" switch was flipped.
    pub(super) fn browser_toggle_show_actions(&mut self, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.show_actions = !b.show_actions);
    }

    fn browser_show_actions_card(
        &mut self,
        setting_: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle = switch_track(
            "mono-browser-show-actions",
            "Show agent actions on the page",
            setting_.show_actions,
        )
        .debug_selector(|| "mono-browser-show-actions".into())
        .cursor_pointer()
        .on_click(cx.listener(|this, _, _, cx| this.browser_toggle_show_actions(cx)));
        card()
            .child(switch_setting(
                "Show agent actions on the page",
                "Before a click or a typed value, a box and a cursor show where the Mono is about to act, for about half a second. Off: no overlay and no delay.",
                false,
                toggle.into_any_element(),
            ))
            .into_any_element()
    }

    fn browser_toggle(&mut self, cx: &mut Context<Self>) {
        self.browser_edit(cx, |b| b.enabled = !b.enabled);
        self.browser_refresh(cx);
    }

    pub(super) fn browser_setting(&self) -> MonoBrowser {
        self.current()
            .and_then(|m| m.browser.clone())
            .unwrap_or_default()
    }

    fn browser_open_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.browser_ui.select.take() {
            open.close(window, cx);
            cx.notify();
            return;
        }
        let current = self.browser_setting().profile_dir;
        let choices = self
            .browser_ui
            .profiles
            .iter()
            .map(|p| Choice {
                value: p.dir.clone(),
                label: p.label().into(),
                checked: p.dir == current,
            })
            .collect::<Vec<_>>();
        if choices.is_empty() {
            return;
        }
        let width = self
            .browser_ui
            .profile_anchor
            .get()
            .map_or(260., |b| b.size.width.as_f32().max(220.));
        self.browser_ui.select = Some(Choices::open(
            "Chrome profile",
            choices,
            width,
            window,
            cx,
            |this, event, window, cx| {
                if let Some(open) = this.browser_ui.select.take() {
                    open.close(window, cx);
                }
                if let ChoiceEvent::Picked(dir) = event {
                    this.browser_choose_profile(&dir, cx);
                }
                cx.notify();
            },
        ));
        cx.notify();
    }

    pub(super) fn browser_select_popover(&self, cx: &App) -> Option<Popover> {
        let open = self.browser_ui.select.as_ref()?;
        Some(open.popover(&self.browser_ui.profile_anchor, SELECT_PRIORITY, cx))
    }

    /// A token belongs to one profile's extension install, so a new profile drops it.
    pub(super) fn browser_choose_profile(&mut self, dir: &str, cx: &mut Context<Self>) {
        // A listed profile, else the saved one, else any valid directory name: a choice always
        // takes effect, also when Chrome's profile list cannot be read.
        let Some(profile) = self
            .browser_ui
            .profiles
            .iter()
            .find(|p| p.dir == dir)
            .map(|p| (p.dir.clone(), p.name.clone()))
            .or_else(|| browser::valid_profile_dir(dir).then(|| (dir.to_owned(), dir.to_owned())))
        else {
            return;
        };
        let changed = self.browser_setting().profile_dir != dir;
        self.browser_edit(cx, |b| {
            b.profile_dir = profile.0.clone();
            b.profile_name = profile.1.clone();
            if changed {
                b.ready = false;
            }
        });
        if changed {
            if let (Some(id), Ok(store)) = (self.active.clone(), self.browser_store()) {
                let _ = store.remove_token(&id);
            }
            self.browser_ui.replacing = false;
            self.browser_ui.test = TestState::Idle;
        }
        self.browser_refresh(cx);
    }

    fn browser_open(&mut self, url: String, cx: &mut Context<Self>) {
        let dir = self.browser_setting().profile_dir;
        let env = self.browser_ui.env();
        self.browser_ui.error = self.browser_ui.ops.open_in_profile(&env, &dir, &url).err();
        cx.notify();
    }

    /// Opens the Web Store page in the chosen profile and watches for the install.
    pub(super) fn browser_install(&mut self, cx: &mut Context<Self>) {
        self.browser_open(browser::install_url(), cx);
        if self.browser_ui.error.is_none() {
            self.browser_watch_install(cx);
        }
    }

    fn browser_watch_install(&mut self, cx: &mut Context<Self>) {
        if self.browser_ui.watching {
            return;
        }
        self.browser_ui.watching = true;
        cx.spawn(async move |this, cx| {
            // Chrome cannot install silently; the user clicks "Add to Chrome" in their own time.
            for _ in 0..300 {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let done = this
                    .update(cx, |this, cx| {
                        this.browser_refresh(cx);
                        this.browser_ui.installed || this.settings != Some(SettingsSection::Browser)
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.browser_ui.watching = false);
        })
        .detach();
    }

    fn browser_save_token(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let field = self.value("browser-token", cx);
        let saved = self
            .browser_store()
            .and_then(|store| store.save_token(&id, &field));
        match saved {
            Ok(()) => {
                stop_server(&id);
                // A new token: read its last four again.
                self.browser_ui.hint = None;
                if let Some(input) = self.fields.get("browser-token").cloned() {
                    input.update(cx, |f, cx| f.set_text(String::new(), cx));
                }
                self.browser_ui.replacing = false;
                self.browser_ui.test = TestState::Idle;
                self.browser_ui.error = None;
            }
            Err(error) => self.browser_ui.error = Some(error),
        }
        self.browser_refresh(cx);
    }

    fn browser_remove_token(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.active.clone() {
            stop_server(&id);
            self.browser_ui.error = self
                .browser_store()
                .and_then(|store| store.remove_token(&id))
                .err();
        }
        self.browser_ui.replacing = false;
        self.browser_ui.test = TestState::Idle;
        self.browser_refresh(cx);
    }

    /// Starts the MCP server with this Mono's setting and lists the browser's tabs.
    pub(super) fn browser_test(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active.clone() else {
            return;
        };
        let setting = self.browser_setting();
        let token = match self.browser_store().and_then(|store| store.token(&id)) {
            Ok(Some(token)) => token,
            Ok(None) => {
                self.browser_ui.test = TestState::Failed("Paste the extension token first.".into());
                cx.notify();
                return;
            }
            Err(error) => {
                self.browser_ui.test = TestState::Failed(error);
                cx.notify();
                return;
            }
        };
        if setting.profile_dir.is_empty() {
            self.browser_ui.test = TestState::Failed("Choose a Chrome profile first.".into());
            cx.notify();
            return;
        }
        let env = self.browser_ui.env();
        let dir = browser::output_dir(&self.root, &id);
        let _ = std::fs::create_dir_all(&dir);
        let runtime = browser::probe_runtime(&setting, token, &dir, &env);
        let ops = self.browser_ui.ops.clone();
        self.browser_ui.test = TestState::Running;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { ops.probe(&runtime, &env) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.browser_ui.test = match result {
                    Ok(()) => TestState::Passed,
                    Err(error) => TestState::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn browser_add_origin(&mut self, allowed: bool, cx: &mut Context<Self>) {
        let key = if allowed {
            "browser-allowed-add"
        } else {
            "browser-blocked-add"
        };
        let text = self.value(key, cx);
        if text.trim().is_empty() {
            return;
        }
        match browser::normalize_origin(&text) {
            Ok(origin) => {
                self.browser_edit(cx, |b| {
                    let list = if allowed {
                        &mut b.allowed_origins
                    } else {
                        &mut b.blocked_origins
                    };
                    list.push(origin);
                });
                if let Some(input) = self.fields.get(key).cloned() {
                    input.update(cx, |f, cx| f.set_text(String::new(), cx));
                }
            }
            Err(error) => {
                self.browser_ui.error = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn browser_remove_origin(
        &mut self,
        allowed: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.browser_edit(cx, |b| {
            let list = if allowed {
                &mut b.allowed_origins
            } else {
                &mut b.blocked_origins
            };
            if index < list.len() {
                list.remove(index);
            }
        });
    }

    fn browser_origin_card(
        &mut self,
        allowed: bool,
        origins: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind: &'static str = if allowed { "allowed" } else { "blocked" };
        let key = format!("browser-{kind}-add");
        let input = self.field(
            &key,
            "Add an origin, for example https://example.com",
            String::new(),
            false,
            cx,
        );
        if self.field_blurs.insert(input.entity_id()) {
            self.observations.push(
                cx.subscribe(&input, move |this, _, _: &input::Submitted, cx| {
                    this.browser_add_origin(allowed, cx)
                }),
            );
        }
        let _ = window;
        let rows = origins.iter().enumerate().map(|(ix, origin)| {
            let row = format!("mono-browser-{kind}-{ix}");
            let remove = format!("mono-browser-{kind}-remove-{ix}");
            div()
                .id(SharedString::from(row.clone()))
                .debug_selector(move || row.clone())
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(28.))
                .text_size(px(12.))
                .text_color(content(0.8))
                .child(div().flex_1().min_w_0().truncate().child(origin.clone()))
                .child(
                    crate::modal::ghost_button(SharedString::from(remove.clone()), "Remove")
                        .debug_selector(move || remove.clone())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.browser_remove_origin(allowed, ix, cx)
                        })),
                )
        });
        let mut footer = div().flex().items_center().gap_2().px_3().py_2().child(
            div()
                .id(SharedString::from(format!("mono-browser-{kind}-input")))
                .flex_1()
                .min_w_0()
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .child(input),
        );
        footer = footer.child(
            if allowed {
                action("mono-browser-allowed-add", "Add")
            } else {
                action("mono-browser-blocked-add", "Add")
            }
            .on_click(cx.listener(move |this, _, _, cx| this.browser_add_origin(allowed, cx))),
        );
        if !allowed {
            footer = footer.child(
                action("mono-browser-blocked-reset", "Reset to defaults").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.browser_edit(cx, |b| {
                            b.blocked_origins = MonoBrowser::default().blocked_origins
                        })
                    },
                )),
            );
        }
        card()
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child(if allowed {
                        "Allowed origins. When any are listed, only these may be used."
                    } else {
                        "Blocked origins: banks, wallets and payment pages. The Mono never opens these."
                    }),
            )
            .children(rows)
            .child(footer)
            .into_any_element()
    }

    pub(super) fn browser_page(
        &mut self,
        mono: &Mono,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let setting = mono.browser.clone().unwrap_or_default();
        let mut sections: Vec<AnyElement> =
            vec![note("mono-browser-subtitle", SUBTITLE, 0.55).into_any_element()];
        if let Some(error) = self.browser_ui.error.clone() {
            sections.push(alert("mono-browser-error", error).into_any_element());
        }
        if let Some(reason) = browser::server::servers()
            .attach_error(&mono.id)
            .filter(|_| setting.enabled)
        {
            sections.push(
                alert(
                    "mono-browser-attach-error",
                    format!("The browser could not be attached to the last run: {reason}."),
                )
                .into_any_element(),
            );
        }
        let toggle = switch_track("mono-browser-enabled", "Use the browser", setting.enabled)
            .debug_selector(|| "mono-browser-enabled".into())
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browser_toggle(cx)));
        sections.push(
            card()
                .child(switch_setting(
                    "Use the browser",
                    "Off by default. The Mono drives the Chrome profile you choose.",
                    false,
                    toggle.into_any_element(),
                ))
                .into_any_element(),
        );
        if setting.enabled {
            sections.push(self.browser_prerequisites(cx));
            sections.push(self.browser_profile_card(mono, &setting, cx));
            sections.push(self.browser_token_card(&setting, cx));
            sections.push(self.browser_safety_card(&setting, window, cx));
            sections.push(self.browser_show_actions_card(&setting, cx));
            sections.push(self.recipes_card(cx));
            sections.push(self.browser_audit_card());
        }
        div()
            .id("mono-browser-page")
            .debug_selector(|| "mono-browser-page".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(Self::settings_page_header("Browser"))
            .child(
                div()
                    .id("mono-browser-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(
                        sections
                            .into_iter()
                            .map(|section| div().flex_shrink_0().child(section)),
                    ),
            )
            .into_any_element()
    }

    fn browser_prerequisites(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prereq) = self.browser_ui.prereq.clone() else {
            return div().into_any_element();
        };
        let row = |id: &'static str, label: &str, check: &Check, fix: Option<AnyElement>| {
            let (text, alpha) = match check {
                Check::Ok => ("Ready".to_owned(), 0.7),
                Check::Missing(why) => (why.clone(), 0.5),
                Check::Denied => ("Needs Full Disk Access".to_owned(), 0.5),
            };
            setting(
                label,
                None,
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(note(id, text, alpha))
                    .children(fix)
                    .into_any_element(),
            )
        };
        let fda_fix = (prereq.full_disk_access == Check::Denied).then(|| {
            action("mono-browser-fda-fix", "Open settings")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.browser_ui.ops.open_full_disk_access();
                    cx.notify();
                }))
                .into_any_element()
        });
        card()
            .child(row(
                "mono-browser-prereq-chrome",
                "Google Chrome",
                &prereq.chrome,
                None,
            ))
            .child(div().h(px(1.)).bg(content(0.05)))
            .child(row(
                "mono-browser-prereq-fda",
                "Access to Chrome's profiles",
                &prereq.full_disk_access,
                fda_fix,
            ))
            .child(div().h(px(1.)).bg(content(0.05)))
            .child(row(
                "mono-browser-prereq-node",
                "Node.js (node and npx)",
                &prereq.node,
                None,
            ))
            .into_any_element()
    }

    fn browser_profile_card(
        &mut self,
        mono: &Mono,
        setting_: &MonoBrowser,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = self
            .browser_ui
            .profiles
            .iter()
            .find(|p| p.dir == setting_.profile_dir)
            .map(ChromeProfile::label)
            .or_else(|| (!setting_.profile_name.is_empty()).then(|| setting_.profile_name.clone()))
            .unwrap_or_else(|| "Choose a profile".into());
        let trigger = div()
            .id("mono-browser-profile")
            .debug_selector(|| "mono-browser-profile".into())
            .role(Role::Button)
            .aria_label(label.clone())
            .relative()
            .h(px(28.))
            .min_w(px(160.))
            .max_w(px(260.))
            .px_2p5()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .text_size(px(12.))
            .text_color(content(0.85))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.06)))
            .on_click(cx.listener(|this, _, window, cx| this.browser_open_select(window, cx)))
            .child(self.browser_ui.profile_anchor.probe())
            .child(div().min_w_0().truncate().child(label))
            .child(icon("chevron-down", 12.).text_color(content(0.5)));
        let shared = browser::profile_shared_with(&self.roster.monos, &mono.id);
        let has_profile = !setting_.profile_dir.is_empty();
        let installed = self.browser_ui.installed;
        let status = if !has_profile {
            "Choose a profile first"
        } else if installed {
            "Installed"
        } else {
            "Not installed"
        };
        let controls = div()
            .flex()
            .items_center()
            .gap_2()
            .child(note(
                "mono-browser-install-status",
                status,
                if installed { 0.8 } else { 0.5 },
            ))
            .when(has_profile && !installed, |d| {
                d.child(
                    action("mono-browser-install", "Install extension")
                        .on_click(cx.listener(|this, _, _, cx| this.browser_install(cx))),
                )
            });
        let mut card_ = card()
            .child(setting(
                "Chrome profile",
                Some("The signed-in browser this Mono drives."),
                trigger.into_any_element(),
            ))
            .child(div().h(px(1.)).bg(content(0.05)));
        if !shared.is_empty() {
            card_ = card_.child(div().px_3().py_2().child(note(
                "mono-browser-shared-warning",
                format!(
                    "{} also uses this profile. Two Monos on one browser can fight over its tabs.",
                    shared.join(", ")
                ),
                0.75,
            )));
        }
        if self.browser_ui.profiles_unreadable {
            card_ = card_.child(div().px_3().py_2().child(note(
                "mono-browser-profiles-note",
                "Other Chrome profiles are not listed: the app cannot read Chrome's profile data. Grant Full Disk Access to list them. The saved profile keeps working.",
                0.6,
            )));
        }
        card_
            .child(setting(
                "Playwright MCP Bridge",
                Some("Chrome extension that lets the Mono connect."),
                controls.into_any_element(),
            ))
            .into_any_element()
    }

    fn browser_token_card(&mut self, setting_: &MonoBrowser, cx: &mut Context<Self>) -> AnyElement {
        let hint = self.browser_ui.hint.clone();
        let editing = hint.is_none() || self.browser_ui.replacing;
        let has_profile = !setting_.profile_dir.is_empty();
        let control: AnyElement = if editing {
            let input = self.field(
                "browser-token",
                "Paste the extension token",
                String::new(),
                false,
                cx,
            );
            input.update(cx, |f, _| f.set_secret(true));
            if self.field_blurs.insert(input.entity_id()) {
                self.observations.push(
                    cx.subscribe(&input, |this, _, _: &input::Submitted, cx| {
                        this.browser_save_token(cx)
                    }),
                );
            }
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("mono-browser-token-input")
                        .w(px(130.))
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(content(0.1))
                        .child(input),
                )
                .child(
                    action("mono-browser-token-save", "Save")
                        .on_click(cx.listener(|this, _, _, cx| this.browser_save_token(cx))),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(note(
                    "mono-browser-token-hint",
                    format!(
                        "\u{2022}\u{2022}\u{2022}\u{2022}{}",
                        hint.clone().unwrap_or_default()
                    ),
                    0.8,
                ))
                .child(
                    action("mono-browser-token-replace", "Replace").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.browser_ui.replacing = true;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    action("mono-browser-token-remove", "Remove")
                        .on_click(cx.listener(|this, _, _, cx| this.browser_remove_token(cx))),
                )
                .into_any_element()
        };
        let test = match self.browser_ui.test.clone() {
            TestState::Idle => None,
            TestState::Running => Some(("Connecting to Chrome\u{2026}".to_owned(), 0.6)),
            TestState::Passed => Some(("Connected. The Mono can see your tabs.".to_owned(), 0.85)),
            TestState::Failed(error) => Some((error, 0.75)),
        };
        let running = self.browser_ui.test == TestState::Running;
        let mut card_ = card().child(setting(
            "Extension token",
            Some("Stored encrypted. Only the last four characters are shown."),
            control,
        ));
        if has_profile {
            card_ = card_
                .child(div().h(px(1.)).bg(content(0.05)))
                .child(setting(
                    "Get the token",
                    Some("Opens the extension page in this profile. Copy its token."),
                    action("mono-browser-open-token-page", "Open extension page")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.browser_open(browser::token_page_url(), cx)
                        }))
                        .into_any_element(),
                ));
        }
        card_ = card_
            .child(div().h(px(1.)).bg(content(0.05)))
            .child(setting(
                "Test connection",
                None,
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!running, |d| {
                        d.child(
                            action("mono-browser-test", "Test connection")
                                .on_click(cx.listener(|this, _, _, cx| this.browser_test(cx))),
                        )
                    })
                    .into_any_element(),
            ));
        if let Some((text, alpha)) = test {
            card_ = card_.child(div().px_3().pb_2().child(note(
                "mono-browser-test-result",
                text,
                alpha,
            )));
        }
        card_.into_any_element()
    }

    fn browser_safety_card(
        &mut self,
        setting_: &MonoBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = BrowserTrust::ALL
            .into_iter()
            .map(|trust| trust_row(trust, setting_.trust == trust, cx))
            .collect();
        let blocked = self.browser_origin_card(false, &setting_.blocked_origins, window, cx);
        let allowed = self.browser_origin_card(true, &setting_.allowed_origins, window, cx);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Trust level", Some(TRUST_SUBTITLE)))
            .child(card().children(rows))
            .child(
                note(
                    "mono-browser-trust-note",
                    BrowserTrust::ENFORCEMENT_NOTE,
                    0.5,
                )
                .px_1()
                .pb_2(),
            )
            .child(blocked)
            .child(allowed)
            .into_any_element()
    }

    fn browser_audit_card(&self) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .browser_ui
            .audit
            .iter()
            .enumerate()
            .map(|(ix, e)| {
                let line = [e.summary.as_str(), e.url.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("  ");
                div()
                    .flex()
                    .gap_3()
                    .px_3()
                    .py_1()
                    .child(
                        note(format!("mono-browser-audit-time-{ix}"), ago(e.ts_ms), 0.4).w(px(64.)),
                    )
                    .child(
                        note(
                            format!("mono-browser-audit-{ix}"),
                            format!("{}  {line}", e.tool),
                            0.8,
                        )
                        .flex_1()
                        .min_w_0()
                        .truncate(),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading("Activity", Some("Pages and actions of this Mono's browser, newest first. Typed values are never kept.")))
            .child(card().children(rows).when(empty, |c| {
                c.child(div().px_3().py_3().child(note("mono-browser-audit-empty", "Nothing yet", 0.45)))
            }))
            .into_any_element()
    }

    /// A deleted Mono leaves no token and no audit rows.
    pub(super) fn browser_forget(&mut self, id: &str) {
        stop_server(id);
        if let Ok(store) = self.browser_store() {
            let _ = store.delete_mono(id);
        }
    }
}
