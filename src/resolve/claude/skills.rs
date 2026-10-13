//! The skill listing (`research/01` §4, §6): one `skill-listing` container (advertised) whose
//! children are the entries Claude Code puts in the listing reminder — user skills
//! (`~/.claude/skills/*/SKILL.md`), project skills (`.claude/skills/` in the target and each
//! parent up to the git root), enabled plugins' skills (`<plugin>:<skill>`), claude.ai-synced
//! skills (`~/.claude/skills/synced/<bucket>/<name>`, listed as `anthropic-skills:<name>`),
//! bundled skills (static table) and custom commands (`.claude/commands`, `~/.claude/commands`,
//! plugin `commands/`).
//!
//! An entry is `- <name>: <description>`; with `when_to_use` the description reads
//! `<description> - <when_to_use>` (the 2026-10-13 capture: `ssot-converge` lists
//! `…continue. - Triggers: …`, its `description` trimmed and its `when_to_use` kept as parsed), cut
//! at 1,536 UTF-16 units (`skillListingMaxDescChars`). `cost.source` = `cost.predicted` = the
//! entry's bytes including its newline (a multi-line entry runs to the next `- name`, the
//! request-body parser's bounds). The container's id is `skills`
//! (`oracle::claude::names::SKILL_LISTING`). The listing budget is 1 % of the context window in
//! tokens, so whether descriptions are dropped is not decidable here: `signal` stays None.

use super::frontmatter;
use super::fs::{esc, file_name};
use super::plugins::Plugin;
use super::{cost_na, env_knob, json_string, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Default `skillListingMaxDescChars`.
const MAX_DESC: usize = 1_536;

/// Bundled skills reported by the Claude Code 2.1.296 `init` event (no file on disk), with
/// whether the 2026-10-13 listing carried them (`false` = user-invocable only: never in the
/// listing, mode on-demand). Static table; a user or project skill with the same name replaces
/// the bundled one. `deep-research` is the unprefixed bundled one; the synced copy is
/// `anthropic-skills:deep-research`.
const BUNDLED: &[(&str, bool)] = &[
    ("deep-research", false),
    ("design", false),
    ("design-sync", false),
    ("dataviz", true),
    ("update-config", true),
    ("verify", false),
    ("debug", false),
    ("code-review", true),
    ("simplify", true),
    ("batch", false),
    ("fewer-permission-prompts", true),
    ("doctor", false),
    ("loop", true),
    ("schedule", true),
    ("claude-api", true),
    ("workflow-authoring", true),
    ("run", true),
    ("run-skill-generator", false),
    ("plugin-authoring", true),
];

/// Built-in commands present in the 2026-10-13 listing with no file origin (not in the `init`
/// event's `skills`). Static table; cost `?`.
const BUILTIN_COMMANDS: &[&str] = &["init", "security-review", "keybindings-help"];

/// UTF-16 length, the unit of the JavaScript-side cut.
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn cut_utf16(s: &str, max: usize) -> (String, bool) {
    if utf16_len(s) <= max {
        return (s.to_string(), false);
    }
    let mut out = String::new();
    let mut n = 0;
    for c in s.chars() {
        if n + c.len_utf16() > max {
            break;
        }
        n += c.len_utf16();
        out.push(c);
    }
    (out, true)
}

/// Listing entry bytes and whether the size is an estimate (cut applied, or no description on
/// disk so the entry form is unverified).
pub(crate) fn entry_bytes(
    name: &str,
    fm: &serde_json::Map<String, Value>,
    max_desc: usize,
) -> (u64, bool) {
    // A frontmatter `name` other than the directory name is shown after it in parentheses
    // (`- stv:excalidraw-diagram-skill (stv:excalidraw-diagram)`, 2026-10-13 capture; seen only
    // name-only, so the form with a description is an estimate).
    let base = name.rsplit(':').next().unwrap_or(name);
    let alias = frontmatter::get_str(fm, "name")
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && n != base)
        .map(|n| format!("{}{n}", &name[..name.len() - base.len()]));
    let shown = match &alias {
        Some(a) => format!("{name} ({a})"),
        None => name.to_string(),
    };
    let (bytes, estimate) = entry_bytes_shown(&shown, fm, max_desc);
    (bytes, estimate || alias.is_some())
}

fn entry_bytes_shown(
    name: &str,
    fm: &serde_json::Map<String, Value>,
    max_desc: usize,
) -> (u64, bool) {
    let description = frontmatter::get_str(fm, "description")
        .map(|d| d.trim().to_string())
        .unwrap_or_default();
    let when = frontmatter::get_str(fm, "when_to_use").filter(|w| !w.trim().is_empty());
    let text = match (description.is_empty(), when) {
        (_, None) => description,
        (true, Some(w)) => w,
        (false, Some(w)) => format!("{description} - {w}"),
    };
    if text.is_empty() {
        return (format!("- {name}\n").len() as u64, true);
    }
    let (text, cut) = cut_utf16(&text, max_desc);
    (format!("- {name}: {text}\n").len() as u64, cut)
}

