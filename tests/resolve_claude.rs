//! Claude Code resolver against the synthetic `tests/fixtures/zbrain-like` fixture (copied per
//! test by [`common::fixture`]).

mod common;

use common::{fixture, slug, target, Fixture};
use harn::model::{self, Item, Kind, Knob, Layer, Mode, Overlay, OverlayOp, Scope, Target, Tree};
use harn::plan;
use harn::resolve::claude::OFF_ALL_HOOKS;
use harn::resolve::{ClaudeCode, Resolver};
use std::path::{Path, PathBuf};

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
fn every_hook_and_the_status_line_stay_with_mode_none_once_all_hooks_are_off() {
    let f = fixture();
    let before = resolve(&f, None);
    let hooks = |t: &Tree| -> Vec<String> {
        let mut ids: Vec<String> = of_kind(t, Kind::Hook)
            .into_iter()
            .map(|i| i.id.clone())
            .collect();
        ids.sort();
        ids
    };
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: local_settings(&f),
            pointer: "/disableAllHooks".to_string(),
            value: serde_json::json!(true),
        }],
    };
    let after = resolve(&f, Some(&overlay));
    // The same hooks (settings and plugin ones), each off and saying why; none is in the request.
    assert_eq!(hooks(&after), hooks(&before));
    assert_eq!(hooks(&after).len(), 3, "2 in settings + the alpha plugin's");
    for hook in of_kind(&after, Kind::Hook) {
        assert_eq!(hook.mode, Mode::None, "{}", hook.id);
        assert!(hook.cost.not_applicable, "{}", hook.id);
        assert_eq!(hook.signal.as_deref(), Some(OFF_ALL_HOOKS), "{}", hook.id);
        assert!(
            hook.knobs.iter().any(
                |k| matches!(k, Knob::Setting { pointer, .. } if pointer == "/disableAllHooks")
            ),
            "{}: the switch that turned it off stays its knob",
            hook.id
        );
    }
    let status = of_kind(&after, Kind::StatusLine);
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].mode, Mode::None);
    assert_eq!(status[0].signal.as_deref(), Some(OFF_ALL_HOOKS));
    assert_eq!(of_kind(&before, Kind::StatusLine)[0].signal, None);
    // Nothing that was `?` before is still counted as unknown.
    assert_eq!(
        before.unknown_rows - after.unknown_rows,
        2,
        "SessionStart and the plugin's UserPromptSubmit"
    );
    assert_eq!(before.predicted_known, after.predicted_known);
}

/// The side effects naming `label` (one line per item that leaves).
fn effects_naming<'p>(plan: &'p plan::ActionPlan, label: &str) -> Vec<&'p String> {
    plan.side_effects
        .iter()
        .filter(|e| e.starts_with(label))
        .collect()
}

#[test]
fn turning_one_hook_off_plans_every_hook_and_the_status_line_as_leaving() {
    let f = fixture();
    let t = target(&f);
    let mut tree = resolve(&f, None);
    let hook = of_kind(&tree, Kind::Hook)
        .into_iter()
        .find(|h| h.label.starts_with("PreToolUse hook"))
        .unwrap()
        .id
        .clone();
    let plan = plan::build_plan(&ClaudeCode, &t, &tree, &hook, None).unwrap();
    // They stay in the after tree (mode none, `off:` signal): they leave all the same.
    let mut effects = plan.side_effects.clone();
    effects.sort();
    assert_eq!(effects.len(), 3, "{effects:#?}");
    for (label, kind) in [
        ("SessionStart hook", "(hook) leaves"),
        ("UserPromptSubmit hook", "(hook) leaves"),
        ("statusLine", "(status-line) leaves"),
    ] {
        let named = effects_naming(&plan, label);
        assert_eq!(named.len(), 1, "{label}: {effects:#?}");
        assert!(named[0].ends_with(kind), "{}", named[0]);
    }
    // Two of them printed into the request, unmeasured: what it frees is unknown.
    assert_eq!(plan.freed.known, 0);
    assert!(plan.freed.delta_unknown);
    // The per-row annotation sees the same.
    plan::annotate_freed(&ClaudeCode, &t, &mut tree).unwrap();
    let freed = model::walk(&tree)
        .find(|i| i.id == hook)
        .and_then(|i| i.freed.clone())
        .unwrap();
    assert!(freed.delta_unknown, "{freed:?}");
}

