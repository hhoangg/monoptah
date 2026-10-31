//! Where the GitHub tab spends its time: model build, one render, a hover
//! redraw and a period switch, on a large synthetic history and, when a git
//! repository is given, on its real data through `CoreBackend`.
//!
//! Run with
//! `cargo test --release perf_ -- --ignored --nocapture --test-threads=1`
//! (add `--config 'profile.release.package.gpui-pre.debug-assertions=true'` and the same for
//! `monocode-gpui` to get the hover rows: gpui records `debug_selector` bounds only
//! with debug assertions). `PERF_REPO=/path/to/repo` picks the repository of
//! `perf_real_repo` (default: the repository this crate is in).
//!
//! The test platform has no GPU: a draw covers building the elements, layout,
//! prepaint and painting into a scene, never the Metal pass.
use super::{
    ContributorsPage, Event, GithubView, Tab,
    backend::{Backend, CoreBackend},
    data::Clock,
    data_tests::{Fake, first_commit},
    github::Section,
    jobs::SyncJobs,
    page_tests::settle,
};
use chrono::{DateTime, Duration as Days, Local, TimeZone};
use gpui::{
    AnyElement, AppContext as _, AvailableSpace, Entity, IntoElement as _, Modifiers, Render as _,
    TestAppContext, VisualTestContext, WindowOptions, point, px, size,
};
use monocode_core::contributors::{
    GitIdentity, RawCommit, RawData, RawFile, RawPr, RawReview, RawSurvival,
    api::{self, Contributors},
    jobs::{EventSink, JobState, LoadResponse, SyncEvent},
    model::{
        config::DEFAULT_CONFIG,
        period::{Period, PeriodKind, current_period, pick_of, sync_window},
        pipeline::{ContributorsModel, ModelInput, build_contributors_model},
        prefs::serialize_pick,
        sync_jobs::iso_seconds,
    },
    ticket_jobs::{TicketJobEvent, TicketSink},
};
use std::{path::Path, rc::Rc, sync::Arc, time::Instant};

const COMMITS: usize = 5_000;
const PRS: usize = 300;
const PEOPLE: usize = 40;
/// The dense case is sized at or above the real repository (92 rows, 1134 PRs).
const DENSE_PRS: usize = 1_200;
const DENSE_PEOPLE: usize = 100;
const KINDS: [PeriodKind; 5] = [
    PeriodKind::Days7,
    PeriodKind::Month,
    PeriodKind::Quarter,
    PeriodKind::Year,
    PeriodKind::All,
];

fn now() -> DateTime<Local> {
    super::data_tests::start_of_tests()
}

// ---- synthetic history ---------------------------------------------------

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn person(i: usize) -> GitIdentity {
    GitIdentity {
        name: format!("Person {i}"),
        email: format!("p{i}@example.com"),
        login: Some(format!("person{i}")),
    }
}

fn stamp(epoch: i64) -> String {
    Local.timestamp_opt(epoch, 0).unwrap().to_rfc3339()
}

/// Commits and PRs with their epoch, so any window can be cut from them.
struct Synth {
    commits: Vec<(i64, RawCommit)>,
    prs: Vec<(i64, RawPr)>,
    reviews: Vec<(i64, RawReview)>,
    survival: Vec<RawSurvival>,
}

