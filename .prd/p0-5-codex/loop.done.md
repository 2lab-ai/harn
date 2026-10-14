# loop.done — P0.5 drive (append-only)

## R1 — closed (2026-10-15)

Codex support landed on `main` in ten PRs merged 2026-10-14 to 2026-10-15 (KST), the last being
f8b65da. `just check` on f8b65da (the author's macOS): exit 0, 673 tests passed, 3 ignored, 0
failed. Acceptance: A7 and A8 observed on the author's macOS, A9 for the Codex tree on cachy-z13
(CachyOS). Raw command output stays outside this public repo.

### Closed WUs

C1–C6 were the planned units; C7 and C8 were added from the acceptance runs; #18 and #21 are
fixes found during the drive. Listed in that order, not by merge time (C7 merged before C5, #18
before C6 and C8).

| WU | scope | PR · merge | outcome |
|---|---|---|---|
| C1 | resolver foundation | #12 · e503653 | config layers, the three trust gates, the AGENTS.md chain, the built-in blocks |
| C2 | skills and plugins | #15 · e2fb2c0 | the skill listing (roots, 2 %/10k-token budget) and plugins with their skills, MCP servers and hooks |
| C3 | hooks, rules, MCP | #13 · d3c6437 | hooks, rules files and MCP servers |
| C4 | TOML knobs | #14 · 39da471 | Codex switches written with toml_edit; plan, journal and undo byte for byte |
| C5 | oracle and verify | #17 · f363102 | `oracle/codex.rs` and `harn verify --run --agent codex`: `debug prompt-input`, `debug models`, `mcp list --json` and `plugin list --json` into a 0700 capture dir; gate 1 presence, gate 2 byte-exact content; base instructions, tool schemas and hook output (outside Codex's dump) and rules files (inside the permissions block) are not checkable |
| C6 | wiring | #20 · 680743d | Codex on every surface (default_agents, TUI tab, brief, report, why); `Tree.trust` and `Tree.codex` are structured facts; the TUI rewrites resolver text only where it is certain |
| C7 | catalog gating | #16 · f4dd7ef | blocks gated by the model catalog, and the `models_cache.json` snapshot contract: catalog facts only from an accepted snapshot (required fields present, an installed codex whose version equals `client_version`), otherwise unknown with the reason; Codex re-checks a catalog older than 5 minutes |
| C8 | verify exemptions | #19 · 03dce18 | bundled `.system` skills that differ are not checkable (they are written by whichever Codex last started; the bundled root is recorded at capture time); account plugins (`source: remote`) are not checkable |
| #18 | TUI signals | #18 · 0f33045 | the TUI exits on SIGTERM/SIGHUP even when crossterm's read is stuck on a hung-up terminal; a watchdog with a 1 s grace never interrupts a write (a P0 robustness fix found during P0.5) |
| #21 | relative CODEX_HOME | #21 · f8b65da | a relative `CODEX_HOME` resolves against the target dir, as Codex does |

### Acceptance receipts

| row | when · where | result |
|---|---|---|
| A7/A8 first run | 2026-10-14, the author's macOS, codex-cli 0.160.1, a real working folder | differences traced to two causes: several Codex installs sharing one home, which rewrite `.system`, and plugins installed for the account → C8; the run also found catalog defects in C1/C2 → C7 |
| A7/A8 final run | 2026-10-15 00:19 KST (capture stamp 2026-10-14T15:19Z), the same machine and folder, `main` f8b65da | `harn verify --run --agent codex`: seen 42, predicted-not-seen 0, seen-not-predicted 0, content-mismatch 0, not-checkable 18, `passed: yes`, exit 0, 31 rows promoted to observed; the folder's git status was the same before and after |
| A9 | 2026-10-15, cachy-z13 (CachyOS), f8b65da built there | the Codex tree resolves from files with the same columns as on macOS; the `tui_pty` and `tui_codex` suites pass on Linux |

The 18 not-checkable rows of the final run, by class: base instructions 1, hook output 1 (both
outside Codex's dump), rules file 1, runtime-decided block 1 (`apps_instructions`, not rendered),
catalog-gated block 1 (`plugins_instructions`; `models_cache.json` was written by codex 0.142.5,
not 0.160.1, so catalog facts were unknown), bundled `.system` skills 2 (listed with
another line), account plugins 11. Every report states what is outside Codex's dump.

Known limit found by A9: Codex on cachy-z13 is the distribution's static binary
(`openai-codex-bin` 0.155.1). harn reads the installed version only from npm or Homebrew layouts
and never runs `codex` in the resolver, so there it reports "the installed codex's version could
not be determined" and leaves catalog facts unknown (`01-spec.md` §Source of truth).

### Round section (as it was in loop.md, 2026-10-14; headings one level down)

#### Build facts

- Gate: `just check` (fmt --check, clippy -D warnings, test --locked). CI runs it on macOS + ubuntu.
- Differential runs (A7/A8) need `codex` on PATH: macOS `codex-cli 0.160.1`; cachy-z13 has none.
- TOML writes do not exist yet (`writers.rs` handles JSON; `journal.rs` only parse-checks TOML).
- `resolve/mod.rs`: `CodexPending` placeholder; `default_agents()` = Claude Code only;
  `main.rs` `verify --agent codex` bails "not implemented".

#### R1 — work units

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
