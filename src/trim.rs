//! `harn trim` (`.prd/02-architecture.md` §trim): the operator loop as one command. The brief
//! (same text as `harn brief`), one headless request to the coding agent for a `harn-plan/1`, the
//! validation and plan print of `harn apply`, a confirmation, then the journaled write.
//!
//! The agent's prose is never parsed: the reply's first JSON object whose `schema` is
//! `harn-plan/1` is the plan, and a reply without one is an error quoting its first lines. The
//! agent runs in a fresh empty temp dir, so the target's own CLAUDE.md, hooks and MCP servers do
//! not load into the planning request; the brief carries every fact.

use crate::journal::{Journal, JournalEntry};
use crate::model::{self, Document, Refused, Target, Tree};
use crate::plan::{self, ActionPlan, PlanRequest, PLAN_SCHEMA};
use crate::render;
use crate::resolve::Resolver;
use crate::writers;
use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// `claude -p --output-format json --tools "" --max-turns 1` with the prompt piped on stdin (the
/// trailing `-`, see [`TrimOpts::agent_cmd`]; no single-argument size limit): no tools at all and
/// one turn by construction, one model request, the answer in `result`. `--agent-cmd` replaces
/// it as given (the user's own, trusted command).
pub const DEFAULT_AGENT_CMD: &[&str] = &[
    "claude",
    "-p",
    "--output-format",
    "json",
    "--tools",
    "",
    "--max-turns",
    "1",
    "-",
];

/// Read last by the agent, after the brief; `{dir}` = the target dir.
const INSTRUCTIONS: &str = "TASK: you are the operator of this directory's harness ({dir}). \
Using the brief above, decide what to turn off so the agent loads less here:
- prefer narrow scopes: omit \"scope\" (= local, this project only) unless an item must go everywhere;
- never a builtin row;
- leave rows whose cost is ? alone;
- a plugin takes its skills, agents and hooks with it: one op on the plugin row, none for its children.
Do not write files, run commands or use tools.
Answer with EXACTLY ONE JSON object of schema harn-plan/1 with \"target_dir\": \"{dir}\" and one op \
per change, each with a \"reason\" a human can read in `harn history` a month from now, and nothing \
else: no prose, no code fence.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimOpts {
    /// Program and arguments. A trailing `-` means: pipe the prompt on stdin instead of passing
    /// it as the last argument. The `-` is harn's marker and is not passed to the agent.
    pub agent_cmd: Vec<String>,
    /// Print the validated plan, write nothing.
    pub dry_run: bool,
    /// Apply without calling `confirm`.
    pub yes: bool,
}

