//! The Overview tab: who the Mono is and what it is doing, its code
//! contributions as a dithered calendar, today's numbers, what runs next and
//! what it made today.
use super::*;
use crate::right_panel::{DIFF_ADD, DIFF_DEL, format_integer, tabular};
use crate::runtime::{Database, locked};
use crate::visuals::calendar;
use crate::visuals::dither::{Motion, repaints};
use chrono::{DateTime, Duration as Days, Local, NaiveDate};
use gpui::{Div, FontWeight, Hsla};
use monocode_core::contributions::{self, Contribution, Kind};
use monocode_core::time::{now_ms, relative};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// One day of a Mono's code contributions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Day {
    pub date: NaiveDate,
    pub commits: u32,
    pub reviews: u32,
    pub opened: u32,
    pub merged: u32,
    pub additions: u32,
    pub deletions: u32,
}
impl Day {
    fn empty(date: NaiveDate) -> Self {
        Self {
            date,
            commits: 0,
            reviews: 0,
            opened: 0,
            merged: 0,
            additions: 0,
            deletions: 0,
        }
    }
    /// Commits, reviews, and pull requests opened or merged each count once;
    /// lines never do.
    pub fn count(&self) -> u32 {
        self.commits + self.reviews + self.opened + self.merged
    }
}

impl calendar::Active for Day {
    fn date(&self) -> NaiveDate {
        self.date
    }
    fn amount(&self) -> u32 {
        Day::count(self)
    }
    fn details(&self, tint: Hsla) -> Vec<(Hsla, String, String)> {
        let mut rows = vec![];
        for (name, n) in [
            ("Commits", self.commits),
            ("Reviews", self.reviews),
            ("PRs opened", self.opened),
            ("PRs merged", self.merged),
        ] {
            if n > 0 {
                rows.push((tint, name.into(), n.to_string()));
            }
        }
        rows.push((
            content(0.3),
            "Lines".into(),
            format!("+{} −{}", self.additions, self.deletions),
        ));
        rows
    }
}

/// Weeks of history the graph can show; a narrow panel keeps the latest.
const WEEKS: i64 = 26;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// How long loaded contributions are shown before they are read again.
const FRESH: std::time::Duration = std::time::Duration::from_secs(30);
/// How often opened pull requests are checked on GitHub for merging.
const LANDED_EVERY: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// The marker a session's saved history has been scanned under.
const SCANNED: &str = "contributions-scanned:";

/// A Mono's contributions as the Overview shows them.
pub(super) struct Loaded {
    at: Instant,
    days: Arc<Vec<Day>>,
    /// Pull requests merged today, by the Mono or after it opened them.
    merged_today: usize,
    landed_checked: Instant,
}

/// Every day from the graph's first Sunday through `today`, with what was
/// contributed on it.
pub(super) fn days_from(items: &[Contribution], today: NaiveDate) -> Vec<Day> {
    let start = first_day(today);
    let mut days: Vec<Day> = (0..=(today - start).num_days())
        .map(|i| Day::empty(start + Days::days(i)))
        .collect();
    for item in items {
        let Some(date) = local_day(item.at) else {
            continue;
        };
        let Some(day) = (date >= start)
            .then(|| (date - start).num_days() as usize)
            .and_then(|ix| days.get_mut(ix))
        else {
            continue;
        };
        match item.kind {
            Kind::Commit => {
                day.commits += 1;
                day.additions += item.additions as u32;
                day.deletions += item.deletions as u32;
            }
            Kind::Review => day.reviews += 1,
            Kind::Opened => day.opened += 1,
            Kind::Merged => day.merged += 1,
            Kind::Landed => {}
        }
    }
    days
}

/// Distinct pull requests merged on `day`, whether the Mono merged them or
/// they merged after it opened them.
pub(super) fn merged_on(items: &[Contribution], day: NaiveDate) -> usize {
    items
        .iter()
        .filter(|c| matches!(c.kind, Kind::Merged | Kind::Landed))
        .filter(|c| local_day(c.at) == Some(day))
        .map(|c| c.reference.as_str())
        .collect::<HashSet<_>>()
        .len()
}

