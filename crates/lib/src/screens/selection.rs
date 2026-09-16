//! Mouse text selection. Capturing the mouse takes selection away from the
//! terminal, so the app draws the highlight itself and copies the text under
//! it the moment the button is released.

use std::time::{Duration, Instant};

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Stylize},
    text::{Line, Span},
    widgets::Widget,
};

/// How long a notice such as "Copied 12 characters" stays on screen.
const NOTICE_DURATION: Duration = Duration::from_secs(3);

/// A drag in progress, or the last one, in screen cells. Runs like terminal
/// selection does: from the anchor to the end of its row, whole rows between,
/// and the start of the cursor's row up to the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Selection {
    anchor: Position,
    cursor: Position,
}

impl Selection {
    pub(crate) fn start(at: Position) -> Self {
        Self {
            anchor: at,
            cursor: at,
        }
    }

    pub(crate) fn extend_to(&mut self, at: Position) {
        self.cursor = at;
    }

    /// A click without a drag selects nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    /// The two ends in reading order.
    fn ordered(&self) -> (Position, Position) {
        let key = |p: Position| (p.y, p.x);
        if key(self.anchor) <= key(self.cursor) {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// The selected columns of row `y`, inclusive, within `area`.
    fn columns(&self, y: u16, area: Rect) -> Option<(u16, u16)> {
        let (start, end) = self.ordered();
        if y < start.y || y > end.y || area.width == 0 {
            return None;
        }
        let first = if y == start.y { start.x } else { area.left() };
        let last = if y == end.y { end.x } else { area.right() - 1 };
        Some((first.max(area.left()), last.min(area.right() - 1)))
    }

    /// Reverses the selected cells, the way a terminal shows a selection.
    pub(crate) fn highlight(&self, buf: &mut Buffer) {
        let area = buf.area;
        for y in area.top()..area.bottom() {
            if let Some((first, last)) = self.columns(y, area) {
                for x in first..=last {
                    buf[(x, y)].modifier.insert(Modifier::REVERSED);
                }
            }
        }
    }

    /// The text under the selection, one line per row, with the padding at the
    /// end of each row dropped as a terminal would.
    pub(crate) fn text(&self, buf: &Buffer) -> String {
        let area = buf.area;
        let mut lines = Vec::new();
        for y in area.top()..area.bottom() {
            let Some((first, last)) = self.columns(y, area) else {
                continue;
            };
            let mut line = String::new();
            let mut x = first;
            while x <= last {
                let symbol = buf[(x, y)].symbol();
                line.push_str(symbol);
                // A wide character owns the cells after it; they hold padding.
                x += Span::raw(symbol).width().max(1) as u16;
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }
}

/// A short-lived message in the top-right corner, confirming a copy or
/// reporting that it failed.
#[derive(Debug)]
pub(crate) struct Notice {
    text: String,
    is_error: bool,
    shown_at: Instant,
}

impl Notice {
    pub(crate) fn copied(text: &str) -> Self {
        Self::new(copied_message(text), false)
    }

    pub(crate) fn copy_failed() -> Self {
        Self::new("Copy failed".to_string(), true)
    }

    fn new(text: String, is_error: bool) -> Self {
        Self {
            text,
            is_error,
            shown_at: Instant::now(),
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.shown_at.elapsed() < NOTICE_DURATION
    }

    /// Drawn over the top border, where every screen has one, clear of the
    /// centred title.
    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let text = format!(" {} ", self.text);
        let line = if self.is_error {
            Line::from(text).red().bold()
        } else {
            Line::from(text).green().bold()
        };
        let width = (line.width() as u16).min(area.width.saturating_sub(4));
        let spot = Rect {
            x: area.right().saturating_sub(width + 2),
            y: area.top(),
            width,
            height: 1.min(area.height),
        };
        line.render(spot, buf);
    }
}

/// "Copied 1 character" / "Copied 12 characters". Counts characters, not
/// bytes, so non-ASCII text reads the way it looks.
fn copied_message(text: &str) -> String {
    match text.chars().count() {
        1 => "Copied 1 character".to_string(),
        n => format!("Copied {n} characters"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(lines: &[&str]) -> Buffer {
        Buffer::with_lines(lines.iter().copied())
    }

    fn drag(from: (u16, u16), to: (u16, u16)) -> Selection {
        let mut selection = Selection::start(Position::new(from.0, from.1));
        selection.extend_to(Position::new(to.0, to.1));
        selection
    }

    #[test]
    fn a_single_row_selection_copies_between_the_ends() {
        let buf = buffer(&["  bravo  running  "]);
        assert_eq!(drag((2, 0), (6, 0)).text(&buf), "bravo");
    }

    #[test]
    fn dragging_backwards_selects_the_same_text() {
        let buf = buffer(&["  bravo  running  "]);
        assert_eq!(drag((6, 0), (2, 0)).text(&buf), "bravo");
    }

    #[test]
    fn a_multi_row_selection_flows_like_text() {
        let buf = buffer(&["alpha   ", "bravo   ", "charlie "]);
        // From "pha" through all of "bravo" to "ch".
        assert_eq!(drag((2, 0), (1, 2)).text(&buf), "pha\nbravo\nch");
    }

    #[test]
    fn wide_characters_are_copied_once() {
        let buf = buffer(&["日本 ok"]);
        assert_eq!(drag((0, 0), (6, 0)).text(&buf), "日本 ok");
    }

    #[test]
    fn a_click_is_not_a_selection() {
        assert!(Selection::start(Position::new(3, 1)).is_empty());
        assert!(!drag((3, 1), (4, 1)).is_empty());
    }

    #[test]
    fn highlight_covers_exactly_the_copied_cells() {
        let mut buf = buffer(&["abcd", "efgh"]);
        drag((2, 0), (1, 1)).highlight(&mut buf);
        let reversed: Vec<(u16, u16)> = (0..2)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].modifier.contains(Modifier::REVERSED))
            .collect();
        assert_eq!(reversed, [(2, 0), (3, 0), (0, 1), (1, 1)]);
    }

    #[test]
    fn copied_message_counts_characters_not_bytes() {
        assert_eq!(copied_message("a"), "Copied 1 character");
        assert_eq!(copied_message("café"), "Copied 4 characters");
    }

    #[test]
    fn a_notice_expires() {
        let mut notice = Notice::copied("bravo");
        assert!(notice.is_visible());
        notice.shown_at -= NOTICE_DURATION;
        assert!(!notice.is_visible());
    }

    #[test]
    fn the_notice_sits_on_the_top_border_at_the_right() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        Notice::copied("bravo").render(buf.area, &mut buf);
        let top: String = (0..40).map(|x| buf[(x, 0)].symbol()).collect();
        assert_eq!(top, format!("{:>38}  ", " Copied 5 characters "));
    }
}
