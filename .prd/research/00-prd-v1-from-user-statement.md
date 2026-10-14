# PRD v1 — `harn`, as stated by the user (2026-10-13)

Status: superseded by `../01-spec.md` once the research-based rewrite lands. This file is the
user's statement in PRD form and adds nothing the statement or the measured captures do not say.
Verbatim statement: [`../prd-rewrite/ssot.md`](../prd-rewrite/ssot.md).

## Problem

When Claude Code starts in `~/work/project` (a real working folder), the model's first request is
already ≈231 KB before the user's instruction (6–61 B) is read. That payload is assembled from four
places the user listed — Claude Code's built-in prompt and built-in skills, the `~/.claude` harness
(prompts, agents, skills, hooks), the working folder's harness, and then the instruction — and the
same happens in Codex with its own files. Nothing shows the user that list, how large each item is,
where it came from, or how to remove it. Optimizing the harness today means guessing.

## Goal

One system tool, run as

```
harn ~/work/project
```

that lists every harness item an AI agent loads when started in that directory and lets the
user edit (remove, disable) items to optimize the harness.

## Users and environments

- One user today: the owner of zbrain, running Claude Code and Codex CLI.
- Runs on CachyOS Linux (`cachy-z13`) and local macOS. Same behavior on both.
- Shape: CLI; a TUI or GUI "if needed" — it is a system tool, so the CLI is the base.

## What "harness" means here (user's four layers, Claude Code)

1. Claude Code built-in prompt and built-in skills. Editable only through the local `llmux`
   proxy (which can rewrite requests), so `harn` must at least show them.
2. `~/.claude` — prompts (CLAUDE.md), agents, skills, hooks.
3. The working folder's harness (`<cwd>/CLAUDE.md`, `.claude/**`, …).
4. The user's instruction, executed after the above.

Codex: the equivalent layers for Codex CLI (user statement names Codex as a target; the
layer list is to be established by research).

## Deliverables of this drive (user's three tasks)

1. This document.
2. Research: similar tools; how to make the listing/editing comfortable.
3. A PRD rewritten from scratch on the research, aiming at the best UI and behavior.

## Repository and distribution

- Repository `2lab-ai/harn` (name chosen by the implementer; user left it open).
- Build and release the way `2lab-ai/llmux` does: open source, GitHub Actions CI, release on
  `v*` tags with per-platform binaries, preview channel, Homebrew tap formula.

## Acceptance scenarios (as stated; to be sharpened in v2)

| # | run | expected observation |
|---|---|---|
| A1 | `harn ~/work/project` on macOS | a list of every harness item Claude Code loads for that directory |
| A2 | same on cachy-z13 | same list shape and the items that exist on that machine |
| A3 | pick an item and remove/disable it | the next Claude Code session in that directory no longer loads it |
| A4 | the same for Codex | the Codex-side list and edits |

## Measured facts the PRD rests on (2026-10-02 capture, 5 fresh sessions)

| element | bytes | note |
|---|---|---|
| tools array | 108,969 | 61 tools; 36 belong to MCP server `blender`, whose config was not found locally |
| messages[0] block 0 | 45,898 | `~/.claude/CLAUDE.md` 738 B · zbrain `CLAUDE.md` 6,506 B · `rules/DEV.md` 8,602 B · `rules/PRINCIPLES.md` 4,261 B · `MEMORY.md` 25,560 B |
| messages[0] block 1 | 3,548 | userEmail + gitStatus |
| messages[0] block 2 | 596 | commit attribution |
| messages[0] block 3 | 6–61 | the instruction |
| messages[1] block 0 | 56,136–56,359 | SessionStart hook stdout 3,457 B · agent roster 16,915 B · MCP instructions 2,076 B · skill list 31,242 B · date · Advisor 2,017 B |
| system blocks | 6,160 | billing header, identity, harness rules — built-in |
| total | ≈230,900 | identical across runs except the instruction and one gitStatus delta |

## Open questions carried into research

- Which items have an official "disable" knob and which can only be deleted or moved?
- Where does a tool definition come from when no local config names it (the `blender` case)?
- Does Codex expose a trustworthy "what the model sees" dump (`codex debug prompt-input`)? Does
  Claude Code expose anything equivalent, or must `harn` re-implement discovery?
- Estimated size (bytes/tokens from files) vs measured size (bytes on the wire through a proxy):
  which does the tool show, and how does it say which one it is?
