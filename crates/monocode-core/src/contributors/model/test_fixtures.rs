//! Builders shared by the model tests (port of `testFixtures.ts`). Test only.
//! Local dates use a 0-based month like the JS `Date` constructor so test
//! cases transcribe one to one.
#![allow(dead_code)]

use super::config::default_config;
use super::identity::{build_resolver, Resolver, ResolverOptions};
use super::period::{current_period, previous_period, Period, PeriodKind};
use super::types::ContributorsConfig;
use crate::contributors::{
    GitIdentity, RawBranchCommit, RawCommit, RawData, RawFile, RawPr, RawReview,
};
use chrono::{DateTime, Local, LocalResult, NaiveDate, SecondsFormat, TimeZone, Utc};

/// `new Date(y, month0, d, h, mi, s, ms)` in the local zone. A time that does
/// not exist (DST gap) moves forward, one that exists twice takes the first.
pub fn local_dt(y: i32, month0: u32, d: u32, h: u32, mi: u32, s: u32, ms: u32) -> DateTime<Local> {
    let naive = NaiveDate::from_ymd_opt(y, month0 + 1, d)
        .expect("valid date")
        .and_hms_milli_opt(h, mi, s, ms)
        .expect("valid time");
    for gap_minutes in [0, 60, 120, 180] {
        let shifted = naive + chrono::Duration::minutes(gap_minutes);
        match Local.from_local_datetime(&shifted) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => return t,
            LocalResult::None => {}
        }
    }
    panic!("no local time near {naive}");
}

pub fn local_at(y: i32, month0: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
    local_dt(y, month0, d, h, mi, 0, 0)
}

/// Local midnight of the day.
pub fn local_date(y: i32, month0: u32, d: u32) -> DateTime<Local> {
    local_dt(y, month0, d, 0, 0, 0, 0)
}

