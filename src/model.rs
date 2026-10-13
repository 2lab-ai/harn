//! The data model every module reads and writes (`.prd/03-p0-interfaces.md` §model.rs).
//!
//! Unit enums serialize as kebab-case strings; the same strings build item ids
//! (`<agent>/<kind>/<layer>/<name-or-realpath>`, see [`item_id`]), so a resolver and a capture
//! reader that both go through [`item_id`] produce ids that join.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

pub const SCHEMA: &str = "harn/1";

/// Implements `as_str` + `Display` with the exact strings serde uses (kept in sync by a test).
macro_rules! kebab_names {
    ($ty:ident { $($variant:ident => $name:literal),+ $(,)? }) => {
        impl $ty {
            pub const ALL: &'static [$ty] = &[$($ty::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($ty::$variant => $name),+ }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Agent {
    ClaudeCode,
    Codex,
}
kebab_names!(Agent { ClaudeCode => "claude-code", Codex => "codex" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Instruction,
    Memory,
    Hook,
    SkillListing,
    Skill,
    AgentRoster,
    SubAgent,
    Command,
    OutputStyle,
    PermissionRule,
    StatusLine,
    McpServer,
    McpTool,
    McpInstructions,
    Plugin,
    BuiltinTool,
    SystemPrompt,
    EnvBlock,
    GitBlock,
    Attribution,
    Unresolved,
}
kebab_names!(Kind {
    Instruction => "instruction",
    Memory => "memory",
    Hook => "hook",
    SkillListing => "skill-listing",
    Skill => "skill",
    AgentRoster => "agent-roster",
    SubAgent => "sub-agent",
    Command => "command",
    OutputStyle => "output-style",
    PermissionRule => "permission-rule",
    StatusLine => "status-line",
    McpServer => "mcp-server",
    McpTool => "mcp-tool",
    McpInstructions => "mcp-instructions",
    Plugin => "plugin",
    BuiltinTool => "builtin-tool",
    SystemPrompt => "system-prompt",
    EnvBlock => "env-block",
    GitBlock => "git-block",
    Attribution => "attribution",
    Unresolved => "unresolved",
});

/// Load mode (01-spec §The model). `Runtime` and `None` cost nothing in the first request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Always,
    Advertised,
    Deferred,
    OnDemand,
    Runtime,
    None,
}
kebab_names!(Mode {
    Always => "always",
    Advertised => "advertised",
    Deferred => "deferred",
    OnDemand => "on-demand",
    Runtime => "runtime",
    None => "none",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    Managed,
    User,
    Project,
    Local,
    Plugin,
    Dynamic,
    /// claude.ai remote connector. Spelled `claudeai` like the `init` event's provenance value.
    #[serde(rename = "claudeai")]
    ClaudeAi,
    Builtin,
    Unresolved,
}
kebab_names!(Layer {
    Managed => "managed",
    User => "user",
    Project => "project",
    Local => "local",
    Plugin => "plugin",
    Dynamic => "dynamic",
    ClaudeAi => "claudeai",
    Builtin => "builtin",
    Unresolved => "unresolved",
});

/// Write scope of a knob, narrowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Local,
    Project,
    User,
}
kebab_names!(Scope { Local => "local", Project => "project", User => "user" });

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub layer: Layer,
    /// Realpath (symlinks resolved).
    pub path: Option<PathBuf>,
    /// `"<file>#<json.pointer>"`.
    pub setting: Option<String>,
    /// `"name@marketplace"`.
    pub plugin: Option<String>,
    /// Admission chain, top-down.
    pub why: Vec<String>,
}

/// Byte cost with evidence tags. `?` = unknown (all None, not_applicable=false); `—` = not_applicable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cost {
    pub source: Option<u64>,
    pub predicted: Option<u64>,
    /// Rendered as `~`.
    pub predicted_is_estimate: bool,
    pub observed: Option<u64>,
    pub not_applicable: bool,
}

