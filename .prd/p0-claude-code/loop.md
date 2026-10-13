# loop — P0 drive

Status: in-progress (2026-10-13; R1–R3 closed in `loop.done.md`; R4 = the user's first-install corrections, 원문 3–6: bare `harn` = TUI, `harn trim` = the AI default)

## R4 — human view + AI default (open)

| WU | files owned | agent | state |
|---|---|---|---|
| WU-M | terminal width detection (`terminal_size`), `--table` flag (also the non-TTY default), `--help` Getting started, `[AI operator]` labels, README usage order | opus-coder (bg) | in `.worktrees/wu-m-first-screen` (branch `wu/m-first-screen`); scope cut from an "overview" screen to this after 원문 5 |
| WU-N | `src/trim.rs`, `Cmd::Trim`, shared plan printing with `apply`, tests/trim.rs, README `harn trim`, skill header line | opus-coder (bg) | in `.worktrees/wu-n-trim` (branch `wu/n-trim`) |
| WU-O | `src/tui/` (ratatui 0.30 + crossterm 0.29, terminal-free `App`), bare-command dispatch on a TTY, tests/tui.rs (TestBackend), README first line | opus-coder (bg) | in `.worktrees/wu-o-tui` (branch `wu/o-tui`) |

Merge order: M → N → O (each adds a main.rs arm; resolve by hand), review per unit before merge (gpt6-reviewer), `just check` after each, one push at the end → preview publish → tap render → `brew upgrade harn-preview` → A14 receipt = tmux captures at 80×24 and 120×40 on fable-m5max + cachy-z13; A15 receipt = fake-agent run + one real `claude -p` run.

Open gaps: G3 (pointer-level undo, P1), G7 (hook stdout join, P1); `TAP_DISPATCH_TOKEN` and the first `v*` tag stay user gates.

Next: agents report → review → merge → gate → push → measure → close R4 → Status done.
