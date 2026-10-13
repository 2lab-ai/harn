//! `harn` CLI: argument parsing, the `--readonly` gate, exit codes, and the wiring from each
//! subcommand to the library (`.prd/03-p0-interfaces.md` §main.rs). No logic lives here that a
//! library module owns.

use anyhow::{anyhow, bail, Context, Result};
use clap::builder::RangedU64ValueParser;
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use harn::journal::{self, Journal};
use harn::model::{self, Agent, Document, Item, Layer, Refused, Scope, SelectError, Stale, Target};
use harn::plan::{self, PlanRequest, PLAN_SCHEMA};
use harn::render::{self, RenderOpts};
use harn::resolve::{self, Resolver};
use harn::{oracle, trim, verify, writers};
use serde::Serialize;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const EXIT_DIFF: u8 = 1;
const EXIT_AMBIGUOUS: u8 = 2;
const EXIT_REFUSED: u8 = 3;
const EXIT_STALE: u8 = 4;

#[derive(Parser, Debug)]
#[command(
    name = "harn",
    about = "Show the effective harness an AI coding agent loads in a directory — origin, cost and \
             the official off switch of every item — and change it reversibly",
    after_help = "Exit codes: 0 ok · 1 error or verify diff · 2 ambiguous selector · \
                  3 refused (readonly / builtin / no knob / rejected plan) · 4 stale journal"
)]
struct Cli {
    /// Target directory, same as --dir (default: cwd)
    #[arg(value_name = "DIR", conflicts_with = "dir")]
    dir_pos: Option<PathBuf>,

    /// Target directory (default: cwd)
    #[arg(long, value_name = "DIR", global = true)]
    dir: Option<PathBuf>,

    /// One agent (default: every agent with a resolver, one table each)
    #[arg(long, value_enum, global = true)]
    agent: Option<AgentArg>,

    /// Machine-readable output (schema harn/1)
    #[arg(long, global = true)]
    json: bool,

    /// Refuse every write and every active verify (exit 3)
    #[arg(long, global = true)]
    readonly: bool,

    /// ASCII only
    #[arg(long, global = true)]
    no_unicode: bool,

