//! `harn trim` on a copy of the zbrain-like fixture with a fake agent: a shell script that saves
//! the prompt it was given and prints a canned reply. `HOME` and `XDG_STATE_HOME` point into the
//! copy, so the journal and every write stay there; stdin is not a terminal.

mod common;

use common::{fixture, Fixture};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// The plugin row of the fixture: one local `enabledPlugins` knob; its skills, MCP server and
/// hook leave with it.
const PLUGIN: &str = "alpha@market-one";
const PLUGIN_ID: &str = "claude-code/plugin/plugin/alpha@market-one";
const REASON: &str = "alpha is not used in this project";

fn harn(f: &Fixture, args: &[&str]) -> Output {
    harn_cmd(f, &[], args).output().expect("run harn")
}

fn harn_cmd(f: &Fixture, envs: &[(&str, &Path)], args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harn"));
    cmd.env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env_remove("NO_COLOR")
        // Tool search is decided from these; the caller's shell must not leak in.
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ENABLE_TOOL_SEARCH")
        .env_remove("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS")
        // The Codex tree is read from these; the caller's must not leak in either.
        .env_remove("CODEX_HOME")
        .env_remove("HARN_CODEX_PROFILE")
        .arg("--dir")
        .arg(&f.project)
        .args(args);
    for (name, value) in envs {
        cmd.env(name, value);
    }
    cmd
}