struct Listing {
    children: Vec<Item>,
    /// Names already listed (user/project namespace), for precedence.
    names: HashSet<String>,
    max_desc: usize,
}

/// `skillOverrides["<name>"]`: `on` / `name-only` / `user-invocable-only` / `off`.
fn override_of(ctx: &Ctx, name: &str) -> Option<(String, String)> {
    let pointer = format!("/skillOverrides/{}", esc(name));
    ctx.settings
        .get_str(&pointer)
        .map(|(v, f)| (v.to_string(), f.at(&pointer)))
}

#[allow(clippy::too_many_arguments)]
fn entry_item(
    ctx: &Ctx,
    kind: Kind,
    layer: Layer,
    name: &str,
    file: &Path,
    plugin: Option<&Plugin>,
    why_line: String,
    max_desc: usize,
) -> Option<Item> {
    let text = ctx.fs.read_string(file)?;
    let fm = frontmatter::parse(&text).unwrap_or_default();
    let real = ctx.fs.realpath(file);
    let mut item = new_item(kind, layer, name, name);
    item.origin.path = Some(real.clone());
    item.origin.why = match plugin {
        Some(p) => p.why_with(why_line),
        None => vec![why_line],
    };
    item.origin.plugin = plugin.map(|p| p.id.clone());
    let (bytes, estimate) = entry_bytes(name, &fm, max_desc);
    let overridable = matches!(layer, Layer::User | Layer::Project);
    let ovr = if overridable {
        override_of(ctx, name)
    } else {
        None
    };
    let dmi = frontmatter::get_bool(&fm, "disable-model-invocation");
    match ovr.as_ref().map(|(v, _)| v.as_str()) {
        Some("off") => return None,
        Some("user-invocable-only") => {
            let (_, origin) = ovr.clone().unwrap_or_default();
            item.mode = Mode::OnDemand;
            item.cost = cost_na(Some(bytes));
            item.origin.setting = Some(origin.clone());
            item.origin
                .why
                .push(format!("{origin} = {}", json_string("user-invocable-only")));
            item.origin
                .why
                .push("user-invocable-only: not in the listing".to_string());
        }
        Some("name-only") => {
            let (_, origin) = ovr.clone().unwrap_or_default();
            let n = format!("- {name}\n").len() as u64;
            item.mode = Mode::Advertised;
            item.cost = Cost {
                source: Some(bytes),
                predicted: Some(n),
                ..Cost::default()
            };
            item.origin.setting = Some(origin.clone());
            item.origin
                .why
                .push(format!("{origin} = {}", json_string("name-only")));
            item.origin
                .why
                .push("name-only: name without description".to_string());
        }
        _ if dmi => {
            item.mode = Mode::OnDemand;
            item.cost = cost_na(Some(bytes));
            item.origin
                .why
                .push("disable-model-invocation: true → not in the listing".to_string());
        }
        _ => {
            item.mode = Mode::Advertised;
            item.cost = Cost {
                source: Some(bytes),
                predicted: Some(bytes),
                predicted_is_estimate: estimate,
                ..Cost::default()
            };
        }
    }
    item.knobs = match (layer, plugin) {
        (_, Some(p)) => vec![p.contribution_knob()],
        (Layer::User | Layer::Project, None) => vec![setting_knob(
            ctx,
            Scope::Local,
            &format!("/skillOverrides/{}", esc(name)),
            json!("off"),
            None,
        )],
        _ => vec![Knob::None {
            reason: "no per-entry switch".to_string(),
        }],
    };
    Some(item)
}

impl Listing {
    fn push(&mut self, item: Option<Item>, name: &str) {
        if let Some(item) = item {
            self.names.insert(name.to_string());
            self.children.push(item);
        }
    }

    /// `<root>/*/SKILL.md` (a root that itself holds `SKILL.md` is one skill).
    fn skill_dirs(ctx: &Ctx, root: &Path) -> Vec<PathBuf> {
        if ctx.fs.is_file(&root.join("SKILL.md")) {
            return vec![root.to_path_buf()];
        }
        ctx.fs
            .read_dir(root)
            .into_iter()
            .filter(|d| !file_name(d).starts_with('.') && ctx.fs.is_file(&d.join("SKILL.md")))
            .collect()
    }

    /// The directory name: the 2.1.296 `init` event lists `stv:excalidraw-diagram-skill` for
    /// `skills/excalidraw-diagram-skill/SKILL.md` whose frontmatter says `name: excalidraw-diagram`.
    fn skill_name(dir: &Path) -> String {
        file_name(dir)
    }

