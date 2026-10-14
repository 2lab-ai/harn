//! Hooks (`research/02` §4), as codex-cli 0.160.1 discovers them (`hooks/src/engine/discovery.rs`
//! at `rust-v0.160.1`, cited below as `discovery:<line>`). Sources, in Codex's display order:
//!
//! 1. managed `[hooks]` in `/etc/codex/requirements.toml` (`discovery:208-242`), keyed by its
//!    `managed_dir` when absolute, else by the file (`discovery:297-309`);
//! 2. every enabled config layer, lowest first: the layer folder's `hooks.json` (system
//!    `/etc/codex`, user `$CODEX_HOME` — read once for user and profile —, project `.codex`;
//!    `config/src/state.rs:219-243`), then the layer's inline `[hooks]` (`discovery:129-187`).
//!    Both forms in one layer load, with Codex's warning. The system and user folders are read
//!    even without a `config.toml` (those layers always exist, `config/src/loader/mod.rs:283-322`);
//!    a project layer only while its dir is explicitly trusted (an untrusted layer is not listed:
//!    the tree's `trust:` signal says so). In a linked worktree a project layer's hooks — both
//!    forms — come from the main checkout's matching `.codex` (`loader:1106-1116`,
//!    `loader:1778-1800`).
//!
//! Plugin hooks are the plugin work unit's (C2).
//!
//! One row per handler: Codex keys trust state per handler,
//! `<source_path>:<event_snake>:<group>:<handler>` (`hooks/src/lib.rs:94-123`), where the indices
//! count every group and handler of the event, skipped ones included (`discovery:487-503`). A
//! group whose matcher is neither match-all, an exact `|` list nor a valid `regex` pattern is
//! skipped with Codex's warning (`discovery:487-502`, `hooks/src/events/common.rs:130-135`); a
//! load failure in `requirements.toml` hooks stops the session (`hooks/src/registry.rs:83-89`).
//!
//! A handler runs when it is enabled and trusted (`discovery:664-734`): managed sources (system
//! layer, legacy `managed_config.toml`, requirements) always; any other only while
//! `[hooks.state."<key>"]` — read from the user and profile files only, keys trimmed, later
//! layers winning field by field (`hooks/src/config_rules.rs:8-66`) — has no `enabled = false` and a `trusted_hash` equal to the handler's current hash
//! ([`hook_hash`], `discovery:766-821`). `features.hooks = false` drops every handler
//! (`hooks/src/engine/mod.rs:238-261`); `allow_managed_hooks_only = true` in requirements skips
//! every non-managed source (`discovery:83-92`). A handler that does not run stays a row: mode
//! `none`, `signal` = `off: <why>`, its knobs as when on.
//!
//! What reaches the first request ([`classify`]): `SessionStart` whose matcher matches the source
//! `startup` with Codex's semantics ([`matcher_matches`]: unanchored `regex` search unless the
//! matcher is match-all or an exact list) and `UserPromptSubmit` stdout
//! or `additionalContext` become developer messages before the first sampling (`core/src/session/turn.rs:322`,
//! `turn.rs:368`; `core/src/hook_runtime.rs:128-175`, `hook_runtime.rs:716-772`) — mode `always`,
//! cost `?` (unknown until a capture), spilled to disk with a preview above `additionalContextLimit`
//! tokens (default 2,500, `hooks/src/output_spill.rs:12`). `SubagentStart` fires only in a spawned
//! subagent's session (`hook_runtime.rs:133-151`), an `async` handler's output is drained after
//! sampling (`turn.rs:175`, `turn.rs:553`), every other event runs later: mode `runtime`.
//!
//! Knobs: `[hooks.state."<key>"] enabled = false` in the entry that decides `enabled` today (its
//! file — user or profile — and its key as written), else in the user config; a project file is
//! never read for hook state. Then `features.hooks = false` by the shared scope rule. Managed
//! hooks ignore hook state: `Knob::None`.
//!
//! [`classify`], [`hook_key`], [`hook_hash`], [`normalize`], the parsers and the matcher
//! functions do not depend on where a hook comes from, so plugin hooks can reuse them.

use super::config::{self, LayerFile, Lookup, Source};
use super::{config_knobs, new_item, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use crate::resolve::claude::cost_na;
use crate::resolve::claude::fs::{self, Json};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// `HookEventsToml` fields in declaration order (`config/src/hook_config.rs:35-61`, iterated by
/// `into_matcher_groups`, `hook_config.rs:128-150`) with their state-key labels
/// (`hooks/src/lib.rs:95-110`).
pub(crate) const EVENTS: [(&str, &str); 12] = [
    ("PreToolUse", "pre_tool_use"),
    ("PermissionRequest", "permission_request"),
    ("PostToolUse", "post_tool_use"),
    ("PreCompact", "pre_compact"),
    ("PostCompact", "post_compact"),
    ("SessionStart", "session_start"),
    ("SessionEnd", "session_end"),
    ("UserPromptSubmit", "user_prompt_submit"),
    ("SubagentStart", "subagent_start"),
    ("SubagentStop", "subagent_stop"),
    ("Stop", "stop"),
    ("Interrupt", "interrupt"),
];

/// Events whose handlers may set `additionalContextLimit` (`discovery:539-556`).
const CONTEXT_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "SessionStart",
    "UserPromptSubmit",
    "SubagentStart",
];

/// `DEFAULT_HOOK_OUTPUT_TOKEN_LIMIT` (`hooks/src/output_spill.rs:12`).
const DEFAULT_CONTEXT_LIMIT: u64 = 2_500;

const REQUIREMENTS: &str = "requirements.toml";

