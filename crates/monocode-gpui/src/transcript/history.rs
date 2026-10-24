//! Compact interaction state survives an inactive transcript cache eviction.
use super::*;

const PAGE_TURNS: usize = 10;

/// Keep source row indices intact for persistence, tools and live patches.
/// Only the displayed window needs Markdown and presentation objects.
pub(crate) struct PreparedHistory {
    rows: Vec<Row>,
    parsers: HashMap<usize, MarkdownParser>,
}
impl PreparedHistory {
    /// Run on the history loader, before handing its result to the UI.
    pub(crate) fn new(rows: Vec<Row>, mono: bool) -> Self {
        let start = if mono {
            page_start(&rows, rows.len())
        } else {
            0
        };
        let parsers = prepare_parsers(&rows[start..], start, mono);
        Self { rows, parsers }
    }
}

pub(super) fn page_start(rows: &[Row], before: usize) -> usize {
    rows[..before.min(rows.len())]
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, row)| row.first && row.is_prompt())
        .nth(PAGE_TURNS - 1)
        .map_or(0, |(at, _)| at)
}

fn prepare_parsers(rows: &[Row], offset: usize, mono: bool) -> HashMap<usize, MarkdownParser> {
    let mut parsers = HashMap::new();
    let mut at = 0;
    while at < rows.len() {
        let end = (at + 1..rows.len())
            .find(|&ix| rows[ix].first)
            .unwrap_or(rows.len());
        if rows[at].role == "assistant" || (!mono && rows[at].role == "reasoning") {
            let text: String = rows[at..end].iter().map(|row| row.text.as_str()).collect();
            let mut parser = MarkdownParser::new();
            parser.set_text(&text);
            parsers.insert(offset + at, parser);
        }
        at = end;
    }
    parsers
}

pub(super) struct HistoryPosition {
    key: Option<(usize, usize)>,
    offset: ListOffset,
    following: bool,
}
impl TranscriptView {
    pub(crate) fn install_prepared_history(
        &mut self,
        history: PreparedHistory,
        cx: &mut Context<Self>,
    ) {
        self.prepared_parsers = history.parsers;
        self.patch(0, history.rows, cx);
    }

    pub(super) fn older_history_row(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .id("older-mono-history")
            .debug_selector(|| "older-mono-history".into())
            .role(Role::Button)
            .aria_label("Load earlier messages")
            .w_full()
            .py_3()
            .flex()
            .justify_center()
            .text_size(px(12.))
            .text_color(content(0.5))
            .cursor_pointer()
            .hover(|s| s.text_color(content(0.8)))
            .child(if self.history_loading {
                "Loading earlier messages…"
            } else {
                "Load earlier messages"
            })
            .on_click(cx.listener(|this, _, _, cx| this.load_older_history(cx)))
            .into_any_element()
    }