/// Skewed authors (a few write most) unless `skewed` is false (everyone active), 300 file paths, a PR on a fifth of the
/// commits, two reviews per PR.
fn synth(
    from: i64,
    to: i64,
    (commits, prs, people): (usize, usize, usize),
    skewed: bool,
    seed: u64,
) -> Synth {
    let mut rng = Lcg(seed);
    let at = |rng: &mut Lcg| from + rng.below((to - from).max(1) as u64) as i64;
    let author = |rng: &mut Lcg| {
        let top = if skewed {
            rng.below(people as u64) + 1
        } else {
            people as u64
        };
        person(rng.below(top) as usize)
    };
    let files = |rng: &mut Lcg, most: u64| -> Vec<RawFile> {
        (0..1 + rng.below(most))
            .map(|_| RawFile {
                path: format!("src/area{}/mod{}.rs", rng.below(20), rng.below(15)),
                added: 1 + rng.below(200),
                deleted: rng.below(80),
            })
            .collect()
    };
    let mut out = Synth {
        commits: vec![],
        prs: vec![],
        reviews: vec![],
        survival: vec![],
    };
    for n in 0..commits {
        let (when, who) = (at(&mut rng), author(&mut rng));
        let stamp = stamp(when);
        out.commits.push((
            when,
            RawCommit {
                hash: format!("{n:040x}"),
                subject: format!("change {n}"),
                author: who.clone(),
                committer: who,
                authored_at: stamp.clone(),
                committed_at: stamp,
                co_authors: vec![],
                files: files(&mut rng, 4),
                pr_id: (rng.below(5) == 0).then(|| format!("pr-{}", rng.below(prs as u64))),
            },
        ));
    }
    for n in 0..prs {
        let (when, who) = (at(&mut rng), author(&mut rng));
        let id = format!("pr-{n}");
        for _ in 0..2 {
            out.reviews.push((
                when,
                RawReview {
                    pr_id: id.clone(),
                    reviewer: author(&mut rng),
                    state: "APPROVED".into(),
                    submitted_at: stamp(when),
                    comments: rng.below(6),
                },
            ));
        }
        out.survival.push(RawSurvival {
            pr_id: id.clone(),
            written: 100,
            alive: 40 + rng.below(60),
        });
        out.prs.push((
            when,
            RawPr {
                id,
                number: Some(n as u64 + 1),
                title: format!("Pull request {n}"),
                state: "merged".into(),
                source: "gh".into(),
                author: who,
                committer: None,
                merged_at: Some(stamp(when)),
                closed_at: None,
                merge_commit: None,
                commit_count: 3,
                branch_commits: vec![],
                landing_co_authors: vec![],
                files: files(&mut rng, 6),
                closes_bug: false,
            },
        ));
    }
    out
}

impl Synth {
    /// What the store would return for `[from, to)`.
    fn cut(&self, from: i64, to: i64) -> RawData {
        let inside = |t: &i64| *t >= from && *t < to;
        let prs: Vec<RawPr> = self
            .prs
            .iter()
            .filter(|(t, _)| inside(t))
            .map(|(_, p)| p.clone())
            .collect();
        let ids: std::collections::HashSet<&str> = prs.iter().map(|p| p.id.as_str()).collect();
        RawData {
            head: "h1".into(),
            branch: "main".into(),
            since: stamp(from),
            until: stamp(to),
            gh_available: true,
            gh_note: None,
            commits: self
                .commits
                .iter()
                .filter(|(t, _)| inside(t))
                .map(|(_, c)| c.clone())
                .collect(),
            reviews: self
                .reviews
                .iter()
                .filter(|(t, r)| inside(t) && ids.contains(r.pr_id.as_str()))
                .map(|(_, r)| r.clone())
                .collect(),
            survival: self
                .survival
                .iter()
                .filter(|s| ids.contains(s.pr_id.as_str()))
                .cloned()
                .collect(),
            prs,
            reverted_pr_ids: vec![],
            truncated: false,
        }
    }
}

// ---- measuring -----------------------------------------------------------

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.
}

/// (median, min) of `runs` calls after `warm` uncounted ones.
fn sample(warm: usize, runs: usize, mut f: impl FnMut() -> f64) -> (f64, f64) {
    for _ in 0..warm {
        f();
    }
    let mut all: Vec<f64> = (0..runs).map(|_| f()).collect();
    all.sort_by(|a, b| a.total_cmp(b));
    (all[all.len() / 2], all[0])
}

