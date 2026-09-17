//! Runs `container` commands on background threads, so the UI never waits
//! for the CLI. `container stop` alone takes up to 5 s, and `system status`
//! up to 10 s when the API server doesn't answer.
//!
//! There are two threads, so neither kind of work queues behind the other:
//! - the action thread runs what the user asked for, one command at a time
//!   and in the order the keys were pressed;
//! - the refresh thread re-reads what the screen shows, so the lists stay
//!   current while a slow stop is still running.
//!
//! Both answer on one channel, which the main loop drains with `try_recv` on
//! every tick, as `crates/examples/external-events` does. Dropping the
//! [`Worker`] closes the request channels, and each thread exits once its
//! current command returns.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use super::actions::Action;
use super::cli;
use super::home_screen::{
    ContainerEntry, DEFAULT_REGISTRY_DOMAIN, ImageEntry, fetch_containers, fetch_images,
    fetch_registry_domain,
};
use super::status_screen::{SystemStatus, fetch_system_status};

/// What a refresh reads, which depends on the screen that asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshKind {
    Status,
    Home,
}

/// A refresh request. `seq` comes back in the reply, so the app can tell
/// which request an answer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refresh {
    pub(crate) seq: u64,
    pub(crate) kind: RefreshKind,
}

#[derive(Debug)]
pub(crate) enum Reply {
    /// `container system status`, for the status screen.
    Status { seq: u64, status: SystemStatus },
    /// Both lists, for the home screen. A list that couldn't be read carries
    /// the reason instead.
    Lists {
        seq: u64,
        containers: Result<Vec<ContainerEntry>, String>,
        images: Result<Vec<ImageEntry>, String>,
    },
    /// The home screen asked for its lists, but the system isn't running.
    SystemDown { seq: u64, status: SystemStatus },
    /// A command the user asked for has exited.
    Finished {
        action: Action,
        result: Result<(), String>,
    },
}

/// The app's end of the two threads.
#[derive(Debug)]
pub(crate) struct Worker {
    actions: Sender<Action>,
    refreshes: Sender<Refresh>,
    replies: Receiver<Reply>,
}

impl Worker {
    pub(crate) fn spawn() -> io::Result<Self> {
        let (actions, action_requests) = mpsc::channel();
        let (refreshes, refresh_requests) = mpsc::channel();
        let (reply_to, replies) = mpsc::channel();

        let action_replies = reply_to.clone();
        thread::Builder::new()
            .name("container-actions".to_string())
            .spawn(move || run_actions(action_requests, action_replies))?;
        thread::Builder::new()
            .name("container-refresh".to_string())
            .spawn(move || run_refreshes(refresh_requests, reply_to))?;

        Ok(Self {
            actions,
            refreshes,
            replies,
        })
    }

    /// Queues `action`. Hands it back if the action thread is gone.
    pub(crate) fn run(&self, action: Action) -> Result<(), Action> {
        self.actions.send(action).map_err(|e| e.0)
    }

    /// Asks for a refresh. If the refresh thread is gone, no answer comes and
    /// the screen keeps what it has.
    pub(crate) fn refresh(&self, refresh: Refresh) {
        let _ = self.refreshes.send(refresh);
    }

    /// The next reply, if one is waiting.
    pub(crate) fn try_recv(&self) -> Option<Reply> {
        self.replies.try_recv().ok()
    }

    /// The next reply, waiting at most `timeout` for it.
    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Option<Reply> {
        self.replies.recv_timeout(timeout).ok()
    }
}

fn run_actions(requests: Receiver<Action>, replies: Sender<Reply>) {
    for action in requests {
        let args = action.args();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = cli::run_action(&args);
        if replies.send(Reply::Finished { action, result }).is_err() {
            break;
        }
    }
}

fn run_refreshes(requests: Receiver<Refresh>, replies: Sender<Reply>) {
    // Decides how image names are shown. The system reads it when it starts,
    // so it is read again only after the system has been seen down.
    let mut registry_domain: Option<String> = None;

    for Refresh { seq, kind } in requests {
        let reply = match kind {
            RefreshKind::Status => Reply::Status {
                seq,
                status: system_status(&mut registry_domain),
            },
            RefreshKind::Home => refresh_home(seq, &mut registry_domain),
        };
        if replies.send(reply).is_err() {
            break;
        }
    }
}

