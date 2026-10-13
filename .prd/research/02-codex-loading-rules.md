---
title: Codex CLI loading rules (codex-cli 0.160.1) — what reaches the model request
type: research
tags: [loadout, codex, prd]
---

# 02 — OpenAI Codex CLI: what is loaded into the model request

Scope: `codex-cli 0.160.1` only. Source pinned to tag `rust-v0.160.1` = commit `d27764b8` (refs `[cN]` below are permalinks).
Docs: `developers.openai.com/codex/*` now 308-redirects to `learn.chatgpt.com/docs/*` (refs `[dN]`).
Local runs `[LN]`: 2026-10-13 on this Mac, scratch git repo with marker files (`ROOT-AGENTS-MARKER-111` etc.).
`[미확인]` = not confirmed. Docs-vs-code conflicts are listed under **Tensions**, not silently resolved.

## 0. Headlines for the PRD

1. **No single offline "what the model sees" dump exists.** `codex debug prompt-input` returns only `Prompt.input` [c41] — it omits the
   `instructions` field (base instructions), the `tools` array, and all hook-injected context (SessionStart/UserPromptSubmit run only
   inside `run_turn` [c31]; `prompt_debug.rs:89` says it builds the request "without entering run_turn"). It also **spawns MCP stdio
   servers** (a `node_repl` child of the prompt-input PID was observed [L5]) and **writes into `CODEX_HOME`** (fresh home gained
   `installation_id`, `skills/.system`, `shell_snapshots` [L1]). No rollout is written (`ephemeral = true` [c43]).
2. **Recipe for loadout (best available, all official surfaces):**
   `codex -C <dir> debug prompt-input` (input list, honours `-c/--enable/--disable/--profile`) + `codex debug models` →
   `base_instructions` of the effective model (unless `model_instructions_file`/`instructions` override it [c17]) + `codex mcp list --json`
   (servers incl. plugin-provided; **no tool schemas**) + loadout's own tool enumeration. The only official *full request* capture is
   `CODEX_ROLLOUT_TRACE_ROOT=<dir>` (records the exact `ResponsesApiRequest{instructions,input,tools}` [c45]) — needs a live model call.
3. **Three different trust gates** (most likely source of loadout being wrong for a directory):
   AGENTS.md chain is skipped only if *explicitly* `trust_level="untrusted"` [c3][c9]; project `.codex/config.toml`, `.codex/hooks.json`,
   `.codex/rules/` load only if *explicitly trusted* — unset = disabled [c13] (PROJCFG marker absent until trusted [L1]); **skills ignore
   trust entirely** — `.codex/skills` is read even from disabled layers and `.agents/skills` is probed independently [c20][c21][c15] [L1].
4. **MCP tools may be deferred**: when the model's catalog entry has `supports_search_tool=true` (true for `gpt-6-astra` [L4]) deferrable
   tools (MCP) leave the upfront `tools` list and sit behind `tool_search` [c39][c40]. Token accounting must model this.
5. **Disabling is config-key based** (no "skip this file" flags). Gotcha: `-c plugins."x@y".enabled=false` is silently ignored because
   `-c` splits keys on `.` with no quote handling [c16]; inline-table form `-c 'plugins={"x@y"={enabled=false}}'` works [L1 t9].

### Request anatomy observed (gpt-6-astra, trusted-less scratch repo) [L1][c42][c47]
| Request field | Content (order as rendered) |
|---|---|
| `instructions` | base instructions: `model_instructions_file` > `instructions` key > model-catalog template (~21.4k chars) [c17][c19][L4] |
| `input[0]` developer | `developer_instructions` (if set) · `<skills_instructions>` · `<permissions instructions>` · `<collaboration_mode>` · `<apps_instructions>` · `<plugins_instructions>` · `<recommended_plugins>` [L1][L6] |
| `input[1..2]` developer | `<multi_agent_role>` · `<multi_agent_mode>` [L1] |
| `input[3]` user | `# AGENTS.md instructions for <cwd>` + `<INSTRUCTIONS>…</INSTRUCTIONS>` · `<environment_context>` (cwd, shell, date, tz, workspace roots) [c6][L1] |
| `input[4]` user | the user prompt |
| runtime-only | hook `additionalContext` → extra developer messages [c30]; MCP/app tool schemas, built-in tools → `tools[]` [c47] |

