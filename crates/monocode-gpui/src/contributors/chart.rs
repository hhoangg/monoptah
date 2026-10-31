//! The chart shared by the Contributors page, the member detail page and the
//! ClickUp tab: buckets on the x axis, a few colored series, one canvas.
//! What it says is computed by the pure functions below (unit tested); the
//! `ChartView` entity draws it, owns the legend focus and the color palette,
//! and repaints itself on hover without touching the page that holds it.
use super::ui;
use crate::{
    popover::{Align, Anchor, AnchorSlot, Side, popover},
    right_panel::tabular,
    theme::*,
    visuals::dither::{Scale, readout},
};
use gpui::{
    AnyElement, App, Bounds, Context, Div, ElementId, EventEmitter, FocusHandle, Hsla,
    KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Render, Role,
    SharedString, Window, canvas, div, fill, point, prelude::*, px, relative, size,
};
use monocode_core::contributors::model::{axis::axis_ticks, palette::PERSON_COLORS};
use std::{cell::Cell, collections::BTreeSet, rc::Rc, sync::Arc};

pub(super) const CHART_H: f32 = 180.;
const PAD: f32 = 8.;
/// The bars fill this much of their slot, so neighbours never touch.
const BAR_SHARE: f32 = 0.7;
/// Dimmed series keep this much of their color.
const DIM: f32 = 0.2;
/// Series shown on their own; everyone else is summed into "Others".
pub(super) const TOP: usize = 5;
pub(super) const OTHERS_ID: &str = "others";

