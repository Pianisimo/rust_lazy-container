//! # Panel Selection Example
//!
//! Demonstrates core ratatui concepts through a two-panel UI with keyboard-driven
//! focus and row selection.
//!
//! ## Key concepts covered
//!
//! - **The draw loop** — ratatui works by re-rendering the entire UI every frame.
//!   State lives in your app struct; the terminal is just a surface to paint onto.
//!
//! - **[`Widget`] trait** — the rendering contract. Implement it for your type and
//!   ratatui can paint it anywhere given a [`Rect`] and a [`Buffer`].
//!
//! - **[`Buffer`]** — a 2-D grid of styled characters representing one frame.
//!   Widgets write into it; ratatui diffs it against the previous frame and flushes
//!   only the changed cells to the terminal.
//!
//! - **[`Rect`]** — a plain `{ x, y, width, height }` value. Every widget receives
//!   one that describes exactly which region of the buffer it may write to.
//!
//! - **[`Layout`]** — splits a [`Rect`] into smaller [`Rect`]s according to
//!   [`Constraint`]s, without any heap allocation or retained state.
//!
//! - **[`Block`]** — a bordered container. It draws its own border into a [`Rect`]
//!   and can tell you the inner [`Rect`] that remains for content.
//!
//! - **[`Line`] / [`Text`]** — the text primitives. A [`Line`] is a row of styled
//!   [`Span`]s; a [`Text`] is a sequence of [`Line`]s. Both implement [`Widget`].
//!
//! - **[`Stylize`]** — an extension trait that lets you call `.bold()`, `.cyan()`,
//!   etc. directly on strings and other text types.
//!
//! [`Span`]: ratatui::text::Span

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Widget},
};

const ITEM_COUNT: usize = 8;

// `ratatui::run` initializes the terminal (raw mode, alternate screen), calls
// your closure, then restores the terminal on exit — even if the closure panics.
fn main() -> io::Result<()> {
    ratatui::run(|terminal| App::default().run(terminal))
}

/// All mutable state for the application.
///
/// Ratatui itself holds no app state — that is entirely your responsibility.
/// The terminal is stateless from your perspective; you repaint it every frame.
#[derive(Debug)]
pub struct App {
    /// Index of the panel that currently has focus (0 = Left, 1 = Right).
    active_panel: usize,
    /// Each panel independently tracks its own selected row.
    selected_rows: [usize; 2],
    /// The list items shared across both panels.
    items: Vec<String>,
    exit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            active_panel: 0,
            selected_rows: [0, 0],
            items: (1..=ITEM_COUNT).map(|i| format!("Line {i}")).collect(),
            exit: false,
        }
    }
}

