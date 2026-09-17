//! Commands the user asks for, and the ones the screen doesn't show the
//! result of yet.

use std::time::{Duration, Instant};

use ratatui::text::Span;

/// Braille spinner shown next to a running command.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_FRAME: Duration = Duration::from_millis(80);

/// A `container` command run on the user's behalf. Each one knows its
/// arguments and how to describe itself while it runs and when it fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    StartSystem,
    StopSystem,
    StartContainer(String),
    StopContainer(String),
    /// `image` is the full reference, such as `docker.io/library/postgres:latest`.
    CreateContainer {
        name: String,
        image: String,
    },
    /// `force` kills the container first if it is running. `volumes` are
    /// anonymous volumes of it to delete once it is gone, as `docker rm -v`
    /// would; `container delete` has no option for that.
    DeleteContainer {
        id: String,
        force: bool,
        volumes: Vec<String>,
    },
}

impl Action {
    /// The arguments after `container`. `--` ends the options, so an ID that
    /// starts with `-` is still read as an ID.
    pub(crate) fn args(&self) -> Vec<String> {
        let args: &[&str] = match self {
            Action::StartSystem => &["system", "start"],
            Action::StopSystem => &["system", "stop"],
            Action::StartContainer(id) => &["start", "--", id],
            Action::StopContainer(id) => &["stop", "--", id],
            Action::CreateContainer { name, image } => {
                &["create", &format!("--name={name}"), "--", image]
            }
            Action::DeleteContainer { id, force, .. } => {
                if *force {
                    &["delete", "--force", "--", id]
                } else {
                    &["delete", "--", id]
                }
            }
        };
        args.iter().map(|arg| arg.to_string()).collect()
    }

    /// Shown while the command runs.
    fn progress(&self) -> String {
        match self {
            Action::StartSystem => "Starting the container system".to_string(),
            Action::StopSystem => "Stopping the container system".to_string(),
            Action::StartContainer(id) => format!("Starting {id}"),
            Action::StopContainer(id) => format!("Stopping {id}"),
            Action::CreateContainer { name, .. } => format!("Creating {name}"),
            Action::DeleteContainer { id, volumes, .. } if volumes.is_empty() => {
                format!("Deleting {id}")
            }
            Action::DeleteContainer { id, .. } => format!("Purging {id}"),
        }
    }

    /// Heads the notice when the command fails.
    pub(crate) fn failure(&self) -> String {
        match self {
            Action::StartSystem => "Couldn't start the container system".to_string(),
            Action::StopSystem => "Couldn't stop the container system".to_string(),
            Action::StartContainer(id) => format!("Couldn't start {id}"),
            Action::StopContainer(id) => format!("Couldn't stop {id}"),
            Action::CreateContainer { name, .. } => format!("Couldn't create {name}"),
            Action::DeleteContainer { id, volumes, .. } if volumes.is_empty() => {
                format!("Couldn't delete {id}")
            }
            Action::DeleteContainer { id, .. } => format!("Couldn't purge {id}"),
        }
    }

    /// What the container's STATE cell says while the command runs.
    fn row_state(&self) -> Option<(&str, &'static str)> {
        match self {
            Action::StartContainer(id) => Some((id, "starting…")),
            Action::StopContainer(id) => Some((id, "stopping…")),
            Action::DeleteContainer { id, volumes, .. } if volumes.is_empty() => {
                Some((id, "deleting…"))
            }
            Action::DeleteContainer { id, .. } => Some((id, "purging…")),
            _ => None,
        }
    }

    /// The container the command acts on, if it acts on one.
    fn container(&self) -> Option<&str> {
        match self {
            Action::StartContainer(id)
            | Action::StopContainer(id)
            | Action::DeleteContainer { id, .. } => Some(id),
            Action::CreateContainer { name, .. } => Some(name),
            Action::StartSystem | Action::StopSystem => None,
        }
    }

    /// Whether both commands act on the same thing, so the second should
    /// wait until the first is done: pressing `s` twice on a stopping
    /// container must not queue a second stop.
    fn overlaps(&self, other: &Action) -> bool {
        let is_system = |a: &Action| matches!(a, Action::StartSystem | Action::StopSystem);
        (is_system(self) && is_system(other))
            || self
                .container()
                .is_some_and(|id| other.container() == Some(id))
    }
}

#[derive(Debug)]
struct Pending {
    action: Action,
    started: Instant,
    /// Set once the command has succeeded: the first refresh whose answer
    /// shows its effect. Until that lands, the row keeps saying "stopping…"
    /// rather than flashing the state from before the command.
    settled_by: Option<u64>,
}

