//! Plugins (`research/02` §6), as codex-cli 0.160.1 loads them (paths below are relative to
//! `codex-rs/` at commit d27764b8):
//!
//! - a plugin is a `[plugins."<name>@<marketplace>"]` key of the effective config
//!   (`core-plugins/src/marketplace_policy.rs:210-311`; marketplace allow-lists from
//!   `requirements.toml` are not modelled); `enabled` defaults to true (`config/src/types.rs:1002-1011`);
//!   keys load in sorted order (`core-plugins/src/loader.rs:163-195`); `features.plugins = false`
//!   turns them all off (default on, `features/src/lib.rs:1456-1461`). `plugins` is typed in the
//!   config (`config/src/config_toml.rs:495`): a wrong-typed or non-finite value there, or in
//!   `features.plugins`, stops codex — harn warns, and a plugin whose own entry is unreadable is
//!   listed loading nothing (neither value is guessed);
//! - its install is `$CODEX_HOME/plugins/cache/<marketplace>/<name>/<version>/`: the `local`
//!   version when present, else the highest by the `semver` crate's order, byte order for names
//!   that are not SemVer (`core-plugin-common/src/installed.rs:40-68`);
//! - its manifest is the first of `.codex-plugin/plugin.json`, `.claude-plugin/plugin.json`,
//!   `.cursor-plugin/plugin.json` (`exec-server-protocol/src/protocol.rs:49-53`). A typed field of
//!   the wrong type (`name`, `version`, `description`, `keywords`, `apps`, `interface.*`) fails
//!   the whole manifest and the plugin loads nothing (`core-plugins/src/manifest.rs:48-117`,
//!   `loader.rs:879-882`); an invalid `skills`, `mcpServers` or `hooks` value — wrong type, or a
//!   path that does not start with `./`, contains `..` or leaves the plugin — is ignored with a
//!   warning and the default location is used (`manifest.rs:134-158`, `:407-466`, `:581-659`);
//! - skills: the manifest's `skills` paths, else `skills/`, plus
//!   `.codex-plugin/migrated-command-skills/` (`loader.rs:1081-1109`), named
//!   `<manifest name>:<skill>` (`loader.rs:906`, `manifest.rs:296-299`) — they are listed in the
//!   skill listing ([`super::skills`]) with `origin.plugin` set;
//! - MCP servers: the manifest's `mcpServers` (a path or an inline object), else `.mcp.json`; a
//!   file is `{"mcpServers": {…}}` or a bare server map (`loader.rs:1111-1132`, `:1534-1590`,
//!   `codex-mcp/src/plugin_config.rs:52-64`); `plugins."<p>".mcp_servers.<s>.enabled = false`
//!   skips one (`loader.rs:968-970`);
//! - hooks: the manifest's `hooks` (paths or inline objects), else `hooks/hooks.json`
//!   (`loader.rs:1188-1296`), parsed with the hooks module's [`hooks::parse_hooks_file`] and handed
//!   to [`hooks::collect`] as [`hooks::PluginHooks`]: keys
//!   `<plugin>:<file or plugin.json#hooks[i]>:<event>:<group>:<handler>`
//!   (`hooks/src/declarations.rs:35-37`, `discovery.rs:279-282`), state, trust hash and
//!   classification are the config hooks' (one rule set, `hooks/src/engine/discovery.rs:244-290`).
//!   Codex also reads hook state from `-c` session flags, which harn does not see. Not modelled:
//!   the allow-listed bundled cleanup hooks codex trusts without state
//!   (`is_allowlisted_bundled_cleanup_hook`, `discovery.rs:667-675`).
//!
//! Rows follow the Claude Code resolver: a plugin's parts carry `origin.plugin` and sit where
//! their kind sits (skills in the listing, MCP servers and hooks top-level); the `plugin` row
//! is a switch: no cost (`—`), no children (each part exists once in the tree), mode `always`
//! while the plugin loads and `none` otherwise.

use super::{config_knobs, hooks, json_type, new_item, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode};
use crate::resolve::claude::cost_na;
use crate::resolve::claude::fs::{esc, file_name, Json};
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

/// `exec-server-protocol/src/protocol.rs:49-53`.
const MANIFEST_PATHS: &[&str] = &[
    ".codex-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
];
/// `core-plugin-common/src/installed.rs:10`.
const LOCAL_VERSION: &str = "local";
/// `<plugins_instructions>` as rendered (`core/src/context/available_plugins_instructions.rs:30-48`,
/// snapshot `core/src/context/world_state/snapshots/…plugins_instructions__tests__snapshots.snap`).
const PLUGINS_INSTRUCTIONS: &str = r###"<plugins_instructions>
## Plugins
A plugin is a local bundle of skills, MCP servers, and apps.
### How to use plugins
- Skill naming: If a plugin contributes skills, those skill entries are prefixed with `plugin_name:` in the Skills list.
- MCP naming: Plugin-provided MCP tools keep standard MCP identifiers such as `mcp__server__tool`; use tool provenance to tell which plugin they come from.
- Trigger rules: If the user explicitly names a plugin, prefer capabilities associated with that plugin for that turn.
- Relationship to capabilities: Plugins are not invoked directly. Use their underlying skills, MCP tools, and app tools to help solve the task.
- Relevance: Determine what a plugin can help with from explicit user mention or from the plugin-associated skills, MCP tools, and apps exposed elsewhere in this turn.
- Missing/blocked: If the user requests a plugin that does not have relevant callable capabilities for the task, say so briefly and continue with the best fallback.
</plugins_instructions>"###;

