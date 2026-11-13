use super::*;
use std::collections::VecDeque;

const SAMPLE: &str = r#"{
  "timePeriod": {"year": 2026, "month": 11},
  "user": "octo",
  "usageItems": [
    {"product":"Codespaces","sku":"codespaces_compute_d4","unitType":"hours","grossQuantity":1.45,
     "discountQuantity":1.45,"netQuantity":0,"pricePerUnit":0.36,"grossAmount":0.52,
     "discountAmount":0.52,"netAmount":0},
    {"product":"Codespaces","sku":"codespaces_storage","unitType":"gigabyte-months","grossQuantity":2.5,
     "discountQuantity":2.5,"netQuantity":0,"pricePerUnit":0.07,"grossAmount":0.18,
     "discountAmount":0.18,"netAmount":0},
    {"product":"Actions","sku":"actions_linux","unitType":"minutes","grossQuantity":600,
     "discountQuantity":600,"netQuantity":0,"pricePerUnit":0.008,"grossAmount":4.8,
     "discountAmount":4.8,"netAmount":0},
    {"product":"Actions","sku":"actions_storage","unitType":"gigabyte-hours","grossQuantity":9,
     "pricePerUnit":0.25,"grossAmount":2.25,"discountAmount":2.25,"netAmount":0},
    {"product":"Copilot","sku":"copilot_premium_request","unitType":"requests","grossQuantity":3,
     "grossAmount":0.12,"discountAmount":0,"netAmount":0.12}
  ]
}"#;

struct Fake {
    calls: Mutex<Vec<Vec<String>>>,
    replies: Mutex<VecDeque<Result<GhOutput, String>>>,
}

impl Fake {
    fn new(replies: Vec<Result<GhOutput, String>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            replies: Mutex::new(replies.into()),
        }
    }
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl GhRunner for Fake {
    fn run(&self, args: &[String], _stdin: Option<&str>) -> Result<GhOutput, String> {
        self.calls.lock().unwrap().push(args.to_vec());
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected gh call")
    }
}

fn ok(stdout: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

fn fail(stderr: &str) -> Result<GhOutput, String> {
    Ok(GhOutput {
        success: false,
        stdout: String::new(),
        stderr: stderr.into(),
    })
}

const USER_PRO: &str = r#"{"login":"octo","plan":{"name":"pro"}}"#;

#[test]
fn the_sample_summary_is_parsed() {
    let usage = parse_summary(SAMPLE, Some("Pro".into())).unwrap();
    assert_eq!((usage.year, usage.month), (Some(2026), Some(11)));
    assert!((usage.compute_machine_hours - 1.45).abs() < 1e-9);
    // 0.52 dollars at 0.09 a core-hour: the d4 hours are 5.8 core-hours, not 1.45.
    assert!((usage.compute_core_hours - 0.52 / 0.09).abs() < 1e-9);
    assert!((usage.storage_gb_months - 2.5).abs() < 1e-9);
    assert!((usage.gross - 0.70).abs() < 1e-9);
    assert!((usage.discount - 0.70).abs() < 1e-9);
    assert_eq!(usage.net, 0.);
    assert!(!usage.is_billed());
    assert_eq!(usage.plan.as_deref(), Some("pro"));
}

#[test]
fn only_codespaces_items_count_towards_codespaces_totals() {
    let usage = parse_summary(SAMPLE, None).unwrap();
    // Actions and Copilot dollars are not Codespaces dollars.
    assert!((usage.gross - 0.70).abs() < 1e-9);
    assert_eq!(usage.net, 0.);
    // Actions minutes do not count as Codespaces compute.
    let actions_only = r#"{"usageItems":[{"product":"Actions","sku":"actions_linux",
        "grossQuantity":3000,"grossAmount":24,"discountAmount":24,"netAmount":0}]}"#;
    let usage = parse_summary(actions_only, Some("pro".into())).unwrap();
    assert_eq!(usage.compute_core_hours, 0.);
    assert_eq!(usage.compute_machine_hours, 0.);
}

#[test]
fn a_billed_amount_is_net_not_gross() {
    let text = r#"{"usageItems":[{"product":"Codespaces","sku":"codespaces_compute_d8",
        "grossQuantity":10,"grossAmount":7.2,"discountAmount":5,"netAmount":2.2}]}"#;
    let usage = parse_summary(text, None).unwrap();
    assert!(usage.is_billed());
    assert!((usage.net - 2.2).abs() < 1e-9);
    assert!((usage.gross - 7.2).abs() < 1e-9);
}

