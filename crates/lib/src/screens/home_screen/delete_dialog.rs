//! The dialog `d` and `D` open. It asks before a container is deleted and
//! says what goes with it.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Clear, Padding, Widget},
};

use super::centered;
use super::container_list_panel::ContainerEntry;
use super::volumes::VolumeEntry;
use crate::screens::actions::Action;
use crate::screens::notice::wrap;

/// Wide enough for a UUID volume name under its heading.
const WIDTH: u16 = 60;

/// Columns the border and the padding take on each side.
const FRAME: u16 = 2;

/// How far volume names sit under their heading.
const NAME_INDENT: usize = 2;

/// A volume the container mounts that stays when it goes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KeptVolume {
    pub(crate) name: String,
    /// Another container that mounts this anonymous volume, which is why
    /// `D` keeps it.
    pub(crate) shared_with: Option<String>,
}

/// The open delete dialog. It holds copies, so a refresh can't change what
/// the user is agreeing to.
#[derive(Debug)]
pub(crate) struct DeleteDialog {
    pub(crate) id: String,
    /// The container was running, so it is killed first (`--force`).
    pub(crate) force: bool,
    /// Opened with `D`, which deletes anonymous volumes too.
    pub(crate) with_volumes: bool,
    /// The anonymous volumes that go with the container. Empty for `d`.
    pub(crate) deleted_volumes: Vec<String>,
    /// Every other volume it mounts.
    pub(crate) kept_volumes: Vec<KeptVolume>,
}

impl DeleteDialog {
    /// Asks about deleting `container`, given every container and volume the
    /// lists know of. `D` takes the anonymous volumes no other container
    /// mounts, as `docker rm -v` does; named volumes always stay.
    pub(crate) fn new(
        container: &ContainerEntry,
        with_volumes: bool,
        containers: &[ContainerEntry],
        volumes: &[VolumeEntry],
    ) -> Self {
        let mut deleted_volumes = Vec::new();
        let mut kept_volumes = Vec::new();
        for name in &container.volumes {
            let anonymous = volumes.iter().any(|v| v.name == *name && v.anonymous);
            let shared_with = containers
                .iter()
                .find(|c| c.id != container.id && c.volumes.contains(name))
                .map(|c| c.id.clone());
            if with_volumes && anonymous && shared_with.is_none() {
                deleted_volumes.push(name.clone());
            } else {
                kept_volumes.push(KeptVolume {
                    name: name.clone(),
                    // Only worth saying where it explains why a volume stays.
                    shared_with: shared_with.filter(|_| with_volumes && anonymous),
                });
            }
        }

        Self {
            id: container.id.clone(),
            force: container.is_running(),
            with_volumes,
            deleted_volumes,
            kept_volumes,
        }
    }

    /// The command the dialog agrees to.
    pub(crate) fn action(&self) -> Action {
        Action::DeleteContainer {
            id: self.id.clone(),
            force: self.force,
            volumes: self.deleted_volumes.clone(),
        }
    }

    /// The body, one paragraph at a time: its text, how far it is indented,
    /// and its style.
    fn paragraphs(&self) -> Vec<(String, usize, Style)> {
        let dim = Style::new().dark_gray();
        let danger = Style::new().red();
        let mut body = Vec::new();

        let question = if self.force {
            format!("{} is running. Kill it and delete it?", self.id)
        } else {
            format!("Delete {}?", self.id)
        };
        body.push((question, 0, Style::new().bold()));

        let has_volumes = !self.deleted_volumes.is_empty() || !self.kept_volumes.is_empty();
        let lost = if has_volumes {
            "Whatever it stored outside its volumes is lost."
        } else {
            "Whatever it stored is lost."
        };
        body.push((lost.to_string(), 0, dim));
        if self.with_volumes && !has_volumes {
            body.push(("It has no volumes.".to_string(), 0, dim));
        }

        let heading = match self.deleted_volumes.len() {
            0 => None,
            1 => Some("Its anonymous volume is deleted too:"),
            _ => Some("Its anonymous volumes are deleted too:"),
        };
        if let Some(heading) = heading {
            body.push((heading.to_string(), 0, danger));
        }
        for name in &self.deleted_volumes {
            body.push((name.clone(), NAME_INDENT, danger));
        }

        let other = !self.deleted_volumes.is_empty();
        let heading = match (other, self.kept_volumes.len()) {
            (_, 0) => None,
            (false, 1) => Some("Its volume is kept:"),
            (false, _) => Some("Its volumes are kept:"),
            (true, 1) => Some("Its other volume is kept:"),
            (true, _) => Some("Its other volumes are kept:"),
        };
        if let Some(heading) = heading {
            body.push((heading.to_string(), 0, dim));
        }
        for kept in &self.kept_volumes {
            let text = match &kept.shared_with {
                Some(other) => format!("{} ({other} uses it too)", kept.name),
                None => kept.name.clone(),
            };
            body.push((text, NAME_INDENT, Style::new()));
        }

        body
    }

