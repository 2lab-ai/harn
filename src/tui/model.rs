//! The view-model of the interactive view (`.prd/04-tui-design.md` §View-model): the resolved
//! tree regrouped by kind — each plugin one row with its skills, commands, agents, hooks and MCP
//! servers folded under it, the two listings by owner, memory, project files, hooks and servers
//! outside plugins, then the built-in parts — with people's names (no raw ids, home-relative
//! paths, scopes in words) and the detail sentences built once. Pure: no terminal, no files.
//! `view.rs` draws a [`Screen`]; `app.rs` acts on the item ids its rows carry.

use crate::model::{self, Agent, Item, Kind, Knob, Layer, Mode, Scope, Tree};
use crate::plan::{knob_scope, ActionPlan};
use crate::render::common::{denominator, json_string_tail, link_value_text, request_bytes};
use crate::resolve::claude::CAP_STATE_UNKNOWN;
use crate::resolve::claude::OFF_ALL_HOOKS;
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
/// What a hook row that switch turned off says instead of a size.
const OFF_WITH_HOOKS: &str = "off, with every other hook";

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
    /// One switch turned the whole group off (every hook): no size; `count_line` says so.
    pub all_off: bool,
    pub rows: Vec<Row>,
    /// `d` can act on some row inside.
    pub can_turn_off: bool,
    pub detail: Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    On,
    /// Turned off: by harn here (journaled), or with every other hook.
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

/// A visible line of the list: a group (`path` empty), a row under it, or (`listed`) the line
/// that stands under a plugin for its entries a filtered listing already shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub group: usize,
    /// A row's index path; for a `listed` line, its plugin row's.
    pub path: Vec<usize>,
    pub depth: usize,
    pub key: String,
    pub listed: Option<Listed>,
}

impl Line {
    /// The key of the line it sits under (its group's or its row's); None for a group line.
    pub fn parent_key(&self) -> Option<&str> {
        self.key.rsplit_once('\n').map(|(parent, _)| parent)
    }
}

