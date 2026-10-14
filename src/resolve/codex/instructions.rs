//! `developer_instructions` and the AGENTS.md user message (`research/02` §1), as codex-cli
//! 0.160.1 builds them:
//!
//! - global: `$CODEX_HOME/AGENTS.override.md`, else `$CODEX_HOME/AGENTS.md` — the first whose
//!   trimmed text is non-empty (`codex-home/src/instructions/mod.rs:40-84`); not charged to
//!   `project_doc_max_bytes`.
//! - project chain: skipped when the active project is explicitly `untrusted`
//!   (`core/src/agents_md.rs:63-66`); otherwise every dir from the AGENTS.md root (nearest ancestor
//!   holding a `project_root_markers` entry, read from non-project layers only) down to the
//!   target; per dir the first existing file of `AGENTS.override.md`, `AGENTS.md`, then each
//!   `project_doc_fallback_filenames` entry (`agents_md.rs:194-301`). The pick is by metadata
//!   only: an empty `AGENTS.override.md` still shadows `AGENTS.md`, then reads as nothing.
//! - one `project_doc_max_bytes` budget for the whole chain: each file is cut to what is left,
//!   decoded lossily, skipped when only whitespace (no budget used), and the budget shrinks by the
//!   bytes read; once it is 0 the rest are not read (`agents_md.rs:139-187`).
//! - the message: `# AGENTS.md instructions` + ` for <cwd>` (only when a project file landed) +
//!   `\n\n<INSTRUCTIONS>\n` + text + `\n</INSTRUCTIONS>`, where text = global, then
//!   `\n\n--- project-doc ---\n\n` (only when both are present), then the project files joined by
//!   `\n\n` (`context/user_instructions.rs:23-34`, `context-fragments/src/fragment.rs:91-98`,
//!   `agents_md.rs:375-405`).
//!
//! Attribution (as the Claude Code resolver keeps file rows to their own bytes): each file row
//! costs the text of it that lands; the parent `agents-md-message` row costs the framing and the
//! separators only, so a sum over rows counts every byte once.

use super::{config_knobs, layer_of, new_item, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode};
use crate::resolve::claude::{cost_na, fs};
use serde_json::{json, Value};
use std::path::PathBuf;

const OVERRIDE: &str = "AGENTS.override.md";
const DEFAULT: &str = "AGENTS.md";
const HEADER: &str = "# AGENTS.md instructions";
const OPEN: &str = "\n\n<INSTRUCTIONS>\n";
const CLOSE: &str = "\n</INSTRUCTIONS>";
const PROJECT_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";
const JOIN: &str = "\n\n";
/// The id name of the AGENTS.md message row.
pub(crate) const MESSAGE: &str = "agents-md-message";

/// `developer_instructions` (the first section of the developer message; codex skips an empty
/// string, `core/src/session/mod.rs:4321-4328`). The knob writes `""` — the same skip, and it
/// also masks a lower layer's value — in the narrowest file that takes effect.
pub(crate) fn developer_instructions(ctx: &Ctx) -> Option<Item> {
    let (text, layer) = ctx.config.get_str("/developer_instructions")?;
    if text.is_empty() {
        return None;
    }
    let at = layer.at("/developer_instructions");
    let mut item = new_item(
        Kind::Instruction,
        layer_of(layer),
        &at,
        "developer_instructions",
    );
    let bytes = text.len() as u64;
    item.cost = Cost {
        source: Some(bytes),
        predicted: Some(bytes),
        ..Cost::default()
    };
    item.origin.path = layer.path.clone();
    item.origin.setting = Some(at.clone());
    item.origin.why = vec![
        at,
        "developer message, first section, verbatim (core/src/session/mod.rs:4321-4328)"
            .to_string(),
    ];
    let note = "\"\" = no developer_instructions section (codex skips an empty string)";
    let mut knobs = config_knobs(ctx, "/developer_instructions", json!(""), Some(note));
    if layer.source == super::config::Source::Profile {
        if let Some(path) = &layer.path {
            knobs.retain(|k| !matches!(k, Knob::None { .. }));
            knobs.push(Knob::Setting {
                file: path.clone(),
                pointer: "/developer_instructions".to_string(),
                value: json!(""),
                scope: crate::model::Scope::User,
                note: Some(format!("{note}; the profile that sets it")),
            });
        }
    }
    item.knobs = knobs;
    Some(item)
}

