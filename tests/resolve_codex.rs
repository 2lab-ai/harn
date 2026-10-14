//! Codex resolver against fixtures built per test in a temp dir: `home/.codex` (the Codex home)
//! and `repo/` (a git checkout, `.git/HEAD` present). No file outside the temp dir is read except
//! `/etc/codex`, which these tests assume absent (as the Claude Code tests assume no managed
//! settings). Layering with the system and managed files is covered in `src/resolve/codex`.

use harn::model::{self, Item, Kind, Knob, Layer, Mode, Overlay, OverlayOp, Scope, Target, Tree};
use harn::resolve::codex::{CAP_STATE_UNKNOWN, CATALOG_GATED, PROFILE_ENV};
use harn::resolve::{Codex, Resolver};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fx {
    root: PathBuf,
    home: PathBuf,
    codex: PathBuf,
    repo: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fx() -> Fx {
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "harn-codex-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let home = root.join("home");
    let codex = home.join(".codex");
    let repo = root.join("repo");
    std::fs::create_dir_all(&codex).unwrap();
    write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
    Fx {
        root,
        home,
        codex,
        repo,
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn target_at(f: &Fx, dir: &Path) -> Target {
    std::fs::create_dir_all(dir).unwrap();
    Target {
        dir: dir.to_path_buf(),
        home: f.home.clone(),
        env: BTreeMap::new(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    }
}

fn target(f: &Fx) -> Target {
    target_at(f, &f.repo)
}

fn resolve(t: &Target, overlay: Option<&Overlay>) -> Tree {
    Codex.resolve(t, overlay).expect("resolve")
}

/// `[projects."<dir>"] trust_level = "<level>"` plus `extra` in the user config.
fn user_config(f: &Fx, trust: Option<(&Path, &str)>, extra: &str) {
    let mut text = String::from(extra);
    if let Some((dir, level)) = trust {
        text.push_str(&format!(
            "\n[projects.\"{}\"]\ntrust_level = \"{level}\"\n",
            dir.display()
        ));
    }
    write(&f.codex.join("config.toml"), &text);
}

fn find<'t>(tree: &'t Tree, suffix: &str) -> Option<&'t Item> {
    model::walk(tree).find(|i| i.id.ends_with(suffix))
}

fn row<'t>(tree: &'t Tree, suffix: &str) -> &'t Item {
    find(tree, suffix).unwrap_or_else(|| {
        let ids: Vec<&str> = model::walk(tree).map(|i| i.id.as_str()).collect();
        panic!("no row ending in {suffix}: {ids:#?}")
    })
}

fn project_rows(tree: &Tree) -> Vec<&Item> {
    model::walk(tree)
        .filter(|i| i.kind == Kind::Instruction && i.origin.layer == Layer::Project)
        .collect()
}

fn signal<'t>(tree: &'t Tree, prefix: &str) -> &'t str {
    tree.signals
        .iter()
        .find(|s| s.starts_with(prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` signal: {:?}", tree.signals))
}

fn setting(knob: &Knob) -> (&Path, &str, &Value, Scope) {
    match knob {
        Knob::Setting {
            file,
            pointer,
            value,
            scope,
            ..
        } => (file.as_path(), pointer.as_str(), value, *scope),
        other => panic!("not a setting knob: {other:?}"),
    }
}

/// Framing bytes of the AGENTS.md message (`core/src/context/user_instructions.rs:23-34`).
fn framing(cwd: Option<&Path>, global: bool, project_files: usize) -> u64 {
    let mut n =
        "# AGENTS.md instructions".len() + "\n\n<INSTRUCTIONS>\n".len() + "\n</INSTRUCTIONS>".len();
    if let Some(cwd) = cwd {
        n += " for ".len() + cwd.display().to_string().len();
        n += project_files.saturating_sub(1) * 2;
        if global {
            n += "\n\n--- project-doc ---\n\n".len();
        }
    }
    n as u64
}

const BLOCKS: &[(&str, &str)] = &[
    (
        "permissions_instructions",
        "/include_permissions_instructions",
    ),
    (
        "collaboration_mode",
        "/include_collaboration_mode_instructions",
    ),
    ("apps_instructions", "/include_apps_instructions"),
    ("multi_agent", "/agents/enabled"),
    ("environment_context", "/include_environment_context"),
];

#[test]
fn builtin_rows_are_present_with_their_knobs() {
    let f = fx();
    user_config(&f, None, "");
    let t = resolve(&target(&f), None);
    assert_eq!(t.agent, model::Agent::Codex);
    assert_eq!(t.rules_verified_against, "0.160.1");

    let base = row(&t, "codex/system-prompt/builtin/instructions");
    assert!(
        matches!(base.knobs.as_slice(), [Knob::None { reason }] if reason.contains("STRONGLY DISCOURAGED"))
    );
    assert_eq!(base.cost.predicted, None, "no model, no catalog: `?`");

    for (name, pointer) in BLOCKS {
        let item = row(&t, &format!("/builtin/{name}"));
        assert_eq!(item.origin.layer, Layer::Builtin);
        assert_eq!(item.mode, Mode::Always);
        assert_eq!(
            item.cost,
            Default::default(),
            "{name}: generated text is `?`"
        );
        // Trust unset: the project file would be ignored, so only the user file is a knob.
        match item.knobs.as_slice() {
            [knob] => {
                let (file, p, value, scope) = setting(knob);
                assert_eq!(file, f.codex.join("config.toml"));
                assert_eq!(p, *pointer);
                assert_eq!(value, &json!(false));
                assert_eq!(scope, Scope::User);
            }
            other => panic!("{name}: {other:?}"),
        }
    }
    assert_eq!(
        row(&t, "/environment_context").kind,
        Kind::EnvBlock,
        "environment is an env block"
    );
    // No model, no catalog entry: the catalog-gated blocks stay listed, marked not checkable.
    for name in ["collaboration_mode", "multi_agent"] {
        let s = row(&t, &format!("/builtin/{name}")).signal.as_deref();
        assert!(
            s.is_some_and(|s| s.starts_with(CATALOG_GATED) && s.contains("no model in config")),
            "{name}: {s:?}"
        );
    }
    for name in [
        "permissions_instructions",
        "apps_instructions",
        "environment_context",
    ] {
        assert_eq!(row(&t, &format!("/builtin/{name}")).signal, None, "{name}");
    }
    // Request order: instructions, developer blocks, multi-agent, user message.
    let order: Vec<&str> = t
        .items
        .iter()
        .map(|i| i.id.rsplit('/').next().unwrap())
        .collect();
    assert_eq!(
        order,
        [
            "instructions",
            "permissions_instructions",
            "collaboration_mode",
            "apps_instructions",
            "recommended_plugins",
            "multi_agent",
            "environment_context"
        ]
    );
    // `<recommended_plugins>` is on by default (features.plugins) and its size is `?`.
    assert_eq!(t.unknown_rows, 7);
    assert_eq!(t.predicted_known, 0);

    // Trusted: the target dir's `.codex/config.toml` is the narrower knob, listed first.
    user_config(&f, Some((&f.repo, "trusted")), "");
    let t = resolve(&target(&f), None);
    let env = row(&t, "/environment_context");
    let scopes: Vec<(PathBuf, Scope)> = env
        .knobs
        .iter()
        .map(|k| {
            let (file, _, _, scope) = setting(k);
            (file.to_path_buf(), scope)
        })
        .collect();
    assert_eq!(
        scopes,
        [
            (f.repo.join(".codex/config.toml"), Scope::Project),
            (f.codex.join("config.toml"), Scope::User)
        ]
    );
}

#[test]
fn switched_off_blocks_leave_the_tree() {
    let f = fx();
    user_config(
        &f,
        None,
        "include_permissions_instructions = false\ninclude_collaboration_mode_instructions = false\ninclude_apps_instructions = false\ninclude_environment_context = false\n[agents]\nenabled = false\n[features]\nplugins = false\n",
    );
    let t = resolve(&target(&f), None);
    for (name, _) in BLOCKS {
        assert!(find(&t, &format!("/builtin/{name}")).is_none(), "{name}");
    }
    assert_eq!(t.items.len(), 1, "only the base instructions remain");
}

#[test]
fn overlay_set_json_on_config_toml_removes_the_row() {
    let f = fx();
    user_config(&f, None, "model = \"m\"\n");
    let t = resolve(&target(&f), None);
    let env = row(&t, "/environment_context");
    let (file, pointer, value, _) = setting(&env.knobs[0]);
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: file.to_path_buf(),
            pointer: pointer.to_string(),
            value: value.clone(),
        }],
    };
    let after = resolve(&target(&f), Some(&overlay));
    assert!(find(&after, "/environment_context").is_none());
    assert_eq!(after.items.len() + 1, t.items.len(), "nothing else changes");
    // The patch is virtual: the file on disk is untouched.
    assert_eq!(
        std::fs::read_to_string(f.codex.join("config.toml")).unwrap(),
        "model = \"m\"\n"
    );

    // A patch on a project config that does not exist yet still counts once the dir is trusted.
    user_config(&f, Some((&f.repo, "trusted")), "");
    let t = resolve(&target(&f), None);
    let (file, pointer, value, scope) = setting(&row(&t, "/apps_instructions").knobs[0]);
    assert_eq!(scope, Scope::Project);
    assert!(!file.exists());
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: file.to_path_buf(),
            pointer: pointer.to_string(),
            value: value.clone(),
        }],
    };
    let after = resolve(&target(&f), Some(&overlay));
    assert!(find(&after, "/apps_instructions").is_none());
}

#[test]
fn profile_layers_on_top_of_the_user_config() {
    let f = fx();
    user_config(
        &f,
        None,
        "model = \"user-model\"\ninclude_apps_instructions = false\n",
    );
    write(
        &f.codex.join("work.config.toml"),
        "model = \"work-model\"\ninclude_apps_instructions = true\n",
    );

    let t = resolve(&target(&f), None);
    assert!(find(&t, "/apps_instructions").is_none());
    assert!(signal(&t, "model:").starts_with("model: user-model ("));

    let mut tp = target(&f);
    tp.env.insert(PROFILE_ENV.to_string(), "work".to_string());
    let t = resolve(&tp, None);
    assert!(find(&t, "/apps_instructions").is_some());
    let model = signal(&t, "model:");
    assert!(model.starts_with("model: work-model ("), "{model}");
    assert!(model.contains("work.config.toml#/model"), "{model}");
    assert!(signal(&t, "profile:").contains("work"));
    // The profile sets the key above the user file: a user-file knob would not take effect.
    assert!(matches!(
        row(&t, "/apps_instructions").knobs.as_slice(),
        [Knob::None { reason }] if reason.contains("work.config.toml")
    ));

    // `target.model` wins over the config.
    let mut tm = target(&f);
    tm.model = Some("pinned".to_string());
    assert!(signal(&resolve(&tm, None), "model:").starts_with("model: pinned (target)"));
}

#[test]
fn project_config_is_ignored_until_the_project_is_trusted() {
    let f = fx();
    write(
        &f.repo.join(".codex/config.toml"),
        "include_environment_context = false\ndeveloper_instructions = \"from the repo\"\n",
    );
    for trust in [None, Some("untrusted")] {
        user_config(&f, trust.map(|l| (f.repo.as_path(), l)), "");
        let t = resolve(&target(&f), None);
        assert!(find(&t, "/environment_context").is_some(), "{trust:?}");
        assert!(find(&t, "#/developer_instructions").is_none(), "{trust:?}");
        let s = signal(&t, "trust:");
        assert!(s.contains(trust.unwrap_or("unset")), "{s}");
    }
    let unset = resolve(&target(&f), None);
    let s = signal(&unset, "trust:");
    assert!(
        s.contains(&format!("projects.\"{}\"", f.repo.display())),
        "{s}"
    );

    user_config(&f, Some((&f.repo, "trusted")), "");
    let t = resolve(&target(&f), None);
    assert!(find(&t, "/environment_context").is_none());
    let dev = row(&t, "#/developer_instructions");
    assert_eq!(dev.origin.layer, Layer::Project);
    assert_eq!(dev.kind, Kind::Instruction);
    assert_eq!(dev.cost.predicted, Some("from the repo".len() as u64));
    let s = signal(&t, "trust: trusted");
    assert!(s.contains("config.toml"), "{s}");
    // Set in the target dir's own project file: the knob writes `""` there; the user file is
    // below it and would not take effect.
    match dev.knobs.as_slice() {
        [knob] => {
            let (file, pointer, value, scope) = setting(knob);
            assert_eq!(file, f.repo.join(".codex/config.toml"));
            assert_eq!(pointer, "/developer_instructions");
            assert_eq!(value, &json!(""));
            assert_eq!(scope, Scope::Project);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn trust_reaches_a_subdir_through_the_project_root() {
    let f = fx();
    let sub = f.repo.join("pkg");
    write(
        &sub.join(".codex/config.toml"),
        "include_collaboration_mode_instructions = false\n",
    );
    // Only the repo root is trusted; the subdir's layer is trusted through the project root.
    user_config(&f, Some((&f.repo, "trusted")), "");
    let t = resolve(&target_at(&f, &sub), None);
    assert!(find(&t, "/collaboration_mode").is_none());
}

#[test]
fn denylisted_project_keys_are_ignored() {
    let f = fx();
    write(
        &f.repo.join(".codex/config.toml"),
        "notify = [\"say\", \"done\"]\nprofile = \"p\"\nmodel_provider = \"elsewhere\"\ninclude_permissions_instructions = false\n",
    );
    user_config(&f, Some((&f.repo, "trusted")), "");
    let t = resolve(&target(&f), None);
    assert!(
        find(&t, "/permissions_instructions").is_none(),
        "allowed key applies"
    );
    let w = t
        .warnings
        .iter()
        .find(|w| w.contains("ignored in a project config"))
        .unwrap_or_else(|| panic!("{:?}", t.warnings));
    for key in ["notify", "profile", "model_provider"] {
        assert!(w.contains(key), "{w}");
    }
    assert!(
        !t.warnings.iter().any(|w| w.contains("legacy `profile")),
        "a denylisted `profile` never reaches the effective config: {:?}",
        t.warnings
    );

    // Not trusted: no warning (Codex does not read the layer at all).
    user_config(&f, None, "");
    let t = resolve(&target(&f), None);
    assert!(!t
        .warnings
        .iter()
        .any(|w| w.contains("ignored in a project config")));
}

