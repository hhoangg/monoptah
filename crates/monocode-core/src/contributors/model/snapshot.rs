//! The JSON snapshot an agent reads, and its prompt. Port of `snapshot.ts`.
//! The serialized shape (key names, key order, numbers rounded to two
//! decimals, whole numbers without a fraction) is the one the desktop app
//! writes, so an agent sees the same file from either app.

use super::identity::Resolver;
use super::leaderboard::{Leaderboard, LeaderboardRow};
use super::period::{is_in_period, parse_iso, Period};
use super::scoring::{attribute_commits, Parts, PeriodScores, PrScore, SCORING};
use super::stats::{Kpis, Stats};
use super::types::{ContributorsConfig, Weights};
use super::{js_iso, js_number_text, js_round, serialize_number, serialize_opt_number};
use crate::contributors::RawData;
use chrono::{DateTime, Local};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SnapshotScope {
    Team,
    Person {
        #[serde(rename = "personId")]
        person_id: String,
    },
    Me {
        #[serde(rename = "personId")]
        person_id: String,
    },
}

impl SnapshotScope {
    /// The person the question is about, if any.
    pub fn person_id(&self) -> Option<&str> {
        match self {
            Self::Team => None,
            Self::Person { person_id } | Self::Me { person_id } => Some(person_id),
        }
    }
}

/// Unweighted subtotals; `delivery` is before quality, `delivery_final` after.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Breakdown {
    #[serde(serialize_with = "serialize_number")]
    pub delivery: f64,
    #[serde(serialize_with = "serialize_number")]
    pub delivery_final: f64,
    #[serde(serialize_with = "serialize_number")]
    pub quality: f64,
    #[serde(serialize_with = "serialize_opt_number")]
    pub survival: Option<f64>,
    #[serde(serialize_with = "serialize_number")]
    pub care: f64,
    #[serde(serialize_with = "serialize_number")]
    pub review: f64,
    #[serde(serialize_with = "serialize_number")]
    pub consistency: f64,
    #[serde(serialize_with = "serialize_number")]
    pub breadth: f64,
}

