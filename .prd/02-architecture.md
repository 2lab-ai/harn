# harn — Architecture

Status: **shipped (P0)** — v1.5, 2026-10-13 (modules as built on `main`; P0.5 Codex resolver still planned). Boundaries for the first
implementation drive. Follows [`01-spec.md`](01-spec.md); where the two disagree, the spec wins
and this file is fixed.

## Stack

- **Rust 2021, one binary `harn`**, no runtime dependencies. P0 dependencies: `clap` 4 (derive,
  `clap_complete`), `serde`/`serde_json` (`preserve_order`), `toml_edit` (format-preserving TOML
  edits), `walkdir`, `dirs`, `similar` (unified diffs). JSON settings files are rewritten through
  `preserve_order` + 2-space serialization and the diff is always printed before the write
  (`serde_json` does not preserve formatting; the diff is the user's check).
- No TUI (user direction 2026-10-13); no `ratatui`/`crossterm`. The human-facing view is
  `render/html.rs`: one self-contained HTML file, inline CSS, no script that writes anything.
- Deliberately absent: a Trash crate (harn deletes nothing), tokenizers (all costs are bytes;
  `~tok` is a later optional feature), `clap_mangen` (`--help` covers eight subcommands).
- Tests: `cargo test` unit tests on fixture directories (a miniature `~/.claude`, `~/.codex`, and
  project trees with every discovery rule exercised), plus **differential conformance tests** that
  run the real `claude`/`codex` binaries when present (`research/03` §A, cclint): compare the
  static tree with `stream-json init` / `debug prompt-input` on a fixture directory and fail on any
  presence or content-conformance diff within the oracle's projection; plus `freed` fixtures
  (listing at cap, plugin side effects, shared items, `@imports`), each fixture defining both the
  before and the after rendering so the assertion is the fixture's own byte difference. Gate = `just check` (`cargo fmt --check`, `cargo clippy --all-targets --locked
  -- -D warnings`, `cargo test --locked`), the llmux justfile shape. Each resolver carries a
  `VERIFIED_AGAINST` version constant; a newer installed agent sets the header drift warning.

## Modules

```
src/
  main.rs            clap entry: harn [DIR] (= --dir) | why | path | off | on | edit | undo |
                     history | verify | brief | apply | report | completions; DIR and --dir together
                     = usage error
  model.rs           Agent, Item, Kind, Mode, Origin, Cost{source,predicted,observed}, Knob,
                     ActionPlan, RunContext (serde; schema "harn/1"); item selector resolution
                     (full id or unique suffix; ambiguous → candidates, exit 2)
  resolve/
    mod.rs           Resolver trait: fn resolve(target: &Target, overlay: Option<&Overlay>) -> Tree
                     Target = canonical dir + env + configured model/window + optional
                     bytes-per-token assumption; Overlay = an ActionPlan applied virtually (for
                     freed computation). Byte caps applied here; token caps yield `?` for the
                     state and `Δ?` for any delta (or `~` under the assumption) until captures
                     settle them — a before-only capture settles the state, only a before/after
                     pair settles a delta
    claude/          settings layering (managed/user/project/local/--settings), instruction chain
                     + @imports + rules, auto-memory (200-line/25 KB cap), hooks (all events; mode
                     always vs runtime), skills listing (1 % budget cap, 1,536-char cut), agents
                     roster, commands/output-style/statusLine/permission rules, mcp (scopes +
                     plugin + connectors + dynamic; schemas `?` until a capture), plugins
                     (installed_plugins.json + enabledPlugins from six sources), builtins table,
                     tool-search state (base URL first-party or not)
    codex/           config layers + profiles + the three trust gates, AGENTS chain + fallbacks +
                     32 KiB budget, skills roots (trust-independent, 2 %/10k cap), hooks
                     (+ trusted_hash), rules, plugins/marketplaces, mcp_servers (+ deferral by
                     model catalog), builtins
  plan.rs            ActionPlan builder: knob candidates per item by scope; quarantine only when
                     no knob and only in the same scope; freed = predicted(before) −
                     predicted(after) via resolve(target, overlay); side effects; warnings
                     (git-tracked file, scope wider than local, agent process running when the
                     target file is one the agent itself writes, e.g. ~/.claude.json)
  journal.rs         state dir: $XDG_STATE_HOME/harn (Linux), ~/Library/Application Support/harn
                     (macOS); journal/<ts>/ snapshots + ops.jsonl with RunContext + file hashes;
                     undo; stale check (hash mismatch → report, never force)
  writers.rs         atomic file replace (temp + fsync + rename); JSON via preserve_order;
                     TOML via toml_edit; quarantine move to <discovery-root-parent>/harn-off/<kind>/<name>
                     + manifest.json
  oracle/
    claude.rs        spawn `claude -p` with OTEL_LOG_RAW_API_BODIES + stream-json; parse body +
                     init; build RunContext from index.jsonl + init
    codex.rs         `codex -C dir debug prompt-input`, `debug models`, `mcp list --json`,
                     `plugin list --json`; CODEX_ROLLOUT_TRACE_ROOT reader
    capture.rs       readers for an OTEL raw-body dir, an llmux raw-io.jsonl record, a Codex
                     trace bundle → (Tree, RunContext); selection = RunContext match, never "newest"
  verify.rs          inputs: static Tree, capture Tree + RunContext, the oracle's observable
                     projection (modes runtime/none excluded; capture-only children attach under
                     their `?` parent, never seen-not-predicted). Gate 1 presence by id over
                     statically discoverable ids; gate 2 content conformance for reproducible
                     static contributions (text, listing entry, capped memory block) — mismatch =
                     resolver defect; per-run values (date, git status, hook stdout, MCP runtime
                     text) replace predicted → observed without failing; promotion only for
                     matched rows and regions
  render/
    table.rs         one table per agent (ncdu-style bars, eighth-block + ASCII fallback),
                     header with cost tag, attributed/framing split, tool-search state, drift warning
    json.rs          schema harn/1
    brief.rs         LLM-oriented text: totals, signals, rows (id · origin · mode · cost · freed ·
                     default knob), unknowns, and the harn-plan/1 format the agent must emit
    html.rs          read-only visualization (one file, inline CSS, both agents, same rows/signals)
  plan.rs (cont.)    PlanRequest (schema harn-plan/1) → validation (unique selectors, supported
                     knobs, no silent scope widening) → one ActionPlan; reasons carried into the journal
```

Data flow: `resolve` → `Tree` (pure, no writes, no spawned processes) → `plan` (freed via a
second `resolve` with an overlay) → `render` or `tui`. Writes happen only in `writers` through
`journal` (snapshot first). `oracle`/`capture` produce a second `Tree` + `RunContext`; `verify`
joins the two by item id.

## Item identity

Ids are stable across runs and agents: `<agent>/<kind>/<origin-layer>/<name-or-realpath>`, e.g.
`claude-code/mcp-server/plugin/plugin:slack:slack`,
`claude-code/instruction/project//Users/me/repo/CLAUDE.md` (files always by realpath, so an
ancestor's `CLAUDE.md` never collides with the cwd's), `codex/skill/repo//Users/me/repo/.agents/
skills/link`. Captures are matched to ids by the same naming rules the resolver uses (tool name
prefixes `mcp__<server>__` / `mcp__plugin_<p>_<s>__`, skill and agent names, file headers inside
`<system-reminder>` blocks, Codex `<skills_instructions>` roots).

