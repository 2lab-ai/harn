//! P0.5 C5: the Codex oracle's capture dir and `verify_codex`, on `tests/fixtures/capture-codex`
//! (hand-built in the shape codex-cli 0.160.1 prints: `debug prompt-input`, `mcp list --json`,
//! `plugin list --json`, `debug models`, `--version`; `@REPO@` / `@HOME@` become the temp
//! fixture's paths). The static tree is the Codex resolver's on a temp fixture whose files the
//! capture renders, plus rows in the shape the skills/plugins (C2) and hooks/rules/MCP (C3)
//! resolvers emit. `/etc/codex` is assumed absent, as in `tests/resolve_codex.rs`.

use harn::model::{item_id, walk, Agent, Cost, Item, Kind, Layer, Mode, Origin, Target, Tree};
use harn::oracle::codex::{self as oracle, CodexCapture};
use harn::resolve::{Codex, Resolver};
use harn::verify::{
    verify_codex, RowVerdict, VerifyReport, CATALOG_GATED, OUTSIDE_THE_DUMP, RUNTIME_DECIDED,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fx {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
    sub: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `home/.codex` with a global AGENTS.md and `developer_instructions`; `repo/` (a git checkout)
/// with an AGENTS.md in the root and in `sub/`, the target. The collaboration-mode and
/// multi-agent blocks are switched off: the capture has neither.
fn fx() -> Fx {
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "harn-verify-codex-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let home = root.join("home");
    let repo = root.join("repo");
    let sub = repo.join("sub");
    write(
        &home.join(".codex/config.toml"),
        "developer_instructions = \"Answer briefly.\"\ninclude_collaboration_mode_instructions = false\n\n[agents]\nenabled = false\n",
    );
    write(&home.join(".codex/AGENTS.md"), "Global rules.\n");
    write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&repo.join("AGENTS.md"), "Root rules.\n");
    write(&sub.join("AGENTS.md"), "Sub rules.\n");
    Fx {
        root,
        home,
        repo,
        sub,
    }
}

fn target(f: &Fx) -> Target {
    Target {
        dir: f.sub.clone(),
        home: f.home.clone(),
        env: BTreeMap::new(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    }
}

fn id(kind: Kind, layer: Layer, name: &str) -> String {
    item_id(Agent::Codex, kind, layer, name)
}

fn row(kind: Kind, layer: Layer, name: &str, mode: Mode, cost: Cost) -> Item {
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
        cost,
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

fn exact(bytes: u64) -> Cost {
    Cost {
        source: Some(bytes),
        predicted: Some(bytes),
        ..Cost::default()
    }
}

fn unknown() -> Cost {
    Cost::default()
}

fn na() -> Cost {
    Cost {
        not_applicable: true,
        ..Cost::default()
    }
}

const ALPHA: &str = "- alpha: The alpha skill. (file: r0/alpha/SKILL.md)";
const BETA: &str = "- demo-plugin:beta: The beta skill. (file: r1/beta/SKILL.md)";

/// The fixture copied to `<root>/capture` with the paths filled in, `edit` applied to the parsed
/// `prompt-input.json` (an array of response items).
fn capture_dir(f: &Fx, edit: impl FnOnce(&mut Value)) -> PathBuf {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capture-codex");
    let to = f.root.join("capture");
    std::fs::create_dir_all(&to).unwrap();
    for entry in std::fs::read_dir(&from).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("@REPO@", &f.repo.display().to_string())
            .replace("@HOME@", &f.home.display().to_string());
        std::fs::write(to.join(path.file_name().unwrap()), text).unwrap();
    }
    let prompt = to.join("prompt-input.json");
    let mut items: Value =
        serde_json::from_str(&std::fs::read_to_string(&prompt).unwrap()).unwrap();
    edit(&mut items);
    std::fs::write(&prompt, serde_json::to_string_pretty(&items).unwrap()).unwrap();
    to
}

/// A block of the unedited fixture, with the paths filled in.
fn pristine_block(f: &Fx, name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/capture-codex/prompt-input.json");
    let text = std::fs::read_to_string(path)
        .unwrap()
        .replace("@REPO@", &f.repo.display().to_string())
        .replace("@HOME@", &f.home.display().to_string());
    oracle::parse_prompt_input(&text)
        .unwrap()
        .into_iter()
        .find(|b| b.name == name)
        .unwrap()
        .text
}

fn capture(f: &Fx, edit: impl FnOnce(&mut Value)) -> CodexCapture {
    oracle::read_capture_dir(&capture_dir(f, edit), &target(f)).expect("capture binds")
}

/// The text of content item `part` of message `input`.
fn text_mut(items: &mut Value, input: usize, part: usize) -> &mut Value {
    &mut items[input]["content"][part]["text"]
}

fn replace_in(items: &mut Value, input: usize, part: usize, from: &str, to: &str) {
    let text = text_mut(items, input, part);
    let new = text.as_str().unwrap().replacen(from, to, 1);
    assert_ne!(text.as_str().unwrap(), new, "{from:?} not found");
    *text = Value::String(new);
}

/// The Codex resolver's tree for the fixture, plus rows shaped like C2's skills/plugins and
/// C3's hooks/rules/MCP rows, predicting what the unedited capture shows.
fn statics(f: &Fx) -> Tree {
    let mut tree = Codex.resolve(&target(f), None).expect("resolve");
    let block = pristine_block(f, "skills_instructions");
    let entries = (ALPHA.len() + 1 + BETA.len() + 1) as u64;
    tree.items.push(row(
        Kind::SystemPrompt,
        Layer::Builtin,
        "skills_instructions",
        Mode::Always,
        exact(block.len() as u64 - entries),
    ));
    let mut listing = row(
        Kind::SkillListing,
        Layer::Builtin,
        "skills",
        Mode::Advertised,
        exact(entries),
    );
    let plugin_root = f
        .home
        .join(".codex/plugins/cache/demo-mkt/demo-plugin/local/skills");
    for (layer, name, line, path, plugin) in [
        (
            Layer::User,
            "alpha",
            ALPHA,
            f.home.join(".codex/skills/alpha/SKILL.md"),
            None,
        ),
        (
            Layer::Plugin,
            "demo-plugin:beta",
            BETA,
            plugin_root.join("beta/SKILL.md"),
            Some("demo-plugin@demo-mkt"),
        ),
    ] {
        let mut skill = row(
            Kind::Skill,
            layer,
            name,
            Mode::Advertised,
            exact(line.len() as u64 + 1),
        );
        skill.origin.why = vec![format!("rendered: {line}")];
        skill.origin.path = Some(path);
        skill.origin.plugin = plugin.map(str::to_string);
        listing.children.push(skill);
    }
    tree.items.push(listing);
    tree.items.push(row(
        Kind::SystemPrompt,
        Layer::Builtin,
        "plugins_instructions",
        Mode::Always,
        Cost {
            predicted: Some(100),
            predicted_is_estimate: true,
            ..Cost::default()
        },
    ));
    tree.items.push(row(
        Kind::Plugin,
        Layer::Plugin,
        "demo-plugin@demo-mkt",
        Mode::Always,
        na(),
    ));
    tree.items.push(row(
        Kind::McpServer,
        Layer::User,
        "demo",
        Mode::Always,
        unknown(),
    ));
    tree.items
        .push(row(Kind::McpServer, Layer::User, "off", Mode::None, na()));
    let hooks = f.home.join(".codex/hooks.json");
    tree.items.push(row(
        Kind::Hook,
        Layer::User,
        &format!("{}:session_start:0:0", hooks.display()),
        Mode::Always,
        unknown(),
    ));
    tree.items.push(row(
        Kind::PermissionRule,
        Layer::User,
        &f.home
            .join(".codex/rules/default.rules")
            .display()
            .to_string(),
        Mode::Always,
        unknown(),
    ));
    tree
}

fn verdict<'a>(report: &'a VerifyReport, id: &str) -> Option<&'a RowVerdict> {
    report.rows.iter().find(|(i, _)| i == id).map(|(_, v)| v)
}

