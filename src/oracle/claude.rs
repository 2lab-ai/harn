//! Claude Code oracle (WU-D): one headless `claude -p` under `OTEL_LOG_RAW_API_BODIES=file:<tmp>`
//! with `--output-format stream-json --verbose`; the request body and the `system/init` event.
//!
//! # How a body is cut into regions (measured on Claude Code 2.1.296, 2026-10-13)
//!
//! Text regions count the UTF-8 bytes of the decoded text; `tools`, `tool_addition` and the
//! request parameters count their compact JSON (the wire form); `framing` is the remainder
//! (JSON structure and escapes), so the regions partition the body exactly.
//!
//! Inside a text block a **section starts at the newline before its header line** (or at 0 when
//! the header is the first line) and runs to the start of the next section. Claude Code separates
//! sections with a blank line, so every section is `"\n" + header line + "\n" + body`, and a
//! `Contents of` section is exactly `"\n" + "Contents of <path> (<desc>):" + "\n\n" + file bytes`
//! — the formula a resolver's `predicted` must follow for content conformance. The
//! `<system-reminder>` preamble and the closing tag of a block are the block's own wrapper,
//! attributed to no item.
//!
//! Listing entries (`- name: description` or `- name`) run to the next entry line and include
//! their trailing newline; the last entry ends at its first blank line, so a note after the
//! entries stays with the container.

use super::{Capture, Region};
use crate::model::{
    item_id, now_iso8601, Agent, Cost, Item, Kind, Layer, Mode, Origin, RunContext, Target, Tree,
};
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Names of the items that occur once per request. A resolver builds the same ids with
/// `item_id(Agent::ClaudeCode, kind, Layer::Builtin, name)` so `verify` joins them.
pub mod names {
    /// `Kind::SystemPrompt`: the `system` array; children `system/<part>`.
    pub const SYSTEM_PROMPT: &str = "system";
    /// `Kind::SkillListing`: the Skill tool's listing in the system text.
    pub const SKILL_LISTING: &str = "skills";
    /// `Kind::AgentRoster`: "Available agent types for the Agent tool".
    pub const AGENT_ROSTER: &str = "agents";
    /// `Kind::EnvBlock`: the `# Environment` section.
    pub const ENVIRONMENT: &str = "environment";
    /// `Kind::EnvBlock`: `Today's date is …`.
    pub const DATE: &str = "date";
    /// `Kind::GitBlock`: the `# gitStatus` context section.
    pub const GIT_STATUS: &str = "gitStatus";
    /// `Kind::Attribution`: the `# userEmail` context section (no switch).
    pub const USER_EMAIL: &str = "userEmail";
}

/// `Item.signal` of an observed listing entry that carries no description.
pub const NAME_ONLY: &str = "name only";

/// The arguments `run_claude` passes (also recorded as `RunContext.launch_opts`).
pub const LAUNCH_ARGS: &[&str] = &[
    "-p",
    "hi",
    "--output-format",
    "stream-json",
    "--verbose",
    "--max-turns",
    "1",
    "--disallowedTools",
    "Edit,Write,NotebookEdit,Bash",
];

/// How long one headless run may take before it is killed.
const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The `system/init` stream-json event: what is enabled, from where (`research/04` §E.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitEvent {
    pub tools: Vec<String>,
    pub mcp_servers: Vec<InitMcpServer>,
    pub skills: Vec<String>,
    pub agents: Vec<String>,
    pub plugins: Vec<InitPlugin>,
    /// e.g. `{"auto": "~/.claude/projects/<slug>/memory/"}`.
    pub memory_paths: BTreeMap<String, String>,
    pub claude_code_version: Option<String>,
    pub model: Option<String>,
    /// The session's working directory.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitMcpServer {
    pub name: String,
    pub status: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitPlugin {
    pub name: String,
    pub path: Option<PathBuf>,
    pub source: Option<String>,
    pub version: Option<String>,
}

/// Parses one stream-json line that must be the `system/init` event.
pub fn parse_init_event(line: &str) -> anyhow::Result<InitEvent> {
    let v: Value = serde_json::from_str(line.trim()).context("init event: not JSON")?;
    if v["type"] != "system" || v["subtype"] != "init" {
        bail!(
            "not a system/init event (type {}, subtype {})",
            v["type"],
            v["subtype"]
        );
    }
    let text = |v: &Value| v.as_str().map(str::to_string);
    let strings = |key: &str| -> Vec<String> {
        v[key]
            .as_array()
            .map(|a| a.iter().filter_map(text).collect())
            .unwrap_or_default()
    };
    let objects = |key: &str| v[key].as_array().cloned().unwrap_or_default();
    Ok(InitEvent {
        tools: strings("tools"),
        mcp_servers: objects("mcp_servers")
            .iter()
            .filter_map(|s| {
                Some(InitMcpServer {
                    name: text(&s["name"])?,
                    status: text(&s["status"]).unwrap_or_default(),
                    source: text(&s["source"]),
                })
            })
            .collect(),
        skills: strings("skills"),
        agents: strings("agents"),
        plugins: objects("plugins")
            .iter()
            .filter_map(|p| {
                Some(InitPlugin {
                    name: text(&p["name"])?,
                    path: text(&p["path"]).map(PathBuf::from),
                    source: text(&p["source"]),
                    version: text(&p["version"]),
                })
            })
            .collect(),
        memory_paths: v["memory_paths"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), text(v)?)))
                    .collect()
            })
            .unwrap_or_default(),
        claude_code_version: text(&v["claude_code_version"]),
        model: text(&v["model"]),
        cwd: text(&v["cwd"]).map(PathBuf::from),
        session_id: text(&v["session_id"]),
    })
}

