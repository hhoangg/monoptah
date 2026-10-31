//! LeaderboardTab.test.ts: the rules and copy of the Leaderboard section as
//! tests of the pure view functions, and the layout, Ask and sidebar-opening
//! cases in a real window over the fake backend. The formula sidebar itself is
//! tested in `formula_tests`.
use super::{
    GithubEvent,
    data_tests::{Fake, start_of_tests},
    github::GithubView,
    leaderboard::{
        self, HEADS, MOTIVATION, MoveTone, ORDINALS, badge_labels, compared, empty_title,
        movement_view, period_line, person_color, plural, podium_column, podium_line, row_cells,
        summary_text,
    },
    page_tests::{Ui, click, github, settle, shown},
};
use chrono::{DateTime, Local, TimeZone};
use gpui::{Entity, TestAppContext, VisualTestContext};
use monocode_core::contributors::{
    GitIdentity, GitUser, RawBranchCommit, RawCommit, RawData, RawFile, RawPr, RawReview,
    RawSurvival,
    jobs::LoadResponse,
    model::{
        config::{default_config, serialize_config},
        format::format_score,
        leaderboard::LeaderboardRow,
        period::{PeriodKind, PeriodPick, current_period, previous_period, sync_window},
        pipeline::{ContributorsModel, ModelInput, build_contributors_model},
        prefs::serialize_pick,
        snapshot::SnapshotScope,
        types::ContributorsConfig,
    },
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub(super) fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Local> {
    Local.with_ymd_and_hms(y, m, d, h, 0, 0).earliest().unwrap()
}

/// `NOW` of leaderboardFixture.ts.
pub(super) fn now() -> DateTime<Local> {
    local(2026, 10, 20, 10)
}

fn who(name: &str, email: &str, login: &str) -> GitIdentity {
    GitIdentity {
        name: name.into(),
        email: email.into(),
        login: Some(login.into()),
    }
}
fn ana() -> GitIdentity {
    who("Ana", "ana@example.com", "ana")
}
fn bob() -> GitIdentity {
    who("Bob", "bob@example.com", "bob")
}
fn cat() -> GitIdentity {
    who("Cat", "cat@example.com", "cat")
}
fn dan() -> GitIdentity {
    who("Dan", "dan@example.com", "dan")
}

fn oct(day: u32) -> DateTime<Local> {
    local(2026, 10, day, 12)
}
fn sept(day: u32) -> DateTime<Local> {
    local(2026, 9, day, 12)
}

fn commit(hash: &str, author: &GitIdentity, at: DateTime<Local>) -> RawCommit {
    let at = at.with_timezone(&chrono::Utc).to_rfc3339();
    RawCommit {
        hash: hash.into(),
        subject: format!("commit {hash}"),
        author: author.clone(),
        committer: author.clone(),
        authored_at: at.clone(),
        committed_at: at,
        co_authors: vec![],
        files: vec![RawFile {
            path: "src/features/a.ts".into(),
            added: 5,
            deleted: 0,
        }],
        pr_id: None,
    }
}

fn empty_raw() -> RawData {
    RawData {
        head: "h1".into(),
        branch: "main".into(),
        since: String::new(),
        until: String::new(),
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

fn file(path: &str, added: u64) -> RawFile {
    RawFile {
        path: path.into(),
        added,
        deleted: 0,
    }
}

/// October: Ana (the git user) 1st with a merged PR, Bob 2nd, Cat 3rd, Dan 4th
/// (new). September: Bob 1st, Ana 2nd, Cat 3rd, Dan absent. So the movements
/// are Ana up 1, Bob down 1, Cat unchanged, Dan new.
pub(super) fn board_raw() -> RawData {
    let mut commits = vec![];
    let mut days =
        |prefix: &str, who: &GitIdentity, list: &[u32], at: fn(u32) -> DateTime<Local>| {
            for &d in list {
                commits.push(commit(&format!("{prefix}{d}"), who, at(d)));
            }
        };
    days("ana-o", &ana(), &[1, 2, 5, 6, 7, 8], oct);
    days("bob-o", &bob(), &[1, 2, 5, 6], oct);
    days("cat-o", &cat(), &[1, 2, 5], oct);
    days("dan-o", &dan(), &[1], oct);
    days("bob-s", &bob(), &[1, 2, 3, 4, 7], sept);
    days("ana-s", &ana(), &[1, 2, 3], sept);
    days("cat-s", &cat(), &[1, 2], sept);
    let merged = oct(8).with_timezone(&chrono::Utc).to_rfc3339();
    RawData {
        commits,
        prs: vec![RawPr {
            id: "1".into(),
            number: None,
            title: "PR 1".into(),
            state: "merged".into(),
            source: "gh".into(),
            author: ana(),
            committer: None,
            merged_at: Some(merged),
            closed_at: None,
            merge_commit: None,
            commit_count: 3,
            branch_commits: Vec::<RawBranchCommit>::new(),
            landing_co_authors: vec![],
            files: vec![
                file("src/features/a.ts", 100),
                file("src/features/a.test.ts", 20),
            ],
            closes_bug: false,
        }],
        reviews: vec![RawReview {
            pr_id: "1".into(),
            reviewer: bob(),
            state: "COMMENTED".into(),
            submitted_at: oct(9).with_timezone(&chrono::Utc).to_rfc3339(),
            comments: 3,
        }],
        survival: vec![RawSurvival {
            pr_id: "1".into(),
            written: 100,
            alive: 85,
        }],
        ..empty_raw()
    }
}

pub(super) struct Over {
    pub raw: RawData,
    pub config: ContributorsConfig,
    pub kind: PeriodKind,
    pub user: bool,
}
impl Default for Over {
    fn default() -> Self {
        Self {
            raw: board_raw(),
            config: default_config(),
            kind: PeriodKind::Month,
            user: true,
        }
    }
}

pub(super) fn model_with(over: Over) -> ContributorsModel {
    let now = now();
    let first = local(2026, 9, 1, 0);
    let period = current_period(over.kind, &now, Some(&first));
    model_in(over, period)
}

pub(super) fn model_in(
    over: Over,
    period: monocode_core::contributors::model::period::Period,
) -> ContributorsModel {
    let mut raw = over.raw;
    raw.since = sync_window(&period, &now()).from.to_rfc3339();
    build_contributors_model(ModelInput {
        raw: Arc::new(raw),
        config: Arc::new(over.config),
        mailmap: vec![],
        merge: true,
        period,
        now: now(),
        user: over.user.then(|| GitUser {
            name: "Ana".into(),
            email: "ana@example.com".into(),
        }),
        color_overrides: Default::default(),
    })
}

pub(super) fn board() -> ContributorsModel {
    model_with(Over::default())
}

fn names(model: &ContributorsModel) -> Vec<&str> {
    model
        .leaderboard
        .rows
        .iter()
        .map(|r| r.name.as_str())
        .collect()
}

fn only_ana() -> RawData {
    let mut raw = board_raw();
    raw.commits.retain(|c| c.hash.starts_with("ana-o"));
    raw.prs.clear();
    raw.reviews.clear();
    raw
}

// ---- ranking ----------------------------------------------------------------------

#[test]
fn lists_the_board_in_rank_order_with_the_table_columns() {
    assert_eq!(
        HEADS,
        [
            "Rank",
            "Contributor",
            "Score",
            "Commits",
            "PRs (commits)",
            "AI-assisted",
            "Streak",
            "Badges",
            "Actions"
        ]
    );
    assert_eq!(names(&board()), ["Ana", "Bob", "Cat", "Dan"]);
}

#[test]
fn shows_score_commits_prs_with_their_commits_ai_share_and_streak_from_the_row() {
    let model = board();
    let ana = &model.leaderboard.rows[0];
    assert_eq!((ana.merged_prs, ana.branch_commits), (1, 3));
    let cells = row_cells(ana);
    assert_eq!(cells.rank, "1");
    assert_eq!(cells.score, format_score(ana.score));
    assert_eq!(cells.commits, ana.main_commits.to_string());
    assert_eq!(cells.prs, "1");
    assert_eq!(cells.pr_commits, "(3)");
    assert_eq!(cells.ai, format!("{}%", ana.ai_assisted_pct));
    assert_eq!(cells.streak, format!("{}d", ana.streak));
}

#[test]
fn names_the_period_and_states_the_motivation_note() {
    let model = board();
    assert_eq!(period_line(&model), "period: Oct 2026");
    assert_eq!(
        MOTIVATION,
        "Rankings are for motivation, not performance review."
    );
}

#[test]
fn shows_the_badges_of_a_row_by_label() {
    let model = board();
    let ana = &model.leaderboard.rows[0];
    assert!(ana.badges.iter().any(|b| b.id.as_str() == "builder"));
    assert_eq!(
        badge_labels(ana),
        ana.badges.iter().map(|b| b.label).collect::<Vec<_>>()
    );
    assert!(badge_labels(ana).contains(&"Builder"));
    let bob = &model.leaderboard.rows[1];
    assert!(badge_labels(bob).contains(&"Reviewer"));
}

#[test]
fn paints_each_avatar_with_the_shared_colour_of_the_person() {
    let mut model = board();
    // Colours that no hash would pick, so only the shared map can explain them.
    let ids: Vec<String> = model
        .leaderboard
        .rows
        .iter()
        .map(|r| r.person_id.clone())
        .collect();
    let picked = ["#102030", "#102031", "#102032", "#102033"];
    model.colors = ids.iter().cloned().zip(picked).collect();
    for (row, color) in model.leaderboard.rows.iter().zip(picked) {
        assert_eq!(person_color(&model, row), color);
    }
}

#[test]
fn gives_the_four_people_on_the_board_four_different_colours() {
    let model = board();
    let colors: std::collections::HashSet<&str> = model
        .leaderboard
        .rows
        .iter()
        .map(|r| person_color(&model, r))
        .collect();
    assert_eq!(colors.len(), 4);
}

// ---- movement ----------------------------------------------------------------------

#[test]
fn shows_up_down_unchanged_and_new_with_the_previous_period_explanation() {
    let model = board();
    let moves: Vec<Option<i64>> = model.leaderboard.rows.iter().map(|r| r.movement).collect();
    assert_eq!(moves, [Some(1), Some(-1), Some(0), None]);
    let views: Vec<_> = moves.into_iter().map(movement_view).collect();
    assert_eq!(
        (views[0].glyph.as_str(), views[0].tone),
        ("▲1", MoveTone::Up)
    );
    assert_eq!(views[0].title, "Up 1 place vs previous period");
    assert_eq!(
        (views[1].glyph.as_str(), views[1].tone),
        ("▼1", MoveTone::Down)
    );
    assert_eq!(views[1].title, "Down 1 place vs previous period");
    assert_eq!(
        (views[2].glyph.as_str(), views[2].tone),
        ("·", MoveTone::Flat)
    );
    assert_eq!(views[2].title, "Same rank as the previous period");
    assert_eq!(
        (views[3].glyph.as_str(), views[3].tone),
        ("·", MoveTone::Flat)
    );
    assert_eq!(views[3].title, "Not ranked in the previous period");
}

#[test]
fn pluralizes_a_bigger_move() {
    let up = movement_view(Some(2));
    assert_eq!(up.glyph, "▲2");
    assert_eq!(up.title, "Up 2 places vs previous period");
    let down = movement_view(Some(-3));
    assert_eq!(down.glyph, "▼3");
    assert_eq!(down.title, "Down 3 places vs previous period");
    assert_eq!(plural(1, "PR"), "1 PR");
    assert_eq!(plural(0, "commit"), "0 commits");
}

#[test]
fn shows_no_movement_at_all_for_all_time_which_has_no_previous_period() {
    let all = model_with(Over {
        kind: PeriodKind::All,
        ..Over::default()
    });
    assert!(all.previous_period.is_none());
    assert!(!compared(&all));
    assert_eq!(all.leaderboard.rows.len(), 4);
    let ranks: Vec<String> = all
        .leaderboard
        .rows
        .iter()
        .map(|r| row_cells(r).rank)
        .collect();
    assert_eq!(ranks, ["1", "2", "3", "4"]);
    assert!(compared(&board()));
}

// ---- you chip ----------------------------------------------------------------------

#[test]
fn marks_only_the_git_users_row() {
    let model = board();
    let marked: Vec<&str> = model
        .leaderboard
        .rows
        .iter()
        .filter(|r| r.is_me)
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(marked, ["Ana"]);
}

#[test]
fn shows_no_chip_when_the_git_user_is_not_on_the_board() {
    let model = model_with(Over {
        user: false,
        ..Over::default()
    });
    assert!(model.leaderboard.rows.iter().all(|r| !r.is_me));
}

// ---- podium ------------------------------------------------------------------------

#[test]
fn places_second_left_first_in_the_middle_and_third_right() {
    assert_eq!(
        [0, 1, 2].map(podium_column),
        [2, 1, 3],
        "rank order in the tree, 2-1-3 on screen"
    );
}

#[test]
fn shows_the_top_three_in_rank_order_with_medals_as_text() {
    let model = board();
    assert_eq!(ORDINALS, ["1st", "2nd", "3rd"]);
    assert_eq!(model.leaderboard.podium.len(), 3);
    let top: Vec<&str> = model
        .leaderboard
        .podium
        .iter()
        .map(|r| r.name.as_str())
        .collect();
    assert_eq!(top, ["Ana", "Bob", "Cat"]);
}

#[test]
fn shows_score_prs_with_commits_streak_and_badges_on_a_podium_card() {
    let model = board();
    let [ana, bob, ..] = &model.leaderboard.rows[..] else {
        panic!("two rows")
    };
    assert_eq!(
        podium_line(ana),
        format!("1 PR (3 commits) · {}d streak", ana.streak)
    );
    assert_eq!(
        podium_line(bob),
        format!("0 PRs (0 commits) · {}d streak", bob.streak)
    );
}

#[test]
fn shows_fewer_places_when_fewer_people_are_ranked() {
    let model = model_with(Over {
        raw: only_ana(),
        ..Over::default()
    });
    assert_eq!(model.leaderboard.podium.len(), 1);
    assert_eq!(model.leaderboard.rows.len(), 1);
}

// ---- period, empty, summary ---------------------------------------------------------

#[test]
fn titles_the_board_with_the_ranked_period() {
    let now = now();
    let october = current_period(PeriodKind::Month, &now, None);
    let september = previous_period(&october).unwrap();
    let model = model_in(Over::default(), september);
    assert_eq!(period_line(&model), "period: Sep 2026");
    assert_eq!(names(&model), ["Bob", "Ana", "Cat"]);
    for (kind, label) in [
        (PeriodKind::Days7, "period: Last 7 days"),
        (PeriodKind::Quarter, "period: Q4 2026"),
        (PeriodKind::Year, "period: 2026"),
    ] {
        let model = model_with(Over {
            kind,
            ..Over::default()
        });
        assert_eq!(period_line(&model), label);
    }
}

#[test]
fn explains_an_empty_period_instead_of_an_empty_table() {
    let model = model_with(Over {
        raw: empty_raw(),
        ..Over::default()
    });
    assert!(model.leaderboard.rows.is_empty());
    assert_eq!(empty_title(&model), "No scored activity in Oct 2026");
}

#[test]
fn words_the_empty_board_of_all_time_as_of_all_time() {
    let model = model_with(Over {
        raw: empty_raw(),
        kind: PeriodKind::All,
        ..Over::default()
    });
    assert_eq!(empty_title(&model), "No scored activity of all time");
}

#[test]
fn states_the_formula_without_a_review_caveat_when_github_data_exists() {
    let text = summary_text(&board());
    assert_eq!(
        text,
        "Score = Delivery × Quality + Care + Review + Consistency + Breadth."
    );
}

#[test]
fn says_review_is_not_counted_when_github_data_is_unavailable() {
    let mut raw = board_raw();
    raw.gh_available = false;
    let model = model_with(Over {
        raw,
        ..Over::default()
    });
    assert!(
        summary_text(&model).ends_with("Breadth. Review is not counted (GitHub data unavailable).")
    );
}

// ---- in a window ----------------------------------------------------------------------

fn window_fake(raw: RawData) -> Arc<Fake> {
    let fake = Fake::new();
    let october = current_period(PeriodKind::Month, &start_of_tests(), None);
    let mut raw = raw;
    raw.since = sync_window(&october, &start_of_tests()).from.to_rfc3339();
    fake.set_load(LoadResponse {
        raw,
        covered: true,
        has_gaps: false,
        missing: vec![],
        missing_commits: 0,
        synced_at: Some(start_of_tests().timestamp() - 300),
        coverage_head: Some("h1".into()),
        head: "h1".into(),
        gh_state: Some("complete".into()),
    });
    fake.with(|s| {
        s.config.default = Ok(Some(serialize_config(&default_config())));
        s.saved_range = Some(serialize_pick(&PeriodPick {
            kind: PeriodKind::Month,
            id: Some("2026-10".into()),
        }));
    });
    fake
}

pub(super) fn leaderboard_window(
    cx: &mut TestAppContext,
    fake: Arc<Fake>,
) -> (Ui, VisualTestContext) {
    let (ui, mut v) = github(cx, fake);
    settle(&mut v);
    click(&mut v, "contributors-section-leaderboard");
    assert!(
        shown(&mut v, "contributors-body-leaderboard"),
        "the Leaderboard section is shown"
    );
    (ui, v)
}

fn window_of_board(cx: &mut TestAppContext) -> (Ui, VisualTestContext) {
    leaderboard_window(cx, window_fake(board_raw()))
}

pub(super) fn view_model(ui: &Ui, v: &mut VisualTestContext) -> Arc<ContributorsModel> {
    let view = ui.view(v);
    v.read(|cx| view.read(cx).snapshot(cx).model.expect("a model"))
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[gpui::test]
fn the_section_shows_the_podium_the_table_and_a_row_per_person(cx: &mut TestAppContext) {
    let (_, mut v) = window_of_board(cx);
    for selector in [
        "contributors-lb-podium",
        "contributors-lb-podium-0",
        "contributors-lb-podium-1",
        "contributors-lb-podium-2",
        "contributors-lb-table",
        "contributors-lb-row-0",
        "contributors-lb-row-3",
        "contributors-lb-how",
    ] {
        assert!(shown(&mut v, selector), "{selector}");
    }
    assert!(!shown(&mut v, "contributors-lb-podium-3"));
    assert!(!shown(&mut v, "contributors-lb-row-4"));
    assert!(!shown(&mut v, "contributors-lb-empty"));
}

#[gpui::test]
fn the_you_chip_is_on_the_git_users_row_and_the_podium_card_only(cx: &mut TestAppContext) {
    let (_, mut v) = window_of_board(cx);
    assert!(shown(&mut v, "contributors-lb-you-0"));
    assert!(shown(&mut v, "contributors-lb-podium-you-0"));
    for i in 1..4 {
        assert!(!shown(&mut v, leak(format!("contributors-lb-you-{i}"))));
    }
    assert!(!shown(&mut v, "contributors-lb-podium-you-1"));
}

#[gpui::test]
fn the_podium_shows_2nd_left_1st_middle_3rd_right_on_a_wide_page(cx: &mut TestAppContext) {
    let (_, mut v) = window_of_board(cx);
    let x = |v: &mut VisualTestContext, i: usize| {
        v.debug_bounds(leak(format!("contributors-lb-podium-{i}")))
            .unwrap()
            .center()
            .x
    };
    let (first, second, third) = (x(&mut v, 0), x(&mut v, 1), x(&mut v, 2));
    assert!(second < first && first < third, "{second} {first} {third}");
}

#[gpui::test]
fn the_podium_stacks_in_rank_order_on_a_narrow_page(cx: &mut TestAppContext) {
    let (_, mut v) = window_of_board(cx);
    v.simulate_resize(gpui::size(gpui::px(480.), gpui::px(900.)));
    settle(&mut v);
    settle(&mut v);
    let y = |v: &mut VisualTestContext, i: usize| {
        v.debug_bounds(leak(format!("contributors-lb-podium-{i}")))
            .unwrap()
            .center()
            .y
    };
    let (first, second, third) = (y(&mut v, 0), y(&mut v, 1), y(&mut v, 2));
    assert!(first < second && second < third, "{first} {second} {third}");
}

fn watch_asks(ui: &Ui, v: &mut VisualTestContext) -> Rc<RefCell<Vec<SnapshotScope>>> {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = asked.clone();
    let view: Entity<GithubView> = ui.view(v);
    let subscription = v.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &GithubEvent, _| {
            let GithubEvent::Ask(scope) = event;
            log.borrow_mut().push(scope.clone());
        })
    });
    std::mem::forget(subscription);
    asked
}

#[gpui::test]
fn ask_reports_the_person_of_the_row(cx: &mut TestAppContext) {
    let (ui, mut v) = window_of_board(cx);
    let asked = watch_asks(&ui, &mut v);
    let bob = view_model(&ui, &mut v).leaderboard.rows[1]
        .person_id
        .clone();
    click(&mut v, "contributors-lb-ask-1");
    assert_eq!(*asked.borrow(), [SnapshotScope::Person { person_id: bob }]);
}

#[gpui::test]
fn an_empty_period_explains_itself_and_still_offers_the_formula(cx: &mut TestAppContext) {
    let (_, mut v) = leaderboard_window(cx, window_fake(empty_raw()));
    assert!(shown(&mut v, "contributors-lb-empty"));
    assert!(!shown(&mut v, "contributors-lb-table"));
    assert!(!shown(&mut v, "contributors-lb-podium"));
    assert!(shown(&mut v, "contributors-lb-how"));
}

#[gpui::test]
fn the_view_model_ranks_the_same_people_as_the_pure_model(cx: &mut TestAppContext) {
    let (ui, mut v) = window_of_board(cx);
    let model = view_model(&ui, &mut v);
    assert_eq!(
        names(&model),
        ["Ana", "Bob", "Cat", "Dan"],
        "the fixture of the window matches the pure one"
    );
    let row: &LeaderboardRow = &model.leaderboard.rows[0];
    assert!(row.is_me);
}

#[test]
fn every_badge_and_section_glyph_is_a_bundled_asset() {
    use gpui::AssetSource as _;
    // The glyph keys of the core's six badges, then the other icons of the section.
    let keys = ["fire", "hammer", "broom", "bug", "eyes", "sprout"];
    assert_eq!(
        keys.len(),
        monocode_core::contributors::model::leaderboard::BadgeId::ALL.len()
    );
    for key in keys {
        let asset = leaderboard::badge_asset(key);
        let bytes = crate::assets::Assets.load(asset).unwrap();
        assert!(bytes.is_some_and(|b| !b.is_empty()), "{key} -> {asset}");
    }
    for asset in ["trophy", "users", "bot", "sparkles", "x"] {
        assert!(
            crate::assets::Assets.load(asset).unwrap().is_some(),
            "{asset}"
        );
    }
}

#[gpui::test]
fn every_header_cell_is_drawn_including_the_unlabelled_actions_column(cx: &mut TestAppContext) {
    let (_, mut v) = window_of_board(cx);
    for (i, head) in HEADS.iter().enumerate() {
        assert!(
            shown(&mut v, leak(format!("contributors-lb-head-{i}"))),
            "{}",
            head
        );
    }
    assert!(!shown(
        &mut v,
        leak(format!("contributors-lb-head-{}", HEADS.len()))
    ));
}
