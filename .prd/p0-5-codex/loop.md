# loop — P0.5 drive (Codex resolver)

Status: in-progress (R1 started 2026-10-14)

## Build facts

- Gate: `just check` (fmt --check, clippy -D warnings, test --locked). CI runs it on macOS + ubuntu.
- Differential runs (A7/A8) need `codex` on PATH: macOS `codex-cli 0.160.1`; cachy-z13 has none.
- TOML writes do not exist yet (`writers.rs` handles JSON; `journal.rs` only parse-checks TOML).
- `resolve/mod.rs`: `CodexPending` placeholder; `default_agents()` = Claude Code only;
  `main.rs` `verify --agent codex` bails "not implemented".

## R1 — work units

| WU | scope | files (owned) | depends on |
|---|---|---|---|
| C1 | foundation: `CODEX_HOME`, config layers (defaults · `/etc/codex` · user · profile · trusted project root→cwd · managed) with the overlay applied to TOML, the three trust gates, AGENTS chain (global + project, override/fallback, 32 KiB shared budget), `developer_instructions`, the built-in blocks (base instructions, permissions, collaboration, apps, environment, multi-agent) as rows with their `include_*`/`agents.enabled` knobs, `Resolver for Codex` | `src/resolve/codex/{mod,config,instructions,builtins}.rs`, fixtures | — |
| C2 | skills roots (repo `.agents/skills` + `.codex/skills` trust-independent, user, bundled `.system`, admin) with the 2 %/10k-token listing cap, and plugins/marketplaces with their children (skills, MCP servers, hooks) and `<plugins_instructions>`/`<recommended_plugins>` | `src/resolve/codex/{skills,plugins}.rs` | C1 |
| C3 | hooks (`hooks.json` + inline, trusted project, `trusted_hash` state, managed-only), rules files, `mcp_servers` (user + trusted project, enabled, deferral when the model supports tool search) | `src/resolve/codex/{hooks,rules,mcp}.rs` | C1 |
| C4 | TOML knobs: `writers.rs` TOML set/append by JSON pointer via toml_edit (quoted keys, `[[skills.config]]` append, `[hooks.state."k"]`), plan + journal + undo byte-for-byte for Codex | `src/writers.rs`, `src/plan.rs` (Codex paths) | C1 |
| C5 | `oracle/codex.rs` (`debug prompt-input`, `debug models`, `mcp list --json`, `plugin list --json`) + `verify` for Codex: A7 presence match, A8 report | `src/oracle/codex.rs`, `src/verify.rs`, `src/main.rs` (verify branch) | C1–C3 |
| C6 | wiring: `default_agents()` adds Codex (one table each), TUI/brief/report/why on the Codex tree, README + 01/02 status | `src/resolve/mod.rs`, `src/tui/*`, `src/render/*`, docs | C1–C5 |

Each WU: one coding agent, its own worktree/branch, `just check` green, external review, merge.
Screens (C6) need captured frames read before ship.
