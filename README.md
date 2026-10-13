# harn

`harn <dir>` shows everything an AI coding agent loads into its model request when started in
`<dir>` — one tree per agent, each row with its size, where it came from, whether it is always
loaded or only advertised, and the official switch that turns it off — and lets you turn items
off reversibly. Claude Code first, Codex CLI second. One binary for macOS and Linux.

Status: **spec only** (2026-10-13). The product definition lives in [`.prd/01-spec.md`](.prd/01-spec.md);
the implementation boundaries in [`.prd/02-architecture.md`](.prd/02-architecture.md); the research it
rests on in [`.prd/research/`](.prd/research/). No code yet.

Why: a fresh Claude Code session in one real working directory sends a 332 KB first request before
reading the user's 30-byte instruction. Nothing shows that list for a directory without starting a
session, with a size per item, the file or setting that produced it, and the switch that removes it.

License: MIT.
