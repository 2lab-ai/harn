//! Claude Code resolver against the synthetic `tests/fixtures/zbrain-like` fixture.
//!
//! The fixture is copied into a fresh temp dir per test so the resolve is hermetic: the real
//! ancestors of the harn checkout (and its git root) never enter the instruction chain. At copy
//! time `@HOME@` / `@PROJECT@` in every `.json` become the canonical temp paths, the
//! `PROJECT_SLUG` memory dir is renamed to the slug computed from the project path, and a `.git`
//! dir makes `project/` its own repo root.

use harn::model::{self, Item, Kind, Knob, Layer, Mode, Overlay, OverlayOp, Scope, Target, Tree};
use harn::resolve::{ClaudeCode, Resolver};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn slug(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let src = entry.path();
        if src.is_dir() {
            copy_dir(&src, &to.join(&name));
        } else {
            std::fs::copy(&src, to.join(&name)).unwrap();
        }
    }
}

fn substitute(dir: &Path, home: &Path, project: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            substitute(&path, home, project);
        } else if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).unwrap();
            let text = text
                .replace("@HOME@", &home.display().to_string())
                .replace("@PROJECT@", &project.display().to_string());
            std::fs::write(&path, text).unwrap();
        }
    }
}

fn fixture() -> Fixture {
    static N: AtomicUsize = AtomicUsize::new(0);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zbrain-like");
    let root = std::env::temp_dir().join(format!(
        "harn-resolve-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&src, &root);
    let root = std::fs::canonicalize(&root).unwrap();
    let home = root.join("home");
    let project = root.join("project");
    std::fs::create_dir(project.join(".git")).unwrap();
    std::fs::rename(
        home.join(".claude/projects/PROJECT_SLUG"),
        home.join(".claude/projects").join(slug(&project)),
    )
    .unwrap();
    substitute(&root, &home, &project);
    Fixture {
        root,
        home,
        project,
    }
}

fn target(f: &Fixture) -> Target {
    Target {
        dir: f.project.clone(),
        home: f.home.clone(),
        env: BTreeMap::new(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    }
}

fn resolve(f: &Fixture, overlay: Option<&Overlay>) -> Tree {
    ClaudeCode.resolve(&target(f), overlay).expect("resolve")
}

fn name(item: &Item) -> &str {
    item.id.splitn(4, '/').nth(3).unwrap()
}

fn of_kind(tree: &Tree, kind: Kind) -> Vec<&Item> {
    model::walk(tree).filter(|i| i.kind == kind).collect()
}

fn names(tree: &Tree, kind: Kind, layer: Layer) -> Vec<String> {
    let mut out: Vec<String> = of_kind(tree, kind)
        .into_iter()
        .filter(|i| i.origin.layer == layer)
        .map(|i| name(i).to_string())
        .collect();
    out.sort();
    out
}

fn find<'t>(tree: &'t Tree, kind: Kind, item_name: &str) -> &'t Item {
    of_kind(tree, kind)
        .into_iter()
        .find(|i| name(i) == item_name)
        .unwrap_or_else(|| panic!("no {kind} named {item_name}"))
}

/// `Contents of <path> (<desc>):` + 3 + body: the section the request-body parser measures.
fn section(path: &Path, desc: &str, body: usize) -> u64 {
    (format!("Contents of {} ({desc}):", path.display()).len() + 3 + body) as u64
}

const PROJECT_DESC: &str = "project instructions, checked into the codebase";

fn local_settings(f: &Fixture) -> PathBuf {
    f.project.join(".claude/settings.local.json")
}

#[test]
fn counts_per_kind_and_layer() {
    let f = fixture();
    let t = resolve(&f, None);
    assert_eq!(t.rules_verified_against, "2.1.296");
    assert_eq!(t.target_dir, f.project);

    assert_eq!(
        names(&t, Kind::Skill, Layer::User),
        ["user-one", "user-two"]
    );
    assert_eq!(
        names(&t, Kind::Skill, Layer::Project),
        ["proj-manual", "proj-one", "proj-two"]
    );
    assert_eq!(
        names(&t, Kind::Skill, Layer::Plugin),
        ["alpha:alpha-other", "alpha:alpha-skill"]
    );
    assert!(!names(&t, Kind::Skill, Layer::Builtin).is_empty());

    assert_eq!(
        names(&t, Kind::SubAgent, Layer::Project),
        ["proj-agent", "proj-other"]
    );
    assert_eq!(names(&t, Kind::SubAgent, Layer::User), ["helper"]);
    assert_eq!(
        names(&t, Kind::SubAgent, Layer::Plugin),
        ["beta:beta-agent", "beta:beta-reviewer"]
    );
    assert_eq!(
        names(&t, Kind::SubAgent, Layer::Builtin),
        [
            "Explore",
            "Plan",
            "claude",
            "general-purpose",
            "statusline-setup"
        ]
    );

    assert_eq!(
        names(&t, Kind::Plugin, Layer::Plugin),
        ["alpha@market-one", "beta@market-two"]
    );
    assert_eq!(names(&t, Kind::McpServer, Layer::User), ["user-server"]);
    assert_eq!(
        names(&t, Kind::McpServer, Layer::Project),
        ["project-server"]
    );
    assert_eq!(
        names(&t, Kind::McpServer, Layer::Plugin),
        ["plugin:alpha:alpha-server"]
    );

    let hooks = of_kind(&t, Kind::Hook);
    assert_eq!(hooks.len(), 3, "{hooks:#?}");
    let mode_of = |event: &str| {
        hooks
            .iter()
            .find(|h| h.id.contains(&format!("/hooks/{event}/")))
            .map(|h| h.mode)
    };
    assert_eq!(mode_of("SessionStart"), Some(Mode::Always));
    assert_eq!(mode_of("UserPromptSubmit"), Some(Mode::Always));
    assert_eq!(mode_of("PreToolUse"), Some(Mode::Runtime));

    assert_eq!(of_kind(&t, Kind::Memory).len(), 1);
    assert_eq!(of_kind(&t, Kind::StatusLine)[0].mode, Mode::None);
    assert_eq!(of_kind(&t, Kind::PermissionRule).len(), 1);
    assert_eq!(
        t.tool_search,
        Some(false),
        "ANTHROPIC_BASE_URL is localhost"
    );
    assert!(t.predicted_known > 0);
    assert!(t.unknown_rows > 0);
    assert!(t
        .warnings
        .iter()
        .any(|w| w.contains("token-denominated; cap state unknown")));
}

#[test]
fn memory_is_capped_at_200_lines() {
    let f = fixture();
    let t = resolve(&f, None);
    let memory = of_kind(&t, Kind::Memory)[0];
    let path = f
        .home
        .join(".claude/projects")
        .join(slug(&f.project))
        .join("memory/MEMORY.md");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.lines().count() > 200);
    let first_200: usize = text.split_inclusive('\n').take(200).map(str::len).sum();
    assert_eq!(memory.cost.source, Some(text.len() as u64));
    assert_eq!(
        memory.cost.predicted,
        Some(section(
            &path,
            "user's auto-memory, persists across conversations",
            first_200
        ))
    );
    assert!(!memory.cost.predicted_is_estimate, "line cap is exact");
    assert_eq!(memory.origin.path.as_deref(), Some(path.as_path()));
    assert_eq!(memory.mode, Mode::Always);
}

