# harn

`harn <dir>` shows everything an AI coding agent loads into its model request when started in
`<dir>` — one tree per agent, each row with its size, where it came from, whether it is always
loaded or only advertised, and the official switch that turns it off — and lets you turn items
off reversibly. Claude Code first, Codex CLI second. One binary for macOS and Linux.

## Install and use

```
brew install 2lab-ai/tap/harn-preview
```

1. `harn` — what Claude Code loads in this directory before your first message, biggest saving
   first. On a terminal this is the interactive view; `harn --table` prints the table (also the
   default when stdout is not a terminal).
2. `harn trim` — the AI decides what to turn off and applies it; every change is journaled and
   `harn undo` puts it back.
3. `harn report --html harn.html` — the same picture as a shareable read-only page.
4. `harn verify --run` — measure one real session (starts the agent once: runs your hooks and MCP
   servers, may cost one model request).

The commands marked `[AI operator]` in `harn --help` (`why`, `path`, `off`, `on`, `edit`, `brief`,
`apply`) are what the AI runs on your behalf; you never type an item name.

Status: P0 for the Claude Code chain is on main; preview builds install via the tap.

The product definition lives in [`.prd/01-spec.md`](.prd/01-spec.md); the implementation
boundaries in [`.prd/02-architecture.md`](.prd/02-architecture.md); the module contract in
[`.prd/03-p0-interfaces.md`](.prd/03-p0-interfaces.md); the research it rests on in
[`.prd/research/`](.prd/research/).

Development gate: `just check` — `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, the same three steps
CI runs on macOS and Ubuntu.

Why: a fresh Claude Code session in one real working directory sends a 332 KB first request before
reading the user's 30-byte instruction. Nothing shows that list for a directory without starting a
session, with a size per item, the file or setting that produced it, and the switch that removes it.

License: MIT.
