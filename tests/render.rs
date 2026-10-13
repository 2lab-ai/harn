//! Renderers on a synthetic tree (WU-C): sort order, cost/freed cells, the runtime block, bars,
//! drift, `brief` (schema, example op, every id, size), `html` (self-contained, read-only), `why`.

use harn::journal::{JournalEntry, Snapshot};
use harn::model::{
    item_id, walk, Agent, Cost, Document, Freed, Item, Kind, Knob, Layer, Mode, Origin, RunContext,
    Scope, Tree, SCHEMA,
};
use harn::plan::{ActionPlan, PlanRequest, PlannedOp, PLAN_SCHEMA};
use harn::render::{self, RenderOpts};
use harn::verify::{RowVerdict, VerifyReport};
use std::path::PathBuf;

const DIR: &str = "/Users/test/2lab.ai/zbrain";
const HOME: &str = "/Users/test";

// ---------- builders ----------

fn origin(layer: Layer) -> Origin {
    Origin {
        layer,
        path: None,
        setting: None,
        plugin: None,
        why: Vec::new(),
    }
}

fn node(kind: Kind, layer: Layer, name: &str, label: &str) -> Item {
    Item {
        id: item_id(Agent::ClaudeCode, kind, layer, name),
        label: label.to_string(),
        kind,
        origin: origin(layer),
        mode: Mode::Always,
        cost: Cost::default(),
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

fn pred(bytes: u64) -> Cost {
    Cost {
        predicted: Some(bytes),
        ..Cost::default()
    }
}

fn est(bytes: u64) -> Cost {
    Cost {
        predicted: Some(bytes),
        predicted_is_estimate: true,
        ..Cost::default()
    }
}

fn src_only(bytes: u64) -> Cost {
    Cost {
        source: Some(bytes),
        ..Cost::default()
    }
}

fn na() -> Cost {
    Cost {
        not_applicable: true,
        ..Cost::default()
    }
}

fn freed(known: u64, delta_unknown: bool) -> Option<Freed> {
    Some(Freed {
        known,
        delta_unknown,
        is_estimate: false,
    })
}

fn local_settings() -> PathBuf {
    PathBuf::from(format!("{DIR}/.claude/settings.local.json"))
}

fn setting(pointer: &str, value: serde_json::Value, scope: Scope) -> Knob {
    let file = match scope {
        Scope::Local => local_settings(),
        Scope::Project => PathBuf::from(format!("{DIR}/.claude/settings.json")),
        Scope::User => PathBuf::from(format!("{HOME}/.claude/settings.json")),
    };
    Knob::Setting {
        file,
        pointer: pointer.to_string(),
        value,
        scope,
        note: None,
    }
}

fn plugin_off(plugin: &str, scope: Scope) -> Knob {
    setting(
        &format!("/enabledPlugins/{plugin}"),
        serde_json::json!(false),
        scope,
    )
}

fn exclude(path: &str) -> Knob {
    setting("/claudeMdExcludes/-", serde_json::json!(path), Scope::Local)
}

// ---------- the synthetic tree ----------

const SLACK: &str = "plugin:slack:slack";
const SLACK_PLUGIN: &str = "slack@claude-plugins-official";

fn slack() -> Item {
    let mut it = node(Kind::McpServer, Layer::Plugin, SLACK, "mcp plugin:slack");
    it.origin.plugin = Some(SLACK_PLUGIN.to_string());
    it.origin.why = vec![
        format!("{HOME}/.claude/settings.json enabledPlugins[\"{SLACK_PLUGIN}\"] = true"),
        format!("{HOME}/.claude/plugins/installed_plugins.json {SLACK_PLUGIN} 1.4.1"),
        format!("{HOME}/.claude/plugins/cache/claude-plugins-official/slack/1.4.1/.mcp.json \"slack\" (stdio)"),
    ];
    it.cost = pred(77_536);
    it.knobs = vec![
        plugin_off(SLACK_PLUGIN, Scope::Local),
        plugin_off(SLACK_PLUGIN, Scope::User),
    ];
    it.freed = freed(77_536, true);
    it.last_used = Some("2026-10-10".to_string());
    for tool in ["slack_send_message", "slack_read_channel", "slack_search"] {
        let mut t = node(
            Kind::McpTool,
            Layer::Plugin,
            &format!("{SLACK}/{tool}"),
            &format!("tool {tool}"),
        );
        t.origin.plugin = Some(SLACK_PLUGIN.to_string());
        t.knobs = vec![plugin_off(SLACK_PLUGIN, Scope::Local)];
        it.children.push(t);
    }
    it
}

const ATLASSIAN_WHY: &str = "\"atlassian\" (http) https://example.invalid/mcp?a=1&b=2";

fn atlassian() -> Item {
    let mut it = node(
        Kind::McpServer,
        Layer::Plugin,
        "plugin:atlassian:atlassian",
        "mcp plugin:atlassian",
    );
    it.origin.plugin = Some("atlassian@claude-plugins-official".to_string());
    it.cost = pred(64_732);
    it.knobs = vec![plugin_off(
        "atlassian@claude-plugins-official",
        Scope::Local,
    )];
    it.freed = freed(67_328, false);
    // A URL in an admission step is text, not a resource.
    it.origin.why = vec![ATLASSIAN_WHY.to_string()];
    it
}

fn memory() -> Item {
    let path = format!("{HOME}/.claude/projects/-Users-test-2lab-ai-zbrain/memory/MEMORY.md");
    let mut it = node(Kind::Memory, Layer::User, &path, "MEMORY.md auto-memory");
    it.cost = pred(23_447);
    it.knobs = vec![setting(
        "/autoMemoryEnabled",
        serde_json::json!(false),
        Scope::Local,
    )];
    it.freed = freed(23_447, false);
    it
}

fn project_md() -> Item {
    let path = format!("{DIR}/CLAUDE.md");
    let mut it = node(
        Kind::Instruction,
        Layer::Project,
        &path,
        "CLAUDE.md project",
    );
    it.origin.path = Some(PathBuf::from(&path));
    it.origin.why = vec![path.clone()];
    it.cost = pred(6_835);
    it.knobs = vec![exclude(&path)];
    it.freed = freed(20_539, false);
    for (name, bytes) in [("PRINCIPLES.md", 4_261), ("DEV.md", 9_443)] {
        let child_path = format!("{DIR}/rules/{name}");
        let mut c = node(
            Kind::Instruction,
            Layer::Project,
            &child_path,
            &format!("rules/{name}"),
        );
        c.cost = pred(bytes);
        c.knobs = vec![exclude(&child_path)];
        c.freed = freed(bytes, false);
        c.origin.why = vec![path.clone(), format!("@rules/{name}")];
        it.children.push(c);
    }
    it
}

fn roster() -> Item {
    let mut it = node(Kind::AgentRoster, Layer::Builtin, "agents", "agent roster");
    it.mode = Mode::Advertised;
    it.cost = pred(17_050);
    it.knobs = vec![setting(
        "/permissions/deny/-",
        serde_json::json!("Agent(x)"),
        Scope::Local,
    )];
    // freed None: not computed, so the row sorts by its predicted bytes.
    it
}

const LISTING_CHILDREN: usize = 14;

fn listing() -> Item {
    let mut it = node(
        Kind::SkillListing,
        Layer::Builtin,
        "skills",
        "skill listing",
    );
    it.mode = Mode::Advertised;
    it.cost = est(31_212);
    it.knobs = vec![setting(
        "/skillOverrides",
        serde_json::json!({}),
        Scope::Local,
    )];
    it.freed = freed(0, true);
    it.signal = Some("over cap: 146 of 216 descriptions dropped".to_string());
    for i in 0..LISTING_CHILDREN {
        let name = format!("{HOME}/.claude/skills/skill-{i:02}");
        let mut c = node(Kind::Skill, Layer::User, &name, &format!("skill-{i:02}"));
        c.mode = Mode::Advertised;
        c.cost = pred(150 + i as u64);
        c.knobs = vec![setting(
            &format!("/skillOverrides/skill-{i:02}"),
            serde_json::json!("off"),
            Scope::Local,
        )];
        it.children.push(c);
    }
    it
}

fn hook_stdout() -> Item {
    let mut it = node(
        Kind::Hook,
        Layer::Plugin,
        "superpowers@claude-plugins-official/SessionStart",
        "SessionStart hook stdout",
    );
    it.origin.plugin = Some("superpowers@claude-plugins-official".to_string());
    it.knobs = vec![plugin_off(
        "superpowers@claude-plugins-official",
        Scope::Local,
    )];
    it
}

fn advisor() -> Item {
    let mut it = node(
        Kind::BuiltinTool,
        Layer::Builtin,
        "Advisor",
        "Advisor tool text",
    );
    it.cost = pred(2_018);
    it.knobs = vec![Knob::Env {
        name: "CLAUDE_CODE_DISABLE_ADVISOR_TOOL".to_string(),
        value: "1".to_string(),
        scope: Scope::Local,
        note: None,
    }];
    it.freed = freed(2_018, false);
    it
}

fn user_md() -> Item {
    let path = format!("{HOME}/.claude/CLAUDE.md");
    let mut it = node(Kind::Instruction, Layer::User, &path, "~/.claude/CLAUDE.md");
    it.cost = pred(738);
    it.knobs = vec![exclude(&path)];
    it.freed = freed(738, false);
    it
}

fn system_prompt() -> Item {
    let mut it = node(
        Kind::SystemPrompt,
        Layer::Builtin,
        "system",
        "system prompt",
    );
    it.cost = pred(6_137);
    it.knobs = vec![Knob::None {
        reason: "built-in".to_string(),
    }];
    it
}

fn nested_md() -> Item {
    let path = format!("{DIR}/books/CLAUDE.md");
    let mut it = node(Kind::Instruction, Layer::Project, &path, "books/CLAUDE.md");
    it.mode = Mode::OnDemand;
    it.cost = src_only(1_200);
    it.knobs = vec![exclude(&path)];
    it
}

fn hooks_runtime() -> Item {
    let mut it = node(
        Kind::Hook,
        Layer::User,
        "PreToolUse",
        "hooks PreToolUse runtime",
    );
    it.mode = Mode::Runtime;
    it.cost = na();
    it.knobs = vec![setting(
        "/disableAllHooks",
        serde_json::json!(true),
        Scope::Local,
    )];
    it
}

fn status_line() -> Item {
    let mut it = node(
        Kind::StatusLine,
        Layer::User,
        "statusLine",
        "statusLine none",
    );
    it.mode = Mode::None;
    it.cost = na();
    it
}

fn permissions() -> Item {
    let mut it = node(
        Kind::PermissionRule,
        Layer::Local,
        "permissions.allow",
        "permissions.allow none",
    );
    it.mode = Mode::None;
    it.cost = na();
    it.knobs = vec![Knob::None {
        reason: "no first-request cost".to_string(),
    }];
    it
}

/// Stored in a scrambled order so the test sees the renderer sort.
fn tree() -> Tree {
    Tree {
        agent: Agent::ClaudeCode,
        target_dir: PathBuf::from(DIR),
        agent_version: Some("2.1.300".to_string()),
        rules_verified_against: "2.1.296".to_string(),
        tool_search: Some(false),
        signals: vec!["skill listing: over cap, 146 of 216 descriptions dropped".to_string()],
        items: vec![
            user_md(),
            listing(),
            hook_stdout(),
            system_prompt(),
            hooks_runtime(),
            project_md(),
            status_line(),
            advisor(),
            atlassian(),
            nested_md(),
            memory(),
            permissions(),
            roster(),
            slack(),
        ],
        predicted_known: 229_705,
        unknown_rows: 4,
        observed_body: None,
        warnings: Vec::new(),
        home: None,
        observed_regions: Vec::new(),
        tool_search_cause: None,
    }
}

fn doc(trees: Vec<Tree>) -> Document {
    Document {
        schema: SCHEMA.to_string(),
        generated_at: "2026-10-13T04:05:06Z".to_string(),
        agents: trees,
    }
}

fn unicode() -> RenderOpts {
    RenderOpts {
        unicode: true,
        color: false,
        width: Some(160),
    }
}

fn ascii() -> RenderOpts {
    RenderOpts {
        unicode: false,
        color: false,
        width: Some(160),
    }
}

/// The line holding `needle`; panics with the whole text otherwise.
fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?} in:\n{text}"))
}

