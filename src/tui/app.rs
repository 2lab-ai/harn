//! The terminal-free core of the interactive view: state, keys and the actions behind them
//! (stage → plan, apply, on, undo, edit through the journal). `view.rs` draws it; `mod.rs` owns
//! the terminal. Tests drive it with key events and a `TestBackend`.

use super::rows::{self, Filter, OffItem, Row, RowKind, Sort};
use super::Opts;
use crate::journal::{self, Journal, JournalEntry, KIND_APPLY};
use crate::model::{self, Item, Layer, Refused, Target, Tree};
use crate::plan::{self, ActionPlan};
use crate::render;
use crate::render::common::has_action;
use crate::resolve::Resolver;
use crate::writers;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

/// One agent's tree with its own fold state and cursor.
pub(crate) struct Tab {
    pub resolver: Box<dyn Resolver>,
    pub tree: Tree,
    pub expanded: HashSet<String>,
    /// The fold state before a filter expanded the ancestors of its matches.
    pub saved: Option<HashSet<String>>,
    /// Items a live journal entry turned off here that the tree no longer has.
    pub off: Vec<OffItem>,
    /// Live journal ops on this target: item id → entry id (newest entry wins).
    pub journaled: HashMap<String, String>,
    pub rows: Vec<Row>,
    pub cursor: usize,
    /// First visible row; `draw` keeps the cursor in view.
    pub offset: Cell<usize>,
}

/// The plan `d` built for one row; `a` applies it.
#[derive(Debug, Clone, PartialEq)]
pub struct Staged {
    pub tab: usize,
    pub item_id: String,
    pub label: String,
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

/// The message line under the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone)]
pub(crate) enum Confirm {
    Apply,
    On { item_id: String, label: String },
    Undo { entry: Box<JournalEntry> },
}

#[derive(Debug, Clone)]
pub(crate) enum Mode {
    Normal,
    /// Typing after `/`.
    Filter,
    Why {
        text: String,
    },
    Plan,
    Help,
    Confirm {
        what: Confirm,
        back: Box<Mode>,
    },
}

/// A cursor position that survives a rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowKey {
    Item(String),
    Off(String),
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
    /// Scroll of the why / plan / help pane; `draw` reports the maximum.
    pub(crate) pane_scroll: usize,
    pub(crate) pane_max: Cell<usize>,
    /// Rows the list showed last frame (page keys).
    pub(crate) list_height: Cell<usize>,
    pub(crate) staged: Option<Staged>,
    pub(crate) message: Option<Message>,
    /// The newest live journal entry, any target (`u` undoes it).
    pub(crate) last_live: Option<JournalEntry>,
    pending: Option<Pending>,
    quit: bool,
}

