//! The divider where a local session continued in the cloud ("Continued in the cloud - codespace
//! x - Nov 11, 2026", design 4.5). A presentation row outside the provider transcript, placed
//! at the move point (`moved_at_row`, a store row) the shell pushed in.
use super::*;
use chrono::{DateTime, Local};

pub(super) const CLOUD_MOVED_ROW: (usize, usize) = (SAVED_DRAFT_START, 3);

/// The divider text for a move: where and when.
pub(crate) fn moved_label(mark: &monocode_core::cloud_sessions::move_ui::MoveMark) -> String {
    let date = i64::try_from(mark.at_ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .map(|date| date.with_timezone(&Local).format("%b %-d, %Y").to_string());
    let mut text = format!("Continued in the cloud - {}", mark.target_label());
    if let Some(date) = date {
        text.push_str(" - ");
        text.push_str(&date);
    }
    text
}

impl TranscriptView {
    /// Shows the divider with `label` at the move point; `None` removes it.
    pub(crate) fn set_cloud_moved(&mut self, label: Option<String>, cx: &mut Context<Self>) {
        if self.cloud_moved == label {
            return;
        }
        self.cloud_moved = label;
        self.rebuild_rows(0, cx);
        cx.notify();
    }

    /// Adds the divider row when it is wanted and not in `rows` yet. Called with the other cloud
    /// rows after a rebuild, once every message has its rows.
    pub(super) fn push_moved_row(&mut self) {
        let Some(point) = self.cloud_move.filter(|_| self.cloud_moved.is_some()) else {
            return;
        };
        let wanted = self.base + point;
        // The history that holds the move point is not loaded (yet): never guess a place.
        if self.source.is_empty()
            || self.source.len() < wanted
            || self
                .rows
                .iter()
                .any(|row| matches!(row.body, Body::CloudMoved))
        {
            return;
        }
        let first_after = self.rows.iter().position(|row| {
            self.messages
                .get(row.message)
                .is_some_and(|message| message.start >= wanted)
        });
        let at = first_after.unwrap_or_else(|| {
            self.rows
                .iter()
                .rposition(|row| row.message != usize::MAX)
                .map_or(self.rows.len(), |last| last + 1)
        });
        self.rows.insert(
            at,
            UiRow {
                key: CLOUD_MOVED_ROW,
                body: Body::CloudMoved,
                message: usize::MAX,
                first: false,
                last: false,
                work: false,
                work_tail: false,
                thought_phase: None,
            },
        );
    }

    pub(super) fn cloud_moved_row(&self) -> gpui::AnyElement {
        let label = self.cloud_moved.clone().unwrap_or_default();
        let rule = || div().h(px(1.)).min_w(px(16.)).flex_1().bg(content(0.12));
        div()
            .id("cloud-moved-row")
            .debug_selector(|| "cloud-moved-row".into())
            .role(Role::Label)
            .aria_label(label.clone())
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(COLUMN))
                    .px(px(crate::theme::COLUMN_PAD))
                    .pt_5()
                    .pb_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(rule())
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(content(0.5))
                            .child(icon("cloud-upload", 12.).text_color(content(0.5)))
                            .child(div().child(label)),
                    )
                    .child(rule()),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};
    use monocode_core::cloud_sessions::{CloudBackendKind, move_ui::MoveMark};

    fn row(role: &str, text: &str) -> Row {
        Row {
            role: role.into(),
            text: text.into(),
            first: true,
            sent_at: None,
        }
    }
    fn window(cx: &mut TestAppContext) -> WindowHandle<TranscriptView> {
        cx.update(|cx| cx.set_reduce_motion(true));
        cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new();
                    view.restore_history(
                        ["One", "Two"]
                            .into_iter()
                            .flat_map(|t| [row("user", t), row("assistant", t)])
                            .collect(),
                        cx,
                    );
                    view
                })
            })
            .unwrap()
        })
    }
    /// `user`, `assistant` or `moved` per message (a message may have several rows).
    fn shape(view: &TranscriptView) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut last = None;
        for r in &view.rows {
            let label = match r.body {
                Body::CloudMoved => Some(("moved", usize::MAX)),
                _ => match view.messages.get(r.message).map(|m| m.role.as_str()) {
                    Some("user") => Some(("user", r.message)),
                    Some("assistant") => Some(("assistant", r.message)),
                    _ => None,
                },
            };
            if let Some(label) = label
                && last != Some(label)
            {
                out.push(label.0);
                last = Some(label);
            }
        }
        out
    }
    fn mark() -> MoveMark {
        MoveMark {
            at_ms: 1_700_000_000_000,
            backend: CloudBackendKind::Codespaces,
            target: "octo-abc".into(),
        }
    }

    #[test]
    fn the_label_names_the_target_and_the_date() {
        let label = moved_label(&mark());
        assert!(
            label.starts_with("Continued in the cloud - codespace octo-abc - "),
            "{label}"
        );
        assert!(
            label.ends_with("2023") || label.ends_with("2024"),
            "{label}"
        );
    }

    #[gpui::test]
    fn the_divider_sits_at_the_move_point(cx: &mut TestAppContext) {
        let window = window(cx);
        window
            .update(cx, |view, _, cx| {
                // Without a move point there is nothing to draw.
                view.set_cloud_moved(Some(moved_label(&mark())), cx);
                assert_eq!(shape(view), ["user", "assistant", "user", "assistant"]);
                // The first two rows were written locally: the divider goes between the turns.
                view.set_cloud_move(Some(2), cx);
                assert_eq!(
                    shape(view),
                    ["user", "assistant", "moved", "user", "assistant"]
                );
                // Moved after the last row: the divider is at the end.
                view.set_cloud_move(Some(4), cx);
                assert_eq!(
                    shape(view),
                    ["user", "assistant", "user", "assistant", "moved"]
                );
                // A removed label removes the row.
                view.set_cloud_moved(None, cx);
                assert_eq!(shape(view), ["user", "assistant", "user", "assistant"]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn the_divider_renders_its_text(cx: &mut TestAppContext) {
        let window = window(cx);
        window
            .update(cx, |view, _, cx| {
                view.set_cloud_move(Some(2), cx);
                view.set_cloud_moved(Some(moved_label(&mark())), cx);
            })
            .unwrap();
        let visual = &mut VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(visual.debug_bounds("cloud-moved-row").is_some());
    }
}
