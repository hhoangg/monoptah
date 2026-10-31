//! Charts drawn as dithered cells: bar, area, line, stat, gauge, donut,
//! heatmap and progress.
use super::{
    color,
    dither::{
        Align, CELL, Curve, Grid, Label, Motion, Scale, compact, crosshair, glow, glowing_line,
        hoverable, inked, marker, pointer, readout, swatch,
    },
};
use crate::theme::content;
use gpui::{
    AnyElement, App, Bounds, FontWeight, Hsla, IntoElement, Pixels, SharedString, Window, canvas,
    div, point, prelude::*, px,
};
use monocode_core::visual::{ChartKind, Series, Slice, ThemeColor};

/// Axis type: the shared mono caption.
fn axis() -> Label {
    Label::mono(10., content(0.45))
}

fn faint(color: Hsla, alpha: f32) -> Hsla {
    let mut c = color;
    c.a = alpha;
    c
}

/// Bars and areas fade from their top edge toward the axis.
const BAR_LEVEL: f32 = 0.75;
const BAR_FLOOR: f32 = 0.125;
const BAR_FADE: f32 = 0.7;
const MAX_BAR: f32 = 36.;
const AREA_LEVEL: f32 = 0.625;
/// Area coverage by depth below the line, as a share of the plot height.
const AREA_BANDS: [(f32, f32); 5] = [
    (0.05, 1.),
    (0.13, 0.8),
    (0.25, 0.65),
    (0.4, 0.5),
    (0.6, 0.35),
];
const AREA_REST: f32 = 0.2;

/// A value for a readout: compact, to two decimals at most.
fn reading(value: f64) -> String {
    compact(value, 0.01)
}

/// Clips a curve's points to those left of `x`, ending exactly there.
fn drawn_to(points: &[[f32; 2]], curve: &Curve, x: f32) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = points.iter().copied().filter(|p| p[0] <= x).collect();
    if x > curve.first_x() && x < curve.last_x() {
        out.push([x, curve.at(x)]);
    }
    out
}

/// A series with its colour resolved, ready to draw.
pub(crate) struct Drawn {
    pub label: String,
    pub color: Hsla,
    pub values: Vec<f64>,
    /// Faded and drawn behind the others.
    pub dimmed: bool,
}

/// Dimmed series keep this much of their colour.
pub(crate) const DIM: f32 = 0.2;

/// One readout row: swatch, name, value.
pub(crate) type Row = (Hsla, String, String);
/// The readout of a bucket: its title and rows. `None` shows no readout.
pub(crate) type Tip<'a> = &'a dyn Fn(usize) -> Option<(String, Vec<Row>)>;

/// Which bucket is hovered.
#[derive(Clone, Copy)]
pub(crate) enum Hover {
    /// Whatever bucket the window's pointer is over.
    Pointer,
    /// What an owner that tracks the pointer itself says.
    At(Option<usize>),
}

/// Everything `cartesian` draws.
pub(crate) struct Plot<'a> {
    pub kind: ChartKind,
    pub series: &'a [Drawn],
    pub labels: &'a [String],
    /// Built by the caller (zero-based or not) so the hit test can share it.
    pub scale: Scale,
    pub hover: Hover,
    /// Replaces the default readout (label, then one row per visible series).
    pub tip: Option<Tip<'a>>,
    /// A line resting on the zero baseline is drawn thin, without glow or
    /// marker, so many idle series do not pile into a thick band.
    pub quiet_zero: bool,
}

fn effective(d: &Drawn) -> Hsla {
    if d.dimmed {
        faint(d.color, d.color.a * DIM)
    } else {
        d.color
    }
}

/// Where the plot sits inside a chart canvas: the one source for paint and hit test.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geometry {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}
impl Geometry {
    /// The cell grid of the plot for a canvas at `origin`.
    pub fn grid(&self, origin: gpui::Point<Pixels>) -> Grid {
        Grid::new(
            origin + point(px(self.left), px(self.top)),
            self.width,
            self.height,
        )
    }
}

/// The plot of a `width` x `height` canvas: room left for the y labels, above
/// for a bar's value, below for the x labels.
pub(crate) fn geometry(kind: ChartKind, scale: &Scale, width: f32, height: f32) -> Geometry {
    let bars = matches!(kind, ChartKind::Bar);
    let longest = scale
        .ticks()
        .iter()
        .map(|t| scale.label(*t).len())
        .max()
        .unwrap_or(1) as f32;
    let left = (longest * 6.1).ceil() + 10.;
    let top = if bars { 16. } else { 8. };
    let right = width - if bars { 0. } else { 6. };
    Geometry {
        left,
        top,
        width: right - left,
        height: height - top - 22.,
    }
}

/// Where bucket `i` of `count` sits across a `plot_w` wide plot: its slot's
/// middle for bars, evenly from edge to edge for lines (one bucket is centred).
pub(crate) fn x_at(kind: ChartKind, i: usize, count: usize, plot_w: f32) -> f32 {
    if matches!(kind, ChartKind::Bar) {
        plot_w / count.max(1) as f32 * (i as f32 + 0.5)
    } else if count <= 1 {
        plot_w / 2.
    } else {
        i as f32 * plot_w / (count - 1) as f32
    }
}