## 1. Instruction files
| Source | Discovery rule / path | Precedence / merge | Lands in | Disable (official) | Evidence |
|---|---|---|---|---|---|
| Global | `$CODEX_HOME/AGENTS.override.md`, else `$CODEX_HOME/AGENTS.md` (first non-empty, trimmed) | first in block; then `\n\n--- project-doc ---\n\n`; **not** charged to `project_doc_max_bytes` | user msg `# AGENTS.md instructions` | move/empty file, or other `CODEX_HOME`; no config key [미확인] | [c5][c3][d1][L1 fake-home] |
| Project chain | root = nearest ancestor with a `project_root_markers` entry (default `[".git"]`); every dir root→cwd; no marker → cwd only; `[]` → cwd only; never above root | concatenated root→cwd joined by `\n\n` (deeper = later) | same user msg | `project_doc_max_bytes=0` (global stays) or `projects."<p>".trust_level="untrusted"` | [c1][c3][c7][d1][d4][L1] |
| Per-dir pick | `AGENTS.override.md` > `AGENTS.md` > each `project_doc_fallback_filenames` entry; **first hit per dir only** | override *replaces* AGENTS.md in that dir | — | rename file | [c4][L1: 444 replaced 222] |
| `project_doc_max_bytes` | default 32768, one budget shared by all project files; last file truncated | — | — | `0` = no project docs | [c3 L68-95][c7] |
| `project_doc_fallback_filenames` | default `[]`; bare filenames only (`/` rejected) | after AGENTS.md | — | drop entry | [c4][c7][L1: CLAUDE.md read only when listed] |
| `project_root_markers` | read from non-project layers only (repo cannot move its own root) | — | — | — | [c1 L200-214] |
| `.codex/` dir | **not** an AGENTS location; holds `config.toml`, `hooks.json`, `rules/`, `skills/` | — | — | trust (see §2) | [c14][c20][c25][c32] |
| `instructions.md` | no such discovery for prompts in 0.160.1 (only memories-extension internals) | — | — | — | grep of tag [c-src] |
| `developer_instructions` key | config string | per layer merge | developer msg, first item | unset | [c8][L1 DEVINSTR] |

## 2. config.toml layering (low → high) and keys that change the request
| Layer | Path | Merge / precedence | Notes | Evidence |
|---|---|---|---|---|
| packaged defaults | embedded `defaults.toml` | lowest | sets `project_doc_*`, `include_*=true`, `project_root_markers=[".git"]` | [c7][c11] |
| macOS MDM / system / cloud | managed prefs · `/etc/codex/config.toml` · enterprise bundle | above defaults | system layer also roots `/etc/codex/skills` (admin) | [c11][c20] |
| user | `$CODEX_HOME/config.toml` (`CODEX_HOME` default `~/.codex`) | | `hooks.json`, `rules/`, `skills/` beside it | [c11][d3] |
| profile | `--profile n` → `$CODEX_HOME/n.config.toml` on top of user | legacy `[profiles.n]`/`profile=` + `--profile` = hard error (≥0.134.0) | `codex --help`: "Layer $CODEX_HOME/<name>.config.toml" | [c12][d4][L7] |
| project | `<dir>/.codex/config.toml` for each dir root→cwd, closest wins | **only if trusted**; keys `model_provider(s)`, `notify`, `profile(s)`, `otel`, base URLs ignored | `/debug-config` shows layers | [c13][c14][c10][d3] |
| session flags | `-c k=v` (TOML value; key split on `.`), `--enable/--disable F`, `-m`, `-s`, `--search` | above project | quoted keys not supported → use inline tables | [c16][c11] |
| legacy managed | `managed_config.toml` (file/MDM) | top | plus `requirements.toml` constraints (e.g. `allow_managed_hooks_only`) | [c11][d2] |

Env: `CODEX_HOME` (home), `CODEX_SQLITE_HOME`, `CODEX_ROLLOUT_TRACE_ROOT` (trace) [c45][d2].

