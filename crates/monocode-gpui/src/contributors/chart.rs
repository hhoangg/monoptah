//! The chart shared by the Contributors page, the member detail page and the
//! ClickUp tab: buckets on the x axis, a few colored series, one canvas.
//! What it says is computed by the pure functions below (unit tested); the
//! `ChartView` entity draws it, owns the legend focus and the color palette,
//! and repaints itself on hover without touching the page that holds it.
use super::ui;
use crate::{
    popover::{Align, Anchor, AnchorSlot, Side, popover},
    theme::*,
    visuals::{
        charts::{self, DIM, Drawn, Hover, Plot, bucket_at, geometry, legend_style},
        dither::{CELL, Motion, Scale, swatch},
        tab_button,
    },
};
use gpui::{
    AnyElement, App, Bounds, Context, Div, ElementId, EventEmitter, FocusHandle, Hsla,
    KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Render, Role, SharedString,
    Window, canvas, div, prelude::*, px,
};
use monocode_core::contributors::model::palette::PERSON_COLORS;
use monocode_core::visual::ChartKind as Shape;
use std::{cell::Cell, collections::BTreeSet, rc::Rc, sync::Arc, time::Instant};

pub(super) const CHART_H: f32 = 200.;
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
    /// How the shared chart draws it: a running total is a line over other values.
    fn shape(self) -> Shape {
        match self {
            Self::Line | Self::Cumulative => Shape::Line,
            Self::Bars => Shape::Bar,
        }
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

/// What a series draws as: its running total for the cumulative view.
fn plotted(kind: ChartKind, series: &Series) -> Vec<f64> {
    match kind {
        ChartKind::Cumulative => cumulative(&series.values),
        _ => series.values.clone(),
    }
}

/// The y domain, from zero: the biggest value drawn (bars sit side by side, so
/// no stacking). Whole counts never get a step below 1.
pub(super) fn y_scale(kind: ChartKind, series: &[Series]) -> Scale {
    let values: Vec<f64> = series.iter().flat_map(|s| plotted(kind, s)).collect();
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

/// Whether two datasets draw the same shape: the same buckets and series
/// (ids, names, values), whatever their colors.
fn same_shape(a: &ChartData, b: &ChartData) -> bool {
    a.buckets == b.buckets
        && a.series.len() == b.series.len()
        && a.series
            .iter()
            .zip(&b.series)
            .all(|(x, y)| x.id == y.id && x.name == y.name && x.values == y.values)
}

/// How many buckets the chart draws: the longest series or the labels.
fn bucket_count(data: &ChartData) -> usize {
    data.series
        .iter()
        .map(|s| s.values.len())
        .max()
        .unwrap_or(0)
        .max(data.buckets.len())
}

/// The plot of a chart canvas at `bounds`: the one grid both the paint and the
/// pointer use, the y label gutter left of it included.
pub(super) fn plot_grid(
    kind: ChartKind,
    scale: &Scale,
    bounds: Bounds<Pixels>,
) -> crate::visuals::dither::Grid {
    geometry(
        kind.shape(),
        scale,
        bounds.size.width.as_f32(),
        bounds.size.height.as_f32(),
    )
    .grid(bounds.origin)
}

/// The bucket under `at` in a chart canvas at `bounds`.
pub(super) fn hover_bucket(
    kind: ChartKind,
    scale: &Scale,
    count: usize,
    bounds: Bounds<Pixels>,
    at: gpui::Point<Pixels>,
) -> Option<usize> {
    let grid = plot_grid(kind, scale, bounds);
    bucket_at(
        kind.shape(),
        (at.x - grid.origin.x).as_f32(),
        grid.cols as f32 * CELL,
        count,
    )
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

/// The tooltip of bucket `index`: every visible (not dimmed) series with its
/// value, zero included, and their total when there are several.
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
        sum += value;
        rows.push((color_of(series), series.name.clone(), value_text(value)));
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
    /// When the lines and bars last started drawing in: new data or another type.
    pub(super) motion: Motion,
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
            motion: Motion(Instant::now()),
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
        // A new color (the palette) is not new data: nothing draws in again.
        if !same_shape(&self.data, &data) {
            self.motion = Motion(Instant::now());
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
            self.motion = Motion(Instant::now());
            cx.notify();
        }
    }

    fn pick_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        if self.kind != kind {
            self.kind = kind;
            self.motion = Motion(Instant::now());
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
        let scale = y_scale(self.kind, &self.data.series);
        let count = bucket_count(&self.data);
        let next = at
            .filter(|p| bounds.contains(p))
            .and_then(|p| hover_bucket(self.kind, &scale, count, bounds, p));
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }
}

// ---- drawing -----------------------------------------------------------------------

/// What the canvas needs, copied out of the view so it can outlive the render.
struct Painted {
    data: Arc<ChartData>,
    kind: ChartKind,
    series: Vec<Drawn>,
    labels: Vec<String>,
    scale: Scale,
    hover: Option<usize>,
    focus: BTreeSet<String>,
    motion: Motion,
}

/// The series as the shared chart draws them: colors resolved, running totals
/// for the cumulative view.
fn drawn(kind: ChartKind, data: &ChartData, dim: &[bool]) -> Vec<Drawn> {
    data.series
        .iter()
        .zip(dim)
        .map(|(s, dimmed)| Drawn {
            label: s.name.clone(),
            color: color_of(s),
            values: plotted(kind, s),
            dimmed: *dimmed,
        })
        .collect()
}

fn paint_plot(painted: &Painted, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let tip = |i: usize| {
        tooltip(painted.kind, &painted.data, &painted.focus, i).map(|t| (t.title, t.rows))
    };
    let plot = Plot {
        kind: painted.kind.shape(),
        series: &painted.series,
        labels: &painted.labels,
        scale: painted.scale,
        hover: Hover::At(painted.hover),
        tip: Some(&tip),
        quiet_zero: true,
    };
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        charts::cartesian(&plot, bounds, painted.motion, window, cx)
    });
}