pub(crate) struct Plugin {
    /// `name@marketplace` (the config key).
    pub id: String,
    /// Realpath of the active install; None when not installed.
    pub root: Option<PathBuf>,
    pub version: Option<String>,
    /// The parsed manifest; None when the plugin does not load (off, not installed, no manifest).
    pub manifest: Option<Value>,
    pub manifest_path: Option<PathBuf>,
    /// Skill name prefix: the manifest `name`, else the install dir name.
    pub namespace: String,
    /// `"<file>#/plugins/<id>/enabled"` when a config layer sets it.
    pub setting: Option<String>,
    /// Admission chain, top-down.
    pub why: Vec<String>,
    /// Why a configured plugin contributes nothing (shown as its signal).
    pub inactive: Option<String>,
    /// Its off switch, narrowest first.
    pub knobs: Vec<Knob>,
    /// The manifest's `skills`, `mcpServers` and `hooks` after codex's checks.
    declared: Declared,
}

/// What a manifest declares once codex has checked it (`core-plugins/src/manifest.rs:279-659`): an
/// invalid `skills`, `mcpServers` or `hooks` value is ignored with a warning and the default
/// location is used, exactly as when the field is absent.
#[derive(Default)]
struct Declared {
    /// Valid `skills` paths; empty = the default `skills/` (`loader.rs:1081-1109`).
    skills: Vec<PathBuf>,
    mcp: McpDecl,
    hooks: HooksDecl,
}

#[derive(Default)]
enum McpDecl {
    /// `.mcp.json` when it is a file (`loader.rs:1111-1132`).
    #[default]
    Default,
    File(PathBuf),
    /// An inline object (`manifest.rs:449-457`).
    Inline(Map<String, Value>),
}

#[derive(Default)]
enum HooksDecl {
    /// `hooks/hooks.json` when it is a file (`loader.rs:1229-1242`).
    #[default]
    Default,
    Files(Vec<PathBuf>),
    /// Inline `HooksFile` objects (`manifest.rs:423-429`), parsed by [`hooks::parse_hooks_file`];
    /// the index of each is its key source `plugin.json#hooks[<i>]`.
    Inline(Vec<hooks::Events>),
}

impl Plugin {
    fn loaded(&self) -> bool {
        self.manifest.is_some()
    }

    fn manifest_field_is_object(&self, key: &str) -> bool {
        self.manifest
            .as_ref()
            .and_then(|m| m.get(key))
            .is_some_and(Value::is_object)
    }

    /// The admission chain followed by the contribution's own line.
    pub fn why_with(&self, line: String) -> Vec<String> {
        let mut why = self.why.clone();
        why.push(line);
        why
    }

