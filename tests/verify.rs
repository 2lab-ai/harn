//! WU-D: the Claude Code oracle and `verify` on a synthetic capture
//! (`tests/fixtures/capture-synthetic`, written by its `generate.py`, which also prints the
//! region table pinned below), plus two `#[ignore]` receipts on real data:
//! `HARN_REF_CAPTURE=<otel dir> cargo test --test verify -- --ignored reference` and
//! `HARN_LIVE_DIR=<dir> cargo test --test verify -- --ignored live` (one model request).
//! Both print region names and byte counts only.

use harn::model::{item_id, walk, Agent, Cost, Item, Kind, Layer, Mode, Origin, Target, Tree};
use harn::oracle::{self, claude, Capture};
use harn::resolve::claude::CAP_STATE_UNKNOWN;
use harn::verify::{verify, RowVerdict};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const PROJECT: &str = "/synthetic/project";
const CLAUDE_MD: &str = "/synthetic/project/CLAUDE.md";
const MEMORY: &str = "/synthetic/home/.claude/projects/-synthetic-project/memory/MEMORY.md";
const SERVER: &str = "plugin:demo:demo";

/// What `generate.py` printed: its own arithmetic over the strings it wrote.
const REGIONS: &[(&str, u64)] = &[
    ("tools · builtin (2 tools)", 219),
    ("tools · mcp plugin:demo:demo (2 tools)", 299),
    ("system · billing header", 72),
    ("system · identity", 26),
    ("system · harness rules", 73),
    ("messages[0] block 0 · reminder wrapper", 82),
    ("messages[0] block 0 · /synthetic/project/CLAUDE.md", 138),
    (
        "messages[0] block 0 · auto-memory /synthetic/home/.claude/projects/-synthetic-project/memory/MEMORY.md",
        197,
    ),
    ("messages[0] block 1 · reminder wrapper", 64),
    ("messages[0] block 1 · userEmail", 61),
    ("messages[0] block 1 · gitStatus", 79),
    ("messages[0] block 2 · instruction", 2),
    ("messages[1] system text · SessionStart hook stdout", 78),
    ("messages[1] system text · Environment", 94),
    ("messages[1] system text · agent roster (3)", 254),
    ("messages[1] system text · MCP instructions", 216),
    (
        "messages[1] system text · skill list (4 entries, 2 with descriptions)",
        169,
    ),
    ("messages[1] system text · date", 29),
    ("messages[1] system text · Advisor", 37),
    ("messages[1] tool_addition", 113),
    ("request parameters", 155),
    ("framing", 422),
];
const BODY_BYTES: u64 = 2879;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capture-synthetic")
}

fn body() -> Vec<u8> {
    std::fs::read(fixture().join("synthetic.request.json")).unwrap()
}

fn target(dir: &str) -> Target {
    Target {
        dir: PathBuf::from(dir),
        home: PathBuf::from("/synthetic/home"),
        env: BTreeMap::new(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    }
}

fn id(kind: Kind, layer: Layer, name: &str) -> String {
    item_id(Agent::ClaudeCode, kind, layer, name)
}

fn find<'a>(tree: &'a Tree, id: &str) -> &'a Item {
    walk(tree)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

// ---- parse --------------------------------------------------------------------------------------

#[test]
fn regions_partition_the_body_exactly() {
    let bytes = body();
    assert_eq!(bytes.len() as u64, BODY_BYTES);
    let (regions, tree) = oracle::parse_request_body(&bytes).unwrap();
    let got: Vec<(&str, u64)> = regions.iter().map(|r| (r.name.as_str(), r.bytes)).collect();
    assert_eq!(got, REGIONS);
    assert_eq!(regions.iter().map(|r| r.bytes).sum::<u64>(), BODY_BYTES);
    assert_eq!(tree.observed_body, Some(BODY_BYTES));
    let groups: Vec<&str> = regions.iter().map(|r| r.group()).collect();
    assert_eq!(groups[0], "tools");
    assert!(groups.contains(&"messages[1] system text"));
    assert_eq!(groups.last(), Some(&"framing"));
}

