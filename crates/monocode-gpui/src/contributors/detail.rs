//! A member's detail page: a header, stat cards, a trend chart, the calendar
//! and the weekly rhythm, and lists. The layout pieces at the top know nothing
//! of GitHub (they take texts, colors and rows), so the ClickUp tab fills the
//! same page with points and tickets. The GitHub fill, and the open and close
//! rules, follow below.
use super::{
    chart::{self, ChartBucket, ChartData, Series},
    github::GithubView,
    stats::{
        ChangeView, CountCard, LinesCard, card, card_title, change, chart_buckets, count_change,
        fraction_change, kpi, unit, versus,
    },
    ui::{self, AvatarSize},
};
use crate::{
    right_panel::{DIFF_ADD, DIFF_DEL, tabular},
    theme::*,
};
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, Entity, Hsla, Pixels, Point, Role, SharedString,
    Stateful, Subscription, Window, div, prelude::*, px, relative, rgb,
};
use monocode_core::contributors::model::{
    member::{MemberDetail, member_detail},
    pipeline::ContributorsModel,
    snapshot::SnapshotScope,
    stats::Stats,
};
use std::sync::Arc;

// ---- the layout, for any kind of member -------------------------------------------

/// A row of a bar list: a label, a bar filled to `fraction` and a value.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BarRow {
    pub label: String,
    pub value: String,
    /// 0 to 1.
    pub fraction: f32,
}

/// A row of a list: a lead (a number), a title, a tag and a trailing text.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ListRow {
    pub lead: String,
    pub title: String,
    pub tag: Option<String>,
    pub trail: String,
}

/// The Back control, the way out of the page.
pub(super) fn back_bar(
    prefix: &'static str,
    on_back: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div().flex().child(
        div()
            .id(SharedString::from(format!("{prefix}-back")))
            .debug_selector(move || format!("{prefix}-back"))
            .role(Role::Button)
            .aria_label("Back to the list")
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded_lg()
            .pl(px(6.))
            .pr(px(10.))
            .py(px(4.))
            .text_size(px(12.))
            .text_color(content(0.6))
            .cursor_pointer()
            .hover(|s| s.bg(content(0.08)).text_color(content(0.9)))
            .child(icon("chevron-left", 14.))
            .child("Back")
            .on_click(on_back),
    )
}

/// Who the page is about: avatar, name, notes under the name (the identities
/// of a person) and an optional action on the right.
pub(super) struct Header {
    /// Prefix of the selectors: `contributors-detail`, `ticket-detail`.
    pub prefix: &'static str,
    pub name: String,
    pub color: String,
    pub notes: Vec<String>,
    pub action: Option<AnyElement>,
}

pub(super) fn header(header: Header) -> Div {
    let prefix = header.prefix;
    div()
        .debug_selector(move || format!("{prefix}-header"))
        .flex()
        .items_center()
        .gap(px(14.))
        .child(ui::avatar(&header.name, &header.color, AvatarSize::Lg))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .id(SharedString::from(format!("{prefix}-name")))
                        .role(Role::Heading)
                        .aria_label(header.name.clone())
                        .text_size(px(18.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .truncate()
                        .child(header.name.clone()),
                )
                .child(
                    div()
                        .debug_selector(move || format!("{prefix}-notes"))
                        .mt(px(2.))
                        .flex()
                        .flex_wrap()
                        .gap_x(px(12.))
                        .text_size(px(12.))
                        .text_color(content(0.5))
                        .font_family(crate::fonts::mono())
                        .children(header.notes.iter().map(|note| div().child(note.clone()))),
                ),
        )
        .children(header.action)
}

/// The Ask button of a header.
pub(super) fn ask_button(
    prefix: &'static str,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("{prefix}-ask")))
        .debug_selector(move || format!("{prefix}-ask"))
        .role(Role::Button)
        .aria_label(label)
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .border_color(content(0.1))
        .px(px(12.))
        .py(px(4.))
        .text_size(px(12.))
        .text_color(accent_color())
        .cursor_pointer()
        .hover(|s| s.bg(content(0.05)))
        .child("Ask")
        .on_click(on_click)
        .into_any_element()
}

