//! The Sources tab: a table of every source and its empty state.
use super::{Today, clock, short_day};
use crate::{inbox::message, theme::*};
use gpui::{AnyElement, Div, FontWeight, Role, div, prelude::*, px};
use monocode_core::today::{Source, SyncStatus};

/// What the Status column says. Text, so it never relies on color alone.
pub(super) fn status_text(source: &Source, running: bool) -> &'static str {
    if running {
        return "Running";
    }
    match source.last_status {
        SyncStatus::Never => "Never",
        SyncStatus::Ok => "OK",
        SyncStatus::Failed => "Failed",
    }
}

const PROJECT_W: f32 = 140.;
const MONO_W: f32 = 120.;
const RUNS_W: f32 = 120.;
const SYNC_W: f32 = 120.;
const ITEMS_W: f32 = 48.;

fn cell(width: Option<f32>) -> Div {
    let cell = div().min_w_0().overflow_hidden().px(px(8.));
    match width {
        Some(width) => cell.w(px(width)).flex_shrink_0(),
        None => cell.flex_1(),
    }
}
fn header(text: &'static str, width: Option<f32>) -> Div {
    cell(width)
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(content(0.4))
        .child(text)
}

impl Today {
    /// `Today 08:00` for the current work day, `Oct 31 08:01` otherwise.
    fn sync_text(&self, source: &Source) -> String {
        let Some(at) = source.last_sync_at else {
            return "Never".into();
        };
        let time = clock(at);
        let current = self.view.as_ref().map(|view| view.current_day.as_str());
        match source.last_sync_day.as_deref() {
            Some(day) if Some(day) == current => format!("Today {time}"),
            Some(day) => format!("{} {time}", short_day(day)),
            None => time,
        }
    }