/// The `system/init` event of a stream-json transcript. Hook events (`hook_started`,
/// `hook_response`) come before it, so it is searched for, not taken from line 1.
pub fn find_init_event(stream: &str) -> anyhow::Result<InitEvent> {
    stream
        .lines()
        .filter(|line| line.contains("\"init\""))
        .find_map(|line| parse_init_event(line).ok())
        .ok_or_else(|| anyhow!("no system/init event in the stream-json output"))
}

/// Claude Code's MCP name normalisation (tool prefix `mcp__<server>__`): every character outside
/// `[A-Za-z0-9_-]` becomes `_`.
pub(crate) fn normalize_mcp_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `major.minor.patch` of a version string (`2.1.296.406` → `2.1.296`), for comparing the
/// billing header's `cc_version` with `claude --version` and the init event.
pub(crate) fn version_triple(version: &str) -> String {
    let version = version.split_whitespace().next().unwrap_or("");
    version.split('.').take(3).collect::<Vec<_>>().join(".")
}

/// Byte length of the compact JSON of `value` (the wire form).
fn compact_len(value: &Value) -> u64 {
    serde_json::to_vec(value).map_or(0, |v| v.len() as u64)
}

/// A section of a text block: `[start, end)` in bytes, with what its header said.
#[derive(Debug, Clone)]
struct Section<T> {
    start: usize,
    end: usize,
    tag: T,
}

/// Splits `text[..limit]` at header lines (see the module docs). Returns the end of the lead
/// (text before the first header) and the sections in order.
fn split_sections<T>(
    text: &str,
    limit: usize,
    mut header: impl FnMut(&str) -> Option<T>,
) -> (usize, Vec<Section<T>>) {
    let mut sections: Vec<Section<T>> = Vec::new();
    let mut offset: usize = 0;
    for line in text[..limit].split('\n') {
        if let Some(tag) = header(line) {
            let start = offset.saturating_sub(1);
            if let Some(last) = sections.last_mut() {
                last.end = start;
            }
            sections.push(Section {
                start,
                end: limit,
                tag,
            });
        }
        offset += line.len() + 1;
    }
    let lead = sections.first().map_or(limit, |s| s.start);
    (lead, sections)
}

/// Start of a trailing `</system-reminder>` (whitespace after it allowed), else `text.len()`.
fn closing_tag_start(text: &str) -> usize {
    const TAG: &str = "</system-reminder>";
    let trimmed = text.trim_end();
    if trimmed.ends_with(TAG) {
        trimmed.len() - TAG.len()
    } else {
        text.len()
    }
}

/// One `- name…` entry of a listing, relative to the listing text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    described: bool,
    start: usize,
    end: usize,
}

/// Entries of a listing section (see the module docs for the boundaries).
fn listing_entries(section: &str) -> Vec<Entry> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut offset = 0;
    for line in section.split('\n') {
        lines.push((offset, line));
        offset += line.len() + 1;
    }
    let line_end = |i: usize| (lines[i].0 + lines[i].1.len() + 1).min(section.len());
    // A bullet inside a description (`- When creating …`) is not an entry: names have no spaces.
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].1.starts_with("- "))
        .filter(|&i| {
            let (name, _) = entry_name(lines[i].1);
            !name.is_empty() && !name.contains(char::is_whitespace)
        })
        .collect();
    let mut entries = Vec::new();
    for (k, &i) in starts.iter().enumerate() {
        let end = match starts.get(k + 1) {
            Some(&next) => lines[next].0,
            None => {
                let mut last = i;
                while last + 1 < lines.len() && !lines[last + 1].1.is_empty() {
                    last += 1;
                }
                line_end(last)
            }
        };
        let (name, described) = entry_name(lines[i].1);
        entries.push(Entry {
            name,
            described,
            start: lines[i].0,
            end,
        });
    }
    entries
}

/// `- name: description` → (`name`, true); `- name` → (`name`, false). A display alias
/// (`- name (alias)`) is dropped so the name joins the `init` event's.
fn entry_name(line: &str) -> (String, bool) {
    let rest = line.strip_prefix("- ").unwrap_or(line);
    let (name, described) = match rest.find(": ") {
        Some(i) => (&rest[..i], true),
        None => (rest.trim_end().trim_end_matches(':'), false),
    };
    let name = match (name.ends_with(')'), name.rfind(" (")) {
        (true, Some(i)) => &name[..i],
        _ => name,
    };
    (name.trim().to_string(), described)
}

/// `Contents of <path> (<desc>):` → (path, desc).
fn contents_header(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("Contents of ")?.strip_suffix("):")?;
    let open = rest.rfind(" (")?;
    Some((rest[..open].to_string(), rest[open + 2..].to_string()))
}

/// What a `Contents of` header's description says about the file.
fn contents_kind(desc: &str) -> (Kind, Layer) {
    let d = desc.to_ascii_lowercase();
    if d.contains("auto-memory") || d.contains("auto memory") {
        (Kind::Memory, Layer::User)
    } else if d.contains("not checked in") {
        (Kind::Instruction, Layer::Local)
    } else if d.contains("global instructions") {
        (Kind::Instruction, Layer::User)
    } else if d.contains("checked into the codebase") {
        (Kind::Instruction, Layer::Project)
    } else if d.contains("managed") || d.contains("enterprise") || d.contains("organization") {
        (Kind::Instruction, Layer::Managed)
    } else {
        (Kind::Instruction, Layer::Unresolved)
    }
}

/// Realpath when the file exists on this machine, else the path as written.
fn realpath(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// Known section headers of the `system`-role message's text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SysSection {
    /// `<Event> hook additional context:`.
    Hook(String),
    Environment,
    Roster,
    McpInstructions,
    Skills,
    Date,
    Advisor,
}