/// The page: back control, header, a wrapping row of cards, then the sections.
pub(super) fn page(
    prefix: &'static str,
    back: Div,
    header: Div,
    cards: Vec<Div>,
    sections: Vec<AnyElement>,
) -> Div {
    div()
        .debug_selector(move || prefix.to_string())
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(back)
        .child(header)
        .child(div().flex().flex_wrap().gap(px(10.)).children(cards))
        .children(sections)
}

/// A page with nothing to show: a line and the way back.
pub(super) fn empty_page(prefix: &'static str, back: Div, text: &'static str) -> Div {
    div()
        .debug_selector(move || prefix.to_string())
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(back)
        .child(
            div()
                .debug_selector(move || format!("{prefix}-empty"))
                .px(px(12.))
                .py(px(24.))
                .flex()
                .justify_center()
                .text_size(px(12.))
                .text_color(content(0.5))
                .child(text),
        )
}

/// A titled card around any body.
pub(super) fn section(title: &'static str, note: String, body: impl IntoElement) -> AnyElement {
    card()
        .child(card_title(title, note))
        .child(body)
        .into_any_element()
}

fn empty_line(text: &'static str) -> Div {
    div()
        .px(px(12.))
        .py(px(16.))
        .text_size(px(12.))
        .text_color(content(0.5))
        .child(text)
}

/// Bars with a label and a value on each row, filled in `color`.
pub(super) fn bar_list(
    id: &'static str,
    label: &'static str,
    rows: &[BarRow],
    color: Hsla,
    empty: &'static str,
) -> Stateful<Div> {
    if rows.is_empty() {
        return div()
            .id(id)
            .debug_selector(move || id.to_string())
            .child(empty_line(empty));
    }
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .role(Role::List)
        .aria_label(label)
        .px(px(12.))
        .py(px(8.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .children(rows.iter().enumerate().map(|(i, row)| {
            div()
                .debug_selector(move || format!("{id}-{i}"))
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(12.))
                .child(
                    div()
                        .w(px(200.))
                        .flex_shrink_0()
                        .truncate()
                        .font_family(crate::fonts::mono())
                        .child(row.label.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(6.))
                        .overflow_hidden()
                        .rounded_full()
                        .bg(content(0.1))
                        .child(
                            div()
                                .h_full()
                                .w(relative(row.fraction.clamp(0., 1.)))
                                .rounded_full()
                                .bg(color),
                        ),
                )
                .child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .text_right()
                        .text_color(content(0.5))
                        .font_features(tabular())
                        .child(row.value.clone()),
                )
        }))
}

/// Whether any row has a lead: when none has, the column is left out and its
/// width is not reserved.
pub(super) fn has_lead_column(rows: &[ListRow]) -> bool {
    rows.iter().any(|row| !row.lead.is_empty())
}

