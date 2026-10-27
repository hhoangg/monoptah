//! The "How the score is calculated" sidebar (`FormulaSidebar.tsx`): the
//! viewer's breakdown, the formula in words, and the weights editor. Every
//! number in the copy is read from the scoring constants (`SCORING`), so the
//! text cannot drift from the scores. The copy and the editor's rules are pure
//! functions (unit tested); `FormulaSidebar` only draws them.
//!
//! The sidebar reads the current model and lock from the `GithubView` on every
//! frame, so a saved weight shows up in the breakdown at once.
use super::{github::GithubView, ui};
use crate::{
    input::{self, Input},
    right_panel::tabular,
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, Entity, FocusHandle, Focusable, HighlightStyle, IntoElement, Render,
    Role, SharedString, Stateful, StyledText, Subscription, WeakEntity, Window, div, prelude::*,
    px, relative, rgb,
};
use monocode_core::contributors::model::{
    config::{CONFIG_PATH, DEFAULT_CONFIG},
    format::format_score,
    js_number_text,
    leaderboard::LeaderboardRow,
    palette::color_for,
    period::{Period, PeriodKind},
    pipeline::ContributorsModel,
    scoring::{Parts, SCORING},
    snapshot::SnapshotScope,
    types::{ContributorsConfig, Weights},
};
use std::{ops::Range, sync::Arc};

pub(super) const TITLE: &str = "How the score is calculated";
pub(super) const INTRO: &str = "Scores reward impact, not busyness. Every part grows slowly (log scale) and has a cap, so splitting work into tiny commits or padding lines does not pay.";
pub(super) const CALLOUT: &str = "This board is for motivation, not performance review. Roles differ: someone who reviews and mentors a lot may ship less code.";
pub(super) const MIN_WEIGHT: f64 = 0.;
pub(super) const MAX_WEIGHT: f64 = 5.;

// ---- the weights ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WeightKey {
    Delivery,
    Care,
    Review,
    Consistency,
    Breadth,
}
impl WeightKey {
    /// The order of the editor and the breakdown.
    pub(super) const ALL: [WeightKey; 5] = [
        Self::Delivery,
        Self::Care,
        Self::Review,
        Self::Consistency,
        Self::Breadth,
    ];
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Delivery => "delivery",
            Self::Care => "care",
            Self::Review => "review",
            Self::Consistency => "consistency",
            Self::Breadth => "breadth",
        }
    }
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Delivery => "Delivery",
            Self::Care => "Care",
            Self::Review => "Review",
            Self::Consistency => "Consistency",
            Self::Breadth => "Breadth",
        }
    }
    pub(super) fn weight(self, weights: &Weights) -> f64 {
        match self {
            Self::Delivery => weights.delivery,
            Self::Care => weights.care,
            Self::Review => weights.review,
            Self::Consistency => weights.consistency,
            Self::Breadth => weights.breadth,
        }
    }
    fn set(self, weights: &mut Weights, value: f64) {
        match self {
            Self::Delivery => weights.delivery = value,
            Self::Care => weights.care = value,
            Self::Review => weights.review = value,
            Self::Consistency => weights.consistency = value,
            Self::Breadth => weights.breadth = value,
        }
    }
    pub(super) fn part(self, parts: &Parts) -> f64 {
        match self {
            Self::Delivery => parts.delivery,
            Self::Care => parts.care,
            Self::Review => parts.review,
            Self::Consistency => parts.consistency,
            Self::Breadth => parts.breadth,
        }
    }
    /// The bar colours of the breakdown.
    pub(super) fn bar_color(self) -> &'static str {
        color_for(match self {
            Self::Delivery => 0,
            Self::Care => 2,
            Self::Review => 1,
            Self::Consistency => 4,
            Self::Breadth => 5,
        })
    }
}

/// A typed weight kept inside the allowed range and rounded to two decimals;
/// an empty or invalid entry keeps `fallback`.
pub(super) fn clamp_weight(text: &str, fallback: f64) -> f64 {
    let value = match text.trim().parse::<f64>() {
        Ok(value) if value.is_finite() => value,
        _ => return fallback,
    };
    (value.clamp(MIN_WEIGHT, MAX_WEIGHT) * 100.).round() / 100.
}

