//! plan / writers / journal against a FAKE resolver (WU-E).
//!
//! The fake builds its tree from `tests/fixtures/plan-fake/tree.json` and honours the overlay the
//! way the real resolver must: a `set-json` op patches an in-memory view of that file before the
//! gates read it, a `remove-path` op drops every item whose path is under it. Every test works on
//! a temp copy of the fixture, with the journal in the same temp dir.

use harn::journal::{self, Journal, JournalEntry, ABSENT};
use harn::model::{
    Agent, Cost, Freed, Item, Kind, Knob, Layer, Mode, Origin, Overlay, OverlayOp, Refused, Scope,
    SelectError, Stale, Target, Tree,
};
use harn::plan::{self, PlanOp, PlanRequest, PLAN_SCHEMA};
use harn::resolve::Resolver;
use harn::writers;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const SYSTEM_PROMPT: &str = "claude-code/system-prompt/builtin/system-prompt";
const HOOK: &str = "claude-code/hook/project/SessionStart:0";
const SLACK: &str = "claude-code/mcp-server/plugin/plugin:slack:slack";
const FOO: &str = "claude-code/skill/project/foo";

/// The fixture's settings.local.json, byte for byte (4-space, no trailing newline).
const LOCAL_BEFORE: &str = "{\n    \"permissions\": {\n        \"allow\": [\n            \"Bash(ls:*)\"\n        ]\n    },\n    \"model\": \"sonnet\"\n}";

// ---------------------------------------------------------------------------------------------
// The fake resolver

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    id: String,
    label: String,
    kind: Kind,
    layer: Layer,
    mode: Mode,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    plugin: Option<String>,
    #[serde(default)]
    predicted: Option<u64>,
    #[serde(default)]
    estimate: bool,
    #[serde(default)]
    knobs: Vec<Knob>,
    #[serde(default)]
    off_if: Vec<Gate>,
    #[serde(default)]
    sum_children: bool,
    #[serde(default)]
    children: Vec<Spec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Gate {
    file: PathBuf,
    pointer: String,
    #[serde(default)]
    equals: Option<Value>,
    #[serde(default)]
    contains: Option<Value>,
}

struct Fake {
    /// tree.json with the placeholders already replaced.
    spec: String,
    /// Resolves so far (atomic: `annotate_freed` re-resolves on several threads).
    resolves: AtomicUsize,
}

impl Fake {
    /// `file` as the agent would read it with the overlay applied.
    fn json(&self, overlay: Option<&Overlay>, file: &Path) -> Value {
        let mut doc = fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| json!({}));
        for op in overlay.map(|o| o.ops.as_slice()).unwrap_or_default() {
            if let OverlayOp::SetJson {
                file: f,
                pointer,
                value,
            } = op
            {
                if f == file {
                    writers::set_json_pointer(&mut doc, pointer, value.clone()).unwrap();
                }
            }
        }
        doc
    }

    fn admitted(&self, spec: &Spec, overlay: Option<&Overlay>) -> bool {
        if let Some(path) = &spec.path {
            let removed = overlay.is_some_and(|o| {
                o.ops.iter().any(
                    |op| matches!(op, OverlayOp::RemovePath { path: p } if path.starts_with(p)),
                )
            });
            if removed || fs::symlink_metadata(path).is_err() {
                return false;
            }
        }
        !spec.off_if.iter().any(|gate| {
            let doc = self.json(overlay, &gate.file);
            let at = doc.pointer(&gate.pointer);
            match (&gate.equals, &gate.contains) {
                (Some(want), _) => at == Some(want),
                (_, Some(want)) => at
                    .and_then(Value::as_array)
                    .is_some_and(|items| items.contains(want)),
                _ => panic!("gate on {} has neither equals nor contains", gate.pointer),
            }
        })
    }

    fn build(
        &self,
        spec: Spec,
        overlay: Option<&Overlay>,
        total: &mut u64,
        unknown: &mut u32,
    ) -> Option<Item> {
        if !self.admitted(&spec, overlay) {
            return None;
        }
        let children: Vec<Item> = spec
            .children
            .into_iter()
            .filter_map(|c| self.build(c, overlay, total, unknown))
            .collect();
        let predicted = if spec.sum_children {
            Some(children.iter().filter_map(|c| c.cost.predicted).sum())
        } else {
            match spec.predicted {
                Some(bytes) => *total += bytes,
                None => *unknown += 1,
            }
            spec.predicted
        };
        Some(Item {
            id: spec.id,
            label: spec.label,
            kind: spec.kind,
            origin: Origin {
                layer: spec.layer,
                path: spec.path,
                setting: None,
                plugin: spec.plugin,
                why: Vec::new(),
            },
            mode: spec.mode,
            cost: Cost {
                source: predicted,
                predicted,
                predicted_is_estimate: spec.estimate,
                observed: None,
                not_applicable: false,
            },
            knobs: spec.knobs,
            freed: None,
            signal: None,
            last_used: None,
            children,
        })
    }
}

impl Resolver for Fake {
    fn agent(&self) -> Agent {
        Agent::ClaudeCode
    }