#[test]
fn a_plugin_hook_leaving_by_both_rules_is_one_side_effect() {
    // One plan turning the alpha plugin off (its hook leaves the tree) and every hook off (the
    // others stay with mode none).
    let f = fixture();
    let t = target(&f);
    let tree = resolve(&f, None);
    let hook = of_kind(&tree, Kind::Hook)
        .into_iter()
        .find(|h| h.label.starts_with("PreToolUse hook"))
        .unwrap()
        .id
        .clone();
    let op = |selector: &str| plan::PlanOp {
        selector: selector.to_string(),
        scope: None,
        knob: None,
        reason: "test".to_string(),
    };
    let req = plan::PlanRequest {
        schema: plan::PLAN_SCHEMA.to_string(),
        target_dir: f.project.clone(),
        ops: vec![op("alpha@market-one"), op(&hook)],
    };
    let plan = plan::build_plan_from_request(&ClaudeCode, &t, &tree, &req).unwrap();
    for label in ["UserPromptSubmit hook", "SessionStart hook", "statusLine"] {
        assert_eq!(
            effects_naming(&plan, label).len(),
            1,
            "{label}: {:#?}",
            plan.side_effects
        );
    }

    // Every hook already off (mode none): the plugin's hook goes from the tree with its plugin,
    // but it was not running, so nothing about it leaves.
    std::fs::write(
        local_settings(&f),
        serde_json::to_string_pretty(&{
            let mut v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(local_settings(&f)).unwrap())
                    .unwrap();
            v["disableAllHooks"] = serde_json::json!(true);
            v
        })
        .unwrap(),
    )
    .unwrap();
    let tree = resolve(&f, None);
    let off = find(&tree, Kind::Plugin, "alpha@market-one").id.clone();
    let plan = plan::build_plan(&ClaudeCode, &t, &tree, &off, None).unwrap();
    for label in ["UserPromptSubmit hook", "SessionStart hook", "statusLine"] {
        assert!(
            effects_naming(&plan, label).is_empty(),
            "{label}: {:#?}",
            plan.side_effects
        );
    }
    // Its skills and server still leave.
    assert!(!plan.side_effects.is_empty());
}

/// Items in the first request: mode always or advertised.
fn at_start(tree: &Tree) -> usize {
    model::walk(tree)
        .filter(|i| matches!(i.mode, Mode::Always | Mode::Advertised))
        .count()
}

#[test]
fn items_after_is_the_count_at_start_once_the_plan_is_applied() {
    let f = fixture();
    let t = target(&f);
    let tree = resolve(&f, None);
    let alpha = find(&tree, Kind::Plugin, "alpha@market-one").id.clone();
    // By hand: 83 at start, and alpha takes 6 with it — its plugin row, its 2 skills' listing
    // entries, its server and that server's instructions, its UserPromptSubmit hook.
    assert_eq!(at_start(&tree), 83);
    let alpha_rows: Vec<(Kind, Mode)> = model::walk(&tree)
        .filter(|i| i.id == alpha || i.origin.plugin.as_deref() == Some("alpha@market-one"))
        .map(|i| (i.kind, i.mode))
        .collect();
    assert_eq!(alpha_rows.len(), 6, "{alpha_rows:?}");
    assert!(alpha_rows
        .iter()
        .all(|(_, m)| matches!(m, Mode::Always | Mode::Advertised)));
    let plan = plan::build_plan(&ClaudeCode, &t, &tree, &alpha, None).unwrap();
    assert_eq!(plan.items_after, 77);

    // Every hook already off: the two that printed into the request are mode none, so 81 at
    // start; alpha then takes 5 (its hook was not counted). The hooks the switch keeps, with
    // mode none, never count.
    let mut local: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(local_settings(&f)).unwrap()).unwrap();
    local["disableAllHooks"] = serde_json::json!(true);
    std::fs::write(
        local_settings(&f),
        serde_json::to_string_pretty(&local).unwrap(),
    )
    .unwrap();
    let tree = resolve(&f, None);
    assert_eq!(at_start(&tree), 81);
    assert_eq!(of_kind(&tree, Kind::Hook).len(), 3, "kept, mode none");
    let plan = plan::build_plan(&ClaudeCode, &t, &tree, &alpha, None).unwrap();
    assert_eq!(plan.items_after, 76);
}