impl App {
    /// The main loop: draw then block on the next input event, repeat.
    ///
    /// `terminal.draw` accepts a closure that receives a [`Frame`]. The frame
    /// gives you the full-screen [`Rect`] and lets you call `render_widget`.
    /// It returns only after the closure finishes, so rendering is synchronous.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    /// Entry point for rendering. We implement [`Widget`] for `&App`, so we can
    /// pass `self` directly. `frame.area()` is the full terminal size as a [`Rect`].
    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    /// Blocks until the next terminal event arrives.
    ///
    /// Crossterm emits key *press*, *release*, and *repeat* events on Windows.
    /// Checking for [`KeyEventKind::Press`] ensures we only react once per key.
    fn handle_events(&mut self) -> io::Result<()> {
        match event::read()? {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event)
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Tab | KeyCode::Left | KeyCode::Char('h') => self.switch_panel_left(),
            KeyCode::Right | KeyCode::Char('l') => self.switch_panel_right(),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.move_down(),
            _ => {}
        }
    }

    fn switch_panel_left(&mut self) {
        if self.active_panel > 0 {
            self.active_panel -= 1;
        }
    }

    fn switch_panel_right(&mut self) {
        if self.active_panel < 1 {
            self.active_panel += 1;
        }
    }

    fn move_up(&mut self) {
        let row = &mut self.selected_rows[self.active_panel];
        if *row > 0 {
            *row -= 1;
        }
    }

    fn move_down(&mut self) {
        let row = &mut self.selected_rows[self.active_panel];
        if *row < ITEM_COUNT - 1 {
            *row += 1;
        }
    }

    /// Renders one panel into `area`.
    ///
    /// Notice the split between *layout* and *content*:
    ///
    /// 1. Build a [`Block`] — this defines the border and title but does not
    ///    paint anything yet.
    /// 2. Call `block.inner(area)` to get the [`Rect`] that sits inside the
    ///    border — this is where content goes.
    /// 3. Render the block first, then render content into `inner`. Order
    ///    matters: widgets write into the [`Buffer`] in sequence, and a later
    ///    write can overwrite an earlier one.
    fn render_panel(&self, panel_idx: usize, label: &str, area: Rect, buf: &mut Buffer) {
        let is_active = self.active_panel == panel_idx;
        let selected = self.selected_rows[panel_idx];

        // `Block::bordered()` draws a single-line border on all four sides.
        // `.border_style` colours just the border characters, leaving the
        // interior style untouched. `Style::new().cyan()` sets the foreground.
        let block = Block::bordered()
            .title(format!(" {label} "))
            .border_style(if is_active {
                Style::new().cyan()
            } else {
                Style::new().dark_gray()
            });

        // Reserve the inner area *before* consuming `block` with `.render`.
        let inner = block.inner(area);
        block.render(area, buf);

        // Build the item list as a `Vec<Line>`. A `Line` is a horizontal row
        // of text. Here each line is a single unstyled (or styled) string, but
        // a `Line` can hold multiple `Span`s with different styles.
        //
        // `Stylize` methods (`.yellow()`, `.bold()`) are available on `&str`,
        // `String`, `Span`, `Line`, and `Text` — they all return the same type
        // with a style applied, so you can chain them fluently.
        let lines: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if i == selected {
                    Line::from(format!(" > {item}")).yellow().bold()
                } else {
                    Line::from(format!("   {item}"))
                }
            })
            .collect();

        // `Text` is a collection of `Line`s and itself implements `Widget`.
        // Calling `.render(inner, buf)` paints it top-to-bottom inside the
        // inner rect, one `Line` per terminal row.
        Text::from(lines).render(inner, buf);
    }
}

/// [`Widget`] is the core rendering trait in ratatui.
///
/// The trait is implemented for `&App` (shared reference) rather than `App`
/// because rendering should not mutate state. The borrow checker enforces the
/// separation: state changes happen in `handle_events`, rendering in `render`.
///
/// `render` receives:
/// - `area: Rect` — the region this widget is allowed to paint into.
/// - `buf: &mut Buffer` — the frame's cell grid; write styled characters here.
impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let panel_name = if self.active_panel == 0 {
            "Left"
        } else {
            "Right"
        };
        let row_num = self.selected_rows[self.active_panel] + 1;

        // A `Line` can be built from a mix of plain `&str`s and styled `Span`s.
        // `.into()` converts a `&str` into a plain `Span`; styled values are
        // created via `Stylize` methods directly on the string literal.
        let title = Line::from(" Panel Selection ".bold());
        let status = Line::from(vec![
            " Panel: ".into(),
            panel_name.cyan().bold(),
            " | Row: ".into(),
            row_num.to_string().yellow().bold(),
            "  ←/→: switch  ↑/↓: navigate  q: quit ".dark_gray(),
        ]);

        // `border::THICK` swaps in box-drawing characters for a heavier border.
        // `.title` places text at the top; `.title_bottom` places it at the bottom.
        // `.centered()` is a `Line` method that sets the alignment.
        let outer_block = Block::bordered()
            .title(title.centered())
            .title_bottom(status.centered())
            .border_set(border::THICK);

        let inner = outer_block.inner(area);
        outer_block.render(area, buf);

        // `Layout` divides a `Rect` into sub-rects according to `Constraint`s.
        //
        // `Constraint::Percentage(50)` means "take 50 % of the available space".
        // Other variants include `Min`, `Max`, `Length` (fixed columns), and
        // `Fill` (greedy, takes remaining space after fixed constraints are met).
        //
        // `.areas(inner)` returns an array of `Rect`s — one per constraint —
        // typed by const generic so the compiler enforces the count matches.
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(inner);

        self.render_panel(0, "Left", left_area, buf);
        self.render_panel(1, "Right", right_area, buf);
    }
}
