//! Port of src/features/settings/ui/TicketScoringSettings.tsx: what scores the
//! closed ClickUp tickets of the Contributors leaderboard (Claude Code or the
//! Anthropic API), the model and the API key. The saved key is never read back:
//! the field only ever holds a new one, and nothing here logs it.
use crate::{
    input::{self, Input},
    settings::{
        SettingsView,
        widgets::{note, red, secondary_button},
    },
    theme::*,
};
use gpui::{AnyElement, Context, Div, Role, SharedString, div, prelude::*, px};
use monocode_core::contributors::{api, assessor::AiConfigStatus};
use std::{path::PathBuf, sync::Arc};

const MODEL: &str = "ticket-scoring-model";
const KEY: &str = "ticket-scoring-key";

const RUNNERS: [(&str, &str); 2] = [
    ("claude-code", "Claude Code (your subscription)"),
    ("api", "Anthropic API key"),
];

/// Where the AI settings are read and written. Every call blocks on disk, so
/// the page makes it on the background executor.
pub(in crate::settings) trait ScoringBackend: Send + Sync {
    fn get(&self) -> Result<AiConfigStatus, String>;
    /// `api_key` of `None` keeps the stored key, `Some("")` deletes it.
    fn set(
        &self,
        runner: &str,
        model: &str,
        api_key: Option<&str>,
    ) -> Result<AiConfigStatus, String>;
}

/// The core's `contributors-ai.json` in the app data folder.
struct DataDir(PathBuf);
impl ScoringBackend for DataDir {
    fn get(&self) -> Result<AiConfigStatus, String> {
        api::ai_config_get_at(&self.0)
    }

    fn set(
        &self,
        runner: &str,
        model: &str,
        api_key: Option<&str>,
    ) -> Result<AiConfigStatus, String> {
        api::ai_config_set_at(&self.0, runner, model, api_key)
    }
}

pub(in crate::settings) struct ScoringState {
    pub(in crate::settings) backend: Option<Arc<dyn ScoringBackend>>,
    runner: String,
    has_key: bool,
    /// The saved settings are being read.
    checking: bool,
    busy: bool,
    error: Option<String>,
    requested: bool,
    fields_ready: bool,
}
impl Default for ScoringState {
    fn default() -> Self {
        Self {
            backend: None,
            runner: "claude-code".into(),
            has_key: false,
            checking: true,
            busy: false,
            error: None,
            requested: false,
            fields_ready: false,
        }
    }
}

impl SettingsView {
    fn scoring_backend(&mut self) -> Option<Arc<dyn ScoringBackend>> {
        if self.ticket_scoring.backend.is_none()
            && let Some(dir) = self.ctx.data_dir.clone()
        {
            self.ticket_scoring.backend = Some(Arc::new(DataDir(dir)));
        }
        self.ticket_scoring.backend.clone()
    }

    fn ensure_scoring_fields(&mut self, cx: &mut Context<Self>) {
        if self.ticket_scoring.fields_ready {
            return;
        }
        self.ticket_scoring.fields_ready = true;
        for (id, label, placeholder, secret) in [
            (MODEL, "Model", "claude-opus-5-5", false),
            (KEY, "API key", "sk-ant-...", true),
        ] {
            let field = cx.new(|cx| {
                let mut field = Input::field(label, placeholder, content(0.35), cx);
                field.set_font_size(13.);
                field.set_secret(secret);
                field
            });
            self._subscriptions
                .push(cx.subscribe(&field, |_, _, _: &input::Changed, cx| cx.notify()));
            self._subscriptions
                .push(cx.subscribe(&field, |this, _, _: &input::Submitted, cx| {
                    this.submit_ticket_scoring(cx)
                }));
            self.fields.insert(id, field);
        }
    }

    fn scoring_text(&self, id: &'static str, cx: &gpui::App) -> String {
        self.fields
            .get(id)
            .map(|field| field.read(cx).content.to_string())
            .unwrap_or_default()
    }