#[test]
fn tree_items_carry_ids_bytes_and_modes() {
    let (_, tree) = oracle::parse_request_body(&body()).unwrap();
    let ids: Vec<&str> = walk(&tree).map(|i| i.id.as_str()).collect();
    let expected = [
        id(Kind::BuiltinTool, Layer::Builtin, "Read"),
        id(Kind::BuiltinTool, Layer::Builtin, "advisor"),
        id(Kind::SystemPrompt, Layer::Builtin, "advisor/instructions"),
        id(Kind::SystemPrompt, Layer::Builtin, "advisor/tool_addition"),
        id(Kind::McpServer, Layer::Plugin, SERVER),
        id(Kind::McpTool, Layer::Plugin, "plugin:demo:demo/search"),
        id(Kind::McpTool, Layer::Plugin, "plugin:demo:demo/fetch"),
        id(Kind::McpInstructions, Layer::Plugin, SERVER),
        id(
            Kind::SystemPrompt,
            Layer::Builtin,
            claude::names::SYSTEM_PROMPT,
        ),
        id(Kind::SystemPrompt, Layer::Builtin, "system/billing-header"),
        id(Kind::SystemPrompt, Layer::Builtin, "system/identity"),
        id(Kind::SystemPrompt, Layer::Builtin, "system/harness-rules"),
        id(Kind::Instruction, Layer::Project, CLAUDE_MD),
        id(Kind::Memory, Layer::User, MEMORY),
        id(Kind::Attribution, Layer::Builtin, claude::names::USER_EMAIL),
        id(Kind::GitBlock, Layer::Builtin, claude::names::GIT_STATUS),
        id(Kind::Hook, Layer::Unresolved, "SessionStart"),
        id(Kind::EnvBlock, Layer::Builtin, claude::names::ENVIRONMENT),
        id(
            Kind::AgentRoster,
            Layer::Builtin,
            claude::names::AGENT_ROSTER,
        ),
        id(Kind::SubAgent, Layer::Unresolved, "general-purpose"),
        id(Kind::SubAgent, Layer::Unresolved, "reviewer"),
        id(Kind::SubAgent, Layer::Plugin, "demo:helper"),
        id(
            Kind::SkillListing,
            Layer::Builtin,
            claude::names::SKILL_LISTING,
        ),
        id(Kind::Skill, Layer::Unresolved, "alpha"),
        id(Kind::Skill, Layer::Unresolved, "beta"),
        id(Kind::Skill, Layer::Plugin, "demo:gamma"),
        id(Kind::Skill, Layer::Unresolved, "delta"),
        id(Kind::EnvBlock, Layer::Builtin, claude::names::DATE),
    ];
    let mut want: Vec<&str> = expected.iter().map(String::as_str).collect();
    let mut have = ids.clone();
    want.sort();
    have.sort();
    assert_eq!(have, want);

    let observed = |id: &str| find(&tree, id).cost.observed;
    // A `Contents of` section = "\n" + header line + "\n\n" + file bytes (44 here).
    assert_eq!(
        observed(&id(Kind::Instruction, Layer::Project, CLAUDE_MD)),
        Some(138)
    );
    assert_eq!(observed(&id(Kind::Memory, Layer::User, MEMORY)), Some(197));
    // Parents carry their children: tools 149 + 150, instructions 93.
    assert_eq!(
        observed(&id(Kind::McpServer, Layer::Plugin, SERVER)),
        Some(392)
    );
    // The advisor tool carries its system text and tool_addition: 68 + 37 + 113.
    assert_eq!(
        observed(&id(Kind::BuiltinTool, Layer::Builtin, "advisor")),
        Some(218)
    );
    assert_eq!(
        observed(&id(Kind::Skill, Layer::Unresolved, "beta")),
        Some(7)
    );
    assert_eq!(
        observed(&id(Kind::SubAgent, Layer::Unresolved, "reviewer")),
        Some(70)
    );

    let beta = find(&tree, &id(Kind::Skill, Layer::Unresolved, "beta"));
    assert_eq!(beta.signal.as_deref(), Some(claude::NAME_ONLY));
    assert_eq!(beta.mode, Mode::Advertised);
    let alpha = find(&tree, &id(Kind::Skill, Layer::Unresolved, "alpha"));
    assert_eq!(alpha.signal, None);
    let instruction = find(&tree, &id(Kind::Instruction, Layer::Project, CLAUDE_MD));
    assert_eq!(instruction.origin.path, Some(PathBuf::from(CLAUDE_MD)));
    assert_eq!(instruction.mode, Mode::Always);
    assert_eq!(tree.tool_search, Some(false));
    assert_eq!(tree.agent_version.as_deref(), Some("9.8.7"));
    assert_eq!(tree.target_dir, PathBuf::from(PROJECT));
}

#[test]
fn a_prompt_that_quotes_a_contents_header_is_still_the_instruction() {
    let mut body: serde_json::Value = serde_json::from_slice(&body()).unwrap();
    body["messages"][0]["content"][2]["text"] =
        "Contents of /x/CLAUDE.md (project instructions, checked into the codebase):\n\nhi".into();
    let bytes = serde_json::to_vec(&body).unwrap();
    let (regions, tree) = oracle::parse_request_body(&bytes).unwrap();
    assert!(regions
        .iter()
        .any(|r| r.name == "messages[0] block 2 · instruction"));
    assert!(!walk(&tree).any(|i| i.id.ends_with("//x/CLAUDE.md")));
    assert_eq!(
        regions.iter().map(|r| r.bytes).sum::<u64>(),
        bytes.len() as u64
    );
}

