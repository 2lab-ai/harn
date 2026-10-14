//! `harn` on a terminal: what loads before the first message, grouped by kind (`model`), with
//! one card per action (`d` turn off, `o` turn back on, `u` undo, `w` where from, `e` edit).
//! [`App`] is the terminal-free core (keys in, frames out); [`run`] owns raw
//! mode, the alternate screen and the event loop, and restores the terminal on every exit path:
//! `q`, an error, a panic, and SIGINT / SIGTERM / SIGHUP (a flag the loop polls every
//! [`TICK`]; when the loop cannot get back to it, a watchdog thread restores what it can and
//! exits with the same code a second later, never in the middle of a write).

mod app;
pub mod model;
mod view;

pub use app::{App, Message, Part, Pending, Staged, Tone, TRIM_HINT};
pub use model::Sort;

use crate::journal::Journal;
use crate::model::{Target, Tree};
use crate::resolve::Resolver;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::Terminal;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Once};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    /// Eighth-block bars, arrows and box borders; ASCII under `--no-unicode`.
    pub unicode: bool,
    /// Colors (yellow warnings, red errors); modifiers only without.
    pub color: bool,
    /// `--readonly`: d/o/u/e are left out of the key bar and refused; the journal dir is not
    /// created.
    pub readonly: bool,
}

/// How the view ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// `q`, `Esc`, or `Ctrl-C` read as a key.
    Quit,
    /// A signal arrived (SIGINT, SIGTERM, SIGHUP); the CLI exits with 128 + it.
    Signal(i32),
}

/// How long the loop waits for an event before it looks at the signal flag again.
pub const TICK: Duration = Duration::from_millis(100);

/// Where events come from: the terminal, or a script in tests.
pub trait Events {
    /// The next event within `timeout`; None when nothing came.
    fn next(&mut self, timeout: Duration) -> io::Result<Option<Event>>;
}

struct TerminalEvents;

impl Events for TerminalEvents {
    fn next(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        if event::poll(timeout)? {
            event::read().map(Some)
        } else {
            Ok(None)
        }
    }
}

/// The event loop: draw, wait up to [`TICK`] for an event, hand key presses to `app`, give
/// pending work to `suspend`. Returns when the app quits or a signal number lands in `signal`.
pub fn drive<B: Backend>(
    app: &mut App,
    terminal: &mut Terminal<B>,
    events: &mut dyn Events,
    signal: &AtomicUsize,
    mut suspend: impl FnMut(&mut App, Pending, &mut Terminal<B>) -> anyhow::Result<()>,
) -> anyhow::Result<Exit>
where
    B::Error: Send + Sync + 'static,
{
    loop {
        match signal.load(Ordering::SeqCst) {
            0 => {}
            n => return Ok(Exit::Signal(n as i32)),
        }
        if app.should_quit() {
            return Ok(Exit::Quit);
        }
        terminal.draw(|frame| app.draw(frame))?;
        // Press only: terminals with the kitty protocol also report releases and repeats.
        if let Some(Event::Key(key)) = events.next(TICK)? {
            if key.kind == KeyEventKind::Press {
                app.handle_key(key);
            }
        }
        if let Some(pending) = app.take_pending() {
            suspend(app, pending, terminal)?;
        }
    }
}

/// The terminal is in raw mode on the alternate screen.
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn enter() -> io::Result<()> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    execute!(io::stdout(), EnterAlternateScreen, crossterm::cursor::Hide)
}

/// Back to the shell's screen: cooked mode and the main screen, both attempted. Marked done
/// only when both succeed, so a failure can be retried; a no-op when already done.
fn leave() -> io::Result<()> {
    if !ACTIVE.load(Ordering::SeqCst) {
        return Ok(());
    }
    let raw = disable_raw_mode();
    let screen = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    raw.and(screen)?;
    ACTIVE.store(false, Ordering::SeqCst);
    Ok(())
}

/// Best effort on the way out of [`run`], whatever path it takes, and bounded
/// ([`leave_briefly`]): a terminal that takes no more output does not hold harn up.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        leave_briefly(NoThread::Inline);
    }
}

/// Once per process: a panic restores the terminal before the message prints.
fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = leave();
            previous(info);
        }));
    });
}