/// Commands sent to the worker whose effect isn't on screen yet, oldest
/// first. The worker runs them in that order, so the first unfinished one is
/// the one running now.
#[derive(Debug, Default)]
pub(crate) struct PendingActions {
    entries: Vec<Pending>,
}

impl PendingActions {
    pub(crate) fn push(&mut self, action: Action) {
        self.entries.push(Pending {
            action,
            started: Instant::now(),
            settled_by: None,
        });
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `action` should be ignored because a command on the same
    /// target is still outstanding.
    pub(crate) fn blocks(&self, action: &Action) -> bool {
        self.entries.iter().any(|p| p.action.overlaps(action))
    }

    /// `action` succeeded; refresh `seq` is the first that shows its effect.
    pub(crate) fn succeeded(&mut self, action: &Action, seq: u64) {
        if let Some(pending) = self.unfinished(action) {
            pending.settled_by = Some(seq);
        }
    }

    /// `action` failed. Nothing changed, so there is nothing to wait for.
    pub(crate) fn failed(&mut self, action: &Action) {
        let index = self
            .entries
            .iter()
            .position(|p| p.settled_by.is_none() && p.action == *action);
        if let Some(index) = index {
            self.entries.remove(index);
        }
    }

    /// Refresh `seq` has landed; forget everything it shows.
    pub(crate) fn settled(&mut self, seq: u64) {
        self.entries
            .retain(|p| p.settled_by.is_none_or(|needed| seq < needed));
    }

    fn unfinished(&mut self, action: &Action) -> Option<&mut Pending> {
        self.entries
            .iter_mut()
            .find(|p| p.settled_by.is_none() && p.action == *action)
    }

    /// "⠋ Stopping postgres…", with "+2" when two more are queued behind it,
    /// cut to fit `width` columns.
    pub(crate) fn progress(&self, now: Instant, width: usize) -> Option<String> {
        let current = self
            .entries
            .iter()
            .find(|p| p.settled_by.is_none())
            .or_else(|| self.entries.first())?;
        let queued = self
            .entries
            .iter()
            .filter(|p| p.settled_by.is_none())
            .count()
            .saturating_sub(1);

        let frame =
            now.saturating_duration_since(current.started).as_millis() / SPINNER_FRAME.as_millis();
        let spinner = SPINNER[frame as usize % SPINNER.len()];
        let suffix = if queued > 0 {
            format!(" +{queued}")
        } else {
            String::new()
        };

        let text = current.action.progress();
        // Spinner, space, text, ellipsis, suffix.
        let fixed = width_of(spinner) + 1 + 1 + width_of(&suffix);
        if fixed + 1 > width {
            return (width > 0).then(|| spinner.to_string());
        }
        let text = truncate(&text, width - fixed);
        Some(format!("{spinner} {text}…{suffix}"))
    }

    /// Replacement text for container `id`'s STATE cell.
    pub(crate) fn row_state(&self, id: &str) -> Option<&'static str> {
        self.entries
            .iter()
            .find_map(|p| match p.action.row_state() {
                Some((target, state)) if target == id => Some(state),
                _ => None,
            })
    }
}

fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