// ---- the pure model ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ChartKind {
    #[default]
    Line,
    Cumulative,
    Bars,
}
impl ChartKind {
    pub(super) const ALL: [ChartKind; 3] = [Self::Line, Self::Cumulative, Self::Bars];
    /// The value saved in the settings and the id of the tab.
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Cumulative => "cumulative",
            Self::Bars => "bars",
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Line => "Line",
            Self::Cumulative => "Cumulative",
            Self::Bars => "Bars",
        }
    }
    pub(super) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Series {
    pub id: String,
    pub name: String,
    pub color: Hsla,
    /// One value per bucket.
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChartBucket {
    /// Tooltip text: "Oct 5, 2026".
    pub label: String,
    /// Axis text: "Oct 5".
    pub short: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChartData {
    pub buckets: Vec<ChartBucket>,
    pub series: Vec<Series>,
    /// What a screen reader gets for the chart.
    pub summary: String,
}

/// The neutral color of the "Others" series, from the theme.
pub(super) fn others_color() -> Hsla {
    content(0.4)
}

fn total(values: &[f64]) -> f64 {
    values.iter().sum()
}

/// A running total.
pub(super) fn cumulative(values: &[f64]) -> Vec<f64> {
    values
        .iter()
        .scan(0., |sum, v| {
            *sum += v;
            Some(*sum)
        })
        .collect()
}

/// The `TOP` series with the biggest total, in that order (ties keep the order
/// they came in), then everyone else summed into one "Others" series. With
/// `TOP` or fewer there is no "Others".
pub(super) fn top_series(mut all: Vec<Series>) -> Vec<Series> {
    // A stable sort keeps the incoming order among equals.
    all.sort_by(|a, b| total(&b.values).total_cmp(&total(&a.values)));
    if all.len() <= TOP {
        return all;
    }
    let rest = all.split_off(TOP);
    let len = rest.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let mut values = vec![0.; len];
    for series in &rest {
        for (sum, v) in values.iter_mut().zip(&series.values) {
            *sum += v;
        }
    }
    all.push(Series {
        id: OTHERS_ID.into(),
        name: "Others".into(),
        color: others_color(),
        values,
    });
    all
}

/// The color to draw: "Others" follows the theme at draw time.
fn color_of(series: &Series) -> Hsla {
    if series.id == OTHERS_ID {
        others_color()
    } else {
        series.color
    }
}

/// Whether a series is faded: some focused member is on the chart and this one
/// is not among the focused. Ids no longer present are ignored, and an empty
/// focus (or one with nobody present) dims no one.
pub(super) fn is_dimmed(focus: &BTreeSet<String>, present_ids: &[&str], id: &str) -> bool {
    present_ids.iter().any(|p| focus.contains(*p)) && !focus.contains(id)
}

/// The focus after clicking `id`: multi-select, so a second click lets go.
pub(super) fn toggled(focus: &BTreeSet<String>, id: &str) -> BTreeSet<String> {
    let mut next = focus.clone();
    if !next.remove(id) {
        next.insert(id.to_string());
    }
    next
}

/// The y domain: stacked totals for bars, the biggest value for lines and
/// running totals for the cumulative view. Whole counts never get a step below 1.
pub(super) fn y_scale(kind: ChartKind, series: &[Series]) -> Scale {
    let count = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let values: Vec<f64> = match kind {
        ChartKind::Bars => (0..count)
            .map(|i| series.iter().filter_map(|s| s.values.get(i)).sum())
            .collect(),
        ChartKind::Line => series.iter().flat_map(|s| s.values.clone()).collect(),
        ChartKind::Cumulative => series.iter().flat_map(|s| cumulative(&s.values)).collect(),
    };
    let scale = Scale::nice(values.iter().copied(), true);
    if scale.step < 1. && values.iter().all(|v| v.fract() == 0.) {
        let max = values.iter().copied().fold(0., f64::max).max(1.);
        return Scale {
            min: 0.,
            max,
            step: 1.,
        };
    }
    scale
}

/// Where bucket `i` of `count` sits across the plot, 0 at the left edge and 1 at
/// the right. Lines run edge to edge so the first and last points line up with
/// the first and last axis label; bars keep their slots; one bucket is centred.
pub(super) fn x_frac(kind: ChartKind, i: usize, count: usize) -> f32 {
    if count <= 1 {
        return 0.5;
    }
    match kind {
        ChartKind::Bars => (i as f32 + 0.5) / count as f32,
        _ => i as f32 / (count - 1) as f32,
    }
}

/// Which bucket `x` (px from the left edge of a `width` wide plot) is on: the
/// slot under it for bars, the nearest point for lines.
pub(super) fn hit_bucket(kind: ChartKind, x: f32, width: f32, count: usize) -> Option<usize> {
    if count == 0 || width <= 0. || !(0. ..width).contains(&x) {
        return None;
    }
    if count == 1 {
        return Some(0);
    }
    let at = match kind {
        ChartKind::Bars => (x / width * count as f32) as usize,
        _ => (x / width * (count - 1) as f32).round() as usize,
    };
    Some(at.min(count - 1))
}

/// One label per gridline, top first: its y in px from the top of the plot
/// element (`height` tall, with `PAD` above and below the lines) and the text.
pub(super) fn y_labels(scale: &Scale, height: f32) -> Vec<(f32, String)> {
    let plot_h = (height - PAD * 2.).max(1.);
    let span = (scale.max - scale.min).max(1e-9);
    let mut labels: Vec<(f32, String)> = scale
        .ticks()
        .into_iter()
        .map(|tick| {
            let y = PAD + (1. - ((tick - scale.min) / span) as f32) * plot_h;
            (y, scale.label(tick))
        })
        .collect();
    labels.sort_by(|a, b| a.0.total_cmp(&b.0));
    labels
}

/// Which buckets get an x label, and the label.
pub(super) fn axis_labels(buckets: &[ChartBucket]) -> Vec<(usize, &str)> {
    axis_ticks(buckets.len())
        .into_iter()
        .map(|i| (i, buckets[i].short.as_str()))
        .collect()
}

fn value_text(value: f64) -> String {
    if value.fract() == 0. {
        format!("{}", value as i64)
    } else {
        format!("{value:.1}")
    }
}

/// The card shown next to a hovered bucket.
#[derive(Debug, PartialEq)]
pub(super) struct Tooltip {
    pub title: String,
    /// Swatch, name, value.
    pub rows: Vec<(Hsla, String, String)>,
}

/// The tooltip of bucket `index`: the visible (not dimmed) series that have
/// something in it, and their total when there are several.
pub(super) fn tooltip(
    kind: ChartKind,
    data: &ChartData,
    focus: &BTreeSet<String>,
    index: usize,
) -> Option<Tooltip> {
    let bucket = data.buckets.get(index)?;
    let present: Vec<&str> = data.series.iter().map(|s| s.id.as_str()).collect();
    let mut rows = Vec::new();
    let mut sum = 0.;
    for series in &data.series {
        if is_dimmed(focus, &present, &series.id) {
            continue;
        }
        let value = match kind {
            ChartKind::Cumulative => cumulative(&series.values).get(index).copied(),
            _ => series.values.get(index).copied(),
        }
        .unwrap_or(0.);
        if value > 0. {
            sum += value;
            rows.push((color_of(series), series.name.clone(), value_text(value)));
        }
    }
    if rows.len() > 1 {
        rows.push((content(0.35), "Total".into(), value_text(sum)));
    }
    let title = match kind {
        ChartKind::Cumulative => format!("{} - running total", bucket.label),
        _ => bucket.label.clone(),
    };
    Some(Tooltip { title, rows })
}

/// The text a screen reader gets: "Commit activity per day: 12 commits over 7
/// days, busiest day Oct 13, 2026 with 5." `totals` is one number per bucket.
pub(super) fn summary(
    title: &str,
    unit: &str,
    noun: &str,
    buckets: &[ChartBucket],
    totals: &[f64],
) -> String {
    let count = buckets.len();
    let all: f64 = totals.iter().sum();
    let mut busiest: Option<usize> = None;
    for (i, t) in totals.iter().enumerate() {
        if busiest.is_none_or(|best| *t > totals[best]) {
            busiest = Some(i);
        }
    }
    match busiest {
        Some(best) if all > 0. => format!(
            "{title} per {unit}: {} {noun} over {count} {unit}s, busiest {unit} {} with {}.",
            value_text(all),
            buckets[best].label,
            value_text(totals[best])
        ),
        _ => format!("{title} per {unit}: no {noun} in {count} {unit}s."),
    }
}

// ---- the app-wide choice -----------------------------------------------------------

/// The chart type, shared by every chart of every open page: one entity per
/// app, so a pick in one project reaches the others at once.
pub(super) struct ChartPrefs {
    pub kind: ChartKind,
    /// The saved value was read, or the user chose; later pages do not read it again.
    pub settled: bool,
}
struct SharedPrefs(gpui::Entity<ChartPrefs>);
impl gpui::Global for SharedPrefs {}

pub(super) fn shared_prefs(cx: &mut App) -> gpui::Entity<ChartPrefs> {
    if let Some(shared) = cx.try_global::<SharedPrefs>() {
        return shared.0.clone();
    }
    let prefs = cx.new(|_| ChartPrefs {
        kind: ChartKind::default(),
        settled: false,
    });
    cx.set_global(SharedPrefs(prefs.clone()));
    prefs
}

// ---- the entity --------------------------------------------------------------------

/// What the chart tells its owner.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ChartEvent {
    /// The user picked another chart type; one value is shared by every chart.
    Kind(ChartKind),
    /// The user picked a palette color for a member.
    Color { id: String, hex: &'static str },
}

pub(super) struct ChartView {
    /// Prefix of the selectors and ids: `contributors-stats`.
    prefix: &'static str,
    pub(super) data: Arc<ChartData>,
    pub(super) kind: ChartKind,
    /// The members the legend focuses; view state, never saved.
    pub(super) focus: BTreeSet<String>,
    pub(super) hover: Option<usize>,
    /// Where the chart was laid out, for the pointer.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The series whose palette is open.
    pub(super) palette: Option<String>,
    /// Series colors can be picked. Off for charts whose series are not people.
    colorable: bool,
    /// The chart type tabs are drawn. Off for a second chart beside one that has them.
    tabs: bool,
    palette_anchor: AnchorSlot,
    palette_focus: FocusHandle,
    /// What had the focus before the palette took it.
    restore: Option<FocusHandle>,
}
impl EventEmitter<ChartEvent> for ChartView {}

impl ChartView {
    pub(super) fn new(
        prefix: &'static str,
        data: Arc<ChartData>,
        kind: ChartKind,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            prefix,
            data,
            kind,
            focus: BTreeSet::new(),
            hover: None,
            bounds: Rc::default(),
            palette: None,
            colorable: true,
            tabs: true,
            palette_anchor: AnchorSlot::default(),
            palette_focus: cx.focus_handle(),
            restore: None,
        }
    }

    /// A chart whose series are not people: no palette, no color button.
    pub(super) fn without_palette(mut self) -> Self {
        self.colorable = false;
        self
    }

    /// A chart that follows the shared chart type without offering the tabs itself.
    pub(super) fn without_tabs(mut self) -> Self {
        self.tabs = false;
        self
    }

    pub(super) fn set_data(&mut self, data: Arc<ChartData>, cx: &mut Context<Self>) {
        if *self.data == *data {
            return;
        }
        self.data = data;
        if self.hover.is_some_and(|i| i >= self.data.buckets.len()) {
            self.hover = None;
        }
        if let Some(id) = &self.palette
            && !self.data.series.iter().any(|s| &s.id == id)
        {
            self.dismiss_palette(cx);
        }
        cx.notify();
    }

    pub(super) fn set_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        if self.kind != kind {
            self.kind = kind;
            cx.notify();
        }
    }

    fn pick_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        if self.kind != kind {
            self.kind = kind;
            cx.emit(ChartEvent::Kind(kind));
            cx.notify();
        }
    }

    pub(super) fn toggle_focus(&mut self, id: &str, cx: &mut Context<Self>) {
        self.focus = toggled(&self.focus, id);
        cx.notify();
    }

    pub(super) fn open_palette(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if id == OTHERS_ID || !self.colorable {
            return;
        }
        self.palette = Some(id.to_string());
        self.restore = window.focused(cx);
        window.focus(&self.palette_focus, cx);
        cx.notify();
    }

    /// Closes the palette when there is no window at hand (new data, the section
    /// or project is left). The focus goes back at once, in the window that holds
    /// it, so the page's keys work even if the chart is never drawn again.
    pub(super) fn dismiss_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette.take().is_none() {
            return;
        }
        if let Some(home) = self.restore.take() {
            let held = self.palette_focus.clone();
            cx.defer(move |cx| {
                for handle in cx.windows() {
                    let (held, home) = (held.clone(), home.clone());
                    let _ = handle.update(cx, move |_, window, cx| {
                        if held.is_focused(window) {
                            window.focus(&home, cx);
                        }
                    });
                }
            });
        }
        cx.notify();
    }

    /// Closes the palette and gives the focus back, so the page's keys work again.
    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            if let Some(handle) = self.restore.take() {
                window.focus(&handle, cx);
            }
            cx.notify();
        }
    }

    fn pick_color(&mut self, hex: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.palette.clone() {
            self.close_palette(window, cx);
            cx.emit(ChartEvent::Color { id, hex });
        }
    }

    fn move_pointer(&mut self, at: Option<gpui::Point<Pixels>>, cx: &mut Context<Self>) {
        let bounds = self.bounds.get();
        let next = at.filter(|p| bounds.contains(p)).and_then(|p| {
            hit_bucket(
                self.kind,
                (p.x - bounds.origin.x).as_f32(),
                bounds.size.width.as_f32(),
                self.data.buckets.len(),
            )
        });
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }
}

