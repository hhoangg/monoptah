//! The Browser page of a Mono's settings: its states, the token handling and the safety
//! rules. Nothing here starts Chrome or the MCP server; `Fake` stands in for both.
use super::browser_ui::{BrowserOps, TestState};
use super::workflows_ui::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::browser::{self, BrowserTrust, MonoBrowser, MonoBrowserStore, PrereqEnv};
use std::sync::{Arc, Mutex as StdMutex};

// Obviously fake: not a real extension token.
const TOKEN: &str = "fake-extension-token-0123456789-WXYZ";

#[derive(Default)]
struct Fake {
    opened: StdMutex<Vec<(String, String)>>,
    fda: StdMutex<u32>,
    probe: StdMutex<Option<Result<(), String>>>,
    probed: StdMutex<Vec<(Vec<String>, String)>>,
}
impl BrowserOps for Fake {
    fn open_in_profile(&self, _: &PrereqEnv, profile: &str, url: &str) -> Result<(), String> {
        self.opened
            .lock()
            .unwrap()
            .push((profile.into(), url.into()));
        Ok(())
    }
    fn open_full_disk_access(&self) {
        *self.fda.lock().unwrap() += 1;
    }
    fn probe(&self, runtime: &browser::Runtime, _: &PrereqEnv) -> Result<(), String> {
        self.probed
            .lock()
            .unwrap()
            .push((runtime.spec.args.clone(), runtime.token().to_owned()));
        self.probe.lock().unwrap().clone().unwrap_or(Ok(()))
    }
}

const LOCAL_STATE: &str = r#"{"profile":{"info_cache":{
  "Default":{"name":"Personal","user_name":"me@example.com"},
  "Profile 6":{"name":"Work","user_name":"work@example.com","avatar_icon":"chrome://theme/IDR_PROFILE_AVATAR_26"}}}}"#;

/// A Chrome that exists, with its profile list readable and node on the path.
fn chrome_env(dir: &std::path::Path, complete: bool) -> PrereqEnv {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let chrome = dir.join("chrome-bin");
    let data = dir.join("Chrome");
    if complete {
        for tool in ["node", "npx"] {
            std::fs::write(bin.join(tool), "").unwrap();
        }
        std::fs::write(&chrome, "").unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("Local State"), LOCAL_STATE).unwrap();
    }
    PrereqEnv {
        chrome_binaries: vec![chrome],
        user_data_dir: data,
        search_path: bin.to_string_lossy().into_owned(),
    }
}

struct Page {
    s: Setup,
    visual: VisualTestContext,
    ops: Arc<Fake>,
    env: PrereqEnv,
    _dir: TempDir,
}
/// A scratch folder that goes away with the test.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn open_with(cx: &mut TestAppContext, complete: bool) -> Page {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let dir = std::env::temp_dir().join(format!("native-browser-{}", mono::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = TempDir(dir);
    let env = chrome_env(&dir.0, complete);
    let ops = Arc::new(Fake::default());
    let (o, e) = (ops.clone(), env.clone());
    s.manager.update(cx, |m, _| m.browser_ui.use_fakes(o, e));
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    click(&mut visual, "mono-settings-Browser");
    visual.refresh().unwrap();
    Page {
        s,
        visual,
        ops,
        env,
        _dir: dir,
    }
}
fn open(cx: &mut TestAppContext) -> Page {
    open_with(cx, true)
}