/// SIGINT / SIGTERM / SIGHUP write their number into the returned flag (instead of killing the
/// process inside a read with the terminal raw); the ids unregister them.
fn trap_signals() -> io::Result<(Arc<AtomicUsize>, Vec<signal_hook::SigId>)> {
    let flag = Arc::new(AtomicUsize::new(0));
    let mut ids = Vec::new();
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        ids.push(signal_hook::flag::register_usize(
            signal,
            Arc::clone(&flag),
            signal as usize,
        )?);
    }
    Ok((flag, ids))
}

/// How long a signal may go unanswered before the watchdog stops the process. The loop can be
/// stuck where it never sees the flag: on a hung-up terminal (its tmux session killed), crossterm
/// 0.29 retries a read that returns 0 bytes, or fails with anything but WouldBlock, without end
/// (`event/source/unix/mio.rs`), and SIGTERM or SIGHUP only set the flag.
const GRACE: Duration = Duration::from_secs(1);

/// The bit of the watchdog's state that says it has claimed the exit; the bits below count the
/// [`Busy`] holds. One atomic word, so a write's admission and the exit claim exclude each other:
/// the claim only goes from exactly 0 (no write), and no hold is admitted once it is set.
const EXITING: usize = 1 << (usize::BITS - 1);

/// The most holds the bits below [`EXITING`] can count; one more is refused, never carried into
/// the exit bit.
const MAX_HOLDS: usize = EXITING - 1;

/// Holds the watchdog off while it lives: harn is writing (a plan, an undo, the journal), where a
/// forced exit would leave the files and the journal disagreeing, or `$EDITOR` runs, whose Ctrl-C
/// reaches harn too. A signal that lands meanwhile is acted on after: by the loop, or by the
/// watchdog a full grace later.
pub(crate) struct Busy(Arc<AtomicUsize>);

impl Busy {
    /// Admission before a write, on the watchdog's `state`. Once the watchdog has claimed the
    /// exit, never returns: the process is stopping, and no write may start. (Nor while
    /// [`MAX_HOLDS`] holds are live, which no harn reaches.)
    pub(crate) fn hold(state: &Arc<AtomicUsize>) -> Busy {
        loop {
            if let Some(busy) = Busy::try_hold(state) {
                return busy;
            }
            thread::park();
        }
    }