fn build_model(raw: &Arc<RawData>, period: &Period, now: DateTime<Local>) -> ContributorsModel {
    build_contributors_model(ModelInput {
        raw: raw.clone(),
        config: Arc::new(DEFAULT_CONFIG.clone()),
        mailmap: vec![],
        merge: true,
        period: period.clone(),
        now,
        user: None,
        color_overrides: Default::default(),
    })
}

fn response(raw: RawData) -> LoadResponse {
    LoadResponse {
        raw,
        covered: true,
        has_gaps: false,
        missing: vec![],
        missing_commits: 0,
        synced_at: Some(now().timestamp() - 300),
        coverage_head: Some("h1".into()),
        head: "h1".into(),
        gh_state: Some("complete".into()),
    }
}

struct Page {
    fake: Arc<Fake>,
    window: gpui::WindowHandle<ContributorsPage>,
    v: VisualTestContext,
    view: Entity<GithubView>,
}

/// The Contributors page on a fake backend that answers every load with `raw`.
fn open_page(
    cx: &mut TestAppContext,
    raw: RawData,
    first: &DateTime<Local>,
    kind: PeriodKind,
    now: DateTime<Local>,
) -> Page {
    cx.update(|cx| cx.set_reduce_motion(true));
    let fake = Fake::new();
    let period = current_period(kind, &now, Some(first));
    fake.with(|s| {
        s.load.default = Ok(response(raw));
        s.first.default = Ok(Some(first.to_rfc3339()));
        s.saved_range = Some(serialize_pick(&pick_of(&period, &now)));
    });
    let clock: Clock = Rc::new(move || now);
    let jobs = cx.update(|cx| cx.new(SyncJobs::new));
    let window = cx
        .update(|cx| {
            cx.open_window(WindowOptions::default(), move |_, cx| {
                cx.new(|cx| ContributorsPage::new(jobs, cx).with_clock(clock, cx))
            })
        })
        .unwrap();
    let page = window.root(cx).unwrap();
    let _quiet = cx.update(|cx| cx.subscribe(&page, |_, _: &Event, _| {}));
    let backend: Arc<dyn Backend> = fake.clone();
    window
        .update(cx, move |page, _, cx| {
            page.set_context(vec!["/work/alpha".into()], "/work/alpha".into(), cx);
            page.show(Some(Tab::Github), None, cx);
            page.set_backend(backend, cx);
        })
        .unwrap();
    let mut v = VisualTestContext::from_window(window.into(), cx);
    settle(&mut v);
    let view = window
        .read_with(&v, |p, _| p.current_view().unwrap())
        .unwrap();
    view.update(&mut v, |this, cx| this.set_section(Section::Stats, cx));
    settle(&mut v);
    Page {
        fake,
        window,
        v,
        view,
    }
}

impl Page {
    fn model(&mut self) -> Option<Arc<ContributorsModel>> {
        let view = self.view.clone();
        self.v.read(|cx| view.read(cx).snapshot(cx).model)
    }

    /// A whole window frame (what a hover costs the app), after a refresh so
    /// no cached view is replayed.
    fn window_draw(&mut self) -> f64 {
        let start = Instant::now();
        self.v.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        ms(start)
    }

