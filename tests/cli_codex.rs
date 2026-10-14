//! The `harn` binary with a Codex home (P0.5 C6): both agents by default, Codex wording in
//! `brief`, `report`, `why` and `off --dry-run`, `apply` finding the plan's agent, `trim` refusing
//! Codex, and the version line. `HOME` and `XDG_STATE_HOME` point into a temp dir built per test
//! (`home/.codex` = the Codex home, `repo/` = a git checkout with its own `AGENTS.md`).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fx {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
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

/// A Codex home that trusts the checkout, with a model in `models_cache.json`.
fn fx() -> Fx {
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "harn-cli-codex-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let home = root.join("home");
    let repo = root.join("repo");
    write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&repo.join("AGENTS.md"), "# Rules\n\nKeep changes small.\n");
    let codex = home.join(".codex");
    let cache = serde_json::json!({
        "client_version": "0.160.1",
        "models": [{
            "slug": "gpt-test",
            "context_window": 200000,
            "model_messages": {"instructions_template": "You are a coding agent."}
        }]
    });
    write(&codex.join("models_cache.json"), &cache.to_string());
    write(
        &codex.join("config.toml"),
        &format!(
            "model = \"gpt-test\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            repo.display()
        ),
    );
    Fx { root, home, repo }
}

fn harn(f: &Fx, args: &[&str]) -> Output {
    harn_with(f, &[], args)
}

fn harn_with(f: &Fx, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harn"));
    cmd.env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env_remove("CODEX_HOME")
        .env_remove("HARN_CODEX_PROFILE")
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ENABLE_TOOL_SEARCH")
        .env_remove("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS")
        .env_remove("COLUMNS")
        .env_remove("NO_COLOR")
        .arg("--dir")
        .arg(&f.repo)
        .args(args);
    // After the removals: a test's own value wins.
    for (name, value) in envs {
        cmd.env(name, value);
    }
    cmd.output().expect("run harn")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn version_names_the_rules_of_both_agents() {
    let out = Command::new(env!("CARGO_BIN_EXE_harn"))
        .arg("--version")
        .output()
        .unwrap();
    let text = stdout(&out);
    assert!(
        text.trim_end()
            .ends_with(") rules: claude-code 2.1.296, codex 0.160.1"),
        "{text}"
    );
}

#[test]
fn the_listing_has_one_table_per_agent_claude_code_first() {
    let f = fx();
    // Wide enough that the default action column is not cut.
    let text = stdout(&harn_with(&f, &[("COLUMNS", "200")], &["--table"]));
    let claude = text.find("   claude-code ").expect("a Claude Code table");
    let codex = text.find("   codex ").expect("a Codex table");
    assert!(claude < codex, "{text}");
    // The Codex table's switches are TOML keys.
    assert!(
        text.contains("include_environment_context = false (project)"),
        "{text}"
    );
}

#[test]
fn brief_on_a_codex_tree_uses_codex_words_and_scopes() {
    let f = fx();
    let out = harn(&f, &["--agent", "codex", "brief"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(text.contains("== codex "), "{text}");
    assert!(
        text.contains("codex rows: agents-md-message and skills_instructions are wrappers"),
        "{text}"
    );
    // knob0 keeps its scope on a Codex row (it is project or user, never local).
    assert!(
        text.contains("|include_environment_context = false (project)"),
        "{text}"
    );
    assert!(text.contains("--agent <agent>"), "{text}");
    assert!(!text.contains("(optional, default local)"), "{text}");
    // The default the plan code uses (`plan::default_scope`): knob 0's own key, never "the
    // narrowest knob listed" (a hook's features.hooks in the project file would win that).
    assert!(
        text.contains("codex the narrowest scope listed for the key of the item's knob 0"),
        "{text}"
    );
    assert!(!text.contains("narrowest knob listed"), "{text}");
    assert!(
        text.contains("codex: the item's own key — knob 0's —"),
        "{text}"
    );
    // The example op is one apply takes: the scope its knob is in.
    assert!(text.contains("\"scope\": \"project\""), "{text}");
    assert!(!text.contains("\"scope\": \"local\""), "{text}");
    assert!(!text.contains(".claude"), "{text}");
}

#[test]
fn report_has_a_section_per_agent() {
    let f = fx();
    let html = f.root.join("out.html");
    let out = harn(&f, &["report", "--html", html.to_str().unwrap()]);
    assert!(out.status.success(), "{out:?}");
    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.contains("claude-code"), "no Claude Code section");
    assert!(
        page.contains("environment context (cwd, shell, date)"),
        "no Codex rows"
    );
    assert!(page.contains("include_environment_context = false (project)"));
}

#[test]
fn why_and_off_dry_run_find_a_codex_item_without_the_agent_flag() {
    let f = fx();
    let why = harn(&f, &["why", "environment_context"]);
    assert!(why.status.success(), "{why:?}");
    let text = stdout(&why);
    assert!(
        text.contains(".codex/config.toml  include_environment_context = false  (project)"),
        "{text}"
    );
    let dry = harn(&f, &["off", "environment_context", "--dry-run"]);
    assert!(dry.status.success(), "{dry:?}");
    let text = stdout(&dry);
    assert!(
        text.contains("+include_environment_context = false"),
        "{text}"
    );
    assert!(
        text.contains("knob    .codex/config.toml  include_environment_context = false  (project)"),
        "{text}"
    );
    assert!(
        !f.repo.join(".codex/config.toml").exists(),
        "dry run wrote nothing"
    );
}

/// The planner, not its wording: a hook's own switch (its `[hooks.state]` entry, user config
/// only) is knob 0; the switch for every hook (`features.hooks`) is listed after it, also in this
/// trusted project's config — a narrower file. `off` without a scope writes knob 0's key in the
/// narrowest file listed for that key, never the broader switch.
#[test]
fn off_without_a_scope_takes_the_hooks_own_switch_not_the_project_switch_for_every_hook() {
    let f = fx();
    let codex = f.home.join(".codex");
    write(
        &codex.join("hooks.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo hello"}]}]}}"#,
    );
    let doc: serde_json::Value =
        serde_json::from_str(&stdout(&harn(&f, &["--agent", "codex", "--json"]))).unwrap();
    let hook = doc["agents"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "hook")
        .expect("the hook row")
        .clone();
    let knobs = hook["knobs"].as_array().unwrap();
    let id = hook["id"].as_str().unwrap();
    // The fixture has what the test needs: knob 0 is the hook's own user switch, and a project
    // switch for every hook is listed after it.
    assert_eq!(knobs[0]["scope"], "user", "{knobs:?}");
    assert!(
        knobs[0]["pointer"]
            .as_str()
            .unwrap()
            .starts_with("/hooks/state/"),
        "{knobs:?}"
    );
    assert!(
        knobs[1..]
            .iter()
            .any(|k| k["pointer"] == "/features/hooks" && k["scope"] == "project"),
        "{knobs:?}"
    );

    let out = harn(&f, &["--json", "off", id, "--dry-run"]);
    assert!(out.status.success(), "{out:?}");
    let plan: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let ops = plan["plan"]["ops"].as_array().unwrap();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(ops[0]["knob"], knobs[0], "knob 0, unchanged");
    assert_eq!(
        ops[0]["file"].as_str(),
        Some(codex.join("config.toml").to_str().unwrap())
    );
    let diff = plan["plan"]["diff"].as_str().unwrap();
    assert!(!diff.contains("features"), "{diff}");
    assert!(
        !f.repo.join(".codex/config.toml").exists(),
        "dry run wrote nothing"
    );
}