/// The AGENTS.md message row with one child per file, or nothing when no file lands. Files that
/// do not land (whitespace only, or past the budget) are listed with mode `none` — under the
/// message when there is one, else top-level.
pub(crate) fn agents_md(ctx: &mut Ctx) -> Vec<Item> {
    let mut landed: Vec<Item> = Vec::new();
    let mut not_landed: Vec<Item> = Vec::new();
    let (global, global_empty) = global(ctx);
    let has_global = global.is_some();
    landed.extend(global);
    not_landed.extend(global_empty);

    let mut project_landed = 0usize;
    if !ctx.config.active_project.is_untrusted() {
        let (rows, count) = project_chain(ctx);
        project_landed = count;
        for row in rows {
            if row.mode == Mode::None {
                not_landed.push(row);
            } else {
                landed.push(row);
            }
        }
    }
    if landed.is_empty() {
        return not_landed;
    }

    let cwd = ctx.dir().display().to_string();
    let mut framing = HEADER.len() + OPEN.len() + CLOSE.len();
    let mut why = vec![
        "user message `# AGENTS.md instructions` + <INSTRUCTIONS> (core/src/context/user_instructions.rs:23-34)"
            .to_string(),
    ];
    if project_landed > 0 {
        framing += " for ".len() + cwd.len();
        framing += project_landed.saturating_sub(1) * JOIN.len();
        why.push(format!("` for {cwd}`: a project file landed"));
        if has_global {
            framing += PROJECT_SEPARATOR.len();
            why.push(
                "`--- project-doc ---` separates the global file from the project files"
                    .to_string(),
            );
        }
    }
    let mut message = new_item(
        Kind::Instruction,
        Layer::Builtin,
        MESSAGE,
        "AGENTS.md message (framing)",
    );
    message.cost = Cost {
        predicted: Some(framing as u64),
        ..Cost::default()
    };
    message.origin.why = why;
    message.knobs = vec![Knob::None {
        reason: "framing: leaves with the last file in it".to_string(),
    }];
    message.children = landed;
    message.children.extend(not_landed);
    vec![message]
}

