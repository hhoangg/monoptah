//! The Leaderboard section of the GitHub tab (`LeaderboardTab.tsx`): the
//! podium of the top three (2nd, 1st, 3rd on a wide page), the ranking table
//! with movement, the "you" chip, badges and a per-person Ask, and the link
//! that opens the formula sidebar (`formula.rs`). What the rows say is computed
//! by the pure functions below (unit tested); the `GithubView` methods only draw.
use super::{
    formula::FormulaSidebar,
    github::GithubView,
    ui::{self, AvatarSize},
};
use crate::{
    right_panel::{DIFF_ADD, DIFF_DEL, tabular},
    theme::*,
};
use gpui::{
    AnyElement, Context, Div, ElementId, FocusHandle, Role, SharedString, Stateful, Window, canvas,
    div, prelude::*, px, rgb,
};
use monocode_core::contributors::model::{
    format::format_score,
    leaderboard::{Badge, LeaderboardRow},
    palette::color_from_map,
    period::PeriodKind,
    pipeline::ContributorsModel,
    snapshot::SnapshotScope,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

/// The page is wide enough for 2nd, 1st, 3rd side by side from this width
/// (`@min-[560px]/contrib` of the TS podium).
pub(super) const WIDE_AT: f32 = 560.;
const TABLE_MIN: f32 = 900.;

pub(super) const HEADS: [&str; 9] = [
    "Rank",
    "Contributor",
    "Score",
    "Commits",
    "PRs (commits)",
    "AI-assisted",
    "Streak",
    "Badges",
    "Actions",
];
pub(super) const MOTIVATION: &str = "Rankings are for motivation, not performance review.";
pub(super) const ORDINALS: [&str; 3] = ["1st", "2nd", "3rd"];
const HOW: &str = "How is the score calculated?";

// Table columns (the contributor column takes the rest).
const W_RANK: f32 = 84.;
const W_SCORE: f32 = 76.;
const W_COMMITS: f32 = 76.;
const W_PRS: f32 = 112.;
const W_AI: f32 = 104.;
const W_STREAK: f32 = 72.;
const W_BADGES: f32 = 200.;
const W_ASK: f32 = 56.;

// ---- the pure view functions ---------------------------------------------------

pub(super) fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MoveTone {
    Up,
    Down,
    Flat,
}

/// Glyph, tone and explanation of the rank change against the previous period.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct MovementView {
    pub glyph: String,
    pub tone: MoveTone,
    pub title: String,
}

pub(super) fn movement_view(movement: Option<i64>) -> MovementView {
    let flat = |title: &str| MovementView {
        glyph: "·".into(),
        tone: MoveTone::Flat,
        title: title.into(),
    };
    match movement {
        None => flat("Not ranked in the previous period"),
        Some(n) if n > 0 => MovementView {
            glyph: format!("▲{n}"),
            tone: MoveTone::Up,
            title: format!("Up {} vs previous period", plural(n as usize, "place")),
        },
        Some(n) if n < 0 => MovementView {
            glyph: format!("▼{}", -n),
            tone: MoveTone::Down,
            title: format!(
                "Down {} vs previous period",
                plural(n.unsigned_abs() as usize, "place")
            ),
        },
        Some(_) => flat("Same rank as the previous period"),
    }
}