impl App {
    /// `tabs`: one resolved (and `plan::annotate_freed`-annotated) tree per agent with the
    /// resolver that produced it. `journal` is read for the off rows and written by a/o/u/e.
    pub fn new(
        target: Target,
        tabs: Vec<(Box<dyn Resolver>, Tree)>,
        journal: Journal,
        opts: Opts,
    ) -> App {
        let tabs = tabs
            .into_iter()
            .map(|(resolver, tree)| Tab {
                resolver,
                tree,
                expanded: HashSet::new(),
                saved: None,
                off: Vec::new(),
                journaled: HashMap::new(),
                rows: Vec::new(),
                cursor: 0,
                offset: Cell::new(0),
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
            pane_scroll: 0,
            pane_max: Cell::new(0),
            list_height: Cell::new(10),
            staged: None,
            message: None,
            last_live: None,
            pending: None,
            quit: false,
        };
        if let Some(err) = app.read_journal() {
            app.error(err);
        }
        app.rebuild_all();
        app
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Work for the terminal owner (`e`), taken once.
    pub fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }

    pub fn message(&self) -> Option<&Message> {
        self.message.as_ref()
    }

    pub fn staged(&self) -> Option<&Staged> {
        self.staged.as_ref()
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

    /// `(depth, label)` of every row the list shows, in order (separators skipped; an off row
    /// is its item id).
    pub fn rows(&self) -> Vec<(usize, String)> {
        let tab = self.tab();
        tab.rows
            .iter()
            .filter_map(|row| match &row.kind {
                RowKind::Item(path) => {
                    rows::item_at(&tab.tree.items, path).map(|i| (row.depth, i.label.clone()))
                }
                RowKind::Off(i) => Some((row.depth, tab.off[*i].item_id.clone())),
                RowKind::Separator(_) => None,
            })
            .collect()
    }

    /// Id of the item (or off item) under the cursor.
    pub fn cursor_id(&self) -> Option<String> {
        match self.cursor_key()? {
            RowKey::Item(id) | RowKey::Off(id) => Some(id),
        }
    }

    /// Moves the cursor to `id` (an item, expanding its ancestors, or an off row).
    pub fn select(&mut self, id: &str) -> bool {
        let tab = &mut self.tabs[self.active];
        if let Some(path) = rows::path_of(&tab.tree.items, id) {
            let mut items = &tab.tree.items;
            for i in &path[..path.len() - 1] {
                tab.expanded.insert(items[*i].id.clone());
                items = &items[*i].children;
            }
        }
        self.rebuild(self.active, Some(RowKey::Item(id.to_string())));
        if self.cursor_id().as_deref() == Some(id) {
            return true;
        }
        self.rebuild(self.active, Some(RowKey::Off(id.to_string())));
        self.cursor_id().as_deref() == Some(id)
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Normal => self.normal_key(key),
            Mode::Filter => self.filter_key(key),
            Mode::Why { .. } | Mode::Plan | Mode::Help => self.pane_key(key),
            Mode::Confirm { .. } => self.confirm_key(key),
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
            KeyCode::Char('g') | KeyCode::Home => self.jump(0, 1),
            KeyCode::Char('G') | KeyCode::End => {
                let last = self.tab().rows.len().saturating_sub(1);
                self.jump(last, -1)
            }
            KeyCode::Char('l') | KeyCode::Right => self.expand(),
            KeyCode::Char('h') | KeyCode::Left => self.collapse(),
            KeyCode::Enter => self.toggle(),
            KeyCode::Char('/') => self.start_filter(),
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.rebuild_all();
            }
            KeyCode::Char('w') => self.why(),
            KeyCode::Char(' ') | KeyCode::Char('d') => self.stage(),
            KeyCode::Char('a') => self.ask_apply(),
            KeyCode::Char('o') => self.ask_on(),
            KeyCode::Char('u') => self.ask_undo(),
            KeyCode::Char('e') => self.ask_edit(),
            KeyCode::Char('r') => {
                let err = self.refresh();
                match err {
                    Some(err) => self.error(err),
                    None => self.info("refreshed: re-resolved every agent".to_string()),
                }
            }
            KeyCode::Char('t') => self.info(TRIM_HINT.to_string()),
            KeyCode::Char('?') => self.open(Mode::Help),
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
                self.clear_filter();
                self.mode = Mode::Normal;
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

    fn pane_key(&mut self, key: KeyEvent) {
        let page = self.list_height.get().max(1);
        let max = self.pane_max.get();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.pane_scroll = (self.pane_scroll + 1).min(max)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.pane_scroll = self.pane_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => self.pane_scroll = (self.pane_scroll + page).min(max),
            KeyCode::PageUp => self.pane_scroll = self.pane_scroll.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => self.pane_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.pane_scroll = max,
            KeyCode::Char('a') if matches!(self.mode, Mode::Plan) => self.ask_apply(),
            KeyCode::Char('d') | KeyCode::Char(' ') if matches!(self.mode, Mode::Plan) => {
                if let Some(staged) = self.staged.take() {
                    self.info(format!("unstaged {}", staged.label));
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Char('?') if matches!(self.mode, Mode::Help) => self.mode = Mode::Normal,
            KeyCode::Char('?') => self.open(Mode::Help),
            KeyCode::Char('w') if matches!(self.mode, Mode::Why { .. }) => self.mode = Mode::Normal,
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            _ => {}
        }
    }

    fn confirm_key(&mut self, key: KeyEvent) {
        let Mode::Confirm { what, back } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        if !matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
            self.mode = *back;
            self.info("cancelled: nothing written".to_string());
            return;
        }
        match what {
            Confirm::Apply => self.apply(),
            Confirm::On { item_id, label } => self.turn_on(&item_id, &label),
            Confirm::Undo { entry } => self.undo(&entry),
        }
    }

    fn open(&mut self, mode: Mode) {
        self.pane_scroll = 0;
        self.mode = mode;
    }

    fn confirm(&mut self, what: Confirm) {
        let back = std::mem::replace(&mut self.mode, Mode::Normal);
        self.mode = Mode::Confirm {
            what,
            back: Box::new(back),
        };
    }

    fn info(&mut self, text: String) {
        self.message = Some(Message {
            text,
            tone: Tone::Info,
        });
    }

    fn error(&mut self, text: String) {
        self.message = Some(Message {
            text,
            tone: Tone::Error,
        });
    }

    /// False (and the message says why) under `--readonly`.
    fn writable(&mut self, key: &str) -> bool {
        if self.opts.readonly {
            self.error(format!(
                "readonly: `{key}` changes files; harn was started with --readonly"
            ));
        }
        !self.opts.readonly
    }

    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub(crate) fn cursor_item(&self) -> Option<&Item> {
        let tab = self.tab();
        match &tab.rows.get(tab.cursor)?.kind {
            RowKind::Item(path) => rows::item_at(&tab.tree.items, path),
            _ => None,
        }
    }

    /// The off item under the cursor.
    pub(crate) fn cursor_off(&self) -> Option<&OffItem> {
        let tab = self.tab();
        match &tab.rows.get(tab.cursor)?.kind {
            RowKind::Off(i) => tab.off.get(*i),
            _ => None,
        }
    }

    fn cursor_key(&self) -> Option<RowKey> {
        if let Some(item) = self.cursor_item() {
            return Some(RowKey::Item(item.id.clone()));
        }
        self.cursor_off()
            .map(|off| RowKey::Off(off.item_id.clone()))
    }

    // ---- rows -------------------------------------------------------------------------------

    fn rebuild_all(&mut self) {
        for i in 0..self.tabs.len() {
            let keep = self.key_of(i);
            self.rebuild(i, keep);
        }
    }

    fn key_of(&self, tab: usize) -> Option<RowKey> {
        let t = &self.tabs[tab];
        match &t.rows.get(t.cursor)?.kind {
            RowKind::Item(path) => {
                rows::item_at(&t.tree.items, path).map(|i| RowKey::Item(i.id.clone()))
            }
            RowKind::Off(i) => t.off.get(*i).map(|o| RowKey::Off(o.item_id.clone())),
            RowKind::Separator(_) => None,
        }
    }

    /// Recomputes the rows of `tab`; the cursor goes to `keep` when shown, else stays at its
    /// index (clamped, off separators).
    fn rebuild(&mut self, tab: usize, keep: Option<RowKey>) {
        let filter = Filter::new(&self.filter);
        let sort = self.sort;
        let t = &mut self.tabs[tab];
        t.rows = rows::flatten(&t.tree, &t.expanded, sort, filter.as_ref(), &t.off);
        let found = keep.and_then(|key| {
            t.rows.iter().position(|row| match (&row.kind, &key) {
                (RowKind::Item(path), RowKey::Item(id)) => {
                    rows::item_at(&t.tree.items, path).is_some_and(|i| i.id == *id)
                }
                (RowKind::Off(i), RowKey::Off(id)) => t.off[*i].item_id == *id,
                _ => false,
            })
        });
        let len = t.rows.len();
        t.cursor = found.unwrap_or(t.cursor.min(len.saturating_sub(1)));
        t.cursor = land(&t.rows, t.cursor, 1);
    }

    fn step(&mut self, delta: isize) {
        let tab = &mut self.tabs[self.active];
        if tab.rows.is_empty() {
            return;
        }
        let last = tab.rows.len() as isize - 1;
        let to = (tab.cursor as isize + delta).clamp(0, last) as usize;
        tab.cursor = land(&tab.rows, to, delta.signum());
    }

    fn jump(&mut self, to: usize, dir: isize) {
        let tab = &mut self.tabs[self.active];
        tab.cursor = land(&tab.rows, to, dir);
    }

    fn expand(&mut self) {
        let Some(item) = self.cursor_item() else {
            return;
        };
        if item.children.is_empty() {
            return;
        }
        let id = item.id.clone();
        let tab = &mut self.tabs[self.active];
        if tab.expanded.insert(id.clone()) {
            self.rebuild(self.active, Some(RowKey::Item(id)));
        } else {
            self.step(1);
        }
    }

    fn collapse(&mut self) {
        let Some(item) = self.cursor_item() else {
            return;
        };
        let id = item.id.clone();
        let has_children = !item.children.is_empty();
        let tab = &mut self.tabs[self.active];
        if has_children && tab.expanded.remove(&id) {
            self.rebuild(self.active, Some(RowKey::Item(id)));
            return;
        }
        // A leaf or a folded row: go to the parent.
        let depth = tab.rows[tab.cursor].depth;
        if depth == 0 {
            return;
        }
        if let Some(parent) = tab.rows[..tab.cursor]
            .iter()
            .rposition(|row| row.depth == depth - 1)
        {
            tab.cursor = parent;
        }
    }

    fn toggle(&mut self) {
        let Some(item) = self.cursor_item() else {
            return;
        };
        if self.tab().expanded.contains(&item.id) {
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

    fn start_filter(&mut self) {
        for tab in &mut self.tabs {
            if tab.saved.is_none() {
                tab.saved = Some(tab.expanded.clone());
            }
        }
        self.mode = Mode::Filter;
    }

    /// Fold state = the state before the filter + every ancestor of a match.
    fn apply_filter(&mut self) {
        let filter = Filter::new(&self.filter);
        for i in 0..self.tabs.len() {
            let keep = self.key_of(i);
            let tab = &mut self.tabs[i];
            let mut expanded = tab.saved.clone().unwrap_or_else(|| tab.expanded.clone());
            if let Some(f) = &filter {
                expanded.extend(rows::ancestors_of_matches(&tab.tree, f));
            }
            tab.expanded = expanded;
            self.rebuild(i, keep);
            // The cursor goes to the first match when its row was filtered out.
            let tab = &mut self.tabs[i];
            if let Some(f) = &filter {
                let on_match = matches!(&tab.rows.get(tab.cursor).map(|r| &r.kind),
                    Some(RowKind::Item(path)) if rows::item_at(&tab.tree.items, path).is_some_and(|it| f.matches(it)));
                if !on_match {
                    if let Some(first) = tab.rows.iter().position(|row| match &row.kind {
                        RowKind::Item(path) => {
                            rows::item_at(&tab.tree.items, path).is_some_and(|it| f.matches(it))
                        }
                        _ => false,
                    }) {
                        tab.cursor = first;
                    }
                }
            }
        }
    }

    fn clear_filter(&mut self) {
        self.filter.clear();
        for i in 0..self.tabs.len() {
            let keep = self.key_of(i);
            let tab = &mut self.tabs[i];
            if let Some(saved) = tab.saved.take() {
                tab.expanded = saved;
            }
            self.rebuild(i, keep);
        }
    }

    // ---- actions ----------------------------------------------------------------------------

    fn why(&mut self) {
        if let Some(off) = self.cursor_off() {
            let text = format!(
                "{}\n  turned off here by journal {}\n  o turns it back on · `harn history` lists the entry",
                off.item_id, off.entry_id
            );
            self.open(Mode::Why { text });
            return;
        }
        let Some(item) = self.cursor_item() else {
            return;
        };
        let tab = self.tab();
        let (plans, note) =
            match plan::plans_by_scope(tab.resolver.as_ref(), &self.target, &tab.tree, &item.id) {
                Ok(plans) => (plans, None),
                Err(err) => (Vec::new(), Some(describe(&err))),
            };
        let mut text = render::why(&tab.tree, item, &plans);
        if let Some(note) = note {
            text.push_str(&format!("\n  plans: {note}"));
        }
        self.open(Mode::Why { text });
    }

    /// `d`: plan the cursor row off (default scope) and show the plan; `d` on the staged row
    /// unstages it.
    fn stage(&mut self) {
        if !self.writable("d") {
            return;
        }
        let Some(item) = self.cursor_item() else {
            self.error("d: the cursor is not on an item".to_string());
            return;
        };
        let (id, label) = (item.id.clone(), item.label.clone());
        if self
            .staged
            .as_ref()
            .is_some_and(|s| s.item_id == id && s.tab == self.active)
        {
            self.staged = None;
            self.info(format!("unstaged {label}"));
            return;
        }
        let tab = self.tab();
        match plan::build_plan(tab.resolver.as_ref(), &self.target, &tab.tree, &id, None) {
            Ok(plan) => {
                let replaced = self
                    .staged
                    .replace(Staged {
                        tab: self.active,
                        item_id: id,
                        label: label.clone(),
                        plan,
                    })
                    .map(|old| format!(" (replaces {})", old.label))
                    .unwrap_or_default();
                self.open(Mode::Plan);
                self.info(format!(
                    "staged {label} off{replaced}: a applies, d unstages"
                ));
            }
            Err(err) => self.error(describe(&err)),
        }
    }

    fn ask_apply(&mut self) {
        if !self.writable("a") {
            return;
        }
        if self.staged.is_none() {
            self.error("nothing staged: d stages the cursor row".to_string());
            return;
        }
        self.confirm(Confirm::Apply);
    }

    fn apply(&mut self) {
        let Some(staged) = self.staged.take() else {
            return;
        };
        match writers::apply(&staged.plan, &mut self.journal) {
            Ok(entry) => {
                let mut text = format!(
                    "applied: journal {} · u undoes · takes effect on the next session",
                    entry.id
                );
                if let Some(err) = self.refresh() {
                    text.push_str(&format!(" · refresh failed: {err}"));
                }
                self.info(text);
            }
            Err(err) => {
                self.error(describe(&err));
                self.staged = Some(staged);
            }
        }
    }

    fn ask_on(&mut self) {
        if !self.writable("o") {
            return;
        }
        if let Some(off) = self.cursor_off() {
            let (item_id, label) = (off.item_id.clone(), short_id(&off.item_id));
            self.confirm(Confirm::On { item_id, label });
            return;
        }
        let Some(item) = self.cursor_item() else {
            return;
        };
        let (item_id, label) = (item.id.clone(), item.label.clone());
        if self.tab().journaled.contains_key(&item_id) {
            self.confirm(Confirm::On { item_id, label });
        } else {
            self.error(format!(
                "o: no journal op turned {label} off here (harn history lists the entries)"
            ));
        }
    }

    fn turn_on(&mut self, item_id: &str, label: &str) {
        match journal::on(&mut self.journal, &self.target, item_id) {
            Ok(entry) => {
                let mut text = format!("on: {label} · journal {} reversed", entry.id);
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
            Some(entry) => self.confirm(Confirm::Undo {
                entry: Box::new(entry),
            }),
            None => self.error("nothing to undo: no live journal entry".to_string()),
        }
    }

    fn undo(&mut self, expected: &JournalEntry) {
        match journal::undo(&mut self.journal, 1) {
            Ok(entries) => {
                let restored: Vec<String> = entries
                    .iter()
                    .flat_map(|e| e.snapshots.iter())
                    .map(|s| s.path.display().to_string())
                    .collect();
                let id = entries
                    .first()
                    .map_or(expected.id.as_str(), |e| e.id.as_str());
                let mut text = format!("undone: journal {id} · restored {}", restored.join(", "));
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
            self.error("e: the cursor is not on an item".to_string());
            return;
        };
        if matches!(item.origin.layer, Layer::Builtin | Layer::Managed) {
            let text = format!(
                "refused: {} is a {} item; e opens only files you own",
                item.label, item.origin.layer
            );
            self.error(text);
            return;
        }
        if item.origin.path.is_none() {
            let text = format!("refused: {} has no file to edit", item.label);
            self.error(text);
            return;
        }
        self.pending = Some(Pending::Edit {
            item_id: item.id.clone(),
        });
    }

    /// `e` after the terminal is suspended: snapshot → `editor <file>` → diff → journal entry,
    /// then re-resolve. An edit that changed nothing journals nothing.
    pub fn edit(&mut self, item_id: &str, editor: &str) {
        let tab = self.tab();
        let Some(item) = model::walk(&tab.tree).find(|i| i.id == item_id).cloned() else {
            self.error(format!("e: {item_id} is no longer in the tree"));
            return;
        };
        let version = tab.tree.agent_version.clone();
        let result = journal::edit(&item, &self.target, &mut self.journal, editor, version);
        let refresh = self.refresh();
        self.select(item_id);
        match result {
            Ok(entry) => {
                let mut text = format!("edited {}: journal {} · u undoes", item.label, entry.id);
                for warning in &entry.warnings {
                    text.push_str(&format!(" · {warning}"));
                }
                if let Some(err) = refresh {
                    text.push_str(&format!(" · refresh failed: {err}"));
                }
                self.info(text);
            }
            Err(err) if format!("{err}").contains("nothing journaled") => {
                self.info(format!("{err}"))
            }
            Err(err) => self.error(describe(&err)),
        }
    }

    /// Re-resolves (and re-annotates) every agent, re-reads the journal, keeps each cursor on
    /// its item when it still exists. Returns the first error.
    pub fn refresh(&mut self) -> Option<String> {
        // Taken from the rows as drawn: after the re-resolve an index path names another item.
        let keys: Vec<Option<RowKey>> = (0..self.tabs.len()).map(|i| self.key_of(i)).collect();
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
        for (i, keep) in keys.into_iter().enumerate() {
            self.rebuild(i, keep);
        }
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
                    if !model::walk(&tab.tree).any(|i| i.id == op.item_id) {
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

/// The cursor index nearest `to` in direction `dir` that is not a separator.
fn land(rows: &[Row], to: usize, dir: isize) -> usize {
    let selectable = |i: usize| !matches!(rows[i].kind, RowKind::Separator(_));
    if rows.is_empty() {
        return 0;
    }
    let to = to.min(rows.len() - 1);
    let forward = (to..rows.len()).find(|i| selectable(*i));
    let backward = (0..=to).rev().find(|i| selectable(*i));
    let first = if dir < 0 { backward } else { forward };
    first.or(forward).or(backward).unwrap_or(0)
}

/// A refusal reads `refused: …`, a stale journal `stale: …`; anything else `error: …`.
fn describe(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(refused) = cause.downcast_ref::<Refused>() {
            return refused.to_string();
        }
        if let Some(stale) = cause.downcast_ref::<model::Stale>() {
            return stale.to_string().replace('\n', " ");
        }
    }
    format!("error: {err:#}")
}

/// `<name-or-realpath>`: the id without `<agent>/<kind>/<layer>/`.
pub(crate) fn short_id(id: &str) -> String {
    id.splitn(4, '/').nth(3).unwrap_or(id).to_string()
}

/// Whether `d` can act on `item` (an applicable default knob).
pub(crate) fn can_stage(item: &Item) -> bool {
    has_action(item)
}
