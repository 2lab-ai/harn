//! Plugins (`research/01` §8): install records in `~/.claude/plugins/installed_plugins.json`
//! (v2: `{id: [{scope, projectPath?, installPath, version}]}`, v1: `{id: {...}}`), enable state from
//! `enabledPlugins` across the settings layers (a local `false` beats a project `true`; a missing key
//! falls back to the manifest's `defaultEnabled`), plus claude.ai-synced plugins
//! (`~/.claude/plugins/synced/<bucket>/<name>/`, id `<name>@synced`) and the built-in plugins the
//! 2.1.296 `init` event reports (`<name>@builtin`). Only enabled plugins are returned; their
//! skills, agents, hooks, commands and MCP servers are listed under their containers with
//! `origin.plugin` set, and [`items`] adds one `Plugin` row per plugin pointing at them.

use super::fs::{esc, file_name, Json};
use super::settings::Source;
use super::{cost_na, new_item, setting_knob, Ctx};
use crate::model::{Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Built-in plugins listed by the 2.1.296 `init` event (`path: "builtin"`). Static table.
const BUILTIN_PLUGINS: &[&str] = &[
    "cc-plugin-agents-md",
    "cc-plugin-telemetry",
    "cc-plugin-plugin-authoring",
];

pub(crate) struct Plugin {
    /// `name@marketplace`.
    pub id: String,
    pub name: String,
    pub layer: Layer,
    /// Realpath of the install dir; None for built-in plugins.
    pub root: Option<PathBuf>,
    pub manifest: Value,
    /// Admission chain, top-down.
    pub why: Vec<String>,
    /// `"<file>#/enabledPlugins/<id>"` when a settings key enabled it.
    pub setting: Option<String>,
    pub knob: Knob,
}

impl Plugin {
    /// The admission chain followed by the contribution's own file line.
    pub fn why_with(&self, line: String) -> Vec<String> {
        let mut why = self.why.clone();
        why.push(line);
        why
    }

    /// The plugin's off switch, annotated for a contribution row.
    pub fn contribution_knob(&self) -> Knob {
        match &self.knob {
            Knob::Setting {
                file,
                pointer,
                value,
                scope,
                ..
            } => Knob::Setting {
                file: file.clone(),
                pointer: pointer.clone(),
                value: value.clone(),
                scope: *scope,
                note: Some(format!("turns off the whole plugin {}", self.id)),
            },
            other => other.clone(),
        }
    }

    fn resolve_rel(&self, rel: &str) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        let rel = rel.replace("${CLAUDE_PLUGIN_ROOT}", &root.display().to_string());
        let path = Path::new(&rel);
        Some(if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(rel.trim_start_matches("./"))
        })
    }

    /// The default component dir plus any manifest override (`"skills": "./x"` or a list).
    pub fn component_paths(&self, ctx: &Ctx, key: &str, default: &str) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut push = |p: PathBuf| {
            let real = ctx.fs.realpath(&p);
            if (ctx.fs.is_dir(&p) || ctx.fs.is_file(&p))
                && !out.iter().any(|o| ctx.fs.realpath(o) == real)
            {
                out.push(p);
            }
        };
        if let Some(p) = self.resolve_rel(default) {
            push(p);
        }
        match self.manifest.get(key) {
            Some(Value::String(rel)) => {
                if let Some(p) = self.resolve_rel(rel) {
                    push(p);
                }
            }
            Some(Value::Array(items)) => {
                for rel in items.iter().filter_map(Value::as_str) {
                    if let Some(p) = self.resolve_rel(rel) {
                        push(p);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Hook sources: `(file, pointer prefix of the events map, events map)`.
    pub fn hook_sources(&self, ctx: &Ctx) -> Vec<(PathBuf, String, Map<String, Value>)> {
        let mut out = Vec::new();
        let Some(root) = self.root.as_ref() else {
            return out;
        };
        let mut files = vec![root.join("hooks/hooks.json")];
        match self.manifest.get("hooks") {
            Some(Value::String(rel)) => files.extend(self.resolve_rel(rel)),
            Some(Value::Object(inline)) => {
                let (pointer, events) = match inline.get("hooks") {
                    Some(Value::Object(events)) => ("/hooks/hooks", events.clone()),
                    _ => ("/hooks", inline.clone()),
                };
                out.push((self.manifest_path(ctx), pointer.to_string(), events));
            }
            _ => {}
        }
        let mut seen: Vec<PathBuf> = Vec::new();
        for file in files {
            let real = ctx.fs.realpath(&file);
            if seen.contains(&real) {
                continue;
            }
            seen.push(real.clone());
            if let Json::Ok(value) = ctx.fs.read_json(&file) {
                if let Some(Value::Object(events)) = value.get("hooks") {
                    out.push((real, "/hooks".to_string(), events.clone()));
                }
            }
        }
        out
    }

    /// MCP server sources: `(file, pointer prefix of the servers map, servers map)`.
    pub fn mcp_sources(&self, ctx: &Ctx) -> Vec<(PathBuf, String, Map<String, Value>)> {
        let mut out = Vec::new();
        let Some(root) = self.root.as_ref() else {
            return out;
        };
        let files: Vec<PathBuf> = match self.manifest.get("mcpServers") {
            Some(Value::Object(inline)) => {
                out.push((
                    self.manifest_path(ctx),
                    "/mcpServers".to_string(),
                    inline.clone(),
                ));
                Vec::new()
            }
            Some(Value::String(rel)) => self.resolve_rel(rel).into_iter().collect(),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|rel| self.resolve_rel(rel))
                .collect(),
            _ => vec![root.join(".mcp.json")],
        };
        for file in files {
            if let Json::Ok(value) = ctx.fs.read_json(&file) {
                let real = ctx.fs.realpath(&file);
                match value.get("mcpServers") {
                    Some(Value::Object(servers)) => {
                        out.push((real, "/mcpServers".to_string(), servers.clone()))
                    }
                    _ => {
                        if let Value::Object(flat) = value {
                            out.push((real, String::new(), flat));
                        }
                    }
                }
            }
        }
        out
    }

    fn manifest_path(&self, ctx: &Ctx) -> PathBuf {
        let root = self.root.clone().unwrap_or_default();
        let primary = root.join(".claude-plugin/plugin.json");
        if ctx.fs.is_file(&primary) {
            ctx.fs.realpath(&primary)
        } else {
            ctx.fs.realpath(&root.join("plugin.json"))
        }
    }
}

fn read_manifest(ctx: &Ctx, root: &Path) -> Value {
    for rel in [".claude-plugin/plugin.json", "plugin.json"] {
        if let Json::Ok(v) = ctx.fs.read_json(&root.join(rel)) {
            return v;
        }
    }
    Value::Object(Map::new())
}

/// Effective `enabledPlugins[id]`: (value, origin "<file>#<pointer>", source).
fn enabled_setting(ctx: &Ctx, id: &str) -> Option<(bool, String, Source)> {
    let pointer = format!("/enabledPlugins/{}", esc(id));
    ctx.settings
        .get_bool(&pointer)
        .map(|(b, f)| (b, f.at(&pointer), f.source))
}

fn off_knob(ctx: &Ctx, id: &str, source: Option<Source>) -> Knob {
    if source == Some(Source::Managed) {
        return Knob::None {
            reason: format!("enabledPlugins[\"{id}\"] is set in managed settings"),
        };
    }
    setting_knob(
        ctx,
        Scope::Local,
        &format!("/enabledPlugins/{}", esc(id)),
        Value::Bool(false),
        None,
    )
}

/// Every enabled plugin that applies to the target dir.
pub(crate) fn discover(ctx: &mut Ctx) -> Vec<Plugin> {
    let mut plugins = Vec::new();
    let installed_path = ctx.home().join(".claude/plugins/installed_plugins.json");
    let installed = match ctx.fs.read_json(&installed_path) {
        Json::Ok(v) => v,
        Json::Invalid(err) => {
            ctx.warnings.push(format!(
                "{}: invalid JSON, plugins skipped ({err})",
                installed_path.display()
            ));
            Value::Null
        }
        Json::Missing => Value::Null,
    };
    let installed_real = ctx.fs.realpath(&installed_path);
    let mut installed_ids: Vec<String> = Vec::new();
    if let Some(Value::Object(records)) = installed.get("plugins") {
        for (id, record) in records {
            installed_ids.push(id.clone());
            let entries: Vec<&Value> = match record {
                Value::Array(list) => list.iter().collect(),
                other => vec![other],
            };
            let Some(entry) = pick_entry(ctx, &entries) else {
                if enabled_setting(ctx, id).is_some_and(|(b, _, _)| b) {
                    ctx.warnings.push(format!(
                        "enabledPlugins[\"{id}\"] = true but {id} is installed only for other projects"
                    ));
                }
                continue;
            };
            let (name, marketplace) = id.split_once('@').unwrap_or((id.as_str(), ""));
            let version = entry
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string);
            let scope = entry
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("user")
                .to_string();
            let root = install_root(ctx, entry, name, marketplace, version.as_deref());
            let Some(root) = root else {
                if enabled_setting(ctx, id).is_some_and(|(b, _, _)| b) {
                    ctx.warnings
                        .push(format!("{id} is enabled but its install dir is missing"));
                }
                continue;
            };
            let manifest = read_manifest(ctx, &root);
            let (enabled, mut why, setting, source) = match enabled_setting(ctx, id) {
                Some((b, origin, source)) => (
                    b,
                    vec![format!("{origin} = {b}")],
                    Some(origin),
                    Some(source),
                ),
                None => match manifest.get("defaultEnabled").and_then(Value::as_bool) {
                    Some(b) => (
                        b,
                        vec![format!(
                            "no enabledPlugins entry; manifest defaultEnabled = {b}"
                        )],
                        None,
                        None,
                    ),
                    None => (false, Vec::new(), None, None),
                },
            };
            if !enabled {
                continue;
            }
            why.push(format!(
                "{}: {id} {} ({scope} scope)",
                installed_real.display(),
                version.as_deref().unwrap_or("?")
            ));
            why.push(root.display().to_string());
            plugins.push(Plugin {
                id: id.clone(),
                name: name.to_string(),
                layer: Layer::Plugin,
                root: Some(root),
                manifest,
                why,
                setting,
                knob: off_knob(ctx, id, source),
            });
        }
    }
    for (id, (_, origin)) in enabled_true_keys(ctx) {
        if !installed_ids.contains(&id)
            && !id.ends_with("@synced")
            && !id.ends_with("@builtin")
            && !id.ends_with("@inline")
        {
            ctx.warnings
                .push(format!("{origin} = true but {id} is not installed"));
        }
    }
    plugins.extend(synced(ctx));
    for name in BUILTIN_PLUGINS {
        let id = format!("{name}@builtin");
        let mut why =
            vec!["built-in plugin (static table, Claude Code 2.1.296 init event)".to_string()];
        let mut setting = None;
        if let Some((b, origin, _)) = enabled_setting(ctx, &id) {
            if !b {
                continue;
            }
            why.insert(0, format!("{origin} = true"));
            setting = Some(origin);
        }
        plugins.push(Plugin {
            id,
            name: name.to_string(),
            layer: Layer::Builtin,
            root: None,
            manifest: Value::Object(Map::new()),
            why,
            setting,
            knob: Knob::None {
                reason: "built-in plugin; toggled in /plugin (settings key unverified)".to_string(),
            },
        });
    }
    plugins
}

fn enabled_true_keys(ctx: &Ctx) -> BTreeMap<String, (bool, String)> {
    let mut out = BTreeMap::new();
    for file in &ctx.settings.files {
        if let Some(Value::Object(map)) = file.value.get("enabledPlugins") {
            for (id, v) in map {
                if let Some(b) = v.as_bool() {
                    out.insert(
                        id.clone(),
                        (b, file.at(&format!("/enabledPlugins/{}", esc(id)))),
                    );
                }
            }
        }
    }
    out.retain(|_, (b, _)| *b);
    out
}

/// The install record that applies to the target: a local/project install whose `projectPath`
/// contains the target dir, else the user install.
fn pick_entry<'v>(ctx: &Ctx, entries: &[&'v Value]) -> Option<&'v Value> {
    let scoped = entries.iter().find(|e| {
        matches!(
            e.get("scope").and_then(Value::as_str),
            Some("local") | Some("project")
        ) && e
            .get("projectPath")
            .and_then(Value::as_str)
            .is_some_and(|p| ctx.dir().starts_with(ctx.fs.realpath(Path::new(p))))
    });
    scoped
        .or_else(|| {
            entries.iter().find(|e| {
                matches!(
                    e.get("scope").and_then(Value::as_str),
                    None | Some("user") | Some("managed")
                )
            })
        })
        .copied()
}

