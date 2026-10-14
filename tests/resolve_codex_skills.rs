//! Codex skills and plugins (P0.5 WU C2) against fixtures built per test in a temp dir:
//! `home/.codex` (the Codex home), `home/.agents` and `repo/` (a git checkout). Like
//! `tests/resolve_codex.rs`, nothing outside the temp dir is read except `/etc/codex`, which these
//! tests assume absent; the admin root (`/etc/codex/skills`) is covered in `src/resolve/codex`.

use harn::model::{self, Item, Kind, Knob, Layer, Mode, Overlay, OverlayOp, Scope, Target, Tree};
use harn::resolve::codex::{CAP_STATE_UNKNOWN, CATALOG_GATED, PROFILE_ENV};
use harn::resolve::{Codex, Resolver};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
        "harn-codex-skills-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let home = root.join("home");
    let codex = home.join(".codex");
    let repo = root.join("repo");
    std::fs::create_dir_all(codex.join("skills/.system")).unwrap();
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

fn skill(dir: &Path, name: &str, description: &str) -> PathBuf {
    let path = dir.join(name).join("SKILL.md");
    write(
        &path,
        &format!("---\nname: {name}\ndescription: {description}\n---\nBODY-SECRET\n"),
    );
    path
}

fn target(f: &Fx) -> Target {
    Target {
        dir: f.repo.clone(),
        home: f.home.clone(),
        env: BTreeMap::new(),
        model: None,
        context_window: Some(272_000),
        bytes_per_token: None,
    }
}

fn resolve(t: &Target, overlay: Option<&Overlay>) -> Tree {
    Codex.resolve(t, overlay).expect("resolve")
}

fn user_config(f: &Fx, text: &str) {
    write(&f.codex.join("config.toml"), text);
}

fn find<'t>(tree: &'t Tree, id: &str) -> Option<&'t Item> {
    model::walk(tree).find(|i| i.id == id)
}

fn row<'t>(tree: &'t Tree, id: &str) -> &'t Item {
    find(tree, id).unwrap_or_else(|| {
        let ids: Vec<&str> = model::walk(tree).map(|i| i.id.as_str()).collect();
        panic!("no row {id}: {ids:#?}")
    })
}

fn listing(tree: &Tree) -> &Item {
    row(tree, "codex/skill-listing/builtin/skills")
}

