---
title: harn PRD research 03 — prior art and UX patterns
type: research
tags: [harn, prd, prior-art, tui, ux]
---

# 03 — Prior art and UX patterns

Researched 2026-10-13. Stars and last-push dates come from `gh api repos/<owner>/<repo>` on
2026-10-13. Every row cites a URL. `[미확인]` = not confirmed from a primary source.

## 0. Read first: a product with the same name already exists

- **migsilva89/loadout**: "See and manage what your coding assistants load". Swift 6/SwiftUI app for
  macOS 15+. 98★. Last push 2026-10-01. Latest release v0.6.2 (2026-09-20). MIT.
  `brew install --cask migsilva89/loadout/loadout`. Site: loadout.migsilva.dev. It covers Claude
  Code, Codex, Antigravity and OpenCode, so the agents and the job overlap with ours.
  https://github.com/migsilva89/loadout
- What it does **not** do, and where our product can be different:
  (a) Its `docs/SPEC.md` never mentions tokens. The only "size" in the SPEC is about incremental
  indexing (AC6.2), so it shows no per-item size.
  (b) Its project scope shows "only what belongs to that project" (AC2.2). It does not show the
  merged set the agent actually loads in a directory, and it gets its project list from
  `~/Projects/INDEX.md`.
  (c) It is a macOS GUI only, with no Linux, CLI or `--json`.
  (d) It does not check its view against the agent's real request.
  https://github.com/migsilva89/loadout/blob/main/docs/SPEC.md
- One more name clash: the TUI simonstrumse/claude-code-manager runs as the command `ccmanager`. That
  is the same name as kbwo/ccmanager. Names in this space are crowded, so check the name before the PRD
  fixes it. https://github.com/simonstrumse/claude-code-manager

## A. Tools that inspect or trim the agent harness today

Legend: **Size** = shows bytes/tokens per item. **Origin** = shows which file, scope or plugin
produced the item. **Disable** = can turn the item off. **cwd** = shows the set for one working
directory.