#[test]
fn agents_chain_runs_root_to_cwd_with_override_and_fallback() {
    let f = fx();
    // Above the project root: never read.
    write(&f.root.join("AGENTS.md"), "outside\n");
    write(&f.repo.join("AGENTS.md"), "root doc\n");
    write(&f.repo.join("a/AGENTS.override.md"), "override\n");
    write(&f.repo.join("a/AGENTS.md"), "shadowed\n");
    write(&f.repo.join("a/b/CLAUDE.md"), "claude\n");
    let cwd = f.repo.join("a/b");
    user_config(&f, None, "");
    let t = resolve(&target_at(&f, &cwd), None);

    let rows = project_rows(&t);
    let paths: Vec<PathBuf> = rows
        .iter()
        .map(|i| i.origin.path.clone().unwrap())
        .collect();
    assert_eq!(
        paths,
        [
            f.repo.join("AGENTS.md"),
            f.repo.join("a/AGENTS.override.md")
        ]
    );
    assert_eq!(rows[0].cost.predicted, Some("root doc\n".len() as u64));
    assert!(rows[1]
        .origin
        .why
        .iter()
        .any(|w| w.contains("shadows AGENTS.md")));
    assert!(rows.iter().all(|r| r.mode == Mode::Always));

    let message = row(&t, "codex/instruction/builtin/agents-md-message");
    assert_eq!(message.cost.predicted, Some(framing(Some(&cwd), false, 2)));
    assert_eq!(message.children.len(), 2);
    assert!(matches!(message.knobs.as_slice(), [Knob::None { .. }]));
    // Files land as the project chain; their knob drops them all and says so.
    let knob = &rows[0].knobs[0];
    let (file, pointer, value, scope) = setting(knob);
    assert_eq!(file, f.codex.join("config.toml"));
    assert_eq!(pointer, "/project_doc_max_bytes");
    assert_eq!(value, &json!(0));
    assert_eq!(scope, Scope::User);
    assert!(
        matches!(knob, Knob::Setting { note: Some(n), .. } if n.contains("every project AGENTS file"))
    );

    // CLAUDE.md is read only when listed as a fallback, and only where no AGENTS file exists.
    user_config(
        &f,
        None,
        "project_doc_fallback_filenames = [\"CLAUDE.md\", \"../x\", \"\"]\n",
    );
    let t = resolve(&target_at(&f, &cwd), None);
    let rows = project_rows(&t);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[2].origin.path.as_deref(),
        Some(f.repo.join("a/b/CLAUDE.md").as_path())
    );
    assert!(rows[2]
        .origin
        .why
        .iter()
        .any(|w| w.contains("project_doc_fallback_filenames")));
    assert!(
        t.warnings.iter().any(|w| w.contains("\"../x\"")),
        "{:?}",
        t.warnings
    );
    assert_eq!(
        row(&t, "/agents-md-message").cost.predicted,
        Some(framing(Some(&cwd), false, 3))
    );
    let total: u64 = model::walk(&t)
        .filter(|i| i.kind == Kind::Instruction)
        .filter_map(|i| i.cost.predicted)
        .sum();
    assert_eq!(
        total,
        framing(Some(&cwd), false, 3)
            + ("root doc\n".len() + "override\n".len() + "claude\n".len()) as u64
    );
    assert_eq!(t.predicted_known, total, "nothing else has a known cost");
}

#[test]
fn empty_override_shadows_agents_md_and_lands_nothing() {
    let f = fx();
    write(&f.repo.join("AGENTS.override.md"), "  \n\n");
    write(&f.repo.join("AGENTS.md"), "never read\n");
    user_config(&f, None, "");
    let t = resolve(&target(&f), None);
    assert!(
        find(&t, "/agents-md-message").is_none(),
        "no file landed: no message"
    );
    let empty = row(&t, "/AGENTS.override.md");
    assert_eq!(empty.mode, Mode::None);
    assert!(empty.cost.not_applicable);
    assert_eq!(
        empty.signal.as_deref(),
        Some("empty, yet shadows AGENTS.md")
    );
    assert!(find(&t, "repo/AGENTS.md").is_none());
}

#[test]
fn shared_budget_truncates_the_last_file_and_drops_the_rest() {
    let f = fx();
    write(&f.repo.join("AGENTS.md"), &"a".repeat(30_000));
    write(&f.repo.join("sub/AGENTS.md"), &"b".repeat(5_000));
    write(&f.repo.join("sub/deep/AGENTS.md"), "late\n");
    user_config(&f, None, "");
    let cwd = f.repo.join("sub/deep");
    let t = resolve(&target_at(&f, &cwd), None);
    let rows = project_rows(&t);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].cost.predicted, Some(30_000));
    assert_eq!(rows[0].signal, None);
    assert_eq!(rows[1].cost.source, Some(5_000));
    assert_eq!(rows[1].cost.predicted, Some(2_768));
    assert_eq!(
        rows[1].signal.as_deref(),
        Some("truncated: 2768 of 5000 source bytes read, 2768 bytes land (project_doc_max_bytes = 32768, shared by the chain)")
    );
    assert_eq!(rows[2].mode, Mode::None);
    assert!(rows[2].cost.not_applicable);
    assert!(rows[2].signal.as_deref().unwrap().starts_with("dropped"));
    // Two landed: the join counts once.
    assert_eq!(
        row(&t, "/agents-md-message").cost.predicted,
        Some(framing(Some(&cwd), false, 2))
    );

    // A smaller budget from config cuts sooner; a multi-byte character cut in half decodes as
    // U+FFFD (3 bytes) while the budget counts the bytes read.
    write(&f.repo.join("AGENTS.md"), "ab\u{00e9}cd");
    user_config(&f, None, "project_doc_max_bytes = 3\n");
    let t = resolve(&target_at(&f, &cwd), None);
    let rows = project_rows(&t);
    assert_eq!(rows[0].cost.predicted, Some("ab\u{fffd}".len() as u64));
    assert_eq!(rows[1].mode, Mode::None, "budget used up after 3 bytes");

    // 0 = no project files at all.
    user_config(&f, None, "project_doc_max_bytes = 0\n");
    let t = resolve(&target_at(&f, &cwd), None);
    assert!(project_rows(&t).is_empty());
}