#[test]
fn init_event_comes_from_the_stream_after_the_hook_events() {
    let stream = std::fs::read_to_string(fixture().join("stream.jsonl")).unwrap();
    assert!(oracle::parse_init_event(stream.lines().next().unwrap()).is_err());
    let init = oracle::find_init_event(&stream).unwrap();
    assert_eq!(
        init,
        oracle::parse_init_event(stream.lines().nth(2).unwrap()).unwrap()
    );
    assert_eq!(init.claude_code_version.as_deref(), Some("9.8.7"));
    assert_eq!(init.cwd, Some(PathBuf::from(PROJECT)));
    assert_eq!(init.mcp_servers.len(), 2);
    assert_eq!(init.mcp_servers[1].status, "disabled");
    assert_eq!(init.plugins[0].source.as_deref(), Some("demo@market"));
    assert_eq!(init.skills, ["alpha", "beta", "demo:gamma", "delta"]);
}

// ---- readers ------------------------------------------------------------------------------------

#[test]
fn otel_dir_binds_to_the_target_and_reads_init() {
    let capture = oracle::read_otel_dir(&fixture(), &target(PROJECT)).unwrap();
    assert_eq!(capture.context.cwd, PathBuf::from(PROJECT));
    assert_eq!(capture.context.agent, Agent::ClaudeCode);
    assert_eq!(capture.context.agent_version.as_deref(), Some("9.8.7"));
    assert_eq!(capture.context.model.as_deref(), Some("claude-test-1"));
    assert_eq!(capture.context.capture_kind, "otel-dir");
    assert_eq!(capture.context.captured_at, "2026-01-02T03:04:05.678Z");
    assert_eq!(capture.body_bytes, BODY_BYTES);
    assert!(capture.init.is_some());
    let plugin = find(
        &capture.tree,
        &id(Kind::Plugin, Layer::Unresolved, "demo@market"),
    );
    assert_eq!(plugin.origin.plugin.as_deref(), Some("demo@market"));
    // init names the server and gives its source layer.
    let server = find(&capture.tree, &id(Kind::McpServer, Layer::Plugin, SERVER));
    assert_eq!(server.origin.plugin.as_deref(), Some("demo@market"));
}

#[test]
fn otel_dir_refuses_another_directory_with_the_mismatch() {
    let err = oracle::read_otel_dir(&fixture(), &target("/elsewhere"))
        .unwrap_err()
        .to_string();
    assert!(err.contains(PROJECT) && err.contains("/elsewhere"), "{err}");
    let mut sonnet = target(PROJECT);
    sonnet.model = Some("sonnet".to_string());
    let err = oracle::read_otel_dir(&fixture(), &sonnet).unwrap_err();
    assert!(format!("{err:#}").contains("model"), "{err:#}");
}

#[test]
fn read_capture_dispatches_on_the_path() {
    let t = target(PROJECT);
    assert_eq!(
        oracle::read_capture(&fixture(), &t)
            .unwrap()
            .context
            .capture_kind,
        "otel-dir"
    );
    let via_index = oracle::read_capture(&fixture().join("index.jsonl"), &t).unwrap();
    assert_eq!(via_index.context.capture_kind, "otel-dir");
    let file = oracle::read_capture(&fixture().join("synthetic.request.json"), &t).unwrap();
    assert_eq!(file.context.capture_kind, "otel-file");
    assert!(file.init.is_none());
    assert!(oracle::read_capture(&fixture().join("stream.jsonl"), &t).is_err());
}

fn llmux_line(ts_ms: u64, body: &str) -> String {
    serde_json::json!({
        "v": 1, "ts_ms": ts_ms, "id": ts_ms, "group": "claude", "model": "claude-test-1",
        "request_body": body, "response_body": "",
    })
    .to_string()
}

fn body_text() -> String {
    String::from_utf8(body()).unwrap()
}

#[test]
fn llmux_record_derives_its_context_from_the_body() {
    let t = target(PROJECT);
    let capture =
        oracle::read_llmux_record(&llmux_line(1_767_323_045_678, &body_text()), &t).unwrap();
    assert_eq!(capture.context.capture_kind, "llmux-raw-io");
    assert_eq!(capture.context.agent_version.as_deref(), Some("9.8.7"));
    assert_eq!(capture.context.captured_at, "2026-01-02T03:04:05.678Z");
    assert_eq!(capture.context.cwd, PathBuf::from(PROJECT));
    assert!(capture.init.is_none());
    assert_eq!(
        capture.regions.iter().map(|r| r.bytes).sum::<u64>(),
        BODY_BYTES
    );

    let err = oracle::read_llmux_record(&llmux_line(1, &body_text()), &target("/elsewhere"));
    assert!(err.unwrap_err().to_string().contains(PROJECT));

    let truncated = format!("{}…[truncated 1879 bytes]", &body_text()[..1000]);
    let err = oracle::read_llmux_record(&llmux_line(1, &truncated), &t).unwrap_err();
    assert!(format!("{err:#}").contains("truncated"), "{err:#}");

    let mut later: serde_json::Value = serde_json::from_str(&body_text()).unwrap();
    later["messages"].as_array_mut().unwrap().push(
        serde_json::json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]}),
    );
    let err = oracle::read_llmux_record(&llmux_line(1, &later.to_string()), &t).unwrap_err();
    assert!(err.to_string().contains("first request"), "{err}");

    let no_env = body_text().replace("# Environment", "# Elsewhere");
    let capture = oracle::read_llmux_record(&llmux_line(1, &no_env), &t).unwrap();
    assert_eq!(
        capture.context.cwd,
        PathBuf::from(PROJECT),
        "falls back to the CLAUDE.md dir"
    );
}

