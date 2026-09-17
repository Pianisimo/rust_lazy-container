use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::Widget,
};

mod actions;
mod cli;
mod home_screen;
mod notice;
mod selection;
mod status_screen;
mod worker;

use actions::{Action, PendingActions};
pub(crate) use home_screen::HomeScreen;
use notice::Notice;
use selection::Selection;
pub(crate) use status_screen::{ContainerStatus, StatusScreen};
use worker::{Refresh, RefreshKind, Reply, Worker};

/// How long the loop waits for a key before doing another pass. Gives the
/// draw loop a heartbeat instead of blocking forever inside `event::read`.
const EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// The heartbeat while the worker owes an answer or a command is running, so
/// replies show promptly and the spinner turns.
const BUSY_POLL_TIMEOUT: Duration = Duration::from_millis(50);

/// How often the current screen is refreshed: `container system status` on
/// the status screen, both lists on the home screen.
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How long a newly opened screen waits for its data before being drawn
/// anyway. The CLI usually answers in a few tens of milliseconds.
const SETTLE_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "the app holds exactly one screen, so the size difference costs nothing"
)]
pub(crate) enum Screen {
    Status(StatusScreen),
    Home(HomeScreen),
}

impl Screen {
    fn refresh_kind(&self) -> RefreshKind {
        match self {
            Screen::Status(_) => RefreshKind::Status,
            Screen::Home(_) => RefreshKind::Home,
        }
    }
}

/// Bookkeeping for refreshes. Normally one is out at a time, and each reply
/// carries the number of the request it answers.
#[derive(Debug, Default)]
struct Refreshes {
    /// Number of the last request sent.
    sent: u64,
    /// Highest request number answered.
    answered: u64,
    last_sent: Option<Instant>,
    /// A command finished while a refresh was out, so that answer may
    /// predate it: ask again once it lands.
    again: bool,
    /// The list failure last shown, so a lasting one is reported once
    /// rather than on every refresh.
    last_error: Option<String>,
}

impl Refreshes {
    fn in_flight(&self) -> bool {
        self.answered < self.sent
    }

    fn is_due(&self) -> bool {
        !self.in_flight()
            && self
                .last_sent
                .is_none_or(|sent| sent.elapsed() >= STATUS_REFRESH_INTERVAL)
    }
}

#[derive(Debug)]
pub struct App {
    pub(crate) screen: Screen,
    pub(crate) exit: bool,
    /// The mouse selection being dragged, or the last one, still highlighted.
    selection: Option<Selection>,
    /// The last frame drawn, which is what a selection copies text from.
    last_frame: Buffer,
    notice: Option<Notice>,
    /// `container --version`, read once: it can't change while the app runs.
    version: String,
    worker: Worker,
    refresh: Refreshes,
    /// Commands sent to the worker whose result isn't on screen yet.
    pending: PendingActions,
    /// A screen has just opened; its data gets a moment to arrive before
    /// the next draw.
    settle_pending: bool,
}

/// Holds mouse capture for as long as it lives. `ratatui::run` restores the
/// rest of the terminal but knows nothing about the mouse; dropping this on
/// the way out, panics included, hands selection back to the terminal.
struct MouseCapture;

