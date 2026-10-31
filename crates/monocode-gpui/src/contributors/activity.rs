//! The Activity card of the Stats section: the last year as a calendar and the
//! weekly rhythm, for the team or one member. The year comes from a read-only
//! load of its own (the store as it is: nothing is synced, fetched or
//! topped up) and never touches the page's model, status or sync state. The
//! calendar and the rhythm are entities, so hovering them repaints them alone.
use super::{
    github::GithubView,
    stats::{card, card_title},
    ui::hex_color,
};
use crate::{
    theme::*,
    visuals::{
        calendar::{self, Active, Surface},
        dither::{Motion, Sheets},
        rhythm,
    },
};
use chrono::{DateTime, Duration as Days, Local, NaiveDate};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, Hsla, MouseMoveEvent, Pixels, Point, Render, Role,
    SharedString, StyleRefinement, WeakEntity, Window, div, prelude::*, px,
};
use monocode_core::contributors::{
    RawData,
    model::{
        activity::{Activity, DayCounts, PersonActivity, build_activity},
        identity::{ResolverOptions, build_resolver},
        period::midnight,
        pipeline::ContributorsModel,
        scoring::observed_identities,
        sync_jobs::iso_seconds,
    },
};
use std::{cell::Cell as StdCell, collections::HashSet, rc::Rc, sync::Arc};

use super::data::Lru;

/// Weeks of history the calendar shows.
pub(super) const WEEKS: i64 = 53;

// ---- pure rules --------------------------------------------------------------------

/// The first day shown, and the `since` and `until` of the read that covers
/// `today`'s last 53 weeks. `until` is the next local midnight, so the key of
/// the read is the same all day.
pub(super) fn window(now: &DateTime<Local>) -> (NaiveDate, String, String) {
    let today = now.date_naive();
    let start = calendar::first_day(today, WEEKS);
    (
        start,
        iso_seconds(&midnight(start)),
        iso_seconds(&midnight(today + Days::days(1))),
    )
}

const DAY_SECS: i64 = 24 * 60 * 60;

fn local_date(epoch: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp(epoch, 0).map(|t| t.with_timezone(&Local).date_naive())
}

/// The one-line note on what the store does not cover of the window, so a
/// hole reads as "not synced" and not as "no work". `missing` is what the
/// load reported (inclusive epoch seconds), `start` the window's first
/// instant and `now` the present. Holes shorter than a day are not worth a note.
pub(super) fn coverage_note(missing: &[(i64, i64)], start: i64, now: i64) -> Option<String> {
    let holes: Vec<(i64, i64)> = missing
        .iter()
        .copied()
        .filter(|(from, to)| to - from + 1 >= DAY_SECS)
        .collect();
    let day = |epoch: i64| local_date(epoch).map(|d| d.format("%b %-d, %Y").to_string());
    let (first, rest) = holes.split_first()?;
    if first.0 <= start + DAY_SECS && first.1 + DAY_SECS >= now {
        return Some("nothing synced in this range".into());
    }
    let (text, others) = if first.0 <= start + DAY_SECS {
        (format!("History starts {}", day(first.1 + 1)?), rest.len())
    } else {
        (
            format!("Not synced {} - {}", day(first.0)?, day(first.1)?),
            rest.len(),
        )
    };
    Some(match others {
        0 => text,
        n => format!("{text} (+{n} more)"),
    })
}

/// The member the card shows: `selected` when they are still in the list,
/// else the team (`None`).
pub(super) fn selection<'a>(selected: Option<&'a str>, present: &[&str]) -> Option<&'a str> {
    selected.filter(|id| present.contains(id))
}

/// One day of the calendar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cell {
    pub date: NaiveDate,
    pub counts: DayCounts,
}
impl Active for Cell {
    fn date(&self) -> NaiveDate {
        self.date
    }
    fn amount(&self) -> u32 {
        self.counts.count()
    }
    fn details(&self, tint: Hsla) -> Vec<(Hsla, String, String)> {
        let mut rows = vec![];
        for (name, n) in [
            ("Commits", self.counts.commits),
            ("PRs reviewed", self.counts.reviews),
            ("PRs merged", self.counts.merged),
        ] {
            if n > 0 {
                rows.push((tint, name.into(), n.to_string()));
            }
        }
        if self.counts.commits > 0 {
            rows.push((
                content(0.3),
                "Lines".into(),
                format!("+{} -{}", self.counts.additions, self.counts.deletions),
            ));
        }
        rows
    }
}

