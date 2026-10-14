//! Claude Code discovery rules (WU-B). The rules are `.prd/research/01-claude-code-loading-rules.md`
//! and `.prd/01-spec.md` §Claude Code coverage: settings layering, the instruction chain with
//! `@imports` and rules, auto-memory (200 lines / 25 KB cap), hooks, the skill listing (1 % budget,
//! 1,536-char cut), the agent roster, commands / output style / statusLine / permission rules, MCP
//! servers by scope, plugins (`installed_plugins.json` + `enabledPlugins`), built-ins and the
//! tool-search state.
//!
//! Reads files only, all of them through [`fs::Fs`] so an [`Overlay`] is honoured everywhere;
//! never spawns a process and never touches the network.
//!
//! Totals rule (relied on by render and plan): `predicted_known` = Σ `cost.predicted` over every
//! item (any depth) with mode `always` or `advertised`, skipping `not_applicable` rows and
//! containers whose cost is the sum of their children (`skill-listing`, `agent-roster`).
//! `unknown_rows` counts the `?` rows (no source, predicted or observed; not `not_applicable`)
//! with mode `always`, `advertised` or `deferred`, under the same skips.
//! A `plugin` row is `not_applicable`: its bytes are on its contribution rows (`origin.plugin`).

mod agents;
mod builtins;
pub(crate) mod frontmatter;
pub(crate) mod fs;
mod glob;
mod hooks;
mod instructions;
mod mcp;
mod memory;
mod plugins;
mod settings;
mod skills;

use crate::model::{
    item_id, walk, Agent, Cost, Item, Kind, Knob, Layer, Mode, Origin, Overlay, Scope, Target, Tree,
};
use fs::{Fs, Json};
pub use hooks::{OFF_ALL_HOOKS, OFF_MANAGED_HOOKS_ONLY};
use serde_json::Value;
use settings::Settings;
use std::path::{Path, PathBuf};

/// `Tree.warnings` entry every resolve pushes; `verify` drops it once a capture is bound.
pub const CAP_STATE_UNKNOWN: &str =
    "skill listing budget is token-denominated; cap state unknown until a capture";

pub(crate) struct Ctx<'a> {
    pub target: &'a Target,
    pub fs: Fs,
    pub settings: Settings,
    /// `~/.claude.json`.
    pub claude_json: Option<Value>,
    pub claude_json_path: PathBuf,
    /// Nearest directory at or above the target holding `.git`.
    pub git_root: Option<PathBuf>,
    pub warnings: Vec<String>,
}

impl Ctx<'_> {
    pub fn home(&self) -> &Path {
        &self.target.home
    }

    pub fn dir(&self) -> &Path {
        &self.target.dir
    }

    pub fn settings_file(&self, scope: Scope) -> PathBuf {
        settings::settings_path(self.dir(), self.home(), scope)
    }

    /// Settings `env` wins over the process environment (Claude Code applies it at startup).
    pub fn env(&self, name: &str) -> Option<String> {
        self.settings
            .env(name)
            .map(|(v, _)| v)
            .or_else(|| self.target.env.get(name).cloned())
    }

    pub fn env_on(&self, name: &str) -> bool {
        self.env(name)
            .is_some_and(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
    }

    /// `~/.claude.json` → `projects["<dir>"]`.
    pub fn project_json(&self) -> Option<&Value> {
        self.claude_json
            .as_ref()?
            .get("projects")?
            .get(self.dir().to_str()?)
    }

    /// JSON pointer of `projects["<dir>"]` in `~/.claude.json`.
    pub fn project_pointer(&self) -> String {
        format!("/projects/{}", fs::esc(&self.dir().display().to_string()))
    }

    /// `~/…` for paths under home, the target-relative path for paths under the target.
    pub fn display(&self, path: &Path) -> String {
        if let Ok(rel) = path.strip_prefix(self.dir()) {
            if !rel.as_os_str().is_empty() {
                return rel.display().to_string();
            }
        }
        if let Ok(rel) = path.strip_prefix(self.home()) {
            return format!("~/{}", rel.display());
        }
        path.display().to_string()
    }

    /// The target dir and each parent up to the git root (closest first), minus `$HOME`, whose
    /// `.claude/` is the user layer.
    pub fn project_dirs(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut cur = Some(self.dir());
        while let Some(dir) = cur {
            if dir != self.home() {
                out.push(dir.to_path_buf());
            }
            match &self.git_root {
                Some(root) if dir != root.as_path() => cur = dir.parent(),
                _ => break,
            }
        }
        out
    }

    /// Filesystem root down to the target dir.
    pub fn chain_dirs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.dir().ancestors().map(Path::to_path_buf).collect();
        out.reverse();
        out
    }
}