| Key | Effect on model input | Disable / knob | Evidence |
|---|---|---|---|
| `model` | picks catalog entry → base instructions, tool shapes (`shell_type`, `apply_patch_tool_type`), `supports_search_tool`, context window (→ skills budget) | `-m` | [L4][c19][c22] |
| `model_reasoning_effort` | request `reasoning` param only, no text [미확인 beyond struct] | — | [c45 common.rs] |
| `features.*` | toggles tool families / blocks (e.g. `plugins`, `apps`, `hooks`, `shell_tool`, `unified_exec`, `memories`, `image_generation`) | `--disable F`, `codex features disable F` | [d2][L2] |
| `mcp_servers.<id>` | tool schemas (+server instructions) | `.enabled=false`, `enabled_tools`/`disabled_tools` | [d2][c38] |
| `plugins."p@m"` / `marketplaces.*` | plugin skills, MCP, apps, hooks, `<plugins_instructions>` | `enabled=false`; `features.plugins=false` removes all [L1 t8] | [d2][c35] |
| `projects."<p>".trust_level` | gates project config/hooks/rules; `untrusted` also drops project AGENTS | set `trusted`/`untrusted` | [c13][c3] |
| `sandbox_mode` / `approval_policy` / `approvals_reviewer` | text of `<permissions instructions>` (+ approved rule prefixes) | `include_permissions_instructions=false` | [c33][c8] |
| `shell_environment_policy` | subprocess env only; no prompt text found [미확인] | — | [d2] |
| `notify`, `tui.*` | none (external program / UI); `notify` is project-denylisted | — | [c10][d4] |
| `hooks` / `[hooks.state]` | runtime developer messages | see §4 | [c25][c27] |
| `web_search` | adds `web_search` tool | `web_search="disabled"` | [d2] |
| `include_{permissions,apps,collaboration_mode}_instructions`, `include_environment_context` | drop those blocks | `=false` | [c8][c17][L1 t11] |
| `agents.enabled` | multi-agent tools + `<multi_agent_role>/<multi_agent_mode>` | `agents.enabled=false` (`--disable multi_agent` did **not** remove the role block) | [c18][L1 t11] |

## 3. Skills
| Source | Path | Precedence | Lands in | Disable | Evidence |
|---|---|---|---|---|---|
| Repo `.agents/skills` | `<dir>/.agents/skills` for each dir root→cwd | trust-independent | `<skills_instructions>` catalog | `[[skills.config]]` | [c21][d5][L1 REPOSKILL] |
| Repo `.codex/skills` | `<dir>/.codex/skills` per project layer, **incl. disabled/untrusted layers** | — | same | same | [c20 L86-93][c15][L1 DOTCODEXSKILL] |
| User | `$CODEX_HOME/skills` (deprecated, still read) + `~/.agents/skills` | — | same | same | [c20 L95-108][d5] |
| System (bundled) | `$CODEX_HOME/skills/.system` (installed on start) | catalog order System > Admin > Repo > User | same | `skills.bundled.enabled=false` [L1 t6] | [c20][c22 L48-56] |
| Admin | `/etc/codex/skills` | — | same | — | [c20 L115-119][d5] |
| Plugin | `~/.codex/plugins/cache/<mkt>/<plugin>/<ver>/skills/*`, shown as `plugin:skill` | — | same | disable plugin | [L1][c35] |

Surfacing: one developer block = roots table (`r0`…) + `- name: description (file: rN/…/SKILL.md)`; the **SKILL.md body is not loaded** until
used (marker `BODY-SECRET-999` absent [L1]). Budget = 2% of context window, cap 10 000 tokens (`skills.max_context_tokens`), description
≤1024 chars [c22][d2]. Frontmatter: `name`, `description` required; optional `agents/openai.yaml` (UI, `allow_implicit_invocation`,
dependencies) [d5]. Disable: `[[skills.config]] path="<abs>/SKILL.md" enabled=false` (folder path did **not** work [L1 t4/t5]) or
`name="x"` [c23][c24][L1 t6]; `skills.include_instructions=false` drops the block [c23][L1]. Invocation `$skill`, `/skills` [d5][c46].

## 4. Hooks
| Source | Path | Merge | Output lands | Disable | Evidence |
|---|---|---|---|---|---|
| User | `$CODEX_HOME/hooks.json` and/or inline `[hooks]` in config.toml | all layers append (no override); both forms in one layer = merged + warning | — | `[hooks.state."<key>"] enabled=false` | [c25][d6] |
| Project | `<dir>/.codex/hooks.json` / `[hooks]` | only if project trusted | — | trust / state | [c13][d6] |
| Plugin | plugin `hooks` (manifest or `hooks/hooks.json`), env `PLUGIN_ROOT`+`CLAUDE_PLUGIN_ROOT` | — | — | disable plugin | [c26][c35][d6] |
| Managed | `requirements.toml` | always trusted | — | `allow_managed_hooks_only` skips others | [d2][c27] |