/// freed = known part + named unknowns. `delta_unknown` renders as `Δ?`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freed {
    pub known: u64,
    pub delta_unknown: bool,
    pub is_estimate: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Knob {
    Setting {
        file: PathBuf,
        /// JSON pointer.
        pointer: String,
        value: serde_json::Value,
        scope: Scope,
        note: Option<String>,
    },
    /// Via `settings.env`.
    Env {
        name: String,
        value: String,
        scope: Scope,
        note: Option<String>,
    },
    Quarantine {
        from: PathBuf,
        to: PathBuf,
        scope: Scope,
    },
    None {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    /// `<agent>/<kind-kebab>/<layer-kebab>/<name-or-realpath>`, see [`item_id`].
    pub id: String,
    pub label: String,
    pub kind: Kind,
    pub origin: Origin,
    pub mode: Mode,
    pub cost: Cost,
    /// Candidates, narrowest first; `[0]` = default.
    pub knobs: Vec<Knob>,
    /// For `knobs[0]`; filled by `plan::annotate_freed`.
    pub freed: Option<Freed>,
    /// e.g. `"over cap: 146 of 216 descriptions dropped"`.
    pub signal: Option<String>,
    /// ISO date.
    pub last_used: Option<String>,
    pub children: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Canonical.
    pub dir: PathBuf,
    /// `$HOME`, overridable in tests.
    pub home: PathBuf,
    pub env: BTreeMap<String, String>,
    pub model: Option<String>,
    pub context_window: Option<u64>,
    pub bytes_per_token: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub agent: Agent,
    pub target_dir: PathBuf,
    pub agent_version: Option<String>,
    pub rules_verified_against: String,
    pub tool_search: Option<bool>,
    pub signals: Vec<String>,
    pub items: Vec<Item>,
    pub predicted_known: u64,
    pub unknown_rows: u32,
    pub observed_body: Option<u64>,
    pub warnings: Vec<String>,
    /// `$HOME` the tree was resolved with (`Target.home`); renderers print it as `~`.
    #[serde(default)]
    pub home: Option<PathBuf>,
    /// The bound capture's leaf request regions `(name, bytes)`, a partition of `observed_body`;
    /// filled by `verify` (the header splits them into attributed / parameters / framing).
    #[serde(default)]
    pub observed_regions: Vec<(String, u64)>,
    /// Why tool search is on or off, e.g. `ANTHROPIC_BASE_URL is non-first-party (localhost)`.
    #[serde(default)]
    pub tool_search_cause: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// [`SCHEMA`].
    pub schema: String,
    pub generated_at: String,
    pub agents: Vec<Tree>,
}

/// Virtual changes applied while resolving (for freed computation). Resolvers MUST honour it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum OverlayOp {
    SetJson {
        file: PathBuf,
        pointer: String,
        value: serde_json::Value,
    },
    RemovePath {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    pub ops: Vec<OverlayOp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunContext {
    pub cwd: PathBuf,
    pub agent: Agent,
    pub agent_version: Option<String>,
    pub model: Option<String>,
    pub launch_opts: Vec<String>,
    pub settings_fingerprint: Option<String>,
    pub capture_kind: String,
    pub captured_at: String,
}

/// Builds an item id: `<agent>/<kind-kebab>/<layer-kebab>/<name-or-realpath>`.
/// Files are named by realpath, so `name` keeps its leading `/`
/// (`claude-code/instruction/project//Users/me/repo/CLAUDE.md`).
pub fn item_id(agent: Agent, kind: Kind, layer: Layer, name: &str) -> String {
    format!("{agent}/{kind}/{layer}/{name}")
}

/// Current UTC time as RFC 3339 (`2026-10-13T04:05:06.123456Z`), for `generated_at`,
/// `captured_at` and `applied_at`.
pub fn now_iso8601() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting of the current UTC time cannot fail")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// Item ids of every candidate, sorted.
    Ambiguous(Vec<String>),
    /// The selector as given.
    NotFound(String),
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectError::Ambiguous(candidates) => {
                write!(f, "ambiguous selector, {} candidates:", candidates.len())?;
                for id in candidates {
                    write!(f, "\n  {id}")?;
                }
                Ok(())
            }
            SelectError::NotFound(selector) => write!(f, "no item matches `{selector}`"),
        }
    }
}

impl std::error::Error for SelectError {}

/// A refused operation (built-in item, no knob in the asked scope, ...). Library code returns it
/// inside `anyhow::Error`; the CLI maps it to exit 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "refused: {}", self.0)
    }
}

impl std::error::Error for Refused {}

/// Journal targets changed since harn wrote them; nothing was written. Library code returns it
/// inside `anyhow::Error`; the CLI maps it to exit 4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub files: Vec<PathBuf>,
}

