# 03 — P0 interfaces (Rust)

Status: shipped — 2026-10-13 (P0 landed on `main`; the amendments below are the contract as built). Owned by the drive; a WU
that needs a change reports it, the dispatcher edits here and re-dispatches. Everything is
`serde::{Serialize, Deserialize}` unless noted. Crate name `harn`, edition 2021, `src/lib.rs`
re-exports `model`, `resolve`, `render`, `oracle`, `verify`, `plan`, `writers`, `journal`.

## model.rs (WU-A owns; everyone reads)

```rust
pub const SCHEMA: &str = "harn/1";

#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub enum Agent { ClaudeCode, Codex }     // serde: "claude-code" | "codex"
#[derive(Clone, Copy, PartialEq, Eq)] pub enum Kind { Instruction, Memory, Hook, SkillListing, Skill,
    AgentRoster, SubAgent, Command, OutputStyle, PermissionRule, StatusLine, McpServer, McpTool,
    McpInstructions, Plugin, BuiltinTool, SystemPrompt, EnvBlock, GitBlock, Attribution, Unresolved }
#[derive(Clone, Copy, PartialEq, Eq)] pub enum Mode { Always, Advertised, Deferred, OnDemand, Runtime, None }
#[derive(Clone, Copy, PartialEq, Eq)] pub enum Layer { Managed, User, Project, Local, Plugin, Dynamic, ClaudeAi, Builtin, Unresolved }
#[derive(Clone, Copy, PartialEq, Eq)] pub enum Scope { Local, Project, User }

pub struct Origin { pub layer: Layer, pub path: Option<PathBuf> /* realpath */, pub setting: Option<String> /* "<file>#<json.pointer>" */,
    pub plugin: Option<String> /* "name@marketplace" */, pub why: Vec<String> /* admission chain, top-down */ }

/// Byte cost with evidence tags. `?` = unknown (all None, not_applicable=false); `—` = not_applicable.
pub struct Cost { pub source: Option<u64>, pub predicted: Option<u64>, pub predicted_is_estimate: bool /* ~ */,
    pub observed: Option<u64>, pub not_applicable: bool }

/// freed = known part + named unknowns. `delta_unknown` renders as `Δ?`.
pub struct Freed { pub known: u64, pub delta_unknown: bool, pub is_estimate: bool }

pub enum Knob {
    Setting { file: PathBuf, pointer: String /* JSON pointer */, value: serde_json::Value, scope: Scope, note: Option<String> },
    Env { name: String, value: String, note: Option<String> },           // via settings.env in `scope`
    Quarantine { from: PathBuf, to: PathBuf, scope: Scope },
    None { reason: String },
}

pub struct Item { pub id: String, pub label: String, pub kind: Kind, pub origin: Origin, pub mode: Mode,
    pub cost: Cost, pub knobs: Vec<Knob> /* candidates, narrowest first; [0] = default */,
    pub freed: Option<Freed> /* for knobs[0]; filled by plan::annotate_freed */,
    pub signal: Option<String> /* e.g. "over cap: 146 of 216 descriptions dropped" */,
    pub last_used: Option<String> /* ISO date */, pub children: Vec<Item> }

pub struct Target { pub dir: PathBuf /* canonical */, pub home: PathBuf /* $HOME, overridable in tests */,
    pub env: BTreeMap<String, String>, pub model: Option<String>, pub context_window: Option<u64>,
    pub bytes_per_token: Option<f64> }

pub struct Tree { pub agent: Agent, pub target_dir: PathBuf, pub agent_version: Option<String>,
    pub rules_verified_against: String, pub tool_search: Option<bool>, pub signals: Vec<String>,
    pub items: Vec<Item>, pub predicted_known: u64, pub unknown_rows: u32, pub observed_body: Option<u64>,
    pub warnings: Vec<String> }

pub struct Document { pub schema: String /* SCHEMA */, pub generated_at: String, pub agents: Vec<Tree> }

/// Virtual changes applied while resolving (for freed computation). Resolvers MUST honour it.
pub enum OverlayOp { SetJson { file: PathBuf, pointer: String, value: serde_json::Value }, RemovePath { path: PathBuf } }
pub struct Overlay { pub ops: Vec<OverlayOp> }

pub struct RunContext { pub cwd: PathBuf, pub agent: Agent, pub agent_version: Option<String>, pub model: Option<String>,
    pub launch_opts: Vec<String>, pub settings_fingerprint: Option<String>, pub capture_kind: String, pub captured_at: String }

/// Item selector: exact id, else unique suffix of id, else unique label. Err(Ambiguous(candidates)) / Err(NotFound).
pub fn select<'a>(tree: &'a Tree, selector: &str) -> Result<&'a Item, SelectError>;
pub fn walk(tree: &Tree) -> impl Iterator<Item = &Item>;   // depth-first
```