fn failing(report: &VerifyReport) -> Vec<&(String, RowVerdict)> {
    report.rows.iter().filter(|(_, v)| !v.passes()).collect()
}

fn agents(f: &Fx) -> [String; 4] {
    [
        id(Kind::Instruction, Layer::Builtin, "agents-md-message"),
        id(
            Kind::Instruction,
            Layer::User,
            &f.home.join(".codex/AGENTS.md").display().to_string(),
        ),
        id(
            Kind::Instruction,
            Layer::Project,
            &f.repo.join("AGENTS.md").display().to_string(),
        ),
        id(
            Kind::Instruction,
            Layer::Project,
            &f.sub.join("AGENTS.md").display().to_string(),
        ),
    ]
}

fn find<'a>(tree: &'a Tree, id: &str) -> &'a Item {
    walk(tree)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

// ---- the capture dir ------------------------------------------------------------------------

#[test]
fn the_fixture_reads_into_blocks_regions_and_lists() {
    let f = fx();
    let c = capture(&f, |_| {});
    let names: Vec<&str> = c.blocks.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "developer_instructions",
            "skills_instructions",
            "permissions_instructions",
            "plugins_instructions",
            "agents-md-message",
            "environment_context",
        ]
    );
    assert!(c.blocks.iter().all(|b| b.known));
    assert_eq!(
        c.regions[0].name,
        "input[0] developer · developer_instructions"
    );
    assert_eq!(c.regions[4].name, "input[1] user · agents-md-message");
    assert_eq!(
        c.input_bytes,
        c.regions.iter().map(|r| r.bytes).sum::<u64>()
    );
    let skills: Vec<&str> = c.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(skills, vec!["alpha", "demo-plugin:beta"]);
    let servers: Vec<(&str, bool)> = c
        .mcp_servers
        .as_ref()
        .unwrap()
        .iter()
        .map(|s| (s.name.as_str(), s.enabled))
        .collect();
    assert_eq!(servers, vec![("demo", true), ("off", false)]);
    assert_eq!(c.plugins.as_deref(), Some(&[][..]));
    assert_eq!(c.models.as_deref(), Some(&["demo-model".to_string()][..]));
    assert_eq!(c.context.agent_version.as_deref(), Some("0.160.1"));
    assert_eq!(c.context.cwd, f.sub);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[test]
fn a_capture_of_another_directory_is_refused_with_both_named() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    let mut other = target(&f);
    other.dir = f.repo.clone();
    let err = oracle::read_capture_dir(&dir, &other)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not to the target"), "{err}");
    assert!(err.contains(&f.sub.display().to_string()), "{err}");
}

#[test]
fn an_input_list_naming_another_cwd_than_its_manifest_is_refused() {
    let f = fx();
    let dir = capture_dir(&f, |items| {
        replace_in(items, 1, 1, "/sub</cwd>", "</cwd>");
    });
    let err = oracle::read_capture_dir(&dir, &target(&f))
        .unwrap_err()
        .to_string();
    assert!(err.contains("<environment_context> <cwd>"), "{err}");
}

