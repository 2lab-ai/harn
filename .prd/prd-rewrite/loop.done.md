# loop.done — PRD authoring drive (append-only)

## R1 — closed (2026-10-13)

Result: PRD v1 from the user's statement (`research/00`), four research inputs (`research/01`
Claude Code rules, `02` Codex rules, `03` prior art + UX, `04` local measurements incl. two
verified Claude Code oracles and the wire partition), `01-spec.md` rewritten from scratch (v2),
`02-architecture.md` (v1), then five trinity rounds (grok-4.7 · gpt-6-astra · fable) to v2.9 /
v1.3 — unanimous APPROVE, MUST-FIX none ×3, round 5/5. Name settled: `harn` (first name `loadout`
dropped for migsilva89/loadout, same job same name). Repo initialised locally with MIT, README,
`.gitignore`; git identity `Z <z@2lab.ai>`.

| WU | scope | owner | state |
|---|---|---|---|
| WU-1 | `research/00-prd-v1-from-user-statement.md` | dispatcher | done (e854c34) |
| WU-2 | `research/01-claude-code-loading-rules.md` | librarian agent | done (353 lines, docs-cited) |
| WU-3 | `research/02-codex-loading-rules.md` | librarian agent | done (244 lines, source-pinned `rust-v0.160.1`) |
| WU-4 | `research/03-prior-art-and-ux.md` | librarian agent | done (309 lines) |
| WU-4b | `research/04-local-measurements.md` | dispatcher | done (`/context`, `codex debug prompt-input`, OTEL raw-body + `stream-json init` oracles, 332,548-byte partition, listing over cap 146/216) |
| WU-5 | `01-spec.md` v2 + `02-architecture.md` v1 | dispatcher | done (35531f3) |
| WU-6 | trinity review until unanimous | panel | done — R1 1/3 · R2 0/3 · R3 1/3 · R4 1/3 · R5 3/3 on v2.9 / v1.3 |

Trinity round ledger (what each round changed): see `01-spec.md` §Revision log v2.1 → v2.9.
