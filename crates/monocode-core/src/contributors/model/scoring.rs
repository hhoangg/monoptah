//! Scoring: who earned what in a period. Pure; port of `scoring.ts`. Floating
//! point follows JS (`Math.log2`, `Math.round` via `js_round`, left to right
//! sums), so the numbers equal the desktop app's.

use super::identity::{identity_key, is_agent, is_excluded, is_human, Resolver};
use super::patterns::is_ignored_path;
use super::period::{
    is_in_period, is_weekend, local_day_key, parse_iso, period_day_keys, previous_period, Period,
};
use super::types::ContributorsConfig;
use super::{js_cmp, js_round, js_trim, locale_cmp, serialize_number, OrderedMap};
use crate::contributors::{GitIdentity, RawCommit, RawData, RawFile, RawPr};
use chrono::{DateTime, Local};
use regex::Regex;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

struct Num(f64);

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_number(&self.0, serializer)
    }
}

macro_rules! scoring_constants {
    ($($field:ident: $json:literal = $value:expr),* $(,)?) => {
        /// The scoring constants. Serialized with the TS key names and order:
        /// the snapshot hands them to the agent.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Scoring {
            $(pub $field: f64),*
        }

        pub const SCORING: Scoring = Scoring { $($field: $value),* };

        impl Serialize for Scoring {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut map = serializer.serialize_map(None)?;
                $(map.serialize_entry($json, &Num(self.$field))?;)*
                map.end()
            }
        }
    };
}

scoring_constants! {
    pr_base: "PR_BASE" = 10.0,
    size_k: "SIZE_K" = 4.0,
    pr_cap: "PR_CAP" = 72.0,
    iter_k: "ITER_K" = 3.0,
    iter_cap: "ITER_CAP" = 12.0,
    review_pr: "REVIEW_PR" = 8.0,
    review_comment: "REVIEW_COMMENT" = 1.0,
    review_comment_cap: "REVIEW_COMMENT_CAP" = 5.0,
    active_day: "ACTIVE_DAY" = 5.0,
    streak_k: "STREAK_K" = 2.0,
    streak_cap: "STREAK_CAP" = 14.0,
    module_pts: "MODULE_PTS" = 15.0,
    module_min_eff: "MODULE_MIN_EFF" = 30.0,
    module_cap: "MODULE_CAP" = 6.0,
    quality_base: "QUALITY_BASE" = 0.8,
    quality_slope: "QUALITY_SLOPE" = 0.4,
    quality_min: "QUALITY_MIN" = 0.8,
    quality_max: "QUALITY_MAX" = 1.2,
    care_test: "CARE_TEST" = 0.2,
    care_docs: "CARE_DOCS" = 0.1,
    care_net_negative: "CARE_NET_NEGATIVE" = 0.15,
    care_fix: "CARE_FIX" = 0.15,
    owner_share: "OWNER_SHARE" = 0.6,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrScore {
    pub id: String,
    pub title: String,
    /// Delivery points of the whole PR (0 when reverted); the person earns
    /// `points * delivery_share`.
    pub points: f64,
    pub delivery_share: f64,
    /// Care bonus this person earned on the PR (already multiplied by the share).
    pub care: f64,
    pub iteration: f64,
    pub size: f64,
    pub added: u64,
    pub deleted: u64,
    pub commit_count: u64,
    pub test_files: bool,
    pub docs: bool,
    pub net_negative: bool,
    pub fix: bool,
    pub reverted: bool,
    pub ai_assisted: bool,
    pub merged_at: String,
}

/// The five dimensions, in the order the snapshot prints them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Parts {
    #[serde(serialize_with = "serialize_number")]
    pub delivery: f64,
    #[serde(serialize_with = "serialize_number")]
    pub care: f64,
    #[serde(serialize_with = "serialize_number")]
    pub review: f64,
    #[serde(serialize_with = "serialize_number")]
    pub consistency: f64,
    #[serde(serialize_with = "serialize_number")]
    pub breadth: f64,
}

/// `delivery`, `care`, `review`, `consistency`, `breadth` and `delivery_final`
/// are the unweighted subtotals (what the formula sidebar explains). `parts`
/// is the same five dimensions multiplied by `config.weights`, with quality
/// already applied to delivery, so `score` is exactly the sum of `parts`. Draw
/// breakdown bars from `parts`.
#[derive(Clone, Debug, PartialEq)]
pub struct PersonScore {
    pub person_id: String,
    pub name: String,
    pub score: f64,
    /// Before quality.
    pub delivery: f64,
    /// Times quality.
    pub delivery_final: f64,
    pub quality: f64,
    pub survival: Option<f64>,
    pub care: f64,
    pub review: f64,
    pub consistency: f64,
    pub breadth: f64,
    pub active_days: usize,
    pub streak: usize,
    pub merged_prs: usize,
    /// Sum of `commit_count` of owned merged PRs.
    pub branch_commits: u64,
    pub main_commits: usize,
    /// 0-100, informational.
    pub ai_assisted_pct: u32,
    pub fix_prs: usize,
    pub net_negative_prs: usize,
    pub reviewed_prs: usize,
    pub parts: Parts,
    pub modules: Vec<String>,
    pub prs: Vec<PrScore>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unattributed {
    pub prs: usize,
    pub commits: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PeriodScores {
    pub period: Period,
    pub people: Vec<PersonScore>,
    pub unattributed: Unattributed,
    pub gh_available: bool,
    /// People whose first activity in the data falls inside the period (empty
    /// when the data does not reach the previous period, and for All time).
    pub newcomers: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CareFlags {
    pub test: bool,
    pub docs: bool,
    pub net_negative: bool,
    pub fix: bool,
}

#[derive(Clone, Debug)]
pub struct AttributedCommit<'a> {
    pub commit: &'a RawCommit,
    /// `None`: nobody to credit.
    pub person_id: Option<String>,
    /// After the ignore globs.
    pub files: Vec<RawFile>,
    pub ai_assisted: bool,
}

fn normalize_path(path: &str) -> String {
    let slashed = path.replace('\\', "/");
    slashed.strip_prefix("./").unwrap_or(&slashed).to_string()
}

static TEST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|/)(tests?|__tests__|spec)/|\.(test|spec)\.[a-z]+$|_test\.(go|rs|py)$")
        .expect("test path pattern")
});
const DOCS_GLOBS: [&str; 4] = ["*.md", "*.mdx", "docs/**", "*.rst"];
static FIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^fix(\(|:|!|\s)").expect("fix title pattern"));

pub fn is_test_path(path: &str) -> bool {
    TEST_RE.is_match(&normalize_path(path))
}

pub fn is_docs_path(path: &str) -> bool {
    is_ignored_path(&DOCS_GLOBS, path)
}

/// Files that survive the config's ignore globs.
pub fn counted_files(config: &ContributorsConfig, files: &[RawFile]) -> Vec<RawFile> {
    files
        .iter()
        .filter(|f| !is_ignored_path(&config.ignore, &f.path))
        .cloned()
        .collect()
}

/// Effective lines: added + 0.5 * deleted. Pass files that are already
/// ignore-filtered.
pub fn effective_lines(files: &[RawFile]) -> f64 {
    let mut eff = 0.0;
    for f in files {
        eff += f.added as f64 + 0.5 * f.deleted as f64;
    }
    eff
}

pub fn pr_size(files: &[RawFile]) -> f64 {
    (1.0 + effective_lines(files)).log2()
}

pub fn iteration_points(commit_count: u64) -> f64 {
    SCORING
        .iter_cap
        .min(SCORING.iter_k * (1.0 + commit_count.max(1) as f64).log2())
}

pub fn pr_points(files: &[RawFile], commit_count: u64) -> f64 {
    SCORING
        .pr_cap
        .min(SCORING.pr_base + SCORING.size_k * pr_size(files) + iteration_points(commit_count))
}

/// `survival` is alive / written in 0..1, or `None` when nothing was sampled.
pub fn quality_factor(survival: Option<f64>) -> f64 {
    let Some(survival) = survival else {
        return 1.0;
    };
    let raw = SCORING.quality_base + SCORING.quality_slope * survival;
    SCORING.quality_max.min(SCORING.quality_min.max(raw))
}

pub fn care_rate(flags: &CareFlags) -> f64 {
    (if flags.test { SCORING.care_test } else { 0.0 })
        + (if flags.docs { SCORING.care_docs } else { 0.0 })
        + (if flags.net_negative {
            SCORING.care_net_negative
        } else {
            0.0
        })
        + (if flags.fix { SCORING.care_fix } else { 0.0 })
}

/// First two segments of the directory part, `(root)` for a file at the repo root.
pub fn module_of(path: &str) -> String {
    let normalized = normalize_path(path);
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    let dir = &segments[..segments.len().saturating_sub(1)];
    if dir.is_empty() {
        "(root)".into()
    } else {
        dir[..dir.len().min(2)].join("/")
    }
}

#[cfg(test)]
thread_local! {
    /// How many times a scoring run built the working-day keys.
    static DAY_KEYS_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Longest run of consecutive active working days (Mon-Fri) in the period.
/// Weekend days are skipped entirely: they neither break nor extend the run,
/// though an active weekend day still counts as an active day elsewhere.
/// Holidays have no data source and are not modelled.
pub fn streak_of<S: AsRef<str>>(
    active_day_keys: &[S],
    period: &Period,
    now: &DateTime<Local>,
) -> usize {
    let active: HashSet<&str> = active_day_keys.iter().map(|k| k.as_ref()).collect();
    streak_over(&active, &working_day_keys(period, now))
}

/// The Mon-Fri day keys of the period, oldest first. Built once per scoring
/// run and shared by every person: for a long period (All time) building it
/// per person costs thousands of dates and strings each.
fn working_day_keys(period: &Period, now: &DateTime<Local>) -> Vec<String> {
    #[cfg(test)]
    DAY_KEYS_CALLS.with(|c| c.set(c.get() + 1));
    period_day_keys(period, now)
        .into_iter()
        .filter(|key| !is_weekend(key))
        .collect()
}

fn streak_over<S: std::borrow::Borrow<str> + std::hash::Hash + Eq>(
    active: &HashSet<S>,
    working_days: &[String],
) -> usize {
    let mut run = 0;
    let mut best = 0;
    for key in working_days {
        if active.contains(key.as_str()) {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

fn for_each_identity(raw: &RawData, mut visit: impl FnMut(&GitIdentity)) {
    for c in &raw.commits {
        visit(&c.author);
        visit(&c.committer);
        c.co_authors.iter().for_each(&mut visit);
    }
    for pr in &raw.prs {
        visit(&pr.author);
        if let Some(committer) = &pr.committer {
            visit(committer);
        }
        pr.landing_co_authors.iter().for_each(&mut visit);
        for bc in &pr.branch_commits {
            bc.authors.iter().for_each(&mut visit);
        }
    }
    for r in &raw.reviews {
        visit(&r.reviewer);
    }
}

/// Every distinct identity in the raw data, for `build_resolver`'s
/// `observed_identities`.
pub fn observed_identities(raw: &RawData) -> Vec<GitIdentity> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for_each_identity(raw, |id| {
        let login = id
            .login
            .as_deref()
            .map_or_else(String::new, |l| js_trim(l).to_lowercase());
        if seen.insert(format!("{}\u{0}{login}", identity_key(id))) {
            out.push(id.clone());
        }
    });
    out
}

struct Ctx<'a> {
    config: &'a ContributorsConfig,
    resolver: &'a Resolver,
    /// Every identity seen per identity key, including each name used with it.
    variants: HashMap<String, Vec<GitIdentity>>,
}

fn make_ctx<'a>(raw: &RawData, config: &'a ContributorsConfig, resolver: &'a Resolver) -> Ctx<'a> {
    let mut variants: HashMap<String, Vec<GitIdentity>> = HashMap::new();
    for_each_identity(raw, |id| {
        let list = variants.entry(identity_key(id)).or_default();
        if !list
            .iter()
            .any(|v| v.name == id.name && v.login == id.login)
        {
            list.push(id.clone());
        }
    });
    Ctx {
        config,
        resolver,
        variants,
    }
}

#[derive(Clone, Debug)]
enum Credit {
    Owned(String),
    /// An agent did the work and no human can be named.
    Unattributed,
    /// A bot: not part of the data at all.
    Dropped,
}

impl Credit {
    fn owner(&self) -> Option<&str> {
        match self {
            Self::Owned(id) => Some(id),
            _ => None,
        }
    }
}

/// An identity string of a resolved person (email, `gh:<login>` or name) as a GitIdentity.
fn identity_of_key(key: &str) -> GitIdentity {
    if let Some(login) = key.strip_prefix("gh:") {
        return GitIdentity {
            name: String::new(),
            email: String::new(),
            login: Some(login.into()),
        };
    }
    if key.contains('@') {
        GitIdentity {
            name: String::new(),
            email: key.into(),
            login: None,
        }
    } else {
        GitIdentity {
            name: key.into(),
            email: String::new(),
            login: None,
        }
    }
}

/// The person an agent or bot identity was mapped to, as opposed to being its
/// own person. Only a person with at least one human identity counts, so an
/// agent can never become a leaderboard entry through an automatic link.
///
/// - A config person is the user's explicit choice: each listed key (email or
///   `gh:<login>`) is judged by the key alone, never by a git display name, so
///   `Devin <devin@company.com>` listed under a person is credited.
/// - Any other person (mailmap group, gh-login link) is judged by what was
///   seen: a key is an agent only when every name used with it is one.
fn mapped_person(ctx: &Ctx, id: &GitIdentity) -> Option<String> {
    let person_id = ctx.resolver.person_of(id);
    if person_id == identity_key(id) {
        return None;
    }
    let configured = ctx.config.people.iter().any(|p| p.id == person_id);
    let people = ctx.resolver.people();
    let identities: Vec<String> = match people.iter().find(|p| p.id == person_id) {
        Some(person) => person.identities.clone(),
        None => vec![person_id.clone()],
    };
    let has_human = identities.iter().any(|key| {
        let bare = identity_of_key(key);
        if configured {
            return is_human(ctx.config, &bare);
        }
        match ctx.variants.get(&key.to_lowercase()) {
            Some(seen) => seen.iter().any(|v| is_human(ctx.config, v)),
            None => is_human(ctx.config, &bare),
        }
    });
    has_human.then_some(person_id)
}

/// Owner 60% and human co-authors 40% split equally; the owner alone gets 100%.
fn shares_of(ctx: &Ctx, pr: &RawPr, owner: &str) -> Vec<(String, f64)> {
    let mut co_authors: Vec<String> = vec![];
    for id in pr
        .landing_co_authors
        .iter()
        .filter(|id| is_human(ctx.config, id))
    {
        let person = ctx.resolver.person_of(id);
        if !co_authors.contains(&person) {
            co_authors.push(person);
        }
    }
    co_authors.retain(|id| id != owner);
    if co_authors.is_empty() {
        return vec![(owner.to_string(), 1.0)];
    }
    let each = (1.0 - SCORING.owner_share) / co_authors.len() as f64;
    let mut shares = vec![(owner.to_string(), SCORING.owner_share)];
    shares.extend(co_authors.into_iter().map(|id| (id, each)));
    shares
}

/// Author, else human committer, else an identity the user mapped to a person.
fn credit_fallback(ctx: &Ctx, author: &GitIdentity, committer: Option<&GitIdentity>) -> Credit {
    if let Some(committer) = committer.filter(|c| is_human(ctx.config, c)) {
        return Credit::Owned(ctx.resolver.person_of(committer));
    }
    let mapped =
        mapped_person(ctx, author).or_else(|| committer.and_then(|c| mapped_person(ctx, c)));
    if let Some(id) = mapped {
        return Credit::Owned(id);
    }
    let agent_involved =
        is_agent(ctx.config, author) || committer.is_some_and(|c| is_agent(ctx.config, c));
    if agent_involved {
        Credit::Unattributed
    } else {
        Credit::Dropped
    }
}

fn credit_pr(ctx: &Ctx, pr: &RawPr) -> Credit {
    if is_human(ctx.config, &pr.author) {
        return Credit::Owned(ctx.resolver.person_of(&pr.author));
    }
    credit_fallback(ctx, &pr.author, pr.committer.as_ref())
}

fn credit_prs(ctx: &Ctx, prs: &[RawPr]) -> HashMap<String, Credit> {
    prs.iter()
        .map(|pr| (pr.id.clone(), credit_pr(ctx, pr)))
        .collect()
}

fn credit_commit(ctx: &Ctx, commit: &RawCommit, pr_credits: &HashMap<String, Credit>) -> Credit {
    if is_excluded(ctx.config, &commit.author) {
        return Credit::Dropped;
    }
    if is_human(ctx.config, &commit.author) {
        return Credit::Owned(ctx.resolver.person_of(&commit.author));
    }
    if let Some(Credit::Owned(id)) = commit.pr_id.as_ref().and_then(|p| pr_credits.get(p)) {
        return Credit::Owned(id.clone());
    }
    credit_fallback(ctx, &commit.author, Some(&commit.committer))
}

fn has_agent<'a>(
    config: &ContributorsConfig,
    ids: impl IntoIterator<Item = &'a GitIdentity>,
) -> bool {
    ids.into_iter().any(|id| is_agent(config, id))
}

fn attribute_with<'a>(
    ctx: &Ctx,
    commits: &'a [RawCommit],
    pr_credits: &HashMap<String, Credit>,
) -> Vec<AttributedCommit<'a>> {
    let mut out = Vec::new();
    for commit in commits {
        let credit = credit_commit(ctx, commit, pr_credits);
        if matches!(credit, Credit::Dropped) {
            continue;
        }
        out.push(AttributedCommit {
            commit,
            person_id: credit.owner().map(str::to_string),
            files: counted_files(ctx.config, &commit.files),
            ai_assisted: has_agent(
                ctx.config,
                std::iter::once(&commit.author).chain(&commit.co_authors),
            ),
        });
    }
    out
}