    /// Admission, or None once the exit is claimed (or the count is full).
    fn try_hold(state: &Arc<AtomicUsize>) -> Option<Busy> {
        // A plain CAS loop (`fetch_update` is deprecated on newer toolchains, `try_update` absent on
        // older ones): refuse once the exit is claimed, else count one more hold.
        let mut held = state.load(Ordering::SeqCst);
        loop {
            if held & EXITING != 0 || held == MAX_HOLDS {
                return None;
            }
            match state.compare_exchange_weak(held, held + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Busy(Arc::clone(state))),
                Err(now) => held = now,
            }
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The watchdog takes the exit: only while no write is admitted, and none is admitted after.
fn claim_exit(state: &AtomicUsize) -> bool {
    state
        .compare_exchange(0, EXITING, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// The watchdog's view of the signal flag.
#[derive(Debug, Default)]
struct Watch {
    /// When the flag was first seen set, since it was last seen clear with nothing [`Busy`].
    since: Option<Instant>,
}

impl Watch {
    /// One look at the flag at `now`: 128 + the signal once one has gone unanswered for `grace`.
    /// A clear flag, or harn [`Busy`], starts the wait over.
    fn look(&mut self, signal: usize, busy: bool, now: Instant, grace: Duration) -> Option<i32> {
        if signal == 0 || busy {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now);
        (now.saturating_duration_since(since) >= grace).then_some(128 + signal as i32)
    }
}

/// Looks at `signal` and `state` every [`TICK`] until `stop` hangs up (the end of [`run`]). A
/// signal unanswered for `grace`: the exit claimed, then `restore`, then `exit` with 128 + it.
fn watch(
    signal: &AtomicUsize,
    state: &AtomicUsize,
    stop: &Receiver<()>,
    grace: Duration,
    restore: impl FnOnce(),
    exit: impl FnOnce(i32),
) {
    let mut watch = Watch::default();
    while let Err(RecvTimeoutError::Timeout) = stop.recv_timeout(TICK) {
        let busy = state.load(Ordering::SeqCst) != 0;
        let Some(code) = watch.look(signal.load(Ordering::SeqCst), busy, Instant::now(), grace)
        else {
            continue;
        };
        if claim_exit(state) {
            restore();
            exit(code);
            return;
        }
        // A write was admitted after the look: it runs whole, and a full grace follows it.
        watch.since = None;
    }
}

/// [`watch`] on a thread of its own while [`run`] lasts. Dropped on every way out of `run`, a
/// panic too: the thread sees the hang-up at once and is joined, so it never acts after `run`
/// returned (unless it was already exiting the process, with the code `run` would have given).
struct Watchdog {
    /// Never sent on: dropping it is the stop.
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    /// `state`: the [`Busy`] holds and the exit claim ([`EXITING`]).
    fn start(
        signal: Arc<AtomicUsize>,
        state: Arc<AtomicUsize>,
        grace: Duration,
        restore: impl FnOnce() + Send + 'static,
        exit: impl FnOnce(i32) + Send + 'static,
    ) -> io::Result<Watchdog> {
        let (stop, stopped) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("harn-watchdog".to_string())
            .spawn(move || watch(&signal, &state, &stopped, grace, restore, exit))?;
        Ok(Watchdog {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What [`leave_briefly`] does when no thread can be spawned for [`leave`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoThread {
    /// [`leave`] here, unbounded: the normal way out of [`run`], as before there was a watchdog.
    Inline,
    /// No restore: the watchdog must reach its exit whatever the terminal does, and once it has
    /// claimed the exit every write waits on that exit.
    Skip,
}

/// [`leave`] on a thread of its own, waited for at most [`GRACE`]: a terminal that takes no more
/// output (its writes block) cannot keep the process alive. A hung-up one fails at once.
fn leave_briefly(no_thread: NoThread) {
    /// A thread of this function is still in [`leave`].
    static LEAVING: AtomicBool = AtomicBool::new(false);
    leave_bounded(
        &LEAVING,
        no_thread,
        |work| thread::Builder::new().spawn(work).map(drop),
        || {
            let _ = leave();
        },
    );
}

/// [`leave_briefly`] with the thread and the restore handed in. One at a time (`leaving`): while
/// an earlier `leave` is still stuck, a second is not started.
fn leave_bounded(
    leaving: &'static AtomicBool,
    no_thread: NoThread,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
    leave: impl Fn() + Send + Sync + 'static,
) {
    if leaving.swap(true, Ordering::SeqCst) {
        return;
    }
    let leave = Arc::new(leave);
    let worker = Arc::clone(&leave);
    let (done, finished) = mpsc::channel();
    let started = spawn(Box::new(move || {
        worker();
        leaving.store(false, Ordering::SeqCst);
        let _ = done.send(());
    }));
    match (started, no_thread) {
        (Ok(()), _) => {
            let _ = finished.recv_timeout(GRACE);
        }
        (Err(_), NoThread::Inline) => {
            leave();
            leaving.store(false, Ordering::SeqCst);
        }
        (Err(_), NoThread::Skip) => leaving.store(false, Ordering::SeqCst),
    }
}

/// A full-screen [`Terminal`] on `backend`, blank and owed a full repaint.
fn full_screen<B: Backend>(backend: B) -> Result<Terminal<B>, B::Error> {
    let mut terminal = Terminal::new(backend)?;
    repaint(&mut terminal)?;
    Ok(terminal)
}

/// Clears the screen and makes the next draw write every cell. Not [`Terminal::clear`]: that
/// first reads the cursor position (`ESC[6n`) and waits for the terminal's reply, and a pty
/// driver or a slow terminal never sends one (harn exited 1 after a 2 s timeout). A full-screen
/// resize to the current size (TIOCGWINSZ) clears the same way and asks the terminal nothing.
/// Unlike `clear`, the cursor is not put back: it stays hidden and every draw places its cells.
fn repaint<B: Backend>(terminal: &mut Terminal<B>) -> Result<(), B::Error> {
    let size = terminal.size()?;
    terminal.resize(Rect::new(0, 0, size.width, size.height))
}

/// Runs the interactive view until `q` or a signal. `tabs` = one resolved, freed-annotated tree
/// per agent with its resolver. The journal is opened (created) only when writes are allowed;
/// `e` runs `$EDITOR` (default `vi`) with the terminal suspended.
pub fn run(
    target: Target,
    tabs: Vec<(Box<dyn Resolver>, Tree)>,
    opts: Opts,
) -> anyhow::Result<Exit> {
    let journal = if opts.readonly {
        Journal {
            dir: Journal::state_dir()?,
        }
    } else {
        Journal::open()?
    };
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let mut app = App::new(target, tabs, journal, opts);

    let (signal, ids) = trap_signals()?;
    install_panic_hook();
    let restore = Restore;
    let busy = app.busy();
    // After `restore`: dropped (stopped and joined) before it on every way out, so the two never
    // restore at once.
    let watchdog = Watchdog::start(
        Arc::clone(&signal),
        Arc::clone(&busy),
        GRACE,
        || leave_briefly(NoThread::Skip),
        |code| std::process::exit(code),
    )?;
    enter()?;
    let mut terminal = full_screen(CrosstermBackend::new(io::stdout()))?;
    let exit = drive(
        &mut app,
        &mut terminal,
        &mut TerminalEvents,
        &signal,
        |app, pending, terminal| {
            let Pending::Edit { item_id } = pending;
            if let Err(err) = leave() {
                // Never start an editor over a half-restored terminal.
                enter()?;
                repaint(terminal)?;
                app.report_error(format!(
                    "e: could not leave the full screen ({err}); $EDITOR not started"
                ));
                return Ok(());
            }
            let before = signal.load(Ordering::SeqCst);
            // Before the editor starts; never returns once the watchdog has claimed the exit.
            let editing = Busy::hold(&busy);
            app.edit(&item_id, &editor);
            // A Ctrl-C typed in the editor reached harn too (same process group): the editor's.
            if before == 0 {
                let _ =
                    signal.compare_exchange(SIGINT as usize, 0, Ordering::SeqCst, Ordering::SeqCst);
            }
            drop(editing);
            enter()?;
            repaint(terminal)?;
            Ok(())
        },
    );
    drop(watchdog);
    drop(restore);
    for id in ids {
        signal_hook::low_level::unregister(id);
    }
    exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Agent;
    use crate::resolve::ClaudeCode;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::{ClearType, TestBackend, WindowSize};
    use ratatui::buffer::Cell;
    use ratatui::layout::{Position, Rect, Size};
    use std::collections::{BTreeMap, VecDeque};
    use std::convert::Infallible;
    use std::path::PathBuf;
    use std::sync::{Barrier, Mutex};

    fn app() -> App {
        let dir = PathBuf::from("/nonexistent/harn-tui-test");
        let tree = Tree {
            agent: Agent::ClaudeCode,
            target_dir: dir.clone(),
            agent_version: None,
            rules_verified_against: "0.0.0".to_string(),
            tool_search: None,
            signals: Vec::new(),
            items: Vec::new(),
            predicted_known: 0,
            unknown_rows: 0,
            observed_body: None,
            warnings: Vec::new(),
            home: None,
            observed_regions: Vec::new(),
            tool_search_cause: None,
        };
        let target = Target {
            dir: dir.clone(),
            home: dir.clone(),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        };
        App::new(
            target,
            vec![(Box::new(ClaudeCode) as Box<dyn Resolver>, tree)],
            Journal {
                dir: dir.join("state"),
            },
            Opts {
                unicode: true,
                color: false,
                readonly: true,
            },
        )
    }

    /// Plays `script` (None = a tick with no event), then calls `after` on every later tick.
    struct Script<'a> {
        script: VecDeque<Option<Event>>,
        ticks: usize,
        after: &'a dyn Fn(usize),
    }

    impl Events for Script<'_> {
        fn next(&mut self, _timeout: Duration) -> io::Result<Option<Event>> {
            self.ticks += 1;
            (self.after)(self.ticks);
            Ok(self.script.pop_front().flatten())
        }
    }

    fn terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(80, 24)).unwrap()
    }

    fn no_suspend(_: &mut App, _: Pending, _: &mut Terminal<TestBackend>) -> anyhow::Result<()> {
        Ok(())
    }

    /// A terminal that never answers a query (a pty driver, a slow or odd emulator): the cursor
    /// position cannot be read, and every attempt is counted. Everything else is a `TestBackend`.
    struct Mute {
        screen: TestBackend,
        queries: usize,
    }

    impl Mute {
        fn new(width: u16, height: u16) -> Mute {
            Mute {
                screen: TestBackend::new(width, height),
                queries: 0,
            }
        }
    }

    fn ok<T>(result: Result<T, Infallible>) -> io::Result<T> {
        result.map_err(|never| match never {})
    }

    impl Backend for Mute {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            ok(self.screen.draw(content))
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            ok(self.screen.hide_cursor())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            ok(self.screen.show_cursor())
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            self.queries += 1;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the cursor position could not be read",
            ))
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            ok(self.screen.set_cursor_position(position))
        }

        fn clear(&mut self) -> io::Result<()> {
            ok(self.screen.clear())
        }

        fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
            ok(self.screen.clear_region(clear_type))
        }

        fn size(&self) -> io::Result<Size> {
            ok(self.screen.size())
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            ok(self.screen.window_size())
        }

        fn flush(&mut self) -> io::Result<()> {
            ok(self.screen.flush())
        }
    }

    fn key(code: KeyCode) -> Option<Event> {
        Some(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }

    #[test]
    fn a_terminal_that_never_answers_starts_and_takes_keys_without_a_cursor_query() {
        let mut terminal = full_screen(Mute::new(80, 24)).expect("no reply is needed to start");
        let mut events = Script {
            script: VecDeque::from([
                None,
                key(KeyCode::Char('j')),
                key(KeyCode::Char('l')),
                key(KeyCode::Char('?')),
                key(KeyCode::Esc),
                key(KeyCode::Char('q')),
            ]),
            ticks: 0,
            after: &|_| {},
        };
        let exit = drive(
            &mut app(),
            &mut terminal,
            &mut events,
            &AtomicUsize::new(0),
            |_, _, terminal| Ok(repaint(terminal)?),
        );
        assert_eq!(exit.unwrap(), Exit::Quit);
        assert_eq!(terminal.backend().queries, 0);
    }

    #[test]
    fn repaint_rewrites_the_cells_the_editor_left_on_the_screen() {
        let app = app();
        let mut terminal = full_screen(Mute::new(80, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let drawn = terminal.backend().screen.buffer().clone();
        // $EDITOR wrote over the whole alternate screen.
        let x = Cell::new("x");
        let cells: Vec<(u16, u16, &Cell)> =
            drawn.area.positions().map(|p| (p.x, p.y, &x)).collect();
        terminal.backend_mut().draw(cells.into_iter()).unwrap();
        // Same frame, no repaint: the diff is empty and the editor's cells stay.
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_ne!(terminal.backend().screen.buffer(), &drawn);
        repaint(&mut terminal).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(terminal.backend().screen.buffer(), &drawn);
        assert_eq!(terminal.backend().queries, 0);
    }

    #[test]
    fn a_resized_terminal_is_picked_up_at_the_next_draw() {
        let app = app();
        let mut terminal = full_screen(Mute::new(80, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal.backend_mut().screen.resize(100, 30);
        let area = terminal.draw(|frame| app.draw(frame)).unwrap().area;
        assert_eq!(area, Rect::new(0, 0, 100, 30));
        assert_eq!(terminal.backend().queries, 0);
    }

    #[test]
    fn the_loop_returns_when_a_signal_lands_between_events() {
        let signal = AtomicUsize::new(0);
        let set_on_third = |tick: usize| {
            if tick == 3 {
                signal.store(SIGTERM as usize, Ordering::SeqCst);
            }
        };
        let mut events = Script {
            script: VecDeque::new(),
            ticks: 0,
            after: &set_on_third,
        };
        let exit = drive(
            &mut app(),
            &mut terminal(),
            &mut events,
            &signal,
            no_suspend,
        );
        assert_eq!(exit.unwrap(), Exit::Signal(SIGTERM));
        assert_eq!(events.ticks, 3, "no tick after the flag");
    }

    #[test]
    fn a_flag_already_set_returns_before_any_draw_or_read() {
        let signal = AtomicUsize::new(SIGINT as usize);
        let mut events = Script {
            script: VecDeque::new(),
            ticks: 0,
            after: &|_| {},
        };
        let exit = drive(
            &mut app(),
            &mut terminal(),
            &mut events,
            &signal,
            no_suspend,
        );
        assert_eq!(exit.unwrap(), Exit::Signal(SIGINT));
        assert_eq!(events.ticks, 0);
    }

    #[test]
    fn q_and_ctrl_c_keys_quit_without_a_signal() {
        for key in [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let signal = AtomicUsize::new(0);
            let mut events = Script {
                script: VecDeque::from([None, Some(Event::Key(key))]),
                ticks: 0,
                after: &|_| {},
            };
            let exit = drive(
                &mut app(),
                &mut terminal(),
                &mut events,
                &signal,
                no_suspend,
            );
            assert_eq!(exit.unwrap(), Exit::Quit);
            assert_eq!(events.ticks, 2);
        }
    }

    #[test]
    fn the_watchdog_acts_once_a_signal_has_waited_out_the_grace() {
        let (t0, grace) = (Instant::now(), Duration::from_secs(1));
        let mut watch = Watch::default();
        assert_eq!(watch.look(0, false, t0, grace), None);
        assert_eq!(watch.look(SIGTERM as usize, false, t0, grace), None);
        assert_eq!(
            watch.look(SIGTERM as usize, false, t0 + grace / 2, grace),
            None
        );
        assert_eq!(
            watch.look(SIGTERM as usize, false, t0 + grace, grace),
            Some(128 + SIGTERM)
        );
    }

    #[test]
    fn a_cleared_flag_or_a_busy_harn_starts_the_wait_over() {
        let (t0, grace) = (Instant::now(), Duration::from_secs(1));
        let mut watch = Watch::default();
        assert_eq!(watch.look(SIGINT as usize, false, t0, grace), None);
        // The loop answered, or the editor's Ctrl-C was taken back.
        assert_eq!(watch.look(0, false, t0 + grace / 2, grace), None);
        assert_eq!(watch.look(SIGINT as usize, false, t0 + grace, grace), None);
        assert_eq!(
            watch.look(SIGINT as usize, false, t0 + grace * 2, grace),
            Some(128 + SIGINT)
        );
        // While a write or $EDITOR runs, however long, and for a full grace after it.
        let mut watch = Watch::default();
        assert_eq!(watch.look(SIGTERM as usize, true, t0, grace), None);
        assert_eq!(
            watch.look(SIGTERM as usize, true, t0 + grace * 10, grace),
            None
        );
        assert_eq!(
            watch.look(SIGTERM as usize, false, t0 + grace * 10, grace),
            None
        );
        assert_eq!(
            watch.look(SIGTERM as usize, false, t0 + grace * 11, grace),
            Some(128 + SIGTERM)
        );
    }

    /// A watchdog on `signal` and `busy` whose restore and exit only report what they were
    /// called with, in order.
    fn watchdog(
        signal: &Arc<AtomicUsize>,
        busy: &Arc<AtomicUsize>,
        grace: Duration,
    ) -> (Watchdog, Receiver<String>) {
        let (acted, acts) = mpsc::channel();
        let restored = acted.clone();
        let dog = Watchdog::start(
            Arc::clone(signal),
            Arc::clone(busy),
            grace,
            move || {
                let _ = restored.send("restore".to_string());
            },
            move |code| {
                let _ = acted.send(format!("exit {code}"));
            },
        )
        .unwrap();
        (dog, acts)
    }

    fn counter(n: usize) -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(n))
    }

    #[test]
    fn a_signal_the_loop_never_answers_restores_the_terminal_then_exits_with_128_plus_it() {
        let (dog, acts) = watchdog(&counter(SIGTERM as usize), &counter(0), Duration::ZERO);
        let wait = Duration::from_secs(10);
        let first = acts.recv_timeout(wait).expect("the watchdog acted");
        let second = acts.recv_timeout(wait).expect("and exited");
        drop(dog);
        assert_eq!(
            [first, second],
            ["restore".to_string(), format!("exit {}", 128 + SIGTERM)]
        );
    }

    #[test]
    fn the_end_of_run_stops_the_watchdog_before_it_acts() {
        let grace = Duration::from_secs(3600);
        let (dog, acts) = watchdog(&counter(SIGTERM as usize), &counter(0), grace);
        // Joined: nothing can come after this.
        drop(dog);
        assert_eq!(acts.try_iter().collect::<Vec<_>>(), Vec::<String>::new());
    }

    #[test]
    fn a_ctrl_c_the_editor_took_never_stops_the_process() {
        let (signal, busy) = (counter(0), counter(0));
        let (dog, acts) = watchdog(&signal, &busy, Duration::from_millis(10));
        // As `run` does around $EDITOR, with the editor's Ctrl-C landing while it runs.
        let editing = Busy::hold(&busy);
        signal.store(SIGINT as usize, Ordering::SeqCst);
        thread::sleep(TICK * 3);
        let _ = signal.compare_exchange(SIGINT as usize, 0, Ordering::SeqCst, Ordering::SeqCst);
        drop(editing);
        thread::sleep(TICK * 3);
        drop(dog);
        assert_eq!(acts.try_iter().collect::<Vec<_>>(), Vec::<String>::new());
    }

    #[test]
    fn a_sigterm_during_a_write_waits_for_it_then_a_full_grace() {
        let (signal, busy) = (counter(0), counter(0));
        let grace = TICK * 3;
        let (dog, acts) = watchdog(&signal, &busy, grace);
        // As `App` does around a plan, an undo or a journal write; nested holds count.
        let writing = Busy::hold(&busy);
        let nested = Busy::hold(&busy);
        signal.store(SIGTERM as usize, Ordering::SeqCst);
        thread::sleep(grace * 3);
        drop(nested);
        thread::sleep(grace * 2);
        assert_eq!(acts.try_recv().ok(), None, "not while the write runs");
        let done = Instant::now();
        drop(writing);
        let wait = Duration::from_secs(10);
        assert_eq!(acts.recv_timeout(wait).as_deref(), Ok("restore"));
        assert!(
            done.elapsed() >= grace,
            "{:?} after the write",
            done.elapsed()
        );
        assert_eq!(
            acts.recv_timeout(wait),
            Ok(format!("exit {}", 128 + SIGTERM))
        );
        drop(dog);
        // Every hold released, and the exit claimed: no write can start any more.
        assert_eq!(busy.load(Ordering::SeqCst), EXITING);
    }

    /// The race the claim closes: the watchdog has decided to exit on a state that showed no
    /// write (as `watch` does: load, then look) when a write reaches its admission. A barrier
    /// forces each order of the claim and the admission; every outcome is logged, nothing is
    /// asserted inside the threads, so a wrong outcome fails the test instead of hanging it.
    #[test]
    fn a_write_at_its_admission_when_the_exit_is_decided_runs_whole_or_not_at_all() {
        for exit_first in [true, false] {
            let state = counter(0);
            let log: Arc<Mutex<Vec<&str>>> = Arc::default();
            let step = Arc::new(Barrier::new(2));
            let dog = {
                let (state, log, step) = (Arc::clone(&state), Arc::clone(&log), Arc::clone(&step));
                thread::spawn(move || {
                    let say = |what| log.lock().unwrap().push(what);
                    let claim = || {
                        say(if claim_exit(&state) {
                            "exit claimed"
                        } else {
                            "exit refused"
                        })
                    };
                    let busy = state.load(Ordering::SeqCst) != 0;
                    let code = Watch::default().look(
                        SIGTERM as usize,
                        busy,
                        Instant::now(),
                        Duration::ZERO,
                    );
                    say(if code == Some(128 + SIGTERM) {
                        "exit decided"
                    } else {
                        "no decision"
                    });
                    step.wait(); // 1: decided; the write stands at its admission.
                    if exit_first {
                        claim();
                        step.wait(); // 2: the write is admitted or refused after the claim.
                    } else {
                        step.wait(); // 2: the write was admitted (or refused) first.
                        claim();
                        step.wait(); // 3: the write may finish.
                        step.wait(); // 4: it has.
                        claim();
                    }
                })
            };
            let say = |what| log.lock().unwrap().push(what);
            let admit = || {
                let held = Busy::try_hold(&state);
                say(if held.is_some() {
                    "write started"
                } else {
                    "write refused"
                });
                held
            };
            step.wait(); // 1
            if exit_first {
                step.wait(); // 2
                drop(admit());
            } else {
                let held = admit();
                step.wait(); // 2
                step.wait(); // 3
                if held.is_some() {
                    say("write finished");
                }
                drop(held);
                step.wait(); // 4
            }
            dog.join().unwrap();
            let log = log.lock().unwrap().clone();
            if exit_first {
                assert_eq!(log, ["exit decided", "exit claimed", "write refused"]);
            } else {
                assert_eq!(
                    log,
                    [
                        "exit decided",
                        "write started",
                        "exit refused",
                        "write finished",
                        "exit claimed"
                    ]
                );
            }
            assert_eq!(state.load(Ordering::SeqCst), EXITING);
        }
    }

    #[test]
    fn the_hold_count_stops_below_the_exit_bit() {
        let state = counter(MAX_HOLDS - 1);
        let last = Busy::try_hold(&state).expect("the last hold that fits");
        assert_eq!(state.load(Ordering::SeqCst), MAX_HOLDS);
        assert!(Busy::try_hold(&state).is_none(), "one more is refused");
        assert_eq!(
            state.load(Ordering::SeqCst),
            MAX_HOLDS,
            "never carried into EXITING"
        );
        assert!(!claim_exit(&state), "writes are still live");
        drop(last);
        assert_eq!(state.load(Ordering::SeqCst), MAX_HOLDS - 1);
    }

    fn no_thread(_: Box<dyn FnOnce() + Send>) -> io::Result<()> {
        Err(io::Error::other("no thread to be had"))
    }

    #[test]
    fn a_watchdog_with_no_thread_for_the_restore_goes_straight_to_its_exit() {
        static LEAVING: AtomicBool = AtomicBool::new(false);
        // A terminal whose restore would block until the test ends.
        let left: Arc<Mutex<Vec<&str>>> = Arc::default();
        let (unblock, blocked) = mpsc::channel::<()>();
        let blocked = Mutex::new(blocked);
        let leave = {
            let left = Arc::clone(&left);
            move || {
                left.lock().unwrap().push("leave");
                let _ = blocked.lock().unwrap().recv();
            }
        };
        let (acted, acts) = mpsc::channel();
        let dog = Watchdog::start(
            counter(SIGTERM as usize),
            counter(0),
            Duration::ZERO,
            move || leave_bounded(&LEAVING, NoThread::Skip, no_thread, leave),
            move |code| {
                let _ = acted.send(code);
            },
        )
        .unwrap();
        let exited = acts.recv_timeout(Duration::from_secs(10));
        drop(unblock);
        drop(dog);
        assert_eq!(exited, Ok(128 + SIGTERM));
        assert_eq!(
            *left.lock().unwrap(),
            Vec::<&str>::new(),
            "no restore tried"
        );
        assert!(!LEAVING.load(Ordering::SeqCst));
    }

    #[test]
    fn the_normal_restore_runs_on_a_thread_or_here_when_there_is_none() {
        static ON_A_THREAD: AtomicBool = AtomicBool::new(false);
        static HERE: AtomicBool = AtomicBool::new(false);
        let (left, leaves) = mpsc::channel();
        let leave = |what: &'static str| {
            let left = Mutex::new(left.clone());
            move || {
                let _ = left.lock().unwrap().send(what);
            }
        };
        leave_bounded(
            &ON_A_THREAD,
            NoThread::Inline,
            |work| thread::Builder::new().spawn(work).map(drop),
            leave("on its thread"),
        );
        let wait = Duration::from_secs(10);
        assert_eq!(leaves.recv_timeout(wait), Ok("on its thread"));
        // Inline: done by the time the call returns.
        leave_bounded(&HERE, NoThread::Inline, no_thread, leave("here"));
        assert_eq!(leaves.try_recv(), Ok("here"));
        assert!(!HERE.load(Ordering::SeqCst));
    }
}