#[test]
fn auto_memory_off_removes_the_item() {
    let f = fixture();
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: f.home.join(".claude/settings.json"),
            pointer: "/autoMemoryEnabled".to_string(),
            value: serde_json::json!(false),
        }],
    };
    let t = resolve(&f, Some(&overlay));
    assert!(of_kind(&t, Kind::Memory).is_empty());
}

#[test]
fn import_chain_nests_and_skips_code_and_addresses() {
    let f = fixture();
    let t = resolve(&f, None);
    let claude_md = find(
        &t,
        Kind::Instruction,
        &f.project.join("CLAUDE.md").display().to_string(),
    );
    assert_eq!(claude_md.mode, Mode::Always);
    assert_eq!(claude_md.children.len(), 1, "{:#?}", claude_md.children);
    let a = &claude_md.children[0];
    assert_eq!(a.label, "rules/a.md");
    assert!(a.origin.why[0].starts_with("@rules/a.md in CLAUDE.md:3"));
    assert_eq!(a.children.len(), 1);
    let b = &a.children[0];
    assert_eq!(b.label, "rules/b.md");
    assert!(b.children.is_empty());
    let len = std::fs::metadata(f.project.join("rules/b.md"))
        .unwrap()
        .len();
    assert_eq!(b.cost.source, Some(len));
    assert_eq!(
        b.cost.predicted,
        Some(section(
            &f.project.join("rules/b.md"),
            PROJECT_DESC,
            len as usize
        ))
    );

    // Load order: user CLAUDE.md, then the project chain.
    let labels: Vec<&str> = t
        .items
        .iter()
        .filter(|i| i.kind == Kind::Instruction)
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "~/.claude/CLAUDE.md",
            "CLAUDE.md",
            ".claude/rules/scoped.md",
            ".claude/rules/style.md"
        ]
    );
    let scoped = find(
        &t,
        Kind::Instruction,
        &f.project
            .join(".claude/rules/scoped.md")
            .display()
            .to_string(),
    );
    assert_eq!(scoped.mode, Mode::OnDemand);
    assert!(scoped.cost.not_applicable);

    // Frontmatter and the blank line after it are not embedded.
    let style_path = f.project.join(".claude/rules/style.md");
    let style = find(&t, Kind::Instruction, &style_path.display().to_string());
    let on_disk = std::fs::read_to_string(&style_path).unwrap();
    let embedded = "# Style\n\nAlways-on project rule.\n";
    assert_eq!(style.mode, Mode::Always);
    assert_eq!(style.cost.source, Some(on_disk.len() as u64));
    assert_eq!(
        style.cost.predicted,
        Some(section(&style_path, PROJECT_DESC, embedded.len()))
    );
}

