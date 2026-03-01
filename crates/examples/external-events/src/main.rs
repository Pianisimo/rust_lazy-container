//! # External Events Example
//!
//! Demonstrates how to integrate an external event source — a file-system
//! watcher — with ratatui's event loop.
//!
//! ## The core problem
//!
//! `crossterm::event::read()` **blocks** the calling thread until a key is
//! pressed. That works fine in simple apps, but the moment you have another
//! source of events (a timer, a channel, a socket, a watcher), blocking
//! forever on keyboard input makes those sources invisible.
//!
//! The solution is `event::poll(timeout)`:
//! - Returns `true` immediately if a key event is ready.
//! - Returns `false` after `timeout` elapses with no key event.
//! - Either way, it does **not** block longer than `timeout`.
//!
//! This gives the main loop a heartbeat — even with no keyboard activity it
//! wakes up regularly and can service any other event source.
//!
//! ## What the example does
//!
//! A `dummy/` directory (next to this crate's `Cargo.toml`) is watched by the
//! [`notify`] crate. Three menu actions let you create and delete files there.
//! The watcher runs on a background thread and posts events onto an
//! `mpsc` channel. Every loop iteration the main thread drains that channel
//! and refreshes the displayed file list when anything arrives.
//!
//! The key architectural insight: **the watcher, not the action, is the source
//! of truth for the UI update.** When "Create dummy file" is selected, the app
//! writes the file and *waits* for the watcher to confirm the change before
//! updating the list. This mirrors how real apps separate writes from reads.
//!
//! ## Key concepts covered
//!
//! - **`event::poll` vs `event::read`** — `poll` with a timeout is the
//!   standard escape hatch for mixing keyboard input with other I/O.
//!
//! - **`mpsc` as an event bridge** — the watcher callback runs on a
//!   notify-managed thread; an `mpsc::Sender` in the callback and an
//!   `mpsc::Receiver` on the main thread is the idiomatic way to cross that
//!   boundary safely.
//!
//! - **`notify` crate** — cross-platform file-system change notifications
//!   backed by OS primitives (FSEvents on macOS, inotify on Linux,
//!   ReadDirectoryChanges on Windows). `recommended_watcher` picks the right
//!   backend automatically.
//!
//! - **Keeping the watcher alive** — [`notify::RecommendedWatcher`] stops
//!   watching the moment it is dropped. Storing it as a field in `App`
//!   (prefixed with `_` to silence the unused warning without triggering an
//!   early drop) ties its lifetime to the application.
//!
//! - **Draining the channel** — a single file write can trigger several OS
//!   events (create, modify, close). Draining the full queue with `try_recv`
//!   in a loop and refreshing only once avoids redundant directory reads.

use std::{fs, io, path::Path, sync::mpsc, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Widget},
};

/// Absolute path to the watched directory, resolved at compile time from the
/// crate's location. Using `env!("CARGO_MANIFEST_DIR")` means this is correct
/// regardless of which directory `cargo run` is invoked from.
const DUMMY_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/dummy");

const ACTIONS: &[&str] = &["Create dummy file", "Remove last", "Remove all"];

fn main() -> io::Result<()> {
    // Make sure the watched directory exists before we hand it to the watcher.
    fs::create_dir_all(DUMMY_DIR)?;
    ratatui::run(|terminal| App::new()?.run(terminal))
}

/// All application state.
///
/// The three fields related to the watcher deserve attention:
///
/// - `watcher_rx` — the receiving end of the channel. The main loop calls
///   `try_recv` on it every iteration.
/// - `_watcher` — the watcher handle. It does nothing actively; it just needs
///   to stay alive. The leading `_` suppresses the "unused variable" warning
///   while keeping the value in scope. If you bound it to `_` (no name) it
///   would drop immediately.
/// - `status` — a one-line message updated by both actions and watcher events,
///   giving the user real-time feedback about what is happening.
struct App {
    /// Index into `ACTIONS` for the currently highlighted menu item.
    selected: usize,
    /// Sorted list of `dummy*.txt` file names, refreshed whenever the watcher fires.
    files: Vec<String>,
    /// One-line feedback message shown at the bottom.
    status: String,
    exit: bool,
    watcher_rx: mpsc::Receiver<notify::Result<notify::Event>>,
    _watcher: RecommendedWatcher,
}

