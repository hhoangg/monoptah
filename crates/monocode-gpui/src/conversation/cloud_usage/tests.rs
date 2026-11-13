use super::*;
use gpui::{TestAppContext, VisualTestContext, WindowHandle};
use monocode_core::codespaces::MachineType;
use monocode_core::codespaces::usage::parse_summary;
use monocode_core::store::Store;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const SAMPLE: &str = r#"{"timePeriod":{"year":2026,"month":11},"user":"octo","usageItems":[
  {"product":"Codespaces","sku":"codespaces_compute_d4","unitType":"hours","grossQuantity":1.45,
   "pricePerUnit":0.36,"grossAmount":0.52,"discountAmount":0.52,"netAmount":0},
  {"product":"Codespaces","sku":"codespaces_storage","unitType":"gigabyte-months","grossQuantity":2.5,
   "grossAmount":0.18,"discountAmount":0.18,"netAmount":0},
  {"product":"Actions","sku":"actions_linux","unitType":"minutes","grossQuantity":600,
   "grossAmount":4.8,"discountAmount":4.8,"netAmount":0}]}"#;

fn usage(plan: &str) -> CodespacesUsage {
    parse_summary(SAMPLE, Some(plan.into())).unwrap()
}

#[test]
fn the_line_states_core_hours_free_usage_and_billed() {
    // $0.52 of compute at $0.09 is 5.8 core-hours (1.45 machine-hours of a 4-core machine).
    let text = usage_text(&usage("free"), Some(2));
    assert_eq!(
        text.facts,
        "This month: 5.8 of 120 core-hours, $0.70 of free usage used, $0.00 billed"
    );
    assert_eq!(text.billed, None);
    let other = usage_text(&usage("team"), Some(2));
    assert_eq!(
        other.facts,
        "This month: 5.8 core-hours, $0.70 of free usage used, $0.00 billed"
    );
    assert_eq!(
        other.estimate, None,
        "no estimate when the allowance is unknown"
    );
}

#[test]
fn a_billed_amount_gets_its_own_part_and_leaves_the_zero_out() {
    let mut paid = usage("free");
    paid.net = 2.2;
    let text = usage_text(&paid, None);
    assert_eq!(text.billed.as_deref(), Some("$2.20 billed to your account"));
    assert!(!text.facts.contains("billed"), "{}", text.facts);
}

#[test]
fn the_estimate_is_labelled_and_needs_a_known_plan_and_machine() {
    // (180 - 5.78) core-hours over 2 cores = 87 hours; Actions usage takes nothing from it.
    let text = usage_text(&usage("pro"), Some(2));
    assert_eq!(
        text.estimate.as_deref(),
        Some("Estimate: ~87 h left on a 2-core machine (assumes 180 core-hours included with Pro)")
    );
    // Free has a known allowance too: (120 - 5.78) / 4.
    let free = usage_text(&usage("free"), Some(4)).estimate.unwrap();
    assert!(
        free.starts_with("Estimate: ~29 h left on a 4-core machine"),
        "{free}"
    );
    assert!(free.contains("120 core-hours included with Free"), "{free}");
    assert_eq!(usage_text(&usage("pro"), None).estimate, None);
    assert_eq!(usage_text(&usage("team"), Some(2)).estimate, None);
}

#[test]
fn the_tooltip_explains_the_units_and_flags_the_allowance_as_unverified() {
    let pro = usage_tooltip(Some(&usage("pro")));
    assert!(pro.contains("unverified"));
    assert!(pro.contains("core-hours"), "{pro}");
    assert!(pro.contains("1.4 machine-hours"), "{pro}");
    assert!(
        pro.contains("Actions minutes are a separate allowance"),
        "{pro}"
    );
    let team = usage_tooltip(Some(&usage("team")));
    assert!(!team.contains("unverified"));
    assert!(team.contains("billing page is authoritative"));
    assert!(usage_tooltip(None).contains("authoritative"));
}

fn machine(name: &str, cpus: u32) -> MachineType {
    MachineType {
        name: name.into(),
        display_name: name.into(),
        cpus,
        ..Default::default()
    }
}

fn conversation(cx: &mut TestAppContext) -> WindowHandle<Conversation> {
    cx.update(|cx| cx.set_reduce_motion(true));
    let store = Store::open(Path::new(":memory:")).unwrap();
    let mut summary = store.create("claude", "/work/widgets").unwrap();
    summary.has_messages = false;
    cx.update(|cx| {
        input::bind(cx);
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut view = Conversation::new(summary, Arc::new(Mutex::new(store)), cx);
                view.loaded = true;
                view
            })
        })
        .unwrap()
    })
}

/// A loader that counts its calls, remembers the `force` flags and answers `result`.
fn counting(
    result: Result<CodespacesUsage, UsageProblem>,
) -> (UsageLoader, Arc<AtomicUsize>, Arc<Mutex<Vec<bool>>>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let flags = Arc::new(Mutex::new(Vec::new()));
    let (seen, sink) = (calls.clone(), flags.clone());
    let loader: UsageLoader = Arc::new(move |force| {
        seen.fetch_add(1, Ordering::SeqCst);
        sink.lock().unwrap().push(force);
        result.clone()
    });
    (loader, calls, flags)
}

fn draw(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    visual.refresh().unwrap();
}

fn open_codespace(
    window: WindowHandle<Conversation>,
    loader: UsageLoader,
    visual: &mut VisualTestContext,
) {
    window
        .update(visual, |view, _, cx| {
            view.set_usage_loader(loader);
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            // Without a loader the machine listing is "failed"; give the panel a list so the
            // selected machine has a size.
            view.cloud_draft.as_mut().unwrap().machines =
                Machines::Loaded(vec![machine("big", 8), machine("small", 2)]);
        })
        .unwrap();
    draw(visual);
}

