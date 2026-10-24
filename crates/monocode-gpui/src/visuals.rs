//! Session visuals are a bounded native component tree, with no browser surface.
use crate::theme::{accent_color, content};
use gpui::{
    AnyElement, Bounds, ClipboardItem, Context, Div, EventEmitter, FontWeight, Hsla, IntoElement,
    MouseButton, PathBuilder, Pixels, Point, Render, Role, SharedString, Stateful, Window, canvas,
    div, fill, point, prelude::*, px, relative, size,
};
use monocode_core::visual::{
    Camera, ChartKind, Node, SceneObject, Series, Shape, TextSize, ThemeColor, VisualDocument,
    VisualRuntime,
};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

mod scene;
mod source;
#[cfg(test)]
mod tests;

/// The transcript must remeasure this row when its content changes.
pub(crate) struct VisualChanged;
pub(crate) struct VisualView {
    document: VisualDocument,
    runtime: Option<VisualRuntime>,
    tree: Option<Node>,
    error: Option<String>,
    show_source: bool,
    source: Option<gpui::Entity<source::SourceView>>,
    width: Rc<Cell<f32>>,
    tracks: HashMap<String, Rc<Cell<Option<Bounds<Pixels>>>>>,
    slider_drag: Option<SliderDrag>,
    orbits: HashMap<String, scene::Orbit>,
    scene_drag: Option<(String, Point<Pixels>)>,
    inputs: HashMap<String, InputSlot>,
    active: HashSet<String>,
    revision: u64,
    scene_cache: HashMap<String, Rc<RefCell<Option<SceneCache>>>>,
}
impl EventEmitter<VisualChanged> for VisualView {}
struct SliderDrag {
    key: String,
    handler: String,
    min: f64,
    max: f64,
    step: f64,
}
struct InputSlot {
    input: gpui::Entity<crate::input::Input>,
    handler: String,
    source_value: String,
    _subscription: gpui::Subscription,
}
struct SceneCache {
    revision: u64,
    dimensions: [f32; 2],
    orbit: scene::Orbit,
    projection: Rc<scene::Projection>,
}

fn color(token: &ThemeColor) -> Hsla {
    // Semantic hues need darker variants when the interface uses dark ink.
    let light_surface = content(1.).l < 0.5;
    let hue = |dark, light| gpui::rgb(if light_surface { light } else { dark }).into();
    match token {
        ThemeColor::Ink => content(1.),
        ThemeColor::Muted => content(0.6),
        ThemeColor::Faint => content(0.2),
        ThemeColor::Accent | ThemeColor::Series1 => accent_color(),
        ThemeColor::Positive | ThemeColor::Series2 => hue(0x45b98b, 0x18734f),
        ThemeColor::Negative => hue(0xe76f73, 0xb9313d),
        ThemeColor::Warning => hue(0xd69b3a, 0x845c0e),
        ThemeColor::Series3 => hue(0xb58ae8, 0x7952aa),
        ThemeColor::Series4 => hue(0xe8ab51, 0x8a5a12),
        ThemeColor::Series5 => hue(0xe5789b, 0xaa3f67),
        ThemeColor::Series6 => hue(0x5bbdc5, 0x24747d),
    }
}
fn control(id: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .role(Role::Button)
        .focusable()
        .tab_stop(true)
        .h(px(26.))
        .px(px(9.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_size(px(12.))
        .line_height(px(16.))
        .cursor_pointer()
}
fn button(id: impl Into<SharedString>) -> Stateful<Div> {
    control(id)
        .border_1()
        .border_color(content(0.12))
        .text_color(content(0.8))
        .hover(|s| s.bg(content(0.06)).text_color(content(1.)))
        .focus(|s| s.border_color(accent_color()))
}
fn tab_button(id: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    let light = content(1.).l < 0.5;
    let selected_fill: Hsla = gpui::rgb(if light { 0x000000 } else { 0xffffff }).into();
    let selected_text: Hsla = gpui::rgb(if light { 0xffffff } else { 0x141414 }).into();
    control(id)
        .text_color(if selected {
            selected_text
        } else {
            content(0.6)
        })
        .when(selected, |d| d.bg(selected_fill))
        .hover(move |s| {
            if selected {
                s.bg(selected_fill).text_color(selected_text)
            } else {
                s.bg(content(0.06)).text_color(content(1.))
            }
        })
        .focus_visible(move |s| {
            s.bg(if selected {
                selected_fill
            } else {
                content(0.08)
            })
        })
}

fn action_button(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .focusable()
        .tab_stop(true)
        .group("visual-action")
        .size(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .when(active, |d| d.bg(content(0.06)))
        .hover(|s| s.bg(content(0.08)))
        .focus(|s| s.bg(content(0.1)))
        .tooltip(move |_, cx| cx.new(|_| ActionTooltip(label)).into())
        .child(
            crate::theme::icon(glyph, 14.)
                .text_color(if active {
                    accent_color()
                } else {
                    content(0.55)
                })
                .group_hover("visual-action", |s| s.text_color(content(1.))),
        )
}

struct ActionTooltip(&'static str);
impl Render for ActionTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut background = crate::theme::shell_fill();
        background.a = 1.;
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(background)
            .border_1()
            .border_color(content(0.12))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.85))
            .child(self.0)
    }
}
fn activated(event: &gpui::KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "enter" | "space" | " ")
}