#[test]
fn apply_without_the_agent_flag_takes_the_agent_whose_tree_has_the_selectors() {
    let f = fx();
    let plan = f.root.join("plan.json");
    std::fs::write(
        &plan,
        serde_json::json!({
            "schema": "harn-plan/1",
            "target_dir": f.repo,
            "ops": [{"selector": "codex/env-block/builtin/environment_context", "reason": "not needed"}]
        })
        .to_string(),
    )
    .unwrap();
    let out = harn(&f, &["apply", plan.to_str().unwrap(), "--dry-run"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(text.contains(" · codex"), "{text}");
    assert!(text.contains("reason  not needed"), "{text}");
}

#[test]
fn trim_refuses_codex_and_says_what_works() {
    let f = fx();
    // stdin is not a terminal here: the Codex refusal comes before the terminal check, with or
    // without --yes or --dry-run.
    for flags in [&["--dry-run"][..], &["--yes"], &[]] {
        let mut args = vec!["--agent", "codex", "trim"];
        args.extend_from_slice(flags);
        let out = harn(&f, &args);
        assert_eq!(out.status.code(), Some(3), "{flags:?}: {out:?}");
        let err = stderr(&out);
        assert!(
            err.contains("trim for codex is not implemented yet"),
            "{flags:?}: {err}"
        );
        assert!(!err.contains("not a terminal"), "{flags:?}: {err}");
    }
}
