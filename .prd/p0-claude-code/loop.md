# loop — P0 drive

Status: in-progress (2026-10-13; R1–R4 closed in `loop.done.md`; one hardening unit open)

## R5 — startup must not wait on a terminal reply (open)

| WU | files owned | agent | state |
|---|---|---|---|
| WU-P | `src/tui/mod.rs` (no cursor-position query at startup; Fullscreen/Fixed viewport sized by ioctl, resize kept) | opus-coder (bg) | in `.worktrees/wu-p-no-dsr` (branch `wu/p-no-dsr`); oracle = the pty probe script (expect `header=True … exit=0`) |

Found by the cachy-z13 pty probe (and reproduced on macOS): under a pty whose other side does not answer `ESC[6n`, harn enters the alternate screen, waits for the cursor position, then exits 1 with the terminal restored. Real terminals answer, so the brew-installed TUI works, but startup must not depend on the reply.

Next: WU-P → review → merge → gate → push → preview → tap → `brew upgrade` → re-run the pty probe on cachy-z13 and macOS → close R5 → Status done.

User gates left: first `v*` tag; `TAP_DISPATCH_TOKEN` (optional).
