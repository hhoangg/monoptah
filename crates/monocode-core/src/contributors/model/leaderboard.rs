//! Ranking, movement and badges. Port of `leaderboard.ts`.

use super::scoring::{PeriodScores, PersonScore};
use super::{js_round, locale_cmp};
use std::collections::HashMap;
use std::ops::Deref;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeId {
    Streak,
    Builder,
    Cleaner,
    BugHunter,
    Reviewer,
    Newcomer,
}

impl BadgeId {
    /// In the order badges are awarded and listed.
    pub const ALL: [BadgeId; 6] = [
        Self::Streak,
        Self::Builder,
        Self::Cleaner,
        Self::BugHunter,
        Self::Reviewer,
        Self::Newcomer,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Streak => "streak",
            Self::Builder => "builder",
            Self::Cleaner => "cleaner",
            Self::BugHunter => "bug-hunter",
            Self::Reviewer => "reviewer",
            Self::Newcomer => "newcomer",
        }
    }
}

/// `icon` is a glyph key; the UI maps it to an icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Badge {
    pub id: BadgeId,
    pub label: &'static str,
    pub icon: &'static str,
}

fn badge(id: BadgeId) -> Badge {
    let (label, icon) = match id {
        BadgeId::Streak => ("Streak", "fire"),
        BadgeId::Builder => ("Builder", "hammer"),
        BadgeId::Cleaner => ("Cleaner", "broom"),
        BadgeId::BugHunter => ("Bug hunter", "bug"),
        BadgeId::Reviewer => ("Reviewer", "eyes"),
        BadgeId::Newcomer => ("Newcomer", "sprout"),
    };
    Badge { id, label, icon }
}

/// A ranked person: the score plus where they stand.
#[derive(Clone, Debug, PartialEq)]
pub struct LeaderboardRow {
    pub person: PersonScore,
    pub rank: usize,
    /// Previous rank minus rank, positive = moved up; `None` without a rank before.
    pub movement: Option<i64>,
    pub badges: Vec<Badge>,
    pub is_me: bool,
}

impl Deref for LeaderboardRow {
    type Target = PersonScore;

    fn deref(&self) -> &PersonScore {
        &self.person
    }
}

const STREAK_BADGE_DAYS: usize = 10;
const MIN_BADGE_PRS: usize = 2;
const MAX_BADGES: usize = 3;
const TREND_PERIODS: usize = 6;
/// `Number.MIN_VALUE`: the smallest positive double, so "any points at all".
const MIN_VALUE: f64 = 5e-324;

fn has_activity(p: &PersonScore) -> bool {
    p.score > 0.0
        || p.active_days > 0
        || p.merged_prs > 0
        || p.main_commits > 0
        || p.reviewed_prs > 0
}

fn ranked(people: &[PersonScore]) -> Vec<&PersonScore> {
    let mut list: Vec<&PersonScore> = people.iter().filter(|p| has_activity(p)).collect();
    list.sort_by(|a, b| {
        js_round(b.score)
            .partial_cmp(&js_round(a.score))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.merged_prs.cmp(&a.merged_prs))
            .then_with(|| locale_cmp(&a.name, &b.name))
            .then_with(|| locale_cmp(&a.person_id, &b.person_id))
    });
    list
}

/// The first person (in rank order) with the highest `value`, if it reaches `min`.
fn leader(
    people: &[&PersonScore],
    value: impl Fn(&PersonScore) -> f64,
    min: f64,
) -> Option<String> {
    let mut best: Option<&PersonScore> = None;
    for p in people {
        if value(p) >= min && best.is_none_or(|b| value(p) > value(b)) {
            best = Some(p);
        }
    }
    best.map(|p| p.person_id.clone())
}