impl ChartView {
    fn tabs(&self, cx: &mut Context<Self>) -> Div {
        div().mb(px(8.)).child(
            div()
                .id("segmented-chart-kind")
                .role(Role::RadioGroup)
                .aria_label("Chart type")
                .flex()
                .flex_wrap()
                .gap_1()
                .children(ChartKind::ALL.into_iter().map(|kind| {
                    let selected = self.kind == kind;
                    let id = kind.id();
                    tab_button(format!("segmented-chart-kind-{id}"), selected)
                        .debug_selector(move || format!("contributors-chart-kind-{id}"))
                        .role(Role::RadioButton)
                        .aria_label(kind.label())
                        .aria_selected(selected)
                        .child(kind.label())
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_kind(kind, cx)))
                })),
        )
    }

    fn plot(&self, dim: &[bool], cx: &mut Context<Self>) -> AnyElement {
        let prefix = self.prefix;
        let id = SharedString::from(format!("{prefix}-chart"));
        let selector = id.to_string();
        let bounds = self.bounds.clone();
        let painted = Painted {
            data: self.data.clone(),
            kind: self.kind,
            series: drawn(self.kind, &self.data, dim),
            labels: self.data.buckets.iter().map(|b| b.short.clone()).collect(),
            scale: y_scale(self.kind, &self.data.series),
            hover: self.hover,
            focus: self.focus.clone(),
            motion: self.motion,
        };
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
                    move |area, (), window, cx| paint_plot(&painted, area, window, cx),
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
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
        legend_style()
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
            .rounded(px(6.))
            .px(px(6.))
            .py(px(2.))
            .cursor_pointer()
            .text_color(content(if dimmed { 0.3 } else { 0.6 }))
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
            .child(swatch(color_of(series)).opacity(if dimmed { DIM } else { 1. }))
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
            .child(self.legend(&dim, cx))
    }
}