    fn local_skills(&mut self, ctx: &Ctx, root: &Path, layer: Layer) {
        for dir in Self::skill_dirs(ctx, root) {
            if file_name(&dir) == "synced" {
                continue;
            }
            let name = Self::skill_name(&dir);
            if self.names.contains(&name) {
                continue;
            }
            let why = format!("{} ({} skill)", ctx.display(&dir.join("SKILL.md")), layer);
            let item = entry_item(
                ctx,
                Kind::Skill,
                layer,
                &name,
                &dir.join("SKILL.md"),
                None,
                why,
                self.max_desc,
            );
            self.push(item, &name);
        }
    }

    /// `*.md` under a commands dir; subdirectories namespace as `dir:name`.
    fn commands(&mut self, ctx: &Ctx, root: &Path, layer: Layer, plugin: Option<&Plugin>) {
        for file in ctx.fs.walk_md(root) {
            let Ok(rel) = file.strip_prefix(root) else {
                continue;
            };
            let base = rel
                .with_extension("")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(":");
            let name = match plugin {
                Some(p) => format!("{}:{base}", p.name),
                None => base,
            };
            // A skill wins over a command with the same name.
            if self.names.contains(&name) {
                continue;
            }
            let why = format!("{} ({layer} command)", ctx.display(&file));
            let item = entry_item(
                ctx,
                Kind::Command,
                layer,
                &name,
                &file,
                plugin,
                why,
                self.max_desc,
            );
            self.push(item, &name);
        }
    }
}

