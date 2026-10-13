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

/// A line over the detail panel until the next key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub tone: Tone,
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
    /// `u`: y/N over the list.
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
                let screen = model::build(&tree, &[], &HashMap::new(), Sort::default());
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
            message: None,
            last_live: None,
            pending: None,
            quit: false,
        };
        let err = app.read_journal();
        app.rebuild_screens();
        if let Some(err) = err {
            app.error(err);
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

    /// Puts `text` over the detail panel as an error (the terminal owner's failures).
    pub fn report_error(&mut self, text: String) {
        self.error(text);
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
                Some(err) => self.error(err),
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
            Card::On { item_id, name, .. } => match key.code {
                KeyCode::Enter => {
                    let (item_id, name) = (item_id.clone(), name.clone());
                    self.mode = Mode::Normal;
                    self.turn_on(&item_id, &name);
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

    fn undo_key(&mut self, key: KeyEvent) {
        let Mode::Undo { entry } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.undo(&entry);
        } else {
            self.info("cancelled: nothing written".to_string());
        }
    }

    fn open(&mut self, card: Card) {
        self.card_scroll = 0;
        self.mode = Mode::Card(Box::new(card));
    }

    fn info(&mut self, text: String) {
        self.message = Some(Message {
            text: self.human(&text),
            tone: Tone::Info,
        });
    }

    fn error(&mut self, text: String) {
        self.message = Some(Message {
            text: self.human(&text),
            tone: Tone::Error,
        });
    }

    /// Paths home- or project-relative, item ids as the names the list shows them by.
    fn human(&self, text: &str) -> String {
        let Some(tab) = self.tabs.get(self.active) else {
            return text.to_string();
        };
        let places = Places::of(&tab.tree);
        let mut out = text.to_string();
        for id in item_ids(text) {
            let name = row_name(&tab.screen, &id).unwrap_or_else(|| model::id_name(&id, &places).0);
            out = out.replace(&id, &name);
        }
        places.text(&out)
    }

    /// False (and the message says why) under `--readonly`.
    fn writable(&mut self, key: &str) -> bool {
        if self.opts.readonly {
            self.error(format!(
                "readonly: {key} would change files; harn was started with --readonly"
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
        let sort = self.sort;
        let keys: Vec<Option<String>> = (0..self.tabs.len()).map(|i| self.key_of(i)).collect();
        for tab in &mut self.tabs {
            tab.screen = model::build(&tab.tree, &tab.off, &tab.journaled, sort);
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
        if row.state == State::Off {
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
            self.info(format!(
                "{} is already off here; o turns it back on",
                row.name
            ));
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
            let why = row.detail.action.trim_end_matches('.');
            let why = match why.strip_prefix("It cannot be turned off: ") {
                Some(reason) => format!("{} cannot be turned off: {reason}", row.name),
                None => format!("{}: {why}", row.name),
            };
            self.error(format!("refused: {why}"));
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
            Err(err) => self.error(describe(&err)),
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
        match writers::apply(&staged.plan, &mut self.journal) {
            Ok(_) => {
                let refresh = self.refresh();
                self.select(&staged.item_id);
                let off = self.cursor_row().is_some_and(|r| r.state == State::Off);
                match refresh {
                    Some(err) => self.error(format!(
                        "turned off {} · refresh failed: {err}",
                        staged.name
                    )),
                    None if !off => self.info(format!(
                        "turned off {} · u undoes · takes effect next session",
                        staged.name
                    )),
                    None => {}
                }
            }
            Err(err) => self.error(describe(&err)),
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
            self.error(format!(
                "o: harn did not turn {} off here (`harn history` lists its changes)",
                row.name
            ));
            return;
        };
        let places = Places::of(&self.tab().tree);
        let files = journal::history(&self.journal)
            .ok()
            .and_then(|entries| entries.into_iter().find(|e| e.id == entry_id))
            .map(|e| e.snapshots.iter().map(|s| places.path(&s.path)).collect())
            .unwrap_or_default();
        self.open(Card::On {
            item_id: id,
            name: row.name,
            files,
        });
    }

    fn turn_on(&mut self, item_id: &str, name: &str) {
        match journal::on(&mut self.journal, &self.target, item_id) {
            Ok(_) => {
                let mut text = format!("{name} is back on · takes effect next session");
                if let Some(err) = self.refresh() {
                    text.push_str(&format!(" · refresh failed: {err}"));
                }
                self.select(item_id);
                self.info(text);
            }
            Err(err) => self.error(describe(&err)),
        }
    }

    fn ask_undo(&mut self) {
        if !self.writable("u") {
            return;
        }
        match self.last_live.clone() {
            Some(entry) => {
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

    /// The `u` confirm's lines for `entry`.
    pub(crate) fn undo_lines(&self, entry: &JournalEntry) -> Vec<String> {
        let places = Places::of(&self.tab().tree);
        let mut lines = vec!["Undo the last change?".to_string()];
        for name in self.op_names(entry) {
            lines.push(if entry.context.capture_kind == KIND_APPLY {
                format!("  {name} comes back on")
            } else {
                format!("  the edit of {name} is reversed")
            });
        }
        let files: Vec<String> = entry
            .snapshots
            .iter()
            .map(|s| places.path(&s.path))
            .collect();
        if !files.is_empty() {
            lines.push(format!("  restores {} byte for byte", files.join(", ")));
        }
        if entry.context.cwd != self.target.dir {
            lines.push(format!(
                "  made in {} (not this directory)",
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
                self.error(describe(&err));
                return;
            }
        };
        if newest.as_ref().map(|e| e.id.as_str()) != Some(expected.id.as_str()) {
            let refresh = self.refresh();
            let mut text =
                "the journal changed since you confirmed; press u again to see the newest change"
                    .to_string();
            if let Some(err) = refresh {
                text.push_str(&format!(" · refresh failed: {err}"));
            }
            self.error(text);
            return;
        }
        let names = self.op_names(expected);
        match journal::undo_entry(&mut self.journal, &expected.id) {
            Ok(entry) => {
                let places = Places::of(&self.tab().tree);
                let restored: Vec<String> = entry
                    .snapshots
                    .iter()
                    .map(|s| places.path(&s.path))
                    .collect();
                let mut text = format!(
                    "undone: {} back · {} restored byte for byte",
                    names.join(", "),
                    restored.join(", ")
                );
                if let Some(err) = self.refresh() {
                    text.push_str(&format!(" · refresh failed: {err}"));
                }
                self.info(text);
            }
            Err(err) => self.error(describe(&err)),
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
        let name = self
            .cursor_row()
            .map(|r| r.name.clone())
            .unwrap_or_default();
        if matches!(item.origin.layer, Layer::Builtin | Layer::Managed) {
            self.error(format!(
                "refused: {name} belongs to Claude Code or your organisation; e opens only \
                 files you own"
            ));
            return;
        }
        if !editable(item) {
            self.error(format!("refused: {name} has no file to edit"));
            return;
        }
        self.pending = Some(Pending::Edit {
            item_id: item.id.clone(),
        });
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
        let name = row_name(&tab.screen, item_id).unwrap_or_else(|| item.label.clone());
        let version = tab.tree.agent_version.clone();
        let result = journal::edit(&item, &self.target, &mut self.journal, editor, version);
        let refresh = self.refresh();
        self.select(item_id);
        match result {
            Ok(entry) => {
                let mut text = format!("edited {name} · u undoes");
                for warning in &entry.warnings {
                    text.push_str(&format!(" · {warning}"));
                }
                if let Some(err) = refresh {
                    text.push_str(&format!(" · refresh failed: {err}"));
                }
                self.info(text);
            }
            Err(err) if format!("{err}").contains("nothing journaled") => {
                self.info(format!("{name}: unchanged, nothing journaled"))
            }
            Err(err) => self.error(describe(&err)),
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
                    if tab.journaled.contains_key(&op.item_id) {
                        continue;
                    }
                    tab.journaled.insert(op.item_id.clone(), entry.id.clone());
                    if !data::walk(&tab.tree).any(|i| i.id == op.item_id) {
                        tab.off.push(OffItem {
                            item_id: op.item_id.clone(),
                            entry_id: entry.id.clone(),
                        });
                    }
                }
            }
        }
        None
    }
}

/// The name of the first row acting on `id`.
fn row_name(screen: &Screen, id: &str) -> Option<String> {
    let (group, path) = screen.find(id)?;
    screen.row(group, &path).map(|r| r.name.clone())
}

/// Item ids (`<agent>/<kind>/<layer>/<name>`) inside `text`: each whitespace-separated word that
/// starts with an agent's and a kind's name.
fn item_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for word in text.split_whitespace() {
        let word = word
            .trim_end_matches([',', ':', ';', ')'])
            .trim_start_matches('(');
        let mut parts = word.splitn(4, '/');
        let (agent, kind, rest) = (parts.next(), parts.next(), parts.nth(1));
        if data::Agent::ALL.iter().any(|a| Some(a.as_str()) == agent)
            && data::Kind::ALL.iter().any(|k| Some(k.as_str()) == kind)
            && rest.is_some()
        {
            ids.push(word.to_string());
        }
    }
    ids
}

/// A refusal reads `refused: …`, a stale journal `stale: …`; anything else `error: …`.
fn describe(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(refused) = cause.downcast_ref::<Refused>() {
            return refused.to_string();
        }
        if let Some(stale) = cause.downcast_ref::<data::Stale>() {
            return stale.to_string().replace('\n', " ");
        }
    }
    format!("error: {err:#}")
}

/// A file the user owns: `e` opens it (a plugin is a directory, not a file).
pub(crate) fn editable(item: &Item) -> bool {
    item.origin.path.is_some()
        && item.kind != data::Kind::Plugin
        && !matches!(item.origin.layer, Layer::Builtin | Layer::Managed)
}