/// False when there is no previous period (All time): no rank movement to show.
pub(super) fn compared(model: &ContributorsModel) -> bool {
    model.previous_period.is_some()
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowCells {
    pub rank: String,
    pub score: String,
    pub commits: String,
    pub prs: String,
    /// "(3)", the commits of the merged PRs.
    pub pr_commits: String,
    pub ai: String,
    pub streak: String,
}

pub(super) fn row_cells(row: &LeaderboardRow) -> RowCells {
    RowCells {
        rank: row.rank.to_string(),
        score: format_score(row.score),
        commits: row.main_commits.to_string(),
        prs: row.merged_prs.to_string(),
        pr_commits: format!("({})", row.branch_commits),
        ai: format!("{}%", row.ai_assisted_pct),
        streak: format!("{}d", row.streak),
    }
}

/// "1 PR (3 commits) · 3d streak".
pub(super) fn podium_line(row: &LeaderboardRow) -> String {
    format!(
        "{} ({}) · {}d streak",
        plural(row.merged_prs, "PR"),
        plural(row.branch_commits as usize, "commit"),
        row.streak
    )
}

/// The grid column of the podium card of this rank on a wide page: 2nd, 1st, 3rd.
pub(super) fn podium_column(rank_index: usize) -> i16 {
    [2, 1, 3][rank_index]
}

#[cfg(test)]
pub(super) fn badge_labels(row: &LeaderboardRow) -> Vec<&'static str> {
    row.badges.iter().map(|b| b.label).collect()
}

/// The asset of a badge's glyph key.
pub(super) fn badge_asset(icon: &str) -> &'static str {
    match icon {
        "fire" => "fire",
        "hammer" => "hammer",
        "broom" => "broom",
        "bug" => "bug",
        "eyes" => "eye",
        _ => "sprout",
    }
}

pub(super) fn period_line(model: &ContributorsModel) -> String {
    format!("period: {}", model.period.label)
}

pub(super) fn empty_title(model: &ContributorsModel) -> String {
    if model.period.kind == PeriodKind::All {
        "No scored activity of all time".into()
    } else {
        format!("No scored activity in {}", model.period.label)
    }
}

pub(super) fn summary_text(model: &ContributorsModel) -> String {
    format!(
        "Score = Delivery × Quality + Care + Review + Consistency + Breadth.{}",
        if model.raw.gh_available {
            ""
        } else {
            " Review is not counted (GitHub data unavailable)."
        }
    )
}

pub(super) fn person_color<'a>(model: &'a ContributorsModel, row: &LeaderboardRow) -> &'a str {
    color_from_map(&model.colors, &row.person_id)
}

// ---- what the section remembers between frames ----------------------------------------

pub(super) struct LeaderboardUi {
    /// The "How is the score calculated?" link: the sidebar hands the focus back to it.
    pub(super) how_focus: FocusHandle,
    /// The podium's width reached `WIDE_AT` at the last paint.
    wide: Rc<Cell<bool>>,
}
impl LeaderboardUi {
    pub(super) fn new(cx: &mut gpui::App) -> Self {
        Self {
            how_focus: cx.focus_handle(),
            wide: Rc::new(Cell::new(true)),
        }
    }
}

// ---- drawing -----------------------------------------------------------------------

fn amber() -> gpui::Hsla {
    let mut c: gpui::Hsla = rgb(0xfbbf24).into();
    c.a = 1.;
    c
}
fn orange() -> gpui::Hsla {
    let mut c: gpui::Hsla = rgb(0xfb923c).into();
    c.a = 0.8;
    c
}
fn medal_tone(index: usize) -> gpui::Hsla {
    [amber(), content(0.6), orange()][index]
}
fn accent_at(alpha: f32) -> gpui::Hsla {
    let mut c = accent_color();
    c.a = alpha;
    c
}
fn move_color(tone: MoveTone) -> gpui::Hsla {
    match tone {
        MoveTone::Up => rgb(DIFF_ADD).into(),
        MoveTone::Down => rgb(DIFF_DEL).into(),
        MoveTone::Flat => content(0.4),
    }
}

fn you_chip(selector: String) -> Div {
    div()
        .debug_selector(move || selector.clone())
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .border_color(accent_at(0.4))
        .px(px(6.))
        .text_size(px(11.))
        .text_color(accent_color())
        .child("you")
}

fn badge_pill(selector: String, badge: &Badge) -> Div {
    div()
        .debug_selector(move || selector.clone())
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded_full()
        .border_1()
        .border_color(content(0.1))
        .px(px(6.))
        .text_size(px(11.))
        .text_color(content(0.6))
        .child(icon(badge_asset(badge.icon), 11.).text_color(content(0.6)))
        .child(badge.label)
}