fn skill_ids(tree: &Tree) -> Vec<String> {
    let mut ids: Vec<String> = model::walk(tree)
        .filter(|i| i.kind == Kind::Skill)
        .map(|i| i.id.clone())
        .collect();
    ids.sort();
    ids
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

fn overlay_of(knob: &Knob) -> Overlay {
    let (file, pointer, value, _) = setting(knob);
    Overlay {
        ops: vec![OverlayOp::SetJson {
            file: file.to_path_buf(),
            pointer: pointer.to_string(),
            value: value.clone(),
        }],
    }
}

/// The `rendered:` line of an entry's admission chain.
fn rendered(item: &Item) -> &str {
    item.origin
        .why
        .iter()
        .find_map(|w| w.strip_prefix("rendered: "))
        .unwrap_or_else(|| panic!("{} has no rendered line: {:?}", item.id, item.origin.why))
}

fn why_has(item: &Item, needle: &str) -> bool {
    item.origin.why.iter().any(|w| w.contains(needle))
}

/// A plugin `demo@mkt` installed at `version` with a manifest named `manifest_name`.
fn plugin(f: &Fx, version: &str, manifest_name: &str) -> PathBuf {
    let root = f.codex.join("plugins/cache/mkt/demo").join(version);
    write(
        &root.join(".codex-plugin/plugin.json"),
        &format!(r#"{{"name": "{manifest_name}", "version": "{version}"}}"#),
    );
    root
}

/// `[hooks.state."<key>"] trusted_hash = "<hash>"` for every hook row the tree lists as not
/// trusted, with the hash codex computes (the row's `current hash` line, C3's `hook_hash`) — the
/// way approving it in codex's `/hooks` would record it.
fn trusted_state(tree: &Tree) -> String {
    let mut out = String::new();
    for item in model::walk(tree).filter(|i| i.kind == Kind::Hook) {
        let Some(hash) = item
            .origin
            .why
            .iter()
            .find_map(|w| w.strip_prefix("current hash "))
        else {
            continue;
        };
        let key = item
            .id
            .strip_prefix("codex/hook/plugin/")
            .unwrap_or(&item.id);
        out.push_str(&format!(
            "[hooks.state.\"{key}\"]\ntrusted_hash = \"{hash}\"\n"
        ));
    }
    out
}

#[test]
fn every_skill_root_is_found_and_untrusted_dot_codex_still_loads() {
    let f = fx();
    skill(
        &f.repo.join(".codex/skills"),
        "proj-dot",
        "From the project .codex.",
    );
    skill(
        &f.repo.join(".agents/skills"),
        "proj-agents",
        "From the repo .agents.",
    );
    skill(&f.codex.join("skills"), "user-codex", "From CODEX_HOME.");
    skill(
        &f.home.join(".agents/skills"),
        "user-agents",
        "From ~/.agents.",
    );
    skill(&f.codex.join("skills/.system"), "sys-one", "Bundled.");
    let p = plugin(&f, "1.0.0", "demo");
    skill(&p.join("skills"), "plug-one", "From a plugin.");
    // The repo is explicitly untrusted: its `.codex/config.toml` is ignored, its skills are not.
    user_config(
        &f,
        &format!(
            "[plugins.\"demo@mkt\"]\nenabled = true\n[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            f.repo.display()
        ),
    );
    let t = resolve(&target(&f), None);
    assert_eq!(
        skill_ids(&t),
        [
            "codex/skill/builtin/sys-one",
            "codex/skill/plugin/demo:plug-one",
            "codex/skill/project/proj-agents",
            "codex/skill/project/proj-dot",
            "codex/skill/user/user-agents",
            "codex/skill/user/user-codex",
        ]
    );
    let dot = row(&t, "codex/skill/project/proj-dot");
    assert!(why_has(dot, "skills load anyway"), "{:?}", dot.origin.why);
    assert_eq!(dot.mode, Mode::Advertised);
    // Every entry is a child of the listing; SKILL.md bodies never count.
    let l = listing(&t);
    assert_eq!(l.children.len(), 6);
    assert!(l
        .children
        .iter()
        .all(|c| c.mode == Mode::Advertised && !rendered(c).contains("BODY-SECRET")));
    // Catalog order: System, Repo, User (name order within a scope; the plugin skill is User).
    let order: Vec<&str> = l.children.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        order,
        [
            "sys-one",
            "proj-agents",
            "proj-dot",
            "demo:plug-one",
            "user-agents",
            "user-codex"
        ]
    );
}

#[test]
fn missing_frontmatter_or_description_is_skipped_with_a_warning() {
    let f = fx();
    let user = f.codex.join("skills");
    write(&user.join("plain/SKILL.md"), "no frontmatter here\n");
    write(
        &user.join("nodesc/SKILL.md"),
        "---\nname: nodesc\n---\nbody\n",
    );
    // `name` is optional: Codex falls back to the dir name (skills/src/parser.rs:64-69).
    write(
        &user.join("dir-named/SKILL.md"),
        "---\ndescription: Named by its dir.\n---\n",
    );
    // Codex does not read `.system` errors as warnings (loader/host.rs:331-334).
    write(&f.codex.join("skills/.system/bad/SKILL.md"), "nothing\n");
    user_config(&f, "");
    let t = resolve(&target(&f), None);
    assert_eq!(skill_ids(&t), ["codex/skill/user/dir-named"]);
    let warned = |needle: &str| t.warnings.iter().any(|w| w.contains(needle));
    assert!(
        warned("plain/SKILL.md: skipped by codex: missing YAML frontmatter"),
        "{:?}",
        t.warnings
    );
    assert!(
        warned("nodesc/SKILL.md: skipped by codex: missing field `description`"),
        "{:?}",
        t.warnings
    );
    assert!(!warned("bad/SKILL.md"), "{:?}", t.warnings);
}

#[test]
fn description_is_cut_at_1024_chars_and_whitespace_collapsed() {
    let f = fx();
    let long = "x".repeat(1_500);
    skill(&f.codex.join("skills"), "long", &long);
    write(
        &f.codex.join("skills/folded/SKILL.md"),
        "---\ndescription: >\n  Two\n  lines.\n---\n",
    );
    user_config(&f, "");
    let t = resolve(&target(&f), None);
    let item = row(&t, "codex/skill/user/long");
    let line = rendered(item);
    let cut = format!("{}...", "x".repeat(1_021));
    assert!(
        line.starts_with(&format!("- long: {cut} (file: ")),
        "{line}"
    );
    assert_eq!(item.cost.predicted, Some(line.len() as u64 + 1));
    assert!(why_has(item, "cut at 1,024 chars"));
    let folded = rendered(row(&t, "codex/skill/user/folded"));
    assert!(
        folded.starts_with("- folded: Two lines. (file: "),
        "{folded}"
    );
}

/// `ext/skills/src/catalog_prompt.rs:3-4`.
const SOURCE_INTRO: &str = "A skill is a set of instructions provided through a `SKILL.md` source. Below is the list of skills that can be used. Each entry includes a name, description, and source locator. `file` locators are on the host filesystem, `executor package` locators are owned by their execution environment, `cloud package` locators are opaque package identifiers, and `custom resource` locators use their provider's access mechanism.";
const HOST_INTRO: &str = "A skill is a set of local instructions to follow that is stored in a `SKILL.md` file. Below is the list of skills that can be used. Each entry includes a name, description, and a short path that can be expanded into an absolute path using the skill roots table.";

#[test]
fn entry_and_framing_bytes_match_a_hand_rendered_block() {
    let f = fx();
    skill(&f.codex.join("skills"), "alpha", "Does alpha.");
    skill(&f.codex.join("skills"), "beta", "Does beta.");
    user_config(&f, "model = \"m\"\n");
    write(
        &f.codex.join("models_cache.json"),
        r#"{"models": [{"slug": "m", "context_window": 100000, "include_skills_usage_instructions": false}]}"#,
    );
    let mut t = target(&f);
    t.context_window = None;
    let tree = resolve(&t, None);

    // Both renderings by hand (render.rs:258-267, catalog_prompt.rs:81-106, world_state.rs:208).
    let root = std::fs::canonicalize(f.codex.join("skills")).unwrap();
    let r = root.display().to_string();
    let abs = [
        format!("- alpha: Does alpha. (file: {r}/alpha/SKILL.md)"),
        format!("- beta: Does beta. (file: {r}/beta/SKILL.md)"),
    ];
    let short = [
        "- alpha: Does alpha. (file: r0/alpha/SKILL.md)".to_string(),
        "- beta: Does beta. (file: r0/beta/SKILL.md)".to_string(),
    ];
    let abs_block = format!(
        "<skills_instructions>\n## Skills\n{SOURCE_INTRO}\n### Available skills\n{}\n</skills_instructions>",
        abs.join("\n")
    );
    let short_block = format!(
        "<skills_instructions>\n## Skills\n{HOST_INTRO}\n### Skill roots\n- `r0` = `{r}`\n### Available skills\n{}\n</skills_instructions>",
        short.join("\n")
    );
    // Codex picks the form with fewer tokens, `ceil(bytes / 4)` per line (render.rs:154-160,
    // :1022-1061): the roots table is charged against the empty absolute body.
    let tok = |n: usize| n.div_ceil(4);
    let lines = |ls: &[String]| ls.iter().map(|l| tok(l.len() + 1)).sum::<usize>();
    let empty_abs = format!("\n## Skills\n{SOURCE_INTRO}\n### Available skills\n");
    let empty_short = format!(
        "\n## Skills\n{HOST_INTRO}\n### Skill roots\n- `r0` = `{r}`\n### Available skills\n"
    );
    let overhead = tok(empty_short.len()).saturating_sub(tok(empty_abs.len()));
    let (block, expected) = if overhead + lines(&short) < lines(&abs) {
        (short_block, short)
    } else {
        (abs_block, abs)
    };

    let l = listing(&tree);
    for (child, line) in l.children.iter().zip(&expected) {
        assert_eq!(rendered(child), line);
        assert_eq!(child.cost.predicted, Some(line.len() as u64 + 1));
        assert!(!child.cost.predicted_is_estimate);
    }
    let framing = row(&tree, "codex/system-prompt/builtin/skills_instructions");
    let entries: u64 = l.children.iter().filter_map(|c| c.cost.predicted).sum();
    assert_eq!(l.cost.predicted, Some(entries));
    assert_eq!(
        framing.cost.predicted.unwrap() + entries,
        block.len() as u64,
        "framing + entries = the whole block"
    );
    // The container is skipped by the totals; framing and entries are counted once each.
    assert!(tree.predicted_known >= block.len() as u64);
    // Request order: the block right after the base instructions (no developer_instructions).
    let ids: Vec<&str> = tree.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(
        &ids[1..3],
        [
            "codex/system-prompt/builtin/skills_instructions",
            "codex/skill-listing/builtin/skills"
        ]
    );
}

#[test]
fn budget_state_follows_the_context_window() {
    let f = fx();
    for name in ["a", "b", "c", "d"] {
        skill(&f.codex.join("skills"), name, &"word ".repeat(40));
    }
    user_config(&f, "");
    // Known window, far below the budget.
    let t = resolve(&target(&f), None);
    let l = listing(&t);
    assert!(why_has(l, "within budget"), "{:?}", l.origin.why);
    assert!(why_has(l, "budget 5440 codex tokens"), "{:?}", l.origin.why);
    assert_eq!(l.signal, None);
    assert!(!t.warnings.iter().any(|w| w == CAP_STATE_UNKNOWN));

    // A 1,000-token window: 20 tokens of budget; the listing does not fit.
    let mut small = target(&f);
    small.context_window = Some(1_000);
    let t = resolve(&small, None);
    let l = listing(&t);
    assert!(
        l.signal
            .as_deref()
            .is_some_and(|s| s.starts_with("over budget:")),
        "{:?}",
        l.signal
    );
    assert!(l.children.iter().all(|c| c.cost.predicted_is_estimate));

    // Unknown window: the state is unknown, the sizes stay as rendered in full.
    let mut unknown = target(&f);
    unknown.context_window = None;
    let t = resolve(&unknown, None);
    assert!(
        t.warnings.iter().any(|w| w == CAP_STATE_UNKNOWN),
        "{:?}",
        t.warnings
    );
    let l = listing(&t);
    assert_eq!(l.signal, None);
    assert!(l.children.iter().all(|c| !c.cost.predicted_is_estimate));

    // `skills.max_context_tokens` wins and is capped at 10,000 (render.rs:130-136).
    user_config(&f, "[skills]\nmax_context_tokens = 50000\n");
    let t = resolve(&unknown, None);
    let l = listing(&t);
    assert!(
        why_has(l, "budget 10000 codex tokens"),
        "{:?}",
        l.origin.why
    );
    assert!(why_has(l, "capped at 10,000"));
}

#[test]
fn skills_config_disable_via_overlay_removes_the_entry() {
    let f = fx();
    let path = skill(&f.codex.join("skills"), "alpha", "Does alpha.");
    skill(&f.codex.join("skills"), "beta", "Does beta.");
    user_config(&f, "");
    let t = resolve(&target(&f), None);
    let alpha = row(&t, "codex/skill/user/alpha");
    let (file, pointer, value, scope) = setting(&alpha.knobs[0]);
    assert_eq!(file, f.codex.join("config.toml"));
    assert_eq!(pointer, "/skills/config/-");
    let canonical = std::fs::canonicalize(&path).unwrap();
    assert_eq!(
        value,
        &json!({"path": canonical.display().to_string(), "enabled": false})
    );
    assert_eq!(scope, Scope::User);

    let after = resolve(&target(&f), Some(&overlay_of(&alpha.knobs[0])));
    assert_eq!(skill_ids(&after), ["codex/skill/user/beta"]);

    // The same rule written as a file: the folder path is not a selector (research/02 Tensions).
    let dir = canonical.parent().unwrap();
    user_config(
        &f,
        &format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\n[[skills.config]]\nname = \"beta\"\nenabled = false\n",
            dir.display()
        ),
    );
    let t = resolve(&target(&f), None);
    assert_eq!(
        skill_ids(&t),
        ["codex/skill/user/alpha"],
        "name selector works, folder does not"
    );
}

