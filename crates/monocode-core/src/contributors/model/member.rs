//! The numbers of one member's detail page: the stat cards with their change
//! against the previous period, the per-bucket trend, the areas of the code
//! and the recent PRs. Pure; counts use the same attribution as the table, the
//! KPIs and the calendar, so the page agrees with them.
use super::palette::color_from_map;
use super::period::{parse_iso, previous_comparable_span, sync_window};
use super::pipeline::ContributorsModel;
use super::scoring::{attribute_commits, attribute_prs, attribute_reviews, module_of};
use super::stats::{bucket_start, day_key, BucketGranularity};
use chrono::{DateTime, Local};
use std::collections::{BTreeMap, HashMap};

/// Areas listed on the page.
pub const MAX_AREAS: usize = 8;
/// Recent PRs listed on the page.
pub const MAX_RECENT_PRS: usize = 10;

/// A count with its change against the previous period.
#[derive(Clone, Debug, PartialEq)]
pub struct Tally {
    pub value: u64,
    /// `None` without a comparable previous period.
    pub delta: Option<i64>,
}

/// A figure with its change as a fraction of the previous one.
#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    pub value: u64,
    pub delta: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemberLines {
    pub added: u64,
    pub deleted: u64,
    /// Change in lines changed (added + deleted) as a fraction.
    pub delta: Option<f64>,
}

/// A directory the member changed, with its share of their lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub name: String,
    pub lines: u64,
    pub share_pct: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecentPr {
    pub id: String,
    pub number: Option<u64>,
    pub title: String,
    /// "merged" or "reverted".
    pub state: &'static str,
    pub merged_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemberDetail {
    pub person_id: String,
    pub name: String,
    pub color: String,
    pub identities: Vec<String>,
    pub commits: Measure,
    pub prs_merged: Tally,
    /// PRs reviewed: `PersonScore.reviewed_prs`.
    pub reviews: Tally,
    pub lines: MemberLines,
    /// Alive / written in 0..1, `None` when nothing was sampled.
    pub survival: Option<f64>,
    /// Change of survival in fraction points (0.05 is 5 points), against the whole
    /// previous period: a ratio is not deflated by a partial period like a count.
    pub survival_delta: Option<f64>,
    /// One entry per bucket of `model.stats.buckets`.
    pub commits_by_bucket: Vec<u64>,
    pub reviews_by_bucket: Vec<u64>,
    pub areas: Vec<Area>,
    pub recent_prs: Vec<RecentPr>,
}

/// A half-open range of instants.
#[derive(Clone, Copy)]
struct Range {
    start: DateTime<Local>,
    end: DateTime<Local>,
}
impl Range {
    fn contains(&self, iso: &str) -> bool {
        parse_iso(iso).is_some_and(|t| t >= self.start && t < self.end)
    }
}

fn fraction(current: f64, previous: f64) -> Option<f64> {
    (previous != 0.0).then(|| (current - previous) / previous)
}

/// What `of` did in a range: the same figures the KPIs total.
#[derive(Default)]
struct Counts {
    commits: u64,
    lines: u64,
    prs: u64,
    reviews: u64,
}

struct Facts<'a> {
    commits: Vec<(&'a str, Vec<(String, u64)>)>,
    prs: Vec<&'a str>,
    /// (PR id, review time) of the member's credited reviews.
    reviews: Vec<(&'a str, &'a str)>,
}

impl Facts<'_> {
    fn count(&self, range: &Range) -> Counts {
        let mut out = Counts::default();
        for (at, files) in &self.commits {
            if range.contains(at) {
                out.commits += 1;
                out.lines += files.iter().map(|(_, n)| n).sum::<u64>();
            }
        }
        out.prs = self.prs.iter().filter(|at| range.contains(at)).count() as u64;
        out.reviews = review_days(&self.reviews, range).len() as u64;
        out
    }
}