    /// The plugin's narrowest off switch, annotated for a contribution row.
    pub fn contribution_knob(&self) -> Knob {
        match &self.knobs[0] {
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

    /// Skill roots (`loader.rs:1081-1109`): the manifest's valid `skills` paths, else `skills/`
    /// when it is a dir, plus `.codex-plugin/migrated-command-skills/` when it is a dir; sorted.
    pub fn skill_roots(&self, ctx: &Ctx) -> Vec<PathBuf> {
        let Some(root) = self.root.as_ref().filter(|_| self.loaded()) else {
            return Vec::new();
        };
        let mut paths = if self.declared.skills.is_empty() {
            let default = root.join("skills");
            if ctx.fs.is_dir(&default) {
                vec![default]
            } else {
                Vec::new()
            }
        } else {
            self.declared.skills.clone()
        };
        let migrated = root.join(".codex-plugin/migrated-command-skills");
        if ctx.fs.is_dir(&migrated) {
            paths.push(migrated);
        }
        paths.sort();
        paths.dedup();
        paths
    }
}

/// `compare_plugin_versions` (`core-plugin-common/src/installed.rs:40-45`): `semver::Version`
/// order when both parse (the same crate codex uses; its `Ord` breaks ties on build metadata),
/// else byte order.
fn compare_versions(a: &str, b: &str) -> Ordering {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

/// `validate_plugin_version_segment` (`core-plugin-common/src/installed.rs:21-38`).
fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && !matches!(v, "." | "..")
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

/// `active_plugin_version` (`core-plugin-common/src/installed.rs:47-68`).
fn active_version(ctx: &Ctx, base: &Path) -> Option<String> {
    let mut versions: Vec<String> = ctx
        .fs
        .read_dir(base)
        .into_iter()
        // `file_type().is_dir()`: a symlinked version dir does not count (installed.rs:52).
        .filter(|p| ctx.fs.is_dir(p) && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        .map(|p| file_name(&p))
        .filter(|v| valid_version(v))
        .collect();
    if versions.iter().any(|v| v == LOCAL_VERSION) {
        return Some(LOCAL_VERSION.to_string());
    }
    // Sort then pop, as codex does: with a mix of SemVer and non-SemVer names the order is not
    // total, so the same procedure is the closest match.
    versions.sort_unstable_by(|a, b| compare_versions(a, b));
    versions.pop()
}

/// The configured plugin keys: every `plugins` table key of an enabled layer, sorted.
fn configured_keys(ctx: &Ctx) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for layer in ctx.config.layers.iter().filter(|l| l.enabled()) {
        if let Some(Value::Object(map)) = layer.value.get("plugins") {
            keys.extend(map.keys().cloned());
        }
    }
    keys
}

/// `features.plugins`: on unless set to `false`. Set to anything else (a non-finite float reads
/// as `null`), codex refuses to start: the plugins stay listed as if on — never silently the
/// default — and the second value says why, for a warning and the rows' `why`.
fn plugins_feature(ctx: &Ctx) -> (bool, Option<String>) {
    match ctx.config.get("/features/plugins") {
        Some((Value::Bool(false), _)) => (false, None),
        Some((Value::Bool(true), _)) | None => (true, None),
        Some((value, layer)) => (
            true,
            Some(format!(
                "{}; codex refuses to start with it",
                hooks::not_a(&layer.at("/features/plugins"), value, "a boolean")
            )),
        ),
    }
}

/// Every configured plugin, enabled or not (`core-plugins/src/loader.rs:823-966`).
pub(crate) fn discover(ctx: &mut Ctx) -> Vec<Plugin> {
    let (on, bad) = plugins_feature(ctx);
    if !on {
        return Vec::new();
    }
    ctx.warnings.extend(bad.clone());
    // `plugins` is `HashMap<String, PluginConfig>` in the config (`config/src/config_toml.rs:495`,
    // `config/src/types.rs:1002-1043`): a wrong type anywhere in it stops codex.
    if let Some((value, layer)) = ctx.config.get("/plugins") {
        if !value.is_object() {
            ctx.warnings.push(format!(
                "{}; codex refuses to start with it",
                hooks::not_a(&layer.at("/plugins"), value, "a table")
            ));
        }
    }
    let mut out = Vec::new();
    for id in configured_keys(ctx) {
        let Some((name, marketplace)) = id
            .rsplit_once('@')
            .filter(|(n, m)| !n.is_empty() && !m.is_empty())
        else {
            ctx.warnings.push(format!(
                "plugins.\"{id}\": invalid plugin key, expected <plugin>@<marketplace>; codex skips it"
            ));
            continue;
        };
        let table = format!("/plugins/{}", esc(&id));
        let pointer = format!("{table}/enabled");
        let knob_note = format!("turns off the plugin {id} (its skills, MCP servers and hooks)");
        let knobs = config_knobs(ctx, &pointer, json!(false), Some(&knob_note));
        // Set but unreadable (not a table, or `enabled` not a boolean): codex refuses to start.
        // The row stays, loading nothing, rather than guessing either value.
        let unreadable = match ctx.config.get(&table) {
            Some((value, layer)) if !value.is_object() => {
                Some(hooks::not_a(&layer.at(&table), value, "a table"))
            }
            _ => match ctx.config.get(&pointer) {
                Some((value, layer)) if !value.is_boolean() => {
                    Some(hooks::not_a(&layer.at(&pointer), value, "a boolean"))
                }
                _ => None,
            },
        };
        if let Some(why) = unreadable {
            let why = format!("{why}; codex refuses to start with it");
            ctx.warnings.push(why.clone());
            out.push(Plugin {
                id: id.clone(),
                root: None,
                version: None,
                manifest: None,
                manifest_path: None,
                namespace: name.to_string(),
                setting: Some(
                    ctx.config
                        .get(&table)
                        .map(|(_, l)| l.at(&table))
                        .unwrap_or_default(),
                ),
                why: vec![why],
                inactive: Some("unreadable: codex refuses to start with this config".to_string()),
                knobs,
                declared: Declared::default(),
            });
            continue;
        }
        let (enabled, setting) = match ctx.config.get(&pointer) {
            Some((Value::Bool(b), layer)) => (*b, Some(layer.at(&pointer))),
            _ => (true, None),
        };
        let mut why = vec![match &setting {
            Some(at) => format!("{at} = {enabled}"),
            None => format!("[plugins.\"{id}\"] without `enabled`: on by default"),
        }];
        why.extend(bad.clone());
        let mut plugin = Plugin {
            id: id.clone(),
            root: None,
            version: None,
            manifest: None,
            manifest_path: None,
            namespace: name.to_string(),
            setting,
            why: Vec::new(),
            inactive: None,
            knobs,
            declared: Declared::default(),
        };
        if !enabled {
            plugin.inactive = Some("off: plugins.enabled = false".to_string());
            plugin.knobs = vec![Knob::None {
                reason: format!("already off ({})", why[0]),
            }];
            plugin.why = why;
            out.push(plugin);
            continue;
        }
        let base = ctx
            .codex_home
            .join("plugins/cache")
            .join(marketplace)
            .join(name);
        let Some(version) = active_version(ctx, &base) else {
            ctx.warnings.push(format!(
                "plugins.\"{id}\" is enabled but not installed (no version under {}); codex loads nothing from it",
                ctx.display(&base)
            ));
            plugin.inactive = Some("not installed".to_string());
            plugin.why = why;
            out.push(plugin);
            continue;
        };
        let root = ctx.fs.realpath(&base.join(&version));
        why.push(format!(
            "{} ({}; core-plugin-common/src/installed.rs:47-68)",
            ctx.display(&root),
            if version == LOCAL_VERSION {
                "the `local` version wins".to_string()
            } else {
                format!("highest version {version}")
            }
        ));
        plugin.root = Some(root.clone());
        plugin.version = Some(version);
        match read_manifest(ctx, &root) {
            Ok((path, manifest)) => {
                let mut warnings = Vec::new();
                plugin.declared = declare(&id, &root, &manifest, &mut warnings);
                ctx.warnings.extend(warnings);
                let declared = manifest
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty());
                plugin.namespace = match declared {
                    Some(n) => n.to_string(),
                    None => file_name(&root),
                };
                why.push(ctx.display(&path));
                plugin.manifest_path = Some(path);
                plugin.manifest = Some(manifest);
            }
            Err(err) => {
                ctx.warnings
                    .push(format!("plugin {id}: {err}; codex loads nothing from it"));
                plugin.inactive = Some(err);
            }
        }
        plugin.why = why;
        out.push(plugin);
    }
    out
}

/// The first manifest that exists, parsed. A root `plugin.json` naming the agent-plugins schema is
/// the Agent Plugin format, which harn does not model.
fn read_manifest(ctx: &Ctx, root: &Path) -> Result<(PathBuf, Value), String> {
    let agent = root.join("plugin.json");
    if ctx
        .fs
        .read_string(&agent)
        .is_some_and(|t| t.contains("https://agent-plugins.org/schemas/"))
    {
        return Err("Agent Plugin manifest (plugin.json) is not modelled by harn".to_string());
    }
    for rel in MANIFEST_PATHS {
        let path = root.join(rel);
        if !ctx.fs.is_file(&path) {
            continue;
        }
        let invalid = |why: String| {
            format!(
                "{}: {why}: missing or invalid plugin.json (core-plugins/src/manifest.rs:48-73, loader.rs:879-882)",
                path.display()
            )
        };
        return match ctx.fs.read_json(&path) {
            Json::Ok(v) => match manifest_error(&v) {
                None => Ok((ctx.fs.realpath(&path), v)),
                Some(why) => Err(invalid(why)),
            },
            Json::Invalid(err) => Err(invalid(format!("invalid JSON ({err})"))),
            Json::Missing => continue,
        };
    }
    Err("missing or invalid plugin.json".to_string())
}

/// Why `RawPluginManifest` (`core-plugins/src/manifest.rs:48-73`, camelCase keys, unknown keys
/// accepted) would not deserialize: a typed field of the wrong type fails the whole manifest.
/// `skills`, `mcpServers` and `hooks` never fail it (each has an `Invalid` catch-all, :134-158),
/// nor does `extensions` (any JSON) or `interface.defaultPrompt`.
fn manifest_error(manifest: &Value) -> Option<String> {
    let Some(m) = manifest.as_object() else {
        return Some(format!("a {} instead of an object", json_type(manifest)));
    };
    let string = |key: &str, v: Option<&Value>, optional: bool| match v {
        None => None,
        Some(Value::String(_)) => None,
        Some(Value::Null) if optional => None,
        Some(other) => Some(format!("`{key}` is a {}, not a string", json_type(other))),
    };
    let strings = |key: &str, v: Option<&Value>| match v {
        None => None,
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => None,
        Some(_) => Some(format!("`{key}` is not an array of strings")),
    };
    string("name", m.get("name"), false)
        .or_else(|| string("version", m.get("version"), true))
        .or_else(|| string("description", m.get("description"), true))
        .or_else(|| strings("keywords", m.get("keywords")))
        .or_else(|| string("apps", m.get("apps"), true))
        .or_else(|| match m.get("interface") {
            None | Some(Value::Null) => None,
            Some(Value::Object(i)) => {
                // `RawPluginManifestInterface` (:81-117), camelCase with `…URL` aliases.
                const STRINGS: &[&str] = &[
                    "displayName",
                    "shortDescription",
                    "longDescription",
                    "developerName",
                    "category",
                    "websiteUrl",
                    "websiteURL",
                    "privacyPolicyUrl",
                    "privacyPolicyURL",
                    "termsOfServiceUrl",
                    "termsOfServiceURL",
                    "brandColor",
                    "composerIcon",
                    "logo",
                    "logoDark",
                ];
                STRINGS
                    .iter()
                    .find_map(|k| string(&format!("interface.{k}"), i.get(*k), true))
                    .or_else(|| strings("interface.capabilities", i.get("capabilities")))
                    .or_else(|| strings("interface.screenshots", i.get("screenshots")))
            }
            Some(other) => Some(format!(
                "`interface` is a {}, not an object",
                json_type(other)
            )),
        })
}

/// `resolve_manifest_path` (`core-plugins/src/manifest.rs:607-659`): `./…` under the plugin root,
/// no `..`, not absolute; an empty string is ignored silently, anything else invalid is warned.
fn manifest_path(
    id: &str,
    root: &Path,
    field: &str,
    path: &str,
    warnings: &mut Vec<String>,
) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let mut warn = |why: &str| {
        warnings.push(format!(
            "plugin {id}: ignoring {field}: {why} (core-plugins/src/manifest.rs:607-659)"
        ))
    };
    let Some(rel) = path.strip_prefix("./") else {
        warn("path must start with `./` relative to plugin root");
        return None;
    };
    if rel.is_empty() {
        warn("path must not be `./`");
        return None;
    }
    if rel.split('/').any(|c| c == "..") {
        warn("path must not contain '..'");
        return None;
    }
    if rel.starts_with('/') {
        warn("path must stay within the plugin root");
        return None;
    }
    Some(root.join(rel))
}

/// The manifest's `skills`, `mcpServers` and `hooks` as codex resolves them
/// (`core-plugins/src/manifest.rs:386-393`, `:407-466`, `:581-605`): absent or null = default
/// location; invalid = a warning, then the default location.
fn declare(id: &str, root: &Path, manifest: &Value, warnings: &mut Vec<String>) -> Declared {
    let mut d = Declared::default();
    let found = |field: &str, expected: &str, v: &Value, warnings: &mut Vec<String>| {
        warnings.push(format!(
            "plugin {id}: ignoring {field}: expected {expected}; found {} (core-plugins/src/manifest.rs:407-466, :596-602)",
            json_type(v)
        ));
    };
    match manifest.get("skills") {
        None | Some(Value::Null) => {}
        Some(Value::String(p)) => d
            .skills
            .extend(manifest_path(id, root, "skills", p, warnings)),
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
            for p in items.iter().filter_map(Value::as_str) {
                d.skills
                    .extend(manifest_path(id, root, "skills", p, warnings));
            }
        }
        Some(other) => found("skills", "a string or string array", other, warnings),
    }
    d.mcp = match manifest.get("mcpServers") {
        None | Some(Value::Null) => McpDecl::Default,
        Some(Value::String(p)) => match manifest_path(id, root, "mcpServers", p, warnings) {
            Some(path) => McpDecl::File(path),
            None => McpDecl::Default,
        },
        Some(Value::Object(inline)) => McpDecl::Inline(inline.clone()),
        Some(other) => {
            found("mcpServers", "a string or object", other, warnings);
            McpDecl::Default
        }
    };
    let hooks_expected = "a string, string array, object, or object array";
    d.hooks = match manifest.get("hooks") {
        None | Some(Value::Null) => HooksDecl::Default,
        Some(Value::String(p)) => match manifest_path(id, root, "hooks", p, warnings) {
            Some(path) => HooksDecl::Files(vec![path]),
            None => HooksDecl::Default,
        },
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
            let files: Vec<PathBuf> = items
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|p| manifest_path(id, root, "hooks", p, warnings))
                .collect();
            if files.is_empty() {
                HooksDecl::Default
            } else {
                HooksDecl::Files(files)
            }
        }
        Some(inline @ Value::Object(_)) if hooks::parse_hooks_file(inline).is_ok() => {
            HooksDecl::Inline(hooks::parse_hooks_file(inline).into_iter().collect())
        }
        Some(Value::Array(items))
            if !items.is_empty()
                && items
                    .iter()
                    .all(|i| i.is_object() && hooks::parse_hooks_file(i).is_ok()) =>
        {
            HooksDecl::Inline(
                items
                    .iter()
                    .filter_map(|i| hooks::parse_hooks_file(i).ok())
                    .collect(),
            )
        }
        Some(other) => {
            let detail = match hooks::parse_hooks_file(other) {
                Err(err) if other.is_object() => format!("{hooks_expected} ({err})"),
                _ => hooks_expected.to_string(),
            };
            found("hooks", &detail, other, warnings);
            HooksDecl::Default
        }
    };
    d
}