#[test]
fn skills_config_outside_the_user_layers_is_ignored() {
    let f = fx();
    let path = skill(&f.codex.join("skills"), "alpha", "Does alpha.");
    let canonical = std::fs::canonicalize(&path).unwrap();
    // A trusted project config: Codex reads [[skills.config]] from user layers only
    // (config/src/skills_config.rs:150-158).
    user_config(
        &f,
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            f.repo.display()
        ),
    );
    write(
        &f.repo.join(".codex/config.toml"),
        &format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            canonical.display()
        ),
    );
    let t = resolve(&target(&f), None);
    assert_eq!(skill_ids(&t), ["codex/skill/user/alpha"]);
    // Its knob still goes to the user config, not the narrower project file.
    let (file, ..) = setting(&row(&t, "codex/skill/user/alpha").knobs[0]);
    assert_eq!(file, f.codex.join("config.toml"));

    // A profile that re-enables it takes the knob (its rules come after the user file's).
    write(
        &f.codex.join("work.config.toml"),
        &format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = true\n",
            canonical.display()
        ),
    );
    let mut t = target(&f);
    t.env.insert(PROFILE_ENV.to_string(), "work".to_string());
    let tree = resolve(&t, None);
    let (file, ..) = setting(&row(&tree, "codex/skill/user/alpha").knobs[0]);
    assert_eq!(file, f.codex.join("work.config.toml"));
}

