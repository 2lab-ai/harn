# 04 — The interactive view: information design

Status: shipped — 2026-10-13 (A16 read green by the dispatcher on the installed binary, preview-2026-10-13-1426-e05077fd3773; rounds and receipts in `p0-claude-code/loop.done.md` R6). Written after the user read the first TUI (frames in zbrain
`workflow/2lab-ai/harn/receipts-2026-10-13/tui-80x24/`) and called it unreadable. He was right:
that screen was the CLI table, the `why` text and the `apply` dump placed inside ratatui boxes.
This document is the screen, not the data model (the model stays `01-spec.md` §The model).

## The one question the screen answers

"What does Claude Code load here before my first message, what is big, what can I turn off, and what
happens if I do?" Everything on screen serves that sentence. Raw ids, absolute paths, hashes and
internal tags (`+Δ?`, `~`, `mode always`, `knob`) never appear in the list or the cards; they exist
only in `--table`, `--json` and `harn why` for the AI operator.

## 80×24 — the first screen (every character below is the contract; the frame is 80 wide)

```
 zbrain · Claude Code 2.1.296 · 345 items load before your first message   ~150 KB
 biggest: skill listing 87 KB (58%) · plugins 48 KB · memory 25 KB · project 22 KB   ! 2 notes
──────────────────────────────────────────────────────────────────────────────
 ▸ Plugins (9)                                         48 KB ████████  32%
 ▸ Skill listing  216 entries · 144 descriptions cut   87 KB ██████████████ 58%
 ▸ Memory (1)                                          25 KB ████       17%
 ▸ Project files (3)                                   22 KB ████       15%
 ▸ Built-in (7)  cannot be turned off                     ?
   
   
   
   
   
   
   
   
   
   
   
──────────────────────────────────────────────────────────────────────────────
 Plugins · 9 enabled for this directory
 Turn one off and its skills, commands, agents and hooks leave the next session.
 Open the group (→) to see each plugin with its size.
──────────────────────────────────────────────────────────────────────────────
 ↑↓ move   →← open/close   / search   d turn off   u undo   ? keys   q quit
```

Rules that produce it:

- **Header = 2 lines.** Line 1: directory name (not the path), agent + version, the item count in a
  sentence, the predicted total (`~` only when some rows are unknown). Line 2: the four biggest
  groups with size and share, then `! N notes` when the resolver has warnings or the tool-search
  state matters; `i` opens the notes card. No warning text in the header, ever.
- **The list is grouped by kind, groups folded at start.** Order: Plugins, Skill listing, Memory,
  Project files, Hooks (if any outside plugins), MCP servers (if any outside plugins), Built-in
  (always last, greyed, no `d`). A group row shows: marker, human name, count (and the one fact
  that matters — for the skill listing, how many descriptions the cap cuts), size, a bar, share.
  Share = of the predicted total. Bars are relative to the largest line in the list (ncdu: the
  listing, not the viewport, so bars do not rescale while scrolling), drawn in eighth-blocks, `#`
  under `--no-unicode`.