impl MouseCapture {
    fn enable() -> io::Result<Self> {
        execute!(io::stdout(), EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

impl App {
    /// Starts the worker threads and opens the status screen, which moves on
    /// to the home screen as soon as the system is seen running.
    pub fn new() -> io::Result<Self> {
        let version = container_version();
        let screen = Screen::Status(StatusScreen::new(version.clone(), true));
        let mut app = Self::with_worker(Worker::spawn()?, version);
        app.show(screen);
        Ok(app)
    }

    /// An app with a placeholder screen; [`App::show`] puts up the real one.
    fn with_worker(worker: Worker, version: String) -> Self {
        Self {
            screen: Screen::Home(HomeScreen::new()),
            exit: false,
            selection: None,
            last_frame: Buffer::default(),
            notice: None,
            version,
            worker,
            refresh: Refreshes::default(),
            pending: PendingActions::default(),
            settle_pending: false,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let _mouse = MouseCapture::enable()?;
        while !self.exit {
            self.drain_replies();
            if self.settle_pending {
                self.wait_for_screen_data();
            }
            if self.refresh.is_due() {
                self.request_refresh();
            }
            let frame = terminal.draw(|frame| self.draw(frame))?;
            self.last_frame = frame.buffer.clone();
            self.handle_events()?;
        }
        Ok(())
    }

    /// Switches to `screen` and asks for its data straight away.
    pub(crate) fn show(&mut self, screen: Screen) {
        self.screen = screen;
        self.refresh.last_error = None;
        self.request_refresh();
        self.settle_pending = true;
    }

    /// Asks the worker to refresh the current screen, and returns the
    /// request's number.
    fn request_refresh(&mut self) -> u64 {
        let refresh = &mut self.refresh;
        refresh.sent += 1;
        refresh.last_sent = Some(Instant::now());
        // This request comes after everything that has finished so far.
        refresh.again = false;
        self.worker.refresh(Refresh {
            seq: refresh.sent,
            kind: self.screen.refresh_kind(),
        });
        refresh.sent
    }

    /// Refreshes as soon as the refresh already out, if any, has landed.
    fn refresh_soon(&mut self) {
        if self.refresh.in_flight() {
            self.refresh.again = true;
        } else {
            self.request_refresh();
        }
    }

    /// Hands `action` to the worker, unless a command on the same target is
    /// still outstanding.
    pub(crate) fn submit(&mut self, action: Action) {
        if self.pending.blocks(&action) {
            return;
        }
        match self.worker.run(action.clone()) {
            Ok(()) => self.pending.push(action),
            Err(action) => {
                self.notice = Some(Notice::command_failed(
                    action.failure(),
                    "The background worker has stopped. Restart lazy-container.",
                ));
            }
        }
    }

    /// Applies every reply the worker has sent since the last tick.
    fn drain_replies(&mut self) {
        while let Some(reply) = self.worker.try_recv() {
            self.apply(reply);
        }
    }

    /// Waits until the refresh a new screen asked for has landed, for at most
    /// `SETTLE_TIMEOUT`, so the screen opens with its data instead of
    /// flashing empty. Other replies that arrive meanwhile are applied too.
    fn wait_for_screen_data(&mut self) {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        while self.refresh.in_flight() {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.worker.recv_timeout(left) {
                Some(reply) => self.apply(reply),
                None => break,
            }
        }
        // A reply may have opened yet another screen; this loop has waited
        // for that one's data as well.
        self.settle_pending = false;
    }

    fn apply(&mut self, reply: Reply) {
        match reply {
            Reply::Finished { action, result } => self.finish(action, result),
            Reply::Status { seq, status } => {
                self.answered(seq);
                if let Screen::Status(screen) = &mut self.screen {
                    screen.update(status);
                    self.auto_navigate();
                }
            }
            Reply::Lists {
                seq,
                containers,
                images,
            } => {
                self.answered(seq);
                if let Screen::Home(home) = &mut self.screen {
                    let error = home.update(containers, images);
                    self.report_list_error(error);
                }
            }
            Reply::SystemDown { seq, status } => {
                self.answered(seq);
                if let Screen::Home(_) = self.screen {
                    // The status screen brings the user back here once the
                    // system is up again.
                    let mut screen = StatusScreen::new(self.version.clone(), true);
                    screen.update(status);
                    self.show(Screen::Status(screen));
                }
            }
        }
    }

    /// Refresh `seq` has landed. Replies for a screen that is no longer
    /// showing still count, so nothing waits on them forever.
    fn answered(&mut self, seq: u64) {
        self.refresh.answered = self.refresh.answered.max(seq);
        self.pending.settled(seq);
        if self.refresh.again && !self.refresh.in_flight() {
            self.request_refresh();
        }
    }

    fn finish(&mut self, action: Action, result: Result<(), String>) {
        match result {
            Ok(()) => {
                // The next refresh sent is the first to show the result.
                self.pending.succeeded(&action, self.refresh.sent + 1);
                if let (Action::CreateContainer { name, .. }, Screen::Home(home)) =
                    (&action, &mut self.screen)
                {
                    home.select_when_listed(name.clone());
                }
            }
            Err(message) => {
                self.pending.failed(&action);
                self.notice = Some(Notice::command_failed(action.failure(), &message));
            }
        }
        self.refresh_soon();
    }

    /// Says why a list couldn't be refreshed, once per distinct failure.
    fn report_list_error(&mut self, error: Option<String>) {
        if let Some(message) = &error
            && self.refresh.last_error.as_ref() != Some(message)
        {
            self.notice = Some(Notice::command_failed(
                "Couldn't refresh the lists".to_string(),
                message,
            ));
        }
        self.refresh.last_error = error;
    }

    /// Leaves the status screen for the home screen as soon as the container
    /// system is up, so a running system never stops on an interstitial.
    fn auto_navigate(&mut self) {
        let should_change = matches!(
            &self.screen,
            Screen::Status(s) if s.auto_navigate && s.status == ContainerStatus::Running
        );
        if should_change {
            self.show(Screen::Home(HomeScreen::new()));
        }
    }

    /// Scroll positions depend on how many rows fit, which only the frame
    /// knows, so they are brought up to date here, just before rendering
    /// from `&self`.
    fn draw(&mut self, frame: &mut Frame) {
        if let Screen::Home(home) = &mut self.screen {
            home.keep_selection_visible(frame.area());
        }
        frame.render_widget(&*self, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        let timeout = if self.refresh.in_flight() || !self.pending.is_empty() {
            BUSY_POLL_TIMEOUT
        } else {
            EVENT_POLL_TIMEOUT
        };
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    self.selection = None;
                    self.handle_key_event(key_event)
                }
                Event::Mouse(mouse_event) => self.handle_mouse_event(mouse_event),
                _ => {}
            };
        }
        Ok(())
    }

    /// Left-drag selects; releasing copies whatever the drag covered.
    fn handle_mouse_event(&mut self, mouse_event: MouseEvent) {
        let area = self.last_frame.area;
        if area.is_empty() {
            return;
        }
        // A drag past the edge of the window keeps selecting up to the edge.
        let at = Position::new(
            mouse_event.column.min(area.right() - 1),
            mouse_event.row.min(area.bottom() - 1),
        );

        match mouse_event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = Some(Selection::start(at));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.extend_to(at);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.extend_to(at);
                }
                self.copy_selection();
            }
            _ => {}
        }
    }

    /// Copies the finished selection and says how much went to the clipboard.
    /// A plain click clears the highlight instead.
    fn copy_selection(&mut self) {
        let Some(selection) = self.selection.filter(|s| !s.is_empty()) else {
            self.selection = None;
            return;
        };
        // The highlight only reverses colours, so the last frame still holds
        // exactly the characters the user saw.
        let text = selection.text(&self.last_frame);
        if text.is_empty() {
            return;
        }
        self.notice = Some(match copy_to_clipboard(&text) {
            Ok(()) => Notice::copied(&text),
            Err(_) => Notice::copy_failed(),
        });
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        // The create-container dialog is modal: while it is open it takes every
        // key, so none of the shortcuts below fire.
        if HomeScreen::handle_dialog_key(self, key_event) {
            return;
        }

        match key_event.code {
            KeyCode::Char('q') => self.exit = true,
            KeyCode::Enter => StatusScreen::handle_enter(self),
            KeyCode::Char('s') => match self.screen {
                Screen::Status(_) => StatusScreen::handle_start_stop(self),
                Screen::Home(_) => HomeScreen::handle_start_stop_container(self),
            },
            KeyCode::Esc => self.handle_esc(),
            KeyCode::Char('.') => HomeScreen::cycle_container_filter(self),
            KeyCode::Char('/') => HomeScreen::toggle_help(self),
            KeyCode::Char('c') => HomeScreen::open_create_dialog(self),
            KeyCode::Up if key_event.modifiers.contains(KeyModifiers::SHIFT) => {
                HomeScreen::handle_shift_up(self)
            }
            KeyCode::Down if key_event.modifiers.contains(KeyModifiers::SHIFT) => {
                HomeScreen::handle_shift_down(self)
            }
            KeyCode::Up => HomeScreen::move_up(self),
            KeyCode::Down => HomeScreen::move_down(self),
            _ => {}
        }
    }

    /// An error stays up until Esc, and dismissing it comes before anything
    /// else Esc does.
    fn handle_esc(&mut self) {
        if self.notice.as_ref().is_some_and(Notice::is_sticky) {
            self.notice = None;
        } else {
            HomeScreen::handle_esc(self);
        }
    }
}

