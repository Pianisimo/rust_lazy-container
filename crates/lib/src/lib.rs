use std::io;
use std::process::Command;

pub use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
pub use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::Rect,
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget},
};

mod screens;

use screens::{ContainerScreen, ContainerStatus, Screen, StatusScreen};

#[derive(Debug)]
pub struct App {
    screen: Screen,
    exit: bool,
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
            KeyCode::Enter => self.handle_enter(),
            KeyCode::Char('s') => self.handle_start_stop(),
            KeyCode::Esc => self.handle_esc(),
            _ => {}
        }
    }

    fn handle_enter(&mut self) {
        let is_running =
            matches!(&self.screen, Screen::Status(s) if s.status == ContainerStatus::Running);
        if is_running {
            self.screen = Screen::Containers(ContainerScreen::fetch());
        }
    }

    fn handle_start_stop(&mut self) {
        let is_running = match &self.screen {
            Screen::Status(s) => Some(s.status == ContainerStatus::Running),
            _ => None,
        };
        if let Some(running) = is_running {
            if running {
                run_command(&["container", "system", "stop"]);
            } else {
                run_command(&["container", "system", "start"]);
            }
            self.screen = Screen::Status(StatusScreen::fetch());
        }
    }

    fn handle_esc(&mut self) {
        if matches!(&self.screen, Screen::Containers(_)) {
            self.screen = Screen::Status(StatusScreen::fetch());
        }
    }
}

fn run_command(args: &[&str]) {
    if let Some((cmd, rest)) = args.split_first() {
        let _ = Command::new(cmd).args(rest).output();
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match &self.screen {
            Screen::Status(s) => render_status(s, area, buf),
            Screen::Containers(c) => render_containers(c, area, buf),
        }
    }
}

fn render_status(screen: &StatusScreen, area: Rect, buf: &mut Buffer) {
    let status_span = match screen.status {
        ContainerStatus::Running => "Running".green().bold(),
        ContainerStatus::NotRunning => "Not Running".red().bold(),
    };

    let instructions = match screen.status {
        ContainerStatus::Running => Line::from(vec![
            " Containers ".into(),
            "<Enter>".blue().bold(),
            " Stop ".into(),
            "<S>".blue().bold(),
            " Quit ".into(),
            "<Q> ".blue().bold(),
        ]),
        ContainerStatus::NotRunning => Line::from(vec![
            " Start ".into(),
            "<S>".blue().bold(),
            " Quit ".into(),
            "<Q> ".blue().bold(),
        ]),
    };

    let block = Block::bordered()
        .title(Line::from(" Apple Container System ".bold()).centered())
        .title_bottom(instructions.centered())
        .border_set(border::THICK);

    let content = Text::from(vec![
        Line::from(vec!["Status: ".into(), status_span]),
        Line::from(screen.message.as_str()),
    ]);

    Paragraph::new(content)
        .centered()
        .block(block)
        .render(area, buf);
}

fn render_containers(screen: &ContainerScreen, area: Rect, buf: &mut Buffer) {
    let instructions = Line::from(vec![
        " Back ".into(),
        "<Esc>".blue().bold(),
        " Quit ".into(),
        "<Q> ".blue().bold(),
    ]);

    let block = Block::bordered()
        .title(Line::from(" Containers ".bold()).centered())
        .title_bottom(instructions.centered())
        .border_set(border::THICK);

    Paragraph::new(screen.content.as_str())
        .block(block)
        .render(area, buf);
}