/// `$CODEX_HOME/AGENTS.override.md`, else `AGENTS.md`: the first with non-empty trimmed text
/// (decoded lossily, `codex-home/src/instructions/mod.rs:68-79`). Unlike the project chain, a
/// whitespace-only global override does not shadow: codex falls through to `AGENTS.md`. Returns
/// the landed file and the whitespace-only files passed over (mode `none`).
fn global(ctx: &Ctx) -> (Option<Item>, Vec<Item>) {
    let mut empty: Vec<Item> = Vec::new();
    for name in [OVERRIDE, DEFAULT] {
        let path = ctx.codex_home.join(name);
        if !ctx.fs.is_file(&path) {
            continue;
        }
        let Some(bytes) = ctx.fs.read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let trimmed = text.trim();
        let real = ctx.fs.realpath(&path);
        let mut item = new_item(
            Kind::Instruction,
            Layer::User,
            &real.display().to_string(),
            &ctx.display(&path),
        );
        item.origin.path = Some(real);
        if trimmed.is_empty() {
            item.mode = Mode::None;
            item.cost = cost_na(Some(bytes.len() as u64));
            item.origin.why = vec![
                "global AGENTS file, whitespace only: codex skips it and tries the next name (codex-home/src/instructions/mod.rs:68-79)"
                    .to_string(),
            ];
            item.knobs = vec![Knob::None {
                reason: "not loaded (whitespace only)".to_string(),
            }];
            empty.push(item);
            continue;
        }
        item.cost = Cost {
            source: Some(bytes.len() as u64),
            predicted: Some(trimmed.len() as u64),
            ..Cost::default()
        };
        item.origin.why = vec![
            "global AGENTS file ($CODEX_HOME, first non-empty of AGENTS.override.md, AGENTS.md; trimmed; outside the project_doc_max_bytes budget)"
                .to_string(),
        ];
        if name == OVERRIDE && ctx.fs.is_file(&ctx.codex_home.join(DEFAULT)) {
            item.origin.why.push(format!(
                "replaces {}",
                ctx.display(&ctx.codex_home.join(DEFAULT))
            ));
        }
        // No config key turns the global file off (`research/02` §1); the Claude Code resolver
        // gives its file-only items (managed CLAUDE.md) `Knob::None` too.
        item.knobs = vec![Knob::None {
            reason: "no config key; empty or move the file".to_string(),
        }];
        let lands = ctx.display(&path);
        for e in &mut empty {
            e.signal = Some(format!("empty: codex falls through to {lands}"));
        }
        return (Some(item), empty);
    }
    for e in &mut empty {
        e.signal = Some("empty: no global AGENTS file lands".to_string());
    }
    (None, empty)
}

/// The project rows (landed ones mode `always`, the rest mode `none`) and how many landed.
fn project_chain(ctx: &mut Ctx) -> (Vec<Item>, usize) {
    let max = match ctx.config.get("/project_doc_max_bytes") {
        Some((v, layer)) => match v.as_u64() {
            Some(n) => (n, layer.at("/project_doc_max_bytes")),
            None => {
                ctx.warnings.push(format!(
                    "{}: not a non-negative integer, codex refuses to start",
                    layer.at("/project_doc_max_bytes")
                ));
                return (Vec::new(), 0);
            }
        },
        None => return (Vec::new(), 0),
    };
    let (max, max_at) = max;
    if max == 0 {
        return (Vec::new(), 0);
    }
    let names = candidate_names(ctx);
    let dir = ctx.dir().to_path_buf();
    let dirs: Vec<PathBuf> = match &ctx.config.agents_root {
        Some(root) => {
            let mut out: Vec<PathBuf> = Vec::new();
            for a in dir.ancestors() {
                out.push(a.to_path_buf());
                if a == root.as_path() {
                    break;
                }
            }
            out.reverse();
            out
        }
        None => vec![dir.clone()],
    };
    let root_why = match &ctx.config.agents_root {
        Some(root) => format!("chain {} → target", ctx.display(root)),
        None => "no project root marker: target dir only".to_string(),
    };

    let knob_note = "drops every project AGENTS file; the global one stays";
    let knobs = config_knobs(ctx, "/project_doc_max_bytes", json!(0), Some(knob_note));
    let mut rows = Vec::new();
    let mut landed = 0usize;
    let mut remaining = max;
    for d in dirs {
        let Some((pos, path)) = names
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (i, d.join(name)))
            .find(|(_, p)| ctx.fs.is_file(p))
        else {
            continue;
        };
        let (name, from) = &names[pos];
        let shadowed: Vec<String> = names[pos + 1..]
            .iter()
            .map(|(n, _)| d.join(n))
            .filter(|p| ctx.fs.is_file(p))
            .map(|p| fs::file_name(&p))
            .collect();
        let where_ = if d == dir {
            "the target dir".to_string()
        } else {
            ctx.display(&d)
        };
        let mut why = vec![format!("{name} in {where_} ({root_why})")];
        if let Some(from) = from {
            why.push(format!("project_doc_fallback_filenames entry ({from})"));
        }
        if !shadowed.is_empty() {
            why.push(format!(
                "first hit in this dir; shadows {}",
                shadowed.join(", ")
            ));
        }
        let real = ctx.fs.realpath(&path);
        let mut item = new_item(
            Kind::Instruction,
            Layer::Project,
            &real.display().to_string(),
            &ctx.display(&path),
        );
        item.origin.path = Some(real);
        item.knobs = knobs.clone();
        let size = ctx.fs.len(&path).unwrap_or(0);
        if remaining == 0 {
            item.mode = Mode::None;
            item.cost = cost_na(Some(size));
            why.push(format!(
                "not read: the {max}-byte budget is used up ({max_at})"
            ));
            item.origin.why = why;
            item.signal = Some(format!(
                "dropped: project_doc_max_bytes budget ({max}) used up"
            ));
            rows.push(item);
            continue;
        }
        let Some(data) = ctx.fs.read(&path) else {
            continue;
        };
        let kept = data.len().min(remaining as usize);
        let text = String::from_utf8_lossy(&data[..kept]);
        if text.trim().is_empty() {
            item.mode = Mode::None;
            item.cost = cost_na(Some(data.len() as u64));
            why.push("whitespace only: codex skips it (no budget used)".to_string());
            item.origin.why = why;
            if !shadowed.is_empty() {
                item.signal = Some(format!("empty, yet shadows {}", shadowed.join(", ")));
            }
            rows.push(item);
            continue;
        }
        if kept < data.len() {
            item.signal = Some(format!(
                "truncated: {kept} of {} source bytes read, {} bytes land (project_doc_max_bytes = {max}, shared by the chain)",
                data.len(),
                text.len()
            ));
        }
        item.cost = Cost {
            source: Some(data.len() as u64),
            predicted: Some(text.len() as u64),
            ..Cost::default()
        };
        item.origin.why = why;
        remaining = remaining.saturating_sub(kept as u64);
        landed += 1;
        rows.push(item);
    }
    (rows, landed)
}

