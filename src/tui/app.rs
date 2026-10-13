//! The terminal-free core of the interactive view: state, keys and the actions behind them
//! (`d` → one card → Enter applies through the journal, `o`, `u`, `e`). `view.rs` draws it;
//! `mod.rs` owns the terminal. Tests drive it with key events and a `TestBackend`.

use super::model::{self, Line, OffCard, OffItem, Places, Row, Screen, Sort, State};
use super::Opts;
use crate::journal::{self, Journal, JournalEntry, KIND_APPLY};
use crate::model::{self as data, Item, Layer, Refused, Target, Tree};
use crate::plan::{self, ActionPlan};
use crate::resolve::Resolver;
use crate::writers;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One agent's tree with its screen, fold state and cursor.
pub(crate) struct Tab {
    pub resolver: Box<dyn Resolver>,
    pub tree: Tree,
    /// Items a live journal entry turned off here that the tree no longer has.
    pub off: Vec<OffItem>,
    /// Live journal ops on this target: item id → entry id (newest entry wins).
    pub journaled: HashMap<String, String>,
    pub screen: Screen,
    /// Keys of the open groups and rows.
    pub expanded: HashSet<String>,
    pub lines: Vec<Line>,
    pub cursor: usize,
    /// First visible line; `draw` keeps the cursor in view.
    pub offset: Cell<usize>,
}

/// The plan `d` built for one row; Enter applies it.
#[derive(Debug, Clone, PartialEq)]
pub struct Staged {
    pub tab: usize,
    pub item_id: String,
    pub name: String,
    pub plan: ActionPlan,
}

/// Work the terminal owner must do outside the alternate screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Suspend, run [`App::edit`], resume.
    Edit { item_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Error,
}

/// One piece of a message. Items and paths are named when the message is made, from their
/// own fields; a message's text is never searched or rewritten afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Shown as it is.
    Text(String),
    /// An item, shown by the name its row has.
    Item { id: String, name: String },
    /// A path, shown home- or project-relative.
    Path { path: PathBuf, shown: String },
}

/// A line over the detail panel until the next key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub parts: Vec<Part>,
    pub tone: Tone,
}

impl Message {
    /// The parts as shown, in order.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Text(text) => text.as_str(),
                Part::Item { name, .. } => name.as_str(),
                Part::Path { shown, .. } => shown.as_str(),
            })
            .collect()
    }
}

/// Text as it is.
fn t(text: impl Into<String>) -> Part {
    Part::Text(text.into())
}

/// A card over the list.
#[derive(Debug, Clone)]
pub(crate) enum Card {
    /// `d`: what turning the row off writes, removes, saves; Enter applies.
    Off {
        staged: Box<Staged>,
        card: OffCard,
        /// The list of what leaves is open (`→` on the card).
        list: bool,
    },
    /// `o`: turning a row harn turned off back on; Enter applies.
    On {
        item_id: String,
        name: String,
        /// The journal entry the card shows; Enter reverses exactly this one.
        entry_id: String,
        files: Vec<String>,
    },
    /// `w`.
    Where { title: String, lines: Vec<String> },
    /// `?`.
    Keys,
    /// `i`.
    Notes,
}

#[derive(Debug, Clone)]
pub(crate) enum Mode {
    Normal,
    /// Typing after `/`.
    Filter,
    Card(Box<Card>),
    /// `u`: the card that undoes the newest live change.
    Undo {
        entry: Box<JournalEntry>,
    },
}

pub const TRIM_HINT: &str = "AI trim: run `harn trim` in the shell (or /harn-trim in Claude Code)";

pub struct App {
    pub(crate) target: Target,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    journal: Journal,
    pub(crate) opts: Opts,
    pub(crate) sort: Sort,
    pub(crate) filter: String,
    pub(crate) mode: Mode,
    /// Scroll of a card; `draw` reports the maximum.
    pub(crate) card_scroll: usize,
    pub(crate) card_max: Cell<usize>,
    /// Lines the list showed last frame (page keys).
    pub(crate) list_height: Cell<usize>,
    /// The last frame could not draw the open card (`view::CARD_MIN`): its keys do
    /// nothing but close it.
    pub(crate) blocked: Cell<bool>,
    pub(crate) message: Option<Message>,
    /// The newest live journal entry, any target (`u` undoes it).
    pub(crate) last_live: Option<JournalEntry>,
    pending: Option<Pending>,
    quit: bool,
}

