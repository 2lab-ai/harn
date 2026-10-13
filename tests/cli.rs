//! The `harn` binary on a copy of the zbrain-like fixture: what a command prints, in order.
//! `HOME` and `XDG_STATE_HOME` point into the copy, so the journal and every write stay there.

mod common;

use common::{fixture, Fixture};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn harn(f: &Fixture, envs: &[(&str, &Path)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_harn"));
    cmd.env("HOME", &f.home)
        .env("XDG_STATE_HOME", f.root.join("state"))
        .env_remove("NO_COLOR")
        // Tool search is decided from these; the caller's shell must not leak in.
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ENABLE_TOOL_SEARCH")
        .env_remove("CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS")
        // The table width comes from COLUMNS off a terminal; the caller's must not leak in.
        .env_remove("COLUMNS")
        .arg("--dir")
        .arg(&f.project)
        .args(args);
    for (name, value) in envs {
        cmd.env(name, value);
    }
    cmd.output().expect("run harn")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn edit_prints_the_diff_then_the_journal_line_and_records_the_target() {
    let f = fixture();
    let editor = f.root.join("append.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf 'added by the editor\\n' >> \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = harn(&f, &[("EDITOR", &editor)], &["edit", "rules/a.md"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    let added = lines
        .iter()
        .position(|l| *l == "+added by the editor")
        .unwrap_or_else(|| panic!("no diff line in:\n{text}"));
    assert!(lines[0].starts_with("--- "), "{text}");
    // The journal line comes after the diff and names the target, not the test's cwd.
    let journal_line = lines.last().unwrap();
    assert!(lines.len() > added + 1, "{text}");
    assert!(
        journal_line.contains(&f.project.display().to_string()) && journal_line.contains("1 op"),
        "{text}"
    );

    let history = harn(&f, &[], &["--json", "history"]);
    let entries: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(
        entries[0]["context"]["cwd"],
        serde_json::json!(f.project.display().to_string())
    );
}

#[test]
fn off_dry_run_on_a_hook_names_every_hook_and_the_status_line_that_leave() {
    let f = fixture();
    let out = harn(
        &f,
        &[],
        &[
            "off",
            "PreToolUse hook [Bash] · 1 cmd · ~/.claude/settings.json",
            "--agent",
            "claude-code",
            "--dry-run",
            "--no-unicode",
            "--color",
            "never",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    // `side effects: none` when nothing leaves; else a `side effects` heading and one line each.
    assert!(!text.contains("side effects: none"), "{text}");
    let effects: Vec<&str> = text
        .lines()
        .skip_while(|l| *l != "side effects")
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .collect();
    assert_eq!(effects.len(), 3, "{text}");
    for (line, name) in
        effects
            .iter()
            .zip(["SessionStart hook", "UserPromptSubmit hook", "statusLine"])
    {
        assert!(line.trim_start().starts_with(name), "{name}:\n{text}");
        assert!(line.ends_with(") leaves"), "{line}");
    }
    // Two of them print into the request, unmeasured.
    assert!(text.contains("freed D?"), "{text}");
    assert!(text.contains("dry run: nothing written"), "{text}");
}

#[test]
fn why_prints_a_string_setting_as_its_text() {
    let f = fixture();
    let dir = f.home.join("memory (old)");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MEMORY.md"), "- a note\n").unwrap();
    let local = f.project.join(".claude/settings.local.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&local).unwrap()).unwrap();
    json["autoMemoryDirectory"] = serde_json::json!("~/memory (old)");
    std::fs::write(&local, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let out = harn(
        &f,
        &[],
        &[
            "why",
            "MEMORY.md",
            "--agent",
            "claude-code",
            "--no-unicode",
            "--color",
            "never",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    let step = text
        .lines()
        .find(|l| l.contains("#/autoMemoryDirectory = "))
        .unwrap_or_else(|| panic!("no autoMemoryDirectory step:\n{text}"));
    assert!(
        step.trim_end()
            .ends_with(&format!("#/autoMemoryDirectory = {}", dir.display())),
        "{step}"
    );
}

#[test]
fn edit_journals_the_agent_version_of_the_picked_tree() {
    let f = fixture();
    // An installed version in the copy's HOME only: the resolver reports the newest
    // `~/.local/share/claude/versions/*` entry, and `edit` journals it.
    std::fs::create_dir_all(f.home.join(".local/share/claude/versions/9.9.9")).unwrap();
    let editor = f.root.join("append.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf 'added by the editor\\n' >> \"$1\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = harn(&f, &[("EDITOR", &editor)], &["edit", "rules/a.md"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    let journal_line = text.lines().last().unwrap();
    assert!(journal_line.contains("claude-code 9.9.9"), "{text}");

    let history = harn(&f, &[], &["--json", "history"]);
    let entries: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(
        entries[0]["context"]["agent_version"],
        serde_json::json!("9.9.9")
    );
    let history = stdout(&harn(&f, &[], &["history"]));
    assert!(history.contains("claude-code 9.9.9"), "{history}");
}

/// `tests/fixtures/capture-synthetic` rebound to the fixture copy: its `/synthetic/project` and
/// `/synthetic/home` paths rewritten to the copy's, so `verify --capture` accepts it there.
fn capture_for(f: &Fixture) -> std::path::PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capture-synthetic");
    let dir = f.root.join("capture");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["index.jsonl", "stream.jsonl", "synthetic.request.json"] {
        let text = std::fs::read_to_string(src.join(name)).unwrap();
        let text = text
            .replace("/synthetic/project", &f.project.display().to_string())
            .replace("/synthetic/home", &f.home.display().to_string());
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

#[test]
fn verify_prints_the_report_then_the_observed_header_or_the_whole_table() {
    let f = fixture();
    let capture = capture_for(&f);
    let capture = capture.to_str().unwrap();

    // `auto` leaves tool search undecidable statically: the capture's state fills it.
    let auto = [("ENABLE_TOOL_SEARCH", Path::new("auto"))];
    let out = harn(&f, &auto, &["verify", "--capture", capture]);
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().collect();
    let verdict = lines
        .iter()
        .position(|l| l.starts_with("passed: "))
        .unwrap_or_else(|| panic!("no report in:\n{text}"));
    let title = lines
        .iter()
        .position(|l| l.starts_with(&format!("harn {}", f.project.display())))
        .unwrap_or_else(|| panic!("no header in:\n{text}"));
    assert!(verdict < title, "{text}");
    let totals = lines[title..]
        .iter()
        .find(|l| l.starts_with("first request: observed "))
        .unwrap_or_else(|| panic!("no observed totals in:\n{text}"));
    assert!(
        totals.contains(" = attributed ")
            && totals.contains(" + parameters ")
            && totals.contains(" + framing ")
            && totals.contains("(bound capture)"),
        "{totals}"
    );
    assert!(
        text.contains("tool search: OFF (observed in the bound capture)"),
        "{text}"
    );
    // Header only: no rows.
    assert!(!text.contains("freed(pred)"), "{text}");

    let table = stdout(&harn(&f, &[], &["verify", "--capture", capture, "--table"]));
    assert!(table.contains("first request: observed "), "{table}");
    let columns = table
        .lines()
        .find(|l| l.contains("freed(pred)"))
        .unwrap_or_else(|| panic!("no table in:\n{table}"));
    assert!(columns.contains("observed"), "{columns}");
    assert!(table.contains("sort: freed"), "{table}");
}

#[test]
fn table_flag_prints_the_whole_table_and_matches_the_piped_listing() {
    let f = fixture();
    // Wide enough that the column header is not cut (at 120 this fixture's cuts it).
    let wide = [("COLUMNS", Path::new("200"))];
    let table = harn(&f, &wide, &["--table"]);
    assert!(table.status.success(), "{table:?}");
    let text = stdout(&table);
    let columns = text
        .lines()
        .find(|l| l.contains("freed(pred)"))
        .unwrap_or_else(|| panic!("no column header in:\n{text}"));
    assert!(columns.contains("default action"), "{columns}");
    assert!(text.contains("sort: freed"), "{text}");
    // stdout is a pipe here: the bare listing is the same table, without the terminal tip.
    assert_eq!(stdout(&harn(&f, &wide, &[])), text);
    assert_eq!(
        stdout(&harn(&f, &[], &[])),
        stdout(&harn(&f, &[], &["--table"]))
    );
    assert!(!text.contains("tip:"), "{text}");
}

#[test]
fn table_flag_is_for_the_listing_only() {
    let f = fixture();
    let out = harn(&f, &[], &["--table", "history"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--table"),
        "{out:?}"
    );
}

/// Rows of the table (between the column header and the footnotes), as printed.
fn table_rows(text: &str) -> Vec<&str> {
    text.lines()
        .skip_while(|l| !l.contains("freed(pred)"))
        .skip(1)
        .filter(|l| !l.contains("more (harn why") && !l.trim_start().starts_with('['))
        .collect()
}

#[test]
fn columns_zero_is_unset_and_a_positive_columns_cuts_rows() {
    let f = fixture();
    let unset = stdout(&harn(&f, &[], &["--table"]));
    // A shell that exports COLUMNS=0 must not cut every row down to the ellipsis.
    let zero = stdout(&harn(&f, &[("COLUMNS", Path::new("0"))], &["--table"]));
    assert_eq!(zero, unset);
    assert!(
        table_rows(&zero).iter().all(|row| row.trim() != "…"),
        "{zero}"
    );
    let garbage = stdout(&harn(&f, &[("COLUMNS", Path::new("wide"))], &["--table"]));
    assert_eq!(garbage, unset);

    let narrow = stdout(&harn(&f, &[("COLUMNS", Path::new("80"))], &["--table"]));
    let rows = table_rows(&narrow);
    assert!(!rows.is_empty(), "{narrow}");
    for row in rows {
        assert!(row.chars().count() <= 80, "{row}");
    }
}
