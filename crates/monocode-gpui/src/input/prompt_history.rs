//! Terminal-style prompt recall: Up on the first line steps back through the
//! prompts already sent in this chat, Down on the last line steps forward and
//! finally returns the draft that was being typed.
//!
//! The prompts come from the owner (a conversation reads its transcript), so
//! they survive restarts. Recalled text is a copy: editing it never changes
//! the history.
use super::{Input, edit_history::Snapshot};
use gpui::{App, Context};

pub(crate) type PromptSource = Box<dyn Fn(&App) -> Vec<String>>;

pub(super) struct PromptHistory {
    source: PromptSource,
    /// Most recent first, read when browsing starts.
    entries: Vec<String>,
    /// The entry shown, if browsing.
    index: Option<usize>,
    /// What was in the field when browsing started.
    draft: Option<Snapshot>,
}

/// Sent prompts oldest first, as the transcript lists them, to most recent
/// first with blank prompts and consecutive duplicates dropped.
pub(crate) fn recall_order(prompts: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for prompt in prompts {
        let prompt = prompt.trim().to_owned();
        if prompt.is_empty() || entries.last() == Some(&prompt) {
            continue;
        }
        entries.push(prompt);
    }
    entries.reverse();
    entries
}

impl Input {
    /// Enable Up/Down prompt recall, reading sent prompts (oldest first) from
    /// `source` each time browsing starts.
    pub(crate) fn set_prompt_history(&mut self, source: PromptSource) {
        self.prompt_history = Some(PromptHistory {
            source,
            entries: Vec::new(),
            index: None,
            draft: None,
        });
    }

    #[cfg(test)]
    pub(crate) fn browsing_prompt_history(&self) -> bool {
        self.prompt_history
            .as_ref()
            .is_some_and(|history| history.index.is_some())
    }

    /// Forget the browsing position, keeping the field's text.
    pub(super) fn reset_prompt_history(&mut self) {
        if let Some(history) = &mut self.prompt_history {
            history.index = None;
            history.draft = None;
            history.entries.clear();
        }
    }

    fn caret_on_edge_line(&self, last: bool) -> bool {
        let at = self.cursor();
        if self.lines.is_empty() {
            return if last {
                !self.content[at..].contains('\n')
            } else {
                !self.content[..at].contains('\n')
            };
        }
        let y = self.position(at).y;
        if last {
            let end = self.position(self.content.len()).y;
            y >= end
        } else {
            y < self.line_height()
        }
    }

    /// Up or Down was pressed with no popup claiming it. Returns whether the
    /// key recalled a prompt; if not, the caret moves as usual.
    pub(super) fn step_prompt_history(&mut self, older: bool, cx: &mut Context<Self>) -> bool {
        if self.prompt_history.is_none()
            || self.code.is_some()
            || self.single_line
            || self.secret
            || self.read_only
            || self.is_composing()
            || !self.selection.is_empty()
            || !self.caret_on_edge_line(!older)
        {
            return false;
        }
        let current = self.snapshot();
        let history = self.prompt_history.as_mut().unwrap();
        let next = match (history.index, older) {
            (None, false) => return false,
            (None, true) => {
                history.entries = (history.source)(cx);
                if history.entries.is_empty() {
                    return false;
                }
                history.draft = Some(current);
                Some(0)
            }
            (Some(ix), true) if ix + 1 < history.entries.len() => Some(ix + 1),
            // Already at the oldest prompt: the key does nothing.
            (Some(_), true) => return true,
            (Some(0), false) => None,
            (Some(ix), false) => Some(ix - 1),
        };
        history.index = next;
        let restore = match next {
            Some(ix) => {
                let text = history.entries[ix].clone();
                let end = text.len();
                Snapshot {
                    content: text.into(),
                    selection: end..end,
                    reversed: false,
                }
            }
            None => {
                history.entries.clear();
                history.draft.take().unwrap_or_else(|| Snapshot {
                    content: "".into(),
                    selection: 0..0,
                    reversed: false,
                })
            }
        };
        self.apply_programmatic(restore, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_order_is_most_recent_first_without_consecutive_duplicates() {
        let sent = ["one", "two", "two", " ", "one", "three\n"].map(String::from);
        assert_eq!(recall_order(sent), ["three", "one", "two", "one"]);
    }
}