impl App {
    /// `tabs`: one resolved (and `plan::annotate_freed`-annotated) tree per agent with the
    /// resolver that produced it. `journal` is read for the off rows and written by d/o/u/e.
    pub fn new(
        target: Target,
        tabs: Vec<(Box<dyn Resolver>, Tree)>,
        journal: Journal,
        opts: Opts,
    ) -> App {
        let tabs = tabs
            .into_iter()
            .map(|(resolver, tree)| {
                let screen =
                    model::build_with(&tree, &[], &HashMap::new(), Sort::default(), opts.readonly);
                Tab {
                    resolver,
                    tree,
                    off: Vec::new(),
                    journaled: HashMap::new(),
                    screen,
                    expanded: HashSet::new(),
                    lines: Vec::new(),
                    cursor: 0,
                    offset: Cell::new(0),
                }
            })
            .collect();
        let mut app = App {
            target,
            tabs,
            active: 0,
            journal,
            opts,
            sort: Sort::default(),
            filter: String::new(),
            mode: Mode::Normal,
            card_scroll: 0,
            card_max: Cell::new(0),
            list_height: Cell::new(10),
            blocked: Cell::new(false),
            message: None,
            last_live: None,
            pending: None,
            quit: false,
        };
        let err = app.read_journal();
        app.rebuild_screens();
        if let Some(err) = err {
            app.report_error(err);
        }
        app
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Work for the terminal owner (`e`), taken once.
    pub fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    /// Puts `text` over the detail panel as an error (the terminal owner's failures), its
    /// paths home- or project-relative.
    pub fn report_error(&mut self, text: String) {
        let text = self.said(text);
        self.say(Tone::Error, vec![text]);
    }

    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }

