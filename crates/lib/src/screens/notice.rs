//! Messages drawn over whatever screen is showing: a short confirmation on
//! the top border, or a box holding what a failed command wrote to stderr.

use std::time::{Duration, Instant};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Padding, Widget},
};

/// How long a notice such as "Copied 12 characters" stays on screen.
const NOTICE_DURATION: Duration = Duration::from_secs(3);

/// Widest an error box gets, borders included.
const ERROR_MAX_WIDTH: u16 = 64;

/// Most lines of error text a box shows.
const ERROR_MAX_LINES: usize = 8;

/// A short-lived confirmation in the top-right corner, or an error that
/// stays there until dismissed.
#[derive(Debug)]
pub(crate) struct Notice {
    text: String,
    is_error: bool,
    /// What a failed command reported. A notice with details is drawn as a
    /// box and stays up until Esc, so there is time to read it.
    details: Option<String>,
    shown_at: Instant,
}

impl Notice {
    pub(crate) fn copied(text: &str) -> Self {
        Self::new(copied_message(text), false)
    }

    pub(crate) fn copy_failed() -> Self {
        Self::new("Copy failed".to_string(), true)
    }

    /// A command finished and has something to say, such as
    /// "Deleted web and 1 volume".
    pub(crate) fn done(text: String) -> Self {
        Self::new(text, false)
    }

    /// `title` says what failed; `details` is the command's own explanation.
    pub(crate) fn command_failed(title: String, details: &str) -> Self {
        Self {
            details: Some(details.to_string()),
            ..Self::new(title, true)
        }
    }

    fn new(text: String, is_error: bool) -> Self {
        Self {
            text,
            is_error,
            details: None,
            shown_at: Instant::now(),
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.is_sticky() || self.shown_at.elapsed() < NOTICE_DURATION
    }

    /// Whether the notice waits for Esc instead of timing out.
    pub(crate) fn is_sticky(&self) -> bool {
        self.details.is_some()
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        match &self.details {
            Some(details) => self.render_box(details, area, buf),
            None => self.render_on_border(area, buf),
        }
    }

    /// Drawn over the top border, where every screen has one, clear of the
    /// centred title.
    fn render_on_border(&self, area: Rect, buf: &mut Buffer) {
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

    /// A box in the top-right corner, just inside the screen's border, sized
    /// to its text.
    fn render_box(&self, details: &str, area: Rect, buf: &mut Buffer) {
        let title = Line::from(format!(" {} ", self.text)).red().bold();
        let hint = Line::from(vec![" Esc".blue().bold(), ": dismiss ".dark_gray()]);

        // Borders and one column of padding on each side.
        const FRAME: u16 = 4;
        let widest_line = details.lines().map(width_of).max().unwrap_or(0) as u16;
        let width = (widest_line + FRAME)
            .max(title.width() as u16 + 2)
            .max(hint.width() as u16 + 2)
            .min(ERROR_MAX_WIDTH)
            .min(area.width.saturating_sub(2));
        if width <= FRAME {
            return;
        }

        let mut lines = wrap(details, (width - FRAME) as usize);
        if lines.len() > ERROR_MAX_LINES {
            lines.truncate(ERROR_MAX_LINES);
            if let Some(last) = lines.last_mut() {
                last.push('…');
            }
        }

        let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
        if height < 3 {
            return;
        }
        let spot = Rect {
            x: area.right().saturating_sub(width + 1),
            y: area.top() + 1,
            width,
            height,
        };

        let block = Block::bordered()
            .title(title)
            .title_bottom(hint.right_aligned())
            .border_style(Style::new().red())
            .padding(Padding::horizontal(1));
        let inner = block.inner(spot);

        Clear.render(spot, buf);
        block.render(spot, buf);
        Text::from(lines.into_iter().map(Line::from).collect::<Vec<_>>()).render(inner, buf);
    }
}

fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

/// Breaks `text` into lines at most `width` columns wide: at spaces where it
/// can, and inside a word only when the word alone is too wide. The text's
/// own line breaks, blank lines included, are kept.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for paragraph in text.lines() {
        let mut line = String::new();
        let mut line_width = 0;

        for word in paragraph.split_whitespace() {
            let word_width = width_of(word);
            if line_width > 0 && line_width + 1 + word_width <= width {
                line.push(' ');
                line.push_str(word);
                line_width += 1 + word_width;
                continue;
            }
            if line_width > 0 {
                lines.push(std::mem::take(&mut line));
                line_width = 0;
            }
            if word_width <= width {
                line.push_str(word);
                line_width = word_width;
                continue;
            }
            // Longer than a whole line: cut it wherever the edge falls.
            for ch in word.chars() {
                let ch_width = width_of(ch.encode_utf8(&mut [0; 4]));
                if line_width + ch_width > width {
                    lines.push(std::mem::take(&mut line));
                    line_width = 0;
                }
                line.push(ch);
                line_width += ch_width;
            }
        }

        lines.push(line);
    }