Item ids: `<agent>/<kind-kebab>/<layer-kebab>/<name-or-realpath>`; files always by realpath.

## resolve/mod.rs (WU-B owns)

```rust
pub trait Resolver { fn agent(&self) -> Agent; fn rules_verified_against(&self) -> &'static str;
    fn resolve(&self, target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree>; }
pub struct ClaudeCode;   // impl Resolver; reads only files; never spawns a process
pub fn for_agent(agent: Agent) -> Box<dyn Resolver>;
```
Claude Code rules = `research/01` + `01-spec.md` §Claude Code coverage. Items with mode Runtime/None
carry `cost.not_applicable = true`. MCP tool schemas and hook stdout = `cost` unknown (`?`).
Token-budgeted listing: `cost.predicted` = sum of entries (source), `signal` set when a capture
later shows overflow (resolver leaves `signal` None; verify fills it). `tool_search` =
`Some(false)` when `ANTHROPIC_BASE_URL` is set to a non-anthropic host, `Some(true)` when unset,
`None` when undecidable.

## plan.rs / writers.rs / journal.rs (WU-E owns)

```rust
pub struct PlannedOp { pub item_id: String, pub knob: Knob, pub file: Option<PathBuf>, pub hash_before: Option<String>, pub reason: Option<String> }
pub struct ActionPlan { pub target_dir: PathBuf, pub agent: Agent, pub ops: Vec<PlannedOp>,
    pub predicted_before: u64, pub predicted_after: u64, pub freed: Freed,
    pub side_effects: Vec<String>, pub warnings: Vec<String>, pub diff: String /* unified, all files */ }
pub fn build_plan(resolver: &dyn Resolver, target: &Target, tree: &Tree, item_id: &str, scope: Option<Scope>) -> anyhow::Result<ActionPlan>;
/// AI-authored plan (schema "harn-plan/1"): {"schema","target_dir","ops":[{"selector","scope"?:"local|project|user","knob"?:usize,"reason"}]}
pub struct PlanRequest { pub schema: String, pub target_dir: PathBuf, pub ops: Vec<PlanOp> }
pub struct PlanOp { pub selector: String, pub scope: Option<Scope>, pub knob: Option<usize>, pub reason: String }
pub fn build_plan_from_request(resolver: &dyn Resolver, target: &Target, tree: &Tree, req: &PlanRequest) -> anyhow::Result<ActionPlan>;  // every selector unique (else Err listing candidates), knob index within item.knobs and not Knob::None, scope never wider than requested; reasons kept in PlannedOp.reason
pub fn annotate_freed(resolver: &dyn Resolver, target: &Target, tree: &mut Tree) -> anyhow::Result<()>;  // fills Item.freed for knobs[0] via overlay re-resolve
// writers: atomic replace (temp + fsync + rename); JSON via serde_json with preserve_order, 2-space
pub fn apply(plan: &ActionPlan, journal: &mut Journal) -> anyhow::Result<JournalEntry>;
pub struct Journal { pub dir: PathBuf /* state dir */ }      // $XDG_STATE_HOME/harn or ~/Library/Application Support/harn
pub struct JournalEntry { pub id: String /* ts-rand */, pub context: RunContext, pub ops: Vec<PlannedOp>,
    pub snapshots: Vec<Snapshot>, pub applied_at: String }
pub struct Snapshot { pub path: PathBuf, pub file: PathBuf /* in journal dir */, pub hash_before: String, pub hash_after: Option<String> }
pub fn undo(journal: &mut Journal, n: usize) -> anyhow::Result<Vec<JournalEntry>>;   // stale (hash mismatch) → error listing the files, nothing written
pub fn history(journal: &Journal) -> anyhow::Result<Vec<JournalEntry>>;
pub fn edit(item: &Item, journal: &mut Journal, editor: &str) -> anyhow::Result<JournalEntry>;  // snapshot → $EDITOR → diff → entry; parse-check warning for json/toml/frontmatter
```
Quarantine root: `<discovery-root-parent>/harn-off/<kind>/<name>` (`.claude/harn-off/…` or `~/.claude/harn-off/…`).

