//! Codex knobs end to end (P0.5 C4): plan → apply → journal → undo / on on `config.toml`, with the
//! real Codex resolver on a fixture built per test in a temp dir (`home/.codex` = the Codex home,
//! `repo/` = a git checkout) and the journal in the same temp dir. `/etc/codex` is assumed absent,
//! as in `tests/resolve_codex.rs`.

use harn::journal::{self, Journal};
use harn::model::{
    self, Agent, Item, Kind, Knob, Layer, Mode, Origin, Overlay, OverlayOp, Refused, Scope, Target,
    Tree,
};
use harn::plan::{self, PlanOp, PlanRequest, PLAN_SCHEMA};
use harn::resolve::{Codex, Resolver};
use harn::writers;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const ENV: &str = "codex/env-block/builtin/environment_context";
const PLUGIN: &str = "codex/plugin/user/x@y";
const SKILL: &str = "codex/skill/user/one";

/// The user config the tests start from: comments, a trailing comment, a quoted table key.
const USER_CONFIG: &str =
    "# my codex config\nmodel = \"gpt-x\" # pinned\n\n[plugins.\"a@b\"]\nenabled = true\n";
/// Where root keys end in [`USER_CONFIG`] (`extra` goes here).
const ROOT_END: &str = "model = \"gpt-x\" # pinned\n";