impl fmt::Display for Stale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "stale: {} file(s) changed since harn wrote them, nothing written:",
            self.files.len()
        )?;
        for file in &self.files {
            write!(f, "\n  {}", file.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for Stale {}

/// Item selector: exact id, else unique suffix of id, else unique label, else unique substring
/// of id.
///
/// A stage that matches exactly one item wins. When no stage is unique, every candidate seen in
/// the suffix and label stages is returned as `Ambiguous`. Only when those stages matched nothing
/// does the substring stage run; there a match inside another match is that ancestor's part (an
/// MCP server's instructions and tools under `plugin:slack`) and is folded into it. When nothing
/// matched, `NotFound`.
pub fn select<'a>(tree: &'a Tree, selector: &str) -> Result<&'a Item, SelectError> {
    let mut candidates: Vec<&Item> = Vec::new();
    let stages: [&dyn Fn(&Item) -> bool; 3] = [
        &|item| item.id == selector,
        &|item| item.id.ends_with(selector),
        &|item| item.label == selector,
    ];
    for matches in stages {
        let found: Vec<&Item> = walk(tree).filter(|item| matches(item)).collect();
        if let [only] = found.as_slice() {
            return Ok(only);
        }
        for item in found {
            if !candidates.iter().any(|c| std::ptr::eq(*c, item)) {
                candidates.push(item);
            }
        }
    }
    if candidates.is_empty() {
        let found: Vec<&Item> = walk(tree)
            .filter(|item| item.id.contains(selector))
            .collect();
        let outermost: Vec<&Item> = found
            .iter()
            .copied()
            .filter(|item| !found.iter().any(|other| is_inside(item, other)))
            .collect();
        if let [only] = outermost.as_slice() {
            return Ok(only);
        }
        candidates = outermost;
    }
    if candidates.is_empty() {
        return Err(SelectError::NotFound(selector.to_string()));
    }
    let mut ids: Vec<String> = candidates.iter().map(|item| item.id.clone()).collect();
    ids.sort();
    Err(SelectError::Ambiguous(ids))
}

/// `item` is a strict descendant of `ancestor`.
fn is_inside(item: &Item, ancestor: &Item) -> bool {
    ancestor
        .children
        .iter()
        .any(|child| std::ptr::eq(child, item) || is_inside(item, child))
}