#[gpui::test]
fn the_panel_asks_once_off_the_spot_and_shows_the_line(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let (loader, calls, flags) = counting(Ok(usage("pro")));
    window
        .update(visual, |view, _, cx| {
            view.set_usage_loader(loader);
            view.set_offers_codespace(true, cx);
            view.cloud_menu_choice("workspace:cloud-codespace", cx);
            // Nothing runs inside the click.
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(view.cloud_usage.view, UsageView::Loading);
        })
        .unwrap();
    draw(visual);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *flags.lock().unwrap(),
        [false],
        "a cached answer is fine on open"
    );
    assert!(visual.debug_bounds("cloud-usage-text").is_some());
    assert!(visual.debug_bounds("cloud-usage-billed").is_none());
    // More frames, typing and a Send do not ask again.
    for _ in 0..3 {
        draw(visual);
    }
    window
        .update(visual, |view, _, cx| {
            view.create_cloud(cx);
        })
        .unwrap();
    draw(visual);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn refresh_forces_a_new_read(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let (loader, calls, flags) = counting(Ok(usage("pro")));
    open_codespace(window, loader, visual);
    window
        .update(visual, |view, _, cx| view.load_codespace_usage(true, cx))
        .unwrap();
    draw(visual);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(*flags.lock().unwrap(), [false, true]);
}

#[gpui::test]
fn a_billed_amount_shows_the_warning_part(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let mut paid = usage("free");
    paid.net = 1.5;
    open_codespace(window, counting(Ok(paid)).0, visual);
    assert!(visual.debug_bounds("cloud-usage-billed").is_some());
}

#[gpui::test]
fn the_estimate_follows_the_selected_machine(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_codespace(window, counting(Ok(usage("pro"))).0, visual);
    assert!(visual.debug_bounds("cloud-usage-estimate").is_some());
    window
        .update(visual, |view, _, _| {
            let draft = view.cloud_draft.as_ref().unwrap();
            // No pick: the smallest listed machine.
            assert_eq!(selected_cores(draft), Some(2));
            let mut view_cores = None;
            if let Some(draft) = view.cloud_draft.as_mut() {
                draft.machine = Some("big".into());
                view_cores = selected_cores(draft);
            }
            assert_eq!(view_cores, Some(8));
        })
        .unwrap();
    // An unknown plan (Team): the facts stay, the estimate goes.
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_codespace(window, counting(Ok(usage("team"))).0, visual);
    assert!(visual.debug_bounds("cloud-usage-text").is_some());
    assert!(visual.debug_bounds("cloud-usage-estimate").is_none());
}

#[gpui::test]
fn a_missing_scope_degrades_to_a_hint_with_the_exact_command(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_codespace(window, counting(Err(UsageProblem::MissingScope)).0, visual);
    window
        .update(visual, |view, _, _| {
            let UsageView::Unavailable(problem) = &view.cloud_usage.view else {
                panic!("expected a hint, got {:?}", view.cloud_usage.view);
            };
            assert!(
                problem
                    .hint()
                    .contains("gh auth refresh -h github.com -s user")
            );
            // Send is not held back.
            assert!(!view.cloud_creating());
        })
        .unwrap();
    assert!(visual.debug_bounds("cloud-usage-text").is_some());
    assert!(visual.debug_bounds("cloud-usage-billed").is_none());
}

#[gpui::test]
fn a_bare_404_hint_offers_no_refresh_command(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_codespace(window, counting(Err(UsageProblem::NotAvailable)).0, visual);
    window
        .update(visual, |view, _, _| {
            let UsageView::Unavailable(problem) = &view.cloud_usage.view else {
                panic!("expected a hint, got {:?}", view.cloud_usage.view);
            };
            assert!(!problem.hint().contains("gh auth refresh"));
        })
        .unwrap();
    assert!(visual.debug_bounds("cloud-usage-text").is_some());
}

#[gpui::test]
fn the_usage_line_sits_after_the_machine_note(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    open_codespace(window, counting(Ok(usage("pro"))).0, visual);
    window
        .update(visual, |view, _, cx| {
            view.cloud_draft.as_mut().unwrap().machine_note =
                Some("Machine changed: small is not offered here".into());
            cx.notify();
        })
        .unwrap();
    draw(visual);
    let machine = visual.debug_bounds("cloud-row-machine").unwrap();
    let note = visual.debug_bounds("cloud-machine-note").unwrap();
    let usage = visual.debug_bounds("cloud-usage").unwrap();
    assert!(note.top() >= machine.bottom(), "{note:?} {machine:?}");
    assert!(usage.top() >= note.bottom(), "{usage:?} {note:?}");
    // Same indent as the note.
    assert_eq!(usage.left(), note.left());
}

#[gpui::test]
fn docker_sandbox_has_no_usage_line_and_asks_nothing(cx: &mut TestAppContext) {
    let window = conversation(cx);
    let visual = &mut VisualTestContext::from_window(window.into(), cx);
    let (loader, calls, _) = counting(Ok(usage("pro")));
    window
        .update(visual, |view, _, cx| {
            view.set_usage_loader(loader);
            let config = monocode_core::cloud_sessions::docker_config::DockerProjectConfig {
                repo_url: "https://github.com/octo/widgets.git".into(),
                ..Default::default()
            };
            monocode_core::cloud_sessions::docker_config::save(
                &locked(&view.store).unwrap(),
                &view.summary.cwd,
                &config,
            )
            .unwrap();
            view.forget_docker_config();
            view.cloud_menu_choice("workspace:cloud-docker", cx);
            assert_eq!(view.cloud_target(), Some(CloudTarget::DockerSandbox));
        })
        .unwrap();
    draw(visual);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(visual.debug_bounds("cloud-usage").is_none());
}