#[test]
fn unknown_skus_units_and_missing_fields_are_tolerated() {
    let text = r#"{"usageItems":[
        {"product":"Codespaces","sku":"codespaces_prebuild_storage","unitType":"mystery","grossAmount":"0.5"},
        {"product":"Codespaces","sku":"codespaces_compute_d2"},
        {"sku":7,"grossAmount":null},
        "junk", null, {"product":"Packages","sku":"packages_bandwidth","grossAmount":3}
    ]}"#;
    let usage = parse_summary(text, None).unwrap();
    assert!((usage.gross - 0.5).abs() < 1e-9, "text numbers parse");
    assert_eq!(usage.compute_machine_hours, 0.);
    assert_eq!(usage.compute_core_hours, 0.);
    assert_eq!(usage.storage_gb_months, 0.);
    assert_eq!(usage.year, None);
    // No usageItems at all is an empty month, not an error.
    assert_eq!(parse_summary("{}", None).unwrap().gross, 0.);
    assert!(matches!(
        parse_summary("[1]", None),
        Err(UsageProblem::Failed(_))
    ));
    assert!(matches!(
        parse_summary("not json", None),
        Err(UsageProblem::Failed(_))
    ));
}

#[test]
fn fetch_asks_for_the_user_then_their_summary() {
    let gh = Fake::new(vec![ok(USER_PRO), ok(SAMPLE)]);
    let usage = fetch(&gh).unwrap();
    let calls = gh.calls.lock().unwrap().clone();
    assert_eq!(calls[0], vec!["api", "user"]);
    assert_eq!(
        calls[1],
        vec!["api", "/users/octo/settings/billing/usage/summary"]
    );
    assert_eq!(usage.plan.as_deref(), Some("pro"));
}

