# loop — P0 drive

Status: in-progress (2026-10-13; spec v3.0 — no TUI, AI operator: brief/apply/report added to WU-A/C/E)

## R1 — scaffold + four modules (open)

| WU | files owned | agent | state |
|---|---|---|---|
| WU-A | `Cargo.toml`, `justfile`, `src/lib.rs`, `src/main.rs`, `src/model.rs`, module stubs, `tests/fixtures/README.md`, `.github/workflows/ci.yml` | opus-coder (bg) | done — ceacd63 (31 tests), interface amendments recorded in 03 |
| WU-B | `src/resolve/mod.rs`, `src/resolve/claude/**`, `tests/resolve_claude.rs`, `tests/fixtures/zbrain-like/**` | opus-coder (bg) | merged 0a094db (73 tests; zbrain tree = init event set-for-set; 57/57 listing entries + 26/26 roster lines byte-exact vs capture) |
| WU-C | `src/render/**` (table, json, brief, html), `tests/render.rs` | opus-coder (bg) | merged 8ee7e9f + 697b159 (63 tests; JournalEntry literal fixed at merge) |
| WU-D | `src/oracle/**`, `src/verify.rs`, `tests/verify.rs`, `tests/fixtures/capture-*/**` | opus-coder (bg) | merged 8d8317f (59 tests; real capture sums to 332,548; one live run bound) |
| WU-E | `src/plan.rs` (incl. PlanRequest harn-plan/1), `src/writers.rs`, `src/journal.rs`, `tests/plan_journal.rs` | opus-coder (bg) | merged ab69a44 + 7eacabb (74 tests; fixture settings.local.json needed a .gitignore negation) |
| WU-F | integration (verify join by name, selector substring, A2 < 1 s, brief < 40 KB, G1/G2/G5, differential test, receipts) | opus-coder (bg) | dispatched in `.worktrees/wu-f-integration` (branch `wu/f-integration`) |

Build facts: cargo 1.98.0 (macOS), 1.96.1 (cachy-z13, no `just`, no `bc`); `just` 1.56.0 (macOS). main = 176 tests. First e2e on zbrain (release, macOS): 343 items, 1.46 s, json 548 KB, brief 99 KB, html 284 KB (0 script), verify --run 6.2 s promoted 267 / 24 pred-not-seen / 29 seen-not-pred (layer-id mismatch, → WU-F). cachy-z13: builds, 176 tests pass, listing runs with drift warning (2.1.287 vs rules 2.1.296), tool search ON there → receipt `harn-receipts/A9-cachy-z13-r0.txt` (scratchpad).

Open gaps (from WU reports, for WU-F):
- G1 `main` `why` loop builds the local plan twice (local + project) when an item has local+user knobs → dedupe by knob (WU-E note).
- G2 `main::open_journal` must call `Journal::open()` (0700 dir) instead of building the struct.
- G3 `harn on A` after `off A; off B` on the same file reports Stale (whole-file snapshot) → pointer-level undo for `on` = P1 (test `on_of_an_older_entry_sharing_a_file_is_stale` pins it).
- G4 brief budget: ids for skills/agents/commands must be init-event names (WU-B told); realpath ids blow 40 KB.
- G5 deferred render inputs: attributed/parameters/framing fields on Tree for the observed header, tool-search cause string, `Tree.home` for `~`, version line exposed from the lib.
- G6 selector 4th tier (unique substring) so `plugin:slack` resolves.
- G7 hook stdout has no join key between config hook and capture stdout → stays `?`, not gated (WU-D); a rule is P1.
- G8 `generate.py` in `tests/fixtures/capture-synthetic/` (Python in a Rust repo) — keep as the fixture generator, document in README.

Next: B–E land → merge each branch into main (disjoint files; `just check` after each) → WU-F integration: selector substring tier, differential test, `/harn-trim` skill, receipts A1–A6 + A9–A13 on macOS, then cachy-z13.
