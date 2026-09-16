use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::Widget,
};

mod home_screen;
mod selection;
mod status_screen;

pub(crate) use home_screen::HomeScreen;
use selection::{Notice, Selection};
pub(crate) use status_screen::{ContainerStatus, StatusScreen};

/// How long the loop waits for a key before doing another pass. Gives the
/// draw loop a heartbeat instead of blocking forever inside `event::read`.
const EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// How often the status screen re-runs `container system status`.
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(crate) enum Screen {
    Status(StatusScreen),
    Home(HomeScreen),
}

impl Default for Screen {
    fn default() -> Self {
        Screen::Status(StatusScreen::fetch())
    }
}

#[derive(Debug)]
pub struct App {
    pub(crate) screen: Screen,
    pub(crate) exit: bool,
    /// The mouse selection being dragged, or the last one, still highlighted.
    selection: Option<Selection>,
    /// The last frame drawn, which is what a selection copies text from.
    last_frame: Buffer,
    notice: Option<Notice>,
}

impl Default for App {
    fn default() -> Self {
        Self::with_screen(Screen::default())
    }
}

/// Holds mouse capture for as long as it lives. `ratatui::run` restores the
/// rest of the terminal but knows nothing about the mouse; dropping this on
/// the way out, panics included, hands selection back to the terminal.
struct MouseCapture;

impl MouseCapture {
    fn enable() -> io::Result<Self> {
        execute!(io::stdout(), EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

impl App {
    pub(crate) fn with_screen(screen: Screen) -> Self {
        Self {
            screen,
            exit: false,
            selection: None,
            last_frame: Buffer::default(),
            notice: None,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let _mouse = MouseCapture::enable()?;
        let mut last_refresh = Instant::now();
        while !self.exit {
            if last_refresh.elapsed() >= STATUS_REFRESH_INTERVAL {
                self.refresh_status();
                last_refresh = Instant::now();
            }
            self.auto_navigate();
            let frame = terminal.draw(|frame| self.draw(frame))?;
            self.last_frame = frame.buffer.clone();
            self.handle_events()?;
        }
        Ok(())
    }

    /// Leaves the status screen for the home screen as soon as the container
    /// system is up, so a running system never stops on an interstitial.
    fn auto_navigate(&mut self) {
        let should_change = matches!(
            &self.screen,
            Screen::Status(s) if s.auto_navigate && s.status == ContainerStatus::Running
        );
        if should_change {
            self.screen = Screen::Home(HomeScreen::fetch());
        }
    }

    /// Keeps the status screen live, so a system started elsewhere is picked up.
    fn refresh_status(&mut self) {
        if let Screen::Status(ref mut status) = self.screen {
            status.refresh();
        }
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        if event::poll(EVENT_POLL_TIMEOUT)? {
            match event::read()? {
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    self.selection = None;
                    self.handle_key_event(key_event)
                }
                Event::Mouse(mouse_event) => self.handle_mouse_event(mouse_event),
                _ => {}
            };
        }
        Ok(())
    }

    /// Left-drag selects; releasing copies whatever the drag covered.
    fn handle_mouse_event(&mut self, mouse_event: MouseEvent) {
        let area = self.last_frame.area;
        if area.is_empty() {
            return;
        }
        // A drag past the edge of the window keeps selecting up to the edge.
        let at = Position::new(
            mouse_event.column.min(area.right() - 1),
            mouse_event.row.min(area.bottom() - 1),
        );

        match mouse_event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = Some(Selection::start(at));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.extend_to(at);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.extend_to(at);
                }
                self.copy_selection();
            }
            _ => {}
        }
    }

    /// Copies the finished selection and says how much went to the clipboard.
    /// A plain click clears the highlight instead.
    fn copy_selection(&mut self) {
        let Some(selection) = self.selection.filter(|s| !s.is_empty()) else {
            self.selection = None;
            return;
        };
        // The highlight only reverses colours, so the last frame still holds
        // exactly the characters the user saw.
        let text = selection.text(&self.last_frame);
        if text.is_empty() {
            return;
        }
        self.notice = Some(match copy_to_clipboard(&text) {
            Ok(()) => Notice::copied(&text),
            Err(_) => Notice::copy_failed(),
        });
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        // The create-container dialog is modal: while it is open it takes every
        // key, so none of the shortcuts below fire.
        if HomeScreen::handle_dialog_key(self, key_event) {
            return;
        }

        match key_event.code {
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Enter => StatusScreen::handle_enter(self),
            KeyCode::Char('s') => match self.screen {
                Screen::Status(_) => StatusScreen::handle_start_stop(self),
                Screen::Home(_) => HomeScreen::handle_start_stop_container(self),
            },
            KeyCode::Esc => HomeScreen::handle_esc(self),
            KeyCode::Char('.') => HomeScreen::cycle_container_filter(self),
            KeyCode::Char('/') => HomeScreen::toggle_help(self),
            KeyCode::Char('c') => HomeScreen::open_create_dialog(self),
            KeyCode::Up if key_event.modifiers.contains(KeyModifiers::SHIFT) => {
                HomeScreen::handle_shift_up(self)
            }
            KeyCode::Down if key_event.modifiers.contains(KeyModifiers::SHIFT) => {
                HomeScreen::handle_shift_down(self)
            }
            KeyCode::Up => HomeScreen::move_up(self),
            KeyCode::Down => HomeScreen::move_down(self),
            _ => {}
        }
    }
}

pub(crate) fn run_command(args: &[&str]) {
    if let Some((cmd, rest)) = args.split_first() {
        let _ = Command::new(cmd).args(rest).output();
    }
}

/// Puts `text` on the system clipboard through `pbcopy`; the container CLI is
/// macOS-only, so the app is too.
pub(crate) fn copy_to_clipboard(text: &str) -> io::Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Dropping stdin closes the pipe, which is what lets `pbcopy` finish.
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("pbcopy has no stdin"))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("pbcopy exited with {status}")))
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match &self.screen {
            Screen::Status(s) => s.render(area, buf),
            Screen::Home(h) => h.render(area, buf),
        }

        if let Some(notice) = self.notice.as_ref().filter(|n| n.is_visible()) {
            notice.render(area, buf);
        }
        if let Some(selection) = &self.selection {
            selection.highlight(buf);
        }
    }
}