/// The five typed texts (in `WeightKey::ALL` order) as weights.
pub(super) fn typed_weights(texts: &[String], current: &Weights) -> Weights {
    let mut next = current.clone();
    for (key, text) in WeightKey::ALL.into_iter().zip(texts) {
        key.set(&mut next, clamp_weight(text, key.weight(current)));
    }
    next
}

pub(super) fn weight_texts(weights: &Weights) -> Vec<String> {
    WeightKey::ALL
        .into_iter()
        .map(|key| js_number_text(key.weight(weights)))
        .collect()
}

/// "Custom weights are active: Consistency ×2." or nothing when all are 1.
pub(super) fn custom_weights_text(weights: &Weights) -> Option<String> {
    let custom: Vec<String> = WeightKey::ALL
        .into_iter()
        .filter(|key| key.weight(weights) != 1.)
        .map(|key| format!("{} ×{}", key.label(), js_number_text(key.weight(weights))))
        .collect();
    (!custom.is_empty()).then(|| format!("Custom weights are active: {}.", custom.join(", ")))
}

pub(super) fn editor_hint() -> String {
    format!(
        "Each dimension is multiplied by its weight, from {} to {}. The default is 1.",
        js_number_text(MIN_WEIGHT),
        js_number_text(MAX_WEIGHT)
    )
}

pub(super) fn saved_in() -> String {
    format!("Saved in {CONFIG_PATH}")
}

// ---- the viewer's score --------------------------------------------------------------

/// "in the last 7 days", "of all time", "in Oct 2026".
pub(super) fn period_word(period: &Period) -> String {
    match period.kind {
        PeriodKind::Days7 => "in the last 7 days".into(),
        PeriodKind::All => "of all time".into(),
        _ => format!("in {}", period.label),
    }
}

pub(super) fn viewer(model: &ContributorsModel) -> Option<&LeaderboardRow> {
    model.leaderboard.rows.iter().find(|row| row.is_me)
}

/// "Your score in Oct 2026: 123", or without the number when the viewer has none.
pub(super) fn score_title(model: &ContributorsModel) -> String {
    let word = period_word(&model.period);
    match viewer(model) {
        Some(me) => format!("Your score {word}: {}", format_score(me.score)),
        None => format!("Your score {word}"),
    }
}

/// "No activity for you in all time yet" ("in all time", not "of all time").
pub(super) fn quiet_text(model: &ContributorsModel) -> String {
    let word = if model.period.kind == PeriodKind::All {
        "in all time".to_string()
    } else {
        period_word(&model.period)
    };
    format!("No activity for you {word} yet")
}

fn pct(fraction: f64) -> String {
    format!("{}%", (fraction * 100.).round() as i64)
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct BarRow {
    pub key: WeightKey,
    /// "×2" when the weight is not 1.
    pub weight: Option<String>,
    pub value: String,
    /// Share of the largest part, 0 to 1.
    pub fraction: f64,
}

/// The five parts with the quality chip that sits between Delivery and Care.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Breakdown {
    pub bars: Vec<BarRow>,
    /// "×1.14".
    pub quality_chip: String,
    /// "(survival 85% at 30 days)".
    pub quality_note: String,
}

pub(super) fn breakdown(row: &LeaderboardRow, weights: &Weights) -> Breakdown {
    let parts = &row.parts;
    let max = WeightKey::ALL
        .into_iter()
        .map(|key| key.part(parts))
        .fold(1., f64::max);
    let survival = match row.survival {
        None => "no 30-day data yet".to_string(),
        Some(s) => format!("survival {} at 30 days", pct(s)),
    };
    Breakdown {
        bars: WeightKey::ALL
            .into_iter()
            .map(|key| {
                let weight = key.weight(weights);
                BarRow {
                    key,
                    weight: (weight != 1.).then(|| format!("×{}", js_number_text(weight))),
                    value: format_score(key.part(parts)),
                    fraction: key.part(parts) / max,
                }
            })
            .collect(),
        quality_chip: format!("×{:.2}", row.quality),
        quality_note: format!("({survival})"),
    }
}

// ---- the copy ------------------------------------------------------------------------