fn sys_header(line: &str) -> Option<SysSection> {
    if let Some(i) = line.find(" hook additional context:") {
        let event = &line[..i];
        if !event.is_empty() && event.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(SysSection::Hook(event.to_string()));
        }
    }
    match line {
        "# Environment" => Some(SysSection::Environment),
        "Available agent types for the Agent tool:" => Some(SysSection::Roster),
        "# MCP Server Instructions" => Some(SysSection::McpInstructions),
        "The following skills are available for use with the Skill tool:" => {
            Some(SysSection::Skills)
        }
        "# Advisor Tool" => Some(SysSection::Advisor),
        _ if line.starts_with("Today's date is ") => Some(SysSection::Date),
        _ => None,
    }
}

/// `# <identifier>` header of a context reminder (`# userEmail`, `# gitStatus`, ...).
fn context_header(line: &str) -> Option<String> {
    let name = line.strip_prefix("# ")?;
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .then(|| name.to_string())
}

/// Facts the capture readers need to bind a body to a `RunContext`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BodyFacts {
    /// ` - Primary working directory:` of the `# Environment` section.
    pub cwd: Option<PathBuf>,
    /// Directories holding the project/local-layer `CLAUDE.md` files (weak cwd evidence: the cwd
    /// is at or below the deepest one).
    pub project_dirs: Vec<PathBuf>,
    /// `cc_version=` of the billing header, as written (`2.1.296.406`).
    pub cc_version: Option<String>,
    pub model: Option<String>,
    /// No assistant message yet: a session's first request (what harn models).
    pub first_request: bool,
    pub messages: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedBody {
    pub regions: Vec<Region>,
    pub tree: Tree,
    pub facts: BodyFacts,
}

/// tools / system / messages[0] blocks / messages[1] sections, named like 01-spec §Problem. The
/// regions partition the body (they sum to `bytes.len()`); the tree holds the observed items.
pub fn parse_request_body(bytes: &[u8]) -> anyhow::Result<(Vec<Region>, Tree)> {
    parse_body(bytes, None).map(|parsed| (parsed.regions, parsed.tree))
}

/// [`parse_request_body`] with the session's `init` event, which names MCP servers exactly,
/// gives their source layer, and adds the enabled plugins.
pub(crate) fn parse_body(bytes: &[u8], init: Option<&InitEvent>) -> anyhow::Result<ParsedBody> {
    let body: Value = serde_json::from_slice(bytes).map_err(|err| {
        let tail = &bytes[bytes.len().saturating_sub(64)..];
        if String::from_utf8_lossy(tail).contains("[truncated ") {
            anyhow!("the request body was truncated by the capture (`…[truncated N bytes]` marker); raise the capture's body limit")
        } else {
            anyhow!(err).context("the request body is not JSON")
        }
    })?;
    let obj = body
        .as_object()
        .ok_or_else(|| anyhow!("the request body is not a JSON object"))?;
    let empty = Vec::new();
    let messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let tools = obj.get("tools").and_then(Value::as_array).unwrap_or(&empty);

    let mut b = Builder {
        init,
        mcp_names: mcp_names(tools, messages, init),
        ..Builder::default()
    };
    b.facts.model = obj.get("model").and_then(Value::as_str).map(str::to_string);
    b.facts.messages = messages.len();
    b.facts.first_request = !messages.iter().any(|m| m["role"] == "assistant");

    b.tools(tools);
    if let Some(system) = obj.get("system") {
        b.system(system);
    }
    b.messages(messages);
    b.init_items();

    let params: serde_json::Map<String, Value> = obj
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "messages" | "system" | "tools"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !params.is_empty() {
        b.region(
            "request parameters",
            None,
            compact_len(&Value::Object(params)),
        );
    }
    let attributed: u64 = b.regions.iter().map(|r| r.bytes).sum();
    let total = bytes.len() as u64;
    let framing = total.checked_sub(attributed).ok_or_else(|| {
        anyhow!("region accounting overshoots the body ({attributed} > {total} bytes)")
    })?;
    b.region("framing", None, framing);

    let tree = Tree {
        agent: Agent::ClaudeCode,
        target_dir: b.facts.cwd.clone().unwrap_or_default(),
        agent_version: b.facts.cc_version.as_deref().map(version_triple),
        rules_verified_against: "observed".to_string(),
        tool_search: b.tool_search,
        signals: Vec::new(),
        items: b.items,
        predicted_known: 0,
        unknown_rows: 0,
        observed_body: Some(total),
        warnings: b.warnings,
        home: None,
        observed_regions: Vec::new(),
        tool_search_cause: None,
    };
    Ok(ParsedBody {
        regions: b.regions,
        tree,
        facts: b.facts,
    })
}

/// Tool prefix (`plugin_slack_slack`) → server name (`plugin:slack:slack`). Candidates are the
/// init event's servers and the `## <name>` headers of the system text; a candidate counts when
/// its normalised form is a prefix that occurs in `tools` (or it is an init server).
fn mcp_names(
    tools: &[Value],
    messages: &[Value],
    init: Option<&InitEvent>,
) -> BTreeMap<String, String> {
    let prefixes: Vec<&str> = tools
        .iter()
        .filter_map(|t| split_mcp_tool(t["name"].as_str()?).map(|(p, _)| p))
        .collect();
    let mut names = BTreeMap::new();
    for server in init.map(|i| i.mcp_servers.as_slice()).unwrap_or(&[]) {
        names.insert(normalize_mcp_name(&server.name), server.name.clone());
    }
    let system_texts = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter_map(|block| block["text"].as_str());
    for text in system_texts {
        for name in text.lines().filter_map(|l| l.strip_prefix("## ")) {
            let key = normalize_mcp_name(name);
            if prefixes.contains(&key.as_str()) {
                names.entry(key).or_insert_with(|| name.to_string());
            }
        }
    }
    names
}

/// `mcp__<server>__<tool>` → (`<server>`, `<tool>`).
fn split_mcp_tool(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("mcp__")?;
    let i = rest.find("__")?;
    Some((&rest[..i], &rest[i + 2..]))
}