pub(crate) fn new_item(kind: Kind, layer: Layer, name: &str, label: &str) -> Item {
    Item {
        id: item_id(Agent::ClaudeCode, kind, layer, name),
        label: label.to_string(),
        kind,
        origin: Origin {
            layer,
            path: None,
            setting: None,
            plugin: None,
            why: Vec::new(),
        },
        mode: Mode::Always,
        cost: Cost::default(),
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

/// `—`: no first-request cost (mode runtime / none / on-demand, or a pointer row).
pub(crate) fn cost_na(source: Option<u64>) -> Cost {
    Cost {
        source,
        not_applicable: true,
        ..Cost::default()
    }
}

pub(crate) fn is_unknown(cost: &Cost) -> bool {
    cost.source.is_none()
        && cost.predicted.is_none()
        && cost.observed.is_none()
        && !cost.not_applicable
}

pub(crate) fn setting_knob(
    ctx: &Ctx,
    scope: Scope,
    pointer: &str,
    value: Value,
    note: Option<String>,
) -> Knob {
    Knob::Setting {
        file: ctx.settings_file(scope),
        pointer: pointer.to_string(),
        value,
        scope,
        note,
    }
}

pub(crate) fn env_knob(name: &str, value: &str, scope: Scope, note: Option<&str>) -> Knob {
    Knob::Env {
        name: name.to_string(),
        value: value.to_string(),
        scope,
        note: note.map(str::to_string),
    }
}

/// `projects["<dir>"].disabledMcpServers += name` in `~/.claude.json` (the `/mcp` toggle).
pub(crate) fn disabled_mcp_knob(ctx: &Ctx, name: &str, note: &str) -> Knob {
    Knob::Setting {
        file: ctx.claude_json_path.clone(),
        pointer: format!("{}/disabledMcpServers/-", ctx.project_pointer()),
        value: Value::String(name.to_string()),
        scope: Scope::Local,
        note: Some(note.to_string()),
    }
}

/// A link's string value as JSON (`"~/memory (old)"`): every link the resolver writes ends in
/// `true`, `false`, a JSON string or a `(note)`, so a reader can tell where the pointer ends.
pub(crate) fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string serializes")
}

/// Containers whose cost is the sum of their children.
pub(crate) fn is_container(item: &Item) -> bool {
    matches!(item.kind, Kind::SkillListing | Kind::AgentRoster)
}

pub fn resolve(target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree> {
    let fs = Fs::new(overlay);
    let mut warnings = Vec::new();
    let settings = Settings::load(&fs, &target.dir, &target.home, &mut warnings);
    let claude_json_path = target.home.join(".claude.json");
    let claude_json = match fs.read_json(&claude_json_path) {
        Json::Ok(v) => Some(v),
        Json::Invalid(err) => {
            warnings.push(format!(
                "{}: invalid JSON, ignored ({err})",
                claude_json_path.display()
            ));
            None
        }
        Json::Missing => None,
    };
    let git_root = find_git_root(&fs, &target.dir);
    let mut ctx = Ctx {
        target,
        fs,
        settings,
        claude_json,
        claude_json_path,
        git_root,
        warnings,
    };

    let (tool_search, tool_search_cause) = tool_search(&ctx);
    let plugins = plugins::discover(&mut ctx);
    let mut items = Vec::new();
    items.extend(builtins::blocks(&ctx));
    items.extend(instructions::collect(&mut ctx));
    items.extend(memory::collect(&mut ctx));
    items.extend(hooks::collect(&ctx, &plugins));
    items.push(skills::listing(&ctx, &plugins));
    items.push(agents::roster(&ctx, &plugins));
    items.extend(mcp::collect(&mut ctx, &plugins, tool_search));
    items.extend(builtins::settings_rows(&ctx));
    let plugin_rows = plugins::items(&plugins, &items);
    items.extend(plugin_rows);

    let agent_version = agent_version(target);
    let mut warnings = ctx.warnings;
    warnings.push(CAP_STATE_UNKNOWN.to_string());
    if let Some(v) = agent_version.as_deref() {
        if version_key(v) > version_key(super::ClaudeCode::VERIFIED_AGAINST) {
            warnings.push(format!(
                "claude-code {v} is newer than the rules (verified against {}); run `harn verify`",
                super::ClaudeCode::VERIFIED_AGAINST
            ));
        }
    }
    if tool_search.is_none() {
        warnings.push(
            "tool search state undecidable statically (ENABLE_TOOL_SEARCH=auto or an unparsable ANTHROPIC_BASE_URL)"
                .to_string(),
        );
    }

    let mut tree = Tree {
        agent: Agent::ClaudeCode,
        target_dir: target.dir.clone(),
        agent_version,
        rules_verified_against: super::ClaudeCode::VERIFIED_AGAINST.to_string(),
        tool_search,
        signals: Vec::new(),
        items,
        predicted_known: 0,
        unknown_rows: 0,
        observed_body: None,
        warnings,
        home: Some(target.home.clone()),
        observed_regions: Vec::new(),
        tool_search_cause: Some(tool_search_cause),
    };
    let (known, unknown) = totals(&tree);
    tree.predicted_known = known;
    tree.unknown_rows = unknown;
    Ok(tree)
}

/// [`crate::resolve::Resolver::after_removal`] for Claude Code. Only a bare tool name appended
/// to `permissions.deny` qualifies: `builtins::blocks` drops exactly the built-in tool row of
/// that name; the deny list concatenates across layers, so no other layer can undo the entry;
/// the only other reader of `permissions.deny` is the `permission-rule` row, which has no
/// first-request cost. Every other knob returns None and is re-resolved, notably
/// `skillOverrides` (an `off` user skill un-shadows a bundled skill or built-in command of the
/// same name, `skills.rs` `Listing::push`), env knobs (settings env layers and the process env
/// decide together; `CLAUDE_CODE_DISABLE_AUTO_MEMORY=0` overrides `autoMemoryEnabled`) and
/// `claudeMdExcludes` (a glob; imports re-home under another importer).
pub(crate) fn after_removal(tree: &Tree, item: &Item, knob: &Knob) -> Option<Tree> {
    let Knob::Setting {
        pointer,
        value: Value::String(tool),
        ..
    } = knob
    else {
        return None;
    };
    let tool_row = item_id(Agent::ClaudeCode, Kind::BuiltinTool, Layer::Builtin, tool);
    if pointer != "/permissions/deny/-" || item.id != tool_row || tool.contains('(') {
        return None;
    }
    // Another row carrying the same entry would leave too.
    if walk(tree).filter(|i| i.knobs.contains(knob)).count() != 1 {
        return None;
    }
    let mut after = tree.clone();
    let before = after.items.len();
    after.items.retain(|i| i.id != item.id);
    if after.items.len() + 1 != before {
        return None;
    }
    let (known, unknown) = totals(&after);
    after.predicted_known = known;
    after.unknown_rows = unknown;
    Some(after)
}

pub(crate) fn totals(tree: &Tree) -> (u64, u32) {
    let mut known = 0;
    let mut unknown = 0;
    for item in walk(tree) {
        if is_container(item) || item.cost.not_applicable {
            continue;
        }
        if matches!(item.mode, Mode::Always | Mode::Advertised) {
            known += item.cost.predicted.unwrap_or(0);
        }
        if matches!(item.mode, Mode::Always | Mode::Advertised | Mode::Deferred)
            && is_unknown(&item.cost)
        {
            unknown += 1;
        }
    }
    (known, unknown)
}

/// `ENABLE_TOOL_SEARCH` decides when set (`auto*` is undecidable); otherwise tool search is on
/// for the first-party endpoint and off behind any other `ANTHROPIC_BASE_URL` (`research/01` §7).
/// Returns the state and its cause (host only: a base URL may carry credentials).
fn tool_search(ctx: &Ctx) -> (Option<bool>, String) {
    if let Some(v) = ctx.env("ENABLE_TOOL_SEARCH") {
        let state = match v.trim() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        };
        return (state, format!("ENABLE_TOOL_SEARCH={}", v.trim()));
    }
    if ctx.env_on("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS") {
        return (
            Some(false),
            "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS is set".to_string(),
        );
    }
    match ctx.env("ANTHROPIC_BASE_URL") {
        None => (
            Some(true),
            "ANTHROPIC_BASE_URL unset (first-party)".to_string(),
        ),
        Some(url) if url.trim().is_empty() => (
            Some(true),
            "ANTHROPIC_BASE_URL empty (first-party)".to_string(),
        ),
        Some(url) => match url_host(&url) {
            Some(host) if host == "anthropic.com" || host.ends_with(".anthropic.com") => (
                Some(true),
                format!("ANTHROPIC_BASE_URL is first-party ({host})"),
            ),
            Some(host) => (
                Some(false),
                format!("ANTHROPIC_BASE_URL is non-first-party ({host})"),
            ),
            None => (None, "ANTHROPIC_BASE_URL has no parsable host".to_string()),
        },
    }
}

fn url_host(url: &str) -> Option<String> {
    let (_, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next()?
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn find_git_root(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| fs.is_dir(&d.join(".git")) || fs.is_file(&d.join(".git")))
        .map(Path::to_path_buf)
}

/// The main worktree root for a linked worktree (`.git` file → `commondir`), else the git root.
pub(crate) fn main_repo_root(ctx: &Ctx) -> Option<PathBuf> {
    let root = ctx.git_root.clone()?;
    let dotgit = root.join(".git");
    if !ctx.fs.is_file(&dotgit) {
        return Some(root);
    }
    let text = ctx.fs.read_string(&dotgit)?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = root.join(gitdir);
    let common = ctx
        .fs
        .read_string(&gitdir.join("commondir"))
        .map(|c| fs::normalize(&gitdir.join(c.trim())));
    match common {
        Some(common) if common.file_name().is_some_and(|n| n == ".git") => {
            common.parent().map(Path::to_path_buf)
        }
        _ => Some(root),
    }
}

/// The installed Claude Code version: the `~/.local/bin/claude` symlink's target, else the newest
/// `versions/*` entry. Files only.
fn agent_version(target: &Target) -> Option<String> {
    if let Ok(link) = std::fs::read_link(target.home.join(".local/bin/claude")) {
        let name = fs::file_name(&link);
        if version_key(&name).is_some() {
            return Some(name);
        }
    }
    let mut best: Option<String> = None;
    for dir in [".local/share/claude/versions", ".claude/versions"] {
        let Ok(entries) = std::fs::read_dir(target.home.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if version_key(&name).is_some()
                && best
                    .as_deref()
                    .is_none_or(|b| version_key(&name) > version_key(b))
            {
                best = Some(name);
            }
        }
    }
    best
}

fn version_key(v: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = v.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| p.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_host_parses_common_forms() {
        assert_eq!(
            url_host("http://localhost:3456").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            url_host("https://api.anthropic.com/v1").as_deref(),
            Some("api.anthropic.com")
        );
        assert_eq!(
            url_host("https://u:p@h.example:8/x").as_deref(),
            Some("h.example")
        );
        assert_eq!(url_host("localhost:3456"), None);
    }

    #[test]
    fn version_key_orders_numerically() {
        assert!(version_key("2.1.296") > version_key("2.1.9"));
        assert_eq!(version_key("latest"), None);
    }
}