#[test]
fn excluding_claude_md_takes_its_imports_along() {
    let f = fixture();
    let before = resolve(&f, None);
    let claude_md = select(&before, &f.project.join("CLAUDE.md").display().to_string());
    let Knob::Setting {
        file,
        pointer,
        value,
        scope,
        ..
    } = claude_md.knobs[0].clone()
    else {
        panic!("claudeMdExcludes knob expected");
    };
    assert_eq!(file, local_settings(&f));
    assert_eq!(pointer, "/claudeMdExcludes/-");
    assert_eq!(scope, Scope::Local);
    let chain_bytes: u64 = model::walk(&before)
        .filter(|i| {
            i.origin.path.as_deref().is_some_and(|p| {
                p.starts_with(&f.project) && !p.starts_with(f.project.join(".claude"))
            }) && i.kind == Kind::Instruction
        })
        .map(|i| i.cost.predicted.unwrap())
        .sum();
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file,
            pointer,
            value,
        }],
    };
    let after = resolve(&f, Some(&overlay));
    assert!(model::walk(&after).all(|i| !i.label.starts_with("rules/") && i.label != "CLAUDE.md"));
    assert_eq!(before.predicted_known - after.predicted_known, chain_bytes);
}

#[test]
fn plugin_off_in_local_beats_on_in_user_and_project() {
    let f = fixture();
    let t = resolve(&f, None);
    assert!(model::walk(&t).all(|i| i.origin.plugin.as_deref() != Some("gamma@market-two")));
    assert!(model::walk(&t).all(|i| !i.id.contains("gamma")));
    let alpha = find(&t, Kind::Plugin, "alpha@market-one");
    assert!(alpha.cost.not_applicable);
    assert_eq!(alpha.mode, Mode::Always, "an MCP server and a prompt hook");
    assert_eq!(
        alpha.knobs[0],
        Knob::Setting {
            file: local_settings(&f),
            pointer: "/enabledPlugins/alpha@market-one".to_string(),
            value: serde_json::json!(false),
            scope: Scope::Local,
            note: None,
        }
    );
    assert!(alpha.origin.why[0].ends_with("#/enabledPlugins/alpha@market-one = true"));
    let beta = find(&t, Kind::Plugin, "beta@market-two");
    assert_eq!(beta.mode, Mode::Advertised, "agents only");
}