/// `container system status`, forgetting the registry domain whenever the
/// system turns out to be down.
fn system_status(registry_domain: &mut Option<String>) -> SystemStatus {
    let status = fetch_system_status();
    if !status.is_running() {
        *registry_domain = None;
    }
    status
}

fn refresh_home(seq: u64, registry_domain: &mut Option<String>) -> Reply {
    let containers = fetch_containers();
    if containers.is_err() {
        // Listing usually fails because the system went away. If so, say
        // that instead of reporting the failure.
        let status = system_status(registry_domain);
        if !status.is_running() {
            return Reply::SystemDown { seq, status };
        }
    }

    // If the property can't be read, the default is almost certainly right,
    // and asking again on every refresh wouldn't change that.
    let domain = registry_domain.get_or_insert_with(|| {
        fetch_registry_domain().unwrap_or_else(|| DEFAULT_REGISTRY_DOMAIN.to_string())
    });
    let images = fetch_images(domain);

    Reply::Lists {
        seq,
        containers,
        images,
    }
}

/// The far ends of a [`Worker`] that has no threads, so tests decide what
/// the CLI "answers".
#[cfg(test)]
pub(crate) struct WorkerProbe {
    pub(crate) actions: Receiver<Action>,
    pub(crate) refreshes: Receiver<Refresh>,
    pub(crate) replies: Sender<Reply>,
}

#[cfg(test)]
impl WorkerProbe {
    /// Every action queued so far.
    pub(crate) fn actions(&self) -> Vec<Action> {
        self.actions.try_iter().collect()
    }

    /// Every refresh requested so far.
    pub(crate) fn refreshes(&self) -> Vec<Refresh> {
        self.refreshes.try_iter().collect()
    }

    pub(crate) fn reply(&self, reply: Reply) {
        self.replies.send(reply).expect("the app is gone");
    }
}

#[cfg(test)]
impl Worker {
    pub(crate) fn probe() -> (Self, WorkerProbe) {
        let (actions, action_requests) = mpsc::channel();
        let (refreshes, refresh_requests) = mpsc::channel();
        let (reply_to, replies) = mpsc::channel();
        (
            Self {
                actions,
                refreshes,
                replies,
            },
            WorkerProbe {
                actions: action_requests,
                refreshes: refresh_requests,
                replies: reply_to,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dead_action_thread_hands_the_action_back() {
        let (worker, probe) = Worker::probe();
        drop(probe);
        assert_eq!(worker.run(Action::StopSystem), Err(Action::StopSystem));
    }

    #[test]
    fn replies_come_back_in_order() {
        let (worker, probe) = Worker::probe();
        assert!(worker.try_recv().is_none());

        probe.reply(Reply::Finished {
            action: Action::StartSystem,
            result: Ok(()),
        });
        probe.reply(Reply::Finished {
            action: Action::StopSystem,
            result: Err("busy".to_string()),
        });

        assert!(matches!(
            worker.try_recv(),
            Some(Reply::Finished {
                action: Action::StartSystem,
                result: Ok(())
            })
        ));
        assert!(matches!(
            worker.recv_timeout(Duration::from_millis(10)),
            Some(Reply::Finished {
                action: Action::StopSystem,
                result: Err(_)
            })
        ));
        assert!(worker.recv_timeout(Duration::from_millis(10)).is_none());
    }

    /// Reads the real CLI, so it only runs on request, with the container
    /// system up: `cargo test -p lib -- --ignored`. Worth running after a
    /// `container` upgrade, since the JSON it parses may have changed.
    #[test]
    #[ignore = "needs a running container system"]
    fn reads_the_installed_cli() {
        let status = fetch_system_status();
        assert!(status.is_running(), "{status:?}");
        assert!(status.message.starts_with("API server "), "{status:?}");

        let worker = Worker::spawn().unwrap();
        worker.refresh(Refresh {
            seq: 1,
            kind: RefreshKind::Home,
        });
        match worker.recv_timeout(Duration::from_secs(30)) {
            Some(Reply::Lists {
                seq: 1,
                containers,
                images,
            }) => {
                let containers = containers.expect("containers");
                let images = images.expect("images");
                assert!(
                    containers
                        .iter()
                        .all(|c| !c.id.is_empty() && !c.state.is_empty())
                );
                assert!(
                    images
                        .iter()
                        .all(|i| !i.reference.is_empty() && !i.digest.is_empty())
                );
            }
            other => panic!("expected both lists, got {other:?}"),
        }
    }
}