impl Default for TrimOpts {
    fn default() -> TrimOpts {
        TrimOpts {
            agent_cmd: DEFAULT_AGENT_CMD.iter().map(|s| s.to_string()).collect(),
            dry_run: false,
            yes: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrimOutcome {
    /// The agent's plan has no ops: nothing validated, nothing written.
    Nothing,
    DryRun(ActionPlan),
    Declined(ActionPlan),
    Applied(JournalEntry),
}

/// Asks the agent in `opts.agent_cmd` for a plan over `tree`, prints it like `harn apply`, and
/// applies it through the journal when `opts.yes` or `confirm` says so. A plan without ops is
/// [`TrimOutcome::Nothing`]; a rejected plan is the `plan::build_plan_from_request` error,
/// unchanged (`Refused` → exit 3).
pub fn trim(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &Tree,
    opts: &TrimOpts,
    confirm: &mut dyn FnMut(&ActionPlan) -> bool,
    out: &mut dyn Write,
) -> anyhow::Result<TrimOutcome> {
    let program = opts
        .agent_cmd
        .first()
        .ok_or_else(|| anyhow!("--agent-cmd is empty"))?;
    let prompt = prompt(&brief(resolver, target, tree)?, target);
    writeln!(
        out,
        "asking {} for a plan (one model request)…",
        display_command(&opts.agent_cmd)
    )?;
    out.flush()?;
    let stdout = run_agent(&opts.agent_cmd, &prompt, &target.dir)?;
    let text = reply_text(&stdout);
    let Some(value) = extract_plan(&text) else {
        bail!(
            "no {PLAN_SCHEMA} object in the reply of {program}; it begins:\n{}",
            first_lines(&text, 3)
        );
    };
    if value
        .get("ops")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        writeln!(out, "the agent found nothing to turn off")?;
        return Ok(TrimOutcome::Nothing);
    }
    let req = plan_request(value, target, program, out)?;
    let plan = plan::build_plan_from_request(resolver, target, tree, &req)?;
    writeln!(out, "{}", render::plan(&plan))?;
    if opts.dry_run {
        writeln!(out, "{}", render::plan_footer(None))?;
        return Ok(TrimOutcome::DryRun(plan));
    }
    out.flush()?;
    if !opts.yes && !confirm(&plan) {
        writeln!(out, "declined: nothing written")?;
        return Ok(TrimOutcome::Declined(plan));
    }
    let mut journal = Journal::open()?;
    let entry = writers::apply(&plan, &mut journal)?;
    writeln!(out, "{}", render::plan_footer(Some(&entry)))?;
    writeln!(out, "undo: harn undo")?;
    Ok(TrimOutcome::Applied(entry))
}

/// `--agent-cmd` as words: split on whitespace; `'…'` and `"…"` group (no expansion), `\`
/// takes the next character literally outside single quotes.
pub fn split_command(text: &str) -> anyhow::Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            words.extend(word.take());
            continue;
        }
        let word = word.get_or_insert_with(String::new);
        match c {
            '\'' | '"' => loop {
                match chars.next() {
                    Some(q) if q == c => break,
                    Some('\\') if c == '"' => word.push(chars.next().ok_or_else(unterminated)?),
                    Some(other) => word.push(other),
                    None => return Err(unterminated()),
                }
            },
            '\\' => word.push(chars.next().ok_or_else(unterminated)?),
            other => word.push(other),
        }
    }
    words.extend(word);
    if words.is_empty() {
        bail!("--agent-cmd is empty");
    }
    Ok(words)
}