/// The `Option` fields are left out of the JSON when `None`: Ask Agent drops
/// them when the user removes a chip.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPerson {
    pub person_id: String,
    pub name: String,
    pub rank: usize,
    pub movement: Option<i64>,
    #[serde(serialize_with = "serialize_number")]
    pub score: f64,
    /// Weighted contributions to `score` (quality and weights applied); they add up to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parts: Option<Parts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<Breakdown>,
    pub active_days: usize,
    pub streak: usize,
    pub merged_prs: usize,
    pub main_commits: usize,
    pub reviewed_prs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_assisted_pct: Option<u32>,
    pub modules: Vec<String>,
    pub badges: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identities: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotPr {
    pub id: String,
    pub title: String,
    #[serde(serialize_with = "serialize_number")]
    pub points: f64,
    #[serde(serialize_with = "serialize_number")]
    pub delivery_share: f64,
    #[serde(serialize_with = "serialize_number")]
    pub care: f64,
    #[serde(serialize_with = "serialize_number")]
    pub iteration: f64,
    #[serde(serialize_with = "serialize_number")]
    pub size: f64,
    pub added: u64,
    pub deleted: u64,
    pub commit_count: u64,
    pub test_files: bool,
    pub docs: bool,
    pub net_negative: bool,
    pub fix: bool,
    pub reverted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_assisted: Option<bool>,
    pub merged_at: String,
    pub person_id: String,
    pub person_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCommit {
    pub hash: String,
    pub author: String,
    pub person: Option<String>,
    pub date: String,
    pub files: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SnapshotPeriod {
    pub kind: String,
    pub id: String,
    pub label: String,
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Formula {
    pub constants: super::scoring::Scoring,
    pub weights: Weights,
    pub ignore: Vec<String>,
    pub description: Vec<String>,
}

/// A stats row without its color.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotStatsRow {
    pub person_id: String,
    pub name: String,
    pub commits: u64,
    pub added: u64,
    pub deleted: u64,
    pub files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_assisted_pct: Option<u32>,
    #[serde(serialize_with = "serialize_number")]
    pub share_pct: f64,
    pub last_active: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identities: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SnapshotStats {
    pub kpis: Kpis,
    pub rows: Vec<SnapshotStatsRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub generated_at: String,
    pub project: String,
    pub branch: String,
    pub period: SnapshotPeriod,
    pub scope: SnapshotScope,
    pub gh_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<Formula>,
    pub people: Vec<SnapshotPerson>,
    /// Present for a person or me scope; `Some(None)` when that person is not
    /// ranked (printed as `null`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<Option<SnapshotPerson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<SnapshotStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prs: Option<Vec<SnapshotPr>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<Vec<SnapshotCommit>>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Include {
    pub per_pr: bool,
    pub raw_commits: bool,
}

const MAX_COMMITS: usize = 2000;

pub(crate) fn round2(n: f64) -> f64 {
    js_round(n * 100.0) / 100.0
}

fn round_or_null(n: Option<f64>) -> Option<f64> {
    n.map(round2)
}

pub fn snapshot_file_name(period: &Period) -> String {
    format!("contributors-{}.json", period.id)
}

pub(crate) fn formula_description() -> Vec<String> {
    let s = &SCORING;
    let n = js_number_text;
    vec![
        "Score = Delivery * Quality + Care + Review + Consistency + Breadth. Each dimension is multiplied by its weight before it is added.".into(),
        format!("Delivery: every merged PR earns {} + {} * log2(1 + effective lines) + iteration, capped at {}. Effective lines = added + 0.5 * deleted of the final diff, after the ignore globs (lockfiles, generated files, snapshots, vendored code). Squash, rebase and merge commits are scored alike because the PR is the unit.", n(s.pr_base), n(s.size_k), n(s.pr_cap)),
        format!("Iteration: {} * log2(1 + commits in the PR), capped at {}, so work hidden by a squash still counts. Closed PRs earn no Delivery.", n(s.iter_k), n(s.iter_cap)),
        format!("Quality: a multiplier on Delivery only, {} + {} * survival, clamped to {}-{}. Survival is the share of the lines a person wrote that are still alive at HEAD (measured for PRs at least 30 days old); no sample means 1.0. A reverted PR loses its Delivery and Care.", n(s.quality_base), n(s.quality_slope), n(s.quality_min), n(s.quality_max)),
        format!("Care: a bonus on a PR's points, +{}% when it touches tests, +{}% docs, +{}% when it deletes more than it adds, +{}% for a fix or a closed bug issue.", n(s.care_test * 100.0), n(s.care_docs * 100.0), n(s.care_net_negative * 100.0), n(s.care_fix * 100.0)),
        format!("Review (needs the GitHub CLI): {} points per PR reviewed for someone else, plus {} per substantive comment, at most {} per PR. Reviewing your own PR earns nothing.", n(s.review_pr), n(s.review_comment), n(s.review_comment_cap)),
        format!("Consistency: {} points per active day (days with a commit on the mainline or on a PR branch) plus {} per day of the longest streak, capped at {} days. Weekends are skipped for the streak: they neither break it nor extend it, but a weekend day with a commit is still an active day.", n(s.active_day), n(s.streak_k), n(s.streak_cap)),
        format!("Breadth: {} points for each module (first two folders of a path) with at least {} effective lines of the person's own PRs, at most {} modules.", n(s.module_pts), n(s.module_min_eff), n(s.module_cap)),
        "Co-authors: with human Co-authored-by trailers the PR author gets 60% and the others share 40%. Agents never take a share.".into(),
        "AI agents: work co-authored by an agent counts fully for the human behind it. Agent-authored work goes to the PR author, then the committer, then the identity mapping. AI-assisted % is informational and never changes a score.".into(),
        "Periods are the last 7 days, a calendar month, quarter or year, or all time (from the first commit to today), in local time, and days are bucketed by committer date. Rank movement is the previous rank minus the current rank; all time has no previous period, so it shows no rank movement and no comparison.".into(),
    ]
}

fn to_person(row: &LeaderboardRow, resolver: &Resolver) -> SnapshotPerson {
    // Each part is rounded first and the score is their sum, so the numbers the
    // agent reads always add up.
    let parts = Parts {
        delivery: round2(row.parts.delivery),
        care: round2(row.parts.care),
        review: round2(row.parts.review),
        consistency: round2(row.parts.consistency),
        breadth: round2(row.parts.breadth),
    };
    let score = round2(
        0.0 + parts.delivery + parts.care + parts.review + parts.consistency + parts.breadth,
    );
    let identities = resolver
        .people()
        .iter()
        .find(|p| p.id == row.person_id)
        .map_or_else(|| vec![row.person_id.clone()], |p| p.identities.clone());
    SnapshotPerson {
        person_id: row.person_id.clone(),
        name: row.name.clone(),
        rank: row.rank,
        movement: row.movement,
        score,
        parts: Some(parts),
        breakdown: Some(Breakdown {
            delivery: round2(row.delivery),
            delivery_final: round2(row.delivery_final),
            quality: round2(row.quality),
            survival: round_or_null(row.survival),
            care: round2(row.care),
            review: round2(row.review),
            consistency: round2(row.consistency),
            breadth: round2(row.breadth),
        }),
        active_days: row.active_days,
        streak: row.streak,
        merged_prs: row.merged_prs,
        main_commits: row.main_commits,
        reviewed_prs: row.reviewed_prs,
        ai_assisted_pct: Some(row.ai_assisted_pct),
        modules: row.modules.clone(),
        badges: row.badges.iter().map(|b| b.label.to_string()).collect(),
        identities: Some(identities),
    }
}

fn to_pr(row: &LeaderboardRow, pr: &PrScore) -> SnapshotPr {
    SnapshotPr {
        id: pr.id.clone(),
        title: pr.title.clone(),
        points: round2(pr.points),
        delivery_share: round2(pr.delivery_share),
        care: round2(pr.care),
        iteration: round2(pr.iteration),
        size: round2(pr.size),
        added: pr.added,
        deleted: pr.deleted,
        commit_count: pr.commit_count,
        test_files: pr.test_files,
        docs: pr.docs,
        net_negative: pr.net_negative,
        fix: pr.fix,
        reverted: pr.reverted,
        ai_assisted: Some(pr.ai_assisted),
        merged_at: pr.merged_at.clone(),
        person_id: row.person_id.clone(),
        person_name: row.name.clone(),
    }
}

fn build_notes(
    scores: &PeriodScores,
    raw: Option<&RawData>,
    gh_available: bool,
    commits_capped: bool,
    weights: &Weights,
) -> Vec<String> {
    let mut notes = vec!["Scores are for motivation, not for performance reviews.".to_string()];
    let custom: Vec<String> = [
        ("delivery", weights.delivery),
        ("review", weights.review),
        ("care", weights.care),
        ("consistency", weights.consistency),
        ("breadth", weights.breadth),
    ]
    .iter()
    .filter(|(_, w)| *w != 1.0)
    .map(|(k, w)| format!("{k} {}", js_number_text(*w)))
    .collect();
    if !custom.is_empty() {
        notes.push(format!(
            "Custom weights are in use ({}). \"parts\" already include them; \"breakdown\" does not.",
            custom.join(", ")
        ));
    }
    if !gh_available {
        let why = raw
            .and_then(|r| r.gh_note.as_deref())
            .filter(|n| !n.is_empty())
            .map_or_else(String::new, |n| format!(" ({n})"));
        notes.push(format!(
            "Review points are not counted because the GitHub CLI is unavailable{why}"
        ));
    }
    let (prs, commits) = (scores.unattributed.prs, scores.unattributed.commits);
    let mut parts = vec![];
    if prs > 0 {
        parts.push(format!("{prs} {}", if prs == 1 { "PR" } else { "PRs" }));
    }
    if commits > 0 {
        parts.push(format!(
            "{commits} {}",
            if commits == 1 { "commit" } else { "commits" }
        ));
    }
    if !parts.is_empty() {
        notes.push(format!(
            "Work by AI agents that no human could be credited for is not scored: {}.",
            parts.join(" and ")
        ));
    }
    if raw.is_some_and(|r| r.truncated) {
        notes.push(
            "The collected history hit a safety cap, so older activity may be missing.".into(),
        );
    }
    if commits_capped {
        notes.push(format!(
            "Only the {MAX_COMMITS} most recent commits are listed."
        ));
    }
    notes
}

pub struct SnapshotInput<'a> {
    pub project: &'a str,
    pub branch: &'a str,
    pub period: &'a Period,
    pub scores: &'a PeriodScores,
    pub leaderboard: &'a Leaderboard,
    pub stats: Option<&'a Stats>,
    pub config: &'a ContributorsConfig,
    pub scope: SnapshotScope,
    pub include: Include,
    pub raw: Option<&'a RawData>,
    pub gh_available: bool,
    pub resolver: &'a Resolver,
    pub now: &'a DateTime<Local>,
}

pub fn build_snapshot(input: &SnapshotInput) -> Snapshot {
    let (period, resolver) = (input.period, input.resolver);
    let people: Vec<SnapshotPerson> = input
        .leaderboard
        .rows
        .iter()
        .map(|row| to_person(row, resolver))
        .collect();
    let focus_id = input.scope.person_id();

    let mut commits: Option<Vec<SnapshotCommit>> = None;
    let mut commits_capped = false;
    if let (true, Some(raw)) = (input.include.raw_commits, input.raw) {
        let mut in_period: Vec<_> = attribute_commits(raw, input.config, resolver)
            .into_iter()
            .filter(|c| {
                is_in_period(period, &c.commit.committed_at)
                    && focus_id.is_none_or(|id| c.person_id.as_deref() == Some(id))
            })
            .collect();
        in_period.sort_by_key(|c| {
            std::cmp::Reverse(parse_iso(&c.commit.committed_at).map(|t| t.timestamp_millis()))
        });
        commits_capped = in_period.len() > MAX_COMMITS;
        commits = Some(
            in_period
                .into_iter()
                .take(MAX_COMMITS)
                .map(|c| SnapshotCommit {
                    hash: c.commit.hash.chars().take(12).collect(),
                    author: format!("{} <{}>", c.commit.author.name, c.commit.author.email),
                    person: c.person_id.as_deref().map(|id| resolver.name_of(id)),
                    date: c.commit.committed_at.clone(),
                    files: c.files.len(),
                })
                .collect(),
        );
    }

    let focus = focus_id.map(|id| people.iter().find(|p| p.person_id == id).cloned());
    let stats = input.stats.map(|stats| SnapshotStats {
        kpis: stats.kpis.clone(),
        rows: stats
            .rows
            .iter()
            .map(|r| SnapshotStatsRow {
                person_id: r.person_id.clone(),
                name: r.name.clone(),
                commits: r.commits,
                added: r.added,
                deleted: r.deleted,
                files: r.files,
                ai_assisted_pct: Some(r.ai_assisted_pct),
                share_pct: r.share_pct,
                last_active: r.last_active.clone(),
                identities: Some(r.identities.clone()),
            })
            .collect(),
    });
    let prs = input.include.per_pr.then(|| {
        input
            .leaderboard
            .rows
            .iter()
            .filter(|row| focus_id.is_none_or(|id| row.person_id == id))
            .flat_map(|row| row.prs.iter().map(|pr| to_pr(row, pr)))
            .collect()
    });

    Snapshot {
        generated_at: js_iso(input.now),
        project: input.project.to_string(),
        branch: input.branch.to_string(),
        period: SnapshotPeriod {
            kind: period.kind.as_str().to_string(),
            id: period.id.clone(),
            label: period.label.clone(),
            start: js_iso(&period.start),
            end: js_iso(&period.end),
        },
        scope: input.scope.clone(),
        gh_available: input.gh_available,
        formula: Some(Formula {
            constants: SCORING,
            weights: input.config.weights.clone(),
            ignore: input.config.ignore.clone(),
            description: formula_description(),
        }),
        people,
        focus,
        stats,
        prs,
        commits,
        notes: build_notes(
            input.scores,
            input.raw,
            input.gh_available,
            commits_capped,
            &input.config.weights,
        ),
    }
}

// These two compare with a previous period, which All time does not have.
const TREND_QUESTION: &str = "Compare this period with the last 3 and explain the trend.";
const MOVEMENT_QUESTION: &str = "Why did my rank change since the previous period?";

const TEAM_QUESTIONS: [&str; 4] = [
    "Summarize this period: who drove what, and where are the risks?",
    "Which modules depend on a single person (bus factor)?",
    TREND_QUESTION,
    "Are any scores inflated by generated code or huge PRs?",
];

const ME_QUESTIONS: [&str; 3] = [
    MOVEMENT_QUESTION,
    "What would raise my Quality factor?",
    "Which of my PRs had the most rework, and why?",
];

/// What is asked in place of a question that needs a previous period.
fn without_previous(question: &str) -> &str {
    match question {
        TREND_QUESTION => "How has activity changed over the years, and who carried each stretch?",
        MOVEMENT_QUESTION => "Where do my contributions stand compared with the rest of the team?",
        other => other,
    }
}

/// `has_previous` is false for All time: the questions that compare with an
/// earlier period are replaced by ones that make sense without it.
pub fn suggested_questions(
    scope: &SnapshotScope,
    name: Option<&str>,
    has_previous: bool,
) -> Vec<String> {
    let fit = |questions: &[&str]| -> Vec<String> {
        questions
            .iter()
            .map(|q| {
                if has_previous {
                    *q
                } else {
                    without_previous(q)
                }
                .to_string()
            })
            .collect()
    };
    match scope {
        SnapshotScope::Team => fit(&TEAM_QUESTIONS),
        SnapshotScope::Me { .. } => fit(&ME_QUESTIONS),
        SnapshotScope::Person { .. } => {
            let who = name.unwrap_or("this person");
            vec![
                format!("Summarize {who}'s contributions this period."),
                format!("What are {who}'s strengths across the 5 dimensions?"),
                format!("Show {who}'s PRs with the most iteration and what slowed them down."),
                format!("Is {who}'s Delivery low because of review or mentoring work?"),
            ]
        }
    }
}

fn scope_line(scope: &SnapshotScope, person_name: Option<&str>) -> String {
    let Some(id) = scope.person_id() else {
        return "the whole team".into();
    };
    let who = match person_name {
        Some(name) if !name.is_empty() => format!("{name} ({id})"),
        _ => id.to_string(),
    };
    match scope {
        SnapshotScope::Me { .. } => format!("one person asking about their own score: {who}"),
        _ => format!("one person: {who}"),
    }
}

const DEFAULT_CONTENTS: [&str; 4] = [
    "scores",
    "breakdowns",
    "identity mapping",
    "the scoring formula in use",
];

fn list_of(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

pub struct AgentPrompt<'a> {
    pub question: &'a str,
    pub scope: &'a SnapshotScope,
    pub file_name: &'a str,
    pub allow_tools: bool,
    pub project_name: &'a str,
    pub period_label: &'a str,
    pub person_name: Option<&'a str>,
    /// What the file holds, as noun phrases; the full snapshot when `None`.
    pub contains: Option<&'a [String]>,
}

pub fn build_agent_prompt(input: &AgentPrompt) -> String {
    let tools = if input.allow_tools {
        "You may run read-only git and gh commands inside this project to dig deeper. Do not change anything."
    } else {
        "Do not run commands; answer only from the attached snapshot."
    };
    let default_contents: Vec<String> = DEFAULT_CONTENTS.iter().map(|s| s.to_string()).collect();
    let contains = list_of(input.contains.unwrap_or(&default_contents));
    [
        format!(
            "I am looking at the Contributors leaderboard of the project \"{}\" for the period \"{}\".",
            input.project_name, input.period_label
        ),
        format!(
            "Scope of my question: {}.",
            scope_line(input.scope, input.person_name)
        ),
        format!(
            "The attached file {} is the exact data I see: {contains}. Treat it as the source of truth for every number.",
            input.file_name
        ),
        tools.to_string(),
        String::new(),
        format!("Question: {}", input.question),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::leaderboard::build_leaderboard;
    use crate::contributors::model::period::{current_period, PeriodKind};
    use crate::contributors::model::scoring::score_period;
    use crate::contributors::model::stats::{build_stats, Previous, StatsInput};
    use crate::contributors::model::test_fixtures::*;
    use crate::contributors::model::types::{ContributorsConfig, Weights};
    use crate::contributors::RawData;

    fn now() -> DateTime<Local> {
        local_at(2026, 9, 15, 9, 30)
    }

    const ANA_ID: &str = "ana@example.com";

    fn none() -> Include {
        Include {
            per_pr: false,
            raw_commits: false,
        }
    }

    fn build(raw: &RawData, scope: SnapshotScope) -> Snapshot {
        build_with(raw, scope, none(), &default_config(), false)
    }

    fn build_with(
        raw: &RawData,
        scope: SnapshotScope,
        include: Include,
        config: &ContributorsConfig,
        with_stats: bool,
    ) -> Snapshot {
        let resolver = make_resolver_with(raw, config);
        let scores = score_period(raw, config, &resolver, &period(), &period_now());
        let leaderboard = build_leaderboard(&scores, None, None);
        let stats = with_stats.then(|| {
            build_stats(&StatsInput {
                raw,
                config,
                resolver: &resolver,
                range_start: period().start,
                range_end: period().end,
                previous: Previous::Default,
                granularity: None,
                colors: None,
            })
        });
        build_snapshot(&SnapshotInput {
            project: "monoptah",
            branch: "main",
            period: &period(),
            scores: &scores,
            leaderboard: &leaderboard,
            stats: stats.as_ref(),
            config,
            scope,
            include,
            raw: Some(raw),
            gh_available: raw.gh_available,
            resolver: &resolver,
            now: &now(),
        })
    }

    fn sample() -> RawData {
        raw_with(|r| {
            r.prs = vec![
                pr_with("1", &ana(), &oct12(8), |p| {
                    p.title = "fix: crash".into();
                    p.landing_co_authors = vec![claude()];
                    p.files = vec![file("src/a.ts", 100), file("src/a.test.ts", 20)];
                }),
                make_pr("2", &bob(), &oct12(9)),
            ];
            r.commits = vec![
                commit_with("aaaaaaaaaaaaaaaaaaaa", &ana(), &oct12(8), |c| {
                    c.pr_id = Some("1".into());
                    c.files = vec![file("src/a.ts", 1), file("Cargo.lock", 500)];
                }),
                commit_with("bbbbbbbbbbbbbbbbbbbb", &bob(), &oct12(9), |c| {
                    c.pr_id = Some("2".into())
                }),
            ];
            r.reviews = vec![make_review("1", &bob(), &oct12(8), 2, "APPROVED")];
        })
    }

    fn person_scope(id: &str) -> SnapshotScope {
        SnapshotScope::Person {
            person_id: id.into(),
        }
    }

    fn me_scope(id: &str) -> SnapshotScope {
        SnapshotScope::Me {
            person_id: id.into(),
        }
    }

    fn parts_sum(p: &Parts) -> f64 {
        p.delivery + p.care + p.review + p.consistency + p.breadth
    }

    fn json(snapshot: &Snapshot) -> serde_json::Value {
        serde_json::to_value(snapshot).unwrap()
    }

    // snapshotFileName

    #[test]
    fn snapshot_file_name_names_the_file_after_the_period() {
        assert_eq!(snapshot_file_name(&period()), "contributors-2026-10.json");
    }

    #[test]
    fn snapshot_file_name_names_the_file_of_all_time() {
        let all = current_period(
            PeriodKind::All,
            &local_date(2026, 9, 20),
            Some(&local_date(2024, 0, 5)),
        );
        assert_eq!(snapshot_file_name(&all), "contributors-all.json");
    }

    // suggestedQuestions

    #[test]
    fn suggested_questions_has_the_team_suggestions() {
        assert_eq!(
            suggested_questions(&SnapshotScope::Team, None, true),
            [
                "Summarize this period: who drove what, and where are the risks?",
                "Which modules depend on a single person (bus factor)?",
                "Compare this period with the last 3 and explain the trend.",
                "Are any scores inflated by generated code or huge PRs?",
            ]
        );
    }

    #[test]
    fn suggested_questions_has_the_suggestions_about_yourself() {
        assert_eq!(
            suggested_questions(&me_scope("x"), None, true),
            [
                "Why did my rank change since the previous period?",
                "What would raise my Quality factor?",
                "Which of my PRs had the most rework, and why?",
            ]
        );
    }

    #[test]
    fn suggested_questions_interpolates_the_name_for_a_person() {
        assert_eq!(
            suggested_questions(&person_scope("x"), Some("Tuan Nguyen"), true),
            [
                "Summarize Tuan Nguyen's contributions this period.",
                "What are Tuan Nguyen's strengths across the 5 dimensions?",
                "Show Tuan Nguyen's PRs with the most iteration and what slowed them down.",
                "Is Tuan Nguyen's Delivery low because of review or mentoring work?",
            ]
        );
    }

    #[test]
    fn suggested_questions_drops_the_questions_about_a_previous_period_when_there_is_none() {
        let team = suggested_questions(&SnapshotScope::Team, None, false);
        assert_eq!(team.len(), 4);
        let text = team.join("\n");
        assert!(!text.contains("last 3") && !text.contains("previous period"));
        let me = suggested_questions(&me_scope("x"), None, false);
        assert_eq!(me.len(), 3);
        let text = me.join("\n");
        assert!(!text.contains("last 3") && !text.contains("previous period"));
        // Nothing changes for a person, or when a previous period exists.
        assert_eq!(
            suggested_questions(&person_scope("x"), Some("Tuan"), false),
            suggested_questions(&person_scope("x"), Some("Tuan"), true)
        );
        assert_eq!(
            suggested_questions(&SnapshotScope::Team, None, true),
            suggested_questions(&SnapshotScope::Team, None, true)
        );
    }

    #[test]
    fn suggested_questions_falls_back_to_a_neutral_name() {
        assert_eq!(
            suggested_questions(&person_scope("x"), None, true)[0],
            "Summarize this person's contributions this period."
        );
    }

    // buildAgentPrompt

    fn prompt_of(scope: SnapshotScope, person_name: Option<&str>, allow_tools: bool) -> String {
        build_agent_prompt(&AgentPrompt {
            question: "Why is Ana first?",
            scope: &scope,
            file_name: "contributors-2026-10.json",
            allow_tools,
            project_name: "monoptah",
            period_label: "Oct 2026",
            person_name,
            contains: None,
        })
    }

    #[test]
    fn build_agent_prompt_states_project_period_file_and_question() {
        let text = prompt_of(SnapshotScope::Team, None, true);
        assert!(text.contains("monoptah"));
        assert!(text.contains("Oct 2026"));
        assert!(text.contains("contributors-2026-10.json"));
        assert!(text.contains("exact data"));
        assert!(text.contains("Why is Ana first?"));
        assert!(text.contains("the whole team"));
    }

    #[test]
    fn build_agent_prompt_allows_read_only_git_and_gh_when_asked() {
        let text = prompt_of(SnapshotScope::Team, None, true);
        assert!(text.contains(
            "You may run read-only git and gh commands inside this project to dig deeper"
        ));
        assert!(!text.contains("Do not run commands"));
    }

    #[test]
    fn build_agent_prompt_forbids_commands_otherwise() {
        let text = prompt_of(SnapshotScope::Team, None, false);
        assert!(text.contains("Do not run commands; answer only from the attached snapshot"));
        assert!(!text.contains("You may run"));
    }

    #[test]
    fn build_agent_prompt_names_the_person_in_scope() {
        let person = prompt_of(person_scope(ANA_ID), Some("Ana"), false);
        assert!(person.contains("Ana"));
        assert!(person.contains(ANA_ID));
        let me = prompt_of(me_scope(ANA_ID), Some("Ana"), false);
        assert!(me.contains("their own score"));
        assert!(me.contains(ANA_ID));
    }

    #[test]
    fn build_agent_prompt_keeps_the_exact_line_layout() {
        let text = prompt_of(SnapshotScope::Team, None, false);
        assert_eq!(
            text,
            "I am looking at the Contributors leaderboard of the project \"monoptah\" for the period \"Oct 2026\".\n\
             Scope of my question: the whole team.\n\
             The attached file contributors-2026-10.json is the exact data I see: scores, breakdowns, identity mapping and the scoring formula in use. Treat it as the source of truth for every number.\n\
             Do not run commands; answer only from the attached snapshot.\n\
             \n\
             Question: Why is Ana first?"
        );
    }

    // buildSnapshot

    #[test]
    fn build_snapshot_describes_the_project_period_and_generation_time() {
        let snap = build(&sample(), SnapshotScope::Team);
        assert_eq!(snap.generated_at, iso(&now()));
        assert_eq!(snap.project, "monoptah");
        assert_eq!(snap.branch, "main");
        assert_eq!(
            snap.period,
            SnapshotPeriod {
                kind: "month".into(),
                id: "2026-10".into(),
                label: "Oct 2026".into(),
                start: iso(&period().start),
                end: iso(&period().end),
            }
        );
        assert_eq!(snap.scope, SnapshotScope::Team);
        assert!(snap.gh_available);
    }

    #[test]
    fn build_snapshot_labels_all_time_and_says_how_periods_and_rank_movement_work() {
        let raw = sample();
        let all = current_period(
            PeriodKind::All,
            &period_now(),
            Some(&local_date(2026, 8, 1)),
        );
        let config = default_config();
        let resolver = make_resolver_with(&raw, &config);
        let scores = score_period(&raw, &config, &resolver, &all, &period_now());
        let snap = build_snapshot(&SnapshotInput {
            project: "monoptah",
            branch: "main",
            period: &all,
            scores: &scores,
            leaderboard: &build_leaderboard(&scores, None, None),
            stats: None,
            config: &config,
            scope: SnapshotScope::Team,
            include: none(),
            raw: Some(&raw),
            gh_available: raw.gh_available,
            resolver: &resolver,
            now: &now(),
        });
        assert_eq!(
            snap.period,
            SnapshotPeriod {
                kind: "all".into(),
                id: "all".into(),
                label: "All time".into(),
                start: iso(&local_date(2026, 8, 1)),
                end: iso(&all.end),
            }
        );
        let notes = snap.formula.unwrap().description.join(" ");
        assert!(notes.contains("all time"));
        assert!(notes.contains("no rank movement"));
    }

    #[test]
    fn build_snapshot_carries_the_formula_in_use() {
        let config = with_config(|c| {
            c.weights = Weights {
                delivery: 2.0,
                review: 1.0,
                care: 1.0,
                consistency: 1.0,
                breadth: 1.0,
            };
            c.ignore = vec!["dist/**".into()];
        });
        let snap = build_with(&sample(), SnapshotScope::Team, none(), &config, false);
        let formula = snap.formula.unwrap();
        assert_eq!(formula.constants, SCORING);
        assert_eq!(formula.weights, config.weights);
        assert_eq!(formula.ignore, ["dist/**"]);
        assert!(formula.description.len() > 5);
        assert!(formula.description.join(" ").contains("Delivery"));
    }

    #[test]
    fn build_snapshot_lists_ranked_people_with_their_breakdown() {
        let snap = build(&sample(), SnapshotScope::Team);
        assert_eq!(
            snap.people
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["Ana", "Bob"]
        );
        let a = &snap.people[0];
        assert_eq!(a.rank, 1);
        assert_eq!(a.movement, None);
        assert_eq!(a.person_id, ANA_ID);
        assert_eq!(a.identities.as_deref(), Some(&[ANA_ID.to_string()][..]));
        assert_eq!(a.ai_assisted_pct, Some(100));
        let breakdown = a.breakdown.as_ref().unwrap();
        assert_eq!(breakdown.quality, 1.0);
        assert_eq!(breakdown.survival, None);
        assert!(breakdown.delivery > 0.0);
        assert!(breakdown.care > 0.0);
        assert_eq!(snap.people[1].breakdown.as_ref().unwrap().review, 10.0);
        assert_eq!(a.merged_prs, 1);
    }

    #[test]
    fn build_snapshot_rounds_numbers_to_2_decimals() {
        let snap = build(&sample(), SnapshotScope::Team);
        let score = snap.people[0].score;
        assert_eq!(js_round(score * 100.0) / 100.0, score);
    }

    #[test]
    fn build_snapshot_sets_focus_only_for_a_person_or_me_scope() {
        assert!(build(&sample(), SnapshotScope::Team).focus.is_none());
        let person = build(&sample(), person_scope(ANA_ID));
        assert_eq!(person.focus.unwrap().unwrap().name, "Ana");
        let me = build(&sample(), me_scope("nobody"));
        assert_eq!(me.focus, Some(None));
    }

    #[test]
    fn build_snapshot_includes_prs_only_when_asked() {
        assert!(build(&sample(), SnapshotScope::Team).prs.is_none());
        let snap = build_with(
            &sample(),
            SnapshotScope::Team,
            Include {
                per_pr: true,
                raw_commits: false,
            },
            &default_config(),
            false,
        );
        let prs = snap.prs.unwrap();
        assert_eq!(
            prs.iter()
                .map(|p| (p.person_id.as_str(), p.id.as_str()))
                .collect::<Vec<_>>(),
            [(ANA_ID, "1"), ("bob@example.com", "2")]
        );
        assert_eq!(prs[0].person_name, "Ana");
        assert_eq!(prs[0].title, "fix: crash");
        assert!(prs[0].fix);
        assert!(prs[0].test_files);
        assert_eq!(prs[0].ai_assisted, Some(true));
    }

    #[test]
    fn build_snapshot_limits_prs_to_the_focus_person() {
        let snap = build_with(
            &sample(),
            person_scope(ANA_ID),
            Include {
                per_pr: true,
                raw_commits: false,
            },
            &default_config(),
            false,
        );
        assert_eq!(
            snap.prs
                .unwrap()
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["1"]
        );
    }

    #[test]
    fn build_snapshot_lists_only_the_focus_persons_commits_for_a_person_scope() {
        let snap = build_with(
            &sample(),
            person_scope(ANA_ID),
            Include {
                per_pr: false,
                raw_commits: true,
            },
            &default_config(),
            false,
        );
        assert_eq!(
            snap.commits
                .as_ref()
                .unwrap()
                .iter()
                .map(|c| c.person.as_deref())
                .collect::<Vec<_>>(),
            [Some("Ana")]
        );
        assert_eq!(
            snap.people
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["Ana", "Bob"]
        );
    }

    #[test]
    fn build_snapshot_includes_weighted_parts_that_add_up_to_the_score_and_a_weights_note() {
        let config = with_config(|c| {
            c.weights = Weights {
                delivery: 2.0,
                review: 1.0,
                care: 1.0,
                consistency: 1.0,
                breadth: 1.0,
            };
        });
        let snap = build_with(&sample(), SnapshotScope::Team, none(), &config, false);
        let a = &snap.people[0];
        let parts = a.parts.as_ref().unwrap();
        assert_close(parts_sum(parts), a.score, 1);
        assert_close(
            parts.delivery,
            a.breakdown.as_ref().unwrap().delivery_final * 2.0,
            1,
        );
        assert!(snap.notes.join(" ").contains("Custom weights"));
        assert!(!build(&sample(), SnapshotScope::Team)
            .notes
            .join(" ")
            .contains("Custom weights"));
    }

    #[test]
    fn build_snapshot_names_the_custom_weights_in_the_ts_key_order() {
        let config = with_config(|c| {
            c.weights = Weights {
                delivery: 2.0,
                review: 1.0,
                care: 1.5,
                consistency: 1.0,
                breadth: 1.0,
            };
        });
        let snap = build_with(&sample(), SnapshotScope::Team, none(), &config, false);
        assert!(snap.notes.contains(
            &"Custom weights are in use (delivery 2, care 1.5). \"parts\" already include them; \"breakdown\" does not."
                .to_string()
        ));
    }

    #[test]
    fn build_snapshot_rounds_each_part_and_sums_the_rounded_parts_into_the_score() {
        let config = with_config(|c| {
            c.weights = Weights {
                delivery: 1.37,
                review: 0.33,
                care: 2.71,
                consistency: 1.13,
                breadth: 0.77,
            };
        });
        let snap = build_with(&sample(), SnapshotScope::Team, none(), &config, false);
        for person in &snap.people {
            let parts = person.parts.as_ref().unwrap();
            let sum = parts_sum(parts);
            assert_eq!(js_round(sum * 100.0) / 100.0, person.score);
            for part in [
                parts.delivery,
                parts.care,
                parts.review,
                parts.consistency,
                parts.breadth,
            ] {
                assert_eq!(js_round(part * 100.0) / 100.0, part);
            }
        }
    }

    #[test]
    fn build_snapshot_includes_in_period_commits_only_when_asked() {
        assert!(build(&sample(), SnapshotScope::Team).commits.is_none());
        let snap = build_with(
            &sample(),
            SnapshotScope::Team,
            Include {
                per_pr: false,
                raw_commits: true,
            },
            &default_config(),
            false,
        );
        assert_eq!(
            snap.commits.unwrap(),
            [
                SnapshotCommit {
                    hash: "bbbbbbbbbbbb".into(),
                    author: "Bob <bob@example.com>".into(),
                    person: Some("Bob".into()),
                    date: oct12(9),
                    files: 0,
                },
                SnapshotCommit {
                    hash: "aaaaaaaaaaaa".into(),
                    author: "Ana <ana@example.com>".into(),
                    person: Some("Ana".into()),
                    date: oct12(8),
                    files: 1,
                },
            ]
        );
    }

    #[test]
    fn build_snapshot_caps_commits_at_2000_and_says_so() {
        let raw = raw_with(|r| {
            r.commits = (0..2100)
                .map(|i| make_commit(&format!("h{i}"), &ana(), &oct12(1 + (i % 28))))
                .collect();
        });
        let snap = build_with(
            &raw,
            SnapshotScope::Team,
            Include {
                per_pr: false,
                raw_commits: true,
            },
            &default_config(),
            false,
        );
        assert_eq!(snap.commits.unwrap().len(), 2000);
        assert!(snap.notes.join(" ").contains("2000"));
    }

    #[test]
    fn build_snapshot_adds_stats_when_given() {
        let snap = build_with(
            &sample(),
            SnapshotScope::Team,
            none(),
            &default_config(),
            true,
        );
        let stats = snap.stats.as_ref().unwrap();
        assert_eq!(stats.kpis.commits.value, 2);
        assert_eq!(
            stats
                .rows
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            ["Ana", "Bob"]
        );
        assert!(build(&sample(), SnapshotScope::Team).stats.is_none());
    }

    #[test]
    fn build_snapshot_notes_when_the_github_cli_is_unavailable() {
        let mut raw = sample();
        raw.gh_available = false;
        raw.gh_note = Some("gh is not installed".into());
        let snap = build(&raw, SnapshotScope::Team);
        assert!(!snap.gh_available);
        assert!(snap.notes.contains(
            &"Review points are not counted because the GitHub CLI is unavailable (gh is not installed)"
                .to_string()
        ));
    }

    #[test]
    fn build_snapshot_notes_unattributed_work_and_truncated_history() {
        let mut raw = sample();
        raw.truncated = true;
        raw.commits = vec![commit_with("c", &claude(), &oct12(6), |c| {
            c.committer = github()
        })];
        let notes = build(&raw, SnapshotScope::Team).notes.join(" ");
        assert!(notes.contains("1 commit"));
        assert!(notes.contains("safety cap"));
    }

    #[test]
    fn build_snapshot_is_plain_json_without_dashes_that_are_not_hyphens() {
        let snap = build_with(
            &sample(),
            me_scope(ANA_ID),
            Include {
                per_pr: true,
                raw_commits: true,
            },
            &default_config(),
            true,
        );
        let text = serde_json::to_string(&snap).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap(),
            json(&snap)
        );
        assert!(!text.contains('\u{2013}') && !text.contains('\u{2014}'));
    }

    // the JSON an agent reads

    #[test]
    fn build_snapshot_serializes_the_ts_shape_and_key_order() {
        let snap = build_with(
            &sample(),
            person_scope(ANA_ID),
            Include {
                per_pr: true,
                raw_commits: true,
            },
            &default_config(),
            true,
        );
        let text = serde_json::to_string(&snap).unwrap();
        let keys = [
            "\"generatedAt\"",
            "\"project\"",
            "\"branch\"",
            "\"period\"",
            "\"scope\"",
            "\"ghAvailable\"",
            "\"formula\"",
            "\"people\"",
            "\"focus\"",
            "\"stats\"",
            "\"prs\"",
            "\"commits\"",
            "\"notes\"",
        ];
        let mut last = 0;
        for key in keys {
            let at = text[last..]
                .find(key)
                .unwrap_or_else(|| panic!("{key} out of order"));
            last += at;
        }
        assert!(text.contains(r#""scope":{"kind":"person","personId":"ana@example.com"}"#));
        assert!(text.contains(r#""period":{"kind":"month","id":"2026-10","label":"Oct 2026","#));
        // Whole numbers print without a fraction, like JSON.stringify.
        assert!(text.contains(r#""rank":1,"movement":null,"score":"#));
        assert!(text.contains(r#""quality":1,"survival":null,"#));
        let person = &json(&snap)["people"][0];
        let object_keys: Vec<&str> = person
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert!(object_keys.contains(&"aiAssistedPct"));
        let first_pr = &json(&snap)["prs"][0];
        assert_eq!(first_pr["personName"], "Ana");
        assert_eq!(first_pr["deliveryShare"], 1);
    }

    #[test]
    fn build_snapshot_person_fields_follow_the_ts_order() {
        let snap = build(&sample(), SnapshotScope::Team);
        let text = serde_json::to_string(&snap.people[0]).unwrap();
        let order = [
            "personId",
            "name",
            "rank",
            "movement",
            "score",
            "parts",
            "breakdown",
            "activeDays",
            "streak",
            "mergedPrs",
            "mainCommits",
            "reviewedPrs",
            "aiAssistedPct",
            "modules",
            "badges",
            "identities",
        ];
        let mut last = 0;
        for key in order {
            let at = text[last..]
                .find(&format!("\"{key}\":"))
                .unwrap_or_else(|| panic!("{key} out of order in {text}"));
            last += at;
        }
    }

    #[test]
    fn build_snapshot_pr_fields_follow_the_ts_order() {
        let snap = build_with(
            &sample(),
            SnapshotScope::Team,
            Include {
                per_pr: true,
                raw_commits: false,
            },
            &default_config(),
            false,
        );
        let text = serde_json::to_string(&snap.prs.unwrap()[0]).unwrap();
        let order = [
            "id",
            "title",
            "points",
            "deliveryShare",
            "care",
            "iteration",
            "size",
            "added",
            "deleted",
            "commitCount",
            "testFiles",
            "docs",
            "netNegative",
            "fix",
            "reverted",
            "aiAssisted",
            "mergedAt",
            "personId",
            "personName",
        ];
        let mut last = 0;
        for key in order {
            let at = text[last..]
                .find(&format!("\"{key}\":"))
                .unwrap_or_else(|| panic!("{key} out of order in {text}"));
            last += at;
        }
    }

    #[test]
    fn round2_rounds_like_math_round_over_100() {
        assert_eq!(round2(1.005), 1.0); // 1.005 * 100 = 100.49999999999999
        assert_eq!(round2(2.675), 2.68); // 267.5 rounds up
        assert_eq!(round2(0.125), 0.13);
        assert_eq!(round2(-0.125), -0.12); // Math.round(-12.5) is -12
    }

    #[test]
    fn formula_description_prints_numbers_like_js_template_strings() {
        let text = formula_description().join("\n");
        assert!(text.contains("10 + 4 * log2(1 + effective lines) + iteration, capped at 72."));
        assert!(text.contains("+20% when it touches tests, +10% docs, +15% when it deletes more than it adds, +15% for a fix"));
        assert!(text.contains("0.8 + 0.4 * survival, clamped to 0.8-1.2."));
    }
}
