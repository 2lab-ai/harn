//! MCP servers (`research/01` §7). Sources, by precedence for the same name: local
//! (`~/.claude.json` `projects["<dir>"].mcpServers`) → project (`<dir>/.mcp.json`, needs approval)
//! → user (`~/.claude.json` `mcpServers`); managed (`managed-mcp.json`) above all; plugin servers
//! (`<plugin>/.mcp.json` or inline `mcpServers` in `plugin.json`) are named `plugin:<p>:<s>` and
//! never collide. A server that is turned off (`disabledMcpServers`, `disabledMcpjsonServers`,
//! `deniedMcpServers`, not in `allowedMcpServers`, an unapproved `.mcp.json` entry) stays in the
//! tree with mode `none` and a `why` line naming the key. Tool schemas and server instructions are
//! unknown until the server has run: the server row and its one `mcp-instructions` child are `?`.
//! Mode: `deferred` when tool search is on, else `always`. `--mcp-config`, claude.ai connectors and
//! the built-in servers (`claude-in-chrome`, `computer-use`, `ide`) are not static facts.

use super::fs::{esc, Json};
use super::plugins::Plugin;
use super::settings::MANAGED_DIRS;
use super::{cost_na, disabled_mcp_knob, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

struct Server {
    name: String,
    layer: Layer,
    file: PathBuf,
    /// JSON pointer of the server's definition in `file`.
    pointer: String,
    def: Value,
    plugin: Option<usize>,
}

const SERVER_ONLY: &str =
    "server only; ~/.claude.json is rewritten by Claude Code — apply with no claude running";

fn servers_of(value: &Value) -> Option<(&Map<String, Value>, &'static str)> {
    match value.get("mcpServers") {
        Some(Value::Object(map)) => Some((map, "/mcpServers")),
        _ => None,
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn collect(ctx: &mut Ctx, plugins: &[Plugin], tool_search: Option<bool>) -> Vec<Item> {
    let mut servers: Vec<Server> = Vec::new();
    let claude_json = ctx.claude_json_path.clone();
    let claude_json_real = ctx.fs.realpath(&claude_json);

    for dir in MANAGED_DIRS {
        let file = Path::new(dir).join("managed-mcp.json");
        if let Json::Ok(v) = ctx.fs.read_json(&file) {
            if let Some((map, prefix)) = servers_of(&v) {
                for (name, def) in map {
                    servers.push(Server {
                        name: name.clone(),
                        layer: Layer::Managed,
                        file: ctx.fs.realpath(&file),
                        pointer: format!("{prefix}/{}", esc(name)),
                        def: def.clone(),
                        plugin: None,
                    });
                }
            }
        }
    }
    if let Some(Value::Object(map)) = ctx.project_json().and_then(|p| p.get("mcpServers")) {
        for (name, def) in map {
            servers.push(Server {
                name: name.clone(),
                layer: Layer::Local,
                file: claude_json_real.clone(),
                pointer: format!("{}/mcpServers/{}", ctx.project_pointer(), esc(name)),
                def: def.clone(),
                plugin: None,
            });
        }
    }
    let project_file = ctx.dir().join(".mcp.json");
    match ctx.fs.read_json(&project_file) {
        Json::Ok(v) => {
            if let Some((map, prefix)) = servers_of(&v) {
                for (name, def) in map {
                    servers.push(Server {
                        name: name.clone(),
                        layer: Layer::Project,
                        file: ctx.fs.realpath(&project_file),
                        pointer: format!("{prefix}/{}", esc(name)),
                        def: def.clone(),
                        plugin: None,
                    });
                }
            }
        }
        Json::Invalid(err) => ctx.warnings.push(format!(
            "{}: invalid JSON, ignored ({err})",
            project_file.display()
        )),
        Json::Missing => {}
    }
    if let Some(Value::Object(map)) = ctx.claude_json.as_ref().and_then(|c| c.get("mcpServers")) {
        for (name, def) in map {
            servers.push(Server {
                name: name.clone(),
                layer: Layer::User,
                file: claude_json_real.clone(),
                pointer: format!("/mcpServers/{}", esc(name)),
                def: def.clone(),
                plugin: None,
            });
        }
    }
    for (i, p) in plugins.iter().enumerate() {
        for (file, prefix, map) in p.mcp_sources(ctx) {
            for (name, def) in map {
                servers.push(Server {
                    name: format!("plugin:{}:{name}", p.name),
                    layer: p.layer,
                    file: file.clone(),
                    pointer: format!("{prefix}/{}", esc(&name)),
                    def,
                    plugin: Some(i),
                });
            }
        }
    }

    let disabled_local = string_list(ctx.project_json().and_then(|p| p.get("disabledMcpServers")));
    let mut out: Vec<Item> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for s in &servers {
        if seen.contains(&s.name) {
            continue;
        }
        seen.push(s.name.clone());
        let plugin = s.plugin.map(|i| &plugins[i]);
        let setting = format!("{}#{}", s.file.display(), s.pointer);
        let mut item = new_item(Kind::McpServer, s.layer, &s.name, &s.name);
        item.origin.path = Some(s.file.clone());
        item.origin.setting = Some(setting.clone());
        item.origin.plugin = plugin.map(|p| p.id.clone());
        let transport = s.def.get("type").and_then(Value::as_str).unwrap_or(
            if s.def.get("command").is_some() {
                "stdio"
            } else {
                "?"
            },
        );
        let line = format!("{setting} ({transport})");
        item.origin.why = match plugin {
            Some(p) => p.why_with(line),
            None => vec![line],
        };
        let shadowed: Vec<String> = servers
            .iter()
            .filter(|o| o.name == s.name && !std::ptr::eq(*o, s))
            .map(|o| format!("shadows the {} definition in {}", o.layer, o.file.display()))
            .collect();
        item.origin.why.extend(shadowed);

        item.knobs = match (s.layer, plugin) {
            (Layer::Managed, _) => vec![Knob::None {
                reason: "managed MCP server".to_string(),
            }],
            (_, Some(p)) => vec![
                p.contribution_knob(),
                disabled_mcp_knob(ctx, &s.name, SERVER_ONLY),
            ],
            (Layer::Project, None) => vec![
                setting_knob(
                    ctx,
                    Scope::Local,
                    "/disabledMcpjsonServers/-",
                    json!(s.name),
                    None,
                ),
                disabled_mcp_knob(ctx, &s.name, SERVER_ONLY),
            ],
            _ => vec![disabled_mcp_knob(ctx, &s.name, SERVER_ONLY)],
        };

        match off_reason(ctx, s, &disabled_local) {
            Some(reason) => {
                item.mode = Mode::None;
                item.cost = cost_na(None);
                item.origin.why.push(reason);
            }
            None => {
                item.mode = if tool_search == Some(true) {
                    Mode::Deferred
                } else {
                    Mode::Always
                };
                item.cost = Cost::default();
                item.origin.why.push(match tool_search {
                    Some(true) => "tool search on: schemas deferred; size unknown until a capture",
                    Some(false) => "tool search off: every schema sent up front; size unknown until the server has run",
                    None => "tool search state undecidable; schema size unknown until a capture",
                }.to_string());
                // Named by the bare server name, as the request-body parser names the observed
                // `## <server>` part (the kind keeps the id distinct from the server's).
                let mut instructions = new_item(
                    Kind::McpInstructions,
                    s.layer,
                    &s.name,
                    &format!("{} · instructions", s.name),
                );
                instructions.mode = Mode::Always;
                instructions.origin = item.origin.clone();
                instructions.origin.why.pop();
                instructions
                    .origin
                    .why
                    .push("server instructions: unknown until the server has run".to_string());
                instructions.knobs = item.knobs.clone();
                item.children.push(instructions);
            }
        }
        out.push(item);
    }

    if !(ctx
        .settings
        .get_bool("/disableClaudeAiConnectors")
        .is_some_and(|(b, _)| b)
        || ctx
            .env("ENABLE_CLAUDEAI_MCP_SERVERS")
            .is_some_and(|v| v == "false" || v == "0"))
    {
        ctx.warnings.push(
            "claude.ai connectors are fetched at session start and are not listed statically"
                .to_string(),
        );
    }
    out
}

/// Why a discovered server is off, naming the key; None when it loads.
fn off_reason(ctx: &Ctx, s: &Server, disabled_local: &[String]) -> Option<String> {
    let pointer = ctx.project_pointer();
    if disabled_local.contains(&s.name) {
        return Some(format!(
            "disabled by {}#{pointer}/disabledMcpServers",
            ctx.claude_json_path.display()
        ));
    }
    for entry in ctx.settings.list("/deniedMcpServers") {
        if entry.0.get("serverName").and_then(Value::as_str) == Some(s.name.as_str()) {
            return Some(format!(
                "denied by {}",
                entry.1.at(&format!("/deniedMcpServers/{}", entry.2))
            ));
        }
    }
    let allowed = ctx.settings.list("/allowedMcpServers");
    if !allowed.is_empty()
        && !allowed
            .iter()
            .any(|(v, _, _)| v.get("serverName").and_then(Value::as_str) == Some(s.name.as_str()))
    {
        return Some("not in allowedMcpServers".to_string());
    }
    if s.layer == Layer::Project {
        let in_settings = |key: &str| {
            ctx.settings
                .strings(&format!("/{key}"))
                .into_iter()
                .find(|(v, _)| *v == s.name)
                .map(|(_, origin)| origin)
        };
        let in_project = |key: &str| {
            string_list(ctx.project_json().and_then(|p| p.get(key)))
                .contains(&s.name)
                .then(|| format!("{}#{pointer}/{key}", ctx.claude_json_path.display()))
        };
        if let Some(origin) =
            in_settings("disabledMcpjsonServers").or_else(|| in_project("disabledMcpjsonServers"))
        {
            return Some(format!("disabled by {origin}"));
        }
        let approved = ctx
            .settings
            .get_bool("/enableAllProjectMcpServers")
            .is_some_and(|(b, _)| b)
            || in_settings("enabledMcpjsonServers").is_some()
            || in_project("enabledMcpjsonServers").is_some();
        if !approved {
            return Some(
                "not approved (no enabledMcpjsonServers / enableAllProjectMcpServers entry)"
                    .to_string(),
            );
        }
    }
    None
}
