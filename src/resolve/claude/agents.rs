//! The subagent roster (`research/01` §5): one `agent-roster` container (advertised) whose
//! children are project agents (`.claude/agents/**/*.md` in the target and each parent up to the
//! git root, closest wins), user agents (`~/.claude/agents/**`), enabled plugins' agents
//! (`<plugin>:<name>`) and the built-ins. A file without both `name:` and `description:`
//! frontmatter is not registered (`*-persona.md`, `CLAUDE.md`; on 2026-10-13 also two zbrain agents
//! without a description were absent from the `init` event).
//!
//! Roster line, as in the 2026-10-13 capture: `- <name>: <description> (Tools: <tools>)\n`,
//! tools = the frontmatter `tools` joined with `, `, or `All tools`. `cost.source` =
//! `cost.predicted` = the line's bytes including its newline (the request-body parser's entry
//! bounds). The container's id is `agents` (`oracle::claude::names::AGENT_ROSTER`). The body is the subagent's own system prompt and costs nothing here.

use super::frontmatter;
use super::fs::file_name;
use super::plugins::Plugin;
use super::{env_knob, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;

/// Built-in agents reported by the Claude Code 2.1.296 `init` event. Static table.
const BUILTIN: &[&str] = &[
    "general-purpose",
    "Explore",
    "Plan",
    "claude",
    "statusline-setup",
];

/// `(name, roster line bytes, estimate)`; None when the file does not register an agent. A
/// missing `name:` falls back to the file stem (zworkflow's `agents/explore.md` has no `name:` and
/// the 2.1.296 `init` event lists `zworkflow:explore`); a missing `description:` means no agent.
fn roster_entry(text: &str, stem: &str, prefix: &str) -> Option<(String, u64, bool)> {
    let fm = frontmatter::parse(text)?;
    let name = frontmatter::get_str(&fm, "name")
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| stem.to_string());
    let description = frontmatter::get_str(&fm, "description")?;
    if name.is_empty() || description.trim().is_empty() {
        return None;
    }
    let tools = frontmatter::get_list(&fm, "tools")
        .filter(|t| !t.is_empty())
        .map(|t| t.join(", "))
        .unwrap_or_else(|| "All tools".to_string());
    // `disallowedTools` changes the tools clause in a form not yet captured.
    let estimate = fm.contains_key("disallowedTools");
    // The description is used as parsed: a `|` block keeps its final newline before ` (Tools:`
    // (zbrain `strategist`, 2026-10-13 capture).
    let line = format!("- {prefix}{name}: {description} (Tools: {tools})\n");
    Some((name, line.len() as u64, estimate))
}

struct Roster {
    children: Vec<Item>,
    names: HashSet<String>,
}

impl Roster {
    fn add_dir(&mut self, ctx: &Ctx, root: &Path, layer: Layer, plugin: Option<&Plugin>) {
        for file in ctx.fs.walk_md(root) {
            let Some(text) = ctx.fs.read_string(&file) else {
                continue;
            };
            let stem = file_name(&file).trim_end_matches(".md").to_string();
            let prefix = plugin.map(|p| format!("{}:", p.name)).unwrap_or_default();
            let Some((base, bytes, estimate)) = roster_entry(&text, &stem, &prefix) else {
                continue;
            };
            let name = match plugin {
                Some(p) => format!("{}:{base}", p.name),
                None => base.clone(),
            };
            if self.names.contains(&name) {
                continue;
            }
            let mut item = new_item(Kind::SubAgent, layer, &name, &name);
            item.mode = Mode::Advertised;
            item.cost = Cost {
                source: Some(bytes),
                predicted: Some(bytes),
                predicted_is_estimate: estimate,
                ..Cost::default()
            };
            item.origin.path = Some(ctx.fs.realpath(&file));
            let line = format!("{} ({layer} agent)", ctx.display(&file));
            item.origin.why = match plugin {
                Some(p) => p.why_with(line),
                None => vec![line],
            };
            item.origin.plugin = plugin.map(|p| p.id.clone());
            item.knobs = match plugin {
                Some(p) => vec![p.contribution_knob()],
                None => vec![setting_knob(
                    ctx,
                    Scope::Local,
                    "/permissions/deny/-",
                    json!(format!("Agent({base})")),
                    Some(
                        "blocks use; roster effect unmeasured (spec Open question 2) — freed unknown"
                            .to_string(),
                    ),
                )],
            };
            self.names.insert(name);
            self.children.push(item);
        }
    }
}

