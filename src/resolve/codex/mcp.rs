//! MCP servers (`research/02` §7): `[mcp_servers.<id>]` in the enabled config layers — system,
//! user, profile, each explicitly trusted project layer, legacy managed — merged first the way
//! Codex merges layers (`config/src/merge.rs:96-150`: tables key by key, any other value replacing
//! what is below it), then validated: a non-table that survives the merge is a warning (codex
//! refuses to start). One row per id, named by the highest layer whose value survives, its `why`
//! listing every such layer. Plugin
//! servers are the plugin work unit's (C2); the `requirements.toml` allowlist is not modelled (a
//! warning names it).
//!
//! A server with `enabled = false` (`config/src/mcp_types.rs:245-246`, default true) is mode `none`
//! with an `off:` signal. Otherwise its tool schemas — and its `initialize` instructions, which
//! become the tool namespace's description inside `tools[]` (`codex-mcp/src/rmcp_client.rs:839-856`)
//! — are unknown until the server has run: cost `?`. `enabled_tools` / `disabled_tools` filter the
//! tools (allow, then deny) and are named in the row's `signal`.
//!
//! Mode: `deferred` when the effective model's `models_cache.json` entry has
//! `supports_search_tool = true` — MCP tools then sit behind `tool_search` instead of the upfront
//! `tools[]` (`core/src/tools/spec_plan.rs:246-254`, `spec_plan.rs:653-655`; the provider half of
//! that test, `namespace_tools`, is true for every provider, `model-provider/src/provider.rs:53-63`)
//! — `always` when it is false or absent (`protocol/src/openai_models.rs:477-478`), and `always`
//! with an undecidable tree state when the entry is unknown.
//!
//! Knob: `mcp_servers.<id>.enabled = false` by the shared scope rule, minus a file for which the
//! server's `command`/`url` comes only from layers that can stop loading while that file still
//! loads (a profile not selected next time, another project layer): the knob would leave a table
//! without a transport there, and codex refuses to start ("invalid transport", `mcp_types.rs:553`).
//! Codex validates the merged config, not each file (`core/src/config/mod.rs:1536-1550`), so such
//! a table works until the defining layer drops out.

use super::config::{LayerFile, Lookup, Source};
use super::hooks::not_a;
use super::{config_knobs, layer_of, new_item, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Mode};
use crate::resolve::claude::cost_na;
use crate::resolve::claude::fs;
use serde_json::{json, Value};
use std::path::Path;

/// The tool-search state and its cause, from the model catalog.
pub(crate) fn tool_search(ctx: &Ctx) -> (Option<bool>, String) {
    if let Some(at) = &ctx.model.catalog_unread {
        return (
            None,
            format!("{at} is set: the catalog comes from that file (not read)"),
        );
    }
    let Some(slug) = &ctx.model.slug else {
        return match &ctx.model.slug_unreadable {
            Some(at) => (
                None,
                format!("{at} is not a string: model unknown (codex refuses to start with it)"),
            ),
            None => (
                None,
                "no model in config: codex picks its default from the catalog at startup"
                    .to_string(),
            ),
        };
    };
    let Some(entry) = &ctx.model.catalog else {
        return (
            None,
            format!("{slug} is not in models_cache.json: supports_search_tool unknown"),
        );
    };
    let (state, cause) = match entry.get("supports_search_tool") {
        Some(Value::Bool(true)) => (
            Some(true),
            format!("models_cache.json: {slug} supports_search_tool = true (MCP tools deferred behind tool_search)"),
        ),
        Some(Value::Bool(false)) => (
            Some(false),
            format!("models_cache.json: {slug} supports_search_tool = false"),
        ),
        _ => (
            Some(false),
            format!("models_cache.json: {slug} has no supports_search_tool (false)"),
        ),
    };
    // A best snapshot from a file codex does not (or may not) use as it is: said, not hidden
    // (the reason is in the `model:` signal). The capture's tool-search state replaces it in
    // verify.
    match ctx.model.cache_use.short_qualifier() {
        Some(q) => (state, format!("{cause}; models_cache.json, {q}")),
        None => (state, cause),
    }
}

