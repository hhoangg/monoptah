//! The pure rules of the shared chart: series ranking, the focus rules, the
//! y domain, the hit test, the tooltip and the screen reader summary.
use super::chart::{
    ChartBucket, ChartData, ChartKind, OTHERS_ID, Series, TOP, axis_labels, cumulative, hit_bucket,
    is_dimmed, others_color, summary, toggled, tooltip, top_series, y_scale,
};
use crate::theme::content;
use std::collections::BTreeSet;

fn series(id: &str, values: &[f64]) -> Series {
    Series {
        id: id.into(),
        name: id.to_uppercase(),
        color: content(0.9),
        values: values.to_vec(),
    }
}

fn buckets(n: usize) -> Vec<ChartBucket> {
    (0..n)
        .map(|i| ChartBucket {
            label: format!("Day {i}"),
            short: format!("D{i}"),
        })
        .collect()
}

fn data(series: Vec<Series>, n: usize) -> ChartData {
    ChartData {
        buckets: buckets(n),
        series,
        summary: String::new(),
    }
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_kinds_have_stable_ids_and_the_line_is_the_default() {
    assert_eq!(ChartKind::default(), ChartKind::Line);
    for kind in ChartKind::ALL {
        assert_eq!(ChartKind::parse(kind.id()), Some(kind));
    }
    assert_eq!(ChartKind::parse("line"), Some(ChartKind::Line));
    assert_eq!(ChartKind::parse("cumulative"), Some(ChartKind::Cumulative));
    assert_eq!(ChartKind::parse("bars"), Some(ChartKind::Bars));
    assert_eq!(ChartKind::parse("pie"), None);
    assert_eq!(ChartKind::parse(""), None);
}

#[test]
fn cumulative_is_a_running_total() {
    assert_eq!(cumulative(&[1., 0., 3., 2.]), [1., 1., 4., 6.]);
    assert!(cumulative(&[]).is_empty());
}

#[test]
fn the_top_five_by_total_stay_and_the_rest_are_summed_into_others() {
    let all = vec![
        series("a", &[1., 0.]),
        series("b", &[5., 5.]),
        series("c", &[0., 2.]),
        series("d", &[3., 3.]),
        series("e", &[4., 1.]),
        series("f", &[1., 1.]),
        series("g", &[0., 1.]),
    ];
    let top = top_series(all);
    let ids: Vec<&str> = top.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(top.len(), TOP + 1);
    // Totals: b 10, d 6, e 5, c 2, then a 1, f 2, g 1: c and f tie at 2, c came first.
    assert_eq!(ids, ["b", "d", "e", "c", "f", OTHERS_ID]);
    let others = top.last().unwrap();
    assert_eq!(others.name, "Others");
    assert_eq!(others.values, [1., 1.]);
    assert_eq!(others.color, others_color());
}

#[test]
fn five_or_fewer_series_get_no_others() {
    let five: Vec<Series> = (0..5).map(|i| series(&format!("p{i}"), &[1.])).collect();
    assert!(top_series(five).iter().all(|s| s.id != OTHERS_ID));
    assert!(top_series(vec![]).is_empty());
    let one = top_series(vec![series("a", &[2.])]);
    assert_eq!(one.len(), 1);
}

#[test]
fn ties_keep_the_order_the_series_came_in() {
    let all: Vec<Series> = ["x", "y", "z", "w", "v", "u", "t"]
        .iter()
        .map(|id| series(id, &[1., 1.]))
        .collect();
    let top = top_series(all);
    let ids: Vec<&str> = top.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["x", "y", "z", "w", "v", OTHERS_ID]);
    assert_eq!(top.last().unwrap().values, [2., 2.]);
}

#[test]
fn an_empty_focus_dims_nobody_and_a_focus_dims_the_rest() {
    let present = ["a", "b", "c"];
    assert!(!is_dimmed(&set(&[]), &present, "a"));
    assert!(!is_dimmed(&set(&["a"]), &present, "a"));
    assert!(is_dimmed(&set(&["a"]), &present, "b"));
    // Several members focus together.
    assert!(!is_dimmed(&set(&["a", "c"]), &present, "c"));
    assert!(is_dimmed(&set(&["a", "c"]), &present, "b"));
    // Others can be focused too.
    assert!(is_dimmed(&set(&[OTHERS_ID]), &["a", OTHERS_ID], "a"));
}