/// `cmd` run to completion within 30 s, else killed and the test fails: a pipe deadlock between
/// harn and its agent shows up as this timeout, not as a hung test run.
fn bounded(mut cmd: Command) -> Output {
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run harn");
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(out) => out.expect("wait for harn"),
        Err(_) => {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            panic!("harn did not finish within 30 s (pipe deadlock?)");
        }
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Where the fake agent saves its prompt; absent = the agent never ran.
fn prompt_file(f: &Fixture) -> PathBuf {
    f.root.join("agent/prompt.txt")
}

/// How many arguments the fake agent got.
fn argc(f: &Fixture) -> String {
    std::fs::read_to_string(f.root.join("agent/argc.txt")).unwrap()
}

/// What the fake agent read on stdin (empty when harn passed the prompt as an argument).
fn agent_stdin(f: &Fixture) -> String {
    std::fs::read_to_string(f.root.join("agent/stdin.txt")).unwrap()
}

/// The fake agent's working directory, as `pwd` printed it.
fn agent_cwd(f: &Fixture) -> PathBuf {
    let text = std::fs::read_to_string(f.root.join("agent/pwd.txt")).unwrap();
    PathBuf::from(text.trim_end())
}

/// A fake agent: saves its argument count to `argc.txt`, its `pwd` to `pwd.txt`, its stdin to
/// `stdin.txt`, and its prompt (stdin when there was any, else the last argument) to
/// [`prompt_file`], then runs `body`.
fn script(f: &Fixture, body: &str) -> PathBuf {
    let dir = f.root.join("agent");
    let text = format!(
        "#!/bin/sh\nprintf '%s' \"$#\" > '{dir}/argc.txt'\npwd > '{dir}/pwd.txt'\n\
         for last; do :; done\ncat > '{dir}/stdin.txt'\n\
         if [ -s '{dir}/stdin.txt' ]; then cp '{dir}/stdin.txt' '{p}'; \
         else printf '%s' \"$last\" > '{p}'; fi\n{body}\n",
        dir = dir.display(),
        p = prompt_file(f).display()
    );
    write_agent(f, &text)
}

/// `text` as the executable fake agent `agent/agent.sh`.
fn write_agent(f: &Fixture, text: &str) -> PathBuf {
    let dir = f.root.join("agent");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("agent.sh");
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// 300 rule files with 200-byte names: every one is a top-level brief row, so the prompt
/// outgrows a pipe buffer (64 KiB) on any temp path. Returns the brief's size.
fn inflate_brief(f: &Fixture) -> usize {
    let rules = f.project.join(".claude/rules");
    for i in 0..300 {
        let name = format!("{i:03}-{}.md", "r".repeat(196));
        std::fs::write(rules.join(name), "# rule\nkeep lines short.\n").unwrap();
    }
    let out = harn(f, &["brief"]);
    assert!(out.status.success(), "{out:?}");
    out.stdout.len()
}

/// A fake agent that prints `reply` verbatim.
fn agent(f: &Fixture, reply: &str) -> String {
    let path = script(f, &format!("cat <<'HARN_REPLY'\n{reply}\nHARN_REPLY"));
    path.to_str().unwrap().to_string()
}

/// `claude -p --output-format json` shape: the answer, prose and a fenced plan, in `result`.
fn claude_reply(plan: &Value) -> String {
    json!({
        "type": "result",
        "is_error": false,
        "result": format!("here is the plan ```json {plan} ```"),
    })
    .to_string()
}

fn plan_for(f: &Fixture, op: Value) -> Value {
    json!({
        "schema": "harn-plan/1",
        "target_dir": f.project.display().to_string(),
        "ops": [op],
    })
}

fn turn_off_plugin(f: &Fixture) -> Value {
    plan_for(f, json!({ "selector": PLUGIN, "reason": REASON }))
}

fn settings(f: &Fixture) -> Vec<Vec<u8>> {
    [
        f.home.join(".claude/settings.json"),
        f.project.join(".claude/settings.json"),
        f.project.join(".claude/settings.local.json"),
    ]
    .iter()
    .map(|path| std::fs::read(path).unwrap())
    .collect()
}

fn history(f: &Fixture) -> Value {
    let out = harn(f, &["--json", "history"]);
    assert!(out.status.success(), "{out:?}");
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn trim_dry_run_prints_the_agents_plan_and_writes_nothing() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));
    let before = settings(&f);

    let out = harn(&f, &["trim", "--dry-run", "--agent-cmd", &cmd]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        format!("asking {cmd} for a plan (one model request)…"),
        "{text}"
    );
    assert_eq!(
        lines[1],
        format!("plan for {} · claude-code", f.project.display()),
        "{text}"
    );
    for expected in [
        format!("  [1] {PLUGIN_ID}"),
        format!("      reason  {REASON}"),
        "side effects".to_string(),
        "  alpha:alpha-skill (skill) leaves".to_string(),
        "+    \"alpha@market-one\": false".to_string(),
    ] {
        assert!(
            lines.contains(&expected.as_str()),
            "{expected:?} in:\n{text}"
        );
    }
    assert_eq!(lines.last(), Some(&"dry run: nothing written"), "{text}");

    assert_eq!(settings(&f), before);
    assert_eq!(history(&f), json!([]));
    // The prompt was the one argument: the brief, then the instructions naming the target.
    assert_eq!(argc(&f), "1");
    assert_eq!(agent_stdin(&f), "");
    let prompt = std::fs::read_to_string(prompt_file(&f)).unwrap();
    assert!(
        prompt.starts_with("harn brief · schema harn/1 · "),
        "{prompt}"
    );
    assert!(prompt.contains(PLUGIN_ID), "{prompt}");
    assert!(
        prompt.contains(&format!("\"target_dir\": \"{}\"", f.project.display())),
        "{prompt}"
    );
    assert!(prompt.contains("EXACTLY ONE JSON object"), "{prompt}");
}

#[test]
fn trim_yes_applies_journals_the_reason_and_undo_restores_the_bytes() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));
    let local = f.project.join(".claude/settings.local.json");
    let before = std::fs::read(&local).unwrap();

    let out = harn(&f, &["trim", "--yes", "--agent-cmd", &cmd]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    let applied = lines[lines.len() - 2];
    assert!(
        applied.starts_with("applied: journal ")
            && applied.ends_with(" · takes effect on the next session"),
        "{text}"
    );
    assert_eq!(lines.last(), Some(&"undo: harn undo"), "{text}");
    let after = std::fs::read_to_string(&local).unwrap();
    assert!(after.contains("\"alpha@market-one\": false"), "{after}");

    let entries = history(&f);
    assert_eq!(entries.as_array().unwrap().len(), 1, "{entries}");
    assert_eq!(entries[0]["ops"][0]["item_id"], PLUGIN_ID);
    assert_eq!(entries[0]["ops"][0]["reason"], REASON);
    assert!(
        applied.contains(entries[0]["id"].as_str().unwrap()),
        "{text}"
    );

    let undo = harn(&f, &["undo"]);
    assert!(undo.status.success(), "{undo:?}");
    assert_eq!(std::fs::read(&local).unwrap(), before);
}