#[test]
fn bundled_enabled_false_removes_the_system_skills() {
    let f = fx();
    skill(
        &f.codex.join("skills/.system"),
        "imagegen",
        "Generate images.",
    );
    skill(&f.codex.join("skills"), "notes", "Keep notes.");
    user_config(&f, "");
    let t = resolve(&target(&f), None);
    let sys = row(&t, "codex/skill/builtin/imagegen");
    assert_eq!(sys.origin.layer, Layer::Builtin);
    let bundled = sys
        .knobs
        .iter()
        .find(|k| setting(k).1 == "/skills/bundled/enabled")
        .expect("bundled knob");
    // `.system` is not walked as part of `~/.codex/skills` (hidden dirs are pruned).
    assert_eq!(skill_ids(&t).len(), 2);

    let after = resolve(&target(&f), Some(&overlay_of(bundled)));
    assert_eq!(skill_ids(&after), ["codex/skill/user/notes"]);

    // A missing `.system` while bundled skills are on: codex installs it at startup.
    std::fs::remove_dir_all(f.codex.join("skills/.system")).unwrap();
    let t = resolve(&target(&f), None);
    assert!(
        t.warnings
            .iter()
            .any(|w| w.contains("codex installs its bundled skills")),
        "{:?}",
        t.warnings
    );
}

#[test]
fn include_instructions_false_drops_the_block() {
    let f = fx();
    skill(&f.codex.join("skills"), "notes", "Keep notes.");
    user_config(&f, "");
    let t = resolve(&target(&f), None);
    let knob = listing(&t).knobs[0].clone();
    assert_eq!(setting(&knob).1, "/skills/include_instructions");
    let after = resolve(&target(&f), Some(&overlay_of(&knob)));
    assert!(find(&after, "codex/skill-listing/builtin/skills").is_none());
    assert!(find(&after, "codex/system-prompt/builtin/skills_instructions").is_none());
}

#[test]
fn hidden_skills_stay_out_of_the_listing() {
    let f = fx();
    let path = skill(&f.codex.join("skills"), "manual", "Only by name.");
    write(
        &path.parent().unwrap().join("agents/openai.yaml"),
        "policy:\n  allow_implicit_invocation: false\n",
    );
    skill(&f.codex.join("skills"), "auto", "Listed.");
    user_config(&f, "");
    let t = resolve(&target(&f), None);
    let manual = row(&t, "codex/skill/user/manual");
    assert_eq!(manual.mode, Mode::OnDemand);
    assert!(manual.cost.not_applicable);
    let l = listing(&t);
    assert_eq!(l.label, "skill listing · 1");
    assert_eq!(
        l.cost.predicted,
        row(&t, "codex/skill/user/auto").cost.predicted
    );
}

#[test]
fn plugin_enabled_and_disabled() {
    let f = fx();
    let p = plugin(&f, "1.0.0", "demo");
    skill(&p.join("skills"), "search", "Search the docs.");
    user_config(&f, "[plugins.\"demo@mkt\"]\nenabled = true\n");
    let t = resolve(&target(&f), None);
    let row_ = row(&t, "codex/plugin/plugin/demo@mkt");
    assert_eq!(row_.origin.layer, Layer::Plugin);
    assert_eq!(
        row_.mode,
        Mode::Always,
        "an enabled plugin row is a switch: always"
    );
    assert!(row_.cost.not_applicable);
    match row_.knobs.as_slice() {
        [knob] => {
            let (file, pointer, value, scope) = setting(knob);
            assert_eq!(file, f.codex.join("config.toml"));
            assert_eq!(pointer, "/plugins/demo@mkt/enabled");
            assert_eq!(value, &json!(false));
            assert_eq!(scope, Scope::User);
        }
        other => panic!("{other:?}"),
    }
    // Turning it off through its knob takes its skill out of the listing.
    let after = resolve(&target(&f), Some(&overlay_of(&row_.knobs[0])));
    assert!(find(&after, "codex/skill/plugin/demo:search").is_none());
    let off = row(&after, "codex/plugin/plugin/demo@mkt");
    assert_eq!(off.mode, Mode::None);
    assert!(off.signal.as_deref().is_some_and(|s| s.starts_with("off:")));

    // `features.plugins = false` turns every plugin and both plugin blocks off.
    user_config(
        &f,
        "[plugins.\"demo@mkt\"]\nenabled = true\n[features]\nplugins = false\n",
    );
    let t = resolve(&target(&f), None);
    assert!(model::walk(&t).all(|i| i.origin.layer != Layer::Plugin));
    assert!(find(&t, "codex/system-prompt/builtin/recommended_plugins").is_none());

    // Configured but not installed: a warning and an inactive row.
    user_config(&f, "[plugins.\"ghost@mkt\"]\nenabled = true\n");
    let t = resolve(&target(&f), None);
    assert!(t
        .warnings
        .iter()
        .any(|w| w.contains("ghost@mkt\" is enabled but not installed")));
    assert_eq!(
        row(&t, "codex/plugin/plugin/ghost@mkt").signal.as_deref(),
        Some("not installed")
    );
}