/// `container --version`, read at startup. It doesn't talk to the system
/// service, so it answers at once even when the service is down.
fn container_version() -> String {
    cli::run(&["--version"])
        .map(|output| output.stdout.trim().to_string())
        .unwrap_or_default()
}

/// Puts `text` on the system clipboard through `pbcopy`; the container CLI is
/// macOS-only, so the app is too.
pub(crate) fn copy_to_clipboard(text: &str) -> io::Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Dropping stdin closes the pipe, which is what lets `pbcopy` finish.
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("pbcopy has no stdin"))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("pbcopy exited with {status}")))
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        match &self.screen {
            Screen::Status(s) => s.render(area, buf, &self.pending),
            Screen::Home(h) => h.render(area, buf, &self.pending),
        }

        if let Some(notice) = self.notice.as_ref().filter(|n| n.is_visible()) {
            notice.render(area, buf);
        }
        if let Some(selection) = &self.selection {
            selection.highlight(buf);
        }
    }
}

#[cfg(test)]
impl App {
    /// An app whose worker has no threads and nobody listening: whatever it
    /// asks for goes nowhere.
    pub(crate) fn with_screen(screen: Screen) -> Self {
        Self::with_probe(screen).0
    }

    /// An app wired to a probe that sees its requests and sends its replies.
    pub(crate) fn with_probe(screen: Screen) -> (Self, worker::WorkerProbe) {
        let (worker, probe) = Worker::probe();
        let mut app = Self::with_worker(worker, "container CLI version 1.4.1".to_string());
        app.screen = screen;
        (app, probe)
    }
}