fn index_of(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?} in:\n{text}"))
}

// ---------- table ----------

const MAIN_ORDER: [&str; 11] = [
    "mcp plugin:slack",
    "mcp plugin:atlassian",
    "MEMORY.md auto-memory",
    "CLAUDE.md project",
    "agent roster",             // freed None → predicted 17,050
    "Advisor tool text",        // 2,018
    "~/.claude/CLAUDE.md",      // 738
    "skill listing · 14",       // 0 + Δ?
    "SessionStart hook stdout", // ? → 0, stored after the listing
    "system prompt",            // knob none → freed 0, although it predicts 6,137
    "books/CLAUDE.md",          // source only → 0
];
const TRAILING_ORDER: [&str; 3] = [
    "hooks PreToolUse runtime",
    "statusLine none",
    "permissions.allow none",
];

#[test]
fn table_sorts_by_freed_then_by_predicted_when_freed_is_not_computed() {
    let out = render::table(&tree(), &unicode());
    let positions: Vec<usize> = MAIN_ORDER.iter().map(|l| index_of(&out, l)).collect();
    let mut ascending = positions.clone();
    ascending.sort_unstable();
    assert_eq!(positions, ascending, "{out}");
    // Children follow their container, larger first, one level deeper.
    let project = index_of(&out, "CLAUDE.md project");
    assert_eq!(index_of(&out, "rules/DEV.md"), project + 1, "{out}");
    assert_eq!(index_of(&out, "rules/PRINCIPLES.md"), project + 2, "{out}");
    let parent_col = line_with(&out, "CLAUDE.md project").find("CLAUDE.md project");
    let child_col = line_with(&out, "rules/DEV.md").find("rules/DEV.md");
    assert_eq!(child_col, parent_col.map(|c| c + 2), "{out}");
}

