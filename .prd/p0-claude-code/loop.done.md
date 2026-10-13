# loop.done — P0 drive (append-only)

## R1 — closed (2026-10-13)

Scaffold + four modules + integration landed on `main` (caf48f3, 193 tests: lib 82 · bin 12 · cli 2 · differential 1 · plan_journal 33 · render 28 · resolve_claude 17 · verify 18 + 2 ignored). P0 gate rows A1–A6, A9–A13 observed on macOS; A1/A2/A9 on cachy-z13. Receipts (raw command output, private paths) are kept in the control-plane repo: zbrain `workflow/2lab-ai/harn/receipts-2026-10-13/`.

### Closed WUs

| WU | files owned | agent | state |
|---|---|---|---|
| WU-A | `Cargo.toml`, `justfile`, `src/lib.rs`, `src/main.rs`, `src/model.rs`, module stubs, `tests/fixtures/README.md`, `.github/workflows/ci.yml` | opus-coder (bg) | done — ceacd63 (31 tests), interface amendments recorded in 03 |
| WU-B | `src/resolve/mod.rs`, `src/resolve/claude/**`, `tests/resolve_claude.rs`, `tests/fixtures/zbrain-like/**` | opus-coder (bg) | merged 0a094db (73 tests; zbrain tree = init event set-for-set; 57/57 listing entries + 26/26 roster lines byte-exact vs capture) |
| WU-C | `src/render/**` (table, json, brief, html), `tests/render.rs` | opus-coder (bg) | merged 8ee7e9f + 697b159 (63 tests; JournalEntry literal fixed at merge) |
| WU-D | `src/oracle/**`, `src/verify.rs`, `tests/verify.rs`, `tests/fixtures/capture-*/**` | opus-coder (bg) | merged 8d8317f (59 tests; real capture sums to 332,548; one live run bound) |
| WU-E | `src/plan.rs` (incl. PlanRequest harn-plan/1), `src/writers.rs`, `src/journal.rs`, `tests/plan_journal.rs` | opus-coder (bg) | merged ab69a44 + 7eacabb (74 tests; fixture settings.local.json needed a .gitignore negation) |
| WU-F | integration: verify joins layers by (kind, name), selector 4th tier (unique substring), parallel freed annotation, brief < 40 KB, observed header (`render::header`, `verify --table`), `harn::version_line()`, `harn edit` prints the diff, one-line JSON arrays kept, differential test, G1/G2/G5 | opus-coder (bg) | merged caf48f3 (193 tests) |

### Closed gaps

| ID | gap | closed by |
|---|---|---|
| G1 | `why` built the local plan twice for items with local+user knobs | WU-F: plans deduped by knob (caf48f3) |
| G2 | `main::open_journal` built the struct instead of `Journal::open()` (0700 dir) | WU-F (caf48f3) |
| G4 | brief ids for skills/agents/commands must be init-event names (realpath ids blew 40 KB) | WU-B ids + WU-F brief (27,550 B on zbrain, 23,465 B on cachy-z13) |
| G5 | observed header inputs (attributed/parameters/framing, tool-search cause, `Tree.home`, version line) | WU-F (`render::header`, `harn::version_line()`) |
| G6 | selector 4th tier so `plugin:slack` resolves | WU-F (`model::select`, substring tier) |
| G8 | `generate.py` fixture generator in a Rust repo | kept, documented in `tests/fixtures/README.md` |

### P0 receipt (2026-10-13, macOS release build unless noted)