/// The bucket under `x` (px from the plot's left edge): the band under it for
/// bars, the nearest point for lines.
pub(crate) fn bucket_at(kind: ChartKind, x: f32, plot_w: f32, count: usize) -> Option<usize> {
    if count == 0 || plot_w <= 0. || !(0. ..=plot_w).contains(&x) {
        return None;
    }
    if count == 1 {
        return Some(0);
    }
    let at = if matches!(kind, ChartKind::Bar) {
        (x / (plot_w / count as f32)) as usize
    } else {
        ((x / plot_w) * (count - 1) as f32).round() as usize
    };
    Some(at.min(count - 1))
}

/// Every how-many-th x label is written, so labels are at least 72px apart.
pub(crate) fn label_stride(count: usize, plot_w: f32) -> usize {
    (count as f32 / (plot_w / 72.).floor().max(1.))
        .ceil()
        .max(1.) as usize
}

pub(super) fn chart(
    kind: ChartKind,
    series: Vec<Series>,
    labels: Vec<String>,
    height: f32,
    motion: Motion,
    id: SharedString,
) -> AnyElement {
    let legend = (series.len() > 1).then(|| {
        div()
            .flex()
            .flex_wrap()
            .gap_x_4()
            .gap_y_1()
            .children(series.iter().map(|s| {
                legend_style()
                    .child(swatch(color(&s.color)))
                    .child(s.label.clone())
            }))
    });
    let drawn: Vec<Drawn> = series
        .into_iter()
        .map(|s| Drawn {
            color: color(&s.color),
            label: s.label,
            values: s.values,
            dimmed: false,
        })
        .collect();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            hoverable(id).w_full().h(px(height)).child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        let scale = Scale::nice(
                            drawn.iter().flat_map(|s| s.values.iter().copied()),
                            !matches!(kind, ChartKind::Line),
                        );
                        let plot = Plot {
                            kind,
                            series: &drawn,
                            labels: &labels,
                            scale,
                            hover: Hover::Pointer,
                            tip: None,
                            quiet_zero: false,
                        };
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            cartesian(&plot, bounds, motion, window, cx)
                        })
                    },
                )
                .size_full(),
            ),
        )
        .children(legend)
        .into_any_element()
}

/// A legend entry's frame and type: a row with the mono caption.
pub(crate) fn legend_style() -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(11.))
        .font_family(crate::fonts::mono())
        .text_color(content(0.6))
}

/// The default readout rows of bucket `i`: the series that are not dimmed.
fn default_tip(plot: &Plot, i: usize) -> (String, Vec<Row>) {
    let title = plot
        .labels
        .get(i)
        .cloned()
        .unwrap_or_else(|| format!("#{}", i + 1));
    let rows = plot
        .series
        .iter()
        .filter(|s| !s.dimmed)
        .filter_map(|s| {
            let v = *s.values.get(i)?;
            Some((s.color, s.label.clone(), reading(v)))
        })
        .collect();
    (title, rows)
}