#[test]
fn llmux_file_scan_takes_the_newest_matching_record_not_the_newest() {
    let other = body_text().replace(PROJECT, "/synthetic/other");
    let lines = [
        llmux_line(1_000, &body_text()),
        "not json mentioning /synthetic/project".to_string(),
        llmux_line(2_000, &body_text()),
        llmux_line(3_000, &other),
        llmux_line(4_000, "{\"model\":\"x\"}"),
    ];
    let path = std::env::temp_dir().join(format!("harn-wu-d-{}.jsonl", std::process::id()));
    std::fs::write(
        &path,
        lines.join("\n") + "\n{\"v\":1,\"ts_ms\":5000,\"request_bo",
    )
    .unwrap();
    let result = oracle::read_capture(&path, &target(PROJECT));
    let miss = oracle::read_capture(&path, &target("/synthetic/nowhere"));
    std::fs::remove_file(&path).unwrap();
    assert_eq!(result.unwrap().context.captured_at, "1970-01-01T00:00:02Z");
    let err = miss.unwrap_err().to_string();
    assert!(
        err.contains("no llmux record") && err.contains("/synthetic/nowhere"),
        "{err}"
    );
}

// ---- verify -------------------------------------------------------------------------------------

fn item(kind: Kind, layer: Layer, name: &str, mode: Mode, predicted: Option<u64>) -> Item {
    Item {
        id: id(kind, layer, name),
        label: name.to_string(),
        kind,
        origin: Origin {
            layer,
            path: None,
            setting: None,
            plugin: None,
            why: Vec::new(),
        },
        mode,
        cost: Cost {
            predicted,
            not_applicable: matches!(mode, Mode::Runtime | Mode::None),
            ..Cost::default()
        },
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

fn with_children(mut parent: Item, children: Vec<Item>) -> Item {
    parent.children = children;
    parent
}

fn static_tree(items: Vec<Item>) -> Tree {
    Tree {
        agent: Agent::ClaudeCode,
        target_dir: PathBuf::from(PROJECT),
        agent_version: Some("9.8.7".to_string()),
        rules_verified_against: "9.8.7".to_string(),
        tool_search: None,
        signals: Vec::new(),
        items,
        predicted_known: 0,
        unknown_rows: 0,
        observed_body: None,
        warnings: Vec::new(),
        home: None,
        observed_regions: Vec::new(),
        tool_search_cause: None,
        trust: None,
    }
}

/// A capture as `run_claude` would bind it (its launch options remove four built-ins).
fn run_capture() -> Capture {
    let mut capture = oracle::read_otel_dir(&fixture(), &target(PROJECT)).unwrap();
    capture.context.launch_opts = claude::LAUNCH_ARGS.iter().map(|a| a.to_string()).collect();
    capture
}

/// What a correct resolver would predict for the fixture, with the given skill items.
fn resolved(skills: Vec<Item>, claude_md: u64) -> Vec<Item> {
    use Kind::*;
    let listing = skills.iter().filter_map(|s| s.cost.predicted).sum();
    vec![
        item(
            Instruction,
            Layer::Project,
            CLAUDE_MD,
            Mode::Always,
            Some(claude_md),
        ),
        item(Memory, Layer::User, MEMORY, Mode::Always, Some(197)),
        with_children(
            item(
                SkillListing,
                Layer::Builtin,
                "skills",
                Mode::Always,
                Some(listing),
            ),
            skills,
        ),
        with_children(
            item(
                AgentRoster,
                Layer::Builtin,
                "agents",
                Mode::Always,
                Some(158),
            ),
            vec![
                item(
                    SubAgent,
                    Layer::Builtin,
                    "general-purpose",
                    Mode::Advertised,
                    Some(45),
                ),
                item(
                    SubAgent,
                    Layer::User,
                    "reviewer",
                    Mode::Advertised,
                    Some(70),
                ),
                item(
                    SubAgent,
                    Layer::Plugin,
                    "demo:helper",
                    Mode::Advertised,
                    Some(43),
                ),
            ],
        ),
        // MCP schemas are `?` until a capture: the observed tools attach here.
        item(McpServer, Layer::Plugin, SERVER, Mode::Always, None),
        item(McpServer, Layer::Plugin, "plugin:off:off", Mode::None, None),
        item(Plugin, Layer::Plugin, "demo@market", Mode::Always, None),
        item(BuiltinTool, Layer::Builtin, "Read", Mode::Always, None),
        item(BuiltinTool, Layer::Builtin, "advisor", Mode::Always, None),
        // Removed by the run's --disallowedTools: not expected.
        item(BuiltinTool, Layer::Builtin, "Bash", Mode::Always, None),
        item(SystemPrompt, Layer::Builtin, "system", Mode::Always, None),
        item(GitBlock, Layer::Builtin, "gitStatus", Mode::Always, None),
        item(EnvBlock, Layer::Builtin, "environment", Mode::Always, None),
        // Fires on a later event: absent from the first request, never missing.
        item(Hook, Layer::User, "PreToolUse/0", Mode::Runtime, None),
        // Prints at session start, but its stdout cannot be attributed by id: per-run, ungated.
        item(Hook, Layer::User, "SessionStart/0", Mode::Always, None),
    ]
}

fn skills_with(extra: Vec<Item>) -> Vec<Item> {
    use Kind::Skill;
    let mut skills = vec![
        item(Skill, Layer::User, "alpha", Mode::Advertised, Some(40)),
        // Has a description on disk (30 B entry) but arrives name-only (7 B): over cap.
        item(Skill, Layer::User, "beta", Mode::Advertised, Some(30)),
        // A plugin command, listed like a skill.
        item(
            Kind::Command,
            Layer::Plugin,
            "demo:gamma",
            Mode::Advertised,
            Some(34),
        ),
        // No description on disk: name-only is expected, not dropped.
        item(Skill, Layer::Project, "delta", Mode::Advertised, Some(22)),
    ];
    skills.extend(extra);
    skills
}

#[test]
fn verify_reports_missing_mismatch_and_overflow_and_promotes() {
    let omega = item(
        Kind::Skill,
        Layer::User,
        "omega",
        Mode::Advertised,
        Some(20),
    );
    let mut tree = static_tree(resolved(skills_with(vec![omega]), 148));
    let report = verify(&mut tree, &run_capture());

    let verdict = |id: &str| {
        report
            .rows
            .iter()
            .find(|(row, _)| row == id)
            .map(|(_, v)| v.clone())
    };
    let failing: Vec<&(String, RowVerdict)> = report
        .rows
        .iter()
        .filter(|(_, v)| *v != RowVerdict::Seen)
        .collect();
    assert_eq!(failing.len(), 2, "{failing:#?}");
    assert_eq!(
        verdict(&id(Kind::Skill, Layer::User, "omega")),
        Some(RowVerdict::PredictedNotSeen)
    );
    assert_eq!(
        verdict(&id(Kind::Instruction, Layer::Project, CLAUDE_MD)),
        Some(RowVerdict::ContentMismatch {
            predicted: 148,
            observed: Some(138)
        })
    );
    assert!(!report.passed);

    // Runtime and disabled items are outside the projection; launch options removed Bash.
    for absent in [
        id(Kind::Hook, Layer::User, "PreToolUse/0"),
        id(Kind::McpServer, Layer::Plugin, "plugin:off:off"),
        id(Kind::BuiltinTool, Layer::Builtin, "Bash"),
        id(Kind::Hook, Layer::User, "SessionStart/0"),
    ] {
        assert_eq!(verdict(&absent), None, "{absent}");
    }
    // Every observed item joined; none is seen-not-predicted.
    assert!(!report
        .rows
        .iter()
        .any(|(_, v)| *v == RowVerdict::SeenNotPredicted));
    assert_eq!(
        verdict(&id(Kind::Command, Layer::Plugin, "demo:gamma")),
        Some(RowVerdict::Seen)
    );
    assert_eq!(
        verdict(&id(Kind::Plugin, Layer::Plugin, "demo@market")),
        Some(RowVerdict::Seen)
    );

    // Capture-only MCP children attach under the `?` server, re-layered to it.
    let server = find(&tree, &id(Kind::McpServer, Layer::Plugin, SERVER));
    let children: Vec<&str> = server.children.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        children,
        [
            id(Kind::McpTool, Layer::Plugin, "plugin:demo:demo/search"),
            id(Kind::McpTool, Layer::Plugin, "plugin:demo:demo/fetch"),
            id(Kind::McpInstructions, Layer::Plugin, SERVER),
        ]
    );
    assert_eq!(server.cost.observed, Some(392));

    // Over cap: beta lost its description, delta never had one.
    assert_eq!(
        tree.signals,
        ["skill listing: OVER CAP — 1 of 4 descriptions dropped (observed)"]
    );
    let listing = find(&tree, &id(Kind::SkillListing, Layer::Builtin, "skills"));
    assert_eq!(
        listing.signal.as_deref(),
        Some("over cap: 1 of 4 descriptions dropped (observed)")
    );
    assert!(find(&tree, &id(Kind::Skill, Layer::User, "beta"))
        .signal
        .is_some());
    assert!(find(&tree, &id(Kind::Skill, Layer::Project, "delta"))
        .signal
        .is_none());

    // Promotion: every matched static item that has observed bytes (the plugin has none).
    let promoted = [
        (id(Kind::Instruction, Layer::Project, CLAUDE_MD), 138),
        (id(Kind::Memory, Layer::User, MEMORY), 197),
        (id(Kind::SkillListing, Layer::Builtin, "skills"), 169),
        (id(Kind::Skill, Layer::User, "alpha"), 40),
        (id(Kind::Skill, Layer::User, "beta"), 7),
        (id(Kind::Command, Layer::Plugin, "demo:gamma"), 34),
        (id(Kind::Skill, Layer::Project, "delta"), 22),
        (id(Kind::AgentRoster, Layer::Builtin, "agents"), 254),
        (id(Kind::SubAgent, Layer::Builtin, "general-purpose"), 45),
        (id(Kind::SubAgent, Layer::User, "reviewer"), 70),
        (id(Kind::SubAgent, Layer::Plugin, "demo:helper"), 43),
        (id(Kind::McpServer, Layer::Plugin, SERVER), 392),
        (id(Kind::BuiltinTool, Layer::Builtin, "Read"), 151),
        (id(Kind::BuiltinTool, Layer::Builtin, "advisor"), 218),
        (id(Kind::SystemPrompt, Layer::Builtin, "system"), 171),
        (id(Kind::GitBlock, Layer::Builtin, "gitStatus"), 79),
        (id(Kind::EnvBlock, Layer::Builtin, "environment"), 94),
    ];
    for (row, bytes) in &promoted {
        assert_eq!(find(&tree, row).cost.observed, Some(*bytes), "{row}");
    }
    assert_eq!(report.promoted, promoted.len() as u32);
    assert_eq!(
        find(&tree, &id(Kind::Memory, Layer::User, MEMORY))
            .cost
            .predicted,
        Some(197)
    );
    assert_eq!(tree.observed_body, Some(BODY_BYTES));
    assert_eq!(tree.tool_search, Some(false));
    assert_eq!(report.context.capture_kind, "otel-dir");
}

#[test]
fn verify_passes_when_the_tree_matches_the_capture() {
    let mut tree = static_tree(resolved(skills_with(Vec::new()), 138));
    let report = verify(&mut tree, &run_capture());
    assert!(report.passed, "{:#?}", report.rows);
    assert!(tree.warnings.is_empty(), "{:#?}", tree.warnings);
}

#[test]
fn verify_drops_the_cap_state_unknown_warning_once_a_capture_is_bound() {
    // The resolver cannot know the token-denominated cap; a bound capture shows it, so the
    // warning would contradict the observed listing. Other warnings survive the bind.
    let mut tree = static_tree(resolved(skills_with(Vec::new()), 138));
    tree.warnings = vec![CAP_STATE_UNKNOWN.to_string(), "unrelated".to_string()];
    verify(&mut tree, &run_capture());
    assert!(tree.observed_body.is_some());
    assert_eq!(
        tree.signals,
        ["skill listing: OVER CAP — 1 of 4 descriptions dropped (observed)"]
    );
    assert_eq!(tree.warnings, ["unrelated"]);
}

#[test]
fn verify_drops_the_cap_state_unknown_warning_when_the_capture_has_no_listing() {
    // No skills on either side: the request carries no listing, so the over-cap branch never
    // runs, and the bound capture still settles the cap state. The capture is the fixture minus
    // its listing item (entries are its children) and its region; `init` keeps its skill names,
    // which `verify` never reads.
    let mut capture = run_capture();
    capture.tree.items.retain(|i| i.kind != Kind::SkillListing);
    let (listing, rest): (Vec<_>, Vec<_>) = capture
        .regions
        .into_iter()
        .partition(|r| r.name.contains("skill list"));
    assert_eq!(listing.len(), 1, "{listing:?}");
    capture.regions = rest;
    capture.body_bytes -= listing.iter().map(|r| r.bytes).sum::<u64>();
    assert!(!walk(&capture.tree).any(|i| matches!(i.kind, Kind::SkillListing | Kind::Skill)));
    assert_eq!(
        capture.regions.iter().map(|r| r.bytes).sum::<u64>(),
        capture.body_bytes
    );

    let mut items = resolved(skills_with(Vec::new()), 138);
    items.retain(|i| i.kind != Kind::SkillListing);
    let mut tree = static_tree(items);
    tree.warnings = vec![CAP_STATE_UNKNOWN.to_string(), "unrelated".to_string()];
    let report = verify(&mut tree, &capture);
    assert!(report.passed, "{:#?}", report.rows);
    assert_eq!(tree.observed_body, Some(capture.body_bytes));
    assert!(tree.signals.is_empty(), "{:?}", tree.signals);
    assert_eq!(tree.warnings, ["unrelated"]);
}

#[test]
fn verify_flags_observed_items_the_tree_missed() {
    // No `alpha` and no `reviewer` in the tree; the roster's cost is known, so its capture-only
    // child is a resolver miss, while MCP tools under a known-cost server still only attach.
    let mut items = resolved(skills_with(Vec::new()), 138);
    items[2].children.retain(|s| s.label != "alpha");
    items[3].children.retain(|s| s.label != "reviewer");
    items[4].cost.predicted = Some(392);
    let mut tree = static_tree(items);
    let report = verify(&mut tree, &run_capture());
    let snp: Vec<&str> = report
        .rows
        .iter()
        .filter(|(_, v)| *v == RowVerdict::SeenNotPredicted)
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        snp,
        [
            id(Kind::SubAgent, Layer::Unresolved, "reviewer"),
            id(Kind::Skill, Layer::Unresolved, "alpha"),
        ]
    );
    assert!(!report.passed);
    let server = find(&tree, &id(Kind::McpServer, Layer::Plugin, SERVER));
    assert_eq!(server.children.len(), 3);
}