#[test]
fn disabled_mcp_server_is_present_with_mode_none() {
    let f = fixture();
    let t = resolve(&f, None);
    let user = find(&t, Kind::McpServer, "user-server");
    assert_eq!(user.mode, Mode::None);
    assert!(user.cost.not_applicable);
    assert!(user.children.is_empty());
    assert!(user
        .origin
        .why
        .iter()
        .any(|w| w.contains("disabled by") && w.contains("disabledMcpServers")));

    for server in ["project-server", "plugin:alpha:alpha-server"] {
        let s = find(&t, Kind::McpServer, server);
        assert_eq!(s.mode, Mode::Always, "{server}: tool search is off");
        assert_eq!(s.cost, model::Cost::default(), "{server}: schemas are `?`");
        assert_eq!(s.children.len(), 1);
        assert_eq!(s.children[0].kind, Kind::McpInstructions);
        // Same name as the server (the request-body parser's id); the kind keeps it distinct.
        assert_eq!(
            s.children[0].id,
            model::item_id(
                model::Agent::ClaudeCode,
                Kind::McpInstructions,
                s.origin.layer,
                server
            )
        );
    }
    let plugin_server = find(&t, Kind::McpServer, "plugin:alpha:alpha-server");
    assert_eq!(
        plugin_server.origin.plugin.as_deref(),
        Some("alpha@market-one")
    );
    let Knob::Setting { file, pointer, .. } = &plugin_server.knobs[1] else {
        panic!("disabledMcpServers knob expected");
    };
    assert_eq!(file, &f.home.join(".claude.json"));
    assert!(pointer.ends_with("/disabledMcpServers/-"), "{pointer}");
}

#[test]
fn overlay_plugin_off_removes_every_contribution() {
    let f = fixture();
    let before = resolve(&f, None);
    let alpha_rows = model::walk(&before)
        .filter(|i| i.origin.plugin.as_deref() == Some("alpha@market-one"))
        .count();
    assert_eq!(
        alpha_rows, 6,
        "plugin row + 2 skills + server + its instructions + hook"
    );
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: local_settings(&f),
            pointer: "/enabledPlugins/alpha@market-one".to_string(),
            value: serde_json::json!(false),
        }],
    };
    let after = resolve(&f, Some(&overlay));
    assert!(model::walk(&after).all(|i| i.origin.plugin.as_deref() != Some("alpha@market-one")));
    let alpha_skill_bytes: u64 = model::walk(&before)
        .filter(|i| i.kind == Kind::Skill && i.origin.plugin.as_deref() == Some("alpha@market-one"))
        .map(|i| i.cost.predicted.unwrap())
        .sum();
    assert_eq!(
        before.predicted_known - after.predicted_known,
        alpha_skill_bytes
    );
    // server + instructions + hook were `?` rows
    assert_eq!(before.unknown_rows - after.unknown_rows, 3);
}

#[test]
fn overlay_claude_json_knob_turns_off_the_server_only() {
    let f = fixture();
    let before = resolve(&f, None);
    let server = find(&before, Kind::McpServer, "plugin:alpha:alpha-server");
    let Knob::Setting {
        file,
        pointer,
        value,
        scope,
        note,
    } = server.knobs[1].clone()
    else {
        panic!("disabledMcpServers knob expected");
    };
    assert_eq!(scope, Scope::Local);
    assert!(note.unwrap().contains("rewritten by Claude Code"));
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file,
            pointer,
            value,
        }],
    };
    let after = resolve(&f, Some(&overlay));
    let server = find(&after, Kind::McpServer, "plugin:alpha:alpha-server");
    assert_eq!(server.mode, Mode::None);
    assert!(server.children.is_empty());
    // The plugin stays on: its skills keep their listing bytes.
    assert_eq!(
        names(&after, Kind::Skill, Layer::Plugin),
        ["alpha:alpha-other", "alpha:alpha-skill"]
    );
    assert_eq!(after.predicted_known, before.predicted_known);
    assert_eq!(
        before.unknown_rows - after.unknown_rows,
        2,
        "server + instructions"
    );
    // The user server disabled in the same list stays off.
    assert_eq!(
        find(&after, Kind::McpServer, "user-server").mode,
        Mode::None
    );
}