impl Page {
    fn setting(&self) -> Option<MonoBrowser> {
        self.s.manager.read_with(&self.visual, |m, _| {
            m.roster.get(&self.s.id).and_then(|m| m.browser.clone())
        })
    }
    fn on(&mut self, selector: &'static str) -> bool {
        self.visual.debug_bounds(selector).is_some()
    }
    fn click(&mut self, selector: &'static str) {
        click(&mut self.visual, selector);
        self.visual.refresh().unwrap();
    }
    fn manager<R>(&mut self, f: impl FnOnce(&mut Monos, &mut Context<Monos>) -> R) -> R {
        let r = self.s.manager.update(&mut self.visual, f);
        self.visual.run_until_parked();
        self.visual.refresh().unwrap();
        r
    }
    fn type_into(&mut self, key: &str, text: &str) {
        let field = self
            .s
            .manager
            .read_with(&self.visual, |m, _| m.fields.get(key).cloned())
            .unwrap_or_else(|| panic!("no field {key}"));
        field.update(&mut self.visual, |f, cx| f.set_text(text.to_owned(), cx));
        self.visual.run_until_parked();
        self.visual.refresh().unwrap();
    }
    fn secrets(&self) -> MonoBrowserStore {
        MonoBrowserStore::for_store(&locked(&self.s.store).unwrap()).unwrap()
    }
    fn enable(&mut self) {
        self.click("mono-browser-enabled");
    }
    fn choose(&mut self, dir: &str) {
        let dir = dir.to_owned();
        self.manager(|m, cx| m.browser_choose_profile(&dir, cx));
    }
    fn ui<R>(&self, f: impl FnOnce(&browser_ui::BrowserUi) -> R) -> R {
        self.s
            .manager
            .read_with(&self.visual, |m, _| f(&m.browser_ui))
    }
    fn save_token(&mut self, token: &str) {
        self.type_into("browser-token", token);
        self.click("mono-browser-token-save");
    }
}

#[gpui::test]
fn the_browser_tab_starts_off_and_shows_only_the_switch(cx: &mut TestAppContext) {
    let mut page = open(cx);
    assert!(page.on("mono-browser-page"));
    assert!(page.on("mono-browser-enabled"));
    assert!(!page.on("mono-browser-prereq-chrome") && !page.on("mono-browser-profile"));
    assert!(
        page.setting().is_none(),
        "nothing is saved until the user turns it on"
    );
}

#[gpui::test]
fn turning_it_on_shows_prerequisites_and_the_prefilled_blocklist(cx: &mut TestAppContext) {
    let mut page = open_with(cx, false);
    page.enable();
    let setting = page.setting().unwrap();
    assert!(setting.enabled && !setting.ready);
    assert_eq!(
        setting.trust,
        BrowserTrust::AskRisky,
        "new Monos start at Level 2"
    );
    assert!(
        setting
            .blocked_origins
            .iter()
            .any(|o| o == "https://www.paypal.com")
    );
    for id in [
        "mono-browser-prereq-chrome",
        "mono-browser-prereq-fda",
        "mono-browser-prereq-node",
        "mono-browser-blocked-0",
        browser_ui::trust_selector(BrowserTrust::AskRisky),
    ] {
        assert!(page.on(id), "{id}");
    }
    let prereq = page.ui(|u| u.prereq.clone()).unwrap();
    assert!(!prereq.all_ok() && !prereq.chrome.is_ok() && !prereq.node.is_ok());
    // Turning it off again keeps the choices but removes the controls.
    page.enable();
    assert!(!page.setting().unwrap().enabled);
    assert!(!page.on("mono-browser-prereq-chrome"));
}

#[gpui::test]
fn a_refused_profile_read_offers_the_full_disk_access_fix(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(!page.on("mono-browser-fda-fix"), "readable: nothing to fix");
    page.manager(|m, cx| {
        let mut prereq = m.browser_ui.prereq.clone().unwrap();
        prereq.full_disk_access = browser::Check::Denied;
        m.browser_ui.prereq = Some(prereq);
        cx.notify();
    });
    page.click("mono-browser-fda-fix");
    assert_eq!(*page.ops.fda.lock().unwrap(), 1);
}

