# 04 — The interactive view: information design

Status: planned — 2026-10-13. Written after the user read the first TUI (frames in zbrain
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
  Share = of the predicted total. Bars are relative to the largest row on screen (ncdu), drawn in
  eighth-blocks, `#` under `--no-unicode`.
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
  `read-only: d o u e are off`; the header says `read-only`); the Built-in group row has no `d`.
- **Footer honesty**: `345 items · 5 shown · filter: none` only when a filter or fold hides rows
  that would otherwise be visible; otherwise no footer (the panel is the footer).

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