#[test]
fn a_failed_side_command_leaves_its_part_out_with_a_warning() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    let manifest = dir.join(oracle::MANIFEST);
    let text = std::fs::read_to_string(&manifest).unwrap().replacen(
        "\"stdout\": \"mcp-list.json\", \"stderr\": \"mcp-list.stderr\", \"exit\": 0",
        "\"stdout\": \"mcp-list.json\", \"stderr\": \"mcp-list.stderr\", \"exit\": 1",
        1,
    );
    std::fs::write(&manifest, text).unwrap();
    std::fs::write(dir.join("mcp-list.stderr"), "Error: boom\n").unwrap();
    let c = oracle::read_capture_dir(&dir, &target(&f)).unwrap();
    assert_eq!(c.mcp_servers, None);
    assert!(c
        .warnings
        .iter()
        .any(|w| w.contains("exited with 1") && w.contains("boom")));
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(&report, &id(Kind::McpServer, Layer::User, "demo")),
        Some(RowVerdict::NotCheckable { .. })
    ));
}

#[test]
fn an_offline_recheck_says_when_the_environment_changed_since_the_capture() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    let mut t = target(&f);
    t.env
        .insert("HARN_CODEX_PROFILE".to_string(), "work".to_string());
    let manifest = dir.join(oracle::MANIFEST);
    let text = std::fs::read_to_string(&manifest).unwrap().replacen(
        "\"settings_fingerprint\": null",
        "\"settings_fingerprint\": \"sha256:old\"",
        1,
    );
    std::fs::write(&manifest, text).unwrap();
    let c = oracle::read_capture_dir(&dir, &t).unwrap();
    assert!(
        c.warnings
            .iter()
            .any(|w| w == "HARN_CODEX_PROFILE was unset at capture time, is work now"),
        "{:?}",
        c.warnings
    );
    assert!(c
        .warnings
        .iter()
        .any(|w| w.contains("settings fingerprint differs")));
}

// ---- gate 1 + gate 2 on the unedited capture ---------------------------------------------------

#[test]
fn the_unedited_capture_passes_and_promotes_the_unknown_blocks() {
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(failing(&report).is_empty(), "{:#?}", report.rows);
    assert!(report.passed);

    let seen = |id: &str| verdict(&report, id) == Some(&RowVerdict::Seen);
    for agents_row in agents(&f) {
        assert!(seen(&agents_row), "{agents_row}");
    }
    let dev = walk(&tree)
        .find(|i| i.id.ends_with("#/developer_instructions"))
        .unwrap()
        .id
        .clone();
    assert!(seen(&dev));
    for row in [
        id(
            Kind::SystemPrompt,
            Layer::Builtin,
            "permissions_instructions",
        ),
        id(Kind::EnvBlock, Layer::Builtin, "environment_context"),
        id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions"),
        id(Kind::SkillListing, Layer::Builtin, "skills"),
        id(Kind::Skill, Layer::User, "alpha"),
        id(Kind::Skill, Layer::Plugin, "demo-plugin:beta"),
        id(Kind::SystemPrompt, Layer::Builtin, "plugins_instructions"),
        id(Kind::McpServer, Layer::User, "demo"),
        // Not in `plugin list` (no marketplace configured), seen through its listed skill.
        id(Kind::Plugin, Layer::Plugin, "demo-plugin@demo-mkt"),
    ] {
        assert!(seen(&row), "{row}: {:?}", verdict(&report, &row));
    }

    // Outside the dump, or decided at runtime: rows of their own, never failures.
    let unchecked: Vec<&str> = report
        .rows
        .iter()
        .filter(|(_, v)| matches!(v, RowVerdict::NotCheckable { .. }))
        .map(|(i, _)| i.as_str())
        .collect();
    assert!(unchecked.contains(&id(Kind::SystemPrompt, Layer::Builtin, "instructions").as_str()));
    assert!(
        unchecked.contains(&id(Kind::SystemPrompt, Layer::Builtin, "apps_instructions").as_str())
    );
    assert!(unchecked.iter().any(|i| i.starts_with("codex/hook/")));
    assert!(unchecked
        .iter()
        .any(|i| i.starts_with("codex/permission-rule/")));
    // Nothing else: the base instructions, a block codex decides at runtime, a hook, a rule.
    let runtime: Vec<String> = RUNTIME_DECIDED
        .iter()
        .map(|(name, _)| id(Kind::SystemPrompt, Layer::Builtin, name))
        .collect();
    for row in &unchecked {
        assert!(
            *row == id(Kind::SystemPrompt, Layer::Builtin, "instructions")
                || runtime.iter().any(|r| r == row)
                || row.starts_with("codex/hook/")
                || row.starts_with("codex/permission-rule/"),
            "{row}"
        );
    }
    // The disabled server is consistent (both off) and not a row.
    assert_eq!(
        verdict(&report, &id(Kind::McpServer, Layer::User, "off")),
        None
    );
    assert_eq!(report.notes[0], OUTSIDE_THE_DUMP);

    // `?` blocks become observed with their measured size; exact rows are promoted as predicted.
    let block = |name: &str| c.blocks.iter().find(|b| b.name == name).unwrap().bytes();
    let permissions = find(
        &tree,
        &id(
            Kind::SystemPrompt,
            Layer::Builtin,
            "permissions_instructions",
        ),
    );
    assert_eq!(permissions.cost.predicted, None);
    assert_eq!(
        permissions.cost.observed,
        Some(block("permissions_instructions"))
    );
    let message = find(&tree, &agents(&f)[0]);
    assert_eq!(message.cost.observed, message.cost.predicted);
    assert_eq!(
        walk(&tree)
            .filter(|i| i.id.starts_with("codex/instruction/"))
            .filter_map(|i| i.cost.observed)
            .sum::<u64>()
            - find(&tree, &dev).cost.observed.unwrap(),
        block("agents-md-message")
    );
    assert_eq!(
        tree.observed_body, None,
        "the input list is not the request"
    );
    assert!(tree
        .signals
        .iter()
        .any(|s| s.contains("input list observed") && s.contains("not in it")));
}

