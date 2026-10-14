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

use super::{config_knobs, new_item, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer};
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
            item.origin.why = vec![format!(
                "{slug} is not in {}: size unknown until a capture",
                ctx.display(&ctx.codex_home.join("models_cache.json"))
            )];
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
            "off leaves a compact block listing approved command prefixes (core/src/session/world_state.rs:168-201)",
        ],
    ));
    out.extend(block(
        ctx,
        Kind::SystemPrompt,
        "collaboration_mode",
        "collaboration mode",
        "/include_collaboration_mode_instructions",
        &["`<collaboration_mode>` developer block (core/src/session/world_state.rs:203-210)"],
    ));
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
    out.extend(block(
        ctx,
        Kind::SystemPrompt,
        "multi_agent",
        "multi-agent role + mode",
        "/agents/enabled",
        &[
            "`<multi_agent_role>` + `<multi_agent_mode>` developer messages and the agent tools (research/02 §9)",
            "`--disable multi_agent` did not remove the role block; agents.enabled = false did (research/02 L1 t11/t12)",
        ],
    ));
    out
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
    let set = ctx.config.get_bool(pointer);
    if matches!(set, Some((false, _))) {
        return None;
    }
    let mut item = new_item(kind, Layer::Builtin, name, label);
    item.origin.why = why.iter().map(|w| w.to_string()).collect();
    item.origin.why.push(match set {
        Some((_, layer)) => format!("{} = true", layer.at(pointer)),
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