#[derive(Debug, Clone)]
pub(crate) enum Handler {
    Command {
        command: String,
        command_windows: Option<String>,
        timeout: Option<u64>,
        is_async: bool,
        status_message: Option<String>,
        context_limit: Option<u64>,
    },
    McpTool {
        server: String,
        tool: String,
        input: Map<String, Value>,
        timeout: Option<u64>,
        status_message: Option<String>,
    },
    /// `prompt` / `agent`: parsed, never run (`discovery:635-654`).
    Unsupported(&'static str),
}

#[derive(Debug, Clone)]
pub(crate) struct Group {
    pub matcher: Option<String>,
    pub handlers: Vec<Handler>,
}

/// Event index into [`EVENTS`] and its matcher groups.
pub(crate) type Events = Vec<(usize, Vec<Group>)>;

struct HookSource {
    /// The key source and the file a row points at.
    path: PathBuf,
    /// The file the handlers are written in (`path` unless a requirements `managed_dir` names
    /// the key source).
    file: PathBuf,
    layer: Layer,
    managed: bool,
    /// Requirements hooks: a handler Codex cannot load stops the session
    /// (`discovery:60-72`, `hooks/src/registry.rs:83-89`).
    required: bool,
    events: Events,
}

/// Appended to a load warning of a required (requirements) source.
const REQUIRED_FAILS: &str =
    "; codex refuses to start (failed to load required managed hooks, hooks/src/registry.rs:83-89)";

/// Effective `[hooks.state."<key>"]` field: the value and `"<file>#<pointer>"` where it is set.
#[derive(Default)]
struct State {
    enabled: Option<(bool, String)>,
    /// The entry that decides `enabled`: its file, its key as written (Codex trims keys) and
    /// whether it is the profile.
    enabled_in: Option<(PathBuf, String, bool)>,
    trusted_hash: Option<(String, String)>,
}

pub(crate) fn collect(ctx: &mut Ctx) -> Vec<Item> {
    let (items, warnings) = rows(ctx);
    ctx.warnings.extend(warnings);
    items
}

fn rows(ctx: &mut Ctx) -> (Vec<Item>, Vec<String>) {
    let requirements = read_requirements(ctx);
    let managed_only = match &requirements {
        Some((path, value)) => match value.get("allow_managed_hooks_only") {
            Some(Value::Bool(true)) => Some(format!(
                "{}#/allow_managed_hooks_only = true",
                path.display()
            )),
            // `null` = a non-finite float, already warned by `read_requirements`.
            None | Some(Value::Bool(false)) | Some(Value::Null) => None,
            Some(other) => {
                ctx.warnings.push(format!(
                    "{}; codex refuses to start with it",
                    not_a(
                        &format!("{}#/allow_managed_hooks_only", path.display()),
                        other,
                        "a boolean"
                    )
                ));
                None
            }
        },
        None => None,
    };
    // Set but not a boolean: codex refuses the config; the hooks stay as if on, never silently
    // the default, with the reason on every row.
    let (features_off, features_bad) = match ctx.config.get("/features/hooks") {
        Some((Value::Bool(false), layer)) => (
            Some(format!("{} = false", layer.at("/features/hooks"))),
            None,
        ),
        Some((Value::Bool(true), _)) | None => (None, None),
        Some((value, layer)) => (
            None,
            Some(format!(
                "{}; codex refuses to start with it",
                not_a(&layer.at("/features/hooks"), value, "a boolean")
            )),
        ),
    };
    if let Some(bad) = &features_bad {
        ctx.warnings.push(bad.clone());
    }
    let sources = sources(ctx, requirements.as_ref());
    let feature_knobs: Vec<Knob> = config_knobs(
        ctx,
        "/features/hooks",
        json!(false),
        Some("turns off every hook"),
    )
    .into_iter()
    .filter(|k| matches!(k, Knob::Setting { .. }))
    .collect();

    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let ctx: &Ctx = ctx;
    for source in &sources {
        for (event_index, groups) in &source.events {
            let (event, snake) = EVENTS[*event_index];
            for (g, group) in groups.iter().enumerate() {
                let matcher = matcher_for_event(event, group.matcher.as_deref());
                // An invalid matcher skips the whole group; its index stays taken
                // (`discovery:487-502`).
                if let Some(Err(err)) = matcher.map(validate_matcher) {
                    let mut warning = format!(
                        "invalid matcher {:?} in {}: {}",
                        matcher.unwrap_or_default(),
                        source.path.display(),
                        one_line(&err.to_string())
                    );
                    if source.required && !group.handlers.is_empty() {
                        warning.push_str(REQUIRED_FAILS);
                    }
                    warnings.push(warning);
                    continue;
                }
                for (h, handler) in group.handlers.iter().enumerate() {
                    let file = ctx.display(&source.file);
                    let before = warnings.len();
                    let normalized = normalize(&file, event, handler, &mut warnings);
                    if source.required {
                        for w in &mut warnings[before..] {
                            w.push_str(REQUIRED_FAILS);
                        }
                    }
                    let Some(normalized) = normalized else {
                        continue;
                    };
                    let key = hook_key(&source.path.display().to_string(), snake, g, h);
                    let hash = hook_hash(snake, matcher, &normalized);
                    let state = hook_state(ctx, &key);
                    let mut item = row(ctx, source, event, g, h, matcher, &key);
                    item.origin.why.extend(features_bad.clone());
                    let off = off_reason(
                        source.managed,
                        &state,
                        &hash,
                        &key,
                        features_off.as_deref(),
                        managed_only.as_deref(),
                    );
                    item.origin.why.push(match (source.managed, &state.trusted_hash) {
                        (true, _) => {
                            "managed source: always trusted, per-hook state ignored (hooks/src/engine/discovery.rs:794-821)"
                                .to_string()
                        }
                        (false, Some((trusted, at))) if *trusted == hash => {
                            format!("trusted: {at} matches the current hash")
                        }
                        _ => format!("current hash {hash}"),
                    });
                    match off {
                        Some((signal, at)) => {
                            item.mode = Mode::None;
                            item.cost = cost_na(None);
                            item.origin.why.push(at);
                            item.signal = Some(signal);
                        }
                        None => {
                            let (mode, why) = classify(event, matcher, handler);
                            item.mode = mode;
                            item.cost = if mode == Mode::Always {
                                Cost::default()
                            } else {
                                cost_na(None)
                            };
                            item.origin.why.extend(why);
                        }
                    }
                    item.knobs = if source.managed {
                        vec![Knob::None {
                            reason: "managed hook: always trusted, no per-hook state".to_string(),
                        }]
                    } else {
                        let mut knobs = vec![state_knob(ctx, &key, &state)];
                        knobs.extend(feature_knobs.iter().cloned());
                        knobs
                    };
                    out.push(item);
                }
            }
        }
    }
    (out, warnings)
}

/// Why a handler does not run (`signal`, the `why` step naming where), in Codex's order:
/// the feature switch drops every handler (`hooks/src/engine/mod.rs:238-261`), then the managed-only
/// requirement skips non-managed sources (`discovery:83-92`), then per-hook state
/// (`discovery:813-821`) and trust (`discovery:794-811`).
fn off_reason(
    managed: bool,
    state: &State,
    hash: &str,
    key: &str,
    features_off: Option<&str>,
    managed_only: Option<&str>,
) -> Option<(String, String)> {
    if let Some(at) = features_off {
        return Some(("off: features.hooks = false".to_string(), at.to_string()));
    }
    if managed {
        return None;
    }
    if let Some(at) = managed_only {
        return Some((
            "off: allow_managed_hooks_only (requirements allow only managed hooks)".to_string(),
            at.to_string(),
        ));
    }
    if let Some((false, at)) = &state.enabled {
        return Some((
            "off: disabled in [hooks.state]".to_string(),
            format!("{at} = false"),
        ));
    }
    match &state.trusted_hash {
        Some((trusted, _)) if trusted == hash => None,
        Some((_, at)) => Some((
            "off: modified since trusted (trusted_hash differs; review it in codex /hooks)"
                .to_string(),
            format!("{at} differs from the current hash {hash}"),
        )),
        None => Some((
            "off: untrusted (no trusted_hash; review it in codex /hooks)".to_string(),
            format!("no [hooks.state] trusted_hash for {key}"),
        )),
    }
}

fn row(
    ctx: &Ctx,
    source: &HookSource,
    event: &str,
    g: usize,
    h: usize,
    matcher: Option<&str>,
    key: &str,
) -> Item {
    let pointer = format!("/hooks/{}/{g}/hooks/{h}", fs::esc(event));
    let setting = format!("{}#{pointer}", source.file.display());
    let label = match matcher.filter(|m| !m.is_empty()) {
        Some(m) => format!("{event} hook [{m}] · {} {g}:{h}", ctx.display(&source.path)),
        None => format!("{event} hook · {} {g}:{h}", ctx.display(&source.path)),
    };
    let mut item = new_item(Kind::Hook, source.layer, key, &label);
    item.origin.path = Some(source.file.clone());
    item.origin.setting = Some(setting.clone());
    item.origin.why = vec![setting, format!("state key {key}")];
    item
}

/// Whether a handler's output reaches the first request: `Always` (cost `?`) or `Runtime`, with
/// the `why` lines saying so. SessionStart fires with source `startup` on a new session
/// (`hooks/src/events/session_start.rs:23-41`); its matcher is tested the way Codex dispatches
/// ([`matcher_matches`]).
pub(crate) fn classify(
    event: &str,
    matcher: Option<&str>,
    handler: &Handler,
) -> (Mode, Vec<String>) {
    let mut why = Vec::new();
    let is_async = matches!(handler, Handler::Command { is_async: true, .. });
    let limit = match handler {
        Handler::Command {
            context_limit: Some(l),
            ..
        } => *l,
        _ => DEFAULT_CONTEXT_LIMIT,
    };
    let injected = match event {
        "SessionStart" if matcher_matches(matcher, "startup") => true,
        "SessionStart" => {
            why.push(format!(
                "matcher {:?} does not match `startup`: fires on resume/clear/compact/fork only",
                matcher.unwrap_or_default()
            ));
            false
        }
        "UserPromptSubmit" => true,
        "SubagentStart" => {
            why.push(
                "SubagentStart fires in a spawned subagent's session, not the main request (core/src/hook_runtime.rs:133-151)"
                    .to_string(),
            );
            false
        }
        _ => false,
    };
    if injected && is_async {
        why.push(
            "async: its output is recorded after the first sampling (core/src/session/turn.rs:175, 553)"
                .to_string(),
        );
    }
    if injected && !is_async {
        let spill = if limit == 0 {
            "additionalContextLimit = 0: never spilled".to_string()
        } else {
            format!("above {limit} tokens it spills to disk with a preview")
        };
        why.push(format!(
            "{event} stdout or additionalContext becomes a developer message (core/src/hook_runtime.rs:849-873); size unknown until a capture; {spill}"
        ));
        (Mode::Always, why)
    } else {
        (Mode::Runtime, why)
    }
}

/// `<source>:<event_snake>:<group>:<handler>` (`hooks/src/lib.rs:112-123`).
pub(crate) fn hook_key(source: &str, snake: &str, group: usize, handler: usize) -> String {
    format!("{source}:{snake}:{group}:{handler}")
}

/// `matcher_pattern_for_event` (`hooks/src/events/common.rs:112-128`): UserPromptSubmit, Stop and
/// Interrupt ignore their matcher.
pub(crate) fn matcher_for_event<'a>(event: &str, matcher: Option<&'a str>) -> Option<&'a str> {
    match event {
        "UserPromptSubmit" | "Stop" | "Interrupt" => None,
        _ => matcher,
    }
}

/// `is_match_all_matcher` / `is_exact_matcher` (`hooks/src/events/common.rs:165-173`).
fn is_match_all(matcher: &str) -> bool {
    matcher.is_empty() || matcher == "*"
}