/// The parts of every loaded plugin that are not skills: its MCP server rows, and its hook
/// sources for [`hooks::collect`] (which builds their rows with the config hooks' rules).
pub(crate) fn parts(ctx: &mut Ctx, plugins: &[Plugin]) -> (Vec<Item>, Vec<hooks::PluginHooks>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let mut servers_seen: BTreeMap<String, String> = BTreeMap::new();
    for p in plugins.iter().filter(|p| p.loaded()) {
        for item in mcp_servers(ctx, p, &mut warnings) {
            if let Some(prev) = servers_seen.insert(item.label.clone(), p.id.clone()) {
                warnings.push(format!(
                    "MCP server {} comes from both {prev} and {}; codex warns about the duplicate name (core-plugins/src/loader.rs:180-191)",
                    item.label, p.id
                ));
            }
            out.push(item);
        }
    }
    let mut hook_sources = Vec::new();
    for p in plugins.iter().filter(|p| p.loaded()) {
        hook_sources.extend(plugin_hooks(ctx, p, &mut warnings));
    }
    ctx.warnings.extend(warnings);
    (out, hook_sources)
}

/// Gives a plugin's hook rows (built by [`hooks::collect`], `origin.plugin` set) the plugin's
/// admission chain and its off switch after the per-hook state knob.
pub(crate) fn decorate_hooks(plugins: &[Plugin], items: &mut [Item]) {
    for item in items.iter_mut().filter(|i| i.kind == Kind::Hook) {
        let Some(p) = plugins
            .iter()
            .find(|p| item.origin.plugin.as_deref() == Some(p.id.as_str()))
        else {
            continue;
        };
        let mut why = p.why.clone();
        why.append(&mut item.origin.why);
        item.origin.why = why;
        let at = 1.min(item.knobs.len());
        item.knobs.insert(at, p.contribution_knob());
    }
}