#[test]
fn overlay_remove_path_hides_a_skill_dir() {
    let f = fixture();
    let overlay = Overlay {
        ops: vec![OverlayOp::RemovePath {
            path: f.project.join(".claude/skills/proj-one"),
        }],
    };
    let t = resolve(&f, Some(&overlay));
    assert_eq!(
        names(&t, Kind::Skill, Layer::Project),
        ["proj-manual", "proj-two"]
    );
}

#[test]
fn skill_listing_entries_and_knobs() {
    let f = fixture();
    let t = resolve(&f, None);
    let listing = of_kind(&t, Kind::SkillListing)[0];
    assert_eq!(listing.mode, Mode::Advertised);
    assert_eq!(listing.signal, None);

    let one = find(&t, Kind::Skill, "user-one");
    let entry = "- user-one: Use when the user asks for the first synthetic user skill.\n";
    assert_eq!(one.cost.source, Some(entry.len() as u64));
    assert_eq!(one.cost.predicted, Some(entry.len() as u64));
    assert_eq!(
        one.knobs[0],
        Knob::Setting {
            file: local_settings(&f),
            pointer: "/skillOverrides/user-one".to_string(),
            value: serde_json::json!("off"),
            scope: Scope::Local,
            note: None,
        }
    );
    let two = find(&t, Kind::Skill, "user-two");
    let entry = "- user-two: Use when a synthetic task needs the second user skill, folded over two lines. - Triggers: \"second skill\", \"user two\".\n\n";
    assert_eq!(two.cost.predicted, Some(entry.len() as u64));

    let manual = find(&t, Kind::Skill, "proj-manual");
    assert_eq!(manual.mode, Mode::OnDemand, "disable-model-invocation");
    assert!(manual.cost.not_applicable);

    let plugin_skill = find(&t, Kind::Skill, "alpha:alpha-skill");
    let Knob::Setting { pointer, note, .. } = &plugin_skill.knobs[0] else {
        panic!("enabledPlugins knob expected");
    };
    assert_eq!(pointer, "/enabledPlugins/alpha@market-one");
    assert!(note.as_deref().unwrap().contains("whole plugin"));
    assert!(plugin_skill.origin.why[0].contains("enabledPlugins/alpha@market-one = true"));

    // Container = sum of the listed (advertised) children with a known size.
    let sum: u64 = listing
        .children
        .iter()
        .filter(|c| c.mode == Mode::Advertised)
        .filter_map(|c| c.cost.predicted)
        .sum();
    assert_eq!(listing.cost.predicted, Some(sum));

    // skillOverrides off (the knob) removes the entry.
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: local_settings(&f),
            pointer: "/skillOverrides/user-one".to_string(),
            value: serde_json::json!("off"),
        }],
    };
    let after = resolve(&f, Some(&overlay));
    assert_eq!(names(&after, Kind::Skill, Layer::User), ["user-two"]);
    assert_eq!(
        t.predicted_known - after.predicted_known,
        one.cost.predicted.unwrap()
    );
}

#[test]
fn agent_roster_lines() {
    let f = fixture();
    let t = resolve(&f, None);
    let helper = find(&t, Kind::SubAgent, "helper");
    let line =
        "- helper: Synthetic user agent that helps with fixture tasks. (Tools: Read, Grep)\n";
    assert_eq!(helper.cost.predicted, Some(line.len() as u64));
    let Knob::Setting { pointer, value, .. } = &helper.knobs[0] else {
        panic!("permissions.deny knob expected");
    };
    assert_eq!(pointer, "/permissions/deny/-");
    assert_eq!(value, &serde_json::json!("Agent(helper)"));

    let other = find(&t, Kind::SubAgent, "proj-other");
    let line = "- proj-other: Second synthetic project agent,\nwith a literal block description.\n (Tools: All tools)\n";
    assert_eq!(other.cost.predicted, Some(line.len() as u64));

    let reviewer = find(&t, Kind::SubAgent, "beta:beta-reviewer");
    assert_eq!(reviewer.origin.plugin.as_deref(), Some("beta@market-two"));
    let explore = find(&t, Kind::SubAgent, "Explore");
    assert_eq!(explore.cost, model::Cost::default());
    assert!(
        matches!(&explore.knobs[0], Knob::Env { name, .. } if name == "CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS")
    );
}