pub(crate) fn cartesian(
    plot: &Plot,
    bounds: Bounds<Pixels>,
    motion: Motion,
    window: &mut Window,
    cx: &mut App,
) {
    let (kind, series, labels, scale) = (plot.kind, plot.series, plot.labels, &plot.scale);
    let bars = matches!(kind, ChartKind::Bar);
    let count = series
        .iter()
        .map(|s| s.values.len())
        .max()
        .unwrap_or(0)
        .max(labels.len());
    if count == 0 {
        return;
    }
    let ticks = scale.ticks();
    let geo = geometry(
        kind,
        scale,
        bounds.size.width.as_f32(),
        bounds.size.height.as_f32(),
    );
    let grid = geo.grid(bounds.origin);
    let plot_w = grid.cols as f32 * CELL;
    let plot_h = grid.rows as f32 * CELL;
    let origin = grid.origin;
    let y_of = |v: f64| scale.depth(v) * plot_h;
    let band = plot_w / count as f32;
    let x_of = |i: usize| x_at(kind, i, count, plot_w);

    // Grid lines and ticks.
    for tick in &ticks {
        let y = y_of(*tick).round();
        window.paint_quad(gpui::fill(
            Bounds::new(
                origin + point(px(0.), px(y)),
                gpui::size(px(plot_w), px(1.)),
            ),
            content(0.07),
        ));
        axis().paint(
            &scale.label(*tick),
            origin + point(px(-10.), px(y)),
            Align::Right,
            window,
            cx,
        );
    }
    let stride = label_stride(count, plot_w);
    for (i, label) in labels.iter().enumerate().step_by(stride) {
        let align = if bars {
            Align::Center
        } else if i == 0 {
            Align::Left
        } else if i + 1 == count {
            Align::Right
        } else {
            Align::Center
        };
        let label = if label.chars().count() > 12 {
            format!("{}…", label.chars().take(11).collect::<String>())
        } else {
            label.clone()
        };
        axis().paint(
            &label,
            origin + point(px(x_of(i)), px(plot_h + 13.)),
            align,
            window,
            cx,
        );
    }

    let hovered = match plot.hover {
        Hover::At(at) => at.filter(|i| *i < count),
        Hover::Pointer => pointer(bounds, window).and_then(|(x, _)| {
            bucket_at(
                kind,
                x - (origin.x - bounds.origin.x).as_f32(),
                plot_w,
                count,
            )
        }),
    };
    let readout_at = |i: usize, anchor: gpui::Point<Pixels>, window: &mut Window, cx: &mut App| {
        let tip = match plot.tip {
            Some(tip) => tip(i),
            None => Some(default_tip(plot, i)),
        };
        if let Some((title, rows)) = tip {
            readout(&title, &rows, anchor, bounds, window, cx);
        }
    };
    if bars {
        if let Some(i) = hovered {
            // A soft column behind the hovered category.
            window.paint_quad(gpui::fill(
                Bounds::new(
                    origin + point(px(band * i as f32), px(0.)),
                    gpui::size(px(band), px(plot_h)),
                ),
                content(0.04),
            ));
        }
        bar_marks(plot, count, &grid, band, hovered, motion, window, cx);
        if let Some(i) = hovered {
            let peak = series
                .iter()
                .filter(|s| !s.dimmed)
                .filter_map(|s| s.values.get(i))
                .copied()
                .fold(f64::MIN, f64::max);
            let anchor = origin
                + point(
                    px(band * (i as f32 + 0.5)),
                    px(y_of(peak).min(plot_h - 20.)),
                );
            readout_at(i, anchor, window, cx);
        }
        return;
    }
    let points: Vec<Vec<[f32; 2]>> = series
        .iter()
        .map(|s| {
            s.values
                .iter()
                .enumerate()
                .map(|(i, v)| [x_of(i), y_of(*v)])
                .collect()
        })
        .collect();
    let curves: Vec<Curve> = points.iter().map(|p| Curve::new(p)).collect();
    // Lines draw in from the left; areas fill in behind them.
    let reach = motion.at(0., 900., window, cx) * plot_w;
    if matches!(kind, ChartKind::Area) {
        let baseline = y_of(scale.min.max(0.));
        let colors: Vec<Hsla> = series.iter().map(effective).collect();
        // A wash of each colour under its area stands in for bloom.
        for (curve, color) in curves.iter().zip(&colors) {
            let mut area: Vec<[f32; 2]> = curve
                .samples(4.)
                .into_iter()
                .filter(|p| p[0] <= reach)
                .collect();
            if let (Some(first), Some(last)) = (area.first().copied(), area.last().copied()) {
                area.push([last[0], baseline]);
                area.push([first[0], baseline]);
                let wash = faint(*color, if super::dither::light() { 0.04 } else { 0.07 });
                super::paint_path(&area, origin, 1., 0., true, wash, window);
            }
        }
        grid.paint(window, |col, row| {
            let (x, y) = Grid::center(col, row);
            if y > baseline || x > reach {
                return None;
            }
            // Later series sit in front, as stacked paper would.
            let (index, curve) = curves
                .iter()
                .enumerate()
                .rev()
                .find(|(_, c)| x >= c.first_x() && x <= c.last_x() && y >= c.at(x))?;
            let depth = (y - curve.at(x)) / plot_h.max(1.);
            let lead = (2. * CELL / plot_h.max(1.)).max(AREA_BANDS[0].0);
            let share = if depth < lead {
                1.
            } else {
                AREA_BANDS
                    .iter()
                    .find(|(d, _)| depth < *d)
                    .map_or(AREA_REST, |(_, s)| *s)
            };
            let tone = (AREA_LEVEL * share * 16.).round() / 16.;
            inked(col, row, tone).then_some(colors[index])
        });
    }
    if let Some(i) = hovered {
        crosshair(origin, x_of(i), plot_h, window);
    }
    // Dimmed series first, so the lit ones draw over them.
    let order = (0..series.len())
        .filter(|s| series[*s].dimmed)
        .chain((0..series.len()).filter(|s| !series[*s].dimmed));
    for s in order {
        let (d, curve, points) = (&series[s], &curves[s], &points[s]);
        let samples = drawn_to(&curve.samples(2.), curve, reach);
        if d.dimmed {
            // No glow: a faded line must not light up.
            super::paint_path(&samples, origin, 1., 2., false, effective(d), window);
            // One point makes no stroke: a faded square stands in.
            if let [only] = samples.as_slice() {
                let side = 2. * CELL;
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        origin + point(px(only[0] - side / 2.), px(only[1] - side / 2.)),
                        gpui::size(px(side), px(side)),
                    ),
                    effective(d),
                ));
            }
            continue;
        }
        let base = y_of(0.);
        let resting = |y: f32| plot.quiet_zero && y >= base - 0.5;
        if plot.quiet_zero {
            for (flat, run) in runs(&samples, resting) {
                if flat {
                    super::paint_path(&run, origin, 1., 2., false, d.color, window);
                } else {
                    glowing_line(&run, origin, 2., d.color, window);
                }
            }
        } else {
            glowing_line(&samples, origin, 2., d.color, window);
        }
        // The end marker rides the head of the line as it draws in.
        let head = if reach >= curve.last_x() {
            points.last().copied()
        } else {
            samples.last().copied()
        };
        if hovered.is_none()
            && let Some(head) = head.filter(|h| !resting(h[1]))
        {
            marker(
                window,
                origin + point(px(head[0]), px(head[1])),
                2. * CELL,
                d.color,
            );
        }
        if let Some(p) = hovered
            .and_then(|i| points.get(i))
            .filter(|p| !resting(p[1]))
        {
            marker(
                window,
                origin + point(px(p[0]), px(p[1])),
                2. * CELL,
                d.color,
            );
        }
    }
    if let Some(i) = hovered {
        let top = series
            .iter()
            .zip(&points)
            .filter(|(s, _)| !s.dimmed)
            .filter_map(|(_, p)| p.get(i))
            .map(|p| p[1])
            .fold(plot_h, f32::min);
        readout_at(i, origin + point(px(x_of(i)), px(top)), window, cx);
    }
}

