# harn — Spec

`harn <dir>` shows everything an AI coding agent loads into its model request when it is started
in `<dir>` — as one tree per agent, each row with its size, where it came from, whether it is
always loaded or only advertised, and the official switch that turns it off — and lets the user
turn items off reversibly. Claude Code first, Codex CLI second. One Rust binary for macOS and
Linux; CLI with `--json` at the base, a TUI on the same model.

Status: **planned** — v2, 2026-10-13. Rewritten from scratch on the research in
[`research/`](research/) (00 = the user's statement as a PRD, 01 = Claude Code loading rules,
02 = Codex loading rules, 03 = prior art and UX, 04 = local measurements). The user's verbatim
statement and the name decision are fixed in [`prd-rewrite/ssot.md`](prd-rewrite/ssot.md). This
document is a target, not a description of shipped behavior; it moves to **shipped** only when
[§Acceptance](#acceptance-scenarios) is green on both machines.

## Problem

A fresh Claude Code 2.1.296 session in `~/2lab.ai/zbrain` sends a first request of 332,548 bytes
before reading the user's instruction, which is 30 of those bytes (`research/04` §E, measured
2026-10-13 with Claude Code's own raw-body capture). The bytes come from four places the user
named — the agent's built-in prompt and tools, the `~/.claude` harness, the working folder's
harness, then the instruction — and Codex assembles its own request from `~/.codex` and
`AGENTS.md` the same way. Nothing today shows that list for a directory **without starting a
session**, with a **size** per item, the **file or setting that produced it**, and the **switch**
that removes it. Claude Code's `/context` is the closest thing (`research/04` §A): it is an
estimate, it runs inside a live session, it omits hooks and git/email blocks, and it has no action.
Trimming the harness today means grepping six config layers and guessing.

The measured composition of that request (bytes of text, 2026-10-13, the same machine):

| region | bytes | what |
|---|---|---|
| `tools` | 214,313 | 26 built-in tools 69,592 · MCP `plugin:slack` 27 tools 78,695 · MCP `plugin:atlassian` 21 tools 66,026 |
| `messages[1]` system text | 57,202 | SessionStart hook stdout 3,444 · Environment 841 · agent roster (31) 17,050 · MCP instructions 2,596 · skill list (219 names) 31,212 · date 29 · Advisor 2,018 |
| `messages[0]` block 0 | 44,936 | `~/.claude/CLAUDE.md` 738 · `zbrain/CLAUDE.md` 6,835 · `@rules/DEV.md` 9,443 · `@rules/PRINCIPLES.md` 4,261 · auto-memory `MEMORY.md` 23,447 |
| `system` | 6,137 | billing header 74 · identity 62 · harness rules 6,001 |
| `messages[0]` block 1 | 3,668 | userEmail + gitStatus |
| `messages[0]` block 2 | 30 | the instruction |

Every MCP schema was sent up front because the session ran behind a non-first-party
`ANTHROPIC_BASE_URL` (the local llmux proxy), which turns Claude Code's tool search off
(`research/01` §7). Behind the first-party endpoint the same 48 MCP tools would be **deferred**
stubs. The same item therefore costs 144 KB in one configuration and a few hundred bytes in
another — a tool that reports a size without the load mode misleads.

## Prior art, in one paragraph

Claude Code's `/context`, `/skills`, `/skill-doctor` and Codex's `codex debug prompt-input`
report; none edits, none runs without a session (Codex's dump runs without a model call but still
spawns MCP servers). migsilva89/loadout (macOS GUI, 98★) manages what assistants load and
quarantines skills into `skills-off/`, but shows no sizes, only per-project ownership, and has no
Linux, CLI or `--json`. ccmcp (Go) has the right default screen — the effective set for this
directory, with one-letter origin badges and undoable per-project overrides — but covers MCP and
plugins only. cclint has the right vocabulary — load classes *always / advertised / on-demand*,
exact counts only when measured, otherwise `~` — and checks itself against the real `claude`
binary. harn takes the default screen from ccmcp, the vocabulary and the self-check from cclint,
the size-sorted tree with confirm-by-default from ncdu, and adds the two things none of them do:
**per-item freed size** and **verification against the agent's own request**. Details and URLs:
`research/03`.

## Goals

1. **List, for one directory and one agent, every item that reaches the model request at session
   start**, from the agent's real discovery rules: instruction files and their imports, auto-memory,
   settings-driven blocks (git status, attribution, environment), hooks whose stdout is injected,
   skills (the advertised listing), subagents (the roster), slash commands, output style, MCP servers
   (tool schemas and server instructions), plugins (what each contributes), built-in tools and the
   built-in system prompt. The static resolver must not start the agent.
2. **Each row carries four facts**: size with provenance (`bytes` exact, `~tok` estimated with the
   model named, `wire` seen in a captured request); **load mode** (`always` / `advertised` /
   `deferred` / `on-demand`); **origin** (kind + resolved real path or setting key + layer); and the
   **knob**: the agent's official way to turn it off without deleting, in the narrowest scope that
   works. Items with no knob say so (`built-in`, `no switch`).
3. **The number that drives decisions is "freed if off"** — the unique bytes that disappear when
   this item is disabled, nix-tree's "added size", not the subtree total. It is the default sort key
   and the first column.
4. **Turn items off reversibly, through the agent's own switch.** Order of mechanisms: official
   knob in the narrowest scope (project-local before user) → quarantine move next to the owner
   (`<owner>/.harn-off/`) with a manifest → **never `rm`**; explicit remove goes to the OS Trash.
   Every write is preceded by a snapshot into harn's own state dir and recorded in a journal;
   `harn undo` reverses it; a target changed since the write is reported stale, not forced.
5. **Verify against the agent's own ground truth.** `harn verify` runs the agent once behind its
   official capture (Claude Code: `OTEL_LOG_RAW_API_BODIES=file:<dir>` request body + the
   `stream-json` `system/init` provenance event; Codex: `codex debug prompt-input` + `codex mcp
   list --json` + `codex plugin list --json`) and diffs it against the static tree. Rows then show
   `seen` / `predicted-not-seen` / `seen-not-predicted`. The header says which tag the total uses
   until verification has run.
6. **Same model everywhere.** `harn <dir> --json` (schema `harn/1`), the TUI, `harn why`, `harn
   off/on/undo/history/verify` all read and write one data model; scripts and humans see the same
   numbers.
7. **Runs on macOS (arm64, x86_64) and Linux (x86_64, arm64), including CachyOS**, with no
   runtime dependency beyond the agents it inspects.

## Non-goals

- A GUI. The user said "TUI 혹은 GUI (필요시)"; nothing in the research makes a GUI necessary, and
  migsilva89/loadout already occupies that niche. Revisit only on evidence.
- Editing the agent's built-in prompt or built-in tool definitions. harn **shows** them (size,
  mode, "built-in, no switch"). The user's premise that llmux can edit them is recorded under
  [§Tensions](#tensions).
- Generating or syncing config files across agents (ruler's job), scaffolding templates, or any
  "add more harness" direction.
- Usage analytics beyond what one column needs ("last used N sessions ago", read from the agent's
  own session logs) — ccusage's job.
- Supporting agents other than Claude Code and Codex in this version. The model is agent-neutral
  (origin kinds and load modes are not Claude-specific), so a third resolver is additive.
- Per-token exactness for Claude models. No official Claude tokenizer exists; `~tok` is labelled
  as an estimate, `wire` is the measured byte count. Anthropic's `count_tokens` is optional and
  itself documented as an estimate.

## The model

```
Agent     claude-code | codex                       (one tree per agent; TUI shows them as tabs)
Item      id, label, kind, size{bytes, tok_est?, wire?}, mode, origin, knob, freed, children[]
kind      instruction | memory | hook-output | skill-listing | agent-roster | command |
          mcp-server | mcp-tool | mcp-instructions | plugin | builtin-tool | system-prompt |
          env-block | git-block | attribution | output-style | unresolved
mode      always      — in every first request
          advertised  — name (+ description) only; body loads on use (skills, agents)
          deferred    — stub now, schema on demand (MCP tools under tool search)
          on-demand   — not in the first request (nested CLAUDE.md, skill bodies, rules with paths:)
origin    layer ∈ managed | user | project | local | plugin@marketplace | dynamic (CLI flag) |
                  claudeai (remote connector) | builtin | unresolved
          path   — resolved realpath (symlinks resolved; shared copies flagged) or setting key
          why    — the chain that admitted it (settings key → plugin manifest → file), nix-tree style
knob      {file, key, value, scope}   — the official off switch, narrowest scope that wins
          | quarantine {from, to}     — only when no knob exists
          | none {reason}            — built-in / undocumented
freed     unique bytes removed if this item (and only this item) is turned off
```

Provenance tags are never dropped: a total is `bytes` until `verify` has matched it to a request,
then `wire`. `~tok` appears only beside a model name.

## Source of truth

- **Default = static resolver, no session.** It implements the discovery rules in `research/01`
  (Claude Code) and `research/02` (Codex), reads only files and settings, and finishes in well
  under a second for a directory like zbrain. It can drift when an agent changes its rules.
- **`harn verify` = the agent's own oracles.** Claude Code: one headless run with
  `OTEL_LOG_RAW_API_BODIES=file:<tmp>` (full request body + `index.jsonl`) and `--output-format
  stream-json --verbose` (`system/init` with `tools`, `mcp_servers[{name,status,source}]`,
  `skills`, `agents`, `plugins[{name,path,source,version}]`, `memory_paths`) — both verified
  on this machine 2026-10-13 (`research/04` §E). Codex: `codex -C <dir> debug prompt-input`
  (exact input list, no model call, but it spawns MCP servers and writes to `CODEX_HOME`) + `codex
  debug models` (base instructions) + `codex mcp list --json` + `codex plugin list --json`;
  tools and hook output are not in any Codex dump, so for Codex `verify` covers instructions,
  skills and blocks, and says so (`research/02` §8).
- **Measured mode without a run.** If llmux is in front of the agent, its `raw-io.jsonl` already
  holds the first request of every session; `harn verify --from llmux` reads it instead of
  spawning the agent. The same for a `CODEX_ROLLOUT_TRACE_ROOT` bundle.
- **Trade-off, stated once:** static is fast, session-free and drifts; oracles are ground truth and
  cost one request (Claude Code) or a process spawn (Codex). The spec keeps both and labels which
  one produced every number.

## Interaction model

### P0 — CLI (ships first; the TUI is a view over it)

| command | does | output |
|---|---|---|
| `harn [dir]` | effective tree for `dir` (default: cwd), both agents, sorted by `freed` desc | table: freed · % · bar · item · origin badge · mode · knob |
| `harn [dir] --agent claude-code\|codex` | one agent | same |
| `harn [dir] --json` | the full model | `{"schema":"harn/1", "agents":[…]}` |
| `harn why <item>` | the admission chain, the resolved path, the knob candidates by scope, `freed` | text |
| `harn off <item> [--scope local\|project\|user]` | stage + apply the narrowest official knob (default scope `local`); quarantine only if no knob; prints the diff and the `freed` delta first; `--dry-run` stops before writing | diff + journal id |
| `harn on <item>` / `harn undo [N]` / `harn history` | reverse journal ops; stale targets reported, not forced | text |
| `harn rm <item>` | move to OS Trash after typed confirmation of the item name | text |
| `harn verify [dir] [--from llmux\|otel-dir\|rollout]` | run or read the oracle, diff against the static tree | per-row `seen`/`predicted-not-seen`/`seen-not-predicted`; exit 1 on any diff |
| `harn completions <shell>` | clap completions | — |

Flags everywhere: `--readonly` (refuses every write), `--no-unicode`, `--color`, `--bytes` (hide
token estimates).

### P1 — TUI (`harn [dir]` in a terminal opens it; `--plain` prints the table instead)

Keys, each borrowed from a tool named in `research/03` §B: `j/k` + arrows move; `l`/`Enter` expand
or open the detail pane; `h` collapse; `s` cycles sort (freed → total → name), same key again
reverses; `/` filters while keeping the hierarchy; `w` = why-loaded pane; `space`/`d` stage a
disable (reversible); `D` stage a remove to Trash (type the item name to confirm); `a` apply all
staged ops after one snapshot, showing the diff and the freed delta; `u` undo last; `v` run verify;
`t` switch agent tab; `?` key overlay; the bottom line always lists the keys for the current pane.
`--readonly` greys every write key out.

### First screen — zbrain on the author's machine (numbers from `research/04`, static, bytes)

```
harn ~/2lab.ai/zbrain                              claude-code 2.1.296 │ codex 0.160.1
first request ≈ 332.5 KB (bytes, static — not yet verified: press v)      sort: freed ▼

  freed      share  item                                origin                  mode        off via
  78.7 KB  23.7% ████████▌ mcp plugin:slack · 27 tools  plugin slack@official   always*     enabledPlugins → false (local)
  69.6 KB  20.9% ███████▍  built-in tools · 26          builtin                 always      --tools / --disallowedTools (partial)
  66.0 KB  19.8% ███████▏  mcp plugin:atlassian · 21    plugin atlassian@offic  always*     enabledPlugins → false (local)
  31.2 KB   9.4% ███▍      skill listing · 219 names    user 22 · project 63 ·  advertised  skillOverrides / enabledPlugins
                                                        plugins 134
  23.4 KB   7.0% ██▌       MEMORY.md · 150 lines        ~/.claude/projects/…/me always      autoMemoryEnabled → false
  17.1 KB   5.1% █▊        agent roster · 31            project 10 · user 2 ·   advertised  permissions.deny Agent(x) / enabledPlugins
                                                        plugins 16 · builtin 3
   9.4 KB   2.8% █         rules/DEV.md                 project @import         always      claudeMdExcludes
   6.8 KB   2.1% ▊         CLAUDE.md                    project                 always      claudeMdExcludes
   6.1 KB   1.8% ▋         system prompt · 3 blocks     builtin                 always      none (built-in)
   4.3 KB   1.3% ▍         rules/PRINCIPLES.md          project @import         always      claudeMdExcludes
   3.7 KB   1.1% ▍         gitStatus + userEmail        builtin                 always      includeGitInstructions → false (git only)
   3.4 KB   1.0% ▍         SessionStart hook stdout     plugin superpowers@offi always      enabledPlugins → false / disableAllHooks
   2.6 KB   0.8% ▎         MCP server instructions      plugin atlassian        always      (follows the server)
   2.0 KB   0.6% ▏         Advisor tool text            builtin                 always      CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1
   0.7 KB   0.2%           ~/.claude/CLAUDE.md          user                    always      claudeMdExcludes
   0.0 KB         ─        your instruction             you                     —           —
* always because tool search is off behind ANTHROPIC_BASE_URL=localhost:3456; first-party → deferred
 j/k move  l open  w why  / filter  s sort  space off  D rm  a apply  u undo  v verify  t agent  ? keys
```

### Detail pane — `w` on the first row

```
┌ plugin:slack:slack ──────────────────────────────────────────────────────────────┐
│ 27 MCP tools · 78,695 B of schemas · 0 B instructions · mode always*              │
│ why loaded                                                                        │
│   ~/.claude/settings.json      enabledPlugins["slack@claude-plugins-official"] = true │
│   → ~/.claude/plugins/installed_plugins.json   slack@claude-plugins-official 1.4.1 │
│   → ~/.claude/plugins/cache/claude-plugins-official/slack/1.4.1/.mcp.json  "slack" (stdio) │
│   also contributes: 8 skills (advertised, 1.1 KB in the listing) · 0 agents · 0 hooks │
│ last used   3 of the last 20 sessions in this directory (session jsonl, 14 d)      │
│ off                                                                               │
│   [1] .claude/settings.local.json  enabledPlugins[…] = false   this project only  ← default │
│   [2] ~/.claude/settings.json      enabledPlugins[…] = false   every project       │
│   [3] ~/.claude.json projects[cwd].disabledMcpServers += "plugin:slack:slack"      │
│       server only — the 8 skills stay advertised                                  │
│ freed   [1]/[2]: 79.8 KB   [3]: 78.7 KB                                            │
│ note    .claude/settings.local.json is git-ignored here; no tracked file changes   │
└──────────────────────────────────────────────────────────────────────────────────┘
```

## Claude Code coverage (condensed from `research/01`; every row has a doc URL there)

| item | discovery | lands | mode | official off switch |
|---|---|---|---|---|
| managed CLAUDE.md | `/Library/Application Support/ClaudeCode/CLAUDE.md`, `/etc/claude-code/CLAUDE.md` | user msg | always | none (admin) |
| `~/.claude/CLAUDE.md`, `~/.claude/rules/**` | user | user msg | always | `claudeMdExcludes` |
| `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md` | cwd and every ancestor, root → cwd; `@path` imports ≤ 4 hops | user msg | always | `claudeMdExcludes` · `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` |
| `.claude/rules/*.md` | recursive; with `paths:` → on-demand | user msg | always / on-demand | `claudeMdExcludes` |
| subdirectory CLAUDE.md, nested skills | below cwd, on first touch | history | on-demand | `claudeMdExcludes` |
| `AGENTS.md` | only when no CLAUDE.md at or above cwd | user msg | always | `pluginConfigs["cc-plugin-agents-md@builtin"]` |
| auto-memory `MEMORY.md` | `~/.claude/projects/<slug>/memory/`, first 200 lines or 25 KB | user msg | always | `autoMemoryEnabled: false` · `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` |
| gitStatus · userEmail · attribution | built-in | user msg | always | `includeGitInstructions: false` · `attribution: {commit:"",pr:""}` · userEmail: **no switch** |
| hooks (settings / plugin / skill frontmatter) | `hooks` key per layer, merged; plugin `hooks/hooks.json` | stdout → system reminder (SessionStart, UserPromptSubmit, …) | always | `disableAllHooks` · `enabledPlugins: false` |
| skills (user / project / plugin / bundled / claude.ai-synced) | `~/.claude/skills`, `.claude/skills` up to repo root, plugin `skills/`, `~/.claude/skills/synced` | listing in system reminder, ≤ 1,536 chars each, budget 1 % of window | advertised | `skillOverrides` (not for plugin skills) · `disableBundledSkills` · `syncClaudeAiSkills: false` · `enabledPlugins` · `disable-model-invocation: true` in frontmatter |
| subagents (managed / `--agents` / project / user / plugin / built-in) | `.claude/agents/**` up to repo root; `~/.claude/agents/**` | roster in system reminder | advertised | `permissions.deny: ["Agent(x)"]` · `enabledPlugins` · `CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS=1` |
| commands, output style, statusLine | `.claude/commands`, `output-styles/`, settings | listing / reminder / **none** | advertised / always / — | `skillOverrides` · `outputStyle` · (statusLine costs 0) |
| MCP servers (managed / local / project / user / plugin / claude.ai / `--mcp-config`) | `~/.claude.json` (`mcpServers`, `projects[cwd].mcpServers`), `.mcp.json`, plugin `.mcp.json` or inline, connectors | `tools` + MCP instructions | always behind a proxy, deferred first-party | `projects[cwd].disabledMcpServers` · `disabledMcpjsonServers` · `deniedMcpServers` · `disableClaudeAiConnectors` · `enabledPlugins` · `--strict-mcp-config` |
| plugins | `installed_plugins.json` + `enabledPlugins` from 6 sources; local `false` beats project `true` | skills, agents, hooks, MCP, commands, LSP | per contribution | `enabledPlugins: {"<id>": false}` in `settings.local.json` |
| built-in tools, system prompt, Environment, date, Advisor | built-in | `tools`, `system`, reminder | always | `--tools`, `--disallowedTools`, `CLAUDE_CODE_DISABLE_{WEB_FETCH,CRON,BACKGROUND_TASKS,ARTIFACT,ADVISOR_TOOL}`; prompt: none |

Provenance values harn must recognise from the `init` event: `sdk, plugin, user, project, local,
dynamic, managed, enterprise, claudeai, agent`.

## Codex coverage (condensed from `research/02`)

| item | discovery | lands | mode | official off switch |
|---|---|---|---|---|
| `$CODEX_HOME/AGENTS.override.md` / `AGENTS.md` | first non-empty | user msg `# AGENTS.md instructions for <cwd>` | always | empty the file (no key) |
| project `AGENTS.md` chain | root = nearest ancestor with `.git` (`project_root_markers`), every dir root → cwd, `AGENTS.override.md` wins per dir, `project_doc_fallback_filenames` (default `[]`, so `CLAUDE.md` is **not** read) | same block, 32 KiB shared budget | always | `projects."<p>".trust_level = "untrusted"` · `project_doc_max_bytes = 0` |
| `config.toml` layers | defaults → MDM/`/etc/codex` → `~/.codex/config.toml` → `<name>.config.toml` profile → trusted `.codex/config.toml` root→cwd → `-c` flags → managed | — | — | — |
| skills | `.agents/skills` and `.codex/skills` per dir (trust-independent), `~/.agents/skills`, `~/.codex/skills`, `.system` bundled, `/etc/codex/skills`, plugin cache | `<skills_instructions>` developer block, 2 % of window, ≤ 10k tokens, descriptions ≤ 1,024 chars | advertised | `[[skills.config]] path="<abs>/SKILL.md" enabled=false` · `skills.bundled.enabled=false` · `skills.include_instructions=false` |
| hooks | `~/.codex/hooks.json` or `[hooks]`, trusted `.codex/hooks.json`, plugin hooks | SessionStart stdout → developer message (≤ 2,500 tokens inline) | always | `[hooks.state."<key>"] enabled=false` · `features.hooks=false` |
| rules | `~/.codex/rules/*.rules`, trusted `.codex/rules/` | `## Approved command prefixes` in permissions block | always | — |
| plugins / marketplaces | `[plugins."p@m"]`, `~/.codex/plugins/cache/<m>/<p>/<v>/` | skills, MCP, apps, hooks, `<plugins_instructions>`; `<recommended_plugins>` lists **uninstalled** plugins | per contribution | `enabled=false` · `features.plugins=false` |
| MCP servers | `[mcp_servers.<id>]` user or trusted project | `tools[]` (+ namespace instructions) | always, or deferred behind `tool_search` when the model `supports_search_tool` | `enabled=false` · `enabled_tools` / `disabled_tools` |
| permission / collaboration / apps / environment blocks, multi-agent role | config | developer / user msgs | always | `include_*_instructions=false` · `include_environment_context=false` · `agents.enabled=false` |
| base instructions, core tools | model catalog | `instructions`, `tools[]` | always | none (`model_instructions_file` exists but is "STRONGLY DISCOURAGED") |

Two Codex traps harn must model: **three trust gates** (AGENTS.md skipped only if explicitly
`untrusted`; `.codex/config.toml`, hooks and rules load only if explicitly `trusted`; skills ignore
trust entirely), and `-c plugins."x@y".enabled=false` being silently ignored (key split on `.`).

## Acceptance scenarios

Each row: run → what must be observed. "Both machines" = the author's macOS and `cachy-z13`.

| # | run | observed |
|---|---|---|
| A1 | `harn ~/2lab.ai/zbrain --json --agent claude-code` | every name in that session's `init` event (`tools`, `skills`, `agents`, `plugins`, `mcp_servers`) appears exactly once in the tree with the same `source`; no session was started (no new file under `~/.claude/projects/<slug>/`) |
| A2 | `harn ~/2lab.ai/zbrain` | first screen matches §First screen in shape: sorted by `freed`, every row has size + mode + origin + knob, header names the provenance tag; runtime < 1 s |
| A3 | `harn verify ~/2lab.ai/zbrain --agent claude-code` | one headless run; report lists 0 `predicted-not-seen` and 0 `seen-not-predicted` rows for the first request on the author's machine; the header switches to `wire` |
| A4 | `harn off plugin:slack` then `claude -p hi --output-format stream-json --verbose` | `.claude/settings.local.json` gained `enabledPlugins["slack@claude-plugins-official"]: false`; the new `init` event lists no `plugin:slack:slack`; `harn history` shows the op; `harn undo` restores the file byte-for-byte |
| A5 | `harn off <item>` on an item with no knob | harn refuses with the reason (`built-in`) or quarantines (skill dir → `<owner>/.harn-off/<name>` + manifest), never `rm`; `harn undo` moves it back |
| A6 | `harn rm <skill>` | prompt asks for the item name; after it, the directory is in the OS Trash (`~/.Trash` or XDG Trash), not deleted |
| A7 | `harn ~/2lab.ai/zbrain --agent codex --json` | AGENTS.md chain, skills roots, hooks, MCP servers, plugins match `codex -C … debug prompt-input` + `codex mcp list --json` + `codex plugin list --json`; the three trust gates are reflected (zbrain is `trusted`) |
| A8 | `harn verify --agent codex` | 0 diffs on instructions, skills listing and blocks; the report states that tools and hook output are outside Codex's dump |
| A9 | same binary on `cachy-z13` (`harn --version`, A1, A2) | identical columns and semantics; Linux paths (`/etc/claude-code/CLAUDE.md`, XDG Trash) resolved |
| A10 | `harn --readonly off anything` | exits non-zero without writing |
| A11 | an item whose config entry cannot be found (the 2026-10-02 `blender` case) | row shows origin `unresolved` with the evidence it has (transport, log dir, last connect), never a guessed path |

## Tensions

- **"llmux can edit the built-in prompt" (user's statement) vs llmux as shipped.** llmux records
  every request (`raw-io.jsonl`) and strips/injects credentials; it does not rewrite prompts
  (`llmux/.prd/01-spec.md:75`). Consequence: layer 1 is *viewable* through llmux and harn, not
  *editable*. Editing it is a possible future llmux feature, outside this spec.
- **MCP description cap**: docs say 2,048 chars; CHANGELOG 2.1.296 says 4,096. harn reads the
  truncation from the debug log when present, otherwise shows the untruncated size with a note.
- **Deferred tools "minimal impact"** (docs) vs ~20–27k tokens of deferred listings reported in
  issues. harn shows the deferred stub size it can measure, never a claim of "free".
- **Codex `[[skills.config]]`**: docs say a folder path, the run showed only the `SKILL.md` path
  works. harn writes the file path.
- **Codex trust**: docs say untrusted projects skip `.codex/`; code says anything not explicitly
  trusted is skipped, and skills load regardless. harn follows the code (`research/02` §2–3).

## Open questions (each with the experiment that settles it)

1. Deferred-stub wire form on first-party Claude Code → the same raw-body capture with tool search
   on. Needed before `freed` for MCP under tool search can be `wire`.
2. Does `permissions.deny: ["Agent(x)"]` remove the roster line or only block the call? → `/context`
   agents rows before and after.
3. `CLAUDE_CODE_DISABLE_CLAUDE_MDS` vs managed CLAUDE.md → needs a test machine with sudo.
4. userEmail / currentDate injection has no documented switch → watch anthropics/claude-code#81138;
   harn shows the row with `knob: none`.
5. Codex `skills.config` key semantics beyond `path`/`name` → `[미확인]` in `research/02`.

## Versioning, repository, distribution

Repository `2lab-ai/harn`, MIT, public. Crate and binary `harn`. Semver in `Cargo.toml`; stable
release on `v*` tags, rolling preview on every `main` push; two Homebrew formulae in
`2lab-ai/homebrew-tap` (`harn`, `harn-preview`); AUR `harn-bin` for CachyOS; release assets as
`.tar.gz` with completions and man page plus `SHA256SUMS`. `harn --version` prints version, build
channel and 12-char revision, the xfx convention. Mechanics: [`02-architecture.md`](02-architecture.md).