#[cfg(test)]
mod tests {
    use super::home_screen::ContainerEntry;
    use super::status_screen::SystemStatus;
    use super::*;

    fn running() -> SystemStatus {
        SystemStatus {
            status: ContainerStatus::Running,
            message: String::new(),
        }
    }

    fn stopped() -> SystemStatus {
        SystemStatus {
            status: ContainerStatus::NotRunning,
            message: "apiserver is not running".to_string(),
        }
    }

    fn container(id: &str, state: &str) -> ContainerEntry {
        ContainerEntry {
            id: id.to_string(),
            state: state.to_string(),
            addr: String::new(),
        }
    }

    fn home_with(containers: Vec<ContainerEntry>) -> Screen {
        let mut home = HomeScreen::new();
        home.update(Ok(containers), Ok(vec![]));
        Screen::Home(home)
    }

    fn status_screen(status: SystemStatus, auto_navigate: bool) -> Screen {
        let mut screen = StatusScreen::new(String::new(), auto_navigate);
        screen.update(status);
        Screen::Status(screen)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key_event(KeyEvent::from(code));
    }

    fn lists(seq: u64, containers: Vec<ContainerEntry>) -> Reply {
        Reply::Lists {
            seq,
            containers: Ok(containers),
            images: Ok(vec![]),
        }
    }

    fn screen_text(app: &App) -> String {
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        app.render(area, &mut buf);
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_home_screen_opens_once_the_system_runs() {
        let (mut app, probe) =
            App::with_probe(Screen::Status(StatusScreen::new(String::new(), true)));

        let seq = app.request_refresh();
        probe.reply(Reply::Status {
            seq,
            status: stopped(),
        });
        app.drain_replies();
        assert!(
            matches!(app.screen, Screen::Status(ref s) if s.status == ContainerStatus::NotRunning)
        );

        let seq = app.request_refresh();
        probe.reply(Reply::Status {
            seq,
            status: running(),
        });
        app.drain_replies();
        assert!(matches!(app.screen, Screen::Home(_)));
        // The home screen asks for its lists at once, and waits for them.
        assert_eq!(
            probe.refreshes().last().map(|r| r.kind),
            Some(RefreshKind::Home)
        );
        assert!(app.settle_pending);
    }

    #[test]
    fn esc_to_the_status_screen_stays_there() {
        let (mut app, probe) = App::with_probe(home_with(vec![]));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.screen, Screen::Status(ref s) if !s.auto_navigate));