/// The two plugin MCP file shapes (`codex-mcp/src/plugin_config.rs:46-64`): `{"mcpServers":
/// {…}}`, else a bare server map. Returns the pointer prefix and the servers.
fn server_map(value: &Map<String, Value>) -> (String, Map<String, Value>) {
    match value.get("mcpServers") {
        Some(Value::Object(servers)) => ("/mcpServers".to_string(), servers.clone()),
        _ => (String::new(), value.clone()),
    }
}

/// `(file, pointer prefix, servers)` of a plugin's MCP config (`loader.rs:1534-1650`): the inline
/// object, the declared file, or `.mcp.json`. A missing declared file gives nothing; a file that
/// is not a JSON object is warned and gives nothing.
fn mcp_sources(
    ctx: &Ctx,
    p: &Plugin,
    warnings: &mut Vec<String>,
) -> Vec<(PathBuf, String, Map<String, Value>)> {
    let root = p.root.as_ref().expect("a loaded plugin has a root");
    let file = match &p.declared.mcp {
        McpDecl::Inline(inline) => {
            let manifest = p.manifest_path.clone().unwrap_or_default();
            let (prefix, servers) = server_map(inline);
            return vec![(manifest, format!("/mcpServers{prefix}"), servers)];
        }
        McpDecl::File(path) => path.clone(),
        McpDecl::Default => root.join(".mcp.json"),
    };
    if !ctx.fs.is_file(&file) {
        return Vec::new();
    }
    match ctx.fs.read_json(&file) {
        Json::Ok(Value::Object(map)) => {
            let (prefix, servers) = server_map(&map);
            vec![(ctx.fs.realpath(&file), prefix, servers)]
        }
        Json::Ok(other) => {
            warnings.push(format!(
                "plugin {}: {}: failed to parse plugin MCP config (a {}); codex loads no server from it (loader.rs:1650-1661)",
                p.id,
                ctx.display(&file),
                json_type(&other)
            ));
            Vec::new()
        }
        Json::Invalid(err) => {
            warnings.push(format!(
                "plugin {}: {}: failed to parse plugin MCP config ({err}); codex loads no server from it (loader.rs:1650-1661)",
                p.id,
                ctx.display(&file)
            ));
            Vec::new()
        }
        Json::Missing => Vec::new(),
    }
}