#[test]
fn global_agents_md_is_not_charged_to_the_budget() {
    let f = fx();
    let global = format!("\n  {}  \n\n", "g".repeat(40_000));
    write(&f.codex.join("AGENTS.md"), &global);
    write(&f.repo.join("AGENTS.md"), &"p".repeat(32_768));
    user_config(&f, None, "");
    let t = resolve(&target(&f), None);
    let g = row(
        &t,
        &format!(
            "codex/instruction/user/{}",
            f.codex.join("AGENTS.md").display()
        ),
    );
    assert_eq!(g.cost.source, Some(global.len() as u64));
    assert_eq!(g.cost.predicted, Some(40_000), "trimmed");
    assert!(matches!(
        g.knobs.as_slice(),
        [Knob::None { reason }] if reason == "no config key; empty or move the file"
    ));
    let p = &project_rows(&t)[0];
    assert_eq!(
        p.cost.predicted,
        Some(32_768),
        "the whole budget is still there"
    );
    assert_eq!(p.signal, None);
    assert_eq!(
        row(&t, "/agents-md-message").cost.predicted,
        Some(framing(Some(&f.repo), true, 1))
    );

    // AGENTS.override.md wins over AGENTS.md in CODEX_HOME.
    write(&f.codex.join("AGENTS.override.md"), "override\n");
    let t = resolve(&target(&f), None);
    let g = row(&t, "/.codex/AGENTS.override.md");
    assert_eq!(g.cost.predicted, Some("override".len() as u64));
    assert!(find(&t, "/.codex/AGENTS.md").is_none());
}

#[test]
fn untrusted_drops_project_agents_but_keeps_the_global_file() {
    let f = fx();
    write(&f.codex.join("AGENTS.md"), "global\n");
    write(&f.repo.join("AGENTS.md"), "project\n");
    user_config(&f, Some((&f.repo, "untrusted")), "");
    let t = resolve(&target(&f), None);
    assert!(project_rows(&t).is_empty());
    assert!(find(&t, "/.codex/AGENTS.md").is_some());
    // No project file: the header carries no ` for <cwd>`.
    assert_eq!(
        row(&t, "/agents-md-message").cost.predicted,
        Some(framing(None, true, 0))
    );
    let s = signal(&t, "trust: untrusted");
    assert!(s.contains("AGENTS.md"), "{s}");

    // Unset trust keeps the project chain (only an explicit `untrusted` drops it).
    user_config(&f, None, "");
    assert_eq!(project_rows(&resolve(&target(&f), None)).len(), 1);
}

#[test]
fn base_instructions_and_context_window_come_from_models_cache() {
    let f = fx();
    cache(
        &f,
        json!([
            {
                "slug": "test-model",
                "context_window": 1000,
                "max_context_window": 2000,
                "base_instructions": "legacy",
                "model_messages": {"instructions_template": "t".repeat(100)}
            },
            {"slug": "legacy-model", "base_instructions": "x".repeat(42)}
        ]),
    );
    user_config(&f, None, "model = \"test-model\"\n");
    let t = resolve(&ctarget(&f), None);
    let base = row(&t, "/builtin/instructions");
    assert_eq!(base.cost.predicted, Some(100));
    assert!(base.cost.predicted_is_estimate);
    assert_eq!(t.agent_version.as_deref(), Some("0.160.1"));
    let model = signal(&t, "model:");
    assert!(
        model.contains("context window 1000 tokens (models_cache.json)") && model.ends_with(USED),
        "{model}"
    );
    assert_eq!(t.predicted_known, 100);

    // `model_context_window` is clamped to the catalog's maximum.
    user_config(
        &f,
        None,
        "model = \"test-model\"\nmodel_context_window = 5000\n",
    );
    let s = resolve(&ctarget(&f), None).signals.join("\n");
    assert!(s.contains("context window 2000 tokens"), "{s}");

    // A catalog entry with only the legacy field.
    user_config(&f, None, "model = \"legacy-model\"\n");
    let t = resolve(&ctarget(&f), None);
    assert_eq!(row(&t, "/builtin/instructions").cost.predicted, Some(42));

    // The `instructions` key replaces the catalog text, byte-exact.
    user_config(
        &f,
        None,
        "model = \"test-model\"\ninstructions = \"custom\"\n",
    );
    let t = resolve(&ctarget(&f), None);
    let base = row(&t, "/builtin/instructions");
    assert_eq!(base.cost.predicted, Some(6));
    assert!(!base.cost.predicted_is_estimate);

    // The cache's writer is reported as a fact about the file, not as the installed codex.
    assert!(signal(&t, "model:").contains(" · models_cache.json written by codex 0.160.1 · "));
}

#[test]
fn a_snapshot_outside_the_contract_supplies_no_window_and_no_base_size() {
    let f = fx();
    // One visible skill: the listing's budget is 2 % of the window (skills.rs `window_budget`).
    write(
        &f.codex.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: Does alpha.\n---\nbody\n",
    );
    let entry = json!({
        "slug": "m",
        "context_window": 1000,
        "max_context_window": 1000,
        "base_instructions": "b".repeat(30)
    });
    // (cache top level, why it is not accepted, the installed codex); `None` = accepted.
    let accepted = json!({
        "fetched_at": FETCHED_AT, "client_version": "0.160.1", "identity": "test-login",
        "models": [entry.clone()]
    });
    let other_version = json!({
        "fetched_at": FETCHED_AT, "client_version": "0.159.0", "identity": "test-login",
        "models": [entry.clone()]
    });
    let no_fetched_at = json!({
        "client_version": "0.160.1", "identity": "test-login", "models": [entry.clone()]
    });
    for (file, why) in [
        (
            &other_version,
            Some("models_cache.json was written by codex 0.159.0, not 0.160.1"),
        ),
        (
            &no_fetched_at,
            Some("models_cache.json: fetched_at is missing, so codex cannot parse the file"),
        ),
        (&accepted, None),
    ] {
        raw_cache(&f, file.clone());

        // No config window: the entry's 1000 counts only from an accepted snapshot.
        user_config(&f, None, "model = \"m\"\n");
        let t = resolve(&ctarget(&f), None);
        let model = signal(&t, "model:");
        let base = row(&t, "/builtin/instructions");
        let listing = row(&t, "codex/skill-listing/builtin/skills");
        let listing_says = |needle: &str| listing.origin.why.iter().any(|w| w.contains(needle));
        match why {
            Some(why) => {
                assert!(
                    model.contains(" · context window unknown · ") && model.contains(why),
                    "{model}"
                );
                // Downstream: no window, so the listing's budget is unknown.
                assert!(
                    t.warnings.iter().any(|w| w == CAP_STATE_UNKNOWN),
                    "{why}: {:?}",
                    t.warnings
                );
                assert!(
                    listing_says("budget unknown (see the warnings"),
                    "{:?}",
                    listing.origin.why
                );
                assert_eq!(listing.signal, None, "{why}");
                assert_eq!(base.cost.predicted, None, "{why}");
                assert!(
                    base.origin.why.contains(
                        &"the file's snapshot says 30 bytes; not used as this row's size"
                            .to_string()
                    ),
                    "{:?}",
                    base.origin.why
                );
            }
            None => {
                assert!(
                    model.contains(" · context window 1000 tokens (models_cache.json) · "),
                    "{model}"
                );
                assert_eq!(base.cost.predicted, Some(30));
                // Downstream: 2 % of 1,000 = a 20-token budget.
                assert!(!t.warnings.iter().any(|w| w == CAP_STATE_UNKNOWN));
                assert!(
                    listing_says(
                        "budget 20 codex tokens: 2 % of the 1000-token context window (models_cache.json; render.rs:138-148)"
                    ),
                    "{:?}",
                    listing.origin.why
                );
                // The `r0/…` form fits: `- alpha: Does alpha. (file: r0/alpha/SKILL.md)` + `\n`
                // is ceil(47 / 4) = 12 codex tokens.
                assert_eq!(listing.signal, None);
                assert!(
                    listing_says("within budget: 12 of 20 codex tokens"),
                    "{:?}",
                    listing.origin.why
                );
            }
        }

        // A config window of 2000: clamped to the catalog's 1000 only from an accepted snapshot;
        // otherwise named, and unknown.
        user_config(&f, None, "model = \"m\"\nmodel_context_window = 2000\n");
        let t = resolve(&ctarget(&f), None);
        let model = signal(&t, "model:");
        let at = format!(
            "{}#/model_context_window",
            f.codex.join("config.toml").display()
        );
        match why {
            Some(why) => assert!(
                model.contains(&format!(
                    " · context window unknown (configured 2000 ({at}); the catalog's max_context_window may lower it, and models_cache.json is not used: {why}"
                )) && !model.contains("1000 tokens"),
                "{model}"
            ),
            None => assert!(
                model.contains(&format!(
                    " · context window 1000 tokens ({at} clamped to models_cache.json max_context_window 1000) · "
                )),
                "{model}"
            ),
        }
    }

    // A target window still wins whatever the snapshot.
    raw_cache(&f, other_version);
    let mut t = ctarget(&f);
    t.context_window = Some(4000);
    assert!(signal(&resolve(&t, None), "model:").contains("context window 4000 tokens (target)"));
}

/// A codex `version` on the target's `PATH` (Homebrew layout, `Caskroom/codex/<v>/bin/codex`): the
/// version codex checks `models_cache.json` against.
fn with_codex(f: &Fx, mut t: Target, version: &str) -> Target {
    let exe = f
        .root
        .join("Caskroom/codex")
        .join(version)
        .join("bin/codex");
    write(&exe, "binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    t.env.insert(
        "PATH".to_string(),
        exe.parent().unwrap().display().to_string(),
    );
    t
}

/// The target with codex 0.160.1 on its `PATH`.
fn ctarget(f: &Fx) -> Target {
    with_codex(f, target(f), "0.160.1")
}

/// `fetched_at` of every fixture cache: a fixed instant, so the `model:` signal is deterministic.
const FETCHED_AT: &str = "2026-10-14T00:00:00Z";

/// `models_cache.json` as codex 0.160.1 writes it: every `ModelsCacheEntry` field
/// (`models-manager/src/cache.rs:64-88`, written at `manager.rs:635-641`) — `fetched_at`, the
/// endpoint's `etag`, its own `client_version`, an `identity` — plus `models`.
fn cache(f: &Fx, models: Value) {
    raw_cache(
        f,
        json!({
            "fetched_at": FETCHED_AT,
            "etag": "W/\"catalog-1\"",
            "client_version": "0.160.1",
            "identity": "test-login",
            "models": with_base(models)
        }),
    );
}