#[test]
fn verify_refuses_a_capture_bound_elsewhere() {
    let mut tree = static_tree(resolved(skills_with(Vec::new()), 138));
    tree.target_dir = PathBuf::from("/synthetic/other");
    let report = verify(&mut tree, &run_capture());
    assert!(!report.passed);
    assert!(
        tree.warnings.iter().any(|w| w.contains("/synthetic/other")),
        "{:?}",
        tree.warnings
    );
}

#[test]
fn verify_counts_an_on_demand_item_in_the_request_as_unpredicted() {
    let mut items = resolved(skills_with(Vec::new()), 138);
    items[0].mode = Mode::OnDemand;
    let mut tree = static_tree(items);
    let report = verify(&mut tree, &run_capture());
    assert!(report.rows.contains(&(
        id(Kind::Instruction, Layer::Project, CLAUDE_MD),
        RowVerdict::SeenNotPredicted
    )));
}

#[test]
fn verify_without_init_does_not_expect_plugins() {
    let mut capture = run_capture();
    capture.init = None;
    let mut tree = static_tree(resolved(skills_with(Vec::new()), 138));
    // The fixture's plugin item came from init; drop it as an llmux capture would not have it.
    capture.tree.items.retain(|i| i.kind != Kind::Plugin);
    let report = verify(&mut tree, &capture);
    assert!(report.passed, "{:#?}", report.rows);
    assert!(!report
        .rows
        .iter()
        .any(|(id, _)| id.contains("/plugin/plugin/")));
}