/// While a filter is on, a plugin's entries that a listing group shows too are not repeated
/// under the plugin: one line per listing stands in their place and names where they are.
/// It acts on nothing; each entry keeps its one line, in the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The entries it stands for.
    pub count: usize,
    /// `8 matching skills — listed above under Skill listing › slack`.
    pub text: String,
    /// `above under Skill listing › slack`.
    pub place: String,
    /// The plugin's name (`slack`).
    pub plugin: String,
    pub detail: Detail,
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

    /// The row of a line; None for a group line and for a `listed` line (neither is a row).
    pub fn line_row(&self, line: &Line) -> Option<&Row> {
        if line.listed.is_some() {
            return None;
        }
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
    /// match, and a plugin's entries shown in a listing are not repeated under the plugin
    /// ([`Listed`]).
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
                listed: None,
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
        if needle.is_some() {
            out = self.listed_once(out);
        }
        out
    }

    /// `lines` with each plugin entry the lines show under a listing's owner row (the same
    /// item: the row's id) taken out from under its plugin row, and one [`Listed`] line per
    /// listing in the place of the first one taken.
    fn listed_once(&self, lines: Vec<Line>) -> Vec<Line> {
        let is_listing = |g: usize| {
            matches!(
                self.groups[g].kind,
                GroupKind::SkillListing | GroupKind::AgentListing
            )
        };
        // Entries under an owner row (an owner row itself acts on its plugin: left out).
        let mut shown: HashMap<&str, (usize, &str)> = HashMap::new();
        for line in lines
            .iter()
            .filter(|l| is_listing(l.group) && l.path.len() >= 2)
        {
            let (Some(row), Some(owner)) =
                (self.line_row(line), self.row(line.group, &line.path[..1]))
            else {
                continue;
            };
            if let Some(id) = row.id.as_deref() {
                shown.entry(id).or_insert((line.group, owner.name.as_str()));
            }
        }
        // A plugin's part shown there too: by its plugin row and that listing.
        let elsewhere = |line: &Line| -> Option<(usize, usize, &str)> {
            if self.groups[line.group].kind != GroupKind::Plugins || line.path.len() != 2 {
                return None;
            }
            let id = self.line_row(line)?.id.as_deref()?;
            let (listing, owner) = shown.get(id)?;
            Some((line.path[0], *listing, *owner))
        };
        let mut words: HashMap<(usize, usize), Vec<&str>> = HashMap::new();
        for line in &lines {
            if let Some((plugin, listing, _)) = elsewhere(line) {
                let kind = self.line_row(line).map_or("", |r| r.kind_word.as_str());
                words.entry((plugin, listing)).or_default().push(kind);
            }
        }
        if words.is_empty() {
            return lines;
        }
        let mut out = Vec::with_capacity(lines.len());
        for line in &lines {
            let Some((plugin, listing, owner)) = elsewhere(line) else {
                out.push(line.clone());
                continue;
            };
            // The first one taken leaves its line to the note; the rest leave nothing.
            let Some(kinds) = words.remove(&(plugin, listing)) else {
                continue;
            };
            let parent = &line.path[..1];
            let name = self.row(line.group, parent).map_or("", |r| r.name.as_str());
            let place = format!(
                "{} under {} › {owner}",
                if listing < line.group {
                    "above"
                } else {
                    "below"
                },
                self.groups[listing].name
            );
            out.push(Line {
                group: line.group,
                path: parent.to_vec(),
                depth: line.depth,
                key: format!(
                    "{}\nlisted:{}",
                    self.key(line.group, parent),
                    self.groups[listing].kind.key()
                ),
                listed: Some(listed(&kinds, &place, name, &self.groups[listing].name)),
            });
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

/// The line standing for `kinds` (the kind word of each entry it stands for) of plugin `plugin`,
/// shown `place` (`above under Skill listing › slack`) in `listing`.
fn listed(kinds: &[&str], place: &str, plugin: &str, listing: &str) -> Listed {
    let n = kinds.len();
    let what = match kinds.split_first() {
        Some((first, rest)) if rest.iter().all(|k| k == first) => {
            count(n, &format!("matching {first}"))
        }
        // Kinds mixed: two entries at least.
        _ => format!("{n} matching entries"),
    };
    let (them, each) = if n == 1 {
        ("it", "it is")
    } else {
        ("them", "each is")
    };
    Listed {
        count: n,
        text: format!("{what} — listed {place}"),
        place: place.to_string(),
        plugin: plugin.to_string(),
        detail: Detail {
            what: format!("{what} of {plugin} · listed {place}"),
            brings: format!(
                "This search finds {them} under the plugin and in the {} too; {each} shown \
                 once, there, with its own size.",
                listing.to_lowercase()
            ),
            ..Detail::default()
        },
    }
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
            listed: None,
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
    build_with(tree, off, journaled, sort, false)
}

/// [`build`] for a session started with `--readonly` when `read_only`: the sentences say what
/// `d` would do (`d would turn it off …`), and promise no `u`.
pub fn build_with(
    tree: &Tree,
    off: &[OffItem],
    journaled: &HashMap<String, String>,
    sort: Sort,
    read_only: bool,
) -> Screen {
    let b = Builder {
        tree,
        places: Places::of(tree),
        total: denominator(tree),
        journaled,
        read_only,
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
    /// `--readonly`: `d` would act, and does not.
    read_only: bool,
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
                        action: format!(
                            "Open it (→); {}",
                            self.d_clause("turns auto-memory off for this project only")
                        ),
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
                        action: format!(
                            "Open the group (→); {}",
                            self.d_clause("leaves a file out for this project only")
                        ),
                        ..Detail::default()
                    },
                ),
            );
        }
        // Every hook off with Claude Code's one switch: still listed, each saying so.
        let switched = hooks
            .iter()
            .filter(|h| h.signal.as_deref() == Some(OFF_ALL_HOOKS))
            .collect::<Vec<_>>();
        let hooks_all_off =
            !switched.is_empty() || off.iter().any(|o| o.pointer.as_deref() == Some(ALL_HOOKS));
        let by_harn = off.iter().any(|o| o.pointer.as_deref() == Some(ALL_HOOKS))
            || switched.iter().any(|h| self.journaled.contains_key(&h.id));
        if !hook_rows.is_empty() && hooks_all_off {
            let detail = if by_harn {
                Detail {
                    what: "Hooks · all off for this project".to_string(),
                    brings: "harn turned every hook off here with Claude Code's one switch for \
                             them; none runs until it is turned back on."
                        .to_string(),
                    action: "u undoes the change; o on any hook below does too.".to_string(),
                    ..Detail::default()
                }
            } else {
                Detail {
                    what: "Hooks · all off here".to_string(),
                    brings: "Your settings turn every hook off (disableAllHooks); none runs here."
                        .to_string(),
                    action: "harn did not make that change, so o and u cannot reverse it."
                        .to_string(),
                    ..Detail::default()
                }
            };
            let mut group = self.group(GroupKind::Hooks, "Hooks", hook_rows, detail);
            group.count_line = if by_harn {
                "all off for this project · u undoes".to_string()
            } else {
                "all off here".to_string()
            };
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
                    format!(
                        "Open it (→) to see the entries by owner; {}",
                        self.d_clause_on("on an entry", "turns that one off")
                    )
                } else {
                    format!(
                        "Open it (→); {}",
                        self.key_clause("o", "on an entry", "turns it back on")
                    )
                },
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

    /// The detail of a row harn turned off here that still resolves: what still shows in the
    /// first request is what Claude Code may still load.
    fn still_listed(&self, item: &Item, name: &str) -> Option<Detail> {
        let loads = matches!(item.mode, Mode::Always | Mode::Advertised);
        self.journaled.contains_key(&item.id).then(|| Detail {
            what: format!("{name} · turned off · u undoes · takes effect next session"),
            brings: if loads {
                "harn turned it off here, but it still shows in what loads; whether its bytes \
                 leave needs `harn verify --run`."
            } else {
                "harn turned it off here; it stays listed, and none of it loads."
            }
            .to_string(),
            action: "o turns it back on (the change is reversed byte for byte).".to_string(),
            ..Detail::default()
        })
    }

    /// Source words of a non-plugin item: `project`, `~/.claude`, `claude.ai`, `Claude Code`.
    fn layer_source(&self, item: &Item) -> String {
        match (item.origin.layer, item.origin.plugin.as_deref()) {
            (Layer::Plugin, Some(plugin)) => market_word(plugin),
            (layer, _) => layer_words(layer).to_string(),
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
        } else if switched_off(item) {
            Some(OFF_WITH_HOOKS)
        } else if self.journaled.contains_key(&item.id) {
            // Turned off here and still listed (a disabled MCP server): the word place_off
            // rows say.
            Some("off")
        } else if item.cost.not_applicable || matches!(item.mode, Mode::Runtime | Mode::None) {
            absent_word(item.mode).or(Some("not loaded"))
        } else {
            None
        };
        (bytes, self.share(bytes), absent)
    }

    /// Off when harn turned it off here (even if it still resolves: a denied agent stays in
    /// the roster) or it went off with every other hook, else On with a switch, Fixed without
    /// one.
    fn state(&self, item: &Item) -> State {
        if self.journaled.contains_key(&item.id) || switched_off(item) {
            State::Off
        } else if any_knob(item).is_some() {
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
                    State::On => format!(
                        "Open it (→) and pick one; {}",
                        self.d_clause("turns that one off")
                    ),
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
                detail.action = format!(
                    "Open it (→) and pick one; {}",
                    self.d_clause("turns that one off")
                );
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
        self.hooks_off(item, name)
            .or_else(|| self.still_listed(item, name))
            .unwrap_or(detail)
    }

    /// The detail of a hook that went off with every other hook.
    fn hooks_off(&self, item: &Item, name: &str) -> Option<Detail> {
        if !switched_off(item) {
            return None;
        }
        Some(if self.journaled.contains_key(&item.id) {
            Detail {
                what: format!(
                    "{name} · turned off with every other hook · u undoes · takes effect next \
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
                what: format!("{name} · off with every other hook"),
                brings: "Your settings turn every hook off (disableAllHooks); it does not run \
                         here."
                    .to_string(),
                action: "harn did not make that change, so o and u cannot reverse it.".to_string(),
                ..Detail::default()
            }
        })
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
                detail.action = self.d_clause(&format!("{verb} for this project only{file}"));
                if it != "it" {
                    detail.action = detail.action.replacen(" it ", &format!(" {it} "), 1);
                }
                detail.saves = self.saves(item, listing);
                // Under `--readonly` u does nothing: no promise of it.
                if !self.read_only {
                    detail.undo = "Reversible with u.".to_string();
                }
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

    /// `d turns it off for …` as a sentence: `d <does>.`; under `--readonly`, `d would <do>.`
    /// — the header, the key bar and the `?` card say read-only, and a second clause here would
    /// push the saving out of a small panel (`does` starts with a verb in the third person:
    /// `turns`, `leaves`, `hides`, `removes`, `moves`).
    fn d_clause(&self, does: &str) -> String {
        self.d_clause_on("", does)
    }

    /// [`Self::d_clause`] with words between `d` and the verb (`d on an entry turns …`).
    fn d_clause_on(&self, on: &str, does: &str) -> String {
        self.key_clause("d", on, does)
    }

    /// A sentence for a write key (`d`, `o`): `<key> <on> <does>.`; under `--readonly`
    /// `<key> <on> would <do>.`.
    fn key_clause(&self, key: &str, on: &str, does: &str) -> String {
        let on = if on.is_empty() {
            String::new()
        } else {
            format!(" {on}")
        };
        if !self.read_only {
            return format!("{key}{on} {does}.");
        }
        let (verb, rest) = does.split_once(' ').unwrap_or((does, ""));
        let verb = verb.strip_suffix('s').unwrap_or(verb);
        let rest = if rest.is_empty() {
            String::new()
        } else {
            format!(" {rest}")
        };
        format!("{key}{on} would {verb}{rest}.")
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
            absent: Some(if all_hooks { OFF_WITH_HOOKS } else { "off" }),
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

/// The source column's words for a layer: `project`, `~/.claude`, `claude.ai`, `Claude Code`.
fn layer_words(layer: Layer) -> &'static str {
    match layer {
        Layer::Project => "project",
        Layer::Local => "this project",
        Layer::User => "~/.claude",
        Layer::ClaudeAi => "claude.ai",
        Layer::Builtin => "Claude Code",
        Layer::Managed => "organisation",
        Layer::Plugin => "plugin",
        Layer::Dynamic => "session",
        Layer::Unresolved => "unresolved",
    }
}

/// A hook (or the status line) that Claude Code's one hooks switch turned off.
fn switched_off(item: &Item) -> bool {
    item.signal.as_deref() == Some(OFF_ALL_HOOKS)
}

/// Name and source of an item known only by its id (`<agent>/<kind>/<layer>/<name>`): a plugin
/// without its marketplace, a file home-relative, a hook by its event, else the name's last
/// `:` part. The source is worded as a live row's (`~/.claude`, not `user`).
pub fn id_name(id: &str, places: &Places) -> (String, String) {
    let mut parts = id.splitn(4, '/');
    let (_, kind, layer, name) = (parts.next(), parts.next(), parts.next(), parts.next());
    let layer = layer.unwrap_or_default();
    let (kind, layer, name) = (
        kind.unwrap_or_default(),
        Layer::ALL
            .iter()
            .find(|l| l.as_str() == layer)
            .map_or(layer, |l| layer_words(*l))
            .to_string(),
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
    let (pointer, _) = split_link(rest);
    Some(setting_scope(Path::new(file), pointer, places).sentence())
}

/// The JSON pointer of an origin link's `#…` part and what follows it. A pointer keeps the
/// spaces, parentheses and ` = ` of the directory names in it (only `/` and `~` are escaped), so
/// the tail is read from the right, its form decided first.
///
/// The grammar is what the resolver writes (`src/resolve`): `<pointer>`, `<pointer> (<note>)`,
/// or `<pointer> = <value>` where the value is `true`, `false` or a JSON string `"…"` — never a
/// bare word or path, never a container (`every_link_value_in_the_fixture_is_a_boolean_or_a_json_string`
/// in `tests/resolve_claude.rs` checks the fixture's links).
///
/// A link with no tail ends in a fixed key (`statusLine`, `allow`, `disabledMcpServers`) or a
/// list index (`/hooks/Stop/0`, `/deniedMcpServers/2`); a key the user chose — a plugin id, a
/// server name, a skill name — is never the last token without a tail after it (`= true`,
/// `= "…"`, `(http)`). So a name like `srv (x)` keeps its parentheses:
/// `pointer_only_links_end_in_a_fixed_key_or_an_index` in `tests/resolve_claude.rs` checks
/// the fixture's links.
///
/// A JSON string tail (the text ends in an unescaped `"`) is read back to its opening quote,
/// and the ` = ` right before it is the separator, whatever the string holds. Else, when the
/// text ends in `)`, what follows its last ` (` is a note if it has no `=` and no `/` (a note
/// harn writes is one word, `(http)`; after a directory's ` (` a pointer goes on with `/`).
/// Else the last ` = ` (a boolean). Anything else is all pointer.
pub fn split_link(rest: &str) -> (&str, &str) {
    if let Some(open) = json_string_tail(rest) {
        if let Some(at) = rest[..open].strip_suffix(" = ").map(str::len) {
            return (&rest[..at], &rest[at + 1..]);
        }
    }
    if rest.ends_with(')') {
        if let Some(at) = rest.rfind(" (") {
            if !rest[at..].contains(['=', '/']) {
                return (&rest[..at], rest[at + 1..].trim_start());
            }
        }
    }
    if let Some(at) = rest.rfind(" = ") {
        return (&rest[..at], rest[at + 1..].trim_start());
    }
    (rest, "")
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
    /// `6 at start (alpha, 2 skills, 1 MCP server, …), 1 later (PreToolUse hook)`: what stops
    /// loading, in the first request and later; `N at start` is the header's drop.
    pub leaves: String,
    /// Each thing that stops loading, under `at start (N)` and `when used / later (M)`
    /// (`block-kit (skill)`); empty when the Leaves line names the one thing.
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
    let all_hooks = plan
        .ops
        .iter()
        .any(|op| matches!(&op.knob, Knob::Setting { pointer, .. } if pointer == ALL_HOOKS));
    let (gone, added) = leaving(plan, tree);
    let shown = |i: &crate::plan::Leaving| {
        if i.id == item.id {
            name.to_string()
        } else {
            human_label(tree, &i.label, i.kind, &places)
        }
    };
    // By the mode each had before the change: at start (what the header counts), later (runs or
    // is read later; the status line runs locally), already off (loaded nothing before the
    // change: never counted).
    type Gone<'p> = Vec<&'p crate::plan::Leaving>;
    let (start, rest): (Gone, Gone) = gone.iter().partition(|i| at_start(i.mode_before));
    let (off, later): (Gone, Gone) = rest
        .into_iter()
        .partition(|i| already_off(i.mode_before, i.kind));
    // `alpha, 2 skills, 1 MCP server`: the picked item by name, the rest counted by kind.
    let summary = |items: &[&crate::plan::Leaving]| -> String {
        let mut parts: Vec<String> = items
            .iter()
            .filter(|i| i.id == item.id)
            .map(|i| shown(i))
            .collect();
        let mut counts: Vec<(Kind, usize)> = Vec::new();
        for i in items.iter().filter(|i| i.id != item.id) {
            match counts.iter_mut().find(|(k, _)| *k == i.kind) {
                Some((_, n)) => *n += 1,
                None => counts.push((i.kind, 1)),
            }
        }
        parts.extend(counts.iter().map(|(k, n)| count(*n, kind_word(*k))));
        parts.join(", ")
    };
    let mut groups = Vec::new();
    if !start.is_empty() {
        groups.push(format!("{} at start ({})", start.len(), summary(&start)));
    }
    if !later.is_empty() {
        groups.push(format!("{} later ({})", later.len(), summary(&later)));
    }
    let mut leaves = if groups.is_empty() {
        "nothing that loads".to_string()
    } else {
        groups.join(", ")
    };
    if !off.is_empty() {
        leaves.push_str(&format!(" · {} already off", off.len()));
    }
    // `UserPromptSubmit (hook)`: the kind word once. The status line has no row to point at.
    let line = |i: &crate::plan::Leaving| {
        let word = kind_word(i.kind);
        let n = shown(i);
        let n = n
            .strip_suffix(&format!(" {word}"))
            .unwrap_or(&n)
            .to_string();
        match i.kind {
            Kind::StatusLine => {
                format!("  {n} ({word} — not a row above; it stops with the hooks)")
            }
            _ => format!("  {n} ({word})"),
        }
    };
    let mut leave_list: Vec<String> = Vec::new();
    if gone.len() > 1 || !added.is_empty() {
        if !start.is_empty() {
            leave_list.push(format!("at start ({})", start.len()));
            leave_list.extend(start.iter().map(|i| line(i)));
        }
        if !later.is_empty() {
            leave_list.push(format!("when used / later ({})", later.len()));
            leave_list.extend(later.iter().map(|i| line(i)));
        }
        if !off.is_empty() {
            leave_list.push(format!("already off ({})", off.len()));
            leave_list.extend(off.iter().map(|i| line(i)));
        }
        if !added.is_empty() {
            leave_list.push(format!("appears ({})", added.len()));
            leave_list.extend(added.iter().map(|a| format!("  {a}")));
        }
    }
    let listing = gone
        .iter()
        .any(|i| matches!(i.kind, Kind::Skill | Kind::Command | Kind::SubAgent))
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

/// `6 fewer items load at start (83 → 77)`, `still 1 item loads at start`: how the header's
/// count moved, for the applied message.
pub(crate) fn start_count(before: usize, after: usize) -> String {
    let items = |n: usize| if n == 1 { "item loads" } else { "items load" };
    let n = before.abs_diff(after);
    match after.cmp(&before) {
        std::cmp::Ordering::Less => {
            format!("{n} fewer {} at start ({before} → {after})", items(n))
        }
        std::cmp::Ordering::Greater => {
            format!("{n} more {} at start ({before} → {after})", items(n))
        }
        std::cmp::Ordering::Equal => format!("still {before} {} at start", items(before)),
    }
}

/// At start: mode always or advertised, as the header counts items.
fn at_start(mode: Mode) -> bool {
    matches!(mode, Mode::Always | Mode::Advertised)
}

/// Loaded nothing before the change (mode none), the status line aside: it runs locally and
/// stops with the hooks.
fn already_off(mode: Mode, kind: Kind) -> bool {
    mode == Mode::None && kind != Kind::StatusLine
}

/// `item` and everything under it.
fn subtree(item: &Item) -> Vec<&Item> {
    let mut out = vec![item];
    for child in &item.children {
        out.extend(subtree(child));
    }
    out
}

/// What `plan` stops loading, as the plan lists it (each item with its mode before the
/// change): the picked items with what is under them first, then the rest by kind (skills,
/// commands, agents, MCP servers, hooks, the rest); and what appears, worded. A picked item
/// that stays (a denied agent stays in the roster) is not on the plan's list, nor is anything
/// that was switched off already.
fn leaving<'p>(plan: &'p ActionPlan, tree: &Tree) -> (Vec<&'p crate::plan::Leaving>, Vec<String>) {
    let places = Places::of(tree);
    let added = plan
        .side_effects
        .iter()
        .filter_map(|effect| parse_effect(effect))
        .filter(|(_, _, leaves)| !leaves)
        .map(|(label, kind, _)| {
            let shown = human_label(tree, &label, kind, &places);
            format!("{shown} ({})", kind_word(kind))
        })
        .collect();
    let mut picked: HashSet<&str> = HashSet::new();
    for op in &plan.ops {
        if let Some(item) = model::walk(tree).find(|i| i.id == op.item_id) {
            picked.extend(subtree(item).into_iter().map(|i| i.id.as_str()));
        }
    }
    let order = |k: Kind| match k {
        Kind::Skill => 0,
        Kind::Command => 1,
        Kind::SubAgent => 2,
        Kind::McpServer | Kind::McpInstructions | Kind::McpTool => 3,
        Kind::Hook => 4,
        _ => 5,
    };
    let (mut gone, mut side): (Vec<_>, Vec<_>) = plan
        .leaving
        .iter()
        .partition(|l| picked.contains(l.id.as_str()));
    side.sort_by_key(|l| order(l.kind));
    gone.extend(side);
    (gone, added)
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
        let (pointer, tail) = split_link(rest);
        let words = pointer_words(pointer, places).join(" › ");
        let scope = setting_scope(Path::new(file), pointer, places).short();
        // A string value as its text, its paths home- or project-relative.
        let tail = match tail.strip_prefix("= ") {
            Some(value) => format!("= {}", link_value_text(value)),
            None => tail.to_string(),
        };
        let tail = places.text(&tail);
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

    fn places_in(dir: &str) -> Places {
        Places {
            dir: PathBuf::from(dir),
            home: Some(PathBuf::from("/home/me")),
        }
    }

    #[test]
    fn the_count_at_start_is_worded_by_how_it_moves() {
        // The applied, `o` and `u` messages: how the header's number moved.
        assert_eq!(start_count(83, 77), "6 fewer items load at start (83 → 77)");
        assert_eq!(start_count(83, 82), "1 fewer item loads at start (83 → 82)");
        assert_eq!(start_count(2, 3), "1 more item loads at start (2 → 3)");
        assert_eq!(start_count(83, 83), "still 83 items load at start");
        assert_eq!(start_count(1, 1), "still 1 item loads at start");
    }

    #[test]
    fn a_link_splits_its_tail_from_the_right() {
        assert_eq!(
            split_link("/enabledPlugins/slack@x = true"),
            ("/enabledPlugins/slack@x", "= true")
        );
        assert_eq!(
            split_link("/projects/~1home~1me~1my app/enabledPlugins/x = true"),
            ("/projects/~1home~1me~1my app/enabledPlugins/x", "= true")
        );
        assert_eq!(
            split_link("/projects/~1home~1me~1foo (x)/mcpServers/s (http)"),
            ("/projects/~1home~1me~1foo (x)/mcpServers/s", "(http)")
        );
        // A parenthesis inside the pointer, no tail: all pointer.
        assert_eq!(
            split_link("/projects/~1home~1me~1foo (x)/disabledMcpServers"),
            ("/projects/~1home~1me~1foo (x)/disabledMcpServers", "")
        );
        assert_eq!(split_link("/hooks/Stop/0"), ("/hooks/Stop/0", ""));
        // A ` (` in a directory, the pointer ending in `)`, no tail: all pointer.
        assert_eq!(
            split_link("/projects/~1home~1me~1foo (x)/mcpServers/srv(x)"),
            ("/projects/~1home~1me~1foo (x)/mcpServers/srv(x)", "")
        );
        assert_eq!(
            split_link("/projects/~1home~1me~1foo (x)/mcpServers/srv (http)"),
            ("/projects/~1home~1me~1foo (x)/mcpServers/srv", "(http)")
        );
        assert_eq!(
            split_link("/projects/~1home~1me~1foo (x)/mcpServers/srv(x) = true"),
            ("/projects/~1home~1me~1foo (x)/mcpServers/srv(x)", "= true")
        );
        // A ` = ` inside a directory name: the note at the end decides the tail first.
        assert_eq!(
            split_link("/projects/~1home~1me~1foo = bar/mcpServers/srv (http)"),
            ("/projects/~1home~1me~1foo = bar/mcpServers/srv", "(http)")
        );
        assert_eq!(
            split_link("/projects/~1home~1me~1foo = bar/enabledPlugins/x = true"),
            ("/projects/~1home~1me~1foo = bar/enabledPlugins/x", "= true")
        );
        // A value that ends in parentheses (inside its quotes) is still a value.
        assert_eq!(
            split_link("/projects/~1home~1me~1foo (x)/env/NOTE = \"a (b)\""),
            ("/projects/~1home~1me~1foo (x)/env/NOTE", "= \"a (b)\"")
        );
        assert_eq!(
            split_link("/env/NOTE = \"a (b)\""),
            ("/env/NOTE", "= \"a (b)\"")
        );
        // A ` = ` inside a quoted value is the value's: the separator is the one before it.
        assert_eq!(
            split_link("/env/NOTE = \"a = b\""),
            ("/env/NOTE", "= \"a = b\"")
        );
        assert_eq!(
            split_link("/projects/~1home~1me~1my app/env/X = \"x (y) = z\""),
            ("/projects/~1home~1me~1my app/env/X", "= \"x (y) = z\"")
        );
        // An escaped quote inside the value does not open it.
        assert_eq!(
            split_link(r#"/env/Q = "say \"hi\" = ok""#),
            ("/env/Q", r#"= "say \"hi\" = ok""#)
        );
    }

    /// A plugin whose admission chain starts with `link`.
    fn enabled_by(link: &str) -> Item {
        let mut plugin = item(SLACK, SLACK, Kind::Plugin, Some(SLACK), Layer::Plugin, None);
        plugin.origin.why = vec![link.to_string()];
        plugin
    }

    #[test]
    fn a_project_key_with_spaces_or_parentheses_keeps_its_project() {
        // (a) the target dir has a space; (b) it has ` (x)`; (e) it has ` = `.
        for dir in ["/home/me/my app", "/home/me/foo (x)", "/home/me/foo = bar"] {
            let p = places_in(dir);
            let escaped = dir.replace('/', "~1");
            let link = format!("/home/me/.claude.json#/projects/{escaped}/enabledPlugins/x = true");
            assert_eq!(
                where_link(&link, &p),
                "~/.claude.json › projects › this project › enabledPlugins › x = true   \
                 (this project)",
                "{dir}"
            );
            assert_eq!(
                enabled_scope(&enabled_by(&link), &p).as_deref(),
                Some("for this project only"),
                "{dir}"
            );
            let (pointer, _) = split_link(link.split_once('#').unwrap().1);
            assert_eq!(pointer_words(pointer, &p)[1], "this project", "{dir}");
            // (d) the `(http)` tail form.
            let server = format!("/home/me/.claude.json#/projects/{escaped}/mcpServers/srv (http)");
            assert_eq!(
                where_link(&server, &p),
                "~/.claude.json › projects › this project › mcpServers › srv (http)   \
                 (this project)",
                "{dir}"
            );
            assert_eq!(
                enabled_scope(&enabled_by(&server), &p).as_deref(),
                Some("for this project only"),
                "{dir}"
            );
        }
        // (c) another project's dir with a space: that project, home-relative, never ours.
        let p = places();
        let link = "/home/me/.claude.json#/projects/~1home~1me~1other app/enabledPlugins/x = true";
        assert_eq!(
            where_link(link, &p),
            "~/.claude.json › projects › ~/other app › enabledPlugins › x = true   (~/other app)"
        );
        assert_eq!(
            enabled_scope(&enabled_by(link), &p).as_deref(),
            Some("for ~/other app only")
        );
        let server = "/home/me/.claude.json#/projects/~1home~1me~1other app/mcpServers/srv (http)";
        assert_eq!(
            where_link(server, &p),
            "~/.claude.json › projects › ~/other app › mcpServers › srv (http)   (~/other app)"
        );
        // (e) another project's dir with ` = `, behind a note tail.
        let server = "/home/me/.claude.json#/projects/~1home~1me~1foo = bar/mcpServers/srv (http)";
        assert_eq!(
            where_link(server, &p),
            "~/.claude.json › projects › ~/foo = bar › mcpServers › srv (http)   (~/foo = bar)"
        );
        assert_eq!(
            enabled_scope(&enabled_by(server), &p).as_deref(),
            Some("for ~/foo = bar only")
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
    fn a_string_value_reads_as_its_text() {
        let p = places();
        assert_eq!(
            where_link(
                "/home/me/2lab.ai/zbrain/.claude/settings.local.json#/autoMemoryDirectory = \
                 \"/home/me/memory (old)\"",
                &p
            ),
            ".claude/settings.local.json › autoMemoryDirectory = ~/memory (old)   (this project \
             only)"
        );
        assert_eq!(
            where_link(
                "/home/me/.claude/settings.json#/outputStyle = \"My Style (x)\"",
                &p
            ),
            "~/.claude/settings.json › outputStyle = My Style (x)   (every project)"
        );
        assert_eq!(
            where_link(
                r#"/home/me/.claude/settings.json#/env/Q = "say \"hi\"""#,
                &p
            ),
            r#"~/.claude/settings.json › env › Q = say "hi"   (every project)"#
        );
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

    /// A plan turning `picked` off through its first knob, under which `leaving` leave (each
    /// with its mode as it is now), with these side effects.
    fn plan_off(picked: &Item, leaving: &[&Item], side_effects: &[&str]) -> ActionPlan {
        ActionPlan {
            target_dir: PathBuf::from("/home/me/2lab.ai/zbrain"),
            agent: Agent::ClaudeCode,
            agent_version: None,
            ops: vec![crate::plan::PlannedOp {
                item_id: picked.id.clone(),
                knob: picked.knobs[0].clone(),
                file: None,
                hash_before: None,
                reason: None,
            }],
            predicted_before: 0,
            predicted_after: 0,
            items_after: 0,
            freed: crate::model::Freed {
                known: 0,
                delta_unknown: false,
                is_estimate: false,
            },
            leaving: leaving
                .iter()
                .map(|i| crate::plan::Leaving {
                    id: i.id.clone(),
                    label: i.label.clone(),
                    kind: i.kind,
                    mode_before: i.mode,
                })
                .collect(),
            side_effects: side_effects.iter().map(|e| e.to_string()).collect(),
            warnings: Vec::new(),
            diff: String::new(),
        }
    }

    #[test]
    fn what_was_already_off_is_its_own_group_and_never_counted() {
        let mut tree = synthetic();
        // A server of slack's that the settings disable: mode none, no `off:` signal; it goes
        // with its plugin.
        let mut server = item(
            "plugin:slack:extra",
            "plugin:slack:extra",
            Kind::McpServer,
            Some(SLACK),
            Layer::Plugin,
            None,
        );
        server.mode = Mode::None;
        tree.items[0].children.push(server);
        let picked = tree.items[0].clone();
        let plan = plan_off(&picked, &[&picked, &picked.children[0]], &[]);
        let card = off_card(&plan, &tree, &picked, "slack", "→", None);
        assert_eq!(card.leaves, "1 at start (slack) · 1 already off");
        assert_eq!(
            card.leave_list,
            [
                "at start (1)",
                "  slack (plugin)",
                "already off (1)",
                "  extra (MCP server)"
            ]
        );
    }

    #[test]
    fn a_picked_item_that_goes_is_listed_even_when_another_appears() {
        let tree = synthetic();
        let picked = tree.items[4].children[2].clone();
        assert_eq!(picked.label, "mine");
        // `mine` goes, a skill appears.
        let plan = plan_off(&picked, &[&picked], &["mine-too (skill) appears"]);
        let card = off_card(&plan, &tree, &picked, "mine", "→", None);
        assert_eq!(card.leaves, "1 at start (mine)");
        assert_eq!(
            card.leave_list,
            [
                "at start (1)",
                "  mine (skill)",
                "appears (1)",
                "  mine-too (skill)"
            ]
        );
        // Not on the plan's list of what leaves: the picked item stays (a denied agent).
        let plan = plan_off(&picked, &[], &[]);
        let card = off_card(&plan, &tree, &picked, "mine", "→", None);
        assert_eq!(card.leaves, "nothing that loads");
    }

    #[test]
    fn a_picked_item_that_stays_is_not_listed_when_a_disabled_server_appears() {
        // Tool search off: an MCP server loads at start, unless it is disabled (mode none).
        let mut tree = synthetic();
        tree.tool_search = Some(false);
        let picked = tree.items[4].children[2].clone();
        assert_eq!(picked.label, "mine");
        // `mine` stays (nothing leaves); a disabled server appears and loads nothing.
        let plan = plan_off(&picked, &[], &["plugin:other:srv (mcp-server) appears"]);
        let card = off_card(&plan, &tree, &picked, "mine", "→", None);
        assert_eq!(card.leaves, "nothing that loads");
        assert_eq!(card.leave_list, ["appears (1)", "  srv (MCP server)"]);
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

    /// The key of the `slack` owner row in the listing group of `kind` (to open it).
    fn slack_owner(screen: &Screen, kind: GroupKind) -> String {
        let g = screen.groups.iter().position(|g| g.kind == kind).unwrap();
        let i = screen.groups[g]
            .rows
            .iter()
            .position(|r| r.name == "slack")
            .unwrap();
        screen.key(g, &[i])
    }

    /// The lines under the plugin row at `at` of the Plugins group, in order: a row by its
    /// name, a note by its text.
    fn under_plugin(screen: &Screen, lines: &[Line], plugins: usize, at: usize) -> Vec<String> {
        lines
            .iter()
            .filter(|l| l.group == plugins && l.path.first() == Some(&at) && l.depth == 2)
            .map(|l| match (&l.listed, screen.line_row(l)) {
                (Some(listed), _) => listed.text.clone(),
                (None, Some(row)) => row.name.clone(),
                (None, None) => unreachable!("a group line under a plugin"),
            })
            .collect()
    }

    #[test]
    fn a_search_shows_each_listed_entry_once_and_points_there_from_its_plugin() {
        let tree = synthetic();
        let screen = build(&tree, &[], &HashMap::new(), Sort::Size);
        let plugins = screen
            .groups
            .iter()
            .position(|g| g.kind == GroupKind::Plugins)
            .unwrap();
        let at = screen.groups[plugins]
            .rows
            .iter()
            .position(|r| r.name == "slack")
            .unwrap();
        let slack = &screen.groups[plugins].rows[at];
        let entry = |label: &str| {
            slack
                .children
                .iter()
                .find(|c| c.name == label)
                .and_then(|c| c.id.clone())
                .unwrap()
        };
        let (block_kit, helper) = (entry("block-kit"), entry("helper"));
        let once = |lines: &[Line]| {
            for id in [&block_kit, &helper] {
                let n = lines
                    .iter()
                    .filter(|l| screen.line_row(l).and_then(|r| r.id.as_ref()) == Some(id))
                    .count();
                assert_eq!(n, 1, "{id}");
            }
        };

        // No filter: the plugin row keeps every part as a child.
        let open: HashSet<String> = ["plugins".to_string(), format!("plugins\n{}", slack.key)]
            .into_iter()
            .collect();
        let lines = screen.lines(&open, None);
        assert!(lines.iter().all(|l| l.listed.is_none()));
        assert_eq!(
            under_plugin(&screen, &lines, plugins, at),
            ["slack", "block-kit", "helper"],
            "its server and its two listing lines"
        );

        // `slack` with the listings' slack rows folded (no entry name matches): the listings
        // show no entry, so the plugin keeps them; each still shows once.
        let lines = screen.lines(&HashSet::new(), Some("slack"));
        once(&lines);
        assert!(lines.iter().all(|l| l.listed.is_none()));
        assert_eq!(
            under_plugin(&screen, &lines, plugins, at),
            ["slack", "block-kit", "helper"]
        );

        // With them open, each entry shows once, in its listing; under the plugin, one line
        // per listing in their places says where. Its server is listed nowhere else and stays.
        let open: HashSet<String> = [
            slack_owner(&screen, GroupKind::SkillListing),
            slack_owner(&screen, GroupKind::AgentListing),
        ]
        .into_iter()
        .collect();
        let lines = screen.lines(&open, Some("slack"));
        once(&lines);
        let notes: Vec<&Line> = lines.iter().filter(|l| l.listed.is_some()).collect();
        assert_eq!(
            under_plugin(&screen, &lines, plugins, at),
            [
                "slack",
                "1 matching skill — listed above under Skill listing › slack",
                "1 matching agent — listed above under Agent listing › slack",
            ],
            "its server, then the two notes in their entries' places (by size)"
        );
        assert_eq!(notes.len(), 2);
        for note in &notes {
            // It sits under the plugin row, acts on nothing and opens nothing.
            assert_eq!(
                (note.group, note.path.as_slice(), note.depth),
                (plugins, &[at][..], 2)
            );
            assert!(screen.line_row(note).is_none());
            let listed = note.listed.as_ref().unwrap();
            assert_eq!((listed.count, listed.plugin.as_str()), (1, "slack"));
            assert!(listed.detail.action.is_empty(), "{:?}", listed.detail);
        }
        assert_ne!(notes[0].key, notes[1].key);
        // The plugin row is unchanged: `d` on it still turns slack off.
        let row = lines
            .iter()
            .find(|l| l.group == plugins && l.path == [at])
            .and_then(|l| screen.line_row(l))
            .unwrap();
        assert_eq!(row, slack);
        assert_eq!(row.state, State::On);
        assert!(
            row.detail.action.starts_with("d turns it off"),
            "{}",
            row.detail.action
        );
    }

    #[test]
    fn a_note_counts_mixed_entries_and_says_below_when_the_listing_is_below() {
        let mut tree = synthetic();
        for item in &mut tree.items {
            if item.kind == Kind::SkillListing {
                // An unmeasured skill listing sorts after the Plugins group.
                item.cost.predicted = None;
                for c in &mut item.children {
                    c.cost.predicted = None;
                }
                let mut post = item.children[0].clone();
                post.id = "claude-code/command/plugin/slack:post".to_string();
                post.label = "slack:post".to_string();
                post.kind = Kind::Command;
                item.children.push(post);
            }
        }
        let screen = build(&tree, &[], &HashMap::new(), Sort::Size);
        let order: Vec<GroupKind> = screen.groups.iter().map(|g| g.kind).collect();
        let (p, s) = (
            order.iter().position(|k| *k == GroupKind::Plugins).unwrap(),
            order
                .iter()
                .position(|k| *k == GroupKind::SkillListing)
                .unwrap(),
        );
        assert!(p < s, "{order:?}");
        let open: HashSet<String> = [
            slack_owner(&screen, GroupKind::SkillListing),
            slack_owner(&screen, GroupKind::AgentListing),
        ]
        .into_iter()
        .collect();
        let texts: Vec<String> = screen
            .lines(&open, Some("slack"))
            .iter()
            .filter_map(|l| l.listed.as_ref().map(|n| n.text.clone()))
            .collect();
        // In their entries' places: the agent (1.0 KB) sorts before the unmeasured two.
        assert_eq!(
            texts,
            [
                "1 matching agent — listed above under Agent listing › slack",
                "2 matching entries — listed below under Skill listing › slack",
            ]
        );
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
    fn a_read_only_listing_of_off_entries_says_what_o_would_do() {
        let tree = without_listings();
        let off = [OffItem {
            item_id: "claude-code/skill/user/mine".to_string(),
            entry_id: "e1".to_string(),
            pointer: None,
        }];
        let detail = |read_only: bool| {
            build_with(&tree, &off, &HashMap::new(), Sort::Size, read_only)
                .groups
                .into_iter()
                .find(|g| g.kind == GroupKind::SkillListing)
                .unwrap()
                .detail
                .action
        };
        assert_eq!(
            detail(false),
            "Open it (→); o on an entry turns it back on."
        );
        assert_eq!(
            detail(true),
            "Open it (→); o on an entry would turn it back on."
        );
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