    pub(super) fn load_older_history(&mut self, cx: &mut Context<Self>) {
        if self.history_start == 0 || self.history_loading {
            return;
        }
        self.history_loading = true;
        self.list.pause_following_tail();
        let previous = self.history_start;
        let before = self
            .history_search
            .as_ref()
            .map_or(previous, |(row, _)| row.saturating_add(1).min(previous));
        let start = page_start(&self.source, before);
        // Snapshot only this page. A render/scroll callback must never copy
        // the long-lived conversation or parse its Markdown synchronously.
        let rows = self.source[start..previous].to_vec();
        let epoch = self.highlight_epoch;
        let mono = self.mono;
        let work = cx
            .background_executor()
            .spawn(async move { prepare_parsers(&rows, start, mono) });
        cx.spawn(async move |this, cx| {
            let parsers = work.await;
            let _ = this.update(cx, |this, cx| {
                if epoch != this.highlight_epoch || this.history_start != previous {
                    return;
                }
                this.history_loading = false;
                this.history_start = start;
                this.prepared_parsers.extend(parsers);
                this.rebuild(start, cx);
                if let Some((row, query)) = this.history_search.take() {
                    this.reveal_search(row, &query, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn replace_history(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        let len = rows.len();
        self.selection.clear();
        self.history_position = None;
        self.history_loading = false;
        self.history_requested = false;
        self.history_search = None;
        self.prepared_parsers.clear();
        self.mono_groups.get_mut().clear();
        self.active_provider_turn_id = None;
        self.search_match = None;
        self.anchor_turn = None;
        self.prompt_rise = None;
        self.prompt_rise_pending = false;
        self.introduced_prompt = None;
        self.turns.retain(|&start, _| start < len);
        self.elapsed.retain(|&start, _| start < len);
        self.expanded.retain(|&start| start < len);
        self.phases.retain(|&start, _| start < len);
        self.phase_layouts.clear();
        self.attachment_images.clear();
        self.visuals.clear();
        self.reveals.clear();
        self.motions.clear();
        self.entrances.clear();
        self.hover_rows.clear();
        self.hovered_turn = None;
        self.hovered_prompt = None;
        self.hovered_prompt_actions = None;
        self.outline = Default::default();
        self.reflow = Default::default();
        self.follow = Default::default();
        self.highlight_epoch = self.highlight_epoch.wrapping_add(1);
        self.highlights.clear();
        self.highlighting.clear();
        self.highlight_queue.clear();
        self.highlight_active = false;
        self.highlight_order.clear();
        self.highlight_bytes = 0;
        self.restore_history(rows, cx);
    }
    /// A conservative payload estimate, including the source, parsed markdown
    /// and presentation copies. The inactive cache also has a session count cap.
    pub(crate) fn history_weight(&self) -> usize {
        self.source_bytes
            .saturating_mul(4)
            .saturating_add(
                self.source
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Row>()),
            )
            .saturating_add(self.highlight_bytes)
            // Charge each retained interpreter's cap to the inactive cache.
            .saturating_add(
                self.visuals
                    .len()
                    .saturating_mul(monocode_core::visual::MEMORY_LIMIT_BYTES),
            )
    }
    pub(crate) fn suspend_history(&mut self, cx: &mut Context<Self>) -> bool {
        if self.visible
            || self.streaming
            || self.history_position.is_some()
            || !self.pending.is_empty()
            || self.saved_draft.is_some()
            || !self.note_pending.is_empty()
            || self.target_action.is_some()
        {
            return false;
        }
        let offset = self.list.logical_scroll_top();
        self.selection.clear();
        self.history_position = Some(HistoryPosition {
            key: self.rows.get(offset.item_ix).map(|row| row.key),
            offset,
            following: self.list.is_following_tail(),
        });
        self.source = Vec::new();
        self.source_bytes = 0;
        self.history_start = 0;
        self.history_loading = false;
        self.history_requested = false;
        self.history_search = None;
        self.prepared_parsers = HashMap::new();
        self.mono_groups = Default::default();
        self.attachment_images = HashMap::new();
        self.visuals = HashMap::new();
        self.messages = Vec::new();
        self.rows = Vec::new();
        self.reveals = HashMap::new();
        self.phase_layouts = HashMap::new();
        self.motions = HashMap::new();
        self.entrances = HashMap::new();
        self.taken = Vec::new();
        self.hover_rows = HashMap::new();
        self.hovered_turn = None;
        self.hovered_prompt = None;
        self.hovered_prompt_actions = None;
        self.action_anchors = HashMap::new();
        self.outline = Default::default();
        self.reflow = Default::default();
        self.follow = Default::default();
        // A running syntax task owns its input until completion; its old epoch
        // cannot repopulate this cache after suspension or after a quick reopen.
        self.highlight_epoch = self.highlight_epoch.wrapping_add(1);
        self.highlights = HashMap::new();
        self.highlighting = HashSet::new();
        self.highlight_queue = VecDeque::new();
        self.highlight_active = false;
        self.highlight_order = VecDeque::new();
        self.highlight_bytes = 0;
        self.list.reset(1);
        cx.notify();
        true
    }
    pub(crate) fn restore_history(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        self.restore_prepared_history(PreparedHistory::new(rows, self.mono), cx);
    }
    pub(crate) fn restore_prepared_history(
        &mut self,
        history: PreparedHistory,
        cx: &mut Context<Self>,
    ) {
        let position = self.history_position.take();
        // Reopening also handles a source that became empty in another window.
        self.source = Vec::new();
        self.source_bytes = 0;
        self.messages = Vec::new();
        self.rows = Vec::new();
        self.history_start = 0;
        self.history_loading = false;
        self.history_requested = false;
        self.mono_groups.get_mut().clear();
        self.list.reset(1);
        self.install_prepared_history(history, cx);
        if let Some(position) = position {
            if position.following {
                self.list.set_follow_mode(FollowMode::Tail);
                self.list.scroll_to_end();
            } else {
                self.list.pause_following_tail();
                let item_ix = position
                    .key
                    .and_then(|key| self.rows.iter().position(|row| row.key == key))
                    .unwrap_or(position.offset.item_ix.min(self.rows.len()));
                self.list.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item: position.offset.offset_in_item,
                });
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn long_mono(turns: usize) -> Vec<Row> {
        (0..turns).flat_map(|turn| [
            Row { role: "user".into(), text: format!("Question {turn}"), first: true, sent_at: Some(turn as u64) },
            Row { role: "assistant".into(), text: format!("Answer **{turn}**.\n\nA second paragraph with [a link](https://example.com)."), first: true, sent_at: None },
        ]).collect()
    }

    #[gpui::test]
    fn long_mono_initial_display_and_live_patches_keep_history_unmaterialized(
        cx: &mut TestAppContext,
    ) {
        let source = long_mono(5_000);
        let prepared = PreparedHistory::new(source, true);
        assert_eq!(prepared.parsers.len(), PAGE_TURNS);
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.mono_mode("mono", 0xffffff, "Mono");
            view.install_prepared_history(prepared, cx);
            assert_eq!(view.source.len(), 10_000);
            assert_eq!(view.messages.len(), PAGE_TURNS * 2);
            assert!(
                view.rows.len() < 100,
                "initial layout must depend on the recent page"
            );
            assert_eq!(view.messages[0].start, 9_980);
            assert_eq!(
                view.source[0].text, "Question 0",
                "the archived source remains available"
            );
            view.streaming = true;
            let mut next = long_mono(1);
            next[0].text = "New question".into();
            next[1].text = "New answer".into();
            view.patch(10_000, next, cx);
            assert_eq!(view.source.len(), 10_002);
            assert!(view.messages.len() <= (PAGE_TURNS + 1) * 2);
            assert_eq!(view.messages.last().unwrap().start, 10_001);
            assert_eq!(view.source[0].text, "Question 0");
        });
    }

    #[gpui::test]
    fn internal_mono_reports_also_keep_the_initial_display_bounded(cx: &mut TestAppContext) {
        let mut rows = long_mono(1_000);
        for row in &mut rows {
            if row.is_prompt() {
                row.role = "mono-system".into();
            }
        }
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.mono_mode("mono", 0xffffff, "Mono");
            view.install_prepared_history(PreparedHistory::new(rows, true), cx);
            assert_eq!(view.messages.len(), PAGE_TURNS * 2);
            assert_eq!(view.messages[0].start, 1_980);
            assert!(view.rows.len() < 100);
            assert!(
                view.rows
                    .iter()
                    .any(|row| matches!(row.body, Body::WorkLine { .. }))
            );
            assert_eq!(view.source.len(), 2_000);
        });
    }

    #[gpui::test]
    fn mono_group_cache_follows_changed_prompt_boundaries(cx: &mut TestAppContext) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.mono_mode("mono", 0xffffff, "Mono");
            let mut rows = long_mono(2);
            rows[2].role = "mono-system".into();
            view.patch(0, rows, cx);
            assert_eq!(view.mono_group_start(3), 1);
            let mut prompt = view.source[2].clone();
            prompt.role = "user".into();
            view.patch(2, vec![prompt], cx);
            assert_eq!(view.mono_group_start(3), 3);
        });
    }