#[test]
fn trim_refuses_an_op_with_an_unknown_knob_naming_it() {
    let f = fixture();
    let plan = plan_for(
        &f,
        json!({ "selector": PLUGIN, "knob": 9, "reason": REASON }),
    );
    let cmd = agent(&f, &claude_reply(&plan));
    let before = settings(&f);

    let out = harn(&f, &["trim", "--yes", "--agent-cmd", &cmd]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let err = stderr(&out);
    assert!(err.contains("op 1: knob 9 is out of range"), "{err}");
    assert_eq!(settings(&f), before);
    assert_eq!(history(&f), json!([]));
}

#[test]
fn trim_fails_on_a_reply_without_a_plan_quoting_its_first_lines() {
    let f = fixture();
    let reply = json!({ "result": "I would turn off alpha.\nIt looks unused.\nThird.\nFourth." });
    let cmd = agent(&f, &reply.to_string());

    let out = harn(&f, &["trim", "--yes", "--agent-cmd", &cmd]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = stderr(&out);
    assert!(
        err.contains("no harn-plan/1 object in the reply of"),
        "{err}"
    );
    assert!(
        err.contains("  I would turn off alpha.\n  It looks unused.\n  Third."),
        "{err}"
    );
    assert!(!err.contains("Fourth."), "{err}");
}

#[test]
fn trim_points_to_the_skill_when_the_agent_is_missing() {
    let f = fixture();
    let out = harn(
        &f,
        &["trim", "--dry-run", "--agent-cmd", "/nonexistent/agent"],
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = stderr(&out);
    assert!(
        err.contains(
            "agent command not found: /nonexistent/agent — run /harn-trim inside Claude Code instead"
        ),
        "{err}"
    );
}

#[test]
fn trim_reports_a_failing_agent_with_the_first_lines_of_its_stderr() {
    let f = fixture();
    let body = "for n in 1 2 3 4 5 6; do echo \"boom $n\" >&2; done\nexit 2";
    let cmd = script(&f, body);

    let out = harn(
        &f,
        &["trim", "--dry-run", "--agent-cmd", cmd.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = stderr(&out);
    assert!(err.contains("failed (exit status: 2)"), "{err}");
    assert!(
        err.contains("  boom 1\n") && err.contains("  boom 5"),
        "{err}"
    );
    assert!(!err.contains("boom 6"), "{err}");
    // The agent's temp dir is removed on the failure path too.
    let cwd = agent_cwd(&f);
    assert!(!cwd.exists(), "{} left behind", cwd.display());
}

#[test]
fn readonly_refuses_trim_including_a_dry_run() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));
    for args in [
        &["--readonly", "trim", "--yes", "--agent-cmd", &cmd][..],
        &["--readonly", "trim", "--dry-run", "--agent-cmd", &cmd],
    ] {
        let out = harn(&f, args);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {out:?}");
        assert!(
            stderr(&out).contains("--readonly: `harn trim`"),
            "{args:?}: {out:?}"
        );
    }
    assert!(!prompt_file(&f).exists(), "the agent ran");
}

#[test]
fn trim_off_a_terminal_without_yes_refuses_before_asking_the_agent() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));

    let out = harn(&f, &["trim", "--agent-cmd", &cmd]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        stderr(&out).contains("not a terminal: pass --yes to apply"),
        "{out:?}"
    );
    assert!(!prompt_file(&f).exists(), "the agent ran");
}

