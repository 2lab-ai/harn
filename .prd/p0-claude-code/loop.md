# loop — P0 drive

Status: in-progress (2026-10-13)

## R1 — scaffold + four modules (open)

| WU | files owned | agent | state |
|---|---|---|---|
| WU-A | `Cargo.toml`, `justfile`, `src/lib.rs`, `src/main.rs`, `src/model.rs`, module stubs, `tests/fixtures/README.md`, `.github/workflows/ci.yml` | opus-coder (bg) | dispatched |
| WU-B | `src/resolve/mod.rs`, `src/resolve/claude/**`, `tests/resolve_claude.rs`, `tests/fixtures/zbrain-like/**` | opus-coder (bg), after WU-A | planned |
| WU-C | `src/render/**`, `tests/render.rs` | opus-coder (bg), after WU-A | planned |
| WU-D | `src/oracle/**`, `src/verify.rs`, `tests/verify.rs`, `tests/fixtures/capture-*/**` | opus-coder (bg), after WU-A | planned |
| WU-E | `src/plan.rs`, `src/writers.rs`, `src/journal.rs`, `tests/plan_journal.rs` | opus-coder (bg), after WU-A | planned |
| WU-F | integration: differential tests, A-row receipts macOS + cachy-z13 | dispatcher + devops | after B–E |

Build facts: cargo 1.98.0 (macOS), 1.96.1 (cachy-z13); `just` 1.56.0 (macOS).

Open gaps: none yet (no measurement).

Next: WU-A lands → commit → create `.worktrees/wu-{b,c,d,e}` on branches `wu/b-resolve` … → dispatch B–E in parallel.
