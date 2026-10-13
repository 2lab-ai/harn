//! The view-model of the interactive view (`.prd/04-tui-design.md` §View-model): the resolved
//! tree regrouped by kind — each plugin one row with its skills, commands, agents, hooks and MCP
//! servers folded under it, the two listings by owner, memory, project files, hooks and servers
//! outside plugins, then the built-in parts — with people's names (no raw ids, home-relative
//! paths, scopes in words) and the detail sentences built once. Pure: no terminal, no files.
//! `view.rs` draws a [`Screen`]; `app.rs` acts on the item ids its rows carry.

use crate::model::{self, Agent, Item, Kind, Knob, Layer, Mode, Scope, Tree};
use crate::plan::{knob_scope, ActionPlan};
use crate::render::common::{denominator, request_bytes};
use crate::resolve::claude::CAP_STATE_UNKNOWN;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// An item a live journal entry turned off here and the resolved tree no longer has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffItem {
    pub item_id: String,
    pub entry_id: String,
    /// The JSON pointer the journal op wrote (`/disableAllHooks`); None for other switches.
    pub pointer: Option<String>,
}

/// The one switch Claude Code has for hooks: every hook at once.
pub const ALL_HOOKS: &str = "/disableAllHooks";

/// Row order inside a group; `s` toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Bytes descending, unknown last, name ascending as the tiebreak.
    #[default]
    Size,
    /// Name ascending.
    Name,
}

impl Sort {
    pub fn toggled(self) -> Sort {
        match self {
            Sort::Size => Sort::Name,
            Sort::Name => Sort::Size,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Sort::Size => "size",
            Sort::Name => "name",
        }
    }
}

/// Where paths are shown from: inside the target dir they are project-relative, under `$HOME`
/// they start with `~`.
#[derive(Debug, Clone)]
pub struct Places {
    pub dir: PathBuf,
    pub home: Option<PathBuf>,
}

impl Places {
    pub fn of(tree: &Tree) -> Places {
        Places {
            dir: tree.target_dir.clone(),
            home: tree.home.clone(),
        }
    }

    /// `.claude/settings.local.json`, `~/.claude/CLAUDE.md`; an outside path as it is.
    pub fn path(&self, path: &Path) -> String {
        if let Ok(rel) = path.strip_prefix(&self.dir) {
            if rel.as_os_str().is_empty() {
                return self.dir_words();
            }
            return rel.display().to_string();
        }
        if let Some(home) = &self.home {
            if let Ok(rel) = path.strip_prefix(home) {
                if rel.as_os_str().is_empty() {
                    return "~".to_string();
                }
                return format!("~/{}", rel.display());
            }
        }
        path.display().to_string()
    }

    /// `~/2lab.ai/zbrain` when the target is under home, else `this directory`.
    fn dir_words(&self) -> String {
        match &self.home {
            Some(home) => match self.dir.strip_prefix(home) {
                Ok(rel) if !rel.as_os_str().is_empty() => format!("~/{}", rel.display()),
                _ => "this directory".to_string(),
            },
            None => "this directory".to_string(),
        }
    }

    /// Every absolute path in `text` shown the same way: under the target dir project-relative,
    /// the target dir itself in words, under home with `~`.
    pub fn text(&self, text: &str) -> String {
        let dir = self.dir.display().to_string();
        let mut out = replace_path(text, &format!("{dir}/"), "");
        out = replace_path(&out, &dir, &self.dir_words());
        if let Some(home) = &self.home {
            out = replace_path(&out, &home.display().to_string(), "~");
        }
        out
    }
}

/// `text` with every `path` that ends at a path boundary replaced by `with`. A root (`/`) or an
/// empty path is never replaced.
fn replace_path(text: &str, path: &str, with: &str) -> String {
    if path.is_empty() || path == "/" {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(path) {
        let after = &rest[at + path.len()..];
        let boundary = path.ends_with('/')
            || after
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.')));
        out.push_str(&rest[..at]);
        out.push_str(if boundary { with } else { path });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `3.7 KB` under 10 KB, `32 KB` from there, `1.2 MB` from 999.5 KB (1 KB = 1000 B).
pub fn size(bytes: u64) -> String {
    let tenths = bytes.saturating_add(50) / 100;
    if tenths < 100 {
        return format!("{}.{} KB", tenths / 10, tenths % 10);
    }
    let kb = bytes.saturating_add(500) / 1000;
    if kb < 1000 {
        return format!("{kb} KB");
    }
    let tenths = bytes.saturating_add(50_000) / 100_000;
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

/// `32%` of `total`, rounded down: shares of parts that partition the total never add up to
/// more than 100. None without bytes or total.
pub fn percent(bytes: Option<u64>, total: u64) -> Option<u64> {
    let bytes = bytes?;
    if total == 0 {
        return None;
    }
    let pct = u128::from(bytes) * 100 / u128::from(total);
    Some(u64::try_from(pct).unwrap_or(u64::MAX))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub title: Title,
    pub groups: Vec<Group>,
    pub notes: Vec<Note>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Title {
    /// The target dir's name (`zbrain`), never its path.
    pub dir: String,
    /// `Claude Code`.
    pub agent: String,
    pub version: Option<String>,
    /// Items in the first request (mode always or advertised).
    pub items: usize,
    /// Every item the resolver found.
    pub all_items: usize,
    /// Predicted, or observed when a capture is bound.
    pub total: u64,
    pub measured: bool,
    /// Some rows' size is unknown: the total is a lower bound (`~`).
    pub estimate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Plugins,
    SkillListing,
    AgentListing,
    Memory,
    ProjectFiles,
    Hooks,
    McpServers,
    Other,
    Builtin,
}

impl GroupKind {
    /// Stable key of the group's row (fold state, cursor).
    pub fn key(self) -> &'static str {
        match self {
            GroupKind::Plugins => "plugins",
            GroupKind::SkillListing => "skill-listing",
            GroupKind::AgentListing => "agent-listing",
            GroupKind::Memory => "memory",
            GroupKind::ProjectFiles => "project-files",
            GroupKind::Hooks => "hooks",
            GroupKind::McpServers => "mcp-servers",
            GroupKind::Other => "other",
            GroupKind::Builtin => "built-in",
        }
    }

    /// The name in the header's `biggest:` line.
    pub fn short(self) -> &'static str {
        match self {
            GroupKind::Plugins => "plugins",
            GroupKind::SkillListing => "skill listing",
            GroupKind::AgentListing => "agent listing",
            GroupKind::Memory => "memory",
            GroupKind::ProjectFiles => "project",
            GroupKind::Hooks => "hooks",
            GroupKind::McpServers => "MCP servers",
            GroupKind::Other => "other",
            GroupKind::Builtin => "built-in",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub kind: GroupKind,
    /// `Plugins (9)`, `Skill listing`.
    pub name: String,
    /// `216 entries · 144 descriptions cut`, `cannot be turned off`; empty when nothing to add.
    pub count_line: String,
    pub bytes: Option<u64>,
    /// Why it has no size (every row costs nothing up front, or counts elsewhere); None with no
    /// bytes = unmeasured.
    pub absent: Option<&'static str>,
    /// Percent of the total.
    pub share: Option<u64>,
    /// The group's rows count bytes other groups count too (Plugins: the listing lines its
    /// plugins bring): no size, bar or share of its own; `count_line` names the overlap.
    pub overlap: bool,
    /// `count_line` for a narrow frame.
    pub count_short: String,
    /// One switch turned the whole group off (every hook): no count, no size; `count_line`
    /// says so.
    pub all_off: bool,
    pub rows: Vec<Row>,
    /// `d` can act on some row inside.
    pub can_turn_off: bool,
    pub detail: Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    On,
    /// Turned off here by harn (journaled, gone from the tree).
    Off,
    /// No switch: it cannot be turned off.
    Fixed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Stable among its siblings (fold state, cursor).
    pub key: String,
    /// The item `d` / `o` / `w` / `e` act on; None for a row that only gathers others.
    pub id: Option<String>,
    pub name: String,
    /// Marketplace, `~/.claude`, `project`, `claude.ai`, `Claude Code`.
    pub source: String,
    /// `plugin`, `skill`, `command`, `MCP server`, `hook`, `agent`, `file`, `tool`.
    pub kind_word: String,
    /// Its own first-request bytes (with what is folded under it), never the listing lines a
    /// plugin brings (those count once, under the listing groups); None = unknown or none.
    pub bytes: Option<u64>,
    pub share: Option<u64>,
    /// Why it has no size: `not loaded`, `runs later`, `when used`, `off`, `in listing(s)`.
    pub absent: Option<&'static str>,
    /// What turning it off frees in all (a plugin: its own bytes and its listing lines); the
    /// order among rows without a size.
    pub weight: Option<u64>,
    /// What turning it off frees, in words (`32 KB: 4.0 KB of its own plus 28 KB of skill and
    /// agent listing entries`) when that is the whole plugin; the `d` card's Saves line.
    pub saving: Option<String>,
    pub children: Vec<Row>,
    pub state: State,
    pub detail: Detail,
}

/// The panel under the list, in sentences.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detail {
    /// What it is and where it is enabled.
    pub what: String,
    /// `what` without its least needed part (a plugin's version), for a panel too narrow for it.
    pub what_short: Option<String>,
    /// What it brings and what it costs.
    pub brings: String,
    /// What `d` would do.
    pub action: String,
    /// What that saves.
    pub saves: String,
    pub undo: String,
}

impl Detail {
    /// The sentences after `what` and `brings`, in order; a narrow panel drops them from the end.
    pub fn sentences(&self) -> Vec<String> {
        [&self.action, &self.saves, &self.undo]
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }
}

/// A visible line of the list: a group (`path` empty) or a row under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub group: usize,
    pub path: Vec<usize>,
    pub depth: usize,
    pub key: String,
}

impl Screen {
    pub fn row(&self, group: usize, path: &[usize]) -> Option<&Row> {
        let (first, rest) = path.split_first()?;
        let mut row = self.groups.get(group)?.rows.get(*first)?;
        for i in rest {
            row = row.children.get(*i)?;
        }
        Some(row)
    }

    pub fn line_row(&self, line: &Line) -> Option<&Row> {
        self.row(line.group, &line.path)
    }

    /// Key of the line at `group` / `path`: the group's key, then each row's key.
    pub fn key(&self, group: usize, path: &[usize]) -> String {
        let mut key = self.groups[group].kind.key().to_string();
        let mut rows = &self.groups[group].rows;
        for i in path {
            key.push('\n');
            key.push_str(&rows[*i].key);
            rows = &rows[*i].children;
        }
        key
    }

    /// The row of item `id` (its own row, before a listing owner row acting on it), in screen
    /// order: its group and index path.
    pub fn find(&self, id: &str) -> Option<(usize, Vec<usize>)> {
        fn within(rows: &[Row], hit: &dyn Fn(&Row) -> bool, path: &mut Vec<usize>) -> bool {
            for (i, row) in rows.iter().enumerate() {
                path.push(i);
                if hit(row) || within(&row.children, hit, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let search = |hit: &dyn Fn(&Row) -> bool| {
            self.groups.iter().enumerate().find_map(|(g, group)| {
                let mut path = Vec::new();
                within(&group.rows, hit, &mut path).then_some((g, path))
            })
        };
        search(&|row: &Row| row.key == id)
            .or_else(|| search(&|row: &Row| row.id.as_deref() == Some(id)))
    }

    /// The lines to show: every group; rows under an expanded group or row. With a filter, a
    /// line stays when it matches, has a match below (and opens to show it) or sits under a
    /// match.
    pub fn lines(&self, expanded: &HashSet<String>, filter: Option<&str>) -> Vec<Line> {
        let needle = filter
            .map(|f| f.trim().to_lowercase())
            .filter(|f| !f.is_empty());
        let mut out = Vec::new();
        for (g, group) in self.groups.iter().enumerate() {
            let key = group.kind.key().to_string();
            let group_hit = needle
                .as_deref()
                .is_some_and(|n| group.name.to_lowercase().contains(n));
            let below = needle
                .as_deref()
                .is_some_and(|n| group.rows.iter().any(|r| reaches(r, n)));
            if needle.is_some() && !group_hit && !below {
                continue;
            }
            out.push(Line {
                group: g,
                path: Vec::new(),
                depth: 0,
                key: key.clone(),
            });
            if expanded.contains(&key) || below {
                push_rows(
                    &group.rows,
                    g,
                    &[],
                    &key,
                    1,
                    expanded,
                    needle.as_deref(),
                    group_hit,
                    &mut out,
                );
            }
        }
        out
    }

    /// Rows anywhere in the screen, depth-first.
    pub fn all_rows(&self) -> Vec<&Row> {
        fn visit<'a>(rows: &'a [Row], out: &mut Vec<&'a Row>) {
            for row in rows {
                out.push(row);
                visit(&row.children, out);
            }
        }
        let mut out = Vec::new();
        for group in &self.groups {
            visit(&group.rows, &mut out);
        }
        out
    }
}

fn hit(row: &Row, needle: &str) -> bool {
    row.name.to_lowercase().contains(needle)
        || row.source.to_lowercase().contains(needle)
        || row.kind_word.to_lowercase().contains(needle)
}

fn reaches(row: &Row, needle: &str) -> bool {
    hit(row, needle) || row.children.iter().any(|c| reaches(c, needle))
}

#[allow(clippy::too_many_arguments)]
fn push_rows(
    rows: &[Row],
    group: usize,
    path: &[usize],
    parent_key: &str,
    depth: usize,
    expanded: &HashSet<String>,
    needle: Option<&str>,
    under_hit: bool,
    out: &mut Vec<Line>,
) {
    for (i, row) in rows.iter().enumerate() {
        let matched = needle.is_some_and(|n| hit(row, n));
        let below = needle.is_some_and(|n| row.children.iter().any(|c| reaches(c, n)));
        if needle.is_some() && !under_hit && !matched && !below {
            continue;
        }
        let mut p = path.to_vec();
        p.push(i);
        let key = format!("{parent_key}\n{}", row.key);
        out.push(Line {
            group,
            path: p.clone(),
            depth,
            key: key.clone(),
        });
        if !row.children.is_empty() && (expanded.contains(&key) || below) {
            push_rows(
                &row.children,
                group,
                &p,
                &key,
                depth + 1,
                expanded,
                needle,
                under_hit || matched,
                out,
            );
        }
    }
}

// ---- building --------------------------------------------------------------------------------

/// The screen for `tree`: `off` = items harn turned off here (no longer in the tree);
/// `journaled` = item ids a live journal entry changed here.
pub fn build(
    tree: &Tree,
    off: &[OffItem],
    journaled: &HashMap<String, String>,
    sort: Sort,
) -> Screen {
    let b = Builder {
        tree,
        places: Places::of(tree),
        total: denominator(tree),
        journaled,
    };
    let mut screen = Screen {
        title: b.title(),
        groups: b.groups(off),
        notes: b.notes(),
    };
    for group in &mut screen.groups {
        sort_rows(&mut group.rows, sort);
    }
    // Biggest first, the groups without a size after them (in their own order), built-in last.
    screen.groups.sort_by_key(|g| {
        (
            g.kind == GroupKind::Builtin,
            g.bytes.is_none(),
            std::cmp::Reverse(g.bytes),
        )
    });
    screen
}

struct Builder<'a> {
    tree: &'a Tree,
    places: Places,
    total: u64,
    journaled: &'a HashMap<String, String>,
}

/// `Claude Code`, `Codex`.
pub fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::ClaudeCode => "Claude Code",
        Agent::Codex => "Codex",
    }
}

/// `slack` from `slack@claude-plugins-official`.
pub fn plugin_name(key: &str) -> &str {
    key.split('@').next().unwrap_or(key)
}

/// The marketplace of `name@marketplace` in the source column: `official`, `claude.ai`, or its
/// own name.
pub fn market_word(key: &str) -> String {
    match key.split_once('@').map(|(_, m)| m) {
        Some("claude-plugins-official") => "official".to_string(),
        Some("synced") => "claude.ai".to_string(),
        Some("builtin") => "Claude Code".to_string(),
        Some(m) => m.to_string(),
        None => String::new(),
    }
}

/// `the official marketplace`, `claude.ai`, `the soma-work marketplace`.
fn market_phrase(key: &str) -> String {
    match key.split_once('@').map(|(_, m)| m) {
        Some("claude-plugins-official") => "the official marketplace".to_string(),
        Some("synced") => "claude.ai".to_string(),
        Some("builtin") => "Claude Code itself".to_string(),
        Some(m) => format!("the {m} marketplace"),
        None => "an unknown marketplace".to_string(),
    }
}

/// A plugin's identity, `name@marketplace`: its `origin.plugin`, else its label. Every lookup of
/// a plugin (its group row, its parts, its owner rows in the listings) goes through this.
fn plugin_key(plugin: &Item) -> &str {
    plugin
        .origin
        .plugin
        .as_deref()
        .unwrap_or(plugin.label.as_str())
}

/// The plugin (`name@marketplace`) an item belongs to, when it is a plugin's part.
fn owner_plugin(item: &Item) -> Option<&str> {
    if item.kind == Kind::Plugin {
        return None;
    }
    item.origin.plugin.as_deref()
}

/// Kind word of a row: `skill`, `command`, `agent`, `MCP server`, `hook`, `file`, `tool`.
pub fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Skill => "skill",
        Kind::Command => "command",
        Kind::SubAgent => "agent",
        Kind::McpServer => "MCP server",
        Kind::McpInstructions => "MCP instructions",
        Kind::McpTool => "MCP tool",
        Kind::Hook => "hook",
        Kind::Instruction => "file",
        Kind::Memory => "memory file",
        Kind::Plugin => "plugin",
        Kind::BuiltinTool => "tool",
        Kind::SkillListing => "listing",
        Kind::AgentRoster => "listing",
        Kind::OutputStyle => "output style",
        Kind::PermissionRule => "permission rules",
        Kind::StatusLine => "status line",
        Kind::SystemPrompt => "system prompt",
        Kind::EnvBlock | Kind::GitBlock | Kind::Attribution => "section",
        Kind::Unresolved => "unresolved",
    }
}

