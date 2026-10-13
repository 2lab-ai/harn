# Local measurements — what each agent already tells you, and what the wire says (2026-10-13)

Status: research input for 01-spec v2. Every number below was produced on the author's macOS
machine; file paths are the author's and appear only as provenance.

## A. Claude Code 2.1.296 — built-in `/context` (headless)

Command (cwd `~/2lab.ai/zbrain`, 2026-10-13):

```
~/.local/bin/claude -p "/context" --output-format text --max-turns 1
```

It printed a 335-line markdown report titled "Context Usage" with **"Estimated usage by category"**:

| Category | Tokens | % of 800k |
|---|---|---|
| System prompt | 2k | 0.3% |
| System tools | 15.7k | 2.0% |
| MCP tools | 53.6k | 6.7% |
| MCP server instructions | 897 | 0.1% |
| Custom agents | 5.5k | 0.7% |
| Memory files | 19.2k | 2.4% |
| Skills | 9.9k | 1.2% |
| Messages | 1.3k | 0.2% |
| Free space | 658.8k | 82.4% |
| Autocompact buffer | 33k | 4.1% |

Then four tables: **MCP Tools** (48 rows: tool, server, tokens), **Custom Agents** (26 rows: agent, Source ∈ {User, Project, Plugin}, tokens), **Memory Files** (5 rows: Type ∈ {User, Project, AutoMem}, absolute path, tokens), **Skills** (216 rows: skill, Source, tokens as `~N` or `< 20`; sources: Built-in 14, Plugin(atlassian) 6, Plugin(cowork-plugin-management) 2, Plugin(frontend-design) 1, Plugin(slack) 13, Plugin(stv) 15, Plugin(superpowers) 15, Plugin(typesafe) 1, Plugin(zworkflow) 50, Project 58, User 19, claude.aisync 22).

What it gives that `harn` can reuse: the category taxonomy, per-item token estimates, Source
labels, memory-file paths. What it does not give (each one is a gap `harn` must fill):

- no **hooks** at all (SessionStart hook stdout was 3,457 B on the wire — invisible here);
- no **gitStatus / userEmail / attribution** blocks (4.1 KB on the wire);
- no **file path** for skills or agents, no plugin version, no MCP config location;
- no **disable action** and no **content** — it is a report, not an editor;
- **estimated**, not measured: the word "Estimated" is in its own heading. The same cwd's first
  request measured on 2026-10-02 (different day, different MCP set) was 87,302 cache-creation
  input tokens for 230,944 bytes of JSON; today's estimate for the equivalent categories sums to
  75.2k before the 33k autocompact buffer. The two are not the same session, so this is not a
  ratio claim — it is the reason every number in `harn` carries a *measured* or *estimated* tag.
- it runs a real session: it cost one model request and loaded the harness it reports on. A
  listing tool that must spawn the agent to list the agent is a round trip, not a lookup.

## B. Codex CLI 0.160.1 — `codex debug prompt-input`

Command (cwd `~/2lab.ai/zbrain`, 2026-10-13), exit 0, 21,962 B of JSON: a list of 4 messages.

| # | role | parts (type → bytes → opening tag) |
|---|---|---|
| 0 | developer | `<skills_instructions>` 9,056 B (skill roots + "Available skills" list) · `<permissions instructions>` 940 B · `<collaboration_mode>` 920 B · `<recommended_plugins>` 941 B |
| 1 | developer | `<multi_agent_role>` 2,429 B |
| 2 | developer | `<multi_agent_mode>` 271 B |
| 3 | user | `# AGENTS.md instructions for <cwd>` 4,163 B (the repo's AGENTS.md wrapped in `<INSTRUCTIONS>`) · `<environment_context>` 953 B (cwd, shell, date, timezone, …) |

What it gives: the exact model-visible *input items* with their text — primary evidence, not an
estimate, and no model request is made. What it does not include: the base/system instructions
(Codex's own prompt), the **tools** array (built-in + MCP tool definitions), and anything a hook
would inject at session start. So for Codex, `harn` can take instructions/skills/AGENTS.md
from this dump verbatim and must still discover tools, hooks and plugins itself.

## C. The wire (Claude Code 2.1.287, 2026-10-02, logging proxy in front of llmux)

Already fixed in `../prd-rewrite/ssot.md` §Fixed facts. Two additions for the spec:

- llmux keeps every proxied request at `$XDG_STATE_HOME/llmux/raw-io.jsonl` (config
  `raw_io.enabled` default true, `max_body_bytes` default 1 MiB, `retention_days` 30; source
  `llmux/src/proxy/raw_io.rs`). A user who runs Claude Code through llmux therefore already owns a
  measured record of the first request of every session; `harn` can read it without a proxy
  of its own. llmux does not rewrite prompts (it strips credential headers and injects the
  account credential — `llmux/.prd/01-spec.md:75`), so "edit the built-in prompt through llmux"
  is a future llmux feature, not a present one.
- The 36 `mcp__blender__*` tools in the 10-02 capture have no local origin: not in
  `~/.claude.json` (user or per-project `mcpServers`), not in any `.mcp.json`, not in any plugin
  cache, not in `~/.codex/config.toml`, not in Claude Desktop's config (grep 2026-10-13). The
  project's `.claude/settings.local.json` gained `disableClaudeAiConnectors: true` on 2026-10-08
  (file mtime), after the capture, and the 10-13 `/context` run lists no blender tools.
  `[추정]` blender was a claude.ai-side connector. Consequence for the spec: an item can be in
  the request with **no local file to point at**; the origin model needs a `remote` kind and an
  `unresolved` kind, and the disable action for `remote` is a settings key, not a file edit.

## D. Harness surface on this machine (counts only, 2026-10-13)

| source | count | note |
|---|---|---|
| `~/.claude/skills` | 22 dirs | user skills |
| `~/.claude/agents` | 2 files | user agents |
| `~/.claude/hooks` | 1 file | plus 11 hook events in `~/.claude/settings.json` |
| `<zbrain>/.claude/skills` | 63 dirs | project skills |
| `<zbrain>/.claude/agents` | 13 files (10 agents + 2 personas + CLAUDE.md) | project agents |
| plugins enabled | 9 (`enabledPlugins`), 10 installed (`installed_plugins.json` v2) | 6 marketplaces |
| plugin `.mcp.json` files in cache | 30+ (every cached plugin version keeps one) | only enabled versions count |
| `~/.codex/skills` | 9 dirs | codex user skills |
| `~/.codex/hooks.json` | 1 SessionStart hook | |
| `~/.codex/config.toml` | 13.4 KB; ~80 `[projects."…"]` trust entries, 2 `mcp_servers`, 9 plugins, 2 marketplaces | |

Instruction-file chain for zbrain: `~/.claude/CLAUDE.md` (738 B) → `zbrain/CLAUDE.md` (6,506 B)
→ `@rules/DEV.md` (8,602 B) + `@rules/PRINCIPLES.md` (4,261 B) → auto-memory `MEMORY.md`
(25,560 B); Codex reads `zbrain/AGENTS.md` (4,163 B incl. wrapper) instead.