    /// The GithubView alone, in a 1024x768 box: (build ms, build+layout+prepaint+paint ms).
    /// The elements are built once inside the closure to time that on its own
    /// and dropped; the draw then renders the view itself (which builds them again).
    fn view_draw(&mut self) -> (f64, f64) {
        let view = self.view.clone();
        let built = std::cell::Cell::new(0.);
        let start = Instant::now();
        self.v.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(1024.)),
                AvailableSpace::Definite(px(768.)),
            ),
            |window, cx| {
                let t = Instant::now();
                let el: AnyElement =
                    view.update(cx, |this, cx| this.render(window, cx).into_any_element());
                built.set(ms(t));
                drop(el);
                view.clone().into_any_element()
            },
        );
        // The throwaway build is part of the wall time; take it out.
        (built.get(), ms(start) - built.get())
    }

    /// Building the Stats body alone (cards, legend, chart, table), dropped unseen.
    fn stats_build(&mut self, model: &Arc<ContributorsModel>) -> f64 {
        let view = self.view.clone();
        let model = model.clone();
        let built = std::cell::Cell::new(0.);
        self.v.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(960.)),
                AvailableSpace::Definite(px(768.)),
            ),
            |_, cx| {
                let t = Instant::now();
                let el = view.update(cx, |this, cx| this.stats_body(&model, cx));
                built.set(ms(t));
                drop(el);
                gpui::div()
            },
        );
        built.get()
    }

    /// Moves between the left and right third of the chart (found by its stable
    /// id, whatever it draws inside): (pointer event ms, redraw ms, total) per
    /// move, or `None` without recorded selectors.
    fn hover(&mut self, moves: usize) -> Option<(f64, f64, f64)> {
        let chart = self.v.debug_bounds("contributors-stats-chart")?;
        let at = |third: f32| {
            point(
                chart.origin.x + chart.size.width * third,
                chart.origin.y + chart.size.height * 0.5,
            )
        };
        let spots = [at(0.33), at(0.67)];
        // Warm: the first move into the chart builds the popover.
        let (mut event, mut draw) = (vec![], vec![]);
        for n in 0..moves + 3 {
            let spot = spots[n % 2];
            let t = Instant::now();
            self.v.simulate_mouse_move(spot, None, Modifiers::default());
            let e = ms(t);
            let d = self.window_draw_plain();
            if n >= 3 {
                event.push(e);
                draw.push(d);
            }
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.total_cmp(b));
            v[v.len() / 2]
        };
        // Leave the chart so later frames do not run under a hovered bucket.
        self.v
            .simulate_mouse_move(point(px(1.), px(1.)), None, Modifiers::default());
        let (e, d) = (med(&mut event), med(&mut draw));
        Some((e, d, e + d))
    }

    /// A frame without `refresh`, as the one that follows a notify.
    fn window_draw_plain(&mut self) -> f64 {
        let start = Instant::now();
        self.v.update(|window, cx| window.draw(cx).clear(cx));
        ms(start)
    }

    fn close(mut self) {
        self.window
            .update(&mut self.v, |_, window, _| window.remove_window())
            .ok();
    }
}

struct Row {
    label: String,
    commits: usize,
    prs: usize,
    model_ms: f64,
    drop_ms: f64,
    buckets: usize,
    legend: usize,
    /// Buckets x legend entries: the most segments a bar chart could have.
    bars: usize,
    rows: usize,
    build_ms: f64,
    view_ms: f64,
    window_ms: f64,
    stats_build_ms: f64,
    hover: Option<(f64, f64, f64)>,
    switch_ms: f64,
    open_ms: f64,
    leaderboard_ms: f64,
    identities_ms: f64,
}