fn install_root(
    ctx: &Ctx,
    entry: &Value,
    name: &str,
    marketplace: &str,
    version: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = entry.get("installPath").and_then(Value::as_str) {
        let p = PathBuf::from(p);
        if ctx.fs.is_dir(&p) {
            return Some(ctx.fs.realpath(&p));
        }
    }
    let fallback = ctx
        .home()
        .join(".claude/plugins/cache")
        .join(marketplace)
        .join(name)
        .join(version?);
    ctx.fs.is_dir(&fallback).then(|| ctx.fs.realpath(&fallback))
}

/// claude.ai-synced plugins: `~/.claude/plugins/synced/<bucket>/<name>/.claude-plugin/plugin.json`.
fn synced(ctx: &Ctx) -> Vec<Plugin> {
    let mut out = Vec::new();
    if let Some((false, _)) = ctx.settings.get_bool("/syncClaudeAiPlugins") {
        return out;
    }
    let base = ctx.home().join(".claude/plugins/synced");
    for bucket in ctx.fs.read_dir(&base) {
        if file_name(&bucket).starts_with('.') || !ctx.fs.is_dir(&bucket) {
            continue;
        }
        for dir in ctx.fs.read_dir(&bucket) {
            let name = file_name(&dir);
            if name.starts_with('.') || !ctx.fs.is_file(&dir.join(".claude-plugin/plugin.json")) {
                continue;
            }
            let id = format!("{name}@synced");
            let mut why = vec![format!(
                "synced from claude.ai into {}",
                ctx.display(&bucket)
            )];
            let mut setting = None;
            let mut source = None;
            if let Some((b, origin, src)) = enabled_setting(ctx, &id) {
                if !b {
                    continue;
                }
                why.insert(0, format!("{origin} = true"));
                setting = Some(origin);
                source = Some(src);
            }
            let root = ctx.fs.realpath(&dir);
            let manifest = read_manifest(ctx, &root);
            why.push(root.display().to_string());
            out.push(Plugin {
                id: id.clone(),
                name,
                layer: Layer::ClaudeAi,
                root: Some(root),
                manifest,
                why,
                setting,
                knob: off_knob(ctx, &id, source),
            });
        }
    }
    out
}