#[test]
fn plugins_instructions_follow_the_catalog_flag() {
    const ID: &str = "codex/system-prompt/builtin/plugins_instructions";
    let f = fx();
    let p = plugin(&f, "1.0.0", "demo");
    skill(&p.join("skills"), "search", "Search the docs.");
    let cache = json!({
        "client_version": "0.160.1",
        "models": [
            {"slug": "on-model", "include_plugin_usage_instructions": true},
            {"slug": "off-model", "include_plugin_usage_instructions": false},
            {"slug": "silent-model"}
        ]
    });
    write(
        &f.codex.join("models_cache.json"),
        &serde_json::to_string(&cache).unwrap(),
    );
    let config = |model: &str| format!("{model}[plugins.\"demo@mkt\"]\nenabled = true\n");

    // true: rendered, the fixed text's exact size (available_plugins_instructions.rs:30-48).
    user_config(&f, &config("model = \"on-model\"\n"));
    let t = resolve(&target(&f), None);
    let r = row(&t, ID);
    assert_eq!((r.mode, r.signal.as_deref()), (Mode::Always, None));
    assert_eq!(r.cost.predicted, Some(1014));
    assert!(!r.cost.predicted_is_estimate);

    // false: codex renders nothing (world_state.rs:271-272).
    user_config(&f, &config("model = \"off-model\"\n"));
    let t = resolve(&target(&f), None);
    assert!(find(&t, ID).is_none());
    assert!(find(&t, "codex/system-prompt/builtin/recommended_plugins").is_some());

    // Unknown — the entry is silent (codex 0.160.1 always writes the flag), the model is not in
    // the cache, or there is no model: listed, marked not checkable, size if rendered in `why`.
    for (model, why) in [
        ("model = \"silent-model\"\n", "silent-model"),
        ("model = \"ghost-model\"\n", "ghost-model is not in"),
        ("", "no model in config"),
    ] {
        user_config(&f, &config(model));
        let t = resolve(&target(&f), None);
        let r = row(&t, ID);
        assert_eq!(r.mode, Mode::Always, "{model}");
        assert_eq!(r.cost.predicted, None, "{model}");
        let s = r.signal.as_deref().unwrap_or_default();
        assert!(
            s.starts_with(CATALOG_GATED)
                && s.contains("include_plugin_usage_instructions = true")
                && s.contains(why),
            "{model}: {s}"
        );
        assert!(why_has(r, "1014 bytes if rendered"), "{:?}", r.origin.why);
    }

    // No plugin with a part: no block, whatever the catalog says.
    user_config(&f, "model = \"on-model\"\n");
    assert!(find(&resolve(&target(&f), None), ID).is_none());
}

#[test]
fn plugin_version_and_namespace_follow_the_cache_and_the_manifest() {
    let f = fx();
    for v in ["1.2.0", "1.10.0"] {
        let p = plugin(&f, v, "demo-tools");
        skill(&p.join("skills"), &format!("s-{v}"), "Versioned.");
    }
    user_config(&f, "[plugins.\"demo@mkt\"]\n");
    let t = resolve(&target(&f), None);
    // Semver order, not byte order; the namespace is the manifest name, not the key.
    assert_eq!(skill_ids(&t), ["codex/skill/plugin/demo-tools:s-1.10.0"]);
    // A `local` install wins over any version.
    let p = plugin(&f, "local", "demo-tools");
    skill(&p.join("skills"), "s-local", "Local build.");
    let t = resolve(&target(&f), None);
    assert_eq!(skill_ids(&t), ["codex/skill/plugin/demo-tools:s-local"]);
}

#[test]
fn plugin_children_carry_the_plugin_and_their_switches() {
    let f = fx();
    let p = plugin(&f, "1.0.0", "demo");
    skill(&p.join("skills"), "search", "Search the docs.");
    write(
        &p.join(".mcp.json"),
        r#"{"mcpServers": {"docs": {"command": "docs-mcp"}, "extra": {"url": "http://localhost:1"}}}"#,
    );
    write(
        &p.join("hooks/hooks.json"),
        r#"{"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}, {"type": "prompt", "prompt": "x"}]}],
            "PreToolUse": [{"matcher": "shell", "hooks": [{"type": "command", "command": "check"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "bye"}]}]
        }}"#,
    );
    let base = "[plugins.\"demo@mkt\"]\nenabled = true\n[plugins.\"demo@mkt\".mcp_servers.extra]\nenabled = false\n";
    user_config(&f, base);
    // Approve every hook with the hash codex records, then turn `Stop` off (same table).
    let stop = "[hooks.state.\"demo@mkt:hooks/hooks.json:stop:0:0\"]\n";
    let trust = trusted_state(&resolve(&target(&f), None))
        .replace(stop, &format!("{stop}enabled = false\n"));
    assert!(trust.contains("enabled = false"));
    user_config(&f, &format!("{base}{trust}"));
    let t = resolve(&target(&f), None);

    let docs = row(&t, "codex/mcp-server/plugin/docs");
    assert_eq!(docs.origin.plugin.as_deref(), Some("demo@mkt"));
    assert_eq!(docs.mode, Mode::Always);
    assert_eq!(docs.cost, Default::default(), "schemas are `?`");
    assert_eq!(
        setting(&docs.knobs[0]).1,
        "/plugins/demo@mkt/mcp_servers/docs/enabled"
    );
    assert_eq!(setting(&docs.knobs[1]).1, "/plugins/demo@mkt/enabled");
    let extra = row(&t, "codex/mcp-server/plugin/extra");
    assert_eq!(
        extra.mode,
        Mode::None,
        "plugins.<p>.mcp_servers.<s>.enabled = false"
    );

    let start = row(
        &t,
        "codex/hook/plugin/demo@mkt:hooks/hooks.json:session_start:0:0",
    );
    assert_eq!(
        start.mode,
        Mode::Always,
        "SessionStart output reaches the model"
    );
    assert_eq!(start.cost, Default::default());
    let pointers: Vec<&str> = start.knobs.iter().map(|k| setting(k).1).collect();
    assert_eq!(
        pointers,
        [
            "/hooks/state/demo@mkt:hooks~1hooks.json:session_start:0:0/enabled",
            "/plugins/demo@mkt/enabled",
            "/features/hooks"
        ],
        "the hook's own state, then the plugin, then every hook"
    );
    assert_eq!(start.origin.plugin.as_deref(), Some("demo@mkt"));
    assert!(
        start.origin.why[0].contains("#/plugins/demo@mkt/enabled")
            || why_has(start, "on by default"),
        "the plugin's admission chain comes first: {:?}",
        start.origin.why
    );
    assert!(
        find(
            &t,
            "codex/hook/plugin/demo@mkt:hooks/hooks.json:session_start:0:1"
        )
        .is_none(),
        "prompt handlers are parsed but never run"
    );
    let pre = row(
        &t,
        "codex/hook/plugin/demo@mkt:hooks/hooks.json:pre_tool_use:0:0",
    );
    assert_eq!(pre.mode, Mode::Runtime);
    assert!(pre.cost.not_applicable);
    let stop = row(&t, "codex/hook/plugin/demo@mkt:hooks/hooks.json:stop:0:0");
    assert_eq!(stop.mode, Mode::None, "hooks.state enabled = false");

    // Without a trusted hash Codex does not run a plugin hook.
    user_config(&f, "[plugins.\"demo@mkt\"]\nenabled = true\n");
    let t = resolve(&target(&f), None);
    let start = row(
        &t,
        "codex/hook/plugin/demo@mkt:hooks/hooks.json:session_start:0:0",
    );
    assert_eq!(start.mode, Mode::None);
    assert!(start
        .signal
        .as_deref()
        .is_some_and(|s| s.starts_with("off: untrusted")));
    // `features.hooks = false` turns them all off; the rows stay (C3's rule).
    user_config(
        &f,
        "[plugins.\"demo@mkt\"]\nenabled = true\n[features]\nhooks = false\n",
    );
    let t = resolve(&target(&f), None);
    let hooks: Vec<&Item> = model::walk(&t).filter(|i| i.kind == Kind::Hook).collect();
    assert_eq!(hooks.len(), 3);
    assert!(hooks.iter().all(
        |h| h.mode == Mode::None && h.signal.as_deref() == Some("off: features.hooks = false")
    ));
}

