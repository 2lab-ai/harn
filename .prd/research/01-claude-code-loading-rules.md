---
title: Claude Code 2.1.x — what gets loaded into the model request, and how to turn each piece off
type: research
tags: [harn, claude-code, prd, context, harness]
---

# 01 — Claude Code loading rules (for `harn`)

Scope: Claude Code native install, installed version `2.1.296`. Docs fetched 2026-10-13 from `code.claude.com/docs/en/*`.
`docs.anthropic.com/en/docs/claude-code/*` now returns 301 to that host. Every row cites its source. `[미확인]` = not confirmed
from a primary source. `[local]` = measured on this machine on 2026-10-13.

**Legend for "lands in":** `SP` = `system` field · `T` = `tools` array · `R0` = `<system-reminder>` inside the first user
message (`messages[0]`) · `R1` = a reminder sent as a separate system-role message (it was `messages[1]` in the 2026-10-02 capture) ·
`H` = added to conversation history mid-session (lazy load).
Docs: *"a system reminder appears wrapped in `<system-reminder>` tags inside a user message, or on some models as a separate
message with the `system` role"* ([glossary][gl]). CLAUDE.md is *"delivered as a user message after the system prompt, not as part
of the system prompt itself"* ([mem-trouble][mem-tr]).

## 0. Request anatomy (docs + 2026-10-02 capture)

| Region | Contents | Evidence |
|---|---|---|
| `SP` | Core prompt (not published), environment info (cwd, platform, shell, OS, git-repo flag), auto-memory *location*, `--append-system-prompt` text. A custom output style also removes the SE-instructions section here. | [settings: "system prompt isn't published"][set-chg] · [ctx-window][cw] · [cli `--exclude-dynamic-system-prompt-sections`][cli] · [output-styles][os] |
| `T` | Built-in tools (Bash description carries git commit/PR instructions), then MCP tools. MCP tools are sent upfront, or as `defer_loading` stubs when tool search is on | [includeGitInstructions][sr-git] · [env `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` names `defer_loading`][env] · [tool search][mcp-ts] |
| `R0` | CLAUDE.md chain + rules + imports + MEMORY.md · git status snapshot · userEmail · attribution text · the prompt. `currentDate` is `[미확인]`: #81138's builder groups it with userEmail, but this session shows `Today's date is …` inside `SP` | [sdk reminders][sdk-rem] · [#81138][i81138] (community) · [local] |
| `R1` | SessionStart hook output · available-subagents roster · MCP server instructions · skill listing (+ deferred-tool name list when tool search is on) | [sdk reminders][sdk-rem] · [hooks additionalContext][hk-ctx] · [mcp-ts][mcp-ts] |

## 1. Instruction files

| Source | Discovery / path(s) | Precedence / merge | Lands | Disable without deleting | Evidence |
|---|---|---|---|---|---|
| Managed CLAUDE.md | macOS `/Library/Application Support/ClaudeCode/CLAUDE.md`, Linux `/etc/claude-code/CLAUDE.md`, Win `C:\Program Files\ClaudeCode\CLAUDE.md`. The managed-settings key `claudeMd` holds the same content inline | Loaded first. *"cannot be excluded"* | R0 | Not possible as a user. `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` lists user/project/auto memory and does not name managed `[미확인]` | [mem-managed][mem-mg] · [env][env] |
| User | `~/.claude/CLAUDE.md` + `~/.claude/rules/**/*.md` | After managed. User rules load before project rules | R0 | `claudeMdExcludes` glob (any layer, arrays merge) · `--setting-sources` without `user` (SDK doc confirms this drops its CLAUDE.md) | [mem-locations][mem-loc] · [user rules][mem-ur] · [sdk turn-off][sdk-off] |
| Project chain | `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md` in cwd **and every ancestor** | Concatenated, never overriding. Order runs from filesystem root down to cwd. `CLAUDE.local.md` comes after `CLAUDE.md` in each directory | R0 | `claudeMdExcludes` · `/config` → Project instructions = `managed-only` · `--setting-sources` without `project`/`local` · `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` · `--bare` / `--safe-mode` | [how-load][mem-hl] · [choose-files][mem-cf] · [cli][cli] |
| Subdirectory CLAUDE.md | Below cwd. Loads when Claude reads/writes/edits a file in that subdirectory (a single-file `cat`/`head` counts) | Loads once. Not listed under `/context` Memory files | H | `claudeMdExcludes` | [how-load][mem-hl] |
| `AGENTS.md` (v2.1.277+) | Read instead of CLAUDE.md only when no CLAUDE.md/CLAUDE.local.md exists in cwd or above it. Provided by built-in plugin `cc-plugin-agents-md@builtin` | Setting values: `claude-md-or-agents-md` (default) / `claude-md-and-agents-md` / `claude-md` / `managed-only` | R0 | `pluginConfigs."cc-plugin-agents-md@builtin".options.instructionFiles` · disable the built-in plugin in `/plugin` | [agents-md][mem-ag] |
| `@path` imports | Inside any memory file. Paths resolve relative to the importing file. Max depth 4 hops. Code spans and fences are skipped | Expanded inline at launch | R0 | Wrap the path in backticks · decline the external-import dialog (project files only) | [imports][mem-imp] |
| `.claude/rules/**/*.md` | Recursive search. No `paths:` → loads at launch with `.claude/CLAUDE.md` priority. With `paths:` → loads when a matching file is read | User rules, then project rules | R0 / H | `claudeMdExcludes` · add `paths:` to make it lazy | [rules][mem-rules] |
| `--add-dir` dirs | CLAUDE.md files are not loaded unless `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` is set. When it is set: `CLAUDE.md`, `.claude/CLAUDE.md`, `.claude/rules/*.md`, `CLAUDE.local.md` | — | R0 | Unset the env var | [add-dir][mem-ad] · [perm add-dir][perm-ad] |
| Limits | Files over 4 MiB are skipped. Block HTML comments are stripped. Startup warns past 200 lines/file and past a combined limit | — | — | — | [mem][mem-ht] |