#[test]
fn table_cells_carry_their_tags() {
    let out = render::table(&tree(), &unicode());
    let slack = line_with(&out, "mcp plugin:slack ");
    assert!(slack.contains("77.5 KB+Δ?"), "{slack}");
    assert!(
        slack.contains("23.3%") || slack.contains("33.8%"),
        "{slack}"
    );
    assert!(slack.contains("enabledPlugins → false (local)"), "{slack}");
    assert!(slack.contains(" plugin slack "), "{slack}");
    assert!(line_with(&out, "agent roster").contains("permissions.deny (local)"));

    let listing = line_with(&out, "skill listing · 14");
    assert!(listing.contains(" Δ? "), "{listing}");
    assert!(listing.contains("~31.2 KB"), "{listing}");
    assert!(listing.contains("[1]"), "{listing}");
    assert!(listing.contains("skillOverrides (local)"), "{listing}");

    let hook = line_with(&out, "SessionStart hook stdout");
    assert!(hook.contains(" ? "), "{hook}");

    let tool = line_with(&out, "tool slack_send_message");
    assert!(tool.contains(" ? "), "{tool}");

    let system = line_with(&out, "system prompt");
    assert!(system.trim_start().starts_with('—'), "{system}");
    assert!(system.contains("none (built-in)"), "{system}");

    let advisor = line_with(&out, "Advisor tool text");
    assert!(
        advisor.contains("CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1 (local)"),
        "{advisor}"
    );
    assert!(line_with(&out, "MEMORY.md").contains("autoMemoryEnabled → false (local)"));
    assert!(line_with(&out, "CLAUDE.md project").contains("claudeMdExcludes (local)"));
    assert!(line_with(&out, "books/CLAUDE.md").contains("src 1.2 KB"));

    // Freed not computed but a knob exists: blank freed cell, not `—`, not `?`.
    let roster = line_with(&out, "agent roster");
    assert!(roster.trim_start().starts_with("17.1 KB"), "{roster}");

    let runtime = line_with(&out, "hooks PreToolUse runtime");
    assert!(runtime.contains(" — "), "{runtime}");
    assert!(
        runtime.contains("disableAllHooks → true (local)"),
        "{runtime}"
    );
    let status = line_with(&out, "statusLine none");
    assert!(status.trim_end().ends_with('—'), "{status}");

    assert!(out.contains("  [1] skill listing: over cap: 146 of 216 descriptions dropped"));
}

#[test]
fn runtime_and_none_rows_form_a_trailing_block() {
    let out = render::table(&tree(), &unicode());
    let block = index_of(&out, "runtime / no first-request cost:");
    for label in MAIN_ORDER {
        assert!(
            index_of(&out, label) < block,
            "{label} after the block:\n{out}"
        );
    }
    let positions: Vec<usize> = TRAILING_ORDER.iter().map(|l| index_of(&out, l)).collect();
    assert!(positions.iter().all(|&p| p > block), "{out}");
    let mut ascending = positions.clone();
    ascending.sort_unstable();
    assert_eq!(positions, ascending, "{out}");
}

#[test]
fn big_containers_collapse_small_ones_expand() {
    let out = render::table(&tree(), &unicode());
    let listing_id = item_id(
        Agent::ClaudeCode,
        Kind::SkillListing,
        Layer::Builtin,
        "skills",
    );
    let more = format!("… {LISTING_CHILDREN} more (harn why {listing_id})");
    assert!(out.contains(&more), "{out}");
    assert_eq!(
        index_of(&out, &more),
        index_of(&out, "skill listing · 14") + 1
    );
    assert!(!out.contains("skill-00"), "{out}");
    // slack has 3 children (≤ 12): all shown.
    for tool in ["slack_send_message", "slack_read_channel", "slack_search"] {
        assert!(out.contains(&format!("tool {tool}")), "{out}");
    }
    assert!(line_with(&out, "mcp plugin:slack ").contains("mcp plugin:slack · 3"));
}

#[test]
fn header_names_target_versions_totals_tool_search_and_signals() {
    let out = render::table(&tree(), &unicode());
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        format!("harn {DIR}   claude-code 2.1.300 (rules verified 2.1.296)")
    );
    assert!(
        lines[1].starts_with("drift: installed claude-code 2.1.300"),
        "{out}"
    );
    assert_eq!(
        lines[2],
        "first request: predicted 229.7 KB + ? (4 rows unknown)   sort: freed ▼"
    );
    assert!(lines[3].starts_with("tool search: OFF (ANTHROPIC_BASE_URL is non-first-party)"));
    assert!(lines[4].contains("ENABLE_TOOL_SEARCH=true"), "{out}");
    assert_eq!(
        lines[5],
        "signal: skill listing: over cap, 146 of 216 descriptions dropped"
    );
    assert_eq!(lines[6], "");
    assert!(lines[7].trim_start().starts_with("freed(pred)"), "{out}");
    assert!(lines[7].contains("predicted"), "{out}");
}

#[test]
fn drift_line_only_when_versions_differ() {
    let mut t = tree();
    t.agent_version = Some("2.1.296".to_string());
    let out = render::table(&t, &unicode());
    assert!(!out.contains("drift"), "{out}");
    t.agent_version = None;
    let out = render::table(&t, &unicode());
    assert!(!out.contains("drift"), "{out}");
    assert!(out.starts_with(&format!("harn {DIR}   claude-code ? (rules verified")));
}

#[test]
fn observed_body_switches_the_totals_and_the_cost_column() {
    let mut t = tree();
    t.observed_body = Some(332_548);
    t.items[13].cost.observed = Some(77_536); // slack
    let out = render::table(&t, &unicode());
    assert!(
        out.contains("first request: observed 332.5 KB (bound capture)"),
        "{out}"
    );
    let header = line_with(&out, "freed(pred)");
    assert!(header.contains("observed"), "{header}");
    let slack = line_with(&out, "mcp plugin:slack ");
    assert!(
        slack.contains("23.3%"),
        "share over the observed body: {slack}"
    );
    // A row still predicted in an observed table says so.
    assert!(
        line_with(&out, "MEMORY.md").contains("23.4 KB pred"),
        "{out}"
    );
}

#[test]
fn observed_header_splits_the_body_and_tool_search_names_its_cause() {
    let mut t = tree();
    t.observed_body = Some(332_548);
    // A partition of the body, as `verify` copies it from the capture.
    t.observed_regions = vec![
        ("tools · builtin (26 tools)".to_string(), 200_000),
        ("messages[0] block 0 · /x/CLAUDE.md".to_string(), 131_000),
        ("request parameters".to_string(), 1_100),
        ("framing".to_string(), 448),
    ];
    t.tool_search_cause = Some("ANTHROPIC_BASE_URL is non-first-party (localhost)".to_string());
    let table = render::table(&t, &unicode());
    assert!(
        table.contains(
            "first request: observed 332.5 KB = attributed 331.0 KB + parameters 1.1 KB + framing 0.4 KB (bound capture)"
        ),
        "{table}"
    );
    assert!(
        table.contains("tool search: OFF (ANTHROPIC_BASE_URL is non-first-party (localhost)) →"),
        "{table}"
    );
    let brief = render::brief(&doc(vec![t.clone()]));
    assert!(
        brief.contains("observed 332548 = attributed 331000 + parameters 1100 + framing 448"),
        "{brief}"
    );
    let html = render::html(&doc(vec![t.clone()]));
    assert!(
        html.contains("= attributed 331.0 KB + parameters"),
        "{html}"
    );
    t.tool_search = Some(true);
    t.tool_search_cause = Some("ANTHROPIC_BASE_URL unset (first-party)".to_string());
    assert!(render::table(&t, &unicode())
        .contains("tool search: ON (ANTHROPIC_BASE_URL unset (first-party)) →"));
}