/// `8 skills`, `1 MCP server`.
pub fn count(n: usize, word: &str) -> String {
    if n == 1 {
        return format!("1 {word}");
    }
    let plural = match word {
        "MCP server" => "MCP servers".to_string(),
        "memory file" => "memory files".to_string(),
        "entry" => "entries".to_string(),
        w if w.ends_with('s') => w.to_string(),
        w => format!("{w}s"),
    };
    format!("{n} {plural}")
}

/// `a, b and c`.
pub fn and_list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// Whether `item` is counted in the first-request total (the resolver's totals rule).
fn in_request(item: &Item) -> bool {
    matches!(item.mode, Mode::Always | Mode::Advertised) && !item.cost.not_applicable
}

/// Known first-request bytes of `item` with everything under it, counted as the total counts
/// them (mode always or advertised); None when none is known. A listing's own cost already is
/// the sum of its entries.
fn subtree_bytes(item: &Item) -> Option<u64> {
    let own = request_bytes(&item.cost).filter(|_| in_request(item));
    if matches!(item.kind, Kind::SkillListing | Kind::AgentRoster) {
        return own;
    }
    sum_known(
        own.into_iter()
            .chain(item.children.iter().filter_map(subtree_bytes)),
    )
}

/// Some part of `item` is in the request with a size nobody knows yet.
fn has_unknown(item: &Item) -> bool {
    (in_request(item) && request_bytes(&item.cost).is_none())
        || item.children.iter().any(has_unknown)
}

/// What turning a plugin off frees: its own parts (MCP servers, hooks) and its lines in the
/// skill and agent listings.
struct PluginFreed {
    /// Known bytes in all; None when nothing of it is known.
    total: Option<u64>,
    /// Of `total`, its listing lines (counted in the listing groups too).
    listing: u64,
    /// Of `total`, its own parts (MCP servers, hooks) whose size is known.
    own: u64,
    /// Some own part is in the request with a size nobody knows yet.
    unknown: bool,
    /// The whole of it in words (`3.7 KB: 3.7 KB of its entries in the skill listing, plus its
    /// own unmeasured part`); None when nothing of it is in the request.
    text: Option<String>,
}

fn plugin_freed(facets: &[&Item]) -> PluginFreed {
    let (listed, own): (Vec<&Item>, Vec<&Item>) = facets.iter().partition(|f| listing_entry(f));
    let listing = sum_known(listed.iter().filter_map(|i| subtree_bytes(i))).unwrap_or(0);
    let mine = sum_known(own.iter().filter_map(|i| subtree_bytes(i))).unwrap_or(0);
    let unknown = own.iter().any(|i| has_unknown(i));
    let skills = listed.iter().any(|i| i.kind != Kind::SubAgent);
    let agents = listed.iter().any(|i| i.kind == Kind::SubAgent);
    let which = match (skills, agents) {
        (true, true) => "skill and agent listings",
        (false, true) => "agent listing",
        _ => "skill listing",
    };
    let (l, m, t) = (size(listing), size(mine), size(listing + mine));
    let text = match (listing > 0, mine > 0, unknown) {
        (true, true, false) => {
            format!("{t}: {m} of its own plus {l} of its entries in the {which}")
        }
        (true, true, true) => format!(
            "{t}: {m} of its own plus {l} of its entries in the {which}, and an unmeasured part"
        ),
        (true, false, true) => {
            format!("{t}: {l} of its entries in the {which}, plus its own unmeasured part")
        }
        (true, false, false) => format!("{t}: its entries in the {which}"),
        (false, true, false) => m,
        (false, true, true) => format!("{m} and an unmeasured part"),
        (false, false, true) => "an amount only `harn verify --run` can measure".to_string(),
        (false, false, false) => String::new(),
    };
    PluginFreed {
        total: Some(listing + mine).filter(|t| *t > 0),
        listing,
        own: mine,
        unknown,
        text: Some(text).filter(|t| !t.is_empty()),
    }
}

fn sum_known(values: impl Iterator<Item = u64>) -> Option<u64> {
    let mut total = None;
    for v in values {
        total = Some(total.unwrap_or(0u64).saturating_add(v));
    }
    total
}

/// What a row with no first-request bytes says instead of a size.
fn absent_word(mode: Mode) -> Option<&'static str> {
    match mode {
        Mode::Runtime => Some("runs later"),
        Mode::None => Some("not loaded"),
        Mode::OnDemand | Mode::Deferred => Some("when used"),
        Mode::Always | Mode::Advertised => None,
    }
}

/// The knob `d` uses (scope local, as `plan::build_plan` with no scope picks it).
fn local_knob(item: &Item) -> Option<&Knob> {
    item.knobs
        .iter()
        .find(|k| knob_scope(k) == Some(Scope::Local))
}

/// The narrowest switch of any scope.
fn any_knob(item: &Item) -> Option<(&Knob, Scope)> {
    item.knobs
        .iter()
        .filter_map(|k| knob_scope(k).map(|s| (k, s)))
        .min_by_key(|(_, s)| *s)
}

/// `for this project only`, `for everyone in this repository`, `for every project`.
pub fn scope_words(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => "for this project only",
        Scope::Project => "for everyone in this repository",
        Scope::User => "for every project",
    }
}

/// `this project`, `this repository`, `every project` (card titles).
pub fn scope_short(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => "this project",
        Scope::Project => "this repository",
        Scope::User => "every project",
    }
}

/// JSON pointer segments, unescaped; a plugin key under `enabledPlugins` loses its
/// `@marketplace`; the target dir as a key (`projects/<dir>`) reads `this project`.
pub fn pointer_words(pointer: &str, places: &Places) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for seg in pointer.split('/').filter(|s| !s.is_empty()) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        let word = if out.last().is_some_and(|p| p == "enabledPlugins") {
            plugin_name(&seg).to_string()
        } else if Path::new(&seg) == places.dir {
            "this project".to_string()
        } else {
            places.text(&seg)
        };
        out.push(word);
    }
    out
}

/// The file a knob writes, shown from the target dir.
fn knob_file(knob: &Knob, places: &Places) -> Option<String> {
    match knob {
        Knob::Setting { file, .. } => Some(places.path(file)),
        Knob::Env { scope, .. } => Some(places.path(&settings_path(places, *scope))),
        Knob::Quarantine { from, .. } => Some(places.path(from)),
        Knob::None { .. } => None,
    }
}

/// `plan::settings_file` without a `Target`.
fn settings_path(places: &Places, scope: Scope) -> PathBuf {
    match scope {
        Scope::Local => places.dir.join(".claude/settings.local.json"),
        Scope::Project => places.dir.join(".claude/settings.json"),
        Scope::User => places
            .home
            .clone()
            .unwrap_or_default()
            .join(".claude/settings.json"),
    }
}

/// A JSON value as it is written: `false`, `"off"`, `{"commit":""}`; strings home-relative.
fn value_text(value: &serde_json::Value, places: &Places) -> String {
    match value {
        serde_json::Value::String(s) => places.text(s),
        other => places.text(&other.to_string()),
    }
}

/// `.claude/settings.local.json → enabledPlugins.slack = false`; an append pointer (`…/-`) reads
/// `permissions.deny += Bash`.
pub fn writes_line(knob: &Knob, places: &Places, arrow: &str) -> String {
    match knob {
        Knob::Setting {
            file,
            pointer,
            value,
            ..
        } => {
            let mut words = pointer_words(pointer, places);
            let append = words.last().is_some_and(|w| w == "-");
            if append {
                words.pop();
            }
            let op = if append { "+=" } else { "=" };
            format!(
                "{} {arrow} {} {op} {}",
                places.path(file),
                words.join("."),
                value_text(value, places)
            )
        }
        Knob::Env {
            name, value, scope, ..
        } => format!(
            "{} {arrow} env.{name} = {value}",
            places.path(&settings_path(places, *scope))
        ),
        Knob::Quarantine { from, to, .. } => {
            format!("moves {} {arrow} {}", places.path(from), places.path(to))
        }
        Knob::None { reason } => format!("nothing ({reason})"),
    }
}

/// A `Knob::None` reason as a sentence end.
fn reason_words(reason: &str) -> String {
    match reason {
        "built-in" => "it is part of Claude Code".to_string(),
        "built-in; no documented switch" => {
            "Claude Code has no documented switch for it".to_string()
        }
        "built-in command" => "it is a command built into Claude Code".to_string(),
        "not in the request" => "it never reaches the model".to_string(),
        other => other.to_string(),
    }
}

