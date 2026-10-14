//! P0.5 C5: the Codex oracle's capture dir and `verify_codex`, on `tests/fixtures/capture-codex`
//! (hand-built in the shape codex-cli 0.160.1 prints: `debug prompt-input`, `mcp list --json`,
//! `plugin list --json`, `debug models`, `--version`; `@REPO@` / `@HOME@` become the temp
//! fixture's paths). The static tree is the Codex resolver's on a temp fixture whose files the
//! capture renders, plus rows in the shape the skills/plugins (C2) and hooks/rules/MCP (C3)
//! resolvers emit. `/etc/codex` is assumed absent, as in `tests/resolve_codex.rs`.

use harn::model::{item_id, walk, Agent, Cost, Item, Kind, Layer, Mode, Origin, Target, Tree};
use harn::oracle::codex::{self as oracle, CodexCapture};
use harn::resolve::codex::{BUNDLED_ROOT_PREFIX, ROOT_PREFIX};
use harn::resolve::{Codex, Resolver};
use harn::verify::{
    verify_codex, RowVerdict, VerifyReport, ACCOUNT_PLUGIN, BUNDLED_SKILL, BUNDLED_SKILLS_WHY,
    CATALOG_GATED, OUTSIDE_THE_DUMP, RUNTIME_DECIDED,
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
        .0
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
    statics_for(f, &target(f), true)
}