#[test]
fn trim_strips_a_trailing_dash_and_pipes_the_prompt_on_stdin() {
    let f = fixture();
    // A bare plan on stdout (no Claude json wrapper) is the reply itself.
    let path = agent(&f, &turn_off_plugin(&f).to_string());
    let cmd = format!("{path} exec -");

    let out = harn(&f, &["trim", "--dry-run", "--agent-cmd", &cmd]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(
        text.starts_with(&format!("asking {path} exec - for a plan")),
        "{text}"
    );
    assert!(text.contains(&format!("  [1] {PLUGIN_ID}")), "{text}");
    // `exec` only: the `-` is harn's marker, and the prompt went to stdin, not into argv.
    assert_eq!(argc(&f), "1");
    let prompt = agent_stdin(&f);
    assert!(
        prompt.starts_with("harn brief · schema harn/1 · "),
        "{prompt}"
    );
    assert!(prompt.ends_with("no prose, no code fence."), "{prompt}");
}

#[test]
fn trim_replaces_a_foreign_target_dir_with_a_note() {
    let f = fixture();
    let mut plan = turn_off_plugin(&f);
    plan["target_dir"] = json!("/somewhere/else");
    let cmd = agent(&f, &claude_reply(&plan));

    let out = harn(&f, &["trim", "--dry-run", "--agent-cmd", &cmd]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(
        text.contains(&format!(
            "note: the plan's target_dir \"/somewhere/else\" is not the target; using {}",
            f.project.display()
        )),
        "{text}"
    );
    assert!(text.contains(&format!("  [1] {PLUGIN_ID}")), "{text}");
}

#[test]
fn trim_takes_a_positional_dir() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));
    // No --dir: the subcommand's own DIR names the target.
    let out = Command::new(env!("CARGO_BIN_EXE_harn"))
        .env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ENABLE_TOOL_SEARCH")
        .env_remove("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS")
        // The Codex tree is read from these; the caller's must not leak in either.
        .env_remove("CODEX_HOME")
        .env_remove("HARN_CODEX_PROFILE")
        .current_dir(Path::new("/"))
        .args([
            "trim",
            f.project.to_str().unwrap(),
            "--dry-run",
            "--agent-cmd",
            &cmd,
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(
        stdout(&out).contains(&format!("plan for {} · claude-code", f.project.display())),
        "{out:?}"
    );
}

#[test]
fn trim_runs_the_agent_in_a_fresh_temp_dir_removed_afterwards() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));

    let out = harn(&f, &["trim", "--dry-run", "--agent-cmd", &cmd]);
    assert!(out.status.success(), "{out:?}");
    let cwd = agent_cwd(&f);
    let name = cwd.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("harn-trim-"), "{}", cwd.display());
    assert!(!cwd.starts_with(&f.project), "{}", cwd.display());
    assert!(!cwd.starts_with(&f.root), "{}", cwd.display());
    let test_cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    assert_ne!(cwd, test_cwd);
    assert!(!cwd.exists(), "{} left behind", cwd.display());
}

#[test]
fn trim_with_an_empty_plan_reports_nothing_to_turn_off() {
    let f = fixture();
    let plan = json!({
        "schema": "harn-plan/1",
        "target_dir": f.project.display().to_string(),
        "ops": [],
    });
    let cmd = agent(&f, &claude_reply(&plan));
    let before = settings(&f);

    let out = harn(&f, &["trim", "--yes", "--agent-cmd", &cmd]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert_eq!(
        text.lines().last(),
        Some("the agent found nothing to turn off"),
        "{text}"
    );
    assert!(!text.contains("plan for "), "{text}");
    assert_eq!(settings(&f), before);
    assert_eq!(history(&f), json!([]));
}

#[test]
fn trim_keeps_the_agent_out_of_the_target_when_tmpdir_is_inside_it() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));
    let tmpdir = f.project.join("tmp");
    std::fs::create_dir(&tmpdir).unwrap();

    let out = harn_cmd(
        &f,
        &[("TMPDIR", &tmpdir)],
        &["trim", "--dry-run", "--agent-cmd", &cmd],
    )
    .output()
    .unwrap();
    assert!(out.status.success(), "{out:?}");
    let cwd = agent_cwd(&f);
    assert!(!cwd.starts_with(&f.project), "{}", cwd.display());
    assert!(
        cwd.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("harn-trim-"),
        "{}",
        cwd.display()
    );
    assert!(!cwd.exists(), "{} left behind", cwd.display());
    assert_eq!(std::fs::read_dir(&tmpdir).unwrap().count(), 0);
}