#[test]
fn a_plugin_skill_appears_once_with_the_same_id_under_the_plugin() {
    let f = fx();
    let p = plugin(&f, "1.0.0", "demo");
    skill(&p.join("skills"), "search", "Search the docs.");
    user_config(&f, "[plugins.\"demo@mkt\"]\nenabled = true\n");
    let t = resolve(&target(&f), None);
    let id = "codex/skill/plugin/demo:search";
    assert_eq!(model::walk(&t).filter(|i| i.id == id).count(), 1);
    // In the listing, owned by the plugin through `origin.plugin` (the TUI's facet join).
    let entry = listing(&t)
        .children
        .iter()
        .find(|c| c.id == id)
        .expect("in the listing");
    assert_eq!(entry.origin.plugin.as_deref(), Some("demo@mkt"));
    assert_eq!(
        model::select(&t, id).unwrap().id,
        id,
        "the exact id is unambiguous"
    );
    let plugin_row = row(&t, "codex/plugin/plugin/demo@mkt");
    assert!(
        plugin_row.children.is_empty(),
        "parts are not duplicated under the plugin row"
    );
    assert!(why_has(plugin_row, "contributes: 1 skills"));
    // Its knobs: the per-skill rule first, then the plugin switch.
    let pointers: Vec<&str> = entry.knobs.iter().map(|k| setting(k).1).collect();
    assert_eq!(pointers, ["/skills/config/-", "/plugins/demo@mkt/enabled"]);
}

// --- review round 1 (regressions) ---

#[test]
fn openai_yaml_is_read_as_yaml_and_a_bad_file_is_ignored() {
    let f = fx();
    let user = f.codex.join("skills");
    let meta = |name: &str, yaml: &str| {
        let path = skill(&user, name, &format!("Skill {name}."));
        write(&path.parent().unwrap().join("agents/openai.yaml"), yaml);
    };
    // A trailing comment and the flow style both say false (serde_yaml, loader/metadata.rs:130).
    meta(
        "commented",
        "policy:\n  allow_implicit_invocation: false # only by name\n",
    );
    meta("flow", "policy: {allow_implicit_invocation: false}\n");
    // An unknown product makes the whole file invalid: codex ignores it (fail open, :130-139).
    meta(
        "badfile",
        "policy:\n  allow_implicit_invocation: false\n  products: [nope]\n",
    );
    // A product restriction without codex drops the skill (skills/src/model.rs:95-108).
    meta("chatonly", "policy:\n  products: [chatgpt]\n");
    meta("forcodex", "policy:\n  products: [CODEX, chatgpt]\n");
    user_config(&f, "");
    let t = resolve(&target(&f), None);
    assert_eq!(row(&t, "codex/skill/user/commented").mode, Mode::OnDemand);
    assert_eq!(row(&t, "codex/skill/user/flow").mode, Mode::OnDemand);
    assert_eq!(row(&t, "codex/skill/user/badfile").mode, Mode::Advertised);
    assert!(
        t.warnings
            .iter()
            .any(|w| w.contains("badfile/agents/openai.yaml") && w.contains("ignores it")),
        "{:?}",
        t.warnings
    );
    assert!(find(&t, "codex/skill/user/chatonly").is_none());
    assert_eq!(row(&t, "codex/skill/user/forcodex").mode, Mode::Advertised);
}

#[test]
fn a_malformed_skills_table_voids_the_layers_rules() {
    let f = fx();
    let alpha = std::fs::canonicalize(skill(&f.codex.join("skills"), "alpha", "A.")).unwrap();
    let beta = std::fs::canonicalize(skill(&f.codex.join("skills"), "beta", "B.")).unwrap();
    // A valid disabling rule, then a rule whose `path` is a number: the whole `skills` table
    // fails to deserialize, so the valid rule does not apply (config/src/skills_config.rs:160-169).
    user_config(
        &f,
        &format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\n[[skills.config]]\npath = 5\nenabled = false\n",
            alpha.display()
        ),
    );
    let t = resolve(&target(&f), None);
    assert_eq!(
        skill_ids(&t),
        ["codex/skill/user/alpha", "codex/skill/user/beta"]
    );
    assert!(
        t.warnings
            .iter()
            .any(|w| w.contains("/skills") && w.contains("rules")),
        "{:?}",
        t.warnings
    );
    // A wrong-typed sibling key voids the rules too.
    user_config(
        &f,
        &format!(
            "[skills]\ninclude_instructions = \"yes\"\n[[skills.config]]\npath = \"{}\"\nenabled = false\n",
            alpha.display()
        ),
    );
    let t = resolve(&target(&f), None);
    assert!(find(&t, "codex/skill/user/alpha").is_some());
    // Unknown keys are not an error: serde accepts them (only the JSON schema denies them).
    user_config(
        &f,
        &format!(
            "[[skills.config]]\npath = \"{}\"\nenabled = false\nnote = \"why\"\n",
            beta.display()
        ),
    );
    let t = resolve(&target(&f), None);
    assert_eq!(skill_ids(&t), ["codex/skill/user/alpha"]);
}

