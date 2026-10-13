//! `harn` on a terminal: what loads before the first message, grouped by kind (`model`), with
//! one card per action (`d` turn off, `o` turn back on, `u` undo, `w` where from, `e` edit).
//! [`App`] is the terminal-free core (keys in, frames out); [`run`] owns raw
//! mode, the alternate screen and the event loop, and restores the terminal on every exit path:
//! `q`, an error, a panic, and SIGINT / SIGTERM / SIGHUP (a flag the loop polls every
//! [`TICK`]).

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
use std::sync::{Arc, Once};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    /// Eighth-block bars, arrows and box borders; ASCII under `--no-unicode`.
    pub unicode: bool,
    /// Colors (yellow warnings, red errors); modifiers only without.
    pub color: bool,
    /// `--readonly`: d/o/u/e are greyed and refused; the journal dir is not created.
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

/// Best effort on the way out of [`run`], whatever path it takes.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = leave();
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
            app.edit(&item_id, &editor);
            // A Ctrl-C typed in the editor reached harn too (same process group): the editor's.
            if before == 0 {
                let _ =
                    signal.compare_exchange(SIGINT as usize, 0, Ordering::SeqCst, Ordering::SeqCst);
            }
            enter()?;
            repaint(terminal)?;
            Ok(())
        },
    );
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
}