/// Every day from `start` through `today`, empty ones included.
pub(super) fn cells(
    activity: Option<&PersonActivity>,
    start: NaiveDate,
    today: NaiveDate,
) -> Vec<Cell> {
    (0..=(today - start).num_days().max(0))
        .map(|i| {
            let date = start + Days::days(i);
            Cell {
                date,
                counts: activity.map(|a| a.day(date)).unwrap_or_default(),
            }
        })
        .collect()
}

// ---- state -------------------------------------------------------------------------

/// What a built `Activity` was made from; a new key is a new build.
#[derive(Clone, Debug, PartialEq)]
struct BuildKey {
    /// Project, branch and window: another scope is another year, never shown here.
    scope: String,
    load: String,
    config: usize,
    mailmap: usize,
    merge: bool,
}

/// One year read: the data and the parts of it the store does not cover.
struct Year {
    raw: Arc<RawData>,
    missing: Vec<(i64, i64)>,
}

struct Built {
    key: BuildKey,
    activity: Arc<Activity>,
    missing: Vec<(i64, i64)>,
    start: NaiveDate,
    today: NaiveDate,
}

/// The year read, built and selected for the Activity card.
#[derive(Default)]
pub(super) struct ActivityUi {
    /// The member shown, `None` for the team. View state, never saved.
    pub(super) selected: Option<String>,
    pub(super) calendar: Option<Entity<CalendarView>>,
    pub(super) rhythm: Option<Entity<RhythmView>>,
    pub(super) pills: Option<Entity<PillsView>>,
    /// The line beside the card's title, kept so a hover frame does not work it out.
    note: String,
    /// The card was drawn once, which starts its first read.
    asked: bool,
    /// The year as the store has it, by project, branch, window and store state.
    loads: Lru<Arc<Year>>,
    /// Bumped when a job ends: a year read before it must not be served after it.
    pub(super) epoch: u64,
    inflight: HashSet<String>,
    /// The read that failed: it is not asked again until its key changes.
    failed: Option<(String, String)>,
    built: Option<Built>,
    building: Option<BuildKey>,
}

// ---- entities ----------------------------------------------------------------------

#[cfg(test)]
thread_local! {
    /// How many times the calendar, the rhythm and the pills were built.
    pub(super) static BUILDS: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) };
}
#[cfg(test)]
fn built(which: usize) {
    BUILDS.with(|n| {
        let mut all = n.get();
        all[which] += 1;
        n.set(all);
    });
}

/// The calendar. It notifies itself when the hovered day changes, not on every
/// mouse move and not the page, and is drawn from a cache otherwise. `D` is a
/// day (the GitHub page's, or the ClickUp tab's).
pub(super) struct CalendarView<D: Active + PartialEq + 'static = Cell> {
    pub(super) days: Arc<Vec<D>>,
    pub(super) tint: Hsla,
    pub(super) motion: Motion,
    pub(super) hover: Option<usize>,
    pub(super) dim: bool,
    area: Rc<StdCell<Bounds<Pixels>>>,
    /// The ink as one image, and the number that says when it must be remade.
    pub(super) sheet: Sheets,
    version: u64,
    /// Selector, screen reader name and the weeks it can show.
    id: &'static str,
    title: &'static str,
    weeks: i64,
}
impl<D: Active + PartialEq + 'static> CalendarView<D> {
    pub(super) fn new(days: Arc<Vec<D>>, tint: Hsla) -> Self {
        Self {
            days,
            tint,
            motion: Motion(std::time::Instant::now()),
            hover: None,
            dim: false,
            area: Rc::default(),
            sheet: Sheets::default(),
            version: 0,
            id: "contributors-activity-calendar",
            title: "Activity calendar, last 12 months",
            weeks: WEEKS,
        }
    }

    /// Another selector, name and number of weeks than the Contributors page's.
    pub(super) fn labeled(mut self, id: &'static str, title: &'static str, weeks: i64) -> Self {
        self.id = id;
        self.title = title;
        self.weeks = weeks;
        self
    }

    pub(super) fn set(&mut self, days: Arc<Vec<D>>, tint: Hsla, cx: &mut Context<Self>) {
        if *self.days == *days && self.tint == tint {
            return;
        }
        self.days = days;
        self.tint = tint;
        self.hover = None;
        self.version += 1;
        self.motion = Motion(std::time::Instant::now());
        cx.notify();
    }

    /// The page around is dimmed or not. The cached frame bakes the opacity in,
    /// so a change must redraw it; the image and the motion stay.
    pub(super) fn set_dim(&mut self, dim: bool, cx: &mut Context<Self>) {
        if self.dim != dim {
            self.dim = dim;
            cx.notify();
        }
    }

    fn point(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let area = self.area.get();
        let next = at.filter(|p| area.contains(p)).and_then(|p| {
            calendar::hit(
                area.size.width.as_f32(),
                self.days.len(),
                self.weeks,
                (p.x - area.origin.x).as_f32(),
                (p.y - area.origin.y).as_f32(),
            )
        });
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }
}
impl<D: Active + PartialEq + 'static> Render for CalendarView<D> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        built(0);
        calendar::calendar(
            self.days.clone(),
            self.tint,
            self.motion,
            self.weeks,
            self.id,
            calendar::summary(self.title, &self.days[..]),
            Surface {
                area: self.area.clone(),
                sheet: Some((self.sheet.clone(), self.version)),
            },
        )
        .on_mouse_move(
            cx.listener(|this, event: &MouseMoveEvent, _, cx| this.point(Some(event.position), cx)),
        )
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !*hovered {
                this.point(None, cx);
            }
        }))
    }
}