/// Who each non-bot commit counts for. An agent-authored commit goes to the
/// owner of its PR, else the committer, else a mapped identity, else nobody.
pub fn attribute_commits<'a>(
    raw: &'a RawData,
    config: &ContributorsConfig,
    resolver: &Resolver,
) -> Vec<AttributedCommit<'a>> {
    let ctx = make_ctx(raw, config, resolver);
    attribute_with(&ctx, &raw.commits, &credit_prs(&ctx, &raw.prs))
}

#[derive(Clone, Debug)]
pub struct AttributedPr<'a> {
    pub pr: &'a RawPr,
    pub person_id: Option<String>,
}

/// Who each non-bot PR counts for; `None` for an agent PR no human can be named for.
pub fn attribute_prs<'a>(
    raw: &'a RawData,
    config: &ContributorsConfig,
    resolver: &Resolver,
) -> Vec<AttributedPr<'a>> {
    let ctx = make_ctx(raw, config, resolver);
    let mut out = Vec::new();
    for pr in &raw.prs {
        let credit = credit_pr(&ctx, pr);
        if matches!(credit, Credit::Dropped) {
            continue;
        }
        out.push(AttributedPr {
            pr,
            person_id: credit.owner().map(str::to_string),
        });
    }
    out
}

struct Acc {
    person_id: String,
    delivery: f64,
    care: f64,
    review: f64,
    reviewed_prs: usize,
    branch_commits: u64,
    main_commits: usize,
    days: HashSet<String>,
    prs: Vec<PrScore>,
    pr_ids: HashSet<String>,
    module_eff: OrderedMap<f64>,
    written: u64,
    alive: u64,
    units: usize,
    ai_units: usize,
}

impl Acc {
    fn new(person_id: &str) -> Self {
        Self {
            person_id: person_id.into(),
            delivery: 0.0,
            care: 0.0,
            review: 0.0,
            reviewed_prs: 0,
            branch_commits: 0,
            main_commits: 0,
            days: HashSet::new(),
            prs: vec![],
            pr_ids: HashSet::new(),
            module_eff: OrderedMap::new(),
            written: 0,
            alive: 0,
            units: 0,
            ai_units: 0,
        }
    }
}

fn acc<'a>(accs: &'a mut OrderedMap<Acc>, person_id: &str) -> &'a mut Acc {
    accs.entry(person_id, || Acc::new(person_id))
}

fn millis(iso: &str) -> Option<i64> {
    parse_iso(iso).map(|t| t.timestamp_millis())
}

fn note(first_activity: &mut HashMap<String, i64>, person_id: &str, iso: &str) {
    if let Some(t) = millis(iso) {
        let current = first_activity.entry(person_id.to_string()).or_insert(t);
        if t < *current {
            *current = t;
        }
    }
}

fn mark_day(accs: &mut OrderedMap<Acc>, period: &Period, person_id: &str, iso: &str) {
    if is_in_period(period, iso) {
        acc(accs, person_id).days.insert(local_day_key(iso));
    }
}