fn mcp_servers(ctx: &Ctx, p: &Plugin, warnings: &mut Vec<String>) -> Vec<Item> {
    let mut out = Vec::new();
    let search = super::mcp::tool_search(ctx).0 == Some(true);
    for (file, prefix, servers) in mcp_sources(ctx, p, warnings) {
        for (name, def) in &servers {
            if !def.is_object() {
                continue;
            }
            let at = format!("{}#{prefix}/{}", file.display(), esc(name));
            let mut item = new_item(Kind::McpServer, Layer::Plugin, name, name);
            item.origin.path = Some(file.clone());
            item.origin.setting = Some(at.clone());
            item.origin.plugin = Some(p.id.clone());
            item.origin.why = p.why_with(at);
            let pointer = format!("/plugins/{}/mcp_servers/{}/enabled", esc(&p.id), esc(name));
            match ctx.config.get(&pointer) {
                Some((Value::Bool(false), layer)) => {
                    item.mode = Mode::None;
                    item.cost = cost_na(None);
                    item.signal = Some("off: plugins.mcp_servers.enabled = false".to_string());
                    item.origin.why.push(format!(
                        "{} = false: codex skips this server (core-plugins/src/loader.rs:968-970)",
                        layer.at(&pointer)
                    ));
                }
                other => {
                    if let Some((value, layer)) = other.filter(|(v, _)| !v.is_boolean()) {
                        let why = format!(
                            "{}; codex refuses to start with it",
                            hooks::not_a(&layer.at(&pointer), value, "a boolean")
                        );
                        warnings.push(why.clone());
                        item.origin.why.push(why);
                    }
                    item.origin.why.push(
                        "tool schemas and server instructions are unknown until the server runs"
                            .to_string(),
                    );
                    if search {
                        item.origin.why.push(
                            "the model supports tool search: codex may defer these tools behind tool_search (research/02 §7)"
                                .to_string(),
                        );
                    }
                }
            }
            let mut knobs = config_knobs(
                ctx,
                &pointer,
                json!(false),
                Some("skips this plugin MCP server; the plugin stays on"),
            );
            knobs.retain(|k| !matches!(k, Knob::None { .. }));
            knobs.push(p.contribution_knob());
            item.knobs = knobs;
            out.push(item);
        }
    }
    out
}