/// `Date.prototype.toISOString` of an instant.
pub fn iso(t: &DateTime<Local>) -> String {
    t.with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Whether the process time zone has daylight saving time in 2026.
pub fn has_dst() -> bool {
    let winter = Local.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    let summer = Local.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap();
    winter.offset().local_minus_utc() != summer.offset().local_minus_utc()
}

/// Runs `body` in a child process of the test binary with `TZ` set, so a time
/// zone never leaks into tests running in parallel. `module` is `module_path!()`
/// of the caller. When the zone database is missing the child sees UTC and the
/// body is expected to return early (`has_dst`).
pub fn with_tz(tz: &str, module: &str, name: &str, body: impl FnOnce()) {
    const CHILD: &str = "CONTRIBUTORS_TEST_TZ_CHILD";
    if std::env::var_os(CHILD).is_some() {
        body();
        return;
    }
    let path = module.split_once("::").map_or(module, |(_, rest)| rest);
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            &format!("{path}::{name}"),
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("TZ", tz)
        .env(CHILD, "1")
        .output()
        .expect("run the test binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "child under TZ={tz} failed:\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `oct(day)` of the TS fixtures: noon.
pub fn oct12(day: u32) -> String {
    oct(day, 12)
}

/// `new Date(y, month0, d, h).toISOString()`.
pub fn stamp(y: i32, month0: u32, d: u32, h: u32) -> String {
    iso(&local_dt(y, month0, d, h, 0, 0, 0))
}

/// ISO timestamp of a local day in October 2026 (Oct 1 2026 is a Thursday).
pub fn oct(day: u32, hour: u32) -> String {
    iso(&local_at(2026, 9, day, hour, 0))
}

/// October 2026 as a period.
pub fn period() -> Period {
    current_period(PeriodKind::Month, &local_date(2026, 9, 15), None)
}

/// The period before `p`; a test that needs one fails loudly when there is none.
pub fn prior_of(p: &Period) -> Period {
    previous_period(p).unwrap_or_else(|| panic!("{} has no previous period", p.id))
}

/// The last day of `period()`, so every day of it counts as elapsed.
pub fn period_now() -> DateTime<Local> {
    local_at(2026, 9, 31, 12, 0)
}

pub fn ident(name: &str, email: &str) -> GitIdentity {
    GitIdentity {
        name: name.into(),
        email: email.into(),
        login: None,
    }
}

pub fn ident_login(name: &str, email: &str, login: &str) -> GitIdentity {
    GitIdentity {
        login: Some(login.into()),
        ..ident(name, email)
    }
}

pub fn ana() -> GitIdentity {
    ident_login("Ana", "ana@example.com", "ana")
}
pub fn bob() -> GitIdentity {
    ident_login("Bob", "bob@example.com", "bob")
}
pub fn cat() -> GitIdentity {
    ident_login("Cat", "cat@example.com", "cat")
}
pub fn claude() -> GitIdentity {
    ident("Claude", "noreply@anthropic.com")
}
pub fn dependabot() -> GitIdentity {
    ident_login(
        "dependabot[bot]",
        "49699333+dependabot[bot]@users.noreply.github.com",
        "dependabot[bot]",
    )
}
pub fn github() -> GitIdentity {
    ident("GitHub", "noreply@github.com")
}

pub fn file(path: &str, added: u64) -> RawFile {
    file_d(path, added, 0)
}

pub fn file_d(path: &str, added: u64, deleted: u64) -> RawFile {
    RawFile {
        path: path.into(),
        added,
        deleted,
    }
}

pub fn make_commit(hash: &str, author: &GitIdentity, committed_at: &str) -> RawCommit {
    RawCommit {
        hash: hash.into(),
        subject: format!("commit {hash}"),
        author: author.clone(),
        committer: author.clone(),
        authored_at: committed_at.into(),
        committed_at: committed_at.into(),
        co_authors: vec![],
        files: vec![],
        pr_id: None,
    }
}

pub fn make_pr(id: &str, author: &GitIdentity, merged_at: &str) -> RawPr {
    RawPr {
        id: id.into(),
        number: None,
        title: format!("PR {id}"),
        state: "merged".into(),
        source: "gh".into(),
        author: author.clone(),
        committer: None,
        merged_at: Some(merged_at.into()),
        closed_at: None,
        merge_commit: None,
        commit_count: 1,
        branch_commits: Vec::<RawBranchCommit>::new(),
        landing_co_authors: vec![],
        files: vec![file("src/features/a.ts", 100)],
        closes_bug: false,
    }
}

pub fn make_review(
    pr_id: &str,
    reviewer: &GitIdentity,
    submitted_at: &str,
    comments: u64,
    state: &str,
) -> RawReview {
    RawReview {
        pr_id: pr_id.into(),
        reviewer: reviewer.clone(),
        state: state.into(),
        submitted_at: submitted_at.into(),
        comments,
    }
}

/// An approving review without comments.
pub fn make_approval(pr_id: &str, reviewer: &GitIdentity, submitted_at: &str) -> RawReview {
    make_review(pr_id, reviewer, submitted_at, 0, "APPROVED")
}

pub fn make_raw() -> RawData {
    RawData {
        head: "abc123".into(),
        branch: "main".into(),
        since: iso(&local_date(2026, 8, 1)),
        until: iso(&local_date(2026, 10, 1)),
        gh_available: true,
        gh_note: None,
        commits: vec![],
        prs: vec![],
        reviews: vec![],
        survival: vec![],
        reverted_pr_ids: vec![],
        truncated: false,
    }
}

/// Every distinct identity in the raw data (`scoring.observedIdentities`).
pub fn observed_identities(raw: &RawData) -> Vec<GitIdentity> {
    super::scoring::observed_identities(raw)
}

pub fn make_resolver(raw: &RawData) -> Resolver {
    make_resolver_with(raw, &default_config())
}

pub fn make_resolver_with(raw: &RawData, config: &ContributorsConfig) -> Resolver {
    build_resolver(
        config,
        ResolverOptions {
            merge: true,
            observed_identities: observed_identities(raw),
            ..Default::default()
        },
    )
}

/// `{ ...DEFAULT_CONFIG, ...extra }`.
pub fn with_config(patch: impl FnOnce(&mut ContributorsConfig)) -> ContributorsConfig {
    let mut config = default_config();
    patch(&mut config);
    config
}

// Partial overrides (the TS `extra` arguments): build, then mutate in a closure.

pub fn commit_with(
    hash: &str,
    author: &GitIdentity,
    committed_at: &str,
    patch: impl FnOnce(&mut RawCommit),
) -> RawCommit {
    let mut commit = make_commit(hash, author, committed_at);
    patch(&mut commit);
    commit
}

pub fn pr_with(
    id: &str,
    author: &GitIdentity,
    merged_at: &str,
    patch: impl FnOnce(&mut RawPr),
) -> RawPr {
    let mut pr = make_pr(id, author, merged_at);
    patch(&mut pr);
    pr
}

pub fn raw_with(patch: impl FnOnce(&mut RawData)) -> RawData {
    let mut raw = make_raw();
    patch(&mut raw);
    raw
}

/// A branch commit with equal author and commit times.
pub fn branch_commit(at: &str, authors: &[&GitIdentity]) -> RawBranchCommit {
    RawBranchCommit {
        authored_at: at.into(),
        committed_at: at.into(),
        authors: authors.iter().map(|a| (*a).clone()).collect(),
    }
}

/// A closed (unmerged) PR: `state` closed, `closedAt` set, no `mergedAt`.
pub fn closed_pr(id: &str, author: &GitIdentity, closed_at: &str) -> RawPr {
    pr_with(id, author, closed_at, |p| {
        p.state = "closed".into();
        p.merged_at = None;
        p.closed_at = Some(closed_at.into());
    })
}

/// `toBeCloseTo(expected, digits)`.
#[track_caller]
pub fn assert_close(actual: f64, expected: f64, digits: i32) {
    let tolerance = 10f64.powi(-digits) / 2.0;
    assert!(
        (actual - expected).abs() < tolerance,
        "{actual} is not close to {expected} ({digits} digits)"
    );
}

// The small team of `ui/leaderboardFixture.ts`, shared by the Ask Agent tests.

pub fn leaderboard_now() -> DateTime<Local> {
    local_at(2026, 9, 20, 10, 0)
}

/// October: Ana (the git user) 1st with a merged PR, Bob 2nd, Cat 3rd, Dan 4th
/// (new). September: Bob 1st, Ana 2nd, Cat 3rd, Dan absent. So the movements
/// are Ana up 1, Bob down 1, Cat unchanged, Dan new.
pub fn leaderboard_raw(patch: impl FnOnce(&mut RawData)) -> RawData {
    use super::period::sync_window;
    let dan = ident_login("Dan", "dan@example.com", "dan");
    let sept = |day: u32| stamp(2026, 8, day, 12);
    let days = |prefix: &str, who: &GitIdentity, list: &[u32], at: &dyn Fn(u32) -> String| {
        list.iter()
            .map(|&d| {
                commit_with(&format!("{prefix}{d}"), who, &at(d), |c| {
                    c.files = vec![file("src/features/a.ts", 5)]
                })
            })
            .collect::<Vec<_>>()
    };
    let month = current_period(super::period::PeriodKind::Month, &leaderboard_now(), None);
    let mut raw = raw_with(|r| {
        r.since = iso(&sync_window(&month, &leaderboard_now()).from);
        r.commits = [
            days("ana-o", &ana(), &[1, 2, 5, 6, 7, 8], &oct12),
            days("bob-o", &bob(), &[1, 2, 5, 6], &oct12),
            days("cat-o", &cat(), &[1, 2, 5], &oct12),
            days("dan-o", &dan, &[1], &oct12),
            days("bob-s", &bob(), &[1, 2, 3, 4, 7], &sept),
            days("ana-s", &ana(), &[1, 2, 3], &sept),
            days("cat-s", &cat(), &[1, 2], &sept),
        ]
        .concat();
        r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
            p.commit_count = 3;
            p.files = vec![
                file("src/features/a.ts", 100),
                file("src/features/a.test.ts", 20),
            ];
        })];
        r.reviews = vec![make_review("1", &bob(), &oct12(9), 3, "APPROVED")];
        r.survival = vec![crate::contributors::RawSurvival {
            pr_id: "1".into(),
            written: 100,
            alive: 85,
        }];
    });
    patch(&mut raw);
    raw
}

pub fn leaderboard_model() -> super::pipeline::ContributorsModel {
    use super::pipeline::{build_contributors_model, ModelInput};
    build_contributors_model(ModelInput {
        raw: std::sync::Arc::new(leaderboard_raw(|_| {})),
        config: std::sync::Arc::new(default_config()),
        mailmap: vec![],
        merge: true,
        period: current_period(super::period::PeriodKind::Month, &leaderboard_now(), None),
        now: leaderboard_now(),
        user: Some(crate::contributors::GitUser {
            name: "Ana".into(),
            email: "ana@example.com".into(),
        }),
        color_overrides: Default::default(),
    })
}