/// `now` is today; the streak does not look at days after it.
pub fn score_period(
    raw: &RawData,
    config: &ContributorsConfig,
    resolver: &Resolver,
    period: &Period,
    now: &DateTime<Local>,
) -> PeriodScores {
    let ctx = make_ctx(raw, config, resolver);
    let pr_credits = credit_prs(&ctx, &raw.prs);
    let mut accs: OrderedMap<Acc> = OrderedMap::new();
    let mut first_activity: HashMap<String, i64> = HashMap::new();
    let mut unattributed = Unattributed::default();

    let mut commits_by_pr: HashMap<&str, Vec<&RawCommit>> = HashMap::new();
    for c in &raw.commits {
        if let Some(pr_id) = &c.pr_id {
            commits_by_pr.entry(pr_id).or_default().push(c);
        }
    }
    let mut survival_by_pr: HashMap<&str, (u64, u64)> = HashMap::new();
    for s in &raw.survival {
        let cur = survival_by_pr.entry(&s.pr_id).or_insert((0, 0));
        cur.0 += s.written;
        cur.1 += s.alive;
    }
    let reverted: HashSet<&str> = raw.reverted_pr_ids.iter().map(String::as_str).collect();

    for pr in &raw.prs {
        let credit = pr_credits.get(&pr.id);
        let owner = credit.and_then(Credit::owner);
        let when = if pr.state == "merged" {
            pr.merged_at.as_deref()
        } else {
            pr.closed_at.as_deref()
        };
        let in_period = when.filter(|w| is_in_period(period, w));

        if let (Some(owner), Some(when)) = (owner, when) {
            note(&mut first_activity, owner, when);
        }
        for bc in &pr.branch_commits {
            let mut people: Vec<String> = owner.map(str::to_string).into_iter().collect();
            for author in &bc.authors {
                if is_human(config, author) {
                    let id = resolver.person_of(author);
                    if !people.contains(&id) {
                        people.push(id);
                    }
                }
            }
            for person_id in &people {
                note(&mut first_activity, person_id, &bc.committed_at);
                // mark_day keeps only days inside the period, so a PR that
                // closes after the period still credits its days here.
                mark_day(&mut accs, period, person_id, &bc.committed_at);
            }
        }
        if let (Some(owner), Some(in_period)) = (owner, in_period) {
            if pr.state == "closed" && pr.branch_commits.is_empty() {
                mark_day(&mut accs, period, owner, in_period);
            }
        }

        let Some(in_period) = in_period.filter(|_| pr.state == "merged") else {
            continue;
        };
        if matches!(credit, Some(Credit::Unattributed)) {
            unattributed.prs += 1;
        }
        let Some(owner) = owner else {
            continue;
        };

        let files = counted_files(config, &pr.files);
        let added: u64 = files.iter().map(|f| f.added).sum();
        let deleted: u64 = files.iter().map(|f| f.deleted).sum();
        let flags = CareFlags {
            test: files.iter().any(|f| is_test_path(&f.path)),
            docs: files.iter().any(|f| is_docs_path(&f.path)),
            net_negative: deleted > added,
            fix: FIX_RE.is_match(&pr.title) || pr.closes_bug,
        };
        let is_reverted = reverted.contains(pr.id.as_str());
        let points = if is_reverted {
            0.0
        } else {
            pr_points(&files, pr.commit_count)
        };
        let rate = care_rate(&flags);
        let ai_assisted = has_agent(
            config,
            std::iter::once(&pr.author).chain(&pr.landing_co_authors),
        ) || pr
            .branch_commits
            .iter()
            .any(|bc| has_agent(config, &bc.authors))
            || commits_by_pr.get(pr.id.as_str()).is_some_and(|list| {
                list.iter()
                    .any(|c| has_agent(config, std::iter::once(&c.author).chain(&c.co_authors)))
            });

        let shares = shares_of(&ctx, pr, owner);

        // A reverted PR's dead lines must not also lower the quality of the rest.
        let sample = if is_reverted {
            None
        } else {
            survival_by_pr.get(pr.id.as_str()).copied()
        };
        for (person_id, share) in &shares {
            let a = acc(&mut accs, person_id);
            let care = share * points * rate;
            a.delivery += share * points;
            a.care += care;
            a.pr_ids.insert(pr.id.clone());
            a.units += 1;
            if ai_assisted {
                a.ai_units += 1;
            }
            if let Some((written, alive)) = sample.filter(|(written, _)| *written > 0) {
                a.written += written;
                a.alive += alive;
            }
            a.prs.push(PrScore {
                id: pr.id.clone(),
                title: pr.title.clone(),
                points,
                delivery_share: *share,
                care,
                iteration: iteration_points(pr.commit_count),
                size: pr_size(&files),
                added,
                deleted,
                commit_count: pr.commit_count,
                test_files: flags.test,
                docs: flags.docs,
                net_negative: flags.net_negative,
                fix: flags.fix,
                reverted: is_reverted,
                ai_assisted,
                merged_at: in_period.to_string(),
            });
            note(&mut first_activity, person_id, in_period);
        }

        let a = acc(&mut accs, owner);
        a.branch_commits += pr.commit_count;
        for f in &files {
            *a.module_eff.entry(&module_of(&f.path), || 0.0) +=
                f.added as f64 + 0.5 * f.deleted as f64;
        }
    }

    for attributed in attribute_with(&ctx, &raw.commits, &pr_credits) {
        let AttributedCommit {
            commit,
            person_id,
            ai_assisted,
            ..
        } = attributed;
        if let Some(person_id) = &person_id {
            note(&mut first_activity, person_id, &commit.committed_at);
        }
        if !is_in_period(period, &commit.committed_at) {
            continue;
        }
        let Some(person_id) = person_id else {
            unattributed.commits += 1;
            continue;
        };
        let a = acc(&mut accs, &person_id);
        a.main_commits += 1;
        a.days.insert(local_day_key(&commit.committed_at));
        if !commit.pr_id.as_ref().is_some_and(|p| a.pr_ids.contains(p)) {
            a.units += 1;
            if ai_assisted {
                a.ai_units += 1;
            }
        }
    }

    if raw.gh_available {
        let pr_by_id: HashMap<&str, &RawPr> =
            raw.prs.iter().map(|pr| (pr.id.as_str(), pr)).collect();
        let mut pairs: OrderedMap<(String, u64)> = OrderedMap::new();
        for review in &raw.reviews {
            // Reviews of PRs missing from raw.prs (e.g. still open) are skipped:
            // the author is unknown, so a self review could not be ruled out.
            let Some(pr) = pr_by_id.get(review.pr_id.as_str()) else {
                continue;
            };
            if !is_human(config, &review.reviewer) {
                continue;
            }
            let reviewer_id = resolver.person_of(&review.reviewer);
            let owner_id = pr_credits.get(&pr.id).and_then(Credit::owner);
            let sharing = owner_id.is_some_and(|owner| {
                shares_of(&ctx, pr, owner)
                    .iter()
                    .any(|(id, _)| *id == reviewer_id)
            });
            if sharing || reviewer_id == resolver.person_of(&pr.author) {
                continue;
            }
            note(&mut first_activity, &reviewer_id, &review.submitted_at);
            if !is_in_period(period, &review.submitted_at) {
                continue;
            }
            let key = format!("{reviewer_id}\u{0}{}", pr.id);
            pairs.entry(&key, || (reviewer_id.clone(), 0)).1 += review.comments;
        }
        for (_, (reviewer_id, comments)) in pairs.into_pairs() {
            let a = acc(&mut accs, &reviewer_id);
            a.review += SCORING.review_pr
                + SCORING
                    .review_comment_cap
                    .min(SCORING.review_comment * comments as f64);
            a.reviewed_prs += 1;
        }
    }

    let working_days = working_day_keys(period, now);
    let mut people: Vec<PersonScore> = accs
        .into_values()
        .map(|a| finish_person(a, config, resolver, &working_days))
        .collect();
    people.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| locale_cmp(&a.name, &b.name))
            .then_with(|| locale_cmp(&a.person_id, &b.person_id))
    });

    // Without a period before it (All time) nobody can be told apart as new.
    let before = previous_period(period);
    let data_reaches_previous_period = before.as_ref().is_some_and(|before| {
        millis(&raw.since).is_some_and(|since| since <= before.start.timestamp_millis())
    });
    let mut newcomers: Vec<String> = if data_reaches_previous_period {
        people
            .iter()
            .map(|p| p.person_id.clone())
            .filter(|id| {
                first_activity.get(id).copied().unwrap_or(0) >= period.start.timestamp_millis()
            })
            .collect()
    } else {
        vec![]
    };
    newcomers.sort_by(|a, b| js_cmp(a, b));

    PeriodScores {
        period: period.clone(),
        people,
        unattributed,
        gh_available: raw.gh_available,
        newcomers,
    }
}

fn finish_person(
    a: Acc,
    config: &ContributorsConfig,
    resolver: &Resolver,
    working_days: &[String],
) -> PersonScore {
    let survival = (a.written > 0).then(|| a.alive as f64 / a.written as f64);
    let quality = quality_factor(survival);
    let mut ranked: Vec<(&str, f64)> = a
        .module_eff
        .iter()
        .filter(|(_, eff)| **eff >= SCORING.module_min_eff)
        .map(|(module, eff)| (module, *eff))
        .collect();
    ranked.sort_by(|x, y| {
        y.1.partial_cmp(&x.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| locale_cmp(x.0, y.0))
    });
    let modules: Vec<String> = ranked
        .into_iter()
        .take(SCORING.module_cap as usize)
        .map(|(module, _)| module.to_string())
        .collect();
    let streak = streak_over(&a.days, working_days);
    let breadth = SCORING.module_pts * modules.len() as f64;
    let consistency = SCORING.active_day * a.days.len() as f64
        + SCORING.streak_k * (streak as f64).min(SCORING.streak_cap);
    let w = &config.weights;
    let parts = Parts {
        delivery: a.delivery * quality * w.delivery,
        care: a.care * w.care,
        review: a.review * w.review,
        consistency: consistency * w.consistency,
        breadth: breadth * w.breadth,
    };
    let live = || a.prs.iter().filter(|p| !p.reverted);
    let fix_prs = live().filter(|p| p.fix).count();
    let net_negative_prs = live().filter(|p| p.net_negative).count();
    PersonScore {
        name: resolver.name_of(&a.person_id),
        score: parts.delivery + parts.care + parts.review + parts.consistency + parts.breadth,
        delivery: a.delivery,
        delivery_final: a.delivery * quality,
        quality,
        survival,
        care: a.care,
        review: a.review,
        consistency,
        breadth,
        active_days: a.days.len(),
        streak,
        merged_prs: a.prs.len(),
        branch_commits: a.branch_commits,
        main_commits: a.main_commits,
        ai_assisted_pct: if a.units == 0 {
            0
        } else {
            js_round(a.ai_units as f64 / a.units as f64 * 100.0) as u32
        },
        fix_prs,
        net_negative_prs,
        reviewed_prs: a.reviewed_prs,
        parts,
        modules,
        prs: a.prs,
        person_id: a.person_id,
    }
}

#[cfg(test)]
mod tests {
    use super::observed_identities;
    use super::*;
    use crate::contributors::model::config::{default_config, DEFAULT_CONFIG};
    use crate::contributors::model::identity::{build_resolver, MailmapPerson, ResolverOptions};
    use crate::contributors::model::period::{current_period, PeriodKind};
    use crate::contributors::model::test_fixtures::*;
    use crate::contributors::model::types::{ContributorsConfig, Person, Weights};
    use crate::contributors::{GitIdentity, RawData, RawPr, RawSurvival};

    fn score(raw: &RawData) -> PeriodScores {
        score_with(raw, &default_config())
    }

    fn score_with(raw: &RawData, config: &ContributorsConfig) -> PeriodScores {
        score_period(
            raw,
            config,
            &make_resolver_with(raw, config),
            &period(),
            &period_now(),
        )
    }