#[test]
fn html_footer_is_the_version_line_of_the_lib() {
    let line = harn::version_line();
    assert!(
        line.starts_with(&format!("harn {} (", env!("CARGO_PKG_VERSION"))),
        "{line}"
    );
    assert!(line.ends_with(") rules: claude-code 2.1.296"), "{line}");
    let html = render::html(&doc(vec![tree()]));
    assert!(html.contains(&format!("by {line}</footer>")), "{html}");
}

#[test]
fn ascii_table_is_pure_ascii_with_hash_bars() {
    let uni = render::table(&tree(), &unicode());
    assert!(uni.contains('█'), "{uni}");
    let out = render::table(&tree(), &ascii());
    assert!(out.is_ascii(), "{out}");
    assert!(out.contains("###"), "{out}");
    assert!(out.contains("77.5 KB+D?"), "{out}");
    assert!(out.contains("enabledPlugins -> false (local)"), "{out}");
    assert!(out.contains(&format!(
        "... {LISTING_CHILDREN} more (harn why claude-code/skill-listing/builtin/skills)"
    )));
    assert!(out.contains("sort: freed v"), "{out}");
}

#[test]
fn rows_fit_the_width_and_output_is_deterministic() {
    let opts = RenderOpts {
        unicode: true,
        color: false,
        width: Some(100),
    };
    let a = render::table(&tree(), &opts);
    let b = render::table(&tree(), &opts);
    assert_eq!(a, b);
    let header_end = index_of(&a, "freed(pred)");
    for line in a.lines().skip(header_end) {
        if line.contains("more (harn why") || line.trim_start().starts_with('[') {
            continue; // pointers and footnotes are never cut
        }
        assert!(line.chars().count() <= 100, "{line}");
    }
    assert!(a.contains('…'), "a 100-column table cuts long rows:\n{a}");
}

#[test]
fn color_only_when_asked() {
    let plain = render::table(&tree(), &unicode());
    assert!(!plain.contains('\x1b'));
    let colored = render::table(
        &tree(),
        &RenderOpts {
            unicode: true,
            color: true,
            width: None,
        },
    );
    assert!(colored.contains("\x1b[33mdrift"), "{colored}");
}

#[test]
fn small_table_golden() {
    let mut t = tree();
    t.items = vec![user_md(), atlassian()];
    t.predicted_known = 65_470;
    t.unknown_rows = 0;
    t.signals.clear();
    t.agent_version = Some("2.1.296".to_string());
    t.tool_search = Some(true);
    let out = render::table(&t, &unicode());
    let expected = [
        format!("harn {DIR}   claude-code 2.1.296 (rules verified 2.1.296)"),
        "first request: predicted 65.5 KB   sort: freed ▼".to_string(),
        "tool search: ON → MCP tool schemas deferred (stubs until used)".to_string(),
        String::new(),
        "  freed(pred) predicted  share            item                 origin           mode   default action".to_string(),
        "      67.3 KB   64.7 KB  98.9% █████████▉ mcp plugin:atlassian plugin atlassian always enabledPlugins → false (local)".to_string(),
        "       0.7 KB    0.7 KB   1.1% ▏          ~/.claude/CLAUDE.md  user             always claudeMdExcludes (local)".to_string(),
    ]
    .join("\n");
    assert_eq!(out, expected);
}

// ---------- why ----------

fn plan_for(item: &Item, knob: Knob, known: u64, warnings: Vec<String>) -> ActionPlan {
    ActionPlan {
        target_dir: PathBuf::from(DIR),
        agent: Agent::ClaudeCode,
        agent_version: None,
        ops: vec![PlannedOp {
            item_id: item.id.clone(),
            knob,
            file: Some(local_settings()),
            hash_before: Some("0123456789abcdef0123".to_string()),
            reason: None,
        }],
        predicted_before: 229_705,
        predicted_after: 229_705 - known,
        freed: Freed {
            known,
            delta_unknown: true,
            is_estimate: false,
        },
        side_effects: vec!["8 skills leave the listing".to_string()],
        warnings,
        diff: String::new(),
    }
}

#[test]
fn why_lists_every_plan_children_chain_and_warnings() {
    let t = tree();
    let item = slack();
    let plans = vec![
        plan_for(
            &item,
            plugin_off(SLACK_PLUGIN, Scope::Local),
            77_536,
            Vec::new(),
        ),
        plan_for(
            &item,
            plugin_off(SLACK_PLUGIN, Scope::User),
            77_536,
            vec!["edits every project".to_string()],
        ),
        plan_for(
            &item,
            setting(
                "/projects/x/disabledMcpServers/-",
                serde_json::json!(SLACK),
                Scope::User,
            ),
            70_000,
            vec!["~/.claude.json is also written by Claude Code".to_string()],
        ),
    ];
    let out = render::why(&t, &item, &plans, true);
    assert!(out.starts_with(&format!("mcp plugin:slack   ({})   target: {DIR}", item.id)));
    assert!(out.contains("predicted 77,536 B"), "{out}");
    assert!(out.contains("  why loaded\n"), "{out}");
    assert!(out.contains("    → "), "{out}");
    assert!(out.contains("installed_plugins.json"), "{out}");
    assert!(
        out.contains("also contributes  3 children: 3 mcp-tool"),
        "{out}"
    );
    for tool in ["slack_send_message", "slack_read_channel", "slack_search"] {
        assert!(out.contains(&format!("{SLACK}/{tool}")), "{out}");
    }
    assert!(out.contains("last used   2026-10-10"), "{out}");
    let actions: Vec<&str> = out.lines().filter(|l| l.starts_with("    [")).collect();
    assert_eq!(actions.len(), 3, "{out}");
    assert!(
        actions[0].contains(".claude/settings.local.json  /enabledPlugins/"),
        "{out}"
    );
    assert!(actions[0].contains("← default"), "{out}");
    assert!(
        actions[1].contains(&format!("{HOME}/.claude/settings.json")),
        "{out}"
    );
    assert!(!actions[1].contains("← default"), "{out}");
    assert!(actions[2].contains("disabledMcpServers"), "{out}");
    assert!(actions[2].contains("70.0 KB+Δ?"), "{out}");
    assert!(
        out.contains("side effect: 8 skills leave the listing"),
        "{out}"
    );
    assert!(out.contains("warnings  [2] edits every project"), "{out}");
    assert!(
        out.contains("[3] ~/.claude.json is also written by Claude Code"),
        "{out}"
    );
}