/// Text with bold runs (`<b>` of the TS copy).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Rich {
    pub text: String,
    pub bold: Vec<Range<usize>>,
}
impl Rich {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: vec![],
        }
    }
    /// Parts in order; the `true` ones are bold.
    fn mixed(parts: &[(&str, bool)]) -> Self {
        let mut text = String::new();
        let mut bold = vec![];
        for (part, is_bold) in parts {
            let start = text.len();
            text.push_str(part);
            if *is_bold {
                bold.push(start..text.len());
            }
        }
        Self { text, bold }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Block {
    Text(Rich),
    /// A preformatted block; each line stays whole and scrolls sideways.
    Formula(String),
    Bullets(Vec<Rich>),
    /// An amber status line.
    Warning(String),
    Globs(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct SectionCopy {
    pub id: &'static str,
    pub title: String,
    pub blocks: Vec<Block>,
}

fn text(s: impl Into<String>) -> Block {
    Block::Text(Rich::plain(s))
}

fn n(value: f64) -> String {
    js_number_text(value)
}

/// The sections under "Your score", copy generated from the scoring constants.
pub(super) fn sections(model: &ContributorsModel) -> Vec<SectionCopy> {
    let s = SCORING;
    let raw = &model.raw;
    let reason = raw
        .gh_note
        .as_deref()
        .map(|note| {
            note.trim()
                .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        })
        .filter(|note| !note.is_empty());
    let mut review = vec![text(format!(
        "{} points per PR you review for someone else, +{} per meaningful comment (max {} per PR). Reviewing your own PR earns nothing.",
        n(s.review_pr),
        n(s.review_comment),
        n(s.review_comment_cap)
    ))];
    if !raw.gh_available {
        review.push(Block::Warning(format!(
            "Review points are not counted because GitHub data is unavailable{}.",
            reason.map(|r| format!(": {r}")).unwrap_or_default()
        )));
    }
    let ignored = if model.config.ignore.is_empty() {
        vec![text("No file patterns are ignored for this project.")]
    } else {
        vec![
            text("File patterns ignored for this project:"),
            Block::Globs(model.config.ignore.clone()),
        ]
    };
    let section = |id, title: &str, blocks| SectionCopy {
        id,
        title: title.to_string(),
        blocks,
    };
    vec![
        section(
            "formula",
            "The formula",
            vec![Block::Formula(
                "Score = Delivery × Quality + Care\n      + Review + Consistency + Breadth".into(),
            )],
        ),
        section(
            "delivery",
            "Delivery: merged pull requests",
            vec![
                text("Each merged PR is one unit, whether it was squashed, rebased or merged. Points come from the final diff that landed, plus how much iteration it took."),
                Block::Formula(
                    [
                        "size      = log2(1 + added + 0.5 × deleted)".to_string(),
                        format!(
                            "iteration = {} × log2(1 + commits in PR)  (max {})",
                            n(s.iter_k),
                            n(s.iter_cap)
                        ),
                        format!(
                            "PR points = {} + {} × size + iteration     (max {})",
                            n(s.pr_base),
                            n(s.size_k),
                            n(s.pr_cap)
                        ),
                    ]
                    .join("\n"),
                ),
                Block::Bullets(vec![
                    Rich::mixed(&[
                        ("Squash-merged PRs keep their effort.", true),
                        (" The commits on the branch count as iteration, and every day you committed on the branch counts as an active day.", false),
                    ]),
                    Rich::plain("Closed PRs that were never merged earn no Delivery points, but their days still count toward Consistency."),
                ]),
            ],
        ),
        section(
            "quality",
            &format!("Quality: ×{} to ×{}", n(s.quality_min), n(s.quality_max)),
            vec![
                text("Based on how much of your code is still alive 30 days later. A reverted PR loses its Delivery points, with no extra penalty."),
                Block::Formula(format!(
                    "quality = {} + {} × survival",
                    n(s.quality_base),
                    n(s.quality_slope)
                )),
            ],
        ),
        section(
            "care",
            "Care: bonus on a PR",
            vec![Block::Bullets(vec![
                Rich::plain(format!("+{} if it adds or updates tests", pct(s.care_test))),
                Rich::plain(format!("+{} if it updates docs", pct(s.care_docs))),
                Rich::plain(format!(
                    "+{} if it deletes more than it adds",
                    pct(s.care_net_negative)
                )),
                Rich::plain(format!("+{} if it fixes a bug", pct(s.care_fix))),
            ])],
        ),
        section("review", "Review", review),
        section(
            "consistency",
            "Consistency",
            vec![
                text(format!(
                    "{} points per active day, plus {} per day of streak (max {}). Weekends never break a streak.",
                    n(s.active_day),
                    n(s.streak_k),
                    n(s.streak_cap)
                )),
                text("Consistency grows with the length of the period; Breadth is capped, so in long periods it weighs less."),
            ],
        ),
        section(
            "breadth",
            "Breadth",
            vec![text(format!(
                "{} points per module where you changed at least {} lines (max {} modules).",
                n(s.module_pts),
                n(s.module_min_eff),
                n(s.module_cap)
            ))],
        ),
        section(
            "ai",
            "AI agents",
            vec![Block::Text(Rich::mixed(&[
                ("Driving an agent is real work. Commits co-authored by Claude, Codex, Copilot or any other agent count ", false),
                ("100%", true),
                (" for the human. If an agent is the commit author, the points go to the PR author or the committer. There is no AI bonus or penalty, but Quality applies to all code alike. \"AI-assisted %\" is shown for information only.", false),
            ]))],
        ),
        section(
            "ignored",
            "What is ignored",
            [
                vec![text("Merge commits, lockfiles, generated files, whitespace-only changes, renames, duplicate cherry-picks, and bots such as dependabot or CI runners.")],
                ignored,
            ]
            .concat(),
        ),
    ]
}

// ---- the sidebar -----------------------------------------------------------------------

/// Draws over the right 400px of the Contributors body (`GithubView::set_sidebar`).
pub(crate) struct FormulaSidebar {
    github: WeakEntity<GithubView>,
    focus: FocusHandle,
    /// The link that opened it: closing hands the focus back.
    return_focus: FocusHandle,
    editing: bool,
    /// One field per weight, made when the editor opens (its texts start from the
    /// saved weights, like the TS editor that mounts with `config.weights`).
    fields: Vec<Entity<Input>>,
    _field_subscriptions: Vec<Subscription>,
}

impl Focusable for FormulaSidebar {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FormulaSidebar {
    pub(super) fn new(
        github: WeakEntity<GithubView>,
        return_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            github,
            focus: cx.focus_handle(),
            return_focus,
            editing: false,
            fields: Vec::new(),
            _field_subscriptions: Vec::new(),
        }
    }

    pub(super) fn panel_focus(&self) -> &FocusHandle {
        &self.focus
    }

    #[cfg(test)]
    pub(super) fn is_editing(&self) -> bool {
        self.editing
    }

    #[cfg(test)]
    pub(super) fn field(&self, key: WeightKey) -> Option<Entity<Input>> {
        let at = WeightKey::ALL.iter().position(|k| *k == key)?;
        self.fields.get(at).cloned()
    }

    fn model_and_lock(&self, cx: &gpui::App) -> Option<(Arc<ContributorsModel>, bool)> {
        let github = self.github.upgrade()?;
        let github = github.read(cx);
        Some((github.model.clone()?, github.locked(cx)))
    }

    fn typed(&self, current: &Weights, cx: &gpui::App) -> Weights {
        let texts: Vec<String> = self
            .fields
            .iter()
            .map(|field| field.read(cx).content.to_string())
            .collect();
        typed_weights(&texts, current)
    }

    pub(super) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(github) = self.github.upgrade() {
            github.update(cx, |github, cx| github.set_sidebar(None, cx));
        }
        window.focus(&self.return_focus, cx);
    }

    fn open_editor(&mut self, weights: &Weights, cx: &mut Context<Self>) {
        self.fields.clear();
        self._field_subscriptions.clear();
        for (key, text) in WeightKey::ALL.into_iter().zip(weight_texts(weights)) {
            let field = cx.new(|cx| {
                let mut input =
                    Input::field(format!("{} weight", key.label()), "", content(0.35), cx);
                input.set_font_size(12.);
                input.set_text(text, cx);
                input
            });
            self._field_subscriptions
                .push(cx.subscribe(&field, |_, _, _: &input::Changed, cx| cx.notify()));
            self._field_subscriptions
                .push(cx.subscribe(&field, |this, _, _: &input::Submitted, cx| this.save(cx)));
            self.fields.push(field);
        }
    }

    fn toggle_editor(&mut self, cx: &mut Context<Self>) {
        let Some((model, locked)) = self.model_and_lock(cx) else {
            return;
        };
        if locked && !self.editing {
            return;
        }
        self.editing = !self.editing;
        if self.editing {
            self.open_editor(&model.config.weights, cx);
        }
        cx.notify();
    }

    /// Saves the typed weights (clamped) with the rest of the config, through
    /// the view's save queue. Nothing happens when nothing changed or the file is locked.
    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        let Some((model, locked)) = self.model_and_lock(cx) else {
            return;
        };
        if locked || !self.editing {
            return;
        }
        let next = self.typed(&model.config.weights, cx);
        if next == model.config.weights {
            return;
        }
        for (field, text) in self.fields.iter().zip(weight_texts(&next)) {
            field.update(cx, |input, cx| input.set_text(text, cx));
        }
        let mut config: ContributorsConfig = (*model.config).clone();
        config.weights = next;
        if let Some(github) = self.github.upgrade() {
            github.update(cx, |github, cx| github.save_config_quietly(config, cx));
        }
        cx.notify();
    }

    /// Puts the defaults in the fields; saving them is a separate click.
    fn reset(&mut self, cx: &mut Context<Self>) {
        let Some((_, locked)) = self.model_and_lock(cx) else {
            return;
        };
        if locked {
            return;
        }
        for (field, text) in self
            .fields
            .iter()
            .zip(weight_texts(&DEFAULT_CONFIG.weights))
        {
            field.update(cx, |input, cx| input.set_text(text, cx));
        }
        cx.notify();
    }

    fn ask_about_me(&mut self, cx: &mut Context<Self>) {
        let Some((model, _)) = self.model_and_lock(cx) else {
            return;
        };
        let Some(me) = viewer(&model) else {
            return;
        };
        let scope = SnapshotScope::Me {
            person_id: me.person_id.clone(),
        };
        if let Some(github) = self.github.upgrade() {
            github.update(cx, |github, cx| {
                github.open_ask(scope, cx);
            });
        }
    }
}

// ---- drawing ---------------------------------------------------------------------------

fn section_title(title: String) -> Div {
    div()
        .mt(px(16.))
        .mb(px(6.))
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(title)
}

fn body_text() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .text_size(px(12.))
        .line_height(px(19.5))
        .text_color(content(0.6))
}

