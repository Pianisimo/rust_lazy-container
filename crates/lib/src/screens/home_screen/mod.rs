pub(crate) mod container_list_panel;
mod delete_dialog;
pub(crate) mod help_panel;
pub(crate) mod images_list_panel;
pub(crate) mod volumes;

use std::time::Instant;

use container_list_panel::ContainerFilter;
pub(crate) use container_list_panel::{ContainerEntry, fetch_containers};
use delete_dialog::DeleteDialog;
pub(crate) use images_list_panel::{
    DEFAULT_REGISTRY_DOMAIN, ImageEntry, fetch_images, fetch_registry_domain,
};
pub(crate) use volumes::{VolumeEntry, delete_left_behind, fetch_volumes};

use crate::screens::actions::{Action, PendingActions};
use crate::screens::{App, Screen, StatusScreen};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    symbols::{border, line},
    text::{Line, Span, Text},
    widgets::{
        Block, Cell, Clear, HighlightSpacing, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
        StatefulWidget, Table, TableState, Widget,
    },
};

const PANEL_CONTAINERS: usize = 0;
const PANEL_IMAGES: usize = 1;

/// Wide enough for the longest image reference the dialog quotes back.
const CREATE_DIALOG_WIDTH: u16 = 54;

/// Longest container name the dialog accepts.
const CREATE_NAME_LIMIT: usize = 40;

/// Marks the cursor row. Every row is indented by its width, selected or
/// not, so the columns don't shift.
const HIGHLIGHT_SYMBOL: &str = " > ";

/// Blank columns between two cells.
const COLUMN_SPACING: u16 = 1;

/// Rows the column titles take at the top of a list.
const HEADER_HEIGHT: u16 = 1;

// Column widths. The ID and name columns grow to fit their longest entry;
// the last column of each list takes whatever is left.
const CONTAINER_ID_WIDTH: u16 = 15;
/// "running" and "stopping…", plus one more column before the IP.
const CONTAINER_STATE_WIDTH: u16 = 11;
/// The longest name among the usual images: `ghcr.io/apple/containerization/vminit`.
const IMAGE_NAME_WIDTH: u16 = 37;
const IMAGE_TAG_WIDTH: u16 = 7;
const IMAGE_DIGEST_WIDTH: u16 = 12;

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

/// Wide enough for `min` columns and for the widest of `cells`.
fn column_width<'a>(min: u16, cells: impl Iterator<Item = &'a str>) -> u16 {
    let widest = cells.map(|cell| Span::raw(cell).width()).max().unwrap_or(0);
    widest.clamp(min as usize, u16::MAX as usize) as u16
}

/// Constraints for columns that want `widths`, in a table `available` cells
/// wide. Each column gets what it wants, left to right, and the last one
/// takes whatever is left. Past the edge, the column there is cut short and
/// the rest are dropped, the way the panel border clips plain text. Handing
/// ratatui only widths that fit also keeps its layout solver from shrinking
/// every column at once when the help panel narrows the lists.
fn fit_columns(widths: &[u16], available: u16) -> Vec<Constraint> {
    let mut constraints = Vec::with_capacity(widths.len());
    let mut used: u16 = 0;
    for (index, &wanted) in widths.iter().enumerate() {
        let gap = if index == 0 { 0 } else { COLUMN_SPACING };
        let room = available.saturating_sub(used.saturating_add(gap));
        if room == 0 {
            break;
        }
        let width = if index + 1 == widths.len() {
            room
        } else {
            wanted.min(room)
        };
        constraints.push(Constraint::Length(width));
        used = used.saturating_add(gap + width);
    }
    constraints
}

/// The first row to show so that row `selected` of `len` is among the
/// `height` on screen, moving the view as little as possible from `offset`.
fn scroll_offset(offset: usize, selected: usize, len: usize, height: usize) -> usize {
    if height == 0 {
        return offset;
    }
    let last_offset = len.saturating_sub(height);
    let offset = offset.min(last_offset);
    let offset = if selected < offset {
        selected
    } else if selected >= offset + height {
        selected + 1 - height
    } else {
        offset
    };
    offset.min(last_offset)
}

fn border_style(is_active: bool) -> Style {
    if is_active {
        Style::new().cyan()
    } else {
        Style::new().dark_gray()
    }
}

/// The rows of a list panel drawn at `area`: inside its border, under the
/// column titles.
fn list_rows_area(area: Rect) -> Rect {
    let inner = Block::bordered().inner(area);
    let header = HEADER_HEIGHT.min(inner.height);
    Rect {
        y: inner.y + header,
        height: inner.height - header,
        ..inner
    }
}

/// One list panel, ready to draw with `HomeScreen::render_list`.
struct ListPanel<'a> {
    /// `PANEL_CONTAINERS` or `PANEL_IMAGES`.
    index: usize,
    block: Block<'a>,
    header: Row<'a>,
    rows: Vec<Row<'a>>,
    /// The width each column asks for; see `fit_columns`.
    widths: Vec<u16>,
}

/// A key the footer names, and what it does.
struct Hint {
    key: &'static str,
    action: &'static str,
    /// When the footer doesn't fit, the lowest go first.
    priority: u8,
}

impl Hint {
    fn new(key: &'static str, action: &'static str, priority: u8) -> Self {
        Self {
            key,
            action,
            priority,
        }
    }

    /// Columns of `key: action `.
    fn width(&self) -> usize {
        Span::raw(self.key).width() + ": ".len() + Span::raw(self.action).width() + 1
    }
}

/// Where each part of the home screen goes.
struct Areas {
    containers: Rect,
    images: Rect,
    help: Option<Rect>,
}

/// The open create-container dialog.
#[derive(Debug)]
struct CreateDialog {
    /// The name typed so far.
    name: String,
    /// The image to create from. Copied when the dialog opens, so a refresh
    /// that moves the images cursor can't change it while the user types.
    image: ImageEntry,
}

#[derive(Debug, Default)]
pub(crate) struct HomeScreen {
    active_panel: usize,
    selected_rows: [usize; 2],
    /// The first row each list shows. `render` only gets `&self`, so the
    /// offsets live here, and `keep_selection_visible` moves them before each
    /// draw, once the frame size is known.
    scroll_offsets: [usize; 2],
    containers: Vec<ContainerEntry>,
    container_filter: ContainerFilter,
    images: Vec<ImageEntry>,
    /// Every volume. No panel shows them; the delete dialog uses them.
    volumes: Vec<VolumeEntry>,
    show_help: bool,
    /// `Some` while the create-container dialog is open, which makes it
    /// modal: it swallows every key.
    create_dialog: Option<CreateDialog>,
    /// `Some` while the delete dialog is open. It is modal too. The two
    /// never open together: each needs a different panel to have focus.
    delete_dialog: Option<DeleteDialog>,
    /// A container created from the dialog. The cursor moves to it once a
    /// refresh lists it.
    awaited_container: Option<String>,
}

