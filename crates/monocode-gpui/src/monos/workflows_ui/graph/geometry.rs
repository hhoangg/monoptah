//! The pure maths of the graph canvas: which part of the graph is in view (pan and zoom, fit),
//! and the orthogonal "smooth step" paths of the edges, routed like xyflow's `getSmoothStepPath`
//! for the cases the workflow graph uses. Nothing here touches GPUI, so it is tested directly.
use monocode_core::mono::workflows::board_model::{Point, Size};

pub(in crate::monos) const MIN_ZOOM: f32 = 0.3;
pub(in crate::monos) const MAX_ZOOM: f32 = 1.5;
/// `fitView({ padding: 0.12, maxZoom: 1 })`.
pub(in crate::monos) const FIT_PADDING: f32 = 0.12;
pub(in crate::monos) const FIT_MAX_ZOOM: f32 = 1.0;
/// One press of a zoom button.
const ZOOM_STEP: f32 = 1.2;

/// A rectangle in graph space (or on screen).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::monos) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub(in crate::monos) fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub(in crate::monos) fn right(&self) -> f32 {
        self.x + self.w
    }

    pub(in crate::monos) fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub(in crate::monos) fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }
}

/// The smallest rectangle holding all of `rects`; None for none.
pub(in crate::monos) fn bounds_of(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    rects.into_iter().reduce(Rect::union)
}

/// Which part of the graph is in view: a graph point `p` sits at `p * zoom + (x, y)` on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::monos) struct Viewport {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            zoom: 1.,
        }
    }
}

impl Viewport {
    pub(in crate::monos) fn to_screen(self, p: Point) -> Point {
        Point {
            x: p.x * self.zoom + self.x,
            y: p.y * self.zoom + self.y,
        }
    }

    pub(in crate::monos) fn to_graph(self, p: Point) -> Point {
        Point {
            x: (p.x - self.x) / self.zoom,
            y: (p.y - self.y) / self.zoom,
        }
    }
}

pub(in crate::monos) fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_nan() {
        return 1.;
    }
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The view that shows all of `content` in a view of `size`, `padding` (a share of the content)
/// around it, never zoomed in past `max_zoom` nor out past the minimum zoom.
pub(in crate::monos) fn fit_view(
    content: Rect,
    size: Size,
    padding: f32,
    max_zoom: f32,
) -> Viewport {
    let across = |view: f32, content: f32| {
        if content > 0. {
            view / (content * (1. + padding))
        } else {
            f32::INFINITY
        }
    };
    let zoom = across(size.width, content.w)
        .min(across(size.height, content.h))
        .clamp(MIN_ZOOM, max_zoom);
    let zoom = if zoom.is_finite() { zoom } else { max_zoom };
    Viewport {
        x: size.width / 2. - (content.x + content.w / 2.) * zoom,
        y: size.height / 2. - (content.y + content.h / 2.) * zoom,
        zoom,
    }
}

/// The view that shows a scene: None while there is no room to show it in (a canvas that has not
/// been laid out has no size), so the view is not fitted to nothing.
pub(in crate::monos) fn fit_scene(content: Option<Rect>, size: Size) -> Option<Viewport> {
    if !(size.width > 0. && size.height > 0.) {
        return None;
    }
    Some(match content {
        Some(content) => fit_view(content, size, FIT_PADDING, FIT_MAX_ZOOM),
        None => Viewport::default(),
    })
}

/// `v` zoomed to `zoom` (kept within the limits) with the graph point under `anchor` staying
/// under it.
pub(in crate::monos) fn zoom_about(v: Viewport, zoom: f32, anchor: Point) -> Viewport {
    let zoom = clamp_zoom(zoom);
    let under = v.to_graph(anchor);
    Viewport {
        x: anchor.x - under.x * zoom,
        y: anchor.y - under.y * zoom,
        zoom,
    }
}

pub(in crate::monos) fn zoom_in(zoom: f32) -> f32 {
    clamp_zoom(zoom * ZOOM_STEP)
}

pub(in crate::monos) fn zoom_out(zoom: f32) -> f32 {
    clamp_zoom(zoom / ZOOM_STEP)
}

// --- edges ------------------------------------------------------------------------------------

/// The side of a card a line leaves or enters by (the left side is never used).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::monos) enum Side {
    Right,
    Top,
    Bottom,
}

impl Side {
    fn dir(self) -> (f32, f32) {
        match self {
            Side::Right => (1., 0.),
            Side::Top => (0., -1.),
            Side::Bottom => (0., 1.),
        }
    }

    fn horizontal(self) -> bool {
        matches!(self, Side::Right)
    }
}

/// The corners of an edge, source to target, and where its label belongs.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct StepPath {
    pub points: Vec<Point>,
    /// Half way between the two ends (xyflow's label position).
    pub label: Point,
    /// Where the long run is: its x for a route out and back in by the Right, its y for one that
    /// steps in the middle or runs beside the top or bottom.
    pub line: f32,
}