/// Every entry codex loads has base instructions, the legacy field or the template
/// (`protocol/src/openai_models.rs:928-938`): `"Base."` where an entry has neither.
fn with_base(mut models: Value) -> Value {
    for entry in models.as_array_mut().unwrap() {
        let template = entry
            .pointer("/model_messages/instructions_template")
            .is_some_and(Value::is_string);
        if !template && entry.get("base_instructions").is_none() {
            entry["base_instructions"] = json!("Base.");
        }
    }
    models
}

fn raw_cache(f: &Fx, cache: Value) {
    write(
        &f.codex.join("models_cache.json"),
        &serde_json::to_string(&cache).unwrap(),
    );
}

/// The end of the `model:` signal when the catalog-gated rows follow a fixture cache.
const USED: &str = " · catalog facts from codex's catalog fetched at 2026-10-14T00:00:00Z (models_cache.json): codex re-checks a catalog older than 5 minutes with the server when it starts, identity is not compared with your login, and `harn verify --run --agent codex` confirms";

/// One entry per catalog shape (generic slugs).
fn catalog(f: &Fx) {
    cache(
        f,
        json!([
            {"slug": "plain-model", "model_messages": null},
            {
                "slug": "rich-model",
                "multi_agent_version": "v2",
                "model_messages": {
                    "collaboration_modes": {"default": "Default mode text.", "plan": null},
                    "multi_agent": {"role": {"root": "Root role."}}
                }
            },
            {"slug": "v2-bare-model", "multi_agent_version": "v2"},
            {"slug": "v1-model", "multi_agent_version": "v1"},
            {"slug": "null-version-model", "multi_agent_version": null},
            {"slug": "v3-model", "multi_agent_version": "v3"},
            {
                "slug": "messages-only-model",
                "model_messages": {
                    "collaboration_modes": {"default": null, "plan": "Plan mode text."},
                    "multi_agent": {"role": {"root": "Root role."}}
                }
            },
            {"slug": "no-modes-model", "model_messages": {"collaboration_modes": null}}
        ]),
    );
}

fn gated(item: &Item) -> bool {
    item.signal
        .as_deref()
        .is_some_and(|s| s.starts_with(CATALOG_GATED))
}

/// The cache states codex would not use as they are, and a field it cannot parse: each leaves
/// the catalog-gated rows unknown. `(cache, reason in the signal)`; `None` = no file.
fn unusable_caches() -> Vec<(Option<Value>, &'static str)> {
    let entry = with_base(json!([{"slug": "rich-model", "multi_agent_version": "v2"}]));
    vec![
        (
            None,
            "no models_cache.json yet (codex writes it on its first run)",
        ),
        (
            Some(
                json!({"fetched_at": FETCHED_AT, "client_version": "0.159.0", "identity": "test-login", "models": entry}),
            ),
            "written by codex 0.159.0, not 0.160.1",
        ),
        (
            Some(json!({"fetched_at": FETCHED_AT, "identity": "test-login", "models": entry})),
            "has no client_version",
        ),
        (
            Some(json!({"fetched_at": FETCHED_AT, "client_version": "0.160.1", "models": entry})),
            "has no identity",
        ),
    ]
}

#[test]
fn catalog_decides_the_collaboration_mode_block() {
    let f = fx();
    catalog(&f);

    // The entry carries the Default mode's text: rendered (collaboration_mode.rs:30-42).
    user_config(&f, None, "model = \"rich-model\"\n");
    let t = resolve(&ctarget(&f), None);
    let c = row(&t, "/builtin/collaboration_mode");
    assert_eq!((c.mode, c.signal.as_deref()), (Mode::Always, None));
    assert!(
        c.origin
            .why
            .iter()
            .any(|w| w.contains("collaboration_modes.default")),
        "{:?}",
        c.origin.why
    );
    // A definite row rests on the cache: the signal says what the snapshot is and what was not
    // checked.
    assert!(
        signal(&t, "model:").ends_with(USED),
        "{}",
        signal(&t, "model:")
    );

    // No Default text — missing or `null` at any level, or only the Plan mode's: `codex exec` and
    // `debug prompt-input` send no block; the row stays for the TUI, mode none.
    for model in [
        "plain-model",
        "messages-only-model",
        "no-modes-model",
        "v1-model",
    ] {
        user_config(&f, None, &format!("model = \"{model}\"\n"));
        let t = resolve(&ctarget(&f), None);
        let c = row(&t, "/builtin/collaboration_mode");
        assert_eq!(c.mode, Mode::None, "{model}");
        assert!(!gated(c), "{model}: the entry is known: {:?}", c.signal);
        assert!(
            c.signal.as_deref().is_some_and(|s| s.contains(model)),
            "{model}: {:?}",
            c.signal
        );
        assert!(
            c.origin.why.iter().any(|w| w.contains("TUI")),
            "{model}: {:?}",
            c.origin.why
        );
        assert_eq!(c.knobs.len(), 1, "the switch still decides it in the TUI");
    }

    // Entry unknown: listed, marked not checkable, naming why.
    for (config, why) in [
        ("model = \"ghost-model\"\n", "ghost-model is not in"),
        ("", "no model in config"),
        ("model = 5\n", "config.toml#/model is not a string"),
        (
            "model = \"rich-model\"\nmodel_catalog_json = \"/nowhere/catalog.json\"\n",
            "model_catalog_json",
        ),
    ] {
        user_config(&f, None, config);
        let t = resolve(&ctarget(&f), None);
        let c = row(&t, "/builtin/collaboration_mode");
        assert_eq!(c.mode, Mode::Always, "{config}");
        assert_eq!(c.cost, Default::default(), "{config}");
        let s = c.signal.as_deref().unwrap_or_default();
        assert!(gated(c) && s.contains(why), "{config}: {s}");
        assert!(s.contains("collaboration_modes.default"), "{s}");
    }

    // A cache codex would not use, or a field it cannot parse: unknown as well.
    user_config(&f, None, "model = \"rich-model\"\n");
    let mut caches = unusable_caches();
    caches.push((
        Some(
            json!({"fetched_at": FETCHED_AT, "client_version": "0.160.1", "identity": "test-login", "models": [
                {"slug": "rich-model", "base_instructions": "B.", "model_messages": {"collaboration_modes": {"default": 5}}}
            ]}),
        ),
        "models[0] (rich-model) model_messages.collaboration_modes.default is a number, so codex cannot parse the file",
    ));
    caches.push((
        Some(
            json!({"fetched_at": FETCHED_AT, "client_version": "0.160.1", "identity": "test-login", "models": [
                {"slug": "rich-model", "base_instructions": "B.", "model_messages": "text"}
            ]}),
        ),
        "models[0] (rich-model) model_messages is a string",
    ));
    for (file, why) in caches {
        match file {
            Some(c) => raw_cache(&f, c),
            None => std::fs::remove_file(f.codex.join("models_cache.json")).unwrap(),
        }
        let t = resolve(&ctarget(&f), None);
        let c = row(&t, "/builtin/collaboration_mode");
        let s = c.signal.as_deref().unwrap_or_default();
        assert!(
            c.mode == Mode::Always && gated(c) && s.contains(why),
            "{why}: {s}"
        );
    }

    // The switch still removes the row whatever the catalog says.
    catalog(&f);
    user_config(
        &f,
        None,
        "model = \"rich-model\"\ninclude_collaboration_mode_instructions = false\n",
    );
    assert!(find(&resolve(&ctarget(&f), None), "/collaboration_mode").is_none());
}

#[test]
fn catalog_decides_the_multi_agent_block() {
    let f = fx();
    catalog(&f);
    let present = |config: &str| -> Option<Item> {
        user_config(&f, None, config);
        find(&resolve(&ctarget(&f), None), "/builtin/multi_agent").cloned()
    };

    // `multi_agent_version = "v2"`: role + mode rendered (session/multi_agents.rs:14-18, :77-82),
    // with or without the catalog's own role text.
    for model in ["rich-model", "v2-bare-model"] {
        let m = present(&format!("model = \"{model}\"\n")).expect(model);
        assert_eq!(
            (m.mode, m.signal.as_deref()),
            (Mode::Always, None),
            "{model}"
        );
        assert_eq!(setting(&m.knobs[0]).1, "/agents/enabled");
    }
    // The plugin blocks stay before it; without it they close the developer blocks.
    user_config(&f, None, "model = \"rich-model\"\n");
    let t = resolve(&ctarget(&f), None);
    let order: Vec<&str> = t
        .items
        .iter()
        .map(|i| i.id.rsplit('/').next().unwrap())
        .collect();
    assert_eq!(
        &order[order.len() - 3..],
        ["recommended_plugins", "multi_agent", "environment_context"]
    );

    // Anything but v2 — "v1", missing, `null`, a value codex does not recognise, role text
    // without a version: no role or mode message, in any front end.
    for model in [
        "v1-model",
        "plain-model",
        "null-version-model",
        "v3-model",
        "messages-only-model",
    ] {
        user_config(&f, None, &format!("model = \"{model}\"\n"));
        let t = resolve(&ctarget(&f), None);
        assert!(find(&t, "/builtin/multi_agent").is_none(), "{model}");
        assert_eq!(
            t.items.last().map(|i| i.id.as_str()),
            Some("codex/env-block/builtin/environment_context"),
            "{model}"
        );
    }

    // agents.enabled = false wins over the catalog (config/mod.rs:1586-1593).
    assert!(present("model = \"rich-model\"\n[agents]\nenabled = false\n").is_none());

    // features.multi_agent_v2 = true wins over both, as a boolean or a table, even with the
    // catalog entry unknown.
    for (model, features, pointer) in [
        (
            "plain-model",
            "[features]\nmulti_agent_v2 = true\n",
            "/features/multi_agent_v2",
        ),
        (
            "plain-model",
            "[features.multi_agent_v2]\nenabled = true\n",
            "/features/multi_agent_v2/enabled",
        ),
        (
            "ghost-model",
            "[features]\nmulti_agent_v2 = true\n",
            "/features/multi_agent_v2",
        ),
    ] {
        let m = present(&format!(
            "model = \"{model}\"\n[agents]\nenabled = false\n{features}"
        ))
        .expect(features);
        assert_eq!(
            (m.mode, m.signal.as_deref()),
            (Mode::Always, None),
            "{features}"
        );
        match m.knobs.as_slice() {
            [knob] => {
                let (_, p, value, _) = setting(knob);
                assert_eq!((p, value), (pointer, &json!(false)), "{features}");
            }
            other => panic!("{features}: {other:?}"),
        }
    }

    // Set to false (a boolean or the table's `enabled`) it is no override: the catalog decides.
    for features in [
        "[features]\nmulti_agent_v2 = false\n",
        "[features.multi_agent_v2]\nenabled = false\n",
        "[features.multi_agent_v2]\nmax_concurrent_threads_per_session = 2\n",
    ] {
        let m = present(&format!("model = \"rich-model\"\n{features}")).expect(features);
        assert_eq!(setting(&m.knobs[0]).1, "/agents/enabled", "{features}");
        assert!(
            present(&format!("model = \"v1-model\"\n{features}")).is_none(),
            "{features}"
        );
    }

    // Entry unknown: listed, marked not checkable.
    let m = present("model = \"ghost-model\"\n").expect("ghost");
    assert_eq!(m.mode, Mode::Always);
    let s = m.signal.as_deref().unwrap_or_default();
    assert!(
        gated(&m) && s.contains("multi_agent_version") && s.contains("ghost-model is not in"),
        "{s}"
    );
    let mut caches = unusable_caches();
    caches.push((
        Some(
            json!({"fetched_at": FETCHED_AT, "client_version": "0.160.1", "identity": "test-login", "models": [
                {"slug": "rich-model", "base_instructions": "B.", "multi_agent_version": 2}
            ]}),
        ),
        "models[0] (rich-model) multi_agent_version is a number",
    ));
    for (file, why) in caches {
        match file {
            Some(c) => raw_cache(&f, c),
            None => std::fs::remove_file(f.codex.join("models_cache.json")).unwrap(),
        }
        let m = present("model = \"rich-model\"\n").expect(why);
        let s = m.signal.as_deref().unwrap_or_default();
        assert!(
            m.mode == Mode::Always && gated(&m) && s.contains(why),
            "{why}: {s}"
        );
    }
}