pub(crate) fn listing(ctx: &Ctx, plugins: &[Plugin]) -> Item {
    let max_desc = ctx
        .settings
        .get("/skillListingMaxDescChars")
        .and_then(|(v, _)| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(MAX_DESC);
    let mut l = Listing {
        children: Vec::new(),
        names: HashSet::new(),
        max_desc,
    };
    // Personal skills win over project skills; the closest project dir wins among project dirs.
    let user_root = ctx.home().join(".claude/skills");
    l.local_skills(ctx, &user_root, Layer::User);
    for dir in ctx.project_dirs() {
        l.local_skills(ctx, &dir.join(".claude/skills"), Layer::Project);
    }
    for p in plugins {
        for root in p.component_paths(ctx, "skills", "skills") {
            for dir in Listing::skill_dirs(ctx, &root) {
                let name = format!("{}:{}", p.name, Listing::skill_name(&dir));
                if l.names.contains(&name) {
                    continue;
                }
                let file = dir.join("SKILL.md");
                let why = format!("{}#skills", file.display());
                let item = entry_item(
                    ctx,
                    Kind::Skill,
                    p.layer,
                    &name,
                    &file,
                    Some(p),
                    why,
                    max_desc,
                );
                l.push(item, &name);
            }
        }
    }
    synced_skills(ctx, &mut l);
    bundled_skills(ctx, &mut l);
    l.commands(ctx, &ctx.home().join(".claude/commands"), Layer::User, None);
    for dir in ctx.project_dirs() {
        l.commands(ctx, &dir.join(".claude/commands"), Layer::Project, None);
    }
    for p in plugins {
        for root in p.component_paths(ctx, "commands", "commands") {
            l.commands(ctx, &root, p.layer, Some(p));
        }
    }
    for name in BUILTIN_COMMANDS {
        if l.names.contains(*name) {
            continue;
        }
        let mut item = new_item(Kind::Command, Layer::Builtin, name, name);
        item.mode = Mode::Advertised;
        item.origin.why = vec![
            "built-in command (static table, 2026-10-13 capture of Claude Code 2.1.296)"
                .to_string(),
        ];
        item.knobs = vec![Knob::None {
            reason: "built-in command".to_string(),
        }];
        l.push(Some(item), name);
    }

    let listed: Vec<&Item> = l
        .children
        .iter()
        .filter(|c| matches!(c.mode, Mode::Advertised))
        .collect();
    let unknown = listed.iter().filter(|c| c.cost.predicted.is_none()).count();
    let mut container = new_item(
        Kind::SkillListing,
        Layer::Builtin,
        "skills",
        &format!("skill listing · {}", listed.len()),
    );
    container.mode = Mode::Advertised;
    container.cost = Cost {
        source: Some(listed.iter().filter_map(|c| c.cost.source).sum()),
        predicted: Some(listed.iter().filter_map(|c| c.cost.predicted).sum()),
        predicted_is_estimate: listed.iter().any(|c| c.cost.predicted_is_estimate),
        ..Cost::default()
    };
    container.origin.why = vec![
        format!(
            "entries = `- name: description` (+ ` - when_to_use`), ≤ {max_desc} chars each; budget 1 % of the context window (tokens)"
        ),
        format!(
            "{} entries listed, {} of unknown size (built-in), {} not listed (disable-model-invocation / overrides)",
            listed.len(),
            unknown,
            l.children.len() - listed.len()
        ),
    ];
    container.knobs = vec![env_knob(
        "CLAUDE_CODE_DISABLE_ATTACHMENTS",
        "1",
        Scope::User,
        Some("drops the whole listing and other attachments; project/local settings cannot set it (2.1.290)"),
    )];
    container.children = l.children;
    container
}

/// `~/.claude/skills/synced/<bucket>/<name>/SKILL.md`, listed as `anthropic-skills:<name>` (the
/// 2.1.296 `init` event prefixes every synced skill, not only on collision).
fn synced_skills(ctx: &Ctx, l: &mut Listing) {
    if let Some((false, _)) = ctx.settings.get_bool("/syncClaudeAiSkills") {
        return;
    }
    let base = ctx.home().join(".claude/skills/synced");
    for bucket in ctx.fs.read_dir(&base) {
        if file_name(&bucket).starts_with('.') || !ctx.fs.is_dir(&bucket) {
            continue;
        }
        for dir in Listing::skill_dirs(ctx, &bucket) {
            let name = format!("anthropic-skills:{}", Listing::skill_name(&dir));
            if l.names.contains(&name) {
                continue;
            }
            let file = dir.join("SKILL.md");
            let why = format!("{} (synced from claude.ai)", ctx.display(&file));
            let item = entry_item(
                ctx,
                Kind::Skill,
                Layer::ClaudeAi,
                &name,
                &file,
                None,
                why,
                l.max_desc,
            )
            .map(|mut item| {
                item.knobs = vec![setting_knob(
                    ctx,
                    Scope::Local,
                    "/syncClaudeAiSkills",
                    json!(false),
                    Some("turns off every synced skill".to_string()),
                )];
                item
            });
            l.push(item, &name);
        }
    }
}

fn bundled_skills(ctx: &Ctx, l: &mut Listing) {
    if ctx
        .settings
        .get_bool("/disableBundledSkills")
        .is_some_and(|(b, _)| b)
        || ctx.env_on("CLAUDE_CODE_DISABLE_BUNDLED_SKILLS")
    {
        return;
    }
    for (name, listed) in BUNDLED {
        if l.names.contains(*name) {
            continue;
        }
        let mut item = new_item(Kind::Skill, Layer::Builtin, name, name);
        item.origin.why =
            vec!["bundled with Claude Code (static table, 2.1.296 init event)".to_string()];
        if *listed {
            item.mode = Mode::Advertised;
        } else {
            item.mode = Mode::OnDemand;
            item.cost = cost_na(None);
            item.origin
                .why
                .push("user-invocable only: not in the listing".to_string());
        }
        item.knobs = vec![
            setting_knob(
                ctx,
                Scope::Local,
                "/disableBundledSkills",
                json!(true),
                Some("turns off every bundled skill".to_string()),
            ),
            env_knob(
                "CLAUDE_CODE_DISABLE_BUNDLED_SKILLS",
                "1",
                Scope::Local,
                Some("turns off every bundled skill"),
            ),
        ];
        l.push(Some(item), name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fm(text: &str) -> serde_json::Map<String, Value> {
        frontmatter::parse(text).unwrap()
    }

    #[test]
    fn entry_joins_when_to_use_and_trims_description() {
        let m =
            fm("---\nname: s\ndescription: >\n  Does x.\nwhen_to_use: >\n  Triggers: y.\n---\n");
        // The folded `when_to_use` keeps its newline, then the entry's own newline.
        assert_eq!(
            entry_bytes("s", &m, MAX_DESC),
            ("- s: Does x. - Triggers: y.\n\n".len() as u64, false)
        );
    }

    #[test]
    fn entry_is_cut_in_utf16_units() {
        let long = "가".repeat(2000);
        let m = fm(&format!("---\ndescription: {long}\n---\n"));
        let (bytes, cut) = entry_bytes("k", &m, MAX_DESC);
        assert!(cut);
        assert_eq!(bytes, ("- k: ".len() + 1536 * "가".len() + 1) as u64);
    }

    #[test]
    fn entry_shows_a_differing_frontmatter_name_in_parentheses() {
        let m = fm("---\nname: excalidraw-diagram\ndescription: d\n---\n");
        assert_eq!(
            entry_bytes("stv:excalidraw-diagram-skill", &m, MAX_DESC),
            (
                "- stv:excalidraw-diagram-skill (stv:excalidraw-diagram): d\n".len() as u64,
                true
            )
        );
    }

    #[test]
    fn entry_without_description_is_name_only_estimate() {
        assert_eq!(
            entry_bytes("caveman", &serde_json::Map::new(), MAX_DESC),
            (10, true)
        );
    }
}
