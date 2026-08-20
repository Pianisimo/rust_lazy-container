pub(crate) mod container_list_panel;
pub(crate) mod help_panel;
pub(crate) mod images_list_panel;
pub(crate) mod table;

use container_list_panel::{ContainerEntry, ContainerFilter, fetch_containers};
use images_list_panel::{ImageEntry, fetch_images};

use crate::screens::{App, Screen, StatusScreen, run_command};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Clear, Widget},
};

const PANEL_CONTAINERS: usize = 0;
const PANEL_IMAGES: usize = 1;

/// Wide enough for the longest image reference the dialog quotes back.
const CREATE_DIALOG_WIDTH: u16 = 54;

/// Longest container name the dialog accepts.
const CREATE_NAME_LIMIT: usize = 40;

/// Centres a `width` x `height` box inside `area`, shrinking to fit.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

/// One containers-panel row. The header uses it too, so the column widths
/// cannot drift apart.
fn container_row(prefix: &str, id: &str, state: &str, addr: &str) -> String {
    format!("{prefix}{id:<15} {state:<10}  {addr}")
}

/// One images-panel row. Widths add up to 76: the panel's inner width at 80
/// columns, with the longest image name (37) and digest (12) in place.
fn image_row(prefix: &str, name: &str, tag: &str, digest: &str, platform: &str) -> String {
    format!("{prefix}{name:<38}{tag:<8}{digest:<13}{platform}")
}

/// Column titles for a list panel, dimmed so they read as a ruler rather than
/// as another row.
fn header_line(text: String) -> Line<'static> {
    Line::from(text.trim_end().to_string()).dark_gray().bold()
}

#[derive(Debug, Default)]
pub(crate) struct HomeScreen {
    active_panel: usize,
    selected_rows: [usize; 2],
    containers: Vec<ContainerEntry>,
    container_filter: ContainerFilter,
    images: Vec<ImageEntry>,
    show_help: bool,
    /// The name being typed in the create-container dialog. `Some` while the
    /// dialog is open, which makes it modal: it swallows every key.
    create_input: Option<String>,
}

impl HomeScreen {
    pub(crate) fn fetch() -> Self {
        Self {
            active_panel: 0,
            selected_rows: [0, 0],
            containers: fetch_containers(),
            container_filter: ContainerFilter::default(),
            images: fetch_images(),
            show_help: false,
            create_input: None,
        }
    }

    /// The image container-creation acts on: the one under the cursor, and only
    /// while the images panel has focus.
    fn focused_image(&self) -> Option<&ImageEntry> {
        if self.active_panel == PANEL_IMAGES {
            self.images.get(self.selected_rows[PANEL_IMAGES])
        } else {
            None
        }
    }

    /// `c` — open the create-container dialog for the selected image.
    pub(crate) fn open_create_dialog(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen
            && home.focused_image().is_some()
        {
            home.create_input = Some(String::new());
        }
    }

    /// While the dialog is open it owns every key, so `q` does not quit and the
    /// arrows do not move the lists underneath. Returns whether it consumed one.
    pub(crate) fn handle_dialog_key(app: &mut App, key: KeyEvent) -> bool {
        let home = match app.screen {
            Screen::Home(ref mut home) if home.create_input.is_some() => home,
            _ => return false,
        };

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => home.confirm_create(),
            // Cancel is Ctrl+C so a bare `c` stays available for the name.
            KeyCode::Char('c') if ctrl => home.create_input = None,
            KeyCode::Esc => home.create_input = None,
            KeyCode::Backspace => {
                if let Some(name) = home.create_input.as_mut() {
                    name.pop();
                }
            }
            KeyCode::Char(ch) if !ctrl => {
                if let Some(name) = home.create_input.as_mut()
                    // Anything the CLI would reject as a name is dropped.
                    && (ch.is_alphanumeric() || "-_.".contains(ch))
                    && name.chars().count() < CREATE_NAME_LIMIT
                {
                    name.push(ch);
                }
            }
            _ => {}
        }