// One call site per case; a struct for these would only be a rename of the arguments.
#[allow(clippy::too_many_arguments)]
fn measure(
    cx: &mut TestAppContext,
    label: &str,
    kind: PeriodKind,
    raw: RawData,
    first: &DateTime<Local>,
    now: DateTime<Local>,
    switch_to: Option<(PeriodKind, RawData)>,
    hover: bool,
) -> Row {
    let period = current_period(kind, &now, Some(first));
    let (commits, prs) = (raw.commits.len(), raw.prs.len());
    let shared = Arc::new(raw.clone());
    let (model_ms, _) = sample(1, 5, || {
        let t = Instant::now();
        std::hint::black_box(build_model(&shared, &period, now));
        ms(t)
    });
    // Dropping the model it replaces is the one cost of a rebuild that lands on the UI thread.
    let (drop_ms, _) = sample(1, 5, || {
        let old = build_model(&shared, &period, now);
        let t = Instant::now();
        drop(old);
        ms(t)
    });
    eprintln!("[{label}] open");
    let opened = Instant::now();
    let mut page = open_page(cx, raw, first, kind, now);
    let open_ms = ms(opened);
    let model = page.model().expect("the page built a model");
    assert_eq!(
        model.period.kind, kind,
        "the page shows the period asked for"
    );
    let stats = &model.stats;
    let (build_ms, view_ms) = {
        let (mut build, mut total) = (vec![], vec![]);
        for n in 0..13 {
            let (b, t) = page.view_draw();
            if n >= 3 {
                build.push(b);
                total.push(t);
            }
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.total_cmp(b));
            v[v.len() / 2]
        };
        (med(&mut build), med(&mut total))
    };
    eprintln!("[{label}] frames");
    let (window_ms, _) = sample(3, 10, || page.window_draw());
    let (stats_build_ms, _) = sample(3, 10, || page.stats_build(&model));
    let mut other_sections = [0.; 2];
    for (n, section) in [Section::Leaderboard, Section::Identities]
        .into_iter()
        .enumerate()
    {
        let view = page.view.clone();
        view.update(&mut page.v, |this, cx| this.set_section(section, cx));
        settle(&mut page.v);
        other_sections[n] = sample(3, 10, || page.window_draw()).0;
    }
    let view = page.view.clone();
    view.update(&mut page.v, |this, cx| this.set_section(Section::Stats, cx));
    settle(&mut page.v);
    page.window_draw();
    eprintln!("[{label}] hover");
    // A tooltip with a hundred lines covers the pointer, and the hover then flips
    // on and off on every frame without settling, so dense cases skip hover.
    let hover = if hover { page.hover(20) } else { None };
    // Switching: swap the data every load returns, ask for another kind, and
    // wait until its model is on screen (load, Loaded, model build, draw).
    eprintln!("[{label}] switch");
    let switch_ms = match switch_to {
        Some((next, data)) => {
            page.fake.with(|s| s.load.default = Ok(response(data)));
            let view = page.view.clone();
            let t = Instant::now();
            view.update(&mut page.v, |this, cx| this.set_kind(next, cx));
            settle(&mut page.v);
            let shown = page.model().map(|m| m.period.kind);
            assert_eq!(shown, Some(next), "the switch ended on the new period");
            ms(t)
        }
        None => f64::NAN,
    };
    let row = Row {
        label: label.into(),
        commits,
        prs,
        model_ms,
        drop_ms,
        buckets: stats.buckets.len(),
        legend: stats.legend.len(),
        bars: stats.buckets.len() * stats.legend.len(),
        rows: stats.rows.len(),
        build_ms,
        view_ms,
        window_ms,
        stats_build_ms,
        hover,
        switch_ms,
        open_ms,
        leaderboard_ms: other_sections[0],
        identities_ms: other_sections[1],
    };
    page.close();
    row
}

fn print_rows(title: &str, rows: &[Row]) {
    println!("\n== {title} ==");
    println!(
        "{:<18} {:>6} {:>4} {:>9} {:>8} {:>5} {:>6} {:>8} {:>5} | {:>9} {:>9} {:>9} | {:>10} | {:>9} {:>9} {:>9} | {:>9} {:>9} | {:>9} {:>9}",
        "case",
        "commit",
        "pr",
        "model ms",
        "drop ms",
        "bkts",
        "legend",
        "bkt*leg",
        "rows",
        "build ms",
        "lay+pnt ms",
        "window ms",
        "stats bld",
        "hov evt",
        "hov draw",
        "hov tot",
        "switch ms",
        "open ms",
        "board ms",
        "ident ms"
    );
    for r in rows {
        let (e, d, t) = r.hover.unwrap_or((f64::NAN, f64::NAN, f64::NAN));
        println!(
            "{:<18} {:>6} {:>4} {:>9.1} {:>8.2} {:>5} {:>6} {:>8} {:>5} | {:>9.2} {:>9.2} {:>9.2} | {:>10.2} | {:>9.2} {:>9.2} {:>9.2} | {:>9.1} {:>9.1} | {:>9.2} {:>9.2}",
            r.label,
            r.commits,
            r.prs,
            r.model_ms,
            r.drop_ms,
            r.buckets,
            r.legend,
            r.bars,
            r.rows,
            r.build_ms,
            r.view_ms,
            r.window_ms,
            r.stats_build_ms,
            e,
            d,
            t,
            r.switch_ms,
            r.open_ms,
            r.leaderboard_ms,
            r.identities_ms
        );
    }
}

