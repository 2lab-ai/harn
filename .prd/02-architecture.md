# harn — Architecture

Status: **planned** — v1, 2026-10-13. Boundaries for the first implementation drive. Follows
[`01-spec.md`](01-spec.md); where the two disagree, the spec wins and this file is fixed.

## Stack

- **Rust 2021, one binary `harn`**, no runtime dependencies. The llmux precedent is the house
  stack: `clap` 4 (derive, `clap_complete` for completions, `clap_mangen` for the man page),
  `ratatui` 0.30 + `crossterm` 0.29 for the TUI, `serde`/`serde_json`/`toml` for config and
  `--json`, `walkdir`, `trash` (OS Trash on macOS/Linux/XDG), `dirs`. xfx draws its terminal
  by hand on `rustix`; harn does not need that control and takes ratatui (`research/03` §B).
- Token estimates: `tiktoken-rs` for Codex/OpenAI models (local BPE). For Claude models no
  tokenizer is published; harn ships a bytes→tokens ratio table per model family measured from
  `wire` captures (`input_tokens` in `index.jsonl`/response vs body bytes), labelled `~tok`.
- Tests: `cargo test` unit tests on fixtures, plus **differential conformance tests** that run the
  real `claude`/`codex` binaries when present (`research/03` §A, cclint): compare the static tree
  with `stream-json init` / `debug prompt-input` on a fixture directory and fail on any diff.
  Gate = `just check` (`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --locked`), the llmux justfile shape.

## Modules

```
src/
  main.rs            clap entry: harn [dir] | why | off | on | rm | undo | history | verify | completions
  model.rs           Agent, Item, Kind, Mode, Origin, Knob, Size (serde; schema "harn/1")
  resolve/
    mod.rs           Resolver trait: fn resolve(dir, env) -> Tree ; both agents implement it
    claude/          settings layering (managed/user/project/local/--settings), instruction chain
                     + @imports + rules, auto-memory, hooks, skills listing, agents roster,
                     commands/output-style, mcp (scopes + plugin + connectors + dynamic),
                     plugins (installed_plugins.json + enabledPlugins), builtins table
    codex/           config layers + profiles + trust gates, AGENTS chain + fallbacks + budget,
                     skills roots (trust-independent), hooks (+ trusted_hash), rules,
                     plugins/marketplaces, mcp_servers (+ deferral by model catalog), builtins
  size.rs            bytes; ~tok per model family; wire (from a capture) ; freed (unique) calculation
  knobs.rs           Knob candidates per item, scope ranking, file+key writers (JSON/TOML edits
                     that preserve formatting; atomic write: temp + fsync + rename)
  journal.rs         $XDG_STATE_HOME/harn/journal/<ts>/ snapshots + ops.jsonl; undo; stale check
  oracle/
    claude.rs        spawn `claude -p` with OTEL_LOG_RAW_API_BODIES + stream-json; parse body + init
    codex.rs         `codex -C dir debug prompt-input`, `debug models`, `mcp list --json`,
                     `plugin list --json`; CODEX_ROLLOUT_TRACE_ROOT reader
    llmux.rs         read $XDG_STATE_HOME/llmux/raw-io.jsonl (first request of the newest session)
  verify.rs          Tree ⟷ capture matcher: seen / predicted-not-seen / seen-not-predicted
  render/
    table.rs         --plain table (ncdu-style bars, eighth-block + ASCII fallback)
    json.rs          schema harn/1
  tui/               ratatui app: tree pane, detail/why pane, staged-ops pane, key bar, overlay
```

Data flow: `resolve` → `Tree` (pure, no writes) → `size` fills `bytes`/`~tok`, `freed` →
`render` or `tui`. Writes happen only in `knobs` through `journal` (snapshot first). `verify`
produces a second `Tree` from a capture and `verify.rs` joins the two by item id.

## Item identity

Ids are stable across runs and agents: `<agent>/<kind>/<origin-layer>/<name-or-path>`, e.g.
`claude-code/mcp-server/plugin/plugin:slack:slack`, `claude-code/instruction/project/CLAUDE.md`,
`codex/skill/repo/.agents/skills/link`. Captures are matched to ids by the same rules the resolver
uses to name them (tool name prefixes `mcp__<server>__`, `mcp__plugin_<p>_<s>__`, skill names,
agent names, file headers inside `<system-reminder>` blocks, Codex `<skills_instructions>` roots).

## Where harn writes

| what | where |
|---|---|
| official knob | the agent's own file in the chosen scope (`.claude/settings.local.json`, `~/.claude/settings.json`, `~/.claude.json#projects[cwd]`, `.codex/config.toml`, `~/.codex/config.toml`, `~/.codex/hooks.json` state) — never a git-tracked team file without printing a warning first |
| quarantine | `<owner dir>/.harn-off/<name>` + `<owner dir>/.harn-off/manifest.json` (origin, date, loaders) |
| remove | OS Trash (`trash` crate; XDG Trash spec on Linux) |
| journal + snapshots | `$XDG_STATE_HOME/harn/` (`~/.local/state/harn/`) on Linux, `~/Library/Application Support/harn/` on macOS — never inside `~/.claude` or `~/.codex` |

A running agent is not restarted; harn prints "takes effect on the next session" after apply.

## Distribution (the llmux pattern, two deliberate differences)

- `.github/workflows/ci.yml` — fmt, clippy `-D warnings`, test on `ubuntu-latest` + `macos-latest`;
  the differential tests are skipped when the agent binaries are absent (CI) and run locally.
- `.github/workflows/release.yml` — on `v*` tag: verify tag == `Cargo.toml` version; build matrix
  `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu` (native `ubuntu-24.04-arm` runner); package each as
  `harn-<target>.tar.gz` holding `harn`, `completions/{bash,zsh,fish}`, `harn.1`, `LICENSE`;
  `SHA256SUMS`; `softprops/action-gh-release` with generated notes. **Difference 1 from llmux:**
  tarballs instead of bare binaries, so the tap formula and the AUR `-bin` package install
  completions and the man page without executing the binary.
- `.github/workflows/preview.yml` — every push to `main` publishes prerelease
  `preview-<YYYY-MM-DD-HHMM>-<sha12>` with the same assets.
- `2lab-ai/homebrew-tap`: `Formula/harn.rb.tmpl` + `harn-preview.rb.tmpl` rendered by the tap's
  scheduled `bump.yml` job, the way `llmux`/`xfx` are bumped; `on_macos`/`on_linux` ×
  `on_arm`/`on_intel` URLs; both formulae install `bin/harn`, so one is held at a time;
  `harn --version` prints `build_channel` (`release`|`preview`) + `build_revision` (xfx convention).
- **Difference 2:** an AUR package `harn-bin` (PKGBUILD repackaging the Linux tarballs) for
  CachyOS/Arch, following the `-bin` rule for prebuilt deliverables. Source package `harn` with
  `cargo fetch --locked` / `cargo build --frozen` is optional and later.
- `cargo install harn` works from crates.io once the name is published; `cargo-binstall`
  metadata points at the release tarballs.

## Receipts for the implementation drive (what "green" means, per `01-spec.md` §Acceptance)

1. `just check` green locally and in CI.
2. Differential tests green on the author's macOS against Claude Code 2.1.x and Codex 0.160.x.
3. A1–A11 run by hand on macOS and on `cachy-z13`, each with its command output captured into the
   workstream's `loop.done.md`.
4. One stable tag → release assets on GitHub → `brew install 2lab-ai/tap/harn` on macOS and the
   tarball (or `harn-bin`) on `cachy-z13` → `harn ~/2lab.ai/zbrain` runs on both.