/// The weekly rhythm, with the same hover rule.
pub(super) struct RhythmView {
    pub(super) counts: Arc<rhythm::Counts>,
    pub(super) tint: Hsla,
    pub(super) motion: Motion,
    pub(super) hover: Option<(usize, usize)>,
    pub(super) dim: bool,
    area: Rc<StdCell<Bounds<Pixels>>>,
    pub(super) sheet: Sheets,
    version: u64,
    id: &'static str,
    noun: rhythm::Noun,
}
impl RhythmView {
    pub(super) fn new(counts: Arc<rhythm::Counts>, tint: Hsla) -> Self {
        Self {
            counts,
            tint,
            motion: Motion(std::time::Instant::now()),
            hover: None,
            dim: false,
            area: Rc::default(),
            sheet: Sheets::default(),
            version: 0,
            id: "contributors-activity-rhythm",
            noun: rhythm::COMMITS,
        }
    }

    /// Another selector and another thing counted than the Contributors page's.
    pub(super) fn labeled(mut self, id: &'static str, noun: rhythm::Noun) -> Self {
        self.id = id;
        self.noun = noun;
        self
    }

    pub(super) fn set(&mut self, counts: Arc<rhythm::Counts>, tint: Hsla, cx: &mut Context<Self>) {
        if *self.counts == *counts && self.tint == tint {
            return;
        }
        self.counts = counts;
        self.tint = tint;
        self.hover = None;
        self.version += 1;
        self.motion = Motion(std::time::Instant::now());
        cx.notify();
    }

    /// The page around is dimmed or not. The cached frame bakes the opacity in,
    /// so a change must redraw it; the image and the motion stay.
    pub(super) fn set_dim(&mut self, dim: bool, cx: &mut Context<Self>) {
        if self.dim != dim {
            self.dim = dim;
            cx.notify();
        }
    }

    fn point(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let area = self.area.get();
        let next = at.filter(|p| area.contains(p)).and_then(|p| {
            rhythm::hit(
                area.size.width.as_f32(),
                (p.x - area.origin.x).as_f32(),
                (p.y - area.origin.y).as_f32(),
            )
        });
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }
}
impl Render for RhythmView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        built(1);
        rhythm::rhythm(
            self.counts.clone(),
            self.tint,
            self.motion,
            self.id,
            Surface {
                area: self.area.clone(),
                sheet: Some((self.sheet.clone(), self.version)),
            },
            self.noun,
        )
        .on_mouse_move(
            cx.listener(|this, event: &MouseMoveEvent, _, cx| this.point(Some(event.position), cx)),
        )
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !*hovered {
                this.point(None, cx);
            }
        }))
    }
}

/// The selection pills: Team and each member, one row that scrolls sideways
/// so its height is fixed and it can be drawn from a cache.
pub(super) struct PillsView {
    host: WeakEntity<GithubView>,
    /// Person id (`None` for the team), label and color of each pill.
    pub(super) people: Arc<Vec<(Option<String>, SharedString, Hsla)>>,
    pub(super) selected: Option<String>,
    pub(super) dim: bool,
}
type Pill = (Option<String>, SharedString, Hsla);
/// The height of the pill row.
const PILLS_H: f32 = 28.;