#[test]
fn leaving_is_the_plugin_and_every_row_it_brings_each_with_its_mode() {
    let f = fixture();
    let t = target(&f);
    let tree = resolve(&f, None);
    let alpha = find(&tree, Kind::Plugin, "alpha@market-one").id.clone();
    // The 6 rows `items_after_is_the_count_at_start_once_the_plan_is_applied` counts by hand:
    // the plugin row, its 2 skills' listing entries, its server and that server's
    // instructions, its UserPromptSubmit hook; in walk order, each with its mode now.
    let rows: Vec<(String, Kind, Mode)> = model::walk(&tree)
        .filter(|i| i.id == alpha || i.origin.plugin.as_deref() == Some("alpha@market-one"))
        .map(|i| (i.id.clone(), i.kind, i.mode))
        .collect();
    assert_eq!(rows.len(), 6, "{rows:?}");
    assert_eq!(rows.iter().filter(|(_, k, _)| *k == Kind::Skill).count(), 2);
    let plan = plan::build_plan(&ClaudeCode, &t, &tree, &alpha, None).unwrap();
    let leaving: Vec<(String, Kind, Mode)> = plan
        .leaving
        .iter()
        .map(|l| (l.id.clone(), l.kind, l.mode_before))
        .collect();
    assert_eq!(leaving, rows);
    // The plugin row is the planned item: on the list, not among the side effects.
    assert!(leaving.iter().any(|(id, ..)| *id == alpha));
    assert!(
        !plan.side_effects.iter().any(|e| e.contains("(plugin)")),
        "{:#?}",
        plan.side_effects
    );
}

#[test]
fn a_server_its_switch_disables_leaves_though_the_tree_keeps_it() {
    // `disabledMcpjsonServers`: the server stays listed (mode none, the reason in its origin, no
    // `off:` signal) and stops loading; its instructions go from the tree.
    let f = fixture();
    let t = target(&f);
    let tree = resolve(&f, None);
    let server = of_kind(&tree, Kind::McpServer)
        .into_iter()
        .find(|s| s.label == "project-server")
        .unwrap();
    assert!(matches!(server.mode, Mode::Always | Mode::Deferred));
    let plan = plan::build_plan(&ClaudeCode, &t, &tree, &server.id, None).unwrap();
    let after = resolve(
        &f,
        Some(&Overlay {
            ops: vec![match &plan.ops[0].knob {
                Knob::Setting {
                    file,
                    pointer,
                    value,
                    ..
                } => OverlayOp::SetJson {
                    file: file.clone(),
                    pointer: pointer.clone(),
                    value: value.clone(),
                },
                other => panic!("{other:?}"),
            }],
        }),
    );
    let kept = model::walk(&after).find(|i| i.id == server.id).unwrap();
    assert_eq!((kept.mode, kept.signal.as_deref()), (Mode::None, None));
    let leaving: Vec<(&str, Mode)> = plan
        .leaving
        .iter()
        .map(|l| (l.id.as_str(), l.mode_before))
        .collect();
    assert!(
        leaving.contains(&(server.id.as_str(), server.mode)),
        "{leaving:?}"
    );
    // What leaves at start is what the count after loses.
    let at_start_leaving = plan
        .leaving
        .iter()
        .filter(|l| matches!(l.mode_before, Mode::Always | Mode::Advertised))
        .count();
    assert_eq!(plan.items_after, at_start(&tree) - at_start_leaving);
}

/// The fixture with a value of every link-writing setting: every hook off, a memory dir whose
/// name has a space and parentheses (`~/memory (old)`), an output style and an advisor model
/// with them too. Returns that memory dir.
fn with_link_values(f: &Fixture) -> PathBuf {
    let dir = f.home.join("memory (old)");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MEMORY.md"), "- a note\n").unwrap();
    let mut local: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(local_settings(f)).unwrap()).unwrap();
    local["disableAllHooks"] = serde_json::json!(true);
    local["autoMemoryDirectory"] = serde_json::json!("~/memory (old)");
    local["outputStyle"] = serde_json::json!("My Style (x)");
    local["advisorModel"] = serde_json::json!("opus (fast)");
    local["skillOverrides"] = serde_json::json!({
        "user-one": "user-invocable-only",
        "user-two": "name-only",
    });
    std::fs::write(
        local_settings(f),
        serde_json::to_string_pretty(&local).unwrap(),
    )
    .unwrap();
    dir
}