/// [`statics`] resolved for `t`; without the hand-built skill rows (framing and listing) when
/// `with_skills` is false, so the resolver's own listing (if any) stands alone.
fn statics_for(f: &Fx, t: &Target, with_skills: bool) -> Tree {
    let mut tree = Codex.resolve(t, None).expect("resolve");
    let block = pristine_block(f, "skills_instructions");
    let entries = (ALPHA.len() + 1 + BETA.len() + 1) as u64;
    let mut framing = row(
        Kind::SystemPrompt,
        Layer::Builtin,
        "skills_instructions",
        Mode::Always,
        exact(block.len() as u64 - entries),
    );
    // The roots table the resolver predicts, as it records it.
    framing.origin.why = [
        f.home.join(".codex/skills"),
        f.home
            .join(".codex/plugins/cache/demo-mkt/demo-plugin/local/skills"),
    ]
    .iter()
    .enumerate()
    .map(|(i, root)| format!("{ROOT_PREFIX}- `r{i}` = `{}`", root.display()))
    .collect();
    if with_skills {
        tree.items.push(framing);
    }
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
    if with_skills {
        tree.items.push(listing);
    }
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
fn a_catalog_gated_block_codex_did_not_render_is_not_a_miss() {
    // The same as the fixture with the collaboration-mode switch left on. Codex 0.160.1 renders
    // the block only when the model's catalog carries its message; with no catalog harn marks
    // the row catalog-gated (C7), so its absence from the capture is not checkable, not a miss.
    let f = fx();
    write(
        &f.home.join(".codex/config.toml"),
        "developer_instructions = \"Answer briefly.\"\n\n[agents]\nenabled = false\n",
    );
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(failing(&report).is_empty(), "{:?}", failing(&report));
    assert!(matches!(
        verdict(
            &report,
            &id(Kind::SystemPrompt, Layer::Builtin, "collaboration_mode")
        ),
        Some(RowVerdict::NotCheckable { .. })
    ));
}

/// Removes the `<permissions instructions>` content item of message 0 (and its kind, when
/// `with_kind`); the item's index.
fn drop_permissions(items: &mut Value, with_kind: bool) -> usize {
    let content = items[0]["content"].as_array_mut().unwrap();
    let at = content
        .iter()
        .position(|part| {
            part["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("<permissions instructions>"))
        })
        .expect("the permissions block is in the fixture");
    content.remove(at);
    if with_kind {
        let kinds = items[0]["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
            .as_array_mut()
            .unwrap();
        assert_eq!(kinds.remove(at), "permissions.instructions");
    }
    at
}

#[test]
fn a_predicted_block_codex_did_not_render_is_predicted_not_seen() {
    // The negative control for the catalog-gated case above: a block harn predicts without any
    // catalog condition (permissions) that is missing from the capture is a miss. A real capture
    // without the block has no kind for it either.
    let f = fx();
    let c = capture(&f, |items| {
        drop_permissions(items, true);
    });
    assert!(
        !c.warnings
            .iter()
            .any(|w| w.starts_with("prompt-input.json")),
        "{:?}",
        c.warnings
    );
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(
            &report,
            &id(
                Kind::SystemPrompt,
                Layer::Builtin,
                "permissions_instructions"
            )
        ),
        Some(&RowVerdict::PredictedNotSeen)
    );
    assert!(!report.passed);
}

#[test]
fn a_message_whose_kinds_do_not_pair_with_its_content_is_named_by_markers_with_a_warning() {
    // The content item is gone but its kind is not: paired by index, the plugins text would
    // carry the permissions kind. The kinds are set aside for the message instead.
    let f = fx();
    let c = capture(&f, |items| {
        drop_permissions(items, false);
    });
    let warning = "prompt-input.json: input[0] has 3 content items but 4 content_item_kinds; its blocks are named by their markers, not their kinds";
    assert!(c.warnings.iter().any(|w| w == warning), "{:?}", c.warnings);
    let names: Vec<(&str, bool, Option<&str>)> = c
        .blocks
        .iter()
        .filter(|b| b.input == 0)
        .map(|b| (b.name.as_str(), b.known, b.content_kind.as_deref()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("developer_instructions", true, None),
            ("skills_instructions", true, None),
            ("plugins_instructions", true, None),
        ]
    );
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(
            &report,
            &id(
                Kind::SystemPrompt,
                Layer::Builtin,
                "permissions_instructions"
            )
        ),
        Some(&RowVerdict::PredictedNotSeen)
    );
    assert_eq!(
        verdict(
            &report,
            &id(Kind::SystemPrompt, Layer::Builtin, "plugins_instructions")
        ),
        Some(&RowVerdict::Seen)
    );
    assert!(report.notes.contains(&format!("warning: {warning}")));
    assert!(!report.passed);
}

#[test]
fn intact_content_with_unpairable_kinds_is_a_warning_and_still_passes() {
    // Every part as in the fixture; only a message's kinds are malformed. The markers name the
    // same blocks, so the report is the pristine one (same fixture) plus the warning.
    for input in [0, 1] {
        let parts = [4, 2][input];
        for (edit, warning) in [
            (
                "one kind too many",
                format!("prompt-input.json: input[{input}] has {parts} content items but {} content_item_kinds; its blocks are named by their markers, not their kinds", parts + 1),
            ),
            (
                "a string",
                format!("prompt-input.json: input[{input}]: content_item_kinds is a string, not an array; its blocks are named by their markers, not their kinds"),
            ),
        ] {
            let f = fx();
            let pristine = verify_codex(&mut statics(&f), &capture(&f, |_| {}));
            assert!(pristine.passed);
            let c = capture(&f, |items| {
                let k = &mut items[input]["internal_chat_message_metadata_passthrough"]
                    ["content_item_kinds"];
                if edit == "a string" {
                    *k = Value::String("permissions.instructions".to_string());
                } else {
                    k.as_array_mut()
                        .unwrap()
                        .push(Value::String("permissions.instructions".to_string()));
                }
            });
            assert!(c.warnings.contains(&warning), "{edit}: {:?}", c.warnings);
            let mut tree = statics(&f);
            let report = verify_codex(&mut tree, &c);
            assert!(report.notes.contains(&format!("warning: {warning}")), "{edit}");
            assert_eq!(report.rows, pristine.rows, "input[{input}], {edit}");
            assert_eq!(report.promoted, pristine.promoted, "input[{input}], {edit}");
            assert!(report.passed, "input[{input}], {edit}: a warning alone does not fail");
        }
    }
}

#[test]
fn an_extra_part_whose_kind_and_marker_disagree_is_seen_not_predicted() {
    // Every expected block intact, kinds aligned; one more part says permissions by its kind
    // and plugins by its marker.
    let f = fx();
    let c = capture(&f, |items| {
        items[0]["content"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "type": "input_text",
                "text": "<plugins_instructions>\nAnother.\n</plugins_instructions>"
            }));
        items[0]["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
            .as_array_mut()
            .unwrap()
            .push(Value::String("permissions.instructions".to_string()));
    });
    let warning = "prompt-input.json: input[0].part[4]: its content kind permissions.instructions names permissions_instructions, but its text opens with the marker of plugins_instructions (<plugins_instructions>); named by neither, kept as an unknown block";
    assert!(c.warnings.iter().any(|w| w == warning), "{:?}", c.warnings);
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let unknown = id(
        Kind::SystemPrompt,
        Layer::Unresolved,
        "input[0].part[4] (kind permissions.instructions, marker <plugins_instructions>)",
    );
    assert_eq!(
        failing(&report),
        vec![&(unknown, RowVerdict::SeenNotPredicted)]
    );
    for name in ["permissions_instructions", "plugins_instructions"] {
        assert_eq!(
            verdict(&report, &id(Kind::SystemPrompt, Layer::Builtin, name)),
            Some(&RowVerdict::Seen),
            "{name}"
        );
    }
    assert!(report.notes.contains(&format!("warning: {warning}")));
    assert!(!report.passed);
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
fn rr5_a_file_running_into_the_closing_framing_is_not_seen() {
    // Predicted `…Sub rules.\n` + `\n</INSTRUCTIONS>`; observed `…Sub rules.\n</INSTRUCTIONS>`.
    // The sub file's text is a prefix of what follows the project files, but its last byte is
    // the newline of the closing framing, which ends the message: it is not inside the body.
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(
            items,
            1,
            0,
            "Sub rules.\n\n</INSTRUCTIONS>",
            "Sub rules.\n</INSTRUCTIONS>",
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
    assert!(
        matches!(
            verdict(&report, &message),
            Some(RowVerdict::ContentMismatch { observed: None, .. })
        ),
        "{:?}",
        verdict(&report, &message)
    );
    // The sub file's bytes are not counted as framing either.
    assert_eq!(find(&tree, &message).cost.observed, None);
    assert!(!report.passed);

    // The same file in the unedited message, which is identical to the predicted one: seen.
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let [message, _, _, sub] = agents(&f);
    assert_eq!(verdict(&report, &sub), Some(&RowVerdict::Seen));
    assert_eq!(
        find(&tree, &sub).cost.observed,
        Some("Sub rules.\n".len() as u64)
    );
    assert_eq!(verdict(&report, &message), Some(&RowVerdict::Seen));
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

// ---- bundled skills and account plugins ----------------------------------------------------------

const SYS_CREATOR: &str = "- skill-creator: Create skills. (file: r2/skill-creator/SKILL.md)";
const SYS_PLUGIN: &str = "- plugin-creator: Create plugins. (file: r2/plugin-creator/SKILL.md)";

/// The resolver's signal on a `.system` row (`src/resolve/codex/skills.rs`).
fn bundled_signal() -> String {
    format!(
        "{BUNDLED_SKILL} ~/.codex/skills/.system {BUNDLED_SKILLS_WHY}; the installed codex 0.160.1 replaces it at start when its set's fingerprint differs, so this skill's presence and listing line are confirmed by `harn verify --run --agent codex`"
    )
}

/// The fixture with a `.system` root (`r2`) whose folder, as harn read it, holds `skill-creator`
/// and `plugin-creator`, while the codex of the run lists its own set: `imagegen`, and
/// `skill-creator` with another description (as two codex versions rewriting `.system` leave
/// it). `edit` changes the capture further. The static framing predicts the roots table with the
/// `.system` root, as the resolver does when it lists a `.system` skill.
fn bundled_case(f: &Fx, edit: impl FnOnce(&mut Value)) -> (Tree, CodexCapture) {
    let system = f.home.join(".codex/skills/.system");
    let c = capture(f, |items| {
        replace_in(
            items,
            0,
            1,
            "### Available skills\n",
            &format!("- `r2` = `{}`\n### Available skills\n", system.display()),
        );
        // Catalog order: System first.
        replace_in(
            items,
            0,
            1,
            "### Available skills\n",
            "### Available skills\n- imagegen: Generate images. (file: r2/imagegen/SKILL.md)\n- skill-creator: Create or update skills. (file: r2/skill-creator/SKILL.md)\n",
        );
        edit(items);
    });
    let mut tree = statics(f);
    let entries: u64 = c.skills.iter().map(|s| s.bytes()).sum();
    let observed_framing = c.block("skills_instructions").unwrap().bytes() - entries;
    let framing = tree
        .items
        .iter_mut()
        .find(|i| i.id.ends_with("/skills_instructions"))
        .unwrap();
    framing.cost = exact(observed_framing);
    framing
        .origin
        .why
        .push(format!("{ROOT_PREFIX}- `r2` = `{}`", system.display()));
    framing
        .origin
        .why
        .push(format!("{BUNDLED_ROOT_PREFIX}{}", system.display()));
    let listing = tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    for (name, line) in [
        ("skill-creator", SYS_CREATOR),
        ("plugin-creator", SYS_PLUGIN),
    ] {
        let bytes = line.len() as u64 + 1;
        let mut skill = row(
            Kind::Skill,
            Layer::Builtin,
            name,
            Mode::Advertised,
            exact(bytes),
        );
        skill.origin.why = vec![format!("rendered: {line}")];
        skill.origin.path = Some(system.join(name).join("SKILL.md"));
        skill.signal = Some(bundled_signal());
        listing.children.insert(0, skill);
        listing.cost.predicted = listing.cost.predicted.map(|p| p + bytes);
    }
    (tree, c)
}

fn reason<'a>(report: &'a VerifyReport, id: &str) -> &'a str {
    match verdict(report, id) {
        Some(RowVerdict::NotCheckable { reason }) => reason,
        other => panic!("{id}: not checkable expected, got {other:?}"),
    }
}

#[test]
fn bundled_skills_codex_lists_otherwise_are_not_checkable() {
    let f = fx();
    let (mut tree, c) = bundled_case(&f, |_| {});
    let report = verify_codex(&mut tree, &c);
    assert!(failing(&report).is_empty(), "{:#?}", failing(&report));
    assert!(report.passed);

    let signal = bundled_signal();
    // Predicted from the folder, not listed by codex.
    assert_eq!(
        reason(&report, &id(Kind::Skill, Layer::Builtin, "plugin-creator")),
        format!("not in codex's listing in this run; {signal}")
    );
    // Listed by codex from its `.system` root, not in the folder harn read.
    assert_eq!(
        reason(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
        format!(
            "{BUNDLED_SKILL} listed by codex 0.160.1 from its skills/.system folder, not predicted: that folder {BUNDLED_SKILLS_WHY}"
        )
    );
    // In both, with another line: the line's first differing byte stays in the notes.
    let creator = id(Kind::Skill, Layer::Builtin, "skill-creator");
    assert_eq!(
        reason(&report, &creator),
        format!("codex lists it with another line (see the note); {signal}")
    );
    let at = SYS_CREATOR.find("skills.").unwrap();
    assert!(
        report
            .notes
            .iter()
            .any(|n| n.starts_with(&creator) && n.contains(&format!("differs from byte {at}"))),
        "{:#?}",
        report.notes
    );
    // The listing's total: its entries explain the whole difference.
    assert_eq!(
        verdict(&report, &id(Kind::SkillListing, Layer::Builtin, "skills")),
        Some(&RowVerdict::Seen)
    );
    // The other entries are checked as before.
    for row in [
        id(Kind::Skill, Layer::User, "alpha"),
        id(Kind::Skill, Layer::Plugin, "demo-plugin:beta"),
        id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions"),
    ] {
        assert_eq!(verdict(&report, &row), Some(&RowVerdict::Seen), "{row}");
    }
}

#[test]
fn a_bundled_skill_the_folder_keeps_out_of_the_listing_is_not_checkable_when_listed() {
    // The folder's copy sets `allow_implicit_invocation: false`; the codex of the run lists its
    // own copy.
    let f = fx();
    let (mut tree, c) = bundled_case(&f, |_| {});
    let listing = tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    let mut quiet = row(
        Kind::Skill,
        Layer::Builtin,
        "imagegen",
        Mode::OnDemand,
        na(),
    );
    quiet.origin.path = Some(f.home.join(".codex/skills/.system/imagegen/SKILL.md"));
    quiet.signal = Some(bundled_signal());
    listing.children.insert(0, quiet);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        reason(&report, &id(Kind::Skill, Layer::Builtin, "imagegen")),
        format!(
            "listed by codex in this run, not predicted; {}",
            bundled_signal()
        )
    );
    assert_eq!(
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
        None,
        "joined to the row: not reported twice"
    );
    assert!(report.passed, "{:#?}", failing(&report));
}

#[test]
fn a_bundled_skill_listed_by_absolute_path_is_known_by_the_recorded_root() {
    // Codex names the file in full (no roots-table line for it); the capture records the
    // bundled root the run saw, which proves where it is from.
    let f = fx();
    let path = f.home.join(".codex/skills/.system/imagegen/SKILL.md");
    let dir = capture_dir(&f, |items| {
        replace_in(
            items,
            0,
            1,
            "### Available skills\n",
            &format!(
                "### Available skills\n- imagegen: Generate images. (file: {})\n",
                path.display()
            ),
        )
    });
    edit_manifest(&dir, |m| {
        m["bundled_root"] = Value::from(f.home.join(".codex/skills/.system").display().to_string())
    });
    let c = oracle::read_capture_dir(&dir, &target(&f)).unwrap();
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
        Some(RowVerdict::NotCheckable { .. })
    ));
    assert!(report.passed, "{:#?}", failing(&report));
}