/// The init event's provenance value → layer (`01-spec` §Claude Code coverage).
fn source_layer(source: &str) -> Layer {
    match source {
        "plugin" => Layer::Plugin,
        "user" => Layer::User,
        "project" => Layer::Project,
        "local" => Layer::Local,
        "claudeai" => Layer::ClaudeAi,
        "dynamic" | "sdk" => Layer::Dynamic,
        "managed" | "enterprise" => Layer::Managed,
        _ => Layer::Unresolved,
    }
}

fn observed_item(
    kind: Kind,
    layer: Layer,
    name: &str,
    label: &str,
    mode: Mode,
    observed: Option<u64>,
    region: &str,
) -> Item {
    Item {
        id: item_id(Agent::ClaudeCode, kind, layer, name),
        label: label.to_string(),
        kind,
        origin: Origin {
            layer,
            path: None,
            setting: None,
            plugin: None,
            why: vec![format!("observed: {region}")],
        },
        mode,
        cost: Cost {
            observed,
            ..Cost::default()
        },
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

fn add_child(parent: &mut Item, child: Item) {
    if let Some(bytes) = child.cost.observed {
        parent.cost.observed = Some(parent.cost.observed.unwrap_or(0) + bytes);
    }
    parent.children.push(child);
}

/// One MCP tool of a server in `tools`: (tool name, compact JSON bytes, `defer_loading`).
type McpToolBytes = (String, u64, bool);

#[derive(Default)]
struct Builder<'a> {
    init: Option<&'a InitEvent>,
    mcp_names: BTreeMap<String, String>,
    regions: Vec<Region>,
    items: Vec<Item>,
    warnings: Vec<String>,
    facts: BodyFacts,
    tool_search: Option<bool>,
}

impl Builder<'_> {
    fn region(&mut self, group: &str, part: Option<&str>, bytes: u64) {
        if bytes == 0 && group != "framing" {
            return;
        }
        let name = match part {
            Some(part) => format!("{group}{}{part}", Region::SEP),
            None => group.to_string(),
        };
        self.regions.push(Region { name, bytes });
    }

    /// Plugin id for `origin.plugin`: `name@marketplace` when the init event knows it.
    fn plugin_ref(&self, plugin: &str) -> String {
        self.init
            .and_then(|i| i.plugins.iter().find(|p| p.name == plugin))
            .and_then(|p| p.source.clone())
            .unwrap_or_else(|| plugin.to_string())
    }

    fn server_name(&self, prefix: &str) -> String {
        if let Some(name) = self.mcp_names.get(prefix) {
            return name.clone();
        }
        // plugin:<p>:<s> normalises to plugin_<p>_<s>; undo it when <p> == <s>.
        if let Some(rest) = prefix.strip_prefix("plugin_") {
            let mid = rest.len() / 2;
            if rest.len() % 2 == 1 && rest.as_bytes()[mid] == b'_' && rest[..mid] == rest[mid + 1..]
            {
                return format!("plugin:{0}:{0}", &rest[..mid]);
            }
        }
        prefix.to_string()
    }

    fn server_layer(&self, server: &str) -> Layer {
        let from_init = self
            .init
            .and_then(|i| i.mcp_servers.iter().find(|s| s.name == server))
            .and_then(|s| s.source.as_deref())
            .map(source_layer);
        match from_init {
            Some(layer) if layer != Layer::Unresolved => layer,
            _ if server.starts_with("plugin:") || server.starts_with("plugin_") => Layer::Plugin,
            _ => Layer::Unresolved,
        }
    }

    /// Index of the McpServer item for `server`, created (cost `None`) when missing.
    fn server_index(&mut self, server: &str, region: &str) -> usize {
        let layer = self.server_layer(server);
        let id = item_id(Agent::ClaudeCode, Kind::McpServer, layer, server);
        if let Some(i) = self.items.iter().position(|item| item.id == id) {
            return i;
        }
        let mut item = observed_item(
            Kind::McpServer,
            layer,
            server,
            server,
            Mode::Always,
            None,
            region,
        );
        if let Some(plugin) = server
            .strip_prefix("plugin:")
            .and_then(|r| r.split(':').next())
        {
            item.origin.plugin = Some(self.plugin_ref(plugin));
        }
        self.items.push(item);
        self.items.len() - 1
    }

    fn tools(&mut self, tools: &[Value]) {
        let group = "tools";
        let mut builtin = (0u64, 0usize);
        let mut servers: Vec<(String, Vec<McpToolBytes>)> = Vec::new();
        for tool in tools {
            let bytes = compact_len(tool);
            let deferred = tool["defer_loading"] == Value::Bool(true);
            let name = tool["name"]
                .as_str()
                .or_else(|| tool["type"].as_str())
                .unwrap_or("unnamed");
            match split_mcp_tool(name) {
                Some((prefix, tool_name)) => {
                    let entry = (tool_name.to_string(), bytes, deferred);
                    match servers.iter_mut().find(|(p, _)| p == prefix) {
                        Some((_, list)) => list.push(entry),
                        None => servers.push((prefix.to_string(), vec![entry])),
                    }
                }
                None => {
                    builtin = (builtin.0 + bytes, builtin.1 + 1);
                    let mode = if deferred {
                        Mode::Deferred
                    } else {
                        Mode::Always
                    };
                    self.items.push(observed_item(
                        Kind::BuiltinTool,
                        Layer::Builtin,
                        name,
                        name,
                        mode,
                        Some(bytes),
                        "tools · builtin",
                    ));
                }
            }
        }
        if builtin.1 > 0 {
            self.region(
                group,
                Some(&format!("builtin ({} tools)", builtin.1)),
                builtin.0,
            );
        }
        let any_mcp = !servers.is_empty();
        let any_deferred = servers.iter().flat_map(|(_, t)| t).any(|t| t.2);
        self.tool_search = match (any_mcp, any_deferred) {
            (_, true) => Some(true),
            (true, false) => Some(false),
            (false, false) => None,
        };
        for (prefix, list) in servers {
            let server = self.server_name(&prefix);
            let part = format!("mcp {server} ({} tools)", list.len());
            let region = format!("{group}{}{part}", Region::SEP);
            self.region(group, Some(&part), list.iter().map(|t| t.1).sum());
            let index = self.server_index(&server, &region);
            let layer = self.items[index].origin.layer;
            let all_deferred = list.iter().all(|t| t.2);
            for (tool, bytes, deferred) in list {
                let mode = if deferred {
                    Mode::Deferred
                } else {
                    Mode::Always
                };
                let child = observed_item(
                    Kind::McpTool,
                    layer,
                    &format!("{server}/{tool}"),
                    &tool,
                    mode,
                    Some(bytes),
                    &region,
                );
                add_child(&mut self.items[index], child);
            }
            if all_deferred {
                self.items[index].mode = Mode::Deferred;
            }
        }
    }

    fn system(&mut self, system: &Value) {
        let group = "system";
        let blocks: Vec<(Option<&str>, u64)> = match system {
            Value::String(text) => vec![(Some(text.as_str()), text.len() as u64)],
            Value::Array(blocks) => blocks
                .iter()
                .map(|block| match block["text"].as_str() {
                    Some(text) => (Some(text), text.len() as u64),
                    None => (None, compact_len(block)),
                })
                .collect(),
            _ => return,
        };
        let mut parent = observed_item(
            Kind::SystemPrompt,
            Layer::Builtin,
            names::SYSTEM_PROMPT,
            "system prompt",
            Mode::Always,
            None,
            group,
        );
        let mut seen_identity = false;
        let mut seen_rules = false;
        for (i, (text, bytes)) in blocks.into_iter().enumerate() {
            let text = text.unwrap_or("");
            let part = if text.starts_with("x-anthropic-billing-header:") {
                if let Some(v) = text.split("cc_version=").nth(1) {
                    let v = v.split([';', ' ', '\n']).next().unwrap_or("");
                    self.facts.cc_version = Some(v.to_string()).filter(|v| !v.is_empty());
                }
                "billing header".to_string()
            } else if !seen_identity && text.starts_with("You are") && text.len() < 1024 {
                seen_identity = true;
                "identity".to_string()
            } else if !seen_rules {
                seen_rules = true;
                "harness rules".to_string()
            } else {
                format!("block {i}")
            };
            let region = format!("{group}{}{part}", Region::SEP);
            self.region(group, Some(&part), bytes);
            let name = format!("{}/{}", names::SYSTEM_PROMPT, part.replace(' ', "-"));
            add_child(
                &mut parent,
                observed_item(
                    Kind::SystemPrompt,
                    Layer::Builtin,
                    &name,
                    &part,
                    Mode::Always,
                    Some(bytes),
                    &region,
                ),
            );
        }
        self.items.push(parent);
    }

    fn messages(&mut self, messages: &[Value]) {
        for (i, message) in messages.iter().enumerate() {
            let role = message["role"].as_str().unwrap_or("");
            let blocks = match &message["content"] {
                Value::String(text) => {
                    let part = if i == 0 { "instruction" } else { role };
                    self.region(&format!("messages[{i}]"), Some(part), text.len() as u64);
                    continue;
                }
                Value::Array(blocks) => blocks,
                _ => continue,
            };
            let mut system_texts = 0;
            for (j, block) in blocks.iter().enumerate() {
                let text = block["text"].as_str().filter(|_| block["type"] == "text");
                let kind = block["type"].as_str().unwrap_or("block");
                match (role, text) {
                    ("system", Some(text)) => {
                        system_texts += 1;
                        let group = match system_texts {
                            1 => format!("messages[{i}] system text"),
                            n => format!("messages[{i}] system text {n}"),
                        };
                        self.system_text(&group, text);
                    }
                    ("system", None) => {
                        let group = format!("messages[{i}] {kind}");
                        let bytes = compact_len(block);
                        self.region(&group, None, bytes);
                        if kind == "tool_addition" {
                            let tool = block["tool"]["name"].as_str().unwrap_or("tool");
                            self.attach_to_tool(tool, "tool_addition", bytes, &group);
                        }
                    }
                    ("user", Some(text)) if i == 0 => {
                        self.user_text(i, j, text, j + 1 == blocks.len());
                    }
                    (_, Some(text)) => {
                        self.region(&format!("messages[{i}] block {j}"), None, text.len() as u64)
                    }
                    (_, None) => self.region(
                        &format!("messages[{i}] block {j}"),
                        Some(kind),
                        compact_len(block),
                    ),
                }
            }
        }
    }

    /// A text block of the first user message: the instruction files, the context reminder
    /// (`# userEmail`, `# gitStatus`), or the user's instruction.
    fn user_text(&mut self, i: usize, j: usize, text: &str, is_last: bool) {
        let group = format!("messages[{i}] block {j}");
        let limit = closing_tag_start(text);
        let wrapper = |lead: usize| (lead + text.len() - limit) as u64;

        let reminder = text.starts_with("<system-reminder>");
        let (lead, sections) = split_sections(text, limit, contents_header);
        if reminder && !sections.is_empty() {
            self.region(&group, Some("reminder wrapper"), wrapper(lead));
            for s in sections {
                let (path, desc) = s.tag;
                let (kind, layer) = contents_kind(&desc);
                let part = match kind {
                    Kind::Memory => format!("auto-memory {path}"),
                    _ => path.clone(),
                };
                let bytes = (s.end - s.start) as u64;
                let region = format!("{group}{}{part}", Region::SEP);
                self.region(&group, Some(&part), bytes);
                let real = realpath(&path);
                if kind == Kind::Instruction && matches!(layer, Layer::Project | Layer::Local) {
                    if let Some(dir) = claude_md_dir(&real) {
                        self.facts.project_dirs.push(dir);
                    }
                }
                let name = real.to_string_lossy();
                let mode = Mode::Always;
                let mut item = observed_item(kind, layer, &name, &path, mode, Some(bytes), &region);
                item.origin.path = Some(real.clone());
                self.items.push(item);
            }
            return;
        }
        if reminder {
            let (lead, sections) = split_sections(text, limit, context_header);
            if sections.is_empty() {
                self.region(&group, Some("system reminder"), text.len() as u64);
                return;
            }
            self.region(&group, Some("reminder wrapper"), wrapper(lead));
            for s in sections {
                let bytes = (s.end - s.start) as u64;
                let region = format!("{group}{}{}", Region::SEP, s.tag);
                self.region(&group, Some(&s.tag), bytes);
                let kind = match s.tag.as_str() {
                    names::GIT_STATUS => Kind::GitBlock,
                    names::USER_EMAIL => Kind::Attribution,
                    _ => Kind::EnvBlock,
                };
                self.items.push(observed_item(
                    kind,
                    Layer::Builtin,
                    &s.tag,
                    &s.tag,
                    Mode::Always,
                    Some(bytes),
                    &region,
                ));
            }
            return;
        }
        let part = if is_last { "instruction" } else { "text" };
        self.region(&group, Some(part), text.len() as u64);
    }

    /// The `system`-role text: hook stdout, Environment, agent roster, MCP instructions, skill
    /// listing, date, Advisor.
    fn system_text(&mut self, group: &str, text: &str) {
        let (lead, sections) = split_sections(text, text.len(), sys_header);
        self.region(group, Some("other"), lead as u64);
        let mut hooks: BTreeMap<String, usize> = BTreeMap::new();
        for s in sections {
            let body = &text[s.start..s.end];
            let bytes = body.len() as u64;
            let (part, item) = match s.tag {
                SysSection::Hook(event) => {
                    let n = hooks.entry(event.clone()).or_insert(0);
                    *n += 1;
                    let (part, name) = match *n {
                        1 => (format!("{event} hook stdout"), event.clone()),
                        n => (format!("{event} hook stdout #{n}"), format!("{event}#{n}")),
                    };
                    let label = format!("{event} hook stdout");
                    (part, Some((Kind::Hook, Layer::Unresolved, name, label)))
                }
                SysSection::Environment => {
                    self.facts.cwd = body.lines().find_map(|line| {
                        let p = line
                            .trim_start()
                            .strip_prefix("- Primary working directory:")?;
                        Some(PathBuf::from(p.trim()))
                    });
                    let name = names::ENVIRONMENT.to_string();
                    let item = (Kind::EnvBlock, Layer::Builtin, name, "Environment".into());
                    ("Environment".to_string(), Some(item))
                }
                SysSection::Date => {
                    let item = (
                        Kind::EnvBlock,
                        Layer::Builtin,
                        names::DATE.into(),
                        "date".into(),
                    );
                    ("date".to_string(), Some(item))
                }
                SysSection::Roster => {
                    self.listing(group, body, Kind::AgentRoster);
                    continue;
                }
                SysSection::Skills => {
                    self.listing(group, body, Kind::SkillListing);
                    continue;
                }
                SysSection::McpInstructions => {
                    self.mcp_instructions(group, body);
                    continue;
                }
                SysSection::Advisor => {
                    let region = format!("{group}{}Advisor", Region::SEP);
                    self.region(group, Some("Advisor"), bytes);
                    self.attach_to_tool("advisor", "instructions", bytes, &region);
                    continue;
                }
            };
            let region = format!("{group}{}{part}", Region::SEP);
            self.region(group, Some(&part), bytes);
            if let Some((kind, layer, name, label)) = item {
                self.items.push(observed_item(
                    kind,
                    layer,
                    &name,
                    &label,
                    Mode::Always,
                    Some(bytes),
                    &region,
                ));
            }
        }
    }

    /// The agent roster or the skill listing: the container plus one child per entry.
    fn listing(&mut self, group: &str, body: &str, kind: Kind) {
        let entries = listing_entries(body);
        let described = entries.iter().filter(|e| e.described).count();
        let (part, name, label, child_kind) = match kind {
            Kind::AgentRoster => (
                format!("agent roster ({})", entries.len()),
                names::AGENT_ROSTER,
                "agent roster",
                Kind::SubAgent,
            ),
            _ => (
                format!(
                    "skill list ({} entries, {described} with descriptions)",
                    entries.len()
                ),
                names::SKILL_LISTING,
                "skill listing",
                Kind::Skill,
            ),
        };
        let region = format!("{group}{}{part}", Region::SEP);
        self.region(group, Some(&part), body.len() as u64);
        let mut parent = observed_item(
            kind,
            Layer::Builtin,
            name,
            label,
            Mode::Always,
            Some(body.len() as u64),
            &region,
        );
        for entry in entries {
            let layer = if entry.name.contains(':') {
                Layer::Plugin
            } else {
                Layer::Unresolved
            };
            let mut child = observed_item(
                child_kind,
                layer,
                &entry.name,
                &entry.name,
                Mode::Advertised,
                Some((entry.end - entry.start) as u64),
                &region,
            );
            if layer == Layer::Plugin {
                let plugin = entry.name.split(':').next().unwrap_or("");
                child.origin.plugin = Some(self.plugin_ref(plugin));
            }
            if !entry.described {
                child.signal = Some(NAME_ONLY.to_string());
            }
            parent.children.push(child);
        }
        self.items.push(parent);
    }

    /// `# MCP Server Instructions`: one `## <server>` part per server, attached to the server.
    fn mcp_instructions(&mut self, group: &str, body: &str) {
        let part = "MCP instructions";
        let region = format!("{group}{}{part}", Region::SEP);
        self.region(group, Some(part), body.len() as u64);
        let known: Vec<String> = self.mcp_names.values().cloned().collect();
        let (_, servers) = split_sections(body, body.len(), |line| {
            let name = line.strip_prefix("## ")?;
            known.iter().any(|k| k == name).then(|| name.to_string())
        });
        for s in servers {
            let index = self.server_index(&s.tag, &region);
            let layer = self.items[index].origin.layer;
            let child = observed_item(
                Kind::McpInstructions,
                layer,
                &s.tag,
                &format!("{} instructions", s.tag),
                Mode::Always,
                Some((s.end - s.start) as u64),
                &region,
            );
            add_child(&mut self.items[index], child);
        }
    }

    /// A built-in tool's extra request text (`# Advisor Tool`, `tool_addition`) as a child of
    /// that tool, so turning the tool off frees it too; a top-level system-prompt item when the
    /// tool is not in `tools`.
    fn attach_to_tool(&mut self, tool: &str, part: &str, bytes: u64, region: &str) {
        let name = format!("{tool}/{part}");
        let child = observed_item(
            Kind::SystemPrompt,
            Layer::Builtin,
            &name,
            &name,
            Mode::Always,
            Some(bytes),
            region,
        );
        let id = item_id(Agent::ClaudeCode, Kind::BuiltinTool, Layer::Builtin, tool);
        match self.items.iter_mut().find(|item| item.id == id) {
            Some(parent) => add_child(parent, child),
            None => self.items.push(child),
        }
    }

    /// Items only the init event knows: enabled plugins, and connected MCP servers whose tools
    /// are not in the body (deferred). Servers that are not connected stay out of the tree;
    /// `verify` reads their status from `Capture.init`.
    fn init_items(&mut self) {
        let Some(init) = self.init else { return };
        for server in init.mcp_servers.iter().filter(|s| s.status == "connected") {
            self.server_index(&server.name, "init · mcp_servers");
        }
        for plugin in &init.plugins {
            let name = plugin.source.clone().unwrap_or_else(|| plugin.name.clone());
            let mut item = observed_item(
                Kind::Plugin,
                Layer::Unresolved,
                &name,
                &name,
                Mode::Always,
                None,
                "init · plugins",
            );
            item.origin.path = plugin.path.clone();
            item.origin.plugin = Some(name.clone());
            self.items.push(item);
        }
    }
}