struct Server<'a> {
    id: String,
    def: Value,
    /// Every enabled layer that defines the id, lowest first.
    layers: Vec<&'a LayerFile>,
}

/// The layer's value at `pointer` as the merge sees it: `null` that an overlay wrote means absent
/// and is dropped; `null` for a non-finite float ([`Lookup::Unreadable`]) is kept — a value that
/// replaces what is below it and that no typed reader accepts. None when absent.
fn readable(layer: &LayerFile, pointer: &str) -> Option<Value> {
    match layer.lookup(pointer) {
        Lookup::Absent => None,
        Lookup::Unreadable => Some(Value::Null),
        Lookup::Set(Value::Object(map)) => {
            let mut out = serde_json::Map::new();
            for key in map.keys() {
                if let Some(v) = readable(layer, &format!("{pointer}/{}", fs::esc(key))) {
                    out.insert(key.clone(), v);
                }
            }
            Some(Value::Object(out))
        }
        Lookup::Set(other) => Some(other.clone()),
    }
}

/// The effective `mcp_servers` the way Codex builds it: every enabled layer merged in order with
/// `merge_toml_values` (`config/src/merge.rs:96-150`) — tables key by key, any other value
/// replacing what is below it, so a scalar drops a lower table and a table replaces a lower
/// scalar — and only then validated. Provenance is the layers whose value survives in the
/// result.
fn effective_servers<'a>(ctx: &'a Ctx, warnings: &mut Vec<String>) -> Vec<Server<'a>> {
    let mut merged: Option<Value> = None;
    let mut last_setter: Option<&LayerFile> = None;
    let mut provenance: Vec<(String, Vec<&LayerFile>)> = Vec::new();
    for layer in ctx.config.layers.iter().filter(|l| l.enabled()) {
        let Some(cleaned) = readable(layer, "/mcp_servers") else {
            continue;
        };
        let value = &cleaned;
        last_setter = Some(layer);
        match (merged.as_mut(), value) {
            (Some(Value::Object(base)), Value::Object(over)) => {
                for (id, def) in over {
                    let merges = base.get(id).is_some_and(Value::is_object) && def.is_object();
                    match base.get_mut(id) {
                        Some(existing) if merges => merge(existing, def),
                        _ => {
                            base.insert(id.clone(), def.clone());
                        }
                    }
                    match provenance.iter_mut().find(|(i, _)| i == id) {
                        Some((_, layers)) if merges => layers.push(layer),
                        Some((_, layers)) => *layers = vec![layer],
                        None => provenance.push((id.clone(), vec![layer])),
                    }
                }
            }
            (_, Value::Object(over)) => {
                merged = Some(value.clone());
                provenance = over.keys().map(|id| (id.clone(), vec![layer])).collect();
            }
            _ => {
                merged = Some(value.clone());
                provenance.clear();
            }
        }
    }
    let table = match merged {
        None => return Vec::new(),
        Some(Value::Object(table)) => table,
        Some(_) => {
            if let Some(layer) = last_setter {
                warnings.push(format!(
                    "{}: not a table; codex refuses to start",
                    layer.at("/mcp_servers")
                ));
            }
            return Vec::new();
        }
    };
    let mut servers = Vec::new();
    for (id, layers) in provenance {
        let def = table.get(&id).cloned().unwrap_or(Value::Null);
        if !def.is_object() {
            if let Some(layer) = layers.last() {
                warnings.push(format!(
                    "{}: not a table; codex refuses to start",
                    layer.at(&pointer(&id, ""))
                ));
            }
            continue;
        }
        servers.push(Server { id, def, layers });
    }
    servers.sort_by(|a, b| a.id.cmp(&b.id));
    servers
}

pub(crate) fn collect(ctx: &mut Ctx, tool_search: Option<bool>) -> Vec<Item> {
    let (items, warnings) = rows(ctx, tool_search);
    ctx.warnings.extend(warnings);
    items
}