| row | result | evidence (summary; raw in zbrain receipts dir) |
|---|---|---|
| A1 | pass | `--json`: every init-event name (skills, agents, plugins, mcp_servers with source) appears once with the same source; no new project session file, no child process |
| A2 | pass | one table, sorted by freed, origin/mode/cost tag/default action per row; 0.35–0.47 s macOS (343 items), 45–49 ms cachy-z13 (82 items) |
| A3 | pass | `verify --run`: one headless run; projection 0 predicted-not-seen / 0 seen-not-predicted / 0 conformance failures among the A1 set; header `observed 332.5 KB = attributed 322.6 + parameters 7.8 + framing 2.1 (bound capture)`; signal `skill listing: OVER CAP — 144 of 216 descriptions dropped (observed)` |
| A3b | pass | `just check` fixture suite: before/after freed cases (token-budgeted listing Δ?, plugin skills leaving, shared-origin item counted once, excluded CLAUDE.md with @imports) |
| A4 | pass | run for real on zbrain: `off plugin:slack` wrote one knob (settings.local.json 763eb592… → b33eb839…); fresh `init` had no `plugin:slack:slack`, no `slack:*` skills (8 before), no slack plugin (216 → 204 skills); `history` showed the op with RunContext; `undo` restored the file byte-for-byte (sha back to 763eb592…) |
| A5 | pass (half) | `builtin` item → refused with reason, exit 3; `--scope` never widened (exit 3). Quarantine half unreachable today (every emitted item has a knob) — covered by plan/journal unit tests |
| A6 | pass | `$EDITOR` opened the realpath; diff printed and journaled; `undo` restored bytes |
| A9 | pass | cachy-z13 (Linux x86_64, cargo 1.96.1, Claude Code 2.1.287): `--locked` build, same columns, drift warning, 193 tests, timing above |
| A10 | pass | `--readonly off …` and `--readonly verify --run` exit 3 without writing or running |
| A11 | pass | unresolved-origin row shows transport/log evidence, no guessed path; `verify --capture` with a mismatched RunContext refuses, exit 1, mismatch named |
| A12 | pass | brief 27,550 B (< 40 KB); plan.json validated (bad key / bad knob / bad scope each rejected naming the op); `apply --dry-run` prints diff + freed without writing; `history` shows reasons |
| A13 | pass | one self-contained HTML (284 KB, 0 `<script`), same rows/totals/signals as the table, 400 px without horizontal scroll (headless Chrome capture) |

Observations from A4 that opened R2: the cap-state warning survives a bound capture; `history` text does not mark undone entries; apply-path RunContext lacks `agent_version`.

## R2 — closed (2026-10-13)

Four defects surfaced by running A4 for real, fixed per-unit in worktrees and merged on `main` f3de040 (196 tests: lib 82 · bin 12 · cli 2 · differential 1 · plan_journal 34 · render 29 · resolve_claude 17 · verify 19 + 2 ignored). Re-verified live on zbrain with the release binary: `verify --run` passed (304 seen, 0 predicted-not-seen, 0 seen-not-predicted, 291 promoted; header observed 334.2 KB, OVER CAP 144/216, no cap-state warning); `off plugin:slack` → `history` prints `claude-code 2.1.296` → `undo` prints and marks `undone <ts>`, settings.local.json restored byte-for-byte. Receipt: zbrain `workflow/2lab-ai/harn/receipts-2026-10-13/A3-r2.txt`.

### Closed WUs