/// The longest start of `text` that fits in `width` columns.
fn truncate(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (index, ch) in text.char_indices() {
        used += width_of(ch.encode_utf8(&mut [0; 4]));
        if used > width {
            return &text[..index];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(id: &str) -> Action {
        Action::StartContainer(id.to_string())
    }

    fn stop(id: &str) -> Action {
        Action::StopContainer(id.to_string())
    }

    #[test]
    fn commands_end_their_options_before_the_ids() {
        assert_eq!(stop("postgres").args(), ["stop", "--", "postgres"]);
        assert_eq!(start("-odd").args(), ["start", "--", "-odd"]);
        assert_eq!(Action::StopSystem.args(), ["system", "stop"]);
        assert_eq!(
            Action::CreateContainer {
                name: "db-1".to_string(),
                image: "docker.io/library/postgres:latest".to_string(),
            }
            .args(),
            [
                "create",
                "--name=db-1",
                "--",
                "docker.io/library/postgres:latest"
            ]
        );
    }

    fn delete(id: &str, force: bool, volumes: &[&str]) -> Action {
        Action::DeleteContainer {
            id: id.to_string(),
            force,
            volumes: volumes.iter().map(|v| v.to_string()).collect(),
        }
    }

    #[test]
    fn delete_forces_only_a_running_container() {
        assert_eq!(delete("web", false, &[]).args(), ["delete", "--", "web"]);
        assert_eq!(
            delete("-web", true, &["9af1cc76"]).args(),
            ["delete", "--force", "--", "-web"]
        );
    }

    #[test]
    fn a_delete_says_whether_volumes_go_too() {
        assert_eq!(delete("web", true, &[]).progress(), "Deleting web");
        assert_eq!(delete("web", false, &[]).failure(), "Couldn't delete web");
        let with_volumes = delete("web", false, &["9af1cc76"]);
        assert_eq!(with_volumes.progress(), "Purging web");
        assert_eq!(with_volumes.failure(), "Couldn't purge web");

        let mut pending = PendingActions::default();
        pending.push(delete("db", false, &[]));
        assert_eq!(pending.row_state("db"), Some("deleting…"));
        pending.push(with_volumes);
        assert_eq!(pending.row_state("web"), Some("purging…"));
        assert!(pending.blocks(&start("web")));
        assert!(pending.blocks(&delete("web", true, &[])));
        assert!(!pending.blocks(&delete("redis", false, &[])));
    }

    #[test]
    fn progress_names_the_running_command() {
        let mut pending = PendingActions::default();
        assert_eq!(pending.progress(Instant::now(), 80), None);

        pending.push(stop("postgres"));
        let started = pending.entries[0].started;
        assert_eq!(
            pending.progress(started, 80).as_deref(),
            Some("⠋ Stopping postgres…")
        );
        // The spinner turns with time.
        assert_eq!(
            pending.progress(started + SPINNER_FRAME, 80).as_deref(),
            Some("⠙ Stopping postgres…")
        );
    }

    #[test]
    fn progress_counts_the_commands_queued_behind() {
        let mut pending = PendingActions::default();
        pending.push(stop("postgres"));
        pending.push(start("redis"));
        pending.push(start("web"));
        let now = pending.entries[0].started;
        assert_eq!(
            pending.progress(now, 80).as_deref(),
            Some("⠋ Stopping postgres… +2")
        );

        // Once the first is done, the next one is what's running.
        pending.succeeded(&stop("postgres"), 7);
        let shown = pending.progress(now, 80).unwrap();
        assert!(shown.ends_with("Starting redis… +1"), "{shown}");
    }

    #[test]
    fn progress_shrinks_to_fit() {
        let mut pending = PendingActions::default();
        pending.push(stop("a-container-with-a-long-name"));
        pending.push(start("redis"));
        let now = pending.entries[0].started;

        // The name gives way; the ellipsis and the queue count stay.
        assert_eq!(
            pending.progress(now, 20).as_deref(),
            Some("⠋ Stopping a-con… +1")
        );
        assert_eq!(width_of(&pending.progress(now, 20).unwrap()), 20);
        assert_eq!(pending.progress(now, 3).as_deref(), Some("⠋"));
        assert_eq!(pending.progress(now, 0), None);
    }

    #[test]
    fn a_second_press_on_the_same_target_is_blocked() {
        let mut pending = PendingActions::default();
        pending.push(stop("postgres"));

        assert!(pending.blocks(&start("postgres")));
        assert!(pending.blocks(&stop("postgres")));
        assert!(!pending.blocks(&stop("redis")));
        assert!(!pending.blocks(&Action::StopSystem));

        pending.push(Action::StartSystem);
        assert!(pending.blocks(&Action::StopSystem));
    }

    #[test]
    fn a_created_name_is_blocked_until_it_is_listed() {
        let mut pending = PendingActions::default();
        pending.push(Action::CreateContainer {
            name: "db-1".to_string(),
            image: "postgres".to_string(),
        });
        assert!(pending.blocks(&start("db-1")));
    }

    #[test]
    fn a_success_stays_pending_until_its_refresh_lands() {
        let mut pending = PendingActions::default();
        pending.push(stop("postgres"));
        pending.succeeded(&stop("postgres"), 5);

        // An older refresh still shows the container running.
        pending.settled(4);
        assert_eq!(pending.row_state("postgres"), Some("stopping…"));

        pending.settled(5);
        assert_eq!(pending.row_state("postgres"), None);
        assert!(pending.is_empty());
    }

    #[test]
    fn a_failure_is_forgotten_at_once() {
        let mut pending = PendingActions::default();
        pending.push(start("postgres"));
        pending.push(start("redis"));
        pending.failed(&start("postgres"));

        assert_eq!(pending.row_state("postgres"), None);
        assert_eq!(pending.row_state("redis"), Some("starting…"));
    }

    #[test]
    fn unfinished_commands_survive_any_refresh() {
        let mut pending = PendingActions::default();
        pending.push(stop("postgres"));
        pending.settled(100);
        assert_eq!(pending.row_state("postgres"), Some("stopping…"));
    }

    #[test]
    fn truncate_counts_columns_not_bytes() {
        assert_eq!(truncate("postgres", 4), "post");
        assert_eq!(truncate("日本語", 4), "日本");
        assert_eq!(truncate("日本語", 5), "日本");
        assert_eq!(truncate("ok", 10), "ok");
    }
}