fn rich(value: &Rich) -> StyledText {
    let bold = HighlightStyle {
        font_weight: Some(gpui::FontWeight::BOLD),
        color: Some(content(1.)),
        ..Default::default()
    };
    StyledText::new(value.text.clone())
        .with_highlights(value.bold.iter().map(|range| (range.clone(), bold)))
}

fn accent_button(id: &'static str, label: &'static str, disabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded_md()
        .bg(accent_color())
        .px(px(10.))
        .py(px(4.))
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(rgb(0xffffff))
        .map(|d| {
            if disabled {
                d.opacity(0.4)
            } else {
                d.cursor_pointer().hover(|s| s.opacity(0.9))
            }
        })
}

fn plain_button(id: &'static str, label: &'static str, disabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .role(Role::Button)
        .aria_label(label)
        .rounded_md()
        .border_1()
        .border_color(content(0.1))
        .px(px(10.))
        .py(px(4.))
        .text_size(px(12.))
        .text_color(content(0.7))
        .map(|d| {
            if disabled {
                d.opacity(0.4)
            } else {
                d.cursor_pointer()
                    .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
            }
        })
        .child(label)
}

impl FormulaSidebar {
    fn render_block(&self, block: &Block, index: usize) -> AnyElement {
        match block {
            Block::Text(value) => div().child(rich(value)).into_any_element(),
            Block::Formula(lines) => div()
                .id(("contributors-formula-pre", index))
                .overflow_x_scroll()
                .rounded_md()
                .border_1()
                .border_color(content(0.1))
                .bg(content(0.03))
                .px(px(10.))
                .py(px(8.))
                .font_family(crate::fonts::mono())
                .text_size(px(11.5))
                .line_height(px(17.))
                .text_color(content(1.))
                .whitespace_nowrap()
                .child(lines.clone())
                .into_any_element(),
            Block::Bullets(items) => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(items.iter().map(|item| {
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(div().flex_shrink_0().child("•"))
                        .child(div().min_w_0().flex_1().child(rich(item)))
                }))
                .into_any_element(),
            Block::Warning(message) => div()
                .id(("contributors-formula-warning", index))
                .role(Role::Status)
                .text_color(if is_light() {
                    rgb(0xb45309).into()
                } else {
                    ui::amber(1.)
                })
                .child(message.clone())
                .into_any_element(),
            Block::Globs(globs) => div()
                .debug_selector(|| "contributors-formula-globs".into())
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .font_family(crate::fonts::mono())
                .text_size(px(11.))
                .children(globs.iter().map(|glob| {
                    div()
                        .rounded(px(4.))
                        .border_1()
                        .border_color(content(0.1))
                        .bg(content(0.03))
                        .px(px(6.))
                        .py(px(2.))
                        .text_color(content(0.7))
                        .child(glob.clone())
                }))
                .into_any_element(),
        }
    }