#[test]
fn a_focused_id_that_is_gone_is_ignored() {
    let present = ["a", "b"];
    // Only a missing member is focused: it is as if nothing were.
    assert!(!is_dimmed(&set(&["gone"]), &present, "a"));
    assert!(!is_dimmed(&set(&["gone"]), &present, "b"));
    // A missing one next to a present one changes nothing.
    assert!(is_dimmed(&set(&["gone", "a"]), &present, "b"));
}

#[test]
fn clicking_toggles_a_member_in_and_out_of_the_focus() {
    let one = toggled(&set(&[]), "a");
    assert_eq!(one, set(&["a"]));
    let two = toggled(&one, "b");
    assert_eq!(two, set(&["a", "b"]));
    assert_eq!(toggled(&two, "a"), set(&["b"]));
    assert_eq!(toggled(&set(&["a"]), "a"), set(&[]));
}

#[test]
fn bars_stack_while_lines_use_the_biggest_value() {
    let s = vec![series("a", &[2., 0.]), series("b", &[3., 1.])];
    let bars = y_scale(ChartKind::Bars, &s);
    // Stacked, the first bucket reaches 5 (the scale rounds up to a tick).
    assert!(bars.max >= 5. && bars.max < 10.);
    let line = y_scale(ChartKind::Line, &s);
    assert_eq!(line.max, 3.);
    // The running total of b reaches 4, a stays at 2.
    let running = y_scale(ChartKind::Cumulative, &s);
    assert_eq!(running.max, 4.);
    assert_eq!(bars.min, 0.);
}

#[test]
fn whole_counts_never_get_a_fractional_step() {
    let s = vec![series("a", &[2., 1.])];
    let scale = y_scale(ChartKind::Line, &s);
    assert_eq!((scale.min, scale.max, scale.step), (0., 2., 1.));
    // Nothing at all still has a range to sit in.
    let empty = y_scale(ChartKind::Bars, &[series("a", &[0., 0.])]);
    assert!(empty.max > empty.min);
    assert_eq!(empty.step, 1.);
    // Real numbers keep their nice step.
    let real = y_scale(ChartKind::Line, &[series("a", &[0.5, 1.5])]);
    assert!(real.max >= 1.5);
}

#[test]
fn a_pointer_maps_to_the_bucket_under_it() {
    assert_eq!(hit_bucket(ChartKind::Bars, 0., 100., 4), Some(0));
    assert_eq!(hit_bucket(ChartKind::Bars, 24.9, 100., 4), Some(0));
    assert_eq!(hit_bucket(ChartKind::Bars, 25., 100., 4), Some(1));
    assert_eq!(hit_bucket(ChartKind::Bars, 99.9, 100., 4), Some(3));
    assert_eq!(hit_bucket(ChartKind::Bars, 100., 100., 4), None);
    assert_eq!(hit_bucket(ChartKind::Bars, -1., 100., 4), None);
    assert_eq!(hit_bucket(ChartKind::Bars, 5., 100., 0), None);
    assert_eq!(hit_bucket(ChartKind::Bars, 5., 0., 4), None);
    assert_eq!(hit_bucket(ChartKind::Bars, 5., 100., 1), Some(0));
}

#[test]
fn the_tooltip_lists_the_visible_series_with_something_and_their_total() {
    let d = data(
        vec![
            series("a", &[2., 0.]),
            series("b", &[3., 0.]),
            series("c", &[0., 0.]),
        ],
        2,
    );
    let tip = tooltip(ChartKind::Bars, &d, &set(&[]), 0).unwrap();
    assert_eq!(tip.title, "Day 0");
    let rows: Vec<(&str, &str)> = tip
        .rows
        .iter()
        .map(|(_, n, v)| (n.as_str(), v.as_str()))
        .collect();
    assert_eq!(rows, [("A", "2"), ("B", "3"), ("Total", "5")]);
    // An empty bucket has a title and no rows.
    let empty = tooltip(ChartKind::Line, &d, &set(&[]), 1).unwrap();
    assert!(empty.rows.is_empty());
    assert!(tooltip(ChartKind::Line, &d, &set(&[]), 2).is_none());
}