#[gpui::test]
fn the_picker_lists_chrome_profiles_and_changing_one_drops_the_token(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    let labels = page.ui(|u| u.profiles.iter().map(|p| p.label()).collect::<Vec<_>>());
    assert_eq!(
        labels,
        ["Personal (me@example.com)", "Work (work@example.com)"]
    );
    page.click("mono-browser-profile");
    assert!(page.ui(|u| u.select_is_open()));
    page.click("mono-browser-profile");
    assert!(
        !page.ui(|u| u.select_is_open()),
        "the trigger closes it again"
    );

    page.choose("Profile 6");
    let setting = page.setting().unwrap();
    assert_eq!(
        (setting.profile_dir.as_str(), setting.profile_name.as_str()),
        ("Profile 6", "Work")
    );
    page.save_token(TOKEN);
    assert!(page.secrets().has_secret(&page.s.id, browser::SECRET_NAME));
    page.choose("Profile 6");
    assert!(
        page.secrets().has_secret(&page.s.id, browser::SECRET_NAME),
        "same profile keeps it"
    );
    page.choose("Default");
    assert!(!page.secrets().has_secret(&page.s.id, browser::SECRET_NAME));
    assert!(!page.setting().unwrap().ready);
    assert!(page.ui(|u| u.hint.is_none()) && page.on("mono-browser-token-save"));
}

#[gpui::test]
fn another_mono_on_the_same_profile_gets_a_warning(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    assert!(!page.on("mono-browser-shared-warning"));
    page.manager(|m, cx| {
        let other = m.roster.create(Vec::new()).id.clone();
        let mono = m.roster.get_mut(&other).unwrap();
        mono.name = "Other".into();
        mono.browser = Some(MonoBrowser {
            enabled: true,
            profile_dir: "Profile 6".into(),
            ..Default::default()
        });
        cx.notify();
    });
    assert!(page.on("mono-browser-shared-warning"));
}

#[gpui::test]
fn install_opens_the_web_store_in_the_profile_and_detects_it(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(!page.on("mono-browser-install"), "needs a profile first");
    page.choose("Profile 6");
    assert!(page.on("mono-browser-install") && !page.ui(|u| u.installed));
    page.click("mono-browser-install");
    assert_eq!(
        page.ops.opened.lock().unwrap().as_slice(),
        [("Profile 6".to_owned(), browser::install_url())]
    );
    // The user installs it in Chrome; the next check sees the folder.
    let folder = page
        .env
        .user_data_dir
        .join("Profile 6/Extensions")
        .join(browser::EXTENSION_ID);
    std::fs::create_dir_all(folder.join("1.0_0")).unwrap();
    page.manager(|m, cx| m.browser_refresh(cx));
    assert!(page.ui(|u| u.installed));
    assert!(!page.on("mono-browser-install"));
    assert!(page.on("mono-browser-install-status"));
}

#[gpui::test]
fn the_token_is_stored_sealed_and_shown_as_its_last_four(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    page.click("mono-browser-open-token-page");
    assert_eq!(
        page.ops.opened.lock().unwrap()[0].1,
        browser::token_page_url()
    );

    page.save_token("too short");
    assert!(page.ui(|u| u.error().is_some()), "refused");
    assert!(page.ui(|u| u.hint.is_none()));

    page.save_token(TOKEN);
    assert_eq!(page.ui(|u| u.hint.clone()).as_deref(), Some("WXYZ"));
    assert_eq!(
        page.secrets().token(&page.s.id).unwrap().unwrap().as_str(),
        TOKEN
    );
    assert!(page.on("mono-browser-token-hint") && !page.on("mono-browser-token-save"));
    // The roster, which is plain JSON, never holds it.
    let roster = locked(&page.s.store)
        .unwrap()
        .setting("mono-roster")
        .unwrap()
        .unwrap();
    assert!(!roster.contains(TOKEN) && !roster.contains("WXYZ"));
    // The field is empty again and stays secret.
    let field = page
        .s
        .manager
        .read_with(&page.visual, |m, _| m.fields.get("browser-token").cloned())
        .unwrap();
    assert!(field.read_with(&page.visual, |f, _| f.content.is_empty()));

    page.click("mono-browser-token-replace");
    assert!(page.on("mono-browser-token-save"));
    assert!(field.read_with(&page.visual, |f, _| f.is_secret()));
    page.save_token(&format!("{}AAAA", &TOKEN[..32]));
    assert_eq!(page.ui(|u| u.hint.clone()).as_deref(), Some("AAAA"));

    page.click("mono-browser-token-remove");
    assert!(page.secrets().token(&page.s.id).unwrap().is_none());
    assert!(page.on("mono-browser-token-save") && page.ui(|u| u.hint.is_none()));
}

