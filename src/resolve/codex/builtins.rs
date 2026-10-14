//! Built-in blocks (`research/02` §0 request anatomy, §2 key table, §9). Codex generates their
//! text at startup from the session (sandbox, approval policy, collaboration mode, connectors,
//! cwd, shell, date), so a file gives no byte count: cost `?` until a capture. The base
//! instructions are the exception — their text is a file (`model_instructions_file`), a config
//! string (`instructions`) or the model catalog cached in `$CODEX_HOME/models_cache.json`.
//!
//! Each block is a row while its switch is on and leaves the tree when it is off:
//! `include_permissions_instructions`, `include_collaboration_mode_instructions`,
//! `include_apps_instructions`, `include_environment_context` (default true,
//! `config/defaults.toml:2-5`; read at `core/src/config/mod.rs:4006-4019`), `agents.enabled`
//! (default true, `core/src/config/mod.rs:3874-3878`). Knob = the key set to `false`.
//!
//! Two of them also need the model's catalog entry ([`collaboration_mode`], [`multi_agent`]);
//! while harn does not know the entry the row carries a [`super::CATALOG_GATED`] signal. A known
//! entry without multi-agent v2 leaves no row (no codex front end renders one); a known entry
//! without the Default mode's text keeps the collaboration-mode row with mode `none`, because
//! `codex exec` and `codex debug prompt-input` send nothing but the interactive TUI sends the
//! bundled preset.

use super::{catalog_field, config_knobs, new_item, unparsable, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode};
use serde_json::{json, Value};
use std::path::Path;

const BASE_KNOB: &str = "no off switch; model_instructions_file only replaces it, and codex calls that STRONGLY DISCOURAGED (config/src/config_toml.rs:266-270)";

/// The request's `instructions` field: `model_instructions_file` (trimmed, must be non-empty),
/// else the `instructions` key, else the model's catalog template (`core/src/config/mod.rs:3988-4002`).
pub(crate) fn base_instructions(ctx: &mut Ctx) -> Item {
    let mut item = new_item(
        Kind::SystemPrompt,
        Layer::Builtin,
        "instructions",
        "base instructions",
    );
    item.knobs = vec![Knob::None {
        reason: BASE_KNOB.to_string(),
    }];
    // Set to something that is not a string: codex refuses the config; the size stays unknown
    // rather than falling back to the next source.
    for key in ["/model_instructions_file", "/instructions"] {
        if let Some((value, layer)) = ctx.config.get(key) {
            if !value.is_string() {
                let at = layer.at(key);
                item.origin.setting = Some(at.clone());
                item.origin.why = vec![format!(
                    "{at} is not a string; codex refuses to start with it; size unknown"
                )];
                return item;
            }
        }
    }
    if let Some((value, layer)) = ctx.config.get_str("/model_instructions_file") {
        let at = layer.at("/model_instructions_file");
        // Relative paths resolve against the layer's own dir (`resolve_relative_paths_in_config_toml`).
        let path = match (&layer.path, Path::new(value).is_absolute()) {
            (Some(file), false) => file
                .parent()
                .map(|p| p.join(value))
                .unwrap_or_else(|| value.into()),
            _ => value.into(),
        };
        item.label = "base instructions (model_instructions_file)".to_string();
        item.origin.setting = Some(at.clone());
        item.origin.why = vec![format!("{at}: replaces the model's base instructions")];
        match ctx.fs.read_string(&path) {
            Some(text) if !text.trim().is_empty() => {
                item.origin.path = Some(ctx.fs.realpath(&path));
                item.cost = Cost {
                    source: Some(text.len() as u64),
                    predicted: Some(text.trim().len() as u64),
                    ..Cost::default()
                };
            }
            _ => ctx.warnings.push(format!(
                "{at}: {} is missing or empty; codex refuses to start (core/src/config/mod.rs:4574-4600)",
                path.display()
            )),
        }
        return item;
    }
    if let Some((text, layer)) = ctx.config.get_str("/instructions") {
        let at = layer.at("/instructions");
        item.label = "base instructions (instructions key)".to_string();
        item.origin.setting = Some(at.clone());
        item.origin.why = vec![format!("{at}: replaces the model's base instructions")];
        item.cost = Cost {
            source: Some(text.len() as u64),
            predicted: Some(text.len() as u64),
            ..Cost::default()
        };
        return item;
    }
    if let Some((file, _)) = ctx.config.get_str("/model_catalog_json") {
        item.origin.why = vec![format!(
            "model_catalog_json = {file}: the catalog comes from that file (not read); size unknown"
        )];
        return item;
    }
    let slug = ctx.model.slug.clone();
    let template = ctx.model.catalog.as_ref().and_then(|entry| {
        entry
            .pointer("/model_messages/instructions_template")
            .and_then(Value::as_str)
            .or_else(|| entry.get("base_instructions").and_then(Value::as_str))
            .map(str::to_string)
    });
    match (slug, template) {
        (Some(slug), Some(template)) => {
            let personality_none = ctx
                .config
                .get_str("/personality")
                .is_some_and(|(p, _)| p == "none");
            let text = if personality_none {
                strip_personality_section(template)
            } else {
                template
            };
            let bytes = text.len() as u64;
            item.label = format!("base instructions ({slug})");
            item.origin.path = Some(ctx.codex_home.join("models_cache.json"));
            item.origin.why = vec![format!(
                "{}: {slug} instructions_template (the catalog may refresh at startup)",
                ctx.display(&ctx.codex_home.join("models_cache.json"))
            )];
            if personality_none {
                item.origin.why.push(
                    "personality = \"none\": the # Personality section is cut (models-manager/src/model_info.rs:50-57)"
                        .to_string(),
                );
            }
            item.cost = Cost {
                source: Some(bytes),
                predicted: Some(bytes),
                predicted_is_estimate: true,
                ..Cost::default()
            };
        }
        (Some(slug), None) => {
            let cache = ctx.display(&ctx.codex_home.join("models_cache.json"));
            item.origin.why = vec![if ctx.model.catalog.is_some() {
                format!("{cache}: the {slug} entry has no instructions template; size unknown until a capture")
            } else {
                format!("{slug} is not in {cache}: size unknown until a capture")
            }];
        }
        (None, _) => {
            item.origin.why = vec![
                "no model in config: codex picks its default from the catalog at startup; size unknown until a capture"
                    .to_string(),
            ];
        }
    }
    item
}

