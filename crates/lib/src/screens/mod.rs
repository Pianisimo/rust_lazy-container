use std::io;
use std::process::Command;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{DefaultTerminal, Frame, buffer::Buffer, layout::Rect, widgets::Widget};

mod home_screen;
mod status_screen;

pub(crate) use home_screen::HomeScreen;
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
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::default(),
            exit: false,
        }
    }
}

impl App {
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();
        while !self.exit {
            if last_refresh.elapsed() >= STATUS_REFRESH_INTERVAL {
                self.refresh_status();
                last_refresh = Instant::now();
            }
            self.auto_navigate();
            terminal.draw(|frame| self.draw(frame))?;
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
                    self.handle_key_event(key_event)
                }
                _ => {}
            };
        }
        Ok(())
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

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match &self.screen {
            Screen::Status(s) => s.render(area, buf),
            Screen::Home(h) => h.render(area, buf),
        }
    }
}
