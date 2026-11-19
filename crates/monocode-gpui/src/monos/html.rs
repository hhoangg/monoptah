//! Generated HTML lives in an opaque sandbox with no app bridge or network.
#[cfg(target_os = "macos")]
use gpui::canvas;
#[cfg(not(target_os = "macos"))]
use gpui::div;
use gpui::{Context, Window, prelude::*};
#[cfg(target_os = "macos")]
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::ProtocolObject,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSView, NSWindowOrderingMode};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize, NSString, NSURL};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSObject, NSObjectProtocol};
#[cfg(target_os = "macos")]
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKNavigationAction, WKNavigationActionPolicy,
    WKNavigationDelegate, WKNavigationType, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
#[cfg(target_os = "macos")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "macos")]
use std::{cell::RefCell, rc::Rc, sync::Arc};
#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSView))]
    #[name = "MonocodeArtifactClip"]
    #[thread_kind = MainThreadOnly]
    #[ivars = std::cell::Cell<bool>]
    struct ArtifactClip;
    unsafe impl NSObjectProtocol for ArtifactClip {}
    impl ArtifactClip {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            if !self.ivars().get() {
                None
            } else {
                unsafe { msg_send![super(self), hitTest: point] }
            }
        }
    }
);
/// What the navigation guard needs: the app handle to open links, and for a page loaded
/// from disk the only folder (canonical, no trailing slash) its `file:` URLs may point into.
#[cfg(target_os = "macos")]
struct GuardState {
    app: gpui::AsyncApp,
    file_root: Option<String>,
}
#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MonocodeArtifactNavigation"]
    #[thread_kind=MainThreadOnly]
    #[ivars=GuardState]
    struct NavigationGuard;
    unsafe impl NSObjectProtocol for NavigationGuard {}
    unsafe impl WKNavigationDelegate for NavigationGuard {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn decide(
            &self,
            _: &WKWebView,
            action: &WKNavigationAction,
            reply: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request().URL() }.and_then(|url| url.absoluteString());
            if unsafe { action.navigationType() } == WKNavigationType::LinkActivated
                && let Some(url) = url.as_ref()
                && (url.to_string().starts_with("https://")
                    || url.to_string().starts_with("http://"))
            {
                let url = url.to_string();
                self.ivars().app.clone().update(|cx| cx.open_url(&url));
            }
            let file_path = unsafe { action.request().URL() }
                .filter(|url| url.isFileURL())
                .and_then(|url| url.path())
                .map(|path| path.to_string());
            let allowed = match (&file_path, &self.ivars().file_root) {
                // A page loaded from the cache may show files below the cache folder only.
                (Some(path), Some(root)) => path == root || path.starts_with(&format!("{root}/")),
                (Some(_), None) => false,
                (None, _) => url.is_none_or(|url| {
                    let s = url.to_string();
                    s.starts_with("about:blank") || s.starts_with("about:srcdoc")
                }),
            };
            reply.call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }
    }
);
#[cfg(target_os = "macos")]
struct Hosted {
    clip: Retained<ArtifactClip>,
    web: Retained<WKWebView>,
    _guard: Retained<NavigationGuard>,
    markup: Arc<str>,
    gate: Rc<RefCell<LoadGate>>,
}
#[cfg(target_os = "macos")]
impl Rules {
    /// A page may load only when it needs no rules or they are installed.
    fn allows_load(self) -> bool {
        matches!(self, Rules::NotNeeded | Rules::Ready)
    }
}
/// Whether the remote-load rules stand in the way of loading a page, and the markup to load
/// once they are installed (the latest one, whatever was asked for earlier).
#[cfg(target_os = "macos")]
struct LoadGate {
    rules: Rules,
    latest: Arc<str>,
}
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rules {
    /// The page needs no remote-load rules.
    NotNeeded,
    /// Being compiled: nothing may load yet.
    Pending,
    Ready,
    /// They could not be had: the page stays blank rather than load without them.
    Failed,
}
#[cfg(target_os = "macos")]
struct Markup {
    ink: gpui::Hsla,
    accent: gpui::Hsla,
    html: Arc<str>,
}
/// A page that lives on disk (a download from a cloud workspace), with the folder its
/// relative assets may be read from.
#[cfg(target_os = "macos")]
struct FilePage {
    path: std::path::PathBuf,
    root: std::path::PathBuf,
    /// Identity of the page for the reload check; never changes.
    marker: Arc<str>,
    /// Whether the page may run JavaScript. A file shown as a preview runs none.
    scripts: bool,
}
pub(crate) struct Html {
    body: String,
    interactive: bool,
    #[cfg(target_os = "macos")]
    file: Option<Rc<FilePage>>,
    #[cfg(target_os = "macos")]
    markup: Option<Markup>,
    #[cfg(target_os = "macos")]
    view: Rc<RefCell<Option<Hosted>>>,
}
#[cfg(any(target_os = "macos", test))]
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
#[cfg(any(target_os = "macos", test))]
fn sandbox(body: &str) -> String {
    let color = |value: gpui::Hsla| {
        let value: gpui::Rgba = value.into();
        format!(
            "#{:02x}{:02x}{:02x}",
            (value.r * 255.) as u8,
            (value.g * 255.) as u8,
            (value.b * 255.) as u8
        )
    };
    let ink: gpui::Rgba = crate::theme::content(1.).into();
    let foreground = color(crate::theme::content(1.));
    let accent = color(crate::theme::accent_color());
    let light = ink.r * 0.2126 + ink.g * 0.7152 + ink.b * 0.0722 < 0.5;
    let background = if light { "#fafafa" } else { "#141414" };
    let muted = if light { "#737373" } else { "#969696" };
    let surface = if light { "#eeeeee" } else { "#222222" };
    let border = if light { "#d4d4d4" } else { "#383838" };
    let source = format!(
        "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline' https:; style-src 'unsafe-inline' https:; img-src data: blob: https:; font-src data: https:; media-src data: blob: https:; worker-src blob:; connect-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'\"><meta name=viewport content=\"width=device-width,initial-scale=1\"><style>:root{{--background:{background};--foreground:{foreground};--muted:{muted};--surface:{surface};--border:{border};--accent:{accent};--link:{accent};--positive:#10b981;--negative:#f43f5e;--chart-1:{accent};--chart-2:#10b981;--chart-3:#f59e0b;--chart-4:#f43f5e;--chart-5:#a78bfa;--chart-6:#22d3ee;--font-sans:system-ui;--font-mono:ui-monospace,monospace;--radius:10px;color-scheme:{scheme}}}body{{font:14px var(--font-sans);margin:20px;color:var(--foreground);background:var(--background)}}a{{color:var(--link)}}</style>{body}",
        scheme = if light { "light" } else { "dark" }
    );
    format!(
        "<!doctype html><style>html,body,iframe{{margin:0;border:0;width:100%;height:100%;overflow:hidden;background:{background}}}</style><iframe sandbox=\"allow-scripts\" referrerpolicy=\"no-referrer\" srcdoc=\"{}\"></iframe>",
        escape(&source)
    )
}
impl Html {
    pub(crate) fn new(body: String) -> Self {
        Self {
            body,
            interactive: true,
            #[cfg(target_os = "macos")]
            file: None,
            #[cfg(target_os = "macos")]
            markup: None,
            #[cfg(target_os = "macos")]
            view: Rc::new(RefCell::new(None)),
        }
    }
    /// Shows the page at `path` straight from disk, with read access to `root` only. Used
    /// for files downloaded from a cloud workspace, whose relative assets sit beside them.
    /// Only macOS has a web view to load it, so other platforms never build one.
    #[cfg(target_os = "macos")]
    pub(crate) fn from_file(path: std::path::PathBuf, root: std::path::PathBuf) -> Self {
        let mut this = Self::new(path.to_string_lossy().into_owned());
        // Canonical forms make the guard's folder comparison reliable (/var vs /private/var).
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        this.file = Some(Rc::new(FilePage {
            marker: path.to_string_lossy().into_owned().into(),
            path,
            root,
            scripts: true,
        }));
        this
    }
    /// The page runs no JavaScript at all: a file opened as a preview is shown, not executed.
    #[cfg(target_os = "macos")]
    pub(crate) fn without_scripts(mut self) -> Self {
        if let Some(file) = self.file.as_mut().and_then(Rc::get_mut) {
            file.scripts = false;
        }
        self
    }
    /// Whether the page may run JavaScript (always, for generated markup in its sandbox).
    #[cfg(all(target_os = "macos", test))]
    pub(crate) fn runs_scripts(&self) -> bool {
        self.file.as_ref().is_none_or(|file| file.scripts)
    }
    #[cfg(target_os = "macos")]
    fn markup(&mut self) -> Arc<str> {
        if let Some(file) = &self.file {
            return file.marker.clone();
        }
        let ink = crate::theme::content(1.);
        let accent = crate::theme::accent_color();
        if self
            .markup
            .as_ref()
            .is_none_or(|m| m.ink != ink || m.accent != accent)
        {
            self.markup = Some(Markup {
                ink,
                accent,
                html: sandbox(&self.body).into(),
            });
        }
        self.markup.as_ref().unwrap().html.clone()
    }
    pub(crate) fn set_interactive(&mut self, interactive: bool) {
        if self.interactive != interactive {
            self.interactive = interactive;
        }
    }
    pub(crate) fn hide(&self) {
        #[cfg(target_os = "macos")]
        if let Some(view) = self.view.borrow().as_ref() {
            view.clip.setHidden(true);
        }
    }
}
/// Loads `markup` as an opaque page, or the file page with read access to its folder only.
#[cfg(target_os = "macos")]
fn load_page(web: &WKWebView, markup: &str, file: Option<&FilePage>) {
    unsafe {
        match file {
            Some(file) => {
                let url = NSURL::fileURLWithPath(&NSString::from_str(&file.path.to_string_lossy()));
                let root =
                    NSURL::fileURLWithPath(&NSString::from_str(&file.root.to_string_lossy()));
                web.loadFileURL_allowingReadAccessToURL(&url, &root);
            }
            None => {
                web.loadHTMLString_baseURL(&NSString::from_str(markup), None);
            }
        }
    }
}
/// Content rules that block every remote load (http, https, ws, wss, ftp). A file shown as a
/// preview runs no script, but its images, stylesheets and fonts would still be fetched from the
/// network, which tells the page's author that (and when) the file was opened.
#[cfg(target_os = "macos")]
const BLOCK_REMOTE_RULES: &str =
    r#"[{"trigger":{"url-filter":"^(https?|wss?|ftp)://"},"action":{"type":"block"}}]"#;