Events: `SessionStart, SessionEnd, PreToolUse, PostToolUse, PermissionRequest, PreCompact, PostCompact, UserPromptSubmit, SubagentStart,
SubagentStop, Stop, Interrupt`; schema `{"hooks":{"Event":[{"matcher":"re","hooks":[{"type":"command","command":"…","timeout":600,
"statusMessage":"…"}]}]}}`; command + MCP-tool handlers run, prompt/agent handlers parsed but skipped [d2][d6]. **Output → model**:
SessionStart/SubagentStart plain stdout or JSON `additionalContext` becomes a **developer** message (no wrapper markers) recorded in
history; oversized context (>`additionalContextLimit`, default 2500 tokens) spills to disk with a preview [c29][c30][d2]. Trust: key =
`<source_path>:<event_snake>:<group>:<handler>` (matches local `hooks.json:session_start:0:0`); runs only if enabled **and**
`trusted_hash` == current hash (else Untrusted/Modified → skipped) [c28][c27]. Global off: `features.hooks=false` [d6]; one-shot bypass
`--dangerously-bypass-hook-trust` [L7]. Matcher value set per event [미확인].

## 5. Rules (exec policy)
| Source | Path | Precedence | Reaches model? | Disable | Evidence |
|---|---|---|---|---|---|
| User | every `*.rules` in `$CODEX_HOME/rules/` (`default.rules` = where approvals are appended) | layers low→high; most restrictive wins (`forbidden`>`prompt`>`allow`) | allow-prefixes rendered as `## Approved command prefixes` inside `<permissions instructions>` only for the bundled OnRequest/Granular template (≤100 prefixes / 5000 B); **not** rendered in our `auto_review` run | remove/rename file; no toggle [미확인] | [c32][c33][c34][d9][L1] |
| Project | `<dir>/.codex/rules/` | trusted only | same | trust | [c32][c13][d9] |
| Admin | `requirements.toml` rules | enforced | same | — | [d9] |

Syntax: Starlark `prefix_rule(pattern=[…], decision="allow|prompt|forbidden", justification=…, match=…, not_match=…)`; test with
`codex execpolicy check --rules <f> -- <cmd>` [d9]. Primary purpose = sandbox-escape approval, not prompt content [d9].

## 6. Plugins & marketplaces
| Item | Rule / path | Lands in | Disable | Evidence |
|---|---|---|---|---|
| Marketplace config | `[marketplaces.<n>] source_type=git/local, source, ref, sparse_paths` | — | remove entry / `codex plugin marketplace remove` | [d2][L local config] |
| Marketplace manifest | `.agents/plugins/marketplace.json`, `.agents/plugins/api_marketplace.json`, `.claude-plugin/marketplace.json`, `.cursor-plugin/marketplace.json` | — | — | [c37] |
| Plugin manifest | `<plugin>/.codex-plugin/plugin.json`: `skills`, `mcpServers`, `apps`, `hooks`, `interface` | — | — | [c36][c35] |
| Install cache | `~/.codex/plugins/cache/<marketplace>/<plugin>/<version>/` | skill roots point here [L1] | `codex plugin remove` (deletes cache) | [L1][d7] |
| Enablement | `[plugins."name@marketplace"] enabled=true/false` | skills, MCP (e.g. `computer-use`, `event-stream` in `codex mcp list` [L3]), hooks, apps, `<plugins_instructions>` | `enabled=false`; per bundled server `plugins.<p>.mcp_servers.<s>.enabled=false`; all: `features.plugins=false` | [d2][L1 t8/t9] |
| Suggestions | `<recommended_plugins>` lists **uninstalled** plugins (costs tokens) | developer block | `features.plugins=false` removed it [L1 t8]; finer knob [미확인] | [L1] |

CLI: `codex plugin add | list [--json --available -m] | remove | marketplace (add|list|upgrade|remove)`; TUI `/plugins` [L7][c46].