/// A regression test on this fixture, not a guarantee about every producer: each link the
/// resolver writes here that carries a value (`<file>#<pointer> = <value>`) carries `true`,
/// `false` or a JSON string — never a bare word or path, never a container — so
/// `tui::model::split_link` reads it without guessing.
#[test]
fn every_link_value_in_the_fixture_is_a_boolean_or_a_json_string() {
    let f = fixture();
    let memory = with_link_values(&f);
    let t = resolve(&f, None);
    // What the scan covers: hooks off, plugins, memory, permission lists, the output style and
    // the advisor tool.
    for kind in [
        Kind::Hook,
        Kind::Plugin,
        Kind::Memory,
        Kind::PermissionRule,
        Kind::OutputStyle,
    ] {
        assert!(!of_kind(&t, kind).is_empty(), "{kind}");
    }
    let mut values = Vec::new();
    for item in model::walk(&t) {
        for why in &item.origin.why {
            // Links only: other steps are prose.
            let Some((_, rest)) = why.split_once('#') else {
                continue;
            };
            let (_, tail) = harn::tui::model::split_link(rest);
            if let Some(value) = tail.strip_prefix("= ") {
                assert!(
                    value == "true"
                        || value == "false"
                        || serde_json::from_str::<String>(value).is_ok(),
                    "{}: {why}",
                    item.id
                );
                values.push(value.to_string());
            }
        }
    }
    // The two skill visibility values, as the producer writes them (not through split_link).
    let local = local_settings(&f);
    for (skill, value) in [
        ("user-one", "user-invocable-only"),
        ("user-two", "name-only"),
    ] {
        let want = format!("{}#/skillOverrides/{skill} = \"{value}\"", local.display());
        let item = find(&t, Kind::Skill, skill);
        assert!(
            item.origin.why.contains(&want),
            "{skill}: {:#?}",
            item.origin.why
        );
    }
    for want in [
        serde_json::to_string(&memory.display().to_string()).unwrap(),
        "\"My Style (x)\"".to_string(),
        "\"opus (fast)\"".to_string(),
        "true".to_string(),
    ] {
        assert!(values.contains(&want), "{want}: {values:#?}");
    }
}