#[test]
fn invalid_manifest_fields_warn_and_follow_codex() {
    let f = fx();
    // An invalid `skills` path is ignored with a warning; codex then uses `skills/`
    // (core-plugins/src/manifest.rs:581-659, loader.rs:1081-1109).
    let p = f.codex.join("plugins/cache/mkt/demo/1.0.0");
    write(
        &p.join(".codex-plugin/plugin.json"),
        r#"{"name": "demo", "skills": "./../outside", "mcpServers": "servers.json", "hooks": {"bogus": 1}}"#,
    );
    skill(&p.join("skills"), "search", "Search.");
    write(
        &p.join(".mcp.json"),
        r#"{"mcpServers": {"docs": {"command": "d"}}}"#,
    );
    write(
        &p.join("hooks/hooks.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
    );
    user_config(&f, "[plugins.\"demo@mkt\"]\n");
    let t = resolve(&target(&f), None);
    let warned = |needle: &str| t.warnings.iter().any(|w| w.contains(needle));
    assert!(
        warned("ignoring skills: path must not contain '..'"),
        "{:?}",
        t.warnings
    );
    assert!(
        warned("ignoring mcpServers: path must start with `./`"),
        "{:?}",
        t.warnings
    );
    assert!(warned("ignoring hooks:"), "{:?}", t.warnings);
    assert!(
        find(&t, "codex/skill/plugin/demo:search").is_some(),
        "skills/ is the default"
    );
    assert!(
        find(&t, "codex/mcp-server/plugin/docs").is_some(),
        ".mcp.json is the default"
    );
    assert!(
        find(
            &t,
            "codex/hook/plugin/demo@mkt:hooks/hooks.json:session_start:0:0"
        )
        .is_some(),
        "an invalid inline hooks value falls back to hooks/hooks.json"
    );

    // A wrong-typed scalar field fails the whole manifest: the plugin loads nothing
    // (manifest.rs:48-73 has no fallback for it; loader.rs:879-882).
    write(&p.join(".codex-plugin/plugin.json"), r#"{"name": 5}"#);
    let t = resolve(&target(&f), None);
    assert!(find(&t, "codex/skill/plugin/demo:search").is_none());
    assert!(
        t.warnings
            .iter()
            .any(|w| w.contains("plugin demo@mkt") && w.contains("name")),
        "{:?}",
        t.warnings
    );
    assert_eq!(row(&t, "codex/plugin/plugin/demo@mkt").mode, Mode::None);
}

#[test]
fn plugin_rows_are_switches() {
    let f = fx();
    // Enabled with nothing in it.
    plugin(&f, "1.0.0", "demo");
    // Enabled with only a runtime hook.
    let r = f.codex.join("plugins/cache/mkt/rt/1.0.0");
    write(&r.join(".codex-plugin/plugin.json"), r#"{"name": "rt"}"#);
    write(
        &r.join("hooks/hooks.json"),
        r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "c"}]}]}}"#,
    );
    // Disabled.
    let o = f.codex.join("plugins/cache/mkt/off/1.0.0");
    write(&o.join(".codex-plugin/plugin.json"), r#"{"name": "off"}"#);
    let base =
        "[plugins.\"demo@mkt\"]\n[plugins.\"rt@mkt\"]\n[plugins.\"off@mkt\"]\nenabled = false\n";
    user_config(&f, base);
    let trust = trusted_state(&resolve(&target(&f), None));
    user_config(&f, &format!("{base}{trust}"));
    let t = resolve(&target(&f), None);
    assert_eq!(
        row(
            &t,
            "codex/hook/plugin/rt@mkt:hooks/hooks.json:pre_tool_use:0:0"
        )
        .mode,
        Mode::Runtime
    );
    for (id, mode) in [
        ("codex/plugin/plugin/demo@mkt", Mode::Always),
        ("codex/plugin/plugin/rt@mkt", Mode::Always),
        ("codex/plugin/plugin/off@mkt", Mode::None),
    ] {
        let item = row(&t, id);
        assert_eq!(item.mode, mode, "{id}");
        assert!(
            item.cost.not_applicable,
            "{id}: a switch costs nothing itself"
        );
    }
}

