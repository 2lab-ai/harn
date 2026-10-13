# harn

`harn <dir>` shows everything an AI coding agent loads into its model request when started in
`<dir>` — one tree per agent, each row with its size, where it came from, whether it is always
loaded or only advertised, and the official switch that turns it off — and lets you turn items
off reversibly. Claude Code first, Codex CLI second. One binary for macOS and Linux.

`harn trim [DIR]` — the default way to trim; you never type an item name. It builds the brief (`harn
brief`), asks your coding agent for a plan in one headless request (`claude -p --output-format json
--tools "" --max-turns 1`: no tools, one turn, the prompt piped on stdin; announced before it runs),
validates the reply exactly as `harn apply` does, and prints the plan: each op with the agent's
reason, the bytes freed, the side effects (a plugin takes its skills, agents and hooks with it) and
the diff of every settings file it touches. On a terminal it then asks `apply? [y/N]`; `--dry-run`
stops before asking, `--yes` applies without asking (required when stdin is not a terminal). Applied
changes are journaled with their reasons (`harn history`), and `harn undo` puts every file back byte
for byte. `--agent-cmd "<cmd>"` replaces the agent command and runs as given (yours, trusted: harn
adds no flags); a trailing `-` means: pipe the prompt on stdin instead of passing it as the last
argument. Without `claude` on PATH, run `/harn-trim` inside Claude Code instead — the same loop,
step by step.

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