fn rows(ctx: &Ctx, tool_search: Option<bool>) -> (Vec<Item>, Vec<String>) {
    let mut warnings = Vec::new();
    let servers = effective_servers(ctx, &mut warnings);
    let any_enabled = servers
        .iter()
        .any(|s| s.def.get("enabled") != Some(&Value::Bool(false)));
    // In the merged definition `null` is a non-finite float (see [`readable`]).
    if any_enabled && tool_search.is_none() {
        warnings.push(
            "tool search state undecidable statically (model unknown or not in models_cache.json): MCP rows are listed as always"
                .to_string(),
        );
    }

    let mut out = Vec::new();
    for s in &servers {
        let top = *s.layers.last().expect("a server has a defining layer");
        let at = top.at(&pointer(&s.id, ""));
        for key in ["command", "url"] {
            if let Some(value) = s.def.get(key).filter(|v| !v.is_string()) {
                let field = pointer(&s.id, &format!("/{key}"));
                let set_at = setter(s, &field).unwrap_or_else(|| at.clone());
                warnings.push(format!(
                    "{}; codex refuses to start with it",
                    not_a(&set_at, value, "a string")
                ));
            }
        }
        let transport = match (s.def.get("command"), s.def.get("url")) {
            (Some(Value::String(_)), None) => "stdio",
            (None, Some(Value::String(_))) => "http",
            (Some(_), None) | (None, Some(_)) => "?",
            (Some(_), Some(_)) => {
                warnings.push(format!(
                    "{at}: both command and url; codex refuses to start (config/src/mcp_types.rs:499-500)"
                ));
                "?"
            }
            (None, None) => {
                warnings.push(format!(
                    "{at}: neither command nor url; codex refuses to start (invalid transport, config/src/mcp_types.rs:553)"
                ));
                "?"
            }
        };
        let mut item = new_item(Kind::McpServer, layer_of(top), &s.id, &s.id);
        item.origin.path = top.path.clone();
        item.origin.setting = Some(at);
        item.origin.why = s.layers.iter().map(|l| l.at(&pointer(&s.id, ""))).collect();
        if let Some(last) = item.origin.why.last_mut() {
            last.push_str(&format!(" ({transport})"));
        }
        item.knobs = knobs(ctx, s);

        let enabled_at = setter(s, &pointer(&s.id, "/enabled"));
        // Set but not a boolean: codex refuses the config; the row stays as if on, with the
        // reason, never silently the default.
        if let (Some(value), Some(at)) = (s.def.get("enabled"), &enabled_at) {
            if !value.is_boolean() {
                let bad = format!(
                    "{}; codex refuses to start with it",
                    not_a(at, value, "a boolean")
                );
                warnings.push(bad.clone());
                item.origin.why.push(bad);
            }
        }
        if let (Some(Value::Bool(false)), Some(at)) = (s.def.get("enabled"), enabled_at) {
            item.mode = Mode::None;
            item.cost = cost_na(None);
            item.origin.why.push(format!("{at} = false"));
            item.signal = Some(format!("off: {} = false", dotted(&s.id, "enabled")));
            out.push(item);
            continue;
        }
        item.cost = Cost::default();
        item.mode = if tool_search == Some(true) {
            Mode::Deferred
        } else {
            Mode::Always
        };
        item.origin.why.push(match tool_search {
            Some(true) => "tool search on: its tools sit behind tool_search; schemas unknown until the server has run",
            Some(false) => "tool search off: every tool schema is sent up front; size unknown until the server has run",
            None => "tool search undecidable; schema size unknown until the server has run",
        }.to_string());
        item.origin.why.push(
            "server instructions become the tool namespace description in tools[] (codex-mcp/src/rmcp_client.rs:839-856)"
                .to_string(),
        );
        let filters: Vec<String> = ["enabled_tools", "disabled_tools"]
            .iter()
            .filter_map(|key| {
                let list = s.def.get(*key)?.as_array()?;
                let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
                Some(format!("{key} [{}]", names.join(", ")))
            })
            .collect();
        if !filters.is_empty() {
            item.signal = Some(format!("tools: {}", filters.join(" · ")));
        }
        out.push(item);
    }
    (out, warnings)
}

/// `"<file>#<pointer>"` of the highest surviving layer that sets `pointer` (readable or not).
fn setter(s: &Server, pointer: &str) -> Option<String> {
    s.layers
        .iter()
        .rev()
        .find(|l| !matches!(l.lookup(pointer), Lookup::Absent))
        .map(|l| l.at(pointer))
}