#[test]
fn other_skill_differences_still_fail_beside_bundled_ones() {
    // The negative control: the same capture, plus an entry of the user root harn did not
    // predict and another line for a user skill.
    let f = fx();
    let (mut tree, c) = bundled_case(&f, |items| {
        replace_in(items, 0, 1, "The alpha skill.", "The alpha skill, changed.");
        replace_in(
            items,
            0,
            1,
            "### How to use skills",
            "- gamma: Not in the tree. (file: r0/gamma/SKILL.md)\n### How to use skills",
        );
    });
    let report = verify_codex(&mut tree, &c);
    let mut failing: Vec<(String, RowVerdict)> = failing(&report).into_iter().cloned().collect();
    failing.sort_by(|a, b| a.0.cmp(&b.0));
    let alpha = ALPHA.len() as u64 + 1;
    assert_eq!(
        failing,
        vec![
            (
                id(Kind::Skill, Layer::Unresolved, "gamma"),
                RowVerdict::SeenNotPredicted
            ),
            (
                id(Kind::Skill, Layer::User, "alpha"),
                RowVerdict::ContentMismatch {
                    predicted: alpha,
                    observed: Some(alpha + ", changed".len() as u64)
                }
            ),
        ]
    );
    assert!(!report.passed);
    for row in [
        id(Kind::Skill, Layer::Builtin, "plugin-creator"),
        id(Kind::Skill, Layer::Builtin, "skill-creator"),
        id(Kind::Skill, Layer::Unresolved, "imagegen"),
    ] {
        assert!(
            matches!(
                verdict(&report, &row),
                Some(RowVerdict::NotCheckable { .. })
            ),
            "{row}"
        );
    }
}

