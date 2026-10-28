//! Stats tab: KPIs with deltas, chart buckets and the contributors table.
//! Port of `stats.ts`.

use super::identity::{identity_key, is_human, Resolver};
use super::palette::color_from_map;
use super::period::{midnight, parse_iso, Period, PeriodKind, Span, MONTHS};
use super::scoring::{attribute_commits, attribute_prs, AttributedCommit};
use super::types::ContributorsConfig;
use super::{js_cmp, js_round, locale_cmp, serialize_opt_number, OrderedMap};
use crate::contributors::RawData;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq)]
pub struct StatsRow {
    pub person_id: String,
    pub name: String,
    pub commits: u64,
    pub added: u64,
    pub deleted: u64,
    pub files: usize,
    pub ai_assisted_pct: u32,
    pub share_pct: f64,
    pub last_active: Option<String>,
    pub identities: Vec<String>,
    pub color: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketGranularity {
    Day,
    Week,
    Month,
    Year,
}

/// The most months All time keeps on a monthly axis; longer spans switch to years.
const MAX_MONTH_BUCKETS: i64 = 48;

/// Chart buckets for a period: days for 7d and month, weeks for a quarter,
/// months for a year. All time uses months while they fit on the axis
/// (`MAX_MONTH_BUCKETS`) and years beyond that.
pub fn bucket_granularity(period: &Period) -> BucketGranularity {
    match period.kind {
        PeriodKind::Days7 | PeriodKind::Month => BucketGranularity::Day,
        PeriodKind::Quarter => BucketGranularity::Week,
        PeriodKind::Year => BucketGranularity::Month,
        PeriodKind::All => {
            if calendar_months_touched(period) <= MAX_MONTH_BUCKETS {
                BucketGranularity::Month
            } else {
                BucketGranularity::Year
            }
        }
    }
}

/// How many calendar months the period touches, first and last included.
fn calendar_months_touched(period: &Period) -> i64 {
    // `end` is exclusive: step back to the last day the period covers.
    let last = period.end.date_naive() - Duration::days(1);
    let first = period.start.date_naive();
    i64::from(last.year() - first.year()) * 12 + i64::from(last.month0())
        - i64::from(first.month0())
        + 1
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    /// Local day key of the first day of the day, week (Monday), month or year.
    pub start: String,
    /// Tooltip text: "Oct 5, 2026", "Week of Oct 5, 2026", "Oct 2026" or "2026".
    pub label: String,
    /// Axis text: "Oct 5", "Oct 2026" or "2026".
    pub short: String,
    pub by_person: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Count {
    pub value: u64,
    #[serde(serialize_with = "serialize_opt_number")]
    pub delta: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Contributors {
    pub value: usize,
    pub identities: usize,
    pub newcomers: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lines {
    pub added: u64,
    pub deleted: u64,
    /// The fraction change in lines changed (added + deleted).
    #[serde(serialize_with = "serialize_opt_number")]
    pub delta: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrsMerged {
    pub value: u64,
    pub delta: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Kpis {
    pub commits: Count,
    pub contributors: Contributors,
    pub lines: Lines,
    pub prs_merged: PrsMerged,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub person_id: String,
    pub name: String,
    pub color: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub kpis: Kpis,
    pub granularity: BucketGranularity,
    pub buckets: Vec<Bucket>,
    pub rows: Vec<StatsRow>,
    pub legend: Vec<LegendEntry>,
}

/// The range the deltas compare with.
#[derive(Clone, Debug, PartialEq)]
pub enum Previous {
    /// The period of equal length right before the range start.
    Default,
    /// Nothing before (no deltas, no newcomers).
    None,
    Span(Span),
}

pub struct StatsInput<'a> {
    pub raw: &'a RawData,
    pub config: &'a ContributorsConfig,
    pub resolver: &'a Resolver,
    pub range_start: DateTime<Local>,
    pub range_end: DateTime<Local>,
    pub previous: Previous,
    /// Chart buckets: calendar weeks by default.
    pub granularity: Option<BucketGranularity>,
    /// Shared person colors; an id missing from it gets its hashed color.
    pub colors: Option<&'a BTreeMap<String, &'static str>>,
}

/// A half-open range in epoch milliseconds.
#[derive(Clone, Copy)]
struct Range {
    start: i64,
    end: i64,
}

impl Range {
    fn contains(&self, iso: &str) -> bool {
        millis(iso).is_some_and(|t| t >= self.start && t < self.end)
    }
}

fn millis(iso: &str) -> Option<i64> {
    parse_iso(iso).map(|t| t.timestamp_millis())
}

fn fraction(current: f64, previous: f64) -> Option<f64> {
    (previous != 0.0).then(|| (current - previous) / previous)
}

#[derive(Default)]
struct Totals {
    commits: u64,
    added: u64,
    deleted: u64,
    prs: u64,
}

pub fn build_stats(input: &StatsInput) -> Stats {
    let (raw, config, resolver) = (input.raw, input.config, input.resolver);
    let granularity = input.granularity.unwrap_or(BucketGranularity::Week);
    let range = Range {
        start: input.range_start.timestamp_millis(),
        end: input.range_end.timestamp_millis(),
    };
    let length = range.end - range.start;
    let previous: Option<Range> = match &input.previous {
        Previous::Default => Some(Range {
            start: range.start - length,
            end: range.start,
        }),
        Previous::None => None,
        Previous::Span(span) => Some(Range {
            start: span.start.timestamp_millis(),
            end: span.end.timestamp_millis(),
        }),
    };
    // Rule: every commit count here is of commits credited to a person, so the
    // KPI total equals the sum of the contributors table. Commits nobody can be
    // credited for (agent work with no human) are left out everywhere.
    let commits: Vec<AttributedCommit> = attribute_commits(raw, config, resolver)
        .into_iter()
        .filter(|c| c.person_id.is_some())
        .collect();
    let prs = attribute_prs(raw, config, resolver);

    let period_of = |r: &Range| -> Totals {
        let mut totals = Totals::default();
        for c in &commits {
            if !r.contains(&c.commit.committed_at) {
                continue;
            }
            totals.commits += 1;
            for f in &c.files {
                totals.added += f.added;
                totals.deleted += f.deleted;
            }
        }
        totals.prs = prs
            .iter()
            .filter(|p| {
                p.pr.state == "merged" && p.pr.merged_at.as_deref().is_some_and(|at| r.contains(at))
            })
            .count() as u64;
        totals
    };

    let now = period_of(&range);
    let earlier = previous.as_ref().map(period_of).unwrap_or_default();
    let previous_covered = previous
        .as_ref()
        .is_some_and(|p| millis(&raw.since).is_some_and(|since| since <= p.start));

    let inside: Vec<&AttributedCommit> = commits
        .iter()
        .filter(|c| range.contains(&c.commit.committed_at))
        .collect();
    let mut by_person: OrderedMap<Vec<&AttributedCommit>> = OrderedMap::new();
    for c in &inside {
        if let Some(person_id) = &c.person_id {
            by_person.entry(person_id, Vec::new).push(c);
        }
    }

    let empty = BTreeMap::new();
    let mut rows = build_rows(by_person, resolver, config, input.colors.unwrap_or(&empty));
    let total: u64 = rows.iter().map(|r| r.commits).sum();
    for row in &mut rows {
        row.share_pct = (row.commits as f64 / total as f64) * 100.0;
    }

    let mut first_commit: HashMap<&str, i64> = HashMap::new();
    for c in &commits {
        let Some(person_id) = &c.person_id else {
            continue;
        };
        let t = millis(&c.commit.committed_at).unwrap_or(i64::MIN);
        let cur = first_commit.entry(person_id).or_insert(t);
        if t < *cur {
            *cur = t;
        }
    }
    // Same rule as the leaderboard: the first activity in the data is inside
    // the range and the data reaches back to the start of the previous period.
    let newcomers = if previous_covered {
        rows.iter()
            .filter(|r| first_commit.get(r.person_id.as_str()).copied().unwrap_or(0) >= range.start)
            .count()
    } else {
        0
    };

    let identities: HashSet<&str> = rows
        .iter()
        .flat_map(|r| r.identities.iter().map(String::as_str))
        .collect();
    let kpis = Kpis {
        commits: Count {
            value: now.commits,
            delta: if previous_covered {
                fraction(now.commits as f64, earlier.commits as f64)
            } else {
                None
            },
        },
        contributors: Contributors {
            value: rows.len(),
            identities: identities.len(),
            newcomers,
        },
        lines: Lines {
            added: now.added,
            deleted: now.deleted,
            delta: if previous_covered {
                fraction(
                    (now.added + now.deleted) as f64,
                    (earlier.added + earlier.deleted) as f64,
                )
            } else {
                None
            },
        },
        prs_merged: PrsMerged {
            value: now.prs,
            delta: previous_covered.then(|| now.prs as i64 - earlier.prs as i64),
        },
    };
    let legend = rows
        .iter()
        .map(|r| LegendEntry {
            person_id: r.person_id.clone(),
            name: r.name.clone(),
            color: r.color.clone(),
        })
        .collect();
    Stats {
        kpis,
        granularity,
        buckets: build_buckets(&inside, input.range_start, input.range_end, granularity),
        rows,
        legend,
    }
}

fn build_rows(
    by_person: OrderedMap<Vec<&AttributedCommit>>,
    resolver: &Resolver,
    config: &ContributorsConfig,
    colors: &BTreeMap<String, &'static str>,
) -> Vec<StatsRow> {
    let mut rows: Vec<StatsRow> = by_person
        .into_pairs()
        .map(|(person_id, list)| {
            let mut added = 0;
            let mut deleted = 0;
            let mut ai = 0;
            let mut last = 0;
            let mut paths: HashSet<&str> = HashSet::new();
            let mut identities: HashSet<String> = HashSet::new();
            for c in &list {
                added += c.files.iter().map(|f| f.added).sum::<u64>();
                deleted += c.files.iter().map(|f| f.deleted).sum::<u64>();
                paths.extend(c.files.iter().map(|f| f.path.as_str()));
                if c.ai_assisted {
                    ai += 1;
                }
                last = last.max(millis(&c.commit.committed_at).unwrap_or(0));
                if is_human(config, &c.commit.author) {
                    identities.insert(identity_key(&c.commit.author));
                }
            }
            let mut identities: Vec<String> = identities.into_iter().collect();
            identities.sort_by(|a, b| js_cmp(a, b));
            StatsRow {
                name: resolver.name_of(&person_id),
                commits: list.len() as u64,
                added,
                deleted,
                files: paths.len(),
                ai_assisted_pct: js_round(ai as f64 / list.len() as f64 * 100.0) as u32,
                share_pct: 0.0,
                last_active: (last > 0).then(|| iso_millis(last)),
                identities,
                color: color_from_map(colors, &person_id).to_string(),
                person_id,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then_with(|| locale_cmp(&a.name, &b.name))
            .then_with(|| locale_cmp(&a.person_id, &b.person_id))
    });
    rows
}

/// `new Date(ms).toISOString()`.
fn iso_millis(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn day_key(date: NaiveDate) -> String {
    format!("{}-{:02}-{:02}", date.year(), date.month(), date.day())
}

fn bucket_at(start: NaiveDate, granularity: BucketGranularity) -> Bucket {
    let month = MONTHS[start.month0() as usize];
    let (year, day) = (start.year(), start.day());
    let (label, short) = match granularity {
        BucketGranularity::Year => (year.to_string(), year.to_string()),
        BucketGranularity::Month => (format!("{month} {year}"), format!("{month} {year}")),
        BucketGranularity::Day => (format!("{month} {day}, {year}"), format!("{month} {day}")),
        BucketGranularity::Week => (
            format!("Week of {month} {day}, {year}"),
            format!("{month} {day}"),
        ),
    };
    Bucket {
        start: day_key(start),
        label,
        short,
        by_person: BTreeMap::new(),
    }
}

/// The first day of the bucket a day belongs to.
fn bucket_start(date: NaiveDate, granularity: BucketGranularity) -> NaiveDate {
    let first = |y: i32, m: u32| NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(date);
    match granularity {
        BucketGranularity::Year => first(date.year(), 1),
        BucketGranularity::Month => first(date.year(), date.month()),
        BucketGranularity::Day => date,
        BucketGranularity::Week => {
            date - Duration::days(i64::from(date.weekday().num_days_from_monday()))
        }
    }
}

fn bucket_next(date: NaiveDate, granularity: BucketGranularity) -> NaiveDate {
    let first = |y: i32, m: u32| NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(date);
    match granularity {
        BucketGranularity::Year => first(date.year() + 1, 1),
        BucketGranularity::Month if date.month() == 12 => first(date.year() + 1, 1),
        BucketGranularity::Month => first(date.year(), date.month() + 1),
        BucketGranularity::Day => date + Duration::days(1),
        BucketGranularity::Week => date + Duration::days(7),
    }
}

fn build_buckets(
    inside: &[&AttributedCommit],
    range_start: DateTime<Local>,
    range_end: DateTime<Local>,
    granularity: BucketGranularity,
) -> Vec<Bucket> {
    let end = range_end.timestamp_millis();
    let mut buckets: OrderedMap<Bucket> = OrderedMap::new();
    let mut b = bucket_start(range_start.date_naive(), granularity);
    while midnight(b).timestamp_millis() < end {
        buckets.entry(&day_key(b), || bucket_at(b, granularity));
        b = bucket_next(b, granularity);
    }
    let mut buckets: Vec<(String, Bucket)> = buckets.into_pairs().collect();
    let index: HashMap<String, usize> = buckets
        .iter()
        .enumerate()
        .map(|(i, (key, _))| (key.clone(), i))
        .collect();
    for c in inside {
        let Some(person_id) = &c.person_id else {
            continue;
        };
        let Some(at) = parse_iso(&c.commit.committed_at) else {
            continue;
        };
        let key = day_key(bucket_start(at.date_naive(), granularity));
        if let Some(&i) = index.get(&key) {
            *buckets[i].1.by_person.entry(person_id.clone()).or_insert(0) += 1;
        }
    }
    buckets.into_iter().map(|(_, bucket)| bucket).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::palette::color_for_person;
    use crate::contributors::model::period::{current_period, PeriodKind, Span};
    use crate::contributors::model::test_fixtures::*;
    use crate::contributors::model::types::{ContributorsConfig, Person};
    use crate::contributors::{GitIdentity, RawData};
    use chrono::{DateTime, Local};

    fn sep(day: u32) -> String {
        stamp(2026, 8, day, 12)
    }

    fn range_start() -> DateTime<Local> {
        local_date(2026, 9, 5)
    }

    fn range_end() -> DateTime<Local> {
        local_date(2026, 9, 19)
    }

    fn ana_laptop() -> GitIdentity {
        ident("Ana", "ana@laptop.local")
    }

    fn config() -> ContributorsConfig {
        with_config(|c| {
            c.people = vec![Person {
                id: "p-ana".into(),
                name: "Ana".into(),
                identities: vec!["ana@example.com".into(), "ana@laptop.local".into()],
            }];
        })
    }

    fn stats(raw: &RawData) -> Stats {
        stats_with(raw, Previous::Default, &config())
    }

    fn stats_with(raw: &RawData, previous: Previous, cfg: &ContributorsConfig) -> Stats {
        build_stats(&StatsInput {
            raw,
            config: cfg,
            resolver: &make_resolver_with(raw, cfg),
            range_start: range_start(),
            range_end: range_end(),
            previous,
            granularity: None,
            colors: None,
        })
    }

    fn bucketed(
        raw: &RawData,
        start: DateTime<Local>,
        end: DateTime<Local>,
        granularity: BucketGranularity,
    ) -> Stats {
        let cfg = config();
        build_stats(&StatsInput {
            raw,
            config: &cfg,
            resolver: &make_resolver_with(raw, &cfg),
            range_start: start,
            range_end: end,
            previous: Previous::Default,
            granularity: Some(granularity),
            colors: None,
        })
    }

    fn sample() -> RawData {
        raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct12(6), |c| {
                    c.files = vec![file_d("src/a.ts", 10, 2), file("Cargo.lock", 500)];
                }),
                commit_with("c2", &ana_laptop(), &oct12(13), |c| {
                    c.co_authors = vec![claude()];
                    c.files = vec![file_d("src/b.ts", 5, 1)];
                }),
                commit_with("c3", &bob(), &oct12(7), |c| {
                    c.files = vec![file("src/c.ts", 20)]
                }),
                commit_with("c4", &dependabot(), &oct12(8), |c| {
                    c.files = vec![file("src/d.ts", 99)]
                }),
                commit_with("c5", &claude(), &oct12(9), |c| c.committer = github()),
                commit_with("c6", &cat(), &oct12(10), |c| {
                    c.files = vec![file("src/e.ts", 7)]
                }),
                make_commit("p0", &ana(), &sep(2)),
                commit_with("p1", &ana(), &sep(25), |c| {
                    c.files = vec![file("src/a.ts", 10)]
                }),
                commit_with("p2", &bob(), &sep(28), |c| {
                    c.files = vec![file_d("src/a.ts", 10, 10)]
                }),
                make_commit("p3", &bob(), &sep(29)),
            ];
            r.prs = vec![
                make_pr("1", &ana(), &oct12(8)),
                make_pr("2", &bob(), &oct12(12)),
                pr_with("3", &dependabot(), &oct12(9), |p| {
                    p.committer = Some(github())
                }),
                pr_with("4", &claude(), &oct12(10), |p| p.committer = Some(github())),
                closed_pr("5", &bob(), &oct12(11)),
                make_pr("old", &ana(), &sep(30)),
            ];
        })
    }

    fn bp(list: &[(&str, u64)]) -> BTreeMap<String, u64> {
        list.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    // kpis

    #[test]
    fn build_stats_kpis_counts_commits_with_a_delta_against_the_previous_period() {
        let kpis = stats(&sample()).kpis;
        assert_eq!(kpis.commits.value, 4);
        assert_close(kpis.commits.delta.unwrap(), (4.0 - 3.0) / 3.0, 10);
    }

    #[test]
    fn build_stats_kpis_counts_the_same_commits_as_the_contributors_table() {
        let s = stats(&sample());
        assert_eq!(
            s.kpis.commits.value,
            s.rows.iter().map(|r| r.commits).sum::<u64>()
        );
    }

    #[test]
    fn build_stats_kpis_counts_contributors_identities_and_newcomers() {
        let kpis = stats(&sample()).kpis;
        assert_eq!(
            kpis.contributors,
            Contributors {
                value: 3,
                identities: 4,
                newcomers: 1
            }
        );
    }

    #[test]
    fn build_stats_kpis_sums_ignore_filtered_lines_with_a_delta_on_lines_changed() {
        let kpis = stats(&sample()).kpis;
        assert_eq!(kpis.lines.added, 42);
        assert_eq!(kpis.lines.deleted, 3);
        assert_close(kpis.lines.delta.unwrap(), (45.0 - 30.0) / 30.0, 10);
    }

    #[test]
    fn build_stats_kpis_counts_merged_prs_with_an_absolute_delta() {
        let kpis = stats(&sample()).kpis;
        assert_eq!(kpis.prs_merged.value, 3);
        assert_eq!(kpis.prs_merged.delta, Some(3 - 1));
    }

    #[test]
    fn build_stats_kpis_excludes_the_range_end() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("in", &ana(), &stamp(2026, 9, 5, 0)),
                make_commit("out", &ana(), &stamp(2026, 9, 19, 0)),
            ];
        });
        assert_eq!(stats(&raw).kpis.commits.value, 1);
    }