impl HomeScreen {
    /// An empty screen; the worker fills in the lists.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Takes fresh copies of the lists and keeps each cursor on the item it
    /// was on. A list that couldn't be read keeps its old rows, and the
    /// reason is handed back for the app to show.
    pub(crate) fn update(
        &mut self,
        containers: Result<Vec<ContainerEntry>, String>,
        images: Result<Vec<ImageEntry>, String>,
        volumes: Result<Vec<VolumeEntry>, String>,
    ) -> Option<String> {
        let mut errors = Vec::new();
        match containers {
            Ok(containers) => self.set_containers(containers),
            Err(e) => errors.push(e),
        }
        match images {
            Ok(images) => self.set_images(images),
            Err(e) => errors.push(e),
        }
        match volumes {
            Ok(volumes) => self.volumes = volumes,
            Err(e) => errors.push(e),
        }
        errors.dedup();
        (!errors.is_empty()).then(|| errors.join("\n"))
    }

    fn set_containers(&mut self, containers: Vec<ContainerEntry>) {
        let row = self.selected_rows[PANEL_CONTAINERS];
        let current = self.cursor_container().map(|c| c.id.clone());
        self.containers = containers;

        let created = self
            .awaited_container
            .take_if(|name| self.containers.iter().any(|c| c.id == *name));
        match created {
            Some(name) => self.reselect_container(&name),
            // If the container is gone, the cursor stays on the same row
            // instead of jumping to the top.
            None => self.select_container_or(current.as_deref(), row),
        }

        // Deleted elsewhere while the dialog asked about it.
        let gone = |id: &str| !self.containers.iter().any(|c| c.id == id);
        if self.delete_dialog.as_ref().is_some_and(|d| gone(&d.id)) {
            self.delete_dialog = None;
        }
    }

    fn set_images(&mut self, images: Vec<ImageEntry>) {
        let row = self.selected_rows[PANEL_IMAGES];
        let current = self.images.get(row).map(|i| i.reference.clone());
        self.images = images;

        let row = current
            .and_then(|reference| self.images.iter().position(|i| i.reference == reference))
            .unwrap_or(row);
        self.selected_rows[PANEL_IMAGES] = row.min(self.images.len().saturating_sub(1));
    }

    /// Moves the cursor to container `name` as soon as a refresh lists it.
    pub(crate) fn select_when_listed(&mut self, name: String) {
        self.awaited_container = Some(name);
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
            && let Some(image) = home.focused_image().cloned()
        {
            home.create_dialog = Some(CreateDialog {
                name: String::new(),
                image,
            });
        }
    }

    /// While a dialog is open it owns every key, so `q` does not quit and the
    /// arrows do not move the lists underneath. Returns whether it consumed one.
    pub(crate) fn handle_dialog_key(app: &mut App, key: KeyEvent) -> bool {
        let Screen::Home(home) = &mut app.screen else {
            return false;
        };
        if home.delete_dialog.is_some() {
            Self::handle_delete_dialog_key(app, key);
            return true;
        }
        let Some(dialog) = home.create_dialog.as_mut() else {
            return false;
        };

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter => {
                // An empty name is not a valid one, so Enter simply keeps waiting.
                let name = dialog.name.trim().to_string();
                if name.is_empty() {
                    return true;
                }
                let image = dialog.image.reference.clone();
                home.create_dialog = None;
                app.submit(Action::CreateContainer { name, image });
            }
            // Cancel is Ctrl+C so a bare `c` stays available for the name.
            KeyCode::Char('c') if ctrl => home.create_dialog = None,
            KeyCode::Esc => home.create_dialog = None,
            KeyCode::Backspace => {
                dialog.name.pop();
            }
            // Anything the CLI would reject as a name is dropped.
            KeyCode::Char(ch)
                if !ctrl
                    && (ch.is_alphanumeric() || "-_.".contains(ch))
                    && dialog.name.chars().count() < CREATE_NAME_LIMIT =>
            {
                dialog.name.push(ch);
            }
            _ => {}
        }