/// Writes `plugin-list.json` of a capture dir with `installed` entries and reads the capture.
fn with_plugins(f: &Fx, dir: &Path, installed: Value) -> CodexCapture {
    std::fs::write(
        dir.join("plugin-list.json"),
        serde_json::json!({"installed": installed, "available": []}).to_string(),
    )
    .unwrap();
    oracle::read_capture_dir(dir, &target(f)).unwrap()
}

/// An account plugin as `codex plugin list --json` prints it (ids are synthetic).
fn account_plugin(name: &str) -> Value {
    serde_json::json!({
        "pluginId": format!("{name}@openai-curated-remote"),
        "name": name,
        "marketplaceName": "openai-curated-remote",
        "installed": true,
        "enabled": true,
        "source": {"source": "remote", "id": format!("plugin_connector_{name}_000")}
    })
}

#[test]
fn an_account_plugin_harn_cannot_see_is_not_checkable() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    let c = with_plugins(&f, &dir, serde_json::json!([account_plugin("mail")]));
    assert_eq!(
        c.plugins.as_ref().unwrap()[0].source.as_deref(),
        Some("remote")
    );
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(
            &report,
            &id(
                Kind::Plugin,
                Layer::Unresolved,
                "mail@openai-curated-remote"
            )
        ),
        Some(&RowVerdict::NotCheckable {
            reason: ACCOUNT_PLUGIN.to_string()
        })
    );
    assert!(report.passed, "{:#?}", failing(&report));
}

#[test]
fn a_skill_an_account_plugin_brings_is_still_an_entry_of_its_own() {
    let f = fx();
    let path = f
        .home
        .join(".codex/plugins/cache/openai-curated-remote/mail/1.0.0/skills/search/SKILL.md");
    let dir = capture_dir(&f, |items| {
        replace_in(
            items,
            0,
            1,
            "### How to use skills",
            &format!(
                "- mail:search: Search mail. (file: {})\n### How to use skills",
                path.display()
            ),
        )
    });
    let c = with_plugins(&f, &dir, serde_json::json!([account_plugin("mail")]));
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(
            &report,
            &id(
                Kind::Plugin,
                Layer::Unresolved,
                "mail@openai-curated-remote"
            )
        ),
        Some(RowVerdict::NotCheckable { .. })
    ));
    let failing: Vec<(String, RowVerdict)> = failing(&report).into_iter().cloned().collect();
    assert_eq!(
        failing,
        vec![(
            id(Kind::Skill, Layer::Unresolved, "mail:search"),
            RowVerdict::SeenNotPredicted
        )]
    );
    assert!(!report.passed);
}

#[test]
fn a_local_plugin_harn_did_not_predict_is_still_seen_not_predicted() {
    let f = fx();
    let dir = capture_dir(&f, |_| {});
    let local = serde_json::json!({
        "pluginId": "docs@demo-mkt",
        "name": "docs",
        "marketplaceName": "demo-mkt",
        "installed": true,
        "enabled": true,
        "source": {"source": "local", "path": f.home.join("plugins/docs").display().to_string()}
    });
    let c = with_plugins(&f, &dir, serde_json::json!([account_plugin("mail"), local]));
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let failing: Vec<(String, RowVerdict)> = failing(&report).into_iter().cloned().collect();
    assert_eq!(
        failing,
        vec![(
            id(Kind::Plugin, Layer::Unresolved, "docs@demo-mkt"),
            RowVerdict::SeenNotPredicted
        )]
    );
    assert!(matches!(
        verdict(
            &report,
            &id(
                Kind::Plugin,
                Layer::Unresolved,
                "mail@openai-curated-remote"
            )
        ),
        Some(RowVerdict::NotCheckable { .. })
    ));
}

// ---- the roots table when only one side has the bundled root -------------------------------------

/// The `.system` root's place in codex's roots table: after the user roots, before the plugin
/// ones, so the plugin root of the fixture becomes `r2`.
fn system_root_added(f: &Fx, items: &mut Value) {
    let system = f.home.join(".codex/skills/.system");
    replace_in(
        items,
        0,
        1,
        "- `r1` = `",
        &format!("- `r1` = `{}`\n- `r2` = `", system.display()),
    );
    replace_in(
        items,
        0,
        1,
        "(file: r1/beta/SKILL.md)",
        "(file: r2/beta/SKILL.md)",
    );
    replace_in(
        items,
        0,
        1,
        "### Available skills\n",
        "### Available skills\n- imagegen: Generate images. (file: r1/imagegen/SKILL.md)\n",
    );
}

fn shift_reason(what: &str, side: &str) -> String {
    format!(
        "{BUNDLED_SKILL} {what}: the skills/.system root's line, which {side}, and the aliases it renumbers; that folder {BUNDLED_SKILLS_WHY}"
    )
}

const CODEX_ONLY: &str = "only codex's listing has (harn read no bundled skill)";

