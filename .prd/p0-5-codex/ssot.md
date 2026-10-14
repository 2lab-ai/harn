# ssot — P0.5 drive: Codex resolver (fixed 2026-10-14)

Status: in-progress

## User statement (verbatim, 2026-10-14, `/goal`)

> 1. 그리고 harn repo 문서도 업데이트해줘
>  2. 그리고 남은 작업 끝까지 진행해줘

"남은 작업" = the open items of the closed P0 drive (`../p0-claude-code/loop.md`: "Next start point:
P0.5 Codex", follow-ups #2–#4). This folder is the P0.5 part.

## Fixed target

`01-spec.md` §P0.5 and acceptance rows **A7–A8**, on the author's macOS against `codex-cli 0.160.x`;
A9's "identical columns and semantics" on `cachy-z13` for the Codex tree too (no `codex` binary
there: the static tree must still resolve from files). The rules are `01-spec.md` §Codex coverage
and `research/02` (pinned to `rust-v0.160.1`); module boundaries `02-architecture.md` (`resolve/codex/`,
`oracle/codex.rs`). Same commands as P0 with `--agent codex`: tree, `why`, `off`/`on`/`undo`,
`brief`, `report`, `verify`; the TUI shows the Codex tree as it shows Claude Code's.

Out of scope (P1): `harn trim --agent codex`, usage evidence, tool schemas from a live run
(`CODEX_ROLLOUT_TRACE_ROOT`).

## Constraints

- A resolver reads files only; it never starts `codex`. Only `oracle/codex.rs` spawns it, and only
  under `verify`.
- `codex debug prompt-input` spawns MCP servers and writes into `CODEX_HOME`: the oracle states
  this before running (as `verify --run` does for Claude Code).
- Knobs are Codex's own config keys in the narrowest scope that has one; `-c` dotted overrides are
  never emitted for quoted keys (`plugins."x@y"` is silently ignored that way — research/02 §0.5).
- Public repo: no home paths, emails or private strings in code, tests, fixtures or docs.
