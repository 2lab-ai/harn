# harn

See and trim what your AI coding agent loads before your first message. Site: <https://harn.2lab.ai>

A fresh Claude Code session in one real directory sends a 332 KB first request before it reads a
2-byte instruction. `harn` lists every item behind those bytes without starting a session, one
tree per agent: its size, where it came from, whether it is always loaded or only advertised, and
the agent's official switch that turns it off. Every change it makes is reversible byte for byte.
Claude Code today, Codex CLI next. One binary for macOS and Linux.

## Install

```sh
brew install 2lab-ai/tap/harn-preview
```

This works with Homebrew on macOS and Linux. Or download a binary from
[GitHub Releases](https://github.com/2lab-ai/harn/releases) (`harn-macos-aarch64`,
`harn-macos-x86_64`, `harn-linux-aarch64`, `harn-linux-x86_64`, checksums in `SHA256SUMS`), make
it executable and put it on your `PATH` as `harn`. A preview release is published on every push to
`main`; stable `v*` releases come later, as the `harn` formula.

## Use

`harn` in a real project on an 80×24 terminal, started with `--readonly` (no write keys shown):

```text
 zbrain · read-only · 312 items load before your first message          ~145 KB
 biggest: skill listing 87 KB (60%) · project 22 KB · memory 20 KB    ! 2 notes
────────────────────────────────────────────────────────────────────────────────
 ▸ Skill listing  218 entries                         87 KB ██████████████  60%
 ▸ Project files (2)                                  22 KB ███▌            15%
 ▸ Memory (1)                                         20 KB ███▎            14%
 ▸ Agent listing  31 entries                          15 KB ██▌             10%
 ▸ Plugins (10)  48 KB would leave with them · counted in the listings above
 ▸ Hooks (14)                                    unmeasured
 ▸ Built-in (8)  mostly cannot be turned off     unmeasured
────────────────────────────────────────────────────────────────────────────────
 Skill listing · 218 entries, one line per skill or command
 Sent in every first request; it has a size cap, and whether it cuts
 descriptions here needs `harn verify --run`.
 Open it (→) to see the entries by owner; d on an entry would turn that one off.
────────────────────────────────────────────────────────────────────────────────
 ↑↓ move   →← open/close   / search   i notes   ? keys   q quit
```

| Command | What it does |
|---|---|
| `harn [DIR]` | The interactive view on a terminal; the table when piped. `DIR` defaults to the current directory. |
| `harn trim` | The AI picks what to turn off, shows the plan, applies on `y`. Journaled; `harn undo` puts it back. |
| `harn report --html harn.html` | The same picture as a shareable read-only page. |
| `harn verify --run` | Measures one real session: starts the agent once, runs your hooks and MCP servers, may cost one model request. |

The header names the directory, how many items load, the total and the biggest groups. Items are
grouped by kind, each group with its size, a bar and its share. The panel below explains the cursor
row in sentences: where it comes from, what `d` would turn off and for whom, what that frees.

| Key | Does |
|---|---|
| `→` `←` | open / close a group (each row inside has its own size) |
| `d` | turn off: one card says what file it writes, what leaves (`→` lists it), what it saves and how to undo; Enter applies, Esc cancels |
| `o` · `u` | turn an item back on · undo the last change (both restore the file byte for byte) |
| `w` · `e` | where it comes from · edit its file in `$EDITOR` |
| `/` · `s` | search · sort |
| `i` · `?` · `q` | notes · every key · quit |

The key bar shows only the keys valid now. `--readonly` says so in the header and leaves `d`, `o`,
`u` and `e` out.

## How it works

- Reads the agent's real config layers without starting a session: settings scopes, `CLAUDE.md`
  and its imports, memory, plugins and marketplaces, the skill, command and agent listings,
  hooks, MCP servers and built-ins.
- Sizes each item in bytes as it lands in the first request. What files cannot tell (built-ins,
  hook output, MCP tool schemas) reads `unmeasured` until `harn verify --run` measures a real
  session, so the screen's total covers only what files can size.
- Turns things off only through the agent's official switches: a settings key, written to
  `.claude/settings.local.json` by default (this project only); never a wider scope silently.
- Snapshots the file before every write and journals the change (`harn history`). `harn undo`, or
  `u` on the screen, restores the file byte for byte.
- `--readonly` refuses every write and every active verify (exit 3).

## For AI operators

The commands marked `[AI operator]` in `harn --help` (`why`, `path`, `off`, `on`, `edit`,
`brief`, `apply`) are what an agent runs on your behalf; you never type an item name. The loop:
`harn brief` prints the facts and the plan format, the agent writes a plan (schema `harn-plan/1`),
`harn apply <plan.json>` validates every op and applies it. `/harn-trim` inside Claude Code runs
the same loop step by step. `harn trim [DIR]` runs it in one command:

- It asks your coding agent for a plan in one headless request, announced before it runs. The
  default command is `claude -p --output-format json --tools '' --max-turns 1 -`: no tools, one
  turn, the prompt piped on stdin.
- It validates the reply exactly as `harn apply` does and prints the plan: each op with the
  agent's reason, the bytes freed, the side effects (a plugin takes its skills, agents and hooks
  with it) and the diff of every settings file it touches.
- On a terminal it asks `apply? [y/N]`. Applied changes are journaled with their reasons
  (`harn history`); `harn undo` puts every file back byte for byte.
- `--dry-run` prints the validated plan and writes nothing.
- `--yes` applies without asking (required when stdin is not a terminal).
- `--agent-cmd "<cmd>"` replaces the agent command and runs it as given, in an empty temp dir
  (yours, trusted: harn adds no flags). A trailing `-` means: pipe the prompt on stdin instead of
  passing it as the last argument.
- Without `claude` on `PATH`, run `/harn-trim` inside Claude Code instead.

Output flags:

- `--table` prints the table; it is what `harn` prints unless stdin and stdout are both a terminal.
- `--json` prints the same model as JSON (schema `harn/1`).
- `--no-unicode` prints ASCII only.
- `--color auto|always|never` (`auto` = stdout is a terminal and `NO_COLOR` is unset).

## Status

P0 for the Claude Code chain is shipped on `main`; preview builds install via the tap. Next: the
Codex CLI resolver (P0.5). Open follow-ups: https://github.com/2lab-ai/harn/issues.

## Development

`just check` runs `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` and
`cargo test --locked`, the same three steps CI runs on macOS and Ubuntu.

The spec: product definition [`.prd/01-spec.md`](.prd/01-spec.md), implementation boundaries
[`.prd/02-architecture.md`](.prd/02-architecture.md), module contract
[`.prd/03-p0-interfaces.md`](.prd/03-p0-interfaces.md), the interactive view
[`.prd/04-tui-design.md`](.prd/04-tui-design.md), and the research it rests on
[`.prd/research/`](.prd/research/).

## License

MIT, see [`LICENSE`](LICENSE).