#[test]
fn no_unicode_switches_why_and_plan_decorations_and_keeps_user_text() {
    let t = tree();
    let mut item = slack();
    item.label = "mcp — slack…".to_string();
    let plans = [plan_for(&item, item.knobs[0].clone(), 77_536, Vec::new())];

    let ascii = render::why(&t, &item, &plans, false);
    assert!(ascii.starts_with("mcp — slack…   ("), "{ascii}");
    let rest = ascii.replace("mcp — slack…", "");
    assert!(rest.is_ascii(), "{rest}");
    assert!(rest.contains(" | mode ") && rest.contains("D?"), "{rest}");
    assert!(render::why(&t, &item, &plans, true).contains(" · mode "));

    let plan = render::plan(&plans[0], false);
    assert!(plan.is_ascii(), "{plan}");
    assert!(
        plan.contains(" -> ") && plan.contains(" | claude-code"),
        "{plan}"
    );
    assert!(render::plan(&plans[0], true).contains(" → "));
}

#[test]
fn why_without_plans_lists_the_knob_candidates() {
    let t = tree();
    let item = slack();
    let out = render::why(&t, &item, &[], true);
    let actions: Vec<&str> = out.lines().filter(|l| l.starts_with("    [")).collect();
    assert_eq!(actions.len(), 2, "{out}");
    assert!(actions[0].contains("freed 77.5 KB+Δ?"), "{out}");
    assert!(actions[1].contains("not planned"), "{out}");
    let builtin = render::why(&t, &system_prompt(), &[], true);
    assert!(builtin.contains("[1] none — built-in"), "{builtin}");
    let none = render::why(&t, &status_line(), &[], true);
    assert!(none.contains("actions     none (no knob"), "{none}");
    assert!(
        none.contains("why loaded  ? (no admission chain recorded)"),
        "{none}"
    );
}

// ---------- brief ----------

/// The pretty JSON after `example…:` up to `rules:`.
fn example_json(brief: &str) -> &str {
    let start = brief.find("\n{\n").expect("example object") + 1;
    let end = brief[start..]
        .find("\nrules:")
        .expect("rules after the example")
        + start;
    &brief[start..end]
}

#[test]
fn brief_has_every_id_the_schema_an_example_op_and_the_rules() {
    let t = tree();
    let out = render::brief(&doc(vec![t.clone()]));
    for item in walk(&t) {
        assert!(out.contains(&item.id), "missing {}", item.id);
    }
    assert!(
        out.contains(&format!("PLAN FORMAT ({PLAN_SCHEMA})")),
        "{out}"
    );
    let req: PlanRequest = serde_json::from_str(example_json(&out)).expect("example parses");
    assert_eq!(req.schema, PLAN_SCHEMA);
    assert_eq!(req.target_dir, PathBuf::from(DIR));
    assert_eq!(req.ops.len(), 1);
    // The first row in display order with an applicable knob: slack.
    assert_eq!(req.ops[0].selector, slack().id);
    assert_eq!(req.ops[0].knob, Some(0));
    assert_eq!(req.ops[0].scope, Some(Scope::Local));
    assert!(
        out.contains("- knob is an index into the item's knobs"),
        "{out}"
    );
    assert!(out.contains("- scope never wider than needed"), "{out}");
    assert!(out.contains("- one reason per op."), "{out}");
    assert!(out.find("UNKNOWNS:").unwrap() < out.find("PLAN FORMAT").unwrap());
}

#[test]
fn brief_rows_header_and_unknowns() {
    let out = render::brief(&doc(vec![tree()]));
    assert!(
        out.contains("== claude-code 2.1.300 (rules verified 2.1.296) · target"),
        "{out}"
    );
    assert!(
        out.contains("drift: installed claude-code 2.1.300"),
        "{out}"
    );
    assert!(
        out.contains("first request: predicted 229705 + ? (4 rows unknown)"),
        "{out}"
    );
    assert!(out.contains("tool search: OFF"), "{out}");
    assert!(out.contains("signal: skill listing: over cap"), "{out}");
    let slack_row = line_with(&out, &format!("{}|", slack().id));
    assert_eq!(
        slack_row,
        format!(
            "{}|plugin|always|77536|77536+Δ?|enabledPlugins → false +1|{HOME}/.claude/plugins/cache/claude-plugins-official/slack/1.4.1/.mcp.json \"slack\" (stdio)",
            slack().id
        )
    );
    // Children: one leading space per level; `^` = same as the line above.
    let tool_id = item_id(
        Agent::ClaudeCode,
        Kind::McpTool,
        Layer::Plugin,
        &format!("{SLACK}/slack_send_message"),
    );
    assert_eq!(
        line_with(&out, &tool_id),
        format!(" {tool_id}|^|^|?||enabledPlugins → false")
    );
    let next_tool = item_id(
        Agent::ClaudeCode,
        Kind::McpTool,
        Layer::Plugin,
        &format!("{SLACK}/slack_read_channel"),
    );
    assert_eq!(
        line_with(&out, &next_tool),
        format!(" {next_tool}|^|^|?||^")
    );
    // Children of a collapsed container are still listed in the brief.
    assert!(
        out.contains(&format!(" {}", listing().children[13].id)),
        "{out}"
    );
    let system = line_with(&out, &system_prompt().id);
    // Sorted after the hook stdout row above it (plugin, always).
    assert_eq!(
        system,
        format!("{}|builtin|^|6137|—|none (built-in)", system_prompt().id)
    );
    let listing_row = line_with(&out, &format!("{}|", listing().id));
    assert!(
        listing_row.ends_with("|~31212|Δ?|skillOverrides"),
        "{listing_row}"
    );
    assert!(line_with(&out, &nested_md().id).ends_with("|on-demand|s1200||claudeMdExcludes"));
    assert!(line_with(&out, &hooks_runtime().id).ends_with("|runtime|—||disableAllHooks → true"));
    // The why step is dropped when the id already names it, kept (relative) otherwise.
    assert!(line_with(&out, &format!("{}|", project_md().id)).ends_with("|claudeMdExcludes"));
    let dev = item_id(
        Agent::ClaudeCode,
        Kind::Instruction,
        Layer::Project,
        &format!("{DIR}/rules/DEV.md"),
    );
    // Same layer and mode as the line above; its own exclude entry is a different knob. A child
    // row carries no why step (top-level rows only).
    assert_eq!(
        line_with(&out, &dev),
        format!(" {dev}|^|^|9443|9443|claudeMdExcludes")
    );
    // Same compact text as slack's knob, another plugin's knob: no ditto.
    let atlassian_row = line_with(&out, &format!("{}|", atlassian().id));
    assert!(
        atlassian_row.ends_with(&format!(
            "|64732|67328|enabledPlugins → false|{ATLASSIAN_WHY}"
        )),
        "{atlassian_row}"
    );
    // A knob in another scope keeps its scope.
    let mut t = tree();
    t.items[0].knobs = vec![plugin_off("x@y", Scope::User)];
    let user = render::brief(&doc(vec![t]));
    assert!(line_with(&user, &user_md().id).contains("|enabledPlugins → false (user)"));

    let unknowns = &out[out.find("UNKNOWNS:").unwrap()..out.find("PLAN FORMAT").unwrap()];
    assert!(
        unknowns.contains(&format!("- {}: cost ?", hook_stdout().id)),
        "{unknowns}"
    );
    assert!(
        unknowns.contains(&format!("- {}: 3 of 3 children cost ?", slack().id)),
        "{unknowns}"
    );
    assert!(
        unknowns.contains(&format!("- {}: freed has Δ?", listing().id)),
        "{unknowns}"
    );
    assert!(unknowns.contains("freed not computed for"), "{unknowns}");
}