/// The developer-message blocks after `developer_instructions`, then the multi-agent messages.
pub(crate) fn developer_blocks(ctx: &Ctx) -> Vec<Item> {
    let mut out = Vec::new();
    out.extend(block(
        ctx,
        Kind::SystemPrompt,
        "permissions_instructions",
        "permissions instructions",
        "/include_permissions_instructions",
        &[
            "`<permissions instructions>` developer block: sandbox, approval policy, approved command prefixes (text generated by codex)",
            "off: codex keeps only a compact tracker of approved command prefixes, which renders nothing in the first request (core/src/context/world_state/compact_permissions.rs:43-50, world_state/mod.rs:398-399); prefixes approved later arrive as runtime messages",
        ],
    ));
    out.extend(collaboration_mode(ctx));
    out.extend(block(
        ctx,
        Kind::SystemPrompt,
        "apps_instructions",
        "apps instructions",
        "/include_apps_instructions",
        &[
            "`<apps_instructions>` developer block",
            "rendered only when apps are on for the login and a connector is reachable — decided at runtime (core/src/session/world_state.rs:256-269)",
        ],
    ));
    out.extend(multi_agent(ctx));
    out
}

/// `<collaboration_mode>`. A fresh session is in Default mode with no text of its own
/// (`core/src/session/mod.rs:802-809`; `codex exec` sends no mode, `exec/src/lib.rs:1177`, and
/// `debug prompt-input` uses the default turn, `core/src/prompt_debug.rs:88`), so the block is
/// the catalog's `model_messages.collaboration_modes.default` or nothing
/// (`core/src/context/world_state/collaboration_mode.rs:30-42`, `:117-118`).
fn collaboration_mode(ctx: &Ctx) -> Option<Item> {
    const CONDITION: &str = "model_messages.collaboration_modes.default";
    let mut item = block(
        ctx,
        Kind::SystemPrompt,
        "collaboration_mode",
        "collaboration mode",
        "/include_collaboration_mode_instructions",
        &["`<collaboration_mode>` developer block (core/src/session/world_state.rs:203-210)"],
    )?;
    // `model_messages`, `collaboration_modes` and `default` are all `Option`s: missing or `null`
    // is `None` (`protocol/src/openai_models.rs:430-431`, `:558`, `:750-753`).
    let text = ctx.catalog_entry().and_then(|(slug, entry)| {
        match catalog_field(
            slug,
            entry,
            &["model_messages", "collaboration_modes", "default"],
        )? {
            None => Ok((slug, false)),
            Some(Value::String(_)) => Ok((slug, true)),
            Some(other) => Err(unparsable(slug, CONDITION, other)),
        }
    });
    let slug = match text {
        Err(why) => {
            item.signal = Some(Ctx::catalog_gated(CONDITION, &why));
            return Some(item);
        }
        Ok((slug, false)) => slug,
        Ok((slug, true)) => {
            item.origin.why.push(format!(
                "models_cache.json: {slug} has {CONDITION}, the Default mode's text (core/src/context/world_state/collaboration_mode.rs:30-42)"
            ));
            return Some(item);
        }
    };
    item.mode = Mode::None;
    item.signal = Some(format!(
        "{slug}'s catalog entry has no Default mode text: none from codex exec or debug prompt-input; the TUI sends the bundled preset"
    ));
    item.origin.why.push(format!(
        "models_cache.json: {slug} has no {CONDITION}, so codex exec and codex debug prompt-input send no block (core/src/context/world_state/collaboration_mode.rs:36-42, :117-118)"
    ));
    item.origin.why.push(
        "the interactive TUI attaches the Default mode to every turn and the app server fills in the bundled preset text, so it sends the block (tui/src/chatwidget/input_submission.rs:416-422, app-server/src/request_processors/turn_processor.rs:403-418); traced in source, not captured"
            .to_string(),
    );
    Some(item)
}