/// Is this item an entry of the skill or agent listing (its bytes are a listing line)?
fn listing_entry(item: &Item) -> bool {
    matches!(item.kind, Kind::Skill | Kind::Command | Kind::SubAgent)
        && item.mode == Mode::Advertised
}

impl Builder<'_> {
    fn title(&self) -> Title {
        let tree = self.tree;
        let dir = tree
            .target_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.places.path(&tree.target_dir));
        Title {
            dir,
            agent: agent_name(tree.agent).to_string(),
            version: tree.agent_version.clone(),
            items: model::walk(tree)
                .filter(|i| matches!(i.mode, Mode::Always | Mode::Advertised))
                .count(),
            all_items: model::walk(tree).count(),
            total: self.total,
            measured: tree.observed_body.is_some(),
            estimate: tree.observed_body.is_none() && tree.unknown_rows > 0,
        }
    }

    fn notes(&self) -> Vec<Note> {
        let tree = self.tree;
        let mut notes = Vec::new();
        let note = |text: String| Note { text };
        if let Some(v) = &tree.agent_version {
            if *v != tree.rules_verified_against {
                notes.push(note(format!(
                    "{} {v} is installed; harn's rules were checked against {}. Sizes and \
                     switches may be off until `harn verify --run` passes.",
                    agent_name(tree.agent),
                    tree.rules_verified_against
                )));
            }
        }
        let cause = tree
            .tool_search_cause
            .as_deref()
            .map(|c| format!(" ({})", self.places.text(c)))
            .unwrap_or_default();
        match tree.tool_search {
            Some(false) => notes.push(note(format!(
                "Tool search is off{cause}: every MCP tool schema loads in the first request. \
                 ENABLE_TOOL_SEARCH=true in settings would load them only when used; the \
                 effect is unmeasured until `harn verify --run`."
            ))),
            None if tree.agent == Agent::ClaudeCode => notes.push(note(format!(
                "Tool search cannot be decided from settings and env{cause}: MCP tool schemas \
                 may load in the first request."
            ))),
            _ => {}
        }
        for signal in &tree.signals {
            notes.push(note(sentence(&self.places.text(signal))));
        }
        for warning in &tree.warnings {
            let text = if warning == CAP_STATE_UNKNOWN {
                "The skill listing has a size cap counted in tokens; whether it cuts \
                 descriptions here is unknown until `harn verify --run` measures a session."
                    .to_string()
            } else {
                sentence(&self.places.text(warning))
            };
            notes.push(note(text));
        }
        notes
    }

    fn share(&self, bytes: Option<u64>) -> Option<u64> {
        percent(bytes, self.total)
    }

    fn groups(&self, off: &[OffItem]) -> Vec<Group> {
        let tree = self.tree;
        // Plugins and the parts that fold under them.
        let plugins: Vec<&Item> = tree
            .items
            .iter()
            .filter(|i| i.kind == Kind::Plugin && i.origin.layer != Layer::Builtin)
            .collect();
        let keys: HashSet<&str> = plugins.iter().map(|p| plugin_key(p)).collect();
        let mut facets: HashMap<&str, Vec<&Item>> = HashMap::new();
        collect_facets(&tree.items, &keys, &mut facets);
        let claimed: HashSet<&str> = facets.values().flatten().map(|i| i.id.as_str()).collect();

        let mut plugin_rows: Vec<Row> = plugins
            .iter()
            .map(|p| {
                self.plugin_row(
                    p,
                    facets.get(plugin_key(p)).map(Vec::as_slice).unwrap_or(&[]),
                )
            })
            .collect();
        // What all plugins bring: listing lines (the listing groups count them) and their own.
        let (plugin_listing, plugin_own) = plugins
            .iter()
            .map(|p| plugin_freed(facets.get(plugin_key(p)).map(Vec::as_slice).unwrap_or(&[])))
            .fold((0u64, 0u64), |(l, o), f| (l + f.listing, o + f.own));
        // What turning each plugin off frees, for its owner rows in the listings.
        let savings: HashMap<String, Option<String>> = plugins
            .iter()
            .zip(&plugin_rows)
            .map(|(p, row)| (plugin_key(p).to_string(), row.saving.clone()))
            .collect();

        let mut skill_owners: Vec<Row> = Vec::new();
        let mut skill_listing: Option<&Item> = None;
        let mut agent_owners: Vec<Row> = Vec::new();
        let mut agent_listing: Option<&Item> = None;
        let mut memory = Vec::new();
        let mut files = Vec::new();
        let mut hooks: Vec<&Item> = Vec::new();
        let mut servers = Vec::new();
        let mut builtin = Vec::new();
        let mut tools: Vec<Row> = Vec::new();
        let mut builtin_plugins: Vec<Row> = Vec::new();
        let mut other = Vec::new();

        for item in &tree.items {
            if claimed.contains(item.id.as_str()) {
                continue;
            }
            match item.kind {
                Kind::Plugin if item.origin.layer == Layer::Builtin => {
                    let mut row = self.leaf(item, "plugin", "");
                    row.name = plugin_name(&item.label).to_string();
                    row.detail = self.detail(item, &row.name, row.bytes, row.absent);
                    builtin_plugins.push(row);
                }
                Kind::Plugin => {}
                Kind::SkillListing => {
                    skill_listing = Some(item);
                    skill_owners = self.owner_rows(item, &plugins, &savings);
                }
                Kind::AgentRoster => {
                    agent_listing = Some(item);
                    agent_owners = self.owner_rows(item, &plugins, &savings);
                }
                Kind::Memory => memory.push(self.file_row(item)),
                Kind::Instruction | Kind::Unresolved => files.push(self.file_row(item)),
                Kind::Hook => hooks.push(item),
                Kind::McpServer | Kind::McpInstructions | Kind::McpTool => {
                    servers.push(self.server_row(item, None))
                }
                Kind::BuiltinTool => tools.push(self.leaf(item, "tool", "")),
                _ if item.origin.layer == Layer::Builtin => {
                    builtin.push(self.leaf(item, kind_word(item.kind), ""))
                }
                // Not in the request at all (status line, permission rules).
                _ if item.cost.not_applicable && matches!(item.mode, Mode::None) => {}
                _ => other.push(self.leaf(item, kind_word(item.kind), &self.layer_source(item))),
            }
        }
        let mut hook_rows = self.hook_rows(&hooks);

        // Off rows: where the item would be.
        for o in off {
            self.place_off(
                o,
                &mut plugin_rows,
                &mut skill_owners,
                &mut agent_owners,
                &mut memory,
                &mut files,
                &mut hook_rows,
                &mut servers,
                &mut tools,
                &mut other,
            );
        }

        if !tools.is_empty() {
            builtin.push(self.gather(
                "built-in tools",
                "tools",
                tools,
                "The tools Claude Code itself offers; each one's schema is in every request.",
            ));
        }
        if !builtin_plugins.is_empty() {
            builtin.push(self.gather(
                "built-in plugins",
                "plugins",
                builtin_plugins,
                "Plugins shipped inside Claude Code; toggled in its /plugin menu.",
            ));
        }

        let mut groups = Vec::new();
        if !plugin_rows.is_empty() {
            groups.push(self.plugins_group(plugin_rows, plugin_listing, plugin_own));
        }
        // A listing shows when it is in the tree or when harn turned some of its entries off
        // (they stay findable for `o`, even with no live listing left).
        if skill_listing.is_some() || !skill_owners.is_empty() {
            groups.push(self.listing_group(GroupKind::SkillListing, skill_listing, skill_owners));
        }
        if agent_listing.is_some() || !agent_owners.is_empty() {
            groups.push(self.listing_group(GroupKind::AgentListing, agent_listing, agent_owners));
        }
        if !memory.is_empty() {
            groups.push(
                self.group(
                    GroupKind::Memory,
                    "Memory",
                    memory,
                    Detail {
                        what: "Memory · notes Claude Code keeps for this directory".to_string(),
                        brings: "MEMORY.md (auto-memory) loads in every session here, up to its \
                             cap."
                            .to_string(),
                        action: "Open it (→); d turns auto-memory off for this project only."
                            .to_string(),
                        ..Detail::default()
                    },
                ),
            );
        }
        if !files.is_empty() {
            groups.push(
                self.group(
                    GroupKind::ProjectFiles,
                    "Project files",
                    files,
                    Detail {
                        what: "Project files · CLAUDE.md with its imports, rules, and your \
                           ~/.claude/CLAUDE.md"
                            .to_string(),
                        brings: "Each one loads in full in every first request.".to_string(),
                        action: "Open the group (→); d leaves a file out for this project only."
                            .to_string(),
                        ..Detail::default()
                    },
                ),
            );
        }
        let hooks_all_off = off.iter().any(|o| o.pointer.as_deref() == Some(ALL_HOOKS));
        if !hook_rows.is_empty() && hooks_all_off {
            // The resolver drops every hook once the switch is on: a count would be a lie.
            let mut group = self.group(
                GroupKind::Hooks,
                "Hooks",
                hook_rows,
                Detail {
                    what: "Hooks · all off for this project".to_string(),
                    brings: "harn turned every hook off here with Claude Code's one switch for \
                             them; none runs until it is turned back on."
                        .to_string(),
                    action: "u undoes the change; o on the hook below does too.".to_string(),
                    ..Detail::default()
                },
            );
            group.name = "Hooks".to_string();
            group.count_line = "all off for this project · u undoes".to_string();
            group.all_off = true;
            group.absent = None;
            groups.push(group);
        } else if !hook_rows.is_empty() {
            groups.push(
                self.group(
                    GroupKind::Hooks,
                    "Hooks",
                    hook_rows,
                    Detail {
                        what: "Hooks · commands Claude Code runs on events, outside any plugin"
                            .to_string(),
                        brings: "What SessionStart and UserPromptSubmit hooks print enters the \
                             request; the others run later."
                            .to_string(),
                        action: "Open the group (→) to see each hook.".to_string(),
                        ..Detail::default()
                    },
                ),
            );
        }
        if !servers.is_empty() {
            groups.push(
                self.group(
                    GroupKind::McpServers,
                    "MCP servers",
                    servers,
                    Detail {
                        what: "MCP servers · tool servers outside any plugin".to_string(),
                        brings: "Their tool schemas load up front while tool search is off; the \
                             size is known only after a run."
                            .to_string(),
                        action: "Open the group (→) to see each server.".to_string(),
                        ..Detail::default()
                    },
                ),
            );
        }
        if !other.is_empty() {
            groups.push(self.group(
                GroupKind::Other,
                "Other",
                other,
                Detail {
                    what: "Other · parts of the request with no group of their own".to_string(),
                    action: "Open the group (→) to see each one.".to_string(),
                    ..Detail::default()
                },
            ));
        }
        if !builtin.is_empty() {
            groups.push(self.builtin_group(builtin));
        }
        groups
    }

    fn group(&self, kind: GroupKind, name: &str, rows: Vec<Row>, detail: Detail) -> Group {
        let bytes = sum_known(rows.iter().filter_map(|r| r.bytes));
        Group {
            kind,
            name: format!("{name} ({})", rows.len()),
            count_line: String::new(),
            bytes,
            absent: common_absent(bytes, &rows),
            share: self.share(bytes),
            overlap: false,
            count_short: String::new(),
            all_off: false,
            can_turn_off: any_on(&rows),
            rows,
            detail,
        }
    }

    /// `listing` = bytes of the listing lines the plugins bring (the listing groups count them);
    /// `own` = the known bytes of their own parts.
    fn plugins_group(&self, rows: Vec<Row>, listing: u64, own: u64) -> Group {
        let enabled = rows.iter().filter(|r| r.state != State::Off).count();
        let detail = Detail {
            what: format!(
                "Plugins · {enabled} enabled for this directory{}",
                match rows.len() - enabled {
                    0 => String::new(),
                    n => format!(", {n} turned off here"),
                }
            ),
            brings: "Turn one off and its skills, commands, agents and hooks leave next \
                     session."
                .to_string(),
            action: "Open the group (→) to see each plugin with its size.".to_string(),
            ..Detail::default()
        };
        let mut group = self.group(GroupKind::Plugins, "Plugins", rows, detail);
        // Each row is what turning that plugin off frees: its listing lines (the listing groups
        // count them already) and its own parts. The group line says which is which and has no
        // size, bar or share of its own.
        if listing + own > 0 {
            let (l, o, t) = (size(listing), size(own), size(listing + own));
            group.bytes = None;
            group.share = None;
            group.absent = None;
            group.overlap = true;
            (group.count_line, group.count_short) = match (listing > 0, own > 0) {
                (true, false) => (
                    format!("{t} would leave with them · counted in the listings above"),
                    format!("{l} in the listings above"),
                ),
                (true, true) => (
                    format!("{t} would leave with them · {l} counted in the listings above"),
                    format!("{l} in the listings above + {o} of their own"),
                ),
                _ => (
                    format!("{o} would leave with them"),
                    format!("{o} would leave with them"),
                ),
            };
        }
        group
    }

    /// `listing` = the live listing item; None when only entries harn turned off are left.
    fn listing_group(&self, kind: GroupKind, listing: Option<&Item>, owners: Vec<Row>) -> Group {
        let entries = listing.map_or(0, |l| {
            l.children
                .iter()
                .filter(|c| c.mode == Mode::Advertised)
                .count()
        });
        let off = off_count(&owners);
        let cut = listing
            .and_then(|l| l.signal.as_deref())
            .and_then(cut_count);
        let count_line = match (cut, listing) {
            (Some((dropped, total)), _) => format!("{total} entries · {dropped} descriptions cut"),
            (None, Some(_)) => format!("{entries} entries"),
            (None, None) => format!("{off} turned off here"),
        };
        let bytes = listing
            .and_then(|l| request_bytes(&l.cost))
            .or(sum_known(owners.iter().filter_map(|r| r.bytes)));
        let (name, what, brings) = match kind {
            _ if listing.is_none() => (
                if kind == GroupKind::SkillListing {
                    "Skill listing"
                } else {
                    "Agent listing"
                },
                format!(
                    "{} · nothing is listed now",
                    if kind == GroupKind::SkillListing {
                        "Skill listing"
                    } else {
                        "Agent listing"
                    }
                ),
                format!("harn turned {} off here.", count(off, "entry")),
            ),
            GroupKind::SkillListing => (
                "Skill listing",
                format!("Skill listing · {entries} entries, one line per skill or command"),
                match cut {
                    Some((dropped, _)) => format!(
                        "Sent in every first request and over its size cap: {dropped} \
                         descriptions are cut to the name."
                    ),
                    None => "Sent in every first request; it has a size cap, and whether it cuts \
                             descriptions here needs `harn verify --run`."
                        .to_string(),
                },
            ),
            _ => (
                "Agent listing",
                format!(
                    "Agent listing · {entries} agents Claude Code can hand work to, one line \
                     each"
                ),
                "Each line names the agent and says when to use it.".to_string(),
            ),
        };
        Group {
            kind,
            name: name.to_string(),
            count_line,
            bytes,
            absent: common_absent(bytes, &owners),
            share: self.share(bytes),
            overlap: false,
            count_short: String::new(),
            all_off: false,
            can_turn_off: any_on(&owners),
            rows: owners,
            detail: Detail {
                what,
                brings,
                action: if listing.is_some() {
                    "Open it (→) to see the entries by owner; d on an entry turns that one off."
                } else {
                    "Open it (→); o on an entry turns it back on."
                }
                .to_string(),
                ..Detail::default()
            },
        }
    }

    fn builtin_group(&self, rows: Vec<Row>) -> Group {
        let all_fixed = !any_on(&rows);
        let mut group = self.group(
            GroupKind::Builtin,
            "Built-in",
            rows,
            Detail {
                what: "Built-in · parts of Claude Code itself: system prompt, environment, \
                       tools"
                    .to_string(),
                brings: "Their sizes are unknown until `harn verify --run` measures a session."
                    .to_string(),
                action: if all_fixed {
                    "They cannot be turned off.".to_string()
                } else {
                    "Most cannot be turned off; the ones with a switch say so when selected."
                        .to_string()
                },
                ..Detail::default()
            },
        );
        group.count_line = if all_fixed {
            "cannot be turned off".to_string()
        } else {
            "mostly cannot be turned off".to_string()
        };
        group
    }

    /// `Turned off here by harn` for items in the journal; `Off` rows say it themselves.
    fn journaled_note(&self, item: &Item) -> Option<String> {
        self.journaled
            .contains_key(&item.id)
            .then(|| "harn changed it here; o reverses that change.".to_string())
    }

    /// Source words of a non-plugin item: `project`, `~/.claude`, `claude.ai`, `Claude Code`.
    fn layer_source(&self, item: &Item) -> String {
        match item.origin.layer {
            Layer::Project => "project".to_string(),
            Layer::Local => "this project".to_string(),
            Layer::User => "~/.claude".to_string(),
            Layer::ClaudeAi => "claude.ai".to_string(),
            Layer::Builtin => "Claude Code".to_string(),
            Layer::Managed => "organisation".to_string(),
            Layer::Plugin => item
                .origin
                .plugin
                .as_deref()
                .map(market_word)
                .unwrap_or_else(|| "plugin".to_string()),
            Layer::Dynamic => "session".to_string(),
            Layer::Unresolved => "unresolved".to_string(),
        }
    }

    /// `the slack plugin`, `this repository`, `your ~/.claude`, `claude.ai`, `Claude Code`.
    fn origin_phrase(&self, item: &Item) -> String {
        if let Some(p) = owner_plugin(item) {
            return format!("the {} plugin", plugin_name(p));
        }
        match item.origin.layer {
            Layer::Project => "this repository".to_string(),
            Layer::Local => "this project only".to_string(),
            Layer::User => "your ~/.claude".to_string(),
            Layer::ClaudeAi => "claude.ai".to_string(),
            Layer::Builtin => "Claude Code".to_string(),
            Layer::Managed => "your organisation's settings".to_string(),
            Layer::Plugin => "a plugin".to_string(),
            Layer::Dynamic => "the session".to_string(),
            Layer::Unresolved => "an unresolved source".to_string(),
        }
    }

    /// Size, share and absent word of a row for `item` (with what is under it).
    fn costs(&self, item: &Item) -> (Option<u64>, Option<u64>, Option<&'static str>) {
        let bytes = subtree_bytes(item);
        let absent = if bytes.is_some() {
            None
        } else if item.cost.not_applicable || matches!(item.mode, Mode::Runtime | Mode::None) {
            absent_word(item.mode).or(Some("not loaded"))
        } else {
            None
        };
        (bytes, self.share(bytes), absent)
    }

    fn state(&self, item: &Item) -> State {
        if any_knob(item).is_some() {
            State::On
        } else {
            State::Fixed
        }
    }

    /// A row for one item, its children folded into its size.
    fn leaf(&self, item: &Item, kind_word: &str, source: &str) -> Row {
        let (bytes, share, absent) = self.costs(item);
        let name = self.places.text(&item.label);
        let detail = self.detail(item, &name, bytes, absent);
        Row {
            key: item.id.clone(),
            id: Some(item.id.clone()),
            name,
            source: source.to_string(),
            kind_word: kind_word.to_string(),
            bytes,
            share,
            absent,
            weight: bytes,
            saving: None,
            children: Vec::new(),
            state: self.state(item),
            detail,
        }
    }

    /// A row gathering `rows` (built-in tools, owners without a single switch).
    fn gather(&self, name: &str, word: &str, rows: Vec<Row>, about: &str) -> Row {
        let bytes = sum_known(rows.iter().filter_map(|r| r.bytes));
        let state = if any_on(&rows) {
            State::On
        } else {
            State::Fixed
        };
        let n = rows.len();
        Row {
            key: format!("gather:{name}"),
            id: None,
            name: format!("{name} ({n})"),
            source: String::new(),
            kind_word: word.to_string(),
            bytes,
            share: self.share(bytes),
            absent: common_absent(bytes, &rows),
            weight: bytes,
            saving: None,
            children: rows,
            state,
            detail: Detail {
                what: format!("{name} · {}", count(n, word.trim_end_matches('s'))),
                brings: about.to_string(),
                action: match state {
                    State::On => "Open it (→) and pick one; d turns that one off.".to_string(),
                    _ => "They cannot be turned off.".to_string(),
                },
                ..Detail::default()
            },
        }
    }

    fn plugin_row(&self, plugin: &Item, facets: &[&Item]) -> Row {
        let key = plugin_key(plugin).to_string();
        let name = plugin_name(&key).to_string();
        let freed = plugin_freed(facets);
        let saving = freed.text.clone();
        let mut children: Vec<Row> = facets
            .iter()
            .map(|f| self.facet_row(f, &name, true, saving.as_deref()))
            .collect();
        number_duplicates(&mut children);
        // Its number is what turning it off frees: its own parts and its listing lines (those
        // are counted in the listing groups too; the Plugins group line says so).
        let bytes = freed.total;
        let absent = match (bytes, freed.unknown) {
            (Some(_), _) | (None, true) => None,
            (None, false) => absent_word(plugin.mode)
                .or_else(|| common_absent(None, &children))
                .or(Some("not loaded")),
        };
        let counts = facet_counts(facets);
        let brings_list = and_list(&counts);
        let what_with = |version: Option<String>| {
            format!(
                "{name} · plugin from {}{} · enabled {}",
                market_phrase(&key),
                version.map(|v| format!(", {v}")).unwrap_or_default(),
                enabled_scope(plugin, &self.places).unwrap_or_else(|| "here".to_string())
            )
        };
        let what = what_with(plugin_version(plugin));
        let what_short = Some(what_with(None)).filter(|short| *short != what);
        let brings = match absent {
            _ if counts.is_empty() => "Brings nothing that loads here.".to_string(),
            Some("not loaded") => format!("Brings {brings_list}, but none of it loads here."),
            Some("runs later") => {
                format!("Brings {brings_list}; it runs later and costs nothing up front.")
            }
            _ => format!("Brings {brings_list}."),
        };
        let mut detail = Detail {
            what,
            what_short,
            brings,
            ..Detail::default()
        };
        self.action_sentences(plugin, &mut detail, freed.listing > 0, "it");
        if let (Some(text), true) = (&saving, !detail.saves.is_empty()) {
            detail.saves = format!("Turning it off frees {text}.");
        }
        Row {
            key: plugin.id.clone(),
            id: Some(plugin.id.clone()),
            name,
            source: market_word(&key),
            kind_word: "plugin".to_string(),
            bytes,
            share: self.share(bytes),
            absent,
            weight: bytes,
            saving,
            children,
            state: self.state(plugin),
            detail,
        }
    }

    /// A plugin's part with its own bytes: `block-kit` (skill), `slack` (MCP server). Under its
    /// plugin row (`in_plugin`) a name that already says its kind (`SessionStart hook`) gets no
    /// kind column; `saving` = what turning the whole plugin off frees.
    fn facet_row(&self, item: &Item, plugin: &str, in_plugin: bool, saving: Option<&str>) -> Row {
        let name = match item.kind {
            Kind::Hook => hook_event(&item.label),
            _ => facet_name(&item.label, plugin),
        };
        let word = kind_word(item.kind);
        let (bytes, share, absent) = self.costs(item);
        let mut detail = self.detail(item, &name, bytes, absent);
        let whole_plugin = local_knob(item).is_some_and(|k| {
            matches!(k, Knob::Setting { pointer, .. } if pointer.starts_with("/enabledPlugins/"))
        });
        let saving = saving.filter(|_| whole_plugin).map(str::to_string);
        if let (Some(text), true) = (&saving, !detail.saves.is_empty()) {
            detail.saves = format!("That frees {text}.");
        }
        let source = if in_plugin && name.ends_with(&format!(" {word}")) {
            String::new()
        } else {
            word.to_string()
        };
        Row {
            key: item.id.clone(),
            id: Some(item.id.clone()),
            name,
            source,
            kind_word: word.to_string(),
            bytes,
            share,
            absent,
            weight: bytes,
            saving,
            children: Vec::new(),
            state: self.state(item),
            detail,
        }
    }

    /// The listing's entries under one row per owner (a plugin, this project, you, claude.ai,
    /// Claude Code), each entry with its own line's bytes.
    fn owner_rows(
        &self,
        listing: &Item,
        plugins: &[&Item],
        savings: &HashMap<String, Option<String>>,
    ) -> Vec<Row> {
        let mut owners: Vec<(String, Vec<&Item>)> = Vec::new();
        for entry in listing
            .children
            .iter()
            .filter(|c| c.mode == Mode::Advertised)
        {
            let key = owner_key(entry);
            match owners.iter_mut().find(|(k, _)| *k == key) {
                Some((_, entries)) => entries.push(entry),
                None => owners.push((key, vec![entry])),
            }
        }
        owners
            .into_iter()
            .map(|(key, entries)| self.owner_row(&key, &entries, plugins, savings))
            .collect()
    }

    fn owner_row(
        &self,
        key: &str,
        entries: &[&Item],
        plugins: &[&Item],
        savings: &HashMap<String, Option<String>>,
    ) -> Row {
        let first = entries[0];
        let plugin =
            owner_plugin(first).and_then(|p| plugins.iter().find(|i| plugin_key(i) == p).copied());
        let (name, source) = match owner_plugin(first) {
            Some(p) => (plugin_name(p).to_string(), market_word(p)),
            None => owner_words(first),
        };
        let saving = owner_plugin(first)
            .and_then(|p| savings.get(p).cloned())
            .flatten();
        let mut children: Vec<Row> = entries
            .iter()
            .map(|e| self.facet_row(e, &name, false, saving.as_deref()))
            .collect();
        number_duplicates(&mut children);
        let bytes = sum_known(children.iter().filter_map(|r| r.bytes));
        let lines = and_list(&facet_counts(entries));
        let mut detail = Detail {
            what: match plugin {
                Some(_) => format!("{name} · listing lines of the {name} plugin"),
                None => format!("{name} · listing lines from {}", self.origin_phrase(first)),
            },
            brings: match bytes {
                Some(b) => format!("{lines}: {} of your first request.", size(b)),
                None => format!("{lines}; their size is unknown until `harn verify --run`."),
            },
            ..Detail::default()
        };
        let state = match plugin {
            Some(p) => {
                self.action_sentences(p, &mut detail, true, "the plugin");
                if let (Some(text), true) = (&saving, !detail.saves.is_empty()) {
                    detail.saves = format!("Turning it off frees {text}.");
                }
                self.state(p)
            }
            None => {
                detail.action = "Open it (→) and pick one; d turns that one off.".to_string();
                if any_on(&children) {
                    State::On
                } else {
                    State::Fixed
                }
            }
        };
        Row {
            key: format!("owner:{key}"),
            id: plugin.map(|p| p.id.clone()),
            name,
            source,
            kind_word: "owner".to_string(),
            bytes,
            share: self.share(bytes),
            absent: None,
            weight: bytes,
            saving: plugin.and(saving),
            children,
            state,
            detail,
        }
    }

    /// CLAUDE.md with its imports as children, rules, memory files.
    fn file_row(&self, item: &Item) -> Row {
        let mut row = self.leaf(item, kind_word(item.kind), &self.layer_source(item));
        row.name = file_name(item, &self.places);
        row.children = item.children.iter().map(|c| self.file_row(c)).collect();
        let imports = descendants(item);
        if imports > 0 {
            row.name = format!("{} + {}", row.name, count(imports, "import"));
        }
        row.detail = self.detail(item, &file_name(item, &self.places), row.bytes, row.absent);
        row
    }

    fn hook_rows(&self, hooks: &[&Item]) -> Vec<Row> {
        let mut rows: Vec<Row> = hooks
            .iter()
            .map(|h| {
                let mut row = self.leaf(h, "hook", &self.layer_source(h));
                row.name = hook_event(&h.label);
                row.detail = self.detail(h, &row.name, row.bytes, row.absent);
                row
            })
            .collect();
        number_duplicates(&mut rows);
        rows
    }

    /// An MCP server outside a plugin (instructions and tools folded in).
    fn server_row(&self, item: &Item, plugin: Option<&str>) -> Row {
        let mut row = self.leaf(item, "MCP server", &self.layer_source(item));
        row.name = facet_name(&item.label, plugin.unwrap_or(""));
        row.detail = self.detail(item, &row.name, row.bytes, row.absent);
        row
    }

    // ---- sentences ---------------------------------------------------------------------------

    /// The detail of a row for `item`.
    fn detail(&self, item: &Item, name: &str, bytes: Option<u64>, absent: Option<&str>) -> Detail {
        let what = format!("{name} · {}", self.what_phrase(item));
        let brings = match (bytes, absent) {
            (Some(b), _) if listing_entry(item) => format!(
                "Its listing line: {} of your first request; the full text loads when used.",
                size(b)
            ),
            (Some(b), _) => {
                let imports = descendants(item);
                if imports > 0 && item.kind == Kind::Instruction {
                    format!(
                        "With {} it is {} of your first request.",
                        count(imports, "import"),
                        size(b)
                    )
                } else {
                    format!("{} of your first request.", size(b))
                }
            }
            (None, Some(word)) => format!("It costs nothing in the first request ({word})."),
            (None, None) => {
                "Its size is unknown until `harn verify --run` measures a session.".to_string()
            }
        };
        let mut detail = Detail {
            what,
            brings,
            ..Detail::default()
        };
        self.action_sentences(item, &mut detail, listing_entry(item), "it");
        if let Some(note) = self.journaled_note(item) {
            detail.undo = note;
        }
        detail
    }

    /// `skill of the slack plugin`, `instructions in this repository · loads every session`.
    fn what_phrase(&self, item: &Item) -> String {
        let from = self.origin_phrase(item);
        match item.kind {
            Kind::Skill | Kind::Command | Kind::SubAgent => {
                format!("{} from {from}", kind_word(item.kind))
            }
            Kind::Memory => format!(
                "auto-memory for this directory, from {}",
                item.origin
                    .path
                    .as_deref()
                    .map(|p| self.places.path(p.parent().unwrap_or(p)))
                    .unwrap_or_else(|| from.clone())
            ),
            Kind::Instruction => match item.origin.layer {
                Layer::User => "your instructions for every project".to_string(),
                Layer::Managed => "instructions from your organisation".to_string(),
                _ => match item.mode {
                    Mode::OnDemand => {
                        "rule of this repository, read when a matching file is".to_string()
                    }
                    _ => "instructions of this repository".to_string(),
                },
            },
            Kind::Hook => {
                let file = item
                    .origin
                    .path
                    .as_deref()
                    .map(|p| self.places.path(p))
                    .unwrap_or_default();
                let when = match item.mode {
                    Mode::Runtime => "runs during the session",
                    _ => "its output enters the first request",
                };
                match owner_plugin(item) {
                    Some(p) => format!("hook of the {} plugin · {when}", plugin_name(p)),
                    None => format!("hook in {file} · {when}"),
                }
            }
            Kind::McpServer => format!("MCP server from {from}"),
            Kind::BuiltinTool => "tool built into Claude Code".to_string(),
            Kind::Plugin => format!("plugin from {}", market_phrase(&item.label)),
            _ => match item.origin.layer {
                Layer::Builtin => "part of Claude Code itself".to_string(),
                _ => format!("{} from {from}", kind_word(item.kind)),
            },
        }
    }

    /// `action`, `saves`, `undo` for `d` on `item`. `listing` = turning it off removes listing
    /// lines; `it` names the thing in the sentence.
    fn action_sentences(&self, item: &Item, detail: &mut Detail, listing: bool, it: &str) {
        let places = &self.places;
        match (local_knob(item), any_knob(item)) {
            (Some(knob), _) => {
                let file = knob_file(knob, places).unwrap_or_default();
                let verb = self.verb(item, knob);
                let file = if file.is_empty() {
                    String::new()
                } else {
                    format!(" ({file})")
                };
                detail.action = format!("d {verb} for this project only{file}.");
                if it != "it" {
                    detail.action = detail.action.replacen(" it ", &format!(" {it} "), 1);
                }
                detail.saves = self.saves(item, listing);
                detail.undo = "Reversible with u.".to_string();
            }
            (None, Some((_, scope))) => {
                detail.action = format!(
                    "Its only switch is {}: `harn off --scope {scope}` changes it there.",
                    scope_words(scope)
                );
            }
            (None, None) => {
                detail.action = format!("It cannot be turned off: {}.", fixed_reason(item));
            }
        }
    }

    /// `turns it off`, `turns off the whole slack plugin`, `turns off every hook`.
    fn verb(&self, item: &Item, knob: &Knob) -> String {
        let Knob::Setting { pointer, .. } = knob else {
            return match knob {
                Knob::Quarantine { .. } => "moves its file aside".to_string(),
                _ => "turns it off".to_string(),
            };
        };
        if pointer.starts_with("/enabledPlugins/") && item.kind != Kind::Plugin {
            let words = pointer_words(pointer, &self.places);
            let p = words.get(1).cloned().unwrap_or_default();
            return format!("turns off the whole {p} plugin");
        }
        match pointer.as_str() {
            "/disableAllHooks" => "turns off every hook".to_string(),
            "/autoMemoryEnabled" => "turns auto-memory off".to_string(),
            "/claudeMdExcludes/-" => "leaves it out".to_string(),
            "/includeGitInstructions" => "leaves it out".to_string(),
            _ if pointer.starts_with("/skillOverrides/") => "hides it".to_string(),
            _ if pointer.starts_with("/permissions/deny") => "removes it".to_string(),
            _ => "turns it off".to_string(),
        }
    }

    fn saves(&self, item: &Item, listing: bool) -> String {
        let Some(freed) = &item.freed else {
            return String::new();
        };
        let which = if item.kind == Kind::SubAgent {
            "agent"
        } else {
            "skill"
        };
        match (freed.known, freed.delta_unknown) {
            (0, true) if listing => {
                format!("The {which} listing shrinks; how much needs `harn verify --run`.")
            }
            (known, true) if listing => format!(
                "That frees {}; the {which} listing may shrink more (unmeasured).",
                size(known)
            ),
            (0, true) => "How much that frees needs `harn verify --run`.".to_string(),
            (known, true) => format!(
                "That frees {} and a part only `harn verify --run` can measure.",
                size(known)
            ),
            (0, false) => "That frees nothing in the first request.".to_string(),
            (known, false) => format!("That frees {}.", size(known)),
        }
    }

    // ---- off rows ----------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn place_off(
        &self,
        off: &OffItem,
        plugins: &mut Vec<Row>,
        skills: &mut Vec<Row>,
        agents: &mut Vec<Row>,
        memory: &mut Vec<Row>,
        files: &mut Vec<Row>,
        hooks: &mut Vec<Row>,
        servers: &mut Vec<Row>,
        tools: &mut Vec<Row>,
        other: &mut Vec<Row>,
    ) {
        let mut parts = off.item_id.splitn(4, '/');
        let (kind, layer) = (parts.nth(1).unwrap_or_default(), parts.next());
        let kind = Kind::ALL.iter().copied().find(|k| k.as_str() == kind);
        let (display, source) = id_name(&off.item_id, &self.places);
        // The hooks switch is one for all: this row says the others went with it.
        let all_hooks = kind == Some(Kind::Hook) && off.pointer.as_deref() == Some(ALL_HOOKS);
        let detail = if all_hooks {
            Detail {
                what: format!(
                    "{display} · turned off with every other hook · u undoes · takes effect next \
                     session"
                ),
                brings: "Claude Code turns hooks off all at once: harn turned every hook off \
                         here, and none runs until they are turned back on."
                    .to_string(),
                action: "o turns them all back on (the change is reversed byte for byte)."
                    .to_string(),
                ..Detail::default()
            }
        } else {
            Detail {
                what: format!("{display} · turned off · u undoes · takes effect next session"),
                brings: "harn turned it off here; it stays off until it is turned back on."
                    .to_string(),
                action: "o turns it back on (the change is reversed byte for byte).".to_string(),
                ..Detail::default()
            }
        };
        let row = Row {
            key: off.item_id.clone(),
            id: Some(off.item_id.clone()),
            name: display.clone(),
            source,
            kind_word: kind.map(kind_word).unwrap_or("item").to_string(),
            bytes: None,
            share: None,
            absent: Some(if all_hooks {
                "off, with every other hook"
            } else {
                "off"
            }),
            weight: None,
            saving: None,
            children: Vec::new(),
            state: State::Off,
            detail,
        };
        let into_owner = |owners: &mut Vec<Row>, row: Row| {
            let key = format!("owner:off:{}", layer.unwrap_or_default());
            match owners.iter_mut().find(|o| o.key == key) {
                Some(owner) => owner.children.push(row),
                None => owners.push(Row {
                    key,
                    id: None,
                    name: "turned off here".to_string(),
                    source: String::new(),
                    kind_word: "owner".to_string(),
                    bytes: None,
                    share: None,
                    absent: Some("off"),
                    weight: None,
                    saving: None,
                    children: vec![row],
                    state: State::Off,
                    detail: Detail {
                        what: "turned off here · listing lines harn removed".to_string(),
                        action: "Open it (→); o on a line turns it back on.".to_string(),
                        ..Detail::default()
                    },
                }),
            }
        };
        match kind {
            Some(Kind::Plugin) => plugins.push(row),
            Some(Kind::Skill | Kind::Command) => into_owner(skills, row),
            Some(Kind::SubAgent) => into_owner(agents, row),
            Some(Kind::Memory) => memory.push(row),
            Some(Kind::Instruction) => files.push(row),
            Some(Kind::Hook) => hooks.push(row),
            Some(Kind::McpServer) => servers.push(row),
            Some(Kind::BuiltinTool) => tools.push(row),
            _ => other.push(row),
        }
    }
}