## 7. MCP
| Item | Rule | Lands in | Disable | Evidence |
|---|---|---|---|---|
| Server def | `[mcp_servers.<id>]` stdio `command,args,env,env_vars,cwd` / HTTP `url,bearer_token_env_var,http_headers,auth`; user or trusted project layer | — | `enabled=false` (keeps config) | [d2][d8] |
| Tool filter | `enabled_tools` (allow) then `disabled_tools` (deny); per-tool `approval_mode`, `output_token_limit` | `tools[]` | list edits | [d2] |
| Startup | `startup_timeout_sec` (10), `tool_timeout_sec` (60), `required`, `mcp_optional_startup_grace_ms` (1000) | — | — | [d2][c8] |
| Server `instructions` | from MCP `initialize` → tool **namespace description** | `tools[]` (not a message) | disable server | [c38] |
| Deferral | deferrable tools hidden behind `tool_search` when model `supports_search_tool` | `tools[]` shrinks | model choice | [c39][c40][L4] |

CLI: `codex mcp list [--json] | get | add | remove | login | logout`; TUI `/mcp` (`/mcp verbose`) [L3][c46].

## 8. Inspection surfaces (can loadout delegate?)
| Surface | Output | Base instr | AGENTS/skills/blocks | Tools | Hook output | Side effects | Evidence |
|---|---|---|---|---|---|---|---|
| `codex -C <dir> debug prompt-input [-c…] [PROMPT]` | JSON array of ResponseItems (`Prompt.input`) | no | **yes** (exact rendered text) | no | no | spawns MCP servers; writes CODEX_HOME; no rollout | [c41][c43][L1][L5] |
| `codex debug models [--bundled]` | model catalog JSON incl. `base_instructions`, `supports_search_tool`, tool types | yes (template) | — | — | — | refreshes catalog [미확인 network] | [L4][d10] |
| `codex mcp list --json` | name, enabled, disabled_reason, transport, timeouts, auth_status | — | — | servers only | — | — | [L3] |
| `codex plugin list --json` / `codex features list` | plugins / flag name+stage+effective | — | — | — | — | — | [L2][L7] |
| `codex doctor --json` | 24 health checks (config.load lists enabled features, mcp.config counts) | no | no | no | no | — | [L doctor] |
| TUI `/status` `/mcp` `/skills` `/hooks` `/plugins` `/debug-config` | session config, tools, skills, hooks, config layers | — | partial | `/mcp` lists tools | `/hooks` | interactive only | [c46] |
| Rollout `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | `session_meta.base_instructions` (full), developer/user context `response_item`s, `turn_context`, `world_state` | **yes** | **yes** | **no** (only `dynamic_tools`) | yes (as items) | written by real sessions | [c44][L6] |
| `CODEX_ROLLOUT_TRACE_ROOT=<dir>` | trace bundle; `record_started(&request)` stores the exact provider request (`model, instructions, input, tools, …`) | yes | yes | **yes** | yes | live model call; sensitive, local-only | [c45] (code-verified, not run) |
| `RUST_LOG` / `~/.codex/logs_2.sqlite` | tracing logs; no request-body logging found in the responses path | — | — | — | — | — | [미확인 beyond grep] |

Verdict: for a static, pre-session listing loadout should call `debug prompt-input` (authoritative for the `input` list, since it runs the
real `build_prompt` path) and `debug models` (base instructions), but must itself enumerate tools (built-ins + MCP `tools/list` + deferral)
and hooks; for ground truth after a run, parse the rollout or a `CODEX_ROLLOUT_TRACE_ROOT` bundle.

## 9. Built-ins and their official knobs
| Built-in | Where | Knob | Evidence |
|---|---|---|---|
| Base instructions | `instructions` field | `model_instructions_file` (code comment: "STRONGLY DISCOURAGED"); `instructions` key = 3rd fallback; `compact_prompt` for compaction | [c8 L266-270][c17] |
| `experimental_instructions_file` | — | **absent** from 0.160.1 source (superseded by `model_instructions_file`) | grep [c-src] |
| Permissions / collaboration / apps / env-context blocks | developer/user msgs | `include_*=false` | [c8][L1 t11] |
| Multi-agent role + tools | developer msgs + tools | `agents.enabled=false` | [c18][L1] |
| Shell / exec (`exec_command`, `write_stdin`) | tools | `features.shell_tool`, `features.unified_exec` | [d2][L2] |
| `web_search` | tool | `web_search="disabled"` / `--search` = live | [d2][L7] |
| `update_plan`, `request_user_input` | tools | `tools.update_plan.enabled`, `tools.experimental_request_user_input.enabled` | [c-tools] |
| `view_image`, `image_generation`, goals, memories | tools / blocks | `--disable view_image` etc.; `memories.use_memories=false` | [L2][d2] |
| `apply_patch` | tool | type comes from model catalog `apply_patch_tool_type`; config knob [미확인] | [L4] |

Floor observed with every toggle off: input = `<multi_agent_role>` + `<multi_agent_mode>` + prompt; with `agents.enabled=false` only the
prompt remains in `input` — base instructions and core tools are not removable [L1 t11].

## 10. Cross-agent conventions
Codex does **not** read `CLAUDE.md` or `.cursorrules` by default (`project_doc_fallback_filenames = []` [c7]; `CLAUDE-MARKER-333` absent
[L1]) but will if listed, e.g. `project_doc_fallback_filenames=["CLAUDE.md"]`, only in dirs lacking AGENTS.md [c4][L1]. Codex plugin
marketplaces accept `.claude-plugin/marketplace.json` and `.cursor-plugin/marketplace.json` [c37], plugin hooks receive
`CLAUDE_PLUGIN_ROOT`/`CLAUDE_PLUGIN_DATA` [c26], and the TUI has `/import` ("import setup, this project, and recent chats from Claude
Code") [c46]. Claude Code (v2.1.277+) reads `AGENTS.md` natively when no `CLAUDE.md`/`CLAUDE.local.md` exists at or above cwd, controlled
by `pluginConfigs["cc-plugin-agents-md@builtin"].options.instructionFiles` (`claude-md-or-agents-md` default, `claude-md-and-agents-md`,
`managed-only`) [d11]. AGENTS.md (https://agents.md/) is stewarded by the Agentic AI Foundation (Linux Foundation); its compatibility list
includes Codex, Cursor, Gemini CLI, Copilot agent etc. but not Claude Code [d12]. De-facto sync tool: `ruler` (`.ruler/` → per-agent files
incl. Codex `AGENTS.md` + `.codex/config.toml`) [d13].

## Tensions (docs vs code/runs)
- Docs: "`[[skills.config]]` path = skill folder" [d2] vs run: only the `SKILL.md` file path disabled the skill [L1 t4/t5].
- Docs: "Untrusted projects skip `.codex/` layers" [d2][d3] vs code: anything not explicitly trusted is disabled [c13]; and skills under
  `.codex/skills` load regardless of trust [c20][L1].
- Loader doc comment lists `cwd ${PWD}/config.toml` [c11 L131] vs code: only `<dir>/.codex/config.toml` is probed [c14].
- Docs: `model_instructions_file` = "Replacement for built-in instructions instead of AGENTS.md" [d2] vs code: replaces base instructions;
  AGENTS.md still loads [c17][c3]. Docs: `instructions` "reserved" [d2] vs code: used as base-instruction fallback [c17].
- Docs list `tools.view_image` [d2] vs 0.160.1 `ToolsToml` (deny_unknown_fields) has only `web_search`, `experimental_request_user_input`,
  `update_plan` [c-tools]; `view_image` is a feature flag [L2].
- `codex features list` shows `recommended_plugins … false` yet `<recommended_plugins>` was rendered [L2][L1].

## Open questions / unconfirmed
1. Exact `tools[]` for a given dir/model — needs one live run with `CODEX_ROLLOUT_TRACE_ROOT=$(mktemp -d) codex exec --disable hooks "ok"`
   then read `payloads/*.json` (not run: costs a model call and writes a session).
2. Does `debug prompt-input` / `debug models` hit the network (catalog refresh, remote plugin catalog)? Spawning MCP was observed; network not measured.
3. Which knob alone removes `<recommended_plugins>` and `<plugins_instructions>` without disabling all plugins.
4. Whether `include_apps_instructions=false` plus `features.apps=false` removes connector tools from `tools[]` (input-side only verified).
5. Hook matcher values per event (e.g. SessionStart `startup|resume|clear`), and whether non-SessionStart hook stdout ever reaches the model.
6. Repo-level auto-discovery of `<repo>/.agents/plugins/marketplace.json` (only `~/.agents/...` + configured roots seen in code).
7. A per-file disable for `rules/*.rules` and for the global `AGENTS.md` (none found).
8. Whether rendered `base_instructions` (personality/template substitution) equals `debug models` text byte-for-byte.

## References
Code base: `https://github.com/openai/codex/blob/d27764b82f7118f674371e6d6e76271d9d606edb/codex-rs/` + path below.
- c1 `core/src/agents_md.rs#L1-L18` · c3 `#L56-L187` · c4 `#L272-L296` · c6 `core/src/context/user_instructions.rs#L10-L30` · c5 `codex-home/src/instructions/mod.rs#L40-L84`
- c7 `config/defaults.toml#L1-L14` · c8 `config/src/config_toml.rs#L247-L270` · c9 `config/src/config_toml.rs#L601-L615`
- c10 `config/src/loader/mod.rs#L84-L101` · c11 `#L122-L136` · c12 `#L286-L334` · c13 `#L1086-L1104` · c14 `#L1640-L1700`
- c15 `config/src/state.rs#L545-L575` · c16 `config/src/overrides.rs#L22` · c17 `core/src/config/mod.rs#L3988-L4019`
- c18 `core/src/config/mod.rs#L3874-L3878` + `#L1586-L1594` · c19 `core/src/session/mod.rs#L744-L748`
- c20 `ext/skills/src/host_roots.rs#L73-L131` · c21 `#L137-L185` · c22 `ext/skills/src/render.rs#L19-L66` · c23 `config/src/skills_config.rs#L18-L46` · c24 `#L90-L213`
- c25 `hooks/src/engine/discovery.rs#L129-L187` · c26 `#L244-L290` · c27 `#L794-L821` · c28 `hooks/src/lib.rs#L113-L122`
- c29 `hooks/src/events/session_start.rs#L218-L310` · c30 `core/src/hook_runtime.rs#L849-L873` + `core/src/context/hook_additional_context.rs#L15-L22` · c31 `core/src/session/turn.rs#L322`
- c32 `core/src/exec_policy.rs#L54-L56` + `#L662-L682` · c33 `prompts/src/permissions_instructions.rs#L33-L34` + `#L286-L345` · c34 `protocol/src/models.rs#L1568-L1572`
- c35 `core-plugins/src/manifest.rs#L50-L72` · c36 `core-plugins/src/store.rs#L474` · c37 `core-plugins/src/marketplace.rs#L20-L25`
- c38 `codex-mcp/src/rmcp_client.rs#L839-L856` · c39 `core/src/tools/spec_plan.rs#L246-L254` + `#L653-L666` · c40 `features/src/lib.rs#L1414-L1424`
- c41 `core/src/prompt_debug.rs#L24-L113` · c42 `core/src/client_common.rs#L21-L43` · c43 `cli/src/main.rs#L1970-L2063` · c47 `core/src/session/turn.rs#L1583-L1600`
- c44 `history/src/lib.rs#L201-L216` + `protocol/src/protocol.rs#L3170-L3176` · c46 `tui/src/slash_command.rs#L94-L152`
- c45 `rollout-trace/README.md#L1-L6` + `rollout-trace/src/thread.rs#L44` + `rollout-trace/src/inference.rs#L169-L174` + `core/src/client.rs#L1757` + `codex-api/src/common.rs#L280-L301`
- c-tools `config/src/config_toml.rs#L670-L694` · c-src = `rg` over the whole tag (no hits for `experimental_instructions_file`; `"instructions.md"` only in memories code)
Docs: d1 https://learn.chatgpt.com/docs/agent-configuration/agents-md · d2 https://learn.chatgpt.com/docs/config-file/config-reference ·
d3 https://learn.chatgpt.com/docs/config-file/config-basic · d4 https://learn.chatgpt.com/docs/config-file/config-advanced ·
d5 https://learn.chatgpt.com/docs/build-skills · d6 https://learn.chatgpt.com/docs/hooks · d7 https://learn.chatgpt.com/docs/plugins ·
d8 https://learn.chatgpt.com/docs/extend/mcp?surface=cli · d9 https://learn.chatgpt.com/docs/agent-configuration/rules ·
d10 https://learn.chatgpt.com/docs/developer-commands?surface=cli · d11 https://code.claude.com/docs/en/memory#agents-md ·
d12 https://agents.md/ · d13 https://github.com/intellectronica/ruler (originals: https://developers.openai.com/codex/… 308 → learn.chatgpt.com)
Local (2026-10-13, codex-cli 0.160.1): L1 `codex debug prompt-input` marker runs t1–t11 in a scratch git repo (+ fake `CODEX_HOME`) ·
L2 `codex features list` · L3 `codex mcp list --json` · L4 `codex debug models --bundled` · L5 `pgrep -P <prompt-input pid>` → `node_repl` ·
L6 newest `~/.codex/sessions/2026/10/13/rollout-*.jsonl` keys · L7 `codex --help` / subcommand `--help` · L doctor `codex doctor --json`.