#[test]
fn hook_state_knob_targets_the_winning_file() {
    let f = fx();
    let p = plugin(&f, "1.0.0", "demo");
    write(
        &p.join("hooks/hooks.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "x"}]}]}}"#,
    );
    let key = "demo@mkt:hooks/hooks.json:session_start:0:0";
    user_config(&f, "[plugins.\"demo@mkt\"]\n");
    let trust = trusted_state(&resolve(&target(&f), None));
    // The user file turns it off; the profile turns it on and trusts it, and is read later: it
    // wins, so the knob must go there.
    user_config(
        &f,
        &format!("[plugins.\"demo@mkt\"]\n[hooks.state.\"{key}\"]\nenabled = false\n"),
    );
    write(
        &f.codex.join("work.config.toml"),
        &format!("{trust}enabled = true\n"),
    );
    let mut t = target(&f);
    t.env.insert(PROFILE_ENV.to_string(), "work".to_string());
    let id = format!("codex/hook/plugin/{key}");
    let tree = resolve(&t, None);
    let hook = row(&tree, &id);
    assert_eq!(hook.mode, Mode::Always);
    let (file, ..) = setting(&hook.knobs[0]);
    assert_eq!(file, f.codex.join("work.config.toml"));
    // Applying the emitted knob turns the hook off.
    let after = resolve(&t, Some(&overlay_of(&hook.knobs[0])));
    assert_eq!(row(&after, &id).mode, Mode::None);
}

#[test]
fn plugin_hooks_follow_the_shared_classifier_and_trust() {
    let f = fx();
    let p = plugin(&f, "1.0.0", "demo");
    write(
        &p.join("hooks/hooks.json"),
        r#"{"hooks": {
            "SessionStart": [
                {"hooks": [{"type": "command", "command": "echo start"}]},
                {"matcher": "resume", "hooks": [{"type": "command", "command": "echo resume"}]},
                {"matcher": "startup|clear", "hooks": [{"type": "command", "command": "echo async", "async": true}]}
            ],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "echo prompt"}]}],
            "SubagentStart": [{"hooks": [{"type": "command", "command": "echo sub"}]}]
        }}"#,
    );
    user_config(&f, "[plugins.\"demo@mkt\"]\n");
    let id = |rest: &str| format!("codex/hook/plugin/demo@mkt:hooks/hooks.json:{rest}");

    // Untrusted: nothing runs.
    let t = resolve(&target(&f), None);
    let start = row(&t, &id("session_start:0:0"));
    assert_eq!(start.mode, Mode::None);
    assert!(
        why_has(start, "current hash sha256:"),
        "{:?}",
        start.origin.why
    );

    // A recorded hash that is not the current one: modified, still off.
    user_config(
        &f,
        &format!(
            "[plugins.\"demo@mkt\"]\n[hooks.state.\"{}\"]\ntrusted_hash = \"sha256:0\"\n",
            id("session_start:0:0").trim_start_matches("codex/hook/plugin/")
        ),
    );
    let t = resolve(&target(&f), None);
    assert!(row(&t, &id("session_start:0:0"))
        .signal
        .as_deref()
        .is_some_and(|s| s.starts_with("off: modified")));

    // Trusted with the hash codex computes: classified by C3's rules.
    user_config(&f, "[plugins.\"demo@mkt\"]\n");
    let trust = trusted_state(&resolve(&target(&f), None));
    user_config(&f, &format!("[plugins.\"demo@mkt\"]\n{trust}"));
    let t = resolve(&target(&f), None);
    let start = row(&t, &id("session_start:0:0"));
    assert_eq!(
        start.mode,
        Mode::Always,
        "a trusted SessionStart hook with a matching hash"
    );
    assert!(
        why_has(start, "matches the current hash"),
        "{:?}",
        start.origin.why
    );
    assert_eq!(start.cost, Default::default(), "its output is `?`");
    assert_eq!(
        row(&t, &id("session_start:1:0")).mode,
        Mode::Runtime,
        "matcher `resume` does not match `startup`"
    );
    assert_eq!(
        row(&t, &id("session_start:2:0")).mode,
        Mode::Runtime,
        "async"
    );
    assert_eq!(row(&t, &id("user_prompt_submit:0:0")).mode, Mode::Always);
    assert_eq!(row(&t, &id("subagent_start:0:0")).mode, Mode::Runtime);
}

#[test]
fn wrong_typed_or_non_finite_settings_are_set_and_warned() {
    let f = fx();
    skill(&f.codex.join("skills"), "notes", "Keep notes.");
    skill(
        &f.codex.join("skills/.system"),
        "imagegen",
        "Generate images.",
    );
    let p = plugin(&f, "1.0.0", "demo");
    skill(&p.join("skills"), "search", "Search.");
    let refuses = |t: &Tree, at: &str| {
        t.warnings
            .iter()
            .any(|w| w.contains(at) && w.contains("codex refuses to start"))
    };
    // A string where a boolean goes: set, so not the `false` that would turn the block off, and
    // not silently the default either — the row stays with the reason, and codex will not start.
    user_config(
        &f,
        "[skills]\ninclude_instructions = \"no\"\n[skills.bundled]\nenabled = nan\n[plugins.\"demo@mkt\"]\nenabled = \"yes\"\n[features]\nplugins = 1\n",
    );
    let t = resolve(&target(&f), None);
    assert!(
        refuses(&t, "#/skills/include_instructions"),
        "{:?}",
        t.warnings
    );
    assert!(refuses(&t, "#/skills/bundled/enabled"), "{:?}", t.warnings);
    assert!(
        refuses(&t, "#/plugins/demo@mkt/enabled"),
        "{:?}",
        t.warnings
    );
    assert!(refuses(&t, "#/features/plugins"), "{:?}", t.warnings);
    let l = listing(&t);
    assert!(
        why_has(l, "include_instructions = \"no\" is not a boolean"),
        "{:?}",
        l.origin.why
    );
    let sys = row(&t, "codex/skill/builtin/imagegen");
    assert!(why_has(sys, "non-finite float"), "{:?}", sys.origin.why);
    // The plugin is neither on nor off: listed, loading nothing.
    let demo = row(&t, "codex/plugin/plugin/demo@mkt");
    assert_eq!(demo.mode, Mode::None);
    assert!(demo
        .signal
        .as_deref()
        .is_some_and(|s| s.starts_with("unreadable")));
    assert!(find(&t, "codex/skill/plugin/demo:search").is_none());
    assert!(why_has(
        row(&t, "codex/system-prompt/builtin/recommended_plugins"),
        "features/plugins"
    ));

    // A wrong-typed budget: unknown, never the 2 % default.
    user_config(&f, "[skills]\nmax_context_tokens = -5\n");
    let t = resolve(&target(&f), None);
    assert!(
        refuses(&t, "#/skills/max_context_tokens"),
        "{:?}",
        t.warnings
    );
    let l = listing(&t);
    assert!(!why_has(l, "budget 5440"), "{:?}", l.origin.why);
    assert!(why_has(l, "budget unknown"), "{:?}", l.origin.why);
    assert_eq!(l.signal, None);
}