    #[test]
    fn build_stats_kpis_uses_null_deltas_when_the_data_does_not_reach_the_previous_period() {
        let mut raw = sample();
        raw.since = stamp(2026, 9, 1, 0);
        let kpis = stats(&raw).kpis;
        assert_eq!(kpis.commits.delta, None);
        assert_eq!(kpis.lines.delta, None);
        assert_eq!(kpis.prs_merged.delta, None);
        assert_eq!(kpis.commits.value, 4);
    }

    #[test]
    fn build_stats_kpis_uses_a_null_fraction_when_the_previous_period_was_empty() {
        let raw = raw_with(|r| r.commits = vec![make_commit("c", &ana(), &oct12(6))]);
        let kpis = stats(&raw).kpis;
        assert_eq!(kpis.commits.delta, None);
        assert_eq!(kpis.lines.delta, None);
        assert_eq!(kpis.prs_merged.delta, Some(0));
    }

    #[test]
    fn build_stats_kpis_accepts_an_explicit_previous_period() {
        let previous = Previous::Span(Span {
            start: local_date(2026, 8, 28),
            end: local_date(2026, 9, 5),
        });
        let kpis = stats_with(&sample(), previous, &config()).kpis;
        assert_close(kpis.commits.delta.unwrap(), (4.0 - 2.0) / 2.0, 10);
    }