#[test]
fn ids_are_stable_across_resolves() {
    let f = fixture();
    let a: Vec<String> = model::walk(&resolve(&f, None))
        .map(|i| i.id.clone())
        .collect();
    let b: Vec<String> = model::walk(&resolve(&f, None))
        .map(|i| i.id.clone())
        .collect();
    assert_eq!(a, b);
    let mut unique = a.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), a.len(), "ids are unique");
}

fn select<'t>(tree: &'t Tree, selector: &str) -> &'t Item {
    model::select(tree, selector).unwrap_or_else(|e| panic!("{selector}: {e}"))
}

#[test]
fn select_finds_items_by_id_suffix_and_label() {
    let f = fixture();
    let t = resolve(&f, None);
    assert_eq!(
        select(&t, "plugin:alpha:alpha-server").kind,
        Kind::McpServer
    );
    assert_eq!(
        select(
            &t,
            "claude-code/mcp-server/plugin/plugin:alpha:alpha-server"
        )
        .kind,
        Kind::McpServer
    );
    assert_eq!(select(&t, "rules/a.md").kind, Kind::Instruction);
    assert_eq!(select(&t, "user-one").kind, Kind::Skill);
    assert_eq!(select(&t, "alpha@market-one").kind, Kind::Plugin);
    assert_eq!(select(&t, "beta:beta-agent").kind, Kind::SubAgent);
}

#[test]
fn builtin_blocks_and_tools() {
    let f = fixture();
    let t = resolve(&f, None);
    // Fixed ids the request-body parser builds (oracle::claude::names).
    for id in [
        "claude-code/system-prompt/builtin/system",
        "claude-code/system-prompt/builtin/system/billing-header",
        "claude-code/system-prompt/builtin/system/identity",
        "claude-code/system-prompt/builtin/system/harness-rules",
        "claude-code/env-block/builtin/environment",
        "claude-code/env-block/builtin/date",
        "claude-code/git-block/builtin/gitStatus",
        "claude-code/skill-listing/builtin/skills",
        "claude-code/agent-roster/builtin/agents",
        "claude-code/builtin-tool/builtin/Agent",
    ] {
        assert_eq!(select(&t, id).id, id);
    }
    let prompt = select(&t, "claude-code/system-prompt/builtin/system");
    assert!(matches!(prompt.knobs[0], Knob::None { .. }));
    assert_eq!(prompt.children.len(), 3);
    let git = of_kind(&t, Kind::GitBlock)[0];
    assert!(
        matches!(&git.knobs[0], Knob::Setting { pointer, .. } if pointer == "/includeGitInstructions")
    );
    let tools = of_kind(&t, Kind::BuiltinTool);
    assert!(tools.iter().any(|c| c.label == "Bash"));
    assert!(tools.iter().all(|c| c.cost == model::Cost::default()));
    assert!(
        tools.iter().all(|c| c.children.is_empty()),
        "no tools container"
    );

    let overlay = Overlay {
        ops: vec![
            OverlayOp::SetJson {
                file: local_settings(&f),
                pointer: "/permissions/deny/-".to_string(),
                value: serde_json::json!("WebSearch"),
            },
            OverlayOp::SetJson {
                file: local_settings(&f),
                pointer: "/includeGitInstructions".to_string(),
                value: serde_json::json!(false),
            },
        ],
    };
    let after = resolve(&f, Some(&overlay));
    assert!(of_kind(&after, Kind::BuiltinTool)
        .iter()
        .all(|c| c.label != "WebSearch"));
    assert!(of_kind(&after, Kind::GitBlock).is_empty());
}