#[test]
fn verify_joins_listing_entries_across_layers_with_one_warning_per_layer_pair() {
    // The capture guesses `plugin` for every `x:y` entry; a claude.ai-synced plugin's skill and a
    // plugin command the resolver files under `claudeai` are the same names, not misses.
    let mut skills = skills_with(Vec::new());
    for s in &mut skills {
        if s.label == "demo:gamma" {
            s.origin.layer = Layer::ClaudeAi;
            s.id = id(Kind::Command, Layer::ClaudeAi, "demo:gamma");
        }
    }
    let mut items = resolved(skills, 138);
    // A plugin agent under another layer joins the same way.
    let helper = &mut items[3].children[2];
    helper.origin.layer = Layer::ClaudeAi;
    helper.id = id(Kind::SubAgent, Layer::ClaudeAi, "demo:helper");
    let mut tree = static_tree(items);
    let report = verify(&mut tree, &run_capture());
    assert!(report.passed, "{:#?}", report.rows);
    assert!(report.rows.contains(&(
        id(Kind::Command, Layer::ClaudeAi, "demo:gamma"),
        RowVerdict::Seen
    )));
    assert_eq!(
        find(&tree, &id(Kind::SubAgent, Layer::ClaudeAi, "demo:helper"))
            .cost
            .observed,
        Some(43)
    );
    assert_eq!(
        tree.warnings,
        [
            format!(
                "{}: layer claudeai in the tree, plugin in the capture",
                id(Kind::SubAgent, Layer::ClaudeAi, "demo:helper")
            ),
            format!(
                "{}: layer claudeai in the tree, plugin in the capture",
                id(Kind::Command, Layer::ClaudeAi, "demo:gamma")
            ),
        ]
    );
}