    fn rules_verified_against(&self) -> &'static str {
        "fake"
    }

    fn resolve(&self, target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        let specs: Vec<Spec> = serde_json::from_str(&self.spec)?;
        let (mut total, mut unknown) = (0, 0);
        let items = specs
            .into_iter()
            .filter_map(|s| self.build(s, overlay, &mut total, &mut unknown))
            .collect();
        Ok(Tree {
            agent: Agent::ClaudeCode,
            target_dir: target.dir.clone(),
            agent_version: None,
            rules_verified_against: "fake".to_string(),
            tool_search: None,
            signals: Vec::new(),
            items,
            predicted_known: total,
            unknown_rows: unknown,
            observed_body: None,
            warnings: Vec::new(),
            home: None,
            observed_regions: Vec::new(),
            tool_search_cause: None,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Sandbox: a temp copy of the fixture + a journal

struct Sandbox {
    root: PathBuf,
    project: PathBuf,
    home: PathBuf,
    fake: Fake,
    target: Target,
    journal: Journal,
}

impl Sandbox {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plan-fake");
        let root = std::env::temp_dir().join(format!(
            "harn-plan-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        // Realpaths, as the resolver reports them (macOS: /var → /private/var).
        let root = fs::canonicalize(&root).unwrap();
        let (project, home) = (root.join("project"), root.join("home"));
        copy_tree(&fixture.join("project"), &project);
        copy_tree(&fixture.join("home"), &home);
        let spec = fs::read_to_string(fixture.join("tree.json"))
            .unwrap()
            .replace("${PROJECT}", project.to_str().unwrap())
            .replace("${HOME}", home.to_str().unwrap());
        Sandbox {
            target: Target {
                dir: project.clone(),
                home: home.clone(),
                env: BTreeMap::new(),
                model: None,
                context_window: None,
                bytes_per_token: None,
            },
            journal: Journal {
                dir: root.join("state"),
            },
            fake: Fake {
                spec,
                resolves: AtomicUsize::new(0),
            },
            root,
            project,
            home,
        }
    }

    fn resolve(&self) -> Tree {
        self.fake.resolve(&self.target, None).unwrap()
    }

    fn plan(&self, item_id: &str, scope: Option<Scope>) -> anyhow::Result<plan::ActionPlan> {
        plan::build_plan(&self.fake, &self.target, &self.resolve(), item_id, scope)
    }

    fn off(&mut self, item_id: &str, scope: Option<Scope>) -> JournalEntry {
        let plan = self.plan(item_id, scope).unwrap();
        writers::apply(&plan, &mut self.journal).unwrap()
    }

    fn id(&self, rel: &str) -> String {
        format!(
            "claude-code/instruction/project/{}/{rel}",
            self.project.display()
        )
    }

    fn local(&self) -> PathBuf {
        self.project.join(".claude/settings.local.json")
    }

    fn user(&self) -> PathBuf {
        self.home.join(".claude/settings.json")
    }

    fn memory_id(&self) -> String {
        format!(
            "claude-code/memory/user/{}/.claude/projects/p/memory/MEMORY.md",
            self.home.display()
        )
    }

    fn request(&self, ops: Vec<PlanOp>) -> PlanRequest {
        PlanRequest {
            schema: PLAN_SCHEMA.to_string(),
            target_dir: self.project.clone(),
            ops,
        }
    }

    /// An executable editor script in the sandbox.
    fn editor(&self, name: &str, body: &str) -> String {
        let path = self.root.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_string()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry.unwrap();
        let dest = to.join(entry.path().strip_prefix(from).unwrap());
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest).unwrap();
        } else {
            fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

fn op(selector: &str, reason: &str) -> PlanOp {
    PlanOp {
        selector: selector.to_string(),
        scope: None,
        knob: None,
        reason: reason.to_string(),
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn refused(err: &anyhow::Error) -> String {
    err.chain()
        .find_map(|c| c.downcast_ref::<Refused>())
        .unwrap_or_else(|| panic!("not Refused: {err:#}"))
        .0
        .clone()
}

fn stale(err: &anyhow::Error) -> Vec<PathBuf> {
    err.chain()
        .find_map(|c| c.downcast_ref::<Stale>())
        .unwrap_or_else(|| panic!("not Stale: {err:#}"))
        .files
        .clone()
}

fn find<'a>(tree: &'a Tree, id: &str) -> Option<&'a Item> {
    harn::model::walk(tree).find(|i| i.id == id)
}

// ---------------------------------------------------------------------------------------------
// freed, side effects, scope

#[test]
fn fake_fixture_totals() {
    let sb = Sandbox::new();
    let tree = sb.resolve();
    // 5000 system prompt + 1200 CLAUDE.md + 300 rules/a.md + 400 memory + 80+100+150 skills.
    assert_eq!(tree.predicted_known, 7230);
    assert_eq!(tree.unknown_rows, 3); // hook, slack server, its tool
}

#[test]
fn freed_of_a_plain_knob_is_the_exact_before_minus_after() {
    let sb = Sandbox::new();
    let tree = sb.resolve();
    let plan = sb.plan(&sb.id("CLAUDE.md"), None).unwrap();
    assert_eq!(plan.predicted_before, tree.predicted_known);
    // The excluded CLAUDE.md takes its @import along: 1200 + 300.
    assert_eq!(plan.predicted_after, 7230 - 1500);
    assert_eq!(
        plan.freed,
        Freed {
            known: 1500,
            delta_unknown: false,
            is_estimate: false
        }
    );
    // Its own import is part of the item, not a side effect.
    assert!(plan.side_effects.is_empty(), "{:?}", plan.side_effects);
    assert_eq!(plan.target_dir, sb.project);
    assert_eq!(plan.ops.len(), 1);
    assert_eq!(plan.ops[0].file.as_deref(), Some(sb.local().as_path()));
    assert_eq!(
        plan.ops[0].hash_before,
        Some(writers::hash_bytes(LOCAL_BEFORE.as_bytes()))
    );
    assert!(
        plan.diff.contains("+  \"claudeMdExcludes\": ["),
        "{}",
        plan.diff
    );
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    // Planning writes nothing.
    assert_eq!(read(&sb.local()), LOCAL_BEFORE);
}

#[test]
fn freed_is_delta_unknown_when_a_removed_row_costs_unknown() {
    let sb = Sandbox::new();
    let plan = sb.plan(HOOK, None).unwrap();
    assert_eq!(
        plan.freed,
        Freed {
            known: 0,
            delta_unknown: true,
            is_estimate: false
        }
    );
}

#[test]
fn plugin_knob_reports_its_other_rows_as_side_effects() {
    let sb = Sandbox::new();
    let plan = sb.plan(SLACK, None).unwrap();
    // Known: the two listing entries leave (100 + 150); the server and its tool are `?`.
    assert_eq!(plan.freed.known, 250);
    assert!(plan.freed.delta_unknown);
    assert_eq!(
        plan.side_effects,
        vec![
            "slack-search (skill) leaves".to_string(),
            "slack-post (skill) leaves".to_string(),
        ]
    );
}

/// `plan.leaving` as (id, kind, mode before).
fn leaving(plan: &plan::ActionPlan) -> Vec<(&str, Kind, Mode)> {
    plan.leaving
        .iter()
        .map(|l| (l.id.as_str(), l.kind, l.mode_before))
        .collect()
}

/// Items in the first request: mode always or advertised.
fn at_start(tree: &Tree) -> usize {
    harn::model::walk(tree)
        .filter(|i| matches!(i.mode, Mode::Always | Mode::Advertised))
        .count()
}

#[test]
fn leaving_lists_the_planned_item_and_every_row_that_goes_with_it_by_mode() {
    let sb = Sandbox::new();
    let plan = sb.plan(SLACK, None).unwrap();
    // Walk order, each with its mode before: the planned server and its tool (the side effects
    // leave them out as the plan's own), then the plugin's two skills in the listing.
    assert_eq!(
        leaving(&plan),
        [
            (SLACK, Kind::McpServer, Mode::Always),
            (
                "claude-code/mcp-tool/plugin/plugin:slack:slack/send_message",
                Kind::McpTool,
                Mode::Always
            ),
            (
                "claude-code/skill/plugin/slack:slack-search",
                Kind::Skill,
                Mode::Advertised
            ),
            (
                "claude-code/skill/plugin/slack:slack-post",
                Kind::Skill,
                Mode::Advertised
            ),
        ]
    );
    assert_eq!(plan.leaving[0].label, "plugin:slack:slack");
    assert_eq!(plan.side_effects.len(), 2, "{:?}", plan.side_effects);
    // The four loaded at start: the count after is the count before less them.
    assert_eq!(plan.items_after, at_start(&sb.resolve()) - 4);
}

#[test]
fn a_row_that_loaded_nothing_and_goes_is_leaving_with_mode_none() {
    // A tool of slack's server that loads nothing (mode none, no `off:` signal): it goes from
    // the tree with its server, and is listed as leaving, its mode before none.
    let mut sb = Sandbox::new();
    let mut spec: Value = serde_json::from_str(&sb.fake.spec).unwrap();
    let server = spec
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|s| s["id"] == json!(SLACK))
        .unwrap();
    server["children"].as_array_mut().unwrap().push(json!({
        "id": "claude-code/mcp-tool/plugin/plugin:slack:slack/dormant",
        "label": "dormant",
        "kind": "mcp-tool",
        "layer": "plugin",
        "plugin": "slack@x",
        "mode": "none",
        "predicted": null
    }));
    sb.fake.spec = spec.to_string();
    let plan = sb.plan(SLACK, None).unwrap();
    assert!(
        leaving(&plan).contains(&(
            "claude-code/mcp-tool/plugin/plugin:slack:slack/dormant",
            Kind::McpTool,
            Mode::None
        )),
        "{:?}",
        leaving(&plan)
    );
    // It loaded nothing: the count after loses only the four that did.
    assert_eq!(plan.leaving.len(), 5);
    assert_eq!(plan.items_after, at_start(&sb.resolve()) - 4);
}

#[test]
fn a_planned_item_that_stays_is_not_leaving_when_a_disabled_server_appears() {
    // Review r9: an agent whose switch keeps it in the roster (as a deny rule does), and a
    // server, disabled (mode none), that the same write makes appear. Nothing leaves; the
    // server is a side effect that appears and loads nothing.
    let mut sb = Sandbox::new();
    let local = sb.local();
    let mut spec: Value = serde_json::from_str(&sb.fake.spec).unwrap();
    let items = spec.as_array_mut().unwrap();
    items.push(json!({
        "id": "claude-code/sub-agent/project/critic",
        "label": "critic",
        "kind": "sub-agent",
        "layer": "project",
        "mode": "advertised",
        "predicted": 50,
        "knobs": [{
            "type": "setting",
            "file": local,
            "pointer": "/model",
            "value": "opus",
            "scope": "local",
            "note": null
        }]
    }));
    items.push(json!({
        "id": "claude-code/mcp-server/project/srv",
        "label": "srv",
        "kind": "mcp-server",
        "layer": "project",
        "mode": "none",
        "predicted": null,
        "off_if": [{ "file": local, "pointer": "/model", "equals": "sonnet" }]
    }));
    sb.fake.spec = spec.to_string();
    let tree = sb.resolve();
    assert!(find(&tree, "claude-code/mcp-server/project/srv").is_none());

    let plan = sb
        .plan("claude-code/sub-agent/project/critic", None)
        .unwrap();
    assert_eq!(leaving(&plan), []);
    assert_eq!(plan.side_effects, ["srv (mcp-server) appears"]);
    // The count at start does not move: the agent stays, the server loads nothing.
    assert_eq!(plan.items_after, at_start(&tree));
}

#[test]
fn a_listing_entry_leaving_is_delta_unknown_even_with_known_bytes() {
    let sb = Sandbox::new();
    let plan = sb.plan(FOO, None).unwrap();
    assert_eq!(plan.freed.known, 80);
    assert!(
        plan.freed.delta_unknown,
        "token-budgeted listing re-renders"
    );
    assert!(plan.diff.contains("rename from "), "{}", plan.diff);
}

#[test]
fn scope_never_widens_silently() {
    let sb = Sandbox::new();
    let memory = sb.memory_id();
    // Only a user-scope knob: the default (local) and project both refuse, naming the scope.
    for scope in [None, Some(Scope::Project)] {
        let err = sb.plan(&memory, scope).unwrap_err();
        assert!(
            refused(&err).contains("scope would widen to user; pass --scope user"),
            "{err:#}"
        );
    }
    let plan = sb.plan(&memory, Some(Scope::User)).unwrap();
    assert_eq!(plan.ops[0].file.as_deref(), Some(sb.user().as_path()));
    assert_eq!(plan.freed.known, 400);
    assert!(
        plan.warnings.iter().any(|w| w.contains("scope user")),
        "{:?}",
        plan.warnings
    );
    assert!(
        plan.diff
            .contains("+  \"env\": {\n+    \"CLAUDE_CODE_DISABLE_AUTO_MEMORY\": \"1\""),
        "{}",
        plan.diff
    );
}

#[test]
fn an_explicit_scope_picks_the_knob_in_that_scope() {
    let sb = Sandbox::new();
    let local = sb.plan(SLACK, None).unwrap();
    assert_eq!(local.ops[0].file.as_deref(), Some(sb.local().as_path()));
    let user = sb.plan(SLACK, Some(Scope::User)).unwrap();
    assert_eq!(user.ops[0].file.as_deref(), Some(sb.user().as_path()));
    // No project-scope knob: the narrower local one is used, never the wider user one.
    let project = sb.plan(SLACK, Some(Scope::Project)).unwrap();
    assert_eq!(project.ops[0].file.as_deref(), Some(sb.local().as_path()));
}

#[test]
fn knob_none_is_refused_with_its_reason() {
    let sb = Sandbox::new();
    let err = sb.plan(SYSTEM_PROMPT, None).unwrap_err();
    assert!(refused(&err).contains("built-in"), "{err:#}");
}

// ---------------------------------------------------------------------------------------------
// harn-plan/1 requests

#[test]
fn request_rejects_unknown_keys_at_parse_time() {
    let text = r#"{"schema":"harn-plan/1","target_dir":"/x","ops":[{"selector":"a","reason":"r","knobs":1}]}"#;
    assert!(serde_json::from_str::<PlanRequest>(text).is_err());
    let text = r#"{"schema":"harn-plan/1","target_dir":"/x","ops":[],"dry":true}"#;
    assert!(serde_json::from_str::<PlanRequest>(text).is_err());
}

#[test]
fn request_validation_refuses_the_offending_op() {
    let sb = Sandbox::new();
    let tree = sb.resolve();
    let build = |req: &PlanRequest| plan::build_plan_from_request(&sb.fake, &sb.target, &tree, req);

    let mut bad_schema = sb.request(vec![op(HOOK, "noise")]);
    bad_schema.schema = "harn-plan/2".to_string();
    assert!(refused(&build(&bad_schema).unwrap_err()).contains("schema"));

    let mut elsewhere = sb.request(vec![op(HOOK, "noise")]);
    elsewhere.target_dir = sb.home.clone();
    assert!(refused(&build(&elsewhere).unwrap_err()).contains("plan is for"));

    let err = build(&sb.request(vec![op(".md", "both")])).unwrap_err();
    match err.chain().find_map(|c| c.downcast_ref::<SelectError>()) {
        Some(SelectError::Ambiguous(ids)) => {
            assert_eq!(
                ids,
                &vec![sb.id("CLAUDE.md"), sb.id("rules/a.md"), sb.memory_id()]
            )
        }
        other => panic!("expected Ambiguous, got {other:?}: {err:#}"),
    }

    let err = build(&sb.request(vec![op("nope", "x")])).unwrap_err();
    assert!(matches!(
        err.chain().find_map(|c| c.downcast_ref::<SelectError>()),
        Some(SelectError::NotFound(_))
    ));

    let mut none = op(SYSTEM_PROMPT, "shrink");
    none.knob = Some(0);
    assert!(refused(&build(&sb.request(vec![none])).unwrap_err()).contains("is none: built-in"));

    let mut out_of_range = op(HOOK, "noise");
    out_of_range.knob = Some(5);
    let msg = refused(&build(&sb.request(vec![out_of_range])).unwrap_err());
    assert!(msg.contains("knob 5 is out of range"), "{msg}");

    // knobs[1] of slack is user-scoped: refused unless the op asks for user scope.
    let mut wide = op(SLACK, "unused");
    wide.knob = Some(1);
    let msg = refused(&build(&sb.request(vec![wide.clone()])).unwrap_err());
    assert!(msg.contains("op 1") && msg.contains("user"), "{msg}");
    wide.scope = Some(Scope::User);
    let plan = build(&sb.request(vec![wide])).unwrap();
    assert_eq!(plan.ops[0].file.as_deref(), Some(sb.user().as_path()));

    let msg = refused(&build(&sb.request(vec![op(HOOK, "  ")])).unwrap_err());
    assert!(msg.contains("reason is empty"), "{msg}");

    let msg = refused(&build(&sb.request(vec![op(HOOK, "a"), op(HOOK, "b")])).unwrap_err());
    assert!(
        msg.contains("op 2") && msg.contains("already op 1"),
        "{msg}"
    );

    // The error names the op that failed, not just the first.
    let msg =
        refused(&build(&sb.request(vec![op(HOOK, "ok"), op(SYSTEM_PROMPT, "no")])).unwrap_err());
    assert!(msg.contains("built-in"), "{msg}");
}

#[test]
fn request_merges_every_op_into_one_plan_and_keeps_reasons() {
    let mut sb = Sandbox::new();
    let tree = sb.resolve();
    let req = sb.request(vec![
        op("CLAUDE.md", "project notes are stale"),
        op("plugin:slack:slack", "slack is unused here"),
    ]);
    let resolves = sb.fake.resolves.load(Ordering::SeqCst);
    let plan = plan::build_plan_from_request(&sb.fake, &sb.target, &tree, &req).unwrap();
    assert_eq!(
        sb.fake.resolves.load(Ordering::SeqCst),
        resolves + 1,
        "one after-resolve"
    );
    assert_eq!(plan.freed.known, 1500 + 250);
    assert_eq!(
        plan.ops
            .iter()
            .map(|o| o.reason.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("project notes are stale"),
            Some("slack is unused here")
        ]
    );
    // Both ops patch one file: one diff header, one snapshot.
    assert_eq!(plan.diff.matches("+++ ").count(), 1, "{}", plan.diff);
    let entry = writers::apply(&plan, &mut sb.journal).unwrap();
    assert_eq!(entry.snapshots.len(), 1);
    let written: Value = serde_json::from_str(&read(&sb.local())).unwrap();
    assert_eq!(written["enabledPlugins"]["slack@x"], json!(false));
    assert_eq!(
        written["claudeMdExcludes"],
        json!([sb.project.join("CLAUDE.md")])
    );
    let history = journal::history(&sb.journal).unwrap();
    assert_eq!(
        history[0].ops[1].reason.as_deref(),
        Some("slack is unused here")
    );
}

// ---------------------------------------------------------------------------------------------
// apply + journal

#[test]
fn apply_writes_two_space_json_and_keeps_unrelated_keys_in_order() {
    let mut sb = Sandbox::new();
    sb.off(SLACK, None);
    assert_eq!(
        read(&sb.local()),
        "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls:*)\"\n    ]\n  },\n  \"model\": \"sonnet\",\n  \"enabledPlugins\": {\n    \"slack@x\": false\n  }\n}\n"
    );
    // The agent's view after the write: slack and both of its skills are gone.
    let after = sb.resolve();
    assert!(find(&after, SLACK).is_none());
    assert!(find(&after, "claude-code/skill/plugin/slack:slack-post").is_none());
}

#[test]
fn apply_records_snapshot_entry_and_log() {
    let mut sb = Sandbox::new();
    fs::set_permissions(sb.local(), fs::Permissions::from_mode(0o640)).unwrap();
    let entry = sb.off(SLACK, None);

    let dir = sb.journal.dir.join("journal").join(&entry.id);
    assert!(dir.join("entry.json").is_file());
    assert_eq!(entry.snapshots.len(), 1);
    let snapshot = &entry.snapshots[0];
    assert_eq!(snapshot.path, sb.local());
    assert!(snapshot.file.starts_with(&dir));
    assert_eq!(read(&snapshot.file), LOCAL_BEFORE);
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&snapshot.file), 0o640, "snapshot keeps the mode");
    assert_eq!(mode(&sb.local()), 0o640, "atomic replace keeps the mode");
    assert_eq!(
        snapshot.hash_before,
        writers::hash_bytes(LOCAL_BEFORE.as_bytes())
    );
    assert_eq!(
        snapshot.hash_after,
        Some(writers::hash_bytes(read(&sb.local()).as_bytes()))
    );
    assert_eq!(entry.context.cwd, sb.project);
    assert_eq!(entry.context.capture_kind, journal::KIND_APPLY);
    assert!(entry
        .diff
        .as_deref()
        .unwrap()
        .contains("+    \"slack@x\": false"));
    let on_disk: JournalEntry = serde_json::from_str(&read(&dir.join("entry.json"))).unwrap();
    assert_eq!(on_disk, entry);

    let log = read(&sb.journal.dir.join("ops.jsonl"));
    let lines: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "apply");
    assert_eq!(lines[0]["id"], entry.id.as_str());
    assert_eq!(lines[0]["ops"][0]["item_id"], SLACK);
    // No temp files left next to the target.
    let leftovers: Vec<_> = fs::read_dir(sb.project.join(".claude"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".harn-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn apply_journals_the_agent_version_the_resolver_saw() {
    let mut sb = Sandbox::new();
    // The fake resolver reports no version; stand in for the installed one.
    let mut tree = sb.resolve();
    tree.agent_version = Some("9.9.9".to_string());
    let plan = plan::build_plan(&sb.fake, &sb.target, &tree, SLACK, None).unwrap();
    let entry = writers::apply(&plan, &mut sb.journal).unwrap();
    assert_eq!(entry.context.agent_version.as_deref(), Some("9.9.9"));

    let history = journal::history(&sb.journal).unwrap();
    assert_eq!(history[0].context.agent_version.as_deref(), Some("9.9.9"));
    let out = harn::render::history(&history);
    assert!(out.contains("claude-code 9.9.9"), "{out}");
}

#[test]
fn plan_json_round_trips_agent_version_and_loads_without_it() {
    let sb = Sandbox::new();
    // The fake resolver reports no version; stand in for the installed one.
    let mut tree = sb.resolve();
    tree.agent_version = Some("9.9.9".to_string());
    let plan = plan::build_plan(&sb.fake, &sb.target, &tree, SLACK, None).unwrap();
    assert_eq!(plan.agent_version.as_deref(), Some("9.9.9"));

    let text = serde_json::to_string(&plan).unwrap();
    assert_eq!(
        serde_json::from_str::<plan::ActionPlan>(&text).unwrap(),
        plan
    );
    let mut doc: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["agent_version"], json!("9.9.9"));

    // A plan written before the field existed has no `agent_version` key: it still loads, as None.
    assert!(doc
        .as_object_mut()
        .unwrap()
        .remove("agent_version")
        .is_some());
    let old: plan::ActionPlan = serde_json::from_str(&doc.to_string()).unwrap();
    assert_eq!(old.agent_version, None);
    assert_eq!(
        plan::ActionPlan {
            agent_version: None,
            ..plan.clone()
        },
        old
    );

    // Nor an `items_after` key: it loads as 0.
    // By hand from tree.json: 11 items, every one always or advertised; turning slack's plugin
    // off takes its server with that server's tool and its 2 skills' listing entries.
    assert_eq!(plan.items_after, 11 - 4);
    assert!(doc.as_object_mut().unwrap().remove("items_after").is_some());
    let older: plan::ActionPlan = serde_json::from_str(&doc.to_string()).unwrap();
    assert_eq!(older.items_after, 0);

    // Nor a `leaving` key: it loads as an empty list.
    assert_eq!(plan.leaving.len(), 4);
    assert!(doc.as_object_mut().unwrap().remove("leaving").is_some());
    let oldest: plan::ActionPlan = serde_json::from_str(&doc.to_string()).unwrap();
    assert!(oldest.leaving.is_empty());
    assert_eq!(
        plan::ActionPlan {
            agent_version: None,
            items_after: 0,
            leaving: Vec::new(),
            ..plan.clone()
        },
        oldest
    );
}

#[test]
fn apply_refuses_a_file_changed_since_the_plan() {
    let mut sb = Sandbox::new();
    let plan = sb.plan(SLACK, None).unwrap();
    fs::write(sb.local(), "{\"model\": \"opus\"}").unwrap();
    let err = writers::apply(&plan, &mut sb.journal).unwrap_err();
    assert_eq!(stale(&err), vec![sb.local()]);
    assert_eq!(read(&sb.local()), "{\"model\": \"opus\"}");
    assert!(journal::history(&sb.journal).unwrap().is_empty());
}

#[test]
fn a_failed_apply_is_reverted_and_does_not_block_undo() {
    let mut sb = Sandbox::new();
    let tree = sb.resolve();
    let mut memory = op(&sb.memory_id(), "memory is noise");
    memory.scope = Some(Scope::User);
    let req = sb.request(vec![op("CLAUDE.md", "stale"), memory]);
    let plan = plan::build_plan_from_request(&sb.fake, &sb.target, &tree, &req).unwrap();
    // The second file cannot be written: the first write must be put back.
    let home_claude = sb.home.join(".claude");
    fs::set_permissions(&home_claude, fs::Permissions::from_mode(0o500)).unwrap();
    let result = writers::apply(&plan, &mut sb.journal);
    fs::set_permissions(&home_claude, fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    assert!(format!("{err:#}").contains("reverted"), "{err:#}");
    assert_eq!(read(&sb.local()), LOCAL_BEFORE);
    let history = journal::history(&sb.journal).unwrap();
    assert_eq!(history.len(), 1, "the attempt stays on record");
    assert!(history[0].undone_at.is_some());
    assert!(
        history[0]
            .warnings
            .iter()
            .any(|w| w.contains("apply failed")),
        "{:?}",
        history[0].warnings
    );
    assert!(journal::undo(&mut sb.journal, 1).unwrap().is_empty());
}

#[test]
fn undo_restores_byte_for_byte() {
    let mut sb = Sandbox::new();
    let entry = sb.off(SLACK, None);
    assert_ne!(read(&sb.local()), LOCAL_BEFORE);
    let undone = journal::undo(&mut sb.journal, 1).unwrap();
    assert_eq!(undone.len(), 1);
    assert_eq!(undone[0].id, entry.id);
    assert!(undone[0].undone_at.is_some());
    assert_eq!(read(&sb.local()), LOCAL_BEFORE);
    assert!(find(&sb.resolve(), SLACK).is_some());
    // Nothing left to undo.
    assert!(journal::undo(&mut sb.journal, 1).unwrap().is_empty());
    let log = read(&sb.journal.dir.join("ops.jsonl"));
    assert_eq!(log.lines().count(), 2, "apply + undo");
}

#[test]
fn undo_of_a_created_file_parks_it_instead_of_deleting() {
    let mut sb = Sandbox::new();
    fs::remove_file(sb.local()).unwrap();
    let plan = sb.plan(HOOK, None).unwrap();
    assert_eq!(plan.ops[0].hash_before, None);
    assert!(plan.diff.starts_with("--- /dev/null\n"), "{}", plan.diff);
    let entry = writers::apply(&plan, &mut sb.journal).unwrap();
    assert_eq!(entry.snapshots[0].hash_before, ABSENT);
    assert_eq!(read(&sb.local()), "{\n  \"disableAllHooks\": true\n}\n");
    journal::undo(&mut sb.journal, 1).unwrap();
    assert!(!sb.local().exists());
    let parked = sb
        .journal
        .dir
        .join("journal")
        .join(&entry.id)
        .join("parked/settings.local.json");
    assert_eq!(read(&parked), "{\n  \"disableAllHooks\": true\n}\n");
}

#[test]
fn undo_refuses_a_stale_file_and_writes_nothing() {
    let mut sb = Sandbox::new();
    sb.off(SLACK, None);
    let edited = format!("{} ", read(&sb.local()));
    fs::write(sb.local(), &edited).unwrap();
    let err = journal::undo(&mut sb.journal, 1).unwrap_err();
    assert_eq!(stale(&err), vec![sb.local()]);
    assert_eq!(read(&sb.local()), edited);
    assert!(journal::history(&sb.journal).unwrap()[0]
        .undone_at
        .is_none());
}

#[test]
fn undo_n_chains_entries_on_one_file() {
    let mut sb = Sandbox::new();
    sb.off(SLACK, None);
    sb.off(HOOK, None);
    let undone = journal::undo(&mut sb.journal, 2).unwrap();
    assert_eq!(undone.len(), 2);
    assert_eq!(read(&sb.local()), LOCAL_BEFORE);
}

#[test]
fn quarantine_moves_and_undo_moves_back() {
    let mut sb = Sandbox::new();
    let from = sb.project.join(".claude/skills/foo");
    let to = sb.project.join(".claude/harn-off/skills/foo");
    let skill = read(&from.join("SKILL.md"));
    let entry = sb.off(FOO, None);

    assert!(!from.exists());
    assert_eq!(read(&to.join("SKILL.md")), skill);
    let manifest_path = sb.project.join(".claude/harn-off/manifest.json");
    let manifest: Value = serde_json::from_str(&read(&manifest_path)).unwrap();
    let records = manifest["entries"].as_array().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["from"], json!(from));
    assert_eq!(records[0]["to"], json!(to));
    assert_eq!(records[0]["kind"], "skill");
    assert_eq!(records[0]["journal"], entry.id.as_str());
    assert!(
        find(&sb.resolve(), FOO).is_none(),
        "the agent no longer sees it"
    );
    let diff = entry.diff.as_deref().unwrap();
    assert!(
        diff.contains(&format!("rename to {}", to.display())),
        "{diff}"
    );

    journal::undo(&mut sb.journal, 1).unwrap();
    assert_eq!(read(&from.join("SKILL.md")), skill);
    assert!(!to.exists());
    let manifest: Value = serde_json::from_str(&read(&manifest_path)).unwrap();
    assert_eq!(manifest["entries"], json!([]));
    assert!(find(&sb.resolve(), FOO).is_some());
}

#[test]
fn quarantine_refuses_an_occupied_destination() {
    let mut sb = Sandbox::new();
    let plan = sb.plan(FOO, None).unwrap();
    let to = sb.project.join(".claude/harn-off/skills/foo");
    fs::create_dir_all(&to).unwrap();
    let err = writers::apply(&plan, &mut sb.journal).unwrap_err();
    assert!(refused(&err).contains("already exists"), "{err:#}");
    assert!(sb.project.join(".claude/skills/foo/SKILL.md").is_file());
}

#[test]
fn undo_of_a_quarantine_refuses_when_the_original_place_is_taken() {
    let mut sb = Sandbox::new();
    sb.off(FOO, None);
    let from = sb.project.join(".claude/skills/foo");
    fs::create_dir_all(&from).unwrap();
    let err = journal::undo(&mut sb.journal, 1).unwrap_err();
    assert_eq!(stale(&err), vec![from]);
    assert!(sb
        .project
        .join(".claude/harn-off/skills/foo/SKILL.md")
        .is_file());
}

#[test]
fn history_is_newest_first() {
    let mut sb = Sandbox::new();
    let first = sb.off(SLACK, None);
    let second = sb.off(FOO, None);
    let ids: Vec<String> = journal::history(&sb.journal)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, vec![second.id.clone(), first.id.clone()]);
    journal::undo(&mut sb.journal, 1).unwrap();
    let history = journal::history(&sb.journal).unwrap();
    assert_eq!(history[0].id, second.id);
    assert!(history[0].undone_at.is_some());
    assert!(history[1].undone_at.is_none());
}

#[test]
fn on_reverses_the_entry_that_turned_the_item_off() {
    let mut sb = Sandbox::new();
    let memory_entry = sb.off(&sb.memory_id(), Some(Scope::User));
    let slack_entry = sb.off(SLACK, None);
    let user_after = read(&sb.user());

    // An older entry on another file: `on` undoes just that one.
    let entry = journal::on(&mut sb.journal, &sb.target, "/MEMORY.md").unwrap();
    assert_eq!(entry.id, memory_entry.id);
    assert_ne!(read(&sb.user()), user_after);
    assert!(read(&sb.user()).contains("\"slack@x\": true"));
    assert!(!read(&sb.user()).contains("CLAUDE_CODE_DISABLE_AUTO_MEMORY"));
    let history = journal::history(&sb.journal).unwrap();
    let live: Vec<&str> = history
        .iter()
        .filter(|e| e.undone_at.is_none())
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(live, vec![slack_entry.id.as_str()]);

    // Matched against journaled ids, by suffix.
    let entry = journal::on(&mut sb.journal, &sb.target, "plugin:slack:slack").unwrap();
    assert_eq!(entry.id, slack_entry.id);
    assert_eq!(read(&sb.local()), LOCAL_BEFORE);

    let err = journal::on(&mut sb.journal, &sb.target, "plugin:slack:slack").unwrap_err();
    assert!(matches!(
        err.downcast_ref::<SelectError>(),
        Some(SelectError::NotFound(_))
    ));
}

#[test]
fn on_with_an_ambiguous_selector_lists_the_journaled_candidates() {
    let mut sb = Sandbox::new();
    sb.off(&sb.id("CLAUDE.md"), None);
    sb.off(&sb.memory_id(), Some(Scope::User));
    let err = journal::on(&mut sb.journal, &sb.target, ".md").unwrap_err();
    match err.downcast_ref::<SelectError>() {
        Some(SelectError::Ambiguous(ids)) => {
            assert_eq!(ids, &vec![sb.id("CLAUDE.md"), sb.memory_id()])
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
    // Another target dir sees none of this project's entries.
    let mut elsewhere = sb.target.clone();
    elsewhere.dir = sb.home.clone();
    let err = journal::on(&mut sb.journal, &elsewhere, "CLAUDE.md").unwrap_err();
    assert!(matches!(
        err.downcast_ref::<SelectError>(),
        Some(SelectError::NotFound(_))
    ));
}

#[test]
fn on_of_an_older_entry_sharing_a_file_is_stale() {
    // Snapshot restore cannot skip a newer write to the same file: reported, not forced.
    let mut sb = Sandbox::new();
    sb.off(SLACK, None);
    sb.off(HOOK, None);
    let before = read(&sb.local());
    let err = journal::on(&mut sb.journal, &sb.target, "plugin:slack:slack").unwrap_err();
    assert_eq!(stale(&err), vec![sb.local()]);
    assert_eq!(read(&sb.local()), before);
}

#[test]
fn prune_deletes_only_entries_older_than_the_age() {
    let mut sb = Sandbox::new();
    let old = sb.off(SLACK, None);
    let new = sb.off(FOO, None);
    let old_dir = sb.journal.dir.join("journal").join(&old.id);
    let mut aged: Value = serde_json::from_str(&read(&old_dir.join("entry.json"))).unwrap();
    aged["applied_at"] = json!("2020-01-01T00:00:00Z");
    fs::write(old_dir.join("entry.json"), aged.to_string()).unwrap();

    let pruned = journal::prune(&mut sb.journal, Duration::from_secs(30 * 86_400)).unwrap();
    assert_eq!(
        pruned.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        vec![old.id.as_str()]
    );
    assert!(!old_dir.exists());
    let ids: Vec<String> = journal::history(&sb.journal)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, vec![new.id.clone()]);
    let log = read(&sb.journal.dir.join("ops.jsonl"));
    assert!(!log.contains(&old.id));
    assert!(log.contains(&new.id));
    // The changes themselves stay applied: prune forgets, it does not revert.
    assert!(read(&sb.local()).contains("\"slack@x\": false"));
}

// ---------------------------------------------------------------------------------------------
// edit

fn file_item(id: &str, path: &Path, layer: Layer) -> Item {
    Item {
        id: id.to_string(),
        label: path.display().to_string(),
        kind: Kind::Instruction,
        origin: Origin {
            layer,
            path: Some(path.to_path_buf()),
            setting: None,
            plugin: None,
            why: Vec::new(),
        },
        mode: Mode::Always,
        cost: Cost::default(),
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

#[test]
fn edit_journals_the_diff_and_undo_restores() {
    let mut sb = Sandbox::new();
    let path = sb.project.join("rules/a.md");
    let before = read(&path);
    let item = find(&sb.resolve(), &sb.id("rules/a.md")).unwrap().clone();
    let editor = sb.editor("append.sh", "printf 'added by the editor\\n' >> \"$1\"");
    // The fake resolver reports no version; stand in for the installed one.
    let entry = journal::edit(
        &item,
        &sb.target,
        &mut sb.journal,
        &editor,
        Some("9.9.9".to_string()),
    )
    .unwrap();
    assert!(entry
        .diff
        .as_deref()
        .unwrap()
        .contains("+added by the editor"));
    assert!(entry.warnings.is_empty(), "{:?}", entry.warnings);
    assert_eq!(entry.context.capture_kind, journal::KIND_EDIT);
    // The target the item was resolved for, not the shell's working directory.
    assert_eq!(entry.context.cwd, sb.target.dir);
    assert_eq!(entry.context.agent_version.as_deref(), Some("9.9.9"));
    let history = journal::history(&sb.journal).unwrap();
    assert_eq!(history[0].context.agent_version.as_deref(), Some("9.9.9"));
    let out = harn::render::history(&history);
    assert!(out.contains("claude-code 9.9.9"), "{out}");
    assert_eq!(read(&entry.snapshots[0].file), before);
    journal::undo(&mut sb.journal, 1).unwrap();
    assert_eq!(read(&path), before);
}

#[test]
fn edit_that_breaks_json_is_journaled_with_a_warning_not_reverted() {
    let mut sb = Sandbox::new();
    let item = file_item("claude-code/x/local/settings", &sb.local(), Layer::Local);
    let editor = sb.editor("break.sh", "printf '{broken' > \"$1\"");
    let entry = journal::edit(&item, &sb.target, &mut sb.journal, &editor, None).unwrap();
    assert_eq!(read(&sb.local()), "{broken");
    assert!(
        entry
            .warnings
            .iter()
            .any(|w| w.contains("no longer parses")),
        "{:?}",
        entry.warnings
    );
    journal::undo(&mut sb.journal, 1).unwrap();
    assert_eq!(read(&sb.local()), LOCAL_BEFORE);
}

#[test]
fn edit_refuses_builtin_managed_and_pathless_items_and_skips_no_change() {
    let mut sb = Sandbox::new();
    let path = sb.project.join("CLAUDE.md");
    for layer in [Layer::Builtin, Layer::Managed] {
        let err = journal::edit(
            &file_item("x", &path, layer),
            &sb.target,
            &mut sb.journal,
            "true",
            None,
        )
        .unwrap_err();
        refused(&err);
    }
    let mut pathless = file_item("x", &path, Layer::User);
    pathless.origin.path = None;
    refused(&journal::edit(&pathless, &sb.target, &mut sb.journal, "true", None).unwrap_err());

    let err = journal::edit(
        &file_item("x", &path, Layer::Project),
        &sb.target,
        &mut sb.journal,
        "true",
        None,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("unchanged"), "{err:#}");
    assert!(journal::history(&sb.journal).unwrap().is_empty());
}

#[test]
fn why_lists_a_knob_that_serves_two_scopes_once() {
    // slack: a local and a user knob. `project` falls back to the local knob (narrowest below),
    // so three scope requests are two plans.
    let sb = Sandbox::new();
    let tree = sb.resolve();
    // The duplicate a plain per-scope loop lists: `project` plans the local file again.
    let project = plan::build_plan(&sb.fake, &sb.target, &tree, SLACK, Some(Scope::Project));
    assert_eq!(project.unwrap().ops[0].file, Some(sb.local()));
    let plans = plan::plans_by_scope(&sb.fake, &sb.target, &tree, SLACK).unwrap();
    let files: Vec<Option<PathBuf>> = plans.iter().map(|p| p.ops[0].file.clone()).collect();
    assert_eq!(files, [Some(sb.local()), Some(sb.user())]);
    // An item with one knob: one plan; with none: no plan, no error.
    assert_eq!(
        plan::plans_by_scope(&sb.fake, &sb.target, &tree, HOOK)
            .unwrap()
            .len(),
        1
    );
    assert!(
        plan::plans_by_scope(&sb.fake, &sb.target, &tree, SYSTEM_PROMPT)
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------------------------------------
// annotate_freed

#[test]
fn annotate_freed_fills_local_defaults_and_memoizes_by_overlay() {
    let sb = Sandbox::new();
    let mut tree = sb.resolve();
    let before = sb.fake.resolves.load(Ordering::SeqCst);
    plan::annotate_freed(&sb.fake, &sb.target, &mut tree).unwrap();
    // CLAUDE.md, hook, foo, and ONE resolve shared by slack and its two skills.
    assert_eq!(sb.fake.resolves.load(Ordering::SeqCst) - before, 4);

    let freed = |id: &str| find(&tree, id).unwrap().freed.clone();
    assert_eq!(
        freed(&sb.id("CLAUDE.md")),
        Some(Freed {
            known: 1500,
            delta_unknown: false,
            is_estimate: false
        })
    );
    let slack = freed(SLACK).unwrap();
    assert_eq!((slack.known, slack.delta_unknown), (250, true));
    assert_eq!(
        freed("claude-code/skill/plugin/slack:slack-post"),
        Some(slack)
    );
    // Only a user-scope knob: would widen, so no freed value. No knob: none.
    assert_eq!(freed(&sb.memory_id()), None);
    assert_eq!(freed(SYSTEM_PROMPT), None);
    assert_eq!(freed(&sb.id("rules/a.md")), None);
    assert!(tree.warnings.is_empty(), "{:?}", tree.warnings);
}

#[test]
fn annotate_freed_caps_the_work_on_a_large_tree() {
    let sb = Sandbox::new();
    let settings = sb.local();
    let children: Vec<Value> = (0..450)
        .map(|n| {
            json!({
                "id": format!("claude-code/skill/user/s{n}"),
                "label": format!("s{n}"),
                "kind": "skill",
                "layer": "user",
                "mode": "advertised",
                "predicted": 10,
                "knobs": [{"type": "setting", "file": settings, "pointer": format!("/skillOverrides/s{n}"),
                           "value": "off", "scope": "local", "note": null}],
                "off_if": [{"file": settings, "pointer": format!("/skillOverrides/s{n}"), "equals": "off"}]
            })
        })
        .collect();
    let spec = json!([{
        "id": "claude-code/skill-listing/builtin/skills",
        "label": "skill listing",
        "kind": "skill-listing",
        "layer": "builtin",
        "mode": "advertised",
        "sum_children": true,
        "children": children
    }]);
    let fake = Fake {
        spec: spec.to_string(),
        resolves: AtomicUsize::new(0),
    };
    let mut tree = fake.resolve(&sb.target, None).unwrap();
    plan::annotate_freed(&fake, &sb.target, &mut tree).unwrap();
    assert_eq!(
        fake.resolves.load(Ordering::SeqCst),
        1 + plan::ANNOTATE_CHILDREN_CAP
    );
    let annotated = harn::model::walk(&tree)
        .filter(|i| i.freed.is_some())
        .count();
    assert_eq!(annotated, plan::ANNOTATE_CHILDREN_CAP);
    let first = tree.items[0].children[0].freed.clone().unwrap();
    assert_eq!((first.known, first.delta_unknown), (10, true));
    assert!(
        tree.warnings
            .iter()
            .any(|w| w.contains("freed computed for 50 of 450 items")),
        "{:?}",
        tree.warnings
    );
}