#[test]
fn the_report_states_what_is_outside_the_dump() {
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let text = harn::render::verify_report(&report);
    assert!(text.contains("\nnot-checkable ("), "{text}");
    assert!(
        text.contains("tools[]") && text.contains("hook output"),
        "{text}"
    );
    assert!(text.contains("base instructions"), "{text}");
    assert!(text.ends_with(&format!(
        "promoted: {} row(s) predicted → observed",
        report.promoted
    )));
}

// ---- the AGENTS.md message, byte for byte ----------------------------------------------------

#[test]
fn a_file_differing_mid_text_is_a_content_mismatch_at_its_offset() {
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(items, 1, 0, "Root rules.", "Root rulez, longer.")
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, global, root, sub] = agents(&f);
    assert_eq!(
        verdict(&report, &root),
        Some(&RowVerdict::ContentMismatch {
            predicted: 12,
            observed: Some(20)
        })
    );
    for other in [&message, &global, &sub] {
        assert_eq!(verdict(&report, other), Some(&RowVerdict::Seen), "{other}");
    }
    let note = report.notes.iter().find(|n| n.starts_with(&root)).unwrap();
    assert!(note.contains("offset 9 of the file's text"), "{note}");
    assert!(note.contains("\"z, longer."), "{note}");
    assert_eq!(failing(&report).len(), 1);
    assert!(!report.passed);
}

#[test]
fn a_missing_middle_file_is_predicted_not_seen_and_the_next_still_seen() {
    let f = fx();
    let c = capture(&f, |items| replace_in(items, 1, 0, "Root rules.\n\n\n", ""));
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, global, root, sub] = agents(&f);
    assert_eq!(verdict(&report, &root), Some(&RowVerdict::PredictedNotSeen));
    assert_eq!(verdict(&report, &sub), Some(&RowVerdict::Seen));
    assert_eq!(verdict(&report, &global), Some(&RowVerdict::Seen));
    assert_eq!(verdict(&report, &message), Some(&RowVerdict::Seen));
    assert_eq!(failing(&report).len(), 1, "{:#?}", failing(&report));
}

#[test]
fn text_harn_did_not_predict_is_a_message_mismatch_at_its_offset() {
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(
            items,
            1,
            0,
            "Sub rules.\n",
            "Sub rules.\n\n\nFallback rules.\n",
        );
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, global, root, sub] = agents(&f);
    for file in [&global, &root, &sub] {
        assert_eq!(verdict(&report, file), Some(&RowVerdict::Seen), "{file}");
    }
    let predicted = find(&tree, &message).cost.predicted.unwrap();
    assert_eq!(
        verdict(&report, &message),
        Some(&RowVerdict::ContentMismatch {
            predicted,
            observed: Some(predicted + "\n\nFallback rules.\n".len() as u64),
        })
    );
    // Predicted `…Sub rules.\n` + `\n</INSTRUCTIONS>`; observed `…Sub rules.\n\n\nFallback…`.
    let at = c
        .block("agents-md-message")
        .unwrap()
        .text
        .find("\nFallback")
        .unwrap();
    let note = report
        .notes
        .iter()
        .find(|n| n.starts_with(&message))
        .unwrap();
    assert!(
        note.contains(&format!("offset {at} of the AGENTS.md message")),
        "{note}"
    );
}

#[test]
fn a_same_length_framing_change_is_still_a_mismatch() {
    // The byte counts agree; only the comparison of the text catches it. (The header's directory
    // is binding evidence: a capture whose header names another directory is refused, r1.)
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(items, 1, 0, "--- project-doc ---", "--- project-dox ---")
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, global, root, sub] = agents(&f);
    let predicted = find(&tree, &message).cost.predicted.unwrap();
    assert_eq!(
        verdict(&report, &message),
        Some(&RowVerdict::ContentMismatch {
            predicted,
            observed: Some(predicted)
        })
    );
    for file in [&global, &root, &sub] {
        assert_eq!(verdict(&report, file), Some(&RowVerdict::Seen), "{file}");
    }
}

#[test]
fn a_message_harn_does_not_predict_is_seen_not_predicted() {
    let f = fx();
    std::fs::remove_file(f.home.join(".codex/AGENTS.md")).unwrap();
    std::fs::remove_file(f.repo.join("AGENTS.md")).unwrap();
    std::fs::remove_file(f.sub.join("AGENTS.md")).unwrap();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(&report, &agents(&f)[0]),
        Some(&RowVerdict::SeenNotPredicted)
    );
}

// ---- the listing, blocks, servers and plugins ----------------------------------------------

