# harn — Spec

`harn <dir>` lists the effective harness an AI coding agent loads when it is started in `<dir>`
— every instruction file, memory, hook, skill, subagent, MCP server, plugin and built-in the agent
will pick up — as one tree per agent, each row with where it came from, whether it costs bytes in
the first model request (and how many), and the agent's official switch that turns it off. It
turns items off, edits them, and undoes both, through a journal. Claude Code first, Codex CLI
second. One Rust binary for macOS and Linux; CLI with `--json` at the base, a TUI on the same model.

Status: **planned** — v2.9, 2026-10-13. Rewritten from scratch on [`research/`](research/) (00 =
the user's statement as a PRD, 01 = Claude Code loading rules, 02 = Codex loading rules, 03 = prior
art and UX, 04 = local measurements), then revised on trinity rounds 1–4 (see [§Revision
log](#revision-log)). The user's verbatim statement and the name decision are fixed in
[`prd-rewrite/ssot.md`](prd-rewrite/ssot.md). Reviewed by the three-engine panel (grok-4.7,
gpt-6-astra, fable): unanimous APPROVE on this text at round 5 of 5, 2026-10-13, no blocking
items; round-by-round changes are in the [§Revision log](#revision-log). This document is a
target, not a description of shipped behavior; it moves to **shipped** only when
[§Acceptance](#acceptance-scenarios) P0 is green on both machines.

## Problem

A fresh Claude Code 2.1.296 session in `~/2lab.ai/zbrain` sends a first request of 332,548 bytes
before reading the user's instruction, which is 2 of those bytes (`hi`) (`research/04` §E, measured
2026-10-13 with Claude Code's own raw-body capture). The bytes come from the four places the user
named — the agent's built-in prompt and tools, the `~/.claude` harness, the working folder's
harness, then the instruction — and Codex assembles its own request from `~/.codex` and `AGENTS.md`
the same way. Nothing today shows the effective harness for a directory **without starting a
session**, with the **file or setting that produced each item**, the **cost** it adds to the
request, and the **switch** that removes it. Claude Code's `/context` is the closest thing
(`research/04` §A): an estimate, inside a live session, no hooks, no paths, no action. Trimming the
harness today means grepping six config layers and guessing.

The measured composition of that request (text bytes of each region, 2026-10-13, the same machine):

| region | bytes | what |
|---|---|---|
| `tools` (compact JSON, as on the wire) | 210,679 | 26 built-in tools 68,411 · MCP `plugin:slack` 27 tools 77,536 · MCP `plugin:atlassian` 21 tools 64,732 |
| `messages[1]` system text | 57,202 | SessionStart hook stdout 3,444 · Environment 841 · agent roster (31) 17,050 · MCP instructions 2,596 · skill list (216 entries, 70 with descriptions) 31,212 · date 29 · Advisor 2,018 |
| `messages[0]` block 0 | 44,936 | `~/.claude/CLAUDE.md` 738 · `zbrain/CLAUDE.md` 6,835 · `@rules/DEV.md` 9,443 · `@rules/PRINCIPLES.md` 4,261 · auto-memory `MEMORY.md` 23,447 |
| `system` | 6,137 | billing header 74 · identity 62 · harness rules 6,001 |
| `messages[0]` block 1 | 3,549 | userEmail + gitStatus |
| `messages[0]` block 2 | 2 | the instruction (`hi`) |
| `messages[1]` `tool_addition` | 122 | the Advisor tool reference |
| attributed sum | 322,627 | |
| request parameters | 7,829 | `model`, `betas`, `metadata`, `max_tokens`, `thinking`, `context_management`, `safeguards`, `output_config`, `stream` — measured as compact JSON |
| JSON framing + escapes of the text regions | 2,092 | the rest of the 332,548-byte body — measured as body − attributed − parameters; never attributed to an item |

Every MCP schema was sent up front because the session ran behind a non-first-party
`ANTHROPIC_BASE_URL` (the local llmux proxy), which turns Claude Code's tool search off
(`research/01` §7, docs). Behind the first-party endpoint the docs say the same 48 tools are
deferred stubs `[docs — stub size not measured; Open question 1]`. The same item can therefore cost
142 KB in one configuration and an unmeasured, smaller amount in another: a size without its load
mode and its configuration misleads.

## Prior art, in one paragraph

Claude Code's `/context`, `/skills`, `/skill-doctor` and Codex's `codex debug prompt-input` report;
none edits, none runs without a session (Codex's dump makes no model call but still spawns MCP
servers). migsilva89/loadout (macOS GUI, 98★) manages what assistants load and quarantines skills
into `skills-off/`, but shows no sizes, only per-project ownership, and has no Linux, CLI or
`--json`. ccmcp (Go) has the right default screen — the effective set for this directory, origin
badges, undoable per-project overrides — but covers MCP and plugins only. cclint has the right
vocabulary — load classes *always / advertised / on-demand*, exact counts only when measured,
otherwise `~` — and checks itself against the real `claude` binary. harn takes the default screen
from ccmcp, the vocabulary and the self-check from cclint, the size-sorted tree with
confirm-by-default from ncdu, and adds what none of them do: **the effective list with the cost of
removing each item computed as a before/after plan**, and **verification against the agent's own
request**. Details and URLs: `research/03`.

## Goals

1. **List, for one directory and one agent, the effective harness** — every item the agent will
   pick up at session start from its real discovery rules — whether or not it costs bytes in the
   first request: instruction files and imports, auto-memory, settings-driven blocks, hooks (all
   events, including those that fire later or print nothing), skills, subagents, commands, output
   style, permission rules, status line, MCP servers, plugins and what each contributes, built-in
   tools and the built-in prompt. The static resolver does not start the agent and does not spawn
   servers.
2. **Each row carries: origin, mode, cost with its evidence level, and knob.** Origin = kind + layer
   + resolved real path or setting key. Mode = `always` / `advertised` / `deferred` / `on-demand` /
   `runtime` (fires on a later event) / `none` (never reaches the model). Cost = `source` bytes (the
   file or definition on disk), `predicted` request bytes (what the resolver expects in the first
   request after the agent's own caps and budgets), and `observed` request bytes (seen in a bound
   capture); each shown only with its tag, `?` when the resolver cannot know (a hook's stdout, an
   MCP server's schemas before it has run). Knob = the agent's official way to turn the item off
   without deleting, in the narrowest scope that works; `none (built-in)` when there is none.
3. **The number that drives decisions is the freed bytes of a concrete action**, not the size of a
   row. `freed(action)` = `predicted(before) − predicted(after)`, computed by re-resolving the tree
   with the action applied as an overlay. This counts a plugin's skills, agents and hooks together
   with its tools, counts shared items once, applies the agent's caps where the cap's unit allows
   it, and keeps the parts it cannot derive explicit: `—` for items with no knob, `?` for a cost or
   a delta that is unknown (a token-budgeted listing re-rendered after a removal is not derivable
   from a before-only capture). A row's freed value is therefore a sum of known parts plus named
   unknowns, e.g. `77.5 KB + Δ?`, never a single number that hides one. The default sort key is
   the known part of each row's default action. Bytes are not the only loss a harness suffers:
   a token-budgeted listing that is over its cap drops descriptions, and an advertised skill
   without its description is invisible to description-based routing. That loss has no byte
   value and would sink to the bottom of a byte sort, so every over-cap container carries a
   **signal** (`over cap: N of M descriptions dropped`) shown in the header and as a row badge.
   `N` is the observed count of dropped descriptions, not the number of entries to change — the
   budget is in tokens, so no byte count says how many entries must leave. The signal's default
   action is therefore **show candidates**: the entries whose descriptions were dropped, each with
   the knob its origin supports (`skillOverrides` for user and project skills; a plugin skill only
   through its plugin's `enabledPlugins`, shown with that plugin's other contributions as side
   effects) and its last-used date as a sorting aid, never as a selection. The user picks; an
   `ActionPlan` is built from the picked items; whether descriptions return is an expected effect
   until a post-apply capture shows it. On this machine the signal reads `146 of 216`.
4. **Change the harness reversibly, through the agent's own switch, with an explicit target.**
   Every command takes `--dir` (default: cwd) and prints the canonical target directory in its plan,
   diff and journal entry. Mechanism order: official knob in the narrowest scope (project-local
   before user; widening the scope is an explicit `--scope`, never a silent fallback) → quarantine
   move outside every discovery root with a manifest → never `rm`. Every write is preceded by a
   snapshot into harn's own state dir and recorded in a journal; `harn undo` reverses it; a target
   changed since the write is reported stale, not forced. `harn edit` opens the real file in
   `$EDITOR` between a snapshot and a diff, so content edits ride the same journal.
5. **Verify against the agent's own ground truth, bound to a run context.** A capture is usable
   only if its context (canonical cwd, agent and version, model, launch options, settings
   fingerprint, time) matches the tree being verified. `harn verify --capture <path>` reads an
   existing capture and never writes; `harn verify --run` starts the agent once under its official
   capture (Claude Code: `OTEL_LOG_RAW_API_BODIES=file:<dir>` request body + the `stream-json`
   `system/init` provenance event; Codex: `codex debug prompt-input` + `codex mcp list --json` +
   `codex plugin list --json`), states before running that it will execute hooks and MCP servers and
   may cost a model request, and is refused under `--readonly`. Matching is done on the **projection each
   oracle can observe**: items with mode `runtime` or `none` are expected to be absent from a
   request and are never counted as missing; capture-only children under a `?` parent (an MCP
   server's tools, a hook's stdout) are attached as `observed` and never counted as
   `seen-not-predicted` — the diff applies to top-level items and statically known children.
   Within that projection the gate has two parts: **presence by item id** for statically
   discoverable ids (A1's set), and **content conformance** for static contributions that are
   reproducible in the same context (an instruction file's text, a skill's listing entry, the
   memory block after its cap) — a size or content mismatch there is a resolver defect and fails
   verify. Values that vary per run (date, git status, hook stdout, MCP runtime text) are
   observations and replace `predicted` with `observed` without failing. Only
   observed rows and regions are promoted; a Codex verify promotes instructions, skills and blocks
   and says that tools and hook output are outside Codex's dump.
6. **Same model everywhere.** `harn <dir> --json` (schema `harn/1`), the TUI, `harn why`, `harn
   path`, `harn off/on/edit/undo/history/verify` read and write one data model.
7. **Runs on macOS (arm64, x86_64) and Linux (x86_64, arm64), including CachyOS**, with no runtime
   dependency beyond the agents it inspects. `harn --version` prints the agent versions the
   resolver's rules were verified against; a newer installed agent puts a drift warning in every
   header until `verify` has passed.

## Non-goals

- A GUI. The user said "TUI 혹은 GUI (필요시)"; nothing in the research makes a GUI necessary, and
  migsilva89/loadout already occupies that niche. Revisit only on evidence.
- An in-tool editor. Content editing is `harn edit` = snapshot → `$EDITOR` on the real path → diff
  → journal; `harn path <item>` prints the resolved path for anything else (`grep`, `rm`, your own
  tools). harn does not delete files itself.
- Editing the agent's built-in prompt or built-in tool definitions. harn **shows** them with
  `knob: none (built-in)`. The user's premise that llmux can edit them is recorded under
  [§Tensions](#tensions).
- Generating or syncing config files across agents (ruler's job), scaffolding templates, or any
  "add more harness" direction.
- Token counts in the first release. All costs are bytes. No official Claude tokenizer exists;
  a labelled `~tok` column is a later, optional feature, never a sort key.
- Usage analytics beyond one column ("last used N sessions ago", from the agent's own session
  logs) — ccusage's job.
- Agents other than Claude Code and Codex in this version. The model is agent-neutral, so a third
  resolver is additive.

## The model

```
Agent     claude-code | codex          one tree, one total, one table per agent — never summed
Item      id, label, kind, origin, mode, cost, knob, children[]
kind      instruction | memory | hook | skill-listing | agent-roster | command | output-style |
          permission-rule | status-line | mcp-server | mcp-tool | mcp-instructions | plugin |
          builtin-tool | system-prompt | env-block | git-block | attribution | unresolved
mode      always      — in every first request
          advertised  — name (+ description) only; the body loads on use (skills, agents)
          deferred    — stub now, full definition on demand (MCP tools under tool search)
          on-demand   — not in the first request (nested CLAUDE.md, skill bodies, rules with paths:)
          runtime     — fires on a later event (PreToolUse hooks, …); cost in the first request = 0
          none        — never reaches the model (statusLine, permission rules)
origin    layer ∈ managed | user | project | local | plugin@marketplace | dynamic (CLI flag) |
                  claudeai (remote connector) | builtin | unresolved
          path   — realpath (symlinks resolved; a shared copy is flagged) or setting file + key
          why    — the admission chain (settings key → plugin manifest → file), nix-tree style
cost      source: bytes on disk | predicted: bytes in the first request after caps | ~: predicted
          through a modelled cap or a declared ratio, shown with its assumption | observed: bytes
          in a bound capture | ? (unknown until a capture) | — (no first-request cost)
freed     known part + named unknowns; Δ? = the delta of a token-budgeted re-render that no
          before-only capture can give (observed from a before/after pair, ~ under a declared ratio)
knob      {file, key, value, scope}  the official off switch, narrowest scope that wins
          | quarantine {from, to}     only when no knob exists
          | none {reason}             built-in / undocumented
ActionPlan  target_dir, items[], mechanism per item, scope, file hashes at read time,
            predicted(before), predicted(after), freed, side effects (what else leaves or appears),
            warnings (git-tracked file, agent process running, scope wider than local)
RunContext  canonical cwd, agent + version, model, launch options/profile, settings fingerprint,
            capture kind + time — attached to every capture and every journal entry
```

Tags are never dropped: a table header says which tag its total uses, and a row shows `predicted`
until a bound capture turns it into `observed`.

## Source of truth

- **Default = static resolver, no session, no spawned process.** It implements the discovery rules
  in `research/01` (Claude Code) and `research/02` (Codex) and reads only files and settings.
  Target: well under a second for a directory like zbrain (acceptance A2). It drifts when an agent
  changes its rules; the drift warning (Goal 7) and `verify` are the recovery.
- **What static cannot know is shown as `?`**: a hook's stdout, an MCP server's tool schemas and
  instructions before the server has run, claude.ai connectors, anything the agent fetches. A
  bound capture fills them in.
- **Caps are evaluated by their unit.** Byte-denominated caps (auto-memory 25 KB / 200 lines,
  Codex `project_doc_max_bytes` 32 KiB) are applied statically. Token-denominated caps (Claude
  Code's skill listing at 1 % of the window, Codex skills at 2 % / 10k tokens, Codex hook context
  at 2,500 tokens) cannot be decided from bytes: statically the cap state is `?`; with
  `--assume-bytes-per-token N` it becomes `~` with that assumption printed; a bound capture settles
  the **state** as `observed` — the listing is at cap when entries appear name-only although their
  `SKILL.md` carries a description (the documented overflow behavior). On this machine the
  2026-10-13 capture shows 146 of 216 entries name-only with descriptions on disk, so the zbrain
  listing is `at cap [observed]`. The cap state does **not** give the delta of a change: which
  descriptions return after an entry leaves, at what byte length, and whether the listing is still
  at cap are properties of the re-rendered listing, which a before-only capture does not contain.
  The listing delta of any action is therefore `Δ?` until (a) a before/after pair of captures
  exists for that action (`observed` delta), or (b) the user supplies a bytes-per-token assumption
  (`~` delta). The same rule applies to every token-budgeted container.
- **`harn verify` = the agent's own oracles**, bound to a `RunContext`. Claude Code: one headless
  run with `OTEL_LOG_RAW_API_BODIES=file:<tmp>` (full request body + `index.jsonl`) and
  `--output-format stream-json --verbose` (`system/init` with `tools`, `mcp_servers[{name,status,
  source}]`, `skills`, `agents`, `plugins[{name,path,source,version}]`, `memory_paths`) — both
  verified on this machine 2026-10-13 (`research/04` §E). Codex: `codex -C <dir> debug
  prompt-input` + `codex debug models` + `codex mcp list --json` + `codex plugin list --json`;
  tools and hook output are in no Codex dump, so a Codex verify is partial by construction and says
  so (`research/02` §8).
- **Captures from elsewhere.** `harn verify --capture <file-or-dir>` accepts an OTEL raw-body
  directory, an llmux `raw-io.jsonl` record, or a `CODEX_ROLLOUT_TRACE_ROOT` bundle, and uses a
  record only when its `RunContext` matches the target (same canonical cwd, agent version, model);
  it refuses with the mismatch otherwise. "The newest llmux session" is never a selection rule.
  For a record that carries no context of its own (an llmux line), the `RunContext` is derived from
  the request body itself — the `gitStatus` block's cwd line, the `Contents of <path>` headers of
  the instruction files, the `cc_version` billing header, `model` — and the record is refused when
  those are absent.
- **Trade-off, stated once:** static is fast, session-free and drifts; oracles are ground truth and
  cost one request (Claude Code) or a process spawn with side effects (Codex). The spec keeps both
  and labels which one produced every number.

## Interaction model

### P0 — CLI (ships first; Claude Code chain complete)

Every command takes the target directory as `--dir <path>`; the listing commands — `harn [DIR]`,
`harn --json [DIR]`, `harn tui [DIR]` — also accept it as the positional `[DIR]` the user's
statement uses (`harn ~/2lab.ai/zbrain`). Both normalize to the
same canonical target; giving both is a usage error. Default: cwd. `--agent claude-code|codex`
selects one agent (default: all, one table each).

| command | does | output |
|---|---|---|
| `harn [DIR]` / `harn --dir D` | the effective tree, one table per agent, sorted by the freed bytes of each row's default action | table: freed · predicted · share · item · origin · mode · default action |
| `harn --json` | the full model | `{"schema":"harn/1", "agents":[…]}` |
| `harn why <item>` | admission chain, realpath, knob candidates by scope, the freed bytes of each | text |
| `harn path <item>` | the resolved realpath, one line | path |
| `harn off <item> [--scope local\|project\|user] [--dry-run]` | build an `ActionPlan`, print the diff + freed delta + side effects + warnings, then apply the official knob in the given scope (default `local`); quarantine only when no knob exists and only in the same scope; a wider scope is never chosen silently | plan + journal id |
| `harn on <item>` / `harn undo [N]` / `harn history [--prune <age>]` | reverse journal ops; stale targets reported, not forced; `--prune` is the only way journal entries are ever deleted | text |
| `harn edit <item>` | snapshot → `$EDITOR <realpath>` → diff → journal entry; a JSON/TOML/frontmatter file that no longer parses is journaled and warned about, not reverted; refused for `builtin`/`managed` items | diff + journal id |
| `harn verify --capture <path>` | read-only diff of the tree against a bound capture | per-row `seen` / `predicted-not-seen` / `seen-not-predicted` / `content-mismatch`; exit 1 on any presence diff or content-conformance mismatch within the projection |
| `harn verify --run` | runs the agent once under its official capture after printing what it will execute; refused under `--readonly` | same |
| `harn completions <shell>` | clap completions | — |

Item selectors: a full id or any unique suffix (`plugin:slack`, `rules/DEV.md`); an ambiguous
selector lists the candidates and exits 2. Flags everywhere: `--readonly` (refuses every write and
every active verify), `--no-unicode`, `--color`.

### P0.5 — Codex resolver (same commands; acceptance A7–A8)

### P1 — TUI (`harn tui [DIR]` or `harn -i`; `harn [DIR]` always prints the table)

One screen per agent as tabs, the same table, a detail/why pane, a staged-plan pane with the diff
and freed delta, `?` key overlay and a context key bar; disable = stage + apply through the same
`ActionPlan`; `--readonly` greys every write key. Key choices follow `research/03` §B (ncdu, yazi,
lazygit, k9s) and are fixed in the P1 drive, not here.

### First screen — zbrain on the author's machine

A mockup: every number is an `observed` byte count from the 2026-10-13 capture (`research/04`),
drawn into the table layout: the `observed` column and the header are measured, the `freed`
column is **predicted** from those measurements by the ActionPlan rule and carries `Δ?` where a
token-budgeted re-render is involved. `share` = the row's observed bytes over the observed body
(332,548 B). Rows that bundle side effects (CLAUDE.md with its `@imports`, atlassian with its
instructions, a plugin with the listing entries that leave with it) repeat bytes or unknowns that
other rows also carry — the slack row's `+Δ?` and the listing row's `Δ?` are the same unknown, not
two — so `freed` does not sum to the body; only the attributed partition in §Problem does. A
real static first run has no capture: its header reads `predicted <sum of known rows> + ? (N rows
unknown)`, the MCP schema rows and the hook row show `?`, and `share` is computed over the known
sum only.

```
harn ~/2lab.ai/zbrain                                     claude-code 2.1.296 (rules verified 2.1.296)
first request: observed 332.5 KB (capture 2026-10-13) = attributed 322.6 + parameters 7.8 + framing 2.1   sort: freed ▼
tool search: OFF (ANTHROPIC_BASE_URL is non-first-party) → 48 MCP schemas eager, 142.3 KB
    knob: env ENABLE_TOOL_SEARCH=true  [docs — effect not measured; verify after]       freed: ?
skill listing: OVER CAP — 146 of 216 descriptions dropped (observed); those skills cannot be routed by description
    default action: show candidates (per-item knob: skillOverrides for 85 user/project, enabledPlugins for 131 plugin)  Δ?

  freed(pred) observed share  item                              origin                    mode        default action
  77.5 KB+Δ? 77.5 KB   23.3%  mcp plugin:slack · 27 tools       plugin slack@official     always      enabledPlugins → false (local) · its 8 skills leave the listing (at cap; re-render Δ unknown)
  67.3 KB    64.7 KB   19.5%  mcp plugin:atlassian · 21 tools   plugin atlassian@official always      enabledPlugins → false (local) · +2.6 KB instructions
  23.4 KB    23.4 KB    7.1%  MEMORY.md · 150 lines / 25 KB cap ~/.claude/projects/…/memory always      autoMemoryEnabled → false
  20.5 KB     6.8 KB    2.1%  CLAUDE.md (+ 2 @imports)          project                   always      claudeMdExcludes · DEV.md 9.4 KB + PRINCIPLES.md 4.3 KB leave with it
   9.4 KB     9.4 KB    2.8%  rules/DEV.md                      project @import           always      claudeMdExcludes
   4.3 KB     4.3 KB    1.3%  rules/PRINCIPLES.md               project @import           always      claudeMdExcludes
   3.4 KB     3.4 KB    1.0%  SessionStart hook stdout          plugin superpowers@official always     enabledPlugins → false (local)
  ~3.3 KB     3.5 KB    1.1%  gitStatus + userEmail             builtin                   always      includeGitInstructions → false (git part only; userEmail has no switch)
   2.0 KB     2.0 KB    0.6%  Advisor tool text                 builtin                   always      CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1
   0.7 KB     0.7 KB    0.2%  ~/.claude/CLAUDE.md               user                      always      claudeMdExcludes
      ?      17.1 KB    5.1%  agent roster · 31                 project 10 · user 2 · plugins 16 · builtin 3   advertised   per plugin: enabledPlugins; per agent: permissions.deny Agent(x) [roster effect unmeasured]
      Δ?    31.2 KB    9.4%  skill listing · 216 · OVER CAP[i] user 22 · project 63 · plugins 131   advertised   show candidates → pick → ActionPlan per supported knob; descriptions return = expected until a capture confirms
  partial    68.4 KB   20.6%  built-in tools · 26               builtin                   always      --disallowedTools <tool> (per tool; which tools are irremovable is undocumented → ?)
      —       6.1 KB    1.8%  system prompt · 3 blocks          builtin                   always      none (built-in)
      —       0.0 KB         your instruction (2 B)             you                       —           —
  runtime / no first-request cost:
      —          —            hooks · 10 other events           ~/.claude/settings.json   runtime     disableAllHooks / per-event entry
      —          —            permissions.allow · 10 rules      .claude/settings.local.json none      —
      —          —            statusLine                        ~/.claude/settings.json   none        —
  [i] at cap [observed 2026-10-13]: 146 of 216 entries are name-only although their SKILL.md has a description
      (documented overflow behavior). The state is observed; the delta of removing an entry is not — `Δ?` until a
      before/after pair exists or a bytes-per-token assumption is given. A static run without a capture shows `?` for both.
```

### Detail pane — `harn why plugin:slack`

```
plugin:slack:slack   (claude-code/mcp-server/plugin/plugin:slack:slack)        target: ~/2lab.ai/zbrain
  27 MCP tools · observed 77,536 B schemas (capture 2026-10-13) · 0 B instructions · mode always (tool search off)
  why loaded
    ~/.claude/settings.json        enabledPlugins["slack@claude-plugins-official"] = true
    → ~/.claude/plugins/installed_plugins.json   slack@claude-plugins-official 1.4.1
    → ~/.claude/plugins/cache/claude-plugins-official/slack/1.4.1/.mcp.json   "slack" (stdio)
    also contributes: 8 skills (advertised, 1.1 KB in the listing) · 0 agents · 0 hooks
  last used   3 of the last 20 sessions in this directory (session jsonl, 14 d)
  actions                                                                    freed (predicted)
    [1] .claude/settings.local.json   enabledPlugins[…] = false   this project   ← default   77.5 KB schemas + listing Δ? (8 entries leave a listing at cap)
    [2] ~/.claude/settings.json       enabledPlugins[…] = false   every project  (--scope user) 77.5 KB schemas + listing Δ?
    [3] ~/.claude.json projects[cwd].disabledMcpServers += "plugin:slack:slack"  server only   77.5 KB (listing unchanged)
        the 8 skills stay advertised
  warnings  .claude/settings.local.json is git-ignored here (no tracked file changes)
            [3] edits ~/.claude.json, which Claude Code also writes — apply with no claude running
```

## Claude Code coverage (condensed from `research/01`; every row has a doc URL there)

| item | discovery | lands | mode | official off switch |
|---|---|---|---|---|
| managed CLAUDE.md | `/Library/Application Support/ClaudeCode/CLAUDE.md`, `/etc/claude-code/CLAUDE.md` | user msg | always | none (admin) |
| `~/.claude/CLAUDE.md`, `~/.claude/rules/**` | user | user msg | always | `claudeMdExcludes` |
| `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md` | cwd and every ancestor, root → cwd; `@path` imports ≤ 4 hops; excluding a file drops its imports | user msg | always | `claudeMdExcludes` · `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` |
| `.claude/rules/*.md` | recursive; with `paths:` → on-demand | user msg | always / on-demand | `claudeMdExcludes` |
| subdirectory CLAUDE.md, nested skills | below cwd, on first touch | history | on-demand | `claudeMdExcludes` |
| `AGENTS.md` | only when no CLAUDE.md at or above cwd | user msg | always | `pluginConfigs["cc-plugin-agents-md@builtin"]` |
| auto-memory `MEMORY.md` | `~/.claude/projects/<slug>/memory/`, first 200 lines or 25 KB (cap) | user msg | always | `autoMemoryEnabled: false` · `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` |
| gitStatus · userEmail · attribution | built-in | user msg | always | `includeGitInstructions: false` · `attribution: {commit:"",pr:""}` · userEmail: **no switch** |
| hooks (settings / plugin / skill frontmatter) | `hooks` key per layer, merged; plugin `hooks/hooks.json` | stdout → system reminder for SessionStart, UserPromptSubmit, UserPromptExpansion, PostModelSwitch; other events: runtime | always / runtime | `disableAllHooks` · `enabledPlugins: false` |
| skills (user / project / plugin / bundled / claude.ai-synced) | `~/.claude/skills`, `.claude/skills` up to repo root, plugin `skills/`, `~/.claude/skills/synced` | listing in system reminder, ≤ 1,536 chars each, **budget 1 % of window** (cap) | advertised | `skillOverrides` (not for plugin skills) · `disableBundledSkills` · `syncClaudeAiSkills: false` · `enabledPlugins` · `disable-model-invocation: true` |
| subagents (managed / `--agents` / project / user / plugin / built-in) | `.claude/agents/**` up to repo root; `~/.claude/agents/**` | roster in system reminder | advertised | `enabledPlugins` · `permissions.deny: ["Agent(x)"]` (blocks use; roster effect unmeasured, Open question 2) · `CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS=1` |
| commands, output style, statusLine, permission rules | `.claude/commands`, `output-styles/`, settings | listing / reminder / **none** / **none** | advertised / always / none / none | `skillOverrides` · `outputStyle` · — · — |
| MCP servers (managed / local / project / user / plugin / claude.ai / `--mcp-config`) | `~/.claude.json` (`mcpServers`, `projects[cwd].mcpServers`), `.mcp.json`, plugin `.mcp.json` or inline, connectors | `tools` + MCP instructions; schemas `?` until the server has run | always behind a non-first-party base URL; deferred first-party `[docs]` | `projects[cwd].disabledMcpServers` · `disabledMcpjsonServers` · `deniedMcpServers` · `disableClaudeAiConnectors` · `enabledPlugins` · `--strict-mcp-config` · `ENABLE_TOOL_SEARCH=true` (defer instead of remove `[docs]`) |
| plugins | `installed_plugins.json` + `enabledPlugins` from 6 sources; local `false` beats project `true` | skills, agents, hooks, MCP, commands, LSP | per contribution | `enabledPlugins: {"<id>": false}` in `settings.local.json` |
| built-in tools, system prompt, Environment, date, Advisor | built-in | `tools`, `system`, reminder | always | `--tools`, `--disallowedTools`, `CLAUDE_CODE_DISABLE_{WEB_FETCH,CRON,BACKGROUND_TASKS,ARTIFACT,ADVISOR_TOOL}`; prompt: none |

Provenance values harn must recognise from the `init` event: `sdk, plugin, user, project, local,
dynamic, managed, enterprise, claudeai, agent`.

## Codex coverage (condensed from `research/02`)

| item | discovery | lands | mode | official off switch |
|---|---|---|---|---|
| `$CODEX_HOME/AGENTS.override.md` / `AGENTS.md` | first non-empty | user msg `# AGENTS.md instructions for <cwd>` | always | empty the file (no key) |
| project `AGENTS.md` chain | root = nearest ancestor with `.git` (`project_root_markers`), every dir root → cwd, `AGENTS.override.md` wins per dir, `project_doc_fallback_filenames` (default `[]`, so `CLAUDE.md` is **not** read) | same block, **32 KiB shared budget** (cap) | always | `projects."<p>".trust_level = "untrusted"` · `project_doc_max_bytes = 0` |
| `config.toml` layers | defaults → MDM/`/etc/codex` → `~/.codex/config.toml` → `<name>.config.toml` profile → trusted `.codex/config.toml` root→cwd → `-c` flags → managed | — | — | — |
| skills | `.agents/skills` and `.codex/skills` per dir (trust-independent), `~/.agents/skills`, `~/.codex/skills`, `.system` bundled, `/etc/codex/skills`, plugin cache | `<skills_instructions>` developer block, **2 % of window, ≤ 10k tokens** (cap), descriptions ≤ 1,024 chars | advertised | `[[skills.config]] path="<abs>/SKILL.md" enabled=false` · `skills.bundled.enabled=false` · `skills.include_instructions=false` |
| hooks | `~/.codex/hooks.json` or `[hooks]`, trusted `.codex/hooks.json`, plugin hooks | SessionStart stdout → developer message (≤ 2,500 tokens inline); other events runtime | always / runtime | `[hooks.state."<key>"] enabled=false` · `features.hooks=false` |
| rules | `~/.codex/rules/*.rules`, trusted `.codex/rules/` | `## Approved command prefixes` in the permissions block | always | — |
| plugins / marketplaces | `[plugins."p@m"]`, `~/.codex/plugins/cache/<m>/<p>/<v>/` | skills, MCP, apps, hooks, `<plugins_instructions>`; `<recommended_plugins>` lists **uninstalled** plugins | per contribution | `enabled=false` · `features.plugins=false` |
| MCP servers | `[mcp_servers.<id>]` user or trusted project | `tools[]` (+ namespace instructions); schemas `?` until run | always, or deferred behind `tool_search` when the model `supports_search_tool` | `enabled=false` · `enabled_tools` / `disabled_tools` |
| permission / collaboration / apps / environment blocks, multi-agent role | config | developer / user msgs | always | `include_*_instructions=false` · `include_environment_context=false` · `agents.enabled=false` |
| base instructions, core tools | model catalog | `instructions`, `tools[]` | always | none (`model_instructions_file` exists but is "STRONGLY DISCOURAGED") |

Two Codex traps harn must model: **three trust gates** (AGENTS.md skipped only if explicitly
`untrusted`; `.codex/config.toml`, hooks and rules load only if explicitly `trusted`; skills
ignore trust entirely), and `-c plugins."x@y".enabled=false` being silently ignored (key split on
`.`).

## Acceptance scenarios

Each row: run → what must be observed. "Both machines" = the author's macOS and `cachy-z13`.
**P0 gate = A1–A6, A9–A11 on both machines. P0.5 = A7–A8. P1 (TUI) has its own rows in its drive.**

| # | run | observed |
|---|---|---|
| A1 | `harn --dir ~/2lab.ai/zbrain --json --agent claude-code` | every statically discoverable name in that session's `init` event (`skills`, `agents`, `plugins`, `mcp_servers` with `source`) appears exactly once in the tree with the same `source`; MCP tool names and hook stdout are `?` rows, not omissions; no session was started (no new file under `~/.claude/projects/<slug>/`, no child process) |
| A2 | `harn ~/2lab.ai/zbrain` (and the same with `--dir`) | one table per agent, sorted by freed; every row has origin + mode + cost tag + default action; the header names the cost tag, the attributed/framing split and the tool-search state; runtime < 1 s on both machines |
| A3 | `harn verify --run --dir ~/2lab.ai/zbrain --agent claude-code` | prints what it will execute, makes one headless run; within the Claude Code projection (modes `runtime`/`none` excluded, capture-only children attached under their `?` parent) the report lists 0 `predicted-not-seen` and 0 `seen-not-predicted` **among statically discoverable ids (the A1 set)** and 0 content-conformance failures on reproducible static contributions; `?` rows become `observed`; the header switches to `observed` for the matched regions only |
| A3b | `just check` | the fixture suite includes before/after `freed` cases, each fixture defining **both** renderings (before and after) so the assertion is against the fixture's actual byte difference and never against a rule such as "at cap → 0": a token-budgeted listing (asserts `Δ?` statically and the exact delta when the fixture supplies the after-listing), a plugin whose skills leave with it, an item shared by two origins (counted once), and an excluded `CLAUDE.md` taking its `@imports` along |
| A4 | `harn off plugin:slack --dir ~/2lab.ai/zbrain` then `claude -p hi --output-format stream-json --verbose` in that directory | the plan showed target dir, diff, freed and side effects; `.claude/settings.local.json` gained `enabledPlugins["slack@claude-plugins-official"]: false`; the new `init` event lists no `plugin:slack:slack` and none of its 8 skills; `harn history` shows the op with its `RunContext`; `harn undo` restores the file byte-for-byte |
| A5 | `harn off <item>` on an item with no knob | `builtin` → refused with the reason; a project skill dir → moved to `.claude/harn-off/skills/<name>` with a manifest, the next `init` does not list it, `harn undo` moves it back; `--scope` was never widened silently |
| A6 | `harn edit rules/DEV.md --dir ~/2lab.ai/zbrain` | `$EDITOR` opened the realpath; on exit the diff was printed and journaled; `harn undo` restores the previous bytes |
| A7 | `harn --dir ~/2lab.ai/zbrain --agent codex --json` | AGENTS.md chain, skills roots, hooks, MCP servers, plugins match `codex -C … debug prompt-input` + `codex mcp list --json` + `codex plugin list --json`; the three trust gates are reflected (zbrain is `trusted`) |
| A8 | `harn verify --run --agent codex` | 0 presence diffs on instructions, skills listing and blocks; the report states that tools and hook output are outside Codex's dump |
| A9 | same binary on `cachy-z13` (`harn --version`, A1, A2) | identical columns and semantics; Linux paths (`/etc/claude-code/CLAUDE.md`, XDG state dir) resolved |
| A10 | `harn --readonly off anything` · `harn --readonly verify --run` | both exit non-zero without writing or running |
| A11 | an item present in a capture with no local config (the 2026-10-02 `blender` case) | row shows origin `unresolved` with the evidence it has (transport, log dir, last connect), never a guessed path; `verify --capture` refuses a capture whose `RunContext` does not match the target |

## Tensions

- **"llmux can edit the built-in prompt" (user's statement) vs llmux as shipped.** llmux records
  every request (`raw-io.jsonl`) and strips/injects credentials; it does not rewrite prompts
  (`llmux/.prd/01-spec.md:75`). Consequence: layer 1 is *viewable* through llmux and harn, not
  *editable*. Editing it is a possible future llmux feature, outside this spec.
- **MCP description cap**: docs say 2,048 chars; CHANGELOG 2.1.296 says 4,096. harn reads the
  truncation from the debug log when present, otherwise shows the untruncated size with a note.
- **Deferred tools "minimal impact"** (docs) vs ~20–27k tokens of deferred listings reported in
  issues. harn shows `?` for deferred cost until a first-party capture exists; never "free".
- **Codex `[[skills.config]]`**: docs say a folder path, the run showed only the `SKILL.md` path
  works. harn writes the file path.
- **Codex trust**: docs say untrusted projects skip `.codex/`; code says anything not explicitly
  trusted is skipped, and skills load regardless. harn follows the code (`research/02` §2–3).

## Open questions (each with the experiment that settles it)

1. Deferred-stub wire form and size on first-party Claude Code, and whether `ENABLE_TOOL_SEARCH=true`
   behind a proxy produces the same → the raw-body capture with tool search on. Until then the
   tool-search knob shows `freed: ?`.
2. Does `permissions.deny: ["Agent(x)"]` remove the roster line or only block the call? → `init`
   event agents list before and after. Until then that knob shows `freed: ?`.
3. Does Claude Code's `.claude/agents/**` / `.claude/skills` discovery skip dot-directories or
   unknown siblings such as `.claude/harn-off/`? → create one, run `init`, confirm absence. If it
   does not skip, the quarantine root moves to `.claude-harn-off/` beside `.claude/`.
4. `CLAUDE_CODE_DISABLE_CLAUDE_MDS` vs managed CLAUDE.md → a test machine with sudo.
5. userEmail / currentDate injection has no documented switch → watch anthropics/claude-code#81138;
   harn shows the row with `knob: none`.
6. Codex `skills.config` key semantics beyond `path`/`name` → `[미확인]` in `research/02`.

## Versioning, repository, distribution

Repository `2lab-ai/harn`, MIT, public. Crate and binary `harn`. Semver in `Cargo.toml`; stable
release on `v*` tags, rolling preview on every `main` push; two Homebrew formulae in
`2lab-ai/homebrew-tap` (`harn`, `harn-preview`) — the llmux asset shape and matrix exactly, so the
tap's existing bump job applies unchanged. `harn --version` prints version, build channel, 12-char
revision and the agent versions the rules were verified against. Mechanics:
[`02-architecture.md`](02-architecture.md).

## Revision log

- **v2.1 (2026-10-13, trinity round 1)** — effective list separated from first-request cost (modes
  `runtime`/`none`, cost tags `source`/`predicted`/`observed`/`?`/`—`); `freed` redefined as the
  before/after delta of an `ActionPlan` with caps, shared items and side effects; the user's
  "편집" restored as `harn edit` + `harn path`; `harn rm` and the Trash dependency dropped; `--dir`
  on every command and `RunContext` binding for captures; active verify split from capture reading
  and refused under `--readonly`; no silent scope widening; quarantine root moved outside discovery
  roots; `harn [dir]` defined once (table), TUI only via `harn tui`; `[docs]` labels on unmeasured
  first-party deferral and the `Agent(x)` roster effect; the 6,262-byte framing remainder
  attributed; P0 / P0.5 / P1 acceptance split; token counts moved out of the first release;
  distribution matched to llmux exactly; drift warning added.
- **v2.9 (2026-10-13, trinity round 4, last item)** — caption names the listing among the
  bundled rows and states that a plugin row's `+Δ?` and the listing row's `Δ?` are one unknown.
- **v2.8 (2026-10-13, trinity round 4, nits)** — `Δ?` added to the model vocabulary; status
  lines name rounds 1–4.
- **v2.7 (2026-10-13, trinity round 4)** — the over-cap default action corrected: no bulk
  "prune the N least-used" (unsupported for plugin skills, and N is a dropped-description count,
  not an amount to change); it is now "show candidates" with each item's supported knob, user
  selection, and recovery treated as an expected effect until a post-apply capture.
- **v2.6 (2026-10-13, trinity round 3, third verdict)** — over-cap **signal** (`N of M
  descriptions dropped`) in the header and as a row badge with a prune-to-fit default action, so
  a routing loss with no byte value is not buried by the byte sort; the positional-`[DIR]`
  commands enumerated.
- **v2.5 (2026-10-13, trinity round 3, second verdict)** — cap *state* and change *delta*
  separated: a token-budgeted listing's delta is `Δ?` until a before/after pair or a declared
  bytes-per-token assumption exists; the `freed` column is labelled predicted and shows
  `known + Δ?`; slack = `77.5 KB + Δ?`; A3b fixtures assert the fixture's own before/after bytes,
  never a cap rule.
- **v2.4 (2026-10-13, trinity round 3)** — mockup header partition aligned with the measured
  table (322.6 + 7.8 + 2.1); caption states the `share` denominator and why `freed` rows do not
  sum; `verify --capture` output lists `content-mismatch` and exits 1 on it.
- **v2.3 (2026-10-13, trinity round 2, all three verdicts)** — request partition re-measured with
  the wire's compact serialization (tools 210,679 B; instruction 2 B; remainder = measured request
  parameters 7,829 B + framing 2,092 B); `harn [DIR]` positional restored beside `--dir` and used
  in A2; caps evaluated by unit (byte caps static, token caps `?`/`~`/observed) with today's
  listing measured at cap (146/216 name-only); `~` added to the cost vocabulary; verify defined on
  each oracle's observable projection with capture-only children attached under `?` parents,
  presence + content conformance, dynamic values exempt; freed fixtures (A3b) in the gate;
  `history --prune`, edit parse-check warning, llmux `RunContext` derivation; P0/P0.5
  differential-test split in 02.
- **v2.2 (2026-10-13, trinity round 2)** — first-screen header and `share` labelled `observed`
  (the mockup's numbers are the 2026-10-13 capture); the static-run header defined as `predicted
  <known> + ? (N unknown)`; slack's default-action freed corrected to 78.7 KB because the skill
  listing is at cap; A3's zero-diff gate limited to statically discoverable ids.
- **v2 (2026-10-13)** — rewritten from scratch on research 00–04.
