use std::time::Instant;

use serde::Deserialize;

use super::actions::{Action, PendingActions};
use super::{App, HomeScreen, Screen, cli};
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
    /// `container system status` hasn't answered yet.
    #[default]
    Checking,
    Running,
    NotRunning,
}

/// One answer from `container system status`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SystemStatus {
    pub(crate) status: ContainerStatus,
    pub(crate) message: String,
}

impl SystemStatus {
    pub(crate) fn is_running(&self) -> bool {
        self.status == ContainerStatus::Running
    }
}

/// Asks the system how it is. This blocks, for up to 10 s when the API
/// server is registered but doesn't answer, so only the worker calls it.
pub(crate) fn fetch_system_status() -> SystemStatus {
    match cli::run(&["system", "status", "--format", "json"]) {
        Ok(output) => parse_system_status(&output),
        Err(e) => SystemStatus {
            status: ContainerStatus::NotRunning,
            message: format!("Failed to run container: {e}"),
        },
    }
}

/// `container system status --format json`, cut down to what the screen
/// shows. Every field is optional, so a CLI release that drops one only
/// shortens the summary.
#[derive(Deserialize)]
struct StatusJson {
    status: Option<String>,
    server: Option<ServerJson>,
    resources: Option<ResourcesJson>,
}

#[derive(Deserialize)]
struct ServerJson {
    version: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResourcesJson {
    containers_running: Option<u64>,
    containers_total: Option<u64>,
    images: Option<u64>,
}

impl StatusJson {
    /// "API server 1.4.1 · 1 of 3 containers running · 5 images", leaving
    /// out whatever the CLI didn't report.
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(version) = self.server.as_ref().and_then(|s| s.version.as_deref()) {
            parts.push(format!("API server {version}"));
        }
        if let Some(resources) = &self.resources {
            if let (Some(running), Some(total)) =
                (resources.containers_running, resources.containers_total)
            {
                parts.push(format!("{running} of {total} containers running"));
            }
            match resources.images {
                Some(1) => parts.push("1 image".to_string()),
                Some(images) => parts.push(format!("{images} images")),
                None => {}
            }
        }
        parts.join(" · ")
    }
}

/// The exit status decides whether the system runs, as it does for the CLI;
/// the JSON only fills in the message.
fn parse_system_status(output: &cli::Output) -> SystemStatus {
    let json = serde_json::from_str::<StatusJson>(&output.stdout).ok();

    if output.success {
        return SystemStatus {
            status: ContainerStatus::Running,
            message: json.map(|json| json.summary()).unwrap_or_default(),
        };
    }

    // The same sentences the CLI prints in its table format.
    let message = match json.and_then(|json| json.status).as_deref() {
        Some("unregistered") => {
            "apiserver is not running and not registered with launchd".to_string()
        }
        Some("not running") => "apiserver is not running".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => output.failure(),
    };
    SystemStatus {
        status: ContainerStatus::NotRunning,
        message,
    }
}

#[derive(Debug, Default)]
pub(crate) struct StatusScreen {
    pub(crate) status: ContainerStatus,
    pub(crate) message: String,
    /// `container --version`, read once when the app starts.
    pub(crate) version: String,
    /// Jump straight to the home screen as soon as the system reports `Running`.
    /// Cleared when the user walks back here with `<Esc>`, so they don't get
    /// bounced out of the screen they just asked for.
    pub(crate) auto_navigate: bool,
}

impl StatusScreen {
    /// A screen still waiting for its first status.
    pub(crate) fn new(version: String, auto_navigate: bool) -> Self {
        Self {
            status: ContainerStatus::Checking,
            message: String::new(),
            version,
            auto_navigate,
        }
    }

    pub(crate) fn update(&mut self, status: SystemStatus) {
        self.status = status.status;
        self.message = status.message;
    }

    pub(crate) fn handle_enter(app: &mut App) {
        let should_change =
            matches!(&app.screen, Screen::Status(s) if s.status == ContainerStatus::Running);
        if should_change {
            app.show(Screen::Home(HomeScreen::new()));
        }
    }

