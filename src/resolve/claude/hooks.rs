//! Hooks (`research/01` §3): every matcher group of every event in every settings layer's `hooks`
//! key and in each enabled plugin's `hooks/hooks.json` (or its manifest's `hooks`). Plain stdout
//! becomes context only for `SessionStart`, `UserPromptSubmit`, `UserPromptExpansion` and
//! `PostModelSwitch`: those rows are `always` with cost `?` (the stdout is unknown until a
//! capture); every other event is `runtime` (`—`). A `SessionStart` group whose matcher cannot
//! match `startup` does not fire on the first request and is `runtime` too.
//! `disableAllHooks` turns off every non-managed hook (managed ones too when set in managed
//! settings); `allowManagedHooksOnly` (managed) keeps only managed hooks. A hook
//! `disableAllHooks` turned off stays a row: mode none, its `signal` naming the setting
//! (`off: disableAllHooks`), a `why` step saying where it is set (`<file>#/disableAllHooks =
//! true`), its knobs as when on — a reader sees what went off and why, and `o`/undo
//! still find it. Under `allowManagedHooksOnly` (settings-reference, "What runs under
//! allowManagedHooksOnly": https://code.claude.com/docs/en/settings-reference) managed hooks run,
//! and so do the hooks of a plugin managed settings force-enable (`enabledPlugins["<name>@<m>"]
//! = true`, the full id: the same name from another marketplace stays blocked); every other hook
//! stays a row the same way (`off: allowManagedHooksOnly (managed policy)`,
//! `<file>#/allowManagedHooksOnly = true`), with no knob: no setting outside the managed ones
//! overrides that policy. The status line, read from managed settings only under it, is off the
//! same way when set elsewhere (`builtins.rs`). Under both switches, a row says
//! `disableAllHooks` only.

use super::fs::esc;
use super::plugins::Plugin;
use super::settings::Source;
use super::{cost_na, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::{json, Map, Value};
use std::path::Path;

/// The `signal` of a hook (and of the status line) `disableAllHooks` turned off.
pub const OFF_ALL_HOOKS: &str = "off: disableAllHooks";

/// The `signal` of a hook (and of the status line) a managed `allowManagedHooksOnly` turned off
/// (and `disableAllHooks` did not).
pub const OFF_MANAGED_HOOKS_ONLY: &str = "off: allowManagedHooksOnly (managed policy)";

/// The `Knob::None` reason of such a hook: what `harn off` / `harn on` refuse with.
const MANAGED_HOOKS_ONLY: &str =
    "managed policy allowManagedHooksOnly allows only managed hooks; no user setting overrides it";

/// `<file>#/allowManagedHooksOnly = true` of the managed settings file that sets it, if any.
pub(crate) fn managed_hooks_only(ctx: &Ctx) -> Option<String> {
    ctx.settings
        .files
        .iter()
        .find(|f| {
            f.source == Source::Managed
                && f.value.get("allowManagedHooksOnly") == Some(&json!(true))
        })
        .map(|f| format!("{} = true", f.at("/allowManagedHooksOnly")))
}

/// Managed settings force-enable the plugin `id` (`name@marketplace`, matched whole).
fn force_enabled(ctx: &Ctx, id: &str) -> bool {
    ctx.settings.files.iter().any(|f| {
        f.source == Source::Managed
            && f.value.get("enabledPlugins").and_then(|e| e.get(id)) == Some(&json!(true))
    })
}

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
    // The signal and the `why` step naming where the switch is set.
    off: Option<(&str, &str)>,
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
            if let Some((signal, set_at)) = off {
                item.origin.why.push(set_at.to_string());
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
    let all_off = matches!(disable_all, Some((true, f)) if f.source == Source::Managed);
    let set_at = disable_all.map(|(_, f)| format!("{} = true", f.at("/disableAllHooks")));
    let switch_off = match (disable_all, &set_at) {
        (Some((true, _)), Some(at)) => Some((OFF_ALL_HOOKS, at.as_str())),
        _ => None,
    };
    let policy_at = managed_hooks_only(ctx);
    let policy_off = policy_at.as_deref().map(|at| (OFF_MANAGED_HOOKS_ONLY, at));
    // Off by either: still listed. Both: the switch's signal alone.
    let non_managed_off = switch_off.or(policy_off);
    let by_policy = matches!(non_managed_off, Some((OFF_MANAGED_HOOKS_ONLY, _)));
    let policy_knobs = vec![Knob::None {
        reason: MANAGED_HOOKS_ONLY.to_string(),
    }];
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
        let off = match (managed, all_off) {
            (true, true) => switch_off,
            (true, false) => None,
            (false, _) => non_managed_off,
        };
        let knobs = if managed {
            vec![Knob::None {
                reason: "managed hook".to_string(),
            }]
        } else if by_policy {
            policy_knobs.clone()
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
    for p in plugins {
        // Force-enabled by managed settings: the policy lets its hooks run.
        let (off, by_policy) = if by_policy && force_enabled(ctx, &p.id) {
            (None, false)
        } else {
            (non_managed_off, by_policy)
        };
        for (file, prefix, events) in p.hook_sources(ctx) {
            let knobs = if by_policy {
                policy_knobs.clone()
            } else {
                vec![p.contribution_knob(), disable_knob.clone()]
            };
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