/// Name and source of an item known only by its id (`<agent>/<kind>/<layer>/<name>`): a plugin
/// without its marketplace, a file home-relative, a hook by its event, else the name's last
/// `:` part.
pub fn id_name(id: &str, places: &Places) -> (String, String) {
    let mut parts = id.splitn(4, '/');
    let (_, kind, layer, name) = (parts.next(), parts.next(), parts.next(), parts.next());
    let (kind, layer, name) = (
        kind.unwrap_or_default(),
        layer.unwrap_or_default().to_string(),
        name.unwrap_or(id),
    );
    match Kind::ALL.iter().copied().find(|k| k.as_str() == kind) {
        Some(Kind::Plugin) => (plugin_name(name).to_string(), market_word(name)),
        Some(Kind::Instruction | Kind::Memory) => (places.path(Path::new(name)), layer),
        Some(Kind::Hook) => (hook_event_from_pointer(name), layer),
        _ => (name.rsplit(':').next().unwrap_or(name).to_string(), layer),
    }
}

/// The word a group (or a row gathering others) shows without a size: the one all its rows
/// share, `none up front` when they differ; None when some row is unmeasured.
fn common_absent(bytes: Option<u64>, rows: &[Row]) -> Option<&'static str> {
    if bytes.is_some() || rows.is_empty() || rows.iter().any(|r| r.absent.is_none()) {
        return None;
    }
    // Rows harn turned off say so themselves; the others decide the word.
    let on: Vec<&Row> = rows.iter().filter(|r| r.state != State::Off).collect();
    let Some(first) = on.first().map(|r| r.absent) else {
        return Some("off");
    };
    if on.iter().all(|r| r.absent == first) {
        first
    } else {
        Some("none up front")
    }
}

