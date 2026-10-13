---
name: harn-trim
description: Use when the user wants the AI-coding-agent harness of the current directory trimmed or reviewed — triggers on "/harn-trim", "하네스 정리", "하네스 줄여", "harness trim", "what loads here and what should go". Args — none (uses the current directory); optional `--dry-run` to plan without applying.
---

# harn-trim — the AI operator loop over `harn`

You are the operator. The human reads; you decide and act, through `harn`, with every change
journaled and reversible. Never edit the agent's settings files by hand — only through `harn`.

## Loop

1. **Facts first.** Run `harn brief` in the target directory. It prints, per agent, the totals,
   the signals (tool search state, over-cap listings), every effective item with its id, origin,
   mode, cost, freed and default knob, the open unknowns, and the exact plan format.
2. **Decide.** For each candidate, answer: does this directory's work need it? Use the item's
   `last_used`, its origin (a plugin enabled for every project vs. this project's own files), and
   the freed bytes. Prefer narrow knobs (project-local) over user-wide ones; never turn off a
   plugin for its MCP tools alone when `disabledMcpServers` exists for the server. Leave items whose
   cost is `?` unless a capture has filled them in (`harn verify --run` once, if the user allows a
   model request). Do not touch `builtin` rows.
3. **Write the plan.** `plan.json` with schema `harn-plan/1`, one op per change, each with a
   `reason` the human can read in `harn history` a month from now.
4. **Dry-run, then apply.** `harn apply plan.json --dry-run` prints the diff, freed and side
   effects; read the side effects (a plugin takes its skills, agents and hooks with it). If they
   are acceptable, `harn apply plan.json`.
5. **Verify.** `harn verify --run` (one headless session of the agent) or `harn --table` again to
   see the predicted totals; report before → after to the human with the journal id, and the one
   line that undoes everything: `harn undo`.

## Rules

- Scope never widens on its own: use `"scope": "user"` in an op only when the user asked for a
  machine-wide change.
- A plan the validator rejects is wrong, not harn: fix the selector or knob index and retry.
- Report in the human's language, operator style: what was turned off, why, how many bytes, how
  to undo. No markers, no praise.