#[gpui::test]
fn the_browser_is_ready_only_with_prerequisites_a_profile_and_a_token(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(!page.setting().unwrap().ready);
    page.choose("Profile 6");
    assert!(!page.setting().unwrap().ready, "no token yet");
    page.save_token(TOKEN);
    let setting = page.setting().unwrap();
    assert!(setting.ready && setting.active());
    // The Mono's rules now carry the browser paragraph for the chosen profile.
    let rules = page.s.manager.read_with(&page.visual, |m, _| {
        monocode_core::mono::instructions(
            m.roster.get(&page.s.id).unwrap(),
            std::path::Path::new("/bin/app"),
        )
    });
    assert!(rules.contains("## Browser") && rules.contains("profile \"Work\""));
    page.click("mono-browser-token-remove");
    assert!(!page.setting().unwrap().ready);
}

#[gpui::test]
fn test_connection_starts_the_pinned_server_with_the_token_and_reports_the_outcome(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    page.click("mono-browser-test");
    assert!(matches!(page.ui(|u| u.test_state()), TestState::Failed(e) if e.contains("token")));
    page.save_token(TOKEN);

    page.click("mono-browser-test");
    page.visual.run_until_parked();
    assert_eq!(page.ui(|u| u.test_state()), TestState::Passed);
    {
        let probed = page.ops.probed.lock().unwrap();
        let (args, token) = &probed[0];
        assert_eq!(token, TOKEN);
        assert!(args.iter().any(|a| a == "@playwright/mcp@0.0.86"));
        assert!(args.iter().any(|a| a == "Profile 6"));
        assert!(
            args.iter().all(|a| !a.contains(TOKEN)),
            "token never in argv"
        );
    }
    page.visual.refresh().unwrap();
    assert!(page.on("mono-browser-test-result"));

    *page.ops.probe.lock().unwrap() = Some(Err("Token needs update: rotate it.".into()));
    page.click("mono-browser-test");
    page.visual.run_until_parked();
    assert_eq!(
        page.ui(|u| u.test_state()),
        TestState::Failed("Token needs update: rotate it.".into())
    );
}

#[gpui::test]
fn safety_rules_can_be_edited_and_invalid_origins_are_refused(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    let defaults = page.setting().unwrap().blocked_origins;

    // The lists are far below the fold of the test window, so the actions are called directly;
    // the buttons call the same methods.
    page.type_into("browser-blocked-add", "Bank.Example");
    page.manager(|m, cx| m.browser_add_origin(false, cx));
    let blocked = page.setting().unwrap().blocked_origins;
    assert_eq!(blocked.last().unwrap(), "https://bank.example");
    assert_eq!(blocked.len(), defaults.len() + 1);

    page.type_into("browser-blocked-add", "*.wildcard.example");
    page.manager(|m, cx| m.browser_add_origin(false, cx));
    assert!(page.ui(|u| u.error().is_some()));
    assert_eq!(
        page.setting().unwrap().blocked_origins.len(),
        defaults.len() + 1
    );

    page.manager(|m, cx| m.browser_remove_origin(false, 0, cx));
    assert_eq!(page.setting().unwrap().blocked_origins[0], defaults[1]);
    page.manager(|m, cx| {
        m.browser_edit(cx, |b| {
            b.blocked_origins = MonoBrowser::default().blocked_origins
        })
    });
    assert_eq!(page.setting().unwrap().blocked_origins, defaults);

    page.type_into("browser-allowed-add", "https://docs.example/some/page");
    page.manager(|m, cx| m.browser_add_origin(true, cx));
    assert_eq!(
        page.setting().unwrap().allowed_origins,
        ["https://docs.example"]
    );
    page.manager(|m, cx| m.browser_remove_origin(true, 0, cx));
    assert!(page.setting().unwrap().allowed_origins.is_empty());
}