## oracle/ + verify.rs (WU-D owns)

```rust
pub struct Capture { pub context: RunContext, pub body_bytes: u64, pub regions: Vec<Region> /* top-level request regions with bytes */,
    pub tree: Tree /* observed items, same id rules */ }
pub fn run_claude(target: &Target, claude_bin: &Path) -> anyhow::Result<Capture>;   // OTEL_LOG_RAW_API_BODIES=file:<tmp> + --output-format stream-json --verbose -p "hi" --max-turns 1 --disallowedTools Edit,Write,NotebookEdit,Bash
pub fn read_otel_dir(dir: &Path, target: &Target) -> anyhow::Result<Capture>;
pub fn read_llmux_record(line: &str, target: &Target) -> anyhow::Result<Capture>;   // RunContext derived from the body; Err if underivable
pub fn parse_request_body(bytes: &[u8]) -> anyhow::Result<(Vec<Region>, Tree)>;   // tools / system / messages[0] blocks / messages[1] sections, named like 01-spec §Problem
pub fn parse_init_event(line: &str) -> anyhow::Result<InitEvent>;   // tools, mcp_servers{name,status,source}, skills, agents, plugins{name,path,source,version}, memory_paths, claude_code_version, model
pub enum RowVerdict { Seen, PredictedNotSeen, SeenNotPredicted, ContentMismatch { predicted: u64, observed: u64 } }
pub struct VerifyReport { pub context: RunContext, pub rows: Vec<(String /* id */, RowVerdict)>, pub passed: bool, pub promoted: u32 }
pub fn verify(static_tree: &mut Tree, capture: &Capture) -> VerifyReport;   // projection: Runtime/None skipped; capture-only children attach under `?` parents (never SeenNotPredicted); presence gate on static ids; content conformance on reproducible static items; promotes predicted→observed on matched rows; fills Tree.observed_body and over-cap signal
```

## render/ (WU-C owns)

```rust
pub struct RenderOpts { pub unicode: bool, pub color: bool, pub width: Option<usize> }
pub fn table(tree: &Tree, opts: &RenderOpts) -> String;   // header lines (totals, tool search, signals, drift) + rows sorted by freed.known desc, Runtime/None rows in a trailing block
pub fn why(tree: &Tree, item: &Item, plans: &[ActionPlan]) -> String;
pub fn json(doc: &Document) -> String;
pub fn verify_report(report: &VerifyReport) -> String;
pub fn brief(doc: &Document) -> String;        // LLM-oriented; ends with the harn-plan/1 format and an example op
pub fn html(doc: &Document) -> String;         // one self-contained read-only page, inline CSS, no writing script, works at 400 px
```

## main.rs (WU-A owns; wires everything; later WUs do not edit it)

