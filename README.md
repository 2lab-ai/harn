# harn

`harn <dir>` shows everything an AI coding agent loads into its model request when started in
`<dir>` — one tree per agent, each row with its size, where it came from, whether it is always
loaded or only advertised, and the official switch that turns it off — and lets you turn items
off reversibly. Claude Code first, Codex CLI second. One binary for macOS and Linux.

## Usage

- `harn [DIR]` on a terminal — the interactive view: one tab per agent, the tree sorted by freed
  bytes, children folded under their parent (`▸ N`). `j`/`k` move · `h`/`l` fold · `/` filter ·
  `s` sort (freed, cost, name; again reverses) · `w` why · `d` stage off (shows the plan) · `a` apply
  · `o` back on · `u` undo · `e` edit in `$EDITOR` · `r` refresh · `t` AI trim · `?` every key · `q`
  quit. `--readonly` greys and refuses the keys that write.
- `harn trim` — the AI decides what to turn off (`/harn-trim` inside Claude Code).
- `harn report --html <OUT>` — a self-contained, read-only HTML report.
- `harn --table [DIR]` — the table on stdout; also what `harn` prints when stdout is not a terminal.
- `harn --json [DIR]` — the same model as JSON (schema `harn/1`).

Status: **scaffold, P0 in progress** (2026-10-13). The CLI parses every P0 command and the data
model is in place; the resolver, renderers, oracle, planner and journal are stubs that say so. The
product definition lives in [`.prd/01-spec.md`](.prd/01-spec.md); the implementation boundaries in
[`.prd/02-architecture.md`](.prd/02-architecture.md); the module contract in
[`.prd/03-p0-interfaces.md`](.prd/03-p0-interfaces.md); the research it rests on in
[`.prd/research/`](.prd/research/).

Development gate: `just check` — `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, the same three steps
CI runs on macOS and Ubuntu.

Why: a fresh Claude Code session in one real working directory sends a 332 KB first request before
reading the user's 30-byte instruction. Nothing shows that list for a directory without starting a
session, with a size per item, the file or setting that produced it, and the switch that removes it.

License: MIT.