fn is_exact(matcher: &str) -> bool {
    matcher
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|')
}

/// `validate_matcher_pattern` (`common.rs:130-135`): match-all and exact matchers are valid;
/// anything else must compile as a `regex` pattern.
pub(crate) fn validate_matcher(matcher: &str) -> Result<(), regex::Error> {
    if is_match_all(matcher) || is_exact(matcher) {
        return Ok(());
    }
    regex::Regex::new(matcher).map(|_| ())
}

/// `matches_matcher` (`common.rs:137-150`): no matcher or match-all matches; an exact matcher
/// is a `|`-separated list compared whole; anything else is an unanchored regex search.
pub(crate) fn matcher_matches(matcher: Option<&str>, input: &str) -> bool {
    match matcher {
        None => true,
        Some(m) if is_match_all(m) => true,
        Some(m) if is_exact(m) => m.split('|').any(|c| c == input),
        Some(m) => regex::Regex::new(m).is_ok_and(|re| re.is_match(input)),
    }
}

/// A multi-line error (regex syntax errors point at the column) as one line.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The handler as Codex normalizes it before hashing (`discovery:503-655`); None (with Codex's
/// warning) for a handler it skips.
pub(crate) fn normalize(
    file: &str,
    event: &str,
    handler: &Handler,
    warnings: &mut Vec<String>,
) -> Option<Value> {
    match handler {
        Handler::Command {
            command,
            command_windows,
            timeout,
            is_async,
            status_message,
            context_limit,
        } => {
            let command = match (cfg!(windows), command_windows) {
                (true, Some(w)) => w,
                _ => command,
            };
            if command.trim().is_empty() {
                warnings.push(format!("skipping empty hook command in {file}"));
                return None;
            }
            let mut v = Map::new();
            v.insert("type".to_string(), json!("command"));
            v.insert("command".to_string(), json!(command));
            v.insert("timeout".to_string(), json!(timeout_for(event, *timeout)));
            v.insert("async".to_string(), json!(is_async));
            if let Some(s) = status_message {
                v.insert("statusMessage".to_string(), json!(s));
            }
            if let Some(l) = context_limit
                .filter(|l| CONTEXT_EVENTS.contains(&event) && *l != DEFAULT_CONTEXT_LIMIT)
            {
                v.insert("additionalContextLimit".to_string(), json!(l));
            }
            Some(Value::Object(v))
        }
        Handler::McpTool {
            server,
            tool,
            input,
            timeout,
            status_message,
        } => {
            if event == "SessionEnd" {
                warnings.push(format!(
                    "skipping MCP tool hook in {file}: SessionEnd MCP hooks are not supported"
                ));
                return None;
            }
            if server.trim().is_empty() || tool.trim().is_empty() {
                warnings.push(format!(
                    "skipping MCP tool hook in {file}: server and tool must not be empty"
                ));
                return None;
            }
            let mut v = Map::new();
            v.insert("type".to_string(), json!("mcp_tool"));
            v.insert("server".to_string(), json!(server));
            v.insert("tool".to_string(), json!(tool));
            v.insert("input".to_string(), Value::Object(input.clone()));
            v.insert("timeout".to_string(), json!(timeout_for(event, *timeout)));
            if let Some(s) = status_message {
                v.insert("statusMessage".to_string(), json!(s));
            }
            Some(Value::Object(v))
        }
        Handler::Unsupported(kind) => {
            warnings.push(format!(
                "skipping {kind} hook in {file}: {kind} hooks are not supported yet"
            ));
            None
        }
    }
}

/// `normalize_command_hook` (`discovery:740-764`; SessionEnd constants
/// `hooks/src/events/session_end.rs:20-23`).
fn timeout_for(event: &str, timeout: Option<u64>) -> u64 {
    match event {
        "SessionEnd" | "Interrupt" => timeout.unwrap_or(1).clamp(1, 3),
        _ => timeout.unwrap_or(600).max(1),
    }
}

/// The trust hash of one handler (`discovery:766-792`): `{event_name, matcher?, hooks:
/// [handler]}` serialized to a TOML value — `None` fields dropped (toml 0.9.11
/// `src/table.rs:385-401`) — then to JSON with keys sorted recursively, compact, SHA-256
/// (`config/src/fingerprint.rs:54-84`).
pub(crate) fn hook_hash(snake: &str, matcher: Option<&str>, handler: &Value) -> String {
    let digest = Sha256::digest(identity_bytes(snake, matcher, handler));
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// The bytes [`hook_hash`] digests.
fn identity_bytes(snake: &str, matcher: Option<&str>, handler: &Value) -> Vec<u8> {
    let mut identity = Map::new();
    identity.insert("event_name".to_string(), json!(snake));
    if let Some(m) = matcher {
        identity.insert("matcher".to_string(), json!(m));
    }
    identity.insert("hooks".to_string(), Value::Array(vec![handler.clone()]));
    serde_json::to_vec(&canonical_json(&Value::Object(identity))).expect("JSON serializes")
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical_json(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_json).collect()),
        other => other.clone(),
    }
}

/// `hook_states_from_stack` (`hooks/src/config_rules.rs:15-66`): user, then profile; later
/// layers win field by field; keys trimmed.
fn hook_state(ctx: &Ctx, key: &str) -> State {
    let mut state = State::default();
    for layer in ctx
        .config
        .layers
        .iter()
        .filter(|l| matches!(l.source, Source::User | Source::Profile))
    {
        let Some(Value::Object(states)) = layer.value.pointer("/hooks/state") else {
            continue;
        };
        for (k, v) in states {
            if k.trim() != key || !v.is_object() {
                continue;
            }
            let field = |name: &str| format!("/hooks/state/{}/{name}", fs::esc(k));
            // A state that does not deserialize — a wrong type or a non-finite float — is skipped
            // whole (`config_rules.rs:43-48`); `null` from an overlay means absent.
            let enabled = match layer.lookup(&field("enabled")) {
                Lookup::Absent => None,
                Lookup::Set(Value::Bool(b)) => Some(*b),
                Lookup::Set(_) | Lookup::Unreadable => continue,
            };
            let hash = match layer.lookup(&field("trusted_hash")) {
                Lookup::Absent => None,
                Lookup::Set(Value::String(h)) => Some(h.clone()),
                Lookup::Set(_) | Lookup::Unreadable => continue,
            };
            if let Some(b) = enabled {
                state.enabled = Some((b, layer.at(&field("enabled"))));
                state.enabled_in = layer
                    .path
                    .clone()
                    .map(|file| (file, k.clone(), layer.source == Source::Profile));
            }
            if let Some(h) = hash {
                state.trusted_hash = Some((h, layer.at(&field("trusted_hash"))));
            }
        }
    }
    state
}

/// `[hooks.state."<key>"] enabled = false`, written into the entry that decides `enabled` today
/// (its file and its key as written — Codex trims keys, `config_rules.rs:49-52`, so a padded key
/// in the profile still decides), else into the user config under the canonical key. Project and
/// managed files are not read for hook state (`config_rules.rs:8-14`), so the shared rule's
/// project scope does not apply.
fn state_knob(ctx: &Ctx, key: &str, state: &State) -> Knob {
    let (file, written_key, note) = match &state.enabled_in {
        Some((file, written, true)) => (
            file.clone(),
            written.clone(),
            "this hook only; the profile that sets it",
        ),
        Some((file, written, false)) => (file.clone(), written.clone(), "this hook only"),
        None => (ctx.user_config(), key.to_string(), "this hook only"),
    };
    Knob::Setting {
        file,
        pointer: format!("/hooks/state/{}/enabled", fs::esc(&written_key)),
        value: json!(false),
        scope: Scope::User,
        note: Some(note.to_string()),
    }
}

/// `/etc/codex/requirements.toml` as JSON, when present and valid UTF-8 TOML. A non-finite float
/// reads as `null` and is warned about, as for config layers.
fn read_requirements(ctx: &mut Ctx) -> Option<(PathBuf, Value)> {
    let path = ctx.system_dir.join(REQUIREMENTS);
    let bytes = ctx.fs.read(&path)?;
    let parsed = match String::from_utf8(bytes) {
        Ok(text) => config::parse(&text),
        Err(e) => Err(format!("not UTF-8: {e}")),
    };
    match parsed {
        Ok((value, non_finite)) => {
            for (pointer, literal) in non_finite {
                ctx.warnings.push(format!(
                    "{}#{pointer}: non-finite float `{literal}`; harn reads it as set but unreadable and codex rejects it unless the key takes a float",
                    path.display()
                ));
            }
            Some((path, value))
        }
        Err(err) => {
            ctx.warnings.push(format!(
                "{}: invalid TOML, codex refuses to start with it ({err})",
                path.display()
            ));
            None
        }
    }
}