/// A plugin's hook sources (`loader.rs:1188-1296`): inline objects keyed `plugin.json#hooks[<i>]`
/// (empty ones skipped, their index kept), else the declared files, else `hooks/hooks.json`. A
/// file that cannot be read or is not a valid hooks file is warned and skipped.
fn plugin_hooks(ctx: &Ctx, p: &Plugin, warnings: &mut Vec<String>) -> Vec<hooks::PluginHooks> {
    let root = p.root.as_ref().expect("a loaded plugin has a root");
    let mut out = Vec::new();
    let files: Vec<PathBuf> = match &p.declared.hooks {
        HooksDecl::Inline(inline) => {
            let single = inline.len() == 1 && p.manifest_field_is_object("hooks");
            for (i, events) in inline.iter().enumerate() {
                if events.is_empty() {
                    continue;
                }
                out.push(hooks::PluginHooks {
                    plugin_id: p.id.clone(),
                    file: manifest_or_default(p),
                    relative: format!("plugin.json#hooks[{i}]"),
                    pointer: if single {
                        "/hooks/hooks".to_string()
                    } else {
                        format!("/hooks/{i}/hooks")
                    },
                    events: events.clone(),
                });
            }
            return out;
        }
        HooksDecl::Files(files) => files.clone(),
        HooksDecl::Default => {
            let default = root.join("hooks/hooks.json");
            if ctx.fs.is_file(&default) {
                vec![default]
            } else {
                Vec::new()
            }
        }
    };
    for file in files {
        let parsed = match ctx.fs.read_json(&file) {
            Json::Ok(v) => hooks::parse_hooks_file(&v),
            Json::Missing => {
                warnings.push(format!(
                    "plugin {}: failed to read plugin hooks config {} (loader.rs:1257-1266)",
                    p.id,
                    ctx.display(&file)
                ));
                continue;
            }
            Json::Invalid(err) => Err(err),
        };
        let events = match parsed {
            Ok(events) => events,
            Err(err) => {
                warnings.push(format!(
                    "plugin {}: failed to parse plugin hooks config {} ({err}; loader.rs:1267-1276)",
                    p.id,
                    ctx.display(&file)
                ));
                continue;
            }
        };
        if events.is_empty() {
            continue;
        }
        let relative = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        out.push(hooks::PluginHooks {
            plugin_id: p.id.clone(),
            file: ctx.fs.realpath(&file),
            relative,
            pointer: "/hooks".to_string(),
            events,
        });
    }
    out
}

fn manifest_or_default(p: &Plugin) -> PathBuf {
    p.manifest_path.clone().unwrap_or_else(|| {
        p.root
            .clone()
            .unwrap_or_default()
            .join(".codex-plugin/plugin.json")
    })
}

/// `<plugins_instructions>` and `<recommended_plugins>` (developer blocks), while
/// `features.plugins` is on.
pub(crate) fn blocks(ctx: &Ctx, plugins: &[Plugin], tree_items: &[Item]) -> Vec<Item> {
    let (on, bad) = plugins_feature(ctx);
    if !on {
        return Vec::new();
    }
    let knobs = config_knobs(
        ctx,
        "/features/plugins",
        json!(false),
        Some("removes every plugin with its skills, MCP servers and hooks; no finer switch is known (research/02 §6 open question 3)"),
    );
    let mut out = Vec::new();
    // `plugins_available()`: a loaded plugin with a part (`core/src/session/world_state.rs:271-272`).
    let with_parts = plugins.iter().any(|p| {
        p.loaded()
            && walk_items(tree_items).any(|i| i.origin.plugin.as_deref() == Some(p.id.as_str()))
    });
    if with_parts {
        out.extend(plugins_instructions(ctx, bad.as_deref(), &knobs));
    }
    let mut rec = new_item(
        Kind::SystemPrompt,
        Layer::Builtin,
        "recommended_plugins",
        "recommended plugins",
    );
    rec.origin.why = vec![
        "`<recommended_plugins>` developer block: plugins available in the marketplaces but not installed (core/src/context/recommended_plugins_instructions.rs:5-55), decided at startup".to_string(),
        "lead, not verified at the call site: features/src/lib.rs:531-535 gates recommendations on apps && plugins && (tool_suggest || recommended_plugins)".to_string(),
    ];
    rec.origin.why.extend(bad);
    rec.knobs = knobs;
    out.push(rec);
    out
}

