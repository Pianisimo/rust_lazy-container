use std::process::Command;

use super::{App, Screen, StatusScreen};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Stylize,
    symbols::border,
    text::Line,
    widgets::{Block, Paragraph, Widget},
};

#[derive(Debug, Default)]
pub(crate) struct ContainerScreen {
    pub(crate) content: String,
}

impl ContainerScreen {
    pub(crate) fn fetch() -> Self {
        let output = Command::new("container").arg("list").output();

        let content = match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if !stdout.is_empty() { stdout } else { stderr }
            }
            Err(e) => format!("Failed to run container list: {e}"),
        };

        ContainerScreen { content }
    }

    pub(crate) fn handle_esc(app: &mut App) {
        if matches!(&app.screen, Screen::Containers(_)) {
            app.screen = Screen::Status(StatusScreen::fetch());
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
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

        Paragraph::new(self.content.as_str())
            .block(block)
            .render(area, buf);
    }
}