/// `<at> is a non-finite float harn cannot read` or `<at> = <value> is not <kind>`.
pub(crate) fn not_a(at: &str, value: &Value, kind: &str) -> String {
    match value {
        Value::Null => format!("{at} is a non-finite float harn cannot read"),
        other => format!("{at} = {other} is not {kind}"),
    }
}

fn sources(ctx: &mut Ctx, requirements: Option<&(PathBuf, Value)>) -> Vec<HookSource> {
    let mut out = Vec::new();
    if let Some((file, value)) = requirements {
        if let Some(hooks) = value.get("hooks") {
            match parse_events(hooks, true) {
                Ok(events) if handler_count(&events) > 0 => {
                    let managed_dir = hooks
                        .get(if cfg!(windows) {
                            "windows_managed_dir"
                        } else {
                            "managed_dir"
                        })
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                        .filter(|p| p.is_absolute());
                    out.push(HookSource {
                        path: managed_dir.unwrap_or_else(|| file.clone()),
                        file: file.clone(),
                        layer: Layer::Managed,
                        managed: true,
                        required: true,
                        events,
                    });
                }
                Ok(_) => {}
                Err(err) => ctx.warnings.push(format!(
                    "{}#/hooks: {err}; codex refuses to start with it",
                    file.display()
                )),
            }
        }
        for key in ["rules", "mcp_servers"] {
            if value.get(key).is_some() {
                ctx.warnings.push(format!(
                    "{}#/{key}: requirements {key} are enforced by codex and not modelled by harn",
                    file.display()
                ));
            }
        }
    }

    let worktree = worktree_roots(ctx);
    let mut visited: Vec<PathBuf> = Vec::new();
    // The system and user layers exist even without a config.toml.
    let system = ctx.system_dir.clone();
    let home = ctx.codex_home.clone();
    let mut plan: Vec<(Source, Option<PathBuf>, Option<usize>)> = Vec::new();
    plan.push((Source::System, Some(system), index_of(ctx, Source::System)));
    plan.push((
        Source::User,
        Some(home.clone()),
        index_of(ctx, Source::User),
    ));
    if let Some(i) = index_of(ctx, Source::Profile) {
        plan.push((Source::Profile, Some(home), Some(i)));
    }
    for (i, layer) in ctx.config.layers.iter().enumerate() {
        if layer.source == Source::Project && layer.enabled() {
            let folder = layer
                .path
                .as_ref()
                .and_then(|p| p.parent())
                .map(Path::to_path_buf);
            plan.push((Source::Project, folder, Some(i)));
        }
    }
    plan.push((Source::Managed, None, index_of(ctx, Source::Managed)));

    for (kind, folder, index) in plan {
        let managed = matches!(kind, Source::System | Source::Managed);
        let layer_kind = match kind {
            Source::System | Source::Managed => Layer::Managed,
            Source::Project => Layer::Project,
            _ => Layer::User,
        };
        // A linked worktree's project layer takes its hooks from the main checkout.
        let hooks_folder = match (kind, &folder, &worktree) {
            (Source::Project, Some(f), Some((checkout, main))) => f
                .parent()
                .and_then(|d| d.strip_prefix(checkout).ok())
                .map(|rel| main.join(rel).join(".codex"))
                .or_else(|| Some(f.clone())),
            _ => folder.clone(),
        };
        let json_hooks = match &hooks_folder {
            Some(f) if !visited.contains(f) => {
                visited.push(f.clone());
                load_hooks_json(ctx, &f.join("hooks.json"))
            }
            _ => None,
        };
        let toml_hooks = match (kind, &hooks_folder, &folder) {
            (Source::Project, Some(hf), Some(f)) if hf != f => {
                let file = hf.join("config.toml");
                let parsed = ctx
                    .fs
                    .read(&file)
                    .map(|bytes| match String::from_utf8(bytes) {
                        Ok(text) => config::parse(&text),
                        Err(e) => Err(format!("not UTF-8: {e}")),
                    });
                match parsed {
                    Some(Ok((value, _))) => value
                        .get("hooks")
                        .cloned()
                        .and_then(|t| load_toml_hooks(ctx, &file, &t)),
                    Some(Err(err)) => {
                        ctx.warnings.push(format!(
                            "{}: invalid TOML in the main checkout's project config, codex refuses to start with it ({err}) (config/src/loader/mod.rs:1815-1826)",
                            file.display()
                        ));
                        None
                    }
                    None => None,
                }
            }
            _ => index.and_then(|i| {
                let layer: &LayerFile = &ctx.config.layers[i];
                let file = layer.path.clone()?;
                let table = match layer.lookup("/hooks") {
                    Lookup::Absent => return None,
                    Lookup::Set(table) => table.clone(),
                    // `hooks = nan`: not a table (the parser says so).
                    Lookup::Unreadable => Value::Null,
                };
                load_toml_hooks(ctx, &file, &table)
            }),
        };
        if let (Some((json_path, _)), Some((toml_path, _))) = (&json_hooks, &toml_hooks) {
            ctx.warnings.push(format!(
                "loading hooks from both {} and {}; prefer a single representation for this layer",
                json_path.display(),
                toml_path.display()
            ));
        }
        for (path, events) in [json_hooks, toml_hooks].into_iter().flatten() {
            out.push(HookSource {
                file: path.clone(),
                path,
                layer: layer_kind,
                managed,
                required: false,
                events,
            });
        }
    }
    out
}

fn index_of(ctx: &Ctx, source: Source) -> Option<usize> {
    ctx.config.layers.iter().position(|l| l.source == source)
}

/// `(checkout root, main checkout)` when the target is in a linked worktree
/// (`loader:1106-1116`; the checkout is the nearest ancestor with a `.git` file or a `.git` dir
/// holding `HEAD`, `loader:1529-1566`).
fn worktree_roots(ctx: &Ctx) -> Option<(PathBuf, PathBuf)> {
    let main = ctx.config.repo_root.clone()?;
    let checkout = ctx.dir().ancestors().find(|a| {
        let dot_git = a.join(".git");
        ctx.fs.is_file(&dot_git) || ctx.fs.is_file(&dot_git.join("HEAD"))
    })?;
    (checkout != main).then(|| (checkout.to_path_buf(), main))
}

/// `load_hooks_json` (`discovery:339-380`): `{"description"?, "hooks": {...}}` with no other key.
fn load_hooks_json(ctx: &mut Ctx, path: &Path) -> Option<(PathBuf, Events)> {
    let parsed = match ctx.fs.read_json(path) {
        Json::Missing => return None,
        Json::Invalid(err) => Err(err),
        Json::Ok(value) => parse_hooks_file(&value),
    };
    match parsed {
        Ok(events) if !events.is_empty() => Some((path.to_path_buf(), events)),
        Ok(_) => None,
        Err(err) => {
            ctx.warnings.push(format!(
                "failed to parse hooks config {}: {err}",
                path.display()
            ));
            None
        }
    }
}

/// `load_toml_hooks_from_layer` (`discovery:382-400`).
///
/// TOML mode (`null` = a non-finite float) holds for the whole table: overlay patches come only
/// from this resolver's knobs (`plan.rs` `overlay_op`; plans pick knobs by index), and no knob
/// writes under `/hooks/<Event>` or writes `null` (pinned by the test
/// `no_knob_writes_into_hook_declarations_or_writes_null`; the hook knob writes `false` under
/// `/hooks/state`, which [`hook_state`] reads through `LayerFile::lookup`). A knob that ever
/// patches a hook declaration must switch this to per-field provenance, as `mcp.rs` `readable`
/// does.
fn load_toml_hooks(ctx: &mut Ctx, file: &Path, table: &Value) -> Option<(PathBuf, Events)> {
    match parse_events(table, true) {
        Ok(events) if !events.is_empty() => Some((file.to_path_buf(), events)),
        Ok(_) => None,
        Err(err) => {
            ctx.warnings.push(format!(
                "failed to parse TOML hooks in {}: {err}",
                file.display()
            ));
            None
        }
    }
}

fn handler_count(events: &Events) -> usize {
    events
        .iter()
        .flat_map(|(_, groups)| groups)
        .map(|g| g.handlers.len())
        .sum()
}

/// `HooksFile` (`config/src/hook_config.rs:10-17`, `deny_unknown_fields`).
pub(crate) fn parse_hooks_file(value: &Value) -> Result<Events, String> {
    let Value::Object(top) = value else {
        return Err(format!(
            "expected an object, found {}",
            type_name(value, false)
        ));
    };
    for key in top.keys() {
        if key != "description" && key != "hooks" {
            return Err(format!(
                "unknown field `{key}`, expected `description` or `hooks`"
            ));
        }
    }
    opt_string(top, "description", false)?;
    match top.get("hooks") {
        None => Ok(Vec::new()),
        Some(hooks) => parse_events(hooks, false),
    }
}