#[gpui::test]
fn the_trust_selector_shows_three_levels_with_an_explanation_each(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    // The old "ask before send, post, delete or pay" switch and its read-only note are gone.
    assert!(!page.on("mono-browser-confirm") && !page.on("mono-browser-full-access-note"));
    for trust in BrowserTrust::ALL {
        let selector = browser_ui::trust_selector(trust);
        assert!(page.on(selector), "{selector}");
    }
    // Each row has its own explanation, and one note says what is enforced and what is trusted.
    for selector in [
        "mono-browser-trust-read-only-text",
        "mono-browser-trust-ask-risky-text",
        "mono-browser-trust-autonomous-text",
        "mono-browser-trust-note",
    ] {
        assert!(page.on(selector), "{selector}");
    }
    assert_eq!(page.setting().unwrap().trust, BrowserTrust::AskRisky);

    for trust in [
        BrowserTrust::ReadOnly,
        BrowserTrust::Autonomous,
        BrowserTrust::AskRisky,
    ] {
        // The rows are far below the fold of the test window, so the row's action is called
        // directly; the click calls the same method.
        page.manager(|m, cx| m.browser_set_trust(trust, cx));
        assert_eq!(page.setting().unwrap().trust, trust);
    }
}

#[gpui::test]
fn changing_the_level_reconfigures_the_gate_and_keeps_the_server(cx: &mut TestAppContext) {
    use monocode_core::mono::browser::gate::Policy;
    let mut page = open(cx);
    page.enable();
    let before = page.setting().unwrap().server_key().1.to_owned();
    page.manager(|m, cx| m.browser_edit(cx, |b| b.trust = BrowserTrust::ReadOnly));
    let setting = page.setting().unwrap();
    assert_eq!(setting.trust, BrowserTrust::ReadOnly);
    assert_eq!(setting.server_key().1, before);
    // The level is not part of the server's shape, so choosing one never restarts the server.
    let mut other = setting.clone();
    other.trust = BrowserTrust::Autonomous;
    assert_eq!(setting.server_key(), other.server_key());
    assert_eq!(
        Policy::new(setting.trust, setting.blocked_origins.clone()).trust,
        BrowserTrust::ReadOnly
    );
}

#[gpui::test]
fn the_activity_log_lists_this_monos_browser_calls(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(page.on("mono-browser-audit-empty"));
    let entry = browser::audit_entry(
        "mcp__playwright_browser__browser_navigate",
        r#"{"url":"https://a.example/"}"#,
        page.s.session,
    )
    .unwrap();
    page.secrets().record(&page.s.id, &entry).unwrap();
    page.manager(|m, cx| m.browser_refresh(cx));
    assert!(page.on("mono-browser-audit-0") && !page.on("mono-browser-audit-empty"));
}

#[gpui::test]
fn deleting_a_mono_deletes_its_token_and_activity(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    page.save_token(TOKEN);
    let entry = browser::audit_entry("mcp__playwright_browser__browser_click", "{}", 1).unwrap();
    page.secrets().record(&page.s.id, &entry).unwrap();
    let id = page.s.id.clone();
    page.manager(|m, cx| m.remove_mono(&id, cx));
    assert!(page.secrets().token(&id).unwrap().is_none());
    assert!(page.secrets().recent(&id, 10).is_empty());
}

#[gpui::test]
fn the_install_poll_does_not_decrypt_the_token_again(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    page.save_token(TOKEN);
    assert_eq!(page.ui(|u| u.hint.clone()).as_deref(), Some("WXYZ"));
    // Another token behind the page's back: a refresh keeps the cached last four.
    let other = format!("{}QQQQ", &TOKEN[..32]);
    page.secrets().save_token(&page.s.id, &other).unwrap();
    page.manager(|m, cx| m.browser_refresh(cx));
    assert_eq!(page.ui(|u| u.hint.clone()).as_deref(), Some("WXYZ"));
    // A token saved through the page is read again.
    page.click("mono-browser-token-replace");
    page.save_token(&other);
    assert_eq!(page.ui(|u| u.hint.clone()).as_deref(), Some("QQQQ"));
    // A vanished row clears it.
    page.secrets().remove_token(&page.s.id).unwrap();
    page.manager(|m, cx| m.browser_refresh(cx));
    assert!(page.ui(|u| u.hint.is_none()));
}
