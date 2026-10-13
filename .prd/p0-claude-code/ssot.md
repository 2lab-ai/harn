# ssot — P0 drive: Claude Code chain (fixed 2026-10-13)

Status: in-progress

## User statement (verbatim, 2026-10-13, `/goal`)

> prd대로 작업 진행

## Fixed target

`01-spec.md` v2.9 §P0 and acceptance rows **A1–A6, A9–A11**, on the author's macOS and on
`cachy-z13` (Linux x86_64, cargo 1.96.1, `~/.local/bin/claude` present, no `codex`). P0.5 (Codex,
A7–A8) and P1 (TUI) are out of this drive. Interfaces between work units are fixed in
[`../03-p0-interfaces.md`](../03-p0-interfaces.md); a WU that needs to change an interface says so
in its report instead of editing another WU's files.

## Gate

`just check` = `cargo fmt --check` · `cargo clippy --all-targets --locked -- -D warnings` ·
`cargo test --locked`. Differential tests run only when `claude` is on PATH (skipped in CI).

## Receipts (what closes a row)

Each A-row: the exact command, its captured output (file under the scratchpad, path recorded in
`loop.done.md`), on both machines where the row says so.