#[test]
fn a_bundled_root_only_codex_lists_is_not_a_framing_or_alias_failure() {
    // harn read no `.system` folder (codex has not started here yet); the codex of the run
    // installs one, lists a bundled skill, and the plugin root moves from r1 to r2.
    let f = fx();
    let c = capture(&f, |items| system_root_added(&f, items));
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(failing(&report).is_empty(), "{:#?}", failing(&report));
    assert!(report.passed);
    assert_eq!(
        reason(
            &report,
            &id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions")
        ),
        shift_reason(
            "the framing differs only in its `### Skill roots` table",
            CODEX_ONLY
        )
    );
    assert_eq!(
        reason(&report, &id(Kind::Skill, Layer::Plugin, "demo-plugin:beta")),
        shift_reason("its line differs only by its root's alias", CODEX_ONLY)
    );
    assert!(matches!(
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
        Some(RowVerdict::NotCheckable { .. })
    ));
    for row in [
        id(Kind::Skill, Layer::User, "alpha"),
        id(Kind::SkillListing, Layer::Builtin, "skills"),
        id(Kind::Plugin, Layer::Plugin, "demo-plugin@demo-mkt"),
    ] {
        assert_eq!(verdict(&report, &row), Some(&RowVerdict::Seen), "{row}");
    }
}