    #[test]
    fn build_stats_kpis_does_not_flag_newcomers_when_the_data_starts_at_the_range() {
        let raw = raw_with(|r| {
            r.since = iso(&range_start());
            r.commits = vec![make_commit("c", &ana(), &oct12(6))];
        });
        assert_eq!(stats(&raw).kpis.contributors.newcomers, 0);
    }

    #[test]
    fn build_stats_kpis_needs_data_reaching_the_previous_period_to_call_someone_new() {
        let raw = raw_with(|r| {
            r.since = stamp(2026, 9, 4, 0);
            r.commits = vec![make_commit("c", &ana(), &oct12(6))];
        });
        assert_eq!(stats(&raw).kpis.contributors.newcomers, 0);
    }

    #[test]
    fn build_stats_kpis_is_all_zeros_for_no_commits() {
        let Stats {
            kpis,
            rows,
            buckets,
            legend,
            ..
        } = stats(&make_raw());
        assert_eq!(
            kpis.commits,
            Count {
                value: 0,
                delta: None
            }
        );
        assert_eq!(
            kpis.contributors,
            Contributors {
                value: 0,
                identities: 0,
                newcomers: 0
            }
        );
        assert_eq!(
            kpis.lines,
            Lines {
                added: 0,
                deleted: 0,
                delta: None
            }
        );
        assert!(rows.is_empty());
        assert!(legend.is_empty());
        assert_eq!(
            buckets
                .iter()
                .map(|w| w.by_person.len())
                .collect::<Vec<_>>(),
            [0, 0]
        );
    }

