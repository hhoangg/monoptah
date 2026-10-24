//! Visuals are persistent transcript messages with lazily created native views.
use super::*;
use crate::visuals::{VisualChanged, VisualView};
use monocode_core::visual::VisualDocument;

// Each view owns a bounded script interpreter. Long conversations must not
// keep one interpreter alive for every visual that has ever been painted.
const RETAINED_VISUALS: usize = 8;

pub(super) struct CachedVisual {
    document: Arc<VisualDocument>,
    view: gpui::Entity<VisualView>,
    touched: Instant,
    _changes: gpui::Subscription,
}

impl TranscriptView {
    pub(super) fn visual_row(
        &mut self,
        message: usize,
        document: Arc<VisualDocument>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let unchanged = self.visuals.get(&message).is_some_and(|cached| {
            cached.document.id == document.id
                && cached.document.title == document.title
                && cached.document.source == document.source
        });
        if !unchanged {
            self.visuals.remove(&message);
            if self.visuals.len() >= RETAINED_VISUALS
                && let Some(oldest) = self
                    .visuals
                    .iter()
                    .min_by_key(|(_, cached)| cached.touched)
                    .map(|(&key, _)| key)
            {
                self.visuals.remove(&oldest);
            }
            let view = cx.new(|cx| VisualView::new((*document).clone(), cx));
            let changes = cx.subscribe(&view, move |this, _, _: &VisualChanged, cx| {
                for (ix, row) in this.rows.iter().enumerate() {
                    if row.key.0 == message && matches!(row.body, Body::Visual(_)) {
                        this.list.remeasure_items(ix..ix + 1);
                    }
                }
                this.list
                    .remeasure_items(this.footer_ix()..this.footer_ix() + 1);
                cx.notify();
            });
            self.visuals.insert(
                message,
                CachedVisual {
                    document,
                    view,
                    touched: Instant::now(),
                    _changes: changes,
                },
            );
        }
        let cached = self.visuals.get_mut(&message).unwrap();
        cached.touched = Instant::now();
        div()
            .w_full()
            .min_w_0()
            .py_3()
            .child(cached.view.clone())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn row(role: &str, text: String) -> Row {
        Row {
            role: role.into(),
            text,
            first: true,
            sent_at: None,
        }
    }

    fn document(id: &str) -> VisualDocument {
        VisualDocument {
            id: id.into(),
            title: "Interactive result".into(),
            source: "return () => visual.text('Ready');".into(),
        }
    }

    #[gpui::test]
    fn visuals_remain_visible_when_work_is_folded_and_history_is_reloaded(cx: &mut TestAppContext) {
        let view = cx.new(|_| TranscriptView::new());
        let doc = document("plot-1");
        let mut rows = vec![row("user", "Show the results".into())];
        let tool = ToolCall::new("read", "Read");
        rows.push(row("tool", serde_json::to_string(&tool).unwrap()));
        let mut visual = monocode_core::transcript::Transcript::default();
        visual.begin("visual");
        visual.append(&serde_json::to_string(&doc).unwrap());
        rows.extend(visual.rows);
        rows.push(row("assistant", "The results are ready.".into()));
        view.update(cx, |view, cx| {
            view.patch(0, rows.clone(), cx);
            for reveal in view.reveals.values_mut() {
                reveal.settle(Instant::now());
            }
            view.rebuild(0, cx);
            assert!(view.rows.iter().any(|row| {
                matches!(&row.body, Body::Visual(value) if value.id == doc.id) && !row.work
            }));
            view.replace_history(rows, cx);
            assert_eq!(
                view.rows
                    .iter()
                    .filter(|row| matches!(row.body, Body::Visual(_)))
                    .count(),
                1
            );
        });
    }

    #[gpui::test]
    fn malformed_visuals_show_a_readable_error_without_exposing_the_payload(
        cx: &mut TestAppContext,
    ) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.patch(0, vec![row("visual", "{malformed".into())], cx);
            assert!(view.rows.iter().any(|row| {
                matches!(&row.body, Body::Plain(text) if text.as_ref() == "This visual could not be loaded.")
            }));
        });
    }

    #[gpui::test]
    fn retained_script_heaps_are_bounded_and_charged_to_history_cache(cx: &mut TestAppContext) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            let initial = view.history_weight();
            for index in 0..RETAINED_VISUALS + 2 {
                let document = Arc::new(document(&format!("visual-{index}")));
                let _ = view.visual_row(index, document, cx);
                assert!(view.visuals.len() <= RETAINED_VISUALS);
            }
            assert_eq!(view.visuals.len(), RETAINED_VISUALS);
            assert!(!view.visuals.contains_key(&0));
            assert_eq!(
                view.history_weight() - initial,
                RETAINED_VISUALS * monocode_core::visual::MEMORY_LIMIT_BYTES,
            );
            view.replace_history(Vec::new(), cx);
            assert!(view.visuals.is_empty());
        });
    }
}