/// `<multi_agent_role>` + `<multi_agent_mode>`: rendered for multi-agent v2 only
/// (`core/src/session/multi_agents.rs:14-18`, `:77-82`). A fresh session's version
/// (`core/src/session/mod.rs:4244-4256`, `core/src/config/mod.rs:1586-1612`):
/// `features.multi_agent_v2 = true` → v2; else `agents.enabled = false` → disabled; else the
/// catalog's `multi_agent_version`; else `features.multi_agent` → v1 or disabled. The catalog's
/// `model_messages.multi_agent` changes the text, not whether it is sent.
fn multi_agent(ctx: &Ctx) -> Option<Item> {
    const CONDITION: &str = "multi_agent_version = \"v2\"";
    const WHY: &[&str] = &[
        "`<multi_agent_role>` + `<multi_agent_mode>` developer messages and the agent tools (research/02 §9)",
        "`--disable multi_agent` did not remove the role block; agents.enabled = false did (research/02 L1 t11/t12): features.multi_agent only picks v1 or nothing when the catalog is silent (core/src/config/mod.rs:1596-1612)",
    ];
    if let Some(pointer) = multi_agent_v2_on(ctx) {
        let at = ctx
            .config
            .get(pointer)
            .map(|(_, layer)| layer.at(pointer))
            .unwrap_or_else(|| pointer.to_string());
        let mut item = new_item(
            Kind::SystemPrompt,
            Layer::Builtin,
            "multi_agent",
            "multi-agent role + mode",
        );
        item.origin.why = WHY.iter().map(|w| w.to_string()).collect();
        item.origin.why.push(format!(
            "{at} = true: multi-agent v2 whatever agents.enabled and the model's catalog say (core/src/config/mod.rs:1586-1593)"
        ));
        item.knobs = config_knobs(
            ctx,
            pointer,
            json!(false),
            Some("agents.enabled and the model's catalog decide then"),
        );
        return Some(item);
    }
    let mut item = block(
        ctx,
        Kind::SystemPrompt,
        "multi_agent",
        "multi-agent role + mode",
        "/agents/enabled",
        WHY,
    )?;
    // Missing, `null` or a string codex does not recognise is `None`; anything but a string or
    // `null` fails the file (`deserialize_optional_model_selector`,
    // `protocol/src/openai_models.rs:352-361`, `:502-507`).
    let version = ctx.catalog_entry().and_then(|(slug, entry)| {
        match catalog_field(slug, entry, &["multi_agent_version"])? {
            None => Ok((slug, None)),
            Some(Value::String(v)) => Ok((slug, Some(v.as_str()))),
            Some(other) => Err(unparsable(slug, "multi_agent_version", other)),
        }
    });
    match version {
        Err(why) => {
            item.signal = Some(Ctx::catalog_gated(CONDITION, &why));
            Some(item)
        }
        Ok((slug, Some("v2"))) => {
            item.origin.why.push(format!(
                "models_cache.json: {slug} {CONDITION} (core/src/session/mod.rs:4244-4256)"
            ));
            Some(item)
        }
        Ok(_) => None,
    }
}