    /// The plan the open `d` card would apply.
    pub fn staged(&self) -> Option<&Staged> {
        match &self.mode {
            Mode::Card(card) => match card.as_ref() {
                Card::Off { staged, .. } => Some(staged),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn sort(&self) -> Sort {
        self.sort
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// The active tab's tree.
    pub fn tree(&self) -> &Tree {
        &self.tab().tree
    }

    /// The active tab's view-model.
    pub fn screen(&self) -> &Screen {
        &self.tab().screen
    }

    /// `(depth, name)` of every line the list shows, in order; a group is depth 0.
    pub fn rows(&self) -> Vec<(usize, String)> {
        let tab = self.tab();
        tab.lines
            .iter()
            .map(|line| {
                let name = match tab.screen.line_row(line) {
                    Some(row) => row.name.clone(),
                    None => tab.screen.groups[line.group].name.clone(),
                };
                (line.depth, name)
            })
            .collect()
    }

    /// Id of the item the cursor row acts on (None on a group or a row that gathers others).
    pub fn cursor_id(&self) -> Option<String> {
        self.cursor_row().and_then(|row| row.id.clone())
    }

    /// Index of the cursor line in [`App::rows`].
    pub fn cursor(&self) -> usize {
        self.tab().cursor
    }

    /// A card (the `u` one too) is open over the list (keys go to it).
    pub fn has_card(&self) -> bool {
        matches!(self.mode, Mode::Card(_) | Mode::Undo { .. })
    }

    /// Name of the cursor line (a group's or a row's).
    pub fn cursor_name(&self) -> Option<String> {
        let tab = self.tab();
        let line = tab.lines.get(tab.cursor)?;
        Some(match tab.screen.line_row(line) {
            Some(row) => row.name.clone(),
            None => tab.screen.groups[line.group].name.clone(),
        })
    }

    /// Moves the cursor to the first row acting on item `id`, opening what is above it.
    pub fn select(&mut self, id: &str) -> bool {
        let tab = &mut self.tabs[self.active];
        let Some((group, path)) = tab.screen.find(id) else {
            return false;
        };
        for depth in 0..path.len() {
            let key = tab.screen.key(group, &path[..depth]);
            tab.expanded.insert(key);
        }
        let key = tab.screen.key(group, &path);
        self.rebuild(self.active, Some(key));
        self.cursor_id().as_deref() == Some(id)
    }

    /// Moves the cursor to the group whose name starts with `name` (`Plugins`).
    pub fn select_group(&mut self, name: &str) -> bool {
        let tab = &mut self.tabs[self.active];
        let Some(at) = tab
            .lines
            .iter()
            .position(|l| l.path.is_empty() && tab.screen.groups[l.group].name.starts_with(name))
        else {
            return false;
        };
        tab.cursor = at;
        true
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Normal => self.normal_key(key),
            Mode::Filter => self.filter_key(key),
            Mode::Card(_) => self.card_key(key),
            Mode::Undo { .. } => self.undo_key(key),
        }
    }

    fn normal_key(&mut self, key: KeyEvent) {
        self.message = None;
        let page = self.list_height.get().max(1) as isize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.step(1),
            KeyCode::Char('k') | KeyCode::Up => self.step(-1),
            KeyCode::PageDown => self.step(page),
            KeyCode::PageUp => self.step(-page),
            KeyCode::Char('g') | KeyCode::Home => self.tabs[self.active].cursor = 0,
            KeyCode::Char('G') | KeyCode::End => {
                let tab = &mut self.tabs[self.active];
                tab.cursor = tab.lines.len().saturating_sub(1);
            }
            KeyCode::Char('l') | KeyCode::Right => self.expand(),
            KeyCode::Char('h') | KeyCode::Left => self.collapse(),
            KeyCode::Enter => self.toggle(),
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Char('s') => {
                self.sort = self.sort.toggled();
                self.rebuild_screens();
                self.info(format!("sorted by {}", self.sort.word()));
            }
            KeyCode::Char('w') => self.where_from(),
            KeyCode::Char(' ') | KeyCode::Char('d') => self.ask_off(),
            KeyCode::Char('o') => self.ask_on(),
            KeyCode::Char('u') => self.ask_undo(),
            KeyCode::Char('e') => self.ask_edit(),
            KeyCode::Char('r') => match self.refresh() {
                Some(err) => self.report_error(err),
                None => self.info("refreshed: every setting and file read again".to_string()),
            },
            KeyCode::Char('t') => self.info(TRIM_HINT.to_string()),
            KeyCode::Char('?') => self.open(Card::Keys),
            KeyCode::Char('i') => {
                if self.tab().screen.notes.is_empty() {
                    self.info("no notes for this directory".to_string());
                } else {
                    self.open(Card::Notes);
                }
            }
            KeyCode::Tab => self.switch_tab(1),
            KeyCode::BackTab => self.switch_tab(-1),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc => {
                if self.filter.is_empty() {
                    self.quit = true;
                } else {
                    self.clear_filter();
                }
            }
            _ => {}
        }
    }

    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.clear_filter();
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                if self.filter.trim().is_empty() {
                    self.clear_filter();
                }
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.apply_filter();
            }
            KeyCode::Down => self.step(1),
            KeyCode::Up => self.step(-1),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push(c);
                self.apply_filter();
            }
            _ => {}
        }
    }

    fn card_key(&mut self, key: KeyEvent) {
        // Not drawn (the terminal is too small): nothing applies blind; only closing works.
        if self.blocked.get() {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n')
            ) {
                let off = matches!(&self.mode, Mode::Card(c) if matches!(c.as_ref(), Card::Off { .. } | Card::On { .. }));
                self.mode = Mode::Normal;
                if off {
                    self.info("cancelled: nothing written");
                }
            }
            return;
        }
        let max = self.card_max.get();
        let page = self.list_height.get().max(1);
        // Scrolling is the same on every card.
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.card_scroll = (self.card_scroll + 1).min(max);
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.card_scroll = self.card_scroll.saturating_sub(1);
                return;
            }
            KeyCode::PageDown => {
                self.card_scroll = (self.card_scroll + page).min(max);
                return;
            }
            KeyCode::PageUp => {
                self.card_scroll = self.card_scroll.saturating_sub(page);
                return;
            }
            _ => {}
        }
        let Mode::Card(card) = &mut self.mode else {
            return;
        };
        let close = matches!(key.code, KeyCode::Esc | KeyCode::Char('q'));
        match card.as_mut() {
            Card::Off {
                list: list @ true, ..
            } => {
                if close
                    || matches!(
                        key.code,
                        KeyCode::Left | KeyCode::Char('h') | KeyCode::Enter
                    )
                {
                    *list = false;
                    self.card_scroll = 0;
                }
            }
            Card::Off {
                card: off, list, ..
            } => match key.code {
                KeyCode::Enter => self.apply(),
                KeyCode::Right | KeyCode::Char('l') if !off.leave_list.is_empty() => {
                    *list = true;
                    self.card_scroll = 0;
                }
                _ if close || key.code == KeyCode::Char('n') => {
                    self.mode = Mode::Normal;
                    self.info("cancelled: nothing written".to_string());
                }
                _ => {}
            },
            Card::On {
                item_id,
                name,
                entry_id,
                ..
            } => match key.code {
                KeyCode::Enter => {
                    let (item_id, name, entry_id) =
                        (item_id.clone(), name.clone(), entry_id.clone());
                    self.mode = Mode::Normal;
                    self.turn_on(&item_id, &name, &entry_id);
                }
                _ if close || key.code == KeyCode::Char('n') => {
                    self.mode = Mode::Normal;
                    self.info("cancelled: nothing written".to_string());
                }
                _ => {}
            },
            Card::Where { .. } => {
                if close || matches!(key.code, KeyCode::Char('w') | KeyCode::Enter) {
                    self.mode = Mode::Normal;
                }
            }
            Card::Keys => {
                if close || matches!(key.code, KeyCode::Char('?') | KeyCode::Enter) {
                    self.mode = Mode::Normal;
                }
            }
            Card::Notes => {
                if close || matches!(key.code, KeyCode::Char('i') | KeyCode::Enter) {
                    self.mode = Mode::Normal;
                }
            }
        }
    }

    /// The `u` card: Enter (or `y`) undoes, Esc (or `n`, `q`) keeps; other keys do nothing, as
    /// on every card.
    fn undo_key(&mut self, key: KeyEvent) {
        let undo = matches!(
            key.code,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y')
        );
        let keep = matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') | KeyCode::Char('N')
        );
        // Not drawn: Enter or `y` would undo what the reader cannot see.
        if (self.blocked.get() && undo) || !(undo || keep) {
            return;
        }
        let Mode::Undo { entry } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        if undo {
            self.undo(&entry);
        } else {
            self.info("cancelled: nothing written".to_string());
        }
    }

    fn open(&mut self, card: Card) {
        self.card_scroll = 0;
        // Whether it fits is the next frame's to say.
        self.blocked.set(false);
        self.mode = Mode::Card(Box::new(card));
    }

    fn say(&mut self, tone: Tone, parts: Vec<Part>) {
        self.message = Some(Message { parts, tone });
    }

    fn info(&mut self, text: impl Into<String>) {
        self.say(Tone::Info, vec![t(text)]);
    }

    fn error(&mut self, text: impl Into<String>) {
        self.say(Tone::Error, vec![t(text)]);
    }

    /// Text a library worded (an error, a warning) as a part, its paths home- or
    /// project-relative. Only paths are shortened; nothing else in it is touched.
    fn said(&self, text: impl Into<String>) -> Part {
        let text = text.into();
        match self.tabs.get(self.active) {
            Some(tab) => Part::Text(Places::of(&tab.tree).text(&text)),
            None => Part::Text(text),
        }
    }

    /// Item `id` as a message part: the name of its row, else the name its id gives.
    fn item(&self, id: &str) -> Part {
        let tab = self.tab();
        let name = row_name(&tab.screen, id)
            .unwrap_or_else(|| model::id_name(id, &Places::of(&tab.tree)).0);
        Part::Item {
            id: id.to_string(),
            name,
        }
    }

    /// `path` as a message part, home- or project-relative.
    fn path(&self, path: &Path) -> Part {
        Part::Path {
            path: path.to_path_buf(),
            shown: Places::of(&self.tab().tree).path(path),
        }
    }

    /// `paths` as parts joined by `, `.
    fn paths<'a>(&self, paths: impl IntoIterator<Item = &'a Path>) -> Vec<Part> {
        let mut parts = Vec::new();
        for (i, path) in paths.into_iter().enumerate() {
            if i > 0 {
                parts.push(t(", "));
            }
            parts.push(self.path(path));
        }
        parts
    }

    /// A failed library call as parts: a refusal (`refused: …`, the item it names as its row
    /// when it is `about`), a stale journal (`stale: …` with its files), else `error: …` as the
    /// library worded it.
    fn failure(&self, err: &anyhow::Error, about: Option<&str>) -> Vec<Part> {
        for cause in err.chain() {
            if let Some(Refused(reason)) = cause.downcast_ref::<Refused>() {
                // plan.rs words a refusal about an item `<id>: <reason>`.
                let rest = about.and_then(|id| reason.strip_prefix(id)?.strip_prefix(": "));
                return match (about, rest) {
                    (Some(id), Some(rest)) => {
                        vec![
                            t("refused: "),
                            self.item(id),
                            self.said(format!(": {rest}")),
                        ]
                    }
                    _ => vec![self.said(format!("refused: {reason}"))],
                };
            }
            if let Some(stale) = cause.downcast_ref::<data::Stale>() {
                let mut parts = vec![t(format!(
                    "stale: {} file(s) changed since harn wrote them, nothing written: ",
                    stale.files.len()
                ))];
                parts.extend(self.paths(stale.files.iter().map(PathBuf::as_path)));
                return parts;
            }
        }
        vec![self.said(format!("error: {err:#}"))]
    }

    /// False (and the message says why) under `--readonly`.
    fn writable(&mut self, key: &str) -> bool {
        if self.opts.readonly {
            self.error(format!(
                "read-only: {key} would change files; harn was started with --readonly"
            ));
        }
        !self.opts.readonly
    }

    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub(crate) fn cursor_line(&self) -> Option<&Line> {
        let tab = self.tab();
        tab.lines.get(tab.cursor)
    }

    pub(crate) fn cursor_row(&self) -> Option<&Row> {
        let tab = self.tab();
        tab.screen.line_row(tab.lines.get(tab.cursor)?)
    }

    /// The tree item the cursor row acts on.
    pub(crate) fn cursor_item(&self) -> Option<&Item> {
        let id = self.cursor_row()?.id.as_deref()?;
        data::walk(&self.tab().tree).find(|i| i.id == id)
    }

    // ---- lines ------------------------------------------------------------------------------

    /// Rebuilds every tab's screen from its tree and journal state; cursors keep their lines.
    fn rebuild_screens(&mut self) {
        let (sort, read_only) = (self.sort, self.opts.readonly);
        let keys: Vec<Option<String>> = (0..self.tabs.len()).map(|i| self.key_of(i)).collect();
        for tab in &mut self.tabs {
            tab.screen = model::build_with(&tab.tree, &tab.off, &tab.journaled, sort, read_only);
        }
        for (i, keep) in keys.into_iter().enumerate() {
            self.rebuild(i, keep);
        }
    }

    fn key_of(&self, tab: usize) -> Option<String> {
        let t = &self.tabs[tab];
        t.lines.get(t.cursor).map(|l| l.key.clone())
    }

    /// Recomputes the lines of `tab`; the cursor goes to the line with key `keep` when shown,
    /// else stays at its index (clamped).
    fn rebuild(&mut self, tab: usize, keep: Option<String>) {
        let filter = (!self.filter.trim().is_empty()).then_some(self.filter.as_str());
        let t = &mut self.tabs[tab];
        t.lines = t.screen.lines(&t.expanded, filter);
        let found = keep.and_then(|key| t.lines.iter().position(|l| l.key == key));
        t.cursor = found.unwrap_or(t.cursor.min(t.lines.len().saturating_sub(1)));
    }

    fn step(&mut self, delta: isize) {
        let tab = &mut self.tabs[self.active];
        if tab.lines.is_empty() {
            return;
        }
        let last = tab.lines.len() as isize - 1;
        tab.cursor = (tab.cursor as isize + delta).clamp(0, last) as usize;
    }

    fn has_children(&self, line: &Line) -> bool {
        let screen = &self.tab().screen;
        match screen.line_row(line) {
            Some(row) => !row.children.is_empty(),
            None => !screen.groups[line.group].rows.is_empty(),
        }
    }

    fn expand(&mut self) {
        let Some(line) = self.cursor_line().cloned() else {
            return;
        };
        if !self.has_children(&line) {
            return;
        }
        let tab = &mut self.tabs[self.active];
        if tab.expanded.insert(line.key.clone()) {
            self.rebuild(self.active, Some(line.key));
        } else {
            self.step(1);
        }
    }

    fn collapse(&mut self) {
        let Some(line) = self.cursor_line().cloned() else {
            return;
        };
        let open = self.has_children(&line) && self.tab().expanded.contains(&line.key);
        let tab = &mut self.tabs[self.active];
        if open {
            tab.expanded.remove(&line.key);
            self.rebuild(self.active, Some(line.key));
            return;
        }
        // A leaf or a folded row: go to the parent.
        if line.depth == 0 {
            return;
        }
        if let Some(parent) = tab.lines[..tab.cursor]
            .iter()
            .rposition(|l| l.depth == line.depth - 1)
        {
            tab.cursor = parent;
        }
    }

    fn toggle(&mut self) {
        let Some(line) = self.cursor_line().cloned() else {
            return;
        };
        if self.tab().expanded.contains(&line.key) {
            self.collapse();
        } else {
            self.expand();
        }
    }

    fn switch_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n > 1 {
            self.active = (self.active as isize + delta).rem_euclid(n) as usize;
        }
    }

    // ---- filter -----------------------------------------------------------------------------

    fn apply_filter(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        for i in 0..self.tabs.len() {
            let keep = self.key_of(i);
            self.rebuild(i, keep);
            if needle.is_empty() {
                continue;
            }
            // The cursor goes to the first matching row when its line is no match.
            let tab = &mut self.tabs[i];
            let matches = |line: &Line| {
                tab.screen
                    .line_row(line)
                    .is_some_and(|r| r.name.to_lowercase().contains(&needle))
            };
            let on_match = tab.lines.get(tab.cursor).is_some_and(matches);
            if !on_match {
                if let Some(first) = tab.lines.iter().position(matches) {
                    tab.cursor = first;
                }
            }
        }
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        for i in 0..self.tabs.len() {
            let keep = self.key_of(i);
            // What the filter showed stays in place: the lines above the cursor open.
            if let Some(key) = &keep {
                let parts: Vec<&str> = key.split('\n').collect();
                for n in 1..parts.len() {
                    self.tabs[i].expanded.insert(parts[..n].join("\n"));
                }
            }
            self.rebuild(i, keep);
        }
    }

    // ---- actions ----------------------------------------------------------------------------

    fn where_from(&mut self) {
        let Some(row) = self.cursor_row().cloned() else {
            self.info(
                "w shows where one thing comes from: open a group (→) and pick a row".to_string(),
            );
            return;
        };
        // harn's own change: what it did and how to reverse it. An item off by the user's own
        // settings (every hook under their disableAllHooks) gets its real origin below.
        let by_harn = row
            .id
            .as_ref()
            .is_some_and(|id| self.tab().journaled.contains_key(id));
        if row.state == State::Off && (by_harn || self.cursor_item().is_none()) {
            let lines = vec![
                "1  harn turned it off in this directory".to_string(),
                "2  `harn history` lists the change; o turns it back on".to_string(),
            ];
            self.open(Card::Where {
                title: format!("Where {} comes from", row.name),
                lines,
            });
            return;
        }
        let Some(item) = self.cursor_item() else {
            self.info(format!(
                "w: {} gathers several; open it (→) and pick one",
                row.name
            ));
            return;
        };
        let (title, lines) = model::where_card(item, &row.name, &Places::of(&self.tab().tree));
        self.open(Card::Where { title, lines });
    }

    /// `d`: the plan for the cursor row in one card.
    fn ask_off(&mut self) {
        if !self.writable("d") {
            return;
        }
        let Some(line) = self.cursor_line().cloned() else {
            return;
        };
        let Some(row) = self.tab().screen.line_row(&line).cloned() else {
            let group = self.tab().screen.groups[line.group].name.clone();
            self.info(format!(
                "d turns off one thing: open {group} (→) and pick one"
            ));
            return;
        };
        if row.state == State::Off {
            let mut parts = row
                .id
                .as_deref()
                .map(|id| vec![self.item(id)])
                .unwrap_or_default();
            let by_harn = row
                .id
                .as_ref()
                .is_some_and(|id| self.tab().journaled.contains_key(id));
            parts.push(t(if by_harn {
                " is already off here; o turns it back on"
            } else {
                " is already off: your settings turn every hook off (disableAllHooks)"
            }));
            self.say(Tone::Info, parts);
            return;
        }
        let Some(item) = self.cursor_item().cloned() else {
            self.info(format!(
                "d: {} gathers several; open it (→) and pick one",
                row.name
            ));
            return;
        };
        if !item
            .knobs
            .iter()
            .any(|k| plan::knob_scope(k) == Some(data::Scope::Local))
        {
            let parts = vec![
                t("refused: "),
                self.item(&item.id),
                t(format!(" {}", model::refusal(&item))),
            ];
            self.say(Tone::Error, parts);
            return;
        }
        let tab = self.tab();
        match plan::build_plan(
            tab.resolver.as_ref(),
            &self.target,
            &tab.tree,
            &item.id,
            None,
        ) {
            Ok(plan) => {
                let arrow = crate::render::common::glyphs(self.opts.unicode).arrow;
                let card = model::off_card(
                    &plan,
                    &tab.tree,
                    &item,
                    &row.name,
                    arrow,
                    row.saving.as_deref(),
                );
                let staged = Staged {
                    tab: self.active,
                    item_id: item.id.clone(),
                    name: row.name.clone(),
                    plan,
                };
                self.open(Card::Off {
                    staged: Box::new(staged),
                    card,
                    list: false,
                });
            }
            Err(err) => {
                let parts = self.failure(&err, Some(&item.id));
                self.say(Tone::Error, parts);
            }
        }
    }

    /// Enter on the `d` card.
    fn apply(&mut self) {
        let Mode::Card(card) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        let Card::Off { staged, .. } = *card else {
            return;
        };
        // Named as the row was when the card opened; the hooks switch by what it does.
        let item = if staged.plan.ops.iter().any(|op| is_all_hooks(&op.knob)) {
            t("all hooks")
        } else {
            Part::Item {
                id: staged.item_id.clone(),
                name: staged.name.clone(),
            }
        };
        let before = self.tabs[staged.tab].screen.title.items;
        match writers::apply(&staged.plan, &mut self.journal) {
            Ok(_) => {
                let refresh = self.refresh();
                self.select(&staged.item_id);
                match refresh {
                    Some(err) => self.say(
                        Tone::Error,
                        vec![
                            t("turned off "),
                            item,
                            self.said(format!(" · refresh failed: {err}")),
                        ],
                    ),
                    None => {
                        let after = self.tabs[staged.tab].screen.title.items;
                        self.say(
                            Tone::Info,
                            vec![
                                t("turned off "),
                                item,
                                t(format!(
                                    " · {} · u undoes · takes effect next session",
                                    model::start_count(before, after)
                                )),
                            ],
                        )
                    }
                }
            }
            Err(err) => {
                let parts = self.failure(&err, Some(&staged.item_id));
                self.say(Tone::Error, parts);
            }
        }
    }

    fn ask_on(&mut self) {
        if !self.writable("o") {
            return;
        }
        let Some(row) = self.cursor_row().cloned() else {
            return;
        };
        let Some(id) = row.id.clone() else {
            return;
        };
        let Some(entry_id) = self.tab().journaled.get(&id).cloned() else {
            let parts = vec![
                t("o: harn did not turn "),
                self.item(&id),
                t(" off here (`harn history` lists its changes)"),
            ];
            self.say(Tone::Error, parts);
            return;
        };
        let entries = match journal::history(&self.journal) {
            Ok(entries) => entries,
            Err(err) => {
                let text = self.said(format!("cannot read the journal: {err:#}"));
                self.say(Tone::Error, vec![text]);
                return;
            }
        };
        let Some(entry) = entries.into_iter().find(|e| e.id == entry_id) else {
            let parts = vec![t("no journal entry turns "), self.item(&id), t(" back on")];
            self.say(Tone::Error, parts);
            return;
        };
        if entry.snapshots.is_empty() {
            // Named as its row is: the entry's id is the journal's, not the reader's.
            let parts = vec![self.item(&id), t(": that change has no file to restore")];
            self.say(Tone::Error, parts);
            return;
        }
        let name = if entry.ops.iter().any(|op| is_all_hooks(&op.knob)) {
            "all hooks".to_string()
        } else {
            row.name
        };
        let places = Places::of(&self.tab().tree);
        let files = entry
            .snapshots
            .iter()
            .map(|s| places.path(&s.path))
            .collect();
        self.open(Card::On {
            item_id: id,
            name,
            entry_id,
            files,
        });
    }

    /// Enter on the `o` card: reverses `entry_id`, the entry the card showed, and only while it
    /// is still live (another harn, or `u`, may have undone it since).
    fn turn_on(&mut self, item_id: &str, name: &str, entry_id: &str) {
        let item = Part::Item {
            id: item_id.to_string(),
            name: name.to_string(),
        };
        let live = match journal::history(&self.journal) {
            Ok(entries) => entries
                .iter()
                .any(|e| e.id == entry_id && e.undone_at.is_none()),
            Err(err) => {
                let text = self.said(format!("cannot read the journal: {err:#}"));
                self.say(Tone::Error, vec![text]);
                return;
            }
        };
        if !live {
            self.error("that change was already undone; r refreshes");
            return;
        }
        let before = self.tab().screen.title.items;
        match journal::undo_entry(&mut self.journal, entry_id) {
            Ok(entry) => {
                let mut parts = if entry.ops.iter().any(|op| is_all_hooks(&op.knob)) {
                    vec![t("all hooks are back on")]
                } else {
                    vec![item, t(" is back on")]
                };
                // How the header's count moved, as apply says it.
                match self.refresh() {
                    Some(err) => parts.push(self.said(format!(" · refresh failed: {err}"))),
                    None => {
                        let after = self.tab().screen.title.items;
                        parts.push(t(format!(
                            " · {} · takes effect next session",
                            model::start_count(before, after)
                        )));
                    }
                }
                self.select(item_id);
                self.say(Tone::Info, parts);
            }
            Err(err) => {
                let parts = self.failure(&err, Some(item_id));
                self.say(Tone::Error, parts);
            }
        }
    }

    fn ask_undo(&mut self) {
        if !self.writable("u") {
            return;
        }
        match self.last_live.clone() {
            Some(entry) => {
                self.blocked.set(false);
                self.card_scroll = 0;
                self.mode = Mode::Undo {
                    entry: Box::new(entry),
                }
            }
            None => self.error("nothing to undo: no change by harn is still live".to_string()),
        }
    }

    /// The names of what `entry` changed.
    fn op_names(&self, entry: &JournalEntry) -> Vec<String> {
        let tab = self.tab();
        let places = Places::of(&tab.tree);
        entry
            .ops
            .iter()
            .map(|op| {
                row_name(&tab.screen, &op.item_id)
                    .unwrap_or_else(|| model::id_name(&op.item_id, &places).0)
            })
            .collect()
    }

    /// The `u` card's lines for `entry`: what comes back, the files restored, where it was made
    /// when not here.
    pub(crate) fn undo_lines(&self, entry: &JournalEntry) -> Vec<String> {
        let places = Places::of(&self.tab().tree);
        let mut lines = Vec::new();
        if entry.ops.iter().any(|op| is_all_hooks(&op.knob)) {
            lines.push("all hooks come back on".to_string());
        } else {
            for name in self.op_names(entry) {
                lines.push(if entry.context.capture_kind == KIND_APPLY {
                    format!("{name} comes back on")
                } else {
                    format!("the edit of {name} is reversed")
                });
            }
        }
        let files: Vec<String> = entry
            .snapshots
            .iter()
            .map(|s| places.path(&s.path))
            .collect();
        if !files.is_empty() {
            lines.push(format!("restores {} byte for byte", files.join(", ")));
        }
        if entry.context.cwd != self.target.dir {
            lines.push(format!(
                "made in {} (not this directory)",
                places.path(&entry.context.cwd)
            ));
        }
        lines
    }

    /// Undoes `expected`, the entry the user confirmed, only while it is still the newest live
    /// entry: another harn may have journaled in between, and `u` must not reach that one.
    fn undo(&mut self, expected: &JournalEntry) {
        let newest = match journal::history(&self.journal) {
            Ok(entries) => entries.into_iter().find(|e| e.undone_at.is_none()),
            Err(err) => {
                let parts = self.failure(&err, None);
                self.say(Tone::Error, parts);
                return;
            }
        };
        if newest.as_ref().map(|e| e.id.as_str()) != Some(expected.id.as_str()) {
            let refresh = self.refresh();
            let mut parts = vec![t(
                "the journal changed since you confirmed; press u again to see the newest change",
            )];
            if let Some(err) = refresh {
                parts.push(self.said(format!(" · refresh failed: {err}")));
            }
            self.say(Tone::Error, parts);
            return;
        }
        // Named as their rows are now, before the refresh brings them back; the hooks switch
        // brings every hook back, not only the one `d` was pressed on.
        let mut parts = vec![t("undone: ")];
        if expected.ops.iter().any(|op| is_all_hooks(&op.knob)) {
            parts.push(t("all hooks back on"));
        } else {
            for (i, op) in expected.ops.iter().enumerate() {
                if i > 0 {
                    parts.push(t(", "));
                }
                parts.push(self.item(&op.item_id));
            }
            parts.push(t(" back"));
        }
        let before = self.tab().screen.title.items;
        match journal::undo_entry(&mut self.journal, &expected.id) {
            Ok(entry) => {
                let refresh = self.refresh();
                // How the header's count moved, as apply says it; the files last, the first
                // clause to drop when the row is narrow.
                if refresh.is_none() {
                    let after = self.tab().screen.title.items;
                    parts.push(t(format!(" · {}", model::start_count(before, after))));
                }
                parts.push(t(" · "));
                parts.extend(self.paths(entry.snapshots.iter().map(|s| s.path.as_path())));
                parts.push(t(" restored byte for byte"));
                if let Some(err) = refresh {
                    parts.push(self.said(format!(" · refresh failed: {err}")));
                }
                self.say(Tone::Info, parts);
            }
            Err(err) => {
                let parts = self.failure(&err, None);
                self.say(Tone::Error, parts);
            }
        }
    }

    fn ask_edit(&mut self) {
        if !self.writable("e") {
            return;
        }
        let Some(item) = self.cursor_item() else {
            self.error("e: the cursor is not on a file".to_string());
            return;
        };
        let (id, layer, editable) = (item.id.clone(), item.origin.layer, editable(item));
        if matches!(layer, Layer::Builtin | Layer::Managed) {
            let parts = vec![
                t("refused: "),
                self.item(&id),
                t(" belongs to Claude Code or your organisation; e opens only files you own"),
            ];
            self.say(Tone::Error, parts);
            return;
        }
        if !editable {
            let parts = vec![t("refused: "), self.item(&id), t(" has no file to edit")];
            self.say(Tone::Error, parts);
            return;
        }
        self.pending = Some(Pending::Edit { item_id: id });
    }

    /// `e` after the terminal is suspended: snapshot → `editor <file>` → diff → journal entry,
    /// then re-resolve. An edit that changed nothing journals nothing.
    pub fn edit(&mut self, item_id: &str, editor: &str) {
        if !self.writable("e") {
            return;
        }
        let tab = self.tab();
        let Some(item) = data::walk(&tab.tree).find(|i| i.id == item_id).cloned() else {
            self.error("e: that file is no longer in the tree".to_string());
            return;
        };
        let version = tab.tree.agent_version.clone();
        let named = self.item(item_id);
        let result = journal::edit(&item, &self.target, &mut self.journal, editor, version);
        let refresh = self.refresh();
        self.select(item_id);
        match result {
            Ok(entry) => {
                let mut parts = vec![t("edited "), named, t(" · u undoes")];
                // The journal's own words (an editor's exit status, a file that no longer
                // parses), as it wrote them.
                for warning in &entry.warnings {
                    parts.push(self.said(format!(" · {warning}")));
                }
                if let Some(err) = refresh {
                    parts.push(self.said(format!(" · refresh failed: {err}")));
                }
                self.say(Tone::Info, parts);
            }
            // journal::edit has no typed "unchanged" outcome (journal.rs is outside this unit),
            // so its message is the only signal.
            Err(err) if format!("{err}").contains("nothing journaled") => {
                self.say(Tone::Info, vec![named, t(": unchanged, nothing journaled")])
            }
            Err(err) => {
                let parts = self.failure(&err, Some(item_id));
                self.say(Tone::Error, parts);
            }
        }
    }

    /// Re-resolves (and re-annotates) every agent, re-reads the journal, keeps each cursor on
    /// its line when it still exists. Returns the first error.
    pub fn refresh(&mut self) -> Option<String> {
        let mut first_error = None;
        for tab in &mut self.tabs {
            let resolved = tab
                .resolver
                .resolve(&self.target, None)
                .and_then(|mut tree| {
                    plan::annotate_freed(tab.resolver.as_ref(), &self.target, &mut tree)?;
                    Ok(tree)
                });
            match resolved {
                Ok(tree) => tab.tree = tree,
                Err(err) => {
                    first_error.get_or_insert(format!("{}: {err:#}", tab.tree.agent));
                }
            }
        }
        if let Some(err) = self.read_journal() {
            first_error.get_or_insert(err);
        }
        self.rebuild_screens();
        first_error
    }

    /// Off rows and live ops of each tab, and the newest live entry.
    fn read_journal(&mut self) -> Option<String> {
        let entries = match journal::history(&self.journal) {
            Ok(entries) => entries,
            Err(err) => return Some(format!("journal: {err:#}")),
        };
        let live: Vec<&JournalEntry> = entries.iter().filter(|e| e.undone_at.is_none()).collect();
        self.last_live = live.first().map(|e| (*e).clone());
        for tab in &mut self.tabs {
            tab.journaled.clear();
            tab.off.clear();
            for entry in &live {
                let ctx = &entry.context;
                if ctx.capture_kind != KIND_APPLY
                    || ctx.cwd != self.target.dir
                    || ctx.agent != tab.tree.agent
                {
                    continue;
                }
                for op in &entry.ops {
                    // The hooks switch turns every hook off (and the status line): each one is
                    // this entry's to reverse, whichever hook `d` was pressed on.
                    if is_all_hooks(&op.knob) {
                        for item in data::walk(&tab.tree).filter(|i| {
                            i.signal.as_deref() == Some(crate::resolve::claude::OFF_ALL_HOOKS)
                        }) {
                            tab.journaled
                                .entry(item.id.clone())
                                .or_insert_with(|| entry.id.clone());
                        }
                    }
                    if tab.journaled.contains_key(&op.item_id) {
                        continue;
                    }
                    tab.journaled.insert(op.item_id.clone(), entry.id.clone());
                    if !data::walk(&tab.tree).any(|i| i.id == op.item_id) {
                        tab.off.push(OffItem {
                            item_id: op.item_id.clone(),
                            entry_id: entry.id.clone(),
                            pointer: match &op.knob {
                                data::Knob::Setting { pointer, .. } => Some(pointer.clone()),
                                _ => None,
                            },
                        });
                    }
                }
            }
        }
        None
    }
}

/// The switch that turns every hook off at once.
fn is_all_hooks(knob: &data::Knob) -> bool {
    matches!(knob, data::Knob::Setting { pointer, .. } if pointer == model::ALL_HOOKS)
}

/// The name of the first row acting on `id`.
fn row_name(screen: &Screen, id: &str) -> Option<String> {
    let (group, path) = screen.find(id)?;
    screen.row(group, &path).map(|r| r.name.clone())
}

/// A file the user owns: `e` opens it (a plugin is a directory, not a file).
pub(crate) fn editable(item: &Item) -> bool {
    item.origin.path.is_some()
        && item.kind != data::Kind::Plugin
        && !matches!(item.origin.layer, Layer::Builtin | Layer::Managed)
}