/// The fixture's links read the way `tui::model::split_link` expects: a link with no tail ends
/// in a fixed key or a list index; a user-chosen key (a plugin id, a server name, a skill name)
/// as the last token always carries a tail. A server named `srv (x)` keeps its name.
#[test]
fn pointer_only_links_end_in_a_fixed_key_or_an_index() {
    let f = fixture();
    with_link_values(&f);
    let mcp = f.project.join(".mcp.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
    json["mcpServers"]["srv (x)"] =
        serde_json::json!({ "type": "http", "url": "http://localhost:1/mcp" });
    std::fs::write(&mcp, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let mut local: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(local_settings(&f)).unwrap()).unwrap();
    local["enabledMcpjsonServers"] = serde_json::json!(["project-server", "srv (x)"]);
    std::fs::write(
        local_settings(&f),
        serde_json::to_string_pretty(&local).unwrap(),
    )
    .unwrap();
    let t = resolve(&f, None);

    // A JSON pointer token, unescaped.
    let token = |pointer: &str| -> (String, String) {
        let unescape = |s: &str| s.replace("~1", "/").replace("~0", "~");
        let mut parts = pointer.rsplit('/');
        let last = unescape(parts.next().unwrap_or_default());
        let parent = unescape(parts.next().unwrap_or_default());
        (parent, last)
    };
    let fixed = |key: &str| {
        !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    };
    let (mut bare, mut named) = (0, 0);
    for item in model::walk(&t) {
        for why in &item.origin.why {
            let Some((_, rest)) = why.split_once('#') else {
                continue;
            };
            let (pointer, tail) = harn::tui::model::split_link(rest);
            let (parent, last) = token(pointer);
            if tail.is_empty() {
                assert!(
                    fixed(&last),
                    "{}: a link ending in {last:?}: {why}",
                    item.id
                );
                bare += 1;
            }
            // A name the user chose (an index under a list is no name).
            let user_key = matches!(
                parent.as_str(),
                "mcpServers" | "enabledPlugins" | "skillOverrides" | "deniedMcpServers"
            ) && !last.chars().all(|c| c.is_ascii_digit());
            if user_key {
                assert!(
                    !tail.is_empty(),
                    "{}: {last:?} with no tail: {why}",
                    item.id
                );
                named += 1;
            }
        }
    }
    assert!(bare > 0 && named > 0, "{bare} bare, {named} named");

    // The server whose name has ` (x)`: its link keeps the name, its transport is the note.
    let server = find(&t, Kind::McpServer, "srv (x)");
    let link = server
        .origin
        .why
        .iter()
        .find(|w| w.contains("#/mcpServers/"))
        .unwrap();
    let (pointer, tail) = harn::tui::model::split_link(link.split_once('#').unwrap().1);
    assert_eq!(token(pointer).1, "srv (x)", "{link}");
    assert_eq!(tail, "(http)", "{link}");
    // A plugin id with `@` is a named key with its value.
    let alpha = find(&t, Kind::Plugin, "alpha@market-one");
    assert!(alpha.origin.why[0].ends_with("#/enabledPlugins/alpha@market-one = true"));
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

/// Claude Code without the re-resolve shortcut: every freed value comes from an overlay resolve.
struct ReResolveOnly;

impl Resolver for ReResolveOnly {
    fn agent(&self) -> model::Agent {
        ClaudeCode.agent()
    }

    fn rules_verified_against(&self) -> &'static str {
        ClaudeCode.rules_verified_against()
    }

    fn resolve(&self, target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree> {
        ClaudeCode.resolve(target, overlay)
    }
}

#[test]
fn freed_shortcut_equals_the_re_resolve_for_every_row() {
    let f = fixture();
    let t = target(&f);
    let tree = resolve(&f, None);

    // The shortcut applies to the bare-name tool denies, and its after-tree differs from the
    // re-resolved one only by rows with no first-request cost.
    let mut shortcuts = 0;
    for item in model::walk(&tree) {
        let Some(knob) = item.knobs.first() else {
            continue;
        };
        let Some(after) = ClaudeCode.after_removal(&tree, item, knob) else {
            continue;
        };
        shortcuts += 1;
        let overlay = match knob {
            Knob::Setting {
                file,
                pointer,
                value,
                ..
            } => Overlay {
                ops: vec![OverlayOp::SetJson {
                    file: file.clone(),
                    pointer: pointer.clone(),
                    value: value.clone(),
                }],
            },
            other => panic!("shortcut on a non-setting knob {other:?}"),
        };
        let real = resolve(&f, Some(&overlay));
        let ids = |tree: &Tree| -> Vec<String> {
            let mut ids: Vec<String> = model::walk(tree)
                .filter(|i| !i.cost.not_applicable)
                .map(|i| i.id.clone())
                .collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(&after), ids(&real), "{}", item.id);
        assert_eq!(after.predicted_known, real.predicted_known, "{}", item.id);
        assert_eq!(after.unknown_rows, real.unknown_rows, "{}", item.id);
    }
    assert!(shortcuts >= 20, "{shortcuts} shortcut rows");

    // Every row's freed is the same either way.
    let mut fast = tree.clone();
    let mut exact = tree;
    plan::annotate_freed(&ClaudeCode, &t, &mut fast).unwrap();
    plan::annotate_freed(&ReResolveOnly, &t, &mut exact).unwrap();
    let freed = |tree: &Tree| -> Vec<(String, Option<model::Freed>)> {
        model::walk(tree)
            .map(|i| (i.id.clone(), i.freed.clone()))
            .collect()
    };
    assert_eq!(freed(&fast), freed(&exact));
    assert!(model::walk(&fast).filter(|i| i.freed.is_some()).count() > shortcuts);
    assert_eq!(fast.warnings, exact.warnings);
}

#[test]
fn shortcut_refuses_every_knob_but_a_bare_tool_deny() {
    let f = fixture();
    let tree = resolve(&f, None);
    for item in model::walk(&tree) {
        for knob in &item.knobs {
            let shortcut = ClaudeCode.after_removal(&tree, item, knob).is_some();
            let bare_deny = item.kind == Kind::BuiltinTool
                && matches!(knob, Knob::Setting { pointer, .. } if pointer == "/permissions/deny/-");
            assert_eq!(shortcut, bare_deny, "{} {knob:?}", item.id);
        }
    }
}
