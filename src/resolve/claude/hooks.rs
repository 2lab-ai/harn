//! Hooks (`research/01` §3): every matcher group of every event in every settings layer's `hooks`
//! key and in each enabled plugin's `hooks/hooks.json` (or its manifest's `hooks`). Plain stdout
//! becomes context only for `SessionStart`, `UserPromptSubmit`, `UserPromptExpansion` and
//! `PostModelSwitch`: those rows are `always` with cost `?` (the stdout is unknown until a
//! capture); every other event is `runtime` (`—`). A `SessionStart` group whose matcher cannot
//! match `startup` does not fire on the first request and is `runtime` too.
//! `disableAllHooks` turns off every non-managed hook (managed ones too when set in managed
//! settings); `allowManagedHooksOnly` (managed) keeps only managed hooks. A hook
//! `disableAllHooks` turned off stays a row: mode none, its `signal` naming the setting
//! (`off: disableAllHooks`), its knobs as when on — a reader sees what went off, and `o`/undo
//! still find it. A hook only `allowManagedHooksOnly` drops is left out, as before.

use super::fs::esc;
use super::plugins::Plugin;
use super::settings::Source;
use super::{cost_na, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::{json, Map, Value};
use std::path::Path;

/// The `signal` of a hook (and of the status line) `disableAllHooks` turned off.
pub const OFF_ALL_HOOKS: &str = "off: disableAllHooks";

const INJECTING: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PostModelSwitch",
];

fn mode_of(event: &str, group: &Value) -> Mode {
    if !INJECTING.contains(&event) {
        return Mode::Runtime;
    }
    if event == "SessionStart" {
        let matcher = group.get("matcher").and_then(Value::as_str).unwrap_or("");
        if !(matcher.is_empty() || matcher == "*" || matcher.contains("startup")) {
            return Mode::Runtime;
        }
    }
    Mode::Always
}

#[allow(clippy::too_many_arguments)]
fn push_groups(
    ctx: &Ctx,
    out: &mut Vec<Item>,
    file: &Path,
    prefix: &str,
    events: &Map<String, Value>,
    layer: Layer,
    plugin: Option<&Plugin>,
    knobs: &[Knob],
    off: Option<&str>,
) {
    for (event, groups) in events {
        let Value::Array(groups) = groups else {
            continue;
        };
        for (i, group) in groups.iter().enumerate() {
            let pointer = format!("{prefix}/{}/{i}", esc(event));
            let setting = format!("{}#{pointer}", file.display());
            let commands = group
                .get("hooks")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty());
            let owner = match plugin {
                Some(p) => format!("plugin {}", p.name),
                None => ctx.display(file),
            };
            let label = match matcher {
                Some(m) => format!("{event} hook [{m}] · {commands} cmd · {owner}"),
                None => format!("{event} hook · {commands} cmd · {owner}"),
            };
            let mut item = new_item(Kind::Hook, layer, &setting, &label);
            item.mode = match off {
                Some(_) => Mode::None,
                None => mode_of(event, group),
            };
            item.cost = if item.mode == Mode::Always {
                Cost::default()
            } else {
                cost_na(None)
            };
            item.origin.path = Some(file.to_path_buf());
            item.origin.setting = Some(setting.clone());
            item.origin.why = match plugin {
                Some(p) => p.why_with(setting),
                None => vec![setting],
            };
            if item.mode == Mode::Always {
                item.origin.why.push(format!(
                    "{event} stdout is injected; size unknown until a capture"
                ));
            }
            if let Some(signal) = off {
                item.origin.why.push(signal.to_string());
                item.signal = Some(signal.to_string());
            }
            item.origin.plugin = plugin.map(|p| p.id.clone());
            item.knobs = knobs.to_vec();
            out.push(item);
        }
    }
}

pub(crate) fn collect(ctx: &Ctx, plugins: &[Plugin]) -> Vec<Item> {
    let mut out = Vec::new();
    let disable_all = ctx.settings.get_bool("/disableAllHooks");
    let managed_only = ctx.settings.files.iter().any(|f| {
        f.source == Source::Managed && f.value.get("allowManagedHooksOnly") == Some(&json!(true))
    });
    let all_off = matches!(disable_all, Some((true, f)) if f.source == Source::Managed);
    let non_managed_off = all_off || matches!(disable_all, Some((true, _))) || managed_only;
    // Off by the switch: still listed. Off only by `allowManagedHooksOnly`: dropped, as before
    // — no view has words for "your organisation allows only its own hooks" yet.
    let switch_off = matches!(disable_all, Some((true, _))).then_some(OFF_ALL_HOOKS);
    let disable_knob = setting_knob(
        ctx,
        Scope::Local,
        "/disableAllHooks",
        json!(true),
        Some("turns off every non-managed hook and the status line".to_string()),
    );
    for file in &ctx.settings.files {
        let Some(Value::Object(events)) = file.value.get("hooks") else {
            continue;
        };
        let managed = file.source == Source::Managed;
        let off = if all_off || (!managed && non_managed_off) {
            match switch_off {
                Some(signal) => Some(signal),
                None => continue,
            }
        } else {
            None
        };
        let knobs = if managed {
            vec![Knob::None {
                reason: "managed hook".to_string(),
            }]
        } else {
            vec![disable_knob.clone()]
        };
        push_groups(
            ctx,
            &mut out,
            &file.path,
            "/hooks",
            events,
            file.source.layer(),
            None,
            &knobs,
            off,
        );
    }
    let off = match (non_managed_off, switch_off) {
        (false, _) => None,
        (true, Some(signal)) => Some(signal),
        (true, None) => return out,
    };
    for p in plugins {
        for (file, prefix, events) in p.hook_sources(ctx) {
            let knobs = vec![p.contribution_knob(), disable_knob.clone()];
            push_groups(
                ctx,
                &mut out,
                &file,
                &prefix,
                &events,
                p.layer,
                Some(p),
                &knobs,
                off,
            );
        }
    }
    out
}
