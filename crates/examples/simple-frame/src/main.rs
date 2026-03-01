//! # Simple Frame Example
//!
//! The smallest complete ratatui application: a bordered frame with a counter
//! that you increment and decrement with the arrow keys.
//!
//! Start here before reading the other examples — this one introduces the
//! fundamental skeleton that every ratatui app is built on.
//!
//! ## Key concepts covered
//!
//! - **[`ratatui::run`]** — the recommended entry point. It sets up raw mode
//!   and the alternate screen, hands you a [`DefaultTerminal`], and cleans up
//!   on exit (including on panic).
//!
//! - **The draw loop** — ratatui re-renders the *entire* UI every frame. There
//!   is no retained widget tree or diffing of state; you just describe what the
//!   screen looks like right now and ratatui handles the rest.
//!
//! - **[`Widget`] trait** — the rendering contract. Implement `render(area, buf)`
//!   for your type and it can be painted anywhere on screen.
//!
//! - **[`Buffer`]** — a 2-D grid of styled characters. Widgets write into it;
//!   ratatui compares it to the previous frame and flushes only the changed
//!   cells to the real terminal.
//!
//! - **[`Rect`]** — a plain `{ x, y, width, height }` value that describes
//!   which region of the buffer a widget may write to.
//!
//! - **[`Block`]** — a bordered container widget. Can carry titles at the top
//!   and bottom, and reports the inner [`Rect`] available for content.
//!
//! - **[`Paragraph`]** — a widget that renders [`Text`] with optional alignment
//!   and word-wrap. It can *own* a [`Block`], handling the border and content
//!   in a single `.render` call.
//!
//! - **[`Line`] / [`Text`]** — the text primitives. A [`Line`] is a row of
//!   styled [`Span`]s; a [`Text`] is a sequence of [`Line`]s.
//!
//! - **[`Stylize`]** — an extension trait that adds `.bold()`, `.blue()`,
//!   `.yellow()`, etc. directly to strings, [`Span`]s, [`Line`]s, and more.
//!
//! [`Span`]: ratatui::text::Span
//! [`Stylize`]: ratatui::style::Stylize

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget},
    DefaultTerminal, Frame,
};

// `ratatui::run` initialises the terminal (raw mode, alternate screen), calls
// your closure, then restores the terminal on exit — even if the closure panics.
fn main() -> io::Result<()> {
    ratatui::run(|terminal| App::default().run(terminal))
}

/// All mutable state for the application.
///
/// Ratatui itself holds no app state — that is entirely your responsibility.
/// The terminal is a stateless surface you repaint every frame.
#[derive(Debug, Default)]
pub struct App {
    counter: u8,
    exit: bool,
}

impl App {
    /// The main loop: draw the current state, then block on the next input event.
    ///
    /// `terminal.draw` accepts a closure that receives a [`Frame`]. The frame
    /// gives you the full-screen [`Rect`] via `frame.area()` and lets you call
    /// `render_widget`. Rendering is synchronous — `draw` returns only after
    /// the closure finishes and the frame has been flushed to the terminal.
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    /// Entry point for rendering.
    ///
    /// We implement [`Widget`] for `&App`, so `self` can be passed directly to
    /// `render_widget`. `frame.area()` returns the full terminal size as a [`Rect`].
    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    /// Blocks until the next terminal event arrives.
    ///
    /// Crossterm emits key *press*, *release*, and *repeat* events on Windows.
    /// Checking for [`KeyEventKind::Press`] ensures we react only once per keystroke.
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
            KeyCode::Char('q') => self.exit(),
            KeyCode::Left => self.decrement_counter(),
            KeyCode::Right => self.increment_counter(),
            _ => {}
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }

    fn increment_counter(&mut self) {
        self.counter += 1;
    }

    fn decrement_counter(&mut self) {
        self.counter -= 1;
    }
}

/// [`Widget`] is the core rendering trait in ratatui.
///
/// Implemented for `&App` (shared reference) rather than `App` so that
/// rendering cannot mutate state. The borrow checker enforces the separation:
/// state changes happen in `handle_events`; rendering happens here.
impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // A `Line` is a horizontal row of text. When built from a single string,
        // it contains one `Span`. `Stylize` methods like `.bold()` are available
        // directly on `&str` and return a styled `Span`, which `Line::from` accepts.
        let title = Line::from(" Counter App Tutorial ".bold());

        // A `Line` can hold multiple `Span`s with independent styles.
        // `.into()` converts a plain `&str` into an unstyled `Span`.
        // Styled spans are created by calling `Stylize` methods on the string.
        let instructions = Line::from(vec![
            " Decrement ".into(),
            "<Left>".blue().bold(),
            " Increment ".into(),
            "<Right>".blue().bold(),
            " Quit ".into(),
            "<Q> ".blue().bold(),
        ]);

        // `Block::bordered()` draws a single-line border on all four sides.
        // `.title` places a `Line` at the top; `.title_bottom` places one at
        // the bottom. `.centered()` is a `Line` method that sets its alignment.
        // `border::THICK` swaps in heavier box-drawing characters.
        let block = Block::bordered()
            .title(title.centered())
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        // `Text` is a collection of `Line`s. Here we build one line that itself
        // contains two spans: a plain label and a yellow-styled counter value.
        let counter_text = Text::from(vec![Line::from(vec![
            "Value: ".into(),
            self.counter.to_string().yellow(),
        ])]);

        // `Paragraph` wraps `Text` and adds layout options (alignment, wrapping).
        // Crucially, it can *own* a `Block` via `.block(block)`. When rendered,
        // `Paragraph` draws the block's border and then renders the text into the
        // block's inner rect automatically — no need to call `block.inner()` or
        // render the block separately.
        Paragraph::new(counter_text)
            .centered()
            .block(block)
            .render(area, buf);
    }
}