/// Splits `samples` into consecutive runs of resting (`true`) and moving
/// points. Neighbouring runs share their joint so the line stays whole.
pub(super) fn runs(
    samples: &[[f32; 2]],
    resting: impl Fn(f32) -> bool,
) -> Vec<(bool, Vec<[f32; 2]>)> {
    let mut out: Vec<(bool, Vec<[f32; 2]>)> = vec![];
    for p in samples {
        let flat = resting(p[1]);
        match out.last_mut() {
            Some((kind, run)) if *kind == flat => run.push(*p),
            _ => {
                if let Some((_, previous)) = out.last_mut() {
                    previous.push(*p);
                }
                out.push((flat, vec![*p]));
            }
        }
    }
    out
}

/// How a bucket's group of bars sits in its band, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BarLayout {
    /// A bar's width.
    pub wide: i32,
    /// Empty cells between the bars of a group.
    pub gap: i32,
    /// How many of the series get a bar.
    pub shown: usize,
}
impl BarLayout {
    pub fn group(&self) -> i32 {
        let shown = self.shown as i32;
        self.wide * shown + self.gap * (shown - 1).max(0)
    }
}

/// The layout of `n` side-by-side bars in a `band` px wide band. Bars take 70%
/// of the band with a one cell gap, as long as that group fits. A group never
/// reaches into its neighbours: when it does not fit, the bars narrow first,
/// then lose the gap, and when even 1 cell bars would not fit, only the first
/// `shown` series get a bar (the rest are in the readout only). Fallbacks also
/// keep one cell clear between groups when the band allows.
pub(crate) fn bar_layout(band: f32, n: usize) -> BarLayout {
    let n = n.max(1);
    let count = n as i32;
    let width_px = ((band * 0.7 - CELL * (n - 1) as f32) / n as f32).min(MAX_BAR);
    let wide = (width_px / CELL).round().max(1.) as i32;
    // A group may take the whole band: starts are rounded, so neighbours
    // never get closer than the whole cells of the band.
    let room = ((band / CELL).floor() as i32).max(1);
    if wide * count + count - 1 <= room {
        return BarLayout {
            wide,
            gap: 1,
            shown: n,
        };
    }
    let room = if room > 1 { room - 1 } else { room };
    let wide = (room - (count - 1)) / count;
    if wide >= 1 {
        return BarLayout {
            wide,
            gap: 1,
            shown: n,
        };
    }
    let wide = room / count;
    if wide >= 1 {
        return BarLayout {
            wide,
            gap: 0,
            shown: n,
        };
    }
    BarLayout {
        wide: 1,
        gap: 0,
        shown: room as usize,
    }
}

/// The series that get a bar when only `shown` fit: the lit ones first, in
/// order, then the dimmed. Returned in series order.
pub(super) fn bar_series(series: &[Drawn], shown: usize) -> Vec<usize> {
    let mut picked: Vec<usize> = (0..series.len())
        .filter(|s| !series[*s].dimmed)
        .chain((0..series.len()).filter(|s| series[*s].dimmed))
        .take(shown)
        .collect();
    picked.sort_unstable();
    picked
}