    #[gpui::test]
    fn older_mono_pages_preserve_the_reading_anchor_and_search_reaches_archived_rows(
        cx: &mut TestAppContext,
    ) {
        let view = cx.new(|_| TranscriptView::new());
        let (key, offset) = view.update(cx, |view, cx| {
            view.mono_mode("mono", 0xffffff, "Mono");
            view.install_prepared_history(PreparedHistory::new(long_mono(35), true), cx);
            view.list.pause_following_tail();
            let offset = ListOffset {
                item_ix: 4,
                offset_in_item: px(7.),
            };
            view.list.scroll_to(offset);
            let key = view.rows[4].key;
            view.load_older_history(cx);
            view.load_older_history(cx);
            (key, offset.offset_in_item)
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert!(!view.history_loading);
            assert_eq!(
                view.messages[0].start, 30,
                "one request adds ten earlier turns"
            );
            let restored = view.list.logical_scroll_top();
            assert_eq!(view.rows[restored.item_ix].key, key);
            assert_eq!(restored.offset_in_item, offset);
            view.reveal_search(1, "Answer", cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, _| {
            assert_eq!(view.search_match, Some((1, "Answer".into())));
            assert!(view.messages.iter().any(|message| message.start == 1));
            assert_eq!(view.source.len(), 70);
        });
    }

    #[gpui::test]
    fn replacing_history_discards_an_in_flight_mono_page(cx: &mut TestAppContext) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.mono_mode("mono", 0xffffff, "Mono");
            view.install_prepared_history(PreparedHistory::new(long_mono(25), true), cx);
            view.load_older_history(cx);
            view.replace_history(long_mono(1), cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, _| {
            assert!(!view.history_loading);
            assert_eq!(view.source.len(), 2);
            assert_eq!(view.messages.len(), 2);
            assert_eq!(view.history_start, 0);
        });
        view.update(cx, |view, cx| view.replace_history(Vec::new(), cx));
        view.update(cx, |view, _| {
            assert!(view.source.is_empty() && view.messages.is_empty());
            assert_eq!(view.history_start, 0);
        });
    }

    fn rows() -> Vec<Row> {
        vec![
            Row {
                role: "user".into(),
                text: "Find the needle".into(),
                first: true,
                sent_at: Some(7),
            },
            Row {
                role: "assistant".into(),
                text: "needle answer\n\nsecond paragraph".into(),
                first: true,
                sent_at: Some(8),
            },
        ]
    }

    #[gpui::test]
    fn suspended_history_releases_payload_and_restores_search_folds_and_scroll(
        cx: &mut TestAppContext,
    ) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.visible(false, cx);
            let original = rows();
            view.patch(0, original.clone(), cx);
            view.reveal_search(1, "needle", cx);
            view.phases.insert(1, true);
            let offset = view.list.logical_scroll_top();
            let key = view.rows.get(offset.item_ix).unwrap().key;
            assert!(view.history_weight() > 0);
            let epoch = view.highlight_epoch;
            assert!(view.suspend_history(cx));
            assert!(view.source.is_empty() && view.rows.is_empty() && view.messages.is_empty());
            assert_eq!(view.history_weight(), 0);
            assert_ne!(view.highlight_epoch, epoch);
            assert!(!view.suspend_history(cx));
            view.restore_history(original.clone(), cx);
            assert_eq!(view.source.len(), original.len());
            assert_eq!(view.source[0].sent_at, Some(7));
            assert_eq!(view.search_match, Some((1, "needle".into())));
            assert!(view.expanded.contains(&1));
            assert_eq!(view.phases.get(&1), Some(&true));
            assert!(!view.list.is_following_tail());
            let restored = view.list.logical_scroll_top();
            assert_eq!(view.rows[restored.item_ix].key, key);
            assert_eq!(restored.offset_in_item, offset.offset_in_item);
        });
    }

    #[gpui::test]
    fn visible_running_and_unsaved_action_histories_are_pinned(cx: &mut TestAppContext) {
        let view = cx.new(|_| TranscriptView::new());
        view.update(cx, |view, cx| {
            view.patch(0, rows(), cx);
            assert!(!view.suspend_history(cx));
            view.visible(false, cx);
            view.streaming = true;
            assert!(!view.suspend_history(cx));
            view.streaming = false;
            view.note_pending.insert(1);
            assert!(!view.suspend_history(cx));
            view.note_pending.clear();
            assert!(view.suspend_history(cx));
            view.restore_history(Vec::new(), cx);
            assert!(view.source.is_empty());
            assert!(view.history_position.is_none());
        });
    }
}