fn award_badges(people: &[&PersonScore], newcomers: &[String]) -> HashMap<String, Vec<Badge>> {
    let min_prs = MIN_BADGE_PRS as f64;
    let builder = leader(people, |p| p.delivery_final, MIN_VALUE);
    let cleaner = leader(people, |p| p.net_negative_prs as f64, min_prs);
    let bug_hunter = leader(people, |p| p.fix_prs as f64, min_prs);
    let reviewer = leader(people, |p| p.review, MIN_VALUE);
    let earned = |p: &PersonScore, id: BadgeId| match id {
        BadgeId::Streak => p.streak >= STREAK_BADGE_DAYS,
        BadgeId::Newcomer => newcomers.contains(&p.person_id),
        BadgeId::Builder => builder.as_deref() == Some(p.person_id.as_str()),
        BadgeId::Cleaner => cleaner.as_deref() == Some(p.person_id.as_str()),
        BadgeId::BugHunter => bug_hunter.as_deref() == Some(p.person_id.as_str()),
        BadgeId::Reviewer => reviewer.as_deref() == Some(p.person_id.as_str()),
    };
    people
        .iter()
        .map(|p| {
            let badges = BadgeId::ALL
                .into_iter()
                .filter(|id| earned(p, *id))
                .take(MAX_BADGES)
                .map(badge)
                .collect();
            (p.person_id.clone(), badges)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Leaderboard {
    pub rows: Vec<LeaderboardRow>,
    /// The first three rows.
    pub podium: Vec<LeaderboardRow>,
}

pub fn build_leaderboard(
    current: &PeriodScores,
    previous: Option<&PeriodScores>,
    me_id: Option<&str>,
) -> Leaderboard {
    let people = ranked(&current.people);
    let previous_rank: HashMap<&str, usize> = previous
        .map(|p| {
            ranked(&p.people)
                .into_iter()
                .enumerate()
                .map(|(i, person)| (person.person_id.as_str(), i + 1))
                .collect()
        })
        .unwrap_or_default();
    let mut badges = award_badges(&people, &current.newcomers);
    let rows: Vec<LeaderboardRow> = people
        .iter()
        .enumerate()
        .map(|(i, p)| LeaderboardRow {
            person: (*p).clone(),
            rank: i + 1,
            movement: previous_rank
                .get(p.person_id.as_str())
                .map(|before| *before as i64 - (i as i64 + 1)),
            badges: badges.remove(&p.person_id).unwrap_or_default(),
            is_me: me_id == Some(p.person_id.as_str()),
        })
        .collect();
    let podium = rows.iter().take(3).cloned().collect();
    Leaderboard { rows, podium }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrendLine {
    pub person_id: String,
    pub points: Vec<f64>,
}

/// Score per person over the last 6 periods, oldest first; 0 for a period they were absent.
pub fn trend(periods: &[PeriodScores]) -> Vec<TrendLine> {
    let mut recent: Vec<&PeriodScores> = periods.iter().collect();
    recent.sort_by_key(|s| s.period.start.timestamp_millis());
    let recent = &recent[recent.len().saturating_sub(TREND_PERIODS)..];
    let mut ids: Vec<&str> = vec![];
    for s in recent.iter().rev() {
        for p in &s.people {
            if !ids.contains(&p.person_id.as_str()) {
                ids.push(&p.person_id);
            }
        }
    }
    ids.into_iter()
        .map(|id| TrendLine {
            person_id: id.to_string(),
            points: recent
                .iter()
                .map(|s| {
                    s.people
                        .iter()
                        .find(|p| p.person_id == id)
                        .map_or(0.0, |p| p.score)
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::period::{current_period, PeriodKind};
    use crate::contributors::model::scoring::{Parts, PeriodScores, PersonScore, Unattributed};
    use crate::contributors::model::test_fixtures::local_date;

    fn person(id: &str, patch: impl FnOnce(&mut PersonScore)) -> PersonScore {
        let mut p = PersonScore {
            person_id: id.into(),
            name: id.into(),
            score: 100.0,
            delivery: 50.0,
            delivery_final: 50.0,
            quality: 1.0,
            survival: None,
            care: 0.0,
            review: 0.0,
            consistency: 10.0,
            breadth: 0.0,
            active_days: 2,
            streak: 2,
            merged_prs: 1,
            branch_commits: 1,
            main_commits: 1,
            ai_assisted_pct: 0,
            fix_prs: 0,
            net_negative_prs: 0,
            reviewed_prs: 0,
            parts: Parts {
                delivery: 50.0,
                care: 0.0,
                review: 0.0,
                consistency: 10.0,
                breadth: 0.0,
            },
            modules: vec![],
            prs: vec![],
        };
        patch(&mut p);
        p
    }

    fn season(people: Vec<PersonScore>) -> PeriodScores {
        season_in(people, 9, |_| {})
    }

    fn season_in(
        people: Vec<PersonScore>,
        month0: u32,
        patch: impl FnOnce(&mut PeriodScores),
    ) -> PeriodScores {
        let mut s = PeriodScores {
            period: current_period(PeriodKind::Month, &local_date(2026, month0, 10), None),
            people,
            unattributed: Unattributed { prs: 0, commits: 0 },
            gh_available: true,
            newcomers: vec![],
        };
        patch(&mut s);
        s
    }

    fn ids(rows: &[LeaderboardRow]) -> Vec<&str> {
        rows.iter().map(|r| r.person_id.as_str()).collect()
    }

    fn board(current: &PeriodScores) -> Leaderboard {
        build_leaderboard(current, None, None)
    }

    // ranking

    #[test]
    fn build_leaderboard_ranking_ranks_by_score_with_1_based_ordinal_ranks() {
        let Leaderboard { rows, podium } = board(&season(vec![
            person("low", |p| p.score = 10.0),
            person("high", |p| p.score = 300.0),
            person("mid", |p| p.score = 200.0),
        ]));
        assert_eq!(ids(&rows), ["high", "mid", "low"]);
        assert_eq!(rows.iter().map(|r| r.rank).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(ids(&podium), ["high", "mid", "low"]);
    }

    #[test]
    fn build_leaderboard_ranking_breaks_ties_by_merged_prs_then_name() {
        let Leaderboard { rows, .. } = board(&season(vec![
            person("zed", |p| p.merged_prs = 2),
            person("bob", |p| p.merged_prs = 1),
            person("amy", |p| p.merged_prs = 1),
            person("top", |p| p.merged_prs = 5),
        ]));
        assert_eq!(ids(&rows), ["top", "zed", "amy", "bob"]);
        assert_eq!(
            rows.iter().map(|r| r.rank).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
    }

    #[test]
    fn build_leaderboard_ranking_ranks_by_the_rounded_score_the_user_sees_then_merged_prs() {
        let Leaderboard { rows, .. } = board(&season(vec![
            person("few", |p| {
                p.score = 612.4;
                p.merged_prs = 1;
            }),
            person("many", |p| {
                p.score = 611.6;
                p.merged_prs = 4;
            }),
            person("ahead", |p| {
                p.score = 613.2;
                p.merged_prs = 0;
            }),
        ]));
        assert_eq!(ids(&rows), ["ahead", "many", "few"]);
    }

    #[test]
    fn build_leaderboard_ranking_limits_the_podium_to_the_top_3() {
        let people = ["a", "b", "c", "d", "e"]
            .iter()
            .enumerate()
            .map(|(i, id)| person(id, |p| p.score = 50.0 - i as f64))
            .collect();
        let Leaderboard { rows, podium } = board(&season(people));
        assert_eq!(rows.len(), 5);
        assert_eq!(ids(&podium), ["a", "b", "c"]);
    }

    #[test]
    fn build_leaderboard_ranking_omits_people_with_no_score_and_no_activity() {
        let empty = person("ghost", |p| {
            p.score = 0.0;
            p.active_days = 0;
            p.merged_prs = 0;
            p.main_commits = 0;
            p.reviewed_prs = 0;
        });
        let Leaderboard { rows, .. } = board(&season(vec![person("real", |_| {}), empty]));
        assert_eq!(ids(&rows), ["real"]);
    }

    #[test]
    fn build_leaderboard_ranking_flags_the_current_user() {
        let current = season(vec![
            person("a", |p| p.score = 2.0),
            person("b", |p| p.score = 1.0),
        ]);
        let Leaderboard { rows, .. } = build_leaderboard(&current, None, Some("b"));
        assert_eq!(
            rows.iter().map(|r| r.is_me).collect::<Vec<_>>(),
            [false, true]
        );
    }

    #[test]
    fn build_leaderboard_ranking_handles_an_empty_season() {
        let Leaderboard { rows, podium } = board(&season(vec![]));
        assert!(rows.is_empty());
        assert!(podium.is_empty());
    }

    // movement

    fn previous() -> PeriodScores {
        season_in(
            vec![
                person("a", |p| p.score = 300.0),
                person("b", |p| p.score = 200.0),
                person("c", |p| p.score = 100.0),
            ],
            8,
            |_| {},
        )
    }

    fn movements(rows: &[LeaderboardRow]) -> Vec<(&str, Option<i64>)> {
        rows.iter()
            .map(|r| (r.person_id.as_str(), r.movement))
            .collect()
    }

    #[test]
    fn build_leaderboard_movement_is_previous_rank_minus_current_rank() {
        let current = season(vec![
            person("c", |p| p.score = 900.0),
            person("a", |p| p.score = 800.0),
            person("b", |p| p.score = 700.0),
        ]);
        let Leaderboard { rows, .. } = build_leaderboard(&current, Some(&previous()), None);
        assert_eq!(
            movements(&rows),
            [("c", Some(2)), ("a", Some(-1)), ("b", Some(-1))]
        );
    }

    #[test]
    fn build_leaderboard_movement_is_0_when_the_rank_did_not_change() {
        let current = season(vec![person("a", |p| p.score = 9.0)]);
        let Leaderboard { rows, .. } = build_leaderboard(&current, Some(&previous()), None);
        assert_eq!(rows[0].movement, Some(0));
    }

    #[test]
    fn build_leaderboard_movement_is_null_for_someone_absent_from_the_previous_season() {
        let current = season(vec![
            person("new", |p| p.score = 999.0),
            person("a", |p| p.score = 5.0),
        ]);
        let Leaderboard { rows, .. } = build_leaderboard(&current, Some(&previous()), None);
        assert_eq!(movements(&rows), [("new", None), ("a", Some(-1))]);
    }

    #[test]
    fn build_leaderboard_movement_is_null_for_everyone_without_a_previous_season() {
        let Leaderboard { rows, .. } = board(&season(vec![person("a", |_| {})]));
        assert_eq!(rows[0].movement, None);
    }

    // badges

    fn badge_ids(rows: &[LeaderboardRow]) -> Vec<Vec<&'static str>> {
        rows.iter()
            .map(|r| r.badges.iter().map(|b| b.id.as_str()).collect())
            .collect()
    }

    #[test]
    fn build_leaderboard_badges_labels_badges_and_gives_each_a_glyph_key() {
        let current = season_in(
            vec![person("a", |p| {
                p.streak = 10;
                p.delivery_final = 10.0;
                p.fix_prs = 2;
                p.net_negative_prs = 2;
                p.review = 5.0;
            })],
            9,
            |s| s.newcomers = vec!["a".into()],
        );
        let Leaderboard { rows, .. } = board(&current);
        let badges = &rows[0].badges;
        assert_eq!(
            badges.iter().map(|b| b.label).collect::<Vec<_>>(),
            ["Streak", "Builder", "Cleaner"]
        );
        assert_eq!(
            badges.iter().map(|b| b.icon).collect::<Vec<_>>(),
            ["fire", "hammer", "broom"]
        );
    }

    #[test]
    fn build_leaderboard_badges_awards_streak_from_10_days() {
        let Leaderboard { rows, .. } = board(&season(vec![
            person("a", |p| {
                p.score = 3.0;
                p.streak = 10;
            }),
            person("b", |p| {
                p.score = 2.0;
                p.streak = 9;
            }),
        ]));
        assert!(badge_ids(&rows)[0].contains(&"streak"));
        assert!(!badge_ids(&rows)[1].contains(&"streak"));
    }

    #[test]
    fn build_leaderboard_badges_awards_builder_to_the_highest_delivery_only() {
        let Leaderboard { rows, .. } = board(&season(vec![
            person("a", |p| {
                p.score = 3.0;
                p.delivery_final = 40.0;
            }),
            person("b", |p| {
                p.score = 2.0;
                p.delivery_final = 90.0;
            }),
        ]));
        assert_eq!(badge_ids(&rows), [vec![], vec!["builder"]]);
    }

    #[test]
    fn build_leaderboard_badges_gives_a_tied_badge_to_the_higher_ranked_person() {
        let Leaderboard { rows, .. } = board(&season(vec![
            person("a", |p| {
                p.score = 3.0;
                p.delivery_final = 90.0;
            }),
            person("b", |p| {
                p.score = 2.0;
                p.delivery_final = 90.0;
            }),
        ]));
        assert_eq!(badge_ids(&rows), [vec!["builder"], vec![]]);
    }

    #[test]
    fn build_leaderboard_badges_does_not_award_builder_or_reviewer_for_zero_points() {
        let Leaderboard { rows, .. } = board(&season(vec![person("a", |p| {
            p.delivery_final = 0.0;
            p.review = 0.0;
        })]));
        assert!(rows[0].badges.is_empty());
    }

    #[test]
    fn build_leaderboard_badges_needs_2_net_negative_prs_for_cleaner_and_2_fixes_for_bug_hunter() {
        let one = board(&season(vec![person("a", |p| {
            p.delivery_final = 0.0;
            p.net_negative_prs = 1;
            p.fix_prs = 1;
        })]));
        assert!(one.rows[0].badges.is_empty());
        let two = board(&season(vec![
            person("a", |p| {
                p.score = 3.0;
                p.delivery_final = 0.0;
                p.net_negative_prs = 3;
            }),
            person("b", |p| {
                p.score = 2.0;
                p.delivery_final = 0.0;
                p.fix_prs = 2;
            }),
            person("c", |p| {
                p.score = 1.0;
                p.delivery_final = 0.0;
                p.fix_prs = 4;
            }),
        ]));
        assert_eq!(
            badge_ids(&two.rows),
            [vec!["cleaner"], vec![], vec!["bug-hunter"]]
        );
        assert_eq!(two.rows[2].badges[0].label, "Bug hunter");
    }

    #[test]
    fn build_leaderboard_badges_awards_reviewer_to_the_most_review_points() {
        let Leaderboard { rows, .. } = board(&season(vec![
            person("a", |p| {
                p.score = 3.0;
                p.delivery_final = 0.0;
                p.review = 20.0;
            }),
            person("b", |p| {
                p.score = 2.0;
                p.delivery_final = 0.0;
                p.review = 60.0;
            }),
        ]));
        assert_eq!(badge_ids(&rows), [vec![], vec!["reviewer"]]);
    }

    #[test]
    fn build_leaderboard_badges_awards_newcomer_from_the_season_data() {
        let current = season_in(vec![person("a", |p| p.delivery_final = 0.0)], 9, |s| {
            s.newcomers = vec!["a".into()]
        });
        let Leaderboard { rows, .. } = board(&current);
        assert_eq!(badge_ids(&rows), [vec!["newcomer"]]);
    }

    #[test]
    fn build_leaderboard_badges_gives_at_most_3_badges_in_a_fixed_order() {
        let current = season_in(
            vec![person("a", |p| {
                p.streak = 12;
                p.delivery_final = 99.0;
                p.net_negative_prs = 3;
                p.fix_prs = 3;
                p.review = 30.0;
            })],
            9,
            |s| s.newcomers = vec!["a".into()],
        );
        let Leaderboard { rows, .. } = board(&current);
        assert_eq!(badge_ids(&rows), [vec!["streak", "builder", "cleaner"]]);
    }

    // trend

    #[test]
    fn trend_returns_oldest_to_newest_scores_for_each_person() {
        let seasons = vec![
            season_in(
                vec![
                    person("a", |p| p.score = 30.0),
                    person("b", |p| p.score = 5.0),
                ],
                9,
                |_| {},
            ),
            season_in(vec![person("a", |p| p.score = 10.0)], 7, |_| {}),
            season_in(
                vec![
                    person("a", |p| p.score = 20.0),
                    person("c", |p| p.score = 7.0),
                ],
                8,
                |_| {},
            ),
        ];
        let out = trend(&seasons);
        let got: Vec<(&str, &[f64])> = out
            .iter()
            .map(|t| (t.person_id.as_str(), t.points.as_slice()))
            .collect();
        assert_eq!(
            got,
            [
                ("a", &[10.0, 20.0, 30.0][..]),
                ("b", &[0.0, 0.0, 5.0][..]),
                ("c", &[0.0, 7.0, 0.0][..]),
            ]
        );
    }

    #[test]
    fn trend_keeps_only_the_last_6_seasons() {
        let seasons: Vec<PeriodScores> = (0..8)
            .map(|i| season_in(vec![person("a", |p| p.score = (i + 1) as f64)], i, |_| {}))
            .collect();
        let out = trend(&seasons);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].person_id, "a");
        assert_eq!(out[0].points, [3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn trend_is_empty_without_seasons() {
        assert!(trend(&[]).is_empty());
    }
}
