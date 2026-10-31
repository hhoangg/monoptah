//! When people work: per-person daily counts for the activity calendar and a
//! weekday x hour rhythm of commits, both in local time. Pure; it reuses the
//! scoring attribution (who a commit or PR counts for, the ignore globs) and
//! the resolver, so the numbers agree with the rest of the page.
use super::identity::Resolver;
use super::period::parse_iso;
use super::scoring::{attribute_commits, attribute_prs, attribute_reviews};
use super::types::ContributorsConfig;
use crate::contributors::RawData;
use chrono::{Datelike, NaiveDate, Timelike};
use std::collections::BTreeMap;

/// What one person did on one local day.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DayCounts {
    pub commits: u32,
    pub reviews: u32,
    pub merged: u32,
    pub additions: u32,
    pub deletions: u32,
}
impl DayCounts {
    /// Commits, reviews and merged PRs count once each; lines never do.
    pub fn count(&self) -> u32 {
        self.commits + self.reviews + self.merged
    }

    fn add(&mut self, other: &DayCounts) {
        self.commits += other.commits;
        self.reviews += other.reviews;
        self.merged += other.merged;
        self.additions += other.additions;
        self.deletions += other.deletions;
    }
}

/// Commits by local weekday (Monday first) and hour of the day.
pub type Rhythm = [[u32; 24]; 7];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PersonActivity {
    pub days: BTreeMap<NaiveDate, DayCounts>,
    pub rhythm: Rhythm,
}
impl PersonActivity {
    /// The counts of `date`, zero when nothing happened.
    pub fn day(&self, date: NaiveDate) -> DayCounts {
        self.days.get(&date).copied().unwrap_or_default()
    }

    fn day_mut(&mut self, date: NaiveDate) -> &mut DayCounts {
        self.days.entry(date).or_default()
    }
}

/// Activity of the team and of each person in a stretch of history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    /// Everyone who is credited with something.
    pub team: PersonActivity,
    pub people: BTreeMap<String, PersonActivity>,
}
impl Activity {
    /// The team (`None`) or one person; `None` for a person with no activity.
    pub fn of(&self, person: Option<&str>) -> Option<&PersonActivity> {
        match person {
            None => Some(&self.team),
            Some(id) => self.people.get(id),
        }
    }
}