## Where harn writes

| what | where |
|---|---|
| official knob | the agent's own file in the chosen scope (`.claude/settings.local.json`, `~/.claude/settings.json`, `~/.claude.json#projects[cwd]`, `.codex/config.toml`, `~/.codex/config.toml`, `~/.codex/hooks.json` state) — a git-tracked team file only after a printed warning; a file the agent itself rewrites (`~/.claude.json`) only after a warning when an agent process is running |
| quarantine | `<discovery-root-parent>/harn-off/<kind>/<name>` (`.claude/harn-off/…` for project items, `~/.claude/harn-off/…` for user items — outside `.claude/agents/**` and `.claude/skills/`; relocated per Open question 3 if the agent proves to scan it) + `manifest.json` (origin, date, loaders) |
| journal + snapshots | harn's state dir (above) — never inside `~/.claude` or `~/.codex`; snapshot files keep the source file's mode; retention: journal entries are never auto-deleted (`harn history --prune` is the user's call) |

A running agent is not restarted; every apply prints "takes effect on the next session".

## Distribution (the llmux pattern, unchanged)

- `.github/workflows/ci.yml` — fmt, clippy `-D warnings`, test on `ubuntu-latest` + `macos-latest`;
  the differential tests are skipped when the agent binaries are absent (CI) and run locally.
- `.github/workflows/release.yml` — on `v*` tag: verify tag == `Cargo.toml` version; the llmux
  build matrix (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu` on `ubuntu-24.04-arm`); bare binaries named `harn-<os>-<arch>` +
  `SHA256SUMS`; `softprops/action-gh-release` with generated notes — the same asset shape as llmux,
  so `2lab-ai/homebrew-tap`'s scheduled `bump.yml` job and the `*.rb.tmpl` renderer apply with a
  name change only.
- `.github/workflows/preview.yml` — every push to `main` publishes prerelease
  `preview-<YYYY-MM-DD-HHMM>-<sha12>` with the same assets.
- `2lab-ai/homebrew-tap`: `Formula/harn.rb.tmpl` + `harn-preview.rb.tmpl`; `on_macos`/`on_linux`
  × `on_arm`/`on_intel`; both install `bin/harn`, one held at a time; `harn --version` prints
  `build_channel` (`release`|`preview`) + `build_revision` (xfx convention). On `cachy-z13`:
  Homebrew on Linux via the same tap, or the release binary copied to `~/.local/bin/harn`.
- `cargo install harn` from crates.io once published. No AUR package, no man page, no tarballs
  in this version — each is added on evidence of a second user, not before.

## Receipts for the implementation drive (what "green" means, per `01-spec.md` §Acceptance)

1. `just check` green locally and in CI.
2. Differential tests green on the author's macOS: P0 against Claude Code 2.1.x; P0.5 adds Codex 0.160.x (Codex never gates P0).
3. P0 rows A1–A6, A9–A11 run by hand on macOS and on `cachy-z13`, each with its command output
   captured into the workstream's `loop.done.md`; P0.5 adds A7–A8; the TUI drive adds its own rows.
4. One stable tag → release assets on GitHub → `brew install 2lab-ai/tap/harn` on macOS and the
   binary on `cachy-z13` → `harn --dir ~/2lab.ai/zbrain` runs on both.