#[test]
fn a_single_series_has_no_total_row_and_dimmed_series_are_left_out() {
    let d = data(vec![series("a", &[2.]), series("b", &[3.])], 1);
    let only_a = tooltip(ChartKind::Line, &d, &set(&["a"]), 0).unwrap();
    assert_eq!(only_a.rows.len(), 1);
    assert_eq!(only_a.rows[0].1, "A");
    assert_eq!(only_a.rows[0].2, "2");
}

#[test]
fn the_cumulative_tooltip_shows_running_totals() {
    let d = data(vec![series("a", &[2., 0., 3.])], 3);
    let tip = tooltip(ChartKind::Cumulative, &d, &set(&[]), 2).unwrap();
    assert_eq!(tip.title, "Day 2 - running total");
    assert_eq!(tip.rows[0].2, "5");
    // Nothing happened on day 1, but 2 had been reached.
    let still = tooltip(ChartKind::Cumulative, &d, &set(&[]), 1).unwrap();
    assert_eq!(still.rows[0].2, "2");
}

#[test]
fn others_follow_the_theme_in_the_tooltip() {
    let mut others = series(OTHERS_ID, &[1.]);
    others.color = content(0.99);
    let d = data(vec![others], 1);
    let tip = tooltip(ChartKind::Line, &d, &set(&[]), 0).unwrap();
    assert_eq!(tip.rows[0].0, others_color());
}

#[test]
fn the_axis_labels_the_first_and_last_bucket_and_few_between() {
    let b = buckets(15);
    let labels = axis_labels(&b);
    assert_eq!(labels[0], (0, "D0"));
    assert_eq!(labels[labels.len() - 1], (14, "D14"));
    assert!((2..=10).contains(&labels.len()));
    assert_eq!(axis_labels(&[]), []);
}

#[test]
fn the_summary_names_the_busiest_bucket_or_says_there_was_nothing() {
    let b = buckets(3);
    assert_eq!(
        summary("Commit activity", "day", "commits", &b, &[1., 4., 2.]),
        "Commit activity per day: 7 commits over 3 days, busiest day Day 1 with 4."
    );
    assert_eq!(
        summary("Commit activity", "day", "commits", &b, &[0., 0., 0.]),
        "Commit activity per day: no commits in 3 days."
    );
    assert_eq!(
        summary("Points", "week", "points", &[], &[]),
        "Points per week: no points in 0 weeks."
    );
}

#[test]
fn lines_run_edge_to_edge_while_bars_keep_their_slots() {
    use super::chart::{x_frac, y_labels};
    for kind in [ChartKind::Line, ChartKind::Cumulative] {
        assert_eq!(x_frac(kind, 0, 3), 0.);
        assert_eq!(x_frac(kind, 1, 3), 0.5);
        assert_eq!(x_frac(kind, 2, 3), 1.);
        // One bucket stays centred.
        assert_eq!(x_frac(kind, 0, 1), 0.5);
    }
    assert_eq!(x_frac(ChartKind::Bars, 0, 4), 0.125);
    assert_eq!(x_frac(ChartKind::Bars, 3, 4), 0.875);
    assert_eq!(x_frac(ChartKind::Bars, 0, 1), 0.5);
    // The pointer picks the nearest point of a line.
    let line = ChartKind::Line;
    assert_eq!(hit_bucket(line, 0., 100., 3), Some(0));
    assert_eq!(hit_bucket(line, 24., 100., 3), Some(0));
    assert_eq!(hit_bucket(line, 26., 100., 3), Some(1));
    assert_eq!(hit_bucket(line, 99., 100., 3), Some(2));
    assert_eq!(hit_bucket(line, 5., 100., 1), Some(0));
    // y labels: one per gridline, top first.
    let scale = crate::visuals::dither::Scale {
        min: 0.,
        max: 4.,
        step: 2.,
    };
    let labels = y_labels(&scale, 100.);
    assert_eq!(labels.len(), 3);
    assert_eq!(labels[0].1, "4");
    assert_eq!(labels[1].1, "2");
    assert_eq!(labels[2].1, "0");
    assert!(labels[0].0 < labels[1].0 && labels[1].0 < labels[2].0);
}