impl App {
    fn new() -> io::Result<Self> {
        // The channel carries raw `notify::Result<notify::Event>` values.
        // We use the stdlib mpsc rather than anything async — no executor needed.
        let (tx, rx) = mpsc::channel();

        // `recommended_watcher` selects the best OS backend and calls the
        // closure on a background thread each time an event arrives.
        // We move `tx` into the closure so each event is forwarded to `rx`.
        let mut watcher = notify::recommended_watcher(move |res| {
            // `ok()` silently discards send errors that occur if the main
            // thread has already exited and `rx` has been dropped.
            tx.send(res).ok();
        })
        .map_err(io::Error::other)?;

        // `RecursiveMode::NonRecursive` watches only the directory itself,
        // not any subdirectories — appropriate here since we create flat files.
        watcher
            .watch(Path::new(DUMMY_DIR), RecursiveMode::NonRecursive)
            .map_err(io::Error::other)?;

        let mut app = Self {
            selected: 0,
            files: Vec::new(),
            status: String::from("Watching dummy/ for changes…"),
            exit: false,
            watcher_rx: rx,
            _watcher: watcher,
        };
        app.refresh_files();
        Ok(app)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        // --- Keyboard events -------------------------------------------------
        //
        // `event::poll` is the key to this whole pattern.
        //
        // With `event::read()` (used in the simpler examples) the thread sleeps
        // until a key arrives — nothing else can happen in the meantime.
        //
        // With `event::poll(duration)` the thread sleeps *at most* `duration`.
        // After that it returns `false` and we fall through to check the watcher.
        // 100 ms means the watcher response feels instant to a human.
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    self.handle_key_event(key);
                }
            }
        }

        // --- Watcher events --------------------------------------------------
        //
        // `try_recv` never blocks — it returns `Err(TryRecvError::Empty)` if the
        // queue is empty, which breaks the loop. We drain the entire queue in one
        // go because a single file write can produce several OS-level events
        // (create, data, close-write). Refreshing once after the drain is enough.
        let mut got_change = false;
        while let Ok(result) = self.watcher_rx.try_recv() {
            match result {
                Ok(_event) => got_change = true,
                Err(e) => self.status = format!("Watcher error: {e}"),
            }
        }
        if got_change {
            self.refresh_files();
            self.status = format!(
                "Watcher: change detected — {} file(s) in dummy/",
                self.files.len()
            );
        }

        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected < ACTIONS.len() - 1 {
                    self.selected += 1;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.run_action(),
            _ => {}
        }
    }

    fn run_action(&mut self) {
        match self.selected {
            0 => self.create_file(),
            1 => self.remove_last(),
            2 => self.remove_all(),
            _ => {}
        }
    }

    /// Creates the next `dummyNN.txt` file (01, 02, …).
    ///
    /// Intentionally does **not** call `refresh_files` itself. The watcher will
    /// fire when the OS delivers the notification, which is what triggers the
    /// refresh. This makes the round-trip visible: action writes → OS notifies
    /// → watcher fires → UI updates.
    fn create_file(&mut self) {
        let next = (1u32..=99).find(|&n| {
            !Path::new(DUMMY_DIR)
                .join(format!("dummy{n:02}.txt"))
                .exists()
        });

        match next {
            Some(n) => {
                let path = Path::new(DUMMY_DIR).join(format!("dummy{n:02}.txt"));
                match fs::write(&path, format!("dummy file {n:02}\n")) {
                    Ok(_) => {
                        self.status = format!(
                            "Created {} — waiting for watcher…",
                            path.file_name().unwrap().to_string_lossy()
                        );
                    }
                    Err(e) => self.status = format!("Error creating file: {e}"),
                }
            }
            None => self.status = String::from("Limit reached (dummy01–dummy99)."),
        }
    }

    /// Removes the highest-numbered `dummy*.txt` file.
    fn remove_last(&mut self) {
        match self.dummy_files().into_iter().max() {
            Some(name) => match fs::remove_file(Path::new(DUMMY_DIR).join(&name)) {
                Ok(_) => self.status = format!("Removed {name} — waiting for watcher…"),
                Err(e) => self.status = format!("Error removing file: {e}"),
            },
            None => self.status = String::from("Nothing to remove."),
        }
    }

    /// Removes all `dummy*.txt` files.
    fn remove_all(&mut self) {
        let files = self.dummy_files();
        if files.is_empty() {
            self.status = String::from("Nothing to remove.");
            return;
        }
        let count = files.len();
        for name in &files {
            fs::remove_file(Path::new(DUMMY_DIR).join(name)).ok();
        }
        self.status = format!("Removed {count} file(s) — waiting for watcher…");
    }

    /// Reads `dummy/` from disk and returns sorted `dummy*.txt` names.
    fn dummy_files(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(DUMMY_DIR) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (name.starts_with("dummy") && name.ends_with(".txt")).then_some(name)
            })
            .collect();
        names.sort();
        names
    }

    /// Syncs `self.files` with the real directory contents.
    /// Called on startup and whenever the watcher fires.
    fn refresh_files(&mut self) {
        self.files = self.dummy_files();
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = Line::from(" External Events ".bold());
        let instructions = Line::from("  ↑/↓: select  Enter: run  q: quit  ".dark_gray());

        let outer_block = Block::bordered()
            .title(title.centered())
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        let inner = outer_block.inner(area);
        outer_block.render(area, buf);

        // Vertical layout:
        //   `Fill(1)`    — the two side-by-side panels; expands to fill all
        //                  remaining height after the fixed-height status line.
        //   `Length(1)`  — exactly one row for the status message.
        //
        // `Constraint::Fill` is a "take the rest" constraint. Use it when you
        // want one region to expand and the others to stay at a fixed size.
        let [panels_area, status_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

        let [actions_area, files_area] =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .areas(panels_area);

        self.render_actions(actions_area, buf);
        self.render_files(files_area, buf);
        self.render_status(status_area, buf);
    }
}

impl App {
    fn render_actions(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::bordered().title(" Actions ");
        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line> = ACTIONS
            .iter()
            .enumerate()
            .map(|(i, label)| {
                if i == self.selected {
                    Line::from(format!(" > {label}")).cyan().bold()
                } else {
                    Line::from(format!("   {label}"))
                }
            })
            .collect();

        Text::from(lines).render(inner, buf);
    }

    fn render_files(&self, area: Rect, buf: &mut Buffer) {
        let title = format!(" dummy/ ({} files) ", self.files.len());
        let block = Block::bordered().title(title);
        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line> = if self.files.is_empty() {
            vec![Line::from("  (empty)").dark_gray().italic()]
        } else {
            self.files
                .iter()
                .map(|name| Line::from(format!("  {name}")).yellow())
                .collect()
        };

        Text::from(lines).render(inner, buf);
    }

    /// The status line sits outside any `Block`, so we render a plain `Line`
    /// directly into the one-row `Rect` allocated by the layout.
    /// `Line` implements `Widget`, so it accepts the same `(area, buf)` call.
    fn render_status(&self, area: Rect, buf: &mut Buffer) {
        Line::from(format!("  {}", self.status))
            .dark_gray()
            .render(area, buf);
    }
}
