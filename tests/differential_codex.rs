//! Differential conformance for Codex (P0.5, acceptance A7–A8): the static Codex tree of a real
//! directory against what the `codex` on `PATH` says about it — `codex -C <dir> debug
//! prompt-input`, `debug models`, `mcp list --json`, `plugin list --json` — through
//! `verify_codex`: 0 `predicted-not-seen`, 0 `seen-not-predicted`, 0 `content-mismatch`.
//!
//! Costs no model request, but `debug prompt-input` starts the configured MCP servers and writes
//! into `CODEX_HOME`, so it runs only when asked. Skipped (it passes, saying why) unless
//! `HARN_DIFFERENTIAL_CODEX=<dir>` is set and `codex` is on `PATH`. The process environment is
//! the run's (`CODEX_HOME`, `HARN_CODEX_PROFILE`, `HOME` included): point them at a scratch home
//! to keep the run off a real one. The raw outputs are kept in a capture dir it prints.
//!
//! `HARN_DIFFERENTIAL_CODEX=<dir> cargo test --test differential_codex -- --nocapture`

use harn::model::Target;
use harn::oracle::codex as oracle;
use harn::resolve::{Codex, Resolver};
use harn::verify::verify_codex;
use std::path::PathBuf;

fn codex_on_path() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join("codex"))
        .find(|p| p.is_file())
}

/// The directory and the codex binary, or why the test is skipped.
fn inputs() -> Result<(PathBuf, PathBuf), String> {
    let dir =
        std::env::var("HARN_DIFFERENTIAL_CODEX").map_err(|_| "HARN_DIFFERENTIAL_CODEX is unset")?;
    let codex = codex_on_path().ok_or("`codex` is not on PATH")?;
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{dir}: {e}"))?;
    Ok((dir, codex))
}

#[test]
fn static_tree_matches_codex_inspection_commands() {
    let (dir, codex) = match inputs() {
        Ok(inputs) => inputs,
        Err(why) => {
            eprintln!("differential_codex: skipped ({why})");
            return;
        }
    };
    let target = Target {
        dir: dir.clone(),
        home: dirs::home_dir().expect("home"),
        env: std::env::vars().collect(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    };
    let mut tree = Codex.resolve(&target, None).expect("resolve");
    let capture = oracle::run_codex(&target, &codex).expect("codex inspection commands");
    let report = verify_codex(&mut tree, &capture);
    println!(
        "differential_codex: {} · codex {} · rules {} · capture {}",
        dir.display(),
        capture.context.agent_version.as_deref().unwrap_or("?"),
        tree.rules_verified_against,
        capture
            .dir
            .as_ref()
            .map_or("-".to_string(), |d| d.display().to_string())
    );
    println!("{}", harn::render::verify_report(&report));
    let failures = report.rows.iter().filter(|(_, v)| !v.passes()).count();
    assert!(
        report.passed,
        "differential_codex: {failures} difference(s), listed above"
    );
}