`harn [DIR] [--dir D] [--agent A] [--json] [--readonly] [--no-unicode] [--color auto|always|never]`;
subcommands `why <sel>`, `path <sel>`, `off <sel> [--scope] [--dry-run]`, `on <sel>`, `edit <sel>`,
`undo [N]`, `history [--prune AGE]`, `verify [--capture P | --run] [--agent]`, `brief [DIR]`,
`apply <plan.json> [--dry-run]`, `report [DIR] --html <out>`, `completions <shell>`. No `tui`.
`DIR` and `--dir` together = clap conflict error. Exit codes: 0 ok · 1 verify diff · 2 ambiguous
selector · 3 refused (readonly / builtin / no knob) · 4 stale journal. `harn --version` prints
`harn <ver> (<channel> <rev12>) rules: claude-code <VERIFIED_AGAINST>`.

## Fixtures (`tests/fixtures/<name>/`)

`home/` (fake `~`: `.claude/settings.json`, `.claude.json`, `.claude/CLAUDE.md`, `.claude/skills/*`,
`.claude/agents/*`, `.claude/plugins/{installed_plugins.json,cache/...}`, `.claude/projects/<slug>/memory/MEMORY.md`),
`project/` (the target dir: `CLAUDE.md`, `rules/`, `.claude/{settings.json,settings.local.json,skills,agents}`, `.mcp.json`),
`expected/tree.json` (WU-B), `capture/` (WU-D: `*.request.json`, `index.jsonl`, `stream.jsonl`).
Target for tests: `Target { dir: project, home: fixture home, env: {} }`.

## Amendments after WU-A (2026-10-13) — the contract as compiled

- `model.rs` additionally has: `item_id(agent, kind, layer, name) -> String` (everyone builds ids
  with it), `now_iso8601()`, `SelectError { Ambiguous(Vec<String>), NotFound(String) }`,
  `Refused(String)` (→ exit 3) and `Stale { files }` (→ exit 4) error types that library code
  returns inside `anyhow` and `main` finds anywhere in the chain; `as_str()`/`Display`/`ALL` on
  the enums. Serialization: `Layer::ClaudeAi` = `"claudeai"` (the init-event provenance value),
  `Kind::SubAgent` = `"sub-agent"`, `Knob` tagged `{"type": …}`, `OverlayOp` tagged `{"op": …}`.
- `Knob::Env` has `scope: Scope` (the settings file whose `env` map receives it).
- `plan::annotate_freed` is an `Ok(())` no-op until WU-E lands (`freed: None` is honest), so
  A1/A2 run end-to-end once WU-B and WU-C exist. `plan::PLAN_SCHEMA = "harn-plan/1"`;
  `PlanRequest`/`PlanOp` reject unknown keys.
- `journal`: `Journal::state_dir() -> anyhow::Result<PathBuf>`; `JournalEntry.diff: Option<String>`
  (unified diff of every changed file; `off`/`apply`/`edit` print it); new `on(journal, target,
  selector)` (matches journaled ids, since an item that is off is no longer in the tree) and
  `prune(journal, Duration) -> Vec<JournalEntry>`.
- `oracle`: `read_capture(path, target)` decides OTEL dir / llmux line / trace bundle (WU-D owns
  the policy); `Region { name, bytes }`; `InitEvent { tools, mcp_servers: Vec<InitMcpServer{name,
  status, source}>, skills, agents, plugins: Vec<InitPlugin{name,path,source,version}>,
  memory_paths: BTreeMap<String,String>, claude_code_version, model }`. `claude.rs` = `run_claude`,
  `parse_request_body`, `parse_init_event`; `capture.rs` = the readers; all re-exported.
- `render`: also `plan(&ActionPlan) -> String` and `history(&[JournalEntry]) -> String`; `main`
  does no formatting.
- `resolve::default_agents()` = `[ClaudeCode]` in P0; `for_agent(Codex)` is a stub.
- Deferred to WU-F (integration): selector 4th tier (unique substring, so `plugin:slack` matches
  `…/plugin:slack:slack`), `--assume-bytes-per-token`, exit-code collision notes (clap usage = 2 =
  ambiguous selector; generic error = 1 = verify diff).

## Amendments after WU-D (2026-10-13) — ids and bytes the resolver must match