    fn set_scoring_text(&self, id: &'static str, text: String, cx: &mut Context<Self>) {
        if let Some(field) = self.fields.get(id) {
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
    }

    /// Reads the saved settings, once, when the Inbox page is first shown.
    pub(in crate::settings) fn load_ticket_scoring(&mut self, cx: &mut Context<Self>) {
        if self.ticket_scoring.requested {
            return;
        }
        let Some(backend) = self.scoring_backend() else {
            return;
        };
        self.ticket_scoring.requested = true;
        self.ensure_scoring_fields(cx);
        let work = cx.background_executor().spawn(async move { backend.get() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(config) => this.apply_scoring(&config, cx),
                    Err(error) => this.ticket_scoring.error = Some(error),
                }
                this.ticket_scoring.checking = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// What was saved is what the page shows; the key field is emptied.
    fn apply_scoring(&mut self, config: &AiConfigStatus, cx: &mut Context<Self>) {
        self.ticket_scoring.runner = config.runner.clone();
        self.ticket_scoring.has_key = config.has_api_key;
        self.set_scoring_text(MODEL, config.model.clone(), cx);
    }

    fn set_scoring_runner(&mut self, runner: &str, cx: &mut Context<Self>) {
        let state = &mut self.ticket_scoring;
        if state.busy || state.checking || state.runner == runner {
            return;
        }
        state.runner = runner.to_owned();
        cx.notify();
    }

    /// Save (the button, Enter in a field): a typed key replaces the saved one,
    /// an empty field keeps it.
    pub(in crate::settings) fn submit_ticket_scoring(&mut self, cx: &mut Context<Self>) {
        let typed = self.scoring_text(KEY, cx).trim().to_owned();
        self.save_ticket_scoring((!typed.is_empty()).then_some(typed), cx);
    }

    fn save_ticket_scoring(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        let model = self.scoring_text(MODEL, cx).trim().to_owned();
        let state = &mut self.ticket_scoring;
        if state.busy || state.checking {
            return;
        }
        let Some(backend) = state.backend.clone() else {
            return;
        };
        state.busy = true;
        state.error = None;
        let runner = state.runner.clone();
        let work = cx
            .background_executor()
            .spawn(async move { backend.set(&runner, &model, key.as_deref()) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.ticket_scoring.busy = false;
                match result {
                    Ok(saved) => {
                        this.apply_scoring(&saved, cx);
                        this.set_scoring_text(KEY, String::new(), cx);
                    }
                    Err(error) => this.ticket_scoring.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn scoring_field(
        &mut self,
        id: &'static str,
        label: &'static str,
        off: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let field = self.fields[id].clone();
        field.update(cx, |field, _| field.set_read_only(off));
        div()
            .debug_selector(move || id.to_owned())
            .flex()
            .flex_col()
            .gap_1()
            .text_size(px(12.))
            .text_color(content(0.65))
            .child(label)
            .child(
                div()
                    .h(px(32.))
                    .min_w_0()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_color(content(1.))
                    .when(off, |d| d.opacity(0.4))
                    .child(field),
            )
    }

    fn scoring_runner_option(
        &self,
        id: &'static str,
        label: &'static str,
        off: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.ticket_scoring.runner == id;
        div()
            .id(SharedString::from(format!("ticket-scoring-runner-{id}")))
            .debug_selector(move || format!("ticket-scoring-runner-{id}"))
            .role(Role::RadioButton)
            .aria_label(label)
            .aria_selected(selected)
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(13.))
            .text_color(content(1.))
            .when(off, |d| d.opacity(0.4))
            .when(!off, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_scoring_runner(id, cx)))
            })
            .child(
                div()
                    .size(px(14.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(content(if selected { 0.7 } else { 0.3 }))
                    .when(selected, |d| {
                        d.child(div().size(px(8.)).rounded_full().bg(accent_color()))
                    }),
            )
            .child(label)
            .into_any_element()
    }

    /// The Ticket scoring group of the Inbox page.
    pub(in crate::settings) fn ticket_scoring_group(
        &mut self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = self.ticket_scoring_body(cx);
        self.group(
            Some("ticket-scoring"),
            false,
            "Ticket scoring",
            Some("The AI runner that scores closed ClickUp tickets.".into()),
            None,
            vec![body],
        )
    }

    fn ticket_scoring_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.scoring_backend().is_none() {
            return note("Ticket scoring settings need the app data folder.");
        }
        self.load_ticket_scoring(cx);
        self.ensure_scoring_fields(cx);
        let off = self.ticket_scoring.busy || self.ticket_scoring.checking;
        let api_runner = self.ticket_scoring.runner == "api";
        let model = self.scoring_field(MODEL, "Model", off, cx);
        let options: Vec<AnyElement> = RUNNERS
            .into_iter()
            .map(|(id, label)| self.scoring_runner_option(id, label, off, cx))
            .collect();
        let mut form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(19.5))
                    .text_color(content(0.45))
                    .child("Choose what scores closed ClickUp tickets on the leaderboard."),
            )
            .child(
                div()
                    .id("ticket-scoring-runner")
                    .role(Role::RadioGroup)
                    .aria_label("Runner")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(options),
            )
            .child(model)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.45))
                    .child("claude-haiku-5-5 costs far less per ticket"),
            );
        if api_runner {
            let key = self.scoring_field(KEY, "Anthropic API key", off, cx);
            form = form.child(key);
            if self.ticket_scoring.has_key {
                form = form.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child("Key saved")
                        .child(
                            div()
                                .id("ticket-scoring-remove-key")
                                .debug_selector(|| "ticket-scoring-remove-key".to_owned())
                                .role(Role::Button)
                                .aria_label("Remove key")
                                .text_color(content(0.65))
                                .when(self.ticket_scoring.busy, |d| d.opacity(0.4))
                                .when(!self.ticket_scoring.busy, |d| {
                                    d.cursor_pointer()
                                        .hover(|s| s.text_color(content(1.)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.save_ticket_scoring(Some(String::new()), cx)
                                        }))
                                })
                                .child("Remove key"),
                        ),
                );
            }
        }
        let label = if self.ticket_scoring.busy {
            "Saving"
        } else {
            "Save"
        };
        form = form.child(
            div().child(
                secondary_button("ticket-scoring-save", label, false, off)
                    .debug_selector(|| "ticket-scoring-save".to_owned())
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !off {
                            this.submit_ticket_scoring(cx)
                        }
                    })),
            ),
        );
        let mut body = div().px_4().py(px(14.)).child(form);
        if let Some(error) = &self.ticket_scoring.error {
            body = body.child(
                div()
                    .id("ticket-scoring-error")
                    .debug_selector(|| "ticket-scoring-error".to_owned())
                    .role(Role::Alert)
                    .mt_3()
                    .text_size(px(12.))
                    .text_color(red(0.9))
                    .child(error.clone()),
            );
        }
        body.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Preferences, Section, SettingsContext};
    use gpui::{
        Bounds, TestAppContext, VisualTestContext, WindowBounds, WindowHandle, WindowOptions,
        point, size,
    };
    use std::sync::Mutex;

    /// The AI settings as the core keeps them, and every call made on them.
    struct Fake {
        stored: Mutex<AiConfigStatus>,
        calls: Mutex<Vec<(String, String, Option<String>)>>,
        fail: Mutex<Option<String>>,
    }
    impl Fake {
        fn new(runner: &str, model: &str, has_api_key: bool) -> Arc<Self> {
            Arc::new(Self {
                stored: Mutex::new(AiConfigStatus {
                    runner: runner.into(),
                    model: model.into(),
                    has_api_key,
                }),
                calls: Mutex::default(),
                fail: Mutex::default(),
            })
        }

        fn calls(&self) -> Vec<(String, String, Option<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl ScoringBackend for Fake {
        fn get(&self) -> Result<AiConfigStatus, String> {
            Ok(self.stored.lock().unwrap().clone())
        }

        fn set(
            &self,
            runner: &str,
            model: &str,
            api_key: Option<&str>,
        ) -> Result<AiConfigStatus, String> {
            self.calls.lock().unwrap().push((
                runner.into(),
                model.into(),
                api_key.map(str::to_owned),
            ));
            if let Some(error) = self.fail.lock().unwrap().clone() {
                return Err(error);
            }
            let mut stored = self.stored.lock().unwrap();
            stored.runner = runner.into();
            stored.model = model.into();
            stored.has_api_key = match api_key {
                None => stored.has_api_key,
                Some(key) => !key.is_empty(),
            };
            Ok(stored.clone())
        }
    }

    fn open(
        cx: &mut TestAppContext,
        fake: Option<Arc<Fake>>,
    ) -> (WindowHandle<SettingsView>, VisualTestContext) {
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(1200.), px(4000.)),
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let mut view = SettingsView::new(
                            Section::Inbox,
                            Preferences {
                                notifications: false,
                                ..Default::default()
                            },
                            SettingsContext::default(),
                            cx,
                        );
                        view.discovery_task = None;
                        view.discovery_loading = false;
                        if let Some(fake) = fake {
                            view.ticket_scoring.backend = Some(fake);
                        }
                        view
                    })
                },
            )
            .unwrap()
        });
        let visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // A frame draws the page, which reads the saved settings; then they land.
        visual.run_until_parked();
        (window, visual)
    }

    fn read<R>(
        window: WindowHandle<SettingsView>,
        v: &mut VisualTestContext,
        f: impl FnOnce(&SettingsView, &gpui::App) -> R,
    ) -> R {
        window.read_with(v, |view, cx| f(view, cx)).unwrap()
    }

    fn type_into(
        window: WindowHandle<SettingsView>,
        v: &mut VisualTestContext,
        field: &'static str,
        text: &str,
    ) {
        let text = text.to_owned();
        window
            .update(v, |view, _, cx| view.set_scoring_text(field, text, cx))
            .unwrap();
        v.run_until_parked();
    }

    fn click(v: &mut VisualTestContext, selector: &'static str) {
        let bounds = v
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"));
        v.simulate_click(bounds.center(), Default::default());
        v.run_until_parked();
    }

    fn shown(v: &mut VisualTestContext, selector: &'static str) -> bool {
        v.debug_bounds(selector).is_some()
    }

    #[gpui::test]
    fn the_key_field_is_only_offered_for_the_api_runner(cx: &mut TestAppContext) {
        let (window, mut v) = open(cx, Some(Fake::new("claude-code", "", false)));
        assert!(shown(&mut v, MODEL));
        assert!(!shown(&mut v, KEY));
        assert!(shown(&mut v, "ticket-scoring-save"));
        let placeholder = read(window, &mut v, |view, cx| {
            view.fields[MODEL].read(cx).placeholder().to_owned()
        });
        assert_eq!(placeholder, "claude-opus-5-5");
        assert_eq!(
            read(window, &mut v, |view, _| view.ticket_scoring.runner.clone()),
            "claude-code"
        );
        click(&mut v, "ticket-scoring-runner-api");
        assert!(shown(&mut v, KEY));
        let secret = read(window, &mut v, |view, cx| {
            view.fields[KEY].read(cx).is_secret()
        });
        assert!(secret, "the key is masked");
        click(&mut v, "ticket-scoring-runner-claude-code");
        assert!(!shown(&mut v, KEY));
    }

    #[gpui::test]
    fn the_saved_settings_fill_the_page(cx: &mut TestAppContext) {
        let (window, mut v) = open(cx, Some(Fake::new("api", "claude-haiku-5-5", true)));
        assert_eq!(
            read(window, &mut v, |view, cx| view.scoring_text(MODEL, cx)),
            "claude-haiku-5-5"
        );
        assert!(shown(&mut v, KEY));
        assert!(shown(&mut v, "ticket-scoring-remove-key"), "Key saved");
        assert_eq!(
            read(window, &mut v, |view, cx| view.scoring_text(KEY, cx)),
            "",
            "the saved key is never put in the field"
        );
    }

    #[gpui::test]
    fn saving_keeps_the_stored_key_when_the_field_is_left_empty(cx: &mut TestAppContext) {
        let fake = Fake::new("api", "claude-opus-5-5", true);
        let (window, mut v) = open(cx, Some(fake.clone()));
        type_into(window, &mut v, MODEL, " claude-haiku-5-5 ");
        click(&mut v, "ticket-scoring-save");
        assert_eq!(
            fake.calls(),
            [("api".into(), "claude-haiku-5-5".into(), None)]
        );
        assert!(shown(&mut v, "ticket-scoring-remove-key"), "Key saved");
        assert_eq!(
            read(window, &mut v, |view, cx| view.scoring_text(MODEL, cx)),
            "claude-haiku-5-5"
        );
    }

    #[gpui::test]
    fn saving_a_typed_key_sends_it_once_and_never_shows_it_back(cx: &mut TestAppContext) {
        let fake = Fake::new("claude-code", "", false);
        let (window, mut v) = open(cx, Some(fake.clone()));
        click(&mut v, "ticket-scoring-runner-api");
        assert!(!shown(&mut v, "ticket-scoring-remove-key"), "no key yet");
        type_into(window, &mut v, KEY, " sk-ant-secret ");
        click(&mut v, "ticket-scoring-save");
        assert_eq!(
            fake.calls(),
            [("api".into(), "".into(), Some("sk-ant-secret".into()))]
        );
        assert!(shown(&mut v, "ticket-scoring-remove-key"), "Key saved");
        assert_eq!(
            read(window, &mut v, |view, cx| view.scoring_text(KEY, cx)),
            ""
        );
    }

    #[gpui::test]
    fn remove_key_sends_an_empty_string(cx: &mut TestAppContext) {
        let fake = Fake::new("api", "claude-opus-5-5", true);
        let (_window, mut v) = open(cx, Some(fake.clone()));
        click(&mut v, "ticket-scoring-remove-key");
        assert_eq!(
            fake.calls(),
            [("api".into(), "claude-opus-5-5".into(), Some("".into()))]
        );
        assert!(!shown(&mut v, "ticket-scoring-remove-key"));
        assert!(shown(&mut v, KEY), "still the API runner");
    }

    #[gpui::test]
    fn a_save_failure_is_shown_and_keeps_what_was_typed(cx: &mut TestAppContext) {
        let fake = Fake::new("api", "claude-opus-5-5", false);
        let (window, mut v) = open(cx, Some(fake.clone()));
        *fake.fail.lock().unwrap() = Some("disk is full".into());
        type_into(window, &mut v, KEY, "sk-ant-typed");
        click(&mut v, "ticket-scoring-save");
        assert!(shown(&mut v, "ticket-scoring-error"));
        assert_eq!(
            read(window, &mut v, |view, _| view.ticket_scoring.error.clone()),
            Some("disk is full".to_owned())
        );
        assert_eq!(
            read(window, &mut v, |view, cx| view.scoring_text(KEY, cx)),
            "sk-ant-typed",
            "a failed save does not lose the key"
        );
        // Saving again clears the error.
        *fake.fail.lock().unwrap() = None;
        click(&mut v, "ticket-scoring-save");
        assert!(!shown(&mut v, "ticket-scoring-error"));
    }

    #[gpui::test]
    fn a_failed_read_shows_the_error_and_unlocks_the_page(cx: &mut TestAppContext) {
        struct Broken;
        impl ScoringBackend for Broken {
            fn get(&self) -> Result<AiConfigStatus, String> {
                Err("The assessment AI settings are invalid".into())
            }
            fn set(&self, _: &str, _: &str, _: Option<&str>) -> Result<AiConfigStatus, String> {
                Err("not reached".into())
            }
        }
        let window = {
            let (window, mut v) = open(cx, None);
            window
                .update(&mut v, |view, _, cx| {
                    view.ticket_scoring.backend = Some(Arc::new(Broken));
                    cx.notify();
                })
                .unwrap();
            v.run_until_parked();
            v.run_until_parked();
            (window, v)
        };
        let (window, mut v) = window;
        assert!(shown(&mut v, "ticket-scoring-error"));
        assert!(!read(window, &mut v, |view, _| view
            .ticket_scoring
            .checking));
    }

    #[gpui::test]
    fn nothing_can_be_saved_while_the_saved_settings_are_being_read(cx: &mut TestAppContext) {
        let fake = Fake::new("claude-code", "claude-opus-5-5", false);
        let (window, mut v) = open(cx, Some(fake.clone()));
        window
            .update(&mut v, |view, _, cx| {
                view.ticket_scoring.checking = true;
                view.submit_ticket_scoring(cx);
                view.set_scoring_runner("api", cx);
            })
            .unwrap();
        v.run_until_parked();
        assert_eq!(fake.calls(), []);
        assert_eq!(
            read(window, &mut v, |view, _| view.ticket_scoring.runner.clone()),
            "claude-code"
        );
    }

    #[gpui::test]
    fn enter_in_a_field_saves(cx: &mut TestAppContext) {
        let fake = Fake::new("claude-code", "claude-opus-5-5", false);
        let (window, mut v) = open(cx, Some(fake.clone()));
        let model = read(window, &mut v, |view, _| view.fields[MODEL].clone());
        model.update(&mut v, |_, cx| cx.emit(input::Submitted));
        v.run_until_parked();
        assert_eq!(
            fake.calls(),
            [("claude-code".into(), "claude-opus-5-5".into(), None)]
        );
    }

    #[gpui::test]
    fn without_a_data_folder_the_group_says_so_and_reads_nothing(cx: &mut TestAppContext) {
        let (window, mut v) = open(cx, None);
        assert!(!shown(&mut v, "ticket-scoring-save"));
        assert!(read(window, &mut v, |view, _| !view
            .ticket_scoring
            .requested));
    }

    #[gpui::test]
    fn the_group_is_in_the_inbox_page_after_clickup(cx: &mut TestAppContext) {
        let (_window, mut v) = open(cx, Some(Fake::new("claude-code", "", false)));
        let clickup = v.debug_bounds("connect-clickup").unwrap();
        let save = v.debug_bounds("ticket-scoring-save").unwrap();
        let linear = v.debug_bounds("connect-linear").unwrap();
        assert!(clickup.top() < save.top() && save.top() < linear.top());
    }

    #[test]
    fn the_search_finds_ticket_scoring_in_inbox() {
        for query in ["ticket scoring", "leaderboard", "anthropic", "haiku"] {
            let results = crate::settings::model::search(query);
            assert!(
                results
                    .iter()
                    .any(|r| r.section == Section::Inbox && r.setting == Some("ticket-scoring")),
                "{query}: {results:?}"
            );
        }
    }
}