/// `HookEventsToml` (`hook_config.rs:35-61`): known event keys only (others are ignored), each a
/// list of matcher groups. Empty when no event has a group (`HookEventsToml::is_empty`).
/// `toml`: the value is a TOML document viewed as JSON, where `null` can only be a non-finite
/// float — a type error for every typed field (JSON's `null` is `None` for optional fields).
pub(crate) fn parse_events(value: &Value, toml: bool) -> Result<Events, String> {
    let Value::Object(map) = value else {
        return Err(format!(
            "expected a table, found {}",
            type_name(value, toml)
        ));
    };
    let mut out = Vec::new();
    for (i, (event, _)) in EVENTS.iter().enumerate() {
        let Some(groups) = map.get(*event) else {
            continue;
        };
        let Value::Array(groups) = groups else {
            return Err(format!(
                "`{event}`: expected a list of matcher groups, found {}",
                type_name(groups, toml)
            ));
        };
        let groups = groups
            .iter()
            .map(|g| parse_group(g, toml).map_err(|e| format!("`{event}`: {e}")))
            .collect::<Result<Vec<Group>, String>>()?;
        if !groups.is_empty() {
            out.push((i, groups));
        }
    }
    Ok(out)
}

/// `MatcherGroup` (`hook_config.rs:153-159`).
fn parse_group(value: &Value, toml: bool) -> Result<Group, String> {
    let Value::Object(map) = value else {
        return Err(format!(
            "expected a matcher group, found {}",
            type_name(value, toml)
        ));
    };
    let matcher = opt_string(map, "matcher", toml)?;
    let handlers = match map.get("hooks") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|h| parse_handler(h, toml))
            .collect::<Result<Vec<Handler>, String>>()?,
        Some(other) => {
            return Err(format!(
                "`hooks`: expected a list, found {}",
                type_name(other, toml)
            ))
        }
    };
    Ok(Group { matcher, handlers })
}

/// `HookHandlerConfig` (`hook_config.rs:161-201`), tagged by `type`.
fn parse_handler(value: &Value, toml: bool) -> Result<Handler, String> {
    let Value::Object(map) = value else {
        return Err(format!(
            "expected a hook handler, found {}",
            type_name(value, toml)
        ));
    };
    let kind = match map.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(other) => {
            return Err(format!(
                "`type`: expected a string, found {}",
                type_name(other, toml)
            ))
        }
        None => return Err("missing field `type`".to_string()),
    };
    match kind {
        "command" => Ok(Handler::Command {
            command: req_string(map, "command", toml)?,
            command_windows: match opt_string(map, "commandWindows", toml)? {
                Some(w) => Some(w),
                None => opt_string(map, "command_windows", toml)?,
            },
            timeout: opt_u64(map, "timeout", toml)?,
            is_async: match map.get("async") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(other) => {
                    return Err(format!(
                        "`async`: expected a boolean, found {}",
                        type_name(other, toml)
                    ))
                }
            },
            status_message: opt_string(map, "statusMessage", toml)?,
            context_limit: opt_u64(map, "additionalContextLimit", toml)?,
        }),
        "mcp_tool" => Ok(Handler::McpTool {
            server: req_string(map, "server", toml)?,
            tool: req_string(map, "tool", toml)?,
            input: match map.get("input") {
                None => Map::new(),
                Some(Value::Object(input)) => {
                    // JSON `null` cannot be TOML (`hook_config.rs:203-217`); in a TOML document a
                    // `null` here is a non-finite float, which TOML holds and the hash reads as
                    // JSON `null` (serde_json has no non-finite number).
                    if !toml && contains_null(&Value::Object(input.clone())) {
                        return Err(
                            "MCP hook input must be representable as TOML (no null)".to_string()
                        );
                    }
                    input.clone()
                }
                Some(other) => {
                    return Err(format!(
                        "`input`: expected a map, found {}",
                        type_name(other, toml)
                    ))
                }
            },
            timeout: opt_u64(map, "timeout", toml)?,
            status_message: opt_string(map, "statusMessage", toml)?,
        }),
        "prompt" => Ok(Handler::Unsupported("prompt")),
        "agent" => Ok(Handler::Unsupported("agent")),
        other => Err(format!(
            "unknown variant `{other}`, expected one of `command`, `mcp_tool`, `prompt`, `agent`"
        )),
    }
}

fn contains_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.iter().any(contains_null),
        Value::Object(map) => map.values().any(contains_null),
        _ => false,
    }
}

fn req_string(map: &Map<String, Value>, key: &str, toml: bool) -> Result<String, String> {
    opt_string(map, key, toml)?.ok_or_else(|| format!("missing field `{key}`"))
}

/// An optional field: JSON `null` is `None`; in TOML mode `null` is a non-finite float.
fn optional<'a>(map: &'a Map<String, Value>, key: &str, toml: bool) -> Option<&'a Value> {
    map.get(key).filter(|v| toml || !v.is_null())
}

fn opt_string(map: &Map<String, Value>, key: &str, toml: bool) -> Result<Option<String>, String> {
    match optional(map, key, toml) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(format!(
            "`{key}`: expected a string, found {}",
            type_name(other, toml)
        )),
    }
}

fn opt_u64(map: &Map<String, Value>, key: &str, toml: bool) -> Result<Option<u64>, String> {
    match optional(map, key, toml) {
        None => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            format!(
                "`{key}`: expected a non-negative integer, found {}",
                type_name(v, toml)
            )
        }),
    }
}