#[test]
fn a_listing_line_with_the_same_length_but_other_text_is_a_mismatch_at_its_byte() {
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(items, 0, 1, "The alpha skill.", "The alpha skill!")
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let alpha = id(Kind::Skill, Layer::User, "alpha");
    let bytes = ALPHA.len() as u64 + 1;
    assert_eq!(
        verdict(&report, &alpha),
        Some(&RowVerdict::ContentMismatch {
            predicted: bytes,
            observed: Some(bytes)
        })
    );
    let at = ALPHA.find('.').unwrap();
    let note = report.notes.iter().find(|n| n.starts_with(&alpha)).unwrap();
    assert!(note.contains(&format!("differs from byte {at}")), "{note}");
}

#[test]
fn an_unlisted_skill_and_an_unknown_block_are_seen_not_predicted() {
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(
            items,
            0,
            1,
            "### How to use skills",
            "- gamma: Not in the tree. (file: r0/gamma/SKILL.md)\n### How to use skills",
        );
        items[0]["content"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"type": "input_text", "text": "<persistent_mode>\nkeep going\n</persistent_mode>"}));
        items[0]["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
            .as_array_mut()
            .unwrap()
            .push(Value::String("persistent_mode.instructions".to_string()));
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let mut failing: Vec<(String, RowVerdict)> = failing(&report).into_iter().cloned().collect();
    failing.sort_by(|a, b| a.0.cmp(&b.0));
    let framing = id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions");
    assert_eq!(
        failing,
        vec![
            (
                id(Kind::Skill, Layer::Unresolved, "gamma"),
                RowVerdict::SeenNotPredicted
            ),
            (
                id(
                    Kind::SystemPrompt,
                    Layer::Unresolved,
                    "persistent_mode.instructions"
                ),
                RowVerdict::SeenNotPredicted
            ),
        ],
        "the framing ({framing}) keeps its bytes: the new line is an entry"
    );
}

#[test]
fn a_predicted_block_codex_did_not_render_is_predicted_not_seen() {
    // The same as the fixture with the collaboration-mode switch left on (C1 predicts it on by
    // default; codex 0.160.1 renders it only when the model's catalog carries its message).
    let f = fx();
    write(
        &f.home.join(".codex/config.toml"),
        "developer_instructions = \"Answer briefly.\"\n\n[agents]\nenabled = false\n",
    );
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        failing(&report),
        vec![&(
            id(Kind::SystemPrompt, Layer::Builtin, "collaboration_mode"),
            RowVerdict::PredictedNotSeen
        )]
    );
}

#[test]
fn servers_and_plugins_are_matched_against_codex_lists() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    std::fs::write(
        dir.join("mcp-list.json"),
        r#"[{"name":"extra","enabled":true},{"name":"off","enabled":true}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("plugin-list.json"),
        r#"{"installed":[{"pluginId":"demo-plugin@demo-mkt","installed":true,"enabled":false},{"pluginId":"other@demo-mkt","installed":true,"enabled":true}],"available":[]}"#,
    )
    .unwrap();
    let c = oracle::read_capture_dir(&dir, &target(&f)).unwrap();
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let mut failing: Vec<(String, RowVerdict)> = failing(&report).into_iter().cloned().collect();
    failing.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        failing,
        vec![
            (
                id(Kind::McpServer, Layer::Unresolved, "extra"),
                RowVerdict::SeenNotPredicted
            ),
            (
                id(Kind::McpServer, Layer::User, "demo"),
                RowVerdict::PredictedNotSeen
            ),
            // Off in the tree, enabled for codex.
            (
                id(Kind::McpServer, Layer::User, "off"),
                RowVerdict::SeenNotPredicted
            ),
            // Listed, but disabled: its listed skill does not overrule the list.
            (
                id(Kind::Plugin, Layer::Plugin, "demo-plugin@demo-mkt"),
                RowVerdict::PredictedNotSeen
            ),
            (
                id(Kind::Plugin, Layer::Unresolved, "other@demo-mkt"),
                RowVerdict::SeenNotPredicted
            ),
        ]
    );
}

#[test]
fn a_plugin_without_list_entry_or_listed_skill_is_not_checkable() {
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    tree.items.push(row(
        Kind::Plugin,
        Layer::Plugin,
        "quiet@demo-mkt",
        Mode::Always,
        na(),
    ));
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(&report, &id(Kind::Plugin, Layer::Plugin, "quiet@demo-mkt")),
        Some(RowVerdict::NotCheckable { .. })
    ));
    assert!(report.passed);
}

// ---- review round 1: regressions ---------------------------------------------------------------

/// Overwrites `prompt-input.json` of a capture dir with `items`.
fn set_prompt(dir: &Path, items: &Value) {
    std::fs::write(dir.join("prompt-input.json"), items.to_string()).unwrap();
}

#[test]
fn r1_an_input_list_without_cwd_evidence_is_refused() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    set_prompt(&dir, &serde_json::json!([]));
    let err = oracle::read_capture_dir(&dir, &target(&f))
        .expect_err("an empty input list names no working directory")
        .to_string();
    assert!(err.contains("names no working directory"), "{err}");
}

#[test]
fn r1_an_environment_block_without_cwd_falls_back_to_the_agents_header() {
    let f = fx();
    // No usable <cwd>; the AGENTS.md header names another directory: refused.
    let dir = capture_dir(&f, |items| {
        replace_in(items, 1, 1, "<cwd>", "<dir>");
        replace_in(
            items,
            1,
            0,
            "/sub\n\n<INSTRUCTIONS>",
            "/elsewhere\n\n<INSTRUCTIONS>",
        );
    });
    let err = oracle::read_capture_dir(&dir, &target(&f))
        .expect_err("the AGENTS.md header names another directory")
        .to_string();
    assert!(err.contains("AGENTS.md message header"), "{err}");
    // The same header naming the target: bound through it.
    let f = fx();
    let dir = capture_dir(&f, |items| replace_in(items, 1, 1, "<cwd>", "<dir>"));
    oracle::read_capture_dir(&dir, &target(&f)).expect("bound through the AGENTS.md header");
}