## 2. Settings layering (what can change the request)

Precedence, highest first: **managed** (server-managed > MDM plist `com.anthropic.claudecode` > `managed-settings.d/*.json`+`managed-settings.json`)
→ `--settings` → `.claude/settings.local.json` → `.claude/settings.json` → `~/.claude/settings.json`. Lists merge across layers.
Env vars are *not* a layer: each var/key pair has its own rule ([settings-precedence][set-prec], [managed tiers][mg-del]).
Some restrictive values win from any layer, e.g. `disableClaudeAiConnectors: true` ([exceptions][set-exc]).

| Key | Effect on what is loaded | Off value | Evidence |
|---|---|---|---|
| `enabledPlugins` | Per-plugin on/off (`name@origin`). Six sources: `--add-dir` (true only) < user < project < local < `--settings` < managed. A project `true` beats a user `false`, so opt out in `settings.local.json` | `false` | [sr-ep][sr-ep] · [plug-enabled][pl-en] |
| `disableAllHooks` | Turns off user/project/local/plugin hooks, the status line and file-suggestion. Managed hooks keep running unless the key is set in managed settings | `true` | [sr-dah][sr-dah] |
| `permissions.deny` / `--disallowedTools` | A bare tool name *removes it from context*. `"mcp__*"` removes all MCP tools. `Agent(x)` and `Skill(x)` block use | add rule | [cli][cli] · [sub-agents disable][sa-dis] |
| `env` | Carries every `CLAUDE_CODE_DISABLE_*` / `ENABLE_TOOL_SEARCH` switch (§10). Telemetry vars and some others are ignored in project/local files | — | [sr-env][sr-env] · [env][env] |
| `disableClaudeAiConnectors` | Stops fetching claude.ai connectors (any-source-true) | `true` | [sr-dcc][sr-dcc] |
| `model`, `effortLevel` | Model/effort only. These keys select the system prompt variant (full vs short) indirectly through model choice | — | [sr-model][sr-model] · [env SIMPLE_SYSTEM_PROMPT][env] |
| `outputStyle` | Adds the style's instructions to every request | unset / `default` | [os][os] |
| `statusLine` | **Not in the request.** Runs locally with no API tokens | — | [statusline][sl] |
| `attribution` (`includeCoAuthoredBy` deprecated) | Commit/PR attribution reminder | `{"commit":"","pr":""}` | [sr-attr][sr-attr] · [sdk-off][sdk-off] |
| `includeGitInstructions` | Git instructions in the Bash tool description + git status snapshot | `false` | [sr-git][sr-git] |
| `autoMemoryEnabled`, `autoMemoryDirectory` | MEMORY.md injection | `false` | [mem-am][mem-am] |
| `claudeMdExcludes` | Skips matching CLAUDE.md/rules files (cannot skip managed) | glob list | [mem-ex][mem-ex] |
| `skillOverrides`, `disableBundledSkills`, `skillListingBudgetFraction`, `skillListingMaxDescChars`, `syncClaudeAiSkills` | Skill listing (§4) | see §4 | [sr-skills][sr-skills] |
| `enabledMcpjsonServers`/`disabledMcpjsonServers`/`enableAllProjectMcpServers`, `allowedMcpServers`/`deniedMcpServers` | MCP server admission (§7) | see §7 | [sr-mcp][sr-mcp] |
| `syncClaudeAiPlugins`, `pluginConfigs`, `strictPluginOnlyCustomization` (managed) | Plugin sources (§8) | `false` | [sr-sync-pl][sr-sync-pl] |
| `advisorModel` | Server-side advisor tool | unset | [sr-adv][sr-adv] |

## 3. Hooks

| Source | Discovery / path | Merge | Lands | Disable | Evidence |
|---|---|---|---|---|---|
| Settings hooks | `hooks` key in user/project/local/managed settings | Entries merge across levels and never replace each other | stdout only for the events below | `disableAllHooks` · `allowManagedHooksOnly` (managed) | [hook-loc][hk-loc] · [disable][hk-dis] |
| Plugin hooks | `<plugin>/hooks/hooks.json` (+ "mods" whose code registers hooks) | Active while the plugin is enabled | same | `enabledPlugins:false` · `disableAllHooks` | [hook-loc][hk-loc] · [sr-dah][sr-dah] |
| Skill / subagent frontmatter `hooks` | Registered when the skill is invoked / while the subagent runs | — | same | Disable the skill/agent | [hook-loc][hk-loc] |

Events (33 `###` event headings in the reference): SessionStart, Setup, InstructionsLoaded, UserPromptSubmit, UserPromptExpansion,
MessageDisplay, PreToolUse, PermissionRequest, PostToolUse, PostToolUseFailure, PostToolBatch, PermissionDenied, Notification,
SubagentStart, SubagentStop, TaskCreated, TaskCompleted, Stop, StopFailure, TeammateIdle, ConfigChange, CwdChanged, DirectoryAdded,
FileChanged, WorktreeCreate, WorktreeRemove, PreCompact, PostCompact, PreModelSwitch, PostModelSwitch, SessionEnd, Elicitation,
ElicitationResult ([hooks][hk]).
**Context injection rules:**
- Plain stdout becomes context only for `UserPromptSubmit`, `UserPromptExpansion`, `SessionStart`, and `PostModelSwitch` ([exit 0][hk-ex0]).
- `hookSpecificOutput.additionalContext` is wrapped in a system reminder that starts with the hook's name. For SessionStart/SubagentStart it lands *"at the start of the conversation, before the first prompt"*. For tool events it lands next to the tool result.
- Values over 10,000 chars are saved to a file, and Claude gets the path plus a 2,000-char preview ([add-context][hk-ctx]).
- SessionStart reruns on resume, clear, compact, and fork ([SessionStart][hk-ss]).