/// What a saved tool call must mention to possibly be a contribution.
const MENTIONS: [&str; 4] = ["git commit", "git -C", "gh pr", "pull_request"];

/// Finds the contributions in a session's saved history, once. Later ones
/// are recorded as its turns run. Sessions that never ran a matching command
/// are ruled out by SQLite and never loaded.
fn scan(store: &Database, session: i64) {
    let key = format!("{SCANNED}{session}");
    let (rows, cwd, at) = {
        let Ok(store) = locked(store) else { return };
        if store.setting(&key).ok().flatten().is_some() {
            return;
        }
        if !store.session_mentions(session, &MENTIONS).unwrap_or(true) {
            let _ = store.save_setting(&key, "1");
            return;
        }
        let Some(summary) = store.mono_session(session).ok().flatten() else {
            let _ = store.save_setting(&key, "1");
            return;
        };
        let mut rows = store.load(session).map(|t| t.rows).unwrap_or_default();
        if let Ok(Some(before)) = store.history_before(session)
            && let Ok(page) = store.load_before(session, before, true)
        {
            rows.splice(0..0, page.rows);
        }
        (rows, summary.cwd, summary.updated_at)
    };
    // Git runs without the store lock.
    let resolved: Vec<_> = rows
        .iter()
        .filter(|row| row.role == "tool")
        .filter(|row| MENTIONS.iter().any(|w| row.text.contains(w)))
        .filter_map(|row| serde_json::from_str::<monocode_core::tool::ToolCall>(&row.text).ok())
        .flat_map(|tool| contributions::found(&tool))
        .map(|found| {
            let item = contributions::resolve(&found, session, std::path::Path::new(&cwd), at);
            (item, found.amend)
        })
        .collect();
    if let Ok(store) = locked(store) {
        for (item, amend) in resolved {
            let _ = store.record_contribution(&item, amend);
        }
        let _ = store.save_setting(&key, "1");
    }
}

/// Records pull requests the Mono opened that have merged since. Few, recent
/// and in the background: each asks GitHub.
fn check_landed(store: &Database, sessions: &[i64]) {
    let since = now_ms().saturating_sub(30 * 24 * 60 * 60 * 1000);
    let Ok(items) = locked(store).and_then(|s| s.contributions(sessions, since)) else {
        return;
    };
    let settled: HashSet<&str> = items
        .iter()
        .filter(|c| matches!(c.kind, Kind::Merged | Kind::Landed))
        .map(|c| c.reference.as_str())
        .collect();
    for opened in items
        .iter()
        .filter(|c| c.kind == Kind::Opened && !settled.contains(c.reference.as_str()))
        .take(10)
    {
        if let Ok(Some(at)) = contributions::merged_at(&opened.reference) {
            let landed = Contribution {
                kind: Kind::Landed,
                at,
                ..opened.clone()
            };
            if let Ok(store) = locked(store) {
                let _ = store.record_contribution(&landed, false);
            }
        }
    }
}

/// The Sunday that opens the graph's first week.
fn first_day(today: NaiveDate) -> NaiveDate {
    calendar::first_day(today, WEEKS)
}

/// The contribution calendar in the Mono's colour. The whole window repaints
/// as the pointer moves: this page has no cached rows to protect.
fn graph(days: Arc<Vec<Day>>, tint: Hsla, motion: Motion) -> AnyElement {
    let label = calendar::summary("Contribution calendar", &days[..]);
    repaints(calendar::calendar(
        days,
        tint,
        motion,
        WEEKS,
        "mono-contributions-graph",
        label,
        Default::default(),
    ))
    .into_any_element()
}

/// Consecutive days with a contribution, ending today or yesterday.
pub(super) fn streak(days: &[Day]) -> usize {
    let mut tail = days.iter().rev().peekable();
    // Today still counts as open until it ends.
    if tail.peek().is_some_and(|d| d.count() == 0) {
        tail.next();
    }
    tail.take_while(|d| d.count() > 0).count()
}