struct Fx {
    root: PathBuf,
    home: PathBuf,
    codex: PathBuf,
    repo: PathBuf,
    journal: Journal,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fx() -> Fx {
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "harn-codex-knobs-{}-{}",
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
        journal: Journal {
            dir: root.join("state"),
        },
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

impl Fx {
    fn target(&self) -> Target {
        Target {
            dir: self.repo.clone(),
            home: self.home.clone(),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        }
    }

    fn user_file(&self) -> PathBuf {
        self.codex.join("config.toml")
    }

    fn project_file(&self) -> PathBuf {
        self.repo.join(".codex/config.toml")
    }

    /// [`USER_CONFIG`] with the root keys `extra` after its own, and the target dir trusted when
    /// `trusted`.
    fn user_config(&self, trusted: bool, extra: &str) -> String {
        let mut text = USER_CONFIG.replace(ROOT_END, &format!("{ROOT_END}{extra}"));
        if trusted {
            text.push_str(&format!(
                "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                self.repo.display()
            ));
        }
        write(&self.user_file(), &text);
        text
    }

    fn resolve(&self) -> Tree {
        Codex.resolve(&self.target(), None).expect("resolve")
    }

    fn plan(
        &self,
        tree: &Tree,
        id: &str,
        scope: Option<Scope>,
    ) -> anyhow::Result<plan::ActionPlan> {
        plan::build_plan(&Codex, &self.target(), tree, id, scope)
    }

    fn off(&mut self, tree: &Tree, id: &str, scope: Option<Scope>) -> plan::ActionPlan {
        let plan = self.plan(tree, id, scope).unwrap();
        writers::apply(&plan, &mut self.journal).unwrap();
        plan
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn refused(err: &anyhow::Error) -> String {
    err.chain()
        .find_map(|c| c.downcast_ref::<Refused>())
        .unwrap_or_else(|| panic!("not Refused: {err:#}"))
        .0
        .clone()
}

fn row<'t>(tree: &'t Tree, suffix: &str) -> &'t Item {
    model::walk(tree)
        .find(|i| i.id.ends_with(suffix))
        .unwrap_or_else(|| panic!("no row ending in {suffix}"))
}

fn knob_file(knob: &Knob) -> &Path {
    match knob {
        Knob::Setting { file, .. } => file,
        other => panic!("not a setting: {other:?}"),
    }
}

/// An item C2 will list (a plugin, a skill), built by hand with the knob C2 will give it.
fn hand_item(id: &str, kind: Kind, file: &Path, pointer: &str, value: Value) -> Item {
    Item {
        id: id.to_string(),
        label: id.rsplit('/').next().unwrap().to_string(),
        kind,
        origin: Origin {
            layer: Layer::User,
            path: None,
            setting: None,
            plugin: None,
            why: Vec::new(),
        },
        mode: Mode::Advertised,
        cost: Default::default(),
        knobs: vec![Knob::Setting {
            file: file.to_path_buf(),
            pointer: pointer.to_string(),
            value,
            scope: Scope::User,
            note: None,
        }],
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

/// What a tree says the request holds: rows, totals, signals (warnings aside).
fn shape(tree: &Tree) -> Value {
    json!({
        "items": tree.items,
        "predicted_known": tree.predicted_known,
        "unknown_rows": tree.unknown_rows,
        "signals": tree.signals,
    })
}

fn overlay_of(plan: &plan::ActionPlan) -> Overlay {
    Overlay {
        ops: plan
            .ops
            .iter()
            .map(|op| match &op.knob {
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
                other => panic!("not a setting: {other:?}"),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------------------------
// default scope, explicit scope

#[test]
fn default_scope_is_project_when_trusted_else_user() {
    let f = fx();
    f.user_config(false, "");
    let untrusted = f.resolve();
    let plan = f.plan(&untrusted, ENV, None).unwrap();
    assert_eq!(plan.ops[0].file.as_deref(), Some(f.user_file().as_path()));
    assert_eq!(plan::default_scope(row(&untrusted, ENV)), Scope::User);

    f.user_config(true, "");
    let trusted = f.resolve();
    let plan = f.plan(&trusted, ENV, None).unwrap();
    assert_eq!(
        plan.ops[0].file.as_deref(),
        Some(f.project_file().as_path())
    );
    assert_eq!(plan::default_scope(row(&trusted, ENV)), Scope::Project);
    // Claude Code keeps `local`.
    let mut claude = row(&trusted, ENV).clone();
    claude.id = "claude-code/env-block/builtin/environment".to_string();
    assert_eq!(plan::default_scope(&claude), Scope::Local);
}

#[test]
fn explicit_scope_is_never_widened() {
    let f = fx();
    f.user_config(false, "");
    let untrusted = f.resolve();
    for scope in [Scope::Local, Scope::Project] {
        let err = f.plan(&untrusted, ENV, Some(scope)).unwrap_err();
        assert!(
            refused(&err).contains("scope would widen to user; pass --scope user"),
            "{scope}: {err:#}"
        );
    }

    f.user_config(true, "");
    let trusted = f.resolve();
    let err = f.plan(&trusted, ENV, Some(Scope::Local)).unwrap_err();
    assert!(
        refused(&err).contains("scope would widen to project; pass --scope project"),
        "{err:#}"
    );
    let user = f.plan(&trusted, ENV, Some(Scope::User)).unwrap();
    assert_eq!(user.ops[0].file.as_deref(), Some(f.user_file().as_path()));
    let project = f.plan(&trusted, ENV, Some(Scope::Project)).unwrap();
    assert_eq!(
        project.ops[0].file.as_deref(),
        Some(f.project_file().as_path())
    );

    // A plan request: no scope = the default, so the user knob (index 1) is wider than it.
    let request = |scope: Option<Scope>, knob: Option<usize>| PlanRequest {
        schema: PLAN_SCHEMA.to_string(),
        target_dir: f.repo.clone(),
        ops: vec![PlanOp {
            selector: ENV.to_string(),
            scope,
            knob,
            reason: "not needed".to_string(),
        }],
    };
    let target = f.target();
    let err = plan::build_plan_from_request(&Codex, &target, &trusted, &request(None, Some(1)))
        .unwrap_err();
    assert!(
        refused(&err).contains("writes user scope, wider than project"),
        "{err:#}"
    );
    let ok = plan::build_plan_from_request(
        &Codex,
        &target,
        &trusted,
        &request(Some(Scope::User), Some(1)),
    )
    .unwrap();
    assert_eq!(ok.ops[0].file.as_deref(), Some(f.user_file().as_path()));
    let default =
        plan::build_plan_from_request(&Codex, &target, &trusted, &request(None, None)).unwrap();
    assert_eq!(
        default.ops[0].file.as_deref(),
        Some(f.project_file().as_path())
    );
}

// ---------------------------------------------------------------------------------------------
// apply, undo, on

#[test]
fn off_writes_the_toml_key_and_undo_restores_byte_for_byte() {
    let mut f = fx();
    let before = f.user_config(false, "");
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    std::fs::set_permissions(f.user_file(), std::fs::Permissions::from_mode(0o600)).unwrap();
    let tree = f.resolve();
    let plan = f.off(&tree, ENV, None);
    assert_eq!(
        mode(&f.user_file()),
        0o600,
        "the rewrite keeps the file's mode"
    );
    assert!(
        plan.diff.contains("+include_environment_context = false\n"),
        "{}",
        plan.diff
    );
    assert_eq!(
        read(&f.user_file()),
        before.replace(
            "model = \"gpt-x\" # pinned\n",
            "model = \"gpt-x\" # pinned\ninclude_environment_context = false\n"
        ),
        "the key lands after the last root key; comments and tables stay"
    );
    assert!(model::walk(&f.resolve()).all(|i| i.id != ENV));

    let history = journal::history(&f.journal).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].context.agent, Agent::Codex);
    assert_eq!(history[0].ops[0].item_id, ENV);

    journal::undo(&mut f.journal, 1).unwrap();
    assert_eq!(read(&f.user_file()), before);
    assert_eq!(mode(&f.user_file()), 0o600);
}

#[test]
fn developer_instructions_off_writes_an_empty_string_and_freed_matches_the_file() {
    let mut f = fx();
    let text = "be brief and kind";
    let before = f.user_config(false, &format!("developer_instructions = \"{text}\"\n"));
    let tree = f.resolve();
    let id = row(&tree, "#/developer_instructions").id.clone();
    let plan = f.plan(&tree, &id, None).unwrap();
    assert_eq!(plan.freed.known, text.len() as u64);
    assert!(!plan.freed.delta_unknown);
    assert!(
        plan.diff.contains(&format!(
            "-developer_instructions = \"{text}\"\n+developer_instructions = \"\"\n"
        )),
        "{}",
        plan.diff
    );

    // The overlay re-resolve is the tree the written file gives.
    let virtual_after = Codex
        .resolve(&f.target(), Some(&overlay_of(&plan)))
        .unwrap();
    writers::apply(&plan, &mut f.journal).unwrap();
    let after = f.resolve();
    assert_eq!(shape(&virtual_after), shape(&after));
    assert_eq!(
        tree.predicted_known - after.predicted_known,
        plan.freed.known
    );

    journal::undo(&mut f.journal, 1).unwrap();
    assert_eq!(read(&f.user_file()), before);
}

#[test]
fn project_scope_creates_the_file_and_undo_takes_it_away() {
    let mut f = fx();
    let user_before = f.user_config(true, "");
    let tree = f.resolve();
    assert!(!f.project_file().exists());
    let id = row(&tree, "/apps_instructions").id.clone();
    let plan = f.plan(&tree, &id, None).unwrap();
    assert!(
        plan.diff.starts_with("--- /dev/null\n"),
        "a new file: {}",
        plan.diff
    );
    // The overlay, resolved while neither the file nor `.codex/` exists, …
    assert!(!f.repo.join(".codex").exists());
    let virtual_after = Codex
        .resolve(&f.target(), Some(&overlay_of(&plan)))
        .unwrap();
    writers::apply(&plan, &mut f.journal).unwrap();
    assert_eq!(
        read(&f.project_file()),
        "include_apps_instructions = false\n"
    );
    // … is the tree the written file gives.
    let after = f.resolve();
    assert!(model::walk(&after).all(|i| !i.id.ends_with("/apps_instructions")));
    assert_eq!(shape(&virtual_after), shape(&after));

    journal::undo(&mut f.journal, 1).unwrap();
    assert!(!f.project_file().exists(), "parked, not left behind");
    assert_eq!(read(&f.user_file()), user_before);
    // harn deletes nothing: the `.codex/` dir it created stays, empty, and changes no row.
    assert!(f.repo.join(".codex").is_dir());
    assert_eq!(shape(&f.resolve()), shape(&tree));
}

#[test]
fn plugin_and_skill_knobs_write_quoted_tables_and_array_blocks() {
    let mut f = fx();
    let before = f.user_config(false, "");
    let user = f.user_file();
    let mut tree = f.resolve();
    tree.items.push(hand_item(
        PLUGIN,
        Kind::Plugin,
        &user,
        "/plugins/x@y/enabled",
        json!(false),
    ));
    tree.items.push(hand_item(
        SKILL,
        Kind::Skill,
        &user,
        "/skills/config/-",
        json!({"path": "/s/one/SKILL.md", "enabled": false}),
    ));
    tree.items.push(hand_item(
        "codex/skill/user/two",
        Kind::Skill,
        &user,
        "/skills/config/-",
        json!({"path": "/s/two/SKILL.md", "enabled": false}),
    ));

    // One request, two ops on one file: one patch, one snapshot.
    let request = PlanRequest {
        schema: PLAN_SCHEMA.to_string(),
        target_dir: f.repo.clone(),
        ops: [PLUGIN, SKILL]
            .iter()
            .map(|selector| PlanOp {
                selector: selector.to_string(),
                scope: None,
                knob: None,
                reason: "unused".to_string(),
            })
            .collect(),
    };
    let plan = plan::build_plan_from_request(&Codex, &f.target(), &tree, &request).unwrap();
    let entry = writers::apply(&plan, &mut f.journal).unwrap();
    assert_eq!(entry.snapshots.len(), 1);
    let once = read(&user);
    assert_eq!(
        once,
        format!(
            "{USER_CONFIG}\n[plugins.\"x@y\"]\nenabled = false\n\n[[skills.config]]\npath = \"/s/one/SKILL.md\"\nenabled = false\n"
        )
    );

    // A second skill appends a second `[[skills.config]]` block.
    f.off(&tree, "codex/skill/user/two", None);
    let twice = read(&user);
    assert_eq!(
        twice,
        format!("{once}\n[[skills.config]]\npath = \"/s/two/SKILL.md\"\nenabled = false\n")
    );

    journal::undo(&mut f.journal, 1).unwrap();
    assert_eq!(read(&user), once);
    journal::undo(&mut f.journal, 1).unwrap();
    assert_eq!(read(&user), before);
}

#[test]
fn on_restores_the_file_through_the_journal() {
    let mut f = fx();
    let before = f.user_config(false, "");
    let tree = f.resolve();
    f.off(&tree, ENV, None);
    assert_ne!(read(&f.user_file()), before);
    let target = f.target();
    let entry = journal::on(&mut f.journal, &target, "environment_context").unwrap();
    assert_eq!(entry.ops[0].item_id, ENV);
    assert_eq!(read(&f.user_file()), before);
    assert!(model::walk(&f.resolve()).any(|i| i.id == ENV));
}

#[test]
fn a_file_changed_since_the_plan_is_stale() {
    let mut f = fx();
    f.user_config(false, "");
    let tree = f.resolve();
    let plan = f.plan(&tree, ENV, None).unwrap();
    // Codex rewrites its own config (a trust entry) between plan and apply.
    let mut text = read(&f.user_file());
    text.push_str("\n[projects.\"/elsewhere\"]\ntrust_level = \"trusted\"\n");
    write(&f.user_file(), &text);
    let err = writers::apply(&plan, &mut f.journal).unwrap_err();
    assert!(
        err.chain()
            .any(|c| c.downcast_ref::<model::Stale>().is_some()),
        "{err:#}"
    );
    assert_eq!(read(&f.user_file()), text);
}

// ---------------------------------------------------------------------------------------------
// warnings

#[test]
fn the_user_config_warns_that_codex_writes_it_too_while_codex_runs() {
    let f = fx();
    f.user_config(false, "");
    let plan = f.plan(&f.resolve(), ENV, None).unwrap();
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("scope user, codex has no local-only config file")),
        "{:?}",
        plan.warnings
    );

    // The process probe decides it, and is asked only when the user config is written.
    let target = f.target();
    let (user, project) = (f.user_file(), f.project_file());
    let warning =
        plan::codex_config_warning(&target, &[project.as_path(), user.as_path()], || true)
            .expect("codex running");
    assert_eq!(
        warning,
        format!(
            "{} is also written by Codex (trust entries, hook state) and a `codex` process is \
             running now: apply with no codex running",
            user.display()
        )
    );
    assert_eq!(
        plan::codex_config_warning(&target, &[user.as_path()], || false),
        None,
        "no codex running: no race"
    );
    assert_eq!(
        plan::codex_config_warning(&target, &[project.as_path()], || panic!("not asked")),
        None,
        "the project file is Codex's to read, not to rewrite"
    );
    // `CODEX_HOME` moves the file the warning is about.
    let mut moved = target.clone();
    let elsewhere = f.root.join("codex-home");
    std::fs::create_dir_all(&elsewhere).unwrap();
    moved
        .env
        .insert("CODEX_HOME".to_string(), elsewhere.display().to_string());
    assert_eq!(
        plan::codex_config_warning(&moved, &[user.as_path()], || panic!("not asked")),
        None
    );
    assert!(
        plan::codex_config_warning(&moved, &[elsewhere.join("config.toml").as_path()], || true)
            .is_some()
    );

    // A trusted dir: the plan writes the project file only, whatever runs.
    f.user_config(true, "");
    let plan = f.plan(&f.resolve(), ENV, None).unwrap();
    assert!(
        !plan
            .warnings
            .iter()
            .any(|w| w.contains("is also written by Codex")),
        "{:?}",
        plan.warnings
    );
}

#[test]
fn a_checked_in_project_config_warns_it_is_tracked() {
    let f = fx();
    std::fs::remove_dir_all(f.repo.join(".git")).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&f.repo)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    write(&f.project_file(), "model = \"team\"\n");
    git(&["add", ".codex/config.toml"]);
    f.user_config(true, "");
    let plan = f.plan(&f.resolve(), ENV, None).unwrap();
    let file = f.project_file().display().to_string();
    assert!(
        plan.warnings
            .iter()
            .any(|w| *w == format!("{file} is tracked by git: the change shows up in the repo")),
        "{:?}",
        plan.warnings
    );
}

// ---------------------------------------------------------------------------------------------
// annotate_freed

#[test]
fn annotate_freed_fills_codex_rows() {
    let f = fx();
    let text = "be brief";
    f.user_config(false, &format!("developer_instructions = \"{text}\"\n"));
    let mut tree = f.resolve();
    // The fixture's `[plugins."a@b"]` is not installed and it has no bundled skills: the resolver
    // warns about both (C2); annotating adds nothing to that.
    let resolved_warnings = tree.warnings.clone();
    plan::annotate_freed(&Codex, &f.target(), &mut tree).unwrap();
    let dev = row(&tree, "#/developer_instructions")
        .freed
        .clone()
        .unwrap();
    assert_eq!((dev.known, dev.delta_unknown), (text.len() as u64, false));
    let env = row(&tree, ENV).freed.clone().unwrap();
    assert_eq!(
        (env.known, env.delta_unknown),
        (0, true),
        "the block is `?`"
    );
    assert_eq!(tree.warnings, resolved_warnings);
    // No knob at all: no freed.
    assert!(row(&tree, "codex/system-prompt/builtin/instructions")
        .freed
        .is_none());

    // Trusted: the default is the project knob, annotated the same way.
    f.user_config(true, &format!("developer_instructions = \"{text}\"\n"));
    let mut tree = f.resolve();
    assert_eq!(
        knob_file(&row(&tree, ENV).knobs[0]),
        f.project_file().as_path()
    );
    plan::annotate_freed(&Codex, &f.target(), &mut tree).unwrap();
    assert!(row(&tree, ENV).freed.is_some());
}

/// A knob the writer refuses (through a scalar, past i64) must not show freed bytes the apply
/// can never deliver: the re-resolve fails, freed stays unknown and the tree says why.
#[test]
fn annotate_freed_leaves_a_knob_the_writer_refuses_unknown() {
    let f = fx();
    let text = "be brief";
    f.user_config(false, &format!("developer_instructions = \"{text}\"\n"));
    for (pointer, value, says) in [
        (
            "/developer_instructions/x",
            json!(""),
            "/developer_instructions is a string, not a table or array",
        ),
        (
            "/developer_instructions",
            json!(u64::MAX),
            "past TOML's 64-bit integer range",
        ),
    ] {
        let mut tree = f.resolve();
        let dev = tree
            .items
            .iter_mut()
            .find(|i| i.id.ends_with("#/developer_instructions"))
            .unwrap();
        let id = dev.id.clone();
        dev.knobs = vec![Knob::Setting {
            file: f.user_file(),
            pointer: pointer.to_string(),
            value,
            scope: Scope::User,
            note: None,
        }];
        let err = f.plan(&tree, &id, None).unwrap_err();
        assert!(format!("{err:#}").contains(says), "{pointer}: {err:#}");

        plan::annotate_freed(&Codex, &f.target(), &mut tree).unwrap();
        assert_eq!(
            row(&tree, "#/developer_instructions").freed,
            None,
            "{pointer}"
        );
        assert!(
            tree.warnings
                .iter()
                .any(|w| w.starts_with("freed not computed for 1 item(s)") && w.contains(says)),
            "{pointer}: {:?}",
            tree.warnings
        );
    }
}

// ---------------------------------------------------------------------------------------------
// the binary: off / --dry-run / history / undo / on

fn harn(f: &Fx, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_harn"))
        .env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env_remove("CODEX_HOME")
        .env_remove("HARN_CODEX_PROFILE")
        .env_remove("COLUMNS")
        .args(["--agent", "codex", "--dir"])
        .arg(&f.repo)
        .args(args)
        .output()
        .expect("run harn")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn cli_off_dry_run_history_undo_and_on() {
    let f = fx();
    let before = f.user_config(false, "");

    let dry = harn(&f, &["off", "environment_context", "--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    let text = stdout(&dry);
    assert!(
        text.contains(" model = \"gpt-x\" # pinned\n+include_environment_context = false\n"),
        "{text}"
    );
    assert!(text.ends_with("dry run: nothing written\n"), "{text}");
    assert_eq!(read(&f.user_file()), before);

    let off = harn(&f, &["off", "environment_context"]);
    assert!(off.status.success(), "{off:?}");
    assert!(read(&f.user_file()).contains("include_environment_context = false\n"));
    let history = stdout(&harn(&f, &["history"]));
    assert!(history.contains(ENV), "{history}");

    let undo = harn(&f, &["undo"]);
    assert!(undo.status.success(), "{undo:?}");
    assert_eq!(read(&f.user_file()), before);

    assert!(harn(&f, &["off", "environment_context"]).status.success());
    let on = harn(&f, &["on", "environment_context"]);
    assert!(on.status.success(), "{on:?}");
    assert_eq!(read(&f.user_file()), before);
}

#[test]
fn cli_off_dry_run_writes_a_relative_codex_home_under_the_target_dir() {
    // Codex takes a relative CODEX_HOME from the folder it runs in, the target dir. harn runs
    // from another folder holding a home of the same name: that one is never the file written.
    let f = fx();
    let home = f.repo.join("codex-rel");
    write(&home.join("config.toml"), USER_CONFIG);
    let elsewhere = f.root.join("elsewhere");
    let decoy = elsewhere.join("codex-rel/config.toml");
    write(&decoy, USER_CONFIG);
    let out = Command::new(env!("CARGO_BIN_EXE_harn"))
        .current_dir(&elsewhere)
        .env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env("CODEX_HOME", "codex-rel")
        .env_remove("HARN_CODEX_PROFILE")
        .env_remove("COLUMNS")
        .args(["--json", "--agent", "codex", "--dir"])
        .arg(&f.repo)
        .args(["off", "environment_context", "--dry-run"])
        .output()
        .expect("run harn");
    assert!(out.status.success(), "{out:?}");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let want = home.join("config.toml").display().to_string();
    let ops = v["plan"]["ops"].as_array().expect("ops");
    assert_eq!(ops.len(), 1, "{v:#}");
    assert_eq!(ops[0]["file"], Value::from(want.clone()));
    assert_eq!(ops[0]["knob"]["file"], Value::from(want));
    assert!(v["entry"].is_null(), "{v:#}");
    assert_eq!(read(&home.join("config.toml")), USER_CONFIG);
    assert_eq!(read(&decoy), USER_CONFIG);

    // The warning that Codex rewrites its user config is about the same file.
    let mut target = f.target();
    target
        .env
        .insert("CODEX_HOME".to_string(), "codex-rel".to_string());
    assert!(
        plan::codex_config_warning(&target, &[home.join("config.toml").as_path()], || true)
            .is_some()
    );
}