#[test]
fn brief_example_falls_back_when_nothing_is_actionable() {
    let mut t = tree();
    t.items = vec![system_prompt(), status_line()];
    let out = render::brief(&doc(vec![t]));
    assert!(
        out.contains("example (no item here has an applicable knob; shape only):"),
        "{out}"
    );
    let req: PlanRequest = serde_json::from_str(example_json(&out)).unwrap();
    assert_eq!(req.ops[0].selector, system_prompt().id);
}

const UHOME: &str = "/Users/zhugehyuk";
const CACHE: &str = "/Users/zhugehyuk/.claude/plugins/cache/claude-plugins-official";

/// A zbrain-sized tree (01-spec §Problem: 216 skills, 31 agents, 48 MCP tools, 26 built-in tools)
/// plus commands, hooks and permission rules: 400 rows. Instruction and memory files are named
/// by realpath (`.prd/02-architecture.md` §Item identity). Skills, agents and commands are named
/// by realpath when `realpath`, else by the name the `init` event reports
/// (`superpowers:brainstorming`); WU-B owns that choice.
fn big_tree(realpath: bool) -> Tree {
    let zbrain = format!("{UHOME}/2lab.ai/zbrain");
    let words = [
        "brainstorming",
        "writing-plans",
        "systematic-debugging",
        "test-driven-development",
        "verification",
        "code-review",
        "finishing-branch",
        "subagent-driven",
        "using-worktrees",
        "dispatching",
        "receiving-review",
        "executing-plans",
    ];
    let named = |path: String, name: String| if realpath { path } else { name };
    let skill = |layer: Layer, path: String, name: String, knob: Knob| {
        let mut s = node(
            Kind::Skill,
            layer,
            &named(path.clone(), name.clone()),
            &name,
        );
        s.mode = Mode::Advertised;
        s.cost = pred(140);
        s.origin.path = Some(PathBuf::from(format!("{path}/SKILL.md")));
        s.origin.why = vec![format!("{path}/SKILL.md")];
        s.knobs = vec![knob];
        s
    };
    let mut listing = listing();
    listing.children.clear();
    for i in 0..85 {
        let name = format!("{}-{i}", words[i % words.len()]);
        let knob = setting(
            &format!("/skillOverrides/{name}"),
            serde_json::json!("off"),
            Scope::Local,
        );
        let path = format!("{UHOME}/.claude/skills/{name}");
        listing.children.push(skill(Layer::User, path, name, knob));
    }
    let plugins = [
        "superpowers",
        "slack",
        "atlassian",
        "frontend-design",
        "pr-review-toolkit",
        "hookify",
    ];
    for i in 0..131 {
        let plugin = plugins[i % plugins.len()];
        let name = format!("{plugin}:{}-{i}", words[i % words.len()]);
        let path = format!(
            "{CACHE}/{plugin}/1.4.1/skills/{}-{i}",
            words[i % words.len()]
        );
        let knob = plugin_off(&format!("{plugin}@claude-plugins-official"), Scope::Local);
        let mut s = skill(Layer::Plugin, path, name, knob);
        s.origin.plugin = Some(format!("{plugin}@claude-plugins-official"));
        listing.children.push(s);
    }
    let mut roster = roster();
    for i in 0..31 {
        let name = format!("agent-{}-{i}", words[i % words.len()]);
        let path = format!("{zbrain}/.claude/agents/{name}.md");
        let mut a = node(
            Kind::SubAgent,
            Layer::Project,
            &named(path, name.clone()),
            &name,
        );
        a.mode = Mode::Advertised;
        a.cost = pred(550);
        a.knobs = vec![setting(
            "/permissions/deny/-",
            serde_json::json!(format!("Agent({name})")),
            Scope::Local,
        )];
        roster.children.push(a);
    }
    let mut items = vec![listing, roster];
    for (server, n) in [("slack", 27), ("atlassian", 21)] {
        let plugin = format!("{server}@claude-plugins-official");
        let mut m = node(
            Kind::McpServer,
            Layer::Plugin,
            &format!("plugin:{server}:{server}"),
            &format!("mcp plugin:{server}"),
        );
        m.origin.plugin = Some(plugin.clone());
        m.origin.why = vec![format!(
            "{CACHE}/{server}/1.4.1/.mcp.json \"{server}\" (stdio)"
        )];
        m.knobs = vec![
            plugin_off(&plugin, Scope::Local),
            plugin_off(&plugin, Scope::User),
        ];
        for i in 0..n {
            let mut t = node(
                Kind::McpTool,
                Layer::Plugin,
                &format!("plugin:{server}:{server}/{server}_tool_{i}"),
                &format!("{server}_tool_{i}"),
            );
            t.knobs = vec![plugin_off(&plugin, Scope::Local)];
            m.children.push(t);
        }
        items.push(m);
    }
    let mut builtins = node(Kind::BuiltinTool, Layer::Builtin, "tools", "built-in tools");
    for i in 0..26 {
        let mut b = node(
            Kind::BuiltinTool,
            Layer::Builtin,
            &format!("Tool{i}"),
            &format!("Tool{i}"),
        );
        b.cost = pred(2_600);
        b.knobs = vec![Knob::None {
            reason: "built-in".to_string(),
        }];
        builtins.children.push(b);
    }
    items.push(builtins);
    for i in 0..20 {
        let name = format!("cmd-{}-{i}", words[i % words.len()]);
        let path = format!("{UHOME}/.claude/commands/{name}.md");
        let mut c = node(
            Kind::Command,
            Layer::User,
            &named(path.clone(), name.clone()),
            &format!("/{name}"),
        );
        c.mode = Mode::Advertised;
        c.cost = pred(90);
        c.knobs = vec![Knob::Quarantine {
            from: PathBuf::from(&path),
            to: PathBuf::from(format!("{UHOME}/.claude/harn-off/commands/{name}.md")),
            scope: Scope::User,
        }];
        items.push(c);
    }
    let mut rules = node(
        Kind::PermissionRule,
        Layer::Local,
        "permissions.allow",
        "permissions.allow",
    );
    rules.mode = Mode::None;
    rules.cost = na();
    for i in 0..30 {
        let mut r = node(
            Kind::PermissionRule,
            Layer::Local,
            &format!("permissions.allow/Bash(git-{i}:*)"),
            &format!("Bash(git-{i}:*)"),
        );
        r.mode = Mode::None;
        r.cost = na();
        r.origin.why = vec![format!(
            "{zbrain}/.claude/settings.local.json permissions.allow[{i}]"
        )];
        rules.children.push(r);
    }
    items.push(rules);
    for i in 0..14 {
        let mut h = hooks_runtime();
        h.id = format!("{}-{i}", h.id);
        h.origin.why = vec![format!(
            "{UHOME}/.claude/settings.json hooks.PreToolUse[{i}]"
        )];
        items.push(h);
    }
    items.extend([
        memory(),
        project_md(),
        user_md(),
        system_prompt(),
        advisor(),
        status_line(),
        hook_stdout(),
    ]);
    Tree {
        target_dir: PathBuf::from(zbrain),
        items,
        ..tree()
    }
}