impl VisualView {
    pub(crate) fn new(document: VisualDocument, _cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            document,
            runtime: None,
            tree: None,
            error: None,
            show_source: false,
            source: None,
            width: Rc::new(Cell::new(640.)),
            tracks: HashMap::new(),
            slider_drag: None,
            orbits: HashMap::new(),
            scene_drag: None,
            inputs: HashMap::new(),
            active: HashSet::new(),
            revision: 0,
            scene_cache: HashMap::new(),
        };
        this.initialize();
        this
    }
    fn initialize(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.error = None;
        self.tree = None;
        self.runtime = None;
        match self
            .document
            .validate()
            .and_then(|()| VisualRuntime::new(&self.document.source))
            .and_then(|mut runtime| {
                let tree = runtime.render()?;
                Ok((runtime, tree))
            }) {
            Ok((runtime, tree)) => {
                self.runtime = Some(runtime);
                self.tree = Some(tree);
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(VisualChanged);
        cx.notify();
    }
    fn dispatch(&mut self, handler: &str, value: Value, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.as_mut() else {
            return;
        };
        match runtime.dispatch(handler, value) {
            Ok(tree) => {
                self.revision = self.revision.wrapping_add(1);
                // A render refreshes callback capabilities. A pointer drag and a
                // focused input survive that render, so refresh their bindings too.
                if let Some(drag) = self.slider_drag.as_mut() {
                    if let Some(Node::Slider {
                        handler,
                        min,
                        max,
                        step,
                        ..
                    }) = node_at(&tree, &drag.key)
                    {
                        drag.handler = handler.clone();
                        drag.min = *min;
                        drag.max = *max;
                        drag.step = *step;
                    } else {
                        self.slider_drag = None;
                    }
                }
                for (key, slot) in &mut self.inputs {
                    if let Some(Node::Input { handler, .. }) = node_at(&tree, key) {
                        slot.handler = handler.clone();
                    }
                }
                self.tree = Some(tree);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                self.slider_drag = None;
                self.scene_drag = None;
            }
        }
        self.changed(cx);
    }
    fn reset(&mut self, cx: &mut Context<Self>) {
        self.slider_drag = None;
        self.scene_drag = None;
        self.orbits.clear();
        self.inputs.clear();
        self.scene_cache.clear();
        self.initialize();
        self.changed(cx);
    }
    fn slide(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = &self.slider_drag else {
            return;
        };
        let Some(bounds) = self.tracks.get(&drag.key).and_then(|b| b.get()) else {
            return;
        };
        let fraction = ((x - bounds.left()).as_f32() / bounds.size.width.as_f32().max(1.))
            .clamp(0., 1.) as f64;
        let raw = drag.min + (drag.max - drag.min) * fraction;
        let value = (drag.min + ((raw - drag.min) / drag.step).round() * drag.step)
            .clamp(drag.min, drag.max);
        let handler = drag.handler.clone();
        self.dispatch(&handler, json!(value), cx);
    }
    fn node(&mut self, node: &Node, key: String, cx: &mut Context<Self>) -> AnyElement {
        self.active.insert(key.clone());
        match node {
            Node::Column { children, gap }
            | Node::Row { children, gap }
            | Node::Grid { children, gap, .. } => {
                let mut layout = div().w_full().min_w_0().gap(px(*gap));
                if let Node::Grid { columns, .. } = node {
                    let responsive =
                        ((*columns).min((self.width.get() / 180.).floor().max(1.) as usize)).max(1);
                    layout = layout.grid().grid_cols(responsive as u16);
                } else if matches!(node, Node::Row { .. }) {
                    layout = layout.flex().flex_wrap().items_start();
                } else {
                    layout = layout.flex().flex_col();
                }
                for (i, child) in children.iter().enumerate() {
                    let element = self.node(child, format!("{key}/{i}"), cx);
                    layout = layout.child(
                        div()
                            .min_w_0()
                            .when(matches!(node, Node::Row { .. }), |d| {
                                d.flex_1().min_w(px(120.))
                            })
                            .child(element),
                    );
                }
                layout.into_any_element()
            }
            Node::Text {
                text,
                size,
                color: token,
            } => {
                let font_size = match size {
                    TextSize::Caption => 11.,
                    TextSize::Small => 12.,
                    TextSize::Body => 14.,
                    TextSize::Subtitle => 17.,
                    TextSize::Title => 22.,
                    TextSize::Hero => 34.,
                };
                div()
                    .min_w_0()
                    .text_size(px(font_size))
                    .line_height(px(font_size * 1.45))
                    .text_color(color(token))
                    .when(
                        matches!(size, TextSize::Subtitle | TextSize::Title | TextSize::Hero),
                        |d| d.font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(text.clone())
                    .into_any_element()
            }
            Node::Stat {
                label,
                value,
                detail,
                color: token,
            } => div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.6))
                        .child(label.clone()),
                )
                .child(
                    div()
                        .text_size(px(28.))
                        .line_height(px(34.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(token))
                        .child(value.clone()),
                )
                .children(detail.iter().map(|d| {
                    div()
                        .text_size(px(12.))
                        .text_color(content(0.6))
                        .child(d.clone())
                }))
                .into_any_element(),
            Node::Table { columns, rows } => {
                let mut table = div()
                    .id(SharedString::from(format!("visual-table-{key}")))
                    .w_full()
                    .overflow_x_scroll()
                    .flex()
                    .flex_col()
                    .text_size(px(12.));
                let count = columns.len().max(1);
                let width = (self.width.get() / count as f32).max(100.);
                let row = |cells: &[String], header: bool| {
                    div()
                        .flex()
                        .min_w(px(width * count as f32))
                        .border_b_1()
                        .border_color(content(0.08))
                        .when(header, |d| {
                            d.font_weight(FontWeight::SEMIBOLD)
                                .text_color(content(0.65))
                        })
                        .children(cells.iter().map(|cell| {
                            div()
                                .w(px(width))
                                .flex_shrink_0()
                                .px_2()
                                .py_2()
                                .child(cell.clone())
                        }))
                };
                table = table.child(row(columns, true));
                for cells in rows {
                    table = table.child(row(cells, false));
                }
                table.into_any_element()
            }
            Node::Button {
                label,
                handler,
                disabled,
            } => {
                let handler = handler.clone();
                let keyboard = handler.clone();
                button(SharedString::from(format!("visual-button-{key}")))
                    .debug_selector(move || format!("visual-button-{key}"))
                    .aria_label(label.clone())
                    .child(label.clone())
                    .when(*disabled, |d| d.opacity(0.4).cursor_default())
                    .when(!disabled, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            this.dispatch(&handler, Value::Null, cx)
                        }))
                        .on_key_down(cx.listener(
                            move |this, event, _, cx| {
                                if activated(event) {
                                    this.dispatch(&keyboard, Value::Null, cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
                    })
                    .into_any_element()
            }
            Node::Toggle {
                label,
                value,
                handler,
            } => {
                let next = !*value;
                let handler = handler.clone();
                let keyboard = handler.clone();
                div()
                    .id(SharedString::from(format!("visual-toggle-{key}")))
                    .debug_selector(move || format!("visual-toggle-{key}"))
                    .role(Role::Switch)
                    .aria_label(label.clone())
                    .aria_toggled(if *value {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .focusable()
                    .tab_stop(true)
                    .flex()
                    .items_center()
                    .gap_2()
                    .py_1()
                    .cursor_pointer()
                    .child(
                        div()
                            .w(px(30.))
                            .h(px(18.))
                            .rounded_full()
                            .bg(if *value {
                                accent_color()
                            } else {
                                content(0.18)
                            })
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .top(px(3.))
                                    .left(px(if *value { 15. } else { 3. }))
                                    .size(px(12.))
                                    .rounded_full()
                                    .bg(content(1.)),
                            ),
                    )
                    .child(label.clone())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.dispatch(&handler, json!(next), cx)),
                    )
                    .on_key_down(cx.listener(move |this, event, _, cx| {
                        if activated(event) {
                            this.dispatch(&keyboard, json!(next), cx);
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
            }
            Node::Slider {
                label,
                value,
                min,
                max,
                step,
                handler,
            } => {
                let fraction =
                    ((*value - *min) / (*max - *min).max(f64::EPSILON)).clamp(0., 1.) as f32;
                let bounds = self
                    .tracks
                    .entry(key.clone())
                    .or_insert_with(|| Rc::new(Cell::new(None)))
                    .clone();
                let min = *min;
                let max = *max;
                let step = *step;
                let current = *value;
                let handler = handler.clone();
                let keyboard = handler.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .min_w(px(120.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_3()
                            .text_size(px(12.))
                            .child(label.clone())
                            .child(format_number(*value)),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("visual-slider-{key}")))
                            .debug_selector({
                                let key = key.clone();
                                move || format!("visual-slider-{key}")
                            })
                            .role(Role::Slider)
                            .aria_label(label.clone())
                            .focusable()
                            .tab_stop(true)
                            .w_full()
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .relative()
                            .cursor_pointer()
                            .child(
                                canvas(move |rect, _, _| bounds.set(Some(rect)), |_, _, _, _| {})
                                    .absolute()
                                    .size_full(),
                            )
                            .child(div().w_full().h(px(4.)).rounded_full().bg(content(0.12)))
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top(px(8.))
                                    .h(px(4.))
                                    .w(relative(fraction))
                                    .rounded_full()
                                    .bg(accent_color()),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top(px(4.))
                                    .left(relative(fraction))
                                    .ml(px(-6.))
                                    .size(px(12.))
                                    .rounded_full()
                                    .bg(accent_color()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                                    this.slider_drag = Some(SliderDrag {
                                        key: key.clone(),
                                        handler: handler.clone(),
                                        min,
                                        max,
                                        step,
                                    });
                                    this.slide(event.position.x, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, _, cx| {
                                    let next = match event.keystroke.key.as_str() {
                                        "left" | "down" => current - step,
                                        "right" | "up" => current + step,
                                        "home" => min,
                                        "end" => max,
                                        _ => return,
                                    };
                                    this.dispatch(&keyboard, json!(next.clamp(min, max)), cx);
                                    cx.stop_propagation();
                                },
                            )),
                    )
                    .into_any_element()
            }
            Node::Tabs {
                labels,
                selected,
                handler,
            } => {
                let mut tabs = div().flex().flex_wrap().gap_1();
                for (i, label) in labels.iter().enumerate() {
                    let handler = handler.clone();
                    let keyboard = handler.clone();
                    tabs =
                        tabs.child(
                            tab_button(
                                SharedString::from(format!("visual-tab-{key}-{i}")),
                                i == *selected,
                            )
                            .debug_selector({
                                let key = key.clone();
                                move || format!("visual-tab-{key}-{i}")
                            })
                            .aria_label(label.clone())
                            .aria_toggled(if i == *selected {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .child(label.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dispatch(&handler, json!(i), cx)
                            }))
                            .on_key_down(cx.listener(
                                move |this, event, _, cx| {
                                    if activated(event) {
                                        this.dispatch(&keyboard, json!(i), cx);
                                        cx.stop_propagation();
                                    }
                                },
                            )),
                        );
                }
                tabs.into_any_element()
            }
            Node::Input {
                label,
                value,
                placeholder,
                handler,
            } => {
                if !self.inputs.contains_key(&key) {
                    let input = cx.new(|cx| {
                        let mut input = crate::input::Input::field(
                            label.clone(),
                            placeholder.clone().unwrap_or_default(),
                            content(0.4),
                            cx,
                        );
                        input.set_max_chars(4096);
                        input.set_text(value.clone(), cx);
                        input
                    });
                    let changed_key = key.clone();
                    let subscription =
                        cx.subscribe(&input, move |this, input, _: &crate::input::Changed, cx| {
                            let value = input.read(cx).content.to_string();
                            let Some(slot) = this.inputs.get_mut(&changed_key) else {
                                return;
                            };
                            if value == slot.source_value {
                                return;
                            }
                            slot.source_value = value.clone();
                            let handler = slot.handler.clone();
                            this.dispatch(&handler, json!(value), cx);
                        });
                    self.inputs.insert(
                        key.clone(),
                        InputSlot {
                            input,
                            handler: handler.clone(),
                            source_value: value.clone(),
                            _subscription: subscription,
                        },
                    );
                }
                let slot = self.inputs.get_mut(&key).unwrap();
                slot.handler = handler.clone();
                slot.source_value = value.clone();
                if slot.input.read(cx).content.as_ref() != value {
                    slot.input
                        .update(cx, |input, cx| input.set_text(value.clone(), cx));
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.6))
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("visual-input-{key}")))
                            .debug_selector({
                                let key = key.clone();
                                move || format!("visual-input-{key}")
                            })
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.14))
                            .child(slot.input.clone()),
                    )
                    .into_any_element()
            }
            Node::Select {
                label,
                options,
                selected,
                handler,
            } => {
                let mut choices = div().flex().flex_wrap().gap_1();
                for (i, option) in options.iter().enumerate() {
                    let handler = handler.clone();
                    let keyboard = handler.clone();
                    choices = choices.child(
                        button(SharedString::from(format!("visual-option-{key}-{i}")))
                            .debug_selector({
                                let key = key.clone();
                                move || format!("visual-option-{key}-{i}")
                            })
                            .aria_label(option.clone())
                            .child(option.clone())
                            .when(i == *selected, |d| {
                                d.bg(content(0.09))
                                    .border_color(accent_color())
                                    .text_color(content(1.))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.dispatch(&handler, json!(i), cx)
                            }))
                            .on_key_down(cx.listener(move |this, event, _, cx| {
                                if activated(event) {
                                    this.dispatch(&keyboard, json!(i), cx);
                                    cx.stop_propagation();
                                }
                            })),
                    );
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.6))
                            .child(label.clone()),
                    )
                    .child(choices)
                    .into_any_element()
            }
            Node::Chart {
                chart_type,
                series,
                labels,
                height,
            } => chart(*chart_type, series.clone(), labels.clone(), *height),
            Node::Canvas {
                width,
                height,
                shapes,
            } => drawing(*width, *height, shapes.clone()),
            Node::Scene {
                width: _,
                height,
                objects,
                camera,
            } => self.scene(key, *height, objects.clone(), camera.clone(), cx),
        }
    }
    fn scene(
        &mut self,
        key: String,
        height: f32,
        objects: Vec<SceneObject>,
        camera: Camera,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let orbit = *self.orbits.entry(key.clone()).or_default();
        let wheel_key = key.clone();
        let reset_key = key.clone();
        let keyboard_key = key.clone();
        let reset_keyboard = key.clone();
        let cache = self.scene_cache.entry(key.clone()).or_default().clone();
        let revision = self.revision;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .child(
                div()
                    .id(SharedString::from(format!("visual-scene-{key}")))
                    .debug_selector({
                        let key = key.clone();
                        move || format!("visual-scene-{key}")
                    })
                    .w_full()
                    .focusable()
                    .tab_stop(true)
                    .aria_label(
                        "3D scene; arrow keys orbit, plus and minus zoom, zero resets camera",
                    )
                    .h(px(height))
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(content(0.08))
                    .cursor(gpui::CursorStyle::OpenHand)
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let dimensions =
                                    [bounds.size.width.as_f32(), bounds.size.height.as_f32()];
                                let mut cached = cache.borrow_mut();
                                if cached.as_ref().is_none_or(|c| {
                                    c.revision != revision
                                        || c.dimensions != dimensions
                                        || c.orbit != orbit
                                }) {
                                    *cached = Some(SceneCache {
                                        revision,
                                        dimensions,
                                        orbit,
                                        projection: Rc::new(scene::project(
                                            &objects,
                                            &camera,
                                            orbit,
                                            dimensions[0],
                                            dimensions[1],
                                        )),
                                    });
                                }
                                let projection = cached.as_ref().unwrap().projection.clone();
                                drop(cached);
                                window.with_content_mask(
                                    Some(gpui::ContentMask { bounds }),
                                    |window| {
                                        for surface in &projection.order {
                                            match surface {
                                                scene::Surface::Face(index) => {
                                                    let face = &projection.faces[*index];
                                                    let mut fill = color(&face.color);
                                                    fill.l *= face.light;
                                                    paint_path(
                                                        &face.points,
                                                        bounds.origin,
                                                        1.,
                                                        0.,
                                                        true,
                                                        fill,
                                                        window,
                                                    );
                                                }
                                                scene::Surface::Stroke(index) => {
                                                    let stroke = &projection.strokes[*index];
                                                    paint_path(
                                                        &stroke.points,
                                                        bounds.origin,
                                                        1.,
                                                        stroke.width,
                                                        false,
                                                        color(&stroke.color),
                                                        window,
                                                    );
                                                }
                                            }
                                        }
                                    },
                                );
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            this.scene_drag = Some((key.clone(), event.position));
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                            // Modifier zoom leaves ordinary transcript scrolling available.
                            if event.modifiers.control || event.modifiers.platform {
                                let orbit = this.orbits.entry(wheel_key.clone()).or_default();
                                orbit.zoom = (orbit.zoom
                                    * (event.delta.pixel_delta(px(16.)).y.as_f32() * 0.008).exp())
                                .clamp(0.15, 8.);
                                cx.notify();
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        let orbit = this.orbits.entry(keyboard_key.clone()).or_default();
                        match event.keystroke.key.as_str() {
                            "left" => orbit.yaw -= 0.12,
                            "right" => orbit.yaw += 0.12,
                            "up" => orbit.pitch = (orbit.pitch - 0.12).max(-1.45),
                            "down" => orbit.pitch = (orbit.pitch + 0.12).min(1.45),
                            "+" | "=" => orbit.zoom = (orbit.zoom / 1.2).max(0.15),
                            "-" => orbit.zoom = (orbit.zoom * 1.2).min(8.),
                            "0" => *orbit = scene::Orbit::default(),
                            _ => return,
                        }
                        cx.notify();
                        cx.stop_propagation();
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .child(
                        div()
                            .flex_1()
                            .child("Drag to orbit · ⌘/Ctrl + scroll to zoom"),
                    )
                    .child(
                        button(SharedString::from(format!(
                            "visual-camera-reset-{reset_key}"
                        )))
                        .aria_label("Reset scene camera")
                        .child("Reset camera")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.orbits.remove(&reset_key);
                            cx.notify();
                        }))
                        .on_key_down(cx.listener(
                            move |this, event, _, cx| {
                                if activated(event) {
                                    this.orbits.remove(&reset_keyboard);
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
            )
            .into_any_element()
    }
}

impl Render for VisualView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.document.title.clone();
        let tree = self.tree.clone();
        let width = self.width.clone();
        let entity = cx.entity().downgrade();
        self.active.clear();
        let body = if self.show_source {
            self.source
                .get_or_insert_with(|| {
                    cx.new(|cx| source::SourceView::new(self.document.source.clone(), cx))
                })
                .clone()
                .into_any_element()
        } else if let Some(tree) = tree {
            self.node(&tree, "root".into(), cx)
        } else {
            div().into_any_element()
        };
        if !self.show_source {
            self.inputs.retain(|key, _| self.active.contains(key));
            self.tracks.retain(|key, _| self.active.contains(key));
            self.orbits.retain(|key, _| self.active.contains(key));
            self.scene_cache.retain(|key, _| self.active.contains(key));
        }
        div()
            .id("native-session-visual")
            .debug_selector(|| "native-session-visual".into())
            .relative()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_3()
            .text_size(px(13.))
            .text_color(content(1.))
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let old = (width.get() / 180.).floor();
                        width.set(bounds.size.width.as_f32());
                        if old != (width.get() / 180.).floor() {
                            let _ = entity.update(cx, |this, cx| this.changed(cx));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .w_full()
                .h(px(1.)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(80.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        action_button(
                            "visual-source",
                            if self.show_source {
                                "Show visual"
                            } else {
                                "Show source"
                            },
                            if self.show_source { "eye" } else { "code" },
                            self.show_source,
                        )
                        .debug_selector(|| "visual-source".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_source = !this.show_source;
                            this.changed(cx);
                        }))
                        .on_key_down(cx.listener(|this, event, _, cx| {
                            if activated(event) {
                                this.show_source = !this.show_source;
                                this.changed(cx);
                                cx.stop_propagation();
                            }
                        })),
                    )
                    .when(self.show_source, |d| {
                        d.child(
                            action_button("visual-copy", "Copy source", "copy", false)
                                .debug_selector(|| "visual-copy".into())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        this.document.source.clone(),
                                    ))
                                }))
                                .on_key_down(cx.listener(|this, event, _, cx| {
                                    if activated(event) {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            this.document.source.clone(),
                                        ));
                                        cx.stop_propagation();
                                    }
                                })),
                        )
                    })
                    .child(
                        action_button("visual-reset", "Reset visual", "rotate-ccw", false)
                            .debug_selector(|| "visual-reset".into())
                            .on_click(cx.listener(|this, _, _, cx| this.reset(cx)))
                            .on_key_down(cx.listener(|this, event, _, cx| {
                                if activated(event) {
                                    this.reset(cx);
                                    cx.stop_propagation();
                                }
                            })),
                    ),
            )
            .children(self.error.iter().map(|error| {
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(color(&ThemeColor::Negative))
                    .text_size(px(12.))
                    .text_color(color(&ThemeColor::Negative))
                    .child(format!("Visual could not update: {error}"))
            }))
            .child(
                div()
                    .id("visual-content")
                    .debug_selector(|| "visual-content".into())
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .child(body),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if !event.dragging() {
                    this.slider_drag = None;
                    this.scene_drag = None;
                    return;
                }
                if this.slider_drag.is_some() {
                    this.slide(event.position.x, cx);
                    cx.stop_propagation();
                }
                if let Some((key, last)) = &mut this.scene_drag {
                    let delta = event.position - *last;
                    *last = event.position;
                    let orbit = this.orbits.entry(key.clone()).or_default();
                    orbit.yaw += delta.x.as_f32() * 0.01;
                    orbit.pitch = (orbit.pitch + delta.y.as_f32() * 0.01).clamp(-1.45, 1.45);
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.slider_drag = None;
                    this.scene_drag = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.slider_drag = None;
                    this.scene_drag = None;
                }),
            )
    }
}