/// Every item of the tree, depth-first (parent before its children, in stored order).
pub fn walk(tree: &Tree) -> impl Iterator<Item = &Item> {
    let mut stack: Vec<&Item> = tree.items.iter().rev().collect();
    std::iter::from_fn(move || {
        let item = stack.pop()?;
        stack.extend(item.children.iter().rev());
        Some(item)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, label: &str, children: Vec<Item>) -> Item {
        Item {
            id: id.to_string(),
            label: label.to_string(),
            kind: Kind::Instruction,
            origin: Origin {
                layer: Layer::Project,
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
            children,
        }
    }

    fn tree(items: Vec<Item>) -> Tree {
        Tree {
            agent: Agent::ClaudeCode,
            target_dir: PathBuf::from("/repo"),
            agent_version: None,
            rules_verified_against: "0.0.0".to_string(),
            tool_search: None,
            signals: Vec::new(),
            items,
            predicted_known: 0,
            unknown_rows: 0,
            observed_body: None,
            warnings: Vec::new(),
            home: None,
            observed_regions: Vec::new(),
            tool_search_cause: None,
        }
    }

    const SLACK: &str = "claude-code/mcp-server/plugin/plugin:slack:slack";
    const SLACK_TOOL: &str = "claude-code/mcp-tool/plugin/plugin:slack:slack/send_message";
    const DEV: &str = "claude-code/instruction/project//repo/rules/DEV.md";
    const USER_MD: &str = "claude-code/instruction/user//home/.claude/CLAUDE.md";
    const PROJECT_MD: &str = "claude-code/instruction/project//repo/CLAUDE.md";

    fn sample() -> Tree {
        tree(vec![
            item(USER_MD, "~/.claude/CLAUDE.md", Vec::new()),
            item(
                PROJECT_MD,
                "CLAUDE.md",
                vec![item(DEV, "rules/DEV.md", Vec::new())],
            ),
            item(
                SLACK,
                "plugin:slack:slack",
                vec![item(SLACK_TOOL, "send_message", Vec::new())],
            ),
        ])
    }

    #[test]
    fn walk_is_depth_first_parent_before_children() {
        let t = sample();
        let ids: Vec<&str> = walk(&t).map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec![USER_MD, PROJECT_MD, DEV, SLACK, SLACK_TOOL]);
    }

    #[test]
    fn walk_of_empty_tree_is_empty() {
        assert_eq!(walk(&tree(Vec::new())).count(), 0);
    }

    #[test]
    fn select_exact_id() {
        let t = sample();
        assert_eq!(select(&t, PROJECT_MD).unwrap().id, PROJECT_MD);
    }

    #[test]
    fn exact_id_wins_over_a_longer_id_with_the_same_suffix() {
        // `short` is also a suffix of `long`; the exact stage must decide before the suffix stage.
        let short = "claude-code/skill/user/x";
        let long = "claude-code/skill/user/claude-code/skill/user/x";
        let t = tree(vec![
            item(long, "a", Vec::new()),
            item(short, "b", Vec::new()),
        ]);
        assert_eq!(select(&t, short).unwrap().id, short);
    }

    #[test]
    fn select_unique_suffix_reaches_children() {
        let t = sample();
        assert_eq!(select(&t, "rules/DEV.md").unwrap().id, DEV);
        assert_eq!(select(&t, "send_message").unwrap().id, SLACK_TOOL);
        assert_eq!(select(&t, "plugin:slack:slack").unwrap().id, SLACK);
    }

    #[test]
    fn select_unique_label_when_no_suffix_matches() {
        let t = sample();
        assert_eq!(select(&t, "~/.claude/CLAUDE.md").unwrap().id, USER_MD);
    }

    #[test]
    fn select_unique_label_breaks_an_ambiguous_suffix() {
        // "CLAUDE.md" is a suffix of two ids but the label of exactly one item.
        let t = sample();
        assert_eq!(select(&t, "CLAUDE.md").unwrap().id, PROJECT_MD);
    }

    #[test]
    fn select_ambiguous_lists_every_candidate_sorted() {
        let t = sample();
        let err = select(&t, ".md").unwrap_err();
        let mut expected = vec![USER_MD.to_string(), PROJECT_MD.to_string(), DEV.to_string()];
        expected.sort();
        assert_eq!(err, SelectError::Ambiguous(expected));
        assert!(err.to_string().contains(DEV));
    }

    #[test]
    fn select_ambiguous_label_without_suffix_match() {
        let t = tree(vec![
            item("claude-code/skill/user/a", "same", Vec::new()),
            item("claude-code/skill/project/b", "same", Vec::new()),
        ]);
        assert_eq!(
            select(&t, "same").unwrap_err(),
            SelectError::Ambiguous(vec![
                "claude-code/skill/project/b".to_string(),
                "claude-code/skill/user/a".to_string(),
            ])
        );
    }

    #[test]
    fn select_unique_substring_takes_the_outermost_match() {
        // `plugin:slack` is no suffix and no label; it is inside the server's id and its tool's.
        let t = sample();
        assert_eq!(select(&t, "plugin:slack").unwrap().id, SLACK);
    }

    #[test]
    fn select_substring_in_two_unrelated_ids_lists_both() {
        let mut t = sample();
        let other = "claude-code/mcp-server/plugin/plugin:slack-extra:x";
        t.items
            .push(item(other, "plugin:slack-extra:x", Vec::new()));
        assert_eq!(
            select(&t, "plugin:slack").unwrap_err(),
            SelectError::Ambiguous(vec![other.to_string(), SLACK.to_string()])
        );
    }

    #[test]
    fn select_substring_never_breaks_an_ambiguous_suffix() {
        // Two ids end with `DEV.md`; the substring stage must not pick one of them.
        let mut t = sample();
        t.items.push(item(
            "claude-code/instruction/user//home/DEV.md",
            "~/DEV.md",
            Vec::new(),
        ));
        assert!(matches!(
            select(&t, "DEV.md"),
            Err(SelectError::Ambiguous(ids)) if ids.len() == 2
        ));
    }

    #[test]
    fn select_not_found() {
        let t = sample();
        assert_eq!(
            select(&t, "nope").unwrap_err(),
            SelectError::NotFound("nope".to_string())
        );
    }

    #[test]
    fn enums_serialize_as_their_kebab_names() {
        fn check<T: Serialize + fmt::Display>(all: &[T]) {
            for v in all {
                assert_eq!(
                    serde_json::to_value(v).unwrap(),
                    serde_json::Value::String(v.to_string())
                );
            }
        }
        check(Agent::ALL);
        check(Kind::ALL);
        check(Mode::ALL);
        check(Layer::ALL);
        check(Scope::ALL);
        assert_eq!(Layer::ClaudeAi.as_str(), "claudeai");
        assert_eq!(Kind::McpServer.as_str(), "mcp-server");
        assert_eq!(Mode::OnDemand.as_str(), "on-demand");
    }

    #[test]
    fn item_id_follows_the_id_grammar() {
        assert_eq!(
            item_id(
                Agent::ClaudeCode,
                Kind::McpServer,
                Layer::Plugin,
                "plugin:slack:slack"
            ),
            SLACK
        );
        assert_eq!(
            item_id(
                Agent::ClaudeCode,
                Kind::Instruction,
                Layer::Project,
                "/repo/CLAUDE.md"
            ),
            PROJECT_MD
        );
    }

    #[test]
    fn knob_is_internally_tagged() {
        let knob = Knob::None {
            reason: "built-in".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&knob).unwrap(),
            r#"{"type":"none","reason":"built-in"}"#
        );
    }

    #[test]
    fn now_iso8601_is_utc_rfc3339() {
        let now = now_iso8601();
        assert!(now.ends_with('Z'), "{now}");
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], "T");
    }
}