/// Why `item` has no switch at all, in words (`it is part of Claude Code`).
fn fixed_reason(item: &Item) -> String {
    item.knobs
        .iter()
        .find_map(|k| match k {
            Knob::None { reason } => Some(reason_words(reason)),
            _ => None,
        })
        .unwrap_or_else(|| "harn knows no switch for it".to_string())
}

/// Why `d` cannot turn `item` off for this project, to follow its name: `cannot be turned off:
/// it is part of Claude Code`, or where its only switch is.
pub fn refusal(item: &Item) -> String {
    match any_knob(item) {
        Some((_, scope)) => format!(
            "has no switch for this project only; its only switch is {}: `harn off --scope \
             {scope}` changes it there",
            scope_words(scope)
        ),
        None => format!("cannot be turned off: {}", fixed_reason(item)),
    }
}

/// Rows harn turned off, at any depth.
fn off_count(rows: &[Row]) -> usize {
    rows.iter()
        .map(|r| usize::from(r.state == State::Off && r.id.is_some()) + off_count(&r.children))
        .sum()
}

/// Items under `item`, at any depth.
fn descendants(item: &Item) -> usize {
    item.children.iter().map(|c| 1 + descendants(c)).sum()
}

/// Some row (or a row under it) has a switch `d` or `o` can use.
fn any_on(rows: &[Row]) -> bool {
    rows.iter()
        .any(|r| (r.state != State::Fixed && r.id.is_some()) || any_on(&r.children))
}