fn type_name(value: &Value, toml: bool) -> &'static str {
    match value {
        Value::Null if toml => "a non-finite float",
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "a table",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{walk, Overlay, OverlayOp, Target, Tree};
    use crate::resolve::codex::resolve_with;
    use std::collections::BTreeMap;
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
            "harn-codex-hooks-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tmp(std::fs::canonicalize(&root).unwrap())
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// `root/repo` (a git checkout) with `root/home/.codex` as the Codex home.
    fn setup(root: &Path) -> Target {
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        std::fs::create_dir_all(root.join("home/.codex")).unwrap();
        target_at(root, &root.join("repo"))
    }

    fn target_at(root: &Path, dir: &Path) -> Target {
        Target {
            dir: dir.to_path_buf(),
            home: root.join("home"),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        }
    }

    fn resolve(root: &Path, target: &Target, overlay: Option<&Overlay>) -> Tree {
        resolve_with(target, overlay, &root.join("etc/codex")).unwrap()
    }

    fn hooks(tree: &Tree) -> Vec<&Item> {
        walk(tree).filter(|i| i.kind == Kind::Hook).collect()
    }

    fn hook<'t>(tree: &'t Tree, key: &str) -> &'t Item {
        walk(tree)
            .find(|i| i.kind == Kind::Hook && i.id.ends_with(key))
            .unwrap_or_else(|| {
                let ids: Vec<&str> = hooks(tree).iter().map(|i| i.id.as_str()).collect();
                panic!("no hook {key}: {ids:#?}")
            })
    }

    /// The hash of `{"type":"command","command":<cmd>}` with defaults, for `[hooks.state]`.
    fn command_hash(snake: &str, matcher: Option<&str>, cmd: &str, timeout: u64) -> String {
        hook_hash(
            snake,
            matcher,
            &json!({"type": "command", "command": cmd, "timeout": timeout, "async": false}),
        )
    }

    fn state(key: &str, enabled: Option<bool>, hash: Option<&str>) -> String {
        let mut out = format!("[hooks.state.{}]\n", serde_json::to_string(key).unwrap());
        if let Some(e) = enabled {
            out.push_str(&format!("enabled = {e}\n"));
        }
        if let Some(h) = hash {
            out.push_str(&format!("trusted_hash = \"{h}\"\n"));
        }
        out
    }

    fn trust_project(root: &Path) -> String {
        format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            root.join("repo").display()
        )
    }

    const HOOKS_JSON: &str = r#"{"hooks": {
        "SessionStart": [{"matcher": "startup|resume", "hooks": [{"type": "command", "command": "echo start"}]}],
        "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo pre", "timeout": 30}]}]
    }}"#;

    #[test]
    fn hook_hash_digests_the_canonical_identity() {
        let handler =
            json!({"type": "command", "command": "echo hi", "timeout": 600, "async": false});
        assert_eq!(
            String::from_utf8(identity_bytes("session_start", Some("startup"), &handler)).unwrap(),
            r#"{"event_name":"session_start","hooks":[{"async":false,"command":"echo hi","timeout":600,"type":"command"}],"matcher":"startup"}"#
        );
        // `printf '%s' '<the line above>' | shasum -a 256`
        assert_eq!(
            hook_hash("session_start", Some("startup"), &handler),
            "sha256:96fce91e330a75049046e8e6e6ee74528be2d90871e21bafdff0b41859318628"
        );
        let handler = json!({"type": "command", "command": "x", "timeout": 1, "async": true,
            "statusMessage": "s", "additionalContextLimit": 100});
        assert_eq!(
            hook_hash("user_prompt_submit", None, &handler),
            "sha256:596ded5f9ce5c23e08f6404acf81914fb7cf4a45eefae5e94508c6c6e01b492e"
        );
    }

    #[test]
    fn normalization_fills_timeouts_and_drops_the_default_context_limit() {
        assert_eq!(timeout_for("PreToolUse", None), 600);
        assert_eq!(timeout_for("PreToolUse", Some(0)), 1);
        assert_eq!(timeout_for("SessionEnd", None), 1);
        assert_eq!(timeout_for("Interrupt", Some(10)), 3);
        let parsed = parse_handler(
            &json!({"type": "command", "command": "c",
            "additionalContextLimit": 2500, "timeout": 5}),
            false,
        )
        .unwrap();
        // The default limit and a limit on an event without context are left out of the hash.
        let mut w = Vec::new();
        let v = normalize("f", "SessionStart", &parsed, &mut w).unwrap();
        assert_eq!(
            v,
            json!({"type": "command", "command": "c", "timeout": 5, "async": false})
        );
        let limited = parse_handler(
            &json!({"type": "command", "command": "c",
            "additionalContextLimit": 10}),
            false,
        )
        .unwrap();
        let v = normalize("f", "Stop", &limited, &mut w).unwrap();
        assert!(v.get("additionalContextLimit").is_none(), "{v}");
        let v = normalize("f", "SessionStart", &limited, &mut w).unwrap();
        assert_eq!(v["additionalContextLimit"], json!(10));
    }

    #[test]
    fn hooks_json_and_inline_hooks_both_load_with_a_warning() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(&codex.join("hooks.json"), HOOKS_JSON);
        write(
            &codex.join("config.toml"),
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"echo stop\"\n",
        );
        let tree = resolve(root, &target, None);
        let keys: Vec<&str> = hooks(&tree).iter().map(|i| i.id.as_str()).collect();
        let json_key = format!("{}:session_start:0:0", codex.join("hooks.json").display());
        let toml_key = format!("{}:stop:0:0", codex.join("config.toml").display());
        // Codex's order: the layer's hooks.json (events in declaration order), then its [hooks].
        assert_eq!(
            keys,
            [
                format!(
                    "codex/hook/user/{}:pre_tool_use:0:0",
                    codex.join("hooks.json").display()
                ),
                format!("codex/hook/user/{json_key}"),
                format!("codex/hook/user/{toml_key}"),
            ]
        );
        assert!(
            tree.warnings
                .iter()
                .any(|w| w.starts_with("loading hooks from both") && w.contains("config.toml")),
            "{:?}",
            tree.warnings
        );
        // Untrusted until a trusted_hash is recorded; the knob is the per-hook state.
        let row = hook(&tree, &json_key);
        assert_eq!(row.mode, Mode::None);
        assert!(row.signal.as_deref().unwrap().starts_with("off: untrusted"));
        match &row.knobs[0] {
            Knob::Setting {
                file,
                pointer,
                value,
                scope,
                ..
            } => {
                assert_eq!(file, &codex.join("config.toml"));
                assert_eq!(
                    pointer,
                    &format!("/hooks/state/{}/enabled", fs::esc(&json_key))
                );
                assert_eq!(value, &json!(false));
                assert_eq!(*scope, Scope::User);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            &row.knobs[1],
            Knob::Setting { pointer, .. } if pointer == "/features/hooks"
        ));
    }

    #[test]
    fn trusted_session_start_is_always_and_other_events_are_runtime() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        let json_file = codex.join("hooks.json");
        write(
            &json_file,
            r#"{"hooks": {
            "SessionStart": [
                {"matcher": "startup|resume", "hooks": [{"type": "command", "command": "a"}]},
                {"matcher": "resume", "hooks": [{"type": "command", "command": "b"}]},
                {"hooks": [{"type": "command", "command": "c", "async": true}]}
            ],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "d"}]}],
            "UserPromptSubmit": [{"matcher": "ignored", "hooks": [{"type": "command", "command": "e"}]}],
            "SubagentStart": [{"hooks": [{"type": "command", "command": "f"}]}]
        }}"#,
        );
        let k = |rest: &str| format!("{}:{rest}", json_file.display());
        let async_hash = hook_hash(
            "session_start",
            None,
            &json!({"type": "command", "command": "c", "timeout": 600, "async": true}),
        );
        let config = [
            state(
                &k("session_start:0:0"),
                None,
                Some(&command_hash(
                    "session_start",
                    Some("startup|resume"),
                    "a",
                    600,
                )),
            ),
            state(
                &k("session_start:1:0"),
                None,
                Some(&command_hash("session_start", Some("resume"), "b", 600)),
            ),
            state(&k("session_start:2:0"), None, Some(&async_hash)),
            state(
                &k("pre_tool_use:0:0"),
                Some(true),
                Some(&command_hash("pre_tool_use", Some("Bash"), "d", 600)),
            ),
            // UserPromptSubmit ignores its matcher in the hash too.
            state(
                &k("user_prompt_submit:0:0"),
                None,
                Some(&command_hash("user_prompt_submit", None, "e", 600)),
            ),
            state(
                &k("subagent_start:0:0"),
                None,
                Some(&command_hash("subagent_start", None, "f", 600)),
            ),
        ]
        .concat();
        write(&codex.join("config.toml"), &config);
        let tree = resolve(root, &target, None);
        let start = hook(&tree, &k("session_start:0:0"));
        assert_eq!((start.mode, start.signal.as_deref()), (Mode::Always, None));
        assert_eq!(start.cost, Cost::default(), "cost `?` until a capture");
        assert!(start.origin.why.iter().any(|w| w.starts_with("trusted:")));
        assert_eq!(hook(&tree, &k("session_start:1:0")).mode, Mode::Runtime);
        assert_eq!(
            hook(&tree, &k("session_start:2:0")).mode,
            Mode::Runtime,
            "async"
        );
        let pre = hook(&tree, &k("pre_tool_use:0:0"));
        assert_eq!((pre.mode, pre.signal.as_deref()), (Mode::Runtime, None));
        assert!(pre.cost.not_applicable);
        assert_eq!(hook(&tree, &k("user_prompt_submit:0:0")).mode, Mode::Always);
        assert_eq!(hook(&tree, &k("subagent_start:0:0")).mode, Mode::Runtime);
        // Two always rows of unknown size.
        assert_eq!(
            hooks(&tree)
                .iter()
                .filter(|i| i.mode == Mode::Always)
                .count(),
            2
        );
    }

    #[test]
    fn state_disabled_modified_and_untrusted_hooks_are_off() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        let json_file = codex.join("hooks.json");
        write(&json_file, HOOKS_JSON);
        let start = format!("{}:session_start:0:0", json_file.display());
        let pre = format!("{}:pre_tool_use:0:0", json_file.display());
        let good = command_hash("session_start", Some("startup|resume"), "echo start", 600);
        write(
            &codex.join("config.toml"),
            &[
                state(&start, Some(false), Some(&good)),
                state(&pre, None, Some("sha256:stale")),
            ]
            .concat(),
        );
        let tree = resolve(root, &target, None);
        let row = hook(&tree, &start);
        assert_eq!(row.mode, Mode::None);
        assert_eq!(
            row.signal.as_deref(),
            Some("off: disabled in [hooks.state]")
        );
        assert!(row
            .origin
            .why
            .iter()
            .any(|w| w.ends_with("/enabled = false")));
        let row = hook(&tree, &pre);
        assert_eq!(row.mode, Mode::None);
        assert!(row.signal.as_deref().unwrap().starts_with("off: modified"));
        assert!(row.cost.not_applicable);
    }

    #[test]
    fn overlay_on_the_state_knob_turns_the_hook_off() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        let json_file = codex.join("hooks.json");
        write(&json_file, HOOKS_JSON);
        let start = format!("{}:session_start:0:0", json_file.display());
        let good = command_hash("session_start", Some("startup|resume"), "echo start", 600);
        write(
            &codex.join("config.toml"),
            &state(&start, None, Some(&good)),
        );
        let tree = resolve(root, &target, None);
        let row = hook(&tree, &start);
        assert_eq!(row.mode, Mode::Always);
        let before = tree.unknown_rows;
        let Knob::Setting {
            file,
            pointer,
            value,
            ..
        } = row.knobs[0].clone()
        else {
            panic!("{:?}", row.knobs)
        };
        let overlay = Overlay {
            ops: vec![OverlayOp::SetJson {
                file,
                pointer,
                value,
            }],
        };
        let after = resolve(root, &target, Some(&overlay));
        assert_eq!(hook(&after, &start).mode, Mode::None);
        assert_eq!(after.unknown_rows + 1, before);
    }

    #[test]
    fn project_hooks_load_only_while_the_project_is_trusted() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(&root.join("repo/.codex/hooks.json"), HOOKS_JSON);
        write(&codex.join("config.toml"), "");
        assert!(hooks(&resolve(root, &target, None)).is_empty());

        write(&codex.join("config.toml"), &trust_project(root));
        let tree = resolve(root, &target, None);
        let rows = hooks(&tree);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.origin.layer == Layer::Project));
        let key = format!(
            "{}:session_start:0:0",
            root.join("repo/.codex/hooks.json").display()
        );
        // Hook state still lives in the user file only.
        assert!(matches!(
            &hook(&tree, &key).knobs[0],
            Knob::Setting { file, scope: Scope::User, .. } if *file == codex.join("config.toml")
        ));
    }

    #[test]
    fn allow_managed_hooks_only_keeps_managed_hooks() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(&codex.join("hooks.json"), HOOKS_JSON);
        write(
            &root.join("etc/codex/requirements.toml"),
            "allow_managed_hooks_only = true\n[hooks]\nmanaged_dir = \"/opt/corp/hooks\"\n[[hooks.SessionStart]]\n[[hooks.SessionStart.hooks]]\ntype = \"command\"\ncommand = \"corp\"\n",
        );
        let tree = resolve(root, &target, None);
        let managed = hook(&tree, "/opt/corp/hooks:session_start:0:0");
        assert_eq!(managed.origin.layer, Layer::Managed);
        assert_eq!(
            (managed.mode, managed.signal.as_deref()),
            (Mode::Always, None)
        );
        assert!(matches!(managed.knobs.as_slice(), [Knob::None { .. }]));
        let user = hook(
            &tree,
            &format!("{}:session_start:0:0", codex.join("hooks.json").display()),
        );
        assert_eq!(user.mode, Mode::None);
        assert!(user
            .signal
            .as_deref()
            .unwrap()
            .starts_with("off: allow_managed_hooks_only"));
        assert!(user
            .origin
            .why
            .iter()
            .any(|w| w.ends_with("requirements.toml#/allow_managed_hooks_only = true")));

        // The system layer's folder is managed too.
        write(&root.join("etc/codex/hooks.json"), HOOKS_JSON);
        let tree = resolve(root, &target, None);
        let system = hook(
            &tree,
            &format!(
                "{}:pre_tool_use:0:0",
                root.join("etc/codex/hooks.json").display()
            ),
        );
        assert_eq!(
            (system.mode, system.origin.layer),
            (Mode::Runtime, Layer::Managed)
        );
    }

    #[test]
    fn features_hooks_false_turns_every_hook_off() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(&codex.join("hooks.json"), HOOKS_JSON);
        write(&codex.join("config.toml"), "[features]\nhooks = false\n");
        let tree = resolve(root, &target, None);
        let rows = hooks(&tree);
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row.mode, Mode::None);
            assert_eq!(row.signal.as_deref(), Some("off: features.hooks = false"));
        }
    }

    #[test]
    fn indices_count_skipped_handlers_and_bad_files_warn() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(
            &codex.join("hooks.json"),
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "prompt"}, {"type": "command", "command": "s"}]}]}}"#,
        );
        let tree = resolve(root, &target, None);
        let keys: Vec<&str> = hooks(&tree).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(keys.len(), 1);
        assert!(keys[0].ends_with(":stop:0:1"), "{keys:?}");
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.contains("prompt hooks are not supported")));

        write(&codex.join("hooks.json"), r#"{"hooks": {}, "extra": 1}"#);
        let tree = resolve(root, &target, None);
        assert!(hooks(&tree).is_empty());
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.starts_with("failed to parse hooks config") && w.contains("`extra`")));
        write(
            &codex.join("hooks.json"),
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "shell"}]}]}}"#,
        );
        let tree = resolve(root, &target, None);
        assert!(hooks(&tree).is_empty());
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.contains("unknown variant `shell`")));
    }

    #[test]
    fn linked_worktree_takes_project_hooks_from_the_main_checkout() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("main/.git/HEAD"), "ref: refs/heads/main\n");
        let wt_git = root.join("main/.git/worktrees/wt");
        write(&wt_git.join("HEAD"), "ref: refs/heads/wt\n");
        write(&wt_git.join("commondir"), "../..\n");
        let wt = root.join("main/.worktrees/wt");
        write(&wt.join(".git"), &format!("gitdir: {}\n", wt_git.display()));
        // The worktree's own hooks are replaced by the main checkout's.
        write(&wt.join(".codex/hooks.json"), HOOKS_JSON);
        write(
            &root.join("main/.codex/hooks.json"),
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "main"}]}]}}"#,
        );
        write(
            &root.join("home/.codex/config.toml"),
            &format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                root.join("main").display()
            ),
        );
        let target = target_at(root, &wt);
        let tree = resolve(root, &target, None);
        let keys: Vec<&str> = hooks(&tree).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            keys,
            [format!(
                "codex/hook/project/{}:stop:0:0",
                root.join("main/.codex/hooks.json").display()
            )]
        );
    }

    /// Applies `knob` (a setting) as an overlay and resolves again.
    fn apply(root: &Path, target: &Target, knob: &Knob) -> Tree {
        let Knob::Setting {
            file,
            pointer,
            value,
            ..
        } = knob.clone()
        else {
            panic!("{knob:?}")
        };
        let overlay = Overlay {
            ops: vec![OverlayOp::SetJson {
                file,
                pointer,
                value,
            }],
        };
        resolve(root, target, Some(&overlay))
    }

    #[test]
    fn a_padded_state_key_in_the_profile_gets_the_knob() {
        let t = tmp();
        let root = &t.0;
        let mut target = setup(root);
        target.env.insert(
            crate::resolve::codex::PROFILE_ENV.to_string(),
            "work".to_string(),
        );
        let codex = root.join("home/.codex");
        let json_file = codex.join("hooks.json");
        write(&json_file, HOOKS_JSON);
        let key = format!("{}:session_start:0:0", json_file.display());
        let good = command_hash("session_start", Some("startup|resume"), "echo start", 600);
        write(&codex.join("config.toml"), "");
        // Codex trims state keys (`hooks/src/config_rules.rs:49-52`).
        write(
            &codex.join("work.config.toml"),
            &state(&format!("  {key} "), Some(true), Some(&good)),
        );
        let tree = resolve(root, &target, None);
        let row = hook(&tree, &key);
        assert_eq!(row.mode, Mode::Always);
        assert!(matches!(
            &row.knobs[0],
            Knob::Setting { file, .. } if *file == codex.join("work.config.toml")
        ));
        let after = apply(root, &target, &row.knobs[0]);
        assert_eq!(
            hook(&after, &key).mode,
            Mode::None,
            "the knob must take effect"
        );
    }

    #[test]
    fn a_broken_main_checkout_config_warns_and_resolving_goes_on() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("main/.git/HEAD"), "ref: refs/heads/main\n");
        let wt_git = root.join("main/.git/worktrees/wt");
        write(&wt_git.join("HEAD"), "ref: refs/heads/wt\n");
        write(&wt_git.join("commondir"), "../..\n");
        let wt = root.join("main/.worktrees/wt");
        write(&wt.join(".git"), &format!("gitdir: {}\n", wt_git.display()));
        std::fs::create_dir_all(wt.join(".codex")).unwrap();
        write(&root.join("main/.codex/hooks.json"), HOOKS_JSON);
        write(&root.join("main/.codex/config.toml"), "x = \n");
        write(
            &root.join("home/.codex/config.toml"),
            &format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                root.join("main").display()
            ),
        );
        let tree = resolve(root, &target_at(root, &wt), None);
        let file = root.join("main/.codex/config.toml").display().to_string();
        assert!(
            tree.warnings
                .iter()
                .any(|w| w.starts_with(&file) && w.contains("invalid TOML")),
            "{:?}",
            tree.warnings
        );
        assert_eq!(hooks(&tree).len(), 2, "hooks.json still listed");
    }

    #[test]
    fn session_start_matchers_follow_codex_regex_semantics() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        let json_file = codex.join("hooks.json");
        let matchers = ["^resume$", "startup|resume", ".*", "st.rt", "(", "*"];
        let groups: Vec<Value> = matchers
            .iter()
            .map(|m| json!({"matcher": m, "hooks": [{"type": "command", "command": "c"}]}))
            .collect();
        write(
            &json_file,
            &json!({"hooks": {"SessionStart": groups}}).to_string(),
        );
        let k = |g: usize| format!("{}:session_start:{g}:0", json_file.display());
        let config: String = matchers
            .iter()
            .enumerate()
            .map(|(g, m)| {
                state(
                    &k(g),
                    None,
                    Some(&command_hash("session_start", Some(m), "c", 600)),
                )
            })
            .collect();
        write(&codex.join("config.toml"), &config);
        let tree = resolve(root, &target, None);
        assert_eq!(hook(&tree, &k(0)).mode, Mode::Runtime, "^resume$");
        assert_eq!(hook(&tree, &k(1)).mode, Mode::Always, "exact list");
        assert_eq!(hook(&tree, &k(2)).mode, Mode::Always, ".*");
        assert_eq!(hook(&tree, &k(3)).mode, Mode::Always, "unanchored regex");
        // An invalid regex: Codex skips the group with a warning; the next keeps its index.
        assert!(walk(&tree).all(|i| !i.id.ends_with(&k(4))));
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.starts_with("invalid matcher \"(\" in")));
        assert_eq!(hook(&tree, &k(5)).mode, Mode::Always, "*");
    }

    /// The equivalences the pinned upstream tests assert on `current_hash`
    /// (`hooks/src/engine/discovery.rs:1174-1219`), and the TOML/JSON convergence the hash is
    /// built for (`discovery.rs:766-767`).
    #[test]
    fn hash_invariants_of_the_pinned_upstream_tests() {
        let hash = |event: &str, limit: Option<u64>| {
            let mut handler = json!({"type": "command", "command": "echo hi"});
            if let Some(l) = limit {
                handler["additionalContextLimit"] = json!(l);
            }
            let snake = EVENTS.iter().find(|(e, _)| *e == event).unwrap().1;
            let normalized = normalize(
                "f",
                event,
                &parse_handler(&handler, false).unwrap(),
                &mut Vec::new(),
            )
            .unwrap();
            hook_hash(snake, None, &normalized)
        };
        for event in CONTEXT_EVENTS {
            assert_eq!(hash(event, None), hash(event, Some(2_500)), "{event}");
            assert_ne!(hash(event, None), hash(event, Some(20_000)), "{event}");
            assert_ne!(hash(event, None), hash(event, Some(0)), "{event}");
        }
        // `Stop` cannot emit context: the limit is ignored.
        assert_eq!(hash("Stop", None), hash("Stop", Some(20_000)));

        // The same hook from hooks.json and from inline TOML hashes the same.
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(
            &codex.join("hooks.json"),
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "s", "timeout": 5}]}]}}"#,
        );
        write(
            &codex.join("config.toml"),
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"s\"\ntimeout = 5\n",
        );
        let tree = resolve(root, &target, None);
        let current: Vec<&String> = hooks(&tree)
            .iter()
            .map(|i| {
                i.origin
                    .why
                    .iter()
                    .find(|w| w.starts_with("current hash"))
                    .unwrap()
            })
            .collect();
        assert_eq!(current.len(), 2);
        assert_eq!(current[0], current[1]);
    }

    #[test]
    fn wrong_typed_or_non_finite_values_are_set_and_warned() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        let json_file = codex.join("hooks.json");
        write(&json_file, HOOKS_JSON);
        let start = format!("{}:session_start:0:0", json_file.display());
        let good = command_hash("session_start", Some("startup|resume"), "echo start", 600);

        // `features.hooks` set to a non-boolean: codex refuses to start; never read as the default.
        write(
            &codex.join("config.toml"),
            &format!(
                "[features]\nhooks = \"no\"\n{}",
                state(&start, None, Some(&good))
            ),
        );
        let tree = resolve(root, &target, None);
        assert!(
            tree.warnings
                .iter()
                .any(|w| w.contains("#/features/hooks") && w.contains("codex refuses to start")),
            "{:?}",
            tree.warnings
        );
        assert!(hook(&tree, &start)
            .origin
            .why
            .iter()
            .any(|w| w.contains("#/features/hooks") && w.contains("not a boolean")));

        // A non-finite `enabled` makes the state entry fail to deserialize: skipped whole, so the
        // trusted_hash beside it does not count (`hooks/src/config_rules.rs:43-48`).
        write(
            &codex.join("config.toml"),
            &format!(
                "[hooks.state.{}]\nenabled = nan\ntrusted_hash = \"{good}\"\n",
                serde_json::to_string(&start).unwrap()
            ),
        );
        let tree = resolve(root, &target, None);
        let row = hook(&tree, &start);
        assert!(
            row.signal.as_deref().unwrap().starts_with("off: untrusted"),
            "{:?}",
            row.signal
        );

        // A non-finite timeout in inline hooks: the TOML hooks fail to parse (no silent default).
        write(
            &codex.join("config.toml"),
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"s\"\ntimeout = inf\n",
        );
        let tree = resolve(root, &target, None);
        assert!(hooks(&tree).iter().all(|i| !i.id.contains(":stop:")));
        assert!(tree
            .warnings
            .iter()
            .any(|w| w.starts_with("failed to parse TOML hooks") && w.contains("timeout")));
    }

    #[test]
    fn requirements_values_are_read_strictly() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(&codex.join("hooks.json"), HOOKS_JSON);
        let req = root.join("etc/codex/requirements.toml");
        write(&req, "allow_managed_hooks_only = \"yes\"\n");
        let tree = resolve(root, &target, None);
        assert!(
            tree.warnings
                .iter()
                .any(|w| w.contains("#/allow_managed_hooks_only")
                    && w.contains("codex refuses to start")),
            "{:?}",
            tree.warnings
        );
        write(&req, "allow_managed_hooks_only = nan\n");
        let tree = resolve(root, &target, None);
        assert!(
            tree.warnings
                .iter()
                .any(|w| w.contains("#/allow_managed_hooks_only") && w.contains("non-finite")),
            "{:?}",
            tree.warnings
        );
        std::fs::write(&req, b"allow_managed_hooks_only = true\n# \xff\n").unwrap();
        let tree = resolve(root, &target, None);
        assert!(
            tree.warnings
                .iter()
                .any(|w| w.contains("requirements.toml") && w.contains("not UTF-8")),
            "{:?}",
            tree.warnings
        );
    }

    #[test]
    fn a_non_finite_inline_hook_field_is_a_parse_error() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let config = root.join("home/.codex/config.toml");
        write(
            &config,
            "[[hooks.SessionStart]]\nmatcher = nan\n[[hooks.SessionStart.hooks]]\ntype = \"command\"\ncommand = \"c\"\n",
        );
        let tree = resolve(root, &target, None);
        assert!(hooks(&tree).is_empty());
        assert!(
            tree.warnings.iter().any(|w| w.starts_with(&format!(
                "failed to parse TOML hooks in {}: `SessionStart`: `matcher`: expected a string, found a non-finite float",
                config.display()
            ))),
            "{:?}",
            tree.warnings
        );
    }

    /// The invariant TOML mode rests on: no knob of this resolver (the only source of overlay
    /// patches, `plan.rs` `overlay_op`) writes under `/hooks/<Event>`, and none writes `null`, so a
    /// `null` inside an inline hooks table can only be a non-finite float.
    #[test]
    fn no_knob_writes_into_hook_declarations_or_writes_null() {
        let t = tmp();
        let root = &t.0;
        let target = setup(root);
        let codex = root.join("home/.codex");
        write(&codex.join("hooks.json"), HOOKS_JSON);
        write(
            &codex.join("rules/default.rules"),
            "prefix_rule(pattern = [\"ls\"])\n",
        );
        write(
            &codex.join("config.toml"),
            &format!(
                "developer_instructions = \"x\"\n[mcp_servers.m]\ncommand = \"m\"\n[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"s\"\n{}",
                trust_project(root)
            ),
        );
        write(&root.join("repo/AGENTS.md"), "agents");
        write(&root.join("repo/.codex/hooks.json"), HOOKS_JSON);
        let tree = resolve(root, &target, None);
        let mut settings = 0;
        for item in walk(&tree) {
            for knob in &item.knobs {
                if let Knob::Setting { pointer, value, .. } = knob {
                    settings += 1;
                    assert!(
                        !pointer.starts_with("/hooks/") || pointer.starts_with("/hooks/state/"),
                        "{} writes {pointer}",
                        item.id
                    );
                    assert!(!value.is_null(), "{} writes null at {pointer}", item.id);
                }
            }
        }
        assert!(settings > 10, "the fixture exercises the knobs: {settings}");
    }
}