/// The displayed period itself, up to now.
fn shown_window(kind: PeriodKind, first: &DateTime<Local>, now: &DateTime<Local>) -> (i64, i64) {
    let period = current_period(kind, now, Some(first));
    let w = sync_window(&period, now);
    (period.start.timestamp(), w.to.timestamp())
}

fn window_of(kind: PeriodKind, first: &DateTime<Local>, now: &DateTime<Local>) -> (i64, i64) {
    let period = current_period(kind, now, Some(first));
    let w = sync_window(&period, now);
    (w.from.timestamp(), w.to.timestamp())
}

/// The test platform shapes no text (`NoopTextSystem`); on macOS the real
/// CoreText system can be put in its place, so text costs show up.
fn context(real_text: bool) -> TestAppContext {
    let dispatcher = gpui::TestDispatcher::new(0);
    #[cfg(target_os = "macos")]
    if real_text {
        return TestAppContext::build_with_text_system(
            dispatcher,
            None,
            Arc::new(gpui_apple::AppleTextSystem::new()),
        );
    }
    assert!(!real_text, "real text shaping needs macOS");
    TestAppContext::build(dispatcher, None)
}

#[test]
#[ignore = "Prints timings; run in release with --ignored --nocapture"]
fn perf_synthetic() {
    synthetic(&mut context(false));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Prints timings; run in release with --ignored --nocapture"]
fn perf_synthetic_real_text() {
    synthetic(&mut context(true));
}

fn synthetic(cx: &mut TestAppContext) {
    let (now, first) = (now(), first_commit());
    // The history of a busy repository, spread over every period of the test.
    let spread = synth(
        first.timestamp(),
        now.timestamp(),
        (COMMITS, PRS, PEOPLE),
        true,
        7,
    );
    let mut sparse = vec![];
    let mut dense = vec![];
    for (i, kind) in KINDS.into_iter().enumerate() {
        let (from, to) = window_of(kind, &first, &now);
        let next = KINDS[(i + 1) % KINDS.len()];
        let (nf, nt) = window_of(next, &first, &now);
        sparse.push(measure(
            cx,
            &format!("spread {kind:?}"),
            kind,
            spread.cut(from, to),
            &first,
            now,
            Some((next, spread.cut(nf, nt))),
            true,
        ));
        // The same size of data all inside the period: the worst case.
        // All of it inside the displayed period (not the sync window, which
        // also covers the previous one), with everyone active.
        let dense_size = (COMMITS, DENSE_PRS, DENSE_PEOPLE);
        let (ps, pe) = shown_window(kind, &first, &now);
        let (nps, npe) = shown_window(next, &first, &now);
        let all = synth(ps, pe, dense_size, false, 11 + i as u64);
        let next_all = synth(nps, npe, dense_size, false, 21 + i as u64);
        dense.push(measure(
            cx,
            &format!("dense {kind:?}"),
            kind,
            all.cut(from, to),
            &first,
            now,
            Some((next, next_all.cut(nf, nt))),
            false,
        ));
    }
    print_rows(
        "synthetic: history of 5000 commits/300 PRs/40 people spread over 2023-05..2026-10",
        &sparse,
    );
    print_rows(
        "synthetic dense: 5000 commits/1200 PRs/100 people, all inside the displayed period",
        &dense,
    );
}

// ---- the real repository ---------------------------------------------------

struct Quiet;
impl EventSink for Quiet {
    fn emit(&self, _: &SyncEvent) {}
}
impl TicketSink for Quiet {
    fn emit(&self, _: &TicketJobEvent) {}
}

#[test]
#[ignore = "Syncs a repository into a private database and prints timings"]
fn perf_real_repo() {
    real_repo(&mut context(false));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "Syncs a repository into a private database and prints timings"]
fn perf_real_repo_real_text() {
    real_repo(&mut context(true));
}

fn real_repo(cx: &mut TestAppContext) {
    let repo = std::env::var("PERF_REPO")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../..").into());
    let repo = Path::new(&repo).canonicalize().unwrap();
    let repo = repo.to_str().unwrap().to_string();
    let data_dir = std::env::temp_dir().join("monoptah-perf-contrib");
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();
    // `PERF_DB=memory` uses the private in-memory database; the default is a
    // file on disk, like the app's.
    let db_path = if std::env::var("PERF_DB").is_ok_and(|v| v == "memory") {
        Path::new(":memory:").to_path_buf()
    } else {
        data_dir.join("contributors.db")
    };
    println!("store: {}", db_path.display());
    let core = Arc::new(
        Contributors::open(&db_path, &data_dir, Arc::new(Quiet), Arc::new(Quiet)).unwrap(),
    );
    let backend = CoreBackend::new(core.clone(), None);
    let now = Local::now();
    let first = api::first_commit(&repo, None)
        .unwrap()
        .and_then(|iso| monocode_core::contributors::model::period::parse_iso(&iso))
        .expect("the repository has commits");
    // One sync of the whole history; every period is then a sub-window of it.
    let start = Instant::now();
    api::sync_start(
        &core,
        &repo,
        None,
        &iso_seconds(&first),
        &iso_seconds(&(now + Days::days(1))),
        "full",
        None,
    )
    .unwrap();
    loop {
        let jobs = api::sync_status(&core);
        if jobs
            .iter()
            .all(|j| !matches!(j.state, JobState::Queued | JobState::Running))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(start.elapsed().as_secs() < 900, "the sync did not end");
    }
    println!(
        "\nreal repo {repo}: sync of the whole history took {:.1} s",
        start.elapsed().as_secs_f64()
    );
    let mut rows = vec![];
    let mut loads = vec![];
    for kind in KINDS {
        let period = current_period(kind, &now, Some(&first));
        let window = sync_window(&period, &now);
        let (since, until) = (iso_seconds(&window.from), iso_seconds(&period.end));
        let (load_ms, _) = sample(1, 5, || {
            let t = Instant::now();
            let loaded = backend.load(&repo, None, &since, &until).unwrap();
            std::hint::black_box(&loaded);
            ms(t)
        });
        let loaded = backend.load(&repo, None, &since, &until).unwrap();
        let (git_ms, _) = sample(1, 5, || git_only_ms(&repo, &loaded.head, &since, &until));
        loads.push((
            kind,
            load_ms,
            git_ms,
            loaded.covered,
            loaded.raw.commits.len(),
        ));
        rows.push(measure(
            cx,
            &format!("real {kind:?}"),
            kind,
            loaded.raw,
            &first,
            now,
            None,
            true,
        ));
    }
    print_rows("real repository (clock = now)", &rows);
    for (kind, ms, git, covered, commits) in loads {
        println!(
            "data load {kind:?}: {ms:.2} ms, of which the same git commands alone {git:.2} ms, rest {:.2} ms (covered {covered}, {commits} commits)",
            ms - git
        );
    }
    let _ = std::fs::remove_dir_all(&data_dir);
}

/// The git processes `jobs::load_response` starts, run bare: repo root, branch
/// lookups, common dir and the two hash listings. An approximation of the git
/// share of `backend.load` (same commands, not the same code path), so the
/// rest is "SQLite + parsing + whatever else".
fn git_only_ms(repo: &str, head: &str, since: &str, until: &str) -> f64 {
    let t = Instant::now();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap()
    };
    git(&["rev-parse", "--show-toplevel"]);
    git(&["symbolic-ref", "-q", "HEAD"]);
    git(&["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]);
    git(&["rev-parse", "--verify", "--quiet", head]);
    git(&["rev-parse", "--git-common-dir"]);
    let (since, until) = (format!("--since={since}"), format!("--until={until}"));
    git(&[
        "log",
        "--format=%H",
        "--no-merges",
        "--date-order",
        &since,
        &until,
        head,
    ]);
    git(&["log", "--format=%H", &since, &until, head]);
    ms(t)
}

// ---- GPU ---------------------------------------------------------------------

/// What the Metal pass costs for a scene of that many small rounded quads (the
/// shape of the chart's segments and the table's cells) at 1024x768, readback
/// to an image included. A real window's scene cannot be taken out of the test
/// platform, so this is a stand-in for the GPU half, not a measurement of it.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "Requires a Metal device; run explicitly on macOS"]
fn perf_metal_quads() {
    use gpui::{Bounds, ContentMask, DevicePixels, Quad, ScaledPixels, Scene, rgba};
    use gpui_apple::metal_renderer::{InstanceBufferPool, MetalRenderer};
    let mut renderer = MetalRenderer::new_headless(Arc::new(parking_lot::Mutex::new(
        InstanceBufferPool::default(),
    )));
    let full = Bounds::new(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(1024.), ScaledPixels(768.)),
    );
    println!("\n== Metal: N small rounded quads, 1024x768, scene -> image ==");
    for count in [500usize, 1_500, 5_000, 15_000] {
        let mut scene = Scene::default();
        for n in 0..count {
            let (x, y) = ((n % 100) as f32 * 10., (n / 100 % 70) as f32 * 10.);
            scene.insert_primitive(Quad {
                order: 0,
                border_style: Default::default(),
                bounds: Bounds::new(
                    point(ScaledPixels(x), ScaledPixels(y)),
                    size(ScaledPixels(8.), ScaledPixels(8.)),
                ),
                content_mask: ContentMask { bounds: full },
                background: rgba(0x4080c0ff).into(),
                border_color: Default::default(),
                corner_radii: gpui::Corners::all(ScaledPixels(1.)),
                border_widths: Default::default(),
            });
        }
        scene.finish();
        let (median, min) = sample(3, 15, || {
            let t = Instant::now();
            std::hint::black_box(
                renderer
                    .render_scene_to_image(&scene, size(DevicePixels(1024), DevicePixels(768)))
                    .unwrap(),
            );
            ms(t)
        });
        println!("{count:>6} quads: median {median:.2} ms, min {min:.2} ms");
    }
}

// ---- the ClickUp tab -----------------------------------------------------------

#[test]
#[ignore = "Prints timings; run with --ignored --nocapture"]
fn perf_ticket_hover() {
    let mut cx = context(false);
    // 60 members of 4 tickets each; one member with 200 tickets on the page.
    let board = super::ticket::hover_numbers(&mut cx, 60, 4, None, "ticket-trend-chart");
    eprintln!("ticket board, hover over the trend chart: {board:?}");
    let page = super::ticket::hover_numbers(&mut cx, 1, 200, Some("m0"), "ticket-detail-chart");
    eprintln!("ticket page of 200 tickets, hover over the trend chart: {page:?}");
    let calendar =
        super::ticket::hover_numbers(&mut cx, 1, 200, Some("m0"), "ticket-detail-calendar");
    eprintln!("ticket page of 200 tickets, hover over the calendar: {calendar:?}");
}