pub(crate) fn roster(ctx: &Ctx, plugins: &[Plugin]) -> Item {
    let mut r = Roster {
        children: Vec::new(),
        names: HashSet::new(),
    };
    // Priority: project (closest first) → user → plugin; built-ins last.
    for dir in ctx.project_dirs() {
        r.add_dir(ctx, &dir.join(".claude/agents"), Layer::Project, None);
    }
    r.add_dir(ctx, &ctx.home().join(".claude/agents"), Layer::User, None);
    for p in plugins {
        for root in p.component_paths(ctx, "agents", "agents") {
            if ctx.fs.is_dir(&root) {
                r.add_dir(ctx, &root, p.layer, Some(p));
            } else if file_name(&root).ends_with(".md") {
                // A manifest may name a single agent file.
                if let Some(parent) = root.parent() {
                    let before = r.children.len();
                    r.add_dir(ctx, parent, p.layer, Some(p));
                    let keep = ctx.fs.realpath(&root);
                    let mut i = before;
                    while i < r.children.len() {
                        if r.children[i].origin.path.as_ref() != Some(&keep) {
                            let gone = r.children.remove(i);
                            r.names.remove(&gone.label);
                        } else {
                            i += 1;
                        }
                    }
                }
            }
        }
    }
    let explore_plan_off = ctx.env_on("CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS");
    for name in BUILTIN {
        if r.names.contains(*name) || (explore_plan_off && matches!(*name, "Explore" | "Plan")) {
            continue;
        }
        let mut item = new_item(Kind::SubAgent, Layer::Builtin, name, name);
        item.mode = Mode::Advertised;
        item.origin.why =
            vec!["built-in agent (static table, Claude Code 2.1.296 init event)".to_string()];
        item.knobs = match *name {
            "Explore" | "Plan" => vec![env_knob(
                "CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS",
                "1",
                Scope::Local,
                Some("turns off both Explore and Plan"),
            )],
            _ => vec![Knob::None {
                reason: "built-in".to_string(),
            }],
        };
        r.names.insert(name.to_string());
        r.children.push(item);
    }

    let mut container = new_item(
        Kind::AgentRoster,
        Layer::Builtin,
        "agents",
        &format!("agent roster · {}", r.children.len()),
    );
    container.mode = Mode::Advertised;
    container.cost = Cost {
        source: Some(r.children.iter().filter_map(|c| c.cost.source).sum()),
        predicted: Some(r.children.iter().filter_map(|c| c.cost.predicted).sum()),
        predicted_is_estimate: r.children.iter().any(|c| c.cost.predicted_is_estimate),
        ..Cost::default()
    };
    let unknown = r
        .children
        .iter()
        .filter(|c| c.cost.predicted.is_none())
        .count();
    container.origin.why =
        vec![
        "roster line = `- name: description (Tools: …)`; the agent body loads only when spawned"
            .to_string(),
        format!("{} agents, {unknown} of unknown size (built-in)", r.children.len()),
    ];
    container.knobs = vec![Knob::None {
        reason: "per agent: permissions.deny Agent(x) or the plugin's enabledPlugins".to_string(),
    }];
    container.children = r.children;
    container
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_line_keeps_a_block_description_newline() {
        let (_, bytes, _) =
            roster_entry("---\nname: s\ndescription: |\n  line\n---\n", "f", "").unwrap();
        assert_eq!(bytes, "- s: line\n (Tools: All tools)\n".len() as u64);
    }

    #[test]
    fn roster_line_uses_tools_or_all_tools() {
        let (name, bytes, est) = roster_entry(
            "---\nname: zhuge\ndescription: \"전략가\"\nmodel: opus\n---\nbody",
            "f",
            "",
        )
        .unwrap();
        assert_eq!(name, "zhuge");
        assert_eq!(bytes, "- zhuge: 전략가 (Tools: All tools)\n".len() as u64);
        assert!(!est);
        let (_, bytes, _) = roster_entry(
            "---\nname: s\ndescription: d\ntools: Read, Edit\n---\n",
            "f",
            "p:",
        )
        .unwrap();
        assert_eq!(bytes, "- p:s: d (Tools: Read, Edit)\n".len() as u64);
    }

    #[test]
    fn files_without_name_and_description_are_not_agents() {
        assert!(roster_entry("<!-- persona -->\nbody", "x-persona", "").is_none());
        assert!(roster_entry("---\nname: devops\nmodel: inherit\n---\n", "devops", "").is_none());
        let (name, _, _) = roster_entry("---\ndescription: d\n---\n", "explore", "").unwrap();
        assert_eq!(name, "explore");
    }
}