/// One `Plugin` row per enabled plugin; its bytes live on the contribution rows (`origin.plugin`),
/// so the row is `—` and its mode is the strongest mode among its contributions.
pub(crate) fn items(plugins: &[Plugin], tree_items: &[Item]) -> Vec<Item> {
    let mut contributions: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    let mut stack: Vec<&Item> = tree_items.iter().collect();
    while let Some(item) = stack.pop() {
        if let Some(p) = item.origin.plugin.as_deref() {
            contributions.entry(p).or_default().push(item);
        }
        stack.extend(item.children.iter());
    }
    plugins
        .iter()
        .map(|p| {
            let rows = contributions
                .get(p.id.as_str())
                .cloned()
                .unwrap_or_default();
            let count = |kind: Kind| rows.iter().filter(|i| i.kind == kind).count();
            let mut item = new_item(Kind::Plugin, p.layer, &p.id, &p.id);
            item.origin.path = p.root.clone();
            item.origin.setting = p.setting.clone();
            item.origin.plugin = Some(p.id.clone());
            item.origin.why = p.why.clone();
            item.origin.why.push(format!(
                "contributes: {} skills · {} commands · {} agents · {} hooks · {} MCP servers",
                count(Kind::Skill),
                count(Kind::Command),
                count(Kind::SubAgent),
                count(Kind::Hook),
                count(Kind::McpServer)
            ));
            item.origin.why.push(
                "cost carried by the contribution rows; freed via an enabledPlugins overlay"
                    .to_string(),
            );
            item.mode = strongest(rows.iter().map(|i| i.mode));
            item.cost = cost_na(None);
            item.knobs = vec![p.knob.clone()];
            item
        })
        .collect()
}

fn strongest(modes: impl Iterator<Item = Mode>) -> Mode {
    let rank = |m: Mode| match m {
        Mode::Always => 0,
        Mode::Deferred => 1,
        Mode::Advertised => 2,
        Mode::OnDemand => 3,
        Mode::Runtime => 4,
        Mode::None => 5,
    };
    modes.min_by_key(|m| rank(*m)).unwrap_or(Mode::None)
}