#[test]
fn brief_of_a_zbrain_sized_tree_stays_under_40_kb() {
    let t = big_tree(false);
    let rows = walk(&t).count();
    assert!(rows >= 400, "{rows} rows");
    let out = render::brief(&doc(vec![t]));
    assert!(
        out.len() <= 40_000,
        "brief is {} bytes for {rows} rows",
        out.len()
    );
}

/// The rows section of a brief: every line between `rows:` and the blank line after the rows.
fn brief_rows(brief: &str) -> Vec<&str> {
    brief
        .lines()
        .skip_while(|l| *l != "rows:")
        .skip(1)
        .take_while(|l| !l.is_empty())
        .collect()
}

/// With realpath ids for skills/agents/commands the ids are most of a 400-row brief; what the
/// format adds on top of the printed ids stays under 45 B per printed row (legend, headers,
/// unknowns and the plan format included).
#[test]
fn brief_overhead_beyond_the_ids_is_bounded() {
    let t = big_tree(true);
    let out = render::brief(&doc(vec![t]));
    let rows = brief_rows(&out);
    let ids: usize = rows
        .iter()
        .map(|l| l.trim_start().split('|').next().unwrap_or_default().len())
        .sum();
    let overhead = out.len() - ids;
    assert!(
        overhead <= 45 * rows.len(),
        "brief {} B = ids {ids} B + format {overhead} B for {} printed rows",
        out.len(),
        rows.len()
    );
}

#[test]
fn brief_collapses_children_past_40_prints_home_as_tilde_and_keeps_every_top_level_id() {
    let mut t = big_tree(true);
    t.home = Some(PathBuf::from(UHOME));
    let listing = t
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .expect("big tree listing");
    let total = listing.children.len();
    assert!(total > 40, "{total}");
    for child in &mut listing.children {
        child.freed = freed(10, true);
        child.origin.why = vec![format!("{UHOME}/.claude/skills/x/SKILL.md (user skill)")];
    }
    listing.origin.why = vec![format!("{UHOME}/.claude/skills (user skills)")];
    let listing_id = listing.id.clone();
    let out = render::brief(&doc(vec![t.clone()]));
    let rows = brief_rows(&out);

    // Every top-level id, verbatim.
    for item in &t.items {
        assert!(
            rows.iter()
                .any(|l| l.split('|').next() == Some(item.id.as_str())),
            "missing top-level {}",
            item.id
        );
    }
    // 40 children printed, the rest counted on one line right after them.
    let at = rows
        .iter()
        .position(|l| l.starts_with(&format!("{listing_id}|")))
        .unwrap();
    let children: Vec<&&str> = rows[at + 1..]
        .iter()
        .take_while(|l| l.starts_with(' '))
        .collect();
    assert_eq!(children.len(), 41, "{children:#?}");
    assert_eq!(
        children[40].to_string(),
        format!(" … {} more (harn --json)", total - 40)
    );
    // The why step: on the top-level row (home → `~`), never on a child row.
    assert!(
        rows[at].ends_with("|~/.claude/skills (user skills)"),
        "{}",
        rows[at]
    );
    for child in &children[..40] {
        assert_eq!(child.split('|').count(), 6, "{child}");
    }
    // The row after the collapse line repeats nothing from a hidden row.
    let next = rows[at + 42];
    assert!(!next.starts_with(' ') && !next.contains("|^|"), "{next}");
    // Unknowns count the children's Δ? on the parent; both closing sections stay.
    let unknowns = &out[out.find("UNKNOWNS:").unwrap()..out.find("PLAN FORMAT").unwrap()];
    assert!(
        unknowns.contains(&format!(
            "- {listing_id}: freed has Δ? on {total} of {total} children"
        )),
        "{unknowns}"
    );
    assert!(unknowns.contains("- claude-code: Δ? = "), "{unknowns}");
    assert!(
        out.contains("= children beyond the first 40, in the same order."),
        "the legend names the collapse threshold"
    );
    assert!(out.contains("PLAN FORMAT (harn-plan/1)"));
}

// ---------- html ----------

