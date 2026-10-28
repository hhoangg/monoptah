//! The pure maths of the graph canvas: fit view, zoom, and the orthogonal edge paths. The paths
//! follow xyflow's smooth-step routing for the two cases the workflow graph uses (a line down
//! from the bottom of one card to the top of the next, and the loop edge round the right side).
use super::geometry::*;
use monocode_core::mono::workflows::board_model::{Point, Size};

fn at(x: f32, y: f32) -> Point {
    Point { x, y }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

fn same(a: Point, b: Point) -> bool {
    close(a.x, b.x) && close(a.y, b.y)
}

// --- viewport ---------------------------------------------------------------------------------

#[test]
fn fit_view_centres_content_that_is_smaller_than_the_view_at_zoom_one() {
    let v = fit_view(
        Rect::new(0., 0., 100., 100.),
        Size {
            width: 200.,
            height: 400.,
        },
        FIT_PADDING,
        FIT_MAX_ZOOM,
    );
    assert!(close(v.zoom, 1.));
    assert!(same(v.to_screen(at(50., 50.)), at(100., 200.)));
}

#[test]
fn fit_view_shrinks_big_content_to_the_view_with_the_padding() {
    let v = fit_view(
        Rect::new(100., 50., 1000., 500.),
        Size {
            width: 500.,
            height: 500.,
        },
        FIT_PADDING,
        FIT_MAX_ZOOM,
    );
    // The width is the tight side: 500 / (1000 * 1.12).
    assert!(close(v.zoom, 500. / 1120.));
    assert!(same(v.to_screen(at(600., 300.)), at(250., 250.)));
}

#[test]
fn fit_view_never_goes_below_the_minimum_zoom_or_above_its_own_maximum() {
    let size = Size {
        width: 400.,
        height: 400.,
    };
    let huge = fit_view(
        Rect::new(0., 0., 10_000., 10_000.),
        size,
        FIT_PADDING,
        FIT_MAX_ZOOM,
    );
    assert!(close(huge.zoom, MIN_ZOOM));
    let tiny = fit_view(Rect::new(0., 0., 10., 10.), size, FIT_PADDING, 1.0);
    assert!(close(tiny.zoom, 1.));
    let capped = fit_view(Rect::new(0., 0., 10., 10.), size, FIT_PADDING, 1.5);
    assert!(close(capped.zoom, 1.5));
}

#[test]
fn a_canvas_without_room_is_not_fitted() {
    let content = Some(Rect::new(0., 0., 100., 100.));
    let none = Size {
        width: 0.,
        height: 300.,
    };
    assert_eq!(fit_scene(content, none), None);
    assert_eq!(
        fit_scene(
            content,
            Size {
                width: 300.,
                height: 0.
            }
        ),
        None
    );
    let room = Size {
        width: 300.,
        height: 300.,
    };
    assert!(fit_scene(content, room).is_some());
    // Nothing to show still fits, to the origin.
    assert_eq!(fit_scene(None, room), Some(Viewport::default()));
}

#[test]
fn zoom_keeps_the_point_under_the_pointer_where_it_is() {
    let v = Viewport {
        x: 30.,
        y: -20.,
        zoom: 0.8,
    };
    let anchor = at(120., 90.);
    let before = v.to_graph(anchor);
    let z = zoom_about(v, 1.3, anchor);
    assert!(close(z.zoom, 1.3));
    assert!(same(z.to_screen(before), anchor));
}

#[test]
fn zoom_stays_between_the_limits() {
    let v = Viewport::default();
    assert!(close(zoom_about(v, 9., at(0., 0.)).zoom, MAX_ZOOM));
    assert!(close(zoom_about(v, 0.01, at(0., 0.)).zoom, MIN_ZOOM));
    assert!(close(clamp_zoom(f32::NAN), 1.));
}

#[test]
fn the_buttons_zoom_by_a_fifth_each_press() {
    assert!(close(zoom_in(1.), 1.2));
    assert!(close(zoom_out(1.2), 1.));
    assert!(close(zoom_in(1.4), MAX_ZOOM));
    assert!(close(zoom_out(0.31), MIN_ZOOM));
}

#[test]
fn bounds_cover_every_rect_and_nothing_gives_none() {
    assert_eq!(bounds_of([]), None);
    let all = bounds_of([
        Rect::new(10., 20., 30., 40.),
        Rect::new(-5., 50., 10., 100.),
    ])
    .unwrap();
    assert_eq!(all, Rect::new(-5., 20., 45., 130.));
}

// --- edge paths -------------------------------------------------------------------------------

#[test]
fn a_line_straight_down_has_no_bends() {
    let path = step_points(at(100., 0.), Side::Bottom, at(100., 80.), Side::Top, 20.);
    assert!(path.points.windows(2).all(|w| close(w[0].x, w[1].x)));
    let segs = rounded(&path.points, 5.);
    assert_eq!(segs.first(), Some(&Seg::Move(at(100., 0.))));
    assert_eq!(segs.last(), Some(&Seg::Line(at(100., 80.))));
    assert!(segs.iter().all(|s| !matches!(s, Seg::Quad { .. })));
}

#[test]
fn a_line_to_a_card_down_and_to_the_right_bends_half_way_down() {
    let path = step_points(at(0., 0.), Side::Bottom, at(100., 100.), Side::Top, 20.);
    let corners: Vec<Point> = path.points.clone();
    assert_eq!(
        corners,
        vec![at(0., 0.), at(0., 50.), at(100., 50.), at(100., 100.)]
    );
    assert!(same(path.label, at(50., 50.)));
    let segs = rounded(&path.points, 5.);
    assert_eq!(segs[0], Seg::Move(at(0., 0.)));
    // Each corner is a quadratic curve through the corner point, five pixels each way.
    assert_eq!(segs[1], Seg::Line(at(0., 45.)));
    assert_eq!(
        segs[2],
        Seg::Quad {
            ctrl: at(0., 50.),
            to: at(5., 50.)
        }
    );
    assert_eq!(segs[3], Seg::Line(at(95., 50.)));
    assert_eq!(
        segs[4],
        Seg::Quad {
            ctrl: at(100., 50.),
            to: at(100., 55.)
        }
    );
    assert_eq!(segs[5], Seg::Line(at(100., 100.)));
}

#[test]
fn a_short_hop_rounds_no_more_than_half_of_the_segment() {
    // 4 pixels between the bends: the curve takes two each side, not the full radius.
    let points = vec![at(0., 0.), at(0., 4.), at(10., 4.), at(10., 8.)];
    let segs = rounded(&points, 12.);
    assert_eq!(segs[1], Seg::Line(at(0., 2.)));
    assert_eq!(
        segs[2],
        Seg::Quad {
            ctrl: at(0., 4.),
            to: at(2., 4.)
        }
    );
}

#[test]
fn a_line_to_a_card_above_goes_round_through_the_middle_column() {
    // A target above its source: down, across at the source's gap, up the middle, across, down.
    let path = step_points(at(0., 100.), Side::Bottom, at(100., 0.), Side::Top, 20.);
    assert_eq!(
        path.points,
        vec![
            at(0., 100.),
            at(0., 120.),
            at(50., 120.),
            at(50., -20.),
            at(100., -20.),
            at(100., 0.)
        ]
    );
}

#[test]
fn the_loop_edge_leaves_the_right_side_runs_up_and_comes_back_in() {
    // Out of the lower card's right side, into the upper card's right side, 26 pixels out.
    let path = step_points(
        at(232., 117.6),
        Side::Right,
        at(232., 14.4),
        Side::Right,
        26.,
    );
    assert_eq!(
        path.points,
        vec![
            at(232., 117.6),
            at(258., 117.6),
            at(258., 14.4),
            at(232., 14.4)
        ]
    );
    // The label belongs half way up the vertical run.
    assert!(close(path.line, 258.));
    assert!(close(path.label.y, (117.6 + 14.4) / 2.));
}

#[test]
fn the_loop_edge_runs_beside_the_farther_card_when_the_two_differ_in_width() {
    let path = step_points(at(100., 80.), Side::Right, at(232., 10.), Side::Right, 14.);
    assert!(close(path.line, 246.));
    assert_eq!(path.points[1], at(246., 80.));
    assert_eq!(path.points[2], at(246., 10.));
}

#[test]
fn the_loop_edge_rounds_its_two_corners_with_the_radius_it_is_given() {
    let path = step_points(at(232., 100.), Side::Right, at(232., 0.), Side::Right, 26.);
    let segs = rounded(&path.points, 12.);
    assert_eq!(segs[0], Seg::Move(at(232., 100.)));
    assert_eq!(segs[1], Seg::Line(at(246., 100.)));
    assert_eq!(
        segs[2],
        Seg::Quad {
            ctrl: at(258., 100.),
            to: at(258., 88.)
        }
    );
    assert_eq!(segs[3], Seg::Line(at(258., 12.)));
    assert_eq!(
        segs[4],
        Seg::Quad {
            ctrl: at(258., 0.),
            to: at(246., 0.)
        }
    );
    assert_eq!(segs[5], Seg::Line(at(232., 0.)));
}

#[test]
fn an_arrowhead_points_the_way_the_last_segment_goes() {
    let down = arrow_head(&[at(0., 0.), at(0., 50.)], 5., 3.5).unwrap();
    assert!(same(down[0], at(0., 50.)));
    assert!(same(down[1], at(-3.5, 45.)) || same(down[1], at(3.5, 45.)));
    assert!(close(down[1].y, 45.) && close(down[2].y, 45.));
    assert!(close(down[1].x, -down[2].x));
    let left = arrow_head(&[at(50., 10.), at(0., 10.)], 5., 3.5).unwrap();
    assert!(same(left[0], at(0., 10.)));
    assert!(close(left[1].x, 5.) && close(left[2].x, 5.));
    assert!(arrow_head(&[at(1., 1.)], 5., 3.5).is_none());
    assert!(arrow_head(&[at(1., 1.), at(1., 1.)], 5., 3.5).is_none());
}