    /// `s` starts or stops the system in the background; the screen follows
    /// once a refresh sees the change.
    pub(crate) fn handle_start_stop(app: &mut App) {
        let Screen::Status(status) = &mut app.screen else {
            return;
        };
        let action = match status.status {
            ContainerStatus::Running => Action::StopSystem,
            ContainerStatus::NotRunning => Action::StartSystem,
            ContainerStatus::Checking => return,
        };
        // One start or stop at a time.
        if app.pending.blocks(&action) {
            return;
        }
        // A failed stop must not fling the user into the home screen.
        status.auto_navigate = action == Action::StartSystem;
        app.submit(action);
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, pending: &PendingActions) {
        let status_span = match self.status {
            ContainerStatus::Running => "Running".green().bold(),
            ContainerStatus::NotRunning => "Not Running".red().bold(),
            ContainerStatus::Checking => "Checking…".dark_gray().bold(),
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
            ContainerStatus::Checking => Line::from(vec![" Quit ".into(), "<Q> ".blue().bold()]),
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

        let width = block.inner(area).width as usize;
        if let Some(progress) = pending.progress(Instant::now(), width) {
            lines.push(Line::from(progress).yellow());
        } else if self.auto_navigate && self.status == ContainerStatus::NotRunning {
            lines.push(Line::from("Waiting for the system to start…").dim());
        }

        let content = Text::from(lines);

        Paragraph::new(content)
            .centered()
            .block(block)
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `container system status --format json` from CLI 1.4.1.
    const RUNNING: &str = r#"{"client":{"appName":"container","build":"release","commit":"9a8917ca2da5cd6ba059b9ba5ca5a74892e9bb7d","version":"1.4.1"},"host":{"architecture":"arm64","cpus":8,"operatingSystem":"Version 27.0 (Build 26A428)"},"paths":{"appRoot":"/Users/me/Library/Application Support/com.apple.container/","installRoot":"/usr/local/"},"resources":{"containersRunning":1,"containersTotal":3,"images":5},"server":{"appName":"container-apiserver","build":"release","commit":"9a8917ca2da5cd6ba059b9ba5ca5a74892e9bb7d","version":"1.4.1"},"status":"running"}"#;

    fn screen_text(screen: &StatusScreen, pending: &PendingActions) -> Vec<String> {
        let area = Rect::new(0, 0, 80, 8);
        let mut buf = Buffer::empty(area);
        screen.render(area, &mut buf, pending);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_matches(|c: char| c == '┃' || c.is_whitespace())
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn a_running_system_is_summarised() {
        let status = parse_system_status(&cli::Output::exited(0, RUNNING, ""));
        assert_eq!(status.status, ContainerStatus::Running);
        assert_eq!(
            status.message,
            "API server 1.4.1 · 1 of 3 containers running · 5 images"
        );
    }

    #[test]
    fn the_summary_leaves_out_what_the_cli_omits() {
        let status = parse_system_status(&cli::Output::exited(
            0,
            r#"{"status":"running","resources":{"containersRunning":0,"images":1}}"#,
            "",
        ));
        assert_eq!(status.message, "1 image");

        // Unreadable output still counts: the exit status says it's running.
        let status = parse_system_status(&cli::Output::exited(0, "FIELD  VALUE", ""));
        assert!(status.is_running());
        assert_eq!(status.message, "");
    }

    #[test]
    fn a_stopped_system_says_why_as_the_cli_does() {
        let unregistered =
            parse_system_status(&cli::Output::exited(1, r#"{"status":"unregistered"}"#, ""));
        assert_eq!(unregistered.status, ContainerStatus::NotRunning);
        assert_eq!(
            unregistered.message,
            "apiserver is not running and not registered with launchd"
        );

        let silent =
            parse_system_status(&cli::Output::exited(1, r#"{"status":"not running"}"#, ""));
        assert_eq!(silent.message, "apiserver is not running");

        let other = parse_system_status(&cli::Output::exited(1, r#"{"status":"booting"}"#, ""));
        assert_eq!(other.message, "booting");
    }

    #[test]
    fn a_failure_without_json_shows_stderr() {
        let status = parse_system_status(&cli::Output::exited(
            64,
            "",
            "Error: Unknown option '--format'\n",
        ));
        assert_eq!(status.status, ContainerStatus::NotRunning);
        assert_eq!(status.message, "Unknown option '--format'");
    }

    #[test]
    fn the_screen_shows_status_and_summary() {
        let mut screen = StatusScreen::new("container CLI version 1.4.1".to_string(), false);
        screen.update(parse_system_status(&cli::Output::exited(0, RUNNING, "")));

        let text = screen_text(&screen, &PendingActions::default());
        assert_eq!(text[1], "container CLI version 1.4.1");
        assert_eq!(text[2], "Status: Running");
        assert_eq!(
            text[3],
            "API server 1.4.1 · 1 of 3 containers running · 5 images"
        );
        assert!(
            text[7].contains("Containers <Enter> Stop <S> Quit <Q>"),
            "{}",
            text[7]
        );
    }

    #[test]
    fn the_screen_says_it_is_checking_until_the_first_answer() {
        let screen = StatusScreen::new(String::new(), true);
        let text = screen_text(&screen, &PendingActions::default());
        assert_eq!(text[2], "Status: Checking…");
        // Nothing to start or stop until the status is known.
        assert!(text[7].contains("Quit <Q>"));
        assert!(!text[7].contains("<S>"));
    }

    #[test]
    fn a_running_start_replaces_the_waiting_line() {
        let mut screen = StatusScreen::new(String::new(), true);
        screen.update(SystemStatus {
            status: ContainerStatus::NotRunning,
            message: "apiserver is not running".to_string(),
        });
        assert_eq!(
            screen_text(&screen, &PendingActions::default())[4],
            "Waiting for the system to start…"
        );

        let mut pending = PendingActions::default();
        pending.push(Action::StartSystem);
        let line = &screen_text(&screen, &pending)[4];
        assert!(line.ends_with("Starting the container system…"), "{line}");
    }
}
