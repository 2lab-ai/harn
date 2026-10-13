# loop — P0 drive

Status: in-progress (2026-10-13; R1–R5 closed in `loop.done.md`; R6 = the TUI as a screen, not a boxed table)

## R6 — TUI information design (open)

User verdict on the R4 frames (verbatim in `prd-rewrite/ssot.md` 원문 7): unreadable; "the old thing with a UI stuck on". My frame review: zbrain `workflow/2lab-ai/harn/receipts-2026-10-13/tui-readability-review.md` (16 defects). Spec: `04-tui-design.md`.

| WU | files owned | agent | state |
|---|---|---|---|
| WU-Q | `src/tui/model.rs` (view-model: groups by kind, human names, plugin facets folded, own bytes), `src/tui/view.rs` (2-line header, group list with bars, detail panel, cards), `src/tui/app.rs` (d → card → Enter; w/i/? cards), tests/tui.rs (rendered-frame assertions + negative test for raw ids/paths/hashes) | opus-coder (bg) | in `.worktrees/wu-q-tui-design` (branch `wu/q-tui-design`) |

Gate for this round (A16): the dispatcher reads every captured frame (80×24, 120×40: first screen, Plugins opened, detail on slack, d card, after Enter, after u, w card, ? card, notes) and finds no raw id / absolute path / hash / `Δ?` / `mode` / `knob` / `ops (` / `rows shown of` / mid-word wrap, and can state the five biggest things from the first screen alone. Then external review (ui-ux reading + gpt6-reviewer), merge, preview, tap, brew upgrade, re-read the installed binary's frames.

Next: WU-Q report → I read the frames → fixes until A16 holds → review → merge → publish.
