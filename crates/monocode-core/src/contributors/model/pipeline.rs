//! The whole model the tabs render, built from raw data, config and the
//! toggles. Pure; port of `pipeline.ts`.

use super::identity::{build_resolver, identity_key, MailmapPerson, Resolver, ResolverOptions};
use super::leaderboard::{build_leaderboard, Leaderboard};
use super::palette::person_color_map;
use super::period::{is_in_period, previous_comparable_span, previous_period, sync_window, Period};
use super::scoring::{observed_identities, score_period, PeriodScores};
use super::stats::{bucket_granularity, build_stats, Previous, Stats, StatsInput};
use super::types::ContributorsConfig;
use super::unmapped::{person_rows, unmapped_identities, UnmappedIdentity};
use crate::contributors::{GitUser, RawData};
use chrono::{DateTime, Local};
use std::collections::BTreeMap;
use std::sync::Arc;

/// What the tabs render; derived from raw data, config and the toggles.
pub struct ContributorsModel {
    pub raw: Arc<RawData>,
    pub config: Arc<ContributorsConfig>,
    pub mailmap: Vec<MailmapPerson>,
    pub merge: bool,
    pub resolver: Resolver,
    /// The selected period; Stats and the Leaderboard both show it.
    pub period: Period,
    /// The period of the same kind right before; KPI deltas and rank movement
    /// compare with it. `None` for All time: there is nothing to compare with.
    pub previous_period: Option<Period>,
    pub current: PeriodScores,
    pub previous: Option<PeriodScores>,
    pub leaderboard: Leaderboard,
    pub stats: Stats,
    pub unmapped: Vec<UnmappedIdentity>,
    /// One color per person, shared by every tab so a person looks the same on
    /// the chart, the board and the Identities list. Built from everyone in
    /// the data, not from the chosen period.
    pub colors: BTreeMap<String, &'static str>,
    /// Person id of the repo's git user, when they appear in the data.
    pub me_id: Option<String>,
    /// Anything to show in the chosen period.
    pub has_activity: bool,
}

pub struct ModelInput {
    pub raw: Arc<RawData>,
    pub config: Arc<ContributorsConfig>,
    pub mailmap: Vec<MailmapPerson>,
    pub merge: bool,
    pub period: Period,
    pub now: DateTime<Local>,
    pub user: Option<GitUser>,
}