/// `over cap: 146 of 216 descriptions dropped` → (146, 216).
fn cut_count(signal: &str) -> Option<(u64, u64)> {
    let rest = signal.strip_prefix("over cap: ")?;
    let (dropped, rest) = rest.split_once(" of ")?;
    let total = rest.split_whitespace().next()?;
    Some((dropped.trim().parse().ok()?, total.parse().ok()?))
}

/// Gathers the parts of every plugin in `keys`, skipping into the two listings; a claimed part
/// keeps what is under it (an MCP server's instructions).
fn collect_facets<'a>(
    items: &'a [Item],
    keys: &HashSet<&'a str>,
    facets: &mut HashMap<&'a str, Vec<&'a Item>>,
) {
    for item in items {
        match item.kind {
            Kind::Plugin => {}
            Kind::SkillListing | Kind::AgentRoster => collect_facets(&item.children, keys, facets),
            _ => match owner_plugin(item) {
                Some(p) if keys.contains(p) => {
                    let key = *keys.get(p).expect("checked");
                    facets.entry(key).or_default().push(item);
                }
                _ => {}
            },
        }
    }
}

/// `8 skills`, `5 commands`, `1 MCP server` of `items`, in that order, zero counts left out.
fn facet_counts(items: &[&Item]) -> Vec<String> {
    let order = [
        Kind::Skill,
        Kind::Command,
        Kind::SubAgent,
        Kind::McpServer,
        Kind::Hook,
    ];
    let mut out = Vec::new();
    for kind in order {
        let n = items.iter().filter(|i| i.kind == kind).count();
        if n > 0 {
            out.push(count(n, kind_word(kind)));
        }
    }
    let rest = items.iter().filter(|i| !order.contains(&i.kind)).count();
    if rest > 0 {
        out.push(count(rest, "other part"));
    }
    out
}

/// The owner of a listing entry: `plugin:<name@m>`, else its layer (claude.ai entries by their
/// `prefix:`).
fn owner_key(entry: &Item) -> String {
    match owner_plugin(entry) {
        Some(p) => format!("plugin:{p}"),
        None => match entry.origin.layer {
            Layer::ClaudeAi => match entry.label.split_once(':') {
                Some((prefix, _)) => format!("claudeai:{prefix}"),
                None => "claudeai".to_string(),
            },
            layer => layer.to_string(),
        },
    }
}

/// Name and source of a non-plugin owner row.
fn owner_words(entry: &Item) -> (String, String) {
    match entry.origin.layer {
        Layer::Project => ("this repository".to_string(), "project".to_string()),
        Layer::Local => ("this project".to_string(), "project".to_string()),
        Layer::User => ("you".to_string(), "~/.claude".to_string()),
        Layer::ClaudeAi => (
            entry
                .label
                .split_once(':')
                .map(|(p, _)| p.to_string())
                .unwrap_or_else(|| "claude.ai".to_string()),
            "claude.ai".to_string(),
        ),
        Layer::Builtin => ("Claude Code".to_string(), "built-in".to_string()),
        Layer::Managed => ("your organisation".to_string(), "managed".to_string()),
        other => (other.to_string(), String::new()),
    }
}

/// `block-kit` from `slack:block-kit`, `slack` from `plugin:slack:slack`; `· instructions` and
/// other ` · ` tails dropped.
fn facet_name(label: &str, plugin: &str) -> String {
    let label = label.split(" · ").next().unwrap_or(label);
    if let Some(rest) = label.strip_prefix("plugin:") {
        return rest.rsplit(':').next().unwrap_or(rest).to_string();
    }
    if !plugin.is_empty() {
        if let Some(rest) = label.strip_prefix(&format!("{plugin}:")) {
            return rest.to_string();
        }
    }
    label.to_string()
}

/// `SessionStart hook` from `SessionStart hook [matcher] · 1 cmd · …`.
fn hook_event(label: &str) -> String {
    match label.split_once(" hook") {
        Some((event, _)) => format!("{event} hook"),
        None => label.split(" · ").next().unwrap_or(label).to_string(),
    }
}

/// `SessionStart hook` from `<file>#/hooks/SessionStart/0`.
fn hook_event_from_pointer(name: &str) -> String {
    name.split("#/hooks/")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .map(|event| format!("{event} hook"))
        .unwrap_or_else(|| "hook".to_string())
}

/// `MEMORY.md (auto-memory)`, `CLAUDE.md`, `rules/a.md`, `~/.claude/CLAUDE.md`.
fn file_name(item: &Item, places: &Places) -> String {
    if item.kind == Kind::Memory {
        let file = item
            .origin
            .path
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "MEMORY.md".to_string());
        return if item.label.ends_with("(auto-memory)") {
            format!("{file} (auto-memory)")
        } else {
            file
        };
    }
    places.text(&item.label)
}

/// `v1.4.1`, `commit f359d43`: the last segment of the install path.
fn plugin_version(plugin: &Item) -> Option<String> {
    let last = plugin
        .origin
        .path
        .as_deref()?
        .file_name()?
        .to_string_lossy();
    if last.chars().next()?.is_ascii_digit() && last.contains('.') {
        Some(format!("v{last}"))
    } else if last.len() >= 7 && last.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("commit {}", &last[..7]))
    } else {
        None
    }
}

/// Where `enabledPlugins` turns the plugin on, in words, from the first link of its chain.
fn enabled_scope(plugin: &Item, places: &Places) -> Option<String> {
    let first = plugin.origin.why.first()?;
    if first.starts_with("synced from claude.ai") {
        return Some("through claude.ai".to_string());
    }
    let (file, rest) = first.split_once('#')?;
    let pointer = rest.split([' ', '(']).next().unwrap_or_default();
    Some(setting_scope(Path::new(file), pointer, places).sentence())
}

/// Whom a setting applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// What the file alone says ([`file_scope`]): `for every project`, `by this repository`,
    /// `for this project only`, `by your organisation`.
    File(&'static str),
    /// A key under `projects/<dir>` (`~/.claude.json`): that project only, whatever the file.
    Project(String),
}

impl Reach {
    /// After a verb: `for every project`, `for this project only`, `for ~/other/dir only`.
    pub fn sentence(&self) -> String {
        match self {
            Reach::File(words) => words.to_string(),
            Reach::Project(dir) => format!("for {dir} only"),
        }
    }

    /// In parentheses after a setting: `every project`, `this project`, `~/other/dir`.
    pub fn short(&self) -> String {
        match self {
            Reach::File(words) => words.strip_prefix("for ").unwrap_or(words).to_string(),
            Reach::Project(dir) => dir.clone(),
        }
    }
}

/// Whom the setting at `pointer` in `file` applies to: the pointer first (a key under
/// `projects/<dir>` is that project's, `this project` when it is the target dir), the file only
/// for any other pointer.
pub fn setting_scope(file: &Path, pointer: &str, places: &Places) -> Reach {
    let words = pointer_words(pointer, places);
    match words.as_slice() {
        [first, dir, ..] if first == "projects" => Reach::Project(dir.clone()),
        _ => Reach::File(file_scope(file, places)),
    }
}

/// `for every project` (`~/.claude/settings.json`), `by this repository`
/// (`.claude/settings.json`), `for this project only` (`.claude/settings.local.json`).
pub fn file_scope(file: &Path, places: &Places) -> &'static str {
    if file.starts_with(&places.dir) {
        if file.file_name().is_some_and(|n| n == "settings.local.json") {
            "for this project only"
        } else {
            "by this repository"
        }
    } else if places
        .home
        .as_deref()
        .is_some_and(|home| file.starts_with(home))
    {
        "for every project"
    } else {
        "by your organisation"
    }
}

/// Same names under one parent get `2`, `3`, …: `SessionStart hook`, `SessionStart hook 2`.
fn number_duplicates(rows: &mut [Row]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for row in rows.iter_mut() {
        let n = seen.entry(row.name.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            row.name = format!("{} {n}", row.name);
        }
    }
}

/// First letter up, a full stop at the end.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

/// Bytes desc, unknown last, name asc; or name asc. Recursive.
fn sort_rows(rows: &mut [Row], sort: Sort) {
    match sort {
        // Sized rows by size; the others by what turning them off frees (a plugin whose bytes
        // are all listing lines), unmeasured before those that cost nothing up front.
        Sort::Size => rows.sort_by(|a, b| {
            b.bytes
                .is_some()
                .cmp(&a.bytes.is_some())
                .then(b.bytes.cmp(&a.bytes))
                .then(b.weight.is_some().cmp(&a.weight.is_some()))
                .then(b.weight.cmp(&a.weight))
                .then_with(|| (a.state == State::Off).cmp(&(b.state == State::Off)))
                .then_with(|| a.absent.is_some().cmp(&b.absent.is_some()))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        Sort::Name => rows.sort_by_key(|r| r.name.to_lowercase()),
    }
    for row in rows.iter_mut() {
        sort_rows(&mut row.children, sort);
    }
}

// ---- cards -----------------------------------------------------------------------------------

/// The `d` card: one plan, worded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffCard {
    pub title: String,
    /// A sentence before Writes when the switch does more than the row (every hook at once).
    pub lead: Option<String>,
    pub writes: Vec<String>,
    /// `8 skills, 5 commands, 1 MCP server (next session)`.
    pub leaves: String,
    /// Each thing that leaves: `block-kit (skill)`.
    pub leave_list: Vec<String>,
    pub saves: String,
    pub undo: String,
    pub notes: Vec<String>,
}

/// The `d` card for `plan` on `item` (shown as `name`); `saving` = what turning the whole plugin
/// off frees, in words, when the plan does that.
pub fn off_card(
    plan: &ActionPlan,
    tree: &Tree,
    item: &Item,
    name: &str,
    arrow: &str,
    saving: Option<&str>,
) -> OffCard {
    let places = Places::of(tree);
    let scope = plan
        .ops
        .first()
        .and_then(|op| knob_scope(&op.knob))
        .unwrap_or(Scope::Local);
    let writes = plan
        .ops
        .iter()
        .map(|op| writes_line(&op.knob, &places, arrow))
        .collect();
    let mut gone: Vec<(String, Kind)> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    for effect in &plan.side_effects {
        let Some((label, kind, leaves)) = parse_effect(effect) else {
            continue;
        };
        let shown = human_label(tree, &label, kind, &places);
        if leaves {
            gone.push((shown, kind));
        } else {
            added.push(format!("{shown} ({}, appears)", kind_word(kind)));
        }
    }
    let leaves = if gone.is_empty() {
        format!("just this {} (next session)", kind_word(item.kind))
    } else {
        let mut counts: Vec<(Kind, usize)> = Vec::new();
        for (_, kind) in &gone {
            match counts.iter_mut().find(|(k, _)| k == kind) {
                Some((_, n)) => *n += 1,
                None => counts.push((*kind, 1)),
            }
        }
        let order = |k: &Kind| match k {
            Kind::Skill => 0,
            Kind::Command => 1,
            Kind::SubAgent => 2,
            Kind::McpServer => 3,
            Kind::Hook => 4,
            _ => 5,
        };
        counts.sort_by_key(|(k, _)| order(k));
        let parts: Vec<String> = counts
            .iter()
            .map(|(k, n)| count(*n, kind_word(*k)))
            .collect();
        format!("{} (next session)", parts.join(", "))
    };
    // `UserPromptSubmit (hook)`: the kind word once.
    let mut leave_list: Vec<String> = gone
        .iter()
        .map(|(n, k)| {
            let word = kind_word(*k);
            let n = n.strip_suffix(&format!(" {word}")).unwrap_or(n);
            format!("{n} ({word})")
        })
        .collect();
    leave_list.extend(added);
    let listing = gone
        .iter()
        .any(|(_, k)| matches!(k, Kind::Skill | Kind::Command | Kind::SubAgent))
        || listing_entry(item);
    let freed = &plan.freed;
    let saves = match (freed.known, freed.delta_unknown) {
        _ if saving.is_some() => saving.unwrap_or_default().to_string(),
        (0, true) => "an amount only `harn verify --run` can measure".to_string(),
        (known, true) if listing => format!(
            "{} now; the {} listing shrinks too (unmeasured)",
            size(known),
            if item.kind == Kind::SubAgent {
                "agent"
            } else {
                "skill"
            }
        ),
        (known, true) => format!("{} now, plus an unmeasured part", size(known)),
        (0, false) => "nothing in the first request".to_string(),
        (known, false) => format!("{} of the first request", size(known)),
    };
    let all_hooks = plan
        .ops
        .iter()
        .any(|op| matches!(&op.knob, Knob::Setting { pointer, .. } if pointer == ALL_HOOKS));
    OffCard {
        title: if all_hooks {
            format!("Turn off all hooks for {}", scope_short(scope))
        } else {
            format!("Turn off {name} for {}", scope_short(scope))
        },
        lead: all_hooks.then(|| {
            format!("{name} has no switch of its own; Claude Code turns hooks off all at once.")
        }),
        writes,
        leaves,
        leave_list,
        saves,
        undo: "u here, or `harn undo` later — the file is restored byte for byte".to_string(),
        notes: plan
            .warnings
            .iter()
            .map(|w| sentence(&places.text(w)))
            .collect(),
    }
}

/// `label (kind) leaves[ with N children]` / `label (kind) appears` → (label, kind, leaves).
fn parse_effect(effect: &str) -> Option<(String, Kind, bool)> {
    let (head, leaves) = match effect.rfind(") leaves") {
        Some(at) => (&effect[..at], true),
        None => (&effect[..effect.rfind(") appears")?], false),
    };
    let (label, kind) = head.rsplit_once(" (")?;
    let kind = Kind::ALL.iter().copied().find(|k| k.as_str() == kind)?;
    Some((label.to_string(), kind, leaves))
}

/// The name a row would show for the item with `label` and `kind` in `tree`.
fn human_label(tree: &Tree, label: &str, kind: Kind, places: &Places) -> String {
    let item = model::walk(tree).find(|i| i.label == label && i.kind == kind);
    match (kind, item) {
        (Kind::Plugin, _) => plugin_name(label).to_string(),
        (Kind::Hook, _) => hook_event(label),
        (Kind::Instruction | Kind::Memory, Some(item)) => file_name(item, places),
        (_, Some(item)) => {
            let plugin = owner_plugin(item).map(plugin_name).unwrap_or("");
            facet_name(label, plugin)
        }
        (_, None) => facet_name(&places.text(label), ""),
    }
}

/// The `w` card: where an item comes from, in numbered lines, then the switches by scope.
pub fn where_card(item: &Item, name: &str, places: &Places) -> (String, Vec<String>) {
    let mut lines = Vec::new();
    for link in &item.origin.why {
        if link.contains("freed via") || link.starts_with("cost carried by") {
            continue;
        }
        lines.push(where_link(link, places));
    }
    if lines.is_empty() {
        if let Some(path) = &item.origin.path {
            lines.push(format!("file: {}", places.path(path)));
        }
    }
    let mut numbered: Vec<String> = lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| format!("{}  {l}", i + 1))
        .collect();
    let mut switches: Vec<String> = Vec::new();
    for knob in &item.knobs {
        let text = match knob {
            Knob::None { reason } => format!("none — {}", places.text(&reason_words(reason))),
            other => {
                let scope = knob_scope(other).unwrap_or(Scope::Local);
                let file = knob_file(other, places).unwrap_or_default();
                format!("{} ({file})", scope_short(scope))
            }
        };
        if !switches.contains(&text) {
            switches.push(text);
        }
    }
    if !switches.is_empty() {
        numbered.push(format!("Switches  {}", switches.join(" · ")));
    }
    (format!("Where {name} comes from"), numbered)
}