/// A plain list: lead, title, tag, trailing text.
pub(super) fn list(
    id: &'static str,
    label: &'static str,
    rows: &[ListRow],
    empty: &'static str,
) -> Stateful<Div> {
    if rows.is_empty() {
        return div()
            .id(id)
            .debug_selector(move || id.to_string())
            .child(empty_line(empty));
    }
    let lead = has_lead_column(rows);
    div()
        .id(id)
        .debug_selector(move || id.to_string())
        .role(Role::List)
        .aria_label(label)
        .flex()
        .flex_col()
        .children(rows.iter().enumerate().map(|(i, row)| {
            div()
                .debug_selector(move || format!("{id}-{i}"))
                .h(px(34.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(10.))
                .text_size(px(12.))
                .when(i > 0, |d| d.border_t_1().border_color(content(0.1)))
                .when(lead, |d| {
                    d.child(
                        div()
                            .w(px(48.))
                            .flex_shrink_0()
                            .text_color(content(0.5))
                            .font_features(tabular())
                            .child(row.lead.clone()),
                    )
                })
                .child(div().flex_1().min_w_0().truncate().child(row.title.clone()))
                .children(row.tag.clone().map(|tag| {
                    div()
                        .flex_shrink_0()
                        .rounded_full()
                        .border_1()
                        .border_color(content(0.15))
                        .px(px(6.))
                        .text_size(px(11.))
                        .text_color(content(0.55))
                        .child(tag)
                }))
                .child(
                    div()
                        .w(px(96.))
                        .flex_shrink_0()
                        .text_right()
                        .text_color(content(0.5))
                        .child(row.trail.clone()),
                )
        }))
}

/// Selector prefix of the GitHub page.
const PREFIX: &str = "contributors-detail";

// ---- the GitHub fill: pure view functions --------------------------------------------

pub(super) struct SurvivalCard {
    pub value: String,
    pub change: Option<ChangeView>,
}

/// What the cards of a member say.
pub(super) struct DetailTexts {
    pub commits: CountCard,
    pub prs: CountCard,
    pub reviews: CountCard,
    pub lines: LinesCard,
    pub survival: SurvivalCard,
}

/// "87%", or "n/a" when nothing was sampled.
pub(super) fn survival_text(survival: Option<f64>) -> String {
    survival.map_or_else(
        || "n/a".into(),
        |s| format!("{}%", (s * 100.0).round() as i64),
    )
}

pub(super) fn detail_texts(d: &MemberDetail, versus: &Option<String>) -> DetailTexts {
    let points = d.survival_delta.map(|s| (s * 100.0).round() as i64);
    DetailTexts {
        commits: CountCard {
            value: d.commits.value.to_string(),
            change: fraction_change(d.commits.delta, versus),
        },
        prs: CountCard {
            value: d.prs_merged.value.to_string(),
            change: count_change(d.prs_merged.delta, versus),
        },
        reviews: CountCard {
            value: d.reviews.value.to_string(),
            change: count_change(d.reviews.delta, versus),
        },
        lines: LinesCard {
            added: format!(
                "+{}",
                monocode_core::contributors::model::compact::format_compact(d.lines.added as f64)
            ),
            deleted: format!(
                "-{}",
                monocode_core::contributors::model::compact::format_compact(d.lines.deleted as f64)
            ),
            change: fraction_change(d.lines.delta, versus),
        },
        survival: SurvivalCard {
            value: survival_text(d.survival),
            change: points.and_then(|p| {
                let text = if p > 0 {
                    format!("+{p} pts")
                } else {
                    format!("{p} pts")
                };
                change(Some(p as f64), text, versus.clone())
            }),
        },
    }
}

/// The ids of the two trend series.
pub(super) const COMMITS_ID: &str = "commits";
pub(super) const REVIEWS_ID: &str = "reviews";

/// The trend chart's input: commits and PRs reviewed per bucket of the plan
/// the list's chart uses.
pub(super) fn trend_data(d: &MemberDetail, stats: &Stats) -> ChartData {
    let buckets: Vec<ChartBucket> = chart_buckets(stats);
    let totals: Vec<f64> = d
        .commits_by_bucket
        .iter()
        .zip(&d.reviews_by_bucket)
        .map(|(c, r)| (c + r) as f64)
        .collect();
    let values = |counts: &[u64]| counts.iter().map(|n| *n as f64).collect();
    ChartData {
        series: vec![
            Series {
                id: COMMITS_ID.into(),
                name: "Commits".into(),
                color: ui::hex_color(&d.color),
                values: values(&d.commits_by_bucket),
            },
            Series {
                id: REVIEWS_ID.into(),
                name: "PRs reviewed".into(),
                color: accent_color(),
                values: values(&d.reviews_by_bucket),
            },
        ],
        summary: chart::summary(
            "Commits and PRs reviewed",
            unit(stats.granularity),
            "commits and reviews",
            &buckets,
            &totals,
        ),
        buckets,
    }
}

fn lines_text(lines: u64) -> String {
    format!("{} {}", lines, if lines == 1 { "line" } else { "lines" })
}

/// The areas as bars: the share of the member's lines, and the lines.
pub(super) fn area_rows(d: &MemberDetail) -> Vec<BarRow> {
    d.areas
        .iter()
        .map(|area| BarRow {
            label: area.name.clone(),
            value: format!(
                "{}% - {}",
                (area.share_pct + 0.5).floor() as i64,
                lines_text(area.lines)
            ),
            fraction: (area.share_pct / 100.0) as f32,
        })
        .collect()
}

/// "Oct 8, 2026" of a timestamp, local; the text itself when unreadable.
fn date_text(iso: &str) -> String {
    monocode_core::contributors::model::period::parse_iso(iso)
        .map_or_else(|| iso.to_string(), |t| t.format("%b %-d, %Y").to_string())
}

pub(super) fn pr_rows(d: &MemberDetail) -> Vec<ListRow> {
    d.recent_prs
        .iter()
        .map(|pr| ListRow {
            lead: pr.number.map_or_else(String::new, |n| format!("#{n}")),
            title: pr.title.clone(),
            tag: Some(pr.state.to_string()),
            trail: date_text(&pr.merged_at),
        })
        .collect()
}

// ---- state ------------------------------------------------------------------------

/// The numbers of the page for one model and person.
struct Shown {
    model: Arc<ContributorsModel>,
    person: String,
    now: i64,
    detail: Option<Arc<MemberDetail>>,
}

/// What an open detail page keeps. The chart is made with the page and dropped
/// with it, so its focus never outlives a visit.
#[derive(Default)]
pub(super) struct DetailUi {
    pub(super) chart: Option<Entity<chart::ChartView>>,
    shown: Option<Shown>,
    /// The scroll of the list underneath, put back by Back.
    saved_scroll: Option<Point<Pixels>>,
    subscription: Option<Subscription>,
}

impl GithubView {
    /// Opens the detail page of a person in place of the Stats body. The list
    /// keeps its section, period, focus, selection and scroll, and the page
    /// starts at the top.
    pub(super) fn open_detail(&mut self, person_id: String, cx: &mut Context<Self>) {
        if self.detail.is_some() {
            return;
        }
        self.leave(cx);
        self.detail_ui.saved_scroll = Some(self.scroll.offset());
        self.detail = Some(person_id);
        self.scroll.set_offset(Point::default());
        self.push_activity(cx);
        cx.notify();
    }

    /// Back to the list, with the scroll where it was, and the keys on this view.
    pub(super) fn close_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.detail_ui.saved_scroll.take();
        if !self.drop_detail(cx) {
            return;
        }
        if let Some(saved) = saved {
            self.scroll.set_offset(saved);
        }
        window.focus(&self.focus, cx);
    }

    /// Forgets the detail page; whether there was one.
    pub(super) fn drop_detail(&mut self, cx: &mut Context<Self>) -> bool {
        if self.detail.take().is_none() {
            return false;
        }
        self.detail_ui = DetailUi::default();
        self.push_activity(cx);
        cx.notify();
        true
    }

    /// The numbers of the open page, worked out once per model.
    fn member(&mut self, model: &Arc<ContributorsModel>) -> Option<Arc<MemberDetail>> {
        let person = self.detail.clone()?;
        let now = self.now.timestamp();
        if let Some(shown) = &self.detail_ui.shown
            && Arc::ptr_eq(&shown.model, model)
            && shown.person == person
            && shown.now == now
        {
            return shown.detail.clone();
        }
        let detail = member_detail(model, &person, &self.now).map(Arc::new);
        self.detail_ui.shown = Some(Shown {
            model: model.clone(),
            person,
            now,
            detail: detail.clone(),
        });
        detail
    }

    /// Hands the trend chart what changed. Never call this while drawing.
    pub(super) fn push_detail(&mut self, cx: &mut Context<Self>) {
        let (Some(chart), Some(model)) = (self.detail_ui.chart.clone(), self.model.clone()) else {
            return;
        };
        if let Some(member) = self.member(&model) {
            let data = Arc::new(trend_data(&member, &model.stats));
            chart.update(cx, |chart, cx| chart.set_data(data, cx));
        }
        let kind = self.chart_kind(cx);
        chart.update(cx, |chart, cx| chart.set_kind(kind, cx));
    }

    fn detail_chart(
        &mut self,
        member: &MemberDetail,
        model: &ContributorsModel,
        cx: &mut Context<Self>,
    ) -> Entity<chart::ChartView> {
        if let Some(chart) = &self.detail_ui.chart {
            return chart.clone();
        }
        let data = Arc::new(trend_data(member, &model.stats));
        let kind = self.chart_kind(cx);
        let chart = cx.new(|cx| {
            chart::ChartView::new("contributors-detail", data, kind, cx).without_palette()
        });
        self.detail_ui.subscription = Some(
            cx.subscribe(&chart, |this, _, event: &chart::ChartEvent, cx| {
                this.chart_event(event, cx)
            }),
        );
        self.detail_ui.chart = Some(chart.clone());
        chart
    }

    /// The page of the open person, drawn in place of the Stats body.
    pub(super) fn detail_body(
        &mut self,
        model: &Arc<ContributorsModel>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let back = back_bar(
            PREFIX,
            cx.listener(|this, _, window, cx| this.close_detail(window, cx)),
        );
        let Some(member) = self.member(model) else {
            return empty_page(PREFIX, back, "No activity in this period.").into_any_element();
        };
        let texts = detail_texts(&member, &versus(model));
        let person = member.person_id.clone();
        let ask = ask_button(
            PREFIX,
            format!("Ask Agent about {}", member.name),
            cx.listener(move |this, _, _, cx| {
                this.open_ask(
                    SnapshotScope::Person {
                        person_id: person.clone(),
                    },
                    cx,
                );
            }),
        );
        let head = header(Header {
            prefix: PREFIX,
            name: member.name.clone(),
            color: member.color.clone(),
            notes: if member.identities.is_empty() {
                vec!["No commit identities".into()]
            } else {
                member.identities.clone()
            },
            action: Some(ask),
        });
        let cards = vec![
            kpi(
                "contributors-detail-kpi-commits",
                "Commits",
                div().child(texts.commits.value),
                texts.commits.change.as_ref(),
            ),
            kpi(
                "contributors-detail-kpi-prs",
                "PRs merged",
                div().child(texts.prs.value),
                texts.prs.change.as_ref(),
            ),
            kpi(
                "contributors-detail-kpi-reviews",
                "PRs reviewed",
                div().child(texts.reviews.value),
                texts.reviews.change.as_ref(),
            ),
            kpi(
                "contributors-detail-kpi-lines",
                "Lines changed",
                div()
                    .flex()
                    .gap(px(6.))
                    .child(div().text_color(rgb(DIFF_ADD)).child(texts.lines.added))
                    .child(div().text_color(rgb(DIFF_DEL)).child(texts.lines.deleted)),
                texts.lines.change.as_ref(),
            ),
            kpi(
                "contributors-detail-kpi-survival",
                "Code surviving",
                div().child(texts.survival.value),
                texts.survival.change.as_ref(),
            ),
        ];
        let chart = self.detail_chart(&member, model, cx);
        let color = ui::hex_color(&member.color);
        let sections = vec![
            section(
                "Trend",
                format!("per {}", unit(model.stats.granularity)),
                chart,
            ),
            self.activity_card(model, false, cx),
            section(
                "Areas of the code",
                "by lines changed".into(),
                bar_list(
                    "contributors-detail-areas",
                    "Areas of the code",
                    &area_rows(&member),
                    color,
                    "No code changes in this period.",
                ),
            ),
            section(
                "Recent PRs",
                "newest first".into(),
                list(
                    "contributors-detail-prs",
                    "Recent PRs",
                    &pr_rows(&member),
                    "No merged PRs in this period.",
                ),
            ),
        ];
        page(PREFIX, back, head, cards, sections).into_any_element()
    }
}
