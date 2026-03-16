use std::io;
use std::process::Command;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    buffer::Buffer, layout::Rect,
    widgets::Widget,
    DefaultTerminal,
    Frame,
};

mod home_screen;
mod status_screen;

pub(crate) use home_screen::HomeScreen;
pub(crate) use status_screen::StatusScreen;

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
            KeyCode::Enter => StatusScreen::handle_enter(self),
            KeyCode::Char('s') => StatusScreen::handle_start_stop(self),
            KeyCode::Esc => HomeScreen::handle_esc(self),
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
