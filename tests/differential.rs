//! Differential conformance (WU-F, acceptance A1): the static tree of a real directory against the
//! `system/init` event of a session Claude Code started there. Every name the event lists under
//! `skills`, `agents`, `plugins` and `mcp_servers` must be a row of that kind exactly once, and no
//! row of those kinds may be missing from it; an MCP server's `source` must be its row's layer.
//!
//! Costs no model request: the event comes from an existing capture. Skipped (it passes, saying
//! why) unless both are set:
//!
//! - `HARN_DIFFERENTIAL=<dir>` — the directory to resolve (`1` = `~/2lab.ai/zbrain`);
//! - `HARN_DIFFERENTIAL_INIT=<stream.jsonl>` — the stream-json output of a session started in
//!   that directory (e.g. the `stream.jsonl` `harn verify --run` binds), read and never copied.
//!
//! `HARN_DIFFERENTIAL=1 HARN_DIFFERENTIAL_INIT=… cargo test --test differential -- --nocapture`

use harn::model::{walk, Kind, Target, Tree};
use harn::oracle::{self, InitEvent};
use harn::resolve::{ClaudeCode, Resolver};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// The directory and the init event, or why the test is skipped.
fn inputs() -> Result<(PathBuf, InitEvent), String> {
    let dir = std::env::var("HARN_DIFFERENTIAL").map_err(|_| "HARN_DIFFERENTIAL is unset")?;
    let stream_path =
        std::env::var("HARN_DIFFERENTIAL_INIT").map_err(|_| "HARN_DIFFERENTIAL_INIT is unset")?;
    let home = dirs::home_dir().ok_or("no home directory")?;
    let dir = match dir.as_str() {
        "1" => home.join("2lab.ai/zbrain"),
        other => PathBuf::from(other),
    };
    let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stream =
        std::fs::read_to_string(&stream_path).map_err(|e| format!("{stream_path}: {e}"))?;
    let init = oracle::find_init_event(&stream).map_err(|e| format!("{stream_path}: {e:#}"))?;
    Ok((dir, init))
}

/// Row names by the `init` list they correspond to: `skills` = every skill row (listed or not —
/// the event names `disable-model-invocation` skills too), `agents` = subagents, `plugins` =
/// plugin rows (`name@marketplace`), `mcp_servers` = MCP server rows (the event lists disabled
/// servers with their status). Names keep duplicates so "exactly once" can be checked.
fn static_sets(tree: &Tree) -> BTreeMap<&'static str, Vec<String>> {
    let mut sets: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for item in walk(tree) {
        let list = match item.kind {
            Kind::Skill => "skills",
            Kind::SubAgent => "agents",
            Kind::Plugin => "plugins",
            Kind::McpServer => "mcp_servers",
            _ => continue,
        };
        let name = item.id.splitn(4, '/').nth(3).unwrap_or_default();
        sets.entry(list).or_default().push(name.to_string());
    }
    sets
}

fn init_sets(init: &InitEvent) -> BTreeMap<&'static str, Vec<String>> {
    BTreeMap::from([
        ("skills", init.skills.clone()),
        ("agents", init.agents.clone()),
        (
            "plugins",
            init.plugins
                .iter()
                .map(|p| p.source.clone().unwrap_or_else(|| p.name.clone()))
                .collect(),
        ),
        (
            "mcp_servers",
            init.mcp_servers.iter().map(|m| m.name.clone()).collect(),
        ),
    ])
}

#[test]
fn static_tree_lists_exactly_the_init_event_names() {
    let (dir, init) = match inputs() {
        Ok(inputs) => inputs,
        Err(why) => {
            eprintln!("differential: skipped ({why})");
            return;
        }
    };
    if let Some(cwd) = &init.cwd {
        assert_eq!(cwd, &dir, "the init event is from another directory");
    }
    let target = Target {
        dir: dir.clone(),
        home: dirs::home_dir().expect("home"),
        env: std::env::vars().collect(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    };
    let tree = ClaudeCode.resolve(&target, None).expect("resolve");
    let statics = static_sets(&tree);
    let observed = init_sets(&init);

    let mut failures = 0;
    println!(
        "differential: {} · claude-code {} (init) · rules {}",
        dir.display(),
        init.claude_code_version.as_deref().unwrap_or("?"),
        tree.rules_verified_against
    );
    for (list, names) in &observed {
        let expected: BTreeSet<&String> = names.iter().collect();
        let got = statics.get(list).cloned().unwrap_or_default();
        let have: BTreeSet<&String> = got.iter().collect();
        let missing: Vec<&&String> = expected.difference(&have).collect();
        let extra: Vec<&&String> = have.difference(&expected).collect();
        let mut seen = BTreeSet::new();
        let twice: BTreeSet<&String> = got.iter().filter(|n| !seen.insert(*n)).collect();
        println!(
            "{list:<12} init {:>3} · tree {:>3} · missing {} · extra {} · twice {}",
            expected.len(),
            got.len(),
            missing.len(),
            extra.len(),
            twice.len()
        );
        for name in &missing {
            println!("  missing  {name}");
        }
        for name in &extra {
            println!("  extra    {name}");
        }
        for name in &twice {
            println!("  twice    {name}");
        }
        failures += missing.len() + extra.len() + twice.len();
    }

    // `source` of each MCP server = the layer of its row.
    let layers: BTreeMap<String, String> = walk(&tree)
        .filter(|i| i.kind == Kind::McpServer)
        .map(|i| {
            let name = i.id.splitn(4, '/').nth(3).unwrap_or_default().to_string();
            (name, i.origin.layer.to_string())
        })
        .collect();
    for server in &init.mcp_servers {
        let (Some(source), Some(layer)) = (&server.source, layers.get(&server.name)) else {
            continue;
        };
        if source != layer {
            println!(
                "  source   {}: init {source}, tree layer {layer}",
                server.name
            );
            failures += 1;
        }
    }
    assert_eq!(
        failures, 0,
        "differential: {failures} difference(s), listed above"
    );
}