#[test]
fn multi_agent_knobs_remove_the_block_one_decider_at_a_time() {
    let f = fx();
    catalog(&f);
    user_config(
        &f,
        None,
        "model = \"rich-model\"\n[features]\nmulti_agent_v2 = true\n",
    );
    let t = resolve(&ctarget(&f), None);
    let first = row(&t, "/builtin/multi_agent").knobs[0].clone();
    assert_eq!(setting(&first).1, "/features/multi_agent_v2");

    // Turning the feature off leaves the block: the catalog's v2 still renders it.
    let op = |knob: &Knob| {
        let (file, pointer, value, _) = setting(knob);
        OverlayOp::SetJson {
            file: file.to_path_buf(),
            pointer: pointer.to_string(),
            value: value.clone(),
        }
    };
    let mut overlay = Overlay {
        ops: vec![op(&first)],
    };
    let after = resolve(&ctarget(&f), Some(&overlay));
    let m = row(&after, "/builtin/multi_agent");
    assert_eq!((m.mode, m.signal.as_deref()), (Mode::Always, None));
    let next = m.knobs[0].clone();
    assert_eq!(setting(&next).1, "/agents/enabled");

    // Its next knob removes it.
    overlay.ops.push(op(&next));
    assert!(find(
        &resolve(&ctarget(&f), Some(&overlay)),
        "/builtin/multi_agent"
    )
    .is_none());
}

#[test]
fn one_bad_field_anywhere_voids_the_cache_for_every_model() {
    let f = fx();
    user_config(&f, None, "model = \"rich-model\"\n");
    let good = json!({
        "slug": "rich-model",
        "multi_agent_version": "v2",
        "base_instructions": "B.",
        "model_messages": {"collaboration_modes": {"default": "D."}}
    });
    let file = |models: Value| {
        raw_cache(
            &f,
            json!({"fetched_at": FETCHED_AT, "client_version": "0.160.1", "identity": "test-login", "models": models}),
        )
    };
    file(json!([good.clone()]));
    let t = resolve(&ctarget(&f), None);
    for name in ["collaboration_mode", "multi_agent"] {
        assert_eq!(row(&t, &format!("/builtin/{name}")).signal, None, "{name}");
    }

    let other = |extra: Value| {
        let mut e = json!({"slug": "other", "base_instructions": "B."});
        e.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        json!([good.clone(), e])
    };
    let mut bad_field = good.clone();
    bad_field["supports_search_tool"] = json!("yes");
    for (models, why) in [
        // Another model's field: codex loads the whole file, so it fails for every model.
        (
            other(json!({"include_plugin_usage_instructions": null})),
            "models[1] (other) include_plugin_usage_instructions is null",
        ),
        (
            other(json!({"context_window": 1.5})),
            "models[1] (other) context_window is not an integer in i64 range",
        ),
        (
            other(json!({"model_messages": {"collaboration_modes": "modes"}})),
            "models[1] (other) model_messages.collaboration_modes is a string",
        ),
        (
            json!([good.clone(), {"slug": "other"}]),
            "models[1] (other) has neither base_instructions nor model_messages.instructions_template",
        ),
        (
            json!([good.clone(), {"base_instructions": "B."}]),
            "models[1] has no slug",
        ),
        // The same entry, a field no gated row reads.
        (
            json!([bad_field]),
            "models[0] (rich-model) supports_search_tool is a string",
        ),
    ] {
        file(models);
        let t = resolve(&ctarget(&f), None);
        for name in ["collaboration_mode", "multi_agent"] {
            let r = row(&t, &format!("/builtin/{name}"));
            let s = r.signal.as_deref().unwrap_or_default();
            assert!(
                r.mode == Mode::Always
                    && gated(r)
                    && s.contains(why)
                    && s.contains("cannot parse the file"),
                "{name}: {s}"
            );
        }
        // No catalog fact from a file outside the contract: the base instructions' size is
        // unknown; the file's figure is only quoted.
        let base = row(&t, "/builtin/instructions");
        assert_eq!(base.cost.predicted, None, "{why}");
        assert!(
            base.origin.why.iter().any(|w| w
                .starts_with("size unknown until a capture: models_cache.json: ")
                && w.contains(why))
                && base.origin.why.iter().any(|w| w
                    == "the file's snapshot says 2 bytes; not used as this row's size"),
            "{why}: {:?}",
            base.origin.why
        );
        let model = signal(&t, "model:");
        assert!(
            model.contains(" · models_cache.json not used: models_cache.json: ")
                && !model.contains("identity not compared"),
            "{model}"
        );
    }

    // Required top-level fields: codex's serde rejects the file.
    for (top, why) in [
        (
            json!({"client_version": "0.160.1", "identity": "test-login", "models": [good.clone()]}),
            "models_cache.json: fetched_at is missing, so codex cannot parse the file",
        ),
        (
            json!({"fetched_at": 5, "client_version": "0.160.1", "identity": "test-login", "models": [good.clone()]}),
            "models_cache.json: fetched_at is a number, so codex cannot parse the file",
        ),
        (
            json!({"fetched_at": FETCHED_AT, "client_version": "0.160.1", "identity": "test-login", "models": {}}),
            "models_cache.json: models is an object, so codex cannot parse the file",
        ),
    ] {
        raw_cache(&f, top);
        let t = resolve(&ctarget(&f), None);
        let s = row(&t, "/builtin/multi_agent")
            .signal
            .clone()
            .unwrap_or_default();
        assert!(s.contains(why), "{why}: {s}");
    }

    // An array where an object is expected: serde derive may read a struct from a sequence, so
    // harn does not claim codex rejects it — the facts are unknown, and the base instructions say
    // codex *may* refetch.
    for (models, why) in [
        (
            other(json!({"model_messages": {"collaboration_modes": []}})),
            "models[1] (other) model_messages.collaboration_modes is an array, a shape harn does not read; catalog facts unknown",
        ),
        (
            other(json!({"model_messages": []})),
            "models[1] (other) model_messages is an array, a shape harn does not read; catalog facts unknown",
        ),
        (
            json!([good.clone(), []]),
            "models[1] is an array, a shape harn does not read; catalog facts unknown",
        ),
    ] {
        file(models);
        let t = resolve(&ctarget(&f), None);
        for name in ["collaboration_mode", "multi_agent"] {
            let r = row(&t, &format!("/builtin/{name}"));
            let s = r.signal.as_deref().unwrap_or_default();
            assert!(
                gated(r) && s.contains(why) && !s.contains("cannot parse"),
                "{name}: {s}"
            );
        }
        let base = row(&t, "/builtin/instructions");
        assert!(
            base.cost.predicted.is_none()
                && base.origin.why.iter().any(|w| w
                    .starts_with("size unknown until a capture: models_cache.json: ")
                    && w.contains(why)),
            "{why}: {:?}",
            base.origin.why
        );
    }
}

/// The facts the screens word themselves: the Codex home the tree was resolved with (also
/// `CODEX_HOME`), the model and its file, and the catalog snapshot's state with why it is not
/// used — the same facts the `model:` signal spells for the CLI.
#[test]
fn codex_facts_name_the_home_the_model_and_the_catalog_state() {
    use model::{CatalogState, CatalogWhy};
    let f = fx();
    catalog(&f);
    user_config(&f, None, "model = \"rich-model\"\n");
    let facts = |t: &Target| {
        resolve(t, None)
            .codex
            .expect("a Codex tree carries its facts")
    };

    let used = facts(&ctarget(&f));
    assert_eq!(used.home, f.codex);
    assert_eq!(used.model.slug.as_deref(), Some("rich-model"));
    assert_eq!(used.model.set_in, Some(f.codex.join("config.toml")));
    assert_eq!(used.catalog.written_by.as_deref(), Some("0.160.1"));
    assert!(used.catalog.lists_model);
    assert_eq!(
        used.catalog.state,
        CatalogState::Used {
            fetched_at: FETCHED_AT.to_string()
        }
    );

    let refused = |t: &Target| match facts(t).catalog.state {
        CatalogState::NotUsed { why } => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused(&target(&f)), CatalogWhy::CodexNotFound);
    assert_eq!(
        refused(&with_codex(&f, target(&f), "0.159.0")),
        CatalogWhy::OtherVersion {
            written_by: "0.160.1".to_string(),
            installed: "0.159.0".to_string()
        }
    );
    assert!(!CatalogWhy::CodexNotFound.refetched());

    // `CODEX_HOME` elsewhere is the home the facts name.
    let elsewhere = f.root.join("elsewhere");
    write(&elsewhere.join("config.toml"), "model = \"plain-model\"\n");
    let mut t = ctarget(&f);
    t.env
        .insert("CODEX_HOME".to_string(), elsewhere.display().to_string());
    let moved = facts(&t);
    assert_eq!(moved.home, elsewhere);
    assert_eq!(moved.model.set_in, Some(elsewhere.join("config.toml")));
    assert_eq!(moved.catalog.state, CatalogState::Missing);
}