| Tool | ★ / last push | Size | Origin | Disable | cwd | URL |
|---|---|---|---|---|---|---|
| Claude Code `/context [all]` | built-in | Y (tokens per category; `all` expands per item) | partial (category + file for memory) | N (suggestions only) | Y, live session only | https://code.claude.com/docs/en/commands |
| Claude Code `/skills` + `/skill-doctor` | built-in | Y (`t` sorts by token count; skill-doctor = cost + use count) | Y (filter by "source") | Y (Space cycles visibility) | Y, live | https://code.claude.com/docs/en/commands |
| Claude Code `/mcp disable`, `/plugin disable` | built-in | N | partial | Y | Y, live | https://code.claude.com/docs/en/commands |
| `codex debug prompt-input` | built-in | N (raw JSON; you count it) | N `[미확인]` | N | Y | https://learn.chatgpt.com/docs/developer-commands?surface=cli |
| migsilva89/loadout | 98 / 2026-10-01 | N | Y ("each marked with its owner") | Y (move to `skills-off/`) | partial | https://github.com/migsilva89/loadout |
| ringo380/ccmcp (Go) | 0 / 2026-10-11 | N (its doctor lints token-budget limits) | Y (badges `u l p P @ s`) | Y (`disabledMcpServers`, stash) | Y ("effective" view is the default) | https://github.com/ringo380/ccmcp |
| henkisdabro/Claude-Code-MCP-Server-Selector (TS) | 8 / 2026-09-19 | estimate per server (`mcp --context-report`) | Y ("12+ configuration sources") | Y (3-way toggle) | Y | https://github.com/henkisdabro/Claude-Code-MCP-Server-Selector |
| spences10/mcpick (TS) | 95 / 2026-10-11 | N `[미확인]` | scope | Y (many clients, profiles, backups) | Y | https://github.com/spences10/mcpick |
| Santhosraj/Claude_Linter = `cclint` (TS) | 1 / 2026-10-05 | Y (exact via count_tokens, else `~` estimate) | Y (layers, `explain`) | N | Y | https://github.com/Santhosraj/Claude_Linter |
| chrysa/claude-lint (renamed prism-ai-lint) | 0 / 2026-10-11 | estimate (bytes/4, TOKENS block, top-5; from a search snippet only, README not read `[미확인]`) | `[미확인]` | N | Y | https://github.com/chrysa/prism-ai-lint |
| iwadjp/agent-rules-linter | 0 / 2026-10-06 | estimate (chars/4) | N | N | Y | https://github.com/iwadjp/agent-rules-linter |
| YawLabs/ctxlint | 11 / 2026-10-09 | `[미확인]` | partial | N | Y | https://github.com/YawLabs/ctxlint |
| simonstrumse/claude-code-manager (Py TUI) | 19 / 2026-01-08 | N | Y (per project, 6 tabs) | N | scans many projects | https://github.com/simonstrumse/claude-code-manager |
| joeyism/claude-code-config (Py TUI) | 22 / 2026-01-04 | N | `~/.claude.json` only | edit | N | https://github.com/joeyism/claude-code-config |
| intellectronica/ruler (TS) | 2943 / 2026-09-30 | N | generator, not inspector | per agent: `[agents.x] enabled=false` | Y | https://github.com/intellectronica/ruler |
| agentsmd/agents.md | 24887 / 2026-09-10 | — (a file format) | — | — | — | https://github.com/agentsmd/agents.md |
| dotagents (iannuttall 710★, getsentry 252★) | 2026-02-04 / 2026-10-10 | N | symlink farm in `~/.agents` | add/remove | Y | https://github.com/iannuttall/dotagents |
| ccusage (now mostly Rust; shipped through npx) | 18986 / 2026-10-13 | per session/day, after the fact | per model/agent | N | per project | https://github.com/ccusage/ccusage |
| davila7/claude-code-templates | 32591 / 2026-10-13 | N | N | N (`--health-check` validates only) | Y | https://github.com/davila7/claude-code-templates |
| kbwo/ccmanager | 1261 / 2026-10-12 | N | N | N (session/worktree manager) | — | https://github.com/kbwo/ccmanager |
| superbasicstudio/claude-conductor | 380 / 2026-05-17 | N | N | N (generates more docs) | — | https://github.com/superbasicstudio/claude-conductor |
| aider `--show-repo-map`, `--map-tokens` (default 1024) | 49507 / 2026-05-22 | budget knob | N | budget knob | Y | https://github.com/Aider-AI/aider/blob/main/aider/website/docs/repomap.md |
| Cline context-window bar | — | total only | N | N | per task | https://cline.bot/blog/understanding-the-new-context-window-progress-bar-in-cline |
| ZanzyTHEbar/cursor-rules (Go) | 3 / 2026-04-13 | N | preset packages | install only | Y | https://github.com/ZanzyTHEbar/cursor-rules |

### Closest relatives: what to take, what to avoid

- **`/context [all]`** shows the categories System prompt / System tools / MCP tools / Custom agents /
  Memory files / Skills / Messages / Autocompact buffer, each as tokens and % of the window.
  - Take: these category names, and showing tokens next to %.
  - Avoid: being usable only inside a running session, and having no disable action. Issue #23228
    shows 28 MCP tools at "0 tokens" each. `[추정]` The likely reason is that the schemas are deferred
    through ToolSearch and so cost 0 at that point (claudelog/wmedia describe this deferral; the
    issue does not state the cause). A static sum of tool schemas would therefore *overstate* what
    reaches the model. Our tree must mark *deferred* items separately from *loaded* items.
  - https://github.com/anthropics/claude-code/issues/23228 ,
    https://github.com/lilhawk7077/claude-code-context-overhead (a real `/context all` breakdown:
    39.6k tokens before the first prompt)
- **`/skills` and `/skill-doctor`.** Claude Code already sorts skills by token count and pairs cost
  with how often each skill is used.
  - Take: a "cost × use" column. Our tree should be able to read usage too, from session JSONL, as
    migsilva89/loadout and `/skill-doctor` do.