    fn source_row(&self, ix: usize, source: &Source) -> AnyElement {
        let habit = source
            .habit_id
            .as_deref()
            .and_then(|habit| self.habit(&source.mono_id, habit));
        let mono = self.mono(&source.mono_id);
        let running = habit.is_some_and(|habit| habit.busy);
        let none = if self.roster_loaded && mono.is_none() {
            "Mono removed"
        } else {
            "No habit"
        };
        let runs = habit.map_or(none.to_string(), |habit| {
            if habit.enabled {
                habit.schedule.clone()
            } else {
                format!("{} (paused)", habit.schedule)
            }
        });
        let failed = source.last_status == SyncStatus::Failed && !running;
        let status = status_text(source, running);

        let projects = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .children(
                source
                    .projects
                    .iter()
                    .take(2)
                    .map(|path| self.project_badge(path, true)),
            )
            .when(source.projects.len() > 2, |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(content(0.5))
                        .child(format!("+{}", source.projects.len() - 2)),
                )
            })
            .when(source.projects.is_empty(), |d| {
                d.text_color(content(0.4)).child("None")
            });
        let mono_cell = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .when_some(self.mono_mark(&source.mono_id, 16.), |d, mark| {
                d.child(mark)
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(mono.map_or_else(|| "Unknown".to_string(), |mono| mono.name.clone())),
            );
        let status_cell = div()
            .child(
                div()
                    .text_color(if failed { danger_color() } else { content(0.8) })
                    .child(status),
            )
            .when(failed, |d| {
                d.when_some(source.last_error.clone(), |d, error| {
                    d.child(
                        div()
                            .id(gpui::SharedString::from(format!("today-source-error-{ix}")))
                            .debug_selector(move || format!("today-source-error-{ix}"))
                            .text_size(px(11.))
                            .text_color(danger_color().opacity(0.85))
                            .child(error),
                    )
                })
            });

        div()
            .id(gpui::SharedString::from(format!("today-source-{ix}")))
            .debug_selector(move || format!("today-source-{ix}"))
            .role(Role::Row)
            .aria_label(source.name.clone())
            .py(px(9.))
            .flex()
            .items_start()
            .border_t_1()
            .border_color(content(0.07))
            .text_size(px(12.))
            .text_color(content(0.8))
            .child(cell(None).truncate().child(source.name.clone()))
            .child(cell(Some(PROJECT_W)).child(projects))
            .child(cell(Some(MONO_W)).child(mono_cell))
            .child(cell(Some(RUNS_W)).truncate().child(runs))
            .child(cell(Some(SYNC_W)).truncate().child(self.sync_text(source)))
            .child(cell(None).child(status_cell))
            .child(cell(Some(ITEMS_W)).child(source.item_count.to_string()))
            .into_any_element()
    }

    pub(super) fn sources_view(&self) -> AnyElement {
        if self.sources.is_empty() {
            return div()
                .id("today-sources-empty")
                .debug_selector(|| "today-sources-empty".into())
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    message(
                        "No sources yet. Ask a Mono to set up a source, for example: track my GitHub review requests in Today.",
                    )
                    .max_w(px(420.))
                    .text_center(),
                )
                .into_any_element();
        }
        div()
            .id("today-sources")
            .role(Role::Table)
            .aria_label("Sources")
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(12.))
            .pb(px(16.))
            .child(
                div()
                    .py(px(6.))
                    .flex()
                    .child(header("SOURCE", None))
                    .child(header("PROJECT", Some(PROJECT_W)))
                    .child(header("MONO", Some(MONO_W)))
                    .child(header("RUNS", Some(RUNS_W)))
                    .child(header("LAST SYNC", Some(SYNC_W)))
                    .child(header("STATUS", None))
                    .child(header("ITEMS", Some(ITEMS_W))),
            )
            .children(
                self.sources
                    .iter()
                    .enumerate()
                    .map(|(ix, source)| self.source_row(ix, source)),
            )
            .child(
                div()
                    .px(px(8.))
                    .pt(px(10.))
                    .text_size(px(12.))
                    .text_color(content(0.5))
                    .child(
                        "Rules live in each habit. Ask the Mono that owns a source to change its filter or schedule.",
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{HabitInfo, MonoInfo, list::testkit::*};
    use super::*;
    use gpui::TestAppContext;

    fn source(name: &str, status: SyncStatus) -> Source {
        Source {
            id: format!("src-{name}"),
            name: name.into(),
            provider: "GitHub".into(),
            mono_id: "mono".into(),
            habit_id: Some("habit".into()),
            last_status: status,
            ..Default::default()
        }
    }
    fn roster() -> Vec<MonoInfo> {
        vec![MonoInfo {
            id: "mono".into(),
            name: "Pikachu".into(),
            mascot: "cat".into(),
            color: 0xf5c84d,
            projects: Vec::new(),
            habits_known: true,
            habits: vec![HabitInfo {
                id: "habit".into(),
                name: "Daily".into(),
                schedule: "Daily 08:00".into(),
                enabled: true,
                busy: false,
            }],
        }]
    }

    #[test]
    fn status_is_always_a_word() {
        let mut s = source("a", SyncStatus::Never);
        assert_eq!(status_text(&s, false), "Never");
        s.last_status = SyncStatus::Ok;
        assert_eq!(status_text(&s, false), "OK");
        s.last_status = SyncStatus::Failed;
        assert_eq!(status_text(&s, false), "Failed");
        assert_eq!(status_text(&s, true), "Running");
    }
    #[gpui::test]
    fn no_sources_shows_the_empty_state(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Sources, |_| {});
        assert!(has(&mut visual, "today-sources-empty"));
        assert!(!has(&mut visual, "today-source-0"));
    }
    #[gpui::test]
    fn a_failed_source_shows_its_error_and_an_ok_one_does_not(cx: &mut TestAppContext) {
        let (_, mut visual) = page(cx, Show::Sources, |today| {
            today.roster = roster();
            let mut bad = source("ClickUp", SyncStatus::Failed);
            bad.last_error = Some("Token expired".into());
            today.sources = vec![bad, source("GitHub", SyncStatus::Ok)];
        });
        assert!(has(&mut visual, "today-source-0"));
        assert!(has(&mut visual, "today-source-1"));
        assert!(has(&mut visual, "today-source-error-0"));
        assert!(!has(&mut visual, "today-source-error-1"));
        assert!(!has(&mut visual, "today-sources-empty"));
    }
}