    /// The body, wrapped to `width` columns.
    fn lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for (text, indent, style) in self.paragraphs() {
            for piece in wrap(&text, width.saturating_sub(indent)) {
                lines.push(Line::styled(
                    format!("{}{piece}", " ".repeat(indent)),
                    style,
                ));
            }
        }
        lines
    }

    /// Drawn over the middle of `area`, sized to its text.
    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let width = WIDTH.min(area.width);
        let mut lines = self.lines(width.saturating_sub(2 * FRAME) as usize);
        // More volumes than the screen has rows: the last row that fits
        // says some are missing.
        let room = area.height.saturating_sub(2) as usize;
        if lines.len() > room {
            lines.truncate(room);
            if let Some(last) = lines.last_mut() {
                *last = Line::from(format!("{}…", " ".repeat(NAME_INDENT))).dark_gray();
            }
        }

        let popup = centered(area, width, lines.len() as u16 + 2);
        Clear.render(popup, buf);

        let (title, verb) = if self.with_volumes {
            (" Purge container ", ": purge  ")
        } else {
            (" Delete container ", ": delete  ")
        };
        let block = Block::bordered()
            .title(Line::from(title).bold())
            .title_bottom(
                Line::from(vec![
                    " Enter".blue().bold(),
                    verb.dark_gray(),
                    "Esc".blue().bold(),
                    ": cancel ".dark_gray(),
                ])
                .centered(),
            )
            .border_style(Style::new().red())
            .padding(Padding::horizontal(1));

        let inner = block.inner(popup);
        block.render(popup, buf);
        Text::from(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANONYMOUS: &str = "9af1cc76-45ca-4205-9ed8-df97a4f132a8";
    const SHARED: &str = "0c5e2b8a-51f3-4c8e-9d41-7a2f7c1e6b90";

    fn container(id: &str, state: &str, volumes: &[&str]) -> ContainerEntry {
        ContainerEntry {
            id: id.to_string(),
            state: state.to_string(),
            volumes: volumes.iter().map(|v| v.to_string()).collect(),
            ..ContainerEntry::default()
        }
    }

    fn volumes() -> Vec<VolumeEntry> {
        [(ANONYMOUS, true), (SHARED, true), ("pgdata", false)]
            .into_iter()
            .map(|(name, anonymous)| VolumeEntry {
                name: name.to_string(),
                anonymous,
            })
            .collect()
    }

    /// A dialog for `web`, which mounts all three volumes; `job` mounts the
    /// shared one too.
    fn dialog(state: &str, with_volumes: bool) -> DeleteDialog {
        let web = container("web", state, &[ANONYMOUS, "pgdata", SHARED]);
        let containers = [web.clone(), container("job", "stopped", &[SHARED])];
        DeleteDialog::new(&web, with_volumes, &containers, &volumes())
    }

    fn text(dialog: &DeleteDialog) -> Vec<String> {
        dialog
            .paragraphs()
            .into_iter()
            .map(|(text, indent, _)| format!("{}{text}", " ".repeat(indent)))
            .collect()
    }

    /// The dialog drawn over an empty screen, one string per row.
    fn draw(dialog: &DeleteDialog, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        dialog.render(area, &mut buf);
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn d_keeps_every_volume() {
        let dialog = dialog("stopped", false);
        assert!(!dialog.force);
        assert_eq!(
            dialog.action(),
            Action::DeleteContainer {
                id: "web".to_string(),
                force: false,
                volumes: vec![],
            }
        );
        assert_eq!(
            text(&dialog),
            [
                "Delete web?",
                "Whatever it stored outside its volumes is lost.",
                "Its volumes are kept:",
                &format!("  {ANONYMOUS}"),
                "  pgdata",
                &format!("  {SHARED}"),
            ]
        );
    }

    #[test]
    fn capital_d_takes_only_the_anonymous_volumes_nobody_else_mounts() {
        let dialog = dialog("running", true);
        assert_eq!(
            dialog.action(),
            Action::DeleteContainer {
                id: "web".to_string(),
                force: true,
                volumes: vec![ANONYMOUS.to_string()],
            }
        );
        assert_eq!(
            text(&dialog),
            [
                "web is running. Kill it and delete it?",
                "Whatever it stored outside its volumes is lost.",
                "Its anonymous volume is deleted too:",
                &format!("  {ANONYMOUS}"),
                "Its other volumes are kept:",
                "  pgdata",
                &format!("  {SHARED} (job uses it too)"),
            ]
        );
    }

    #[test]
    fn a_container_without_volumes_says_so() {
        let bare = container("db", "stopped", &[]);
        let d = DeleteDialog::new(&bare, false, &[], &[]);
        assert_eq!(text(&d), ["Delete db?", "Whatever it stored is lost."]);

        let capital_d = DeleteDialog::new(&bare, true, &[], &[]);
        assert_eq!(
            text(&capital_d),
            [
                "Delete db?",
                "Whatever it stored is lost.",
                "It has no volumes."
            ]
        );
        assert_eq!(capital_d.action(), d.action());
    }

    #[test]
    fn capital_d_with_nothing_to_delete_lists_what_stays() {
        let named = container("db", "stopped", &["pgdata"]);
        let dialog = DeleteDialog::new(&named, true, std::slice::from_ref(&named), &volumes());
        assert!(dialog.deleted_volumes.is_empty());
        assert_eq!(text(&dialog)[2..], ["Its volume is kept:", "  pgdata"]);
    }

    #[test]
    fn a_volume_the_lists_dont_know_is_kept() {
        let web = container("web", "stopped", &["unlisted"]);
        let dialog = DeleteDialog::new(&web, true, std::slice::from_ref(&web), &[]);
        assert!(dialog.deleted_volumes.is_empty());
        assert_eq!(dialog.kept_volumes[0].name, "unlisted");
    }

    #[test]
    fn several_anonymous_volumes_go_together() {
        let web = container("web", "stopped", &[ANONYMOUS, SHARED]);
        let dialog = DeleteDialog::new(&web, true, std::slice::from_ref(&web), &volumes());
        assert_eq!(dialog.deleted_volumes, [ANONYMOUS, SHARED]);
        assert_eq!(text(&dialog)[2], "Its anonymous volumes are deleted too:");
        assert_eq!(text(&dialog).len(), 5);
    }

    #[test]
    fn the_dialog_is_drawn_in_the_middle_with_its_keys() {
        let rows = draw(&dialog("stopped", true), 80, 24);
        let top = rows.iter().position(|r| r.contains("┌")).unwrap();
        assert!(rows[top].contains("┌ Purge container ─"), "{}", rows[top]);
        assert!(rows[top].starts_with(&" ".repeat(10)), "{}", rows[top]);
        assert!(
            rows[top + 1].contains("│ Delete web? "),
            "{}",
            rows[top + 1]
        );
        assert!(
            rows[top + 4].contains(&format!("│   {ANONYMOUS} ")),
            "{}",
            rows[top + 4]
        );
        // Seven lines of text between the borders.
        assert!(
            rows[top + 8].contains("Enter: purge  Esc: cancel"),
            "{}",
            rows[top + 8]
        );
        assert!(rows[top + 8].contains('└'));
    }

    #[test]
    fn a_long_name_wraps_inside_the_dialog() {
        let long = format!("{ANONYMOUS}-{ANONYMOUS}");
        let web = container(&long, "running", &[]);
        let dialog = DeleteDialog::new(&web, false, &[], &[]);
        let rows = draw(&dialog, 80, 24);
        let top = rows.iter().position(|r| r.contains("┌")).unwrap();
        assert!(rows[top].contains("┌ Delete container ─"));
        // The 73-column name is cut at the edge, and the question carries on
        // after the rest of it.
        assert!(rows[top + 1].contains(&format!("│ {} │", &long[..56])));
        assert!(rows[top + 2].contains(&format!(
            "│ {} is running. Kill it and delete it?",
            &long[56..]
        )));
        assert!(rows[top + 3].contains("│ Whatever it stored is lost."));
    }

    #[test]
    fn a_short_screen_cuts_the_volume_list() {
        let many: Vec<String> = (0..30).map(|n| format!("vol-{n:02}")).collect();
        let names: Vec<&str> = many.iter().map(String::as_str).collect();
        let web = container("web", "stopped", &names);
        let rows = draw(&DeleteDialog::new(&web, false, &[], &[]), 80, 24);
        assert!(rows[0].contains("┌ Delete container "), "{}", rows[0]);
        assert!(rows[22].contains("│   … "), "{}", rows[22]);
        assert!(rows[23].contains("Enter: delete"), "{}", rows[23]);
    }
}
