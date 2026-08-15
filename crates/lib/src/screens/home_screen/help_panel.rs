use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Widget},
};

/// Widest the panel gets. The longest command below is 38 columns, plus a
/// two-column indent and the panel's own borders.
pub(crate) const WIDTH: u16 = 44;

/// Terminal commands worth keeping at hand, grouped by section.
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Lifecycle",
        &[
            (
                "container create --name <name> <image>",
                "create it without starting it",
            ),
            (
                "container run -d --name <name> <image>",
                "create and start in the background",
            ),
            ("container start <id>", "start a stopped container"),
            ("container stop <id>", "stop a running container"),
            ("container delete <id>", "remove a stopped container"),
        ],
    ),
    (
        "Working with one",
        &[
            ("container exec -it <id> /bin/sh", "open a shell inside it"),
            ("container ls -a", "list every container"),
            ("container logs -f <id>", "follow its output"),
        ],
    ),
    (
        "Images",
        &[
            ("container image pull <image>", "download an image"),
            ("container image ls", "list downloaded images"),
            ("container image delete <image>", "remove one image"),
            ("container image prune", "remove every unused image"),
        ],
    ),
];

pub(crate) fn render(area: Rect, buf: &mut Buffer) {
    let block = Block::bordered()
        .title(Line::from(" Help ").bold())
        .border_style(Style::new().magenta());

    let inner = block.inner(area);
    block.render(area, buf);

    let annotated = lines(true);
    let body = if annotated.len() as u16 <= inner.height {
        annotated
    } else {
        lines(false)
    };

    Text::from(body).render(inner, buf);
}

/// Builds the panel body. Descriptions are the first thing dropped when the
/// terminal is too short to show every command with one.
fn lines(with_descriptions: bool) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    for (section, commands) in SECTIONS {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(*section).magenta().bold());
        for (command, description) in *commands {
            lines.push(Line::from(format!("  {command}")).yellow());
            if with_descriptions {
                lines.push(Line::from(format!("    {description}")).dark_gray());
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_fit_the_panel_width() {
        let inner = (WIDTH - 2) as usize;
        for line in lines(true) {
            assert!(
                line.width() <= inner,
                "help line is {} wide, panel fits {inner}: {line:?}",
                line.width()
            );
        }
    }

    #[test]
    fn compact_body_fits_a_short_terminal() {
        // A 24-row terminal leaves 20 rows inside the outer and help borders.
        let compact = lines(false);
        assert!(
            compact.len() <= 20,
            "compact help needs {} rows, a 24-row terminal has 20",
            compact.len()
        );
        assert!(lines(true).len() > compact.len());
    }
}