The 2026-10-02 capture places SessionStart output (3.4 KB) in `R1`.

## 4. Skills

| Source | Path | Precedence | Lands | Disable without deleting | Evidence |
|---|---|---|---|---|---|
| Enterprise | `<managed dir>/.claude/skills/<n>/SKILL.md` | Wins over personal, which wins over project | listing R1 | `CLAUDE_CODE_DISABLE_POLICY_SKILLS=1` | [where][sk-where] · [env][env] |
| Personal | `~/.claude/skills/<n>/SKILL.md` | — | listing R1 | `skillOverrides` | [where][sk-where] |
| Project | `.claude/skills/` in cwd and each parent up to the repo root | — | listing R1 | `skillOverrides` | [where][sk-where] |
| Nested | `<subdir>/.claude/skills/` | Loads when a file in that subdirectory is touched | H | `skillOverrides` | [sk-bundled-sec][sk-b] |
| `--add-dir` | `.claude/skills/` of the added dir | — | listing | drop the flag | [perm-ad][perm-ad] |
| Plugin | `<plugin>/skills/<n>/SKILL.md` → `/plugin:skill` | Namespaced, so names never collide | listing R1 | `enabledPlugins:false`. **`skillOverrides` does not apply** | [sr-skills][sr-skills] |
| claude.ai synced | `~/.claude/skills/synced/` (claude.ai login only, checked every ~10 min) | Loses to any local name. Shown as `anthropic-skills:<n>` only on collision (2.1.281) | listing R1 | `syncClaudeAiSkills:false` (user/local/managed/`--settings`) | [synced][sk-sync] · [CHANGELOG][cl] |
| Bundled | built into the CLI (`/doctor`, `/code-review`, `/loop`, `/claude-api`, ...) | Your skill replaces the bundled one, but not its aliases | listing R1 | `disableBundledSkills` / `CLAUDE_CODE_DISABLE_BUNDLED_SKILLS=1` | [bundled][sk-b] |
| Commands / workflows | `.claude/commands/*.md`; `.claude/workflows/*.js` → `/<name>` | A skill wins over a command with the same name | listing | `skillOverrides` [미확인 for workflows] | [where][sk-where] · [claude-dir][cd-ref] |

- **The listing has names plus `description`+`when_to_use`, capped at 1,536 chars per skill** (`skillListingMaxDescChars`).
- **Total budget is 1% of the context window** (`skillListingBudgetFraction`; `SLASH_COMMAND_TOOL_CHAR_BUDGET` sets a fixed character count, fallback 8,000). On overflow, names stay and the least-used skills lose their descriptions ([cut-short][sk-cut]).
- `disable-model-invocation: true` keeps the description **out** of context. `user-invocable: false` keeps it in context and hides `/name` ([control][sk-ctl]).
- **The body loads only when the skill is invoked**, as one message, and is re-attached after compaction up to 5k tokens per skill and 25k in total ([lifecycle][sk-life]).
- `skillOverrides` states: `on` / `name-only` / `user-invocable-only` / `off` ([override][sk-ovr]).
- `CLAUDE_CODE_DISABLE_ATTACHMENTS=1` drops the whole listing. The subagent roster still arrives ([sdk-off][sdk-off]).
- [local] counts: 22 entries in `~/.claude/skills`, 60 in zbrain `.claude/skills`, ~21 synced, plus plugin skills. The 2026-10-02 listing was 31 KB for 210 skills.

## 5. Subagents

| Source | Path | Priority (same name) | What reaches the model | Disable | Evidence |
|---|---|---|---|---|---|
| Managed | managed settings | 1 | roster entry (name + description) | admin only | [scope][sa-scope] |
| `--agents` JSON | CLI flag | 2 | roster | drop the flag | [scope][sa-scope] |
| Project | `.claude/agents/**` from cwd up to the repo root; the closest definition wins | 3 | roster | `permissions.deny: ["Agent(x)"]` / `--disallowedTools "Agent(x)"` | [scope][sa-scope] · [disable][sa-dis] |
| User | `~/.claude/agents/**` | 4 | roster | same | [scope][sa-scope] |
| Plugin | `<plugin>/agents/**` → `plugin:sub:name` | 5 | roster | `enabledPlugins:false` | [scope][sa-scope] |
| Built-in | Explore, Plan, general-purpose | — | roster | `CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS=1` | [env][env] |

- The roster is a system reminder: *"Available subagents: the names and descriptions"* ([sdk-rem][sdk-rem]). It was `R1` in the capture (16.9 KB).
- A combined description size over 15,000 tokens triggers a startup warning, and every subagent still loads ([delegation][sa-del]).
- The agent body is used only as the subagent's own system prompt, when the subagent is spawned ([startup][sa-start]).
- `[미확인]` whether an `Agent(x)` deny removes the roster line or only blocks the call.
- [local] 2 user agents, 13 zbrain project agents.

## 6. Slash commands, output styles, statusLine