/// One admission link in words: `file › key › key = value   (every project)`, `installed from
/// m, version v`, `files: <path>`.
fn where_link(link: &str, places: &Places) -> String {
    // `<installed_plugins.json>: name@m version (user scope)`
    if let Some((_, rest)) = link.split_once("installed_plugins.json: ") {
        let mut words = rest.split_whitespace();
        let key = words.next().unwrap_or_default();
        let version = words.next().unwrap_or_default();
        let market = key.split_once('@').map(|(_, m)| m).unwrap_or(key);
        return format!("installed from {market}, version {version}");
    }
    if let Some(rest) = link.strip_prefix("contributes: ") {
        let parts: Vec<String> = rest
            .split(" · ")
            .filter_map(|p| match p.split_once(' ') {
                Some(("0", _)) => None,
                Some(("1", word)) => Some(format!("1 {}", word.trim_end_matches('s'))),
                _ => Some(p.to_string()),
            })
            .collect();
        if parts.is_empty() {
            return "contributes nothing that loads".to_string();
        }
        return format!("contributes {}", parts.join(" · "));
    }
    // `[words ]<file>#/a/b = value` or `<file>#/a/b (http)`
    if let Some((head, rest)) = link.split_once("#/") {
        let start = head.find(['/', '~']).unwrap_or(0);
        let (prefix, file) = head.split_at(start);
        let (pointer, tail) = match rest.find([' ', '(']) {
            Some(at) => (&rest[..at], rest[at..].trim_start()),
            None => (rest, ""),
        };
        let words = pointer_words(pointer, places).join(" › ");
        let scope = setting_scope(Path::new(file), pointer, places).short();
        let tail = places.text(tail);
        let tail = if tail.is_empty() {
            String::new()
        } else {
            format!(" {tail}")
        };
        return format!(
            "{prefix}{} › {words}{tail}   ({scope})",
            places.path(Path::new(file))
        );
    }
    let text = places.text(link);
    if Path::new(link).is_absolute() && !link.contains(' ') {
        return format!("files: {text}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_has_one_decimal_under_10_kb_and_none_above() {
        assert_eq!(size(3712), "3.7 KB");
        assert_eq!(size(738), "0.7 KB");
        assert_eq!(size(9_949), "9.9 KB");
        assert_eq!(size(9_950), "10 KB");
        assert_eq!(size(87_352), "87 KB");
        assert_eq!(size(142_288), "142 KB");
        assert_eq!(size(999_499), "999 KB");
        assert_eq!(size(999_500), "1.0 MB");
    }

    #[test]
    fn percent_rounds_down_and_guards_zero() {
        assert_eq!(percent(Some(87_352), 142_288), Some(61));
        assert_eq!(
            percent(Some(995), 1000),
            Some(99),
            "down: parts never pass 100"
        );
        assert_eq!(percent(Some(1), 142_288), Some(0));
        assert_eq!(percent(Some(1), 0), None);
        assert_eq!(percent(None, 10), None);
    }

    fn places() -> Places {
        Places {
            dir: PathBuf::from("/home/me/2lab.ai/zbrain"),
            home: Some(PathBuf::from("/home/me")),
        }
    }

    #[test]
    fn places_show_project_paths_relative_and_home_with_a_tilde() {
        let p = places();
        assert_eq!(
            p.path(Path::new(
                "/home/me/2lab.ai/zbrain/.claude/settings.local.json"
            )),
            ".claude/settings.local.json"
        );
        assert_eq!(
            p.path(Path::new("/home/me/.claude/CLAUDE.md")),
            "~/.claude/CLAUDE.md"
        );
        assert_eq!(
            p.text("memory dir slug of /home/me/2lab.ai/zbrain"),
            "memory dir slug of ~/2lab.ai/zbrain"
        );
        assert_eq!(
            p.text("/home/me/.claude/settings.json#/hooks/Stop/0 and /home/meow/x"),
            "~/.claude/settings.json#/hooks/Stop/0 and /home/meow/x"
        );
        assert_eq!(
            p.text("/home/me/2lab.ai/zbrain-other/x"),
            "~/2lab.ai/zbrain-other/x"
        );
    }

    #[test]
    fn pointers_lose_the_marketplace_and_the_escaped_target_dir() {
        let p = places();
        assert_eq!(
            pointer_words("/enabledPlugins/slack@claude-plugins-official", &p),
            ["enabledPlugins", "slack"]
        );
        assert_eq!(
            pointer_words(
                "/projects/~1home~1me~12lab.ai~1zbrain/disabledMcpServers/-",
                &p
            ),
            ["projects", "this project", "disabledMcpServers", "-"]
        );
        let knob = Knob::Setting {
            file: PathBuf::from("/home/me/2lab.ai/zbrain/.claude/settings.local.json"),
            pointer: "/enabledPlugins/slack@claude-plugins-official".to_string(),
            value: serde_json::Value::Bool(false),
            scope: Scope::Local,
            note: None,
        };
        assert_eq!(
            writes_line(&knob, &p, "→"),
            ".claude/settings.local.json → enabledPlugins.slack = false"
        );
        let deny = Knob::Setting {
            file: PathBuf::from("/home/me/2lab.ai/zbrain/.claude/settings.local.json"),
            pointer: "/permissions/deny/-".to_string(),
            value: serde_json::Value::String("Bash".to_string()),
            scope: Scope::Local,
            note: None,
        };
        assert_eq!(
            writes_line(&deny, &p, "->"),
            ".claude/settings.local.json -> permissions.deny += Bash"
        );
    }

    #[test]
    fn a_setting_reaches_whom_its_pointer_says_then_whom_its_file_says() {
        let p = places();
        let home = Path::new("/home/me/.claude.json");
        let user = Path::new("/home/me/.claude/settings.json");
        let local = Path::new("/home/me/2lab.ai/zbrain/.claude/settings.local.json");
        let scope = |file, pointer| setting_scope(file, pointer, &p);
        assert_eq!(
            scope(user, "/enabledPlugins/slack@claude-plugins-official").sentence(),
            "for every project"
        );
        assert_eq!(
            scope(local, "/enabledPlugins/x").sentence(),
            "for this project only"
        );
        let this = scope(
            home,
            "/projects/~1home~1me~12lab.ai~1zbrain/disabledMcpServers/-",
        );
        assert_eq!(
            (this.sentence(), this.short()),
            ("for this project only".into(), "this project".into())
        );
        let other = scope(home, "/projects/~1home~1me~1other~1app/disabledMcpServers");
        assert_eq!(
            (other.sentence(), other.short()),
            (
                "for ~/other/app only".to_string(),
                "~/other/app".to_string()
            )
        );
        // Only a key under `projects` is a project's; anything else in the file is the file's.
        assert_eq!(scope(home, "/mcpServers/x").short(), "every project");
    }

    #[test]
    fn a_target_outside_home_keeps_absolute_paths_absolute() {
        let p = Places {
            dir: PathBuf::from("/srv/app"),
            home: Some(PathBuf::from("/home/me")),
        };
        assert_eq!(p.path(Path::new("/srv/other/f.md")), "/srv/other/f.md");
        assert_eq!(
            p.path(Path::new("/etc/claude/managed.json")),
            "/etc/claude/managed.json"
        );
        assert_eq!(
            p.path(Path::new("/srv/app/.claude/settings.json")),
            ".claude/settings.json"
        );
        assert_eq!(
            p.path(Path::new("/home/me/.claude/CLAUDE.md")),
            "~/.claude/CLAUDE.md"
        );
        assert_eq!(p.text("read /srv/other/f.md"), "read /srv/other/f.md");
        assert!(!p.path(Path::new("/srv/other/f.md")).contains('~'));
    }

    #[test]
    fn where_links_read_as_words() {
        let p = places();
        assert_eq!(
            where_link(
                "/home/me/.claude/settings.json#/enabledPlugins/slack@claude-plugins-official = true",
                &p
            ),
            "~/.claude/settings.json › enabledPlugins › slack = true   (every project)"
        );
        assert_eq!(
            where_link(
                "/home/me/.claude/plugins/installed_plugins.json: slack@claude-plugins-official 1.4.1 (user scope)",
                &p
            ),
            "installed from claude-plugins-official, version 1.4.1"
        );
        assert_eq!(
            where_link(
                "disabled by /home/me/.claude.json#/projects/~1home~1me~12lab.ai~1zbrain/disabledMcpServers",
                &p
            ),
            "disabled by ~/.claude.json › projects › this project › disabledMcpServers   (this project)"
        );
        // Another project's key in the same file is that project's, never every project's.
        assert_eq!(
            where_link(
                "disabled by /home/me/.claude.json#/projects/~1home~1me~1other~1app/disabledMcpServers",
                &p
            ),
            "disabled by ~/.claude.json › projects › ~/other/app › disabledMcpServers   (~/other/app)"
        );
        assert_eq!(
            where_link("/home/me/.claude/plugins/cache/x/slack/1.4.1", &p),
            "files: ~/.claude/plugins/cache/x/slack/1.4.1"
        );
        assert_eq!(
            where_link(
                "contributes: 8 skills · 5 commands · 0 agents · 0 hooks · 1 MCP servers",
                &p
            ),
            "contributes 8 skills · 5 commands · 1 MCP server"
        );
        assert_eq!(
            where_link(
                "contributes: 0 skills · 0 commands · 0 agents · 0 hooks · 0 MCP servers",
                &p
            ),
            "contributes nothing that loads"
        );
    }

    const SLACK: &str = "slack@claude-plugins-official";
    const ZWORKFLOW: &str = "zworkflow@soma-work";

    fn item(
        id: &str,
        label: &str,
        kind: Kind,
        plugin: Option<&str>,
        layer: Layer,
        bytes: Option<u64>,
    ) -> Item {
        let owner = plugin.unwrap_or(SLACK);
        Item {
            id: format!("claude-code/{kind}/{layer}/{id}"),
            label: label.to_string(),
            kind,
            origin: crate::model::Origin {
                layer,
                path: None,
                setting: None,
                plugin: plugin.map(str::to_string),
                why: vec![format!(
                    "/home/me/.claude/settings.json#/enabledPlugins/{owner} = true"
                )],
            },
            mode: match kind {
                Kind::Skill | Kind::SubAgent | Kind::SkillListing | Kind::AgentRoster => {
                    Mode::Advertised
                }
                Kind::Hook => Mode::Runtime,
                _ => Mode::Always,
            },
            cost: crate::model::Cost {
                predicted: bytes,
                not_applicable: matches!(kind, Kind::Plugin | Kind::Hook),
                ..Default::default()
            },
            knobs: vec![Knob::Setting {
                file: PathBuf::from("/home/me/2lab.ai/zbrain/.claude/settings.local.json"),
                pointer: format!("/enabledPlugins/{owner}"),
                value: serde_json::Value::Bool(false),
                scope: Scope::Local,
                note: None,
            }],
            freed: Some(crate::model::Freed {
                known: 3400,
                delta_unknown: true,
                is_estimate: false,
            }),
            signal: None,
            last_used: None,
            children: Vec::new(),
        }
    }

    /// slack: its MCP server (4.0 KB) and its lines in both listings (2.0 + 1.0 KB); zworkflow:
    /// 32 KB of listing lines and a hook that runs later; a user skill, memory, a CLAUDE.md.
    fn synthetic() -> Tree {
        let (s, z, p) = (Some(SLACK), Some(ZWORKFLOW), Layer::Plugin);
        let mut listing = item(
            "skills",
            "skill listing",
            Kind::SkillListing,
            None,
            Layer::Builtin,
            Some(22_700),
        );
        listing.children = vec![
            item(
                "slack:block-kit",
                "slack:block-kit",
                Kind::Skill,
                s,
                p,
                Some(2000),
            ),
            item(
                "zworkflow:autoz",
                "zworkflow:autoz",
                Kind::Skill,
                z,
                p,
                Some(20_000),
            ),
            item("mine", "mine", Kind::Skill, None, Layer::User, Some(700)),
        ];
        let mut roster = item(
            "agents",
            "agent roster",
            Kind::AgentRoster,
            None,
            Layer::Builtin,
            Some(13_000),
        );
        roster.children = vec![
            item(
                "slack:helper",
                "slack:helper",
                Kind::SubAgent,
                s,
                p,
                Some(1000),
            ),
            item(
                "zworkflow:critic",
                "zworkflow:critic",
                Kind::SubAgent,
                z,
                p,
                Some(12_000),
            ),
        ];
        let plugin = |key: &str, version: &str| {
            let mut i = item(key, key, Kind::Plugin, Some(key), p, None);
            i.origin.path = Some(PathBuf::from(format!(
                "/home/me/.claude/plugins/cache/x/{version}"
            )));
            i
        };
        let items = vec![
            plugin(SLACK, "1.4.1"),
            plugin(ZWORKFLOW, "1.4.1"),
            item(
                "plugin:slack:slack",
                "plugin:slack:slack",
                Kind::McpServer,
                s,
                p,
                Some(4000),
            ),
            item(
                "zw-stop",
                "Stop hook · 1 cmd · plugin zworkflow",
                Kind::Hook,
                z,
                p,
                None,
            ),
            listing,
            roster,
            item(
                "/home/me/.claude/projects/x/memory/MEMORY.md",
                "MEMORY.md (auto-memory)",
                Kind::Memory,
                None,
                Layer::User,
                Some(5000),
            ),
            item(
                "/home/me/2lab.ai/zbrain/CLAUDE.md",
                "CLAUDE.md",
                Kind::Instruction,
                None,
                Layer::Project,
                Some(3000),
            ),
        ];
        Tree {
            agent: Agent::ClaudeCode,
            target_dir: PathBuf::from("/home/me/2lab.ai/zbrain"),
            agent_version: Some("2.1.296".to_string()),
            rules_verified_against: "2.1.296".to_string(),
            tool_search: Some(true),
            signals: Vec::new(),
            items,
            // The resolver's totals rule: every leaf once, the listings by their entries.
            predicted_known: 4000 + 22_700 + 13_000 + 5000 + 3000,
            unknown_rows: 0,
            observed_body: None,
            warnings: Vec::new(),
            home: Some(PathBuf::from("/home/me")),
            observed_regions: Vec::new(),
            tool_search_cause: None,
        }
    }

    #[test]
    fn sized_groups_partition_the_total_and_plugins_show_what_they_free() {
        let tree = synthetic();
        let screen = build(&tree, &[], &HashMap::new(), Sort::Size);
        let order: Vec<(GroupKind, Option<u64>, Option<u64>)> = screen
            .groups
            .iter()
            .map(|g| (g.kind, g.bytes, g.share))
            .collect();
        assert_eq!(
            order,
            [
                (GroupKind::SkillListing, Some(22_700), Some(47)),
                (GroupKind::AgentListing, Some(13_000), Some(27)),
                (GroupKind::Memory, Some(5000), Some(10)),
                (GroupKind::ProjectFiles, Some(3000), Some(6)),
                (GroupKind::Plugins, None, None),
            ]
        );
        // The sized groups partition the total (the Plugins group is out of the sum: its rows
        // repeat listing lines); slack's server is the only byte no sized group holds.
        let sized: Vec<&Group> = screen.groups.iter().filter(|g| !g.overlap).collect();
        let bytes: u64 = sized.iter().filter_map(|g| g.bytes).sum();
        assert_eq!(bytes, tree.predicted_known - 4000);
        let shares: u64 = sized.iter().filter_map(|g| g.share).sum();
        assert!(shares <= 100, "{shares}");

        let plugins = &screen.groups[4];
        assert!(plugins.overlap);
        // The overlap claim is the listing lines only; their own bytes are named apart.
        assert_eq!(
            plugins.count_line,
            "39 KB would leave with them · 35 KB counted in the listings above"
        );
        assert_eq!(
            plugins.count_short,
            "35 KB in the listings above + 4.0 KB of their own"
        );
        let names: Vec<(&str, Option<u64>)> = plugins
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.bytes))
            .collect();
        assert_eq!(names, [("zworkflow", Some(32_000)), ("slack", Some(7000))]);
        assert_eq!(size(32_000), "32 KB");
        assert_eq!(
            plugins.rows[0].share,
            Some(67),
            "32,000 of 47,700, rounded down"
        );
        // Its parts keep their own bytes; the hook runs later.
        let slack = &plugins.rows[1];
        let part = |name: &str| {
            let r = slack.children.iter().find(|c| c.name == name).unwrap();
            (r.bytes, r.absent)
        };
        assert_eq!(part("block-kit"), (Some(2000), None));
        assert_eq!(part("slack"), (Some(4000), None));
        // The detail names both parts of what turning it off frees.
        assert_eq!(
            slack.detail.saves,
            "Turning it off frees 7.0 KB: 4.0 KB of its own plus 3.0 KB of its entries in the \
             skill and agent listings."
        );
        assert_eq!(
            plugins.rows[0].detail.saves,
            "Turning it off frees 32 KB: its entries in the skill and agent listings."
        );
        // Its owner row in the skill listing turns off the same plugin and says the same.
        let owner = screen.groups[0]
            .rows
            .iter()
            .find(|r| r.name == "slack")
            .unwrap();
        assert_eq!(owner.bytes, Some(2000));
        assert_eq!(owner.detail.saves, slack.detail.saves);
    }

    #[test]
    fn plugins_with_only_their_own_bytes_say_so_without_the_listings() {
        let mut tree = synthetic();
        // Only slack's server: no plugin brings listing lines.
        for item in &mut tree.items {
            if matches!(item.kind, Kind::SkillListing | Kind::AgentRoster) {
                item.children.retain(|c| c.origin.plugin.is_none());
            }
        }
        let screen = build(&tree, &[], &HashMap::new(), Sort::Size);
        let plugins = screen
            .groups
            .iter()
            .find(|g| g.kind == GroupKind::Plugins)
            .unwrap();
        assert_eq!(plugins.count_line, "4.0 KB would leave with them");
        assert_eq!(plugins.count_short, "4.0 KB would leave with them");
    }

    #[test]
    fn a_plugin_without_its_origin_field_keeps_its_owner_rows_and_action() {
        let mut tree = synthetic();
        // The plugin item names itself only by its label; its parts still say `slack@…`.
        let plugin = tree
            .items
            .iter_mut()
            .find(|i| i.kind == Kind::Plugin && i.label == SLACK);
        plugin.unwrap().origin.plugin = None;
        let screen = build(&tree, &[], &HashMap::new(), Sort::Size);
        let plugins = screen
            .groups
            .iter()
            .find(|g| g.kind == GroupKind::Plugins)
            .unwrap();
        let slack = plugins.rows.iter().find(|r| r.name == "slack").unwrap();
        assert_eq!(slack.children.len(), 3, "its server and two listing lines");
        let id = slack.id.clone().unwrap();
        for kind in [GroupKind::SkillListing, GroupKind::AgentListing] {
            let group = screen.groups.iter().find(|g| g.kind == kind).unwrap();
            let owner = group.rows.iter().find(|r| r.name == "slack").unwrap();
            assert_eq!(
                owner.id.as_ref(),
                Some(&id),
                "{kind:?}: d on it turns slack off"
            );
            assert_eq!(owner.saving, slack.saving, "{kind:?}");
        }
    }

    /// A tree with no listings at all: harn turned off every skill and agent it had.
    fn without_listings() -> Tree {
        let mut tree = synthetic();
        tree.items
            .retain(|i| !matches!(i.kind, Kind::SkillListing | Kind::AgentRoster));
        tree
    }

    #[test]
    fn skills_turned_off_stay_listed_with_no_live_skill_listing() {
        let tree = without_listings();
        let off = [OffItem {
            item_id: "claude-code/skill/user/mine".to_string(),
            entry_id: "e1".to_string(),
            pointer: None,
        }];
        let screen = build(&tree, &off, &HashMap::new(), Sort::Size);
        let group = screen
            .groups
            .iter()
            .find(|g| g.kind == GroupKind::SkillListing)
            .expect("the skill listing group, for its off row");
        assert_eq!(group.count_line, "1 turned off here");
        assert_eq!(group.bytes, None);
        let (g, path) = screen.find(&off[0].item_id).expect("o can find it");
        let row = screen.row(g, &path).unwrap();
        assert_eq!((row.name.as_str(), row.state), ("mine", State::Off));
        assert!(screen
            .groups
            .iter()
            .all(|g| g.kind != GroupKind::AgentListing));
    }

    #[test]
    fn agents_turned_off_stay_listed_with_no_live_agent_roster() {
        let tree = without_listings();
        let off = [
            OffItem {
                item_id: "claude-code/sub-agent/project/helper".to_string(),
                entry_id: "e1".to_string(),
                pointer: None,
            },
            OffItem {
                item_id: "claude-code/sub-agent/user/critic".to_string(),
                entry_id: "e2".to_string(),
                pointer: None,
            },
        ];
        let screen = build(&tree, &off, &HashMap::new(), Sort::Size);
        let group = screen
            .groups
            .iter()
            .find(|g| g.kind == GroupKind::AgentListing)
            .expect("the agent listing group, for its off rows");
        assert_eq!(group.count_line, "2 turned off here");
        assert_eq!(group.absent, Some("off"));
        for o in &off {
            let (g, path) = screen.find(&o.item_id).expect("o can find it");
            assert_eq!(screen.row(g, &path).unwrap().state, State::Off);
        }
    }

    #[test]
    fn effects_parse_into_label_kind_and_direction() {
        assert_eq!(
            parse_effect("slack:block-kit (skill) leaves"),
            Some(("slack:block-kit".to_string(), Kind::Skill, true))
        );
        assert_eq!(
            parse_effect("plugin:slack:slack (mcp-server) leaves with 1 children"),
            Some(("plugin:slack:slack".to_string(), Kind::McpServer, true))
        );
        assert_eq!(
            parse_effect("x (hook) appears"),
            Some(("x".to_string(), Kind::Hook, false))
        );
        assert_eq!(parse_effect("nonsense"), None);
    }

    #[test]
    fn names_drop_ids_and_matchers() {
        assert_eq!(facet_name("slack:block-kit", "slack"), "block-kit");
        assert_eq!(facet_name("plugin:slack:slack", "slack"), "slack");
        assert_eq!(
            facet_name("plugin:alpha:alpha-server · instructions", "alpha"),
            "alpha-server"
        );
        assert_eq!(
            hook_event("SessionStart hook [^(startup|resume)$] · 1 cmd · ~/.claude/settings.json"),
            "SessionStart hook"
        );
        assert_eq!(
            hook_event_from_pointer("/x/.claude/settings.json#/hooks/Stop/1"),
            "Stop hook"
        );
        assert_eq!(
            cut_count("over cap: 146 of 216 descriptions dropped (observed)"),
            Some((146, 216))
        );
        assert_eq!(market_word("slack@claude-plugins-official"), "official");
        assert_eq!(market_word("cowork-plugin-management@synced"), "claude.ai");
        assert_eq!(market_word("zworkflow@soma-work"), "soma-work");
    }
}
