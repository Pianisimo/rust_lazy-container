pub(crate) mod container_list_panel;
pub(crate) mod images_list_panel;

use container_list_panel::{ContainerEntry, fetch_containers};
use images_list_panel::{ImageEntry, fetch_images};

use crate::screens::{App, Screen, StatusScreen};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    symbols::border,
    text::Line,
    widgets::{Block, Widget},
};

const PANEL_CONTAINERS: usize = 0;
const PANEL_IMAGES: usize = 1;

#[derive(Debug, Default)]
pub(crate) struct HomeScreen {
    active_panel: usize,
    selected_rows: [usize; 2],
    containers: Vec<ContainerEntry>,
    images: Vec<ImageEntry>,
}

impl HomeScreen {
    pub(crate) fn fetch() -> Self {
        Self {
            active_panel: 0,
            selected_rows: [0, 0],
            containers: fetch_containers(),
            images: fetch_images(),
        }
    }

    pub(crate) fn handle_esc(app: &mut App) {
        if matches!(&app.screen, Screen::Home(_)) {
            app.screen = Screen::Status(StatusScreen::fetch());
        }
    }

    pub(crate) fn handle_shift_up(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            if home.active_panel > 0 {
                home.active_panel -= 1;
            }
        }
    }

    pub(crate) fn handle_shift_down(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            if home.active_panel < 1 {
                home.active_panel += 1;
            }
        }
    }

    pub(crate) fn move_up(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            let row = &mut home.selected_rows[home.active_panel];
            if *row > 0 {
                *row -= 1;
            }
        }
    }

    pub(crate) fn move_down(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            let max = if home.active_panel == PANEL_CONTAINERS {
                home.containers.len()
            } else {
                home.images.len()
            };
            let row = &mut home.selected_rows[home.active_panel];
            if max > 0 && *row < max - 1 {
                *row += 1;
            }
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let instructions = Line::from(vec![
            " Shift+↑↓".blue().bold(),
            ": switch panel  ".dark_gray(),
            "↑↓".blue().bold(),
            ": navigate  ".dark_gray(),
            "Esc".blue().bold(),
            ": back  ".dark_gray(),
            "q".blue().bold(),
            ": quit ".dark_gray(),
        ]);

        let outer_block = Block::bordered()
            .title(Line::from(" lazy-container ").bold().centered())
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        let inner = outer_block.inner(area);
        outer_block.render(area, buf);

        let [top_area, bottom_area] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(inner);

        self.render_containers(top_area, buf);
        self.render_images(bottom_area, buf);
    }

    fn render_containers(&self, area: Rect, buf: &mut Buffer) {
        let is_active = self.active_panel == PANEL_CONTAINERS;
        let selected = self.selected_rows[PANEL_CONTAINERS];

        let block = Block::bordered()
            .title(Line::from(" Containers ").bold())
            .border_style(if is_active {
                Style::new().cyan()
            } else {
                Style::new().dark_gray()
            });

        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line> = self
            .containers
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let addr_part = if !c.addr.is_empty() {
                    format!("  {}", c.addr)
                } else {
                    String::new()
                };
                let prefix = if i == selected && is_active { " > " } else { "   " };
                let text = format!("{prefix}{:<15} {:<10}{addr_part}", c.id, c.state);
                if i == selected && is_active {
                    Line::from(text).yellow().bold()
                } else if c.is_running() {
                    Line::from(text)
                } else {
                    Line::from(text).dark_gray()
                }
            })
            .collect();

        ratatui::text::Text::from(lines).render(inner, buf);
    }

    fn render_images(&self, area: Rect, buf: &mut Buffer) {
        let is_active = self.active_panel == PANEL_IMAGES;
        let selected = self.selected_rows[PANEL_IMAGES];

        let block = Block::bordered()
            .title(Line::from(" Images ").bold())
            .border_style(if is_active {
                Style::new().cyan()
            } else {
                Style::new().dark_gray()
            });

        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line> = self
            .images
            .iter()
            .enumerate()
            .map(|(i, img)| {
                let prefix = if i == selected && is_active { " > " } else { "   " };
                let text = format!("{prefix}{}", img.display);
                if i == selected && is_active {
                    Line::from(text).yellow().bold()
                } else {
                    Line::from(text)
                }
            })
            .collect();

        ratatui::text::Text::from(lines).render(inner, buf);
    }
}
