# harn — Spec

`harn <dir>` lists the effective harness an AI coding agent loads when it is started in `<dir>`
— every instruction file, memory, hook, skill, subagent, MCP server, plugin and built-in the agent
will pick up — as one tree per agent, each row with where it came from, whether it costs bytes in
the first model request (and how many), and the agent's official switch that turns it off. It
turns items off, edits them, and undoes both, through a journal. Claude Code first, Codex CLI
second. One Rust binary for macOS and Linux; CLI with `--json` at the base, a TUI on the same model.

Status: **planned** — v2.1, 2026-10-13. Rewritten from scratch on [`research/`](research/) (00 =
the user's statement as a PRD, 01 = Claude Code loading rules, 02 = Codex loading rules, 03 = prior
art and UX, 04 = local measurements), then revised on trinity round 1 (see [§Revision
log](#revision-log)). The user's verbatim statement and the name decision are fixed in
[`prd-rewrite/ssot.md`](prd-rewrite/ssot.md). This document is a target, not a description of
shipped behavior; it moves to **shipped** only when [§Acceptance](#acceptance-scenarios) P0 is green
on both machines.

## Problem

A fresh Claude Code 2.1.296 session in `~/2lab.ai/zbrain` sends a first request of 332,548 bytes
before reading the user's instruction, which is 30 of those bytes (`research/04` §E, measured
2026-10-13 with Claude Code's own raw-body capture). The bytes come from the four places the user
named — the agent's built-in prompt and tools, the `~/.claude` harness, the working folder's
harness, then the instruction — and Codex assembles its own request from `~/.codex` and `AGENTS.md`
the same way. Nothing today shows the effective harness for a directory **without starting a
session**, with the **file or setting that produced each item**, the **cost** it adds to the
request, and the **switch** that removes it. Claude Code's `/context` is the closest thing
(`research/04` §A): an estimate, inside a live session, no hooks, no paths, no action. Trimming the
harness today means grepping six config layers and guessing.

The measured composition of that request (text bytes of each region, 2026-10-13, the same machine):

| region | text bytes | what |
|---|---|---|
| `tools` | 214,313 | 26 built-in tools 69,592 · MCP `plugin:slack` 27 tools 78,695 · MCP `plugin:atlassian` 21 tools 66,026 |
| `messages[1]` system text | 57,202 | SessionStart hook stdout 3,444 · Environment 841 · agent roster (31) 17,050 · MCP instructions 2,596 · skill list (219 names) 31,212 · date 29 · Advisor 2,018 |
| `messages[0]` block 0 | 44,936 | `~/.claude/CLAUDE.md` 738 · `zbrain/CLAUDE.md` 6,835 · `@rules/DEV.md` 9,443 · `@rules/PRINCIPLES.md` 4,261 · auto-memory `MEMORY.md` 23,447 |
| `system` | 6,137 | billing header 74 · identity 62 · harness rules 6,001 |
| `messages[0]` block 1 | 3,668 | userEmail + gitStatus |
| `messages[0]` block 2 | 30 | the instruction |
| attributed sum | 326,286 | |
| JSON framing + request parameters | 6,262 | keys, quoting, escapes, `metadata`, `thinking`, `safeguards`, … — the difference to the 332,548-byte body; never attributed to an item |

Every MCP schema was sent up front because the session ran behind a non-first-party
`ANTHROPIC_BASE_URL` (the local llmux proxy), which turns Claude Code's tool search off
(`research/01` §7, docs). Behind the first-party endpoint the docs say the same 48 tools are
deferred stubs `[docs — stub size not measured; Open question 1]`. The same item can therefore cost
144 KB in one configuration and an unmeasured, smaller amount in another: a size without its load
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
   with its tools, counts shared items once, respects the agent's caps (a listing at its budget
   frees ≈ 0 when one entry leaves — the next entry fills the slot), and shows `—` for items with no
   knob and `?` for items whose cost is unknown. The default sort key is the freed bytes of each
   row's default action.
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
   may cost a model request, and is refused under `--readonly`. Matching is **presence by item id**;
   byte differences are not diffs, they replace `predicted` with `observed` for that row. Only
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
cost      source: bytes on disk | predicted: bytes in the first request after caps | observed: bytes
          in a bound capture | ? (unknown until a capture) | — (no first-request cost)
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
- **Trade-off, stated once:** static is fast, session-free and drifts; oracles are ground truth and
  cost one request (Claude Code) or a process spawn with side effects (Codex). The spec keeps both
  and labels which one produced every number.

## Interaction model

### P0 — CLI (ships first; Claude Code chain complete)

Every command takes `--dir <path>` (default: cwd) and `--agent claude-code|codex` (default: all,
one table each).

| command | does | output |
|---|---|---|
| `harn [--dir D]` | the effective tree, one table per agent, sorted by the freed bytes of each row's default action | table: freed · predicted · share · item · origin · mode · default action |
| `harn --json` | the full model | `{"schema":"harn/1", "agents":[…]}` |
| `harn why <item>` | admission chain, realpath, knob candidates by scope, the freed bytes of each | text |
| `harn path <item>` | the resolved realpath, one line | path |
| `harn off <item> [--scope local\|project\|user] [--dry-run]` | build an `ActionPlan`, print the diff + freed delta + side effects + warnings, then apply the official knob in the given scope (default `local`); quarantine only when no knob exists and only in the same scope; a wider scope is never chosen silently | plan + journal id |
| `harn on <item>` / `harn undo [N]` / `harn history` | reverse journal ops; stale targets reported, not forced | text |
| `harn edit <item>` | snapshot → `$EDITOR <realpath>` → diff → journal entry; refused for `builtin`/`managed` items | diff + journal id |
| `harn verify --capture <path>` | read-only diff of the tree against a bound capture | per-row `seen` / `predicted-not-seen` / `seen-not-predicted`; exit 1 on any presence diff |
| `harn verify --run` | runs the agent once under its official capture after printing what it will execute; refused under `--readonly` | same |
| `harn completions <shell>` | clap completions | — |

Item selectors: a full id or any unique suffix (`plugin:slack`, `rules/DEV.md`); an ambiguous
selector lists the candidates and exits 2. Flags everywhere: `--readonly` (refuses every write and
every active verify), `--no-unicode`, `--color`.

### P0.5 — Codex resolver (same commands; acceptance A7–A8)

### P1 — TUI (`harn tui` or `harn -i`; `harn` alone always prints the table)

One screen per agent as tabs, the same table, a detail/why pane, a staged-plan pane with the diff
and freed delta, `?` key overlay and a context key bar; disable = stage + apply through the same
`ActionPlan`; `--readonly` greys every write key. Key choices follow `research/03` §B (ncdu, yazi,
lazygit, k9s) and are fixed in the P1 drive, not here.

### First screen — zbrain on the author's machine

A mockup: the numbers are the 2026-10-13 `observed` bytes from `research/04` drawn into a static
(`predicted`) table; a real first run shows `?` where noted and no `observed` column.

```
harn ~/2lab.ai/zbrain                                     claude-code 2.1.296 (rules verified 2.1.296)
first request: predicted 332.5 KB · attributed 326.3 KB · framing 6.3 KB         sort: freed ▼
tool search: OFF (ANTHROPIC_BASE_URL is non-first-party) → 48 MCP schemas eager, 144.7 KB
    knob: env ENABLE_TOOL_SEARCH=true  [docs — effect not measured; verify after]       freed: ?

  freed     predicted  share  item                              origin                    mode        default action
  79.8 KB    78.7 KB   23.7%  mcp plugin:slack · 27 tools       plugin slack@official     always      enabledPlugins → false (local) · +8 skills leave listing (at cap: ~0)
  68.6 KB    66.0 KB   19.8%  mcp plugin:atlassian · 21 tools   plugin atlassian@official always      enabledPlugins → false (local) · +2.6 KB instructions
  23.4 KB    23.4 KB    7.0%  MEMORY.md · 150 lines / 25 KB cap ~/.claude/projects/…/memory always      autoMemoryEnabled → false
  20.6 KB     6.8 KB    2.1%  CLAUDE.md (+ 2 @imports)          project                   always      claudeMdExcludes · DEV.md 9.4 KB + PRINCIPLES.md 4.3 KB leave with it
   9.4 KB     9.4 KB    2.8%  rules/DEV.md                      project @import           always      claudeMdExcludes
   4.3 KB     4.3 KB    1.3%  rules/PRINCIPLES.md               project @import           always      claudeMdExcludes
   3.4 KB     3.4 KB    1.0%  SessionStart hook stdout          plugin superpowers@official always     enabledPlugins → false (local)
  ~3.4 KB     3.7 KB    1.1%  gitStatus + userEmail             builtin                   always      includeGitInstructions → false (git part only; userEmail has no switch)
   2.0 KB     2.0 KB    0.6%  Advisor tool text                 builtin                   always      CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1
   0.7 KB     0.7 KB    0.2%  ~/.claude/CLAUDE.md               user                      always      claudeMdExcludes
      ?      17.1 KB    5.1%  agent roster · 31                 project 10 · user 2 · plugins 16 · builtin 3   advertised   per plugin: enabledPlugins; per agent: permissions.deny Agent(x) [roster effect unmeasured]
   ~0 KB     31.2 KB    9.4%  skill listing · 219 · AT CAP      user 22 · project 63 · plugins 134   advertised   skillOverrides / enabledPlugins — one entry out frees ≈ 0 while over budget
  partial    69.6 KB   20.9%  built-in tools · 26               builtin                   always      --disallowedTools <tool> (per tool; Bash/Read/Edit stay)
      —       6.1 KB    1.8%  system prompt · 3 blocks          builtin                   always      none (built-in)
      —       0.0 KB         your instruction (30 B)            you                       —           —
  runtime / no first-request cost:
      —          —            hooks · 10 other events           ~/.claude/settings.json   runtime     disableAllHooks / per-event entry
      —          —            permissions.allow · 10 rules      .claude/settings.local.json none      —
      —          —            statusLine                        ~/.claude/settings.json   none        —
```

### Detail pane — `harn why plugin:slack`

```
plugin:slack:slack   (claude-code/mcp-server/plugin/plugin:slack:slack)        target: ~/2lab.ai/zbrain
  27 MCP tools · predicted 78,695 B schemas · 0 B instructions · mode always (tool search off)
  why loaded
    ~/.claude/settings.json        enabledPlugins["slack@claude-plugins-official"] = true
    → ~/.claude/plugins/installed_plugins.json   slack@claude-plugins-official 1.4.1
    → ~/.claude/plugins/cache/claude-plugins-official/slack/1.4.1/.mcp.json   "slack" (stdio)
    also contributes: 8 skills (advertised, 1.1 KB in the listing) · 0 agents · 0 hooks
  last used   3 of the last 20 sessions in this directory (session jsonl, 14 d)
  actions                                                                    freed (predicted)
    [1] .claude/settings.local.json   enabledPlugins[…] = false   this project   ← default   79.8 KB
    [2] ~/.claude/settings.json       enabledPlugins[…] = false   every project  (--scope user) 79.8 KB
    [3] ~/.claude.json projects[cwd].disabledMcpServers += "plugin:slack:slack"  server only   78.7 KB
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
| A2 | `harn --dir ~/2lab.ai/zbrain` | one table per agent, sorted by freed; every row has origin + mode + cost tag + default action; the header names the cost tag, the attributed/framing split and the tool-search state; runtime < 1 s on both machines |
| A3 | `harn verify --run --dir ~/2lab.ai/zbrain --agent claude-code` | prints what it will execute, makes one headless run; the report lists 0 `predicted-not-seen` and 0 `seen-not-predicted` by item id against that run's capture on the author's machine; `?` rows become `observed`; the header switches to `observed` for the matched regions only |
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
- **v2 (2026-10-13)** — rewritten from scratch on research 00–04.