#[allow(clippy::too_many_arguments)]
fn bar_marks(
    plot: &Plot,
    count: usize,
    grid: &Grid,
    band: f32,
    hovered: Option<usize>,
    motion: Motion,
    window: &mut Window,
    cx: &mut App,
) {
    let (series, scale) = (plot.series, &plot.scale);
    let plot_h = grid.rows as f32 * CELL;
    let layout = bar_layout(band, series.len());
    let (wide, gap) = (layout.wide, layout.gap);
    let group = layout.group();
    let picked = bar_series(series, layout.shown);
    let row_of = |v: f64| ((scale.depth(v) * plot_h) / CELL).round() as i32;
    let zero = row_of(scale.min.max(0.).min(scale.max));
    let last = count - 1;
    let fade = (grid.rows as f32 * BAR_FADE).max(1.);
    // Each column's bar, if any: (top row, bottom row, series, category).
    let mut columns: Vec<Option<(i32, i32, usize, usize)>> = vec![None; grid.cols.max(0) as usize];
    let mut last_top = None;
    let mut settled = true;
    for i in 0..count {
        // Each group rises from the axis, a beat after the one before.
        let rise = motion.at((i as f32 * 30.).min(360.), 700., window, cx);
        settled &= rise >= 1.;
        let start = ((band * (i as f32 + 0.5)) / CELL - group as f32 / 2.).round() as i32;
        for (slot, j) in picked.iter().enumerate() {
            let Some(value) = series[*j].values.get(i) else {
                continue;
            };
            let end = zero + ((row_of(*value) - zero) as f32 * rise).round() as i32;
            let (top, bottom) = (end.min(zero), end.max(zero));
            let first = start + slot as i32 * (wide + gap);
            for col in first..first + wide {
                if let Some(cell) = usize::try_from(col).ok().and_then(|c| columns.get_mut(c)) {
                    *cell = Some((top, bottom, *j, i));
                }
            }
            if i == last {
                last_top = Some(top);
            }
        }
    }
    let colors: Vec<Hsla> = series.iter().map(effective).collect();
    grid.paint(window, |col, row| {
        let (top, bottom, j, i) = columns.get(col as usize).copied().flatten()?;
        if row < top || row >= bottom {
            return None;
        }
        let depth = (row - top) as f32;
        let mut tone = BAR_LEVEL - (BAR_LEVEL - BAR_FLOOR) * (depth / fade).min(1.);
        // The hovered group lights up: a denser dither, never solid.
        if hovered == Some(i) {
            tone = 0.5 + tone * 0.5;
        }
        (row == top || inked(col, row, tone)).then_some(colors[j])
    });
    // With one series, the latest value is written over its bar once it has
    // risen. A group's value would be ambiguous.
    if let ([s], Some(top)) = (series, last_top)
        && settled
        && hovered.is_none()
        && !s.dimmed
        && let Some(value) = s.values.get(last)
    {
        let text = compact(*value, scale.step / 10.);
        Label::mono(10., content(0.9)).paint(
            &text,
            grid.origin + point(px(band * (last as f32 + 0.5)), px(top as f32 * CELL - 9.)),
            Align::Center,
            window,
            cx,
        );
    }
}

/// The sparkline under a stat: a line over bands that step lighter downward.
pub(super) fn trend(values: Vec<f64>, tint: Hsla, motion: Motion) -> AnyElement {
    const HEIGHT: f32 = 56.;
    const LEVELS: [f32; 4] = [10. / 16., 6. / 16., 3. / 16., 1. / 16.];
    const DEPTH: f32 = 7.;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            if values.len() < 2 {
                return;
            }
            let grid = Grid::new(bounds.origin, bounds.size.width.as_f32(), HEIGHT);
            let width = grid.cols as f32 * CELL;
            let height = grid.rows as f32 * CELL;
            let low = values.iter().copied().fold(f64::MAX, f64::min);
            let high = values.iter().copied().fold(f64::MIN, f64::max);
            let range = if high > low { high - low } else { 1. };
            let (top, floor) = (6., height * 0.3);
            let points: Vec<[f32; 2]> = values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    [
                        i as f32 / (values.len() - 1) as f32 * width,
                        top + (1. - ((v - low) / range) as f32) * (height - top - floor),
                    ]
                })
                .collect();
            let curve = Curve::new(&points);
            let reach = motion.at(150., 900., window, cx) * width;
            grid.paint(window, |col, row| {
                let (x, y) = Grid::center(col, row);
                let depth = y - curve.at(x);
                if depth < 0. || x > reach {
                    return None;
                }
                let tone = LEVELS[((depth / DEPTH) as usize).min(LEVELS.len() - 1)];
                inked(col, row, tone).then_some(tint)
            });
            let samples = drawn_to(&curve.samples(2.), &curve, reach);
            glowing_line(&samples, grid.origin, 1.5, tint, window);
            if let Some(end) = samples.last() {
                marker(
                    window,
                    grid.origin + point(px(end[0].min(width - CELL)), px(end[1])),
                    2. * CELL,
                    tint,
                );
            }
        },
    )
    .w_full()
    .h(px(HEIGHT))
    .into_any_element()
}

/// A signed change drawn with an arrow in the good or bad colour.
pub(super) fn delta(text: &str, lower_is_better: bool) -> AnyElement {
    let down = text.starts_with('-') || text.starts_with('−');
    let tone = color(if down != lower_is_better {
        &ThemeColor::Negative
    } else {
        &ThemeColor::Positive
    });
    div()
        .flex()
        .items_center()
        .gap_1()
        .flex_shrink_0()
        .text_size(px(12.))
        .font_family(crate::fonts::mono())
        .text_color(tone)
        .child(if down { "▼" } else { "▲" })
        .child(text.to_owned())
        .into_any_element()
}

/// Splits a leading signed change off a stat's detail: "+12.4% vs last
/// month" becomes ("+12.4%", "vs last month").
pub(super) fn split_delta(detail: &str) -> Option<(&str, &str)> {
    let mut chars = detail.chars();
    let sign = chars.next()?;
    if !matches!(sign, '+' | '-' | '−') || !chars.next()?.is_ascii_digit() {
        return None;
    }
    let (delta, rest) = detail.split_once(' ').unwrap_or((detail, ""));
    Some((delta, rest.trim()))
}