        true
    }

    /// `d` — ask before deleting the focused container. `D` asks before
    /// deleting its anonymous volumes as well.
    pub(crate) fn open_delete_dialog(app: &mut App, with_volumes: bool) {
        let Screen::Home(home) = &mut app.screen else {
            return;
        };
        let Some(container) = home.focused_container() else {
            return;
        };
        let dialog = DeleteDialog::new(container, with_volumes, &home.containers, &home.volumes);
        // As with `s`, a container something is still being done to has to
        // wait for that to finish.
        if app.pending.blocks(&dialog.action()) {
            return;
        }
        home.delete_dialog = Some(dialog);
    }

    /// Enter or `y` deletes; Esc, `n` or Ctrl+C cancels. The command runs in
    /// the background, like `s`.
    fn handle_delete_dialog_key(app: &mut App, key: KeyEvent) {
        let Screen::Home(home) = &mut app.screen else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Enter | KeyCode::Char('y' | 'Y') if !ctrl => {
                if let Some(dialog) = home.delete_dialog.take() {
                    app.submit(dialog.action());
                }
            }
            KeyCode::Char('c') if ctrl => home.delete_dialog = None,
            KeyCode::Esc | KeyCode::Char('n' | 'N') => home.delete_dialog = None,
            _ => {}
        }
    }

    /// `/` — show or hide the help panel.
    pub(crate) fn toggle_help(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen {
            home.show_help = !home.show_help;
        }
    }

    /// The containers the current filter lets through. The worker always
    /// lists every container, so cycling the filter never needs another
    /// subprocess.
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

    /// Puts the cursor on `id` if the filter shows it, and otherwise on row
    /// `fallback`, or the last row if there are fewer.
    fn select_container_or(&mut self, id: Option<&str>, fallback: usize) {
        let row = {
            let visible = self.visible_containers();
            id.and_then(|id| visible.iter().position(|c| c.id == id))
                .unwrap_or(fallback)
                .min(visible.len().saturating_sub(1))
        };
        self.selected_rows[PANEL_CONTAINERS] = row;
    }

    /// Puts the cursor back on `id`, falling back to the first row when the
    /// current filter no longer shows it.
    fn reselect_container(&mut self, id: &str) {
        self.select_container_or(Some(id), 0);
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

    /// `s` — start the selected container, or stop it if it is already
    /// running. The command runs in the background; the list shows the
    /// result once a refresh sees it.
    pub(crate) fn handle_start_stop_container(app: &mut App) {
        let Screen::Home(home) = &app.screen else {
            return;
        };
        let Some(container) = home.focused_container() else {
            return;
        };

        let id = container.id.clone();
        let action = if container.is_running() {
            Action::StopContainer(id)
        } else {
            Action::StartContainer(id)
        };
        app.submit(action);
    }

    pub(crate) fn handle_esc(app: &mut App) {
        match app.screen {
            // Esc dismisses the help panel before it leaves the screen.
            Screen::Home(ref mut home) if home.show_help => home.show_help = false,
            Screen::Home(_) => {
                // The user asked for the status screen; auto-navigation would
                // immediately drag them back here.
                let status = StatusScreen::new(app.version.clone(), false);
                app.show(Screen::Status(status));
            }
            Screen::Status(_) => {}
        }
    }

    pub(crate) fn handle_shift_up(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen
            && home.active_panel > 0
        {
            home.active_panel -= 1;
        }
    }

    pub(crate) fn handle_shift_down(app: &mut App) {
        if let Screen::Home(ref mut home) = app.screen
            && home.active_panel < 1
        {
            home.active_panel += 1;
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

    /// Scrolls each list just enough to keep its cursor on screen. Called
    /// before every draw, since only then is it known how many rows fit.
    pub(crate) fn keep_selection_visible(&mut self, area: Rect) {
        let areas = self.areas(area);
        let lengths = [self.visible_containers().len(), self.images.len()];
        for (panel, panel_area) in [
            (PANEL_CONTAINERS, areas.containers),
            (PANEL_IMAGES, areas.images),
        ] {
            let height = list_rows_area(panel_area).height as usize;
            self.scroll_offsets[panel] = scroll_offset(
                self.scroll_offsets[panel],
                self.selected_rows[panel],
                lengths[panel],
                height,
            );
        }
    }

    /// The footer hints that fit in `width` columns. Built in one place so a
    /// test can check what an 80-column terminal shows.
    ///
    /// Every hint fits from 96 columns. Below that, hints are dropped by
    /// priority: first the keys anyone would try anyway (arrows, Esc), last
    /// the ones that act on the focused item, and quit.
    fn footer_hints(&self, width: usize) -> Vec<Span<'static>> {
        let mut hints = vec![
            Hint::new("Shift+↑↓", "panel", 3),
            Hint::new("↑↓", "move", 0),
            Hint::new(".", "filter", 2),
            Hint::new("/", if self.show_help { "close" } else { "help" }, 4),
        ];

        // Contextual: they follow which panel has focus, so the container keys
        // and `c` never show together.
        if let Some(action) = self.container_action_label() {
            hints.push(Hint::new("s", action, 6));
        }
        if self.focused_container().is_some() {
            hints.push(Hint::new("d/D", "delete/purge", 6));
        }
        if self.focused_image().is_some() {
            hints.push(Hint::new("c", "create", 6));
        }

        hints.push(Hint::new("Esc", "back", 1));
        hints.push(Hint::new("q", "quit", 7));

        // A space before the first key; each hint ends with one.
        let total = |hints: &[Hint]| 1 + hints.iter().map(Hint::width).sum::<usize>();
        while total(&hints) > width {
            // The rightmost of the least important.
            let lowest = hints
                .iter()
                .enumerate()
                .rev()
                .min_by_key(|(_, hint)| hint.priority)
                .map(|(index, _)| index);
            match lowest {
                Some(index) => hints.remove(index),
                None => break,
            };
        }

        // Single spaces between hints: the bold key names carry the separation.
        let mut spans = vec![Span::raw(" ")];
        for hint in hints {
            spans.push(hint.key.blue().bold());
            spans.push(format!(": {} ", hint.action).dark_gray());
        }
        spans
    }

    /// Splits the screen. `render` and `keep_selection_visible` both use
    /// this, so the scroll offsets are worked out for the rows that are drawn.
    fn areas(&self, area: Rect) -> Areas {
        let inner = Block::bordered().inner(area);

        // The help panel claims the full height of the right-hand side and
        // squeezes the lists rather than overlaying them.
        let (body, help) = if self.show_help {
            let [body, help] =
                Layout::horizontal([Constraint::Min(0), Constraint::Max(help_panel::WIDTH)])
                    .areas(inner);
            (body, Some(help))
        } else {
            (inner, None)
        };

        let [containers, images] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);

        Areas {
            containers,
            images,
            help,
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, pending: &PendingActions) {
        // The footer sits on the bottom border, between its corners.
        let instructions = Line::from(self.footer_hints(area.width.saturating_sub(2) as usize));

        Block::bordered()
            .title(Line::from(" lazy-container ").bold().centered())
            .title_bottom(instructions.centered())
            .border_set(border::THICK)
            .render(area, buf);

        let areas = self.areas(area);
        if let Some(help_area) = areas.help {
            help_panel::render(help_area, buf);
        }
        self.render_containers(areas.containers, buf, pending);
        self.render_images(areas.images, buf);

        // Drawn last so they sit over everything else.
        if let Some(dialog) = &self.create_dialog {
            render_create_dialog(dialog, area, buf);
        }
        if let Some(dialog) = &self.delete_dialog {
            dialog.render(area, buf);
        }
    }

    fn render_containers(&self, area: Rect, buf: &mut Buffer, pending: &PendingActions) {
        let is_active = self.active_panel == PANEL_CONTAINERS;
        let title = Line::from(format!(" Containers ({}) ", self.container_filter.label())).bold();

        // What is running goes on the same border, right-aligned, in the
        // room the title leaves: two corners, a gap, and a space either side.
        let room = (area.width as usize).saturating_sub(title.width() + 5);
        let progress = pending
            .progress(Instant::now(), room)
            .map(|progress| Line::from(format!(" {progress} ")).yellow().right_aligned());

        let mut block = Block::bordered()
            .title(title)
            .border_style(border_style(is_active));
        if let Some(progress) = progress {
            block = block.title(progress);
        }

        let containers = self.visible_containers();
        let widths = vec![
            column_width(CONTAINER_ID_WIDTH, containers.iter().map(|c| c.id.as_str())),
            CONTAINER_STATE_WIDTH,
            0,
        ];
        let rows = containers
            .iter()
            .map(|c| {
                // A container being started or stopped says so.
                let state = pending.row_state(&c.id).unwrap_or(&c.state);
                let row = Row::new([c.id.as_str(), state, c.addr.as_str()]);
                if c.is_running() { row } else { row.dark_gray() }
            })
            .collect();

        let list = ListPanel {
            index: PANEL_CONTAINERS,
            block,
            // `ID`, as `container ls` calls it.
            header: Row::new(["ID", "STATE", "IP"]),
            rows,
            widths,
        };
        self.render_list(list, area, buf);
    }

    fn render_images(&self, area: Rect, buf: &mut Buffer) {
        let is_active = self.active_panel == PANEL_IMAGES;
        let block = Block::bordered()
            .title(Line::from(" Images ").bold())
            .border_style(border_style(is_active));

        let widths = vec![
            column_width(
                IMAGE_NAME_WIDTH,
                self.images.iter().map(|i| i.name.as_str()),
            ),
            column_width(IMAGE_TAG_WIDTH, self.images.iter().map(|i| i.tag.as_str())),
            IMAGE_DIGEST_WIDTH,
            0,
        ];
        let rows = self
            .images
            .iter()
            .map(|image| {
                Row::new([
                    Cell::from(image.name.as_str()),
                    Cell::from(image.tag.as_str()),
                    Cell::from(image.digest.as_str()),
                    Cell::from(image.platform()),
                ])
            })
            .collect();

        let list = ListPanel {
            index: PANEL_IMAGES,
            block,
            header: Row::new(["NAME", "TAG", "DIGEST", "PLATFORM"]),
            rows,
            widths,
        };
        self.render_list(list, area, buf);
    }

    /// Draws a list panel: column titles, then as many rows as fit from the
    /// panel's scroll offset on. The cursor row is highlighted only while the
    /// panel has focus. When the rows don't all fit, a scrollbar on the right
    /// border shows where the view is.
    fn render_list(&self, list: ListPanel, area: Rect, buf: &mut Buffer) {
        let is_active = self.active_panel == list.index;
        let offset = self.scroll_offsets[list.index];
        let row_count = list.rows.len();

        let inner = list.block.inner(area);
        list.block.render(area, buf);

        let highlight_width = Span::raw(HIGHLIGHT_SYMBOL).width() as u16;
        let widths = fit_columns(&list.widths, inner.width.saturating_sub(highlight_width));
        let table = Table::new(list.rows, widths)
            .header(list.header.dark_gray().bold())
            .column_spacing(COLUMN_SPACING)
            .highlight_symbol(HIGHLIGHT_SYMBOL)
            .highlight_spacing(HighlightSpacing::Always)
            .row_highlight_style(Style::new().yellow().bold());
        let mut state = TableState::new()
            .with_offset(offset)
            .with_selected(is_active.then_some(self.selected_rows[list.index]));
        StatefulWidget::render(table, inner, buf, &mut state);

        let rows_area = list_rows_area(area);
        let visible = rows_area.height as usize;
        if visible > 0 && row_count > visible {
            let mut position = ScrollbarState::new(row_count - visible + 1)
                .position(offset)
                .viewport_content_length(visible);
            // Over the right border, level with the rows.
            let track = Rect {
                x: area.x,
                width: area.width,
                ..rows_area
            };
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some(line::VERTICAL))
                .thumb_symbol(line::THICK_VERTICAL)
                .style(border_style(is_active))
                .render(track, buf, &mut position);
        }
    }
}