- **ccmcp**
  - Take: the *effective view as the default screen*. Take the one-letter origin badges, and the
    tri-state marks `[x]` load / `[~]` disabled-here (undoable) / `[ ]` absent / `[?]` stale
    override with no live source (it offers `prune`). It writes the official knob
    (`disabledMcpServers`) for this project only.
  - Avoid: covering MCP and plugins only, with no sizes.
- **cclint**
  - Take: its three load classes. **always** (CLAUDE.md and its `@imports`). **advertised** (skills
    and agents, name + description only). **on demand** (bodies, nested CLAUDE.md). Only the first
    two count toward the per-turn floor.
  - Take: exact counts via count_tokens when a key is present, otherwise `~` plus the word
    "estimated". It ships no GPT tokenizer ("it's the wrong BPE for Claude").
  - Take: *differential conformance tests*. It checks itself against the real binary through
    `claude doctor`, `claude mcp list`, hook firing and `claude --debug`, without making any API
    call.
  - It also found that Claude Code keys trust on the **git root**, not on the `.claude` directory.
    This is the kind of trap loadout's resolver also faces.
- **MCP Server Selector**
  - Take: atomic writes, automatic backups, file locking, explicit consent for global changes, and a
    warning when changes only apply after a session restart.
  - Avoid: hard-coded "~600–800 tokens per tool" style claims in the UI. They are not measured.
- **ruler**
  - Take: `.bak` before overwrite, `ruler revert [--dry-run] [--keep-backups]`, and a managed
    `.gitignore` block between start and end markers.
  - Avoid: generating more files (it is the opposite direction from trimming).
- **ccusage**
  - Take: local-only, `--json` on every report, statusline mode, and a Rust core shipped through
    `npx`/`bunx`.
  - Avoid: it is after-the-fact usage, not what the harness costs.

### The MCP-bloat discussion (why this product should exist)