#[test]
fn any_other_framing_or_line_difference_beside_the_bundled_root_still_fails() {
    let f = fx();
    let framing = id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions");
    // An unrelated framing edit.
    let c = capture(&f, |items| {
        system_root_added(&f, items);
        replace_in(
            items,
            0,
            1,
            "Below is the list of skills that can be used.",
            "Below are the skills.",
        );
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let failing_ids: Vec<&str> = failing(&report).iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(failing_ids, vec![framing.as_str()]);
    assert!(matches!(
        verdict(&report, &framing),
        Some(RowVerdict::ContentMismatch { .. })
    ));
    assert!(!report.passed);

    // A renumbered line whose text changed too.
    let f = fx();
    let c = capture(&f, |items| {
        system_root_added(&f, items);
        replace_in(items, 0, 1, "The beta skill.", "The beta skill!");
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let failing_ids: Vec<&str> = failing(&report).iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(
        failing_ids,
        vec![id(Kind::Skill, Layer::Plugin, "demo-plugin:beta").as_str()]
    );

    // The roots renumbered with no bundled root: a real difference (as r11).
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(
            items,
            0,
            1,
            "- `r1` = `",
            &format!(
                "- `r1` = `{}`\n- `r2` = `",
                f.repo.join(".agents/skills").display()
            ),
        );
        replace_in(
            items,
            0,
            1,
            "(file: r1/beta/SKILL.md)",
            "(file: r2/beta/SKILL.md)",
        );
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    let mut failing_ids: Vec<&str> = failing(&report).iter().map(|(i, _)| i.as_str()).collect();
    failing_ids.sort();
    assert_eq!(
        failing_ids,
        vec![
            id(Kind::Skill, Layer::Plugin, "demo-plugin:beta").as_str(),
            framing.as_str()
        ]
    );
}

#[test]
fn a_bundled_root_only_harn_read_is_not_a_framing_or_alias_failure() {
    // The other way round: the folder harn read had a bundled skill (r1), codex lists none.
    let f = fx();
    let c = capture(&f, |_| {});
    let mut tree = harn_read_bundled(&f, "imagegen", "Generate images.");
    let report = verify_codex(&mut tree, &c);
    assert!(failing(&report).is_empty(), "{:#?}", failing(&report));
    let side = "only harn's prediction has (codex lists no bundled skill)";
    assert_eq!(
        reason(
            &report,
            &id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions")
        ),
        shift_reason(
            "the framing differs only in its `### Skill roots` table",
            side
        )
    );
    assert_eq!(
        reason(&report, &id(Kind::Skill, Layer::Plugin, "demo-plugin:beta")),
        shift_reason("its line differs only by its root's alias", side)
    );
}

/// [`statics`] where the folder harn read holds one bundled skill: the `.system` root is `r1` of
/// the predicted table (after the user root, before the plugin root, now `r2`).
fn harn_read_bundled(f: &Fx, name: &str, description: &str) -> Tree {
    let system = f.home.join(".codex/skills/.system");
    let mut tree = statics(f);
    let added = format!("- `r1` = `{}`", system.display());
    let framing = tree
        .items
        .iter_mut()
        .find(|i| i.id.ends_with("/skills_instructions"))
        .unwrap();
    framing.cost.predicted = framing.cost.predicted.map(|p| p + added.len() as u64 + 1);
    let plugin_root = framing
        .origin
        .why
        .pop()
        .unwrap()
        .replacen("`r1`", "`r2`", 1);
    framing.origin.why.push(format!("{ROOT_PREFIX}{added}"));
    framing.origin.why.push(plugin_root);
    framing
        .origin
        .why
        .push(format!("{BUNDLED_ROOT_PREFIX}{}", system.display()));
    let listing = tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    let beta = listing
        .children
        .iter_mut()
        .find(|c| c.label == "demo-plugin:beta")
        .unwrap();
    beta.origin.why = vec![format!("rendered: {}", BETA.replacen("r1/", "r2/", 1))];
    let line = format!("- {name}: {description} (file: r1/{name}/SKILL.md)");
    let mut sys = row(
        Kind::Skill,
        Layer::Builtin,
        name,
        Mode::Advertised,
        exact(line.len() as u64 + 1),
    );
    sys.origin.why = vec![format!("rendered: {line}")];
    sys.origin.path = Some(system.join(name).join("SKILL.md"));
    sys.signal = Some(bundled_signal());
    listing.children.insert(0, sys);
    listing.cost.predicted = listing.cost.predicted.map(|p| p + line.len() as u64 + 1);
    tree
}

#[test]
fn a_listing_of_bundled_skills_only_where_harn_predicts_none_is_not_checkable() {
    // harn read no skill at all; the codex of the run lists only its bundled ones.
    let f = fx();
    let without_listing = |f: &Fx| {
        let mut tree = statics(f);
        tree.items
            .retain(|i| !i.id.ends_with("/skills_instructions") && i.kind != Kind::SkillListing);
        tree
    };
    let block = bundled_only_block(&f, "");
    let c = capture(&f, |items| *text_mut(items, 0, 1) = Value::String(block));
    let mut tree = without_listing(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(report.passed, "{:#?}", failing(&report));
    let why = format!(
        "{BUNDLED_SKILL} every entry codex lists is from its skills/.system folder, and harn read no skill to list: that folder {BUNDLED_SKILLS_WHY}"
    );
    for row in [
        id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions"),
        id(Kind::SkillListing, Layer::Builtin, "skills"),
    ] {
        assert_eq!(reason(&report, &row), why, "{row}");
    }

    // One entry of another root: the listing is a prediction miss again.
    let f = fx();
    let block = bundled_only_block(
        &f,
        &format!(
            "- alpha: The alpha skill. (file: {})\n",
            f.home.join(".codex/skills/alpha/SKILL.md").display()
        ),
    );
    let c = capture(&f, |items| *text_mut(items, 0, 1) = Value::String(block));
    let mut tree = without_listing(&f);
    let report = verify_codex(&mut tree, &c);
    let mut failing_ids: Vec<&str> = failing(&report).iter().map(|(i, _)| i.as_str()).collect();
    failing_ids.sort();
    assert_eq!(
        failing_ids,
        vec![
            "codex/skill-listing/builtin/skills",
            "codex/skill/unresolved/alpha",
            "codex/system-prompt/builtin/skills_instructions",
        ]
    );
}

// ---- review round: an exemption needs proof the contribution is bundled ---------------------------

#[test]
fn a_bundled_row_never_joins_a_user_skill_of_the_same_name() {
    // The folder harn read held the old bundled `plugin-creator`; codex lists none, but the user
    // root has a `plugin-creator` of its own.
    let f = fx();
    let c = capture(&f, |items| {
        replace_in(
            items,
            0,
            1,
            "### How to use skills",
            "- plugin-creator: Mine. (file: r0/plugin-creator/SKILL.md)\n### How to use skills",
        )
    });
    let mut tree = harn_read_bundled(&f, "plugin-creator", "Create plugins.");
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        reason(&report, &id(Kind::Skill, Layer::Builtin, "plugin-creator")),
        format!("not in codex's listing in this run; {}", bundled_signal())
    );
    assert_eq!(
        failing(&report)
            .into_iter()
            .cloned()
            .collect::<Vec<(String, RowVerdict)>>(),
        vec![(
            id(Kind::Skill, Layer::Unresolved, "plugin-creator"),
            RowVerdict::SeenNotPredicted
        )]
    );
    assert!(!report.passed);
}

#[test]
fn a_user_row_never_joins_a_bundled_skill_of_the_same_name() {
    // The reverse: harn predicts a user `imagegen`, codex lists only its bundled one.
    let f = fx();
    let c = capture(&f, |items| system_root_added(&f, items));
    let mut tree = statics(&f);
    let listing = tree
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    let line = "- imagegen: Generate images. (file: r0/imagegen/SKILL.md)";
    let mut user = row(
        Kind::Skill,
        Layer::User,
        "imagegen",
        Mode::Advertised,
        exact(line.len() as u64 + 1),
    );
    user.origin.why = vec![format!("rendered: {line}")];
    user.origin.path = Some(f.home.join(".codex/skills/imagegen/SKILL.md"));
    listing.children.push(user);
    listing.cost.predicted = listing.cost.predicted.map(|p| p + line.len() as u64 + 1);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(&report, &id(Kind::Skill, Layer::User, "imagegen")),
        Some(&RowVerdict::PredictedNotSeen)
    );
    assert!(matches!(
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
        Some(RowVerdict::NotCheckable { .. })
    ));
    assert!(!report.passed);
}

#[test]
fn user_skills_of_a_codex_home_under_a_skills_system_dir_are_still_checked() {
    // CODEX_HOME sits inside an unrelated `x/skills/.system/…`: only its own skills/.system is
    // the bundled root (codex checks hidden dirs below the scan root only,
    // ext/skills/src/loader/discovery.rs:179-185).
    let f = fx();
    let nested = f.root.join("x/skills/.system/state/codex-home");
    std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
    std::fs::rename(f.home.join(".codex"), &nested).unwrap();
    let mut t = target(&f);
    t.env
        .insert("CODEX_HOME".to_string(), nested.display().to_string());
    let dir = capture_dir(&f, |items| {
        replace_in(
            items,
            0,
            1,
            &format!("- `r0` = `{}`", f.home.join(".codex/skills").display()),
            &format!("- `r0` = `{}`", nested.join("skills").display()),
        );
        replace_in(
            items,
            0,
            1,
            "### How to use skills",
            "- gamma: Not in the tree. (file: r0/gamma/SKILL.md)\n### How to use skills",
        );
    });
    let manifest = dir.join(oracle::MANIFEST);
    let text = std::fs::read_to_string(&manifest).unwrap().replacen(
        "\"CODEX_HOME\": null",
        &format!("\"CODEX_HOME\": \"{}\"", nested.display()),
        1,
    );
    std::fs::write(&manifest, text).unwrap();
    let c = oracle::read_capture_dir(&dir, &t).unwrap();
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    let mut tree = statics_for(&f, &t, true);
    // The framing harn predicts names the user root under CODEX_HOME, as the resolver does.
    let entries: u64 = c.skills.iter().map(|s| s.bytes()).sum();
    let framing = tree
        .items
        .iter_mut()
        .find(|i| i.id.ends_with("/skills_instructions"))
        .unwrap();
    framing.cost = exact(c.block("skills_instructions").unwrap().bytes() - entries);
    framing.origin.why[0] = format!(
        "{ROOT_PREFIX}- `r0` = `{}`",
        nested.join("skills").display()
    );
    let report = verify_codex(&mut tree, &c);
    // gamma's line is the only difference, and it is checked.
    assert_eq!(
        failing(&report)
            .into_iter()
            .cloned()
            .collect::<Vec<(String, RowVerdict)>>(),
        vec![(
            id(Kind::Skill, Layer::Unresolved, "gamma"),
            RowVerdict::SeenNotPredicted
        )]
    );
    assert!(!report.passed);
}

/// The fixture with `home/.codex/skills/.system/imagegen` hidden from the listing
/// (`allow_implicit_invocation: false`): the folder harn reads lists nothing.
fn quiet_bundled(f: &Fx) {
    let dir = f.home.join(".codex/skills/.system/imagegen");
    write(
        &dir.join("SKILL.md"),
        "---\nname: imagegen\ndescription: Old images.\n---\nbody\n",
    );
    write(
        &dir.join("agents/openai.yaml"),
        "policy:\n  allow_implicit_invocation: false\n",
    );
}

/// The whole `<skills_instructions>` block codex renders with only bundled entries (and `extra`
/// lines after them).
fn bundled_only_block(f: &Fx, extra: &str) -> String {
    format!(
        "<skills_instructions>\n## Skills\nA skill is a set of local instructions.\n### Skill roots\n- `r0` = `{}`\n### Available skills\n- imagegen: Generate images. (file: r0/imagegen/SKILL.md)\n{extra}</skills_instructions>",
        f.home.join(".codex/skills/.system").display()
    )
}

#[test]
fn a_retained_inactive_listing_with_only_bundled_entries_observed_is_not_checkable() {
    // The resolver keeps the listing as `none` (every bundled skill it read is hidden); the
    // codex of the run advertises its own bundled copy.
    let f = fx();
    quiet_bundled(&f);
    let block = bundled_only_block(&f, "");
    let c = capture(&f, |items| *text_mut(items, 0, 1) = Value::String(block));
    let mut tree = statics_for(&f, &target(&f), false);
    let container = find(&tree, &id(Kind::SkillListing, Layer::Builtin, "skills"));
    assert_eq!(container.mode, Mode::None, "the resolver's own container");
    assert_eq!(container.children.len(), 1);
    let report = verify_codex(&mut tree, &c);
    assert!(report.passed, "{:#?}", failing(&report));
    for row in [
        id(Kind::SkillListing, Layer::Builtin, "skills"),
        id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions"),
        id(Kind::Skill, Layer::Builtin, "imagegen"),
    ] {
        assert!(
            matches!(
                verdict(&report, &row),
                Some(RowVerdict::NotCheckable { .. })
            ),
            "{row}: {:?}",
            verdict(&report, &row)
        );
    }

    // An ordinary entry beside it: a prediction miss again.
    let f = fx();
    quiet_bundled(&f);
    let block = bundled_only_block(
        &f,
        &format!(
            "- alpha: The alpha skill. (file: {})\n",
            f.home.join(".codex/skills/alpha/SKILL.md").display()
        ),
    );
    let c = capture(&f, |items| *text_mut(items, 0, 1) = Value::String(block));
    let mut tree = statics_for(&f, &target(&f), false);
    let report = verify_codex(&mut tree, &c);
    let mut failing_ids: Vec<&str> = failing(&report).iter().map(|(i, _)| i.as_str()).collect();
    failing_ids.sort();
    assert_eq!(
        failing_ids,
        vec![
            "codex/skill-listing/builtin/skills",
            "codex/skill/unresolved/alpha",
            "codex/system-prompt/builtin/skills_instructions",
        ]
    );
}

#[test]
fn an_alias_is_exempt_only_when_the_tables_agree_without_the_bundled_root() {
    // Codex adds the bundled root and also an extra repo root harn did not predict: the tables
    // disagree for a reason that is not the bundled root, so beta's moved alias is a mismatch.
    let f = fx();
    let c = capture(&f, |items| {
        system_root_added(&f, items);
        replace_in(
            items,
            0,
            1,
            "### Available skills\n",
            &format!(
                "- `r3` = `{}`\n### Available skills\n",
                f.repo.join(".agents/skills").display()
            ),
        );
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(&report, &id(Kind::Skill, Layer::Plugin, "demo-plugin:beta")),
        Some(RowVerdict::ContentMismatch { .. })
    ));
    assert!(matches!(
        verdict(
            &report,
            &id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions")
        ),
        Some(RowVerdict::ContentMismatch { .. })
    ));
}

#[test]
fn a_permutation_of_two_unrelated_roots_is_not_explained_by_the_bundled_root() {
    // Codex adds the bundled root and lists the user and plugin roots in the other order: the
    // same roots, permuted, so the tables disagree once the bundled line is dropped.
    let f = fx();
    let user = f.home.join(".codex/skills");
    let plugin = f
        .home
        .join(".codex/plugins/cache/demo-mkt/demo-plugin/local/skills");
    let system = f.home.join(".codex/skills/.system");
    let c = capture(&f, |items| {
        let text = text_mut(items, 0, 1);
        let old = text.as_str().unwrap().to_string();
        let table = format!(
            "- `r0` = `{}`\n- `r1` = `{}`\n",
            user.display(),
            plugin.display()
        );
        let permuted = format!(
            "- `r0` = `{}`\n- `r1` = `{}`\n- `r2` = `{}`\n",
            plugin.display(),
            system.display(),
            user.display()
        );
        assert!(old.contains(&table));
        let new = old
            .replacen(&table, &permuted, 1)
            .replacen("(file: r0/alpha/SKILL.md)", "(file: r2/alpha/SKILL.md)", 1)
            .replacen("(file: r1/beta/SKILL.md)", "(file: r0/beta/SKILL.md)", 1)
            .replacen(
                "### Available skills\n",
                "### Available skills\n- imagegen: Generate images. (file: r1/imagegen/SKILL.md)\n",
                1,
            );
        *text = Value::String(new);
    });
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    for row in [
        id(Kind::Skill, Layer::User, "alpha"),
        id(Kind::Skill, Layer::Plugin, "demo-plugin:beta"),
        id(Kind::SystemPrompt, Layer::Builtin, "skills_instructions"),
    ] {
        assert!(
            matches!(
                verdict(&report, &row),
                Some(RowVerdict::ContentMismatch { .. })
            ),
            "{row}: {:?}",
            verdict(&report, &row)
        );
    }
    assert!(!report.passed);
}

// ---- review round 3: the bundled root is bound to the capture, not to the verifier -----------

/// Rewrites the capture manifest of `dir` (parsed JSON).
fn edit_manifest(dir: &Path, edit: impl FnOnce(&mut Value)) {
    let path = dir.join(oracle::MANIFEST);
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut manifest);
    std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
}

/// Adds `- r2 = <root>` to codex's roots table and `name` listed from it, before the others.
fn listed_from(items: &mut Value, root: &Path, name: &str) {
    replace_in(
        items,
        0,
        1,
        "### Available skills\n",
        &format!(
            "- `r2` = `{}`\n### Available skills\n- {name}: Bundled. (file: r2/{name}/SKILL.md)\n",
            root.display()
        ),
    );
}

const RECONSTRUCTED: &str = "bundled skills: the bundled root harn reconstructs for this capture (";

#[test]
fn a_relative_codex_home_resolves_against_the_capture_cwd_not_the_verifier() {
    // Codex ran in `sub` with CODEX_HOME=.codex-home; harn verifies from its own cwd.
    let f = fx();
    let home = f.sub.join(".codex-home");
    std::fs::create_dir_all(home.join("skills/.system/imagegen")).unwrap();
    assert_ne!(std::env::current_dir().unwrap(), f.sub);
    let mut t = target(&f);
    t.env
        .insert("CODEX_HOME".to_string(), ".codex-home".to_string());
    let dir = capture_dir(&f, |items| {
        listed_from(items, &home.join("skills/.system"), "imagegen")
    });
    edit_manifest(&dir, |m| {
        m["env"]["CODEX_HOME"] = Value::from(".codex-home")
    });
    let c = oracle::read_capture_dir(&dir, &t).unwrap();
    assert!(
        !c.warnings.iter().any(|w| w.starts_with(RECONSTRUCTED)),
        "{:?}",
        c.warnings
    );
    assert_eq!(c.bundled_root, Some(home.join("skills/.system")));
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(
        matches!(
            verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
            Some(RowVerdict::NotCheckable { .. })
        ),
        "{:?}",
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen"))
    );
}

#[test]
#[cfg(unix)]
fn a_recorded_bundled_root_wins_over_where_codex_home_points_now() {
    // At capture time CODEX_HOME (a symlink) pointed to home-a; it points to home-b now.
    let f = fx();
    let (a, b) = (f.root.join("home-a"), f.root.join("home-b"));
    for h in [&a, &b] {
        std::fs::create_dir_all(h.join("skills/.system")).unwrap();
    }
    let link = f.root.join("codex-link");
    std::os::unix::fs::symlink(&b, &link).unwrap();
    let mut t = target(&f);
    t.env
        .insert("CODEX_HOME".to_string(), link.display().to_string());
    let dir = capture_dir(&f, |items| {
        listed_from(items, &a.join("skills/.system"), "imagegen");
        replace_in(
            items,
            0,
            1,
            "### How to use skills",
            &format!(
                "- rogue: Today's target. (file: {})\n### How to use skills",
                b.join("skills/.system/rogue/SKILL.md").display()
            ),
        );
    });
    edit_manifest(&dir, |m| {
        m["env"]["CODEX_HOME"] = Value::from(link.display().to_string());
        m["bundled_root"] = Value::from(a.join("skills/.system").display().to_string());
    });
    let c = oracle::read_capture_dir(&dir, &t).unwrap();
    assert_eq!(c.bundled_root, Some(a.join("skills/.system")));
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert!(matches!(
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
        Some(RowVerdict::NotCheckable { .. })
    ));
    assert_eq!(
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "rogue")),
        Some(&RowVerdict::SeenNotPredicted)
    );
}