    lines
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

    fn row(buf: &Buffer, y: u16) -> String {
        (buf.area.left()..buf.area.right())
            .map(|x| buf[(x, y)].symbol())
            .collect()
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
    fn a_command_failure_stays_until_dismissed() {
        let mut notice = Notice::command_failed("Couldn't start web".to_string(), "not found");
        notice.shown_at -= NOTICE_DURATION * 100;
        assert!(notice.is_sticky());
        assert!(notice.is_visible());
        assert!(!Notice::copy_failed().is_sticky());
    }

    #[test]
    fn the_notice_sits_on_the_top_border_at_the_right() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        Notice::copied("bravo").render(buf.area, &mut buf);
        assert_eq!(row(&buf, 0), format!("{:>38}  ", " Copied 5 characters "));
    }

    #[test]
    fn a_failure_box_shows_the_title_and_the_whole_error() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 8));
        Notice::command_failed(
            "Couldn't start web".to_string(),
            "get failed: container web not found",
        )
        .render(buf.area, &mut buf);

        // Sized to the error, one column in from the screen's right border.
        let indent = " ".repeat(10);
        assert_eq!(
            row(&buf, 1),
            format!("{indent}┌ Couldn't start web {}┐ ", "─".repeat(17))
        );
        assert_eq!(
            row(&buf, 2),
            format!("{indent}│ get failed: container web not found │ ")
        );
        assert_eq!(
            row(&buf, 3),
            format!("{indent}└{} Esc: dismiss ┘ ", "─".repeat(23))
        );
        // Nothing drawn over the top border or below the box.
        assert_eq!(row(&buf, 0).trim(), "");
        assert_eq!(row(&buf, 4).trim(), "");
    }

    #[test]
    fn a_long_error_wraps_inside_the_box() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        Notice::command_failed(
            "Couldn't stop db".to_string(),
            "failed to stop container because it is busy",
        )
        .render(buf.area, &mut buf);

        assert_eq!(row(&buf, 2), " │ failed to stop container │ ");
        assert_eq!(row(&buf, 3), " │ because it is busy       │ ");
        assert!(row(&buf, 4).starts_with(" └"));
    }

    #[test]
    fn wrap_breaks_at_spaces_and_keeps_line_breaks() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert_eq!(wrap("first\n\nthird", 20), ["first", "", "third"]);
    }

    #[test]
    fn wrap_cuts_a_word_that_is_wider_than_a_line() {
        assert_eq!(wrap("abcdefgh ij", 3), ["abc", "def", "gh", "ij"]);
        assert_eq!(wrap("日本語", 4), ["日本", "語"]);
    }

    #[test]
    fn a_very_long_error_is_cut_short() {
        let details = (1..=20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 24));
        Notice::command_failed("Couldn't stop db".to_string(), &details).render(buf.area, &mut buf);

        let body: Vec<String> = (2..2 + ERROR_MAX_LINES as u16)
            .map(|y| row(&buf, y).trim().to_string())
            .collect();
        assert!(body[0].contains("line 1 "), "{body:?}");
        assert!(body[ERROR_MAX_LINES - 1].contains("line 8…"), "{body:?}");
        assert!(row(&buf, 2 + ERROR_MAX_LINES as u16).contains('└'));
    }
}