| Item | Path | Lands | Disable | Evidence |
|---|---|---|---|---|
| Custom commands | `.claude/commands/`, `~/.claude/commands/`, plugin `commands/`, `--add-dir` | Same listing as skills | `skillOverrides`; drop the plugin | [where][sk-where] · [perm-ad][perm-ad] |
| Output style | `~/.claude/output-styles`, `.claude/output-styles` (cwd → repo root), managed, plugin `output-styles/` (`force-for-plugin`) | Instructions: R reminder. The SE section is dropped from `SP` unless `keep-coding-instructions` is set | `outputStyle` unset; `/output-style` | [glossary][gl] · [os-how][os] · issue [#89875][i89875] says the flag is ineffective |
| statusLine | settings `statusLine` | **none** (runs locally, no tokens) | `disableAllHooks` | [statusline][sl] |

## 7. MCP

| Source (provenance `source`) | Path / mechanism | Precedence (same server) | Lands | Disable one server without deleting | Evidence |
|---|---|---|---|---|---|
| `managedMcpServers` (`managed`) | managed settings, http/sse only | above everything | T | admin only | [scope-prec][mcp-prec] · [sr][sr-mcp] |
| `managed-mcp.json` (`enterprise` `[미확인 mapping]`) | `/Library/Application Support/ClaudeCode/managed-mcp.json` | exclusive control. Suppresses connectors unless `allowAllClaudeAiMcps` | T | admin only | [mg-del][mg-del] · [sr-aacm][sr-aacm] |
| local (`local`) | `~/.claude.json` → `projects[<path>].mcpServers` | 1 | T | `/mcp` toggle → `projects[<path>].disabledMcpServers` · `/mcp disable <s>` | [scopes][mcp-scope] · [disable][mcp-dis] |
| project (`project`) | `.mcp.json` at the project root | 2. Needs approval | T | `disabledMcpjsonServers` (wins over the enable keys) | [sr-mcp][sr-mcp] |
| user (`user`) | `~/.claude.json` → `mcpServers` | 3 | T | `/mcp` toggle | [scopes][mcp-scope] |
| plugin (`plugin`) | `<plugin>/.mcp.json` **or inline `mcpServers` in `plugin.json`** → name `plugin:<p>:<s>` | 4 (matched by endpoint) | T | `/mcp` toggle · `enabledPlugins:false` | [plugin-mcp][mcp-pl] |
| claude.ai connectors (`claudeai`) | fetched only with a claude.ai subscription login | 5 | T | `deniedMcpServers:[{"serverName":"claude.ai Slack"}]` · `/mcp` toggle · all of them: `disableClaudeAiConnectors` / `ENABLE_CLAUDEAI_MCP_SERVERS=false` | [connectors][mcp-ai] · [disable][mcp-aid] |
| `--mcp-config` (`dynamic`) | CLI JSON files/strings. SDK `mcpServers` also reports `dynamic` | — | T | drop the flag. `--strict-mcp-config` ignores every other source | [cli][cli] · [provenance][sdk-prov] |
| subagent `mcpServers` (`agent`) | agent frontmatter. Connects when the subagent starts, or **at startup when that agent is the main session (`--agent`/`agent`)** | — | subagent T | edit the agent / skip `--agent` | [scope-mcp][sa-mcp] |
| built-ins | `claude-in-chrome`, `computer-use` (default-off → `enabledMcpServers`), `ide` | — | T + SP section (Chrome) | `/mcp` toggle · `CLAUDE_CODE_DISABLE_CFC_PROMPT` | [disable][mcp-dis] · [env][env] |

**How MCP text enters the request:**
- With tool search on (the default with a first-party base URL), only tool *names* and server *instructions* load at start. Schemas stay deferred (`defer_loading`) until `ToolSearch` loads them ([tool search][mcp-ts]).
- Tool search is **turned off automatically when `ANTHROPIC_BASE_URL` is a non-first-party host**, which loads every MCP tool upfront. `ENABLE_TOOL_SEARCH=true|auto[:N]|false` overrides this, and CHANGELOG 2.1.72 confirms the override works behind a base URL ([configure][mcp-cfg], [CHANGELOG][cl]).
- Each tool description and each server's instructions are truncated at 2,048 chars per the docs (`CLAUDE_CODE_MAX_MCP_DESCRIPTION_LENGTH`). See Tensions for the newer CHANGELOG value ([authors][mcp-auth]).
- `/context` counts server instructions as their own row since 2.1.283 ([CHANGELOG][cl]).
- [local] Where instructions land: this session received a `# MCP Server Instructions` block (plugin:atlassian) as a separate system-role message. That matches `R1` in the 10-02 capture.
- Two **unrelated** list pairs: `disabledMcpServers`/`enabledMcpServers` (per project in `~/.claude.json`, written by the `/mcp` toggle) vs `disabledMcpjsonServers`/`enabledMcpjsonServers` (settings files, `.mcp.json` approval) ([disable][mcp-dis]).
- `claude mcp remove <name>` deletes the entry ([managing][mcp-man]).
- [local] zbrain has `disabledMcpServers: ["plugin:context7:context7"]`; `~/.claude.json` user scope is empty.

## 8. Plugins

| Item | Path / rule | Evidence |
|---|---|---|
| Install records | `~/.claude/plugins/installed_plugins.json` (`scope`, `installPath`, `version`), `known_marketplaces.json`, `cache/`, `marketplaces/`, `data/<id>/`. Plugins load at startup from these without network access. Root is overridable with `CLAUDE_CODE_PLUGIN_CACHE_DIR` | [loading-stages][pl-stage] · [env][env] |
| ID origins | `@<marketplace>` · `@inline` (`--plugin-dir`/`--plugin-url`/`CLAUDE_CODE_PLUGIN_DIRS`/SDK `plugins`) · `@skills-dir` (`.claude-plugin/plugin.json` inside a skills dir) · `@synced` (claude.ai, `~/.claude/plugins/synced/`) · `@builtin` (e.g. `cc-plugin-agents-md`) | [origins][pl-origin] · [agents-md][mem-cf] |
| Enable resolution | `enabledPlugins` by key, from 6 sources (§2). A missing key falls back to the manifest's `defaultEnabled` | [pl-en][pl-en] · [sr-ep][sr-ep] |
| Contributions | `skills/`, `commands/`, `agents/`, `hooks/hooks.json`, `.mcp.json` (or inline `mcpServers`), `.lsp.json`, `output-styles/`, `workflows/`, `themes/`, `monitors/monitors.json`, `bin/` (added to Bash PATH), `settings.json` (`agent`, `subagentStatusLine`). **A plugin-root `CLAUDE.md` is not loaded** | [manifest layout][pl-ref] |
| Disable | `"<id>": false` in `enabledPlugins` (put it in local settings to beat a project `true`) · `claude plugin disable <id>` · `/plugin` · `syncClaudeAiPlugins:false` · `"<n>@synced": false` · `"<n>@inline": false` | [pl-en][pl-en] · [sr-sync-pl][sr-sync-pl] |

## 9. Auto memory

| Item | Rule | Evidence |
|---|---|---|
| Path | `~/.claude/projects/<project>/memory/MEMORY.md` (+ topic files). `<project>` comes from the git repo, so worktrees share it. `autoMemoryDirectory` relocates it | [storage][mem-am] |
| Injected amount | **First 200 lines or 25 KB of MEMORY.md, whichever comes first.** Topic files are read on demand. Not loaded into subagents (forks excepted) | [how-it-works][mem-am] |
| Where | Conversation context; the capture shows it in the same `R0` block as CLAUDE.md. The memory *location/instructions* sit in `SP` (`--exclude-dynamic-system-prompt-sections` moves them to the first user message) | [ctx-window][cw] · [cli][cli] |
| Off | `autoMemoryEnabled:false` (the `/memory` toggle writes user settings) · `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` (`0` forces it on) · `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` · `--bare` · `--safe-mode` | [mem-am][mem-am] · [env][env] |
| `/memory` | Lists CLAUDE.md/CLAUDE.local.md/memory locations, toggles auto memory, and opens the folder | [/memory][mem-cmd] |

## 10. Built-ins and the official knobs that change them

| Built-in | Knob | Evidence |
|---|---|---|
| Whole system prompt | `--system-prompt[-file]` replaces it. `--append-system-prompt[-file]` appends. `--system-prompt-snapshot off` rebuilds it per request (it is otherwise recorded on the first request). `CLAUDE_CODE_SIMPLE_SYSTEM_PROMPT=1/0` selects the short or full variant | [cli][cli] · [snapshot][cli-snap] · [env][env] |
| Minimal harness | `--bare` / `CLAUDE_CODE_SIMPLE=1` keeps only Bash/read/edit and loads no hooks, skills, agents, plugins, MCP, memory, or CLAUDE.md. `--safe-mode` drops all customizations and keeps built-in tools | [cli][cli] · [env][env] |
| Built-in tools | `--tools "Bash,Read,Edit"` / `""` · `--disallowedTools Edit` removes the tool · `CLAUDE_CODE_DISABLE_WEB_FETCH`, `_CRON`, `_BACKGROUND_TASKS`, `_ARTIFACT`, `_ADVISOR_TOOL`, `_EXPLORE_PLAN_AGENTS`. Some built-ins can be deferred (`deferredBuiltinTools`) | [cli][cli] · [env][env] · [sdk ctx type][sdk-ctx] |
| Git instructions + status | `includeGitInstructions:false` / `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS=1` | [sr-git][sr-git] |
| Attribution reminder | `attribution.commit/pr = ""` | [sdk-off][sdk-off] |
| Skill list, file-change notes, todo nudges | `CLAUDE_CODE_DISABLE_ATTACHMENTS=1` (as of 2.1.290 project/local settings can't set it) | [sdk-off][sdk-off] · [CHANGELOG][cl] |
| Chrome SP section | `CLAUDE_CODE_DISABLE_CFC_PROMPT=1` | [env][env] |
| Pre-release betas | `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` strips `defer_loading` and keeps tool search off | [env][env] |
| userEmail / currentDate | **Undocumented.** Community decompile: read from `~/.claude.json` `oauthAccount.emailAddress`, skipped when `ANTHROPIC_UNIX_SOCKET` is set. No official switch `[미확인]`. [local] this session: `# userEmail` + `# gitStatus` arrive in a reminder, and the date sits in `SP` | [#81138][i81138] |
| Environment section | No documented switch `[미확인]` | [cw][cw] |

## 11. Inspection surfaces already in Claude Code

| Surface | What it shows | Evidence |
|---|---|---|
| `/context [all]` | Categories: system prompt, system tools, MCP tools, custom agents (with source), memory files, skills, messages. Since 2.1.283 MCP server instructions have their own row. Users also report "deferred" rows | [debug-config][dbg] · [CHANGELOG][cl] · [#92255][i92255] |
| SDK `getContextUsage()` / `/context` sent as prompt | **Structured version of `/context`**: `memoryFiles[{path,type,tokens}]`, `mcpTools[{name,serverName,tokens,isLoaded}]`, `deferredBuiltinTools`, `systemTools`, `systemPromptSections`, `agents[{agentType,source,tokens}]`, `skills.skillFrontmatter[{name,source,tokens}]`, `slashCommands`. As a prompt it arrives in the `context_usage` field (SDK ≥0.3.232) | [sdk ctx][sdk-ctx] |
| `--output-format stream-json --verbose` `system/init` | `tools[]`, `mcp_servers[{name,status,source}]`, `slash_commands`, `skills`, `agents`, `plugins[{name,path}]`, `plugin_errors`, `output_style`. `source` ∈ `sdk, plugin, user, project, local, dynamic, managed, enterprise, claudeai, agent` | [headless][hl] · [SDKSystemMessage][sdk-init] · [provenance][sdk-prov] |
| `/status` | Setting sources (files read, not per-key origin) | [confirm][set-conf] |
| `claude doctor` / `/doctor` | Install and settings validation; skill-listing cost; duplicate agent names; CLAUDE.md trims | [cli][cli] · [skills cut][sk-cut] |
| `/skills` (`t` sorts by tokens), `/skill-doctor` | Per-skill cost and usage | [commands][cmd] · [unused][sk-un] |
| `/memory`, `/hooks` (source-labelled), `/mcp`, `/plugin`, `/permissions` | Per-category browsers. `/agents` only prints a reminder since 2.1.198 | [dbg][dbg] · [commands][cmd] |
| `--debug[=mcp,…]`, `--debug-file` | `~/.claude/debug/<session>.txt`. Skill-budget overflow warning; hook matching | [cli][cli] · [dbg][dbg] |
| `InstructionsLoaded` hook | Which CLAUDE.md/rules loaded, when, and why | [mem-tr][mem-tr] |
| **Prompt dump** | **No `codex debug prompt-input` equivalent.** The docs say the system prompt *"isn't published"*. The official raw capture is `OTEL_LOG_RAW_API_BODIES=file:<dir>`, which writes untruncated `<dir>/<uuid>.request.json` plus `index.jsonl` (2.1.274+). Whether it also needs `CLAUDE_CODE_ENABLE_TELEMETRY=1`/an exporter is `[미확인]`. Alternative: log bodies at an `ANTHROPIC_BASE_URL` proxy (llmux already sits there) | [set-chg][set-chg] · [see-what][sdk-see] · [raw-body][mon-raw] · [CHANGELOG][cl] |

## 12. Cross-check of the 2026-10-02 measurement

| Measured | Docs say | Verdict | Evidence |
|---|---|---|---|
| tools 109 KB, 61 tools, 36 `blender` sent upfront | Default is deferred, but tool search turns off when `ANTHROPIC_BASE_URL` is non-first-party | **Consistent on mechanism.** [local] `claude` is aliased to `llmux run`, and the live claude process env has `ANTHROPIC_BASE_URL=http://localhost:3456`. Caveat: with tool search off, *every connected* server loads upfront. 61 − 36 = 25 is about the built-in set alone, so chrome/computer-use/slack/atlassian/context7 were apparently not connected for that first request `[추정]` | [mcp-cfg][mcp-cfg] · `ps -E` |
| messages[0] CLAUDE.md+MEMORY 45.9 KB | MEMORY.md is cut at 200 lines/25 KB | **Consistent.** [local] MEMORY.md is 150 lines / 22,924 B, so it loads whole. CLAUDE.md chain: 6,728 + `@rules/DEV.md` 8,608 + `@rules/PRINCIPLES.md` 4,440 + `~/.claude/CLAUDE.md` 634 = 20,410 B. Total ≈ 43.3 KB + wrappers. Sizes are from 10-13, not 10-02 | `wc -c` |
| userEmail/gitStatus 3.5 KB | git snapshot documented. userEmail not documented | partial | [sr-git][sr-git] · [#81138][i81138] |
| attribution 0.6 KB | documented reminder | consistent | [sdk-rem][sdk-rem] |
| messages[1] system 56 KB | Reminders can be "a separate message with the `system` role" | consistent | [gl][gl] |
| skill list 31 KB / 210 skills | Budget is 1% of the window (≈10k tokens at 1M `[추정]`). ≤1,536 chars per skill | plausible | [sk-cut][sk-cut] |
| agent roster 16.9 KB | Warning above 15k tokens | under the limit | [sa-del][sa-del] |
| MCP instructions 2 KB | Cap 2,048 chars per server | consistent. [local] blender log: `Server instructions truncated from 2110 to 2048 chars` | mcp-logs (below) |
| SessionStart 3.4 KB | Under the 10,000-char inline cap | consistent | [hk-ctx][hk-ctx] |

**Where could `blender` have come from?** [local] facts:
- Transcript `8f46bdfe-….jsonl` uses the bare tool prefix `mcp__blender__*`. Plugin tools use `mcp__plugin_<p>_<s>__`, as seen with `mcp__plugin_slack_slack__`.
- The log dir is `~/Library/Caches/claude-cli-nodejs/-Users-zhugehyuk-2lab-ai-zbrain/mcp-logs-blender/`. Plugin servers get `mcp-logs-plugin-*` dirs.
- Transport is stdio (`Successfully connected (transport: stdio)`, `uvx blender-mcp` banner). The **last connection was 2026-10-02T07:08:23Z**.
- Today `blender` appears in none of: `~/.claude.json`, its 3 backups, user/project/local settings, plugin dirs (incl. `synced/`), Claude Desktop config. `/Library/Application Support/ClaudeCode/` does not exist.
- All of these checks postdate the last connection. **Absence now does not prove absence at measurement time.**

Candidates, most likely first:
1. A user/local-scope entry in `~/.claude.json`, or an untracked zbrain `.mcp.json`, removed after 07:08Z `[가설]` ([mcp-scope][mcp-scope]). Weakening fact: zbrain `.claude/settings.local.json` has no `enabledMcpjsonServers` entry for blender.
2. `--mcp-config` or SDK `mcpServers` from a wrapper or harness (`dynamic`) ([cli][cli], [sdk-prov][sdk-prov]). [local] current `llmux run` children pass only `--settings` with `modelPicker`.
3. An agent file run as the main session via `--agent`/`agent` setting with inline `mcpServers` (`agent`) ([sa-mcp][sa-mcp]).
4. `managed-mcp.json` / MDM plist (`enterprise`/`managed`) ([mg-del][mg-del]).
5. `.mcp.json` in an ancestor directory. The docs only say "project root"; walk-up is `[미확인]` ([mcp-scope][mcp-scope]).

Ruled out:
- claude.ai connector: those arrive via claude.ai with a `claudeai` source, and the prefix and transport don't match `[추정 on prefix]`. [local] The llmux child env has no `ANTHROPIC_AUTH_TOKEN`/`ANTHROPIC_API_KEY`/`CLAUDE_CODE_OAUTH_TOKEN`, so the claude.ai-login path is active. Connector fetch and skill sync are therefore eligible even behind llmux ([mcp-ai][mcp-ai], [sk-sync][sk-sync]).
- plugin: the prefix and log-dir naming don't match.

**The definitive mechanism for `harn`:** read `system/init.mcp_servers[].source`, or run `claude mcp get <name>`. Do not grep config files.

## Tensions

- **MCP text cap:** the docs say 2,048 chars ([authors][mcp-auth]). CHANGELOG 2.1.296 says *"Changed the default limit on MCP tool descriptions sent up front and on MCP server instructions from 2,048 to 4,096"* ([cl][cl]). The installed version is 2.1.296, so the live cap is probably 4,096 `[미확인]`.
- **`keep-coding-instructions`:** the docs say it keeps the SE section. Issue [#89875][i89875] reports it does nothing.
- **Deferred tools:** docs describe them as "minimal impact". Issues [#83363][i83363] and [#92255][i92255] report about 20k / 27k tokens of deferred listings.
- **Skill list content:** issue [#95582][i95582] reports catalog descriptions missing intermittently.

## Open questions / unconfirmed (each with the command that settles it)

1. Does `OTEL_LOG_RAW_API_BODIES=file:<d>` need telemetry enabled? → `CLAUDE_CODE_ENABLE_TELEMETRY=1 OTEL_LOGS_EXPORTER=none OTEL_LOG_RAW_API_BODIES=file:/tmp/x claude -p hi; ls /tmp/x`
2. Exact wire form of deferred MCP tools (`defer_loading` stub vs names-only reminder) on first-party. → the same raw-body capture with tool search on.
3. Does `Agent(x)` in `permissions.deny` remove the roster line? → compare `/context` agents rows before and after.
4. Live MCP cap on 2.1.296 (2,048 vs 4,096)? → `claude --debug=mcp` and look for "truncated from N to M".
5. Does `.mcp.json` walk up from cwd to ancestors? → put `.mcp.json` in a parent dir, start in a child, run `claude mcp list`.
6. Does `claude -p "/context" --output-format stream-json` carry `context_usage`? → run it and grep.
7. Does `CLAUDE_CODE_DISABLE_CLAUDE_MDS` also drop managed CLAUDE.md? → test with a temporary `/Library/Application Support/ClaudeCode/CLAUDE.md` (needs sudo).
8. Where the recorded system-prompt snapshot is persisted (session jsonl?) → `grep -l systemPrompt ~/.claude/projects/<p>/*.jsonl`.
9. Is there a switch for userEmail/currentDate injection? → none documented. Watch [#81138][i81138].
10. Exact provenance value for `managed-mcp.json` servers (`enterprise` vs `managed`). → stream-json init on a test machine.
11. `blender` origin on 10-02 (§12). → only recoverable if a wrapper log or `.claude.json` backup from 10-02 exists.

[gl]: https://code.claude.com/docs/en/glossary#system-reminder
[mem-tr]: https://code.claude.com/docs/en/memory#troubleshoot-memory-issues
[mem-ht]: https://code.claude.com/docs/en/memory#my-claude-md-is-too-large
[mem-loc]: https://code.claude.com/docs/en/memory#choose-where-to-put-claude-md-files
[mem-mg]: https://code.claude.com/docs/en/memory#deploy-organization-wide-claude-md
[mem-ur]: https://code.claude.com/docs/en/memory#user-level-rules
[mem-hl]: https://code.claude.com/docs/en/memory#how-claude-md-files-load
[mem-cf]: https://code.claude.com/docs/en/memory#choose-which-instruction-files-load
[mem-ag]: https://code.claude.com/docs/en/memory#agents-md
[mem-imp]: https://code.claude.com/docs/en/memory#import-additional-files
[mem-rules]: https://code.claude.com/docs/en/memory#path-specific-rules
[mem-ad]: https://code.claude.com/docs/en/memory#load-from-additional-directories
[mem-ex]: https://code.claude.com/docs/en/memory#exclude-specific-claude-md-files
[mem-am]: https://code.claude.com/docs/en/memory#auto-memory
[mem-cmd]: https://code.claude.com/docs/en/memory#view-and-edit-with-%2Fmemory
[set-prec]: https://code.claude.com/docs/en/settings#settings-precedence
[set-exc]: https://code.claude.com/docs/en/settings#exceptions-to-managed-settings-precedence
[set-chg]: https://code.claude.com/docs/en/settings#change-a-setting
[set-conf]: https://code.claude.com/docs/en/settings#confirm-what-loaded
[mg-del]: https://code.claude.com/docs/en/managed-settings#delivery-mechanisms
[sr-ep]: https://code.claude.com/docs/en/settings-reference#enabledplugins
[sr-dah]: https://code.claude.com/docs/en/settings-reference#disableallhooks
[sr-env]: https://code.claude.com/docs/en/settings-reference#env
[sr-dcc]: https://code.claude.com/docs/en/settings-reference#disableclaudeaiconnectors
[sr-model]: https://code.claude.com/docs/en/settings-reference#model
[sr-attr]: https://code.claude.com/docs/en/settings-reference#attribution
[sr-git]: https://code.claude.com/docs/en/settings-reference#includegitinstructions
[sr-skills]: https://code.claude.com/docs/en/settings-reference#skilloverrides
[sr-mcp]: https://code.claude.com/docs/en/settings-reference#disabledmcpjsonservers
[sr-aacm]: https://code.claude.com/docs/en/settings-reference#allowallclaudeaimcps
[sr-sync-pl]: https://code.claude.com/docs/en/settings-reference#syncclaudeaiplugins
[sr-adv]: https://code.claude.com/docs/en/settings-reference#advisormodel
[env]: https://code.claude.com/docs/en/env-vars
[cli]: https://code.claude.com/docs/en/cli-reference#cli-flags
[cli-snap]: https://code.claude.com/docs/en/cli-reference#system-prompt-flags-in-resumed-conversations
[hk]: https://code.claude.com/docs/en/hooks
[hk-loc]: https://code.claude.com/docs/en/hooks#hook-locations
[hk-dis]: https://code.claude.com/docs/en/hooks#disable-or-remove-hooks
[hk-ex0]: https://code.claude.com/docs/en/hooks#exit-code-0
[hk-ctx]: https://code.claude.com/docs/en/hooks#add-context-for-claude
[hk-ss]: https://code.claude.com/docs/en/hooks#sessionstart
[sk-where]: https://code.claude.com/docs/en/skills#where-skills-live
[sk-b]: https://code.claude.com/docs/en/skills#bundled-skills
[sk-sync]: https://code.claude.com/docs/en/skills#how-synced-skills-behave
[sk-ctl]: https://code.claude.com/docs/en/skills#control-who-invokes-a-skill
[sk-life]: https://code.claude.com/docs/en/skills#skill-content-lifecycle
[sk-ovr]: https://code.claude.com/docs/en/skills#override-skill-visibility-from-settings
[sk-cut]: https://code.claude.com/docs/en/skills#skill-descriptions-are-cut-short
[sk-un]: https://code.claude.com/docs/en/skills#find-unused-skills
[sa-scope]: https://code.claude.com/docs/en/sub-agents#choose-the-subagent-scope
[sa-dis]: https://code.claude.com/docs/en/sub-agents#disable-specific-subagents
[sa-del]: https://code.claude.com/docs/en/sub-agents#understand-automatic-delegation
[sa-start]: https://code.claude.com/docs/en/sub-agents#what-loads-at-startup
[sa-mcp]: https://code.claude.com/docs/en/sub-agents#scope-mcp-servers-to-a-subagent
[os]: https://code.claude.com/docs/en/output-styles#how-output-styles-work
[sl]: https://code.claude.com/docs/en/statusline
[mcp-scope]: https://code.claude.com/docs/en/mcp#mcp-installation-scopes
[mcp-prec]: https://code.claude.com/docs/en/mcp#scope-hierarchy-and-precedence
[mcp-dis]: https://code.claude.com/docs/en/mcp#disable-a-server-without-removing-it
[mcp-man]: https://code.claude.com/docs/en/mcp#managing-your-servers
[mcp-pl]: https://code.claude.com/docs/en/mcp#plugin-provided-mcp-servers
[mcp-ai]: https://code.claude.com/docs/en/mcp#use-mcp-servers-from-claude-ai
[mcp-aid]: https://code.claude.com/docs/en/mcp#disable-claude-ai-connectors
[mcp-ts]: https://code.claude.com/docs/en/mcp#scale-with-mcp-tool-search
[mcp-cfg]: https://code.claude.com/docs/en/mcp#configure-tool-search
[mcp-auth]: https://code.claude.com/docs/en/mcp#for-mcp-server-authors
[pl-stage]: https://code.claude.com/docs/en/plugins/loading#check-which-stage-a-plugin-reached
[pl-origin]: https://code.claude.com/docs/en/plugins/loading#find-where-a-plugin-came-from
[pl-en]: https://code.claude.com/docs/en/plugins/loading#find-where-a-plugin-is-enabled
[pl-ref]: https://code.claude.com/docs/en/plugins-reference#standard-layout
[perm-ad]: https://code.claude.com/docs/en/permissions#additional-directories-grant-file-access-not-configuration
[cw]: https://code.claude.com/docs/en/context-window
[dbg]: https://code.claude.com/docs/en/debug-your-config#see-what-loaded-into-context
[cmd]: https://code.claude.com/docs/en/commands
[cd-ref]: https://code.claude.com/docs/en/claude-directory#file-reference
[hl]: https://code.claude.com/docs/en/headless
[sdk-rem]: https://code.claude.com/docs/en/agent-sdk/modifying-system-prompts#reminders-claude-code-adds-to-the-conversation
[sdk-off]: https://code.claude.com/docs/en/agent-sdk/modifying-system-prompts#turn-off-the-context-your-agent-replaces
[sdk-see]: https://code.claude.com/docs/en/agent-sdk/modifying-system-prompts#see-what-claude-received
[sdk-init]: https://code.claude.com/docs/en/agent-sdk/typescript#sdksystemmessage
[sdk-prov]: https://code.claude.com/docs/en/agent-sdk/typescript#mcpserverprovenance
[sdk-ctx]: https://code.claude.com/docs/en/agent-sdk/typescript#sdkcontrolgetcontextusageresponse
[mon-raw]: https://code.claude.com/docs/en/monitoring-usage#api-request-body-event
[cl]: https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md
[i81138]: https://github.com/anthropics/claude-code/issues/81138
[i92255]: https://github.com/anthropics/claude-code/issues/92255
[i83363]: https://github.com/anthropics/claude-code/issues/83363
[i89875]: https://github.com/anthropics/claude-code/issues/89875
[i95582]: https://github.com/anthropics/claude-code/issues/95582