fn render_create_dialog(dialog: &CreateDialog, area: Rect, buf: &mut Buffer) {
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
        Line::from(format!(" from {}", dialog.image.label())).dark_gray(),
        // A block for the caret, since the terminal's own cursor is hidden.
        Line::from(format!(" {}█", dialog.name)).bold(),
    ])
    .render(inner, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::worker::{Refresh, RefreshKind, Reply};

    fn entry(id: &str, state: &str) -> ContainerEntry {
        ContainerEntry {
            id: id.to_string(),
            state: state.to_string(),
            ..ContainerEntry::default()
        }
    }

    fn image(name: &str, tag: &str) -> ImageEntry {
        ImageEntry {
            reference: format!("docker.io/library/{name}:{tag}"),
            name: name.to_string(),
            tag: tag.to_string(),
            digest: "78481659c47e".to_string(),
            platforms: vec!["linux/arm64".to_string()],
        }
    }

    fn volume(name: &str, anonymous: bool) -> VolumeEntry {
        VolumeEntry {
            name: name.to_string(),
            anonymous,
        }
    }

    fn home() -> HomeScreen {
        HomeScreen {
            containers: vec![
                entry("alpha", "stopped"),
                entry("bravo", "running"),
                entry("charlie", "stopped"),
            ],
            images: vec![image("postgres", "latest")],
            ..HomeScreen::default()
        }
    }

    fn many_containers(count: usize) -> Vec<ContainerEntry> {
        (0..count)
            .map(|n| entry(&format!("container-{n:02}"), "running"))
            .collect()
    }

    fn app_with(screen: HomeScreen) -> App {
        App::with_screen(Screen::Home(screen))
    }

    fn home_of(app: &App) -> &HomeScreen {
        match &app.screen {
            Screen::Home(home) => home,
            Screen::Status(_) => panic!("expected the home screen"),
        }
    }

    fn home_mut(app: &mut App) -> &mut HomeScreen {
        match &mut app.screen {
            Screen::Home(home) => home,
            Screen::Status(_) => panic!("expected the home screen"),
        }
    }

    impl HomeScreen {
        fn dialog_name(&self) -> Option<&str> {
            self.create_dialog.as_ref().map(|d| d.name.as_str())
        }
    }

    /// The screen as text, one string per row, after the same scroll update
    /// the app does before drawing.
    fn draw(
        screen: &mut HomeScreen,
        pending: &PendingActions,
        width: u16,
        height: u16,
    ) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        screen.keep_selection_visible(area);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, pending);
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn row_containing<'a>(rows: &'a [String], text: &str) -> Option<&'a String> {
        rows.iter().find(|row| row.contains(text))
    }

    #[test]
    fn container_columns_keep_their_layout() {
        let mut screen = home();
        screen.containers[1].addr = "192.168.64.3/24".to_string();
        screen.active_panel = PANEL_IMAGES;
        let rows = draw(&mut screen, &PendingActions::default(), 80, 24);

        // Rows 2 and 4 of the screen: the header and bravo, inside the borders.
        assert_eq!(
            rows[2][6..].trim_end_matches([' ', '│', '┃']),
            "   ID              STATE       IP"
        );
        assert_eq!(
            rows[4][6..].trim_end_matches([' ', '│', '┃']),
            "   bravo           running     192.168.64.3/24"
        );
    }

    #[test]
    fn image_columns_keep_their_layout() {
        let mut screen = home();
        screen.images = vec![ImageEntry {
            platforms: vec!["linux/amd64".to_string(), "linux/arm64".to_string()],
            ..image("alpine", "edge")
        }];
        let rows = draw(&mut screen, &PendingActions::default(), 80, 24);

        let header = row_containing(&rows, "PLATFORM").unwrap();
        assert_eq!(
            header[6..].trim_end_matches([' ', '│', '┃']),
            "   NAME                                  TAG     DIGEST       PLATFORM"
        );
        let row = row_containing(&rows, "alpine").unwrap();
        assert!(
            row[6..].starts_with(
                "   alpine                                edge    78481659c47e linux/"
            ),
            "{row}"
        );
    }

    #[test]
    fn a_long_id_widens_its_column_for_every_row() {
        let mut screen = home();
        screen.containers[0].id = "a-very-long-container-name".to_string();
        let rows = draw(&mut screen, &PendingActions::default(), 80, 24);

        let header = row_containing(&rows, "STATE").unwrap();
        let bravo = row_containing(&rows, "bravo").unwrap();
        let long = row_containing(&rows, "a-very-long-container-name").unwrap();
        let state_column = header.find("STATE").unwrap();
        assert_eq!(bravo.find("running"), Some(state_column));
        assert_eq!(long.find("stopped"), Some(state_column));
    }

    #[test]
    fn fit_columns_clips_at_the_edge_like_plain_text() {
        assert_eq!(
            fit_columns(&[37, 7, 12, 0], 73),
            [
                Constraint::Length(37),
                Constraint::Length(7),
                Constraint::Length(12),
                Constraint::Length(14),
            ]
        );
        // The help panel leaves 29 columns: the name is cut, the rest dropped.
        assert_eq!(fit_columns(&[37, 7, 12, 0], 29), [Constraint::Length(29)]);
        assert_eq!(
            fit_columns(&[15, 11, 0], 29),
            [
                Constraint::Length(15),
                Constraint::Length(11),
                Constraint::Length(1),
            ]
        );
        assert_eq!(fit_columns(&[15, 11, 0], 0), []);
    }

    #[test]
    fn scrolling_moves_the_view_as_little_as_possible() {
        // Moving down past the bottom scrolls by one row.
        assert_eq!(scroll_offset(0, 7, 20, 8), 0);
        assert_eq!(scroll_offset(0, 8, 20, 8), 1);
        // Moving back up inside the view leaves it alone...
        assert_eq!(scroll_offset(1, 7, 20, 8), 1);
        assert_eq!(scroll_offset(1, 1, 20, 8), 1);
        // ...until the cursor passes the top.
        assert_eq!(scroll_offset(1, 0, 20, 8), 0);
        // A list that shrank doesn't leave blank rows at the bottom.
        assert_eq!(scroll_offset(12, 3, 5, 8), 0);
        assert_eq!(scroll_offset(12, 15, 16, 8), 8);
        // No rows fit: nothing to decide.
        assert_eq!(scroll_offset(3, 9, 20, 0), 3);
    }

    #[test]
    fn the_cursor_stays_on_screen_in_a_long_list() {
        let mut app = app_with(HomeScreen {
            containers: many_containers(20),
            ..HomeScreen::default()
        });
        let no_pending = PendingActions::default();
        for _ in 0..15 {
            HomeScreen::move_down(&mut app);
        }

        // At 80×24 the containers panel shows 8 rows.
        let rows = draw(home_mut(&mut app), &no_pending, 80, 24);
        let cursor = row_containing(&rows, " > ").expect("cursor row on screen");
        assert!(cursor.contains("container-15"), "{cursor}");
        assert!(rows[10].contains(" > container-15"), "{}", rows[10]);
        assert!(row_containing(&rows, "container-07").is_none());
        // The column titles stay put above the scrolled rows.
        assert!(rows[2].contains("ID"), "{}", rows[2]);
        assert!(rows[3].contains("container-08"), "{}", rows[3]);

        // Back up to the top row of the view: nothing moves.
        for _ in 0..7 {
            HomeScreen::move_up(&mut app);
        }
        let rows = draw(home_mut(&mut app), &no_pending, 80, 24);
        assert!(rows[3].contains(" > container-08"), "{}", rows[3]);

        // One more and the view follows.
        HomeScreen::move_up(&mut app);
        let rows = draw(home_mut(&mut app), &no_pending, 80, 24);
        assert!(rows[3].contains(" > container-07"), "{}", rows[3]);
        assert!(rows[10].contains("container-14"), "{}", rows[10]);
    }

    #[test]
    fn the_images_list_scrolls_too() {
        let mut screen = HomeScreen {
            images: (0..20)
                .map(|n| image(&format!("image-{n:02}"), "1"))
                .collect(),
            active_panel: PANEL_IMAGES,
            selected_rows: [0, 19],
            ..HomeScreen::default()
        };
        let rows = draw(&mut screen, &PendingActions::default(), 80, 24);
        let cursor = row_containing(&rows, " > ").expect("cursor row on screen");
        assert!(cursor.contains("image-19"), "{cursor}");
    }

    #[test]
    fn a_scrollbar_appears_only_when_rows_are_hidden() {
        let mut screen = home();
        let rows = draw(&mut screen, &PendingActions::default(), 80, 24);
        assert!(rows[3..11].iter().all(|row| row.ends_with("│┃")));

        let mut screen = HomeScreen {
            containers: many_containers(20),
            ..HomeScreen::default()
        };
        let rows = draw(&mut screen, &PendingActions::default(), 80, 24);
        // The thumb sits at the top of the rows, in the panel's right border.
        let right_border: String = rows[3..11]
            .iter()
            .map(|row| row.chars().rev().nth(1).unwrap())
            .collect();
        assert_eq!(right_border, "┃┃┃│││││");
    }

    #[test]
    fn a_running_command_shows_on_its_row_and_the_panel_border() {
        let mut screen = home();
        let mut pending = PendingActions::default();
        pending.push(Action::StopContainer("bravo".to_string()));
        let rows = draw(&mut screen, &pending, 80, 24);

        // bravo is the second row; its STATE cell says what's happening.
        assert!(rows[4].contains("bravo           stopping…"), "{}", rows[4]);
        assert!(rows[1].contains("Containers (all)"), "{}", rows[1]);
        // Right-aligned, after the spinner, whose frame depends on timing.
        assert!(rows[1].ends_with(" Stopping bravo… ┐┃"), "{}", rows[1]);
    }

    #[test]
    fn the_progress_never_covers_the_panel_title() {
        let mut screen = home();
        screen.show_help = true;
        let mut pending = PendingActions::default();
        pending.push(Action::StopContainer(
            "a-container-with-a-long-name".to_string(),
        ));
        let rows = draw(&mut screen, &pending, 80, 24);
        assert!(rows[1].contains(" Containers (all) "), "{}", rows[1]);
        assert!(rows[1].contains("Stoppi"), "{}", rows[1]);
    }

    #[test]
    fn a_refresh_keeps_the_cursor_on_the_same_container() {
        let mut screen = home();
        screen.selected_rows[PANEL_CONTAINERS] = 1; // bravo

        // A container created elsewhere sorts in front of bravo.
        let error = screen.update(
            Ok(vec![
                entry("alpha", "stopped"),
                entry("aardvark", "running"),
                entry("bravo", "stopped"),
            ]),
            Ok(vec![image("postgres", "latest")]),
            Ok(vec![]),
        );
        assert_eq!(error, None);
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 2);
        assert_eq!(
            screen.cursor_container().map(|c| c.state.as_str()),
            Some("stopped")
        );
    }

    #[test]
    fn a_vanished_container_leaves_the_cursor_on_its_row() {
        let mut screen = home();
        screen.selected_rows[PANEL_CONTAINERS] = 1; // bravo
        screen.update(
            Ok(vec![entry("alpha", "stopped"), entry("charlie", "stopped")]),
            Ok(vec![]),
            Ok(vec![]),
        );
        assert_eq!(
            screen.cursor_container().map(|c| c.id.as_str()),
            Some("charlie")
        );

        // With fewer rows than the cursor's, it stops at the last one.
        screen.update(Ok(vec![entry("alpha", "stopped")]), Ok(vec![]), Ok(vec![]));
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 0);
        screen.update(Ok(vec![]), Ok(vec![]), Ok(vec![]));
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 0);
        assert!(screen.cursor_container().is_none());
    }

    #[test]
    fn a_refresh_keeps_the_cursor_on_the_same_image() {
        let mut screen = home();
        screen.images = vec![image("alpine", "edge"), image("postgres", "latest")];
        screen.selected_rows[PANEL_IMAGES] = 1;

        screen.update(
            Ok(screen.containers.clone()),
            Ok(vec![
                image("alpine", "edge"),
                image("nginx", "1"),
                image("postgres", "latest"),
            ]),
            Ok(vec![]),
        );
        assert_eq!(screen.selected_rows[PANEL_IMAGES], 2);

        // Deleted elsewhere: the cursor stays on its row, within the list.
        screen.update(
            Ok(screen.containers.clone()),
            Ok(vec![image("alpine", "edge")]),
            Ok(vec![]),
        );
        assert_eq!(screen.selected_rows[PANEL_IMAGES], 0);
    }

    #[test]
    fn a_list_that_fails_to_load_keeps_its_rows() {
        let mut screen = home();
        let error = screen.update(Err("no answer".to_string()), Ok(vec![]), Ok(vec![]));
        assert_eq!(error.as_deref(), Some("no answer"));
        assert_eq!(screen.containers.len(), 3);
        assert!(screen.images.is_empty());

        // The same failure from every list is reported once.
        fn down<T>() -> Result<T, String> {
            Err("down".to_string())
        }
        let error = screen.update(down(), down(), down());
        assert_eq!(error.as_deref(), Some("down"));

        // So is a volume list that fails on its own, and the volumes stay.
        screen.volumes = vec![volume("pgdata", false)];
        let error = screen.update(
            Ok(home().containers),
            Ok(vec![]),
            Err("volume ls failed".to_string()),
        );
        assert_eq!(error.as_deref(), Some("volume ls failed"));
        assert_eq!(screen.volumes, [volume("pgdata", false)]);
    }

    #[test]
    fn a_created_container_takes_the_cursor_once_listed() {
        let mut screen = home();
        screen.select_when_listed("delta".to_string());

        // Not there yet: nothing moves.
        screen.update(Ok(home().containers), Ok(vec![]), Ok(vec![]));
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 0);

        let mut containers = home().containers;
        containers.push(entry("delta", "stopped"));
        screen.update(Ok(containers.clone()), Ok(vec![]), Ok(vec![]));
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 3);

        // Only once: later refreshes follow the cursor again.
        screen.selected_rows[PANEL_CONTAINERS] = 1;
        screen.update(Ok(containers), Ok(vec![]), Ok(vec![]));
        assert_eq!(screen.selected_rows[PANEL_CONTAINERS], 1);
    }

    #[test]
    fn container_headers_stay_even_with_no_rows() {
        let mut screen = HomeScreen::new();
        let rows = draw(&mut screen, &PendingActions::default(), 80, 24);
        assert!(
            rows[2].contains("ID              STATE       IP"),
            "{}",
            rows[2]
        );
        assert!(rows[3].trim_matches([' ', '│', '┃']).is_empty());
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

    fn footer(screen: &HomeScreen, width: usize) -> String {
        Line::from(screen.footer_hints(width)).to_string()
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

                    let width = Line::from(screen.footer_hints(AVAILABLE)).width();
                    assert!(
                        width <= AVAILABLE,
                        "footer is {width} wide (panel {panel}, help {show_help}), \
                         an 80-column terminal fits {AVAILABLE}"
                    );

                    // Only the arrows and Esc make way.
                    let text = footer(&screen, AVAILABLE);
                    let focused: &[&str] = if panel == PANEL_CONTAINERS {
                        &["s: st", "d/D: delete/purge"]
                    } else {
                        &["↑↓: move", "c: create", "Esc: back"]
                    };
                    let always = ["Shift+↑↓: panel", ".: filter", "/: ", "q: quit"];
                    for hint in focused.iter().chain(&always) {
                        assert!(text.contains(hint), "no {hint:?} in {text:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_wide_footer_shows_every_hint() {
        let screen = home(); // alpha, stopped
        let full = footer(&screen, usize::MAX);
        assert_eq!(
            full,
            " Shift+↑↓: panel ↑↓: move .: filter /: help s: start d/D: delete/purge Esc: back q: quit "
        );

        let width = Line::from(full.as_str()).width();
        assert_eq!(footer(&screen, width), full);
        // One column less and the arrows go first, then Esc.
        assert_eq!(
            footer(&screen, width - 1),
            " Shift+↑↓: panel .: filter /: help s: start d/D: delete/purge Esc: back q: quit "
        );
        // What an 80-column terminal has room for, inside its border.
        assert_eq!(
            footer(&screen, 78),
            " Shift+↑↓: panel .: filter /: help s: start d/D: delete/purge q: quit "
        );
    }

    #[test]
    fn a_narrow_footer_keeps_what_acts_on_the_focused_item() {
        let screen = home();
        assert_eq!(footer(&screen, 40), " s: start d/D: delete/purge q: quit ");
        assert_eq!(footer(&screen, 20), " s: start q: quit ");
        assert_eq!(footer(&screen, 9), " q: quit ");
        assert_eq!(footer(&screen, 0), " ");
    }

    #[test]
    fn delete_hints_show_only_while_a_container_is_focused() {
        let mut screen = home();
        assert!(footer(&screen, usize::MAX).contains(" d/D: delete/purge "));

        screen.active_panel = PANEL_IMAGES;
        assert!(!footer(&screen, usize::MAX).contains("delete"));

        // The filter hides the only container.
        screen.active_panel = PANEL_CONTAINERS;
        screen.containers = vec![entry("alpha", "stopped")];
        screen.container_filter = ContainerFilter::RunningOnly;
        assert!(!footer(&screen, usize::MAX).contains("delete"));
    }

    #[test]
    fn create_hint_shows_only_while_an_image_is_selected() {
        let mut screen = home();
        assert!(screen.focused_image().is_none()); // containers panel has focus

        screen.active_panel = PANEL_IMAGES;
        assert_eq!(
            screen.focused_image().map(|i| i.reference.as_str()),
            Some("docker.io/library/postgres:latest")
        );
    }

    #[test]
    fn the_dialog_only_opens_over_a_selected_image() {
        let mut app = app_with(home());

        // Containers panel has focus, so there is nothing to create from.
        HomeScreen::open_create_dialog(&mut app);
        assert!(home_of(&app).create_dialog.is_none());

        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::open_create_dialog(&mut app);
        assert!(home_of(&app).create_dialog.is_some());
    }

    #[test]
    fn the_dialog_swallows_keys_that_would_otherwise_act() {
        let mut app = app_with(home());
        home_mut(&mut app).active_panel = PANEL_IMAGES;
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
        assert_eq!(home_of(&app).dialog_name(), Some("db-1q"));
        assert!(!app.exit);

        // Spaces and slashes are not valid in a name.
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char(' ')));
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char('/')));
        assert_eq!(home_of(&app).dialog_name(), Some("db-1q"));

        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Backspace));
        assert_eq!(home_of(&app).dialog_name(), Some("db-1"));
    }

    #[test]
    fn a_bare_c_is_a_name_character() {
        let mut app = app_with(home());
        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::open_create_dialog(&mut app);

        for ch in "cache".chars() {
            HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char(ch)));
        }
        assert_eq!(home_of(&app).dialog_name(), Some("cache"));
    }

    #[test]
    fn ctrl_c_cancels_the_dialog_and_discards_the_name() {
        let mut app = app_with(home());
        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::open_create_dialog(&mut app);
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char('d')));

        assert!(HomeScreen::handle_dialog_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
        assert!(home_of(&app).create_dialog.is_none());

        // With the dialog closed the key is no longer swallowed.
        assert!(!HomeScreen::handle_dialog_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
    }

    #[test]
    fn enter_on_an_empty_name_keeps_the_dialog_open() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::open_create_dialog(&mut app);

        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(home_of(&app).dialog_name(), Some(""));
        assert!(probe.actions().is_empty());
    }

    #[test]
    fn enter_creates_from_the_full_reference_in_the_background() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::open_create_dialog(&mut app);
        for ch in "db-1".chars() {
            HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char(ch)));
        }

        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(home_of(&app).create_dialog.is_none());
        assert_eq!(
            probe.actions(),
            [Action::CreateContainer {
                name: "db-1".to_string(),
                image: "docker.io/library/postgres:latest".to_string(),
            }]
        );

        // Once it exists, the next list puts the cursor on it.
        let refresh_seq = app.refresh.sent + 1;
        probe.reply(Reply::Finished {
            action: Action::CreateContainer {
                name: "db-1".to_string(),
                image: "docker.io/library/postgres:latest".to_string(),
            },
            result: Ok(None),
        });
        app.drain_replies();
        assert_eq!(
            probe.refreshes(),
            [Refresh {
                seq: refresh_seq,
                kind: RefreshKind::Home
            }]
        );

        let mut containers = home().containers;
        containers.push(entry("db-1", "stopped"));
        probe.reply(Reply::Lists {
            seq: refresh_seq,
            containers: Ok(containers),
            images: Ok(vec![image("postgres", "latest")]),
            volumes: Ok(vec![]),
        });
        app.drain_replies();
        assert_eq!(
            home_of(&app).cursor_container().map(|c| c.id.as_str()),
            Some("db-1")
        );
    }

    #[test]
    fn the_dialog_keeps_its_image_when_a_refresh_moves_the_cursor() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::open_create_dialog(&mut app);
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Char('x')));

        // postgres was deleted elsewhere while the dialog was open.
        let seq = app.request_refresh();
        probe.reply(Reply::Lists {
            seq,
            containers: Ok(home().containers),
            images: Ok(vec![image("alpine", "edge")]),
            volumes: Ok(vec![]),
        });
        app.drain_replies();
        HomeScreen::handle_dialog_key(&mut app, KeyEvent::from(KeyCode::Enter));

        assert_eq!(
            probe.actions(),
            [Action::CreateContainer {
                name: "x".to_string(),
                image: "docker.io/library/postgres:latest".to_string(),
            }]
        );
    }

    #[test]
    fn s_stops_the_focused_container_in_the_background() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        home_mut(&mut app).selected_rows[PANEL_CONTAINERS] = 1; // bravo, running

        HomeScreen::handle_start_stop_container(&mut app);
        HomeScreen::handle_start_stop_container(&mut app);
        assert_eq!(
            probe.actions(),
            [Action::StopContainer("bravo".to_string())],
            "a second press while stopping does nothing"
        );

        // Another container can go at the same time.
        home_mut(&mut app).selected_rows[PANEL_CONTAINERS] = 0;
        HomeScreen::handle_start_stop_container(&mut app);
        assert_eq!(
            probe.actions(),
            [Action::StartContainer("alpha".to_string())]
        );

        // Nothing is focused on the images panel.
        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::handle_start_stop_container(&mut app);
        assert!(probe.actions().is_empty());
    }

    fn press_in_dialog(app: &mut App, key: KeyEvent) -> bool {
        HomeScreen::handle_dialog_key(app, key)
    }

    fn delete(id: &str, force: bool, volumes: &[&str]) -> Action {
        Action::DeleteContainer {
            id: id.to_string(),
            force,
            volumes: volumes.iter().map(|v| v.to_string()).collect(),
        }
    }

    #[test]
    fn d_asks_before_deleting_the_focused_container() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        home_mut(&mut app).selected_rows[PANEL_CONTAINERS] = 2; // charlie, stopped

        HomeScreen::open_delete_dialog(&mut app, false);
        assert!(probe.actions().is_empty(), "nothing goes before the answer");

        // Keys that would act on the screen only reach the dialog.
        for code in [
            KeyCode::Char('q'),
            KeyCode::Char('s'),
            KeyCode::Char('d'),
            KeyCode::Up,
        ] {
            assert!(press_in_dialog(&mut app, KeyEvent::from(code)));
        }
        assert!(!app.exit);
        assert_eq!(home_of(&app).selected_rows[PANEL_CONTAINERS], 2);
        assert!(probe.actions().is_empty());

        assert!(press_in_dialog(&mut app, KeyEvent::from(KeyCode::Enter)));
        assert!(home_of(&app).delete_dialog.is_none());
        assert_eq!(probe.actions(), [delete("charlie", false, &[])]);
        // With the dialog gone, keys are no longer swallowed.
        assert!(!press_in_dialog(&mut app, KeyEvent::from(KeyCode::Enter)));
    }

    #[test]
    fn y_confirms_and_a_running_container_is_forced() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        home_mut(&mut app).selected_rows[PANEL_CONTAINERS] = 1; // bravo, running

        HomeScreen::open_delete_dialog(&mut app, false);
        press_in_dialog(&mut app, KeyEvent::from(KeyCode::Char('y')));
        assert_eq!(probe.actions(), [delete("bravo", true, &[])]);
    }

    #[test]
    fn esc_n_and_ctrl_c_cancel_the_delete() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        for key in [
            KeyEvent::from(KeyCode::Esc),
            KeyEvent::from(KeyCode::Char('n')),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            HomeScreen::open_delete_dialog(&mut app, true);
            assert!(home_of(&app).delete_dialog.is_some());
            assert!(press_in_dialog(&mut app, key));
            assert!(home_of(&app).delete_dialog.is_none(), "{key:?}");
        }

        // Ctrl+Y isn't a yes.
        HomeScreen::open_delete_dialog(&mut app, false);
        press_in_dialog(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        );
        assert!(home_of(&app).delete_dialog.is_some());
        assert!(probe.actions().is_empty());
    }

    #[test]
    fn capital_d_takes_the_anonymous_volumes_only_it_mounts() {
        let mut screen = home();
        screen.containers[0].volumes = ["9af1", "pgdata", "shared"].map(String::from).to_vec();
        screen.containers[2].volumes = vec!["shared".to_string()];
        screen.volumes = vec![
            volume("9af1", true),
            volume("pgdata", false),
            volume("shared", true),
        ];
        let (mut app, probe) = App::with_probe(Screen::Home(screen));

        // `d` keeps all three.
        HomeScreen::open_delete_dialog(&mut app, false);
        press_in_dialog(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(probe.actions(), [delete("alpha", false, &[])]);

        // A failure clears the way for another try.
        let seq = app.refresh.sent;
        probe.reply(Reply::Finished {
            action: delete("alpha", false, &[]),
            result: Err("busy".to_string()),
        });
        app.drain_replies();
        assert!(app.refresh.sent > seq);

        HomeScreen::open_delete_dialog(&mut app, true);
        press_in_dialog(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(probe.actions(), [delete("alpha", false, &["9af1"])]);
    }

    #[test]
    fn delete_needs_a_focused_container() {
        let mut app = app_with(home());
        home_mut(&mut app).active_panel = PANEL_IMAGES;
        HomeScreen::open_delete_dialog(&mut app, false);
        HomeScreen::open_delete_dialog(&mut app, true);
        assert!(home_of(&app).delete_dialog.is_none());

        let mut app = app_with(HomeScreen::new());
        HomeScreen::open_delete_dialog(&mut app, true);
        assert!(home_of(&app).delete_dialog.is_none());
    }

    #[test]
    fn delete_waits_for_a_command_on_the_same_container() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        home_mut(&mut app).selected_rows[PANEL_CONTAINERS] = 1; // bravo
        HomeScreen::handle_start_stop_container(&mut app);
        assert_eq!(
            probe.actions(),
            [Action::StopContainer("bravo".to_string())]
        );

        HomeScreen::open_delete_dialog(&mut app, false);
        assert!(home_of(&app).delete_dialog.is_none());

        // Another container can go meanwhile.
        home_mut(&mut app).selected_rows[PANEL_CONTAINERS] = 0;
        HomeScreen::open_delete_dialog(&mut app, false);
        assert!(home_of(&app).delete_dialog.is_some());
    }

    #[test]
    fn the_dialog_closes_when_its_container_is_deleted_elsewhere() {
        let mut app = app_with(home());
        HomeScreen::open_delete_dialog(&mut app, false); // alpha
        let screen = home_mut(&mut app);

        screen.update(Ok(home().containers), Ok(vec![]), Ok(vec![]));
        assert!(screen.delete_dialog.is_some());
        // A list that couldn't be read proves nothing.
        screen.update(Err("timeout".to_string()), Ok(vec![]), Ok(vec![]));
        assert!(screen.delete_dialog.is_some());

        screen.update(Ok(vec![entry("bravo", "running")]), Ok(vec![]), Ok(vec![]));
        assert!(screen.delete_dialog.is_none());
    }

    #[test]
    fn the_dialog_does_what_it_asked_even_if_the_container_changes() {
        let (mut app, probe) = App::with_probe(Screen::Home(home()));
        HomeScreen::open_delete_dialog(&mut app, false); // alpha, stopped

        // Started elsewhere: the user wasn't asked about killing it, so the
        // CLI refuses rather than forcing it.
        home_mut(&mut app).update(Ok(vec![entry("alpha", "running")]), Ok(vec![]), Ok(vec![]));
        press_in_dialog(&mut app, KeyEvent::from(KeyCode::Enter));
        assert_eq!(probe.actions(), [delete("alpha", false, &[])]);
    }

    #[test]
    fn the_dialog_is_drawn_over_the_lists() {
        let mut app = app_with(home());
        home_mut(&mut app).selected_rows[PANEL_CONTAINERS] = 1; // bravo
        HomeScreen::open_delete_dialog(&mut app, true);

        let rows = draw(home_mut(&mut app), &PendingActions::default(), 80, 24);
        assert!(row_containing(&rows, "┌ Purge container ").is_some());
        assert!(row_containing(&rows, "│ bravo is running. Kill it and delete it?").is_some());
        assert!(row_containing(&rows, "│ It has no volumes.").is_some());
        assert!(row_containing(&rows, "Enter: purge  Esc: cancel").is_some());
        // The footer underneath doesn't change.
        assert!(rows[23].contains("d/D: delete/purge"), "{}", rows[23]);
    }

    #[test]
    fn a_container_being_deleted_says_so() {
        let mut screen = home();
        let mut pending = PendingActions::default();
        pending.push(delete("bravo", true, &["9af1"]));
        let rows = draw(&mut screen, &pending, 80, 24);

        assert!(rows[4].contains("bravo           purging…"), "{}", rows[4]);
        assert!(rows[1].ends_with(" Purging bravo… ┐┃"), "{}", rows[1]);
    }

    #[test]
    fn esc_closes_the_help_panel_before_leaving_the_screen() {
        let mut app = app_with(home());

        HomeScreen::toggle_help(&mut app);
        assert!(home_of(&app).show_help);

        // First Esc only dismisses the panel; the screen stays put.
        HomeScreen::handle_esc(&mut app);
        assert!(!home_of(&app).show_help);

        HomeScreen::handle_esc(&mut app);
        assert!(matches!(app.screen, Screen::Status(ref s) if !s.auto_navigate));
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