/// `mcp_servers.<id>.enabled = false` in the narrowest file that takes effect, kept only when the
/// server's `command`/`url` comes from a layer that loads whenever that file does: the system,
/// user and legacy managed files always load, the file itself loads with itself. A profile (not
/// selected next time) or another project layer (its own trust decision) can drop out and leave
/// the written table without a transport.
fn knobs(ctx: &Ctx, s: &Server) -> Vec<Knob> {
    let ptr = pointer(&s.id, "/enabled");
    let defines_transport = |file: &Path| {
        let file = fs::normalize(file);
        s.layers.iter().any(|l| {
            let stable = matches!(l.source, Source::System | Source::User | Source::Managed)
                || l.path.as_deref().map(fs::normalize).as_ref() == Some(&file);
            stable
                && l.value.pointer(&pointer(&s.id, "")).is_some_and(|d| {
                    d.get("command").is_some_and(Value::is_string)
                        || d.get("url").is_some_and(Value::is_string)
                })
        })
    };
    let note = "this server only; its config stays";
    let mut out: Vec<Knob> = config_knobs(ctx, &ptr, json!(false), Some(note))
        .into_iter()
        .filter(|k| match k {
            Knob::Setting { file, .. } => defines_transport(file),
            _ => true,
        })
        .collect();
    if out.is_empty() {
        let defined_in: Vec<String> = s
            .layers
            .iter()
            .filter_map(|l| l.path.as_ref().map(|p| ctx.display(p)))
            .collect();
        out.push(Knob::None {
            reason: format!(
                "set {} = false in {}: a file harn writes would hold a server table whose command/url comes from a layer that can stop loading",
                dotted(&s.id, "enabled"),
                defined_in.join(" or ")
            ),
        });
    }
    out
}

/// `/mcp_servers/<id><rest>`.
fn pointer(id: &str, rest: &str) -> String {
    format!("/mcp_servers/{}{rest}", fs::esc(id))
}

/// `mcp_servers.<id>.<key>` as written in TOML (the id quoted unless it is a bare key).
fn dotted(id: &str, key: &str) -> String {
    let bare = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        format!("mcp_servers.{id}.{key}")
    } else {
        format!(
            "mcp_servers.{}.{key}",
            serde_json::to_string(id).expect("a string serializes")
        )
    }
}

/// Layer merge: tables key by key, every other value replaced.
fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge(existing, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{walk, Layer, Overlay, OverlayOp, Scope, Target, Tree};
    use crate::resolve::codex::resolve_with;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "harn-codex-mcp-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::write(root.join("repo/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        Tmp(root)
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn target(root: &Path) -> Target {
        Target {
            dir: root.join("repo"),
            home: root.join("home"),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        }
    }

    fn resolve(root: &Path, overlay: Option<&Overlay>) -> Tree {
        resolve_with(&target(root), overlay, &root.join("etc/codex")).unwrap()
    }

    fn server<'t>(tree: &'t Tree, id: &str) -> &'t Item {
        walk(tree)
            .find(|i| i.kind == Kind::McpServer && i.label == id)
            .unwrap_or_else(|| panic!("no server {id}"))
    }

    fn servers(tree: &Tree) -> Vec<&str> {
        walk(tree)
            .filter(|i| i.kind == Kind::McpServer)
            .map(|i| i.label.as_str())
            .collect()
    }

    fn trust(root: &Path) -> String {
        format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            root.join("repo").display()
        )
    }

    const USER: &str = r#"
[mcp_servers.docs]
command = "docs-server"
args = ["--stdio"]
enabled_tools = ["search", "fetch"]
disabled_tools = ["fetch"]