        true
    }

    /// Creates the container and closes the dialog. An empty name is not a
    /// valid one, so Enter simply keeps waiting.
    fn confirm_create(&mut self) {
        let name = self
            .create_input
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        if name.is_empty() {
            return;
        }

        if let Some(image) = self.focused_image().map(|i| i.reference()) {
            run_command(&["container", "create", "--name", &name, &image]);
            self.containers = fetch_containers();
            self.reselect_container(&name);
        }
        self.create_input = None;
    }

    /// `/` — show or hide the help panel.
    pub(crate) fn toggle_help(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            home.show_help = !home.show_help;
        }
    }

    /// The containers the current filter lets through. `fetch_containers` always
    /// pulls `ls -a`, so cycling the filter never needs another subprocess.
    fn visible_containers(&self) -> Vec<&ContainerEntry> {
        self.containers
            .iter()
            .filter(|c| self.container_filter.matches(c))
            .collect()
    }

    /// The container the cursor sits on, regardless of which panel has focus.
    fn cursor_container(&self) -> Option<&ContainerEntry> {
        self.visible_containers()
            .get(self.selected_rows[PANEL_CONTAINERS])
            .copied()
    }

    /// The container that container-specific keys act on: the one under the
    /// cursor, and only while the containers panel actually has focus.
    fn focused_container(&self) -> Option<&ContainerEntry> {
        if self.active_panel == PANEL_CONTAINERS {
            self.cursor_container()
        } else {
            None
        }
    }

    /// Puts the cursor back on `id`, falling back to the first row when the
    /// current filter no longer shows it.
    fn reselect_container(&mut self, id: &str) {
        let row = {
            let visible = self.visible_containers();
            visible
                .iter()
                .position(|c| c.id == id)
                .unwrap_or(0)
                .min(visible.len().saturating_sub(1))
        };
        self.selected_rows[PANEL_CONTAINERS] = row;
    }

    /// `.` — cycle the container panel between all and running-only.
    pub(crate) fn cycle_container_filter(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            // Follow the highlighted container across the switch instead of
            // leaving the cursor on whatever row happens to inherit the index.
            let selected_id = home.cursor_container().map(|c| c.id.clone());

            home.container_filter = home.container_filter.next();

            match selected_id {
                Some(id) => home.reselect_container(&id),
                None => home.selected_rows[PANEL_CONTAINERS] = 0,
            }
        }
    }

    /// Label for the contextual `s` shortcut, or `None` when no container is
    /// selected and the key does nothing.
    fn container_action_label(&self) -> Option<&'static str> {
        self.focused_container()
            .map(|c| if c.is_running() { "stop" } else { "start" })
    }

    /// `s` — start the selected container, or stop it if it is already running.
    pub(crate) fn handle_start_stop_container(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            let (id, running) = match home.focused_container() {
                Some(c) => (c.id.clone(), c.is_running()),
                None => return,
            };

            if running {
                run_command(&["container", "stop", &id]);
            } else {
                run_command(&["container", "start", &id]);
            }
            home.containers = fetch_containers();
            home.reselect_container(&id);
        }
    }

    pub(crate) fn handle_esc(app: &mut App) {
        match app.screen {
            // Esc dismisses the help panel before it leaves the screen.
            Screen::Home(ref mut home) if home.show_help => home.show_help = false,
            Screen::Home(_) => {
                let mut status = StatusScreen::fetch();
                // The user asked for the status screen; auto-navigation would
                // immediately drag them back here.
                status.auto_navigate = false;
                app.screen = Screen::Status(status);
            }
            Screen::Status(_) => {}
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
                home.visible_containers().len()
            } else {
                home.images.len()
            };
            let row = &mut home.selected_rows[home.active_panel];
            if max > 0 && *row < max - 1 {
                *row += 1;
            }
        }
    }

    /// The footer hints. Built in one place so a test can check that the widest
    /// combination still fits an 80-column terminal.
    fn footer_hints(&self) -> Vec<Span<'static>> {
        let help_hint = if self.show_help {
            ": close "
        } else {
            ": help "
        };

        // Single spaces between hints: the bold key names carry the separation,
        // and the line is at the width an 80-column terminal allows.
        let mut hints = vec![
            " Shift+↑↓".blue().bold(),
            ": panel ".dark_gray(),
            "↑↓".blue().bold(),
            ": move ".dark_gray(),
            ".".blue().bold(),
            ": filter ".dark_gray(),
            "/".blue().bold(),
            help_hint.dark_gray(),
        ];

        // Contextual: start a stopped container, stop a running one. Only one of
        // these two can apply, since they follow which panel has focus.
        if let Some(action) = self.container_action_label() {
            hints.push("s".blue().bold());
            hints.push(format!(": {action} ").dark_gray());
        }
        if self.focused_image().is_some() {
            hints.push("c".blue().bold());
            hints.push(": create ".dark_gray());
        }

        hints.extend([
            "Esc".blue().bold(),
            ": back ".dark_gray(),
            "q".blue().bold(),
            ": quit ".dark_gray(),
        ]);

        hints
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let instructions = Line::from(self.footer_hints());

        let outer_block = Block::bordered()
            .title(Line::from(" lazy-container ").bold().centered())
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        let inner = outer_block.inner(area);
        outer_block.render(area, buf);

        // The help panel claims the full height of the right-hand side and
        // squeezes the lists rather than overlaying them.
        let body = if self.show_help {
            let [body, help_area] =
                Layout::horizontal([Constraint::Min(0), Constraint::Max(help_panel::WIDTH)])
                    .areas(inner);
            help_panel::render(help_area, buf);
            body
        } else {
            inner
        };

        let [top_area, bottom_area] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);

        self.render_containers(top_area, buf);
        self.render_images(bottom_area, buf);

        // Drawn last so it sits over everything else.
        if let Some(name) = &self.create_input {
            self.render_create_dialog(name, area, buf);
        }
    }

    fn render_create_dialog(&self, name: &str, area: Rect, buf: &mut Buffer) {
        let image = self
            .focused_image()
            .map(|i| i.reference())
            .unwrap_or_default();

        let popup = centered(area, CREATE_DIALOG_WIDTH, 4);
        Clear.render(popup, buf);

        let block = Block::bordered()
            .title(Line::from(" New container ").bold())
            .title_bottom(
                Line::from(vec![
                    " Enter".blue().bold(),
                    ": create  ".dark_gray(),
                    "Ctrl+C".blue().bold(),
                    ": cancel ".dark_gray(),
                ])
                .centered(),
            )
            .border_style(Style::new().cyan());

        let inner = block.inner(popup);
        block.render(popup, buf);

        Text::from(vec![
            Line::from(format!(" from {image}")).dark_gray(),
            // A block for the caret, since the terminal's own cursor is hidden.
            Line::from(format!(" {name}█")).bold(),
        ])
        .render(inner, buf);
    }

    fn render_containers(&self, area: Rect, buf: &mut Buffer) {
        let is_active = self.active_panel == PANEL_CONTAINERS;
        let selected = self.selected_rows[PANEL_CONTAINERS];

        let block = Block::bordered()
            .title(Line::from(format!(" Containers ({}) ", self.container_filter.label())).bold())
            .border_style(if is_active {
                Style::new().cyan()
            } else {
                Style::new().dark_gray()
            });

        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines: Vec<Line> = vec![header_line(container_row("   ", "NAME", "STATE", "IP"))];

        lines.extend(self.visible_containers().iter().enumerate().map(|(i, c)| {
            let prefix = if i == selected && is_active {
                " > "
            } else {
                "   "
            };
            let text = container_row(prefix, &c.id, &c.state, &c.addr);
            if i == selected && is_active {
                Line::from(text).yellow().bold()
            } else if c.is_running() {
                Line::from(text)
            } else {
                Line::from(text).dark_gray()
            }
        }));

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

        let mut lines: Vec<Line> = vec![header_line(image_row(
            "   ", "NAME", "TAG", "DIGEST", "PLATFORM",
        ))];

        lines.extend(self.images.iter().enumerate().map(|(i, img)| {
            let prefix = if i == selected && is_active {
                " > "
            } else {
                "   "
            };
            let text = image_row(prefix, &img.name, &img.tag, &img.digest, &img.platform());
            if i == selected && is_active {
                Line::from(text).yellow().bold()
            } else {
                Line::from(text)
            }
        }));

        ratatui::text::Text::from(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, state: &str) -> ContainerEntry {
        ContainerEntry {
            id: id.to_string(),
            state: state.to_string(),
            addr: String::new(),
        }
    }

    fn home() -> HomeScreen {
        HomeScreen {
            active_panel: PANEL_CONTAINERS,
            selected_rows: [0, 0],
            containers: vec![
                entry("alpha", "stopped"),
                entry("bravo", "running"),
                entry("charlie", "stopped"),
            ],
            container_filter: ContainerFilter::All,
            images: vec![ImageEntry {
                name: "docker.io/library/postgres".to_string(),
                tag: "latest".to_string(),
                digest: "78481659c47e".to_string(),
                platforms: vec!["linux/arm64".to_string()],
            }],
            show_help: false,
            create_input: None,
        }
    }

    /// The column a title sits in, so a header can be checked against a row.
    fn offsets(line: &str) -> Vec<usize> {
        line.match_indices(|c: char| !c.is_whitespace())
            .filter(|(i, _)| *i == 0 || line.as_bytes()[i - 1] == b' ')
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn container_headers_line_up_with_their_rows() {
        let header = container_row("   ", "NAME", "STATE", "IP");
        let row = container_row("   ", "bravo", "running", "192.168.64.3");
        assert_eq!(offsets(&header), offsets(&row));
    }

    #[test]
    fn image_headers_line_up_with_their_rows() {
        let header = image_row("   ", "NAME", "TAG", "DIGEST", "PLATFORM");
        let row = image_row("   ", "alpine", "edge", "020dfcbaaf4c", "linux/arm64 +6");
        // The platform cell is the only one with an internal space.
        assert_eq!(offsets(&header), offsets(&row)[..offsets(&header).len()]);
    }

    #[test]
    fn a_container_with_no_address_leaves_no_trailing_padding() {
        assert_eq!(
            container_row("   ", "alpha", "stopped", "").trim_end(),
            "   alpha           stopped"
        );
    }

    #[test]
    fn action_hint_tracks_the_selected_container_state() {
        let mut screen = home();
        assert_eq!(screen.container_action_label(), Some("start")); // alpha, stopped

        screen.selected_rows[PANEL_CONTAINERS] = 1;
        assert_eq!(screen.container_action_label(), Some("stop")); // bravo, running
    }

    #[test]
    fn no_container_is_focused_while_the_images_panel_has_focus() {
        let mut screen = home();
        screen.active_panel = PANEL_IMAGES;
        assert!(screen.focused_container().is_none());
        assert_eq!(screen.container_action_label(), None);
        // The cursor still remembers its container row for the `.` filter.
        assert_eq!(
            screen.cursor_container().map(|c| c.id.as_str()),
            Some("alpha")
        );
    }

    #[test]
    fn nothing_is_focused_when_the_filter_hides_every_container() {
        let mut screen = home();
        screen.containers = vec![entry("alpha", "stopped")];
        screen.container_filter = ContainerFilter::RunningOnly;
        assert!(screen.focused_container().is_none());
        assert_eq!(screen.container_action_label(), None);
    }

    #[test]
    fn reselect_falls_back_when_the_filter_hides_the_container() {
        let mut screen = home();
        screen.selected_rows[PANEL_CONTAINERS] = 2; // charlie, stopped
        screen.container_filter = ContainerFilter::RunningOnly;

        screen.reselect_container("charlie");
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 0);
        assert_eq!(
            screen.focused_container().map(|c| c.id.as_str()),
            Some("bravo")
        );
    }

    #[test]
    fn footer_fits_an_eighty_column_terminal() {
        // 80 columns, less the outer border.
        const AVAILABLE: usize = 78;
        let mut screen = home();

        for panel in [PANEL_CONTAINERS, PANEL_IMAGES] {
            for show_help in [false, true] {
                for row in 0..3 {
                    screen.active_panel = panel;
                    screen.show_help = show_help;
                    screen.selected_rows[PANEL_CONTAINERS] = row;

                    let width = Line::from(screen.footer_hints()).width();
                    assert!(
                        width <= AVAILABLE,
                        "footer is {width} wide (panel {panel}, help {show_help}), \
                         an 80-column terminal fits {AVAILABLE}"
                    );
                }
            }
        }
    }

    #[test]
    fn create_hint_shows_only_while_an_image_is_selected() {
        let mut screen = home();
        assert!(screen.focused_image().is_none()); // containers panel has focus

        screen.active_panel = PANEL_IMAGES;
        assert_eq!(
            screen.focused_image().map(|i| i.reference()),
            Some("docker.io/library/postgres:latest".to_string())
        );
    }

    #[test]
    fn the_dialog_only_opens_over_a_selected_image() {
        let mut app = App {
            screen: Screen::Home(home()),
            exit: false,
        };

        // Containers panel has focus, so there is nothing to create from.
        HomeScreen::open_create_dialog(&mut app);
        assert!(matches!(app.screen, Screen::Home(ref h) if h.create_input.is_none()));

        if let Screen::Home(ref mut home) = app.screen {
            home.active_panel = PANEL_IMAGES;
        }
        HomeScreen::open_create_dialog(&mut app);
        assert!(matches!(app.screen, Screen::Home(ref h) if h.create_input.is_some()));
    }

    #[test]
    fn the_dialog_swallows_keys_that_would_otherwise_act() {
        let mut app = App {
            screen: Screen::Home(home()),
            exit: false,
        };
        if let Screen::Home(ref mut home) = app.screen {
            home.active_panel = PANEL_IMAGES;
        }
        HomeScreen::open_create_dialog(&mut app);

        for ch in "db-1".chars() {
            assert!(HomeScreen::handle_dialog_key(
                &mut app,
                KeyEvent::from(KeyCode::Char(ch))
            ));
        }
        // `q` is a name character here, not quit.
        assert!(HomeScreen::handle_dialog_key(
            &mut app,
            KeyEvent::from(KeyCode::Char('q'))
        ));
        assert!(
            matches!(app.screen, Screen::Home(ref h) if h.create_input.as_deref() == Some("db-1q"))
        );
        assert!(!app.exit);

        // Spaces and slashes are not valid in a name.
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char(' ')));
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        assert!(
            matches!(app.screen, Screen::Home(ref h) if h.create_input.as_deref() == Some("db-1q"))
        );

        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Backspace));
        assert!(
            matches!(app.screen, Screen::Home(ref h) if h.create_input.as_deref() == Some("db-1"))
        );
    }

    #[test]
    fn a_bare_c_is_a_name_character() {
        let mut app = App {
            screen: Screen::Home(home()),
            exit: false,
        };
        if let Screen::Home(ref mut home) = app.screen {
            home.active_panel = PANEL_IMAGES;
        }
        HomeScreen::open_create_dialog(&mut app);

        for ch in "cache".chars() {
            HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char(ch)));
        }
        assert!(
            matches!(app.screen, Screen::Home(ref h) if h.create_input.as_deref() == Some("cache"))
        );
    }

    #[test]
    fn ctrl_c_cancels_the_dialog_and_discards_the_name() {
        let mut app = App {
            screen: Screen::Home(home()),
            exit: false,
        };
        if let Screen::Home(ref mut home) = app.screen {
            home.active_panel = PANEL_IMAGES;
        }
        HomeScreen::open_create_dialog(&mut app);
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char('d')));

        assert!(HomeScreen::handle_dialog_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
        assert!(matches!(app.screen, Screen::Home(ref h) if h.create_input.is_none()));

        // With the dialog closed the key is no longer swallowed.
        assert!(!HomeScreen::handle_dialog_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
    }

    #[test]
    fn enter_on_an_empty_name_keeps_the_dialog_open() {
        let mut app = App {
            screen: Screen::Home(home()),
            exit: false,
        };
        if let Screen::Home(ref mut home) = app.screen {
            home.active_panel = PANEL_IMAGES;
        }
        HomeScreen::open_create_dialog(&mut app);

        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Home(ref h) if h.create_input.as_deref() == Some("")));
    }

    #[test]
    fn esc_closes_the_help_panel_before_leaving_the_screen() {
        let mut app = App {
            screen: Screen::Home(home()),
            exit: false,
        };

        HomeScreen::toggle_help(&mut app);
        assert!(matches!(app.screen, Screen::Home(ref h) if h.show_help));

        // First Esc only dismisses the panel; the screen stays put.
        HomeScreen::handle_esc(&mut app);
        assert!(matches!(app.screen, Screen::Home(ref h) if !h.show_help));
    }

    #[test]
    fn reselect_follows_the_container_across_a_filter_change() {
        let mut screen = home();
        screen.container_filter = ContainerFilter::RunningOnly;
        screen.reselect_container("bravo");
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 0);

        screen.container_filter = ContainerFilter::All;
        screen.reselect_container("bravo");
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 1);
    }
}