| WU | files owned | agent | state |
|---|---|---|---|
| WU-G | `src/resolve/claude/mod.rs` (`CAP_STATE_UNKNOWN` constant), `src/verify.rs` (dropped once a capture binds), `tests/verify.rs` | opus-coder (bg) | merged aba4f6b / c81dcc4 (+1 test, RED first) |
| WU-H | `src/render/text.rs` (`history` line ends `undone <ts>` for undone entries; `harn undo` output shares the renderer), `tests/render.rs` | opus-coder (bg) | merged 6f9a8a0 (+1 test, RED first) |
| WU-I | `src/plan.rs` (`ActionPlan.agent_version`, serde default), `src/writers.rs` (apply RunContext), `tests/plan_journal.rs`, `tests/render.rs` literal | opus-coder (bg) | merged 4930141 / 4c5b9f0 (+1 test, RED first) |
| WU-J | `src/journal.rs` (`edit` takes `agent_version`), `src/main.rs` (passes the picked tree's version), `tests/plan_journal.rs` | opus-coder (bg) | merged f3de040 (assertions added to `edit_journals_the_diff_and_undo_restores`, RED first; manual binary run with a scratch HOME showed `claude-code 9.9.9`) |

Noted, not changed: `tests/cli.rs` `edit_prints_the_diff_then_the_journal_line_and_records_the_target` cannot assert the version because the zbrain-like fixture HOME has no `versions/` dir; the verify report against the stale 02:52Z capture reads `passed: no` by capture drift (tools disallowed in that run, DEV.md/MEMORY.md changed since), not by code.

External review of R2 (gpt6-reviewer, one dispatch per fix, post-merge): 4/4 no MUST-FIX (G conditionally-ready on evidence the drive holds: the later `warnings.extend` only merges format-built strings; H/I/J release-ready). The three suggested tests landed per unit: CLI edit journals the discovered version (a0b24b8), ActionPlan JSON round-trip and missing-key load (f36625b), bound capture without a skill listing still drops the cap-state warning with a mutation check (412c758). `main` 412c758 = 199 tests, CI green on each push.

## R3 — closed (2026-10-13) — distribution chain

The R2 close claimed "release/preview/tap assets exist"; `main` had only `ci.yml`. This round delivered 02-architecture §Distribution and measured a real install. Receipt: zbrain `workflow/2lab-ai/harn/receipts-2026-10-13/preview-r1.txt`.

### Closed WUs

| WU | files owned | agent | state |
|---|---|---|---|
| WU-K | `.github/workflows/release.yml` (v* tag = Cargo version, 4-target matrix, `harn-<os>-<arch>` + SHA256SUMS, action-gh-release), `.github/workflows/preview.yml` (prerelease `preview-<stamp>-<sha12>` per main push, prune to 15, `tap-bump` job isolated: job-level presence boolean, token on the bump step only), `.github/scripts/bump-tap-preview.sh` + hermetic test (43 cases; placeholder guard `@[A-Z0-9_]+@`, per-platform url/sha pairing, drift-digits and hash-swap mutations caught) | opus-coder (bg) | review MUST-FIX 1 (digit-less placeholder guard — inherited from llmux, filed 2lab-ai/llmux#205) → fixed → re-review release-ready → merged 79b3279 (actionlint 0, `just check` 199) |
| WU-L | 2lab-ai/homebrew-tap: `Formula/harn.rb.tmpl`, `Formula/harn-preview.rb.tmpl`, scheduled `bump-harn` + `bump-harn-preview`, README rows | opus-coder (bg) | review release-ready → tap master 6839b4a |

### Measured

- Preview run 38358998618 on 79b3279: preflight, 4 builds, tap-bump-test, publish, tap-bump all green; prerelease `preview-2026-10-13-0650-79b3279e5c63` with 4 binaries (3.6–4.3 MB) + SHA256SUMS. Without `TAP_DISPATCH_TOKEN` the tap-bump job printed the notice and skipped the bump (HAS_TAP_TOKEN=false), as designed.
- macOS arm64: asset sha equals SHA256SUMS; `harn 0.1.0 (preview 79b3279e5c63) rules: claude-code 2.1.296`.
- cachy-z13 (Linux x86_64): `sha256sum -c` ok, installed to `~/.local/bin/harn`, same version line, zbrain listing in 40 ms.
- Tap: `Formula/harn-preview.rb` rendered from the tag with the bump job's commands (version 2026.10.13.0650) → tap master d41c203. `brew install 2lab-ai/tap/harn-preview` → Cellar 2026.10.13.0650; `/opt/homebrew/bin/harn --version` = the preview line; listing from PATH ~1 s.

User-held pieces left: `TAP_DISPATCH_TOKEN` on 2lab-ai/harn (immediate tap bump instead of the 6-hourly schedule) and the first `v*` tag (stable formula). Neither blocks preview installs.

## R4 — closed (2026-10-13) — the human's view and the AI default, after the user's first-install corrections (원문 3–6)

`main` 3f68443 (272 tests). Receipts: zbrain `workflow/2lab-ai/harn/receipts-2026-10-13/` (A14-tui-macos.txt + tui-80x24/ + tui-120x40/ frames, A14-tui-cachy.txt, A15-trim-live.txt + two full outputs, preview-r2.txt).

### Closed WUs

| WU | files owned | agent | state |
|---|---|---|---|
| WU-M | terminal width detection (`terminal_size`; `COLUMNS=0` = unset), `--table`, clap `wrap_help` + Getting started + `[AI operator]` labels, README install/use | opus-coder (bg) | review release-ready → merged 0e06125 (206 tests); its tip line was dropped at the TUI merge |
| WU-N | `src/trim.rs` + `Cmd::Trim`, shared `render::plan_footer`, tests/trim.rs (18), README trim paragraph, skill header line | opus-coder (bg) | review: MUST-FIX 3 (tool-less default planner; scratch cwd outside the target AND its enclosing git repo; decoded-schema extractor) → fixed abd8da5 + ac695b4 → release-ready → merged 3f68443 |
| WU-O | `src/tui/{mod,app,rows,view}.rs` (ratatui 0.30 + crossterm 0.29 + signal-hook), bare-command dispatch (TUI only when stdin+stdout are terminals), tests/tui.rs (27), README | opus-coder (bg) | 4 review units: app (MUST-FIX undo bound to the confirmed entry → `journal::undo_entry`), wiring (MUST-FIX signal exit through the guard), view (MUST-FIX ×7 over three rounds: tiny-frame underflow, ASCII completeness without rewriting user text, width-aware wrap and list cells, fold marker reserved, answer line fits, off-row gutter panic), tests (MUST-FIX rendered-frame assertions) → all release-ready → merged e04441a (244 tests) |

### Measured

- A14 macOS (release, zbrain, tmux 80×24 and 120×40): TUI opens with header + folded tree (345 items, skill listing `▸ 229`), j/k, `l` unfolds the listing, `h` folds, `/slack@` filters keeping the hierarchy, `w` why pane, `d` stages the slack plugin with its skills as side effects, `a`+`y` applies (journal a3c5a0d9 / 0c065c36), `u`+`y` restores — settings.local.json byte-identical after each size; `q` → EXIT=0, `stty` shows cooked mode back; every frame fits its width (display columns); item labels cut from the left with the tail kept, full id in `w`.
- A14 cachy-z13 (Linux): TUI test suite 27/27 on Linux (frames via TestBackend); `--table` and piped output 82 rows; no tmux there, so no live frames. The pty probe (no emulator answering queries) exposed a startup cursor-position query (ESC[6n) that times out and exits 1 with the terminal restored → WU-P opened (below).
- A15: `harn trim ~/2lab.ai/zbrain --dry-run` with the real `claude -p --output-format json --tools '' --max-turns 1` (prompt on stdin, fresh scratch cwd outside the repo): run 1 (branch) 69 s, 2 ops with reasons, nothing written; run 2 (merged main) 52 s, 2 ops (zworkflow 32 KB, cowork-plugin-management), `predicted 149,609 B → 116,848 B`, nothing written, 0 scratch dirs left. Fake-agent rows: 18 tests incl. rejected knob exit 3, missing binary → `/harn-trim` hint, readonly exit 3, non-TTY without `--yes`, TMPDIR-inside-target and sibling-in-repo isolation, > 64 KiB prompt no-deadlock.
- Distribution: previews `preview-2026-10-13-0834-e04441a02be6` and `…-0840-3f6844340e4a`; tap `Formula/harn-preview.rb` rendered → tap master 08c57c5; `brew upgrade harn-preview` → 2026.10.13.0840; `/opt/homebrew/bin/harn --version` = `harn 0.1.0 (preview 3f6844340e4a)`, `harn --help` lists `trim`; the installed binary opened the TUI on zbrain in tmux and quit with EXIT=0.

## R5 — closed (2026-10-13) — startup never waits on a terminal reply

WU-P (opus-coder, `.worktrees/wu-p-no-dsr`): the `ESC[6n` came from `Terminal::clear()` (ratatui-core 0.1.2 `buffers.rs:148` reads the cursor position first; crossterm waits 2 s), called at startup and twice after `$EDITOR`. Replaced by `full_screen()` = `Terminal::new` + `repaint()` (= `terminal.resize` at the current size → `ESC[2J` + back-buffer reset). A `Mute` TestBackend that fails cursor queries pins zero queries across start, keys, editor return and resize (3 tests, RED first with `clear()` inside). Review (wiring reviewer): release-ready, no MUST-FIX. Merged 54bc8d7 (275 tests, `just check` green). Measured: pty probe without an emulator on macOS and cachy-z13 → `header=True … exit=0` and no DSR in the stream (was exit 1 after 2 s); TUI suite 27/27 on Linux. Distribution: preview `preview-2026-10-13-0905-54bc8d7f04e1` → tap 4146ff3 → `brew upgrade harn-preview` 2026.10.13.0905 → `/opt/homebrew/bin/harn --version` = `harn 0.1.0 (preview 54bc8d7f04e1)`; installed binary passes the same probe and opens on zbrain in tmux (EXIT=0 on `q`).