/// A radial dial of dithered blocks.
pub(super) fn gauge(
    label: String,
    value: f64,
    min: f64,
    max: f64,
    unit: Option<String>,
    token: ThemeColor,
    motion: Motion,
) -> AnyElement {
    const SEGMENTS: usize = 36;
    const SPAN: f32 = 270.;
    const START: f32 = -135.;
    let ratio = ((value - min) / (max - min)).clamp(0., 1.) as f32;
    let format = move |value: f64| match unit.as_deref() {
        Some("%") => format!("{}%", compact(value, 1.)),
        Some(unit) => format!("{} {unit}", compact(value, 0.1)),
        None => compact(value, 0.1),
    };
    let size = 220f32;
    let outer = size / 2. - 1. - CELL;
    let height = size / 2. + (outer + CELL) * std::f32::consts::FRAC_1_SQRT_2 + 18.;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let tint = color(&token);
            let size = size.min(bounds.size.width.as_f32());
            let left = ((bounds.size.width.as_f32() - size) / 2.).round();
            let grid = Grid::new(bounds.origin + point(px(left), px(0.)), size, height);
            let center = (size / 2., size / 2.);
            let outer = size / 2. - 1. - CELL;
            let thickness = ((size * 0.12) / CELL).round().max(2.) * CELL;
            let inner = outer - thickness;
            let step = SPAN / SEGMENTS as f32;
            let middle = outer - thickness / 2.;
            let gap = (2. / middle).to_degrees();
            // The blocks fill in one by one as the reading counts up.
            let grow = motion.at(0., 1000., window, cx);
            let filled = ratio * grow * SEGMENTS as f32;
            let reading = format(min + (value - min) * grow as f64);
            let active = (filled - 1e-6).ceil().max(0.) as usize;
            let track = faint(tint, 0.14);
            grid.paint(window, |col, row| {
                let (x, y) = Grid::center(col, row);
                let (dx, dy) = (x - center.0, y - center.1);
                let r = (dx * dx + dy * dy).sqrt();
                if r < inner || r > outer + CELL {
                    return None;
                }
                let angle = dx.atan2(-dy).to_degrees();
                let along = angle - START;
                if !(0. ..SPAN).contains(&along) {
                    return None;
                }
                let index = (along / step) as usize;
                let within = along - index as f32 * step;
                if within < gap / 2. || within > step - gap / 2. {
                    return None;
                }
                let head = active > 0 && index == active - 1;
                // The block at the value steps out a cell, like a needle.
                if r > outer && !head {
                    return None;
                }
                if index >= active {
                    return Some(track);
                }
                if head {
                    let part = (filled - index as f32).clamp(0., 1.);
                    if within > gap / 2. + (step - gap) * part.max(0.15) {
                        return (r <= outer).then_some(track);
                    }
                    return Some(tint);
                }
                let tone = (1. - (active - 1 - index) as f32 / 16.).max(9. / 16.);
                inked(col, row, tone).then_some(tint)
            });
            let at = |dx: f32, dy: f32| grid.origin + point(px(center.0 + dx), px(center.1 + dy));
            Label::sans((size * 0.15).round(), content(1.), FontWeight::SEMIBOLD).paint(
                &reading,
                at(0., -4.),
                Align::Center,
                window,
                cx,
            );
            Label::sans(12., content(0.6), FontWeight::NORMAL).paint(
                &label,
                at(0., size * 0.1 + 6.),
                Align::Center,
                window,
                cx,
            );
            let ends = middle * std::f32::consts::FRAC_1_SQRT_2;
            for (dx, value) in [(-ends, min), (ends, max)] {
                axis().paint(
                    &compact(value, 1.),
                    at(dx, ends + thickness / 2. + 12.),
                    Align::Center,
                    window,
                    cx,
                );
            }
        },
    )
    .w_full()
    .h(px(height))
    .into_any_element()
}