/// `cmd` as a shell would read it back: a word that is empty or holds whitespace or a quote is
/// single-quoted (so `--tools ""` shows as `--tools ''`, not as a double space).
fn display_command(cmd: &[String]) -> String {
    cmd.iter()
        .map(|word| {
            if !word.is_empty()
                && !word.contains(|c: char| c.is_whitespace() || "'\"\\".contains(c))
            {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn unterminated() -> anyhow::Error {
    anyhow!("--agent-cmd: unterminated quote or trailing \\")
}

/// What `harn brief` prints for this one agent: the tree annotated with freed bytes.
fn brief(resolver: &dyn Resolver, target: &Target, tree: &Tree) -> anyhow::Result<String> {
    let mut annotated = tree.clone();
    plan::annotate_freed(resolver, target, &mut annotated)
        .with_context(|| tree.agent.to_string())?;
    Ok(render::brief(&Document {
        schema: model::SCHEMA.to_string(),
        generated_at: model::now_iso8601(),
        agents: vec![annotated],
    }))
}

fn prompt(brief: &str, target: &Target) -> String {
    let dir = target.dir.display().to_string();
    format!("{brief}\n\n{}", INSTRUCTIONS.replace("{dir}", &dir))
}

/// A fresh empty dir (0700) for the agent's cwd, removed on drop — every return of
/// [`run_agent`], its failures included.
struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Under [`scratch_base`] of `std::env::temp_dir()`, else `/tmp`.
    fn new(target_dir: &Path) -> anyhow::Result<ScratchDir> {
        let base = scratch_base(target_dir, &[std::env::temp_dir(), PathBuf::from("/tmp")])?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("harn-trim-{}-{nanos}", std::process::id()));
        // `create`, not `create_all`: an existing path (planted or not) is an error.
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("create {}", dir.display()))?;
        Ok(ScratchDir(dir))
    }
}

/// The first candidate (canonicalized) that is not inside `target_dir`: the agent's cwd and its
/// ancestors must not be the target or under it, or the agent would load the target's own
/// CLAUDE.md, hooks and MCP servers into the planning request.
fn scratch_base(target_dir: &Path, candidates: &[PathBuf]) -> anyhow::Result<PathBuf> {
    candidates
        .iter()
        .filter_map(|dir| std::fs::canonicalize(dir).ok())
        .find(|dir| !dir.starts_with(target_dir))
        .ok_or_else(|| {
            anyhow!(
                "cannot find a scratch directory outside {}",
                target_dir.display()
            )
        })
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        // Best effort: whatever the agent left there goes too.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the agent once, in a [`ScratchDir`], and returns its stdout. Not found → the
/// `/harn-trim` hint; a failing exit → the first lines of its stderr (of its stdout when stderr
/// is empty: Claude Code's json output reports errors there).
fn run_agent(cmd: &[String], prompt: &str, target_dir: &Path) -> anyhow::Result<String> {
    let (program, args) = cmd
        .split_first()
        .ok_or_else(|| anyhow!("--agent-cmd is empty"))?;
    // A trailing `-` is harn's stdin-delivery marker, not an argument for the agent.
    let (args, on_stdin) = match args.split_last() {
        Some((last, rest)) if last == "-" => (rest, true),
        _ => (args, false),
    };
    let cwd = ScratchDir::new(target_dir)?;
    let mut command = Command::new(program);
    command
        .current_dir(&cwd.0)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if on_stdin {
        command.stdin(Stdio::piped());
    } else {
        command.arg(prompt).stdin(Stdio::null());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            bail!("agent command not found: {program} — run /harn-trim inside Claude Code instead")
        }
        Err(e) => return Err(anyhow::Error::new(e).context(format!("run {program}"))),
    };
    // Written from a thread so a child that answers before reading all of stdin cannot block
    // on a full stdout pipe while harn blocks on a full stdin pipe.
    let writer = child.stdin.take().map(|mut stdin| {
        let prompt = prompt.to_string();
        std::thread::spawn(move || stdin.write_all(prompt.as_bytes()))
    });
    let output = child
        .wait_with_output()
        .with_context(|| format!("run {program}"))?;
    if let Some(writer) = writer {
        // A child that exits without reading its stdin breaks the pipe; its exit status and
        // reply say more than the write error.
        let _ = writer.join();
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let (stream, text) = if stderr.trim().is_empty() {
            ("stdout", String::from_utf8_lossy(&output.stdout))
        } else {
            ("stderr", stderr)
        };
        bail!(
            "agent command {program} failed ({}); its {stream} begins:\n{}",
            output.status,
            first_lines(&text, 5)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The answer text: Claude Code's `--output-format json` puts it in a string `result` (one
/// object, or the `result` message of an array); any other stdout is the answer itself.
fn reply_text(stdout: &str) -> String {
    let result = |value: &Value| {
        value
            .get("result")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    match serde_json::from_str::<Value>(stdout.trim()) {
        Ok(value @ Value::Object(_)) => result(&value),
        Ok(Value::Array(messages)) => messages.iter().rev().find_map(result),
        _ => None,
    }
    .unwrap_or_else(|| stdout.to_string())
}

/// The first balanced `{…}` of `text` that parses as a JSON object whose decoded `schema` is
/// [`PLAN_SCHEMA`] (so `harn-plan\/1` and `harn-plan\u002f1` count). Every `{` is a candidate
/// start, so an inner object is still found when an outer one is prose or a wrapper
/// (`{"plan": {…}}`).
fn extract_plan(text: &str) -> Option<Value> {
    text.match_indices('{').find_map(|(start, _)| {
        let candidate = &text[start..start + balanced_len(&text.as_bytes()[start..])?];
        let value: Value = serde_json::from_str(candidate).ok()?;
        (value.get("schema").and_then(Value::as_str) == Some(PLAN_SCHEMA)).then_some(value)
    })
}

/// Length of the brace-balanced object at the start of `bytes` (which begins with `{`); braces
/// inside JSON strings, with their backslash escapes, do not count. Byte-wise is UTF-8 safe:
/// the bytes compared are ASCII and never occur inside a multi-byte character.
fn balanced_len(bytes: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// The reply's plan as a request for `target`. A missing or foreign `target_dir` becomes the
/// target with a note (the ops are resolved against the target's tree either way); a plan that
/// does not deserialize is refused like a plan file `harn apply` rejects.
fn plan_request(
    mut value: Value,
    target: &Target,
    program: &str,
    out: &mut dyn Write,
) -> anyhow::Result<PlanRequest> {
    let given = value.get("target_dir").cloned();
    let same = given
        .as_ref()
        .and_then(Value::as_str)
        .and_then(|dir| std::fs::canonicalize(dir).ok())
        .is_some_and(|dir| dir == target.dir);
    let dir = target.dir.display().to_string();
    if !same {
        match &given {
            None => writeln!(out, "note: the plan has no target_dir; using {dir}")?,
            Some(given) => writeln!(
                out,
                "note: the plan's target_dir {given} is not the target; using {dir}"
            )?,
        }
        value["target_dir"] = Value::String(dir);
    }
    let mut req: PlanRequest = serde_json::from_value(value)
        .map_err(|e| Refused(format!("plan from {program} rejected: {e}")))?;
    if !same {
        // The canonical path itself, not its display string (which is lossy off UTF-8).
        req.target_dir = target.dir.clone();
    }
    Ok(req)
}

/// The first `n` lines of `text`, indented for an error message.
fn first_lines(text: &str, n: usize) -> String {
    let lines: Vec<String> = text.lines().take(n).map(|l| format!("  {l}")).collect();
    if lines.is_empty() {
        "  (empty)".to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn extract_plan_finds_the_object_nested_in_prose_with_braces_in_strings() {
        let plan = r#"{"schema": "harn-plan/1", "target_dir": "/x", "ops": [{"selector": "a", "reason": "has } and { and \" in it"}]}"#;
        let text = format!(
            "Sure {{not json}} — here is {{ the plan:\n```json\n{plan}\n```\nand a stray }} {{}}"
        );
        let found = extract_plan(&text).expect("a plan");
        assert_eq!(found, serde_json::from_str::<Value>(plan).unwrap());
        assert_eq!(found["ops"][0]["reason"], "has } and { and \" in it");
    }

    #[test]
    fn extract_plan_decodes_the_schema_instead_of_matching_its_text() {
        for schema in [
            r#""harn-plan\/1""#,
            r#""harn-plan\u002f1""#,
            r#""harn-plan\u002F1""#,
        ] {
            let text = format!(r#"plan: {{"schema":{schema},"target_dir":"/x","ops":[]}} done"#);
            let found = extract_plan(&text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(found["schema"], PLAN_SCHEMA, "{text}");
            assert_eq!(found["ops"], serde_json::json!([]), "{text}");
        }
    }

    #[test]
    fn extract_plan_skips_wrappers_and_other_schemas() {
        let text = r#"{"plan": {"schema": "harn-plan/1", "target_dir": "/x", "ops": []}}"#;
        assert_eq!(extract_plan(text).unwrap()["target_dir"], "/x");
        assert_eq!(
            extract_plan(r#"{"schema": "harn/1", "note": "harn-plan/1"}"#),
            None
        );
        assert_eq!(extract_plan(r#"{"schema": "harn-plan/1", "ops": ["#), None);
        assert_eq!(extract_plan("no braces, harn-plan/1"), None);
        assert_eq!(extract_plan("} {"), None);
    }

    #[test]
    fn balanced_len_ignores_braces_in_strings_and_escapes() {
        assert_eq!(balanced_len(br#"{"a":"}\"}"} tail"#), Some(12));
        assert_eq!(balanced_len(b"{{}}x"), Some(4));
        assert_eq!(balanced_len(b"{\"open"), None);
    }

    #[test]
    fn reply_text_unwraps_claude_json_only() {
        assert_eq!(
            reply_text(r#"{"type":"result","is_error":false,"result":"the {plan}"}"#),
            "the {plan}"
        );
        assert_eq!(
            reply_text(r#"[{"type":"system"},{"type":"result","result":"from the array"}]"#),
            "from the array"
        );
        let raw = r#"{"schema":"harn-plan/1","target_dir":"/x","ops":[]}"#;
        assert_eq!(reply_text(raw), raw);
        assert_eq!(reply_text("plain prose\n"), "plain prose\n");
    }

    #[test]
    fn split_command_splits_on_whitespace_and_groups_quotes() {
        assert_eq!(
            split_command("  claude -p   --output-format json --tools '' --max-turns 1 - ")
                .unwrap(),
            DEFAULT_AGENT_CMD
        );
        assert_eq!(
            split_command(r#"'/a b/agent' "x \"y\"" c\ d '' -"#).unwrap(),
            ["/a b/agent", "x \"y\"", "c d", "", "-"]
        );
        assert!(split_command("   ").is_err());
        assert!(split_command("agent 'open").is_err());
        assert!(split_command("agent \\").is_err());
    }

    #[test]
    fn display_command_quotes_empty_and_spaced_words_only() {
        let default: Vec<String> = DEFAULT_AGENT_CMD.iter().map(|s| s.to_string()).collect();
        let shown = display_command(&default);
        assert_eq!(
            shown,
            "claude -p --output-format json --tools '' --max-turns 1 -"
        );
        assert_eq!(split_command(&shown).unwrap(), default);
        let odd = [
            "/a b/agent".to_string(),
            "it's".to_string(),
            "x".to_string(),
        ];
        assert_eq!(display_command(&odd), r"'/a b/agent' 'it'\''s' x");
    }

    #[test]
    fn scratch_base_is_never_inside_the_target() {
        let root = std::env::temp_dir().join(format!("harn-trim-unit-{}", std::process::id()));
        let target = root.join("project");
        let inside = target.join("tmp");
        let outside = root.join("elsewhere");
        for dir in [&inside, &outside] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let target = std::fs::canonicalize(&target).unwrap();
        let outside = std::fs::canonicalize(&outside).unwrap();
        // The first candidate is inside the target: the next one is used.
        assert_eq!(
            scratch_base(&target, &[inside.clone(), outside.clone()]).unwrap(),
            outside
        );
        // The target itself counts as inside; a missing candidate is skipped.
        let err = scratch_base(&target, &[target.clone(), inside, root.join("missing")])
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!(
                "cannot find a scratch directory outside {}",
                target.display()
            )
        );
        // A target that contains everything leaves no candidate.
        assert!(scratch_base(Path::new("/"), &[outside]).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn prompt_is_the_brief_then_the_instructions_naming_the_target() {
        let target = Target {
            dir: PathBuf::from("/work/project"),
            home: PathBuf::from("/home/u"),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        };
        let text = prompt("harn brief · schema harn/1", &target);
        assert!(
            text.starts_with("harn brief · schema harn/1\n\nTASK: "),
            "{text}"
        );
        assert!(text.contains("harness (/work/project)"), "{text}");
        assert!(text.contains(r#""target_dir": "/work/project""#), "{text}");
        assert!(text.ends_with("no prose, no code fence."), "{text}");
        assert!(!text.contains("{dir}"), "{text}");
    }

    #[test]
    fn first_lines_quotes_at_most_n_lines() {
        assert_eq!(first_lines("a\nb\nc\nd", 3), "  a\n  b\n  c");
        assert_eq!(first_lines("", 3), "  (empty)");
    }
}