#[test]
fn trim_keeps_the_agent_out_of_the_enclosing_repo_when_tmpdir_is_a_sibling() {
    let f = fixture();
    let cmd = agent(&f, &claude_reply(&turn_off_plugin(&f)));
    // The project is a plain subdirectory of a repo rooted at the fixture copy, and TMPDIR is
    // the project's sibling inside that repo.
    std::fs::remove_dir(f.project.join(".git")).unwrap();
    std::fs::create_dir(f.root.join(".git")).unwrap();
    let tmpdir = f.root.join("tmp");
    std::fs::create_dir(&tmpdir).unwrap();

    let out = harn_cmd(
        &f,
        &[("TMPDIR", &tmpdir)],
        &["trim", "--dry-run", "--agent-cmd", &cmd],
    )
    .output()
    .unwrap();
    assert!(out.status.success(), "{out:?}");
    let cwd = agent_cwd(&f);
    assert!(!cwd.starts_with(&f.root), "{}", cwd.display());
    assert!(
        cwd.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("harn-trim-"),
        "{}",
        cwd.display()
    );
    assert!(!cwd.exists(), "{} left behind", cwd.display());
    assert_eq!(std::fs::read_dir(&tmpdir).unwrap().count(), 0);
}

#[test]
fn trim_decodes_an_escaped_schema_and_an_empty_plan_is_nothing() {
    let f = fixture();
    // Raw stdout, the slash escaped: the text never contains `harn-plan/1` literally.
    let reply = format!(
        r#"{{"schema":"harn-plan\/1","target_dir":"{}","ops":[]}}"#,
        f.project.display()
    );
    let cmd = agent(&f, &reply);
    let before = settings(&f);

    let out = harn(&f, &["trim", "--yes", "--agent-cmd", &cmd]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert_eq!(
        text.lines().last(),
        Some("the agent found nothing to turn off"),
        "{text}"
    );
    assert_eq!(settings(&f), before);
}

#[test]
fn trim_survives_an_agent_that_writes_a_large_reply_before_reading_stdin() {
    let f = fixture();
    assert!(inflate_brief(&f) > 64 * 1024);
    let dir = f.root.join("agent");
    // 200,000 bytes of prose (> 128 KiB) and the plan on stdout before stdin is read: harn must
    // drain stdout while it is still writing the prompt.
    let text = format!(
        "#!/bin/sh\nyes x | head -c 200000\nprintf '\\n'\ncat <<'HARN_REPLY'\n{}\nHARN_REPLY\n\
         cat > '{}/stdin.txt'\n",
        turn_off_plugin(&f),
        dir.display()
    );
    let path = write_agent(&f, &text);

    let out = bounded(harn_cmd(
        &f,
        &[],
        &[
            "trim",
            "--dry-run",
            "--agent-cmd",
            &format!("{} -", path.display()),
        ],
    ));
    assert!(out.status.success(), "{out:?}");
    assert!(
        stdout(&out).contains(&format!("  [1] {PLUGIN_ID}")),
        "{out:?}"
    );
    let prompt = std::fs::read_to_string(dir.join("stdin.txt")).unwrap();
    assert!(prompt.len() > 64 * 1024, "{}", prompt.len());
    assert!(prompt.ends_with("no prose, no code fence."));
}

#[test]
fn trim_survives_an_agent_that_exits_without_reading_stdin() {
    let f = fixture();
    assert!(inflate_brief(&f) > 64 * 1024);
    // The prompt fills the pipe and the agent never reads it: harn's write fails with a broken
    // pipe once the agent exits, and the reply still counts.
    let text = format!(
        "#!/bin/sh\ncat <<'HARN_REPLY'\n{}\nHARN_REPLY\nexit 0\n",
        turn_off_plugin(&f)
    );
    let path = write_agent(&f, &text);

    let out = bounded(harn_cmd(
        &f,
        &[],
        &[
            "trim",
            "--dry-run",
            "--agent-cmd",
            &format!("{} -", path.display()),
        ],
    ));
    assert!(out.status.success(), "{out:?}");
    assert!(
        stdout(&out).contains(&format!("  [1] {PLUGIN_ID}")),
        "{out:?}"
    );
}