        let refresh = probe.refreshes().pop().unwrap();
        assert_eq!(refresh.kind, RefreshKind::Status);
        probe.reply(Reply::Status {
            seq: refresh.seq,
            status: running(),
        });
        app.drain_replies();
        assert!(
            matches!(app.screen, Screen::Status(ref s) if s.status == ContainerStatus::Running)
        );
    }

    #[test]
    fn losing_the_system_goes_back_to_the_status_screen() {
        let (mut app, probe) = App::with_probe(home_with(vec![container("web", "running")]));

        let seq = app.request_refresh();
        probe.reply(Reply::SystemDown {
            seq,
            status: stopped(),
        });
        app.drain_replies();
        match &app.screen {
            Screen::Status(status) => {
                assert_eq!(status.status, ContainerStatus::NotRunning);
                assert_eq!(status.message, "apiserver is not running");
                assert!(status.auto_navigate);
            }
            Screen::Home(_) => panic!("still on the home screen"),
        }

        // It comes back on its own once the system is up again.
        let seq = probe.refreshes().pop().unwrap().seq;
        probe.reply(Reply::Status {
            seq,
            status: running(),
        });
        app.drain_replies();
        assert!(matches!(app.screen, Screen::Home(_)));
    }

    #[test]
    fn replies_for_another_screen_are_ignored() {
        let (mut app, probe) = App::with_probe(home_with(vec![]));
        let seq = app.request_refresh();
        probe.reply(Reply::Status {
            seq,
            status: stopped(),
        });
        app.drain_replies();
        assert!(matches!(app.screen, Screen::Home(_)));
        assert!(!app.refresh.in_flight());

        let (mut app, probe) = App::with_probe(status_screen(running(), false));
        let seq = app.request_refresh();
        probe.reply(Reply::SystemDown {
            seq,
            status: stopped(),
        });
        probe.reply(lists(seq, vec![container("web", "running")]));
        app.drain_replies();
        assert!(
            matches!(app.screen, Screen::Status(ref s) if s.status == ContainerStatus::Running)
        );
    }

    #[test]
    fn refreshes_go_out_one_at_a_time_on_the_interval() {
        let (mut app, probe) = App::with_probe(home_with(vec![]));
        assert!(app.refresh.is_due());

        let seq = app.request_refresh();
        assert!(!app.refresh.is_due(), "one is already out");

        probe.reply(lists(seq, vec![]));
        app.drain_replies();
        assert!(!app.refresh.is_due(), "the interval hasn't passed");

        app.refresh.last_sent = Instant::now().checked_sub(STATUS_REFRESH_INTERVAL);
        assert!(app.refresh.is_due());
    }

    #[test]
    fn a_failed_command_shows_its_error_until_esc() {
        let (mut app, probe) = App::with_probe(home_with(vec![container("web", "stopped")]));
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(probe.actions(), [Action::StartContainer("web".to_string())]);
        assert!(screen_text(&app).contains("Starting web…"));

        probe.reply(Reply::Finished {
            action: Action::StartContainer("web".to_string()),
            result: Err("get failed: container web not found".to_string()),
        });
        app.drain_replies();
        let text = screen_text(&app);
        assert!(text.contains("Couldn't start web"), "{text}");
        assert!(
            text.contains("get failed: container web not found"),
            "{text}"
        );
        assert!(!text.contains("Starting web…"), "{text}");

        // A refresh confirms the state, and nothing is left pending, so `s`
        // can try again.
        assert_eq!(probe.refreshes().len(), 1);
        assert!(app.pending.is_empty());
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(probe.actions(), [Action::StartContainer("web".to_string())]);

        // The first Esc only dismisses the error.
        press(&mut app, KeyCode::Esc);
        assert!(app.notice.is_none());
        assert!(matches!(app.screen, Screen::Home(_)));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.screen, Screen::Status(_)));
    }

    #[test]
    fn a_finished_command_stays_pending_until_a_newer_refresh_lands() {
        let (mut app, probe) = App::with_probe(home_with(vec![container("web", "running")]));
        let before = app.request_refresh();

        press(&mut app, KeyCode::Char('s'));
        probe.reply(Reply::Finished {
            action: Action::StopContainer("web".to_string()),
            result: Ok(()),
        });
        app.drain_replies();
        assert_eq!(app.pending.row_state("web"), Some("stopping…"));
        assert_eq!(
            probe.refreshes().len(),
            1,
            "only the refresh that was already out"
        );

        // The answer to that one predates the stop.
        probe.reply(lists(before, vec![container("web", "running")]));
        app.drain_replies();
        assert_eq!(app.pending.row_state("web"), Some("stopping…"));
        assert!(screen_text(&app).contains("stopping…"));

        // So another goes out at once, and its answer settles it.
        let after = probe.refreshes();
        assert_eq!(after.len(), 1);
        assert!(after[0].seq > before);
        probe.reply(lists(after[0].seq, vec![container("web", "stopped")]));
        app.drain_replies();
        assert!(app.pending.is_empty());
        let text = screen_text(&app);
        assert!(text.contains("stopped"), "{text}");
        assert!(!text.contains("stopping…"), "{text}");
    }

    #[test]
    fn a_list_failure_is_reported_once() {
        let (mut app, probe) = App::with_probe(home_with(vec![container("web", "running")]));
        let failing = |seq| Reply::Lists {
            seq,
            containers: Err("unexpected output".to_string()),
            images: Ok(vec![]),
        };

        let seq = app.request_refresh();
        probe.reply(failing(seq));
        app.drain_replies();
        assert!(app.notice.as_ref().is_some_and(Notice::is_sticky));
        assert!(screen_text(&app).contains("web"), "the old rows stay");

        press(&mut app, KeyCode::Esc);
        let seq = app.request_refresh();
        probe.reply(failing(seq));
        app.drain_replies();
        assert!(app.notice.is_none(), "the same failure isn't shown again");

        // Once it recovers, a new failure is news again.
        let seq = app.request_refresh();
        probe.reply(lists(seq, vec![]));
        app.drain_replies();
        let seq = app.request_refresh();
        probe.reply(failing(seq));
        app.drain_replies();
        assert!(app.notice.is_some());
    }

    #[test]
    fn s_starts_a_stopped_system_and_the_home_screen_follows() {
        let (mut app, probe) = App::with_probe(status_screen(stopped(), false));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(
            probe.actions(),
            [Action::StartSystem],
            "one start at a time"
        );
        assert!(matches!(app.screen, Screen::Status(ref s) if s.auto_navigate));
        assert!(screen_text(&app).contains("Starting the container system…"));

        let next = app.refresh.sent + 1;
        probe.reply(Reply::Finished {
            action: Action::StartSystem,
            result: Ok(()),
        });
        app.drain_replies();
        assert_eq!(probe.refreshes().pop().map(|r| r.seq), Some(next));

        probe.reply(Reply::Status {
            seq: next,
            status: running(),
        });
        app.drain_replies();
        assert!(matches!(app.screen, Screen::Home(_)));
        assert!(app.pending.is_empty());
    }

    #[test]
    fn s_stops_a_running_system_without_leaving_the_screen() {
        let (mut app, probe) = App::with_probe(status_screen(running(), true));
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(probe.actions(), [Action::StopSystem]);

        // A status taken before the stop landed still says running.
        let seq = app.request_refresh();
        probe.reply(Reply::Status {
            seq,
            status: running(),
        });
        app.drain_replies();
        assert!(matches!(app.screen, Screen::Status(_)));
    }

    #[test]
    fn nothing_starts_before_the_status_is_known() {
        let (mut app, probe) =
            App::with_probe(Screen::Status(StatusScreen::new(String::new(), true)));
        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Enter);
        assert!(probe.actions().is_empty());
        assert!(matches!(app.screen, Screen::Status(_)));
    }

    #[test]
    fn a_new_screen_is_drawn_with_its_data() {
        let (mut app, probe) =
            App::with_probe(Screen::Status(StatusScreen::new(String::new(), true)));
        app.show(Screen::Status(StatusScreen::new(String::new(), true)));
        let first = app.refresh.sent;

        // The status answer opens the home screen, whose lists come next.
        probe.reply(Reply::Status {
            seq: first,
            status: running(),
        });
        probe.reply(lists(first + 1, vec![container("web", "running")]));
        app.wait_for_screen_data();

        assert!(!app.settle_pending);
        assert!(!app.refresh.in_flight());
        assert!(screen_text(&app).contains("web"));
    }

    #[test]
    fn waiting_for_a_screen_gives_up_in_time() {
        let (mut app, _probe) = App::with_probe(home_with(vec![]));
        app.show(Screen::Home(HomeScreen::new()));

        let started = Instant::now();
        app.wait_for_screen_data();
        let waited = started.elapsed();
        assert!(waited >= SETTLE_TIMEOUT, "{waited:?}");
        assert!(waited < SETTLE_TIMEOUT * 3, "{waited:?}");
        assert!(!app.settle_pending);
        // The answer is still expected, and applied whenever it lands.
        assert!(app.refresh.in_flight());
    }

    #[test]
    fn a_dead_worker_is_reported() {
        let mut app = App::with_screen(home_with(vec![container("web", "stopped")]));
        press(&mut app, KeyCode::Char('s'));
        assert!(app.notice.as_ref().is_some_and(Notice::is_sticky));
        assert!(app.pending.is_empty());
    }
}