/// `AGENTS.override.md`, `AGENTS.md`, then each valid `project_doc_fallback_filenames` entry
/// (`agents_md.rs:272-296`): non-empty, not `.`/`..`, no `/` or NUL, no duplicates. Each name
/// carries the setting it came from.
fn candidate_names(ctx: &mut Ctx) -> Vec<(String, Option<String>)> {
    let mut names: Vec<(String, Option<String>)> =
        vec![(OVERRIDE.to_string(), None), (DEFAULT.to_string(), None)];
    let (entries, layer) = match ctx.config.get("/project_doc_fallback_filenames") {
        Some((Value::Array(entries), layer)) => (entries, layer),
        Some((_, layer)) => {
            ctx.warnings.push(format!(
                "{}: not an array of file names; codex refuses to start with it",
                layer.at("/project_doc_fallback_filenames")
            ));
            return names;
        }
        None => return names,
    };
    let at = layer.at("/project_doc_fallback_filenames");
    for entry in entries {
        let Some(name) = entry.as_str() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        if matches!(name, "." | "..") || name.contains(['/', '\0']) {
            ctx.warnings.push(format!(
                "{at}: {name:?} is not a file name; codex ignores it"
            ));
            continue;
        }
        if !names.iter().any(|(n, _)| n == name) {
            names.push((name.to_string(), Some(at.clone())));
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_constants_match_the_codex_rendering() {
        // `# AGENTS.md instructions for /tmp\n\n<INSTRUCTIONS>\nbody\n</INSTRUCTIONS>`
        // (core/src/context/contextual_user_message_tests.rs:34).
        let rendered = format!("{HEADER} for /tmp{OPEN}body{CLOSE}");
        assert_eq!(
            rendered,
            "# AGENTS.md instructions for /tmp\n\n<INSTRUCTIONS>\nbody\n</INSTRUCTIONS>"
        );
        assert_eq!(PROJECT_SEPARATOR.len(), 23);
    }
}