/// Parts of a whole as a dithered ring with a legend.
pub(super) fn donut(
    slices: Vec<Slice>,
    label: Option<String>,
    value: Option<String>,
    motion: Motion,
    id: SharedString,
) -> AnyElement {
    const SIZE: f32 = 184.;
    const FLOOR: f32 = 3. / 16.;
    let total: f64 = slices.iter().map(|s| s.value.max(0.)).sum();
    let total_reading = value.unwrap_or_else(|| {
        let s = format!("{total:.0}");
        if total.abs() >= 1e4 {
            // 106000 → 106,000: the hole has room for the full figure.
            let digits: Vec<char> = s.chars().collect();
            let mut out = String::new();
            for (i, c) in digits.iter().enumerate() {
                if i > 0 && (digits.len() - i).is_multiple_of(3) && c.is_ascii_digit() {
                    out.push(',');
                }
                out.push(*c);
            }
            out
        } else {
            compact(total, 0.1)
        }
    });
    let legend = div()
        .flex_1()
        .min_w(px(140.))
        .max_w(px(320.))
        .flex()
        .flex_col()
        .gap_2()
        .children(slices.iter().map(|s| {
            let share = if total > 0. {
                s.value / total * 100.
            } else {
                0.
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .child(swatch(color(&s.color)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(content(0.8))
                        .child(s.label.clone()),
                )
                .child(
                    div()
                        .font_family(crate::fonts::mono())
                        .text_color(content(0.5))
                        .child(format!("{} · {share:.0}%", compact(s.value, 0.1))),
                )
        }));
    let ring = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let grid = Grid::new(bounds.origin, SIZE, SIZE);
            let half = (grid.cols as f32 * CELL) / 2.;
            let outer = half - 2. * CELL;
            let inner = outer - ((SIZE * 0.17) / CELL).round() * CELL;
            // The ring sweeps round from twelve o'clock.
            let sweep = motion.at(0., 900., window, cx) * std::f32::consts::TAU;
            let angle_of = |dx: f32, dy: f32| {
                let a = dx.atan2(-dy);
                if a < 0. { a + std::f32::consts::TAU } else { a }
            };
            let mut edges = Vec::new();
            let mut running = 0.;
            for s in &slices {
                edges.push((running / total.max(1e-9)) as f32 * std::f32::consts::TAU);
                running += s.value.max(0.);
            }
            let shown = slices.iter().filter(|s| s.value > 0.).count();
            // The hovered slice stays bright and the others dim.
            let hovered = pointer(bounds, window).and_then(|(x, y)| {
                let (dx, dy) = (x - half, y - half);
                let r = (dx * dx + dy * dy).sqrt();
                (r >= inner - CELL && r <= outer + CELL)
                    .then(|| edges.iter().rposition(|e| angle_of(dx, dy) >= *e))
                    .flatten()
            });
            let colors: Vec<Hsla> = slices
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let c = color(&s.color);
                    if hovered.is_some_and(|h| h != i) {
                        faint(c, 0.25)
                    } else {
                        c
                    }
                })
                .collect();
            grid.paint(window, |col, row| {
                let (x, y) = Grid::center(col, row);
                let (dx, dy) = (x - half, y - half);
                let r = (dx * dx + dy * dy).sqrt();
                if r < inner || r > outer {
                    return None;
                }
                let angle = angle_of(dx, dy);
                if angle > sweep {
                    return None;
                }
                let index = edges.iter().rposition(|e| angle >= *e)?;
                let mut outline = r > outer - CELL || r < inner + CELL;
                if shown > 1 {
                    // Distance in cells from the nearest slice edge.
                    let near = edges
                        .iter()
                        .zip(&slices)
                        .filter(|(_, s)| s.value > 0.)
                        .map(|(e, _)| {
                            let d = (angle - e).abs();
                            let d = d.min(std::f32::consts::TAU - d);
                            if d > std::f32::consts::FRAC_PI_2 {
                                f32::MAX
                            } else {
                                r * d.sin() / CELL
                            }
                        })
                        .fold(f32::MAX, f32::min);
                    if near < 0.75 {
                        return None;
                    }
                    outline |= near < 1.75;
                }
                let depth = (outer - r) / (outer - inner);
                let tone = FLOOR + (1. - FLOOR) * (1. - depth).powf(1.4);
                (outline || inked(col, row, tone)).then_some(colors[index])
            });
            let center = grid.origin + point(px(half), px(half));
            // The middle names the hovered slice, else the whole.
            let (caption, figure, share) = match hovered.and_then(|h| slices.get(h)) {
                Some(s) => (
                    Some(s.label.clone()),
                    reading(s.value),
                    Some(format!("{:.0}%", s.value / total.max(1e-9) * 100.)),
                ),
                None => (label.clone(), total_reading.clone(), None),
            };
            if let Some(caption) = &caption {
                let caption: String = caption.to_uppercase().chars().take(18).collect();
                axis().paint(
                    &caption,
                    center - point(px(0.), px(16.)),
                    Align::Center,
                    window,
                    cx,
                );
            }
            let fit = ((inner * 2. - 16.) / (figure.chars().count().max(1) as f32 * 0.62)).min(26.);
            Label::sans(fit.floor(), content(1.), FontWeight::SEMIBOLD).paint(
                &figure,
                center + point(px(0.), px(if caption.is_some() { 4. } else { 0. })),
                Align::Center,
                window,
                cx,
            );
            if let Some(share) = share {
                axis().paint(
                    &share,
                    center + point(px(0.), px(24.)),
                    Align::Center,
                    window,
                    cx,
                );
            }
        },
    )
    .size_full();
    div()
        .w_full()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_6()
        .child(hoverable(id).size(px(SIZE)).flex_shrink_0().child(ring))
        .child(legend)
        .into_any_element()
}