#[test]
fn verify_aggregates_a_repeated_layer_difference_into_one_warning() {
    // Two synced skills (`claudeai` in the tree) that the capture lists as `plugin` entries.
    let synced = ["anthropic-skills:one", "anthropic-skills:two"];
    let extra = synced
        .iter()
        .map(|name| item(Kind::Skill, Layer::ClaudeAi, name, Mode::Advertised, None))
        .collect();
    let mut tree = static_tree(resolved(skills_with(extra), 138));
    let mut capture = run_capture();
    let listing = capture
        .tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .expect("fixture listing");
    let template = listing.children[0].clone();
    for name in synced {
        let mut entry = template.clone();
        entry.id = id(Kind::Skill, Layer::Plugin, name);
        entry.label = name.to_string();
        entry.origin.layer = Layer::Plugin;
        listing.children.push(entry);
    }
    let report = verify(&mut tree, &capture);
    assert!(report.passed, "{:#?}", report.rows);
    assert_eq!(
        tree.warnings,
        [format!(
            "2 skill rows: layer claudeai in the tree, plugin in the capture (joined by name; \
             e.g. {})",
            id(Kind::Skill, Layer::ClaudeAi, synced[0])
        )]
    );
}

#[test]
fn verify_expects_every_enabled_plugin_in_init_whatever_its_mode() {
    // `demo@market` contributes nothing to the first request (mode none) but is enabled: the
    // `init` event lists it, so it is seen, not seen-not-predicted.
    let mut items = resolved(skills_with(Vec::new()), 138);
    let plugin = items
        .iter_mut()
        .find(|i| i.kind == Kind::Plugin)
        .expect("fixture plugin");
    plugin.mode = Mode::None;
    plugin.cost.not_applicable = true;
    // An enabled plugin the init event does not list is missing, even with mode runtime.
    items.push(item(
        Kind::Plugin,
        Layer::Builtin,
        "ghost@builtin",
        Mode::Runtime,
        None,
    ));
    let mut tree = static_tree(items);
    let report = verify(&mut tree, &run_capture());
    let failing: Vec<&(String, RowVerdict)> = report
        .rows
        .iter()
        .filter(|(_, v)| *v != RowVerdict::Seen)
        .collect();
    assert_eq!(
        failing,
        [&(
            id(Kind::Plugin, Layer::Builtin, "ghost@builtin"),
            RowVerdict::PredictedNotSeen
        )]
    );
    assert!(report.rows.contains(&(
        id(Kind::Plugin, Layer::Plugin, "demo@market"),
        RowVerdict::Seen
    )));
}

