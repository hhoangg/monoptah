//! Link mode for read-only text: http(s) links in the content are coloured,
//! underlined on hover and opened by a plain click.
use super::*;
use monocode_core::user_links::find_web_links;

impl Input {
    /// Every http(s) link in the content as (bytes, address). Empty unless
    /// link mode is on.
    pub(crate) fn link_ranges(&self) -> Vec<(Range<usize>, String)> {
        if !self.links {
            return Vec::new();
        }
        find_web_links(&self.content)
            .into_iter()
            .map(|found| (found.range, found.link.url))
            .collect()
    }

    /// The link under a window position. Unlike text selection this is exact:
    /// the space after a line's last character is not part of its link.
    pub(super) fn link_at(&self, position: Point<Pixels>) -> Option<(Range<usize>, String)> {
        if !self.links || self.content.is_empty() {
            return None;
        }
        let bounds = self.bounds.filter(|b| b.contains(&position))?;
        let mut y = position.y - bounds.origin.y + self.scroll_y;
        let x = position.x - bounds.origin.x + self.scroll_x;
        let line_height = self.line_height();
        let mut base = 0;
        for line in &self.lines {
            let height = line.size(line_height).height;
            if y < height {
                let at = base
                    + line
                        .index_for_position(point(x, y.max(px(0.))), line_height)
                        .ok()?;
                return self
                    .link_ranges()
                    .into_iter()
                    .find(|(range, _)| range.contains(&at));
            }
            y -= height;
            base += line.len() + 1;
        }
        None
    }

    /// A plain left press (no shift, not a double or triple click) on a link.
    pub(super) fn pressed_link(&self, e: &MouseDownEvent) -> Option<String> {
        if e.click_count != 1 || e.modifiers.shift {
            return None;
        }
        self.link_at(e.position).map(|(_, url)| url)
    }

    /// Opens the pressed link if the release is still on it and the press did
    /// not turn into a selection.
    pub(super) fn open_pressed_link(&mut self, e: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(url) = self.link_press.take() else {
            return;
        };
        if self.selection.is_empty() && self.link_at(e.position).is_some_and(|(_, u)| u == url) {
            cx.open_url(&url);
        }
    }

    pub(super) fn link_hover(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.links {
            return;
        }
        let hovered = self.link_at(e.position).map(|(range, _)| range);
        if hovered != self.hovered_link {
            self.hovered_link = hovered;
            cx.notify();
        }
    }

    pub(super) fn link_leave(&mut self, cx: &mut Context<Self>) {
        if self.hovered_link.take().is_some() {
            cx.notify();
        }
    }
}

#[cfg(test)]
impl Input {
    /// A window position inside the `nth` link, for clicks in tests.
    pub(crate) fn link_point(&self, nth: usize) -> Point<Pixels> {
        let (range, _) = self.link_ranges().swap_remove(nth);
        let middle = range.start + (range.end - range.start) / 2;
        self.bounds.expect("painted").origin
            + self.position(middle)
            + point(px(0.), self.line_height() / 2.)
            - point(self.scroll_x, self.scroll_y)
    }
}
