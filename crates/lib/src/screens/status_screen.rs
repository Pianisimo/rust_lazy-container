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
}

impl StatusScreen {
    pub(crate) fn fetch() -> Self {
        let output = Command::new("container")
            .args(["system", "status"])
            .output();

        let version = Command::new("container")
            .arg("--version")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let message = if !stdout.is_empty() { stdout } else { stderr };
                let status = if out.status.success() {
                    ContainerStatus::Running
                } else {
                    ContainerStatus::NotRunning
                };
                StatusScreen { status, message, version }
            }
            Err(e) => StatusScreen {
                status: ContainerStatus::NotRunning,
                message: format!("Failed to run container: {e}"),
                version,
            },
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
        let running = match &app.screen {
            Screen::Status(s) => Some(s.status == ContainerStatus::Running),
            _ => None,
        };
        if let Some(is_running) = running {
            if is_running {
                run_command(&["container", "system", "stop"]);
            } else {
                run_command(&["container", "system", "start"]);
            }
            app.screen = Screen::Status(StatusScreen::fetch());
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

        let content = Text::from(vec![
            Line::from(self.version.as_str()).dim(),
            Line::from(vec!["Status: ".into(), status_span]),
            Line::from(self.message.as_str()),
        ]);

        Paragraph::new(content)
            .centered()
            .block(block)
            .render(area, buf);
    }
}