#[test]
fn r2_a_manifest_naming_files_outside_the_artifacts_is_refused() {
    let f = fx();
    let outside = f.root.join("secret.txt");
    std::fs::write(&outside, "SECRET-LINE\n").unwrap();
    for stderr in [outside.display().to_string(), "../secret.txt".to_string()] {
        let dir = capture_dir(&f, |_| {});
        let manifest = dir.join(oracle::MANIFEST);
        let text = std::fs::read_to_string(&manifest).unwrap().replacen(
            "\"stdout\": \"models.json\", \"stderr\": \"models.stderr\", \"exit\": 0",
            &format!("\"stdout\": \"models.json\", \"stderr\": {stderr:?}, \"exit\": 1"),
            1,
        );
        std::fs::write(&manifest, text).unwrap();
        match oracle::read_capture_dir(&dir, &target(&f)) {
            Err(err) => assert!(!format!("{err:#}").contains("SECRET-LINE"), "{err:#}"),
            Ok(c) => panic!("accepted {stderr}: {:?}", c.warnings),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[test]
#[cfg(unix)]
fn r2_a_symlinked_artifact_is_refused() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    let real = f.root.join("elsewhere.json");
    std::fs::rename(dir.join("prompt-input.json"), &real).unwrap();
    std::os::unix::fs::symlink(&real, dir.join("prompt-input.json")).unwrap();
    let err = oracle::read_capture_dir(&dir, &target(&f))
        .expect_err("a symlinked prompt-input.json")
        .to_string();
    assert!(err.contains("symlink"), "{err}");
}

#[test]
fn r4_a_message_whose_content_is_not_an_array_is_seen_not_predicted() {
    let f = fx();
    let c = capture(&f, |items| {
        items.as_array_mut().unwrap().push(
            serde_json::json!({"type": "message", "role": "developer", "content": "loose text"}),
        );
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(
            &report,
            &id(Kind::SystemPrompt, Layer::Unresolved, "input[2].content")
        ),
        Some(&RowVerdict::SeenNotPredicted),
        "{:#?}",
        report.rows
    );
}

/// A stand-in `codex`: `--version` fails (exit 1) though it prints a version, `debug
/// prompt-input` prints a minimal input list for its `-C` dir, `debug models` removes its own
/// execute bit so the later commands cannot start.
#[cfg(unix)]
fn fake_codex(f: &Fx) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = f.root.join("bin/codex");
    write(
        &bin,
        r#"#!/bin/sh
dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    -C) dir="$2"; shift 2;;
    --profile) shift 2;;
    *) break;;
  esac
done
case "$1" in
  --version) echo "codex-cli 9.9.9"; exit 1;;
  debug)
    if [ "$2" = "prompt-input" ]; then
      printf '[{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\\n  <cwd>%s</cwd>\\n</environment_context>"}]}]\n' "$dir"
      exit 0
    fi
    /bin/chmod -x "$0"
    echo '{"models":[]}'
    exit 0;;
esac
exit 3
"#,
    );
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

#[test]
#[cfg(unix)]
fn r5_side_command_failures_are_warnings_and_a_failed_version_is_not_read() {
    let f = fx();
    let bin = fake_codex(&f);
    let mut t = target(&f);
    t.env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    let c = oracle::run_codex(&t, &bin).expect("the prompt capture is kept");
    let dir = c.dir.clone().unwrap();
    assert_eq!(c.context.agent_version, None, "--version exited 1");
    assert_eq!(c.blocks.len(), 1);
    assert_eq!(c.mcp_servers, None);
    assert_eq!(c.plugins, None);
    assert!(
        c.warnings.iter().any(|w| w.contains("mcp list")),
        "{:?}",
        c.warnings
    );
    assert!(c.warnings.iter().any(|w| w.contains("plugin list")));
    assert!(dir.join(oracle::MANIFEST).is_file());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn r6_the_runtime_exemption_is_for_blocks_only() {
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    tree.items.push(row(
        Kind::McpServer,
        Layer::User,
        "apps_instructions",
        Mode::Always,
        unknown(),
    ));
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(
            &report,
            &id(Kind::McpServer, Layer::User, "apps_instructions")
        ),
        Some(&RowVerdict::PredictedNotSeen)
    );
}

#[test]
fn r7_a_listing_total_that_disagrees_with_its_entries_is_a_mismatch() {
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let listing = tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    let total = listing.cost.predicted.unwrap();
    listing.cost.predicted = Some(total + 7);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(&report, &id(Kind::SkillListing, Layer::Builtin, "skills")),
        Some(&RowVerdict::ContentMismatch {
            predicted: total + 7,
            observed: Some(total)
        })
    );
    // The framing row is compared by its bytes too.
    let mut tree = statics(&f);
    let framing = tree
        .items
        .iter_mut()
        .find(|i| i.id.ends_with("/skills_instructions"))
        .unwrap();
    let bytes = framing.cost.predicted.unwrap();
    framing.cost.predicted = Some(bytes + 1);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(
            &report,
            &id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions")
        ),
        Some(&RowVerdict::ContentMismatch {
            predicted: bytes + 1,
            observed: Some(bytes)
        })
    );
}