#[test]
fn a_capture_without_a_recorded_root_still_reads() {
    // The fixture's manifest predates `bundled_root`.
    let f = fx();
    let c = capture(&f, |_| {});
    assert_eq!(c.bundled_root, None, "codex listed no bundled root");
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    // Codex listed the root harn rebuilds (`.codex` in the home, CODEX_HOME unset): bound.
    let c = capture(&f, |items| system_root_added(&f, items));
    assert_eq!(c.bundled_root, Some(f.home.join(".codex/skills/.system")));
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[test]
#[cfg(unix)]
fn a_run_records_the_bundled_root_against_its_cwd() {
    let f = fx();
    let bin = fake_codex(&f);
    std::fs::create_dir_all(f.sub.join(".codex-run")).unwrap();
    let mut t = target(&f);
    t.env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    t.env
        .insert("CODEX_HOME".to_string(), ".codex-run".to_string());
    let c = oracle::run_codex(&t, &bin).expect("the prompt capture is kept");
    let dir = c.dir.clone().unwrap();
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(oracle::MANIFEST)).unwrap())
            .unwrap();
    let recorded = f.sub.join(".codex-run/skills/.system");
    assert_eq!(manifest["schema"], "harn-codex-capture/1");
    assert_eq!(
        manifest["bundled_root"],
        Value::from(recorded.display().to_string())
    );
    assert_eq!(c.bundled_root, Some(recorded));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_reconstructed_root_codex_did_not_list_binds_nothing_and_says_why() {
    // An old capture (no recorded root) whose listing names a bundled-looking file absolutely:
    // the root harn reconstructs is not in codex's roots table, so nothing is bundled.
    let f = fx();
    let path = f.home.join(".codex/skills/.system/imagegen/SKILL.md");
    let c = capture(&f, |items| {
        replace_in(
            items,
            0,
            1,
            "### Available skills\n",
            &format!(
                "### Available skills\n- imagegen: Generate images. (file: {})\n",
                path.display()
            ),
        )
    });
    let warning = c
        .warnings
        .iter()
        .find(|w| w.starts_with(RECONSTRUCTED))
        .unwrap_or_else(|| panic!("{:?}", c.warnings));
    assert!(
        warning.contains(&f.home.join(".codex/skills/.system").display().to_string()),
        "{warning}"
    );
    let mut tree = statics(&f);
    let report = verify_codex(&mut tree, &c);
    assert_eq!(
        verdict(&report, &id(Kind::Skill, Layer::Unresolved, "imagegen")),
        Some(&RowVerdict::SeenNotPredicted)
    );
    assert!(report.notes.contains(&format!("warning: {warning}")));
    assert!(!report.passed);
}