- `Capture.init: Option<InitEvent>`; `InitEvent.cwd`, `.session_id`; exports `oracle::{find_init_event,
  settings_fingerprint, InitMcpServer, InitPlugin}`, `oracle::claude::{names, NAME_ONLY, LAUNCH_ARGS}`,
  `oracle::capture::LLMUX_SCAN_LIMIT`, `Region::{SEP, group}`, `verify::{CONTENT_TOLERANCE, gated, conformance}`.
- **Fixed ids (Layer::Builtin unless noted)** — import `oracle::claude::names`: SystemPrompt `system`
  (children `system/billing-header`, `system/identity`, `system/harness-rules`); SkillListing `skills`;
  AgentRoster `agents`; EnvBlock `environment` and `date`; GitBlock `gitStatus`; Attribution
  `userEmail`; MCP tools `mcp-tool/<layer>/<server>/<tool>`; built-in tools by the body's names
  (`Agent`, not init's `Task`); plugins by `name@marketplace`; Memory = Layer::User; Instruction and
  Memory items also join by realpath with the layer ignored. Listing names drop a display alias
  (`x (alias)` → `x`).
- **Predicted bytes for content conformance (tolerance 2 B)**: instruction/memory section =
  `len("Contents of <realpath> (<desc>):") + 3 + file bytes`, where **YAML frontmatter plus the
  blank line after it is stripped** before embedding (PRINCIPLES.md: 4,557 → 4,261). Descriptions:
  "user's private global instructions for all projects" (user) · "project instructions, checked into
  the codebase" (project) · "…not checked in" (local) · "user's auto-memory, persists across
  conversations" (memory). Listing and roster entries include their trailing newline; a multi-line
  entry runs to the next `- name`; roster lines carry ` (Tools: …)`. The skill listing also lists
  slash commands (verify joins a listing entry to a static Skill or Command of that name). Skills
  with `disable-model-invocation: true` never appear → mode OnDemand (not gated).
- **cwd evidence** in a capture: Environment "Primary working directory" → init `cwd` → deepest
  project CLAUDE.md dir; gitStatus has no cwd line (spec §Source of truth corrected in v3.1).
- Hook stdout is never promoted (no join key between a config hook and its stdout); hooks stay `?`
  and are not gated. Version mismatch tree↔capture = warning; cwd mismatch = `passed=false`.
- Spec §Problem byte corrections (v3.1): SessionStart hook 3,456 B (not 3,444 chars), MEMORY.md
  23,429 + 18 B closing tag as wrapper, `messages[0]` block 1 = wrapper 107 + userEmail 276 +
  gitStatus 3,166.

## Amendments after R3 (user correction 2026-10-13, 원문 3)

- `Cmd::Trim { dir, dry_run: bool, yes: bool, agent_cmd: Option<String> }` — `src/trim.rs`
  `pub fn trim(resolver, target, tree, opts: TrimOpts) -> Result<TrimOutcome>` where `TrimOpts`
  carries `agent_cmd: Vec<String>` (default `["claude", "-p", "--output-format", "json"]`; the
  prompt is appended as the last argument or fed on stdin when the command ends with `-`),
  `dry_run`, `yes`, and a `confirm: Box<dyn FnMut(&ActionPlan) -> bool>`; `TrimOutcome::{DryRun(ActionPlan),
  Declined(ActionPlan), Applied(JournalEntry)}`. Exit codes: 3 for a rejected plan (same as
  `apply`), 1 for a missing agent binary or a reply without a plan.
- `render::overview(tree, opts) -> String`; `RenderOpts.width` is the detected terminal width
  (`terminal_size` on a TTY, `COLUMNS` > 0 otherwise, else 120). `harn --all` = `render::table`.
- `src/tui/`: `App::new(target, trees, opts) -> App`, `App::handle_key(&mut self, KeyEvent)`, `App::draw(&self, &mut Frame)`, `tui::run(target, trees, opts) -> Result<()>` (raw mode + alternate screen, restored on every exit path incl. panic). Bare `harn` dispatches to `tui::run` when stdout is a terminal and neither `--json` nor `--table` is set.