[mcp_servers.remote]
url = "https://mcp.example.test/mcp"
enabled = false
"#;

    #[test]
    fn user_and_trusted_project_servers_with_their_switches() {
        let t = tmp();
        let root = &t.0;
        let codex = root.join("home/.codex");
        write(&codex.join("config.toml"), USER);
        write(
            &root.join("repo/.codex/config.toml"),
            "[mcp_servers.local]\ncommand = \"local-server\"\n",
        );
        let tree = resolve(root, None);
        assert_eq!(
            servers(&tree),
            ["docs", "remote"],
            "project waits for trust"
        );
        let docs = server(&tree, "docs");
        assert_eq!(docs.id, "codex/mcp-server/user/docs");
        assert_eq!(
            (docs.mode, docs.cost.clone()),
            (Mode::Always, Cost::default())
        );
        assert_eq!(
            docs.signal.as_deref(),
            Some("tools: enabled_tools [search, fetch] · disabled_tools [fetch]")
        );
        assert!(docs.origin.why[0].ends_with("config.toml#/mcp_servers/docs (stdio)"));
        match docs.knobs.as_slice() {
            [Knob::Setting {
                file,
                pointer,
                value,
                scope,
                ..
            }] => {
                assert_eq!(file, &codex.join("config.toml"));
                assert_eq!(pointer, "/mcp_servers/docs/enabled");
                assert_eq!(value, &json!(false));
                assert_eq!(*scope, Scope::User);
            }
            other => panic!("{other:?}"),
        }
        let remote = server(&tree, "remote");
        assert_eq!(remote.mode, Mode::None);
        assert!(remote.cost.not_applicable);
        assert_eq!(
            remote.signal.as_deref(),
            Some("off: mcp_servers.remote.enabled = false")
        );

        write(
            &codex.join("config.toml"),
            &format!("{USER}{}", trust(root)),
        );
        let tree = resolve(root, None);
        assert_eq!(servers(&tree), ["docs", "local", "remote"]);
        let local = server(&tree, "local");
        assert_eq!(local.origin.layer, Layer::Project);
        // Defined only in the project file: no user-file knob (it would hold no transport).
        match local.knobs.as_slice() {
            [Knob::Setting { file, scope, .. }] => {
                assert_eq!(*scope, Scope::Project);
                assert_eq!(file, &root.join("repo/.codex/config.toml"));
            }
            other => panic!("{other:?}"),
        }
        // A user server reachable from both files gets both knobs, narrowest first.
        assert!(matches!(
            server(&tree, "docs").knobs.as_slice(),
            [
                Knob::Setting {
                    scope: Scope::Project,
                    ..
                },
                Knob::Setting {
                    scope: Scope::User,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn layers_merge_and_the_highest_names_the_server() {
        let t = tmp();
        let root = &t.0;
        write(
            &root.join("home/.codex/config.toml"),
            &format!("{USER}{}", trust(root)),
        );
        write(
            &root.join("repo/.codex/config.toml"),
            "[mcp_servers.remote]\nenabled = true\n",
        );
        let tree = resolve(root, None);
        let remote = server(&tree, "remote");
        assert_eq!(remote.origin.layer, Layer::Project);
        assert_eq!(remote.mode, Mode::Always, "the project layer re-enables it");
        assert_eq!(remote.origin.why.len(), 4, "{:?}", remote.origin.why);
        assert!(
            remote.origin.why[1].ends_with("repo/.codex/config.toml#/mcp_servers/remote (http)")
        );
    }

    #[test]
    fn deferral_follows_the_model_catalog() {
        let t = tmp();
        let root = &t.0;
        let codex = root.join("home/.codex");
        let cache = |search: Option<bool>| {
            let mut entry = json!({"slug": "m", "context_window": 1000});
            if let Some(s) = search {
                entry["supports_search_tool"] = json!(s);
            }
            write(
                &codex.join("models_cache.json"),
                &json!({"models": [entry]}).to_string(),
            );
        };
        write(
            &codex.join("config.toml"),
            &format!("model = \"m\"\n{USER}"),
        );

        cache(Some(true));
        let tree = resolve(root, None);
        assert_eq!(tree.tool_search, Some(true));
        assert!(tree
            .tool_search_cause
            .as_deref()
            .unwrap()
            .contains("m supports_search_tool = true"));
        assert_eq!(server(&tree, "docs").mode, Mode::Deferred);
        assert_eq!(server(&tree, "remote").mode, Mode::None);
        assert_eq!(tree.unknown_rows, unknown_without_mcp(&tree) + 1);

        cache(Some(false));
        let tree = resolve(root, None);
        assert_eq!(tree.tool_search, Some(false));
        assert_eq!(server(&tree, "docs").mode, Mode::Always);

        cache(None);
        assert_eq!(
            resolve(root, None).tool_search,
            Some(false),
            "absent = false"
        );

        write(&codex.join("config.toml"), USER);
        let tree = resolve(root, None);
        assert_eq!(tree.tool_search, None, "no model in config");
        assert_eq!(server(&tree, "docs").mode, Mode::Always);
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.starts_with("tool search state undecidable")));
    }

    fn unknown_without_mcp(tree: &Tree) -> u32 {
        walk(tree)
            .filter(|i| i.kind != Kind::McpServer)
            .filter(|i| {
                matches!(i.mode, Mode::Always | Mode::Advertised | Mode::Deferred)
                    && crate::resolve::claude::is_unknown(&i.cost)
            })
            .count() as u32
    }

    #[test]
    fn overlay_disabling_a_server_removes_its_cost() {
        let t = tmp();
        let root = &t.0;
        let codex = root.join("home/.codex");
        write(&codex.join("config.toml"), USER);
        let tree = resolve(root, None);
        let docs = server(&tree, "docs");
        assert_eq!(docs.mode, Mode::Always);
        let Knob::Setting {
            file,
            pointer,
            value,
            ..
        } = docs.knobs[0].clone()
        else {
            panic!("{:?}", docs.knobs)
        };
        let overlay = Overlay {
            ops: vec![OverlayOp::SetJson {
                file,
                pointer,
                value,
            }],
        };
        let after = resolve(root, Some(&overlay));
        let docs = server(&after, "docs");
        assert_eq!(docs.mode, Mode::None);
        assert!(docs.cost.not_applicable);
        assert_eq!(after.unknown_rows + 1, tree.unknown_rows);
    }

    #[test]
    fn quoted_ids_and_invalid_transports() {
        assert_eq!(dotted("a-b_1", "enabled"), "mcp_servers.a-b_1.enabled");
        assert_eq!(dotted("x.y", "enabled"), "mcp_servers.\"x.y\".enabled");
        let t = tmp();
        let root = &t.0;
        write(
            &root.join("home/.codex/config.toml"),
            "[mcp_servers.\"x.y\"]\nenabled = false\n",
        );
        let tree = resolve(root, None);
        let row = server(&tree, "x.y");
        assert_eq!(
            row.signal.as_deref(),
            Some("off: mcp_servers.\"x.y\".enabled = false")
        );
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.contains("neither command nor url")));
        assert!(matches!(row.knobs.as_slice(), [Knob::None { .. }]));
    }

    fn profile_target(root: &Path) -> Target {
        let mut t = target(root);
        t.env.insert(
            crate::resolve::codex::PROFILE_ENV.to_string(),
            "work".to_string(),
        );
        t
    }

    #[test]
    fn a_higher_table_replaces_a_lower_scalar() {
        let t = tmp();
        let root = &t.0;
        write(
            &root.join("home/.codex/config.toml"),
            &format!("[mcp_servers]\nx = \"bad\"\n{}", trust(root)),
        );
        write(
            &root.join("repo/.codex/config.toml"),
            "[mcp_servers.x]\ncommand = \"x-server\"\n",
        );
        let tree = resolve(root, None);
        assert!(
            !tree.warnings.iter().any(|w| w.contains("refuses to start")),
            "{:?}",
            tree.warnings
        );
        let x = server(&tree, "x");
        assert_eq!((x.origin.layer, x.mode), (Layer::Project, Mode::Always));
        assert_eq!(
            x.origin.why.len(),
            3,
            "only the project table contributes: {:?}",
            x.origin.why
        );
        assert!(x.origin.why[0].ends_with("#/mcp_servers/x (stdio)"));
    }

    #[test]
    fn a_middle_scalar_drops_the_lower_command() {
        let t = tmp();
        let root = &t.0;
        let codex = root.join("home/.codex");
        write(
            &codex.join("config.toml"),
            &format!("[mcp_servers.x]\ncommand = \"old\"\n{}", trust(root)),
        );
        write(&codex.join("work.config.toml"), "[mcp_servers]\nx = 1\n");
        write(
            &root.join("repo/.codex/config.toml"),
            "[mcp_servers.x]\nurl = \"https://mcp.example.test/x\"\n",
        );
        let tree = resolve_with(&profile_target(root), None, &root.join("etc/codex")).unwrap();
        assert!(
            !tree
                .warnings
                .iter()
                .any(|w| w.contains("both command and url")),
            "{:?}",
            tree.warnings
        );
        let x = server(&tree, "x");
        assert!(x.origin.why[0].ends_with("repo/.codex/config.toml#/mcp_servers/x (http)"));
        assert_eq!(x.origin.why.len(), 3, "{:?}", x.origin.why);

        // A scalar on top is what codex sees: it refuses to start.
        write(&codex.join("work.config.toml"), "");
        write(
            &root.join("repo/.codex/config.toml"),
            "[mcp_servers]\nx = 1\n",
        );
        let tree = resolve_with(&profile_target(root), None, &root.join("etc/codex")).unwrap();
        assert!(servers(&tree).is_empty());
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.contains("#/mcp_servers/x: not a table; codex refuses to start")));
    }

    #[test]
    fn a_transport_from_a_profile_gives_no_stub_knob() {
        let t = tmp();
        let root = &t.0;
        let codex = root.join("home/.codex");
        write(&codex.join("config.toml"), &trust(root));
        write(
            &codex.join("work.config.toml"),
            "[mcp_servers.p]\ncommand = \"p-server\"\n",
        );
        std::fs::create_dir_all(root.join("repo/.codex")).unwrap();
        let tree = resolve_with(&profile_target(root), None, &root.join("etc/codex")).unwrap();
        let p = server(&tree, "p");
        assert_eq!(p.mode, Mode::Always);
        // Neither the project nor the user file: without the profile, a stub there has no
        // transport.
        match p.knobs.as_slice() {
            [Knob::None { reason }] => assert!(reason.contains("work.config.toml"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_tool_search_warning_needs_an_enabled_server() {
        let t = tmp();
        let root = &t.0;
        write(
            &root.join("home/.codex/config.toml"),
            "[mcp_servers.off]\ncommand = \"c\"\nenabled = false\n",
        );
        let tree = resolve(root, None);
        assert_eq!(tree.tool_search, None);
        assert!(!tree
            .warnings
            .iter()
            .any(|w| w.starts_with("tool search state undecidable")));
    }

    #[test]
    fn a_non_finite_or_wrong_typed_enabled_is_set_and_warned() {
        let t = tmp();
        let root = &t.0;
        let codex = root.join("home/.codex");
        write(
            &codex.join("config.toml"),
            "[mcp_servers.a]\ncommand = \"a\"\nenabled = nan\n[mcp_servers.b]\ncommand = 7\n",
        );
        let tree = resolve(root, None);
        let a = server(&tree, "a");
        assert!(
            a.origin
                .why
                .iter()
                .any(|w| w.contains("/mcp_servers/a/enabled") && w.contains("non-finite")),
            "{:?}",
            a.origin.why
        );
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.contains("/mcp_servers/a/enabled") && w.contains("codex refuses to start")));
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.contains("#/mcp_servers/b/command = 7 is not a string")));
        // A non-string command is no transport for a stub knob.
        assert!(matches!(
            server(&tree, "b").knobs.as_slice(),
            [Knob::None { .. }]
        ));
    }

    #[test]
    fn a_catalog_file_or_unreadable_model_leaves_tool_search_unknown() {
        let t = tmp();
        let root = &t.0;
        let codex = root.join("home/.codex");
        write(
            &codex.join("models_cache.json"),
            &json!({"models": [{"slug": "m", "supports_search_tool": true}]}).to_string(),
        );
        write(
            &codex.join("config.toml"),
            "model = \"m\"\nmodel_catalog_json = 5\n",
        );
        let tree = resolve(root, None);
        assert_eq!(tree.tool_search, None, "{:?}", tree.tool_search_cause);
        write(&codex.join("config.toml"), "model = 5\n");
        let tree = resolve(root, None);
        assert_eq!(tree.tool_search, None);
        assert!(tree
            .tool_search_cause
            .as_deref()
            .unwrap()
            .contains("not a string"));
    }
}