// ---- drawing -----------------------------------------------------------------------

/// What the canvas needs, copied out of the view so it can outlive the render.
struct Plot {
    data: Arc<ChartData>,
    kind: ChartKind,
    dim: Vec<bool>,
    hover: Option<usize>,
    focus: BTreeSet<String>,
}

fn faded(color: Hsla, dim: bool) -> Hsla {
    let mut color = color;
    if dim {
        color.a *= DIM;
    }
    color
}

fn quad(window: &mut Window, x: f32, y: f32, w: f32, h: f32, color: Hsla, radius: f32) {
    window.paint_quad(
        fill(Bounds::new(point(px(x), px(y)), size(px(w), px(h))), color).corner_radii(px(radius)),
    );
}

fn paint_plot(plot: &Plot, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let data = &plot.data;
    let count = data.buckets.len();
    if count == 0 {
        return;
    }
    let (left, top) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
    let (width, height) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    let plot_h = (height - PAD * 2.).max(1.);
    let scale = y_scale(plot.kind, &data.series);
    let span = (scale.max - scale.min).max(1e-9);
    let y_of = |v: f64| top + PAD + (1. - ((v - scale.min) / span) as f32) * plot_h;
    let band = width / count as f32;
    let x_of = |i: usize| left + width * x_frac(plot.kind, i, count);

    for tick in scale.ticks() {
        quad(
            window,
            left,
            y_of(tick).round(),
            width,
            1.,
            content(0.07),
            0.,
        );
    }
    quad(window, left, y_of(scale.min), width, 1., content(0.15), 0.);
    if let Some(i) = plot.hover {
        let (x0, w) = match plot.kind {
            ChartKind::Bars => (left + band * i as f32, band),
            _ => {
                let x0 = (x_of(i) - band / 2.).max(left);
                let x1 = (x_of(i) + band / 2.).min(left + width);
                (x0, x1 - x0)
            }
        };
        quad(window, x0, top, w, height, content(0.04), 0.);
    }

    match plot.kind {
        ChartKind::Bars => {
            let bar = band * BAR_SHARE;
            for b in 0..count {
                let x = x_of(b) - bar / 2.;
                let mut base = 0.;
                for (s, series) in data.series.iter().enumerate() {
                    let v = series.values.get(b).copied().unwrap_or(0.);
                    if v <= 0. {
                        continue;
                    }
                    let (y0, y1) = (y_of(base), y_of(base + v));
                    base += v;
                    let color = faded(color_of(series), plot.dim[s]);
                    quad(window, x, y1, bar, (y0 - y1).max(0.), color, 1.);
                }
            }
        }
        ChartKind::Line | ChartKind::Cumulative => {
            let lines: Vec<Vec<f64>> = data
                .series
                .iter()
                .map(|s| match plot.kind {
                    ChartKind::Cumulative => cumulative(&s.values),
                    _ => s.values.clone(),
                })
                .collect();
            // Faded series first, so the focused ones draw over them.
            let order = (0..lines.len())
                .filter(|s| plot.dim[*s])
                .chain((0..lines.len()).filter(|s| !plot.dim[*s]));
            for s in order {
                let color = faded(color_of(&data.series[s]), plot.dim[s]);
                let at = |i: usize| {
                    point(
                        px(x_of(i)),
                        px(y_of(lines[s].get(i).copied().unwrap_or(0.))),
                    )
                };
                if count == 1 {
                    let p = at(0);
                    quad(
                        window,
                        p.x.as_f32() - 4.,
                        p.y.as_f32() - 4.,
                        8.,
                        8.,
                        color,
                        4.,
                    );
                    continue;
                }
                let mut path = PathBuilder::stroke(px(2.));
                path.move_to(at(0));
                for i in 1..count {
                    path.line_to(at(i));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            }
            if let Some(i) = plot.hover {
                for (s, line) in lines.iter().enumerate() {
                    if plot.dim[s] {
                        continue;
                    }
                    let y = y_of(line.get(i).copied().unwrap_or(0.));
                    quad(
                        window,
                        x_of(i) - 3.,
                        y - 3.,
                        6.,
                        6.,
                        color_of(&data.series[s]),
                        3.,
                    );
                }
            }
        }
    }

    if let Some(i) = plot.hover
        && let Some(tip) = tooltip(plot.kind, data, &plot.focus, i)
    {
        let high = match plot.kind {
            ChartKind::Bars => {
                let sum: f64 = data
                    .series
                    .iter()
                    .enumerate()
                    .filter(|(s, _)| !plot.dim[*s])
                    .filter_map(|(_, s)| s.values.get(i))
                    .sum();
                y_of(sum)
            }
            _ => data
                .series
                .iter()
                .enumerate()
                .filter(|(s, _)| !plot.dim[*s])
                .map(|(_, s)| {
                    let value = match plot.kind {
                        ChartKind::Cumulative => cumulative(&s.values).get(i).copied(),
                        _ => s.values.get(i).copied(),
                    };
                    y_of(value.unwrap_or(0.))
                })
                .fold(top + height / 2., f32::min),
        };
        readout(
            &tip.title,
            &tip.rows,
            point(px(x_of(i)), px(high)),
            bounds,
            window,
            cx,
        );
    }
}

impl ChartView {
    fn tabs(&self, cx: &mut Context<Self>) -> Div {
        let options: Vec<(&'static str, &'static str)> = ChartKind::ALL
            .iter()
            .map(|kind| (kind.id(), kind.label()))
            .collect();
        div().mb(px(8.)).flex().justify_end().child(ui::segmented(
            "chart-kind",
            "Chart type",
            self.kind.id(),
            &options,
            cx,
            |this: &mut Self, value, cx| {
                if let Some(kind) = ChartKind::parse(value) {
                    this.pick_kind(kind, cx);
                }
            },
        ))
    }

    fn plot(&self, dim: &[bool], cx: &mut Context<Self>) -> AnyElement {
        let prefix = self.prefix;
        let id = SharedString::from(format!("{prefix}-chart"));
        let selector = id.to_string();
        let bounds = self.bounds.clone();
        let plot = Plot {
            data: self.data.clone(),
            kind: self.kind,
            dim: dim.to_vec(),
            hover: self.hover,
            focus: self.focus.clone(),
        };
        let scale = y_scale(self.kind, &self.data.series);
        div()
            .id(id)
            .debug_selector(move || selector.clone())
            .role(Role::Image)
            .aria_label(self.data.summary.clone())
            .relative()
            .h(px(CHART_H))
            .w_full()
            // The chart repaints itself; the page around it is not asked to.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                this.move_pointer(Some(event.position), cx)
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.move_pointer(None, cx);
                }
            }))
            .child(
                canvas(
                    move |area, _, _| bounds.set(area),
                    move |area, (), window, cx| paint_plot(&plot, area, window, cx),
                )
                .absolute()
                .size_full(),
            )
            .children(
                y_labels(&scale, CHART_H)
                    .into_iter()
                    .enumerate()
                    .map(|(n, (y, text))| {
                        let label = div()
                            .absolute()
                            .top(px((y - 6.).max(0.)))
                            .left_0()
                            .text_size(px(10.))
                            .text_color(content(0.4))
                            .font_features(tabular())
                            .child(text);
                        if n == 0 {
                            label.debug_selector(move || format!("{prefix}-ymax"))
                        } else {
                            label.debug_selector(move || format!("{prefix}-ytick-{n}"))
                        }
                    }),
            )
            .into_any_element()
    }

    fn axis(&self) -> Div {
        let prefix = self.prefix;
        let count = self.data.buckets.len();
        let last = count.saturating_sub(1);
        let kind = self.kind;
        div()
            .relative()
            .mt(px(4.))
            .h(px(16.))
            .text_size(px(10.))
            .text_color(content(0.45))
            .children(
                axis_labels(&self.data.buckets)
                    .into_iter()
                    .map(|(i, text)| {
                        // The first label starts at the left edge, the last ends at the
                        // right one, the others are centred on their slot.
                        let slot = div()
                            .debug_selector(move || format!("{prefix}-axis-{i}"))
                            .absolute()
                            .top_0()
                            .whitespace_nowrap()
                            .child(text.to_string());
                        if i == 0 {
                            slot.left_0()
                        } else if i == last {
                            slot.right_0()
                        } else {
                            let centre = x_frac(kind, i, count);
                            slot.left(relative(centre - 0.5 / count as f32))
                                .w(relative(1. / count as f32))
                                .flex()
                                .justify_center()
                        }
                    }),
            )
    }

    fn legend(&self, dim: &[bool], cx: &mut Context<Self>) -> Div {
        let prefix = self.prefix;
        div()
            .debug_selector(move || format!("{prefix}-legend"))
            .mt(px(8.))
            .flex()
            .flex_wrap()
            .gap_x(px(4.))
            .gap_y(px(2.))
            .text_size(px(12.))
            .children(
                self.data
                    .series
                    .iter()
                    .enumerate()
                    .map(|(index, series)| self.legend_item(index, series, dim[index], cx)),
            )
    }

    fn legend_item(
        &self,
        index: usize,
        series: &Series,
        dimmed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let prefix = self.prefix;
        let id = series.id.clone();
        let selector = format!("{prefix}-legend-{index}");
        let group = SharedString::from(format!("{selector}-group"));
        let focused = self.focus.contains(&series.id);
        let editable = series.id != OTHERS_ID && self.colorable;
        let open = self.palette.as_deref() == Some(series.id.as_str());
        let (on_click, on_right) = (id.clone(), id.clone());
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector({
                let selector = selector.clone();
                move || selector.clone()
            })
            .group(group.clone())
            .role(Role::Button)
            .aria_label(series.name.clone())
            .aria_toggled(if focused {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .relative()
            .max_w(px(200.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .px(px(6.))
            .py(px(2.))
            .cursor_pointer()
            .text_color(content(if dimmed { 0.3 } else { 0.7 }))
            .hover(|s| s.bg(content(0.06)))
            .when(focused, |d| d.bg(content(0.08)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_focus(&on_click, cx)))
            .when(editable, |d| {
                // On release, so the page's own press handler does not take the focus back.
                d.on_mouse_up(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseUpEvent, window, cx| {
                        this.open_palette(&on_right, window, cx)
                    }),
                )
            })
            .child(
                div()
                    .size(px(8.))
                    .flex_shrink_0()
                    .rounded(px(2.))
                    .bg(faded(color_of(series), dimmed)),
            )
            .child(div().min_w_0().truncate().child(series.name.clone()))
            .when(editable, |d| {
                let open_id = id.clone();
                d.child(
                    div()
                        .id(SharedString::from(format!("{selector}-color")))
                        .debug_selector({
                            let selector = format!("{selector}-color");
                            move || selector.clone()
                        })
                        .role(Role::Button)
                        .aria_label(format!("Change color of {}", series.name))
                        .size(px(16.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .hover(|s| s.bg(content(0.1)))
                        .child(icon("palette", 11.))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_palette(&open_id, window, cx);
                            cx.stop_propagation();
                        })),
                )
            })
            .when(open, |d| {
                d.child(self.palette_anchor.probe())
                    .child(self.palette_popover(series, cx))
            })
            .into_any_element()
    }

    fn palette_popover(&self, series: &Series, cx: &mut Context<Self>) -> AnyElement {
        let prefix = self.prefix;
        let weak = cx.weak_entity();
        let current = series.color;
        popover(
            ElementId::from(SharedString::from(format!("{prefix}-palette"))),
            Anchor::Slot(self.palette_anchor.clone()),
        )
        .side(Side::Bottom)
        .align(Align::Start)
        .gap(6.)
        .padding(8.)
        .on_dismiss(move |window, cx| {
            let _ = weak.update(cx, |this, cx| this.close_palette(window, cx));
        })
        .child(
            div()
                .id(SharedString::from(format!("{prefix}-palette-list")))
                .role(Role::Group)
                .aria_label(format!("Colors for {}", series.name))
                .track_focus(&self.palette_focus)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_palette(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .grid()
                .grid_cols(4)
                .gap(px(6.))
                .children(PERSON_COLORS.iter().enumerate().map(|(n, hex)| {
                    let color = ui::hex_color(hex);
                    let selector = format!("{prefix}-palette-{n}");
                    div()
                        .id(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .role(Role::Button)
                        .aria_label(format!("Color {}", n + 1))
                        .aria_toggled(if color == current {
                            gpui::Toggled::True
                        } else {
                            gpui::Toggled::False
                        })
                        .size(px(22.))
                        .rounded_full()
                        .border_2()
                        .border_color(if color == current {
                            content(0.9)
                        } else {
                            content(0.)
                        })
                        .bg(color)
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.pick_color(hex, window, cx)
                            }),
                        )
                })),
        )
        .into_any_element()
    }
}

impl Render for ChartView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let present: Vec<&str> = self.data.series.iter().map(|s| s.id.as_str()).collect();
        let dim: Vec<bool> = present
            .iter()
            .map(|id| is_dimmed(&self.focus, &present, id))
            .collect();
        div()
            .px(px(12.))
            .py(px(8.))
            .when(self.tabs, |d| d.child(self.tabs(cx)))
            .child(self.plot(&dim, cx))
            .child(self.axis())
            .child(self.legend(&dim, cx))
    }
}