#[test]
fn r8_a_multibyte_mismatch_does_not_lose_the_later_files() {
    let f = fx();
    write(&f.repo.join("AGENTS.md"), "Root rulé.\n");
    let c = capture(&f, |items| {
        replace_in(items, 1, 0, "Root rules.", "Root rulè.")
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, global, root, sub] = agents(&f);
    let bytes = "Root rulé.\n".len() as u64;
    assert_eq!(
        verdict(&report, &root),
        Some(&RowVerdict::ContentMismatch {
            predicted: bytes,
            observed: Some(bytes)
        })
    );
    for other in [&message, &global, &sub] {
        assert_eq!(verdict(&report, other), Some(&RowVerdict::Seen), "{other}");
    }
}

#[test]
fn r9_a_file_whose_end_is_not_established_has_no_observed_size() {
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(
            items,
            1,
            0,
            "Sub rules.\n\n</INSTRUCTIONS>",
            "Sub rulez</INSTRUCTIONS>",
        )
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, _, _, sub] = agents(&f);
    assert_eq!(
        verdict(&report, &sub),
        Some(&RowVerdict::ContentMismatch {
            predicted: "Sub rules.\n".len() as u64,
            observed: None
        })
    );
    assert_eq!(find(&tree, &sub).cost.observed, None);
    assert_eq!(find(&tree, &message).cost.observed, None);
}

#[test]
fn r10_a_plugin_sharing_a_skill_prefix_is_not_seen_through_anothers_skill() {
    // The listing prefixes plugin skills with the manifest name only
    // (core-plugins/src/loader.rs:906), so `demo-plugin:beta` cannot tell two marketplaces apart.
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    tree.items.push(row(
        Kind::Plugin,
        Layer::Plugin,
        "demo-plugin@other-mkt",
        Mode::Always,
        na(),
    ));
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(
            &report,
            &id(Kind::Plugin, Layer::Plugin, "demo-plugin@demo-mkt")
        ),
        Some(&RowVerdict::Seen),
        "its skill row is joined to the listed entry at its own path"
    );
    assert!(matches!(
        verdict(
            &report,
            &id(Kind::Plugin, Layer::Plugin, "demo-plugin@other-mkt")
        ),
        Some(RowVerdict::NotCheckable { .. })
    ));
}