    /// Color output (auto = stdout is a terminal and NO_COLOR is unset)
    #[arg(long, value_enum, value_name = "WHEN", default_value_t = ColorArg::Auto, global = true)]
    color: ColorArg,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Admission chain, realpath, knob candidates by scope and the freed bytes of each
    Why { selector: String },
    /// Print the resolved realpath of an item
    Path { selector: String },
    /// Turn an item off through the agent's official switch (plan, then apply)
    Off {
        selector: String,
        /// Scope of the switch; never widened silently (default: local)
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        /// Print the plan, write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Turn an item back on by reversing the journaled op that turned it off
    On { selector: String },
    /// Open an item's real file in $EDITOR between a snapshot and a diff
    Edit { selector: String },
    /// Reverse the last N journal entries
    Undo {
        #[arg(value_name = "N", default_value_t = 1, value_parser = RangedU64ValueParser::<usize>::new().range(1..))]
        n: usize,
    },
    /// List journal entries
    History {
        /// Delete entries older than AGE (e.g. 30d, 12h, 2w) — the only way entries are deleted
        #[arg(long, value_name = "AGE", value_parser = parse_age)]
        prune: Option<Duration>,
    },
    /// Diff the static tree against the agent's own request
    Verify(VerifyArgs),
    /// LLM-oriented summary of the tree, ending with the harn-plan/1 format
    Brief {
        /// Target directory, same as --dir (default: cwd)
        #[arg(value_name = "DIR", conflicts_with = "dir")]
        dir_pos: Option<PathBuf>,
    },
    /// Apply an AI-authored plan file (schema harn-plan/1)
    Apply {
        #[arg(value_name = "PLAN_JSON")]
        plan: PathBuf,
        /// Print the plan, write nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Ask the coding agent for a trimming plan (one model request), print it, apply on y
    Trim {
        /// Target directory, same as --dir (default: cwd)
        #[arg(value_name = "DIR", conflicts_with = "dir")]
        dir_pos: Option<PathBuf>,
        /// Print the agent's validated plan, write nothing
        #[arg(long)]
        dry_run: bool,
        /// Apply without asking (required when stdin is not a terminal)
        #[arg(long)]
        yes: bool,
        /// Agent command, run as given in an empty temp dir; a trailing `-` means: pipe the
        /// prompt on stdin instead of passing it as the last argument (default: claude -p
        /// --output-format json --tools '' --max-turns 1 -)
        #[arg(long, value_name = "CMD")]
        agent_cmd: Option<String>,
    },
    /// Write a self-contained read-only HTML report
    Report {
        /// Target directory, same as --dir (default: cwd)
        #[arg(value_name = "DIR", conflicts_with = "dir")]
        dir_pos: Option<PathBuf>,
        #[arg(long, value_name = "OUT", required = true)]
        html: PathBuf,
    },
    /// Print shell completions
    Completions { shell: clap_complete::Shell },
}

#[derive(Args, Debug)]
struct VerifyArgs {
    #[command(flatten)]
    source: VerifySource,
    /// After the report, print the whole verified table (observed costs, freed) instead of only
    /// its header (text output)
    #[arg(long)]
    table: bool,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct VerifySource {
    /// Read an existing capture (OTEL raw-body dir or llmux raw-io.jsonl); never writes
    #[arg(long, value_name = "PATH")]
    capture: Option<PathBuf>,
    /// Run the agent once under its official capture (runs hooks and MCP servers)
    #[arg(long)]
    run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum AgentArg {
    ClaudeCode,
    Codex,
}

impl From<AgentArg> for Agent {
    fn from(arg: AgentArg) -> Agent {
        match arg {
            AgentArg::ClaudeCode => Agent::ClaudeCode,
            AgentArg::Codex => Agent::Codex,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ScopeArg {
    Local,
    Project,
    User,
}

impl From<ScopeArg> for Scope {
    fn from(arg: ScopeArg) -> Scope {
        match arg {
            ScopeArg::Local => Scope::Local,
            ScopeArg::Project => Scope::Project,
            ScopeArg::User => Scope::User,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ColorArg {
    Auto,
    Always,
    Never,
}

fn main() -> ExitCode {
    let cli = parse_cli(std::env::args_os()).unwrap_or_else(|err| err.exit());
    match run(&cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("harn: {err:#}");
            ExitCode::from(exit_code_for(&err))
        }
    }
}

/// clap parse + the DIR rules clap cannot express across levels: the top-level positional DIR
/// belongs to the listing (`harn [DIR]`); subcommands take `--dir` (`brief`/`report` also their own
/// positional DIR); a positional DIR and `--dir` together are a conflict in every order.
fn parse_cli<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut command = Cli::command().version(version_text());
    let matches = command.try_get_matches_from_mut(args)?;
    let cli = Cli::from_arg_matches(&matches).map_err(|err| err.format(&mut command))?;
    if cli.dir_pos.is_some() && cli.command.is_some() {
        return Err(command.error(
            ErrorKind::ArgumentConflict,
            "the positional [DIR] is for the listing; give a subcommand its target with --dir",
        ));
    }
    // `--dir` before the subcommand is set on the parent, which clap's per-level conflict check
    // does not see against the subcommand's own positional DIR.
    if let Some(
        Cmd::Brief { dir_pos: Some(_) }
        | Cmd::Report {
            dir_pos: Some(_), ..
        }
        | Cmd::Trim {
            dir_pos: Some(_), ..
        },
    ) = &cli.command
    {
        if cli.dir.is_some() {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                "the argument '--dir <DIR>' cannot be used with '[DIR]'",
            ));
        }
    }
    Ok(cli)
}

/// [`harn::version_line`] without its `harn ` prefix (clap adds it).
fn version_text() -> String {
    let line = harn::version_line();
    line.strip_prefix("harn ").unwrap_or(&line).to_string()
}

fn exit_code_for(err: &anyhow::Error) -> u8 {
    for cause in err.chain() {
        if let Some(SelectError::Ambiguous(_)) = cause.downcast_ref::<SelectError>() {
            return EXIT_AMBIGUOUS;
        }
        if cause.is::<Refused>() {
            return EXIT_REFUSED;
        }
        if cause.is::<Stale>() {
            return EXIT_STALE;
        }
    }
    1
}

/// The subcommand's name when it would write or run the agent; `--readonly` refuses these.
/// `trim` runs the agent even with `--dry-run`, like `verify --run`.
fn write_command(cmd: &Cmd) -> Option<&'static str> {
    match cmd {
        Cmd::Off { .. } => Some("off"),
        Cmd::On { .. } => Some("on"),
        Cmd::Edit { .. } => Some("edit"),
        Cmd::Undo { .. } => Some("undo"),
        Cmd::Apply { .. } => Some("apply"),
        Cmd::Trim { .. } => Some("trim"),
        Cmd::History { prune: Some(_) } => Some("history --prune"),
        Cmd::Verify(VerifyArgs {
            source: VerifySource { run: true, .. },
            ..
        }) => Some("verify --run"),
        _ => None,
    }
}

fn run(cli: &Cli) -> Result<ExitCode> {
    // Before any library call: --readonly never reaches a writer or the agent.
    if cli.readonly {
        if let Some(name) = cli.command.as_ref().and_then(write_command) {
            return Err(Refused(format!(
                "--readonly: `harn {name}` writes or runs the agent"
            ))
            .into());
        }
    }
    let opts = RenderOpts {
        unicode: !cli.no_unicode,
        color: color_enabled(cli.color),
        width: std::env::var("COLUMNS").ok().and_then(|c| c.parse().ok()),
    };
    match &cli.command {
        None => {
            let doc = document(cli, cli.dir_pos.as_deref())?;
            if cli.json {
                println!("{}", render::json(&doc));
            } else {
                let tables: Vec<String> =
                    doc.agents.iter().map(|t| render::table(t, &opts)).collect();
                println!("{}", tables.join("\n\n"));
            }
        }
        Some(Cmd::Brief { dir_pos }) => {
            let doc = document(cli, dir_pos.as_deref())?;
            println!("{}", render::brief(&doc));
        }
        Some(Cmd::Report { dir_pos, html }) => {
            let doc = document(cli, dir_pos.as_deref())?;
            std::fs::write(html, render::html(&doc) + "\n")
                .with_context(|| format!("write {}", html.display()))?;
            eprintln!("wrote {}", html.display());
        }
        Some(Cmd::Why { selector }) => {
            let target = target(cli, None)?;
            let trees = resolve_trees(cli, &target)?;
            let (resolved, item) = pick(&trees, selector)?;
            let plans = plan::plans_by_scope(
                resolved.resolver.as_ref(),
                &target,
                &resolved.tree,
                &item.id,
            )?;
            if cli.json {
                print_json(&serde_json::json!({ "item": item, "plans": plans }))?;
            } else {
                println!("{}", render::why(&resolved.tree, item, &plans));
            }
        }
        Some(Cmd::Path { selector }) => {
            let target = target(cli, None)?;
            let trees = resolve_trees(cli, &target)?;
            let (_, item) = pick(&trees, selector)?;
            match (&item.origin.path, &item.origin.setting) {
                (Some(path), _) => println!("{}", path.display()),
                (None, Some(setting)) => println!("{setting}"),
                (None, None) => bail!(
                    "{} has no file or setting ({} item)",
                    item.id,
                    item.origin.layer
                ),
            }
        }
        Some(Cmd::Off {
            selector,
            scope,
            dry_run,
        }) => {
            let target = target(cli, None)?;
            let trees = resolve_trees(cli, &target)?;
            let (resolved, item) = pick(&trees, selector)?;
            let plan = plan::build_plan(
                resolved.resolver.as_ref(),
                &target,
                &resolved.tree,
                &item.id,
                scope.map(Scope::from),
            )?;
            apply_plan(cli, &plan, *dry_run)?;
        }
        Some(Cmd::Apply {
            plan: path,
            dry_run,
        }) => {
            let req = read_plan_request(path)?;
            let dir = cli.dir.as_deref().unwrap_or(&req.target_dir);
            let target = target_for(dir)?;
            let plan_dir = std::fs::canonicalize(&req.target_dir)
                .with_context(|| format!("plan target_dir {}", req.target_dir.display()))?;
            if plan_dir != target.dir {
                return Err(Refused(format!(
                    "plan is for {} but --dir is {}",
                    plan_dir.display(),
                    target.dir.display()
                ))
                .into());
            }
            let agent = single_agent(cli, "apply")?;
            let resolver = resolve::for_agent(agent);
            let tree = resolver
                .resolve(&target, None)
                .with_context(|| agent.to_string())?;
            let plan = plan::build_plan_from_request(resolver.as_ref(), &target, &tree, &req)?;
            apply_plan(cli, &plan, *dry_run)?;
        }
        Some(Cmd::Trim {
            dir_pos,
            dry_run,
            yes,
            agent_cmd,
        }) => {
            // Before the model request: a plan nobody can confirm is not worth one.
            if !dry_run && !yes && !std::io::stdin().is_terminal() {
                bail!("not a terminal: pass --yes to apply");
            }
            let target = target(cli, dir_pos.as_deref())?;
            let agent = single_agent(cli, "trim")?;
            let resolver = resolve::for_agent(agent);
            let tree = resolver
                .resolve(&target, None)
                .with_context(|| agent.to_string())?;
            let mut opts = trim::TrimOpts {
                dry_run: *dry_run,
                yes: *yes,
                ..trim::TrimOpts::default()
            };
            if let Some(cmd) = agent_cmd {
                opts.agent_cmd = trim::split_command(cmd)?;
            }
            trim::trim(
                resolver.as_ref(),
                &target,
                &tree,
                &opts,
                &mut confirm_on_tty,
                &mut std::io::stdout(),
            )?;
        }
        Some(Cmd::On { selector }) => {
            let target = target(cli, None)?;
            let mut journal = open_journal()?;
            let entry = journal::on(&mut journal, &target, selector)?;
            print_entries(cli, std::slice::from_ref(&entry))?;
        }
        Some(Cmd::Edit { selector }) => {
            let target = target(cli, None)?;
            let trees = resolve_trees(cli, &target)?;
            let (resolved, item) = pick(&trees, selector)?;
            if matches!(item.origin.layer, Layer::Builtin | Layer::Managed) {
                return Err(Refused(format!("{} is a {} item", item.id, item.origin.layer)).into());
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
            let mut journal = open_journal()?;
            let entry = journal::edit(
                item,
                &target,
                &mut journal,
                &editor,
                resolved.tree.agent_version.clone(),
            )?;
            // Text: the diff, then the journal line (`--json` carries the diff in the entry).
            if let (false, Some(diff)) = (cli.json, entry.diff.as_deref()) {
                print!("{diff}");
                if !diff.ends_with('\n') {
                    println!();
                }
            }
            print_entries(cli, std::slice::from_ref(&entry))?;
        }
        Some(Cmd::Undo { n }) => {
            let mut journal = open_journal()?;
            let entries = journal::undo(&mut journal, *n)?;
            print_entries(cli, &entries)?;
        }
        Some(Cmd::History { prune }) => {
            let entries = match prune {
                Some(age) => {
                    let mut journal = open_journal()?;
                    journal::prune(&mut journal, *age)?
                }
                None => journal::history(&open_journal()?)?,
            };
            print_entries(cli, &entries)?;
        }
        Some(Cmd::Verify(args)) => return verify_cmd(cli, args, &opts),
        Some(Cmd::Completions { shell }) => {
            clap_complete::generate(*shell, &mut Cli::command(), "harn", &mut std::io::stdout());
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The report, then the verified tree's header (`--table`: the whole table, freed annotated) so
/// the observed totals and their region split are visible; `--json` prints the report only.
fn verify_cmd(cli: &Cli, args: &VerifyArgs, opts: &RenderOpts) -> Result<ExitCode> {
    if cli.agent == Some(AgentArg::Codex) {
        bail!("not implemented: codex verify (P0.5)");
    }
    let target = target(cli, None)?;
    let resolver = resolve::for_agent(Agent::ClaudeCode);
    let mut tree = resolver.resolve(&target, None).context("claude-code")?;
    let capture = match (&args.source.capture, args.source.run) {
        (Some(path), _) => oracle::read_capture(path, &target)?,
        (None, _) => {
            let claude =
                find_on_path("claude").ok_or_else(|| anyhow!("`claude` is not on PATH"))?;
            eprintln!(
                "harn verify --run: starting {} once in {} under its official capture \
                 (raw request body + stream-json init); this runs your hooks and MCP servers \
                 and may cost one model request",
                claude.display(),
                target.dir.display()
            );
            oracle::run_claude(&target, &claude)?
        }
    };
    let report = verify::verify(&mut tree, &capture);
    if cli.json {
        print_json(&report)?;
    } else {
        println!("{}", render::verify_report(&report));
        println!();
        if args.table {
            plan::annotate_freed(resolver.as_ref(), &target, &mut tree).context("claude-code")?;
            println!("{}", render::table(&tree, opts));
        } else {
            println!("{}", render::header(&tree, opts));
        }
    }
    Ok(if report.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_DIFF)
    })
}

/// Prints the plan; applies it through the journal unless `dry_run`.
fn apply_plan(cli: &Cli, plan: &plan::ActionPlan, dry_run: bool) -> Result<()> {
    let entry = if dry_run {
        None
    } else {
        let mut journal = open_journal()?;
        Some(writers::apply(plan, &mut journal)?)
    };
    if cli.json {
        return print_json(&serde_json::json!({ "plan": plan, "entry": entry }));
    }
    println!("{}", render::plan(plan));
    println!("{}", render::plan_footer(entry.as_ref()));
    Ok(())
}

/// `apply? [y/N]` on stderr, the answer from stdin; anything but y/yes declines.
fn confirm_on_tty(_: &plan::ActionPlan) -> bool {
    eprint!("apply? [y/N] ");
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn read_plan_request(path: &Path) -> Result<PlanRequest> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let reject = |why: String| -> anyhow::Error {
        Refused(format!("plan {} rejected: {why}", path.display())).into()
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| reject(e.to_string()))?;
    match value.get("schema").and_then(|s| s.as_str()) {
        Some(PLAN_SCHEMA) => {}
        other => {
            return Err(reject(format!(
                "schema is {other:?}, expected \"{PLAN_SCHEMA}\""
            )))
        }
    }
    serde_json::from_value(value).map_err(|e| reject(e.to_string()))
}

/// Every requested agent's tree, resolved and annotated with freed bytes.
fn document(cli: &Cli, dir_pos: Option<&Path>) -> Result<Document> {
    let target = target(cli, dir_pos)?;
    let mut agents = Vec::new();
    for resolved in resolve_trees(cli, &target)? {
        let mut tree = resolved.tree;
        plan::annotate_freed(resolved.resolver.as_ref(), &target, &mut tree)
            .with_context(|| tree.agent.to_string())?;
        agents.push(tree);
    }
    Ok(Document {
        schema: model::SCHEMA.to_string(),
        generated_at: model::now_iso8601(),
        agents,
    })
}

struct Resolved {
    resolver: Box<dyn Resolver>,
    tree: model::Tree,
}

fn agents(cli: &Cli) -> Vec<Agent> {
    match cli.agent {
        Some(agent) => vec![agent.into()],
        None => resolve::default_agents().to_vec(),
    }
}

fn single_agent(cli: &Cli, command: &str) -> Result<Agent> {
    match agents(cli).as_slice() {
        [agent] => Ok(*agent),
        _ => bail!("`harn {command}` works on one agent; pass --agent"),
    }
}

fn resolve_trees(cli: &Cli, target: &Target) -> Result<Vec<Resolved>> {
    agents(cli)
        .into_iter()
        .map(|agent| {
            let resolver = resolve::for_agent(agent);
            let tree = resolver
                .resolve(target, None)
                .with_context(|| agent.to_string())?;
            Ok(Resolved { resolver, tree })
        })
        .collect()
}

/// `model::select` across every resolved agent; a selector matching in two trees is ambiguous.
fn pick<'a>(trees: &'a [Resolved], selector: &str) -> Result<(&'a Resolved, &'a Item)> {
    let mut found = Vec::new();
    let mut candidates = Vec::new();
    for resolved in trees {
        match model::select(&resolved.tree, selector) {
            Ok(item) => found.push((resolved, item)),
            Err(SelectError::Ambiguous(ids)) => candidates.extend(ids),
            Err(SelectError::NotFound(_)) => {}
        }
    }
    if let ([only], true) = (found.as_slice(), candidates.is_empty()) {
        return Ok(*only);
    }
    candidates.extend(found.iter().map(|(_, item)| item.id.clone()));
    if candidates.is_empty() {
        return Err(SelectError::NotFound(selector.to_string()).into());
    }
    candidates.sort();
    Err(SelectError::Ambiguous(candidates).into())
}

/// `--dir` or the subcommand's positional DIR (clap rejects both), else cwd.
fn target(cli: &Cli, dir_pos: Option<&Path>) -> Result<Target> {
    target_for(cli.dir.as_deref().or(dir_pos).unwrap_or(Path::new(".")))
}

fn target_for(dir: &Path) -> Result<Target> {
    let canonical =
        std::fs::canonicalize(dir).with_context(|| format!("target {}", dir.display()))?;
    if !canonical.is_dir() {
        bail!("target {} is not a directory", canonical.display());
    }
    Ok(Target {
        dir: canonical,
        home: dirs::home_dir().ok_or_else(|| anyhow!("cannot determine the home directory"))?,
        env: std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    })
}

fn open_journal() -> Result<Journal> {
    Journal::open()
}

fn print_entries(cli: &Cli, entries: &[journal::JournalEntry]) -> Result<()> {
    if cli.json {
        print_json(&entries)
    } else {
        println!("{}", render::history(entries));
        Ok(())
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn color_enabled(color: ColorArg) -> bool {
    match color {
        ColorArg::Always => true,
        ColorArg::Never => false,
        ColorArg::Auto => {
            std::io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        }
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// `<N><unit>`, unit = s | m | h | d | w.
fn parse_age(text: &str) -> Result<Duration, String> {
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, unit) = text.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("`{text}`: expected <N><unit>, unit = s|m|h|d|w (e.g. 30d)"))?;
    let secs = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return Err(format!("`{text}`: unit must be one of s|m|h|d|w")),
    };
    n.checked_mul(secs)
        .map(Duration::from_secs)
        .ok_or_else(|| format!("`{text}`: too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        parse_cli(std::iter::once("harn").chain(args.iter().copied()))
    }

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn positional_dir_and_dir_flag_conflict() {
        assert_eq!(
            parse(&["/tmp", "--dir", "/tmp"]).unwrap_err().kind(),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            parse(&["brief", "/tmp", "--dir", "/tmp"])
                .unwrap_err()
                .kind(),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            parse(&["--dir", "/tmp", "report", "/tmp", "--html", "o.html"])
                .unwrap_err()
                .kind(),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            parse(&["--dir", "/tmp", "trim", "/tmp"])
                .unwrap_err()
                .kind(),
            ErrorKind::ArgumentConflict
        );
        assert!(parse(&["trim", "/tmp", "--dry-run"]).is_ok());
    }

    #[test]
    fn top_level_dir_is_listing_only() {
        assert_eq!(
            parse(&["/tmp", "why", "x"]).unwrap_err().kind(),
            ErrorKind::ArgumentConflict
        );
        assert!(parse(&["/tmp"]).unwrap().dir_pos.is_some());
        assert!(parse(&["--json", "/tmp"]).unwrap().json);
        assert!(parse(&["--dir", "/tmp", "why", "x"]).is_ok());
        assert!(parse(&["why", "x", "--dir", "/tmp"]).is_ok());
    }

    #[test]
    fn global_flags_work_before_and_after_the_subcommand() {
        let cli = parse(&["--readonly", "off", "x"]).unwrap();
        assert!(cli.readonly);
        let cli = parse(&["off", "x", "--readonly", "--agent", "claude-code"]).unwrap();
        assert!(cli.readonly);
        assert!(matches!(cli.agent, Some(AgentArg::ClaudeCode)));
    }

    #[test]
    fn verify_needs_exactly_one_source() {
        assert!(parse(&["verify"]).is_err());
        assert!(parse(&["verify", "--run", "--capture", "x"]).is_err());
        assert!(parse(&["verify", "--run"]).is_ok());
        assert!(parse(&["verify", "--capture", "x", "--agent", "claude-code"]).is_ok());
        assert!(parse(&["verify", "--table"]).is_err());
        assert!(parse(&["verify", "--run", "--table"])
            .unwrap()
            .command
            .is_some());
        let cli = parse(&["verify", "--capture", "x", "--table"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Cmd::Verify(VerifyArgs { table: true, .. }))
        ));
    }

    #[test]
    fn report_requires_html_and_undo_rejects_zero() {
        assert!(parse(&["report"]).is_err());
        assert!(parse(&["report", "--html", "o.html"]).is_ok());
        assert!(parse(&["undo", "0"]).is_err());
        assert!(matches!(
            parse(&["undo"]).unwrap().command,
            Some(Cmd::Undo { n: 1 })
        ));
    }

    #[test]
    fn readonly_refuses_writes_with_exit_3_before_any_library_call() {
        for args in [
            &["--readonly", "off", "x"][..],
            &["--readonly", "on", "x"],
            &["--readonly", "edit", "x"],
            &["--readonly", "undo"],
            &["--readonly", "apply", "/dev/null"],
            &["--readonly", "trim"],
            &["--readonly", "trim", "--dry-run"],
            &[
                "--readonly",
                "trim",
                "--yes",
                "--agent-cmd",
                "/nonexistent/agent",
            ],
            &["--readonly", "history", "--prune", "30d"],
            &["--readonly", "verify", "--run"],
            // A target that does not exist proves nothing past the gate ran.
            &[
                "--readonly",
                "--dir",
                "/nonexistent/harn",
                "off",
                "x",
                "--dry-run",
            ],
        ] {
            let err = run(&parse(args).unwrap()).unwrap_err();
            assert_eq!(exit_code_for(&err), EXIT_REFUSED, "{args:?}: {err:#}");
        }
    }

    #[test]
    fn readonly_allows_reads() {
        for args in [
            &["--readonly", "history"][..],
            &["--readonly", "verify", "--capture", "x"],
            &["--readonly", "why", "x"],
        ] {
            let cli = parse(args).unwrap();
            assert_eq!(
                cli.command.as_ref().and_then(write_command),
                None,
                "{args:?}"
            );
        }
    }

    #[test]
    fn exit_codes_follow_the_error_type_through_context() {
        let ambiguous =
            anyhow::Error::from(SelectError::Ambiguous(vec!["a".into()])).context("why");
        assert_eq!(exit_code_for(&ambiguous), EXIT_AMBIGUOUS);
        let refused = anyhow::Error::from(Refused("builtin".into())).context("off");
        assert_eq!(exit_code_for(&refused), EXIT_REFUSED);
        let stale = anyhow::Error::from(Stale { files: vec![] }).context("undo");
        assert_eq!(exit_code_for(&stale), EXIT_STALE);
        assert_eq!(exit_code_for(&SelectError::NotFound("x".into()).into()), 1);
        assert_eq!(exit_code_for(&anyhow!("not implemented: WU-B")), 1);
    }

    #[test]
    fn plan_schema_mismatch_is_refused() {
        let dir = std::env::temp_dir().join(format!("harn-main-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            (
                "wrong-schema.json",
                r#"{"schema":"harn/1","target_dir":"/tmp","ops":[]}"#,
            ),
            ("no-schema.json", r#"{"target_dir":"/tmp","ops":[]}"#),
            ("not-json.json", "{"),
            (
                "unknown-key.json",
                r#"{"schema":"harn-plan/1","target_dir":"/tmp","ops":[{"selector":"x","knbo":1,"reason":"r"}]}"#,
            ),
        ];
        for (name, body) in cases {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            let err = read_plan_request(&path).unwrap_err();
            assert_eq!(exit_code_for(&err), EXIT_REFUSED, "{name}: {err:#}");
        }
        let ok = dir.join("ok.json");
        std::fs::write(
            &ok,
            r#"{"schema":"harn-plan/1","target_dir":"/tmp","ops":[{"selector":"plugin:slack:slack","scope":"local","reason":"unused"}]}"#,
        )
        .unwrap();
        let req = read_plan_request(&ok).unwrap();
        assert_eq!(req.ops[0].scope, Some(Scope::Local));
        assert_eq!(req.ops[0].knob, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn version_line_shape() {
        let v = version_text();
        assert!(v.starts_with(env!("CARGO_PKG_VERSION")), "{v}");
        assert!(v.contains(") rules: claude-code "), "{v}");
    }

    #[test]
    fn parse_age_units() {
        assert_eq!(parse_age("30d"), Ok(Duration::from_secs(30 * 86_400)));
        assert_eq!(parse_age("12h"), Ok(Duration::from_secs(12 * 3_600)));
        assert_eq!(parse_age("2w"), Ok(Duration::from_secs(2 * 604_800)));
        assert!(parse_age("30").is_err());
        assert!(parse_age("d").is_err());
        assert!(parse_age("3y").is_err());
        assert!(parse_age("-1d").is_err());
    }
}