    // week buckets

    #[test]
    fn build_stats_week_buckets_buckets_commits_per_person_into_monday_start_weeks() {
        let buckets = stats(&sample()).buckets;
        let got: Vec<(&str, &BTreeMap<String, u64>)> = buckets
            .iter()
            .map(|b| (b.start.as_str(), &b.by_person))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "2026-10-05",
                    &bp(&[("p-ana", 1), ("bob@example.com", 1), ("cat@example.com", 1)])
                ),
                ("2026-10-12", &bp(&[("p-ana", 1)])),
            ]
        );
        assert_eq!(buckets[0].label, "Week of Oct 5, 2026");
        assert_eq!(buckets[0].short, "Oct 5");
    }

    #[test]
    fn build_stats_week_buckets_starts_at_the_monday_before_a_mid_week_range_start() {
        let raw = raw_with(|r| r.commits = vec![make_commit("c", &ana(), &oct12(8))]);
        let cfg = config();
        let weekly = build_stats(&StatsInput {
            raw: &raw,
            config: &cfg,
            resolver: &make_resolver_with(&raw, &cfg),
            range_start: local_date(2026, 9, 8),
            range_end: local_date(2026, 9, 9),
            previous: Previous::Default,
            granularity: None,
            colors: None,
        })
        .buckets;
        assert_eq!(
            weekly.iter().map(|w| w.start.as_str()).collect::<Vec<_>>(),
            ["2026-10-05"]
        );
    }

    #[test]
    fn build_stats_week_buckets_keeps_empty_weeks() {
        let raw = raw_with(|r| r.commits = vec![make_commit("c", &ana(), &oct12(6))]);
        let cfg = config();
        let weekly = build_stats(&StatsInput {
            raw: &raw,
            config: &cfg,
            resolver: &make_resolver_with(&raw, &cfg),
            range_start: local_date(2026, 9, 5),
            range_end: local_date(2026, 9, 26),
            previous: Previous::Default,
            granularity: None,
            colors: None,
        })
        .buckets;
        assert_eq!(
            weekly.iter().map(|w| w.start.as_str()).collect::<Vec<_>>(),
            ["2026-10-05", "2026-10-12", "2026-10-19"]
        );
        assert!(weekly[1].by_person.is_empty());
    }

    #[test]
    fn build_stats_week_buckets_puts_a_sunday_commit_in_the_week_that_began_the_monday_before() {
        let raw = raw_with(|r| r.commits = vec![make_commit("c", &ana(), &oct12(11))]);
        assert_eq!(stats(&raw).buckets[0].by_person, bp(&[("p-ana", 1)]));
    }

    // rows and legend

    #[test]
    fn build_stats_rows_builds_one_row_per_person_busiest_first() {
        let rows = stats(&sample()).rows;
        assert_eq!(
            rows.iter()
                .map(|r| r.person_id.as_str())
                .collect::<Vec<_>>(),
            ["p-ana", "bob@example.com", "cat@example.com"]
        );
        let a = &rows[0];
        assert_eq!(a.name, "Ana");
        assert_eq!(a.commits, 2);
        assert_eq!(a.added, 15);
        assert_eq!(a.deleted, 3);
        assert_eq!(a.files, 2);
        assert_eq!(a.ai_assisted_pct, 50);
        assert_eq!(a.share_pct, 50.0);
        assert_eq!(a.last_active.as_deref(), Some(oct12(13).as_str()));
        assert_eq!(a.identities, ["ana@example.com", "ana@laptop.local"]);
        assert_eq!(rows[1].share_pct, 25.0);
    }

    #[test]
    fn build_stats_rows_colors_each_person_by_a_stable_hash_of_the_id_and_mirrors_it_in_the_legend()
    {
        let s = stats(&sample());
        for row in &s.rows {
            assert_eq!(row.color, color_for_person(&row.person_id));
        }
        let legend: Vec<(String, String, String)> = s
            .legend
            .iter()
            .map(|l| (l.person_id.clone(), l.name.clone(), l.color.clone()))
            .collect();
        let expected: Vec<(String, String, String)> = s
            .rows
            .iter()
            .map(|r| {
                (
                    r.person_id.clone(),
                    r.name.clone(),
                    color_for_person(&r.person_id).to_string(),
                )
            })
            .collect();
        assert_eq!(legend, expected);
    }

    #[test]
    fn build_stats_rows_gives_a_person_the_same_color_whatever_range_or_rank_they_have() {
        let raw = sample();
        let cfg = config();
        let wide = build_stats(&StatsInput {
            raw: &raw,
            config: &cfg,
            resolver: &make_resolver_with(&raw, &cfg),
            range_start: local_date(2026, 8, 1),
            range_end: local_date(2026, 10, 1),
            previous: Previous::Default,
            granularity: None,
            colors: None,
        });
        let narrow = stats(&raw);
        for row in &narrow.rows {
            let other = wide
                .rows
                .iter()
                .find(|r| r.person_id == row.person_id)
                .unwrap();
            assert_eq!(other.color, row.color);
        }
    }

    #[test]
    fn build_stats_rows_uses_the_shared_color_map_when_given() {
        let raw = sample();
        let cfg = config();
        let colors =
            crate::contributors::model::palette::person_color_map(&["bob@example.com", "p-ana"]);
        let s = build_stats(&StatsInput {
            raw: &raw,
            config: &cfg,
            resolver: &make_resolver_with(&raw, &cfg),
            range_start: range_start(),
            range_end: range_end(),
            previous: Previous::Default,
            granularity: None,
            colors: Some(&colors),
        });
        for row in &s.rows {
            assert_eq!(
                row.color,
                crate::contributors::model::palette::color_from_map(&colors, &row.person_id)
            );
        }
    }

    #[test]
    fn build_stats_rows_credits_an_agent_authored_commit_to_its_pr_owner() {
        let raw = raw_with(|r| {
            r.prs = vec![make_pr("9", &bob(), &oct12(8))];
            r.commits = vec![commit_with("c", &claude(), &oct12(8), |c| {
                c.pr_id = Some("9".into());
                c.committer = github();
                c.files = vec![file("src/a.ts", 4)];
            })];
        });
        let rows = stats(&raw).rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].person_id, "bob@example.com");
        assert_eq!(rows[0].commits, 1);
        assert_eq!(rows[0].added, 4);
        assert_eq!(rows[0].ai_assisted_pct, 100);
        assert!(rows[0].identities.is_empty());
    }

    #[test]
    fn build_stats_rows_breaks_commit_ties_by_name() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("c1", &cat(), &oct12(6)),
                make_commit("c2", &bob(), &oct12(6)),
            ];
        });
        let rows = stats_with(&raw, Previous::Default, &default_config()).rows;
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["Bob", "Cat"]
        );
    }

    // bucketGranularity

    #[test]
    fn bucket_granularity_is_day_for_7d_and_month_week_for_a_quarter_month_for_a_year() {
        let now = local_date(2026, 9, 20);
        let g = |kind| bucket_granularity(&current_period(kind, &now, None));
        assert_eq!(g(PeriodKind::Days7), BucketGranularity::Day);
        assert_eq!(g(PeriodKind::Month), BucketGranularity::Day);
        assert_eq!(g(PeriodKind::Quarter), BucketGranularity::Week);
        assert_eq!(g(PeriodKind::Year), BucketGranularity::Month);
    }

    fn all_g(now: &DateTime<Local>, first: DateTime<Local>) -> BucketGranularity {
        bucket_granularity(&current_period(PeriodKind::All, now, Some(&first)))
    }

    #[test]
    fn bucket_granularity_is_month_for_all_time_up_to_48_months_and_year_beyond() {
        let now = local_date(2026, 9, 20);
        // Jan 2023 .. Oct 2026 is 46 months, Oct 2022 .. Oct 2026 is 49.
        assert_eq!(
            all_g(&now, local_date(2023, 0, 1)),
            BucketGranularity::Month
        );
        assert_eq!(
            all_g(&now, local_date(2026, 9, 20)),
            BucketGranularity::Month
        );
        assert_eq!(all_g(&now, local_date(2022, 9, 3)), BucketGranularity::Year);
        assert_eq!(all_g(&now, local_date(2010, 0, 1)), BucketGranularity::Year);
    }

    #[test]
    fn bucket_granularity_keeps_month_at_exactly_48_months_and_switches_at_49() {
        let now = local_date(2026, 9, 20);
        assert_eq!(
            all_g(&now, local_date(2022, 11, 31)),
            BucketGranularity::Month
        ); // 47
        assert_eq!(
            all_g(&now, local_date(2022, 10, 1)),
            BucketGranularity::Month
        ); // 48
        assert_eq!(
            all_g(&now, local_date(2022, 9, 31)),
            BucketGranularity::Year
        ); // 49
    }

    #[test]
    fn bucket_granularity_counts_the_month_of_the_last_day_not_of_tomorrow() {
        // Today is Oct 31, so the period ends Nov 1 00:00 but touches only October.
        let now = local_at(2026, 9, 31, 10, 0);
        assert_eq!(
            all_g(&now, local_date(2022, 10, 1)),
            BucketGranularity::Month
        ); // 48
        assert_eq!(
            all_g(&now, local_date(2022, 9, 31)),
            BucketGranularity::Year
        );
    }

    // year buckets

    #[test]
    fn build_stats_year_buckets_makes_one_bucket_per_local_calendar_year_and_labels_it_with_the_year(
    ) {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a", &ana(), &stamp(2020, 5, 10, 12)),
                make_commit("b", &bob(), &iso(&local_dt(2022, 0, 1, 0, 30, 0, 0))),
                make_commit("c", &ana(), &iso(&local_dt(2022, 11, 31, 23, 30, 0, 0))),
            ];
        });
        let cfg = config();
        let Stats {
            buckets,
            granularity,
            ..
        } = build_stats(&StatsInput {
            raw: &raw,
            config: &cfg,
            resolver: &make_resolver_with(&raw, &cfg),
            range_start: local_date(2020, 5, 10),
            range_end: local_date(2023, 3, 2),
            previous: Previous::None,
            granularity: Some(BucketGranularity::Year),
            colors: None,
        });
        assert_eq!(granularity, BucketGranularity::Year);
        let starts: Vec<&str> = buckets.iter().map(|b| b.start.as_str()).collect();
        assert_eq!(
            starts,
            ["2020-01-01", "2021-01-01", "2022-01-01", "2023-01-01"]
        );
        let labels: Vec<&str> = buckets.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["2020", "2021", "2022", "2023"]);
        let shorts: Vec<&str> = buckets.iter().map(|b| b.short.as_str()).collect();
        assert_eq!(shorts, ["2020", "2021", "2022", "2023"]);
        assert_eq!(buckets[0].by_person, bp(&[("p-ana", 1)]));
        assert_eq!(
            buckets[2].by_person,
            bp(&[("bob@example.com", 1), ("p-ana", 1)])
        );
        assert!(buckets[3].by_person.is_empty());
    }

    // no previous range

    #[test]
    fn build_stats_no_previous_range_reports_no_deltas_and_no_newcomers_whatever_the_data_holds() {
        let kpis = stats_with(&sample(), Previous::None, &config()).kpis;
        assert_eq!(kpis.commits.value, 4);
        assert_eq!(kpis.commits.delta, None);
        assert_eq!(kpis.lines.delta, None);
        assert_eq!(kpis.prs_merged.delta, None);
        assert_eq!(kpis.contributors.newcomers, 0);
    }

    // day and month buckets

    #[test]
    fn build_stats_day_and_month_buckets_defaults_to_weeks() {
        assert_eq!(stats(&make_raw()).granularity, BucketGranularity::Week);
    }

    #[test]
    fn build_stats_day_and_month_buckets_makes_one_bucket_per_local_day_empty_days_included() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a", &ana(), &oct12(5)),
                make_commit("b", &bob(), &oct12(5)),
                make_commit("c", &ana(), &oct12(7)),
            ];
        });
        let Stats {
            buckets,
            granularity,
            ..
        } = bucketed(
            &raw,
            local_date(2026, 9, 5),
            local_date(2026, 9, 8),
            BucketGranularity::Day,
        );
        assert_eq!(granularity, BucketGranularity::Day);
        let starts: Vec<&str> = buckets.iter().map(|b| b.start.as_str()).collect();
        assert_eq!(starts, ["2026-10-05", "2026-10-06", "2026-10-07"]);
        assert_eq!(buckets[0].label, "Oct 5, 2026");
        assert_eq!(buckets[0].short, "Oct 5");
        assert_eq!(
            buckets[0].by_person,
            bp(&[("bob@example.com", 1), ("p-ana", 1)])
        );
        assert!(buckets[1].by_person.is_empty());
        assert_eq!(buckets[2].by_person, bp(&[("p-ana", 1)]));
    }

    #[test]
    fn build_stats_day_and_month_buckets_buckets_by_local_calendar_month_and_labels_them() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a", &ana(), &sep(20)),
                make_commit("b", &bob(), &oct12(3)),
                make_commit("c", &ana(), &oct12(30)),
            ];
        });
        let Stats {
            buckets,
            granularity,
            ..
        } = bucketed(
            &raw,
            local_date(2026, 8, 15),
            local_date(2026, 9, 31),
            BucketGranularity::Month,
        );
        assert_eq!(granularity, BucketGranularity::Month);
        let starts: Vec<&str> = buckets.iter().map(|b| b.start.as_str()).collect();
        assert_eq!(starts, ["2026-09-01", "2026-10-01"]);
        let labels: Vec<&str> = buckets.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels, ["Sep 2026", "Oct 2026"]);
        let shorts: Vec<&str> = buckets.iter().map(|b| b.short.as_str()).collect();
        assert_eq!(shorts, ["Sep 2026", "Oct 2026"]);
        assert_eq!(buckets[0].by_person, bp(&[("p-ana", 1)]));
        assert_eq!(
            buckets[1].by_person,
            bp(&[("bob@example.com", 1), ("p-ana", 1)])
        );
    }

    #[test]
    fn build_stats_day_and_month_buckets_keeps_empty_months_and_crosses_the_year_boundary() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a", &ana(), &stamp(2025, 10, 10, 12)),
                make_commit("b", &ana(), &stamp(2026, 1, 10, 12)),
            ];
        });
        let buckets = bucketed(
            &raw,
            local_date(2025, 10, 1),
            local_date(2026, 2, 1),
            BucketGranularity::Month,
        )
        .buckets;
        let starts: Vec<&str> = buckets.iter().map(|b| b.start.as_str()).collect();
        assert_eq!(
            starts,
            ["2025-11-01", "2025-12-01", "2026-01-01", "2026-02-01"]
        );
        assert!(buckets[1].by_person.is_empty());
    }

    #[test]
    fn build_stats_day_and_month_buckets_only_counts_commits_inside_the_range() {
        let raw = raw_with(|r| {
            r.commits = vec![
                make_commit("a", &ana(), &sep(2)),
                make_commit("b", &ana(), &oct12(3)),
            ];
        });
        let buckets = bucketed(
            &raw,
            local_date(2026, 8, 15),
            local_date(2026, 9, 31),
            BucketGranularity::Month,
        )
        .buckets;
        assert!(buckets[0].by_person.is_empty());
        assert_eq!(buckets[1].by_person, bp(&[("p-ana", 1)]));
    }

    #[test]
    fn build_stats_serializes_kpis_in_the_ts_shape() {
        let json = serde_json::to_string(&stats(&sample()).kpis).unwrap();
        assert!(json.starts_with(r#"{"commits":{"value":4,"delta":0.3333333333333333},"contributors":{"value":3,"identities":4,"newcomers":1},"lines":{"added":42,"deleted":3,"delta":0.5},"prsMerged":{"value":3,"delta":2}}"#), "{json}");
    }
}
