use std::process::Command;

use super::{App, HomeScreen, Screen, run_command};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget},
};

#[derive(Debug, Default, PartialEq, Clone, Copy)]
pub(crate) enum ContainerStatus {
    Running,
    #[default]
    NotRunning,
}

#[derive(Debug, Default)]
pub(crate) struct StatusScreen {
    pub(crate) status: ContainerStatus,
    pub(crate) message: String,
    pub(crate) version: String,
    /// Jump straight to the home screen as soon as the system reports `Running`.
    /// Cleared when the user walks back here with `<Esc>`, so they don't get
    /// bounced out of the screen they just asked for.
    pub(crate) auto_navigate: bool,
}

impl StatusScreen {
    pub(crate) fn fetch() -> Self {
        let version = Command::new("container")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        let mut screen = StatusScreen {
            status: ContainerStatus::NotRunning,
            message: String::new(),
            version,
            auto_navigate: true,
        };
        screen.refresh();
        screen
    }

    /// Re-runs `container system status`. The version is fixed for the life of
    /// the process, so it is not fetched again.
    pub(crate) fn refresh(&mut self) {
        match Command::new("container")
            .args(["system", "status"])
            .output()
        {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                self.message = if !stdout.is_empty() { stdout } else { stderr };
                self.status = if out.status.success() {
                    ContainerStatus::Running
                } else {
                    ContainerStatus::NotRunning
                };
            }
            Err(e) => {
                self.status = ContainerStatus::NotRunning;
                self.message = format!("Failed to run container: {e}");
            }
        }
    }

    pub(crate) fn handle_enter(app: &mut App) {
        let should_change =
            matches!(&app.screen, Screen::Status(s) if s.status == ContainerStatus::Running);
        if should_change {
            app.screen = Screen::Home(HomeScreen::fetch());
        }
    }

    pub(crate) fn handle_start_stop(app: &mut App) {
        if let Screen::Status(ref mut status) = app.screen {
            if status.status == ContainerStatus::Running {
                run_command(&["container", "system", "stop"]);
                // A failed stop must not fling the user into the home screen.
                status.auto_navigate = false;
            } else {
                run_command(&["container", "system", "start"]);
                status.auto_navigate = true;
            }
            status.refresh();
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let status_span = match self.status {
            ContainerStatus::Running => "Running".green().bold(),
            ContainerStatus::NotRunning => "Not Running".red().bold(),
        };

        let instructions = match self.status {
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

        let mut lines = vec![
            Line::from(self.version.as_str()).dim(),
            Line::from(vec!["Status: ".into(), status_span]),
            Line::from(self.message.as_str()),
        ];

        if self.auto_navigate && self.status == ContainerStatus::NotRunning {
            lines.push(Line::from("Waiting for the system to start…").dim());
        }

        let content = Text::from(lines);

        Paragraph::new(content)
            .centered()
            .block(block)
            .render(area, buf);
    }
}
