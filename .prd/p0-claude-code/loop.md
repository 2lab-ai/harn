# loop — P0 drive

Status: in-progress (2026-10-13; spec v3.0 — no TUI, AI operator: brief/apply/report added to WU-A/C/E)

## R1 — scaffold + four modules (open)

| WU | files owned | agent | state |
|---|---|---|---|
| WU-A | `Cargo.toml`, `justfile`, `src/lib.rs`, `src/main.rs`, `src/model.rs`, module stubs, `tests/fixtures/README.md`, `.github/workflows/ci.yml` | opus-coder (bg) | done — ceacd63 (31 tests), interface amendments recorded in 03 |
| WU-B | `src/resolve/mod.rs`, `src/resolve/claude/**`, `tests/resolve_claude.rs`, `tests/fixtures/zbrain-like/**` | opus-coder (bg) | dispatched 2026-10-13 13:5x in `.worktrees/wu-b-resolve` (branch `wu/b-resolve`) |
| WU-C | `src/render/**` (table, json, brief, html), `tests/render.rs` | opus-coder (bg) | dispatched 2026-10-13 13:5x in `.worktrees/wu-c-render` (branch `wu/c-render`) |
| WU-D | `src/oracle/**`, `src/verify.rs`, `tests/verify.rs`, `tests/fixtures/capture-*/**` | opus-coder (bg) | dispatched 2026-10-13 13:5x in `.worktrees/wu-d-oracle` (branch `wu/d-oracle`) |
| WU-E | `src/plan.rs` (incl. PlanRequest harn-plan/1), `src/writers.rs`, `src/journal.rs`, `tests/plan_journal.rs` | opus-coder (bg) | dispatched 2026-10-13 13:5x in `.worktrees/wu-e-plan` (branch `wu/e-plan`) |
| WU-F | integration: differential tests, `.claude/skills/harn-trim/SKILL.md`, A-row receipts A1–A6 + A9–A13 on macOS + cachy-z13 | dispatcher + devops | after B–E |

Build facts: cargo 1.98.0 (macOS), 1.96.1 (cachy-z13); `just` 1.56.0 (macOS).

Open gaps: none yet (no measurement).

Next: B–E land → merge each branch into main (disjoint files; `just check` after each) → WU-F integration: selector substring tier, differential test, `/harn-trim` skill, receipts A1–A6 + A9–A13 on macOS, then cachy-z13.