/// Loads `markup` unless the remote-load rules are still pending or failed. While pending, the
/// markup is only remembered: the completion of [`load_page_without_remote`] loads the latest.
#[cfg(target_os = "macos")]
fn request_load(
    web: &WKWebView,
    gate: &RefCell<LoadGate>,
    markup: &Arc<str>,
    file: Option<&FilePage>,
) {
    let mut gate = gate.borrow_mut();
    gate.latest = markup.clone();
    if gate.rules.allows_load() {
        load_page(web, markup, file);
    }
}

/// Installs the remote-load rules in `web`, then loads the gate's latest markup. Compiling a
/// rule list is asynchronous, so nothing loads before its completion (see [`request_load`]);
/// when the rules cannot be had, the page stays blank rather than load without them.
#[cfg(target_os = "macos")]
fn load_page_without_remote(
    web: &Retained<WKWebView>,
    mtm: MainThreadMarker,
    gate: Rc<RefCell<LoadGate>>,
    file: Option<Rc<FilePage>>,
) {
    let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else {
        gate.borrow_mut().rules = Rules::Failed;
        return;
    };
    gate.borrow_mut().rules = Rules::Pending;
    let web = web.clone();
    let done = block2::RcBlock::new(move |list: *mut WKContentRuleList, _: *mut NSError| {
        // SAFETY: WebKit passes a live list, or null with an error.
        let Some(list) = (unsafe { list.as_ref() }) else {
            gate.borrow_mut().rules = Rules::Failed;
            return;
        };
        unsafe {
            web.configuration()
                .userContentController()
                .addContentRuleList(list)
        };
        let latest = {
            let mut gate = gate.borrow_mut();
            gate.rules = Rules::Ready;
            gate.latest.clone()
        };
        load_page(&web, &latest, file.as_deref());
    });
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(&NSString::from_str("monoptah-no-remote-loads")),
            Some(&NSString::from_str(BLOCK_REMOTE_RULES)),
            Some(&done),
        );
    }
}
impl gpui::Render for Html {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            let view = self.view.clone();
            let html = self.markup();
            let file = self.file.clone();
            let app = cx.to_async();
            let interactive = self.interactive;
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let Ok(handle) = HasWindowHandle::window_handle(window) else {
                        return;
                    };
                    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                        return;
                    };
                    // GPUI keeps its AppKit content view alive throughout paint.
                    let parent = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
                    // AppKit frames are in window points; GPUI's are divided
                    // by the interface zoom.
                    let zoom = window.content_zoom();
                    let clipped = bounds
                        .intersect(&window.content_mask().bounds)
                        .map(|v| v * zoom);
                    let bounds = bounds.map(|v| v * zoom);
                    let mut view = view.borrow_mut();
                    if clipped.size.width <= gpui::px(0.) || clipped.size.height <= gpui::px(0.) {
                        if let Some(view) = view.as_ref() {
                            view.clip.setHidden(true);
                        }
                        return;
                    }
                    let x = f64::from(clipped.origin.x);
                    let y = f64::from(clipped.origin.y);
                    let clip_width = f64::from(clipped.size.width);
                    let clip_height = f64::from(clipped.size.height);
                    let width = f64::from(bounds.size.width);
                    let height = f64::from(bounds.size.height);
                    let y = if parent.isFlipped() {
                        y
                    } else {
                        parent.bounds().size.height - y - clip_height
                    };
                    let rect =
                        NSRect::new(NSPoint::new(x, y), NSSize::new(clip_width, clip_height));
                    // Keep the document at its full layout size while its
                    // native host follows GPUI's moving content mask.
                    let web_rect = NSRect::new(
                        NSPoint::new(
                            f64::from(bounds.left() - clipped.left()),
                            f64::from(bounds.top() - clipped.top()),
                        ),
                        NSSize::new(width, height),
                    );
                    if view.is_none() {
                        let Some(mtm) = MainThreadMarker::new() else {
                            return;
                        };
                        // No message handlers, file base URL,
                        // app credentials or persistent website data are installed.
                        let hosted = unsafe {
                            let clip = ArtifactClip::alloc(mtm)
                                .set_ivars(std::cell::Cell::new(interactive));
                            let clip: Retained<ArtifactClip> =
                                msg_send![super(clip), initWithFrame: rect];
                            clip.setWantsLayer(true);
                            clip.setClipsToBounds(true);
                            let config = WKWebViewConfiguration::new(mtm);
                            config
                                .preferences()
                                .setJavaScriptCanOpenWindowsAutomatically(false);
                            if file.as_ref().is_some_and(|file| !file.scripts) {
                                config
                                    .defaultWebpagePreferences()
                                    .setAllowsContentJavaScript(false);
                            }
                            config.setWebsiteDataStore(
                                &WKWebsiteDataStore::nonPersistentDataStore(mtm),
                            );
                            let web = WKWebView::initWithFrame_configuration(
                                WKWebView::alloc(mtm),
                                web_rect,
                                &config,
                            );
                            let guard = NavigationGuard::alloc(mtm).set_ivars(GuardState {
                                app: app.clone(),
                                file_root: file
                                    .as_ref()
                                    .map(|file| file.root.to_string_lossy().into_owned()),
                            });
                            let guard: Retained<NavigationGuard> = msg_send![super(guard), init];
                            web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*guard)));
                            let remote_blocked = file.as_ref().is_some_and(|file| !file.scripts);
                            let gate = Rc::new(RefCell::new(LoadGate {
                                rules: if remote_blocked {
                                    Rules::Pending
                                } else {
                                    Rules::NotNeeded
                                },
                                latest: html.clone(),
                            }));
                            if remote_blocked {
                                load_page_without_remote(&web, mtm, gate.clone(), file.clone());
                            } else {
                                load_page(&web, &html, file.as_deref());
                            }
                            clip.addSubview(&web);
                            Hosted {
                                clip,
                                web,
                                _guard: guard,
                                markup: html.clone(),
                                gate,
                            }
                        };
                        *view = Some(hosted);
                    }
                    if let Some(view) = view.as_mut() {
                        if !Arc::ptr_eq(&view.markup, &html) {
                            request_load(&view.web, &view.gate, &html, file.as_deref());
                            view.markup = html.clone();
                        }
                        parent.addSubview_positioned_relativeTo(
                            &view.clip,
                            NSWindowOrderingMode::Above,
                            None,
                        );
                        view.clip.ivars().set(interactive);
                        view.clip.setFrame(rect);
                        view.web.setFrame(web_rect);
                        unsafe { view.web.setPageZoom(f64::from(zoom)) };
                        view.clip.setHidden(false);
                    }
                },
            )
            .size_full()
            .into_any_element()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cx;
            div().p_4().child(self.body.clone()).into_any_element()
        }
    }
}
#[cfg(target_os = "macos")]
impl Drop for Html {
    fn drop(&mut self) {
        if let Some(view) = self.view.borrow_mut().take() {
            view.clip.removeFromSuperview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    #[test]
    fn no_page_loads_while_the_remote_load_rules_are_pending_or_failed() {
        assert!(Rules::NotNeeded.allows_load());
        assert!(Rules::Ready.allows_load());
        assert!(!Rules::Pending.allows_load());
        assert!(!Rules::Failed.allows_load());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_page_runs_scripts_only_until_it_is_made_a_preview() {
        let page = Html::from_file("/tmp/a.html".into(), "/tmp".into());
        assert!(page.runs_scripts());
        assert!(!page.without_scripts().runs_scripts());
        // Generated markup keeps its own sandbox and is not a file page.
        assert!(Html::new("<p>".into()).runs_scripts());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn the_remote_load_rules_are_json_that_blocks_every_network_scheme() {
        let rules: serde_json::Value = serde_json::from_str(BLOCK_REMOTE_RULES).unwrap();
        assert_eq!(rules[0]["action"]["type"], "block");
        let filter = rules[0]["trigger"]["url-filter"].as_str().unwrap();
        for scheme in ["http", "https", "ws", "wss", "ftp"] {
            assert!(filter.contains(scheme), "{scheme} is not blocked: {filter}");
        }
    }
    #[test]
    fn generated_markup_cannot_escape_the_opaque_iframe() {
        let html = sandbox("\"><script>window.top.postMessage('x','*')</script>");
        assert!(html.contains("sandbox=\"allow-scripts\""));
        assert!(!html.contains("allow-same-origin"));
        assert!(html.contains("connect-src 'none'"));
        assert!(html.contains("&lt;script&gt;"));
    }
}