fn node_at<'a>(mut node: &'a Node, key: &str) -> Option<&'a Node> {
    let mut parts = key.split('/');
    if parts.next() != Some("root") {
        return None;
    }
    for part in parts {
        let index = part.parse::<usize>().ok()?;
        node = match node {
            Node::Column { children, .. }
            | Node::Row { children, .. }
            | Node::Grid { children, .. } => children.get(index)?,
            _ => return None,
        };
    }
    Some(node)
}

fn format_number(value: f64) -> String {
    if value.abs() >= 1_000_000. {
        format!("{:.1}m", value / 1_000_000.)
    } else if value.abs() >= 10_000. {
        format!("{:.1}k", value / 1_000.)
    } else if value.fract().abs() < 0.00001 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}
fn paint_path(
    points: &[[f32; 2]],
    origin: Point<Pixels>,
    scale: f32,
    width: f32,
    closed: bool,
    color: Hsla,
    window: &mut Window,
) {
    if points.is_empty() {
        return;
    }
    let mut path = if closed {
        PathBuilder::fill()
    } else {
        PathBuilder::stroke(px(width))
    };
    for (i, p) in points.iter().enumerate() {
        let p = origin + point(px(p[0] * scale), px(p[1] * scale));
        if i == 0 {
            path.move_to(p);
        } else {
            path.line_to(p);
        }
    }
    if closed {
        path.close();
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}
fn paint_label(
    text: String,
    at: Point<Pixels>,
    token: &ThemeColor,
    font_size: f32,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let style = window.text_style();
    let run = gpui::TextRun {
        len: text.len(),
        font: style.font(),
        color: color(token),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.into(), px(font_size), &[run], None);
    let _ = line.paint(
        at,
        px(font_size * 1.4),
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
}
fn drawing(width: f32, height: f32, shapes: Vec<Shape>) -> AnyElement {
    div()
        .w_full()
        .aspect_ratio(width / height)
        .max_h(px(height))
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let scale = (bounds.size.width.as_f32() / width)
                        .min(bounds.size.height.as_f32() / height);
                    let origin = bounds.origin
                        + point(
                            px((bounds.size.width.as_f32() - width * scale) / 2.),
                            px(0.),
                        );
                    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                        for shape in &shapes {
                            match shape {
                                Shape::Rect {
                                    x,
                                    y,
                                    width,
                                    height,
                                    color: token,
                                } => window.paint_quad(fill(
                                    Bounds::new(
                                        origin + point(px(x * scale), px(y * scale)),
                                        size(px(width * scale), px(height * scale)),
                                    ),
                                    color(token),
                                )),
                                Shape::Circle {
                                    x,
                                    y,
                                    radius,
                                    color: token,
                                } => {
                                    let points = (0..48)
                                        .map(|i| {
                                            let a = i as f32 * std::f32::consts::TAU / 48.;
                                            [x + radius * a.cos(), y + radius * a.sin()]
                                        })
                                        .collect::<Vec<_>>();
                                    paint_path(
                                        &points,
                                        origin,
                                        scale,
                                        0.,
                                        true,
                                        color(token),
                                        window,
                                    );
                                }
                                Shape::Line {
                                    x1,
                                    y1,
                                    x2,
                                    y2,
                                    width,
                                    color: token,
                                } => paint_path(
                                    &[[*x1, *y1], [*x2, *y2]],
                                    origin,
                                    scale,
                                    width * scale,
                                    false,
                                    color(token),
                                    window,
                                ),
                                Shape::Polyline {
                                    points,
                                    width,
                                    color: token,
                                } => paint_path(
                                    points,
                                    origin,
                                    scale,
                                    width * scale,
                                    false,
                                    color(token),
                                    window,
                                ),
                                Shape::Label {
                                    x,
                                    y,
                                    text,
                                    color: token,
                                } => paint_label(
                                    text.clone(),
                                    origin + point(px(x * scale), px(y * scale)),
                                    token,
                                    12. * scale.max(0.7),
                                    window,
                                    cx,
                                ),
                            }
                        }
                    });
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn chart(kind: ChartKind, series: Vec<Series>, labels: Vec<String>, height: f32) -> AnyElement {
    let legend = div()
        .flex()
        .flex_wrap()
        .gap_3()
        .children(series.iter().map(|s| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(11.))
                .text_color(content(0.6))
                .child(div().size(px(7.)).rounded_full().bg(color(&s.color)))
                .child(s.label.clone())
        }));
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let mut low = 0f64;
                    let mut high = 0f64;
                    let count = series
                        .iter()
                        .map(|s| s.values.len())
                        .max()
                        .unwrap_or(0)
                        .max(labels.len())
                        .max(1);
                    for value in series.iter().flat_map(|s| &s.values) {
                        low = low.min(*value);
                        high = high.max(*value);
                    }
                    if (high - low).abs() < f64::EPSILON {
                        high = low + 1.;
                    }
                    let w = (bounds.size.width.as_f32() - 52.).max(1.);
                    let h = (bounds.size.height.as_f32() - 30.).max(1.);
                    let origin = bounds.origin + point(px(44.), px(4.));
                    let y = |v: f64| ((high - v) / (high - low)) as f32 * h;
                    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                        for i in 0..=4 {
                            let value = low + (high - low) * i as f64 / 4.;
                            let yy = y(value);
                            paint_path(
                                &[[0., yy], [w, yy]],
                                origin,
                                1.,
                                1.,
                                false,
                                content(0.07),
                                window,
                            );
                            paint_label(
                                format_number(value),
                                bounds.origin + point(px(0.), px(yy - 3.)),
                                &ThemeColor::Muted,
                                10.,
                                window,
                                cx,
                            );
                        }
                        for (index, s) in series.iter().enumerate() {
                            let points = s
                                .values
                                .iter()
                                .enumerate()
                                .map(|(i, v)| {
                                    let x = if matches!(kind, ChartKind::Bar) {
                                        (i as f32 + 0.5) * w / count as f32
                                    } else {
                                        if count <= 1 {
                                            w / 2.
                                        } else {
                                            i as f32 * w / (count - 1) as f32
                                        }
                                    };
                                    [x, y(*v)]
                                })
                                .collect::<Vec<_>>();
                            if matches!(kind, ChartKind::Bar) {
                                let group = w / count as f32;
                                let bar = group * 0.75 / series.len().max(1) as f32;
                                for (p, value) in points.iter().zip(&s.values) {
                                    let xx = p[0] - group * 0.375 + index as f32 * bar;
                                    let yy = y(*value).min(y(0.));
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            origin + point(px(xx), px(yy)),
                                            size(
                                                px((bar - 2.).max(1.)),
                                                px((y(*value) - y(0.)).abs().max(1.)),
                                            ),
                                        ),
                                        color(&s.color),
                                    ));
                                }
                            } else {
                                if matches!(kind, ChartKind::Area) && !points.is_empty() {
                                    let mut area = points.clone();
                                    area.push([points.last().unwrap()[0], y(0.)]);
                                    area.push([points[0][0], y(0.)]);
                                    let mut tint = color(&s.color);
                                    tint.a = 0.14;
                                    paint_path(&area, origin, 1., 0., true, tint, window);
                                }
                                paint_path(&points, origin, 1., 2., false, color(&s.color), window);
                                if points.len() <= 32 {
                                    for p in &points {
                                        window.paint_quad(
                                            fill(
                                                Bounds::new(
                                                    origin + point(px(p[0] - 2.5), px(p[1] - 2.5)),
                                                    size(px(5.), px(5.)),
                                                ),
                                                color(&s.color),
                                            )
                                            .corner_radii(px(3.)),
                                        );
                                    }
                                }
                            }
                        }
                        let stride = (count as f32 / (w / 70.).max(1.)).ceil() as usize;
                        for (i, label) in labels.iter().enumerate().step_by(stride.max(1)) {
                            let x = if matches!(kind, ChartKind::Bar) {
                                (i as f32 + 0.5) * w / count as f32
                            } else {
                                if count <= 1 {
                                    w / 2.
                                } else {
                                    i as f32 * w / (count - 1) as f32
                                }
                            };
                            let label = if label.chars().count() > 14 {
                                format!("{}…", label.chars().take(13).collect::<String>())
                            } else {
                                label.clone()
                            };
                            paint_label(
                                label,
                                origin + point(px((x - 12.).max(0.)), px(h + 8.)),
                                &ThemeColor::Muted,
                                10.,
                                window,
                                cx,
                            );
                        }
                    });
                },
            )
            .w_full()
            .h(px(height)),
        )
        .child(legend)
        .into_any_element()
}