/// The pointer that turns `features.multi_agent_v2` on — the key itself as a boolean, or its
/// table's `enabled` (`features/src/lib.rs:907-918`, `:861-863`) — when it is on (default off,
/// `features/src/lib.rs:1337-1341`).
fn multi_agent_v2_on(ctx: &Ctx) -> Option<&'static str> {
    const FEATURE: &str = "/features/multi_agent_v2";
    const ENABLED: &str = "/features/multi_agent_v2/enabled";
    match ctx.config.get(FEATURE)?.0 {
        Value::Bool(true) => Some(FEATURE),
        Value::Object(_) => {
            matches!(ctx.config.get(ENABLED), Some((Value::Bool(true), _))).then_some(ENABLED)
        }
        _ => None,
    }
}

pub(crate) fn environment_context(ctx: &Ctx) -> Option<Item> {
    block(
        ctx,
        Kind::EnvBlock,
        "environment_context",
        "environment context (cwd, shell, date)",
        "/include_environment_context",
        &["`<environment_context>` user block: cwd, shell, date, timezone (core/src/session/world_state.rs:228-247)"],
    )
}

/// A generated block: a row while `pointer` is not `false`, cost `?`, knob `pointer = false`.
fn block(
    ctx: &Ctx,
    kind: Kind,
    name: &str,
    label: &str,
    pointer: &str,
    why: &[&str],
) -> Option<Item> {
    let set = ctx.config.get(pointer);
    if matches!(set, Some((Value::Bool(false), _))) {
        return None;
    }
    let mut item = new_item(kind, Layer::Builtin, name, label);
    item.origin.why = why.iter().map(|w| w.to_string()).collect();
    item.origin.why.push(match set {
        Some((Value::Bool(_), layer)) => format!("{} = true", layer.at(pointer)),
        // Set to a non-boolean: codex refuses the config; harn keeps the row rather than
        // guessing the default.
        Some((Value::Null, layer)) => format!(
            "{} is a non-finite float harn cannot read; codex refuses to start with it",
            layer.at(pointer)
        ),
        Some((value, layer)) => format!(
            "{} = {value} is not a boolean; codex refuses to start with it",
            layer.at(pointer)
        ),
        None => format!("{} unset: on by default", pointer[1..].replace('/', ".")),
    });
    item.knobs = config_knobs(ctx, pointer, json!(false), None);
    Some(item)
}

/// `strip_personality_section` (`models-manager/src/model_info.rs:62-96`): the `# Personality`
/// section up to the next H1 heading.
fn strip_personality_section(mut text: String) -> String {
    let mut start = None;
    let mut end = None;
    let mut offset = 0;
    for line_with_ending in text.split_inclusive('\n') {
        let line = match line_with_ending.strip_suffix('\n') {
            Some(line) => line.strip_suffix('\r').unwrap_or(line),
            None => line_with_ending,
        };
        if start.is_some() {
            if is_h1(line) {
                end = Some(offset);
                break;
            }
        } else if line == "# Personality" {
            start = Some(offset);
        }
        offset += line_with_ending.len();
    }
    if let Some(start) = start {
        let end = end.unwrap_or(text.len());
        text.replace_range(start..end, "");
    }
    text
}

fn is_h1(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('#') else {
        return false;
    };
    rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personality_section_is_cut_to_the_next_h1() {
        let text = "# A\nx\n# Personality\np\n## sub\n# B\ny\n".to_string();
        assert_eq!(strip_personality_section(text), "# A\nx\n# B\ny\n");
        let tail = "# A\n# Personality\np\n".to_string();
        assert_eq!(strip_personality_section(tail), "# A\n");
        let none = "# A\n".to_string();
        assert_eq!(strip_personality_section(none), "# A\n");
    }
}