#[test]
fn r11_each_listing_line_is_compared_with_its_own_occurrence() {
    let f = fx();
    let repo_alpha = "- alpha: The repo alpha. (file: r2/alpha/SKILL.md)";
    let c = capture(&f, |items| {
        replace_in(
            items,
            0,
            1,
            "### Available skills\n",
            &format!(
                "- `r2` = `{}`\n### Available skills\n",
                f.repo.join(".agents/skills").display()
            ),
        );
        replace_in(
            items,
            0,
            1,
            "### How to use skills",
            &format!("{repo_alpha}\n### How to use skills"),
        );
    });
    let mut tree = statics(&f);
    let listing = tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    // The repo skill is listed first in the tree; codex lists the user one first.
    let mut repo = row(
        Kind::Skill,
        Layer::Project,
        "alpha",
        Mode::Advertised,
        exact(repo_alpha.len() as u64 + 1),
    );
    repo.origin.why = vec![format!("rendered: {repo_alpha}")];
    repo.origin.path = Some(f.repo.join(".agents/skills/alpha/SKILL.md"));
    listing.children.insert(0, repo);
    listing.cost.predicted = listing
        .cost
        .predicted
        .map(|p| p + repo_alpha.len() as u64 + 1);
    let framing = pristine_block(&f, "skills_instructions").len() as u64;
    let block = c.block("skills_instructions").unwrap().bytes();
    let report = verify_codex(&mut tree, &c);
    for alpha in [
        id(Kind::Skill, Layer::User, "alpha"),
        id(Kind::Skill, Layer::Project, "alpha"),
    ] {
        assert_eq!(verdict(&report, &alpha), Some(&RowVerdict::Seen), "{alpha}");
    }
    // The roots table grew by one line: only the framing differs.
    assert!(block > framing);
    assert_eq!(
        failing(&report)
            .iter()
            .map(|(i, _)| i.as_str())
            .collect::<Vec<_>>(),
        vec![id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions").as_str()]
    );
}

#[test]
fn r12_a_file_is_never_found_inside_the_message_header() {
    // The global file's whole text is `instructions`; codex left it out, and the header says
    // `# AGENTS.md instructions for …`.
    let f = fx();
    write(&f.home.join(".codex/AGENTS.md"), "instructions\n");
    let c = capture(&f, |items| {
        replace_in(items, 1, 0, "Global rules.\n\n--- project-doc ---\n\n", "")
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [_, global, root, sub] = agents(&f);
    assert_eq!(
        verdict(&report, &global),
        Some(&RowVerdict::PredictedNotSeen)
    );
    assert_eq!(verdict(&report, &root), Some(&RowVerdict::Seen));
    assert_eq!(verdict(&report, &sub), Some(&RowVerdict::Seen));
}

#[test]
fn r13_a_catalog_gated_row_codex_did_not_render_is_not_checkable() {
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let mut gated = row(
        Kind::SystemPrompt,
        Layer::Builtin,
        "collaboration_mode",
        Mode::Always,
        unknown(),
    );
    gated.signal = Some(format!(
        "{CATALOG_GATED} rendered only when the model's catalog entry carries its message"
    ));
    tree.items.push(gated);
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(
            &report,
            &id(Kind::SystemPrompt, Layer::Builtin, "collaboration_mode")
        ),
        Some(RowVerdict::NotCheckable { .. })
    ));
    assert!(report.passed, "{:#?}", failing(&report));
}

// ---- review round 2: regressions ---------------------------------------------------------------

/// An edit of the input list, for table-driven cases.
type Edit = Box<dyn Fn(&mut Value)>;

#[test]
fn rr1_every_cwd_occurrence_must_agree() {
    let env_block = "<environment_context>\n  <cwd>/elsewhere</cwd>\n</environment_context>";
    let agents_block =
        "# AGENTS.md instructions for /elsewhere\n\n<INSTRUCTIONS>\nx\n</INSTRUCTIONS>";
    let cases: [(&str, Edit); 3] = [
        (
            "a second environment block",
            Box::new(move |items: &mut Value| {
                items[1]["content"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"type": "input_text", "text": env_block}));
                items[1]["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
                    .as_array_mut()
                    .unwrap()
                    .push(Value::String(
                        "environments.environment_context".to_string(),
                    ));
            }),
        ),
        (
            "a second <cwd> in the block",
            Box::new(|items: &mut Value| {
                replace_in(items, 1, 1, "</cwd>", "</cwd>\n  <cwd>/elsewhere</cwd>")
            }),
        ),
        (
            "a second AGENTS.md message",
            Box::new(move |items: &mut Value| {
                items[1]["content"]
                    .as_array_mut()
                    .unwrap()
                    .push(serde_json::json!({"type": "input_text", "text": agents_block}));
                items[1]["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
                    .as_array_mut()
                    .unwrap()
                    .push(Value::String("agents_md.instructions".to_string()));
            }),
        ),
    ];
    let mut accepted = Vec::new();
    for (case, edit) in cases {
        let f = fx();
        let dir = capture_dir(&f, |items| edit(items));
        match oracle::read_capture_dir(&dir, &target(&f)) {
            Ok(_) => accepted.push(case),
            Err(err) => assert!(err.to_string().contains("/elsewhere"), "{case}: {err}"),
        }
    }
    assert!(accepted.is_empty(), "accepted: {accepted:?}");
}

#[test]
fn rr2_a_file_is_attributed_only_inside_established_body_boundaries() {
    // The closing framing is not at the end: the sub file's text after it is not the body.
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(
            items,
            1,
            0,
            "Root rules.\n\n\nSub rules.\n\n</INSTRUCTIONS>",
            "Root rules.\n\n</INSTRUCTIONS>\n\n\nSub rules.\n",
        )
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, global, root, sub] = agents(&f);
    assert!(
        matches!(
            verdict(&report, &sub),
            Some(RowVerdict::NotCheckable { .. })
        ),
        "{:?}",
        verdict(&report, &sub)
    );
    assert_eq!(find(&tree, &sub).cost.observed, None);
    assert_eq!(verdict(&report, &global), Some(&RowVerdict::Seen));
    assert_eq!(verdict(&report, &root), Some(&RowVerdict::Seen));
    assert!(matches!(
        verdict(&report, &message),
        Some(RowVerdict::ContentMismatch { .. })
    ));

    // The opening framing differs (same length): no file after it is attributed.
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(items, 1, 0, "\n\n<INSTRUCTIONS>\n", "\n\n<INSTRUCTI0NS>\n")
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, global, root, sub] = agents(&f);
    for file in [&global, &root, &sub] {
        assert!(
            matches!(
                verdict(&report, file),
                Some(RowVerdict::NotCheckable { .. })
            ),
            "{file}: {:?}",
            verdict(&report, file)
        );
    }
    assert!(matches!(
        verdict(&report, &message),
        Some(RowVerdict::ContentMismatch { .. })
    ));
    assert!(!report.passed);
}

#[test]
fn rr3_a_listing_total_with_its_own_residual_is_reported_beside_entry_differences() {
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(items, 0, 1, "The alpha skill.", "The alpha skills.")
    });
    // The entry differs by one byte and the total carries 7 bytes of its own.
    let mut tree = statics(&f);
    let listing = tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    let entries = listing.cost.predicted.unwrap();
    listing.cost.predicted = Some(entries + 7);
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(&report, &id(Kind::Skill, Layer::User, "alpha")),
        Some(RowVerdict::ContentMismatch { .. })
    ));
    assert_eq!(
        verdict(&report, &id(Kind::SkillListing, Layer::Builtin, "skills")),
        Some(&RowVerdict::ContentMismatch {
            predicted: entries + 7,
            observed: Some(entries + 1)
        })
    );
    // Without a residual of its own, the entry accounts for the whole difference.
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(&report, &id(Kind::SkillListing, Layer::Builtin, "skills")),
        Some(&RowVerdict::Seen)
    );
}

#[test]
#[cfg(unix)]
fn rr4_a_fifo_manifest_is_refused_without_blocking() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    let manifest = dir.join(oracle::MANIFEST);
    std::fs::remove_file(&manifest).unwrap();
    let c_path = std::ffi::CString::new(manifest.display().to_string()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let t = target(&f);
    let read_dir = dir.clone();
    std::thread::spawn(move || {
        let _ = tx.send(oracle::read_capture_dir(&read_dir, &t).map(|_| ()));
    });
    let result = rx.recv_timeout(std::time::Duration::from_secs(5));
    if result.is_err() {
        // Release the blocked reader before failing.
        let _ = std::fs::OpenOptions::new().write(true).open(&manifest);
    }
    let err = result
        .expect("read_capture_dir blocked on a FIFO")
        .expect_err("a FIFO manifest is refused");
    assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");
}