/// Each reviewed PR once, on the time of its first review inside the range: a
/// (reviewer, PR) pair counts once however many reviews it has, as
/// `reviewed_prs` and the calendar count it.
fn review_days(reviews: &[(&str, &str)], range: &Range) -> HashMap<String, DateTime<Local>> {
    let mut firsts: HashMap<String, DateTime<Local>> = HashMap::new();
    for (pr_id, at) in reviews {
        let Some(t) = parse_iso(at).filter(|t| *t >= range.start && *t < range.end) else {
            continue;
        };
        let slot = firsts.entry((*pr_id).to_string()).or_insert(t);
        *slot = (*slot).min(t);
    }
    firsts
}

/// The detail of `person_id` for the period of `model`; `None` when the person
/// has nothing in it. `now` decides how much of the previous period is
/// compared, as for the KPI deltas.
pub fn member_detail(
    model: &ContributorsModel,
    person_id: &str,
    now: &DateTime<Local>,
) -> Option<MemberDetail> {
    let row = model.stats.rows.iter().find(|r| r.person_id == person_id);
    let score = model
        .current
        .people
        .iter()
        .find(|p| p.person_id == person_id);
    if row.is_none() && score.is_none() {
        return None;
    }
    let (raw, config, resolver) = (&model.raw, &model.config, &model.resolver);
    let range = Range {
        start: model.period.start,
        end: sync_window(&model.period, now).to,
    };
    let span = previous_comparable_span(&model.period, now).map(|s| Range {
        start: s.start,
        end: s.end,
    });
    // Like the KPIs: no delta when the data does not reach back to the span.
    let span = span.filter(|s| parse_iso(&raw.since).is_some_and(|since| since <= s.start));

    let facts = Facts {
        commits: attribute_commits(raw, config, resolver)
            .into_iter()
            .filter(|c| c.person_id.as_deref() == Some(person_id))
            .map(|c| {
                (
                    c.commit.committed_at.as_str(),
                    c.files
                        .iter()
                        .map(|f| (f.path.clone(), f.added + f.deleted))
                        .collect(),
                )
            })
            .collect(),
        prs: attribute_prs(raw, config, resolver)
            .into_iter()
            .filter(|p| p.person_id.as_deref() == Some(person_id) && p.pr.state == "merged")
            .filter_map(|p| p.pr.merged_at.as_deref())
            .collect(),
        reviews: attribute_reviews(raw, config, resolver)
            .into_iter()
            .filter(|r| r.person_id == person_id)
            .map(|r| (r.review.pr_id.as_str(), r.review.submitted_at.as_str()))
            .collect(),
    };
    let now_counts = facts.count(&range);
    let before = span.as_ref().map(|s| facts.count(s));

    let (added, deleted) = row.map_or((0, 0), |r| (r.added, r.deleted));
    let commits = row.map_or(now_counts.commits, |r| r.commits);
    let reviews = score.map_or(0, |s| s.reviewed_prs as u64);

    // Per bucket, on the plan the chart uses.
    let granularity: BucketGranularity = model.stats.granularity;
    let index: HashMap<&str, usize> = model
        .stats
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| (b.start.as_str(), i))
        .collect();
    let commits_by_bucket = model
        .stats
        .buckets
        .iter()
        .map(|b| b.by_person.get(person_id).copied().unwrap_or(0))
        .collect();
    let mut reviews_by_bucket = vec![0u64; model.stats.buckets.len()];
    for at in review_days(&facts.reviews, &range).values() {
        let key = day_key(bucket_start(at.date_naive(), granularity));
        if let Some(&i) = index.get(key.as_str()) {
            reviews_by_bucket[i] += 1;
        }
    }

    // Areas: the lines of the member's commits in the period, by directory.
    let mut by_area: BTreeMap<String, u64> = BTreeMap::new();
    for (at, files) in &facts.commits {
        if !range.contains(at) {
            continue;
        }
        for (path, lines) in files {
            if *lines > 0 {
                *by_area.entry(module_of(path)).or_default() += lines;
            }
        }
    }
    let total: u64 = by_area.values().sum();
    let mut areas: Vec<Area> = by_area
        .into_iter()
        .map(|(name, lines)| Area {
            name,
            lines,
            share_pct: lines as f64 / total as f64 * 100.0,
        })
        .collect();
    areas.sort_by(|a, b| b.lines.cmp(&a.lines).then_with(|| a.name.cmp(&b.name)));
    areas.truncate(MAX_AREAS);

    let mut recent: Vec<(DateTime<Local>, RecentPr)> = score
        .map(|s| {
            s.prs
                .iter()
                .filter_map(|pr| {
                    let at = parse_iso(&pr.merged_at)?;
                    Some((
                        at,
                        RecentPr {
                            id: pr.id.clone(),
                            number: raw
                                .prs
                                .iter()
                                .find(|p| p.id == pr.id)
                                .and_then(|p| p.number),
                            title: pr.title.clone(),
                            state: if pr.reverted { "reverted" } else { "merged" },
                            merged_at: pr.merged_at.clone(),
                        },
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    recent.sort_by_key(|a| std::cmp::Reverse(a.0));
    recent.truncate(MAX_RECENT_PRS);

    let survival = score.and_then(|s| s.survival);
    let earlier_survival = span.as_ref().and(
        model
            .previous
            .as_ref()
            .and_then(|p| p.people.iter().find(|p| p.person_id == person_id))
            .and_then(|p| p.survival),
    );
    let diff = |now: u64, before: Option<u64>| before.map(|b| now as i64 - b as i64);

    Some(MemberDetail {
        person_id: person_id.to_string(),
        name: row
            .map(|r| r.name.clone())
            .or_else(|| score.map(|s| s.name.clone()))
            .unwrap_or_else(|| resolver.name_of(person_id)),
        color: row.map_or_else(
            || color_from_map(&model.colors, person_id).to_string(),
            |r| r.color.clone(),
        ),
        identities: row.map(|r| r.identities.clone()).unwrap_or_default(),
        commits: Measure {
            value: commits,
            delta: before
                .as_ref()
                .and_then(|b| fraction(commits as f64, b.commits as f64)),
        },
        prs_merged: Tally {
            value: now_counts.prs,
            delta: diff(now_counts.prs, before.as_ref().map(|b| b.prs)),
        },
        reviews: Tally {
            value: reviews,
            delta: diff(reviews, before.as_ref().map(|b| b.reviews)),
        },
        lines: MemberLines {
            added,
            deleted,
            delta: before
                .as_ref()
                .and_then(|b| fraction((added + deleted) as f64, b.lines as f64)),
        },
        survival,
        survival_delta: survival.zip(earlier_survival).map(|(a, b)| a - b),
        commits_by_bucket,
        reviews_by_bucket,
        areas,
        recent_prs: recent.into_iter().map(|(_, pr)| pr).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::period::{current_period, PeriodKind};
    use crate::contributors::model::pipeline::{build_contributors_model, ModelInput};
    use crate::contributors::model::test_fixtures::*;
    use crate::contributors::RawData;
    use std::sync::Arc;

    fn model_of(raw: RawData, patch: impl FnOnce(&mut ModelInput)) -> ContributorsModel {
        let mut input = ModelInput {
            raw: Arc::new(raw),
            config: Arc::new(default_config()),
            mailmap: vec![],
            merge: true,
            period: period(),
            now: period_now(),
            user: None,
            color_overrides: Default::default(),
        };
        patch(&mut input);
        build_contributors_model(input)
    }

    fn sep(day: u32, hour: u32) -> String {
        stamp(2026, 8, day, hour)
    }

    fn id_of(model: &ContributorsModel, who: &crate::contributors::GitIdentity) -> String {
        model.resolver.person_of(who)
    }

    fn sample() -> RawData {
        raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct(5, 9), |c| {
                    c.files = vec![file_d("src/a/x.rs", 10, 2), file_d("docs/y.md", 4, 0)]
                }),
                commit_with("c2", &ana(), &oct(6, 9), |c| {
                    c.files = vec![file_d("src/a/z.rs", 6, 0)]
                }),
                commit_with("c3", &ana(), &sep(10, 9), |c| {
                    c.files = vec![file_d("src/a/x.rs", 3, 0)]
                }),
                commit_with("c4", &bob(), &oct(7, 9), |_| {}),
            ];
            r.prs = vec![
                pr_with("1", &ana(), &oct(8, 12), |p| p.number = Some(41)),
                pr_with("2", &ana(), &oct(9, 12), |p| p.number = Some(42)),
                pr_with("3", &ana(), &sep(12, 12), |_| {}),
                pr_with("4", &bob(), &oct(9, 12), |_| {}),
            ];
            r.reviews = vec![
                make_approval("1", &bob(), &oct(8, 13)),
                make_approval("1", &bob(), &oct(10, 13)),
                make_approval("2", &bob(), &oct(11, 13)),
                make_approval("3", &bob(), &sep(13, 13)),
            ];
        })
    }

    #[test]
    fn totals_agree_with_the_table_and_the_scores() {
        let model = model_of(sample(), |_| {});
        let ana_id = id_of(&model, &ana());
        let bob_id = id_of(&model, &bob());
        let a = member_detail(&model, &ana_id, &period_now()).unwrap();
        let row = model
            .stats
            .rows
            .iter()
            .find(|r| r.person_id == ana_id)
            .unwrap();
        assert_eq!(a.commits.value, row.commits);
        assert_eq!((a.lines.added, a.lines.deleted), (row.added, row.deleted));
        assert_eq!(a.commits.value, 2);
        let score = model
            .current
            .people
            .iter()
            .find(|p| p.person_id == ana_id)
            .unwrap();
        assert_eq!(a.prs_merged.value as usize, score.merged_prs);
        assert_eq!(a.prs_merged.value, 2);
        let b = member_detail(&model, &bob_id, &period_now()).unwrap();
        let bob_score = model
            .current
            .people
            .iter()
            .find(|p| p.person_id == bob_id)
            .unwrap();
        assert_eq!(b.reviews.value as usize, bob_score.reviewed_prs);
        assert_eq!(b.reviews.value, 2);
    }

    #[test]
    fn changes_compare_with_the_previous_period() {
        let model = model_of(sample(), |_| {});
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        // Ana: 2 commits against 1; 2 merged PRs against 1; lines 22 against 3.
        assert_eq!(a.commits.delta, Some(1.0));
        assert_eq!(a.prs_merged.delta, Some(1));
        assert_eq!(a.lines.delta, Some((22.0 - 3.0) / 3.0));
        let b = member_detail(&model, &id_of(&model, &bob()), &period_now()).unwrap();
        // Bob reviewed PR 3 in September and PRs 1 and 2 in October.
        assert_eq!(b.reviews.delta, Some(1));
        let previous = model.previous.as_ref().unwrap();
        let bob_id = id_of(&model, &bob());
        let earlier = previous
            .people
            .iter()
            .find(|p| p.person_id == bob_id)
            .unwrap();
        assert_eq!(earlier.reviewed_prs, 1);
    }

    #[test]
    fn there_is_no_change_when_the_data_does_not_reach_the_previous_period() {
        let raw = raw_with(|r| {
            *r = sample();
            r.since = iso(&local_date(2026, 9, 1));
        });
        let model = model_of(raw, |_| {});
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        assert_eq!(a.commits.delta, None);
        assert_eq!(a.prs_merged.delta, None);
        assert_eq!(a.reviews.delta, None);
        assert_eq!(a.lines.delta, None);
        assert_eq!(a.survival_delta, None);
    }

    #[test]
    fn all_time_has_no_change() {
        let first = local_date(2026, 8, 10);
        let model = model_of(sample(), |i| {
            i.period = current_period(PeriodKind::All, &period_now(), Some(&first))
        });
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        assert_eq!(a.commits.delta, None);
        assert_eq!(a.prs_merged.delta, None);
        assert_eq!(a.reviews.delta, None);
    }

    #[test]
    fn a_review_pair_counts_once_in_the_bucket_of_its_first_review() {
        let model = model_of(sample(), |_| {});
        let bob_id = id_of(&model, &bob());
        let b = member_detail(&model, &bob_id, &period_now()).unwrap();
        assert_eq!(b.reviews_by_bucket.len(), model.stats.buckets.len());
        assert_eq!(b.reviews_by_bucket.iter().sum::<u64>(), b.reviews.value);
        let at = |label: &str| {
            model
                .stats
                .buckets
                .iter()
                .position(|x| x.short == label)
                .unwrap()
        };
        // PR 1 on Oct 8 (not again on Oct 10), PR 2 on Oct 11.
        assert_eq!(b.reviews_by_bucket[at("Oct 8")], 1);
        assert_eq!(b.reviews_by_bucket[at("Oct 10")], 0);
        assert_eq!(b.reviews_by_bucket[at("Oct 11")], 1);
    }

    #[test]
    fn the_review_trend_agrees_with_the_calendar_when_no_pair_spans_periods() {
        use crate::contributors::model::activity::build_activity;
        let model = model_of(sample(), |_| {});
        let bob_id = id_of(&model, &bob());
        let b = member_detail(&model, &bob_id, &period_now()).unwrap();
        let activity = build_activity(&model.raw, &model.config, &model.resolver);
        let in_october: u32 = activity.people[&bob_id]
            .days
            .iter()
            .filter(|(d, _)| d.format("%Y-%m").to_string() == "2026-10")
            .map(|(_, c)| c.reviews)
            .sum();
        assert_eq!(
            b.reviews_by_bucket.iter().sum::<u64>(),
            u64::from(in_october)
        );
    }

    #[test]
    fn commits_by_bucket_sum_to_the_commit_count() {
        let model = model_of(sample(), |_| {});
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        assert_eq!(a.commits_by_bucket.iter().sum::<u64>(), a.commits.value);
    }

    #[test]
    fn areas_rank_by_lines_and_respect_the_ignore_globs() {
        let config = with_config(|c| c.ignore = vec!["docs/**".into()]);
        let model = model_of(sample(), |i| i.config = Arc::new(config));
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        assert_eq!(
            a.areas,
            vec![Area {
                name: "src/a".into(),
                lines: 18,
                share_pct: 100.0
            }]
        );
        let all = model_of(sample(), |_| {});
        let a = member_detail(&all, &id_of(&all, &ana()), &period_now()).unwrap();
        let names: Vec<&str> = a.areas.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["src/a", "docs"]);
        assert!((a.areas[1].share_pct - 4.0 / 22.0 * 100.0).abs() < 1e-9);
    }

    #[test]
    fn areas_stop_at_eight() {
        let raw = raw_with(|r| {
            r.commits = vec![commit_with("c1", &ana(), &oct(5, 9), |c| {
                c.files = (1..=12)
                    .map(|n| file(&format!("src/m{n}/a.rs"), n))
                    .collect()
            })];
        });
        let model = model_of(raw, |_| {});
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        assert_eq!(a.areas.len(), MAX_AREAS);
        assert_eq!(a.areas[0].name, "src/m12");
    }

    #[test]
    fn recent_prs_are_newest_first_and_capped() {
        let raw = raw_with(|r| {
            r.prs = (1..=12)
                .map(|n| {
                    pr_with(&n.to_string(), &ana(), &oct(n as u32, 12), |p| {
                        p.number = Some(100 + n)
                    })
                })
                .collect();
        });
        let model = model_of(raw, |_| {});
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        assert_eq!(a.recent_prs.len(), MAX_RECENT_PRS);
        assert_eq!(a.recent_prs[0].number, Some(112));
        assert_eq!(a.recent_prs[0].state, "merged");
        assert_eq!(a.recent_prs[9].number, Some(103));
    }

    #[test]
    fn survival_is_none_when_nothing_was_sampled() {
        let model = model_of(sample(), |_| {});
        let a = member_detail(&model, &id_of(&model, &ana()), &period_now()).unwrap();
        assert_eq!(a.survival, None);
        assert_eq!(a.survival_delta, None);
    }

    #[test]
    fn a_person_without_activity_has_no_detail() {
        let model = model_of(sample(), |_| {});
        assert_eq!(member_detail(&model, "nobody", &period_now()), None);
    }
}