/// Directory a `CLAUDE.md` / `CLAUDE.local.md` / `.claude/CLAUDE.md` belongs to.
fn claude_md_dir(path: &Path) -> Option<PathBuf> {
    let file = path.file_name()?.to_str()?;
    if file != "CLAUDE.md" && file != "CLAUDE.local.md" {
        return None;
    }
    let dir = path.parent()?;
    match dir.file_name().and_then(|n| n.to_str()) {
        Some(".claude") => dir.parent().map(Path::to_path_buf),
        _ => Some(dir.to_path_buf()),
    }
}

/// `OTEL_LOG_RAW_API_BODIES=file:<tmp>` + [`LAUNCH_ARGS`], run in `target.dir` with
/// `target.env` (and `HOME = target.home`). The scratch directory is private and deleted
/// afterwards: the body holds the account email and every instruction file. The `--max-turns 1`
/// run usually exits non-zero (`error_max_turns`); success is a captured first request whose
/// working directory is `target.dir`.
pub fn run_claude(target: &Target, claude_bin: &Path) -> anyhow::Result<Capture> {
    let work = scratch_dir()?;
    let result = run_in(&work, target, claude_bin);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn scratch_dir() -> anyhow::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("harn-capture-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("otel-raw"))
        .with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

fn run_in(work: &Path, target: &Target, claude_bin: &Path) -> anyhow::Result<Capture> {
    use std::process::{Command, Stdio};
    let otel = work.join("otel-raw");
    let fingerprint = settings_fingerprint(target);
    let stdout = std::fs::File::create(work.join("stream.jsonl"))?;
    let stderr = std::fs::File::create(work.join("stderr.txt"))?;
    let mut child = Command::new(claude_bin)
        .args(LAUNCH_ARGS)
        .current_dir(&target.dir)
        .env_clear()
        .envs(&target.env)
        .env("HOME", &target.home)
        .env("CLAUDE_CODE_ENABLE_TELEMETRY", "1")
        .env("OTEL_LOGS_EXPORTER", "none")
        .env("OTEL_METRICS_EXPORTER", "none")
        .env(
            "OTEL_LOG_RAW_API_BODIES",
            format!("file:{}", otel.display()),
        )
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("starting {}", claude_bin.display()))?;
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{} did not finish within {} s; killed",
                claude_bin.display(),
                RUN_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let mut capture = super::capture::read_otel_dir(&otel, target).with_context(|| {
        let stderr = std::fs::read_to_string(work.join("stderr.txt")).unwrap_or_default();
        let tail: String = stderr.lines().rev().take(5).collect::<Vec<_>>().join(" | ");
        format!(
            "{} exited with {status} without a usable capture; stderr tail: {tail}",
            claude_bin.display()
        )
    })?;
    capture.context.launch_opts = LAUNCH_ARGS.iter().map(|a| a.to_string()).collect();
    capture.context.settings_fingerprint = Some(fingerprint);
    capture.context.capture_kind = "otel-run".to_string();
    capture.context.captured_at = now_iso8601();
    Ok(capture)
}

/// `sha256:<hex>` over the settings files the resolver reads, in a fixed order, each hashed as
/// `path NUL contents NUL` (`<absent>` for a missing file): managed settings, the user's
/// `settings.json` / `settings.local.json` (under `CLAUDE_CONFIG_DIR` when set), the project's
/// `.claude/settings.json` / `.claude/settings.local.json` and `.mcp.json`. `~/.claude.json` is
/// left out on purpose: Claude Code rewrites it at every start (counters, caches), so including
/// it would make every fingerprint unique.
pub fn settings_fingerprint(target: &Target) -> String {
    use sha2::{Digest, Sha256};
    let managed = if cfg!(target_os = "macos") {
        "/Library/Application Support/ClaudeCode/managed-settings.json"
    } else {
        "/etc/claude-code/managed-settings.json"
    };
    let user = target
        .env
        .get("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| target.home.join(".claude"));
    let files = [
        PathBuf::from(managed),
        user.join("settings.json"),
        user.join("settings.local.json"),
        target.dir.join(".claude/settings.json"),
        target.dir.join(".claude/settings.local.json"),
        target.dir.join(".mcp.json"),
    ];
    let mut hasher = Sha256::new();
    for file in &files {
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update([0]);
        match std::fs::read(file) {
            Ok(bytes) => hasher.update(&bytes),
            Err(_) => hasher.update(b"<absent>"),
        }
        hasher.update([0]);
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// `RunContext` for a parsed body: cwd and version as derived by the caller.
pub(crate) fn context(
    cwd: PathBuf,
    version: Option<String>,
    model: Option<String>,
    kind: &str,
    captured_at: String,
) -> RunContext {
    RunContext {
        cwd,
        agent: Agent::ClaudeCode,
        agent_version: version.as_deref().map(version_triple),
        model,
        launch_opts: Vec::new(),
        settings_fingerprint: None,
        capture_kind: kind.to_string(),
        captured_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_start_at_the_newline_before_the_header() {
        let text = "lead\n\n# A\nx\n\n# B\ny";
        let (lead, sections) = split_sections(text, text.len(), |l| {
            l.strip_prefix("# ").map(str::to_string)
        });
        assert_eq!(&text[..lead], "lead\n");
        assert_eq!(&text[sections[0].start..sections[0].end], "\n# A\nx\n");
        assert_eq!(&text[sections[1].start..sections[1].end], "\n# B\ny");
        let first = split_sections("# A\nx", 5, |l| l.strip_prefix("# ").map(str::to_string));
        assert_eq!(first.0, 0);
        assert_eq!((first.1[0].start, first.1[0].end), (0, 5));
    }

    #[test]
    fn listing_entries_cover_multi_line_entries_and_leave_the_trailing_note() {
        let section =
            "\nHeader:\n- a: one\n- When a bullet: inside\n  more\n\n- b\n- c (alias): three\n\nA note.\n";
        let entries = listing_entries(section);
        let got: Vec<(&str, bool, &str)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.described, &section[e.start..e.end]))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a", true, "- a: one\n- When a bullet: inside\n  more\n\n"),
                ("b", false, "- b\n"),
                ("c", true, "- c (alias): three\n"),
            ]
        );
    }

    #[test]
    fn entry_names_drop_the_alias_and_keep_plugin_namespaces() {
        assert_eq!(entry_name("- p:x (p:y)"), ("p:x".to_string(), false));
        assert_eq!(entry_name("- p:x: does x"), ("p:x".to_string(), true));
        assert_eq!(entry_name("- plain"), ("plain".to_string(), false));
    }

    #[test]
    fn contents_headers_name_the_layer() {
        let (path, desc) = contents_header(
            "Contents of /a (b)/CLAUDE.md (project instructions, checked into the codebase):",
        )
        .unwrap();
        assert_eq!(path, "/a (b)/CLAUDE.md");
        assert_eq!(contents_kind(&desc), (Kind::Instruction, Layer::Project));
        assert_eq!(
            contents_kind("user's private global instructions for all projects"),
            (Kind::Instruction, Layer::User)
        );
        assert_eq!(
            contents_kind("user's private project instructions, not checked in"),
            (Kind::Instruction, Layer::Local)
        );
        assert_eq!(
            contents_kind("user's auto-memory, persists across conversations"),
            (Kind::Memory, Layer::User)
        );
        assert_eq!(
            contents_kind("something new"),
            (Kind::Instruction, Layer::Unresolved)
        );
    }

    #[test]
    fn mcp_server_names_round_trip_through_the_tool_prefix() {
        assert_eq!(
            normalize_mcp_name("plugin:slack:slack"),
            "plugin_slack_slack"
        );
        assert_eq!(normalize_mcp_name("claude.ai Gmail"), "claude_ai_Gmail");
        let b = Builder::default();
        assert_eq!(b.server_name("plugin_slack_slack"), "plugin:slack:slack");
        assert_eq!(b.server_name("plugin_a_b"), "plugin_a_b");
        assert_eq!(b.server_name("blender"), "blender");
        assert_eq!(
            split_mcp_tool("mcp__plugin_x_x__do_it"),
            Some(("plugin_x_x", "do_it"))
        );
        assert_eq!(split_mcp_tool("Read"), None);
    }

    #[test]
    fn version_triple_drops_the_build_number() {
        assert_eq!(version_triple("2.1.296.406"), "2.1.296");
        assert_eq!(version_triple("2.1.296 (Claude Code)"), "2.1.296");
    }

    #[test]
    fn init_event_is_found_after_hook_events() {
        let stream = concat!(
            r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart:startup"}"#,
            "\n",
            r#"{"type":"system","subtype":"init","cwd":"/r","session_id":"s","tools":["Read"],"#,
            r#""mcp_servers":[{"name":"m","status":"failed","source":"user"}],"skills":["a"],"#,
            r#""agents":["g"],"plugins":[{"name":"p","source":"p@m","version":"1"}],"#,
            r#""memory_paths":{"auto":"/m/"},"claude_code_version":"2.1.296","model":"x"}"#,
            "\n"
        );
        let init = find_init_event(stream).unwrap();
        assert_eq!(init.cwd, Some(PathBuf::from("/r")));
        assert_eq!(init.mcp_servers[0].status, "failed");
        assert_eq!(init.plugins[0].source.as_deref(), Some("p@m"));
        assert_eq!(init.plugins[0].path, None);
        assert_eq!(init.memory_paths["auto"], "/m/");
        assert!(parse_init_event(r#"{"type":"result"}"#).is_err());
        assert!(find_init_event("{}\n").is_err());
    }
}