#[test]
fn html_is_one_self_contained_read_only_page() {
    let t = tree();
    let out = render::html(&doc(vec![t.clone()]));
    assert!(out.starts_with("<!doctype html>\n"), "{out}");
    assert!(out.ends_with("</html>"));
    assert!(
        out.contains("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">")
    );
    assert!(out.contains("<style>"));
    assert!(out.contains("@media (max-width:600px)"));
    for forbidden in [
        "<script",
        "src=",
        "href=",
        "url(",
        "@import",
        "<form",
        "<input",
        "<button",
        "<textarea",
        "<select",
        "contenteditable",
        "<link",
        "<iframe",
        "<img",
    ] {
        assert!(!out.contains(forbidden), "html contains {forbidden:?}");
    }
    // Admission steps are shown as escaped text; a URL there is not a resource.
    assert!(
        out.contains("&quot;atlassian&quot; (http) https://example.invalid/mcp?a=1&amp;b=2"),
        "{out}"
    );
    assert!(!out.contains("mcp?a=1&b=2"), "unescaped &");
    for item in walk(&t) {
        assert!(
            out.contains(&format!("<code class=\"id\">{}</code>", item.id)),
            "missing {}",
            item.id
        );
    }
    assert!(out.contains("first request: predicted 229.7 KB + ? (4 rows unknown)"));
    assert!(out.contains("signal: skill listing: over cap, 146 of 216 descriptions dropped"));
    assert!(out.contains("drift: installed claude-code 2.1.300"));
    assert!(out.contains("<h3>runtime / no first-request cost</h3>"));
    assert!(
        out.contains("<i style=\"width:33.8%\"></i>"),
        "slack bar as a CSS width"
    );
    let footer = line_with(&out, "<footer>");
    assert!(
        footer.contains("generated 2026-10-13T04:05:06Z by harn "),
        "{footer}"
    );
    assert!(footer.contains("rules: claude-code 2.1.296"), "{footer}");
}

#[test]
fn html_escapes_data() {
    let mut t = tree();
    t.items[0].label = "<b>&\"x\"</b>".to_string();
    let out = render::html(&doc(vec![t]));
    assert!(
        out.contains("&lt;b&gt;&amp;&quot;x&quot;&lt;/b&gt;"),
        "{out}"
    );
    assert!(!out.contains("<b>&"), "{out}");
}

// ---------- verify / plan / history ----------

fn context() -> RunContext {
    RunContext {
        cwd: PathBuf::from(DIR),
        agent: Agent::ClaudeCode,
        agent_version: Some("2.1.296".to_string()),
        model: Some("claude-opus".to_string()),
        launch_opts: Vec::new(),
        settings_fingerprint: None,
        capture_kind: "otel".to_string(),
        captured_at: "2026-10-13T01:00:00Z".to_string(),
    }
}

#[test]
fn verify_report_groups_rows_by_verdict() {
    let report = VerifyReport {
        context: context(),
        rows: vec![
            ("a".to_string(), RowVerdict::Seen),
            (
                "b".to_string(),
                RowVerdict::ContentMismatch {
                    predicted: 6_835,
                    observed: 6_900,
                },
            ),
            ("c".to_string(), RowVerdict::PredictedNotSeen),
            ("d".to_string(), RowVerdict::Seen),
        ],
        passed: false,
        promoted: 2,
    };
    let out = render::verify_report(&report);
    let expected = [
        format!(
            "verify claude-code 2.1.296 · model claude-opus · in {DIR} · otel 2026-10-13T01:00:00Z"
        ),
        "seen (2)".to_string(),
        "  a".to_string(),
        "  d".to_string(),
        "predicted-not-seen (1)".to_string(),
        "  c".to_string(),
        "seen-not-predicted (0)".to_string(),
        "content-mismatch (1)".to_string(),
        "  b   predicted 6,835 B · observed 6,900 B".to_string(),
        "passed: no".to_string(),
        "promoted: 2 row(s) predicted → observed".to_string(),
    ]
    .join("\n");
    assert_eq!(out, expected);
}

#[test]
fn plan_shows_ops_numbers_side_effects_warnings_then_the_diff() {
    let item = slack();
    let mut p = plan_for(
        &item,
        plugin_off(SLACK_PLUGIN, Scope::Local),
        77_536,
        vec!["w1".to_string()],
    );
    p.ops[0].reason = Some("unused in this repo".to_string());
    p.diff = "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n".to_string();
    let out = render::plan(&p, true);
    assert!(out.starts_with(&format!(
        "plan for {DIR} · claude-code\nops (1)\n  [1] {}",
        item.id
    )));
    assert!(out.contains("      knob    .claude/settings.local.json  /enabledPlugins/slack@claude-plugins-official = false  (local)"), "{out}");
    assert!(out.contains("(sha256 0123456789ab)"), "{out}");
    assert!(out.contains("      reason  unused in this repo"), "{out}");
    assert!(
        out.contains("predicted  229,705 B → 152,169 B   freed 77.5 KB+Δ?"),
        "{out}"
    );
    assert!(
        out.contains("side effects\n  8 skills leave the listing"),
        "{out}"
    );
    assert!(out.contains("warnings\n  w1"), "{out}");
    assert!(
        out.ends_with("diff\n--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y"),
        "{out}"
    );
}

#[test]
fn history_is_newest_first_with_context_and_reasons() {
    let entry = |id: &str, at: &str, reason: Option<&str>| JournalEntry {
        id: id.to_string(),
        context: context(),
        ops: vec![PlannedOp {
            item_id: slack().id,
            knob: plugin_off(SLACK_PLUGIN, Scope::Local),
            file: None,
            hash_before: None,
            reason: reason.map(str::to_string),
        }],
        snapshots: vec![Snapshot {
            path: local_settings(),
            file: PathBuf::from("/state/x"),
            hash_before: "h".to_string(),
            hash_after: None,
        }],
        applied_at: at.to_string(),
        warnings: vec![],
        undone_at: None,
        diff: None,
    };
    let out = render::history(&[
        entry("1-old", "2026-10-12T00:00:00Z", None),
        entry("2-new", "2026-10-13T00:00:00Z", Some("not used here")),
    ]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(
        lines[0],
        format!(
            "2-new  2026-10-13T00:00:00Z  claude-code 2.1.296 · {DIR}  1 op: {} (not used here)",
            slack().id
        )
    );
    assert!(lines[1].starts_with("1-old  2026-10-12T00:00:00Z"), "{out}");
    assert_eq!(render::history(&[]), "no journal entries");
}

#[test]
fn history_marks_undone_entries_on_their_line_only() {
    let entry = |id: &str, at: &str, undone_at: Option<&str>| JournalEntry {
        id: id.to_string(),
        context: context(),
        ops: vec![PlannedOp {
            item_id: slack().id,
            knob: plugin_off(SLACK_PLUGIN, Scope::Local),
            file: None,
            hash_before: None,
            reason: None,
        }],
        snapshots: vec![],
        applied_at: at.to_string(),
        warnings: vec![],
        undone_at: undone_at.map(str::to_string),
        diff: None,
    };
    let out = render::history(&[
        entry("1-live", "2026-10-12T00:00:00Z", None),
        entry(
            "2-undone",
            "2026-10-13T00:00:00Z",
            Some("2026-10-13T00:01:00Z"),
        ),
    ]);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert_eq!(
        lines[0],
        format!(
            "2-undone  2026-10-13T00:00:00Z  claude-code 2.1.296 · {DIR}  1 op: {}  undone 2026-10-13T00:01:00Z",
            slack().id
        )
    );
    assert_eq!(
        lines[1],
        format!(
            "1-live  2026-10-12T00:00:00Z  claude-code 2.1.296 · {DIR}  1 op: {}",
            slack().id
        )
    );
}

#[test]
fn renderers_end_without_a_newline() {
    let t = tree();
    let d = doc(vec![t.clone()]);
    let item = slack();
    let mut p = plan_for(&item, plugin_off(SLACK_PLUGIN, Scope::Local), 1, Vec::new());
    p.diff = "--- a\n+++ b\n".to_string();
    for out in [
        render::table(&t, &unicode()),
        render::table(&t, &ascii()),
        render::why(&t, &item, &[], true),
        render::brief(&d),
        render::html(&d),
        render::plan(&p, true),
        render::history(&[]),
    ] {
        assert!(!out.ends_with('\n'), "{out}");
    }
}

/// `cargo test --test render -- --nocapture print_samples` shows the table and the brief.
#[test]
fn print_samples() {
    let t = tree();
    println!("{}", render::table(&t, &unicode()));
    println!("-----");
    let brief = render::brief(&doc(vec![t]));
    for line in brief.lines().take(20) {
        println!("{line}");
    }
    println!("-----");
    for realpath in [false, true] {
        let t = big_tree(realpath);
        let ids: usize = walk(&t).map(|item| item.id.len()).sum();
        let big = render::brief(&doc(vec![t]));
        println!(
            "big brief (realpath ids: {realpath}): {} bytes, ids {ids} bytes",
            big.len()
        );
    }
    let big = render::brief(&doc(vec![big_tree(true)]));
    if std::env::var_os("HARN_DUMP_BIG_BRIEF").is_some() {
        println!("{big}");
    }
    if std::env::var_os("HARN_DUMP_HTML").is_some() {
        println!("=====HTML=====");
        println!("{}", render::html(&doc(vec![tree()])));
        println!("=====WHY=====");
        println!("{}", render::why(&tree(), &slack(), &[], true));
    }
}