fn same(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn offset_from(p: Point, side: Side, by: f32) -> Point {
    let (dx, dy) = side.dir();
    Point {
        x: p.x + dx * by,
        y: p.y + dy * by,
    }
}

/// Routes an edge at right angles from `source` (leaving by `from`) to `target` (entered by
/// `to`), `offset` pixels clear of each card before the first turn.
pub(in crate::monos) fn step_points(
    source: Point,
    from: Side,
    target: Point,
    to: Side,
    offset: f32,
) -> StepPath {
    let label = Point {
        x: (source.x + target.x) / 2.,
        y: (source.y + target.y) / 2.,
    };
    let mut line = label.x;
    let sg = offset_from(source, from, offset);
    let tg = offset_from(target, to, offset);
    let mut points = vec![source, sg];
    if from == to {
        // Out and back in on the same side: one long run beside the cards, clear of the farther.
        let (corner_s, corner_t) = match from {
            Side::Right => {
                let x = source.x.max(target.x) + offset;
                line = x;
                (Point { x, y: source.y }, Point { x, y: target.y })
            }
            Side::Bottom | Side::Top => {
                let y = if from == Side::Bottom {
                    source.y.max(target.y) + offset
                } else {
                    source.y.min(target.y) - offset
                };
                line = y;
                (Point { x: source.x, y }, Point { x: target.x, y })
            }
        };
        points.push(corner_s);
        points.push(corner_t);
    } else if from.horizontal() == to.horizontal() {
        // Opposite sides, top to bottom or the other way: a step in the middle.
        let heading = if sg.y < tg.y { 1. } else { -1. };
        let centre_x = (sg.x + tg.x) / 2.;
        let centre_y = (sg.y + tg.y) / 2.;
        let split = if from.dir().1 == heading {
            // On the way: across half way down.
            [
                Point {
                    x: sg.x,
                    y: centre_y,
                },
                Point {
                    x: tg.x,
                    y: centre_y,
                },
            ]
        } else {
            // Against the way: round through the middle column.
            [
                Point {
                    x: centre_x,
                    y: sg.y,
                },
                Point {
                    x: centre_x,
                    y: tg.y,
                },
            ]
        };
        line = centre_y;
        points.extend(split);
    } else {
        // Sideways out, up or down in (or the other way round): one corner.
        points.push(if from.horizontal() {
            Point { x: tg.x, y: sg.y }
        } else {
            Point { x: sg.x, y: tg.y }
        });
    }
    points.push(tg);
    points.push(target);
    StepPath {
        points: tidy(points),
        label,
        line,
    }
}

/// Drops repeated points and the ones in the middle of a straight run.
fn tidy(points: Vec<Point>) -> Vec<Point> {
    let mut distinct: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if distinct
            .last()
            .is_none_or(|last| !(same(last.x, p.x) && same(last.y, p.y)))
        {
            distinct.push(p);
        }
    }
    let mut out: Vec<Point> = Vec::with_capacity(distinct.len());
    for (i, p) in distinct.iter().enumerate() {
        let straight = i > 0 && i + 1 < distinct.len() && {
            let (a, c) = (out[out.len() - 1], distinct[i + 1]);
            (same(a.x, p.x) && same(p.x, c.x)) || (same(a.y, p.y) && same(p.y, c.y))
        };
        if !straight {
            out.push(*p);
        }
    }
    out
}

/// One piece of a drawn path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::monos) enum Seg {
    Move(Point),
    Line(Point),
    /// A quadratic curve through `ctrl` (the corner it rounds) ending at `to`.
    Quad {
        ctrl: Point,
        to: Point,
    },
}

fn distance(a: Point, b: Point) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

/// The path through `points` with every corner rounded by up to `radius` (xyflow's `getBend`:
/// never more than half of either segment).
pub(in crate::monos) fn rounded(points: &[Point], radius: f32) -> Vec<Seg> {
    let mut out = Vec::with_capacity(points.len() * 2);
    for (i, &b) in points.iter().enumerate() {
        if i == 0 {
            out.push(Seg::Move(b));
        } else if i + 1 == points.len() {
            out.push(Seg::Line(b));
        } else {
            let (a, c) = (points[i - 1], points[i + 1]);
            let size = (distance(a, b) / 2.).min(distance(b, c) / 2.).min(radius);
            if (same(a.x, b.x) && same(b.x, c.x)) || (same(a.y, b.y) && same(b.y, c.y)) {
                out.push(Seg::Line(b));
            } else if same(a.y, b.y) {
                // The first segment is horizontal.
                let x_dir = if a.x < c.x { -1. } else { 1. };
                let y_dir = if a.y < c.y { 1. } else { -1. };
                out.push(Seg::Line(Point {
                    x: b.x + size * x_dir,
                    y: b.y,
                }));
                out.push(Seg::Quad {
                    ctrl: b,
                    to: Point {
                        x: b.x,
                        y: b.y + size * y_dir,
                    },
                });
            } else {
                let x_dir = if a.x < c.x { 1. } else { -1. };
                let y_dir = if a.y < c.y { -1. } else { 1. };
                out.push(Seg::Line(Point {
                    x: b.x,
                    y: b.y + size * y_dir,
                }));
                out.push(Seg::Quad {
                    ctrl: b,
                    to: Point {
                        x: b.x + size * x_dir,
                        y: b.y,
                    },
                });
            }
        }
    }
    out
}

/// The triangle of an arrowhead at the end of the path: its tip, then its two back corners.
/// None when the path has no direction yet.
pub(in crate::monos) fn arrow_head(points: &[Point], length: f32, half: f32) -> Option<[Point; 3]> {
    let tip = *points.last()?;
    let from = points
        .iter()
        .rev()
        .find(|p| !(same(p.x, tip.x) && same(p.y, tip.y)))?;
    let span = distance(*from, tip);
    let (dx, dy) = ((tip.x - from.x) / span, (tip.y - from.y) / span);
    let base = Point {
        x: tip.x - dx * length,
        y: tip.y - dy * length,
    };
    Some([
        tip,
        Point {
            x: base.x - dy * half,
            y: base.y + dx * half,
        },
        Point {
            x: base.x + dy * half,
            y: base.y - dx * half,
        },
    ])
}