    fn of<'a>(scores: &'a PeriodScores, name: &str) -> &'a PersonScore {
        scores
            .people
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no person named {name}"))
    }

    fn names(scores: &PeriodScores) -> Vec<&str> {
        scores.people.iter().map(|p| p.name.as_str()).collect()
    }

    fn ids(scores: &PeriodScores) -> Vec<&str> {
        scores.people.iter().map(|p| p.person_id.as_str()).collect()
    }

    fn person(id: &str, name: &str, identities: &[&str]) -> Person {
        Person {
            id: id.into(),
            name: name.into(),
            identities: identities.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // prSize

    #[test]
    fn pr_size_is_log2_of_one_plus_added_plus_half_deleted() {
        assert_close(pr_size(&[file("a.ts", 7)]), 3.0, 10);
        assert_close(
            pr_size(&[file("a.ts", 3), file_d("b.ts", 2, 8)]),
            (1.0 + 3.0 + 2.0 + 4.0f64).log2(),
            10,
        );
    }

    #[test]
    fn pr_size_is_0_for_no_files() {
        assert_eq!(pr_size(&[]), 0.0);
    }

    // iterationPoints

    #[test]
    fn iteration_points_follows_3_log2_1_plus_n_and_caps_at_12() {
        assert_close(iteration_points(1), 3.0, 10);
        assert_close(iteration_points(3), 6.0, 10);
        assert_close(iteration_points(7), 9.0, 10);
        assert_close(iteration_points(15), 12.0, 10);
        assert_eq!(iteration_points(500), 12.0);
    }

    #[test]
    fn iteration_points_treats_a_commit_count_below_1_as_1() {
        assert_close(iteration_points(0), 3.0, 10);
    }

    // prPoints

    #[test]
    fn pr_points_is_10_plus_4_size_plus_iteration() {
        let files = [file("a.ts", 7)];
        assert_close(pr_points(&files, 7), 10.0 + 4.0 * 3.0 + 9.0, 10);
    }

    #[test]
    fn pr_points_caps_at_72() {
        assert_eq!(pr_points(&[file("a.ts", 10_000_000)], 50), 72.0);
    }

    // qualityFactor

    #[test]
    fn quality_factor_is_1_without_a_survival_sample() {
        assert_eq!(quality_factor(None), 1.0);
    }

    #[test]
    fn quality_factor_is_0_8_plus_0_4_survival() {
        assert_close(quality_factor(Some(0.0)), 0.8, 10);
        assert_close(quality_factor(Some(0.5)), 1.0, 10);
        assert_close(quality_factor(Some(0.85)), 1.14, 10);
        assert_close(quality_factor(Some(1.0)), 1.2, 10);
    }

    #[test]
    fn quality_factor_clamps_to_0_8_1_2() {
        assert_eq!(quality_factor(Some(3.0)), 1.2);
        assert_eq!(quality_factor(Some(-2.0)), 0.8);
    }

    // careRate

    #[test]
    fn care_rate_adds_the_four_bonuses() {
        let none = CareFlags {
            test: false,
            docs: false,
            net_negative: false,
            fix: false,
        };
        assert_eq!(care_rate(&none), 0.0);
        assert_close(care_rate(&CareFlags { test: true, ..none }), 0.2, 10);
        assert_close(care_rate(&CareFlags { docs: true, ..none }), 0.1, 10);
        assert_close(
            care_rate(&CareFlags {
                net_negative: true,
                ..none
            }),
            0.15,
            10,
        );
        assert_close(care_rate(&CareFlags { fix: true, ..none }), 0.15, 10);
        assert_close(
            care_rate(&CareFlags {
                test: true,
                docs: true,
                net_negative: true,
                fix: true,
            }),
            0.6,
            10,
        );
    }

    // moduleOf

    #[test]
    fn module_of_takes_the_first_two_segments_of_the_directory_part() {
        assert_eq!(module_of("src/features/x/a.ts"), "src/features");
        assert_eq!(module_of("src-tauri/src/lib.rs"), "src-tauri/src");
        assert_eq!(module_of("src/a.ts"), "src");
        assert_eq!(module_of("docs/a.md"), "docs");
    }

    #[test]
    fn module_of_uses_root_for_a_root_file() {
        assert_eq!(module_of("README.md"), "(root)");
    }

    #[test]
    fn module_of_normalizes_backslashes_and_a_leading_dot_slash() {
        assert_eq!(module_of("./src\\features\\a.ts"), "src/features");
    }

    // isTestPath and isDocsPath

    #[test]
    fn is_test_path_recognizes_test_files() {
        for p in [
            "src/a.test.ts",
            "src/a.spec.tsx",
            "tests/a.ts",
            "src/test/a.ts",
            "src/__tests__/a.ts",
            "spec/a.rb",
            "pkg/a_test.go",
            "src/a_test.rs",
            "py/a_test.py",
            "SRC/A.TEST.TS",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in ["src/latest.ts", "src/contest/a.ts", "src/a.ts"] {
            assert!(!is_test_path(p), "{p}");
        }
    }

    #[test]
    fn is_docs_path_recognizes_docs_files() {
        for p in [
            "README.md",
            "src/a/guide.mdx",
            "docs/a.png",
            "docs/deep/a.txt",
            "notes.rst",
        ] {
            assert!(is_docs_path(p), "{p}");
        }
        for p in ["src/a.ts", "src/docs/a.ts", "Makefile"] {
            assert!(!is_docs_path(p), "{p}");
        }
    }

    // streakOf

    fn day(d: u32) -> String {
        format!("2026-10-{d:02}")
    }

    fn streak(days: &[u32]) -> usize {
        let keys: Vec<String> = days.iter().map(|&d| day(d)).collect();
        streak_of(&keys, &period(), &period_now())
    }

    #[test]
    fn streak_of_is_0_without_activity() {
        assert_eq!(streak(&[]), 0);
    }

    #[test]
    fn streak_of_counts_consecutive_working_days() {
        assert_eq!(streak(&[5, 6, 7, 8, 9]), 5);
        assert_eq!(streak(&[5, 6, 8, 9]), 2);
    }

    #[test]
    fn streak_of_does_not_break_across_an_idle_weekend() {
        assert_eq!(streak(&[8, 9, 12, 13]), 4);
    }

    #[test]
    fn streak_of_skips_weekend_days_entirely_active_or_not() {
        assert_eq!(streak(&[9, 10, 12]), 2);
        assert_eq!(streak(&[9, 11, 12]), 2);
        assert_eq!(streak(&[9, 10, 11, 12]), 2);
    }

    #[test]
    fn streak_of_gives_a_weekend_only_run_no_streak() {
        assert_eq!(streak(&[3, 4]), 0);
    }

    #[test]
    fn streak_of_breaks_on_a_missed_working_day() {
        assert_eq!(streak(&[9, 13, 14]), 2);
    }

    #[test]
    fn streak_of_ignores_days_outside_the_period() {
        assert_eq!(
            streak_of(&["2026-09-30", "2026-10-01"], &period(), &period_now()),
            1
        );
        assert_eq!(streak_of(&["2026-11-02"], &period(), &period_now()), 0);
    }

    #[test]
    fn streak_of_takes_the_longest_run() {
        assert_eq!(streak(&[1, 2, 6, 7, 8, 9]), 4);
    }

    #[test]
    fn streak_of_does_not_look_past_today_in_the_current_period() {
        let now = local_at(2026, 9, 9, 10, 0);
        let keys: Vec<String> = [5, 6, 7, 8, 9, 12, 13].iter().map(|&d| day(d)).collect();
        assert_eq!(streak_of(&keys, &period(), &now), 5);
    }

    #[test]
    fn streak_of_runs_over_the_days_of_a_7d_period_even_across_a_month_change() {
        let now = local_at(2026, 10, 3, 10, 0); // Tue 2026-11-03
        let week = current_period(PeriodKind::Days7, &now, None);
        assert_eq!(
            streak_of(
                &["2026-10-28", "2026-10-29", "2026-10-30", "2026-11-02"],
                &week,
                &now
            ),
            4
        );
        assert_eq!(streak_of(&["2026-10-27"], &week, &now), 0);
    }

    #[test]
    fn streak_of_runs_across_the_whole_of_a_quarter() {
        let now = local_at(2026, 11, 31, 10, 0);
        let quarter = current_period(PeriodKind::Quarter, &now, None);
        assert_eq!(
            streak_of(&["2026-10-30", "2026-11-02", "2026-12-31"], &quarter, &now),
            2
        );
    }

    // scoring.streak.test.ts

    fn streak_run(
        days: &[(&str, &[u32])],
        kind: PeriodKind,
    ) -> (
        crate::contributors::model::period::Period,
        PeriodScores,
        DateTime<Local>,
    ) {
        let now = local_at(2026, 9, 20, 10, 0);
        let mut commits = vec![];
        for (name, list) in days {
            let who = match *name {
                "ana" => ana(),
                "bob" => bob(),
                _ => cat(),
            };
            for d in *list {
                commits.push(make_commit(&format!("{name}{d}"), &who, &oct12(*d)));
            }
        }
        let raw = raw_with(|r| {
            r.since = stamp(2026, 7, 1, 0);
            r.commits = commits;
        });
        let period = current_period(kind, &now, Some(&local_date(2026, 8, 25)));
        let scores = score_period(
            &raw,
            &default_config(),
            &make_resolver_with(&raw, &default_config()),
            &period,
            &now,
        );
        (period, scores, now)
    }

    const STREAK_DAYS: [(&str, &[u32]); 3] = [
        ("ana", &[1, 2, 5, 6, 7, 8, 9]),
        ("bob", &[1, 5, 6]),
        ("cat", &[12, 13, 14, 15, 16, 19]),
    ];

    #[test]
    fn score_period_streaks_build_the_periods_day_keys_once_however_many_people_there_are() {
        DAY_KEYS_CALLS.with(|c| c.set(0));
        let (_, scores, _) = streak_run(&STREAK_DAYS, PeriodKind::Month);
        assert_eq!(scores.people.len(), 3);
        assert_eq!(DAY_KEYS_CALLS.with(|c| c.get()), 1);
    }

    #[test]
    fn score_period_streaks_give_every_person_the_streak_streak_of_finds_for_their_days() {
        for kind in [PeriodKind::Month, PeriodKind::All] {
            let (period, scores, now) = streak_run(&STREAK_DAYS, kind);
            for p in &scores.people {
                let key = p.name.to_lowercase();
                let list = STREAK_DAYS.iter().find(|(n, _)| *n == key).unwrap().1;
                let active: Vec<String> = list.iter().map(|&d| day(d)).collect();
                let expected = match p.name.as_str() {
                    "Ana" => 7,
                    "Bob" => 2,
                    _ => 6,
                };
                assert_eq!(p.streak, streak_of(&active, &period, &now));
                assert_eq!(p.streak, expected);
            }
        }
    }

    // observedIdentities

    #[test]
    fn observed_identities_collects_every_identity_in_the_raw_data_once() {
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct12(5), |c| {
                    c.co_authors = vec![bob()];
                    c.committer = github();
                }),
                make_commit("c2", &ana(), &oct12(6)),
            ];
            r.prs = vec![pr_with("1", &cat(), &oct12(6), |p| {
                p.committer = Some(bob());
                p.landing_co_authors = vec![claude()];
                p.branch_commits = vec![branch_commit(&oct12(5), &[&cat(), &ana()])];
            })];
            r.reviews = vec![make_review(
                "1",
                &ident_login("dev", "", "dev-login"),
                &oct12(7),
                0,
                "APPROVED",
            )];
        });
        let keys: Vec<String> = observed_identities(&raw)
            .into_iter()
            .map(|i| {
                if i.email.is_empty() {
                    i.login.unwrap_or_default()
                } else {
                    i.email
                }
            })
            .collect();
        assert_eq!(
            sorted(keys),
            sorted(strs(&[
                "ana@example.com",
                "bob@example.com",
                "cat@example.com",
                "noreply@anthropic.com",
                "noreply@github.com",
                "dev-login",
            ]))
        );
    }

    // delivery

    #[test]
    fn score_period_delivery_scores_a_merged_pr_by_its_final_diff_and_commit_count() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.commit_count = 3;
                p.files = vec![file("src/features/a.ts", 100)];
            })];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        let expected = pr_points(&[file("src/features/a.ts", 100)], 3);
        assert_close(a.delivery, expected, 8);
        assert_close(a.delivery_final, expected, 8);
        assert_eq!(a.quality, 1.0);
        assert_eq!(a.survival, None);
        assert_eq!(a.merged_prs, 1);
        assert_eq!(a.branch_commits, 3);
        let pr = &a.prs[0];
        assert_eq!(pr.id, "1");
        assert_close(pr.points, expected, 8);
        assert_eq!(pr.delivery_share, 1.0);
        assert_close(pr.size, pr_size(&[file("a.ts", 100)]), 8);
        assert_close(pr.iteration, 6.0, 8);
        assert_eq!(pr.added, 100);
        assert_eq!(pr.commit_count, 3);
        assert_eq!(pr.merged_at, oct12(8));
    }

    #[test]
    fn score_period_delivery_caps_a_pr_at_72_points() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.commit_count = 40;
                p.files = vec![file("src/a.ts", 5_000_000)];
            })];
        });
        assert_eq!(of(&score(&raw), "Ana").prs[0].points, 72.0);
    }

    #[test]
    fn score_period_delivery_applies_ignore_globs_before_sizing() {
        let with_lock = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.files = vec![file("package-lock.json", 5000), file("src/a.ts", 100)];
            })];
        });
        let without = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.files = vec![file("src/a.ts", 100)];
            })];
        });
        let a = score(&with_lock);
        let a = of(&a, "Ana");
        assert_close(a.delivery, of(&score(&without), "Ana").delivery, 8);
        assert_eq!(a.prs[0].added, 100);
    }

    #[test]
    fn score_period_delivery_applies_ignore_globs_before_test_and_docs_detection() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.files = vec![file("src/a.ts", 100), file("src/gen/a.test.ts", 50)];
            })];
        });
        let ignored = with_config(|c| c.ignore = strs(&["src/gen/**"]));
        assert!(of(&score(&raw), "Ana").prs[0].test_files);
        assert!(!of(&score_with(&raw, &ignored), "Ana").prs[0].test_files);
    }

    #[test]
    fn score_period_delivery_gives_closed_prs_no_delivery() {
        let raw = raw_with(|r| {
            let mut pr = closed_pr("1", &ana(), &oct12(8));
            pr.branch_commits = vec![
                branch_commit(&oct12(6), &[&ana()]),
                branch_commit(&oct12(7), &[&ana()]),
            ];
            r.prs = vec![pr];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.delivery, 0.0);
        assert_eq!(a.merged_prs, 0);
        assert!(a.prs.is_empty());
        assert_eq!(a.active_days, 2);
        assert_eq!(a.consistency, (5 * 2 + 2 * 2) as f64);
    }

    #[test]
    fn score_period_delivery_falls_back_to_closed_at_for_a_closed_pr_without_branch_commits() {
        let raw = raw_with(|r| r.prs = vec![closed_pr("1", &ana(), &oct12(8))]);
        assert_eq!(of(&score(&raw), "Ana").active_days, 1);
    }

    #[test]
    fn score_period_delivery_only_counts_prs_merged_inside_the_period() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("before", &ana(), &stamp(2026, 8, 30, 23)),
                make_pr("first", &ana(), &iso(&local_dt(2026, 9, 1, 0, 0, 1, 0))),
                make_pr("last", &ana(), &stamp(2026, 9, 31, 23)),
                make_pr("after", &ana(), &stamp(2026, 10, 1, 0)),
            ];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        let got: Vec<&str> = a.prs.iter().map(|p| p.id.as_str()).collect();
        assert!(got.contains(&"first") && got.contains(&"last"));
        assert_eq!(a.merged_prs, 2);
    }

    #[test]
    fn score_period_delivery_zeroes_delivery_and_care_of_a_reverted_pr_and_nothing_else() {
        let raw = raw_with(|r| {
            r.prs = vec![
                pr_with("1", &ana(), &oct12(8), |p| {
                    p.title = "fix: a thing".into();
                    p.files = vec![file("src/a.ts", 100), file("src/a.test.ts", 20)];
                }),
                make_pr("2", &ana(), &oct12(9)),
            ];
            r.reverted_pr_ids = strs(&["1"]);
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        let reverted = a.prs.iter().find(|p| p.id == "1").unwrap();
        assert!(reverted.reverted);
        assert_eq!(reverted.points, 0.0);
        assert_eq!(reverted.care, 0.0);
        assert_close(a.delivery, pr_points(&[file("a", 100)], 1), 8);
        assert_eq!(a.care, 0.0);
        assert_eq!(a.merged_prs, 2);
        assert_eq!(a.fix_prs, 0);
    }

    // quality

    fn survival(pr_id: &str, written: u64, alive: u64) -> RawSurvival {
        RawSurvival {
            pr_id: pr_id.into(),
            written,
            alive,
        }
    }

    #[test]
    fn score_period_quality_multiplies_delivery_by_the_survival_factor() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &ana(), &oct12(8)),
                make_pr("2", &ana(), &oct12(9)),
            ];
            r.survival = vec![survival("1", 100, 50), survival("2", 300, 300)];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_close(a.survival.unwrap(), 350.0 / 400.0, 10);
        assert_close(a.quality, 0.8 + 0.4 * 0.875, 10);
        assert_close(a.delivery_final, a.delivery * a.quality, 8);
    }

    #[test]
    fn score_period_quality_ignores_samples_with_nothing_written() {
        let raw = raw_with(|r| {
            r.prs = vec![make_pr("1", &ana(), &oct12(8))];
            r.survival = vec![survival("1", 0, 0)];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.survival, None);
        assert_eq!(a.quality, 1.0);
    }

    #[test]
    fn score_period_quality_ignores_the_survival_sample_of_a_reverted_pr() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &ana(), &oct12(6)),
                make_pr("2", &ana(), &oct12(7)),
                make_pr("3", &ana(), &oct12(8)),
                make_pr("4", &ana(), &oct12(9)),
            ];
            r.reverted_pr_ids = strs(&["4"]);
            r.survival = vec![
                survival("1", 1000, 900),
                survival("2", 1000, 900),
                survival("3", 1000, 900),
                survival("4", 1000, 0),
            ];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_close(a.survival.unwrap(), 0.9, 10);
        assert_close(a.quality, 1.16, 10);
    }

    #[test]
    fn score_period_quality_does_not_apply_quality_to_care() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.files = vec![file("src/a.ts", 100), file("src/a.test.ts", 20)];
            })];
            r.survival = vec![survival("1", 100, 0)];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_close(a.quality, 0.8, 10);
        assert_close(a.care, a.prs[0].points * 0.2, 8);
    }

    // care

    fn care(patch: impl FnOnce(&mut RawPr)) -> PersonScore {
        let raw = raw_with(|r| r.prs = vec![pr_with("1", &ana(), &oct12(8), patch)]);
        of(&score(&raw), "Ana").clone()
    }

    #[test]
    fn score_period_care_adds_20_percent_for_test_files() {
        let a = care(|p| p.files = vec![file("src/a.ts", 100), file("src/a.test.ts", 10)]);
        assert!(a.prs[0].test_files);
        assert_close(a.care, a.prs[0].points * 0.2, 8);
    }

    #[test]
    fn score_period_care_adds_10_percent_for_docs() {
        let a = care(|p| p.files = vec![file("src/a.ts", 100), file("docs/a.md", 10)]);
        assert!(a.prs[0].docs);
        assert_close(a.care, a.prs[0].points * 0.1, 8);
    }

    #[test]
    fn score_period_care_adds_15_percent_when_more_lines_are_deleted_than_added() {
        let a = care(|p| p.files = vec![file_d("src/a.ts", 100, 400)]);
        assert!(a.prs[0].net_negative);
        assert_eq!(a.net_negative_prs, 1);
        assert_close(a.care, a.prs[0].points * 0.15, 8);
    }

    #[test]
    fn score_period_care_is_not_net_negative_when_deleted_equals_added() {
        assert!(!care(|p| p.files = vec![file_d("src/a.ts", 100, 100)]).prs[0].net_negative);
    }

    #[test]
    fn score_period_care_adds_15_percent_for_a_fix_title_or_a_closed_bug_issue() {
        for title in ["fix: a", "Fix(core): a", "fix!: a", "fix a bug"] {
            let a = care(|p| p.title = title.into());
            assert!(a.prs[0].fix, "{title}");
            assert_close(a.care, a.prs[0].points * 0.15, 8);
        }
        assert!(!care(|p| p.title = "prefix: a".into()).prs[0].fix);
        assert!(!care(|p| p.title = "fixture update".into()).prs[0].fix);
        let bug = care(|p| p.closes_bug = true);
        assert!(bug.prs[0].fix);
        assert_eq!(bug.fix_prs, 1);
    }

    #[test]
    fn score_period_care_stacks_the_bonuses() {
        let a = care(|p| {
            p.title = "fix: all".into();
            p.files = vec![
                file_d("src/a.ts", 10, 400),
                file("src/a.test.ts", 5),
                file("README.md", 1),
            ];
        });
        assert_close(a.care, a.prs[0].points * 0.6, 8);
    }

    // credit

    #[test]
    fn score_period_credit_splits_60_40_with_human_co_authors() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.landing_co_authors = vec![bob(), cat()];
                p.files = vec![file("src/a.ts", 100), file("src/a.test.ts", 10)];
            })];
        });
        let scores = score(&raw);
        let points = of(&scores, "Ana").prs[0].points;
        assert_close(of(&scores, "Ana").prs[0].delivery_share, 0.6, 10);
        assert_close(of(&scores, "Bob").prs[0].delivery_share, 0.2, 10);
        assert_close(of(&scores, "Cat").prs[0].delivery_share, 0.2, 10);
        assert_close(of(&scores, "Ana").delivery, points * 0.6, 8);
        assert_close(of(&scores, "Bob").delivery, points * 0.2, 8);
        assert_close(of(&scores, "Bob").care, points * 0.2 * 0.2, 8);
        assert_eq!(of(&scores, "Bob").merged_prs, 1);
        assert_eq!(of(&scores, "Bob").branch_commits, 0);
        assert_eq!(of(&scores, "Ana").branch_commits, 1);
        let total: f64 = scores.people.iter().map(|p| p.delivery).sum();
        assert_close(total, points, 8);
    }

    #[test]
    fn score_period_credit_gives_a_single_human_co_author_40_percent() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.landing_co_authors = vec![bob()]
            })];
        });
        let scores = score(&raw);
        assert_close(of(&scores, "Ana").prs[0].delivery_share, 0.6, 10);
        assert_close(of(&scores, "Bob").prs[0].delivery_share, 0.4, 10);
    }

    #[test]
    fn score_period_credit_keeps_100_percent_when_the_only_co_author_is_the_owner_or_an_agent() {
        let raw = raw_with(|r| {
            r.prs = vec![
                pr_with("1", &ana(), &oct12(8), |p| {
                    p.landing_co_authors = vec![ana(), claude()]
                }),
                pr_with("2", &ana(), &oct12(9), |p| {
                    p.landing_co_authors = vec![claude(), dependabot()]
                }),
            ];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(
            a.prs.iter().map(|p| p.delivery_share).collect::<Vec<_>>(),
            [1.0, 1.0]
        );
        assert_eq!(scores.people.len(), 1);
    }

    #[test]
    fn score_period_credit_falls_back_to_the_committer_when_the_author_is_an_agent() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &claude(), &oct12(8), |p| {
                p.committer = Some(bob())
            })];
        });
        let scores = score(&raw);
        assert_eq!(names(&scores), ["Bob"]);
        assert_eq!(scores.unattributed.prs, 0);
    }

    #[test]
    fn score_period_credit_falls_back_to_an_identity_mapping_when_the_committer_is_not_human() {
        let config = with_config(|c| {
            c.people = vec![person(
                "p-dev",
                "Dev",
                &["noreply@anthropic.com", "dev@example.com"],
            )];
        });
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &claude(), &oct12(8), |p| {
                p.committer = Some(github())
            })];
        });
        let scores = score_with(&raw, &config);
        assert_eq!(ids(&scores), ["p-dev"]);
        assert_eq!(scores.people[0].name, "Dev");
    }

    #[test]
    fn score_period_credit_never_credits_an_agent_mapped_to_a_person_with_no_human_identity() {
        let config = with_config(|c| {
            c.people = vec![person("p-bot", "Bot", &["noreply@anthropic.com"])];
        });
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &claude(), &oct12(8), |p| {
                p.committer = Some(github())
            })];
        });
        let scores = score_with(&raw, &config);
        assert!(scores.people.is_empty());
        assert_eq!(scores.unattributed.prs, 1);
    }

    #[test]
    fn score_period_credit_never_credits_an_agent_linked_to_itself_through_its_gh_login() {
        let copilot = ident_login("Copilot", "", "Copilot");
        let copilot_mail = ident("Copilot", "198982749+Copilot@users.noreply.github.com");
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &copilot, &oct12(8), |p| {
                p.committer = Some(github())
            })];
            r.commits = vec![commit_with("c1", &copilot_mail, &oct12(8), |c| {
                c.committer = github()
            })];
        });
        let scores = score(&raw);
        assert!(scores.people.is_empty());
        assert_eq!(scores.unattributed, Unattributed { prs: 1, commits: 1 });
    }

    #[test]
    fn score_period_credit_never_credits_an_agent_grouped_by_a_mailmap_entry() {
        let alias = ident("Claude", "claude-alt@anthropic.com");
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &alias, &oct12(8), |p| {
                p.committer = Some(github())
            })];
        });
        let resolver = build_resolver(
            &DEFAULT_CONFIG,
            ResolverOptions {
                merge: true,
                mailmap: vec![MailmapPerson {
                    name: "Claude".into(),
                    identities: strs(&["claude@anthropic.com", "claude-alt@anthropic.com"]),
                }],
                observed_identities: observed_identities(&raw),
            },
        );
        let scores = score_period(&raw, &DEFAULT_CONFIG, &resolver, &period(), &period_now());
        assert!(scores.people.is_empty());
        assert_eq!(scores.unattributed.prs, 1);
    }

    #[test]
    fn score_period_credit_credits_a_human_whose_git_name_is_an_agent_word_once_a_config_person_lists_their_email(
    ) {
        let devin = ident("Devin", "devin@company.com");
        let config = with_config(|c| {
            c.people = vec![person("p-devin", "Devin Lee", &["devin@company.com"])];
        });
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &devin, &oct12(8), |p| {
                p.committer = Some(github())
            })];
            r.commits = vec![make_commit("c1", &devin, &oct12(8))];
        });
        let scores = score_with(&raw, &config);
        assert_eq!(ids(&scores), ["p-devin"]);
        assert_eq!(scores.people[0].main_commits, 1);
        assert_eq!(scores.unattributed, Unattributed { prs: 0, commits: 0 });
        assert!(score(&raw).people.is_empty());
    }

    #[test]
    fn score_period_credit_trusts_a_config_person_built_around_an_agent_noreply_email() {
        let config = with_config(|c| {
            c.people = vec![person(
                "p-copilot",
                "Copilot",
                &["198982749+copilot@users.noreply.github.com"],
            )];
        });
        let raw = raw_with(|r| {
            r.prs = vec![pr_with(
                "1",
                &ident_login("Copilot", "", "Copilot"),
                &oct12(8),
                |p| p.committer = Some(github()),
            )];
        });
        assert_eq!(ids(&score_with(&raw, &config)), ["p-copilot"]);
    }

    mod mailmap_person_judged_by_what_was_observed {
        use super::*;

        fn agent_named() -> GitIdentity {
            ident("Cursor Agent", "ana@example.com")
        }
        fn human() -> GitIdentity {
            ident("Ana", "ana@example.com")
        }

        fn run(commits: Vec<crate::contributors::RawCommit>) -> PeriodScores {
            let claude = ident("Claude", "claude@anthropic.com");
            let raw = raw_with(|r| {
                r.prs = vec![pr_with("1", &claude, &oct12(8), |p| {
                    p.committer = Some(github())
                })];
                r.commits = commits;
            });
            let resolver = build_resolver(
                &DEFAULT_CONFIG,
                ResolverOptions {
                    merge: true,
                    mailmap: vec![MailmapPerson {
                        name: "Ana".into(),
                        identities: strs(&["ana@example.com", "claude@anthropic.com"]),
                    }],
                    observed_identities: observed_identities(&raw),
                },
            );
            score_period(&raw, &DEFAULT_CONFIG, &resolver, &period(), &period_now())
        }

        #[test]
        fn is_credited_when_any_observed_name_of_its_email_is_human_in_any_order() {
            for order in [[agent_named(), human()], [human(), agent_named()]] {
                let commits = order
                    .iter()
                    .enumerate()
                    .map(|(i, id)| make_commit(&format!("c{i}"), id, &oct12(6 + i as u32)))
                    .collect();
                let scores = run(commits);
                let a = scores
                    .people
                    .iter()
                    .find(|p| p.person_id == "ana@example.com")
                    .unwrap();
                assert_eq!(a.merged_prs, 1);
                assert_eq!(scores.unattributed.prs, 0);
            }
        }

        #[test]
        fn is_not_credited_when_every_observed_name_is_an_agent() {
            let scores = run(vec![make_commit("c0", &agent_named(), &oct12(6))]);
            assert_eq!(scores.unattributed.prs, 1);
            assert!(scores.people.iter().all(|p| p.merged_prs == 0));
        }
    }

    #[test]
    fn score_period_credit_reports_an_agent_pr_nobody_can_own_as_unattributed() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &claude(), &oct12(8), |p| {
                p.committer = Some(github())
            })];
        });
        let scores = score(&raw);
        assert!(scores.people.is_empty());
        assert_eq!(scores.unattributed.prs, 1);
    }

    #[test]
    fn score_period_credit_drops_bot_prs_nobody_can_own_without_reporting_them() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &dependabot(), &oct12(8), |p| {
                p.committer = Some(github())
            })];
        });
        let scores = score(&raw);
        assert!(scores.people.is_empty());
        assert_eq!(scores.unattributed.prs, 0);
    }

    #[test]
    fn score_period_credit_resolves_aliases_of_one_person_to_the_same_score() {
        let config = with_config(|c| {
            c.people = vec![person(
                "p-ana",
                "Ana Full",
                &["ana@example.com", "ana@laptop.local"],
            )];
        });
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &ana(), &oct12(8)),
                make_pr("2", &ident("Ana", "ana@laptop.local"), &oct12(9)),
            ];
        });
        let scores = score_with(&raw, &config);
        assert_eq!(scores.people.len(), 1);
        assert_eq!(scores.people[0].name, "Ana Full");
        assert_eq!(scores.people[0].merged_prs, 2);
    }

    // commits and consistency

    #[test]
    fn score_period_commits_counts_owned_mainline_commits_and_their_days() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("c1", &ana(), &oct(5, 9)),
                make_commit("c2", &ana(), &oct(5, 15)),
                make_commit("c3", &ana(), &oct12(6)),
                make_commit("old", &ana(), &stamp(2026, 8, 20, 0)),
                make_commit("bot", &dependabot(), &oct12(7)),
            ];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.main_commits, 3);
        assert_eq!(a.active_days, 2);
        assert_eq!(a.streak, 2);
        assert_eq!(a.consistency, (5 * 2 + 2 * 2) as f64);
        assert_eq!(scores.people.len(), 1);
    }

    #[test]
    fn score_period_commits_buckets_days_by_committer_date_not_author_date() {
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct12(6), |c| c.authored_at = oct12(1)),
                commit_with("c2", &ana(), &oct(6, 20), |c| c.authored_at = oct12(2)),
            ];
        });
        assert_eq!(of(&score(&raw), "Ana").active_days, 1);
    }

    #[test]
    fn score_period_commits_counts_days_of_branch_commits_of_owned_prs() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(9), |p| {
                p.commit_count = 3;
                p.branch_commits = vec![
                    branch_commit(&oct12(6), &[&ana()]),
                    branch_commit(&oct12(7), &[&ana()]),
                    branch_commit(&oct12(9), &[&ana()]),
                ];
            })];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.active_days, 3);
        assert_eq!(a.streak, 2);
    }

    #[test]
    fn score_period_commits_ignores_branch_commit_days_outside_the_period() {
        let sep = stamp(2026, 8, 29, 0);
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(2), |p| {
                p.branch_commits = vec![branch_commit(&sep, &[&ana()])];
            })];
        });
        assert_eq!(of(&score(&raw), "Ana").active_days, 0);
    }

    #[test]
    fn score_period_commits_credits_a_branch_commit_day_to_its_human_author_on_someone_elses_pr() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(9), |p| {
                p.branch_commits = vec![
                    branch_commit(&oct12(7), &[&ana(), &bob()]),
                    branch_commit(&oct12(8), &[&claude(), &dependabot()]),
                ];
            })];
        });
        let scores = score(&raw);
        assert_eq!(of(&scores, "Bob").active_days, 1);
        assert_eq!(of(&scores, "Bob").merged_prs, 0);
        assert_eq!(of(&scores, "Ana").active_days, 2);
    }

    #[test]
    fn score_period_commits_caps_the_streak_bonus_at_14_days() {
        let days = [
            1, 2, 5, 6, 7, 8, 9, 12, 13, 14, 15, 16, 19, 20, 21, 22, 23, 26, 27, 28, 29, 30,
        ];
        let raw = raw_with(|r| {
            r.commits = days
                .iter()
                .map(|&d| make_commit(&format!("c{d}"), &ana(), &oct12(d)))
                .collect();
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.active_days, 22);
        assert_eq!(a.streak, 22);
        assert_eq!(a.consistency, (5 * 22 + 2 * 14) as f64);
    }

    #[test]
    fn score_period_commits_does_not_let_an_idle_weekend_break_the_streak() {
        let raw = raw_with(|r| {
            r.commits = [8, 9, 12, 13]
                .iter()
                .map(|&d| make_commit(&format!("c{d}"), &ana(), &oct12(d)))
                .collect();
        });
        assert_eq!(of(&score(&raw), "Ana").streak, 4);
    }

    #[test]
    fn score_period_commits_counts_a_weekend_with_activity_as_an_active_day() {
        let raw = raw_with(|r| {
            r.commits = [9, 10, 12]
                .iter()
                .map(|&d| make_commit(&format!("c{d}"), &ana(), &oct12(d)))
                .collect();
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.active_days, 3);
        assert_eq!(a.streak, 2);
        assert_eq!(a.consistency, (5 * 3 + 2 * 2) as f64);
    }

    #[test]
    fn score_period_commits_counts_branch_commit_days_of_a_pr_merged_in_the_next_period() {
        let at = |d: u32| oct(d, 10);
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &stamp(2026, 10, 3, 12), |p| {
                p.commit_count = 3;
                p.branch_commits = [27, 28, 29]
                    .iter()
                    .map(|&d| branch_commit(&at(d), &[&ana()]))
                    .collect();
            })];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.active_days, 3);
        assert_eq!(a.merged_prs, 0);
        assert_eq!(a.delivery, 0.0);
    }

    // breadth

    fn module_files(n: usize) -> Vec<crate::contributors::RawFile> {
        (0..n)
            .map(|i| file(&format!("mod{i}/sub/a.ts"), 40))
            .collect()
    }

    #[test]
    fn score_period_breadth_awards_15_points_per_module_with_at_least_30_effective_lines() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.files = vec![
                    file("src/features/a.ts", 30),
                    file("src-tauri/src/a.rs", 29),
                    file_d("docs/a.md", 20, 20),
                    file("README.md", 40),
                ];
            })];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(
            sorted(a.modules.clone()),
            ["(root)", "docs", "src/features"]
        );
        assert_eq!(a.breadth, 45.0);
    }

    #[test]
    fn score_period_breadth_sums_the_lines_of_a_module_across_prs() {
        let raw = raw_with(|r| {
            r.prs = vec![
                pr_with("1", &ana(), &oct12(8), |p| {
                    p.files = vec![file("src/features/a.ts", 20)]
                }),
                pr_with("2", &ana(), &oct12(9), |p| {
                    p.files = vec![file("src/features/b.ts", 10)]
                }),
            ];
        });
        assert_eq!(of(&score(&raw), "Ana").breadth, 15.0);
    }

    #[test]
    fn score_period_breadth_caps_at_6_modules() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.files = module_files(9)
            })];
        });
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.modules.len(), 6);
        assert_eq!(a.breadth, 90.0);
    }

    #[test]
    fn score_period_breadth_ignores_ignored_files_and_closed_prs() {
        let raw = raw_with(|r| {
            let mut closed = closed_pr("2", &ana(), &oct12(9));
            closed.files = vec![file("src/features/a.ts", 500)];
            r.prs = vec![
                pr_with("1", &ana(), &oct12(8), |p| {
                    p.files = vec![file("vendor/x/a.ts", 500)]
                }),
                closed,
            ];
        });
        assert_eq!(of(&score(&raw), "Ana").breadth, 0.0);
    }

    #[test]
    fn score_period_breadth_only_credits_the_pr_owner() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.landing_co_authors = vec![bob()];
                p.files = vec![file("src/features/a.ts", 100)];
            })];
        });
        let scores = score(&raw);
        assert_eq!(of(&scores, "Ana").breadth, 15.0);
        assert_eq!(of(&scores, "Bob").breadth, 0.0);
    }

    // review

    fn base() -> RawData {
        raw_with(|r| r.prs = vec![make_pr("1", &ana(), &oct12(8))])
    }

    fn with_reviews(mut raw: RawData, reviews: Vec<crate::contributors::RawReview>) -> RawData {
        raw.reviews = reviews;
        raw
    }

    #[test]
    fn score_period_review_gives_8_plus_one_per_comment_capped_at_5_comments() {
        let raw = with_reviews(
            base(),
            vec![
                make_review("1", &bob(), &oct12(8), 3, "COMMENTED"),
                make_review("1", &cat(), &oct12(8), 50, "COMMENTED"),
            ],
        );
        let scores = score(&raw);
        assert_eq!(of(&scores, "Bob").review, 11.0);
        assert_eq!(of(&scores, "Cat").review, 13.0);
        assert_eq!(of(&scores, "Bob").reviewed_prs, 1);
    }

    #[test]
    fn score_period_review_gives_8_for_an_approval_without_comments() {
        let raw = with_reviews(base(), vec![make_approval("1", &bob(), &oct12(8))]);
        assert_eq!(of(&score(&raw), "Bob").review, 8.0);
    }

    #[test]
    fn score_period_review_counts_several_reviews_by_one_reviewer_on_one_pr_once() {
        let raw = with_reviews(
            base(),
            vec![
                make_review("1", &bob(), &oct12(7), 2, "CHANGES_REQUESTED"),
                make_review("1", &bob(), &oct12(8), 1, "COMMENTED"),
                make_review("1", &bob(), &oct12(9), 0, "APPROVED"),
            ],
        );
        let scores = score(&raw);
        let b = of(&scores, "Bob");
        assert_eq!(b.review, (8 + 3) as f64);
        assert_eq!(b.reviewed_prs, 1);
    }

    #[test]
    fn score_period_review_gives_nothing_for_reviewing_your_own_pr() {
        let raw = with_reviews(
            base(),
            vec![make_review("1", &ana(), &oct12(8), 4, "APPROVED")],
        );
        let scores = score(&raw);
        let a = of(&scores, "Ana");
        assert_eq!(a.review, 0.0);
        assert_eq!(a.reviewed_prs, 0);
    }

    #[test]
    fn score_period_review_gives_nothing_to_a_credited_co_author_for_reviewing_the_pr() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.landing_co_authors = vec![bob()]
            })];
            r.reviews = vec![
                make_review("1", &bob(), &oct12(8), 2, "APPROVED"),
                make_approval("1", &cat(), &oct12(8)),
            ];
        });
        let scores = score(&raw);
        assert_eq!(of(&scores, "Bob").review, 0.0);
        assert_eq!(of(&scores, "Cat").review, 8.0);
    }

    #[test]
    fn score_period_review_does_not_let_an_agent_authored_pr_be_reviewed_by_its_human_owner() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &claude(), &oct12(8), |p| {
                p.committer = Some(ana())
            })];
            r.reviews = vec![make_review("1", &ana(), &oct12(8), 2, "APPROVED")];
        });
        assert_eq!(of(&score(&raw), "Ana").review, 0.0);
    }

    #[test]
    fn score_period_review_matches_a_login_only_reviewer_to_the_person_seen_on_commits() {
        let mut raw = with_reviews(
            base(),
            vec![make_review(
                "1",
                &ident_login("bob", "", "bob"),
                &oct12(8),
                1,
                "APPROVED",
            )],
        );
        raw.prs.push(make_pr("2", &bob(), &oct12(9)));
        let scores = score(&raw);
        assert_eq!(scores.people.len(), 2);
        assert_eq!(of(&scores, "Bob").review, 9.0);
    }

    #[test]
    fn score_period_review_skips_bot_reviewers_unknown_prs_and_reviews_outside_the_period() {
        let raw = with_reviews(
            base(),
            vec![
                make_review("1", &dependabot(), &oct12(8), 2, "APPROVED"),
                make_review("nope", &bob(), &oct12(8), 2, "APPROVED"),
                make_review("1", &cat(), &stamp(2026, 8, 30, 23), 2, "APPROVED"),
                make_review("1", &cat(), &stamp(2026, 10, 1, 0), 2, "APPROVED"),
            ],
        );
        assert_eq!(names(&score(&raw)), ["Ana"]);
    }

    #[test]
    fn score_period_review_is_0_and_flagged_when_github_is_unavailable() {
        let mut raw = with_reviews(
            base(),
            vec![make_review("1", &bob(), &oct12(8), 2, "APPROVED")],
        );
        raw.gh_available = false;
        let scores = score(&raw);
        assert!(!scores.gh_available);
        assert_eq!(names(&scores), ["Ana"]);
        assert_eq!(of(&scores, "Ana").review, 0.0);
    }

    #[test]
    fn score_period_review_reports_gh_available_when_it_is() {
        assert!(score(&base()).gh_available);
    }

    // weights

    fn weights(delivery: f64, review: f64, care: f64, consistency: f64, breadth: f64) -> Weights {
        Weights {
            delivery,
            review,
            care,
            consistency,
            breadth,
        }
    }

    #[test]
    fn score_period_weights_multiplies_each_dimension_subtotal() {
        let raw = raw_with(|r| {
            r.prs = vec![
                pr_with("1", &ana(), &oct12(8), |p| {
                    p.files = vec![file("src/features/a.ts", 100), file("src/a.test.ts", 100)];
                    p.branch_commits = vec![branch_commit(&oct12(8), &[&ana()])];
                }),
                make_pr("2", &bob(), &oct12(9)),
            ];
            r.reviews = vec![make_review("2", &ana(), &oct12(9), 2, "APPROVED")];
            r.survival = vec![survival("1", 100, 100)];
        });
        let config = with_config(|c| c.weights = weights(2.0, 3.0, 4.0, 5.0, 6.0));
        let weighted = score_with(&raw, &config);
        let a = of(&weighted, "Ana");
        assert_close(
            a.score,
            a.delivery * 2.0 * a.quality
                + a.care * 4.0
                + a.review * 3.0
                + a.consistency * 5.0
                + a.breadth * 6.0,
            8,
        );
        let plain = score(&raw);
        let p = of(&plain, "Ana");
        assert_close(
            p.score,
            p.delivery_final + p.care + p.review + p.consistency + p.breadth,
            8,
        );
        assert!(a.score > p.score);
    }

    #[test]
    fn score_period_weights_exposes_weighted_parts_that_add_up_to_the_score_exactly() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(8), |p| {
                p.files = vec![file("src/features/a.ts", 100), file("src/a.test.ts", 100)];
            })];
            r.reviews = vec![make_review("1", &bob(), &oct12(9), 2, "APPROVED")];
            r.commits = vec![make_commit("c1", &bob(), &oct12(9))];
            r.survival = vec![survival("1", 100, 50)];
        });
        let config = with_config(|c| c.weights = weights(1.5, 0.5, 3.0, 2.0, 0.7));
        let scores = score_with(&raw, &config);
        for person in &scores.people {
            let parts = &person.parts;
            assert_eq!(
                person.score,
                parts.delivery + parts.care + parts.review + parts.consistency + parts.breadth
            );
        }
        let a = of(&scores, "Ana");
        assert_close(a.parts.delivery, a.delivery_final * 1.5, 8);
        assert_close(a.parts.care, a.care * 3.0, 8);
        assert_close(a.parts.breadth, a.breadth * 0.7, 8);
        assert_close(of(&scores, "Bob").parts.review, 5.0, 8);
    }

    #[test]
    fn score_period_weights_lets_a_zero_weight_remove_a_dimension() {
        let raw = raw_with(|r| r.prs = vec![make_pr("1", &ana(), &oct12(8))]);
        let config = with_config(|c| c.weights = weights(0.0, 1.0, 1.0, 0.0, 0.0));
        assert_eq!(of(&score_with(&raw, &config), "Ana").score, 0.0);
    }

    // AI assistance

    #[test]
    fn score_period_ai_assistance_is_the_share_of_prs_with_an_agent_author_or_co_author() {
        let raw = raw_with(|r| {
            r.prs = vec![
                pr_with("1", &ana(), &oct12(6), |p| {
                    p.landing_co_authors = vec![claude()]
                }),
                make_pr("2", &ana(), &oct12(7)),
                pr_with("3", &claude(), &oct12(8), |p| p.committer = Some(ana())),
                pr_with("4", &ana(), &oct12(9), |p| {
                    p.branch_commits = vec![branch_commit(&oct12(9), &[&ana(), &claude()])];
                }),
            ];
        });
        assert_eq!(of(&score(&raw), "Ana").ai_assisted_pct, 75);
    }

    #[test]
    fn score_period_ai_assistance_looks_at_the_landing_commit_of_the_pr() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &ana(), &oct12(6)),
                make_pr("2", &ana(), &oct12(7)),
            ];
            r.commits = vec![
                commit_with("c1", &ana(), &oct12(6), |c| {
                    c.pr_id = Some("1".into());
                    c.co_authors = vec![claude()];
                }),
                commit_with("c2", &ana(), &oct12(7), |c| c.pr_id = Some("2".into())),
            ];
        });
        assert_eq!(of(&score(&raw), "Ana").ai_assisted_pct, 50);
    }

    #[test]
    fn score_period_ai_assistance_counts_commits_that_belong_to_no_pr_as_units() {
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct12(6), |c| c.co_authors = vec![claude()]),
                make_commit("c2", &ana(), &oct12(7)),
                make_commit("c3", &ana(), &oct12(8)),
                make_commit("c4", &ana(), &oct12(9)),
            ];
        });
        assert_eq!(of(&score(&raw), "Ana").ai_assisted_pct, 25);
    }

    #[test]
    fn score_period_ai_assistance_does_not_change_the_score() {
        let with_ai = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct12(6), |p| {
                p.landing_co_authors = vec![claude()]
            })];
        });
        let without = raw_with(|r| r.prs = vec![make_pr("1", &ana(), &oct12(6))]);
        assert_eq!(
            of(&score(&with_ai), "Ana").score,
            of(&score(&without), "Ana").score
        );
    }

    #[test]
    fn score_period_ai_assistance_is_0_without_units() {
        let raw = raw_with(|r| {
            r.reviews = vec![make_approval("1", &bob(), &oct12(8))];
            r.prs = vec![make_pr("1", &ana(), &stamp(2026, 8, 3, 0))];
        });
        assert_eq!(of(&score(&raw), "Bob").ai_assisted_pct, 0);
    }

    // unattributed commits and newcomers

    #[test]
    fn score_period_unattributed_counts_agent_commits_nobody_can_own() {
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &claude(), &oct12(6), |c| c.committer = github()),
                commit_with("c2", &claude(), &oct12(7), |c| c.committer = github()),
                commit_with("c3", &claude(), &stamp(2026, 8, 7, 0), |c| {
                    c.committer = github()
                }),
            ];
        });
        let scores = score(&raw);
        assert_eq!(scores.unattributed.commits, 2);
        assert!(scores.people.is_empty());
    }

    #[test]
    fn score_period_newcomers_flags_people_whose_first_activity_is_inside_the_period() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a1", &ana(), &stamp(2026, 8, 10, 0)),
                make_commit("a2", &ana(), &oct12(5)),
                make_commit("b1", &bob(), &oct12(6)),
                make_commit("c1", &cat(), &stamp(2026, 8, 12, 0)),
            ];
        });
        assert_eq!(score(&raw).newcomers, ["bob@example.com"]);
    }

    #[test]
    fn score_period_newcomers_does_not_guess_when_the_raw_data_does_not_reach_the_previous_period()
    {
        let raw = raw_with(|r| {
            r.since = stamp(2026, 9, 1, 0);
            r.commits = vec![make_commit("b1", &bob(), &oct12(6))];
        });
        assert!(score(&raw).newcomers.is_empty());
    }

    #[test]
    fn score_period_newcomers_counts_a_review_as_earlier_activity() {
        let raw = raw_with(|r| {
            r.prs = vec![make_pr("1", &ana(), &oct12(8))];
            r.reviews = vec![
                make_approval("1", &bob(), &stamp(2026, 8, 20, 0)),
                make_approval("1", &bob(), &oct12(8)),
            ];
            r.commits = vec![make_commit("b1", &bob(), &oct12(6))];
        });
        assert_eq!(score(&raw).newcomers, ["ana@example.com"]);
    }

    // output

    #[test]
    fn score_period_output_sorts_people_by_score_then_name_and_carries_the_period() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &bob(), &oct12(8)),
                make_pr("2", &ana(), &oct12(8)),
                pr_with("3", &cat(), &oct12(8), |p| {
                    p.files = vec![file("src/a.ts", 5000)]
                }),
            ];
        });
        let scores = score(&raw);
        assert_eq!(scores.period.id, "2026-10");
        assert_eq!(names(&scores), ["Cat", "Ana", "Bob"]);
    }

    #[test]
    fn score_period_output_is_deterministic() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &bob(), &oct12(8)),
                make_pr("2", &ana(), &oct12(9)),
            ];
        });
        assert_eq!(score(&raw), score(&raw));
    }

    // attributeCommits

    fn attribute<'a>(raw: &'a RawData, config: &ContributorsConfig) -> Vec<AttributedCommit<'a>> {
        attribute_commits(raw, config, &make_resolver_with(raw, config))
    }

    #[test]
    fn attribute_commits_credits_human_authors_and_drops_excluded_ones() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("c1", &ana(), &oct12(5)),
                make_commit("c2", &dependabot(), &oct12(5)),
                make_commit("c3", &github(), &oct12(5)),
            ];
        });
        let out = attribute(&raw, &default_config());
        let got: Vec<(&str, Option<&str>)> = out
            .iter()
            .map(|a| (a.commit.hash.as_str(), a.person_id.as_deref()))
            .collect();
        assert_eq!(got, [("c1", Some("ana@example.com"))]);
    }

    #[test]
    fn attribute_commits_credits_an_agent_commit_to_the_owner_of_its_pr() {
        let raw = raw_with(|r| {
            r.prs = vec![make_pr("7", &ana(), &oct12(5))];
            r.commits = vec![commit_with("c1", &claude(), &oct12(5), |c| {
                c.pr_id = Some("7".into());
                c.committer = github();
            })];
        });
        let out = attribute(&raw, &default_config());
        assert_eq!(out[0].person_id.as_deref(), Some("ana@example.com"));
        assert!(out[0].ai_assisted);
    }

    #[test]
    fn attribute_commits_falls_back_to_a_human_committer_then_a_mapping_then_nobody() {
        let config = with_config(|c| {
            c.people = vec![person(
                "p-dev",
                "Dev",
                &["noreply@anthropic.com", "dev@example.com"],
            )];
        });
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &claude(), &oct12(5), |c| c.committer = bob()),
                commit_with("c2", &claude(), &oct12(6), |c| c.committer = github()),
            ];
        });
        let ids = |config: &ContributorsConfig| -> Vec<Option<String>> {
            attribute(&raw, config)
                .iter()
                .map(|a| a.person_id.clone())
                .collect()
        };
        assert_eq!(
            ids(&default_config()),
            [Some("bob@example.com".to_string()), None]
        );
        assert_eq!(
            ids(&config),
            [
                Some("bob@example.com".to_string()),
                Some("p-dev".to_string())
            ]
        );
        let lonely = raw_with(|r| {
            r.commits = vec![commit_with("c3", &claude(), &oct12(6), |c| {
                c.committer = github()
            })];
        });
        assert_eq!(attribute(&lonely, &default_config())[0].person_id, None);
    }

    #[test]
    fn attribute_commits_uses_the_committer_when_the_pr_itself_cannot_be_owned() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("7", &claude(), &oct12(5), |p| {
                p.committer = Some(github())
            })];
            r.commits = vec![commit_with("c1", &claude(), &oct12(5), |c| {
                c.pr_id = Some("7".into());
                c.committer = bob();
            })];
        });
        assert_eq!(
            attribute(&raw, &default_config())[0].person_id.as_deref(),
            Some("bob@example.com")
        );
    }

    #[test]
    fn attribute_commits_flags_agent_co_authors_and_filters_files() {
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct12(5), |c| {
                    c.co_authors = vec![claude()];
                    c.files = vec![file_d("src/a.ts", 5, 1), file("Cargo.lock", 900)];
                }),
                commit_with("c2", &ana(), &oct12(6), |c| c.co_authors = vec![bob()]),
            ];
        });
        let out = attribute(&raw, &default_config());
        assert!(out[0].ai_assisted);
        assert_eq!(out[0].files, [file_d("src/a.ts", 5, 1)]);
        assert!(!out[1].ai_assisted);
    }

    // attributePrs

    #[test]
    fn attribute_prs_credits_the_owner_reports_agent_only_prs_and_drops_bot_prs() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &ana(), &oct12(5)),
                pr_with("2", &claude(), &oct12(6), |p| p.committer = Some(bob())),
                pr_with("3", &claude(), &oct12(7), |p| p.committer = Some(github())),
                pr_with("4", &dependabot(), &oct12(8), |p| {
                    p.committer = Some(github())
                }),
            ];
        });
        let out = attribute_prs(&raw, &DEFAULT_CONFIG, &make_resolver(&raw));
        let got: Vec<(&str, Option<&str>)> = out
            .iter()
            .map(|o| (o.pr.id.as_str(), o.person_id.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("1", Some("ana@example.com")),
                ("2", Some("bob@example.com")),
                ("3", None)
            ]
        );
    }

    // worked example (scoring-vi.md section 7)

    mod worked_example {
        use super::*;

        // Oct 2026: the 5th is a Monday. The 16 active days contain a run of 9
        // working days (5-9, 12-15); every other active day is isolated.
        const ACTIVE_DAYS: [u32; 16] = [1, 5, 6, 7, 8, 9, 12, 13, 14, 15, 19, 21, 23, 27, 29, 31];

        fn raw() -> RawData {
            let x = ident_login("Member X", "x@example.com", "member-x");
            let y = ident_login("Member Y", "y@example.com", "member-y");
            // Every final diff is 295 effective lines: size = log2(296) = 8.21,
            // 10 + 4 * 8.21 + 9 = 51.84 per PR. PR 5 is net negative (+100 -390),
            // PRs 1 and 2 touch test files. Three modules reach 30 lines.
            let diffs = vec![
                vec![
                    file("src/features/model/a.ts", 235),
                    file("src/features/model/a.test.ts", 60),
                ],
                vec![
                    file("src-tauri/src/b.rs", 235),
                    file("src-tauri/src/b_test.rs", 60),
                ],
                vec![file("scripts/ci/c.sh", 295), file("Cargo.lock", 900)],
                vec![
                    file("src/features/model/d.ts", 295),
                    file("package-lock.json", 5000),
                ],
                vec![file_d("src-tauri/src/e.rs", 100, 390)],
                vec![file("scripts/ci/f.sh", 295)],
            ];
            let merged_at = |index: usize| oct(ACTIVE_DAYS[index], 18);
            let own_prs: Vec<RawPr> = diffs
                .into_iter()
                .enumerate()
                .map(|(i, files)| {
                    pr_with(&format!("x{}", i + 1), &x, &merged_at(i), |p| {
                        p.title = format!("Feature {}", i + 1);
                        p.commit_count = 7;
                        p.branch_commits = (0..7)
                            .map(|j| {
                                let at = oct(ACTIVE_DAYS[(i * 7 + j) % ACTIVE_DAYS.len()], 10);
                                branch_commit(&at, &[&x])
                            })
                            .collect();
                        p.landing_co_authors = if i % 2 == 0 { vec![claude()] } else { vec![] };
                        p.files = files;
                    })
                })
                .collect();
            let others_prs: Vec<RawPr> = (0..9)
                .map(|i| {
                    pr_with(&format!("y{}", i + 1), &y, &oct(2, 10 + i as u32), |p| {
                        p.files = vec![file("src/features/y.ts", 10)];
                    })
                })
                .collect();
            // 14 meaningful comments over 9 PRs, none above the cap of 5 on a PR.
            let review_comments = [2, 2, 2, 2, 2, 1, 1, 1, 1];
            let reviewer = ident_login("Member X", "", "member-x");
            let mut reviews: Vec<_> = others_prs
                .iter()
                .enumerate()
                .map(|(i, pr)| {
                    make_review(
                        &pr.id,
                        &reviewer,
                        &oct(3, 10 + i as u32),
                        review_comments[i],
                        "APPROVED",
                    )
                })
                .collect();
            reviews.push(make_review("x1", &reviewer, &oct12(8), 5, "APPROVED")); // self review: worth nothing
            let commits = own_prs
                .iter()
                .enumerate()
                .map(|(i, pr)| {
                    commit_with(&format!("land{i}"), &x, &merged_at(i), |c| {
                        c.pr_id = Some(pr.id.clone())
                    })
                })
                .collect();
            // 85% of 6000 written lines are still alive.
            let survival = own_prs
                .iter()
                .map(|pr| survival(&pr.id, 1000, 850))
                .collect();
            raw_with(|r| {
                r.prs = own_prs.iter().chain(others_prs.iter()).cloned().collect();
                r.commits = commits;
                r.reviews = reviews;
                r.survival = survival;
            })
        }

        #[test]
        fn matches_the_documented_numbers() {
            let raw = raw();
            let scores = score(&raw);
            let x = of(&scores, "Member X");
            assert_eq!(x.merged_prs, 6);
            assert!(x.prs.iter().all(|p| (p.size - 8.2).abs() < 0.05));
            assert!(x.prs.iter().all(|p| (p.iteration - 9.0).abs() < 1e-9));
            assert!(x.delivery > 310.8 - 1.5);
            assert!(x.delivery < 310.8 + 1.5);
            assert_close(x.survival.unwrap(), 0.85, 10);
            assert_close(x.quality, 1.14, 10);
            assert!(x.delivery_final > 354.3 - 1.5);
            assert!(x.delivery_final < 354.3 + 1.5);
            assert!((x.care - 28.5).abs() < 1.0);
            assert_eq!(x.review, 86.0);
            assert_eq!(x.reviewed_prs, 9);
            assert_eq!(x.active_days, 16);
            assert_eq!(x.streak, 9);
            assert_eq!(x.consistency, 98.0);
            assert_eq!(
                sorted(x.modules.clone()),
                ["scripts/ci", "src-tauri/src", "src/features"]
            );
            assert_eq!(x.breadth, 45.0);
            assert!((x.score - 612.0).abs() < 3.0);
            assert_close(x.parts.delivery, x.delivery_final, 10);
            assert_eq!(x.ai_assisted_pct, 50);
            assert_eq!(x.net_negative_prs, 1);
            assert_eq!(x.main_commits, 6);
            assert_eq!(x.branch_commits, 42);
        }
    }

    // period kinds

    fn score_in(
        raw: &RawData,
        period: &crate::contributors::model::period::Period,
        now: &DateTime<Local>,
    ) -> PeriodScores {
        score_period(
            raw,
            &default_config(),
            &make_resolver_with(raw, &default_config()),
            period,
            now,
        )
    }

    fn at(month0: u32, d: u32) -> String {
        stamp(2026, month0, d, 12)
    }

    fn commit(id: &str, iso: &str) -> crate::contributors::RawCommit {
        commit_with(id, &ana(), iso, |c| c.files = vec![file("src/a.ts", 5)])
    }

    #[test]
    fn score_period_kinds_scores_only_the_last_7_days_for_7d() {
        let now = local_at(2026, 9, 20, 10, 0);
        let raw = raw_with(|r| {
            r.commits = vec![
                commit("old", &oct12(13)),
                commit("a", &oct12(14)),
                commit("b", &oct12(20)),
            ];
        });
        let scores = score_in(&raw, &current_period(PeriodKind::Days7, &now, None), &now);
        assert_eq!(scores.period.id, "7d");
        assert_eq!(of(&scores, "Ana").active_days, 2);
    }

    #[test]
    fn score_period_kinds_counts_every_active_day_of_a_quarter_and_of_a_year() {
        let now = local_at(2026, 11, 20, 10, 0);
        let raw = raw_with(|r| {
            r.commits = vec![
                commit("q3", &at(8, 30)),
                commit("q4a", &at(9, 1)),
                commit("q4b", &at(10, 15)),
                commit("q4c", &at(11, 19)),
            ];
        });
        let q = score_in(&raw, &current_period(PeriodKind::Quarter, &now, None), &now);
        assert_eq!(of(&q, "Ana").active_days, 3);
        let y = score_in(&raw, &current_period(PeriodKind::Year, &now, None), &now);
        assert_eq!(of(&y, "Ana").active_days, 4);
    }

    #[test]
    fn score_period_kinds_flags_newcomers_against_the_previous_period_of_the_same_kind() {
        let now = local_at(2026, 9, 20, 10, 0);
        let week = current_period(PeriodKind::Days7, &now, None);
        let raw = raw_with(|r| {
            r.since = stamp(2026, 9, 7, 0);
            r.commits = vec![commit("a", &oct12(15))];
        });
        assert_eq!(score_in(&raw, &week, &now).newcomers, ["ana@example.com"]);
        let short = raw_with(|r| {
            r.since = stamp(2026, 9, 14, 0);
            r.commits = vec![commit("a", &oct12(15))];
        });
        assert!(score_in(&short, &week, &now).newcomers.is_empty());
    }

    #[test]
    fn score_period_kinds_flags_nobody_as_a_newcomer_in_all_time_which_has_no_period_before_it() {
        let now = local_at(2026, 9, 20, 10, 0);
        let all = current_period(PeriodKind::All, &now, Some(&local_date(2026, 8, 1)));
        let raw = raw_with(|r| {
            r.since = stamp(2026, 7, 1, 0);
            r.commits = vec![commit("a", &oct12(15))];
        });
        assert!(score_in(&raw, &all, &now).newcomers.is_empty());
    }

    #[test]
    fn scoring_constants_serialize_with_the_ts_key_order_and_number_text() {
        let json = serde_json::to_string(&SCORING).unwrap();
        assert!(
            json.starts_with(r#"{"PR_BASE":10,"SIZE_K":4,"PR_CAP":72,"ITER_K":3,"ITER_CAP":12,"#)
        );
        assert!(json.contains(
            r#""QUALITY_BASE":0.8,"QUALITY_SLOPE":0.4,"QUALITY_MIN":0.8,"QUALITY_MAX":1.2,"#
        ));
        assert!(json.ends_with(r#""OWNER_SHARE":0.6}"#));
    }
}