Users report MCP tools at 98.7k tokens (49.3% of 200k) (#13717), and more than 25k tokens before
the first message (#23787). A `--no-mcp` flag was requested (#20873). Until then the workaround is
`claude --strict-mcp-config --mcp-config '{}'`. One bug: `--strict-mcp-config` does not override
`disabledMcpServers` (#14490). Another: schemas are still counted after every connector was
disabled (#92255). Tool Search deferral has since changed the math.

The lesson for loadout: the cost of each item depends on the **mode** (eager vs. deferred), so
showing a size without the mode misleads.

https://github.com/anthropics/claude-code/issues/13717 ·
https://github.com/anthropics/claude-code/issues/23787 ·
https://github.com/anthropics/claude-code/issues/20873 ·
https://github.com/anthropics/claude-code/issues/14490 ·
https://github.com/anthropics/claude-code/issues/92255

## B. TUI patterns for a size-sorted tree with actions

| Tool | Stack | ★ | Conventions worth copying | URL |
|---|---|---|---|---|
| ncdu | C/Zig `[미확인]` | — | Sort keys `s`/`n`/`C`/`M` (press again to reverse); `--sort` default = `disk-usage`; `--graph-style` hash/half-block/eighth-block; `--show-percent`; `g` toggles the graph; `d` deletes, and `--confirm-delete` is on by default; `-r` read-only; `-o`/`-f` JSON export/import | https://dev.yorhel.nl/ncdu/man |
| gdu | Go | 6102 | `d` delete, `e` empty, `n`/`s` sort, `?` help, `--no-delete`, `-n` non-interactive, `-o`/`-f` JSON, `--no-unicode` | https://github.com/dundee/gdu |
| dust | Rust; prints once, **not** a TUI | 12494 | The grey "shadow" bar shows that a child belongs to a parent's share (README l.23–25, 101); `-b` no bars, `-B` bars on the right, `-R` screen-reader mode, `-j` JSON | https://github.com/bootandy/dust |
| broot | Rust, termimad + crokey (not ratatui) | 13065 | `-w`/`--whale-spotting`; typing filters but keeps the hierarchy; `?` help; preview pane (Ctrl+→); `:rm` verb | https://dystroy.org/broot/ |
| nix-tree | Haskell | 1082 | **closure size vs added (unique) size**; `w` why-depends modal; `/` search; `s` sort; `?` help; 3 panes | https://github.com/utdemir/nix-tree |
| cargo tree | Rust (cargo) | — | `--invert` = "Show the reverse dependencies"; `(*)` = already shown (dedupe), `--no-dedupe` | https://doc.rust-lang.org/cargo/commands/cargo-tree.html |
| lazygit | Go | 83129 | `?` opens the keybindings menu; bottom line shows the keys for the current context; `confirmOnQuit` | https://github.com/jesseduffield/lazygit/blob/master/docs/Config.md |
| gitui | Rust, **ratatui 0.30 + crossterm 0.29** | 22562 | the main proof that a ratatui git TUI works at this scale (keys not verified here) | https://github.com/gitui-org/gitui |
| k9s | Go | 34797 | `:` command mode, `/` regex filter, `?` help, `esc` back; `ctrl-d` deletes **with** a confirm dialog, `ctrl-k` without; **`--readonly`** | https://k9scli.io/topics/commands/ |
| yazi | Rust, forked ratatui-core/widgets | 42792 | **`d` = move to Trash (undoable) vs `D` = permanent delete**; `?`/`~` help; `f` filter; `,s` sort by size | https://yazi-rs.github.io/docs/quick-start |
| bottom | Rust, ratatui 0.30 + crossterm 0.29 | 14096 | ratatui reference for gauges and bars | https://github.com/ClementTsang/bottom |
| tig / btop | C / C++ | 13363 / 34950 | not studied here (no behaviour verified) | https://github.com/jonas/tig · https://github.com/aristocratos/btop |
| tui-rs-tree-widget | Rust, ratatui | 129 | a ready-made ratatui tree widget (expand/collapse) | https://github.com/EdJoPaTo/tui-rs-tree-widget |

**House stack, checked:** llmux `Cargo.toml` pins `ratatui = "0.30.1"` and `crossterm = { version
= "0.29.0", features = ["event-stream"] }`, so ratatui + crossterm is confirmed for llmux.
**xfx does not use ratatui.** Its `Cargo.toml` depends on `rustix` (`pty`, `termios`) + `libc`, so
its terminal layer is written by hand. Treat "house stack = ratatui + crossterm" as the llmux
precedent, not as the convention for both repos. https://github.com/2lab-ai/llmux ·
https://github.com/2lab-ai/xfx

**Conventions to adopt** (each one appears in at least two tools above):
1. Sort by size descending by default (ncdu `--sort disk-usage`, dust, broot `-w`). Pressing the
   same sort key again reverses it.
2. On each row: size, then % of the parent, then a bar in eighth-blocks (ncdu
   `--graph-style eighth-block`), with an ASCII fallback (`--no-unicode` like gdu, `-R` like dust).
   A dust-style shadow shows the parent's share.
3. Two sizes for every subtree, nix-tree style. **Total** = everything under this node. **Unique**
   = what disappears if you disable this node. For a plugin: its skills, agents, MCP tools and
   hooks, minus anything also supplied elsewhere.
4. A "why is this here" pane, like nix-tree `w` and `cargo tree --invert`. It shows the chain:
   setting file → plugin → skill → file, with `(*)` dedupe markers for items reached twice.
5. Keys: `j/k/h/l` + arrows, `/` filter (keep the hierarchy, like broot), `?` overlay listing all
   keys, a context key bar at the bottom (lazygit), `esc` back, `:` command mode (k9s).
6. Two-step destructive keys, like yazi `d` vs `D`. **`space`/`d` = disable (reversible).
   `D` = remove (moved to Trash; you must type the item name to confirm).** Confirmation is on by
   default, as in ncdu.
7. `--readonly` (k9s, ncdu `-r`, gdu `--no-delete`) and a non-interactive `--json` (gdu `-n`,
   dust `-j`, ncdu `-o`) that print the same model the TUI shows.
- Avoid: treemaps (imsnif/diskonaut, last push 2024-03-07) are hard to read for text items with
  names. Also avoid `ctrl-k` (delete without confirmation): there should be no such path.

## C. Disabling without destroying: conventions

| Mechanism | Example (primary source) | Note for loadout |
|---|---|---|
| Official enable flag | Claude Code `enabledPlugins` ("Turn individual plugins on or off per scope"), `disabledMcpjsonServers`, `skillOverrides` ("Hide or collapse a skill without editing its SKILL.md"), `claudeMdExcludes`, `disableBundledSkills`, `disableAllHooks`. Source: https://code.claude.com/docs/en/settings-reference . Codex: `mcp_servers.<id>.enabled`, `mcp_servers.<id>.enabled_tools` / `disabled_tools`, `plugins.<plugin>.enabled`, `skills.config` (the key exists, but what it controls is `[미확인]`). Source: https://learn.chatgpt.com/docs/config-file/config-reference | Best option: the agent itself honours it, and other tools understand it |
| Interactive toggles that write the flag | `/mcp disable <server>` and `/plugin disable`. The choice is "saved per project in ~/.claude.json" (https://loadout.migsilva.dev/guides/disable-mcp-server-claude-code/) | If we write the same knob, our state and the agent's own UI stay in step |
| Move to a quarantine folder | migsilva89/loadout moves a skill to `skills-off/` next to its owner. The owner and loaders are recorded in `skills-off.json`, and a round trip leaves the disk "exactly as it was, symlinks included" (AC3.1/3.3/3.7) | Fallback when no flag exists (e.g. user CLAUDE.md, rules files) |
| Parking a config entry | ccmcp's stash `~/.claude-mcp-stash.json`, with `mcp stash`/`restore` | Same idea for JSON entries |
| Mask | systemd `mask` links the unit to `/dev/null`, "a stronger version of disable". https://www.freedesktop.org/software/systemd/man/latest/systemctl.html | Do not use: a symlink pointing at nothing hides intent |
| Symlink farm | GNU Stow (https://www.gnu.org/software/stow/manual/stow.html); dotagents `~/.agents` | Resolve realpath. Disabling one link must never delete the shared copy (migsilva89 already enforces this) |
| Trash with restore | trash-cli `trash-put`/`trash-list`/`trash-restore` (https://github.com/andreafrancia/trash-cli), following the FreeDesktop Trash spec (https://specifications.freedesktop.org/trash/latest/). macOS `/usr/bin/trash`: its `man trash` page says "First appeared in macOS 15.0" (checked locally on 26.6.2) | Use for `D` (remove), never `rm` |
| Plan then apply | `terraform plan -out=FILE` writes a saved plan that `terraform apply` executes (https://developer.hashicorp.com/terraform/cli/commands/plan); chezmoi `diff` prints the difference between target and destination (https://github.com/twpayne/chezmoi); `git add -p` (https://git-scm.com/docs/git-add) | Every batch of changes is shown as a diff before it is written |
| Generations and rollback | `nix-env --rollback` (https://nix.dev/manual/nix/latest/command-ref/nix-env/rollback) | `loadout undo` / `loadout history` |
| Backups and revert | ruler `.bak` + `ruler revert` (https://github.com/intellectronica/ruler); migsilva89 snapshots to `backups/<ISO-8601>/` before every write and refuses the write if the backup fails | A snapshot is required before every write |

**Recommended model: a journal of reversible operations, plan before apply.**
1. Every action produces an `Op {target, mechanism, before_hash, after_hash, scope}`. Mechanism
   priority:
   (a) the agent's official knob in the narrowest scope that works. Prefer project-local
   (`.claude/settings.local.json`, `~/.claude.json#projects[cwd]`, project `.codex/config.toml`)
   over user scope, so that we never edit a git-tracked team file without saying so.
   (b) If there is no knob, move the item to a quarantine folder next to its owner
   (`<root>/.loadout-off/…`), with a manifest entry.
   (c) Never `rm`. `D` goes to the OS Trash.
2. Ops are staged in the TUI and shown as a unified diff with the token delta (terraform-plan
   style). `a` applies them atomically: write to a temp file, fsync, rename, and lock against a
   running agent.
3. Before each write, take a snapshot into `$XDG_STATE_HOME/loadout/journal/<ts>/`. On macOS use
   `~/Library/Application Support/loadout/`. Never write inside `~/.claude`; migsilva89 gives the
   reason: whoever wipes `.claude` loses the app's data.
4. `loadout undo [N]` and `loadout history` replay the journal backwards. A *stale* op (the target
   changed since we wrote it, so the hash differs) is reported, not forced. This is ccmcp's `[?]`.
5. If the file is under git, show "tracked by git: this edit will show in `git status`".
   migsilva89 says this once for project skills (AC3.12).

## D. Measuring what the model actually sees, instead of guessing

Static resolution, which is our core, will drift from the agents. The agents change their loading
rules (Tool Search changed how MCP is counted), so we need an **oracle**.

Codex has one built in: `codex debug prompt-input` "Render[s] the exact model-visible prompt input
list as JSON" (https://learn.chatgpt.com/docs/developer-commands?surface=cli).

For Claude Code, the real request has to be captured. Options:
- a reverse proxy through `ANTHROPIC_BASE_URL`, the variable Claude Code uses to reach a gateway
  (https://code.claude.com/docs/en/llm-gateway);
- llmux's raw request viewer, which records the client request, the rewritten upstream request and
  the verbatim response, with copy-as-curl redacted (https://github.com/2lab-ai/llmux);
- liaohch3/claude-tap (3278★, last push 2026-10-10). It is a reverse proxy for Claude Code/Codex and
  a forward proxy for others, and its viewer shows a "structural diff" between consecutive requests
  (https://github.com/liaohch3/claude-tap);
- mitmproxy (https://docs.mitmproxy.org/stable/).
- badlogic's claude-trace monkeypatches `fetch` inside Claude Code. It is **stale**: lemmy's last
  push was 2025-08-13 (https://github.com/badlogic/lemmy/tree/main/apps/claude-trace).

For counting:
- **Anthropic count_tokens** (https://platform.claude.com/docs/en/build-with-claude/token-counting)
  is free but rate-limited. The docs call the result "an **estimate**". It rejects MCP-connector and
  server tools, and Opus 4.7+ / Fable / Mythos use a tokenizer that produces "approximately 30
  percent more tokens". So any offline ratio must be tied to a model.
- For Codex/OpenAI models, `tiktoken-rs` (409★; https://github.com/zurawiki/tiktoken-rs) gives local
  BPE counts. HF `tokenizers` (11167★; https://github.com/huggingface/tokenizers) is the general
  Rust option. No official Claude tokenizer is published `[미확인]`.

**The honesty rule, borrowed from cclint:**
- Every number carries a provenance tag: `bytes` (exact), `~tok est` (offline ratio, with the model
  named), `tok api` (count_tokens) or `tok wire` (seen in a captured request).
- The header always says which tag the total uses.
- `loadout verify` runs the agent once behind the capture proxy, diffs the captured request against
  the static tree, and lists every item that was predicted but not seen, or seen but not predicted.

## E. Distribution for a Rust system CLI (2026)

- **GitHub Releases + a Homebrew tap, the llmux way.** Pushing a `v*` tag starts a build matrix for
  `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and
  `aarch64-unknown-linux-gnu`. The arm64 Linux build uses the native runner `ubuntu-24.04-arm`. The
  workflow checks that the tag matches `Cargo.toml`, uploads bare binaries plus `SHA256SUMS`, and
  `2lab-ai/homebrew-tap/Formula/llmux.rb` picks a URL per OS/arch with `on_macos`/`on_linux` and
  `on_arm`/`on_intel`.
  https://github.com/2lab-ai/llmux/blob/main/.github/workflows/release.yml ·
  https://github.com/2lab-ai/homebrew-tap
- **cargo-dist** (2133★) generates the release CI, tarballs, checksums, `curl | sh` installers and a
  Homebrew formula published to `<org>/homebrew-tap`. Its docs explain why: "the core Homebrew tap
  does not accept prebuilt binaries from third parties."
  https://axodotdev.github.io/cargo-dist/book/installers/homebrew.html
- **`cargo install`** and **cargo-binstall** (2921★), which fetches release binaries:
  https://github.com/cargo-bins/cargo-binstall
- **AUR, for the user's CachyOS machine.** The AUR rule: "Packages that use prebuilt deliverables,
  when the sources are available, must use the `-bin` suffix"
  (https://wiki.archlinux.org/title/AUR_submission_guidelines). The usual setup is two packages.
  - `loadout-bin` repackages the release tarball for x86_64 and aarch64, plus completions and the
    licence.
  - `harn` (or `loadout-git`) builds from source, following the Rust guidelines:
    `cargo fetch --locked` in `prepare()` and `cargo build --frozen --release` in `build()`
    (https://manual.archlinux.page/package-guidelines/rust/).
  - Ship the release as a `.tar.gz` holding the binary, `completions/` and the man page, not as a
    bare binary, so that `-bin` packages and formulas install them without running the binary. This
    is a deliberate difference from llmux's bare-binary assets.
- **Completions and man page:** clap + `clap_complete` (https://docs.rs/clap_complete/latest/clap_complete/),
  exposed as `loadout completions <shell>`. ccmcp ships the same command, including dynamic
  completion of live names.
- **`--json`** on every read command: ccusage, mcpick, ccmcp `status --json` and `codex mcp list --json`
  all do this. Make it a versioned schema (`"schema": "loadout/1"`), so that scripts and
  `loadout verify` share one model.

## UX recommendation (the single best interaction model)

1. Run `loadout [dir]` and get a **read-only effective view** by default (as ccmcp does). Show one
   tree per agent (Claude Code | Codex as tabs), sorted by size descending (ncdu).
2. Top-level nodes are the load classes **always / advertised / deferred / on-demand** (cclint).
   Under them come origins: managed / user / project / local / plugin@marketplace / MCP server.
3. Each row shows: name, origin badge (`u p l P M @`), size, % of total, an eighth-block bar, and
   `~` or `api` or `wire` provenance.
4. The header shows the total, the share of the model's window, and the provenance of the total. It
   also says "static estimate, not yet verified" until `verify` has run.
5. Enter opens a **detail pane**. It shows the resolved real path, the scope, and a "why loaded"
   chain (nix-tree `w`, cargo `--invert`). It also shows *unique* tokens, meaning what disabling
   this would save.
6. `space` stages a disable using the official knob. Fallback order: knob, then quarantine, then
   never rm. `D` stages a remove into the Trash; you type the item name to confirm it.
7. Staged ops show in a bottom diff panel with the token delta. `a` applies them atomically after a
   snapshot. `u` / `loadout undo` reverts them. `--readonly` turns all of this off.
8. If the target file is tracked by git or shared with a team, show a warning before apply. If an
   agent session is running, warn that the change takes effect on the next start.
9. `loadout verify` captures one real request through a local proxy, or through
   `codex debug prompt-input`. Afterwards each row shows ✓ (seen on the wire), ✗ (predicted, not
   seen) or ? (seen, not predicted).
10. `?` opens an overlay with every key, with a context key bar at the bottom (lazygit). `/`
    filters while keeping the hierarchy (broot).
11. `loadout --json` and `loadout plan|apply|undo|history` give the same model without the TUI.

Strongest references: **ncdu** (size-sorted tree, `d` with confirmation by default, `-r`,
JSON snapshots); **ccmcp** (effective view as default, origin badges, per-project undoable
override, stale detection); **cclint** (load-class taxonomy, `~` estimated vs exact, checking
itself against the real binary).