/// Pure; callers cache it, so rescoring never touches git or the network.
pub fn build_contributors_model(input: ModelInput) -> ContributorsModel {
    let ModelInput {
        raw,
        config,
        mailmap,
        merge,
        period,
        now,
        user,
    } = input;
    let observed = observed_identities(&raw);
    let resolver = build_resolver(
        &config,
        ResolverOptions {
            mailmap: mailmap.clone(),
            observed_identities: observed.clone(),
            merge,
        },
    );
    let before = previous_period(&period);
    let current = score_period(&raw, &config, &resolver, &period, &now);
    let previous = before
        .as_ref()
        .map(|before| score_period(&raw, &config, &resolver, before, &now));

    // Only an identity seen in the data resolves, so the resolver does not grow
    // a person for a git user who never committed here.
    let email = user
        .as_ref()
        .map(|u| super::js_trim(&u.email).to_lowercase())
        .filter(|e| !e.is_empty());
    let me_id = email.and_then(|email| {
        observed
            .iter()
            .find(|o| identity_key(o) == email)
            .map(|mine| resolver.person_of(mine))
    });

    let ids: Vec<String> = person_rows(&raw, &config, &resolver)
        .into_iter()
        .map(|p| p.id)
        .chain(current.people.iter().map(|p| p.person_id.clone()))
        .chain(
            previous
                .iter()
                .flat_map(|p| p.people.iter().map(|p| p.person_id.clone())),
        )
        .collect();
    let colors = person_color_map(&ids);
    let stats = build_stats(&StatsInput {
        raw: &raw,
        config: &config,
        resolver: &resolver,
        range_start: period.start,
        // The current period runs to today, so the chart has no empty future buckets.
        range_end: sync_window(&period, &now).to,
        // Same elapsed days of the previous period, so a partly elapsed period is
        // not compared with a whole one. Rank movement still uses the whole of it.
        previous: previous_comparable_span(&period, &now).map_or(Previous::None, Previous::Span),
        granularity: Some(bucket_granularity(&period)),
        colors: Some(&colors),
    });
    let leaderboard = build_leaderboard(&current, previous.as_ref(), me_id.as_deref());
    let unmapped = unmapped_identities(&raw, &config, &mailmap);
    let has_activity = raw
        .commits
        .iter()
        .any(|c| is_in_period(&period, &c.committed_at))
        || stats.kpis.prs_merged.value > 0;

    ContributorsModel {
        raw,
        config,
        mailmap,
        merge,
        resolver,
        period,
        previous_period: before,
        current,
        previous,
        leaderboard,
        stats,
        unmapped,
        colors,
        me_id,
        has_activity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contributors::model::config::default_config;
    use crate::contributors::model::period::{current_period, sync_window, PeriodKind};
    use crate::contributors::model::stats::BucketGranularity;
    use crate::contributors::model::test_fixtures::*;
    use crate::contributors::model::types::{ContributorsConfig, Person};
    use crate::contributors::{GitIdentity, GitUser, RawCommit, RawData, RawPr, RawReview};

    fn now() -> DateTime<Local> {
        local_at(2026, 9, 20, 10, 0)
    }

    fn month() -> Period {
        current_period(PeriodKind::Month, &now(), None)
    }

    /// The since the hook collects for a period.
    fn since_of(period: &Period) -> String {
        iso(&sync_window(period, &now()).from)
    }

    fn input(raw: RawData, period: Period) -> ModelInput {
        ModelInput {
            raw: Arc::new(raw),
            config: Arc::new(default_config()),
            mailmap: vec![],
            merge: true,
            period,
            now: now(),
            user: None,
        }
    }

    fn model(raw: RawData, period: Period) -> ContributorsModel {
        build_contributors_model(input(raw, period))
    }

    fn model_with(
        raw: RawData,
        period: Period,
        patch: impl FnOnce(&mut ModelInput),
    ) -> ContributorsModel {
        let mut i = input(raw, period);
        patch(&mut i);
        build_contributors_model(i)
    }

    fn raw() -> RawData {
        raw_with(|r| {
            r.commits = vec![
                commit_with("a1", &ana(), &oct12(5), |c| {
                    c.files = vec![file("src/a.ts", 10)]
                }),
                commit_with("a2", &ident("Ana B", "ana@laptop.local"), &oct12(6), |c| {
                    c.files = vec![file("src/b.ts", 5)]
                }),
                commit_with("b1", &bob(), &oct12(7), |c| {
                    c.files = vec![file("src/c.ts", 7)]
                }),
            ];
            r.prs = vec![make_pr("1", &ana(), &oct12(8))];
        })
    }

    fn with_people(people: Vec<Person>) -> ContributorsConfig {
        with_config(|c| c.people = people)
    }

    fn ana_person() -> Person {
        Person {
            id: "ana".into(),
            name: "Ana".into(),
            identities: vec!["ana@example.com".into(), "ana@laptop.local".into()],
        }
    }

    #[test]
    fn build_contributors_model_scores_the_period_and_the_one_before_it_and_builds_the_board_and_stats(
    ) {
        let m = model(raw(), month());
        assert_eq!(m.period.id, "2026-10");
        assert_eq!(m.previous_period.as_ref().unwrap().id, "2026-09");
        assert_eq!(m.current.period.id, "2026-10");
        assert_eq!(m.previous.as_ref().unwrap().period.id, "2026-09");
        assert!(!m.leaderboard.rows.is_empty());
        assert_eq!(m.stats.kpis.commits.value, 3);
        assert!(m.has_activity);
    }

    mod all_time {
        use super::*;

        fn first() -> DateTime<Local> {
            local_date(2026, 7, 3)
        }

        fn all() -> Period {
            current_period(PeriodKind::All, &now(), Some(&first()))
        }

        fn data() -> RawData {
            raw_with(|r| {
                r.since = since_of(&all());
                r.commits = vec![
                    commit_with("old", &ana(), &stamp(2026, 7, 3, 9), |c| {
                        c.files = vec![file("src/a.ts", 4)]
                    }),
                    commit_with("new", &bob(), &oct12(14), |c| {
                        c.files = vec![file("src/b.ts", 6)]
                    }),
                ];
                r.prs = vec![make_pr("1", &ana(), &oct12(8))];
            })
        }

        fn build() -> ContributorsModel {
            model(data(), all())
        }

        #[test]
        fn has_no_previous_period_or_scores_and_compares_nothing() {
            let m = build();
            assert_eq!(m.period.id, "all");
            assert!(m.previous_period.is_none());
            assert!(m.previous.is_none());
            let kpis = &m.stats.kpis;
            assert_eq!(kpis.commits.value, 2);
            assert_eq!(kpis.commits.delta, None);
            assert_eq!(kpis.lines.delta, None);
            assert_eq!(kpis.prs_merged.value, 1);
            assert_eq!(kpis.prs_merged.delta, None);
            assert_eq!(kpis.contributors.newcomers, 0);
        }

        #[test]
        fn ranks_everyone_without_a_rank_movement_or_a_newcomer_badge() {
            let m = build();
            let rows = &m.leaderboard.rows;
            assert_eq!(rows.len(), 2);
            for row in rows {
                assert_eq!(row.movement, None);
                assert!(!row.badges.iter().any(|b| b.id.as_str() == "newcomer"));
            }
        }

        #[test]
        fn buckets_the_chart_by_month_and_counts_the_whole_history() {
            let m = build();
            assert_eq!(m.stats.granularity, BucketGranularity::Month);
            assert_eq!(
                m.stats
                    .buckets
                    .iter()
                    .map(|b| b.start.as_str())
                    .collect::<Vec<_>>(),
                ["2026-08-01", "2026-09-01", "2026-10-01"]
            );
            assert!(m.has_activity);
        }

        #[test]
        fn still_gives_a_period_without_activity_an_empty_state() {
            let empty = raw_with(|r| r.since = since_of(&all()));
            assert!(!model(empty, all()).has_activity);
        }
    }

    #[test]
    fn build_contributors_model_merges_identities_through_config_people_only_when_merge_is_on() {
        let config = Arc::new(with_people(vec![ana_person()]));
        let on = model_with(raw(), month(), |i| i.config = config.clone());
        let off = model_with(raw(), month(), |i| {
            i.config = config.clone();
            i.merge = false;
        });
        let mut names: Vec<&str> = on.stats.rows.iter().map(|r| r.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["Ana", "Bob"]);
        assert_eq!(off.stats.rows.len(), 3);
    }

    #[test]
    fn build_contributors_model_marks_the_person_that_owns_the_git_user_email_as_me() {
        let m = model_with(raw(), month(), |i| {
            i.user = Some(GitUser {
                name: "Bob".into(),
                email: "BOB@example.com".into(),
            })
        });
        assert_eq!(m.me_id.as_deref(), Some("bob@example.com"));
        assert_eq!(
            m.leaderboard.rows.iter().find(|r| r.is_me).unwrap().name,
            "Bob"
        );
    }

    #[test]
    fn build_contributors_model_has_no_me_when_the_git_user_was_never_seen_or_has_no_email() {
        let user = |name: &str, email: &str| {
            Some(GitUser {
                name: name.into(),
                email: email.into(),
            })
        };
        assert_eq!(
            model_with(raw(), month(), |i| i.user = user("Zed", "z@x.io")).me_id,
            None
        );
        assert_eq!(
            model_with(raw(), month(), |i| i.user = user("", "")).me_id,
            None
        );
        assert_eq!(model(raw(), month()).me_id, None);
    }

    #[test]
    fn build_contributors_model_does_not_add_the_git_user_to_the_resolver_people_when_they_never_committed(
    ) {
        let m = model_with(raw(), month(), |i| {
            i.user = Some(GitUser {
                name: "Zed".into(),
                email: "z@x.io".into(),
            })
        });
        assert!(!m.resolver.people().iter().any(|p| p.id == "z@x.io"));
    }

    #[test]
    fn build_contributors_model_reports_no_activity_when_nothing_falls_inside_the_period() {
        assert!(!model(make_raw(), month()).has_activity);
    }

    #[test]
    fn build_contributors_model_counts_only_the_identities_that_need_attention_with_the_shared_selector(
    ) {
        let m = model(raw(), month());
        // ana@example.com and bob@example.com are ordinary authors; the .local
        // address is a machine.
        assert_eq!(
            m.unmapped
                .iter()
                .map(|u| u.key.as_str())
                .collect::<Vec<_>>(),
            ["ana@laptop.local"]
        );
    }

    #[test]
    fn build_contributors_model_compares_kpis_and_the_board_with_the_previous_period_of_the_same_kind(
    ) {
        let week = current_period(PeriodKind::Days7, &now(), None);
        let data = raw_with(|r| {
            r.since = since_of(&week);
            let four = |id: &str, at: &str| {
                commit_with(id, &ana(), at, |c| c.files = vec![file("src/a.ts", 4)])
            };
            r.commits = vec![
                four("old", &stamp(2026, 8, 25, 12)),
                four("new1", &oct12(14)),
                four("new2", &oct12(15)),
                four("new3", &oct12(16)),
            ];
        });
        let m = model(data, week);
        // 7d = Oct 14-20; the previous 7 days are Oct 7-13 (empty), not Sep 25.
        assert_eq!(m.stats.kpis.commits.value, 3);
        assert_eq!(m.stats.kpis.commits.delta, None);
        assert_eq!(m.stats.kpis.prs_merged.delta, Some(0));
        assert_eq!(m.period.id, "7d");
        assert_eq!(
            m.previous_period.as_ref().unwrap().start,
            local_date(2026, 9, 7)
        );
        assert_eq!(
            m.previous.as_ref().unwrap().period.start,
            local_date(2026, 9, 7)
        );
        assert!(m.previous.as_ref().unwrap().people.is_empty());
    }

    #[test]
    fn build_contributors_model_shows_deltas_for_a_month_with_the_since_the_hook_really_collects() {
        let data = raw_with(|r| {
            r.since = since_of(&month());
            r.commits = vec![
                commit_with("old", &ana(), &stamp(2026, 8, 5, 9), |c| {
                    c.files = vec![file("src/a.ts", 4)]
                }),
                commit_with("new", &ana(), &oct12(14), |c| {
                    c.files = vec![file("src/a.ts", 4)]
                }),
            ];
        });
        let m = model(data, month());
        assert!(m.stats.kpis.commits.delta.is_some());
        assert!(m.stats.kpis.prs_merged.delta.is_some());
    }

    #[test]
    fn build_contributors_model_merges_identities_through_the_mailmap_layer() {
        let m = model_with(raw(), month(), |i| {
            i.mailmap = vec![crate::contributors::model::identity::MailmapPerson {
                name: "Ana".into(),
                identities: vec!["ana@example.com".into(), "ana@laptop.local".into()],
            }]
        });
        let mut names: Vec<&str> = m.stats.rows.iter().map(|r| r.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["Ana", "Bob"]);
        assert!(m.unmapped.is_empty());
    }

    mod periods {
        use super::*;

        fn sept(day: u32) -> String {
            stamp(2026, 8, day, 12)
        }

        fn period_raw() -> RawData {
            let ten = |p: &str, n: u64| vec![file(p, n)];
            raw_with(|r| {
                r.since = stamp(2025, 0, 1, 0);
                r.commits = vec![
                    commit_with("y1", &ana(), &stamp(2025, 5, 3, 12), |c| {
                        c.files = ten("src/a.ts", 10)
                    }),
                    commit_with("q1", &ana(), &stamp(2026, 5, 3, 12), |c| {
                        c.files = ten("src/a.ts", 10)
                    }),
                    commit_with("s1", &bob(), &sept(8), |c| c.files = ten("src/a.ts", 30)),
                    commit_with("s2", &bob(), &sept(9), |c| c.files = ten("src/a.ts", 30)),
                    commit_with("o1", &ana(), &oct12(5), |c| c.files = ten("src/b.ts", 10)),
                    commit_with("o2", &ana(), &oct12(18), |c| c.files = ten("src/b.ts", 10)),
                ];
            })
        }

        fn build(period: Period) -> ContributorsModel {
            model(period_raw(), period)
        }

        fn row_names(m: &ContributorsModel) -> Vec<&str> {
            m.leaderboard.rows.iter().map(|r| r.name.as_str()).collect()
        }

        #[test]
        fn ranks_and_shows_the_stats_of_the_same_month() {
            let m = build(month());
            assert_eq!(m.period.id, "2026-10");
            assert_eq!(row_names(&m), ["Ana"]);
            assert_eq!(m.stats.kpis.commits.value, 2);
            assert_eq!(m.stats.granularity, BucketGranularity::Day);
        }

        #[test]
        fn re_scores_an_older_month_from_the_same_raw_data() {
            let m = build(prior_of(&month()));
            assert_eq!(m.period.id, "2026-09");
            assert_eq!(m.previous_period.as_ref().unwrap().id, "2026-08");
            assert_eq!(m.current.period.id, "2026-09");
            assert_eq!(row_names(&m), ["Bob"]);
            assert_eq!(m.stats.kpis.commits.value, 2);
        }

        #[test]
        fn moves_rank_against_the_previous_period_of_the_same_kind() {
            let m = build(month());
            assert_eq!(m.previous_period.as_ref().unwrap().id, "2026-09");
            assert_eq!(m.leaderboard.rows[0].movement, None);
        }

        #[test]
        fn covers_a_quarter_with_weekly_buckets_and_a_year_with_monthly_ones() {
            let quarter = build(current_period(PeriodKind::Quarter, &now(), None));
            assert_eq!(quarter.period.id, "2026-Q4");
            assert_eq!(quarter.stats.granularity, BucketGranularity::Week);
            assert_eq!(quarter.stats.kpis.commits.value, 2);
            let year = build(current_period(PeriodKind::Year, &now(), None));
            assert_eq!(year.period.id, "2026");
            assert_eq!(year.stats.granularity, BucketGranularity::Month);
            assert_eq!(year.stats.kpis.commits.value, 5);
            assert_eq!(
                year.stats
                    .buckets
                    .iter()
                    .map(|b| b.start.as_str())
                    .collect::<Vec<_>>(),
                [
                    "2026-01-01",
                    "2026-02-01",
                    "2026-03-01",
                    "2026-04-01",
                    "2026-05-01",
                    "2026-06-01",
                    "2026-07-01",
                    "2026-08-01",
                    "2026-09-01",
                    "2026-10-01",
                ]
            );
        }

        #[test]
        fn compares_a_partly_elapsed_month_with_the_same_days_of_the_previous_one() {
            let day3 = local_at(2026, 9, 3, 10, 0);
            let at = |m: u32, d: u32| stamp(2026, m, d, 12);
            let c = |id: &str, when: String| {
                commit_with(id, &ana(), &when, |c| c.files = vec![file("src/a.ts", 4)])
            };
            let data = raw_with(|r| {
                r.since = stamp(2026, 8, 1, 0);
                r.commits = vec![
                    c("s1", at(8, 1)),
                    c("s2", at(8, 2)),
                    c("s3", at(8, 3)),
                    c("s20", at(8, 20)),
                    c("s21", at(8, 21)),
                    c("o1", at(9, 1)),
                    c("o2", at(9, 2)),
                    c("o3", at(9, 3)),
                ];
            });
            let m = model_with(data, current_period(PeriodKind::Month, &day3, None), |i| {
                i.now = day3
            });
            // 3 commits so far against 3 in Sep 1-3 (not 5 in all of September).
            assert_eq!(m.stats.kpis.commits.value, 3);
            assert_eq!(m.stats.kpis.commits.delta, Some(0.0));
            // Rank movement still scores the whole previous month.
            let previous = m.previous.as_ref().unwrap();
            assert_eq!(previous.period.id, "2026-09");
            assert_eq!(previous.people[0].active_days, 5);
        }

        #[test]
        fn compares_a_complete_past_quarter_with_the_whole_previous_quarter() {
            let at = |m: u32, d: u32| stamp(2026, m, d, 12);
            let c = |id: &str, when: String| {
                commit_with(id, &ana(), &when, |c| c.files = vec![file("src/a.ts", 4)])
            };
            let data = raw_with(|r| {
                r.since = stamp(2026, 0, 1, 0);
                r.commits = vec![
                    c("q1a", at(0, 5)),
                    c("q1b", at(2, 28)),
                    c("q2a", at(3, 2)),
                    c("q2b", at(4, 2)),
                    c("q2c", at(5, 28)),
                    c("q2d", at(5, 29)),
                ];
            });
            let q2 = prior_of(&prior_of(&current_period(
                PeriodKind::Quarter,
                &now(),
                None,
            )));
            assert_eq!(q2.id, "2026-Q2");
            let m = model(data, q2);
            assert_eq!(m.stats.kpis.commits.value, 4);
            assert_eq!(m.stats.kpis.commits.delta, Some(1.0)); // 4 vs the 2 of Q1
        }

        #[test]
        fn compares_the_year_with_the_year_before() {
            let year = build(current_period(PeriodKind::Year, &now(), None));
            assert_eq!(year.previous_period.as_ref().unwrap().id, "2025");
            assert_eq!(year.stats.kpis.commits.delta, Some(4.0));
        }

        #[test]
        fn stops_the_chart_at_today_for_the_current_period() {
            let week = build(current_period(PeriodKind::Days7, &now(), None));
            assert_eq!(week.stats.buckets.len(), 7);
            assert_eq!(week.stats.buckets[6].start, "2026-10-20");
            let m = build(month());
            assert_eq!(m.stats.buckets.len(), 20);
        }

        #[test]
        fn has_no_activity_for_a_period_with_no_commits() {
            assert!(build(current_period(PeriodKind::Days7, &now(), None)).has_activity);
            let empty = build(prior_of(&prior_of(&month())));
            assert!(!empty.has_activity);
        }
    }

    mod person_colors {
        use super::*;

        fn people() -> Vec<GitIdentity> {
            (0..8)
                .map(|i| ident(&format!("P{i}"), &format!("person{i}@corp.com")))
                .collect()
        }

        fn many_raw() -> RawData {
            raw_with(|r| {
                r.since = stamp(2026, 7, 1, 0);
                r.commits = people()
                    .iter()
                    .enumerate()
                    .flat_map(|(i, who)| {
                        let i = i as u32;
                        [
                            commit_with(&format!("o{i}"), who, &oct12(5 + i % 5), |c| {
                                c.files = vec![file("src/a.ts", 3)]
                            }),
                            commit_with(
                                &format!("s{i}"),
                                who,
                                &stamp(2026, 8, 3 + i % 5, 12),
                                |c| c.files = vec![file("src/a.ts", 3)],
                            ),
                        ]
                    })
                    .collect();
            })
        }

        fn build_for(period: Period) -> ContributorsModel {
            model(many_raw(), period)
        }

        #[test]
        fn gives_every_person_a_color_and_no_two_of_eight_people_share_one() {
            let m = build_for(month());
            let ids: Vec<&str> = m
                .leaderboard
                .rows
                .iter()
                .map(|r| r.person_id.as_str())
                .collect();
            assert_eq!(ids.len(), 8);
            for id in &ids {
                assert!(m.colors.contains_key(*id), "{id}");
            }
            let distinct: std::collections::BTreeSet<&str> =
                ids.iter().map(|id| m.colors[*id]).collect();
            assert_eq!(distinct.len(), 8);
        }

        #[test]
        fn uses_the_same_colors_for_the_stats_rows_the_legend_and_the_board() {
            let m = build_for(month());
            for row in &m.stats.rows {
                assert_eq!(row.color, m.colors[row.person_id.as_str()]);
            }
            for entry in &m.stats.legend {
                assert_eq!(entry.color, m.colors[entry.person_id.as_str()]);
            }
        }

        #[test]
        fn keeps_a_persons_color_across_periods() {
            let base = build_for(month()).colors;
            assert_eq!(
                build_for(current_period(PeriodKind::Days7, &now(), None)).colors,
                base
            );
            assert_eq!(build_for(prior_of(&month())).colors, base);
        }

        #[test]
        fn keeps_colors_of_people_outside_the_period_distinct_too() {
            let m = build_for(current_period(PeriodKind::Days7, &now(), None));
            let distinct: std::collections::BTreeSet<&str> = m.colors.values().copied().collect();
            assert_eq!(distinct.len(), m.colors.len());
        }
    }

    // pipeline.perf.test.ts: the TS test asserts a median of 400 ms on its
    // synthetic repo; here the bound is generous because test profiles are not
    // optimized, and it exists to catch a quadratic regression.

    const DAY_MS: i64 = 86_400_000;

    /// Small deterministic generator so the fixture is the same on every run.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = (self.0 * 1_664_525 + 1_013_904_223) % 4_294_967_296;
            self.0 as f64 / 4_294_967_296.0
        }
    }

    /// A repo of about a team of 40: several emails per person, bots and agents.
    fn synthetic_raw(commit_count: usize, pr_count: usize) -> RawData {
        let mut rand = Lcg(42);
        let people: Vec<Vec<GitIdentity>> = (0..40)
            .map(|i| {
                vec![
                    ident_login(
                        &format!("Person {i}"),
                        &format!("person{i}@example.com"),
                        &format!("person{i}"),
                    ),
                    ident(&format!("Person {i}"), &format!("person{i}@laptop.local")),
                    ident(
                        &format!("P. {i}"),
                        &format!("{}+person{i}@users.noreply.github.com", 1000 + i),
                    ),
                ]
            })
            .collect();
        let others = [
            ident(
                "dependabot[bot]",
                "49699333+dependabot[bot]@users.noreply.github.com",
            ),
            ident("Claude", "noreply@anthropic.com"),
            ident("Copilot", "198982749+Copilot@users.noreply.github.com"),
            ident("Build Box", "ci@build-box"),
        ];
        let pick = |rand: &mut Lcg| -> GitIdentity {
            if rand.next() < 0.06 {
                return others[(rand.next() * others.len() as f64) as usize].clone();
            }
            let group = &people[(rand.next() * people.len() as f64) as usize];
            group[(rand.next() * group.len() as f64) as usize].clone()
        };
        let now_ms = now().timestamp_millis();
        let when = |rand: &mut Lcg| {
            let ms = now_ms - (rand.next() * 60.0 * DAY_MS as f64) as i64;
            chrono::DateTime::from_timestamp_millis(ms)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        };

        let mut commits: Vec<RawCommit> = vec![];
        for i in 0..commit_count {
            let author = pick(&mut rand);
            let at = when(&mut rand);
            let committer = if rand.next() < 0.2 {
                pick(&mut rand)
            } else {
                author.clone()
            };
            let co_authors = if rand.next() < 0.15 {
                vec![pick(&mut rand)]
            } else {
                vec![]
            };
            let n_files = 1 + (rand.next() * 6.0) as usize;
            let files = (0..n_files)
                .map(|n| {
                    file_d(
                        &format!(
                            "src/{}/f{}.ts",
                            ["app", "lib", "ui"][n % 3],
                            (rand.next() * 90.0) as u32
                        ),
                        1 + (rand.next() * 80.0) as u64,
                        (rand.next() * 30.0) as u64,
                    )
                })
                .collect();
            commits.push(commit_with(&format!("c{i}"), &author, &at, |c| {
                c.committer = committer;
                c.co_authors = co_authors;
                c.files = files;
                c.pr_id = (i < pr_count).then(|| (i + 1).to_string());
            }));
        }
        let mut prs: Vec<RawPr> = vec![];
        for i in 0..pr_count {
            let author = pick(&mut rand);
            let at = when(&mut rand);
            let branch_commits = (0..3)
                .map(|_| {
                    let at = when(&mut rand);
                    let authors = [pick(&mut rand), pick(&mut rand)];
                    branch_commit(&at, &[&authors[0], &authors[1]])
                })
                .collect();
            let landing = if rand.next() < 0.2 {
                vec![pick(&mut rand)]
            } else {
                vec![]
            };
            prs.push(pr_with(&(i + 1).to_string(), &author, &at, |p| {
                p.commit_count = 3;
                p.branch_commits = branch_commits;
                p.landing_co_authors = landing;
                p.files = vec![
                    file_d("src/app/x.ts", 40, 5),
                    file("package-lock.json", 900),
                ];
            }));
        }
        let reviews: Vec<RawReview> = (0..pr_count * 2)
            .map(|i| {
                let who = pick(&mut rand);
                let at = when(&mut rand);
                make_review(
                    &((i % pr_count) + 1).to_string(),
                    &who,
                    &at,
                    (i % 4) as u64,
                    "APPROVED",
                )
            })
            .collect();
        raw_with(|r| {
            r.since = chrono::DateTime::from_timestamp_millis(now_ms - 120 * DAY_MS)
                .unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            r.survival = prs
                .iter()
                .take(80)
                .map(|pr| crate::contributors::RawSurvival {
                    pr_id: pr.id.clone(),
                    written: 100,
                    alive: 80,
                })
                .collect();
            r.commits = commits;
            r.prs = prs;
            r.reviews = reviews;
        })
    }

    #[test]
    fn model_rebuild_performance_rebuilds_1500_commits_and_300_prs_well_within_a_budget() {
        let data = Arc::new(synthetic_raw(1500, 300));
        let build = || {
            build_contributors_model(ModelInput {
                raw: data.clone(),
                config: Arc::new(default_config()),
                mailmap: vec![],
                merge: true,
                period: month(),
                now: now(),
                user: None,
            })
        };
        // Warm the pattern caches the way a second edit finds them.
        build();
        let mut runs = vec![];
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let m = build();
            runs.push(start.elapsed().as_millis());
            assert!(m.leaderboard.rows.len() > 10);
        }
        runs.sort();
        // Target is 60 ms optimized on a laptop; unoptimized test builds are
        // several times slower, so the bound only catches a complexity blow-up.
        assert!(runs[1] < 2000, "median {} ms (runs {runs:?})", runs[1]);
    }
}