fn badges(prefix: &'static str, index: usize, row: &LeaderboardRow, center: bool) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(px(4.))
        .when(center, |d| d.justify_center())
        .children(row.badges.iter().map(|badge| {
            badge_pill(
                format!("contributors-lb-{prefix}-{index}-{}", badge.id.as_str()),
                badge,
            )
        }))
}

/// One header cell by column: the last (Actions) has no visible text, only its label.
fn head_cell(index: usize, text: &'static str) -> Stateful<Div> {
    let cell = cell()
        .id(("contributors-lb-head-cell", index))
        .debug_selector(move || format!("contributors-lb-head-{index}"))
        .role(Role::ColumnHeader)
        .aria_label(text)
        .text_color(content(0.5));
    match index {
        0 => cell.w(px(W_RANK)),
        1 => cell.flex_1().min_w(px(180.)).justify_start(),
        2 => cell.w(px(W_SCORE)).justify_end(),
        3 => cell.w(px(W_COMMITS)).justify_end(),
        4 => cell.w(px(W_PRS)).justify_end(),
        5 => cell.w(px(W_AI)).justify_end(),
        6 => cell.w(px(W_STREAK)).justify_end(),
        7 => cell.w(px(W_BADGES)).justify_start(),
        _ => cell.w(px(W_ASK)),
    }
    .when(index < HEADS.len() - 1, |d| d.child(text))
}

fn cell() -> Div {
    div()
        .flex_shrink_0()
        .px(px(12.))
        .py(px(8.))
        .flex()
        .items_center()
}

fn row_div(id: impl Into<ElementId>) -> Stateful<Div> {
    div().id(id)
}

impl GithubView {
    /// The Leaderboard section for the built model.
    pub(super) fn leaderboard_body(
        &mut self,
        model: &Arc<ContributorsModel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let podium = (!model.leaderboard.rows.is_empty()).then(|| self.podium(model, cx));
        let ranking = self.ranking(model, cx);
        div()
            .debug_selector(|| "contributors-body-leaderboard".into())
            .relative()
            .children(podium)
            .child(ranking)
            .into_any_element()
    }

    /// Opens the formula sidebar over the right edge of the body and takes the focus.
    pub(crate) fn open_formula(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.is_some() {
            return;
        }
        let github = cx.weak_entity();
        let return_focus = self.leaderboard_ui.how_focus.clone();
        let sidebar = cx.new(|cx| FormulaSidebar::new(github, return_focus, cx));
        let focus = sidebar.read(cx).panel_focus().clone();
        self.set_sidebar(Some(sidebar.into()), cx);
        window.focus(&focus, cx);
    }