/// Tool search is undecided exactly when harn cannot use the model's catalog entry — an
/// accepted snapshot that lists a set model always decides it (a missing
/// `supports_search_tool` is false) — so the TUI's tool-search note can point to the model note
/// for why. Checked across the states the facts name.
#[test]
fn tool_search_is_undecided_exactly_when_the_catalog_entry_cannot_be_used() {
    use model::CatalogState;
    let mcp = "[mcp_servers.github]\ncommand = \"gh-mcp\"\n";
    let entry_usable = |t: &Tree| {
        let c = t.codex.as_ref().expect("facts");
        matches!(c.catalog.state, CatalogState::Used { .. })
            && c.catalog.lists_model
            && c.model.slug.is_some()
    };
    // Accepted and listed: decided, whatever the entry says about search.
    for (said, decided) in [(Some(true), true), (Some(false), false), (None, false)] {
        let f = fx();
        let mut m = json!({"slug": "m"});
        if let Some(v) = said {
            m["supports_search_tool"] = json!(v);
        }
        cache(&f, json!([m]));
        user_config(&f, None, &format!("model = \"m\"\n{mcp}"));
        let t = resolve(&ctarget(&f), None);
        assert!(entry_usable(&t), "{said:?}");
        assert_eq!(t.tool_search, Some(decided), "{said:?}");
    }
    // Every way the entry cannot be used: undecided.
    let f = fx();
    cache(&f, json!([{"slug": "m", "supports_search_tool": true}]));
    let cases: Vec<(&str, String, Target)> = vec![
        ("no model", mcp.to_string(), ctarget(&f)),
        (
            "model not listed",
            format!("model = \"other\"\n{mcp}"),
            ctarget(&f),
        ),
        ("model not text", format!("model = 3\n{mcp}"), ctarget(&f)),
        (
            "no codex on PATH",
            format!("model = \"m\"\n{mcp}"),
            target(&f),
        ),
        (
            "another codex",
            format!("model = \"m\"\n{mcp}"),
            with_codex(&f, target(&f), "0.159.0"),
        ),
        (
            "catalog elsewhere",
            format!("model = \"m\"\nmodel_catalog_json = \"/srv/catalog.json\"\n{mcp}"),
            ctarget(&f),
        ),
    ];
    for (case, config, t) in cases {
        user_config(&f, None, &config);
        let tree = resolve(&t, None);
        assert!(!entry_usable(&tree), "{case}");
        assert_eq!(tree.tool_search, None, "{case}");
    }
    // No snapshot at all.
    std::fs::remove_file(f.codex.join("models_cache.json")).unwrap();
    user_config(&f, None, &format!("model = \"m\"\n{mcp}"));
    let tree = resolve(&ctarget(&f), None);
    assert!(!entry_usable(&tree));
    assert_eq!(tree.tool_search, None);
}

#[test]
fn without_codex_on_path_the_cache_is_not_trusted() {
    const NOT_FOUND: &str =
        "codex not found on PATH: cannot tell whether it accepts models_cache.json";
    let f = fx();
    catalog(&f);
    user_config(&f, None, "model = \"rich-model\"\n");
    let check_as = |t: &Tree, reason: &str| {
        for name in ["collaboration_mode", "multi_agent"] {
            let r = row(t, &format!("/builtin/{name}"));
            let s = r.signal.as_deref().unwrap_or_default();
            assert!(gated(r) && s.ends_with(reason), "{name}: {s}");
        }
        let model = signal(t, "model:");
        assert!(
            model.ends_with(&format!(" · models_cache.json not used: {reason}")),
            "{model}"
        );
    };
    let check = |t: &Tree| check_as(t, NOT_FOUND);
    check(&resolve(&target(&f), None));

    // A codex on PATH whose version no file names: found, version unknown — not "not found".
    let exe = f.root.join("plain/bin/codex");
    write(&exe, "binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut unversioned = target(&f);
    unversioned.env.insert(
        "PATH".to_string(),
        exe.parent().unwrap().display().to_string(),
    );
    check_as(
        &resolve(&unversioned, None),
        "the installed codex's version could not be determined: cannot tell whether it accepts models_cache.json",
    );
    // Another version's cache: still no version claim without an installed codex.
    raw_cache(
        &f,
        json!({"fetched_at": FETCHED_AT, "client_version": "0.159.0", "identity": "test-login", "models": with_base(json!([{"slug": "rich-model"}]))}),
    );
    let t = resolve(&target(&f), None);
    check(&t);
    let all: Vec<&str> = model::walk(&t)
        .filter_map(|i| i.signal.as_deref())
        .chain(t.signals.iter().map(String::as_str))
        .collect();
    assert!(
        all.iter().all(|s| !s.contains(", not 0.")),
        "no version-mismatch claim: {all:?}"
    );

    // With a newer codex installed, the 0.160.1 cache is another version's: refetched.
    catalog(&f);
    let t = resolve(&with_codex(&f, target(&f), "0.161.0"), None);
    let s = row(&t, "/builtin/multi_agent")
        .signal
        .clone()
        .unwrap_or_default();
    assert!(
        s.contains("models_cache.json was written by codex 0.160.1, not 0.161.0"),
        "{s}"
    );
}

#[test]
fn developer_instructions_from_the_user_config() {
    let f = fx();
    user_config(&f, None, "developer_instructions = \"Be terse.\"\n");
    let t = resolve(&target(&f), None);
    let dev = row(&t, "#/developer_instructions");
    assert_eq!(dev.origin.layer, Layer::User);
    assert_eq!(dev.cost.predicted, Some(9));
    assert_eq!(
        dev.origin.setting.as_deref(),
        Some(
            format!(
                "{}#/developer_instructions",
                f.codex.join("config.toml").display()
            )
            .as_str()
        )
    );
    let (file, _, value, scope) = setting(&dev.knobs[0]);
    assert_eq!(file, f.codex.join("config.toml"));
    assert_eq!(value, &json!(""));
    assert_eq!(scope, Scope::User);
    // The knob's own value turns the row off.
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: file.to_path_buf(),
            pointer: "/developer_instructions".to_string(),
            value: json!(""),
        }],
    };
    assert!(find(
        &resolve(&target(&f), Some(&overlay)),
        "#/developer_instructions"
    )
    .is_none());
}

#[test]
fn codex_home_is_never_a_project_layer() {
    let f = fx();
    // The target is $HOME itself: `~/.codex` is the user layer, not the target's project layer.
    user_config(&f, Some((&f.home, "trusted")), "");
    let t = resolve(&target_at(&f, &f.home.clone()), None);
    let env = row(&t, "/environment_context");
    assert_eq!(env.knobs.len(), 1);
    assert_eq!(setting(&env.knobs[0]).3, Scope::User);
}

#[test]
fn codex_home_env_overrides_the_default() {
    let f = fx();
    let other = f.root.join("alt-codex");
    write(
        &other.join("config.toml"),
        "include_environment_context = false\n",
    );
    user_config(&f, None, "");
    let mut t = target(&f);
    t.env
        .insert("CODEX_HOME".to_string(), other.display().to_string());
    let tree = resolve(&t, None);
    assert!(find(&tree, "/environment_context").is_none());
    // An empty CODEX_HOME is ignored.
    t.env.insert("CODEX_HOME".to_string(), String::new());
    assert!(find(&resolve(&t, None), "/environment_context").is_some());
}

#[test]
fn no_codex_home_is_an_empty_tree_with_a_warning() {
    let f = fx();
    std::fs::remove_dir_all(&f.codex).unwrap();
    write(&f.repo.join("AGENTS.md"), "project\n");
    let t = resolve(&target(&f), None);
    assert!(t.items.is_empty());
    assert_eq!(t.predicted_known, 0);
    assert_eq!(t.unknown_rows, 0);
    assert!(
        t.warnings
            .iter()
            .any(|w| w.starts_with("no codex home at") && w.contains("first run")),
        "{:?}",
        t.warnings
    );

    let mut te = target(&f);
    te.env.insert(
        "CODEX_HOME".to_string(),
        f.root.join("missing").display().to_string(),
    );
    let t = resolve(&te, None);
    assert!(t.items.is_empty());
    assert!(
        t.warnings[0].contains("refuses to start"),
        "{:?}",
        t.warnings
    );
    // The facts still name the home; nothing else could be read, so the catalog is missing and
    // the model is the target's own, when it names one.
    let facts = t.codex.as_ref().expect("a Codex tree carries its facts");
    assert_eq!(facts.home, f.root.join("missing"));
    assert_eq!(facts.catalog.state, model::CatalogState::Missing);
    assert_eq!(facts.catalog.written_by, None);
    assert!(!facts.catalog.lists_model);
    assert_eq!(facts.model.slug, None);
    assert_eq!(facts.model.set_in, None);
    te.model = Some("gpt-x".to_string());
    te.context_window = Some(4000);
    let facts = resolve(&te, None).codex.expect("facts");
    assert_eq!(facts.model.slug.as_deref(), Some("gpt-x"));
    assert_eq!(facts.model.context_window, Some(4000));
    assert_eq!(
        resolve(&target(&f), None).codex.map(|c| c.home),
        Some(f.codex.clone())
    );
}

// Review round 1 (C1): each test below fails on 393df03 except where it says it pins upstream.

/// Pins upstream, no defect: Codex's AGENTS.md gate reads `projects` from every enabled layer,
/// trusted project layers included (`config/src/state.rs:475-484`, `:548-551`;
/// `core/src/config/mod.rs:1913-1916`, `:3460-3465`; `projects` is not on the denylist,
/// `config/src/loader/mod.rs:88-101`, `:1137-1230`), while the config gate reads only non-project
/// layers (`loader:336-356`). A trusted repo can re-trust a subdir for AGENTS.md, not for config.
#[test]
fn review_trusted_project_layer_retrusts_a_subdir_for_agents_md_only() {
    let f = fx();
    let child = f.repo.join("child");
    write(&child.join("AGENTS.md"), "child doc\n");
    write(
        &child.join(".codex/config.toml"),
        "include_environment_context = false\n",
    );
    write(
        &f.repo.join(".codex/config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            child.display()
        ),
    );
    write(
        &f.codex.join("config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            f.repo.display(),
            child.display()
        ),
    );
    let t = resolve(&target_at(&f, &child), None);
    assert!(
        find(&t, "/environment_context").is_some(),
        "the child's own config layer stays disabled"
    );
    let rows = project_rows(&t);
    assert_eq!(
        rows.last().and_then(|r| r.origin.path.clone()),
        Some(child.join("AGENTS.md")),
        "the AGENTS.md chain loads"
    );
    let s = signal(&t, "trust: untrusted");
    assert!(
        s.contains(&format!(
            "AGENTS.md gate: trusted (projects.\"{}\".trust_level in {})",
            child.display(),
            f.repo.join(".codex/config.toml").display()
        )),
        "{s}"
    );
}