- **One row per thing.** A plugin is one row; its MCP server, hook, skills, commands and agents are
  its children, each with its own size (the skill's listing bytes, the hook's stdout bytes or `?`,
  the MCP server's schema bytes or `?`). The old screen showed `plugin:slack:slack` and
  `slack@claude-plugins-official` as siblings and copied the plugin's number onto every child; both
  are forbidden. Children never inherit a parent's number.
- **Human names.** `zworkflow` not `zworkflow@soma-work` (the marketplace goes in the source column
  when there is room, and always in the detail panel); `MEMORY.md` with `(auto-memory)`; `CLAUDE.md
  + 2 imports`; `SessionStart hook` under its plugin; `~/.claude/CLAUDE.md` (home-relative, never
  `/Users/...`). A name that still does not fit is cut on the left with the tail kept.
- **The numbers column is one number: bytes in the first request** (predicted, or observed when a
  capture is bound — then the header says `measured` instead of `~`). `freed` is not a column: what
  turning a row off saves is a sentence in the detail panel.
- **Detail panel = 3–4 lines, always present, for the cursor row**, in sentences: what it is and
  where it is enabled (scope, in words: "for every project" / "for this project only" / "by this
  repository"), what it brings (counts), what `d` would do and what it saves, and whether the skill
  listing would also shrink ("…and the skill listing shrinks; how much needs `harn verify --run`").
  Built-in rows say why they cannot be turned off. Group rows describe the group.
- **Key bar in words**, only the keys valid now. `d turn off` (not `off`), `u undo`, `→← open/close`.
  Under `--readonly` d, o, u and e are absent (the `?` card still lists them, with the line
  `read-only: d, o, u and e are off`; the header says `read-only`; the detail says what `d`
  would do — "d would turn it off …" — and promises no `u`); the Built-in group row has no `d`.
- **Footer honesty**: `345 items · 5 shown · filter: none` only when a filter or fold hides rows
  that would otherwise be visible; otherwise no footer (the panel is the footer).
- **A search shows each thing once**: while a filter is on, a plugin's entries that a filtered
  listing already shows are not repeated under its row in Plugins — one note per listing the
  entries are in stands in their place (`8 matching skills — listed above under Skill listing ›
  slack`), acts on nothing (d and w say where to pick one, read-only too), and counts as one line
  in the footer, while the plugin row keeps its d; when the search is cleared or emptied, a cursor
  on a note goes to its plugin row.

## Rules fixed after reading the first rebuild's frames (2026-10-13 19:1x)

- **Shares partition at the top level; plugin rows show what leaving frees.** The sized groups
  (Skill listing, Project files, Memory, Agent listing, measured Hooks/MCP) carry own bytes, bars
  and shares that sum to ≤ 100%. The Plugins group line carries no bar and no share — it names the
  overlap: `48 KB would leave with them · counted in the listings above`. Each plugin row shows
  what turning it off frees (its own bytes plus its listing entries) with a bar and the share of
  the total it would free, because that is the number a person acts on; its detail sentence gives
  both parts: "frees 3.7 KB: 3.7 KB of its entries in the skill listing, plus its own unmeasured
  part" (nix-tree's two sizes). Round 2 had hidden every plugin number behind `in listing`; that
  was worse than the double count it removed.
- **Groups are ordered by bytes, descending**; groups without a size follow; Built-in is last.
- **An unknown size reads `unmeasured`**, never a bare `?`; the notes card says how to measure.
- **Counting.** "N items load before your first message" counts items that are always loaded or
  advertised (312 on zbrain); runtime / not-loaded items (33) are listed in their groups with the
  words `runs later` / `not loaded` / `when used` instead of a size. A hook that is off but still
  listed says why instead: `off, with every other hook` (`disableAllHooks`) or `off by policy`.
  `off by policy` is a managed `allowManagedHooksOnly`, under which only managed hooks and the
  hooks of a plugin managed settings force-enable (its full `name@marketplace` id) run, and the
  status line is read from managed settings only
  ([settings-reference](https://code.claude.com/docs/en/settings-reference), "What runs under
  allowManagedHooksOnly"). Such a row has no `d` or `o`, since no setting of the user's overrides
  that policy; its detail says the organisation allows only its own hooks, a group whose every
  row is off by policy says `off by policy` too, and its plugin's detail does not count it among
  what the plugin brings ("Its hook is off by policy."). The status line has no row here; it is
  off by policy the same way in the table and `harn why`. When `o` or `u` restores a change of
  harn's and what it turned off still does not run (a policy arrived since), the message says the
  file is restored and what stays off ("3 hooks stay off by policy"), never "back on"; `harn on`
  adds the same as a line after the journal line. "Back on" is said only once the refreshed tree
  shows it runs: when that check cannot run (the refresh fails), the message says the file is
  restored and that whether it runs could not be checked.
- **Agent listing** is a group of its own (31 entries, 15 KB on zbrain); the catch-all `Other`
  exists but is empty on known trees; status-line and permission rules are never shown (they are
  never in the request).
- **Built-in** rows that have a switch (gitStatus → `includeGitInstructions`, attribution,
  built-in tools → `permissions.deny`) keep `d`; the group line says `mostly cannot be turned off`.
- **A message line** (refusal, `undone: …`) takes the panel's first line and drops whole sentences
  from the end of the detail; a sentence is never cut with `…`.
- **80-column adaptations** of the mockups: the version drops from the title (shown at 120), the
  `biggest:` line keeps what fits, the key bar tightens gaps before dropping keys, the plugin
  "what" line drops the version, the footer rule appears only when lines are hidden and never
  says `filter: none`.
- **Readonly**: write keys are greyed (attributes do not survive `capture-pane -p`); pressing one
  shows `readonly: d would change files; harn was started with --readonly` on the message line.

## 80×24 — group opened, cursor on a plugin

```
 zbrain · Claude Code 2.1.296 · 345 items load before your first message   ~150 KB
 biggest: skill listing 87 KB (58%) · plugins 48 KB · memory 25 KB · project 22 KB   ! 2 notes
──────────────────────────────────────────────────────────────────────────────
 ▾ Plugins (9)                                         48 KB ████████  32%
   ▸ zworkflow        soma-work                        32 KB ████████  21%
   ▸ slack            official                        3.7 KB █          2%
   ▸ atlassian        official                        3.2 KB █          2%
   ▸ superpowers      official                        2.9 KB █          2%
   ▸ stv              oh-my-claude                    4.3 KB █          3%
     cowork-plugin-management  claude.ai             0.7 KB            0%
     typesafe         typesafe-ai                     0.7 KB            0%
     frontend-design  official                        0.2 KB            0%
     context7         official                           —   not loaded
 ▸ Skill listing  216 entries · 144 descriptions cut   87 KB ██████████████ 58%
 ▸ Memory (1)                                          25 KB ████       17%
 ▸ Project files (3)                                   22 KB ████       15%
 ▸ Built-in (7)  cannot be turned off                     ?
   
──────────────────────────────────────────────────────────────────────────────
 slack · plugin from the official marketplace, v1.4.1 · enabled for every project
 Brings 8 skills, 5 commands and 1 MCP server here: 3.7 KB of your first request.
 d turns it off for this project only (.claude/settings.local.json); the skill
 listing shrinks too — how much needs `harn verify --run`. Reversible with u.
──────────────────────────────────────────────────────────────────────────────
 ↑↓ move   →← open/close   / search   d turn off   w where from   ? keys   q quit
```

## 80×24 — `d` on slack: one card, Enter applies

```
┌ Turn off slack for this project ─────────────────────────────────────────────┐
│ Writes   .claude/settings.local.json → enabledPlugins.slack = false          │
│ Leaves   15 at start (slack, 8 skills, 5 commands, 1 MCP server)      → list │
│ Saves    3.7 KB now; the skill listing shrinks too (unmeasured)              │
│ Undo     u here, or `harn undo` later — the file is restored byte for byte   │
│                                                        Enter apply · Esc    │
└──────────────────────────────────────────────────────────────────────────────┘
```

One card replaces the old `d` → plan pane → `a` → `y/N` chain: `d` opens the card, Enter applies,
Esc cancels; `→` on the "Leaves" line opens the list of what leaves (scrollable, Esc back). After
Enter the message row says how the header's count moved (`turned off alpha · 6 fewer items load at
start (83 → 77)`), the detail panel shows `turned off · u undoes · takes effect next session` and
the row shows `off` in the state column (greyed name). `o` on an off row = the same card titled
"Turn … back on"; `u` = the same card titled "Undo the last change" (Enter undo · Esc keep; `y` and
`n` work too).

`→` list: every item that stops loading, under `at start (N)` — N is exactly what the header's
count drops by after Enter — then `when used / later (M)` (hooks that run later, rules read on
demand, the status line); what a change makes appear comes last under `appears (K)`. The Leaves
line summarises the two groups (`6 at start (alpha, 2 skills, 1 MCP server, …), 1 later (hook)`);
its `→ list` hint takes a line of its own when the summary fills the first one. The Saves line
carries no count.

```
┌ What leaves with slack ──────────────────────────────────────────────────────┐
│ at start (15)                                                                │
│   slack (plugin)                                                             │
│   slack-search (skill)                                                       │
│   …                                                                          │
│   slack (MCP server)                                                         │
│                                                                     Esc back │
└──────────────────────────────────────────────────────────────────────────────┘
```

Action cards (`d`, `o`, `u`) show their whole body before Enter (or `y`) can do anything, because
Enter commits a write; in a smaller frame the card is not drawn, the message row
says the size it needs, and only Esc works. Informational cards (`w`, `?`, `i`) have nothing to
commit and scroll instead — ↑↓ moves the body, Esc closes — and are drawn whenever the frame is
at least 40×10.

## `w` — where it comes from (card, not a path wall)

```
┌ Where slack comes from ──────────────────────────────────────────────────────┐
│ 1  ~/.claude/settings.json › enabledPlugins › slack = true   (every project) │
│ 2  installed from claude-plugins-official, version 1.4.1                     │
│ 3  files: ~/.claude/plugins/cache/claude-plugins-official/slack/1.4.1        │
│ 4  contributes 8 skills · 5 commands · 1 MCP server                          │
│ Switches  this project (.claude/settings.local.json) · you (~/.claude/…)     │
│                                                              Esc close      │
└──────────────────────────────────────────────────────────────────────────────┘
```

Paths are home-relative, wrapped on `/` boundaries, never mid-word; no sha256; the full id is the
last line only under `--table`-style verbosity (`harn why` keeps it for the AI).

## `?` keys — two aligned columns, no mid-word wraps

```
┌ Keys ────────────────────────────────────────────────────────────────────────┐
│ ↑ ↓   move            →  ←   open / close group        /   search (Esc clears)│
│ g G   top / bottom    s      sort: size · name          i   notes             │
│ d     turn off        o      turn back on               u   undo last change  │
│ w     where from      e      edit the file ($EDITOR)    t   let the AI trim   │
│ ?     this card       q Esc  quit / close               r   refresh           │
└──────────────────────────────────────────────────────────────────────────────┘
```

## 120×40

Same structure with room: a `source` column (marketplace / `~/.claude` / project), the bar 16
wide, the detail panel 5 lines, children shown with their kind word (`skill`, `command`, `MCP
server`, `hook`, `agent`).

## Codex (P0.5) — the same screen, Codex's words

One tab per agent, Claude Code first. `Tab` / `Shift-Tab` switch; the rule under the header says
`Tab shows Codex` (or `Tab shows Claude Code`) whenever there is a second tab — the only line a
Claude Code frame gains — and a plain rule otherwise. Without `--agent`, an agent with no item in
this directory gets no tab. Everything else follows the rules above; what differs is wording:

- **Header line 1** names the agent and, for Codex, its trust in this folder: `repo · Codex ·
  trusted · 9 items load before your first message` (`untrusted`, `not trusted` when unset). At 80
  columns the trust word goes before the agent's name and before `read-only`.
- **Notes** (`i`): the trust sentence is the first one, each claim from the gate that decides
  it — the project `.codex/config.toml`, hooks and rules from the folder's trust, the AGENTS.md
  files from the AGENTS.md gate: `This folder is trusted by Codex (set in ~/.codex/config.toml):
  its .codex/config.toml, hooks, rules and AGENTS.md files load; skills load either way.` / `This
  folder is marked untrusted for Codex (set in …): its .codex/config.toml, hooks and rules are
  skipped; its AGENTS.md files load, by a separate entry (set in …); skills load either way.` /
  `Codex has no trust decision for this folder: its .codex/config.toml, hooks and rules are
  ignored; its AGENTS.md files load; skills load either way.` Tool search gets a note when there is an MCP
  server: `Tool search is on: the model's catalog entry supports it, so MCP tools are sent only
  when the model looks them up.` / `… off: … every MCP tool schema loads in the first request; no
  Codex setting changes that.` Every resolver text reads `key in file` (`model in
  ~/.codex/config.toml`), never `file#/pointer`, and codex source references stay in `harn why`.
- **Groups**: `AGENTS.md files` (one row per file — the global one and the project chain;
  `biggest:` calls it `AGENTS.md`), `Skill listing` (`N entries · over its budget` when the
  2 %/10k-token budget is exceeded), `Plugins` (a plugin row is a switch; its skills, MCP servers
  and hooks fold under it; "Turn one off and its skills, MCP servers and hooks leave next
  session."), `Hooks`, `MCP servers`, `Rules files` (`command rules Codex reads (*.rules)`; no
  switch, `e` edits), `Built-in blocks` (`text Codex writes itself`; `6 of 8 can be turned off`
  when most have an `include_*` switch). Wrappers are rows of Built-in blocks with names for
  people: `AGENTS.md wrapper` ("the heading and tags Codex puts around the AGENTS.md files"),
  `skill listing wrapper`.
- **Block sentences** say what the block is for: base instructions "Codex's own instructions for
  the model, from its model catalog", permissions "tells the model its sandbox, approval policy
  and approved commands", environment context "the working folder, shell, date and time zone",
  multi-agent "lets the model hand work to sub-agents, with their tools", apps "about connected
  apps; sent only when an app is reachable".
- **What `d` does** is the item's own switch (its first knob: a hook's `[hooks.state]` entry, a
  skill's `[[skills.config]]` entry), never a wider one that happens to sit in a narrower file
  (`features.hooks`, the whole plugin); one sentence with the scope, the key and the file: `d turns it off for every
  project by writing include_environment_context = false to ~/.codex/config.toml.` In a trusted
  folder the same key goes to `.codex/config.toml`, "for this project only". The verb follows the
  key: `leaves out the project AGENTS.md files` (`project_doc_max_bytes = 0`), `hides it`
  (`adding an entry to skills.config`), `turns off the whole docs plugin`, `drops the whole skill
  listing`. The card's Writes line is the TOML key as written: `~/.codex/config.toml →
  plugins."docs@market".enabled = false`.
- **Sizes and what can measure them.** A group with some rows of unknown size shows its known
  part as a floor — `0.1 KB  + unmeasured`, no bar, no share — never as if the group cost only
  that. A sentence names `harn verify --run --agent codex` only for what Codex's dump has (the
  blocks other than the base instructions, the skill listing's fit, the AGENTS.md message); hook
  output ("Codex's dump does not include hook output, so harn cannot measure it"), MCP tool
  schemas ("not in Codex's dump"), rules ("inside the permissions block; harn does not measure
  them separately") and the base instructions ("counted from the model catalog; Codex sends it
  outside its dump") say so instead. Tool search has one note: "Tool search cannot be decided
  from the files (…): MCP servers are shown as loading up front."
- **`w` cards and notes** show only what is for a person: a resolver step that cites codex's
  source or research, or is a developer's note (a lead not verified, an `[unverified]`/`[추정]`
  tag), is cut to its human part or dropped; content hashes, hook state keys and a skill's
  rendered listing line stay in `harn why`. The AGENTS chain reads `AGENTS.md in this folder
  (~/2lab.ai/zbrain)`, or `AGENTS.md in ~/repo, one of the files from the project root down to
  this folder`; a project file's `d` says what else it frees ("That frees 4.2 KB: the file and
  the wrapper around the AGENTS.md files"). A rules file is named by its file (`default.rules`,
  source `~/.codex`).
- **Rows that do not load or run** (an `off:` row) say why instead of a size and offer no `d`:
  `not approved` ("Codex runs a hook only after you approve it in codex /hooks; …", "harn cannot
  approve it; codex /hooks does."), `off by policy` (allow_managed_hooks_only), `off, with every
  other hook` (features.hooks = false), `off` ("Your settings turn it off (mcp_servers.old.enabled =
  false); it does not load here.", "harn did not make that change, so o and u cannot reverse it.").
- **No switch**: "It cannot be turned off: Codex has no setting for it; empty the file or move it."
  (the global AGENTS.md), "… it goes when the last AGENTS.md file goes." (the AGENTS.md wrapper),
  "… Codex has no switch for it; model_instructions_file can only replace it, and Codex strongly
  discourages that." (base instructions).
- `t` says `AI trim for Codex is not implemented yet; `harn trim` trims Claude Code`.

## View-model (what the renderer consumes; `src/tui/model.rs`)

- `Screen { title: Title, groups: Vec<Group>, notes: Vec<Note> }`, `Group { kind: GroupKind, name,
  count_line, bytes: Option<u64>, share, rows: Vec<Row>, can_turn_off: bool }`, `Row { id, name,
  source, kind_word, bytes: Option<u64>, share, children: Vec<Row>, state: On|Off|Fixed, detail:
  Detail }`, `Detail { what, brings, action, saves, undo }` — sentences built once from the Tree,
  with every path home-relative and every scope in words.
- Grouping rule: plugin facets (`plugin:<p>:<server>` MCP rows, hooks whose origin is a plugin,
  skills/commands/agents of a plugin) fold under the plugin row. The skill listing group lists the
  216 entries grouped by owner (plugin / project / user) with each entry's own listing bytes; the
  group line carries the cap fact. Memory = auto-memory + memory files. Project files = CLAUDE.md
  chain (imports as children), rules, `~/.claude/CLAUDE.md`. Built-in = system prompt, environment,
  date, gitStatus, userEmail, attribution, built-in tools.
- Sizes: a row's bytes are its own contribution (cost.predicted/observed); a group's bytes are the
  sum of its rows; `?` when unknown (never `+Δ?` in the list).
- Sorting inside a group: bytes desc, unknown last, name asc as the tiebreak; `s` toggles name.

## Acceptance (A16 — replaces A14's readability clause)

Run `harn ~/2lab.ai/zbrain` in tmux 80×24 and 120×40, capture: first screen, Plugins opened, cursor
on slack, `d` card, `w` card, `?` card, after Enter (applied), after `u` (undone). A reader who has
never seen harn must be able to say, from the first screen alone, what the five biggest things are
and that `d` turns a thing off; from the detail panel, where the cursor row is enabled and what
leaves if it goes. No raw id, absolute path, hash, `Δ?`, `mode`, `knob` or `ops (1)` appears in any
captured frame. No mid-word wrap. The dispatcher reads every frame before declaring the row green.