    fn podium(&mut self, model: &Arc<ContributorsModel>, cx: &mut Context<Self>) -> AnyElement {
        let wide = self.leaderboard_ui.wide.get();
        let measured = self.leaderboard_ui.wide.clone();
        let weak = cx.weak_entity();
        // Measures the width the podium has; a change of side of `WIDE_AT` redraws it.
        let probe = canvas(
            move |bounds, window, cx| {
                let now = bounds.size.width >= px(WIDE_AT);
                if measured.get() != now {
                    measured.set(now);
                    window.defer(cx, move |_, cx| {
                        let _ = weak.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let cards = model
            .leaderboard
            .rows
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, row)| {
                podium_card(model, i, row)
                    .when(wide, |d| d.col_start(podium_column(i)).row_start(1))
            });
        div()
            .relative()
            .mb(px(14.))
            .child(probe)
            .child(
                div()
                    .id("contributors-lb-podium")
                    .debug_selector(|| "contributors-lb-podium".into())
                    .role(Role::List)
                    .aria_label("Top three")
                    .gap(px(10.))
                    .map(|d| {
                        if wide {
                            d.grid().grid_cols(3).items_end()
                        } else {
                            d.flex().flex_col()
                        }
                    })
                    .children(cards),
            )
            .into_any_element()
    }

    fn ranking(&mut self, model: &Arc<ContributorsModel>, cx: &mut Context<Self>) -> AnyElement {
        let open = self.sidebar.is_some();
        let how_focus = self.leaderboard_ui.how_focus.clone();
        let table = if model.leaderboard.rows.is_empty() {
            empty_state(model)
        } else {
            self.ranking_table(model, cx)
        };
        div()
            .id("contributors-lb-ranking")
            .role(Role::Region)
            .aria_label("Ranking")
            .rounded_xl()
            .border_1()
            .border_color(content(0.1))
            .child(
                div()
                    .px(px(16.))
                    .pt(px(12.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(12.))
                    .gap_y(px(6.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Ranking"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child(period_line(model)),
                    ),
            )
            .child(
                div()
                    .px(px(16.))
                    .pt(px(2.))
                    .pb(px(8.))
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(MOTIVATION),
            )
            .child(table)
            .child(
                div()
                    .border_t_1()
                    .border_color(content(0.1))
                    .px(px(16.))
                    .py(px(10.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(6.))
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(summary_text(model))
                    .child(
                        div()
                            .id("contributors-lb-how")
                            .debug_selector(|| "contributors-lb-how".into())
                            .track_focus(&how_focus)
                            .role(Role::Button)
                            .aria_label(HOW)
                            .aria_expanded(open)
                            .cursor_pointer()
                            .text_color(accent_color())
                            .hover(|s| s.opacity(0.8))
                            .child(HOW)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_formula(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn ranking_table(
        &mut self,
        model: &Arc<ContributorsModel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compared = compared(model);
        div()
            .id("contributors-lb-table-scroll")
            .role(Role::Region)
            .aria_label("Ranking table")
            .overflow_x_scroll()
            .child(
                div()
                    .id("contributors-lb-table")
                    .debug_selector(|| "contributors-lb-table".into())
                    .role(Role::Table)
                    .min_w(px(TABLE_MIN))
                    .text_size(px(13.))
                    .child(
                        div()
                            .id("contributors-lb-head")
                            .role(Role::Row)
                            .flex()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .children(HEADS.iter().enumerate().map(|(i, text)| head_cell(i, text))),
                    )
                    .children(
                        model
                            .leaderboard
                            .rows
                            .iter()
                            .enumerate()
                            .map(|(i, row)| ranking_row(model, i, row, compared, cx)),
                    ),
            )
            .into_any_element()
    }
}

fn empty_state(model: &ContributorsModel) -> AnyElement {
    div()
        .id("contributors-lb-empty")
        .debug_selector(|| "contributors-lb-empty".into())
        .border_t_1()
        .border_color(content(0.1))
        .px(px(16.))
        .py(px(32.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .text_center()
        .child(icon("users", 20.).text_color(content(0.35)))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(empty_title(model)),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child("Merged pull requests, commits and reviews in the period appear here."),
        )
        .into_any_element()
}

fn podium_card(model: &ContributorsModel, index: usize, row: &LeaderboardRow) -> Stateful<Div> {
    let first = index == 0;
    let tone = medal_tone(index);
    div()
        .id(("contributors-lb-podium-card", index))
        .debug_selector(move || format!("contributors-lb-podium-{index}"))
        .role(Role::ListItem)
        .aria_label(format!(
            "{} {}, score {}",
            ORDINALS[index],
            row.name,
            format_score(row.score)
        ))
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .rounded_xl()
        .border_1()
        .border_color(if first { accent_at(0.6) } else { content(0.1) })
        .bg(content(0.03))
        .px(px(10.))
        .py(px(if first { 20. } else { 14. }))
        .text_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(tone)
                .child(icon("trophy", 20.).text_color(tone))
                .child(ORDINALS[index]),
        )
        .child(ui::avatar(&row.name, person_color(model, row), AvatarSize::Lg).mt(px(8.)))
        .child(
            div()
                .mt(px(6.))
                .max_w_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(row.name.clone()),
                )
                .when(row.is_me, |d| {
                    d.child(you_chip(format!("contributors-lb-podium-you-{index}")))
                }),
        )
        .child(
            div()
                .mt(px(4.))
                .text_size(px(22.))
                .font_weight(gpui::FontWeight::BOLD)
                .font_features(tabular())
                .child(format_score(row.score)),
        )
        .child(
            div()
                .mt(px(4.))
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(podium_line(row)),
        )
        .child(badges("pbadge", index, row, true).mt(px(4.)))
}

fn ranking_row(
    model: &ContributorsModel,
    index: usize,
    row: &LeaderboardRow,
    compared: bool,
    cx: &mut Context<GithubView>,
) -> AnyElement {
    let cells = row_cells(row);
    let group = SharedString::from(format!("contributors-lb-group-{index}"));
    let person = row.person_id.clone();
    let move_view = movement_view(row.movement);
    row_div(("contributors-lb-row", index))
        .debug_selector(move || format!("contributors-lb-row-{index}"))
        .group(group.clone())
        .role(Role::Row)
        .flex()
        .border_t_1()
        .border_color(content(0.1))
        .map(|d| {
            if row.is_me {
                d.bg(accent_at(0.08))
            } else {
                d.hover(|s| s.bg(content(0.03)))
            }
        })
        .child(
            cell()
                .w(px(W_RANK))
                .whitespace_nowrap()
                .font_features(tabular())
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(cells.rank),
                )
                .when(compared, |d| {
                    d.child(
                        div()
                            .id(("contributors-lb-move", index))
                            .debug_selector(move || format!("contributors-lb-move-{index}"))
                            .aria_label(move_view.title.clone())
                            .ml(px(6.))
                            .w(px(28.))
                            .text_size(px(11.))
                            .text_color(move_color(move_view.tone))
                            .child(move_view.glyph.clone()),
                    )
                }),
        )
        .child(
            cell().flex_1().min_w(px(180.)).justify_start().child(
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(ui::avatar(
                        &row.name,
                        person_color(model, row),
                        AvatarSize::Sm,
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(row.name.clone()),
                    )
                    .when(row.is_me, |d| {
                        d.child(you_chip(format!("contributors-lb-you-{index}")))
                    }),
            ),
        )
        .child(
            cell()
                .w(px(W_SCORE))
                .justify_end()
                .font_features(tabular())
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(cells.score),
        )
        .child(
            cell()
                .w(px(W_COMMITS))
                .justify_end()
                .font_features(tabular())
                .child(cells.commits),
        )
        .child(
            cell()
                .w(px(W_PRS))
                .justify_end()
                .gap(px(4.))
                .whitespace_nowrap()
                .font_features(tabular())
                .child(cells.prs)
                .child(div().text_color(content(0.5)).child(cells.pr_commits)),
        )
        .child(
            cell()
                .w(px(W_AI))
                .justify_end()
                .gap(px(4.))
                .whitespace_nowrap()
                .font_features(tabular())
                .text_color(content(0.6))
                .child(icon("bot", 12.).text_color(content(0.6)))
                .child(cells.ai),
        )
        .child(
            cell()
                .w(px(W_STREAK))
                .justify_end()
                .font_features(tabular())
                .child(cells.streak),
        )
        .child(
            cell()
                .w(px(W_BADGES))
                .justify_start()
                .child(badges("badge", index, row, false)),
        )
        .child(
            cell().w(px(W_ASK)).justify_end().child(
                div()
                    .id(("contributors-lb-ask", index))
                    .debug_selector(move || format!("contributors-lb-ask-{index}"))
                    .role(Role::Button)
                    .aria_label(format!("Ask Agent about {}", row.name))
                    .rounded_full()
                    .border_1()
                    .border_color(content(0.1))
                    .px(px(8.))
                    .py(px(2.))
                    .text_size(px(11.))
                    .text_color(accent_color())
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .cursor_pointer()
                    .hover(|s| s.bg(content(0.05)))
                    .child("Ask")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_ask(
                            SnapshotScope::Person {
                                person_id: person.clone(),
                            },
                            cx,
                        );
                        cx.stop_propagation();
                    })),
            ),
        )
        .into_any_element()
}