/// `<plugins_instructions>` while a plugin is available: only when the model's catalog entry sets
/// `include_plugin_usage_instructions` (`core/src/session/world_state.rs:271-272`), read as codex
/// reads it ([`Ctx::catalog_flag`]: missing is `false`).
fn plugins_instructions(ctx: &Ctx, bad: Option<&str>, knobs: &[Knob]) -> Option<Item> {
    const CONDITION: &str = "include_plugin_usage_instructions = true";
    let flag = ctx.catalog_flag("include_plugin_usage_instructions");
    if flag == Ok(false) {
        return None;
    }
    let mut item = new_item(
        Kind::SystemPrompt,
        Layer::Builtin,
        "plugins_instructions",
        "plugins instructions",
    );
    item.origin.why = vec![
        "`<plugins_instructions>` developer block: fixed text (core/src/context/available_plugins_instructions.rs:30-48)".to_string(),
        "rendered when a plugin is available and the model's catalog entry sets include_plugin_usage_instructions (core/src/session/world_state.rs:271-272)".to_string(),
    ];
    let bytes = PLUGINS_INSTRUCTIONS.len() as u64;
    if flag == Ok(true) {
        item.cost = Cost {
            source: Some(bytes),
            predicted: Some(bytes),
            ..Cost::default()
        };
        item.origin.why.push(
            "models_cache.json: include_plugin_usage_instructions = true; whether a plugin is available is decided at startup".to_string(),
        );
    } else if let Err(why) = flag {
        item.signal = Some(Ctx::catalog_gated(CONDITION, &why));
        item.origin
            .why
            .push(format!("size {bytes} bytes if rendered"));
    }
    item.origin.why.extend(bad.map(str::to_string));
    item.knobs = knobs.to_vec();
    Some(item)
}

/// One `plugin` row per configured plugin. The row is a switch: its bytes live on its parts
/// (`origin.plugin`), so it is `—`; its mode is `always` while the plugin loads (enabled,
/// installed, readable manifest — even with no parts) and `none` otherwise.
pub(crate) fn items(plugins: &[Plugin], tree_items: &[Item]) -> Vec<Item> {
    plugins
        .iter()
        .map(|p| {
            let parts: Vec<&Item> = walk_items(tree_items)
                .filter(|i| {
                    i.kind != Kind::Plugin && i.origin.plugin.as_deref() == Some(p.id.as_str())
                })
                .collect();
            let count = |kind: Kind| parts.iter().filter(|i| i.kind == kind).count();
            let mut item = new_item(Kind::Plugin, Layer::Plugin, &p.id, &p.id);
            item.origin.path = p.root.clone();
            item.origin.setting = p.setting.clone();
            item.origin.plugin = Some(p.id.clone());
            item.origin.why = p.why.clone();
            if let Some(v) = &p.version {
                item.origin.why.push(format!("version {v}"));
            }
            item.origin.why.push(format!(
                "contributes: {} skills · {} MCP servers · {} hooks (apps not modelled)",
                count(Kind::Skill),
                count(Kind::McpServer),
                count(Kind::Hook)
            ));
            item.origin.why.push(
                "cost carried by the part rows; freed via the plugins.enabled overlay".to_string(),
            );
            // A switch, not a cost: on while the plugin loads, none otherwise.
            item.mode = if p.loaded() { Mode::Always } else { Mode::None };
            item.cost = cost_na(None);
            item.signal = p.inactive.clone();
            item.knobs = p.knobs.clone();
            item
        })
        .collect()
}

/// Every item of `items`, depth-first.
fn walk_items(items: &[Item]) -> impl Iterator<Item = &Item> {
    let mut stack: Vec<&Item> = items.iter().rev().collect();
    std::iter::from_fn(move || {
        let item = stack.pop()?;
        stack.extend(item.children.iter().rev());
        Some(item)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_follow_the_semver_crate() {
        // Invalid SemVer (leading zeros, empty or malformed pre-release / build) compares as bytes.
        assert_eq!(compare_versions("01.0.0", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0-", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-a..b", "1.0.0-a"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0-01", "1.0.0-1"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0+", "1.0.0"), Ordering::Greater);
        // A pre-release sorts before its release.
        assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0"), Ordering::Less);
        // Build metadata does not change precedence...
        assert_eq!(compare_versions("1.0.0+zzz", "1.0.1"), Ordering::Less);
        assert_eq!(
            compare_versions("1.0.0+b", "1.0.0-rc.1+z"),
            Ordering::Greater
        );
        // ...but the crate's total order breaks ties with it (semver::Version derives Ord).
        assert_eq!(compare_versions("1.0.0+b", "1.0.0+a"), Ordering::Greater);
    }

    #[test]
    fn versions_compare_as_semver_then_bytes() {
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.0-beta"), Ordering::Greater);
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0-beta.10"),
            Ordering::Less
        );
        assert_eq!(compare_versions("abc", "abd"), Ordering::Less);
        assert!(valid_version("1.0.0+build"));
        assert!(!valid_version(".."));
        assert!(!valid_version("1/0"));
    }
}