impl PillsView {
    /// The page around is dimmed or not. The cached frame bakes the opacity in,
    /// so a change must redraw it; the image and the motion stay.
    pub(super) fn set_dim(&mut self, dim: bool, cx: &mut Context<Self>) {
        if self.dim != dim {
            self.dim = dim;
            cx.notify();
        }
    }

    fn set(&mut self, people: Arc<Vec<Pill>>, selected: Option<String>, cx: &mut Context<Self>) {
        if *self.people == *people && self.selected == selected {
            return;
        }
        self.people = people;
        self.selected = selected;
        cx.notify();
    }
}
impl Render for PillsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        built(2);
        div()
            .id("contributors-activity-pills")
            .debug_selector(|| "contributors-activity-pills".into())
            .w_full()
            .h(px(PILLS_H))
            .flex()
            .flex_nowrap()
            .items_center()
            .gap(px(6.))
            .overflow_x_scroll()
            .children(
                self.people
                    .iter()
                    .enumerate()
                    .map(|(index, (person, label, color))| {
                        let on = self.selected == *person;
                        let person = person.clone();
                        div()
                            .id(SharedString::from(format!(
                                "contributors-activity-pill-{index}"
                            )))
                            .debug_selector(move || format!("contributors-activity-pill-{index}"))
                            .role(Role::Button)
                            .aria_label(label.clone())
                            .aria_toggled(if on {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .flex_shrink_0()
                            .max_w(px(200.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded_full()
                            .border_1()
                            .border_color(content(if on { 0.25 } else { 0.1 }))
                            .px(px(10.))
                            .py(px(3.))
                            .text_size(px(12.))
                            .text_color(content(if on { 0.95 } else { 0.6 }))
                            .cursor_pointer()
                            .hover(|s| s.bg(content(0.06)))
                            .when(on, |d| d.bg(content(0.08)))
                            .child(
                                div()
                                    .size(px(8.))
                                    .flex_shrink_0()
                                    .rounded(px(2.))
                                    .bg(*color),
                            )
                            .child(div().min_w_0().truncate().child(label.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (person, host) = (person.clone(), this.host.clone());
                                // After this update ends: the host pushes into this view.
                                cx.defer(move |cx| {
                                    let _ = host
                                        .update(cx, |host, cx| host.select_activity(person, cx));
                                });
                            }))
                    }),
            )
    }
}

// ---- the page ----------------------------------------------------------------------

impl GithubView {
    /// The member shown by the card, checked against the people of the model.
    /// A detail page locks it to its member without touching the saved pick.
    pub(super) fn activity_person(&self, model: &ContributorsModel) -> Option<String> {
        // The year does not depend on the period's rows: a member who only
        // reviewed has no row but still gets their own calendar.
        if let Some(member) = &self.detail {
            return Some(member.clone());
        }
        let present: Vec<&str> = model
            .stats
            .rows
            .iter()
            .map(|r| r.person_id.as_str())
            .collect();
        selection(self.activity.selected.as_deref(), &present).map(str::to_string)
    }

    /// The project, branch and window the year is for. It does not depend on
    /// the period's load, so the card keeps its year while a period loads.
    fn year_scope(&self) -> String {
        let (_, since, until) = window(&self.now);
        format!(
            "{}\u{0}{}\u{0}{since}\u{0}{until}",
            self.cwd,
            self.picked_branch.as_deref().unwrap_or(""),
        )
    }

    /// The scope and the key of the read the page is on, `None` before the
    /// period it hangs on is loaded. The key is the branch head and the job
    /// epoch: a sync of another part of the year changes the epoch.
    fn year_ids(&self, cx: &App) -> Option<(String, String)> {
        let loaded = self.derive(cx).loaded?;
        let scope = self.year_scope();
        let load = format!("{scope}\u{0}{}\u{0}{}", loaded.head, self.activity.epoch);
        Some((scope, load))
    }

    /// A job ended: the year as read before it may lack what it stored. The
    /// built year stays on screen until the new read lands.
    pub(super) fn forget_activity_loads(&mut self) {
        self.activity.epoch += 1;
        self.activity.loads = Lru::default();
        self.activity.failed = None;
    }

    /// The year to show: built for this project, branch and window, and not
    /// the answer of a read that has just failed.
    fn shown_year(&self, cx: &App) -> Option<&Built> {
        let scope = self.year_scope();
        let failed = self.year_ids(cx).is_some_and(|(_, load)| {
            self.activity
                .failed
                .as_ref()
                .is_some_and(|(k, _)| *k == load)
        });
        self.activity
            .built
            .as_ref()
            .filter(|built| built.key.scope == scope && !failed)
    }

    /// What the calendar and the rhythm show now: counts and the tint.
    fn activity_input(
        &self,
        model: &ContributorsModel,
        cx: &App,
    ) -> (Arc<Vec<Cell>>, Arc<rhythm::Counts>, Hsla) {
        let person = self.activity_person(model);
        let tint = person
            .as_ref()
            .and_then(|id| model.colors.get(id))
            .map_or_else(accent_color, |hex| hex_color(hex));
        match self.shown_year(cx) {
            Some(built) => {
                let activity = built.activity.of(person.as_deref());
                (
                    Arc::new(cells(activity, built.start, built.today)),
                    Arc::new(activity.map(|a| a.rhythm).unwrap_or_default()),
                    tint,
                )
            }
            None => (Arc::default(), Arc::default(), tint),
        }
    }

    /// Hands the calendar and the rhythm what changed. Each notifies itself
    /// only when its input differs. Never call this while drawing.
    pub(super) fn push_activity(&mut self, cx: &mut Context<Self>) {
        self.activity.note = self.activity_note(cx);
        let (Some(calendar), Some(rhythm), Some(pills)) = (
            self.activity.calendar.clone(),
            self.activity.rhythm.clone(),
            self.activity.pills.clone(),
        ) else {
            return;
        };
        let Some(model) = self.model.clone() else {
            return;
        };
        let (days, counts, tint) = self.activity_input(&model, cx);
        calendar.update(cx, |view, cx| view.set(days, tint, cx));
        rhythm.update(cx, |view, cx| view.set(counts, tint, cx));
        let (people, selected) = (self.activity_pills(&model), self.activity_person(&model));
        pills.update(cx, |view, cx| view.set(people, selected, cx));
        let dim = self.page_dim(cx);
        calendar.update(cx, |view, cx| view.set_dim(dim, cx));
        rhythm.update(cx, |view, cx| view.set_dim(dim, cx));
        pills.update(cx, |view, cx| view.set_dim(dim, cx));
    }

    /// Team first, then each member of the table in its order.
    fn activity_pills(&self, model: &ContributorsModel) -> Arc<Vec<Pill>> {
        let team = (None, SharedString::from("Team"), accent_color());
        Arc::new(
            std::iter::once(team)
                .chain(model.stats.rows.iter().map(|row| {
                    (
                        Some(row.person_id.clone()),
                        SharedString::from(row.name.clone()),
                        hex_color(&row.color),
                    )
                }))
                .collect(),
        )
    }

    pub(super) fn select_activity(&mut self, person: Option<String>, cx: &mut Context<Self>) {
        if self.activity.selected != person {
            self.activity.selected = person;
            self.push_activity(cx);
            cx.notify();
        }
    }

    /// Starts what the card is missing: the read of the year, then its build.
    /// Safe to call on every draw; it only spawns, never notifies.
    pub(super) fn ensure_activity(&mut self, cx: &mut Context<Self>) {
        if self.cwd.is_empty() || !self.prefs_ready {
            return;
        }
        let Some(meta) = &self.meta else {
            return;
        };
        let (config, mailmap) = (meta.config.clone(), meta.mailmap.clone());
        // The period decides the branch head and when the store last changed.
        let Some((scope, load_key)) = self.year_ids(cx) else {
            return;
        };
        let (start, since, until) = window(&self.now);
        let Some(year) = self.activity.loads.peek(&load_key).cloned() else {
            if self.activity.inflight.contains(&load_key)
                || self
                    .activity
                    .failed
                    .as_ref()
                    .is_some_and(|(key, _)| *key == load_key)
            {
                return;
            }
            self.activity.inflight.insert(load_key.clone());
            let (cwd, branch) = (self.cwd.clone(), self.picked_branch.clone());
            self.spawn_job(
                "activity",
                cx,
                move |backend| backend.load(&cwd, branch.as_deref(), &since, &until),
                move |this, answer, cx| {
                    this.activity.inflight.remove(&load_key);
                    match answer {
                        Ok(response) => {
                            this.activity.failed = None;
                            let year = Year {
                                raw: Arc::new(response.raw),
                                missing: response.missing,
                            };
                            this.activity.loads.remember(&load_key, Arc::new(year));
                        }
                        Err(error) => this.activity.failed = Some((load_key, error)),
                    }
                    this.ensure_activity(cx);
                    this.push_activity(cx);
                    cx.notify();
                },
            );
            return;
        };
        let key = BuildKey {
            scope,
            load: load_key,
            config: Arc::as_ptr(&config) as usize,
            mailmap: Arc::as_ptr(&mailmap) as usize,
            merge: self.merge,
        };
        if self.activity.built.as_ref().is_some_and(|b| b.key == key)
            || self.activity.building.as_ref() == Some(&key)
        {
            return;
        }
        self.activity.building = Some(key.clone());
        let merge = self.merge;
        let today = self.now.date_naive();
        let raw = year.raw.clone();
        let missing = year.missing.clone();
        let work = cx.background_executor().spawn(async move {
            let resolver = build_resolver(
                &config,
                ResolverOptions {
                    mailmap: (*mailmap).clone(),
                    observed_identities: observed_identities(&raw),
                    merge,
                },
            );
            build_activity(&raw, &config, &resolver)
        });
        cx.spawn(async move |this, cx| {
            let activity = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.activity.building.as_ref() != Some(&key) {
                    return;
                }
                this.activity.building = None;
                this.activity.built = Some(Built {
                    key,
                    activity: Arc::new(activity),
                    missing,
                    start,
                    today,
                });
                this.push_activity(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The calendar, the rhythm and the pills, made on first use. They learn
    /// about new data from `push_activity`, which runs when this view notifies.
    fn activity_views(
        &mut self,
        model: &ContributorsModel,
        cx: &mut Context<Self>,
    ) -> (Entity<CalendarView>, Entity<RhythmView>, Entity<PillsView>) {
        if let (Some(calendar), Some(rhythm), Some(pills)) = (
            &self.activity.calendar,
            &self.activity.rhythm,
            &self.activity.pills,
        ) {
            return (calendar.clone(), rhythm.clone(), pills.clone());
        }
        let (days, counts, tint) = self.activity_input(model, cx);
        let calendar = cx.new(|_| CalendarView::new(days, tint));
        let rhythm = cx.new(|_| RhythmView::new(counts, tint));
        let host = cx.weak_entity();
        let (people, selected) = (self.activity_pills(model), self.activity_person(model));
        let pills = cx.new(|_| PillsView {
            host,
            people,
            selected,
            dim: false,
        });
        self.activity.note = self.activity_note(cx);
        self.activity.calendar = Some(calendar.clone());
        self.activity.rhythm = Some(rhythm.clone());
        self.activity.pills = Some(pills.clone());
        (calendar, rhythm, pills)
    }

    /// The line beside the card's title: what the store lacks of the year,
    /// or why there is no year yet.
    pub(super) fn activity_note(&self, cx: &App) -> String {
        if let Some(built) = self.shown_year(cx) {
            let start = midnight(built.start).timestamp();
            return coverage_note(&built.missing, start, self.now.timestamp())
                .unwrap_or_else(|| "last 12 months".into());
        }
        let failed = self.year_ids(cx).is_some_and(|(_, load)| {
            self.activity
                .failed
                .as_ref()
                .is_some_and(|(k, _)| *k == load)
        });
        if failed {
            "could not be read".into()
        } else {
            "loading".into()
        }
    }

    /// The card. `pills` is off on a member's page, where the member is fixed.
    pub(super) fn activity_card(
        &mut self,
        model: &ContributorsModel,
        pills: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.activity.asked {
            // Later changes reach `ensure_activity` through `push_stats`.
            self.activity.asked = true;
            self.ensure_activity(cx);
        }
        let (calendar, rhythm, pills_entity) = self.activity_views(model, cx);
        // Replayed frames carry no accessibility nodes: draw them afresh.
        let (calendar, rhythm, pills_view) = if self.a11y {
            (
                calendar.into_any_element(),
                rhythm.into_any_element(),
                pills_entity.into_any_element(),
            )
        } else {
            let fixed = |height: f32| StyleRefinement::default().w_full().h(px(height));
            (
                calendar
                    .cached(fixed(calendar::height()))
                    .into_any_element(),
                rhythm.cached(fixed(rhythm::height())).into_any_element(),
                pills_entity.cached(fixed(PILLS_H)).into_any_element(),
            )
        };
        card()
            .debug_selector(|| "contributors-activity".into())
            .child(card_title("Activity", self.activity.note.clone()))
            .child(
                div()
                    .p(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .when(pills, |d| d.child(pills_view))
                    .child(calendar)
                    .child(rhythm),
            )
            .into_any_element()
    }
}