fn local_day(ms: u64) -> Option<NaiveDate> {
    DateTime::from_timestamp_millis(ms as i64).map(|t| t.with_timezone(&Local).date_naive())
}

/// How far list rows reach past the section's edge: their hover fill gets
/// room around the content, which still lines up with the section labels.
const ROW_HANG: f32 = 6.;

fn section(title: &'static str) -> Div {
    div()
        .flex_shrink_0()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .px_1()
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(content(0.4))
                .child(title),
        )
}

fn tile(value: AnyElement, label: &'static str) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .px_3()
        .py(px(10.))
        .rounded_lg()
        .border_1()
        .border_color(content(0.05))
        .bg(content(0.03))
        .flex()
        .flex_col()
        .gap_0p5()
        .child(value)
        .child(
            div()
                .text_size(px(11.))
                .text_color(content(0.45))
                .truncate()
                .child(label),
        )
}

fn figure(text: String) -> Div {
    div()
        .text_size(px(20.))
        .line_height(px(26.))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(tabular())
        .text_color(content(0.95))
        .child(text)
}

impl Monos {
    /// The Mono's contributions as last loaded, reloading them in the
    /// background once they are stale. Drawing never touches the store.
    fn contributions_for(
        &mut self,
        mono: &Mono,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) -> (Arc<Vec<Day>>, usize) {
        let loaded = self.contributions.get(&mono.id);
        let stale = loaded.is_none_or(|l| l.at.elapsed() > FRESH);
        if stale && self.contributions_loading.insert(mono.id.clone()) {
            let landed = loaded.is_none_or(|l| l.landed_checked.elapsed() > LANDED_EVERY);
            let mut sessions: Vec<i64> = mono.session_id.into_iter().collect();
            sessions.extend(mono.launches.iter().map(|l| l.session_id));
            sessions.extend(mono.habit_sessions.iter().copied());
            sessions.sort_unstable();
            sessions.dedup();
            let store = self.store.clone();
            let id = mono.id.clone();
            let work = cx.background_executor().spawn(async move {
                for session in &sessions {
                    scan(&store, *session);
                }
                if landed {
                    check_landed(&store, &sessions);
                }
                let since = DateTime::<Local>::from_naive_utc_and_offset(
                    first_day(today).and_hms_opt(0, 0, 0).unwrap_or_default(),
                    *Local::now().offset(),
                )
                .timestamp_millis()
                .max(0) as u64;
                let items = locked(&store)
                    .and_then(|s| s.contributions(&sessions, since.saturating_sub(DAY_MS)))
                    .unwrap_or_default();
                (days_from(&items, today), merged_on(&items, today), landed)
            });
            cx.spawn(async move |this, cx| {
                let (days, merged_today, landed) = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.contributions_loading.remove(&id);
                    let landed_checked = match this.contributions.get(&id) {
                        Some(old) if !landed => old.landed_checked,
                        _ => Instant::now(),
                    };
                    this.contributions.insert(
                        id,
                        Loaded {
                            at: Instant::now(),
                            days: Arc::new(days),
                            merged_today,
                            landed_checked,
                        },
                    );
                    cx.notify();
                });
            })
            .detach();
        }
        match self.contributions.get(&mono.id) {
            Some(l) => (l.days.clone(), l.merged_today),
            None => (Arc::new(days_from(&[], today)), 0),
        }
    }

    pub(super) fn dashboard(
        &mut self,
        mono: &Mono,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = match &self.dashboard_shown {
            Some((id, at)) if *id == mono.id => *at,
            _ => {
                let at = Instant::now();
                self.dashboard_shown = Some((mono.id.clone(), at));
                at
            }
        };
        let now = now_ms();
        let today = Local::now().date_naive();
        let tint: Hsla = rgb(mono.color).into();
        let views = self
            .owner
            .upgrade()
            .map(|owner| owner.read(cx).mono_session_views(cx))
            .unwrap_or_default();
        let mine: Vec<_> = views
            .iter()
            .filter(|v| mono.launches.iter().any(|l| l.session_id == v.id))
            .collect();
        let state = self
            .chats
            .get(&mono.id)
            .map(|chat| chat.read(cx).mono_status())
            .unwrap_or("idle");
        let busy: Vec<_> = mine.iter().filter(|v| v.busy).collect();
        let state = match state {
            "idle" if !busy.is_empty() => "working",
            s => s,
        };

        // Identity, with a live line about what it is doing right now.
        let (live, ticker) = match (state, busy.first()) {
            ("needs-you", _) => (true, "Waiting for your input".to_string()),
            (_, Some(v)) if busy.len() > 1 => (
                true,
                format!("Working on {} +{} more", v.title, busy.len() - 1),
            ),
            (_, Some(v)) => (true, format!("Working on {}", v.title)),
            ("working", None) => (true, "Thinking…".to_string()),
            _ => match mine.iter().map(|v| v.updated_at).max().filter(|t| *t > 0) {
                Some(at) => (
                    false,
                    format!("Idle · last active {} ago", relative(at, now)),
                ),
                None => (false, "Idle".to_string()),
            },
        };
        let header = div()
            .flex_shrink_0()
            .w_full()
            .min_w_0()
            .px_4()
            .pt_4()
            .pb_3()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .size(px(48.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(avatar::avatar(
                        &mono.mascot,
                        mono.color,
                        state,
                        44.,
                        state == "idle",
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(15.))
                            .line_height(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(content(1.))
                            .child(mono.label()),
                    )
                    .child(
                        div()
                            .debug_selector(|| "mono-dashboard-ticker".into())
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(12.))
                            .text_color(if state == "needs-you" {
                                accent_color()
                            } else {
                                content(0.55)
                            })
                            .when(live, |d| {
                                d.child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(
                                    if state == "needs-you" {
                                        accent_color()
                                    } else {
                                        tint
                                    },
                                ))
                            })
                            .child(div().min_w_0().truncate().child(ticker)),
                    ),
            );

        // Contributions: placeholder history until they are recorded.
        let (days, merged_today) = self.contributions_for(mono, today, cx);
        let total: u32 = days.iter().map(Day::count).sum();
        let streak = streak(&days);
        let today_stats = *days.last().expect("the graph always includes today");
        let contributions = section("Contributions")
            .child(
                div()
                    .px_1()
                    .flex()
                    .items_end()
                    .gap_1p5()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .font_features(tabular())
                            .text_color(content(0.9))
                            .child(format_integer(total as i64)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(content(0.5))
                            .child("in the last 6 months"),
                    )
                    .when(streak > 1, |d| {
                        d.child(
                            div()
                                .ml_auto()
                                .text_size(px(11.))
                                .font_features(tabular())
                                .text_color(content(0.5))
                                .child(format!("{streak}-day streak")),
                        )
                    }),
            )
            .child(graph(days, accent_color(), Motion(shown)));

        // Today's numbers: code from contributions, work from the Mono.
        let sessions_today = mine
            .iter()
            .filter(|v| local_day(v.updated_at) == Some(today))
            .count();
        let ran_today: usize = self
            .habits
            .iter()
            .map(|h| {
                h.runs
                    .iter()
                    .filter(|r| local_day(r.at) == Some(today))
                    .count()
            })
            .sum();
        let still_today = self
            .habits
            .iter()
            .filter(|h| h.enabled && h.next_run_at > now && local_day(h.next_run_at) == Some(today))
            .count();
        let lines = div()
            .flex()
            .items_end()
            .gap_1p5()
            .text_size(px(20.))
            .line_height(px(26.))
            .font_weight(FontWeight::SEMIBOLD)
            .font_features(tabular())
            .child(
                div()
                    .text_color(rgb(DIFF_ADD))
                    .child(format!("+{}", format_integer(today_stats.additions as i64))),
            )
            .child(
                div()
                    .text_color(rgb(DIFF_DEL))
                    .child(format!("−{}", format_integer(today_stats.deletions as i64))),
            );
        let today_tiles = section("Today")
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(tile(
                        figure(merged_today.to_string()).into_any_element(),
                        "PRs merged",
                    ))
                    .child(tile(lines.into_any_element(), "Lines changed")),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(tile(
                        figure(sessions_today.to_string()).into_any_element(),
                        "Sessions",
                    ))
                    .child(tile(
                        figure(format!("{ran_today}/{}", ran_today + still_today))
                            .into_any_element(),
                        "Habits run",
                    )),
            );

        // Up next: the soonest scheduled habits, running ones first.
        let mut upcoming: Vec<_> = self
            .habits
            .iter()
            .filter(|h| h.enabled || self.habit_running_since(h).is_some())
            .map(|h| (self.habit_running_since(h), h))
            .collect();
        upcoming.sort_by_key(|(running, h)| (running.is_none(), h.next_run_at));
        let up_next = section("Up next").children(upcoming.is_empty().then(|| {
            div()
                .px_1()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(content(0.45))
                .child(format!(
                    "Nothing scheduled. Ask {} to put recurring work on a habit.",
                    mono.label()
                ))
                .into_any_element()
        }));
        let up_next = up_next.children(upcoming.into_iter().take(3).map(|(running, h)| {
            let id = h.id.clone();
            let at = local_day(h.next_run_at);
            // Today's runs give their time, later ones their weekday.
            let when = match running {
                Some(since) => format!("Running · {}", relative(since, now)),
                None => {
                    let day = DateTime::from_timestamp_millis(h.next_run_at as i64)
                        .map(|t| {
                            let t = t.with_timezone(&Local);
                            t.format(if at == Some(today) { "%H:%M" } else { "%a" })
                                .to_string()
                        })
                        .unwrap_or_default();
                    match relative(now, h.next_run_at.max(now)) {
                        soon if soon == "now" => format!("{day} · soon"),
                        later => format!("{day} · in {later}"),
                    }
                }
            };
            div()
                .id(SharedString::from(format!("mono-dashboard-habit-{id}")))
                .role(Role::Button)
                .aria_label(h.name.clone())
                .mx(px(-ROW_HANG))
                .px(px(ROW_HANG + 4.))
                .py_2()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .hover(|s| s.bg(content(0.05)))
                .child(
                    div()
                        .size(px(28.))
                        .flex_shrink_0()
                        .rounded_md()
                        .bg(content(0.06))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if running.is_some() {
                            div()
                                .size(px(8.))
                                .rounded_full()
                                .bg(tint)
                                .into_any_element()
                        } else {
                            icon("clock", 14.)
                                .text_color(content(0.6))
                                .into_any_element()
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(content(0.85))
                                .child(h.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(content(0.45))
                                .child(h.schedule.label()),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .font_features(tabular())
                        .text_color(if running.is_some() {
                            tint
                        } else {
                            content(0.4)
                        })
                        .child(when),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.route = Some(Route::Habit(id.clone()));
                    cx.notify();
                }))
        }));

        // New today: what the Mono wrote, newest first.
        let mut fresh: Vec<_> = mono
            .documents
            .iter()
            .filter(|d| local_day(d.created_at) == Some(today))
            .collect();
        fresh.sort_by_key(|d| std::cmp::Reverse(d.created_at));
        let new_today = section("New today").children(fresh.is_empty().then(|| {
            div()
                .px_1()
                .text_size(px(12.))
                .text_color(content(0.45))
                .child("Nothing new yet today.")
                .into_any_element()
        }));
        let new_today = new_today.children(fresh.into_iter().take(4).map(|doc| {
            let id = doc.id.clone();
            div()
                .id(SharedString::from(format!("mono-dashboard-doc-{id}")))
                .role(Role::Button)
                .aria_label(doc.title.clone())
                .mx(px(-ROW_HANG))
                .px(px(ROW_HANG + 4.))
                .py_2()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .hover(|s| s.bg(content(0.05)))
                .child(
                    div()
                        .size(px(28.))
                        .flex_shrink_0()
                        .rounded_md()
                        .bg(content(0.06))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("file", 14.).text_color(content(0.6))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(content(0.85))
                                .child(doc.title.clone()),
                        )
                        .when(!doc.summary.trim().is_empty(), |d| {
                            d.child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .text_color(content(0.45))
                                    .child(doc.summary.clone()),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(relative(doc.created_at, now)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.route = Some(Route::Document(id.clone()));
                    cx.notify();
                }))
        }));

        div()
            .id("mono-dashboard")
            .debug_selector(|| "mono-dashboard".into())
            .size_full()
            .min_w_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .px_3()
                    .pt_2()
                    .pb_6()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .child(contributions)
                    .child(today_tiles)
                    .child(up_next)
                    .child(new_today),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visuals::calendar::levels;
    use chrono::Datelike;

    fn day(count: u32) -> Day {
        Day {
            date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            commits: count,
            reviews: 0,
            opened: 0,
            merged: 0,
            additions: 0,
            deletions: 0,
        }
    }

    fn at(date: NaiveDate, hour: u32) -> u64 {
        date.and_hms_opt(hour, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap()
            .timestamp_millis() as u64
    }

    fn item(kind: Kind, reference: &str, at: u64) -> Contribution {
        Contribution {
            session: 1,
            kind,
            reference: reference.into(),
            at,
            additions: 10,
            deletions: 2,
        }
    }

    #[test]
    fn days_cover_whole_weeks_and_count_each_kind() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let yesterday = today - Days::days(1);
        let items = [
            item(Kind::Commit, "a", at(today, 9)),
            item(Kind::Commit, "b", at(today, 10)),
            item(Kind::Review, "acme/app#1@x", at(today, 11)),
            item(Kind::Opened, "acme/app#2", at(yesterday, 12)),
            item(Kind::Landed, "acme/app#2", at(today, 13)),
            item(Kind::Merged, "acme/app#3", at(today, 14)),
            // Older than the graph.
            item(Kind::Commit, "c", at(today - Days::weeks(40), 9)),
        ];
        let days = days_from(&items, today);
        assert_eq!(days.first().unwrap().date.weekday(), chrono::Weekday::Sun);
        assert_eq!(days.len() as i64, (WEEKS - 1) * 7 + 5);
        let last = days.last().unwrap();
        assert_eq!((last.commits, last.reviews, last.merged), (2, 1, 1));
        assert_eq!((last.additions, last.deletions), (20, 4));
        // Landing counts toward merged pull requests, not contributions.
        assert_eq!(last.count(), 4);
        assert_eq!(days[days.len() - 2].opened, 1);
        assert_eq!(days.iter().map(Day::count).sum::<u32>(), 5);
        assert_eq!(merged_on(&items, today), 2);
        assert_eq!(merged_on(&items, yesterday), 0);
    }

    #[test]
    fn levels_split_active_days_by_quartile() {
        let days: Vec<_> = [0, 1, 2, 3, 4, 5, 6, 7, 8].into_iter().map(day).collect();
        assert_eq!(levels(&days), [0, 1, 1, 1, 2, 2, 3, 3, 4]);
        assert_eq!(levels(&[day(0), day(0)]), [0, 0]);
        assert_eq!(levels(&[day(3)]), [1]);
    }

    #[test]
    fn streaks_count_back_from_today_or_yesterday() {
        let run = |counts: &[u32]| streak(&counts.iter().map(|c| day(*c)).collect::<Vec<_>>());
        assert_eq!(run(&[1, 0, 2, 3, 1]), 3);
        assert_eq!(run(&[1, 2, 3, 0]), 3);
        assert_eq!(run(&[1, 0, 0]), 0);
    }
}