/// Counts for the whole data. Commits are credited like `attribute_commits`
/// (nobody-to-credit commits count for no one), merged PRs like
/// `attribute_prs` on the day they merged, and a review once per reviewer and
/// PR on the day of the first one, never for the PR's own author.
pub fn build_activity(raw: &RawData, config: &ContributorsConfig, resolver: &Resolver) -> Activity {
    let mut out = Activity::default();
    fn credit(out: &mut Activity, person: &str, date: NaiveDate, add: DayCounts) {
        out.people
            .entry(person.to_string())
            .or_default()
            .day_mut(date)
            .add(&add);
        out.team.day_mut(date).add(&add);
    }

    for item in attribute_commits(raw, config, resolver) {
        let Some(at) = parse_iso(&item.commit.committed_at) else {
            continue;
        };
        let date = at.date_naive();
        let Some(person) = &item.person_id else {
            continue;
        };
        let add = DayCounts {
            commits: 1,
            additions: item.files.iter().map(|f| f.added as u32).sum(),
            deletions: item.files.iter().map(|f| f.deleted as u32).sum(),
            ..Default::default()
        };
        credit(&mut out, person, date, add);
        let weekday = at.weekday().num_days_from_monday() as usize;
        let hour = at.hour() as usize;
        out.people.entry(person.clone()).or_default().rhythm[weekday][hour] += 1;
        out.team.rhythm[weekday][hour] += 1;
    }

    for item in attribute_prs(raw, config, resolver) {
        if item.pr.state != "merged" {
            continue;
        }
        let (Some(person), Some(at)) = (
            &item.person_id,
            item.pr.merged_at.as_deref().and_then(parse_iso),
        ) else {
            continue;
        };
        let add = DayCounts {
            merged: 1,
            ..Default::default()
        };
        credit(&mut out, person, at.date_naive(), add);
    }

    // The earliest review of each (reviewer, PR) pair decides its day. The
    // rule of who is credited is `attribute_reviews`, the one `reviewed_prs`
    // uses; a pair reviewed in two periods counts once here, in the first.
    let mut firsts: BTreeMap<(String, &str), chrono::DateTime<chrono::Local>> = BTreeMap::new();
    for item in attribute_reviews(raw, config, resolver) {
        let Some(at) = parse_iso(&item.review.submitted_at) else {
            continue;
        };
        let slot = firsts
            .entry((item.person_id, item.review.pr_id.as_str()))
            .or_insert(at);
        *slot = (*slot).min(at);
    }
    for ((reviewer, _), at) in firsts {
        let add = DayCounts {
            reviews: 1,
            ..Default::default()
        };
        credit(&mut out, &reviewer, at.date_naive(), add);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::test_fixtures::*;

    fn day(m0: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m0 + 1, d).unwrap()
    }

    fn build(raw: &RawData) -> (Activity, Resolver) {
        let resolver = make_resolver(raw);
        (build_activity(raw, &default_config(), &resolver), resolver)
    }

    #[test]
    fn commits_are_counted_per_person_and_day_with_lines() {
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct(12, 9), |c| {
                    c.files = vec![file_d("src/a.rs", 10, 2)]
                }),
                commit_with("c2", &ana(), &oct(12, 15), |c| {
                    c.files = vec![file_d("src/b.rs", 5, 1)]
                }),
                commit_with("c3", &bob(), &oct(13, 9), |_| {}),
            ];
        });
        let (activity, resolver) = build(&raw);
        let ana_id = resolver.person_of(&ana());
        let ana_day = activity.of(Some(&ana_id)).unwrap().day(day(9, 12));
        assert_eq!(ana_day.commits, 2);
        assert_eq!((ana_day.additions, ana_day.deletions), (15, 3));
        assert_eq!(activity.team.day(day(9, 12)).commits, 2);
        assert_eq!(activity.team.day(day(9, 13)).commits, 1);
        assert_eq!(activity.team.day(day(9, 14)), DayCounts::default());
    }

    #[test]
    fn ignored_files_do_not_count_as_lines() {
        let config = with_config(|c| c.ignore = vec!["gen/**".into()]);
        let raw = raw_with(|r| {
            r.commits = vec![commit_with("c1", &ana(), &oct(12, 9), |c| {
                c.files = vec![file("gen/x.rs", 500), file("src/a.rs", 7)];
            })];
        });
        let resolver = make_resolver_with(&raw, &config);
        let activity = build_activity(&raw, &config, &resolver);
        assert_eq!(activity.team.day(day(9, 12)).additions, 7);
    }

    #[test]
    fn rhythm_uses_local_weekday_and_hour_monday_first() {
        // Oct 12 2026 is a Monday.
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &ana(), &oct(12, 14), |_| {}),
                commit_with("c2", &ana(), &oct(12, 14), |_| {}),
                commit_with("c3", &bob(), &oct(18, 23), |_| {}),
            ];
        });
        let (activity, resolver) = build(&raw);
        assert_eq!(activity.team.rhythm[0][14], 2);
        assert_eq!(activity.team.rhythm[6][23], 1);
        let total: u32 = activity.team.rhythm.iter().flatten().sum();
        assert_eq!(total, 3);
        let bob_id = resolver.person_of(&bob());
        assert_eq!(activity.of(Some(&bob_id)).unwrap().rhythm[6][23], 1);
        assert_eq!(activity.of(Some(&bob_id)).unwrap().rhythm[0][14], 0);
    }

    #[test]
    fn merged_prs_count_on_the_merge_day_and_closed_ones_do_not() {
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &ana(), &oct(14, 10)),
                closed_pr("2", &ana(), &oct(15, 10)),
            ];
        });
        let (activity, resolver) = build(&raw);
        let ana_id = resolver.person_of(&ana());
        let mine = activity.of(Some(&ana_id)).unwrap();
        assert_eq!(mine.day(day(9, 14)).merged, 1);
        assert_eq!(mine.day(day(9, 15)).merged, 0);
    }

    #[test]
    fn a_review_counts_once_per_pr_and_never_for_the_author() {
        let raw = raw_with(|r| {
            r.prs = vec![make_pr("1", &ana(), &oct(14, 10))];
            r.reviews = vec![
                make_approval("1", &bob(), &oct(13, 9)),
                make_approval("1", &bob(), &oct(14, 9)),
                make_approval("1", &ana(), &oct(14, 9)),
                make_approval("zzz", &cat(), &oct(14, 9)),
            ];
        });
        let (activity, resolver) = build(&raw);
        let bob_id = resolver.person_of(&bob());
        let bob_activity = activity.of(Some(&bob_id)).unwrap();
        assert_eq!(bob_activity.day(day(9, 13)).reviews, 1);
        assert_eq!(bob_activity.day(day(9, 14)).reviews, 0);
        assert_eq!(
            activity.team.days.values().map(|d| d.reviews).sum::<u32>(),
            1
        );
    }

    #[test]
    fn reviews_are_skipped_without_github() {
        let raw = raw_with(|r| {
            r.gh_available = false;
            r.prs = vec![make_pr("1", &ana(), &oct(14, 10))];
            r.reviews = vec![make_approval("1", &bob(), &oct(13, 9))];
        });
        let (activity, _) = build(&raw);
        assert_eq!(
            activity.team.days.values().map(|d| d.reviews).sum::<u32>(),
            0
        );
    }

    #[test]
    fn bots_are_not_credited_and_a_missing_person_has_no_entry() {
        let raw = raw_with(|r| {
            r.commits = vec![
                commit_with("c1", &dependabot(), &oct(12, 9), |_| {}),
                commit_with("c2", &ana(), &oct(12, 9), |_| {}),
            ];
        });
        let (activity, _) = build(&raw);
        assert_eq!(activity.team.day(day(9, 12)).commits, 1);
        assert!(activity.of(Some("nobody")).is_none());
        assert!(activity.of(None).is_some());
    }

    #[test]
    fn a_co_author_reviewing_is_not_credited_like_reviewed_prs() {
        let raw = raw_with(|r| {
            r.prs = vec![pr_with("1", &ana(), &oct(14, 10), |p| {
                p.landing_co_authors = vec![bob()]
            })];
            r.reviews = vec![
                make_approval("1", &bob(), &oct(13, 9)),
                make_approval("1", &cat(), &oct(13, 9)),
            ];
        });
        let (activity, resolver) = build(&raw);
        let (bob_id, cat_id) = (resolver.person_of(&bob()), resolver.person_of(&cat()));
        assert!(activity
            .of(Some(&bob_id))
            .is_none_or(|a| a.days.values().all(|d| d.reviews == 0)));
        assert_eq!(
            activity.of(Some(&cat_id)).unwrap().day(day(9, 13)).reviews,
            1
        );
    }

    #[test]
    fn reviews_agree_with_reviewed_prs_for_a_period_holding_all_of_them() {
        use crate::contributors::model::scoring::score_period;
        let raw = raw_with(|r| {
            r.prs = vec![
                make_pr("1", &ana(), &oct(14, 10)),
                pr_with("2", &ana(), &oct(15, 10), |p| {
                    p.landing_co_authors = vec![bob()]
                }),
            ];
            r.reviews = vec![
                make_approval("1", &bob(), &oct(13, 9)),
                make_approval("1", &bob(), &oct(14, 9)),
                make_approval("2", &bob(), &oct(14, 9)),
                make_approval("2", &cat(), &oct(14, 9)),
                make_approval("1", &ana(), &oct(14, 9)),
            ];
        });
        let (activity, resolver) = build(&raw);
        let scores = score_period(&raw, &default_config(), &resolver, &period(), &period_now());
        for person in &scores.people {
            let calendar: u32 = activity
                .of(Some(&person.person_id))
                .map_or(0, |a| a.days.values().map(|d| d.reviews).sum());
            assert_eq!(calendar as usize, person.reviewed_prs, "{}", person.name);
        }
    }
}
