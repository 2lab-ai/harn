//! `harn` on a terminal: the interactive tree (fold, filter, sort, why, stage → plan → apply,
//! on, undo, edit). [`App`] is the terminal-free core (keys in, frames out); [`run`] owns raw
//! mode, the alternate screen and the event loop, and restores the terminal on every exit path,
//! a panic included.

mod app;
mod rows;
mod view;

pub use app::{App, Message, Pending, Staged, Tone, TRIM_HINT};
pub use rows::{Sort, SortKey};

use crate::journal::Journal;
use crate::model::{Target, Tree};
use crate::resolve::Resolver;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    /// Eighth-block bars and fold arrows; ASCII under `--no-unicode`.
    pub unicode: bool,
    /// Colors (yellow warnings, red errors); modifiers only without.
    pub color: bool,
    /// `--readonly`: d/a/o/u/e are greyed and refused; the journal dir is not created.
    pub readonly: bool,
}

/// The terminal is in raw mode on the alternate screen.
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn enter() -> io::Result<()> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    execute!(io::stdout(), EnterAlternateScreen, crossterm::cursor::Hide)
}

/// Back to the shell's screen; a no-op when already there, so every path may call it.
fn leave() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        leave();
    }
}

/// Once per process: a panic restores the terminal before the message prints.
fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            leave();
            previous(info);
        }));
    });
}

/// Runs the interactive view until `q`. `tabs` = one resolved, freed-annotated tree per agent
/// with its resolver. The journal is opened (created) only when writes are allowed; `e` runs
/// `$EDITOR` (default `vi`) with the terminal suspended.
pub fn run(target: Target, tabs: Vec<(Box<dyn Resolver>, Tree)>, opts: Opts) -> anyhow::Result<()> {
    let journal = if opts.readonly {
        Journal {
            dir: Journal::state_dir()?,
        }
    } else {
        Journal::open()?
    };
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let mut app = App::new(target, tabs, journal, opts);

    install_panic_hook();
    let _restore = Restore;
    enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    while !app.should_quit() {
        terminal.draw(|frame| app.draw(frame))?;
        // Press only: terminals with the kitty protocol also report releases and repeats.
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                app.handle_key(key);
            }
        }
        if let Some(Pending::Edit { item_id }) = app.take_pending() {
            leave();
            app.edit(&item_id, &editor);
            enter()?;
            terminal.clear()?;
        }
    }
    Ok(())
}