/// The trust signal says what each gate decides: project config, hooks and rules from the
/// config gate, the project AGENTS.md files from the AGENTS.md gate, with the second gate's own
/// level when the two differ. Built the four ways the gates can stand, from files only: a project
/// layer is enabled by its own dir's decision, so an enabled ancestor layer can set the target's
/// AGENTS.md entry while the target's own config gate stays unset.
#[test]
fn the_trust_signal_reads_each_gate_for_what_it_decides() {
    let claims = |config: &str, agents: &str| {
        format!(
            "project .codex/config.toml, hooks and rules {config}; project AGENTS.md files {agents}; skills load either way"
        )
    };
    // Same level: one gate, no second entry.
    let f = fx();
    user_config(&f, Some((&f.repo, "trusted")), "");
    let t = resolve(&target(&f), None);
    let s = signal(&t, "trust: trusted");
    assert!(s.ends_with(&claims("load", "load")), "{s}");
    assert!(!s.contains("AGENTS.md gate"), "{s}");

    // The config gate untrusted, the AGENTS.md gate re-trusted by the repo's own layer.
    let f = fx();
    let child = f.repo.join("child");
    write(&child.join("AGENTS.md"), "child doc\n");
    write(
        &f.repo.join(".codex/config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            child.display()
        ),
    );
    write(
        &f.codex.join("config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            f.repo.display(),
            child.display()
        ),
    );
    let t = resolve(&target_at(&f, &child), None);
    let s = signal(&t, "trust: untrusted");
    assert!(s.contains(&claims("are skipped", "load")), "{s}");
    assert!(s.contains("· AGENTS.md gate: trusted ("), "{s}");
    assert!(!project_rows(&t).is_empty(), "the AGENTS.md files load");

    // The config gate trusted (the repo's entry), the AGENTS.md gate untrusted by the repo's
    // layer: project config loads, the AGENTS.md files do not.
    let f = fx();
    let child = f.repo.join("child");
    write(&child.join("AGENTS.md"), "child doc\n");
    write(
        &child.join(".codex/config.toml"),
        "include_environment_context = false\n",
    );
    write(
        &f.repo.join(".codex/config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            child.display()
        ),
    );
    user_config(&f, Some((&f.repo, "trusted")), "");
    let t = resolve(&target_at(&f, &child), None);
    let s = signal(&t, "trust: trusted");
    assert!(s.contains(&claims("load", "are skipped")), "{s}");
    assert!(s.contains("· AGENTS.md gate: untrusted ("), "{s}");
    assert!(
        project_rows(&t).is_empty(),
        "the AGENTS.md files are skipped"
    );
    assert!(
        find(&t, "/environment_context").is_none(),
        "the child's config layer loads"
    );

    // The config gate unset (no entry for the target, its project root or its repo), the
    // AGENTS.md gate untrusted by an enabled layer between them.
    let f = fx();
    let mid = f.repo.join("mid");
    let leaf = mid.join("leaf");
    write(&leaf.join("AGENTS.md"), "leaf doc\n");
    write(
        &mid.join(".codex/config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            leaf.display()
        ),
    );
    user_config(&f, Some((&mid, "trusted")), "");
    let t = resolve(&target_at(&f, &leaf), None);
    let s = signal(&t, "trust: unset");
    assert!(s.contains(&claims("are ignored", "are skipped")), "{s}");
    assert!(s.contains("· AGENTS.md gate: untrusted ("), "{s}");
    assert!(
        project_rows(&t).is_empty(),
        "the AGENTS.md files are skipped"
    );
}

/// The fifth way the gates stand: no decision for the folder, its AGENTS.md files trusted by an
/// enabled layer above it. The folder's own config is ignored, its AGENTS.md files load, and the
/// tree says where the trusted gate was set — in its facts and in its signal.
#[test]
fn an_unset_folder_with_a_trusted_agents_md_gate_ignores_its_config_and_loads_its_agents_md() {
    let f = fx();
    let mid = f.repo.join("mid");
    let leaf = mid.join("leaf");
    write(&leaf.join("AGENTS.md"), "leaf doc\n");
    write(
        &leaf.join(".codex/config.toml"),
        "include_environment_context = false\n",
    );
    let mid_file = mid.join(".codex/config.toml");
    write(
        &mid_file,
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            leaf.display()
        ),
    );
    user_config(&f, Some((&mid, "trusted")), "");
    let t = resolve(&target_at(&f, &leaf), None);

    let trust = t
        .trust
        .as_ref()
        .expect("a Codex tree carries its trust facts");
    assert_eq!(trust.folder.level, None);
    assert_eq!(trust.folder.set_in, None);
    assert_eq!(trust.agents_md.level, Some(model::TrustLevel::Trusted));
    assert_eq!(trust.agents_md.key, leaf.display().to_string());
    assert_eq!(trust.agents_md.set_in.as_deref(), Some(mid_file.as_path()));
    assert!(trust.agents_md_apart());

    let s = signal(&t, "trust: unset");
    assert!(
        s.contains(
            "project .codex/config.toml, hooks and rules are ignored; project AGENTS.md files load"
        ),
        "{s}"
    );
    assert!(
        s.ends_with(&format!(
            " · AGENTS.md gate: trusted (projects.\"{}\".trust_level in {})",
            leaf.display(),
            mid_file.display()
        )),
        "{s}"
    );
    assert!(
        find(&t, "/environment_context").is_some(),
        "the folder's own config is ignored"
    );
    assert!(
        project_rows(&t)
            .iter()
            .any(|i| i.origin.path.as_deref() == Some(leaf.join("AGENTS.md").as_path())),
        "the folder's AGENTS.md loads: {:?}",
        project_rows(&t).iter().map(|i| &i.id).collect::<Vec<_>>()
    );
}

/// Each gate's file is carried as a path, not cut out of `<file>#<pointer>`: a `#` in the Codex
/// home and in the repo dir stays in the file (and in the signal).
#[test]
fn trust_gate_files_keep_a_hash_in_their_path() {
    let f = fx();
    let codex_home = f.root.join("co#dex");
    let repo = f.root.join("re#po");
    let child = repo.join("child");
    write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&child.join("AGENTS.md"), "child doc\n");
    let entry = |dir: &Path, level: &str| {
        format!(
            "[projects.\"{}\"]\ntrust_level = \"{level}\"\n",
            dir.display()
        )
    };
    let repo_file = repo.join(".codex/config.toml");
    let user_file = codex_home.join("config.toml");
    write(&repo_file, &entry(&child, "trusted"));
    write(
        &user_file,
        &format!("{}{}", entry(&repo, "trusted"), entry(&child, "untrusted")),
    );
    let mut target = target_at(&f, &child);
    target
        .env
        .insert("CODEX_HOME".to_string(), codex_home.display().to_string());
    let t = resolve(&target, None);

    let trust = t.trust.as_ref().expect("trust facts");
    assert_eq!(trust.folder.level, Some(model::TrustLevel::Untrusted));
    assert_eq!(trust.folder.set_in.as_deref(), Some(user_file.as_path()));
    assert_eq!(trust.agents_md.level, Some(model::TrustLevel::Trusted));
    assert_eq!(trust.agents_md.set_in.as_deref(), Some(repo_file.as_path()));
    let s = signal(&t, "trust: untrusted");
    assert!(
        s.starts_with(&format!(
            "trust: untrusted (projects.\"{}\".trust_level in {}): ",
            child.display(),
            user_file.display()
        )),
        "{s}"
    );
    assert!(
        s.ends_with(&format!(
            " · AGENTS.md gate: trusted (projects.\"{}\".trust_level in {})",
            child.display(),
            repo_file.display()
        )),
        "{s}"
    );
}

#[test]
fn review_invalid_utf8_config_is_rejected_not_rewritten() {
    let f = fx();
    std::fs::write(f.codex.join("config.toml"), b"model = \"a\xffb\"\n").unwrap();
    let t = resolve(&target(&f), None);
    let model = signal(&t, "model:");
    assert!(model.starts_with("model: not set in config"), "{model}");
    assert!(
        t.warnings
            .iter()
            .any(|w| w.contains("not UTF-8") && w.contains("refuses to start")),
        "{:?}",
        t.warnings
    );
}

#[test]
fn review_non_finite_float_is_set_not_absent() {
    let f = fx();
    write(&f.repo.join("AGENTS.md"), &"x".repeat(50));
    write(
        &f.repo.join(".codex/config.toml"),
        "project_doc_max_bytes = nan\ninclude_environment_context = inf\n",
    );
    user_config(
        &f,
        Some((&f.repo, "trusted")),
        "project_doc_max_bytes = 10\n",
    );
    let t = resolve(&target(&f), None);
    // Not the user layer's 10: the project layer set the key (to something codex rejects).
    assert!(project_rows(&t).is_empty(), "{:#?}", project_rows(&t));
    for key in ["project_doc_max_bytes", "include_environment_context"] {
        assert!(
            t.warnings
                .iter()
                .any(|w| w.contains(&format!("config.toml#/{key}")) && w.contains("non-finite")),
            "{key}: {:?}",
            t.warnings
        );
    }
    let env = row(&t, "/environment_context");
    assert!(
        env.origin
            .why
            .iter()
            .any(|w| w.contains("non-finite float harn cannot read")),
        "{:?}",
        env.origin.why
    );
}