/// A grid of dithered squares, one Bayer tile each.
pub(super) fn heatmap(
    values: Vec<Vec<f64>>,
    rows: Vec<String>,
    columns: Vec<String>,
    token: ThemeColor,
    motion: Motion,
    id: SharedString,
) -> AnyElement {
    const SQUARE: i32 = 4;
    const PITCH: i32 = 5;
    let width = values.first().map_or(0, Vec::len) as i32;
    let gutter = if rows.is_empty() { 0. } else { 34. };
    let top = if columns.is_empty() { 0. } else { 18. };
    let height = top + (values.len() as i32 * PITCH * CELL as i32) as f32;
    let high = values
        .iter()
        .flatten()
        .copied()
        .fold(0f64, f64::max)
        .max(1e-9);
    let map = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let tint = color(&token);
            let track = content(0.07);
            let grid = Grid::new(
                bounds.origin + point(px(gutter), px(top)),
                bounds.size.width.as_f32() - gutter,
                height - top,
            );
            let pitch = PITCH as f32 * CELL;
            // A narrow heatmap keeps its latest columns, as calendars do.
            let fits = ((grid.cols + PITCH - SQUARE) / PITCH).max(1);
            let skip = (width - fits).max(0);
            // Each column's dither thickens in, left to right.
            let levels: Vec<f32> = (0..fits)
                .map(|c| motion.at(c as f32 * 14., 420., window, cx))
                .collect();
            let hovered = pointer(bounds, window).and_then(|(x, y)| {
                let (x, y) = (x - gutter, y - top);
                if x < 0. || y < 0. {
                    return None;
                }
                let (c, r) = ((x / pitch) as i32, (y / pitch) as i32);
                (c < fits && (r as usize) < values.len()).then_some((c, r))
            });
            grid.paint(window, |col, row| {
                let (x, y) = (col % PITCH, row % PITCH);
                if x >= SQUARE || y >= SQUARE || col / PITCH >= fits {
                    return None;
                }
                let v = *values
                    .get((row / PITCH) as usize)?
                    .get((col / PITCH + skip) as usize)?;
                if v <= 0. {
                    return Some(track);
                }
                let mut tone = (0.25 + 0.75 * (v / high) as f32) * levels[(col / PITCH) as usize];
                // The hovered square lights up, still dithered.
                if hovered == Some((col / PITCH, row / PITCH)) {
                    tone = (tone + 0.3).min(14. / 16.);
                }
                let tone = (tone * 16.).round() / 16.;
                Some(if inked(x, y, tone) { tint } else { track })
            });
            for (i, label) in rows.iter().enumerate() {
                // Every other row, as calendars label Mon, Wed, Fri.
                if rows.len() > 4 && i % 2 == 1 {
                    continue;
                }
                axis().paint(
                    label,
                    grid.origin
                        + point(
                            px(-gutter),
                            px(i as f32 * pitch + SQUARE as f32 * CELL / 2.),
                        ),
                    Align::Left,
                    window,
                    cx,
                );
            }
            // Each new label (a month over its weeks) is written where it
            // starts, when it clears the one before.
            let mut previous: Option<(&String, f32)> = None;
            for (i, label) in columns.iter().enumerate().skip(skip as usize) {
                let x = (i - skip as usize) as f32 * pitch;
                if previous.is_some_and(|(p, at)| p == label || x - at < 36.) {
                    continue;
                }
                previous = Some((label, x));
                axis().paint(
                    label,
                    grid.origin + point(px(x), px(-top / 2.)),
                    Align::Left,
                    window,
                    cx,
                );
            }
            if let Some((c, r)) = hovered
                && let Some(v) = values
                    .get(r as usize)
                    .and_then(|row| row.get((c + skip) as usize))
            {
                let name = [
                    rows.get(r as usize).cloned(),
                    columns.get((c + skip) as usize).cloned(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let square = SQUARE as f32 * CELL;
                readout(
                    &name,
                    &[(tint, "Value".into(), reading(*v))],
                    grid.origin
                        + point(
                            px(c as f32 * pitch + square),
                            px(r as f32 * pitch + square / 2.),
                        ),
                    bounds,
                    window,
                    cx,
                );
            }
        },
    )
    .size_full();
    hoverable(id)
        .w_full()
        .h(px(height))
        .child(map)
        .into_any_element()
}

/// A dithered progress bar with a solid head.
pub(super) fn progress(
    label: String,
    value: f64,
    max: f64,
    detail: Option<String>,
    token: ThemeColor,
    motion: Motion,
) -> AnyElement {
    let ratio = (value / max).clamp(0., 1.) as f32;
    let bar = canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let tint = color(&token);
            let track = faint(tint, 0.14);
            let grid = Grid::new(bounds.origin, bounds.size.width.as_f32(), 3. * CELL);
            let grow = motion.at(0., 800., window, cx);
            let head = (grid.cols as f32 * ratio * grow).round() as i32;
            if head > 0 {
                glow(
                    window,
                    Bounds::new(
                        grid.origin,
                        gpui::size(px(head as f32 * CELL), px(3. * CELL)),
                    ),
                    tint,
                    0.3,
                );
            }
            grid.paint(window, |col, row| {
                if col >= head {
                    return Some(track);
                }
                // A comet: sparse at the start, solid at the head.
                let tone = 0.3 + 0.7 * (col as f32 / head.max(1) as f32).powf(1.5);
                Some(if col >= head - 3 || inked(col, row, tone) {
                    tint
                } else {
                    track
                })
            });
        },
    )
    .w_full()
    .h(px(3. * CELL));
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .child(label),
                )
                .child(
                    div()
                        .font_family(crate::fonts::mono())
                        .text_size(px(12.))
                        .text_color(content(0.55))
                        .child(format!("{:.0}%", ratio * 100.)),
                ),
        )
        .child(bar)
        .children(detail.map(|d| div().text_size(px(12.)).text_color(content(0.55)).child(d)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn stat_details_lend_their_leading_change_to_the_delta() {
        assert_eq!(
            super::split_delta("+12.4% vs last month"),
            Some(("+12.4%", "vs last month"))
        );
        assert_eq!(super::split_delta("-3"), Some(("-3", "")));
        assert_eq!(super::split_delta("requests per day"), None);
        assert_eq!(super::split_delta("+ more"), None);
    }
}