    fn render_breakdown(&self, row: &LeaderboardRow, weights: &Weights) -> Div {
        let data = breakdown(row, weights);
        let bar = |bar: &BarRow| {
            let key = bar.key;
            div()
                .debug_selector(move || format!("contributors-formula-part-{}", key.id()))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .text_color(content(1.))
                .child(
                    div()
                        .w(px(90.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .child(key.label())
                        .children(bar.weight.clone().map(|weight| {
                            div()
                                .ml(px(4.))
                                .text_size(px(11.))
                                .text_color(content(0.45))
                                .child(weight)
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .h(px(6.))
                        .overflow_hidden()
                        .rounded_full()
                        .bg(content(0.1))
                        .child(
                            div()
                                .debug_selector(move || {
                                    format!("contributors-formula-bar-{}", key.id())
                                })
                                .h_full()
                                .w(relative(bar.fraction as f32))
                                .rounded_full()
                                .bg(ui::hex_color(key.bar_color())),
                        ),
                )
                .child(
                    div()
                        .w(px(44.))
                        .flex_shrink_0()
                        .flex()
                        .justify_end()
                        .font_features(tabular())
                        .child(bar.value.clone()),
                )
        };
        let quality = div()
            .id("contributors-formula-quality")
            .debug_selector(|| "contributors-formula-part-quality".into())
            .aria_description("Already included in the Delivery number above")
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(12.))
            .text_color(content(1.))
            .child(div().w(px(90.)).flex_shrink_0().child("× Quality"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(content(0.5))
                    .child(
                        div()
                            .rounded_full()
                            .border_1()
                            .border_color(content(0.1))
                            .px(px(6.))
                            .text_size(px(11.))
                            .font_features(tabular())
                            .text_color(content(1.))
                            .child(data.quality_chip.clone()),
                    )
                    .child(data.quality_note.clone()),
            );
        let mut bars = data.bars.iter().map(bar);
        div()
            .mt(px(4.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .children(bars.next())
            .child(quality)
            .children(bars)
    }

    fn render_editor(&mut self, weights: &Weights, locked: bool, cx: &mut Context<Self>) -> Div {
        let typed = self.typed(weights, cx);
        let unchanged = typed == *weights;
        let rows = WeightKey::ALL
            .into_iter()
            .zip(self.fields.clone())
            .map(|(key, field)| {
                field.update(cx, |input, _| input.set_read_only(locked));
                let focus = self.focus.clone();
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(key.label())
                    .child(
                        div()
                            .id(("contributors-formula-field", key as usize))
                            .debug_selector(move || {
                                format!("contributors-formula-weight-{}", key.id())
                            })
                            .w(px(72.))
                            .rounded_md()
                            .border_1()
                            .border_color(content(0.1))
                            .px(px(8.))
                            .py(px(4.))
                            .when(locked, |d| d.opacity(0.4))
                            // Escape in a field only leaves it; the next one closes the sidebar.
                            .on_key_down(cx.listener(
                                move |_, event: &gpui::KeyDownEvent, window, cx| {
                                    if event.keystroke.key == "escape" {
                                        window.focus(&focus, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .child(field),
                    )
            });
        div()
            .debug_selector(|| "contributors-formula-editor".into())
            .mt(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .rounded_md()
            .border_1()
            .border_color(content(0.1))
            .bg(content(0.03))
            .px(px(12.))
            .py(px(10.))
            .text_size(px(12.))
            .child(div().text_color(content(0.6)).child(editor_hint()))
            .child(div().flex().flex_col().gap(px(6.)).children(rows))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        accent_button("contributors-formula-save", "Save", unchanged || locked)
                            .child("Save")
                            .when(!(unchanged || locked), |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.save(cx)))
                            }),
                    )
                    .child(
                        plain_button("contributors-formula-reset", "Reset to defaults", locked)
                            .when(!locked, |d| {
                                d.on_click(cx.listener(|this, _, _, cx| this.reset(cx)))
                            }),
                    ),
            )
    }
}

impl Render for FormulaSidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_focus = self.focus.clone();
        let shell = move |content_: Option<AnyElement>, cx: &mut Context<Self>| {
            div()
                .id("contributors-formula")
                .debug_selector(|| "contributors-formula".into())
                .role(Role::Complementary)
                .aria_label(TITLE)
                .track_focus(&panel_focus)
                .size_full()
                .border_l_1()
                .border_color(content(0.1))
                .bg(floating_fill())
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .children(content_)
        };
        let Some((model, locked)) = self.model_and_lock(cx) else {
            return shell(None, cx);
        };
        let weights = model.config.weights.clone();
        let me = viewer(&model);
        let custom = custom_weights_text(&weights);
        let ask_disabled = me.is_none();
        let editing = self.editing;
        let editor = editing.then(|| self.render_editor(&weights, locked, cx));
        let me_block = match me {
            Some(row) => self.render_breakdown(row, &weights).into_any_element(),
            None => div().child(quiet_text(&model)).into_any_element(),
        };
        let sections = sections(&model);
        let body = div()
            .id("contributors-formula-scroll")
            .size_full()
            .overflow_y_scroll()
            .px(px(16.))
            .py(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(TITLE),
                    )
                    .child(
                        ui::icon_button("contributors-formula-close", "Close", "x", 14., false)
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                    ),
            )
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(12.))
                    .line_height(px(19.5))
                    .text_color(content(0.6))
                    .child(INTRO),
            )
            .child(section_title(score_title(&model)))
            .child(
                body_text()
                    .child(me_block)
                    .children(custom.map(|text| div().child(text))),
            )
            .children(sections.into_iter().map(|section| {
                let id = section.id;
                div()
                    .debug_selector(move || format!("contributors-formula-section-{id}"))
                    .child(section_title(section.title.clone()))
                    .child(
                        body_text().children(
                            section
                                .blocks
                                .iter()
                                .enumerate()
                                .map(|(i, block)| self.render_block(block, i)),
                        ),
                    )
            }))
            .child(
                // A 3px accent strip on the left (`border-l-[3px] border-l-accent`).
                div()
                    .mt(px(16.))
                    .flex()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(content(0.1))
                    .child(
                        div()
                            .debug_selector(|| "contributors-formula-callout-strip".into())
                            .w(px(3.))
                            .flex_shrink_0()
                            .bg(accent_color()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .px(px(10.))
                            .py(px(8.))
                            .text_size(px(12.))
                            .line_height(px(19.5))
                            .text_color(content(0.6))
                            .child(CALLOUT),
                    ),
            )
            .child(
                div()
                    .mt(px(12.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        accent_button(
                            "contributors-formula-ask",
                            "Ask Agent about my score",
                            ask_disabled,
                        )
                        .child(icon("sparkles", 14.).text_color(rgb(0xffffff)))
                        .child("Ask Agent about my score")
                        .when(!ask_disabled, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| this.ask_about_me(cx)))
                        }),
                    )
                    .child(
                        plain_button(
                            "contributors-formula-customize",
                            "Customize weights",
                            locked,
                        )
                        .aria_expanded(editing)
                        .when(!locked, |d| {
                            d.on_click(cx.listener(|this, _, _, cx| this.toggle_editor(cx)))
                        }),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(content(0.45))
                            .child(SharedString::from(saved_in())),
                    ),
            )
            .children(editor);
        shell(Some(body.into_any_element()), cx)
    }
}