#[test]
fn a_hostile_login_is_not_put_in_the_path() {
    let gh = Fake::new(vec![ok(r#"{"login":"../x/y"}"#)]);
    assert_eq!(fetch(&gh), Err(UsageProblem::NotLoggedIn));
    assert_eq!(gh.count(), 1);
}

#[test]
fn a_missing_scope_says_the_exact_fix() {
    for stderr in [
        "This API operation needs the \"user\" scope. To request it, run:  gh auth refresh -h github.com -s user",
        "gh: Not Found (HTTP 404)\nThis API operation needs the \"user\" scope.",
        "HTTP 403: Resource not accessible by personal access token (missing scope)",
    ] {
        let gh = Fake::new(vec![ok(USER_PRO), fail(stderr)]);
        let problem = fetch(&gh).unwrap_err();
        assert_eq!(problem, UsageProblem::MissingScope, "{stderr}");
        assert!(
            problem
                .hint()
                .contains("gh auth refresh -h github.com -s user")
        );
    }
}

#[test]
fn a_bare_404_or_403_is_not_a_missing_scope() {
    for stderr in [
        "gh: Not Found (HTTP 404)",
        "{\"message\":\"Not Found\",\"status\":\"404\"}",
        "gh: Resource not accessible by personal access token (HTTP 403)",
        "gh: Forbidden (HTTP 403)",
    ] {
        let gh = Fake::new(vec![ok(USER_PRO), fail(stderr)]);
        let problem = fetch(&gh).unwrap_err();
        assert_eq!(problem, UsageProblem::NotAvailable, "{stderr}");
        // No command that would loop the user through a refresh that changes nothing.
        let hint = problem.hint();
        assert_eq!(hint, "Usage not available for this account");
        assert!(!hint.contains("gh auth"), "{hint}");
    }
    let gh = Fake::new(vec![
        ok(USER_PRO),
        fail("gh: API rate limit exceeded (HTTP 403)"),
    ]);
    assert_eq!(fetch(&gh), Err(UsageProblem::RateLimited));
}

#[test]
fn other_failures_degrade_to_short_hints() {
    let gh = Fake::new(vec![Err("GitHub CLI (gh) is not installed".into())]);
    assert_eq!(fetch(&gh), Err(UsageProblem::GhMissing));
    let gh = Fake::new(vec![fail(
        "To get started with GitHub CLI, please run:  gh auth login",
    )]);
    assert_eq!(fetch(&gh), Err(UsageProblem::NotLoggedIn));
    let gh = Fake::new(vec![fail("error connecting to api.github.com")]);
    assert_eq!(fetch(&gh), Err(UsageProblem::Offline));
    let gh = Fake::new(vec![ok(USER_PRO), ok("<html>")]);
    assert!(matches!(fetch(&gh), Err(UsageProblem::Failed(_))));
    let long = "x".repeat(300);
    let hint = classify(&GhOutput {
        success: false,
        stdout: String::new(),
        stderr: long,
    })
    .hint();
    assert!(hint.chars().count() < 120);
}

#[test]
fn the_cache_asks_once_until_it_expires_or_is_refreshed() {
    let cache = UsageCache::default();
    let gh = Fake::new(vec![
        ok(USER_PRO),
        ok(SAMPLE),
        ok(USER_PRO),
        ok(SAMPLE),
        ok(USER_PRO),
        ok(SAMPLE),
    ]);
    let start = Instant::now();
    for _ in 0..5 {
        assert!(cache.get_at(&gh, false, start).is_ok());
    }
    assert_eq!(gh.count(), 2, "one fetch is two gh calls");
    assert!(cache
        .get_at(&gh, false, start + CACHE_TTL - Duration::from_secs(1))
        .is_ok());
    assert_eq!(gh.count(), 2);
    // An explicit refresh always asks.
    assert!(cache
        .get_at(&gh, true, start + Duration::from_secs(5))
        .is_ok());
    assert_eq!(gh.count(), 4);
    // Expired: asks again.
    assert!(cache
        .get_at(&gh, false, start + Duration::from_secs(5) + CACHE_TTL)
        .is_ok());
    assert_eq!(gh.count(), 6);
}

#[test]
fn a_failure_is_remembered_briefly_and_refresh_retries() {
    let cache = UsageCache::default();
    let gh = Fake::new(vec![
        ok(USER_PRO),
        fail("This API operation needs the \"user\" scope"),
        ok(USER_PRO),
        ok(SAMPLE),
    ]);
    let start = Instant::now();
    assert_eq!(
        cache.get_at(&gh, false, start),
        Err(UsageProblem::MissingScope)
    );
    assert_eq!(
        cache.get_at(&gh, false, start + Duration::from_secs(5)),
        Err(UsageProblem::MissingScope)
    );
    assert_eq!(gh.count(), 2, "the second call came from the cache");
    // The user fixed the scope and pressed refresh.
    assert!(cache
        .get_at(&gh, true, start + Duration::from_secs(6))
        .is_ok());
    assert_eq!(gh.count(), 4);
}

#[test]
fn the_estimate_counts_core_hours_of_compute_against_the_allowance() {
    let pro = parse_summary(SAMPLE, Some("pro".into())).unwrap();
    // Used: $0.52 of compute at $0.09 a core-hour = 5.78 core-hours of the 180.
    let used = 0.52 / 0.09;
    assert!((pro.core_hours_left().unwrap() - (180. - used)).abs() < 1e-9);
    // Actions usage (600 minutes in the sample) takes nothing from it.
    let no_actions = parse_summary(
        r#"{"usageItems":[{"product":"Codespaces","sku":"codespaces_compute_d4",
            "grossQuantity":1.45,"grossAmount":0.52}]}"#,
        Some("pro".into()),
    )
    .unwrap();
    assert_eq!(pro.core_hours_left(), no_actions.core_hours_left());
    // 2-core machine: half the core-hours left in machine hours; 4-core: a quarter.
    assert!((pro.estimated_hours_left(2).unwrap() - (180. - used) / 2.).abs() < 1e-9);
    assert!((pro.estimated_hours_left(4).unwrap() - (180. - used) / 4.).abs() < 1e-9);
    assert_eq!(pro.estimated_hours_left(0), None, "unknown machine size");
    // Free has a known allowance too.
    let free = parse_summary(SAMPLE, Some("free".into())).unwrap();
    assert!((free.estimated_hours_left(2).unwrap() - (120. - used) / 2.).abs() < 1e-9);
    assert_eq!(free.included_storage_gb_months(), Some(15.));
    assert_eq!(pro.included_storage_gb_months(), Some(20.));
    for plan in [Some("team"), Some("enterprise"), None] {
        let other = parse_summary(SAMPLE, plan.map(str::to_owned)).unwrap();
        assert_eq!(other.included_core_hours(), None);
        assert_eq!(other.core_hours_left(), None);
        assert_eq!(other.estimated_hours_left(4), None);
    }
}

#[test]
fn heavy_actions_usage_leaves_the_codespaces_allowance_alone() {
    // 3,000 Linux minutes ($24 gross, fully discounted) and no Codespaces at all: all 180
    // core-hours are left, 90 hours of a 2-core machine.
    let text = r#"{"usageItems":[{"product":"Actions","sku":"actions_linux",
        "grossQuantity":3000,"grossAmount":24,"discountAmount":24,"netAmount":0}]}"#;
    let pro = parse_summary(text, Some("pro".into())).unwrap();
    assert_eq!(pro.estimated_hours_left(2), Some(90.));
}

#[test]
fn overspent_allowance_never_goes_negative() {
    // $20 of compute = 222 core-hours, above the 180 included.
    let text = r#"{"usageItems":[{"product":"Codespaces","sku":"codespaces_compute_d4",
        "grossQuantity":55.5,"grossAmount":20,"discountAmount":16.2,"netAmount":3.8}]}"#;
    let spent = parse_summary(text, Some("pro".into())).unwrap();
    assert_eq!(spent.core_hours_left(), Some(0.));
    assert_eq!(spent.estimated_hours_left(2), Some(0.));
}