/// No fix needed: `Target.dir` is canonical (`model.rs`, `main.rs` `target_for`), so a symlinked
/// `--dir` into another checkout resolves by its real path. Exercised through the binary.
#[test]
fn review_symlinked_target_resolves_by_real_path() {
    let f = fx();
    let real_sub = f.repo.join("sub");
    write(&f.repo.join("AGENTS.md"), "real doc\n");
    std::fs::create_dir_all(&real_sub).unwrap();
    let other = f.root.join("other");
    write(&other.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&other.join("AGENTS.md"), "other doc\n");
    let link = other.join("link");
    std::os::unix::fs::symlink(&real_sub, &link).unwrap();
    write(
        &f.codex.join("config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            f.repo.display(),
            other.display()
        ),
    );
    let out = Command::new(env!("CARGO_BIN_EXE_harn"))
        .env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env_remove("CODEX_HOME")
        .env_remove(PROFILE_ENV)
        .args(["--agent", "codex", "--json", "--dir"])
        .arg(&link)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let tree = &doc["agents"][0];
    assert_eq!(tree["target_dir"], json!(real_sub.display().to_string()));
    let signals: Vec<&str> = tree["signals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let trust = signals.iter().find(|s| s.starts_with("trust:")).unwrap();
    assert!(
        trust.starts_with(&format!(
            "trust: trusted (projects.\"{}\"",
            f.repo.display()
        )),
        "{trust}"
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(&f.repo.join("AGENTS.md").display().to_string()));
    assert!(!text.contains(&other.join("AGENTS.md").display().to_string()));
}

#[test]
fn review_model_catalog_json_leaves_catalog_facts_unknown() {
    let f = fx();
    let cache = json!({
        "fetched_at": FETCHED_AT,
        "client_version": "0.160.1",
        "models": [{
            "slug": "test-model",
            "context_window": 1000,
            "max_context_window": 2000,
            "model_messages": {"instructions_template": "t".repeat(100)}
        }]
    });
    write(
        &f.codex.join("models_cache.json"),
        &serde_json::to_string(&cache).unwrap(),
    );
    for extra in ["", "model_context_window = 5000\n"] {
        user_config(
            &f,
            None,
            &format!(
                "model = \"test-model\"\nmodel_catalog_json = \"{}\"\n{extra}",
                f.root.join("catalog.json").display()
            ),
        );
        let t = resolve(&target(&f), None);
        let model = signal(&t, "model:");
        assert!(
            model.contains("context window unknown") && model.contains("model_catalog_json"),
            "{extra:?}: {model}"
        );
        assert_eq!(row(&t, "/builtin/instructions").cost.predicted, None);
    }
}

#[test]
fn review_permissions_off_leaves_no_first_request_block() {
    let f = fx();
    user_config(&f, None, "");
    let t = resolve(&target(&f), None);
    let row = row(&t, "/permissions_instructions");
    assert!(
        row.origin
            .why
            .iter()
            .any(|w| w.contains("renders nothing in the first request")),
        "{:?}",
        row.origin.why
    );
    user_config(&f, None, "include_permissions_instructions = false\n");
    let t = resolve(&target(&f), None);
    assert!(!model::walk(&t).any(|i| i.id.contains("permission") || i.id.contains("approved")));
}

#[cfg(unix)]
#[test]
fn review_path_skips_a_non_executable_codex() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx();
    user_config(&f, None, "");
    let make = |version: &str, mode: u32, dir: &str| {
        let bin = f.root.join(format!("Caskroom/codex/{version}/bin/codex"));
        write(&bin, "binary");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(mode)).unwrap();
        let path_dir = f.root.join(dir);
        std::fs::create_dir_all(&path_dir).unwrap();
        std::os::unix::fs::symlink(&bin, path_dir.join("codex")).unwrap();
        path_dir
    };
    let first = make("9.9.9", 0o644, "path-a");
    let second = make("0.161.0", 0o755, "path-b");
    let mut t = target(&f);
    t.env.insert(
        "PATH".to_string(),
        format!("{}:{}", first.display(), second.display()),
    );
    assert_eq!(resolve(&t, None).agent_version.as_deref(), Some("0.161.0"));
}

#[test]
fn review_empty_global_override_falls_through_to_agents_md() {
    let f = fx();
    user_config(&f, None, "");
    write(&f.codex.join("AGENTS.override.md"), "  \n\n");
    write(&f.codex.join("AGENTS.md"), "global\n");
    let t = resolve(&target(&f), None);
    let message = row(&t, "/agents-md-message");
    let children: Vec<(PathBuf, Mode)> = message
        .children
        .iter()
        .map(|c| (c.origin.path.clone().unwrap(), c.mode))
        .collect();
    assert_eq!(
        children,
        [
            (f.codex.join("AGENTS.md"), Mode::Always),
            (f.codex.join("AGENTS.override.md"), Mode::None)
        ]
    );
    let empty = &message.children[1];
    assert_eq!(
        empty.signal.as_deref(),
        Some("empty: codex falls through to ~/.codex/AGENTS.md")
    );
    assert!(empty.cost.not_applicable);
    assert_eq!(message.children[0].cost.predicted, Some(6));
    assert_eq!(message.cost.predicted, Some(framing(None, true, 0)));

    // No AGENTS.md behind it: the empty override is listed on its own, no message.
    std::fs::remove_file(f.codex.join("AGENTS.md")).unwrap();
    let t = resolve(&target(&f), None);
    assert!(find(&t, "/agents-md-message").is_none());
    let empty = row(&t, "/.codex/AGENTS.override.md");
    assert_eq!(empty.mode, Mode::None);
    assert_eq!(
        empty.signal.as_deref(),
        Some("empty: no global AGENTS file lands")
    );
}

#[test]
fn review_competing_fallback_names_take_the_first_hit_per_dir() {
    let f = fx();
    user_config(
        &f,
        None,
        "project_doc_fallback_filenames = [\"A.md\", \"B.md\"]\n",
    );
    write(&f.repo.join("A.md"), "a\n");
    write(&f.repo.join("B.md"), "b\n");
    let sub = f.repo.join("sub");
    write(&sub.join("AGENTS.md"), "agents\n");
    write(&sub.join("A.md"), "a2\n");
    let t = resolve(&target_at(&f, &sub), None);
    let rows = project_rows(&t);
    let picked: Vec<PathBuf> = rows
        .iter()
        .map(|r| r.origin.path.clone().unwrap())
        .collect();
    assert_eq!(picked, [f.repo.join("A.md"), sub.join("AGENTS.md")]);
    assert!(rows[0]
        .origin
        .why
        .iter()
        .any(|w| w.contains("shadows B.md")));
    assert!(rows[1]
        .origin
        .why
        .iter()
        .any(|w| w.contains("shadows A.md")));
}

#[test]
fn review_whitespace_ancestor_override_uses_no_budget() {
    let f = fx();
    user_config(&f, None, "");
    write(&f.repo.join("AGENTS.override.md"), " \n");
    write(&f.repo.join("AGENTS.md"), &"r".repeat(100));
    let sub = f.repo.join("sub");
    write(&sub.join("AGENTS.md"), &"s".repeat(32_768));
    let t = resolve(&target_at(&f, &sub), None);
    assert_eq!(
        project_rows(&t).len(),
        2,
        "the shadowed AGENTS.md has no row"
    );
    let empty = row(&t, "repo/AGENTS.override.md");
    assert_eq!(empty.mode, Mode::None);
    assert_eq!(
        empty.signal.as_deref(),
        Some("empty, yet shadows AGENTS.md")
    );
    let deep = row(&t, "sub/AGENTS.md");
    assert_eq!(
        deep.cost.predicted,
        Some(32_768),
        "the whole budget is left"
    );
    assert_eq!(deep.signal, None);
}

#[test]
fn review_message_children_are_in_request_order() {
    let f = fx();
    user_config(&f, None, "project_doc_max_bytes = 20\n");
    write(&f.codex.join("AGENTS.md"), "global\n");
    write(&f.repo.join("AGENTS.md"), &"r".repeat(15));
    let a = f.repo.join("a");
    write(&a.join("AGENTS.md"), &"a".repeat(10));
    let b = a.join("b");
    write(&b.join("AGENTS.md"), "late\n");
    let t = resolve(&target_at(&f, &b), None);
    let message = row(&t, "/agents-md-message");
    let order: Vec<(PathBuf, Mode, Option<u64>)> = message
        .children
        .iter()
        .map(|c| (c.origin.path.clone().unwrap(), c.mode, c.cost.predicted))
        .collect();
    assert_eq!(
        order,
        [
            (f.codex.join("AGENTS.md"), Mode::Always, Some(6)),
            (f.repo.join("AGENTS.md"), Mode::Always, Some(15)),
            (a.join("AGENTS.md"), Mode::Always, Some(5)),
            (b.join("AGENTS.md"), Mode::None, None),
        ]
    );
    assert_eq!(message.cost.predicted, Some(framing(Some(&b), true, 2)));
}

// Review round 2 (C1): fail on 9ae1390.

/// A non-finite float in a string-valued key is set (no lower layer shows through) and readable
/// as no type: not the string "nan".
#[test]
fn review2_non_finite_string_key_is_unknown_not_a_string() {
    let f = fx();
    write(
        &f.repo.join(".codex/config.toml"),
        "model = nan\ndeveloper_instructions = inf\n",
    );
    user_config(
        &f,
        Some((&f.repo, "trusted")),
        "model = \"user-model\"\ndeveloper_instructions = \"lower\"\n",
    );
    let t = resolve(&target(&f), None);
    let model = signal(&t, "model:");
    let at = format!("{}#/model", f.repo.join(".codex/config.toml").display());
    assert!(
        model.starts_with(&format!("model: unknown ({at} is not a string")),
        "{model}"
    );
    assert!(!model.contains("user-model"), "{model}");
    assert!(
        t.warnings
            .iter()
            .any(|w| w.starts_with(&at) && w.contains("non-finite float `nan`")),
        "{:?}",
        t.warnings
    );
    assert!(
        find(&t, "#/developer_instructions").is_none(),
        "neither \"inf\" nor the user layer's value"
    );
}

/// The non-finite list is taken after the overlay: a patch that replaces the value clears the
/// warning and makes the key readable again.
#[test]
fn review2_overlay_replacing_a_non_finite_value_clears_it() {
    let f = fx();
    user_config(&f, None, "include_environment_context = nan\n");
    let t = resolve(&target(&f), None);
    assert!(t.warnings.iter().any(|w| w.contains("non-finite")));
    let env = row(&t, "/environment_context");
    let (file, pointer, value, _) = setting(&env.knobs[0]);
    let overlay = Overlay {
        ops: vec![OverlayOp::SetJson {
            file: file.to_path_buf(),
            pointer: pointer.to_string(),
            value: value.clone(),
        }],
    };
    let after = resolve(&target(&f), Some(&overlay));
    assert!(find(&after, "/environment_context").is_none());
    assert!(
        !after.warnings.iter().any(|w| w.contains("non-finite")),
        "{:?}",
        after.warnings
    );
}