// ---- receipts on real data (ignored) --------------------------------------------------------------

fn print_regions(capture_regions: &[oracle::Region], body_bytes: u64) {
    for region in capture_regions {
        println!("{:>9}  {}", region.bytes, region.name);
    }
    let sum: u64 = capture_regions.iter().map(|r| r.bytes).sum();
    println!(
        "{sum:>9}  sum (body {body_bytes}, equal: {})",
        sum == body_bytes
    );
}

fn counts(tree: &Tree) {
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for item in walk(tree) {
        *by_kind.entry(item.kind.as_str()).or_default() += 1;
    }
    println!("items by kind: {by_kind:?}");
    println!(
        "signals: {:?}  tool_search: {:?}",
        tree.signals, tree.tool_search
    );
}

#[test]
#[ignore = "reads a real capture: HARN_REF_CAPTURE=<otel dir>"]
fn reference_capture_region_table() {
    let dir = PathBuf::from(std::env::var("HARN_REF_CAPTURE").expect("HARN_REF_CAPTURE"));
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with(".request.json") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let (regions, tree) = oracle::parse_request_body(&bytes).unwrap();
        print_regions(&regions, bytes.len() as u64);
        counts(&tree);
        assert_eq!(
            regions.iter().map(|r| r.bytes).sum::<u64>(),
            bytes.len() as u64
        );
    }
}

#[test]
#[ignore = "runs claude once (one model request): HARN_LIVE_DIR=<dir>"]
fn live_run_claude() {
    let dir =
        std::fs::canonicalize(std::env::var("HARN_LIVE_DIR").expect("HARN_LIVE_DIR")).unwrap();
    let claude = std::env::var("PATH")
        .unwrap()
        .split(':')
        .map(|p| Path::new(p).join("claude"))
        .find(|p| p.is_file())
        .expect("claude on PATH");
    let target = Target {
        dir,
        home: PathBuf::from(std::env::var("HOME").unwrap()),
        env: std::env::vars().collect(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    };
    let capture = oracle::run_claude(&target, &claude).unwrap();
    let c = &capture.context;
    println!(
        "context: cwd {} · version {:?} · model {:?} · kind {} · opts {} · fingerprint {}",
        c.cwd.display(),
        c.agent_version,
        c.model,
        c.capture_kind,
        c.launch_opts.join(" "),
        c.settings_fingerprint.as_deref().unwrap_or("-")
    );
    print_regions(&capture.regions, capture.body_bytes);
    counts(&capture.tree);
    let init = capture.init.as_ref().expect("init event");
    println!(
        "init: {} tools · {} mcp servers · {} skills · {} agents · {} plugins",
        init.tools.len(),
        init.mcp_servers.len(),
        init.skills.len(),
        init.agents.len(),
        init.plugins.len()
    );
}
