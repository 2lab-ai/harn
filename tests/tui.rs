//! The interactive view on a copy of the zbrain-like fixture, driven by key events and drawn on a
//! ratatui `TestBackend` (no terminal): the view-model (groups, one row per plugin, own bytes,
//! names), each mockup of `.prd/04-tui-design.md` asserted on the drawn frame at 80×24 and
//! 120×40, the `d` card → Enter → journal → `u`, `o`, `e`, `--readonly`, and what no frame may
//! ever show.

mod common;

use common::{fixture, target, Fixture};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use harn::journal::{self, Journal, KIND_EDIT};
use harn::model::{self, Kind, Tree};
use harn::plan::{self, PlanOp, PlanRequest, PLAN_SCHEMA};
use harn::resolve::{ClaudeCode, Resolver};
use harn::tui::model::{GroupKind, Row, State};
use harn::tui::{App, Opts, Part, Pending, Sort, Tone, TRIM_HINT};
use harn::writers;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use ratatui::Terminal;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const ALPHA: &str = "claude-code/plugin/plugin/alpha@market-one";
const BETA: &str = "claude-code/plugin/plugin/beta@market-two";

/// Resolved and freed-annotated, as `harn` does before the view.
fn tree(f: &Fixture) -> Tree {
    let t = target(f);
    let mut tree = ClaudeCode.resolve(&t, None).expect("resolve");
    plan::annotate_freed(&ClaudeCode, &t, &mut tree).expect("annotate");
    tree
}

fn journal(f: &Fixture) -> Journal {
    Journal {
        dir: f.root.join("state"),
    }
}

fn app_on(f: &Fixture, tree: Tree, readonly: bool, unicode: bool) -> App {
    App::new(
        target(f),
        vec![(Box::new(ClaudeCode) as Box<dyn Resolver>, tree)],
        journal(f),
        Opts {
            unicode,
            color: false,
            readonly,
        },
    )
}

fn app_with(f: &Fixture, readonly: bool) -> App {
    app_on(f, tree(f), readonly, true)
}

fn app(f: &Fixture) -> App {
    app_with(f, false)
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn keys(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn draw(app: &App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal.backend().buffer().clone()
}

/// Each row as the terminal shows it (a wide char's second cell skipped).
fn lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut row = String::new();
            let mut x = 0;
            while x < area.width {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                x += unicode_width::UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            row
        })
        .collect()
}

fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
    let frame = lines(&draw(app, width, height));
    if std::env::var_os("HARN_TUI_DUMP").is_some() {
        eprintln!("{}\n", frame.join("\n"));
    }
    frame
}

fn text(app: &App, width: u16, height: u16) -> String {
    screen(app, width, height).join("\n")
}

fn local_settings(f: &Fixture) -> PathBuf {
    f.project.join(".claude/settings.local.json")
}

fn message(app: &App) -> String {
    app.message().map(|m| m.text()).unwrap_or_default()
}

/// The frame row drawn reversed below the header (the cursor line).
fn cursor_line(buffer: &Buffer) -> Option<String> {
    let rows = lines(buffer);
    (3..buffer.area.height).find_map(|y| {
        let reversed =
            (0..buffer.area.width).any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED));
        reversed.then(|| rows[y as usize].clone())
    })
}

/// The detail panel: the lines between the last two rules, right-trimmed.
fn panel(frame: &[String]) -> Vec<String> {
    let rules: Vec<usize> = frame
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("──") || l.starts_with("--"))
        .map(|(i, _)| i)
        .collect();
    let (from, to) = (rules[rules.len() - 2], rules[rules.len() - 1]);
    frame[from + 1..to]
        .iter()
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// The lines inside the card whose top border starts with `title`, borders cut, right-trimmed.
fn card(frame: &[String], title: &str) -> Vec<String> {
    let top = frame
        .iter()
        .position(|l| l.trim_start().starts_with(&format!("┌ {title} ─")))
        .unwrap_or_else(|| panic!("no card {title}:\n{}", frame.join("\n")));
    frame[top + 1..]
        .iter()
        .take_while(|l| !l.trim_start().starts_with('└'))
        .map(|l| {
            let start = l.find('│').unwrap() + '│'.len_utf8();
            let end = l.rfind('│').unwrap();
            l[start..end].trim_end().to_string()
        })
        .collect()
}

/// Every row of the group of `kind`.
fn group_rows(app: &App, kind: GroupKind) -> Vec<Row> {
    app.screen()
        .groups
        .iter()
        .find(|g| g.kind == kind)
        .unwrap_or_else(|| panic!("no group {kind:?}"))
        .rows
        .clone()
}

fn row<'a>(rows: &'a [Row], name: &str) -> &'a Row {
    rows.iter().find(|r| r.name == name).unwrap_or_else(|| {
        panic!(
            "no row {name}: {:?}",
            rows.iter().map(|r| &r.name).collect::<Vec<_>>()
        )
    })
}

/// Opens every group and row, top to bottom, and leaves the cursor on the first line.
fn open_all(app: &mut App) {
    press(app, KeyCode::Char('g'));
    loop {
        press(app, KeyCode::Char('l'));
        let at = app.cursor();
        press(app, KeyCode::Char('j'));
        if app.cursor() == at {
            break;
        }
    }
    press(app, KeyCode::Char('g'));
}

/// A card's or the confirm's top border (`┌ title ─┐`, ASCII `+ title -+`).
fn box_top(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('┌') || (t.starts_with('+') && t.ends_with('+') && t.len() > 1)
}

/// A card's or the confirm's bottom border (`└──┘`, ASCII `+--+`).
fn box_bottom(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('└') || (t.starts_with("+-") && t.ends_with('+'))
}

/// The list rows of a frame, by the view's layout: the rows between the header rule and the
/// rule over the panel (a 4-row panel, 5 from 40 rows high, fewer when short), less the rows a
/// card or the confirm takes, from its top border to its bottom border. A row whose text holds
/// `+` or `|` is a list row like any other.
fn list_rows(frame: &str) -> Vec<&str> {
    let lines: Vec<&str> = frame.lines().collect();
    let h = lines.len();
    let panel = if h >= 40 { 5 } else { 4 }
        .min(h.saturating_sub(9))
        .max(usize::from(h >= 8));
    let end = h.saturating_sub(3 + panel);
    let mut rows = Vec::new();
    let mut in_box = false;
    for (i, line) in lines.iter().enumerate() {
        if !in_box && box_top(line) {
            in_box = true;
            continue;
        }
        if in_box {
            in_box = !box_bottom(line);
            continue;
        }
        if (3..end).contains(&i) {
            rows.push(*line);
        }
    }
    rows
}

/// Bytes of every file under the fixture (its project, its home, the journal).
fn files(f: &Fixture) -> Vec<(PathBuf, Vec<u8>)> {
    snapshot(&f.root)
}

// ---- view-model ------------------------------------------------------------------------------

#[test]
fn groups_are_sorted_by_size_and_their_shares_partition_the_total() {
    let f = fixture();
    let app = app(&f);
    let kinds: Vec<GroupKind> = app.screen().groups.iter().map(|g| g.kind).collect();
    assert_eq!(
        kinds,
        [
            GroupKind::Memory,
            GroupKind::ProjectFiles,
            GroupKind::SkillListing,
            GroupKind::AgentListing,
            GroupKind::Plugins,
            GroupKind::Hooks,
            GroupKind::McpServers,
            GroupKind::Builtin,
        ],
        "biggest first, unmeasured after (in their own order), built-in last"
    );
    let names: Vec<String> = app.screen().groups.iter().map(|g| g.name.clone()).collect();
    assert_eq!(
        names,
        [
            "Memory (1)",
            "Project files (4)",
            "Skill listing",
            "Agent listing",
            "Plugins (2)",
            "Hooks (2)",
            "MCP servers (2)",
            "Built-in (7)"
        ]
    );
    let group = |kind| app.screen().groups.iter().find(|g| g.kind == kind).unwrap();
    assert_eq!(group(GroupKind::SkillListing).count_line, "20 entries");
    assert_eq!(group(GroupKind::AgentListing).count_line, "10 entries");
    // The Plugins group has no size or share: its rows are what turning each plugin off frees,
    // mostly listing lines the listings count already. Its line names that overlap.
    let plugins = group(GroupKind::Plugins);
    assert!(plugins.overlap);
    assert_eq!((plugins.bytes, plugins.share), (None, None));
    assert_eq!(
        plugins.count_line,
        "0.3 KB would leave with them · counted in the listings above"
    );
    assert_eq!(plugins.count_short, "0.3 KB in the listings above");
    // The other groups' bytes are their rows'; together they never pass the total, nor their
    // shares 100.
    let mut bytes = 0;
    let mut shares = 0;
    for g in app.screen().groups.iter().filter(|g| !g.overlap) {
        let rows = g
            .rows
            .iter()
            .filter_map(|r| r.bytes)
            .fold(None, |sum: Option<u64>, b| Some(sum.unwrap_or(0) + b));
        assert_eq!(g.bytes, rows, "{}", g.name);
        bytes += g.bytes.unwrap_or(0);
        shares += g.share.unwrap_or(0);
    }
    assert!(bytes <= app.tree().predicted_known, "{bytes}");
    assert!(shares <= 100, "{shares}");
    // Every first-request byte is counted once: the sized groups add up to the whole total
    // here (no plugin's own part is measured in the fixture).
    assert_eq!(bytes, app.tree().predicted_known);
    let builtin: Vec<String> = group_rows(&app, GroupKind::Builtin)
        .iter()
        .map(|r| r.name.clone())
        .collect();
    assert!(
        builtin.iter().any(|n| n.starts_with("built-in tools (")),
        "{builtin:?}"
    );
    assert!(builtin.contains(&"built-in plugins (3)".to_string()));
    assert_eq!(app.screen().notes.len(), 3);
}

#[test]
fn a_plugin_is_one_row_with_its_parts_folded_under_it() {
    let f = fixture();
    let app = app(&f);
    let plugins = group_rows(&app, GroupKind::Plugins);
    let names: Vec<&str> = plugins.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["beta", "alpha"], "one row each, biggest first");
    let alpha = row(&plugins, "alpha");
    let mut parts: Vec<(&str, &str)> = alpha
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.kind_word.as_str()))
        .collect();
    parts.sort();
    assert_eq!(
        parts,
        [
            ("UserPromptSubmit hook", "hook"),
            ("alpha-other", "skill"),
            ("alpha-server", "MCP server"),
            ("alpha-skill", "skill"),
        ]
    );
    let beta = row(&plugins, "beta");
    let mut agents: Vec<&str> = beta.children.iter().map(|c| c.name.as_str()).collect();
    agents.sort();
    assert_eq!(agents, ["beta-agent", "beta-reviewer"]);
    // Never beside it: alpha's server is not among the MCP servers, its hook not in Hooks.
    let servers: Vec<String> = group_rows(&app, GroupKind::McpServers)
        .iter()
        .map(|r| r.name.clone())
        .collect();
    assert_eq!(servers.len(), 2, "{servers:?}");
    assert!(!servers.iter().any(|n| n.contains("alpha")), "{servers:?}");
    let hooks = group_rows(&app, GroupKind::Hooks);
    assert!(
        hooks
            .iter()
            .all(|h| !h.id.as_deref().unwrap().contains("/alpha/")),
        "{hooks:?}"
    );
    // No group holds the same item twice side by side.
    for group in &app.screen().groups {
        let mut ids: Vec<&str> = group.rows.iter().filter_map(|r| r.id.as_deref()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "{}", group.name);
    }
    // The skill listing lists entries by owner; alpha's two skills under `alpha`.
    let owners = group_rows(&app, GroupKind::SkillListing);
    let owner = row(&owners, "alpha");
    assert_eq!(owner.children.len(), 2);
    assert_eq!(
        owner.id.as_deref(),
        Some(ALPHA),
        "d on the owner = the plugin"
    );
    assert_eq!(owner.source, "market-one");
    let entries: usize = owners.iter().map(|o| o.children.len()).sum();
    assert_eq!(entries, 20, "every listed entry once");
}

#[test]
fn every_row_carries_its_own_bytes_never_its_parents() {
    let f = fixture();
    let app = app(&f);
    let tree = app.tree().clone();
    let plugins = group_rows(&app, GroupKind::Plugins);
    let alpha = row(&plugins, "alpha");
    // A plugin row is what turning it off frees: its two skills' listing lines (73 B each) and
    // its own server and hook, unmeasured.
    assert_eq!((alpha.bytes, alpha.absent), (Some(146), None));
    assert_eq!(
        alpha.saving.as_deref(),
        Some("0.1 KB: 0.1 KB of its entries in the skill listing, plus its own unmeasured part")
    );
    // Its parts carry their own bytes, never the plugin's.
    let cell = |name: &str| {
        let r = row(&alpha.children, name);
        (r.bytes, r.absent)
    };
    assert_eq!(cell("alpha-skill"), (Some(73), None));
    assert_eq!(cell("alpha-other"), (Some(73), None));
    assert_eq!(cell("UserPromptSubmit hook"), (None, None), "unmeasured");
    assert_eq!(cell("alpha-server"), (None, None), "unmeasured");
    let owners = group_rows(&app, GroupKind::SkillListing);
    let listed = row(&owners, "alpha");
    assert_eq!(listed.bytes, Some(146));
    assert_eq!(row(&listed.children, "alpha-skill").bytes, Some(73));
    let beta = row(&plugins, "beta");
    assert_eq!((beta.bytes, beta.absent), (Some(80 + 104), None));
    assert_eq!(
        beta.saving.as_deref(),
        Some("0.2 KB: its entries in the agent listing")
    );
    // The resolver copied the plugin's freed bytes onto its server; the row does not.
    let server = model::walk(&tree)
        .find(|i| i.label == "plugin:alpha:alpha-server")
        .unwrap();
    assert_eq!(server.freed.as_ref().map(|f| f.known), Some(146));

    let files = group_rows(&app, GroupKind::ProjectFiles);
    let claude = row(&files, "CLAUDE.md + 2 imports");
    let a = row(&claude.children, "rules/a.md + 1 import");
    let b = row(&a.children, "rules/b.md");
    let cost = |label: &str| {
        model::walk(&tree)
            .find(|i| i.label == label)
            .and_then(|i| i.cost.predicted)
            .unwrap()
    };
    assert_eq!(b.bytes, Some(cost("rules/b.md")));
    assert_eq!(a.bytes, Some(cost("rules/a.md") + cost("rules/b.md")));
    assert_eq!(
        claude.bytes,
        Some(cost("CLAUDE.md") + cost("rules/a.md") + cost("rules/b.md"))
    );
    // On-demand, not-loaded and later rows cost nothing up front and say why.
    assert_eq!(
        row(&files, ".claude/rules/scoped.md").absent,
        Some("when used")
    );
    let servers = group_rows(&app, GroupKind::McpServers);
    assert_eq!(row(&servers, "user-server").absent, Some("not loaded"));
    let hooks = group_rows(&app, GroupKind::Hooks);
    assert_eq!(row(&hooks, "PreToolUse hook").absent, Some("runs later"));
}

#[test]
fn names_are_for_people_not_ids_or_paths() {
    let f = fixture();
    let app = app(&f);
    assert_eq!(app.screen().title.dir, "project");
    assert_eq!(app.screen().title.agent, "Claude Code");
    let memory = group_rows(&app, GroupKind::Memory);
    assert_eq!(memory[0].name, "MEMORY.md (auto-memory)");
    assert_eq!(memory[0].source, "~/.claude");
    let files: Vec<String> = group_rows(&app, GroupKind::ProjectFiles)
        .iter()
        .map(|r| r.name.clone())
        .collect();
    assert!(
        files.contains(&"~/.claude/CLAUDE.md".to_string()),
        "{files:?}"
    );
    assert!(
        files.contains(&".claude/rules/style.md".to_string()),
        "{files:?}"
    );
    let hooks: Vec<String> = group_rows(&app, GroupKind::Hooks)
        .iter()
        .map(|r| r.name.clone())
        .collect();
    assert!(
        hooks.contains(&"SessionStart hook".to_string()),
        "{hooks:?}"
    );
    let plugins = group_rows(&app, GroupKind::Plugins);
    assert_eq!(row(&plugins, "alpha").source, "market-one");
    let root = f.root.display().to_string();
    for r in app.screen().all_rows() {
        for text in [&r.name, &r.source, &r.kind_word] {
            assert!(!text.contains('@'), "{text}");
            assert!(!text.contains(&root), "{text}");
            assert!(!text.contains("claude-code/"), "{text}");
            assert!(!text.starts_with("plugin:"), "{text}");
        }
    }
}

// ---- the mockups -----------------------------------------------------------------------------

#[test]
fn first_screen_at_80x24_is_the_groups_the_panel_and_the_keys() {
    let f = fixture();
    let app = app(&f);
    let frame = screen(&app, 80, 24);
    let all = frame.join("\n");
    assert_eq!(frame.len(), 24);
    assert!(frame.iter().all(|l| l.chars().count() == 80), "{all}");
    assert_eq!(
        frame[0].trim_end(),
        " project · Claude Code · 83 items load before your first message         ~15 KB"
    );
    assert_eq!(
        frame[1].trim_end(),
        " biggest: memory 13 KB (85%) · project 1.2 KB                         ! 3 notes"
    );
    assert!(frame[2].chars().all(|c| c == '─'), "{all}");
    let groups: Vec<String> = frame[3..11]
        .iter()
        .map(|l| l.trim_end().to_string())
        .collect();
    assert_eq!(
        groups,
        [
            " ▸ Memory (1)                                         13 KB ██████████████  85%",
            " ▸ Project files (4)                                 1.2 KB █▍               8%",
            " ▸ Skill listing  20 entries                         0.5 KB ▌                3%",
            " ▸ Agent listing  10 entries                         0.4 KB ▌                2%",
            " ▸ Plugins (2)  0.3 KB would leave with them · counted in the listings above",
            " ▸ Hooks (2)                                     unmeasured",
            " ▸ MCP servers (2)                               unmeasured",
            " ▸ Built-in (7)  mostly cannot be turned off     unmeasured",
        ]
    );
    assert_eq!(
        panel(&frame),
        [
            " Memory · notes Claude Code keeps for this directory",
            " MEMORY.md (auto-memory) loads in every session here, up to its cap.",
            " Open it (→); d turns auto-memory off for this project only.",
            "",
        ]
    );
    assert_eq!(
        frame[23].trim_end(),
        " ↑↓ move   →← open/close   / search   d turn off   i notes   ? keys   q quit"
    );
    // The cursor starts on the first group; built-in is grey.
    let buffer = draw(&app, 80, 24);
    assert!(cursor_line(&buffer).unwrap().starts_with(" ▸ Memory (1)"));
    assert!(buffer[(3, 10)].modifier.contains(Modifier::DIM));
    assert!(!buffer[(3, 9)].modifier.contains(Modifier::DIM));
    // No footer: neither a filter nor the frame hides a line.
    assert!(!all.contains(" shown "), "{all}");
}

#[test]
fn first_screen_at_120x40_has_the_version_a_wider_bar_and_a_taller_panel() {
    let f = fixture();
    let mut app = app(&f);
    let frame = screen(&app, 120, 40);
    assert!(
        frame[0].starts_with(" project · Claude Code · 83 items load before your first message ")
    );
    assert!(frame[0].trim_end().ends_with("~15 KB"));
    assert_eq!(
        frame[1].trim_end(),
        " biggest: memory 13 KB (85%) · project 1.2 KB · skill listing 0.5 KB · agent listing \
         0.4 KB                   ! 3 notes"
    );
    let memory = frame.iter().find(|l| l.contains("Memory (1)")).unwrap();
    assert!(
        memory.contains(&"█".repeat(16)),
        "16 cells at 120: {memory}"
    );
    assert!(!memory.contains(&"█".repeat(17)), "{memory}");
    assert_eq!(panel(&frame).len(), 5);
    // Opened rows carry their source; their parts carry their kind word.
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Right);
    let frame = screen(&app, 120, 40);
    let alpha = frame.iter().find(|l| l.contains("▾ alpha")).unwrap();
    assert!(alpha.contains("market-one"), "{alpha}");
    // The Plugins group line names the overlap and has no bar or share.
    let group = frame.iter().find(|l| l.contains("Plugins (2)")).unwrap();
    assert_eq!(
        group.trim_end(),
        " ▾ Plugins (2)  0.3 KB would leave with them · counted in the listings above"
    );
    // Narrower than the long form: the short one, never a cut sentence.
    let narrow = screen(&app, 60, 24);
    let group = narrow.iter().find(|l| l.contains("Plugins (2)")).unwrap();
    assert_eq!(
        group.trim_end(),
        " ▾ Plugins (2)  0.3 KB in the listings above"
    );
    // Under its plugin a skill shows its own listing line's bytes.
    let skill = frame.iter().find(|l| l.contains("alpha-skill")).unwrap();
    assert!(
        skill.contains(" skill ") && skill.contains("0.1 KB ▏"),
        "{skill}"
    );
    let server = frame.iter().find(|l| l.contains("alpha-server")).unwrap();
    assert!(server.contains("MCP server"), "{server}");
    assert!(server.trim_end().ends_with(" unmeasured"), "{server}");
    // A name that says its kind gets no kind column.
    let hook = frame
        .iter()
        .find(|l| l.contains("UserPromptSubmit hook"))
        .unwrap();
    assert_eq!(hook.matches("hook").count(), 1, "{hook}");
}

#[test]
fn plugins_opened_show_one_row_per_plugin_and_its_detail() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select_group("Plugins"));
    press(&mut app, KeyCode::Right);
    let rows = app.rows();
    assert_eq!(
        rows[4..8],
        [
            (0, "Plugins (2)".to_string()),
            (1, "beta".to_string()),
            (1, "alpha".to_string()),
            (0, "Hooks (2)".to_string()),
        ],
        "children folded"
    );
    assert!(app.select(ALPHA));
    let frame = screen(&app, 80, 24);
    let all = frame.join("\n");
    // The group line names the overlap, no bar or share; each plugin shows what turning it
    // off frees, its bar against the largest line shown, its share of the total.
    assert_eq!(
        frame[7].trim_end(),
        " ▾ Plugins (2)  0.3 KB would leave with them · counted in the listings above"
    );
    assert_eq!(
        frame[8].trim_end(),
        "   ▸ beta   market-two                               0.2 KB ▎                1%"
    );
    assert_eq!(
        frame[9].trim_end(),
        "   ▸ alpha  market-one                               0.1 KB ▏               <1%"
    );
    // The full saving is a sentence with both parts; every sentence whole.
    assert_eq!(
        panel(&frame),
        [
            " alpha · plugin from the market-one marketplace · enabled for every project",
            " Brings 2 skills, 1 MCP server and 1 hook. d turns it off for this project only",
            " (.claude/settings.local.json). Turning it off frees 0.1 KB: 0.1 KB of its",
            " entries in the skill listing, plus its own unmeasured part. Reversible with u.",
        ],
        "{all}"
    );
    assert_eq!(
        frame[23].trim_end(),
        " ↑↓ move  →← open/close  / search  d turn off  w where from  ? keys  q quit"
    );
    // 120 columns keep the version and the line structure.
    let wide = panel(&screen(&app, 120, 40));
    assert_eq!(
        wide[..3],
        [
            " alpha · plugin from the market-one marketplace, v1.0.0 · enabled for every project",
            " Brings 2 skills, 1 MCP server and 1 hook.",
            " d turns it off for this project only (.claude/settings.local.json). Turning it off frees 0.1 KB: 0.1 KB of its entries",
        ]
    );
}

#[test]
fn a_builtin_row_says_why_it_cannot_be_turned_off() {
    let f = fixture();
    let mut app = app(&f);
    let prompt = model::walk(app.tree())
        .find(|i| i.kind == Kind::SystemPrompt)
        .unwrap()
        .id
        .clone();
    assert!(app.select(&prompt));
    let frame = screen(&app, 80, 24);
    assert_eq!(
        panel(&frame)[..3],
        [
            " system prompt (built-in) · part of Claude Code itself",
            " Its size is unknown until `harn verify --run` measures a session.",
            " It cannot be turned off: it is part of Claude Code.",
        ]
    );
    assert!(!frame[23].contains("d turn off"), "{}", frame[23]);
    let builtin = app
        .screen()
        .groups
        .iter()
        .find(|g| g.kind == GroupKind::Builtin)
        .unwrap();
    assert_eq!(builtin.count_line, "mostly cannot be turned off");
    let fixed = builtin
        .rows
        .iter()
        .filter(|r| r.state == State::Fixed)
        .count();
    assert!(fixed >= 3, "{:?}", builtin.rows);
}

#[test]
fn d_opens_one_card_enter_applies_through_the_journal_and_u_restores_the_bytes() {
    let f = fixture();
    let before = std::fs::read(local_settings(&f)).unwrap();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    let untouched = files(&f);
    press(&mut app, KeyCode::Char('d'));
    assert!(app.staged().is_some());
    // Nothing is written while the card is open, nor while the list of what leaves is.
    assert_eq!(files(&f), untouched, "d wrote before Enter");
    press(&mut app, KeyCode::Right);
    card(&screen(&app, 80, 24), "What leaves with alpha");
    assert_eq!(files(&f), untouched, "the leaves list wrote before Enter");
    press(&mut app, KeyCode::Esc);
    assert_eq!(files(&f), untouched);
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let frame = screen(&app, width, height);
        let inside = card(&frame, "Turn off alpha for this project");
        assert_eq!(
            inside[..5],
            [
                " Writes   .claude/settings.local.json → enabledPlugins.alpha = false",
                " Leaves   2 skills, 1 MCP server, 1 hook (next session)                → list",
                " Saves    0.1 KB: 0.1 KB of its entries in the skill listing, plus its own",
                "          unmeasured part · 6 fewer items at start",
                " Undo     u here, or `harn undo` later — the file is restored byte for byte",
            ],
            "{width}x{height}"
        );
        assert_eq!(inside[5].trim(), "Enter apply · Esc");
        assert_eq!(inside.len(), 6);
        assert!(frame[usize::from(height) - 1].contains("Enter apply"));
    }
    assert!(
        journal::history(&journal(&f)).unwrap().is_empty(),
        "nothing yet"
    );

    let count = app.screen().title.items;
    let predicted = fewer_on_card(&card(
        &screen(&app, 80, 24),
        "Turn off alpha for this project",
    ));
    press(&mut app, KeyCode::Enter);
    // The card's prediction is the header's drop.
    assert_eq!(count - app.screen().title.items, predicted);
    let entries = journal::history(&journal(&f)).unwrap();
    assert_eq!(entries.len(), 1);
    assert_ne!(std::fs::read(local_settings(&f)).unwrap(), before);
    assert!(
        model::walk(app.tree()).all(|i| i.id != ALPHA),
        "re-resolved"
    );
    assert_eq!(app.cursor_id().as_deref(), Some(ALPHA), "on its off row");
    // The header's count moves by what the message says: the plugin row, its 2 skills, its
    // server with that server's instructions, its hook.
    assert_eq!(app.screen().title.items, count - 6);
    assert_eq!(
        message(&app),
        format!(
            "turned off alpha · 6 fewer items load at start ({count} → {}) · u undoes · takes \
             effect next session",
            count - 6
        )
    );
    let frame = screen(&app, 80, 24);
    assert_eq!(
        panel(&frame)[1],
        " alpha · turned off · u undoes · takes effect next session"
    );
    let buffer = draw(&app, 80, 24);
    let off = cursor_line(&buffer).unwrap();
    assert!(off.starts_with("     alpha  market-one"), "{off}");
    assert!(off.trim_end().ends_with("—  off"), "{off}");
    let y = lines(&buffer)
        .iter()
        .position(|l| l.starts_with("     alpha"))
        .unwrap();
    assert!(
        buffer[(5, y as u16)].modifier.contains(Modifier::DIM),
        "greyed"
    );

    press(&mut app, KeyCode::Char('u'));
    let inside = card(&screen(&app, 80, 24), "Undo the last change");
    assert_eq!(
        inside,
        [
            " alpha comes back on",
            " restores .claude/settings.local.json byte for byte",
            "                                                        Enter undo · Esc keep",
        ]
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(std::fs::read(local_settings(&f)).unwrap(), before);
    assert!(journal::history(&journal(&f)).unwrap()[0]
        .undone_at
        .is_some());
    assert!(model::walk(app.tree()).any(|i| i.id == ALPHA));
    assert_eq!(
        message(&app),
        "undone: alpha back · .claude/settings.local.json restored byte for byte"
    );
}

#[test]
fn the_leaves_line_opens_the_list_of_what_leaves() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Right);
    let mut inside = card(&screen(&app, 80, 24), "What leaves with alpha");
    assert_eq!(inside.pop().unwrap().trim(), "Esc back");
    // In the Leaves line's order: skills, then MCP servers, then hooks.
    let kinds: Vec<&str> = inside
        .iter()
        .map(|l| l.rsplit_once(" (").unwrap().1)
        .collect();
    assert_eq!(kinds, ["skill)", "skill)", "MCP server)", "hook)"]);
    inside.sort();
    assert_eq!(
        inside,
        [
            " UserPromptSubmit (hook)",
            " alpha-other (skill)",
            " alpha-server (MCP server)",
            " alpha-skill (skill)",
        ]
    );
    press(&mut app, KeyCode::Esc);
    card(&screen(&app, 80, 24), "Turn off alpha for this project");
    assert!(app.staged().is_some(), "back on the card");
}

#[test]
fn esc_on_the_card_writes_nothing() {
    let f = fixture();
    let before = files(&f);
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    // Checked at every step, not only after Esc: writing on `d` and rolling back on Esc fails.
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(files(&f), before, "after d");
    press(&mut app, KeyCode::Right);
    assert_eq!(files(&f), before, "with the leaves list open");
    press(&mut app, KeyCode::Esc);
    assert_eq!(files(&f), before, "back on the card");
    press(&mut app, KeyCode::Esc);
    assert!(app.staged().is_none());
    assert_eq!(message(&app), "cancelled: nothing written");
    assert_eq!(files(&f), before, "after Esc");
    assert!(!f.root.join("state").exists(), "not even the journal dir");
    assert!(!app.should_quit());
}

#[test]
fn w_shows_where_it_comes_from_home_relative_and_wrapped_on_slashes() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('w'));
    assert_eq!(
        card(&screen(&app, 80, 24), "Where alpha comes from"),
        [
            " 1  ~/.claude/settings.json › enabledPlugins › alpha = true   (every project)",
            " 2  installed from market-one, version 1.0.0",
            " 3  files: ~/.claude/plugins/cache/market-one/alpha/1.0.0",
            " 4  contributes 2 skills · 1 hook · 1 MCP server",
            " Switches  this project (.claude/settings.local.json)",
            "                                                                    Esc close",
        ]
    );
    // Narrow: the path wraps after a `/`, never inside a word.
    let narrow = card(&screen(&app, 44, 30), "Where alpha comes from").join("\n");
    assert!(narrow.contains("~/.claude/plugins/cache/"), "{narrow}");
    for word in [
        "market-one/",
        "enabledPlugins",
        "settings.json",
        "installed",
    ] {
        assert!(narrow.contains(word), "{word} cut:\n{narrow}");
    }
    press(&mut app, KeyCode::Esc);
    assert!(!text(&app, 80, 24).contains("Where alpha"));
}

#[test]
fn the_keys_card_has_aligned_columns_and_whole_words() {
    let f = fixture();
    let mut app = app(&f);
    press(&mut app, KeyCode::Char('?'));
    let frame = screen(&app, 80, 24);
    let top = frame
        .iter()
        .position(|l| l.starts_with("┌ Keys ─"))
        .unwrap();
    assert_eq!(
        frame[top + 1..top + 6],
        [
            "│ ↑ ↓  move           →  ←   open / close group        /   search (Esc clears) │",
            "│ g G  top / bottom   s      sort: size · name         i   notes               │",
            "│ d    turn off       o      turn back on              u   undo last change    │",
            "│ w    where from     e      edit the file ($EDITOR)   t   let the AI trim     │",
            "│ ?    this card      q Esc  quit / close              r   refresh             │",
        ]
    );
    // Every line keeps a space before the border, as on every other card.
    for line in &frame[top + 1..top + 6] {
        assert!(line.ends_with(" │"), "{line}");
    }
    // Narrower than the three columns: one key per line, words whole.
    let narrow = card(&screen(&app, 50, 30), "Keys").join("\n");
    for words in [
        "open / close group",
        "edit the file ($EDITOR)",
        "undo last change",
    ] {
        assert!(narrow.contains(words), "{words}:\n{narrow}");
    }
    press(&mut app, KeyCode::Char('?'));
    assert!(!text(&app, 80, 24).contains("┌ Keys"));
}

#[test]
fn i_opens_the_notes_and_the_header_counts_them() {
    let f = fixture();
    let mut app = app(&f);
    assert!(screen(&app, 80, 24)[1].contains("! 3 notes"));
    press(&mut app, KeyCode::Char('i'));
    let notes = card(&screen(&app, 80, 24), "Notes").join("\n");
    assert!(notes.starts_with(" 1  Tool search is off"), "{notes}");
    assert!(notes.contains("ENABLE_TOOL_SEARCH=true"), "{notes}");
    assert!(
        notes.contains(" 3  The skill listing has a size cap"),
        "{notes}"
    );
    // The header never carries a note's text.
    let header = screen(&app, 80, 24)[..2].join("\n");
    assert!(!header.contains("Tool search") && !header.contains("warning"));
    press(&mut app, KeyCode::Esc);
    assert!(!text(&app, 80, 24).contains("┌ Notes"));
}

#[test]
fn ascii_mode_draws_markers_bars_and_borders_in_ascii() {
    let f = fixture();
    let mut app = app_on(&f, tree(&f), false, false);
    let first = screen(&app, 80, 24);
    assert!(first[3].starts_with(" > Memory (1)"), "{}", first[3]);
    assert!(first[3].contains("##############  85%"), "{}", first[3]);
    assert_eq!(
        first[7].trim_end(),
        " > Plugins (2)  0.3 KB would leave with them | counted in the listings above"
    );
    assert!(first[8].contains(" unmeasured"), "{}", first[8]);
    assert!(first[2].chars().all(|c| c == '-'));
    assert!(
        first[23].starts_with(" j/k move   l/h open/close"),
        "{}",
        first[23]
    );
    press(&mut app, KeyCode::Right);
    assert!(screen(&app, 80, 24)[3].starts_with(" v Memory (1)"));
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    let frame = screen(&app, 80, 24);
    let top = frame
        .iter()
        .find(|l| l.starts_with("+ Turn off alpha for this project -"))
        .unwrap_or_else(|| panic!("{}", frame.join("\n")));
    assert!(top.ends_with('+'));
    let all = frame.join("\n");
    assert!(
        all.contains("| Writes   .claude/settings.local.json -> enabledPlugins.alpha = false"),
        "{all}"
    );
    assert!(all.contains("Enter apply | Esc"), "{all}");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('?'));
    assert!(text(&app, 80, 24).contains("| j k  move"));
}

/// Every string in `value` with its non-ASCII chars replaced by `-`.
fn ascii_only(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(s) if !s.is_ascii() => Value::String(
            s.chars()
                .map(|c| if c.is_ascii() { c } else { '-' })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(ascii_only).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, ascii_only(v))).collect())
        }
        other => other,
    }
}

#[test]
fn ascii_data_under_no_unicode_draws_zero_non_ascii_bytes() {
    let f = fixture();
    let data = ascii_only(serde_json::to_value(tree(&f)).unwrap());
    let ascii_tree: Tree = serde_json::from_value(data).unwrap();
    let mut app = app_on(&f, ascii_tree, false, false);
    let strict = |app: &App, what: &str| {
        for (width, height) in [(120, 40), (80, 24), (40, 12)] {
            for line in screen(app, width, height) {
                assert!(line.is_ascii(), "{what} {width}x{height}: {line}");
            }
        }
    };
    strict(&app, "list");
    press(&mut app, KeyCode::Char('?'));
    strict(&app, "keys");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('i'));
    strict(&app, "notes");
    press(&mut app, KeyCode::Esc);
    open_all(&mut app);
    strict(&app, "everything open");
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('w'));
    strict(&app, "where");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('d'));
    strict(&app, "d card");
    press(&mut app, KeyCode::Right);
    strict(&app, "leave list");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);
    strict(&app, "applied");
    press(&mut app, KeyCode::Char('u'));
    strict(&app, "undo confirm");
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char('/'));
    keys(&mut app, "alpha");
    strict(&app, "filter");
}

/// What the readability review found on the old screen; none of it may be drawn. The fixture
/// lives outside `/Users/`, so its own root and home are checked too.
fn forbidden(f: &Fixture) -> Vec<String> {
    let mut words: Vec<String> = [
        "@market-one",
        "@market-two",
        "@claude-plugins-official",
        "/Users/",
        "sha256",
        "Δ?",
        "+Δ",
        "D?",
        "mode always",
        "knob",
        "ops (",
        "rows shown of",
        "filter: none",
        "claude-code/",
        "plugin:alpha",
        "#/",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    words.push(f.root.display().to_string());
    words.push(f.home.display().to_string());
    words
}

fn assert_clean(frame: &str, words: &[String], what: &str) {
    for word in words {
        assert!(
            !frame.contains(word.as_str()),
            "{what}: `{word}` in\n{frame}"
        );
    }
    for line in frame.lines() {
        // A sentence cut with an ellipsis.
        let line = line.trim_end().trim_end_matches(['│', '|']).trim_end();
        assert!(!line.ends_with('…'), "{what}: a cut sentence in\n{frame}");
    }
    // A bare `?` size cell anywhere in a list row. A `?` in a card, the key bar or a question
    // is no size: only list rows are searched, for a `?` standing alone between spaces.
    for row in list_rows(frame) {
        assert!(
            !row.split_whitespace().any(|w| w == "?"),
            "{what}: a bare ? size in {row:?}\n{frame}"
        );
    }
}

/// [`assert_clean`] on the frame at 80×24 and at 120×40.
fn clean_both(app: &App, words: &[String], what: &str) {
    for (w, h) in [(80, 24), (120, 40)] {
        assert_clean(&text(app, w, h), words, &format!("{what} at {w}x{h}"));
    }
}

#[test]
fn the_bare_question_mark_guard_catches_a_size_cell_anywhere_in_a_list_row() {
    // An 80×24 frame's layout: header, rule, 14 list rows, rule, 4 panel rows, rule, keys.
    let rule = "─".repeat(20);
    let frame = |list: &[&str]| {
        let mut lines = vec!["head".to_string(), "head".to_string(), rule.clone()];
        lines.extend(list.iter().map(|l| l.to_string()));
        lines.resize(17, String::new());
        lines.push(rule.clone());
        lines.extend([
            "panel ?".to_string(),
            String::new(),
            String::new(),
            String::new(),
        ]);
        lines.push(rule.clone());
        lines.push(" ? keys   q quit".to_string());
        lines.join("\n")
    };
    let caught = |list: &[&str]| {
        let frame = frame(list);
        std::panic::catch_unwind(|| assert_clean(&frame, &[], "probe")).is_err()
    };
    assert!(caught(&[" ▸ Hooks (2)    ?   0%"]), "mid-row");
    assert!(
        caught(&[" ▸ Hooks (2)                                              ?"]),
        "row end"
    );
    // A `+` in a row's own text does not make it a card's: still checked.
    assert!(
        caught(&[" ▸ CLAUDE.md + 2 imports    ?   0%"]),
        "a + in the name"
    );
    assert!(!caught(&[
        " ▸ Hooks (2)                                     unmeasured"
    ]));
    // Inside a card over the list (from its top border to its bottom), in the key bar, in the
    // panel: no size cell.
    assert!(!caught(&[
        "┌ Keys ──────────────┐",
        "│ ?     this card    │",
        "│ ▸ CLAUDE.md + 2 imports    ?   0% │",
        "└────────────────────┘",
    ]));
    assert!(!caught(&[
        "┌ Undo the last change ─────┐",
        "│ alpha comes back on   ?   │",
        "└───────────────────────────┘",
    ]));
    assert!(!caught(&[
        "+ Keys ---------------+",
        "| ?     this card     |",
        "+---------------------+",
    ]));
    // A row below a card is a list row again.
    assert!(caught(&[
        "┌ Keys ──────────────┐",
        "│ ?     this card    │",
        "└────────────────────┘",
        " ▸ Hooks (2)    ?   0%",
    ]));
}

/// What `d` must do on a line, decided from the model before any key acts on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A group: `d` points into it.
    Group,
    /// A row with a switch for this project: `d` opens a card.
    Actionable,
    /// A row with no such switch, or one gathering others: `d` says why, no card.
    Refused,
}

/// The journal's entries, newest first.
fn entries(f: &Fixture) -> Vec<journal::JournalEntry> {
    journal::history(&journal(f)).unwrap()
}

#[test]
fn no_frame_shows_a_raw_id_an_absolute_path_a_hash_or_an_internal_tag() {
    let f = fixture();
    let words = forbidden(&f);
    let mut app = app(&f);
    clean_both(&app, &words, "first screen");
    for key in ['i', '?'] {
        press(&mut app, KeyCode::Char(key));
        clean_both(&app, &words, &format!("{key} card"));
        press(&mut app, KeyCode::Esc);
    }
    open_all(&mut app);
    let total = app.rows().len();
    assert!(total > 60, "{total}");
    // Everything is open: no folded marker anywhere in the list.
    let tall = text(&app, 120, u16::try_from(total + 20).unwrap());
    assert!(
        list_rows(&tall).iter().all(|l| !l.contains('▸')),
        "a folded line left:\n{tall}"
    );

    // Every line classified from the model first (the cursor only moves).
    let mut expect = Vec::new();
    press(&mut app, KeyCode::Char('g'));
    for at in 0..total {
        assert_eq!(app.cursor(), at);
        let kind = if app.rows()[at].0 == 0 {
            Expect::Group
        } else {
            match app.cursor_id() {
                None => Expect::Refused,
                Some(id) => {
                    let item = model::walk(app.tree())
                        .find(|i| i.id == id)
                        .unwrap_or_else(|| panic!("{id}: a row for no item"));
                    let local = item
                        .knobs
                        .iter()
                        .any(|k| plan::knob_scope(k) == Some(model::Scope::Local));
                    if local {
                        Expect::Actionable
                    } else {
                        Expect::Refused
                    }
                }
            }
        };
        expect.push(kind);
        press(&mut app, KeyCode::Char('j'));
    }

    // Then every line in every state, each frame at 80×24 and 120×40: the list, its `w` card,
    // `d`; for an actionable line its card, the list of what leaves, the applied frame (its off
    // row), the `w` card on the off row, the `o` card, and the frame after turning it back on
    // (by `o` → Enter and by `u` → `y`, in turn).
    let (mut applied, mut refusals, mut by_o, mut by_u) = (0, 0, 0, 0);
    for (at, kind) in expect.iter().enumerate() {
        press(&mut app, KeyCode::Char('g'));
        for _ in 0..at {
            press(&mut app, KeyCode::Char('j'));
        }
        assert_eq!(app.cursor(), at);
        let name = app.cursor_name().unwrap();
        let id = app.cursor_id();
        let source = id.as_deref().and_then(|id| {
            let (g, path) = app.screen().find(id)?;
            app.screen().row(g, &path).map(|r| r.source.clone())
        });
        clean_both(&app, &words, &name);
        press(&mut app, KeyCode::Char('w'));
        clean_both(&app, &words, &format!("w {name}"));
        if app.has_card() {
            press(&mut app, KeyCode::Esc);
        }
        press(&mut app, KeyCode::Char('d'));
        clean_both(&app, &words, &format!("d {name}"));
        match kind {
            Expect::Group => {
                assert!(!app.has_card(), "{name}");
                assert!(message(&app).starts_with("d turns off one thing"), "{name}");
            }
            Expect::Refused => {
                assert!(!app.has_card(), "{name}: a card for a refused line");
                let said = message(&app);
                assert!(
                    said.starts_with("refused: ") || said.contains("gathers several"),
                    "{name}: {said}"
                );
                // The message row shows it.
                let first = panel(&screen(&app, 80, 24))[0].clone();
                let lead: String = said.chars().take(12).collect();
                assert!(first.starts_with(&format!(" {lead}")), "{name}: {first}");
                refusals += 1;
            }
            Expect::Actionable => {
                let id = id.clone().unwrap();
                assert!(
                    app.staged().is_some(),
                    "{name}: d opened no card: {}",
                    message(&app)
                );
                press(&mut app, KeyCode::Right);
                clean_both(&app, &words, &format!("leaves {name}"));
                if text(&app, 80, 24).contains("┌ What leaves with") {
                    press(&mut app, KeyCode::Esc);
                }
                let before = entries(&f).len();
                press(&mut app, KeyCode::Enter);
                let after = entries(&f);
                assert_eq!(after.len(), before + 1, "{name}: one new journal entry");
                let entry = after[0].clone();
                assert!(entry.undone_at.is_none(), "{name}");
                assert!(entry.ops.iter().any(|op| op.item_id == id), "{name}");
                let (g, path) = app.screen().find(&id).expect("its row");
                let off = app.screen().row(g, &path).unwrap();
                assert_eq!(off.state, State::Off, "{name}: the row flips to off");
                // Worded as live rows are: never the journal's raw scope.
                assert!(
                    !["user", "local", "managed", "claudeai", "builtin", "dynamic"]
                        .contains(&off.source.as_str()),
                    "{name}: source {:?}",
                    off.source
                );
                // Still resolved (a hook the switch keeps, a denied agent): the same words.
                if model::walk(app.tree()).any(|i| i.id == id) {
                    assert_eq!(Some(&off.source), source.as_ref(), "{name}");
                }
                clean_both(&app, &words, &format!("applied {name}"));
                press(&mut app, KeyCode::Char('w'));
                clean_both(&app, &words, &format!("w on the off row {name}"));
                if app.has_card() {
                    press(&mut app, KeyCode::Esc);
                }
                press(&mut app, KeyCode::Char('o'));
                assert!(
                    app.has_card(),
                    "{name}: o opened no card: {}",
                    message(&app)
                );
                clean_both(&app, &words, &format!("o {name}"));
                if applied % 2 == 0 {
                    press(&mut app, KeyCode::Enter);
                    by_o += 1;
                } else {
                    press(&mut app, KeyCode::Esc);
                    press(&mut app, KeyCode::Char('u'));
                    assert!(app.has_card(), "{name}: u asked nothing");
                    clean_both(&app, &words, &format!("u {name}"));
                    press(&mut app, KeyCode::Char('y'));
                    by_u += 1;
                }
                clean_both(&app, &words, &format!("restored {name}"));
                let now = entries(&f);
                let undone = now.iter().find(|e| e.id == entry.id).unwrap();
                assert!(undone.undone_at.is_some(), "{name}: that entry is undone");
                assert_eq!(app.rows().len(), total, "{name}: every line is back");
                applied += 1;
            }
        }
        assert!(
            !app.should_quit(),
            "{name}: a key meant for a card reached the list"
        );
    }
    let count = |k: Expect| expect.iter().filter(|e| **e == k).count();
    let (groups, actionable, refused) = (
        count(Expect::Group),
        count(Expect::Actionable),
        count(Expect::Refused),
    );
    eprintln!(
        "traversal: {total} lines · {groups} groups · {actionable} actionable · {refused} \
         refused · {applied} applied and undone ({by_o} by o, {by_u} by u) · {refusals} \
         refusals seen"
    );
    assert_eq!(applied, actionable);
    assert_eq!(refusals, refused);
    assert_eq!(actionable + refused, total - groups);
    assert!(by_o > 0 && by_u > 0);
    assert!(entries(&f).iter().all(|e| e.undone_at.is_some()));
}

// ---- lifecycle --------------------------------------------------------------------------------

/// Bytes of every file under `dir`, by path.
fn snapshot(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out: Vec<(PathBuf, Vec<u8>)> = walkdir::WalkDir::new(dir)
        .into_iter()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().is_file())
        .map(|e| (e.path().to_path_buf(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort();
    out
}

#[test]
fn readonly_is_said_in_the_header_before_any_key() {
    let f = fixture();
    let app = app_with(&f, true);
    let n = app.screen().title.items;
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let first = screen(&app, width, height)[0].clone();
        assert!(
            first.starts_with(&format!(
                " project · Claude Code · read-only · {n} items load"
            )) || first.starts_with(&format!(" project · read-only · {n} items load")),
            "{width}: {first}"
        );
    }
    let wide = screen(&app, 120, 40)[0].clone();
    assert!(
        wide.starts_with(" project · Claude Code") && wide.contains(" · read-only · "),
        "{wide}"
    );
    // Not when writes are allowed.
    assert!(!screen(&app_with(&f, false), 120, 40)[0].contains("read-only"));
}

#[test]
fn readonly_refuses_every_write_key_and_greys_it() {
    let f = fixture();
    let before = snapshot(&f.root);
    let mut app = app_with(&f, true);
    assert!(app.select(ALPHA));
    for key in ['d', ' ', 'o', 'u', 'e'] {
        press(&mut app, KeyCode::Char(key));
        let m = app.message().expect("a message").clone();
        assert_eq!(m.tone, Tone::Error);
        assert!(m.text().starts_with("readonly:"), "{key}: {}", m.text());
    }
    // The refusal takes the panel's first line; the detail loses whole sentences from its end,
    // never a sentence cut with `…`.
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(
        panel(&screen(&app, 80, 24)),
        [
            " readonly: d would change files; harn was started with --readonly",
            " alpha · plugin from the market-one marketplace · enabled for every project",
            " Brings 2 skills, 1 MCP server and 1 hook.",
            " d turns it off for this project only (.claude/settings.local.json).",
        ]
    );
    assert!(!text(&app, 80, 24).contains('…'));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('y'));
    assert!(app.staged().is_none());
    assert!(app.take_pending().is_none());
    assert!(!f.root.join("state").exists(), "no journal dir created");
    assert_eq!(
        snapshot(&f.root),
        before,
        "every fixture file byte for byte"
    );

    let buffer = draw(&app, 120, 40);
    let bar = lines(&buffer)[39].clone();
    let at = |words: &str| bar[..bar.find(words).unwrap()].chars().count() as u16;
    assert!(
        buffer[(at("d turn off"), 39)]
            .modifier
            .contains(Modifier::DIM),
        "{bar}"
    );
    assert!(
        !buffer[(at("w where from"), 39)]
            .modifier
            .contains(Modifier::DIM),
        "{bar}"
    );
}

#[test]
fn refused_items_say_why_without_ids() {
    let f = fixture();
    let mut app = app(&f);
    let prompt = model::walk(app.tree())
        .find(|i| i.kind == Kind::SystemPrompt)
        .unwrap()
        .id
        .clone();
    assert!(app.select(&prompt));
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(
        message(&app),
        "refused: system prompt (built-in) cannot be turned off: it is part of Claude Code"
    );
    assert!(app.staged().is_none());
    press(&mut app, KeyCode::Char('e'));
    assert!(message(&app).starts_with("refused: "), "{}", message(&app));
    assert!(app.take_pending().is_none());
    // `d` on a group points into it.
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(
        message(&app),
        "d turns off one thing: open Memory (1) (→) and pick one"
    );
}

#[test]
fn e_hands_the_editor_to_the_terminal_owner_and_journals_the_edit() {
    let f = fixture();
    let mut app = app(&f);
    let rule = model::walk(app.tree())
        .find(|i| i.label == ".claude/rules/style.md")
        .unwrap()
        .id
        .clone();
    assert!(app.select(&rule));
    press(&mut app, KeyCode::Char('e'));
    let Some(Pending::Edit { item_id }) = app.take_pending() else {
        panic!("no pending edit");
    };
    assert_eq!(item_id, rule);
    let editor = f.root.join("append.sh");
    std::fs::write(&editor, "#!/bin/sh\nprintf 'added\\n' >> \"$1\"\n").unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.edit(&item_id, editor.to_str().unwrap());
    let entries = journal::history(&journal(&f)).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].context.capture_kind, KIND_EDIT);
    assert_eq!(message(&app), "edited .claude/rules/style.md · u undoes");
    assert_eq!(app.cursor_id().as_deref(), Some(rule.as_str()));
    press(&mut app, KeyCode::Char('u'));
    let confirm = text(&app, 80, 24);
    assert!(
        confirm.contains("the edit of .claude/rules/style.md is reversed"),
        "{confirm}"
    );
}

#[test]
fn edit_called_directly_under_readonly_refuses_and_journals_nothing() {
    let f = fixture();
    let mut app = app_with(&f, true);
    let rule = model::walk(app.tree())
        .find(|i| i.label == ".claude/rules/style.md")
        .unwrap()
        .clone();
    let before = std::fs::read(rule.origin.path.as_ref().unwrap()).unwrap();
    let editor = f.root.join("append.sh");
    std::fs::write(&editor, "#!/bin/sh\nprintf 'added\\n' >> \"$1\"\n").unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.edit(&rule.id, editor.to_str().unwrap());
    let m = app.message().unwrap().clone();
    assert_eq!(m.tone, Tone::Error);
    assert!(m.text().starts_with("readonly:"), "{}", m.text());
    assert_eq!(
        std::fs::read(rule.origin.path.as_ref().unwrap()).unwrap(),
        before
    );
    assert!(journal::history(&journal(&f)).unwrap().is_empty());
    assert!(!f.root.join("state").exists());
}

#[test]
fn u_undoes_only_the_entry_it_confirmed() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    let a = journal::history(&journal(&f)).unwrap()[0].id.clone();
    press(&mut app, KeyCode::Char('u'));
    assert!(text(&app, 100, 30).contains("alpha comes back on"));
    // Another harn appends B before the user answers.
    let t = target(&f);
    let fresh = tree(&f);
    let plan = plan::build_plan(&ClaudeCode, &t, &fresh, BETA, None).unwrap();
    let b = writers::apply(&plan, &mut journal(&f)).unwrap().id;
    let after_b = std::fs::read(local_settings(&f)).unwrap();

    press(&mut app, KeyCode::Char('y'));
    let m = app.message().unwrap().clone();
    assert_eq!(m.tone, Tone::Error);
    assert_eq!(
        m.text(),
        "the journal changed since you confirmed; press u again to see the newest change"
    );
    assert_eq!(
        std::fs::read(local_settings(&f)).unwrap(),
        after_b,
        "B untouched"
    );
    let entries = journal::history(&journal(&f)).unwrap();
    assert!(
        entries.iter().all(|e| e.undone_at.is_none()),
        "A and B live"
    );
    assert!(entries.iter().any(|e| e.id == a) && entries.iter().any(|e| e.id == b));
    // Asked again, `u` names B now.
    press(&mut app, KeyCode::Char('u'));
    assert!(text(&app, 100, 30).contains("beta comes back on"));
}

#[test]
fn the_o_card_words_the_files_it_restores_by_their_count() {
    // One change writing two files: the alpha server's switch in ~/.claude.json, proj-agent's in
    // .claude/settings.local.json.
    let f = fixture();
    let t = tree(&f);
    let id = |kind: Kind, suffix: &str| {
        model::walk(&t)
            .find(|i| i.kind == kind && i.id.ends_with(suffix))
            .unwrap()
            .id
            .clone()
    };
    let (server, agent) = (
        id(Kind::McpServer, "/plugin:alpha:alpha-server"),
        id(Kind::SubAgent, "/proj-agent"),
    );
    let op = |selector: &str, knob: Option<usize>| PlanOp {
        selector: selector.to_string(),
        scope: None,
        knob,
        reason: "test".to_string(),
    };
    let request = PlanRequest {
        schema: PLAN_SCHEMA.to_string(),
        target_dir: f.project.clone(),
        ops: vec![op(&server, Some(1)), op(&agent, None)],
    };
    let plan = plan::build_plan_from_request(&ClaudeCode, &target(&f), &t, &request).unwrap();
    let entry = writers::apply(&plan, &mut journal(&f)).unwrap();
    assert_eq!(entry.snapshots.len(), 2);
    let mut app = app_on(&f, tree(&f), false, true);
    assert!(app.select(&agent));
    press(&mut app, KeyCode::Char('o'));
    let inside = card(&screen(&app, 80, 24), "Turn proj-agent back on")
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        inside.contains("~/.claude.json, .claude/settings.local.json back as they were before"),
        "{inside}"
    );
}

#[test]
fn o_on_an_off_row_opens_the_back_on_card_and_enter_turns_it_on() {
    let f = fixture();
    let before = std::fs::read(local_settings(&f)).unwrap();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.cursor_id().as_deref(), Some(ALPHA), "the off row");
    assert!(screen(&app, 80, 24)[23].contains("o turn back on"));
    let off = files(&f);
    press(&mut app, KeyCode::Char('o'));
    assert_eq!(files(&f), off, "o wrote before Enter");
    let inside = card(&screen(&app, 80, 24), "Turn alpha back on");
    assert_eq!(
        inside[..3],
        [
            " Writes   .claude/settings.local.json back as it was before harn's change",
            " Returns  alpha (next session)",
            " Undo     d turns it off again",
        ]
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(std::fs::read(local_settings(&f)).unwrap(), before);
    assert!(model::walk(app.tree()).any(|i| i.id == ALPHA));
    assert_eq!(
        app.cursor_id().as_deref(),
        Some(ALPHA),
        "the cursor stays on it"
    );
    assert_eq!(
        message(&app),
        "alpha is back on · takes effect next session"
    );
}

#[test]
fn apply_and_undo_keep_the_cursor_on_the_same_item() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(BETA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert!(model::walk(app.tree()).all(|i| i.id != BETA), "applied");
    assert_eq!(app.cursor_id().as_deref(), Some(BETA), "after apply");
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Char('y'));
    assert!(model::walk(app.tree()).any(|i| i.id == BETA), "undone");
    assert_eq!(app.cursor_id().as_deref(), Some(BETA), "after undo");
}

#[test]
fn cursor_moves_folds_and_jumps() {
    let f = fixture();
    let mut app = app(&f);
    let groups = app.rows().len();
    assert_eq!(groups, 8, "groups folded at start");
    assert_eq!(app.cursor_name().as_deref(), Some("Memory (1)"));
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.cursor_name().as_deref(), Some("Project files (4)"));
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(
        app.cursor_name().as_deref(),
        Some("Memory (1)"),
        "k stops at the top"
    );
    press(&mut app, KeyCode::Char('G'));
    assert_eq!(app.cursor_name().as_deref(), Some("Built-in (7)"));
    assert!(app.select_group("Skill listing"));
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.rows()[3].0, 1, "an owner follows the opened listing");
    assert!(text(&app, 80, 24).contains(" ▾ Skill listing"));
    // Onto a child, `h` goes back to the group; `h` again folds it; Enter toggles.
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Left);
    assert_eq!(app.cursor_name().as_deref(), Some("Skill listing"));
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(app.rows().len(), groups, "folded again");
    press(&mut app, KeyCode::Enter);
    assert!(app.rows().len() > groups, "Enter toggles");
}

#[test]
fn filter_shows_matches_with_their_group_and_esc_restores() {
    let f = fixture();
    let mut app = app(&f);
    press(&mut app, KeyCode::Char('/'));
    keys(&mut app, "beta-rev");
    assert_eq!(
        app.rows(),
        [
            (0, "Agent listing".to_string()),
            (1, "beta".to_string()),
            (2, "beta-reviewer".to_string()),
            (0, "Plugins (2)".to_string()),
            (1, "beta".to_string()),
            (2, "beta-reviewer".to_string()),
        ]
    );
    assert_eq!(app.cursor_name().as_deref(), Some("beta-reviewer"));
    // What is drawn: exactly these lines, in this order, nothing else in the list.
    let frame = screen(&app, 80, 24);
    let drawn: Vec<String> = list_rows(&frame.join("\n"))
        .iter()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let starts = [
        " ▾ Agent listing  10 entries",
        "   ▾ beta ",
        "       beta-reviewer ",
        " ▾ Plugins (2)  0.3 KB would leave with them · counted in the listings above",
        "   ▾ beta ",
        "       beta-reviewer ",
    ];
    assert_eq!(drawn.len(), starts.len(), "{drawn:#?}");
    for (line, start) in drawn.iter().zip(starts) {
        assert!(line.starts_with(start), "{line:?} is not {start:?}");
    }
    assert!(frame[23].starts_with(" /beta-rev_"), "{}", frame[23]);
    let footer = frame
        .iter()
        .find(|l| l.contains(" shown "))
        .expect("a footer while a filter hides lines");
    assert!(footer.contains("filter: \"beta-rev\""), "{footer}");
    // Without a filter the footer says only what the frame hides.
    press(&mut app, KeyCode::Esc);
    open_all(&mut app);
    let frame = screen(&app, 80, 24);
    let footer = frame.iter().find(|l| l.contains(" shown ")).unwrap();
    assert!(footer.starts_with("── 83 items · 14 of "), "{footer}");
    assert!(footer.contains(" lines shown ──"), "{footer}");
    assert!(!footer.contains("filter"), "{footer}");
    press(&mut app, KeyCode::Char('/'));
    keys(&mut app, "beta-rev");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter(), "beta-rev");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.filter(), "");
    assert!(!app.should_quit(), "Esc clears the filter first");
    assert_eq!(
        app.cursor_name().as_deref(),
        Some("beta-reviewer"),
        "kept in place"
    );
    press(&mut app, KeyCode::Char('/'));
    keys(&mut app, "zzz");
    assert!(text(&app, 80, 24).contains("nothing matches \"zzz\""));
}

#[test]
fn s_toggles_size_and_name_order() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select_group("Plugins"));
    press(&mut app, KeyCode::Right);
    let names =
        |app: &App| -> Vec<String> { app.rows()[5..7].iter().map(|r| r.1.clone()).collect() };
    // The order drawn, not only the order kept.
    let drawn = |app: &App| -> Vec<String> {
        screen(app, 80, 24)[8..10]
            .iter()
            .map(|l| l.split_whitespace().nth(1).unwrap_or_default().to_string())
            .collect()
    };
    assert_eq!(names(&app), ["beta", "alpha"]);
    assert_eq!(
        drawn(&app),
        ["beta", "alpha"],
        "by size: 0.2 KB, then 0.1 KB"
    );
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.sort(), Sort::Name);
    assert_eq!(names(&app), ["alpha", "beta"]);
    assert_eq!(drawn(&app), ["alpha", "beta"]);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.sort(), Sort::Size);
}

#[test]
fn tiny_frames_draw_without_panic() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    let states: [&[char]; 4] = [&[], &['d'], &['w'], &['?']];
    for keys in states {
        for key in keys {
            press(&mut app, KeyCode::Char(*key));
        }
        for width in 0..=14 {
            for height in 0..=8 {
                let frame = screen(&app, width, height);
                assert_eq!(frame.len(), usize::from(height));
            }
        }
        press(&mut app, KeyCode::Esc);
    }
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('u'));
    // A `u` card taller than the frame is not drawn: the message row says what it needs and
    // neither Enter nor `y` undoes what the reader cannot see.
    let short = screen(&app, 80, 8);
    assert!(
        short
            .iter()
            .any(|l| l.starts_with(" terminal too small for this card; needs at least 40×10")),
        "{}",
        short.join("\n")
    );
    assert!(!short.join("\n").contains("Enter undo"));
    for key in [KeyCode::Enter, KeyCode::Char('y')] {
        press(&mut app, key);
        assert!(journal::history(&journal(&f)).unwrap()[0]
            .undone_at
            .is_none());
        assert!(app.has_card(), "still asking");
    }
    assert!(text(&app, 80, 24).contains("Enter undo · Esc keep"));
    press(&mut app, KeyCode::Char('y'));
    assert!(journal::history(&journal(&f)).unwrap()[0]
        .undone_at
        .is_some());
}

#[test]
fn a_card_too_big_for_the_frame_is_not_drawn_and_enter_writes_nothing() {
    let f = fixture();
    let before = files(&f);
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    // 30 columns: under the 40 a card needs. Not drawn; one message row, cut at a word.
    let small = screen(&app, 30, 12);
    let all = small.join("\n");
    assert!(!all.contains("Turn off alpha"), "not drawn:\n{all}");
    assert_eq!(panel(&small)[0], " terminal too small for this", "{all}");
    assert!(small[11].starts_with(" Esc close"), "{all}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(files(&f), before, "Enter applied a card nobody saw");
    assert!(app.staged().is_some(), "the card is still open");
    // Wide enough to say it whole: the size it needs.
    let short = screen(&app, 80, 9);
    assert_eq!(
        panel(&short)[0],
        " terminal too small for this card; needs at least 40×10",
        "{}",
        short.join("\n")
    );
    // 40×12 passes the minimum, but at 40 columns the card's body wraps past 12 rows: still
    // not drawn, and Enter still writes nothing.
    let narrow = screen(&app, 40, 12);
    assert!(
        panel(&narrow)[0].starts_with(" terminal too small"),
        "{}",
        narrow.join("\n")
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!(files(&f), before);
    card(&screen(&app, 40, 20), "Turn off alpha for this project");
    // Drawn whole at 80×24: Enter applies.
    card(&screen(&app, 80, 24), "Turn off alpha for this project");
    press(&mut app, KeyCode::Enter);
    let applied = files(&f);
    assert_ne!(applied, before, "applied once drawn");
    // The `o` card too.
    press(&mut app, KeyCode::Char('o'));
    assert!(app.has_card());
    let small = screen(&app, 30, 12);
    assert!(!small.join("\n").contains("back on ─"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(files(&f), applied, "o applied unseen");
    press(&mut app, KeyCode::Esc);
    assert!(!app.has_card());
    assert_eq!(message(&app), "cancelled: nothing written");
}

#[test]
fn a_message_takes_one_row_of_whole_sentences_and_the_detail_keeps_the_rest() {
    let f = fixture();
    let mut app = app(&f);
    // Three sentences; two fit in 79 columns.
    app.report_error(
        "The first sentence is here. The second one fits as well. The third sentence is far too \
         long to fit on the same row."
            .to_string(),
    );
    let rows = panel(&screen(&app, 80, 24));
    assert_eq!(
        rows,
        [
            " The first sentence is here. The second one fits as well.",
            " Memory · notes Claude Code keeps for this directory",
            " MEMORY.md (auto-memory) loads in every session here, up to its cap.",
            " Open it (→); d turns auto-memory off for this project only.",
        ]
    );
    // One sentence longer than the row: cut at a word, no ellipsis, still one row.
    let long = format!("cannot read the journal: {}", "a-long-word ".repeat(12));
    app.report_error(long.trim_end().to_string());
    let rows = panel(&screen(&app, 80, 24));
    assert_eq!(
        rows[0],
        " cannot read the journal: a-long-word a-long-word a-long-word a-long-word"
    );
    assert!(!rows[0].ends_with('…'));
    assert_eq!(
        rows[1], " Memory · notes Claude Code keeps for this directory",
        "a detail sentence below it"
    );
}

#[test]
fn bars_stay_put_when_the_largest_line_scrolls_out_of_view() {
    let f = fixture();
    let mut app = app(&f);
    open_all(&mut app);
    // The two largest lines are the first two: the Memory group and MEMORY.md, 13 KB each.
    let rows = app.rows();
    assert_eq!(rows[0].1, "Memory (1)");
    assert_eq!(rows[1].1, "MEMORY.md (auto-memory)");
    let top = screen(&app, 80, 24);
    // 14 list lines at 80×24: the cursor on line 15 scrolls both out of view.
    for _ in 0..15 {
        press(&mut app, KeyCode::Char('j'));
    }
    let scrolled = screen(&app, 80, 24);
    assert!(
        !scrolled.iter().any(|l| l.contains("MEMORY.md")),
        "{}",
        scrolled.join("\n")
    );
    // Lines 2..=13 are in both frames: the same text, bars included.
    assert_eq!(top[3 + 2..3 + 14], scrolled[3..3 + 12]);
    assert!(
        scrolled[3..15]
            .iter()
            .any(|l| l.contains('█') || l.contains('▍')),
        "some bar in view"
    );
}

#[test]
fn short_screens_keep_the_cursor_row_visible() {
    let f = fixture();
    let mut app = app(&f);
    open_all(&mut app);
    for height in 10..=16 {
        for key in ['g', 'j', 'j', 'G', 'k'] {
            press(&mut app, KeyCode::Char(key));
            let buffer = draw(&app, 80, height);
            let line = cursor_line(&buffer).unwrap_or_else(|| {
                panic!("{height}: no cursor row:\n{}", lines(&buffer).join("\n"))
            });
            let name = app.cursor_name().unwrap();
            let tail: String = name
                .chars()
                .skip(name.chars().count().saturating_sub(12))
                .collect();
            assert!(line.contains(&tail), "{height}: {name} not in {line}");
        }
    }
}

#[test]
fn a_long_name_is_cut_on_the_left_and_keeps_its_tail() {
    let f = fixture();
    let mut t = tree(&f);
    let long = "x".repeat(110) + "-the-tail";
    t.items
        .iter_mut()
        .find(|i| i.id == ALPHA)
        .unwrap()
        .origin
        .plugin = Some(format!("{long}@market-one"));
    let mut app = app_on(&f, t, false, true);
    assert!(app.select_group("Plugins"));
    press(&mut app, KeyCode::Right);
    for width in [80u16, 120] {
        let frame = screen(&app, width, 24);
        let line = frame.iter().find(|l| l.contains("-the-tail")).unwrap();
        assert!(line.contains('…'), "{width}: {line}");
        assert_eq!(line.chars().count(), usize::from(width));
    }
}

#[test]
fn trim_hint_refresh_and_quit() {
    let f = fixture();
    let mut app = app(&f);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(message(&app), TRIM_HINT);
    press(&mut app, KeyCode::Char('r'));
    assert!(message(&app).starts_with("refreshed"), "{}", message(&app));
    press(&mut app, KeyCode::Char('q'));
    assert!(app.should_quit());
}

/// Every item of `plugin` (`name@market`), at any depth, renamed `to`.
fn rename_plugin(items: &mut [model::Item], plugin: &str, to: &str) {
    for item in items {
        if item.origin.plugin.as_deref() == Some(plugin) {
            item.origin.plugin = Some(to.to_string());
        }
        rename_plugin(&mut item.children, plugin, to);
    }
}

#[test]
fn a_wide_cjk_name_is_cut_on_the_left_and_keeps_its_tail_its_marker_and_the_width() {
    let f = fixture();
    let mut t = tree(&f);
    let wide = format!("{}尾部", "技能一覧".repeat(12)); // 50 chars, 100 columns
    rename_plugin(
        &mut t.items,
        "alpha@market-one",
        &format!("{wide}@market-one"),
    );
    let mut app = app_on(&f, t, false, true);
    assert!(app.select_group("Plugins"));
    press(&mut app, KeyCode::Right);
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let frame = screen(&app, width, height);
        let row = frame
            .iter()
            .find(|l| l.contains("尾部"))
            .unwrap_or_else(|| panic!("{width}: no wide row:\n{}", frame.join("\n")));
        assert!(row.contains('…'), "{width}: cut: {row}");
        assert!(row.contains("一覧尾部"), "{width}: its tail: {row}");
        assert!(!row.contains(&wide), "{width}: {row}");
        assert!(row.starts_with("   ▸ …"), "{width}: its marker: {row}");
        assert!(row.contains("0.1 KB"), "{width}: its size: {row}");
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(row.as_str()),
            usize::from(width),
            "{width}: {row}"
        );
    }
}

#[test]
fn a_wide_cjk_off_row_draws_at_every_width_from_1_to_12() {
    let f = fixture();
    // A journal op for an item whose id names `漢字`: built from a tree where alpha has that id,
    // applied for real (alpha turns off). The fresh tree has no such item: an off row.
    let t = target(&f);
    let mut renamed = tree(&f);
    let wide_id = "claude-code/plugin/plugin/漢字";
    renamed.items.iter_mut().find(|i| i.id == ALPHA).unwrap().id = wide_id.to_string();
    let plan = plan::build_plan(&ClaudeCode, &t, &renamed, wide_id, None).unwrap();
    writers::apply(&plan, &mut journal(&f)).unwrap();
    let mut app = app(&f);
    assert!(app.select(wide_id), "the off row");
    assert_eq!(app.cursor_name().as_deref(), Some("漢字"));
    for width in 1..=12u16 {
        let buffer = draw(&app, width, 12);
        let rows = lines(&buffer);
        let reversed: Vec<u16> = (3..12)
            .filter(|&y| (0..width).any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED)))
            .collect();
        assert_eq!(reversed.len(), 1, "{width}: {rows:#?}");
        let y = reversed[0];
        assert!(
            buffer[(0, y)].modifier.contains(Modifier::REVERSED),
            "{width}: the cursor row from its first cell: {rows:#?}"
        );
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(rows[usize::from(y)].as_str()),
            usize::from(width),
            "{width}: {rows:#?}"
        );
    }
}

#[test]
fn a_label_equal_to_another_items_id_is_shown_as_it_is() {
    let f = fixture();
    let mut t = tree(&f);
    let prompt = t
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SystemPrompt)
        .unwrap();
    prompt.label = ALPHA.to_string();
    let id = prompt.id.clone();
    let mut app = app_on(&f, t, false, true);
    assert!(app.select(&id));
    press(&mut app, KeyCode::Char('d'));
    // Not rewritten into `alpha`, the item that id belongs to.
    assert_eq!(
        message(&app),
        format!("refused: {ALPHA} cannot be turned off: it is part of Claude Code")
    );
    assert_eq!(
        app.message().unwrap().parts[1],
        Part::Item {
            id,
            name: ALPHA.to_string()
        }
    );
}

#[test]
fn ids_sharing_a_prefix_each_resolve_to_their_own_name() {
    let f = fixture();
    let mut t = tree(&f);
    let one = "claude-code/skill/user/user-one";
    let longer = format!("{one}-two");
    // user-two's id now starts with user-one's.
    let listing = t
        .items
        .iter_mut()
        .find(|i| i.kind == Kind::SkillListing)
        .unwrap();
    let two = listing
        .children
        .iter_mut()
        .find(|c| c.label == "user-two")
        .unwrap();
    two.id = longer.clone();
    let op = |selector: &str| PlanOp {
        selector: selector.to_string(),
        scope: None,
        knob: None,
        reason: "test".to_string(),
    };
    let request = PlanRequest {
        schema: PLAN_SCHEMA.to_string(),
        target_dir: f.project.clone(),
        ops: vec![op(one), op(&longer)],
    };
    let plan = plan::build_plan_from_request(&ClaudeCode, &target(&f), &t, &request).unwrap();
    writers::apply(&plan, &mut journal(&f)).unwrap();
    let mut app = app_on(&f, t, false, true);
    press(&mut app, KeyCode::Char('u'));
    let confirm = text(&app, 100, 30);
    assert!(confirm.contains("user-one comes back on"), "{confirm}");
    assert!(confirm.contains("user-two comes back on"), "{confirm}");
    assert!(
        !confirm.contains("user-one comes back on\n  user-one"),
        "{confirm}"
    );
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(
        message(&app),
        "undone: user-one, user-two back · .claude/settings.local.json restored byte for byte"
    );
    let items: Vec<&Part> = app
        .message()
        .unwrap()
        .parts
        .iter()
        .filter(|p| matches!(p, Part::Item { .. }))
        .collect();
    assert_eq!(
        items,
        [
            &Part::Item {
                id: one.to_string(),
                name: "user-one".to_string()
            },
            &Part::Item {
                id: longer,
                name: "user-two".to_string()
            },
        ]
    );
}

#[test]
fn o_says_why_when_the_journal_cannot_turn_it_back_on() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.cursor_id().as_deref(), Some(ALPHA), "the off row");
    // The entry is gone from disk (pruned by another harn): a message, no card.
    let entries = f.root.join("state/journal");
    let aside = f.root.join("aside");
    std::fs::rename(&entries, &aside).unwrap();
    press(&mut app, KeyCode::Char('o'));
    assert!(!app.has_card());
    assert_eq!(message(&app), "no journal entry turns alpha back on");
    // An entry that does not read: the journal's error, no card.
    std::fs::rename(&aside, &entries).unwrap();
    let entry = std::fs::read_dir(&entries)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("entry.json");
    let good = std::fs::read(&entry).unwrap();
    std::fs::write(&entry, b"{ not json").unwrap();
    press(&mut app, KeyCode::Char('o'));
    assert!(!app.has_card());
    assert!(
        message(&app).starts_with("cannot read the journal: "),
        "{}",
        message(&app)
    );
    assert_eq!(app.message().unwrap().tone, Tone::Error);
    // Readable again: the card.
    std::fs::write(&entry, good).unwrap();
    press(&mut app, KeyCode::Char('o'));
    assert!(app.has_card());
    card(&screen(&app, 80, 24), "Turn alpha back on");
}

#[test]
fn turning_off_a_hook_turns_off_every_hook_and_the_screen_says_so() {
    let f = fixture();
    let mut app = app(&f);
    let id_of = |app: &App, label: &str| {
        model::walk(app.tree())
            .find(|i| i.label.starts_with(label))
            .unwrap()
            .id
            .clone()
    };
    let id = id_of(&app, "PreToolUse hook");
    assert!(app.select(&id));
    let hook_rows = |app: &App, width: u16, height: u16| -> Vec<String> {
        screen(app, width, height)
            .into_iter()
            // List rows (indented), not the panel's sentence.
            .filter(|l| l.starts_with("     "))
            .filter(|l| l.contains("PreToolUse hook") || l.contains("SessionStart hook"))
            .map(|l| l.trim_end().to_string())
            .collect()
    };
    let on = [hook_rows(&app, 80, 24), hook_rows(&app, 120, 40)];
    assert_eq!(on[0].len(), 2, "{:?}", on[0]);
    // The card names what the switch really does before what it writes.
    press(&mut app, KeyCode::Char('d'));
    let inside = card(&screen(&app, 80, 24), "Turn off all hooks for this project");
    assert_eq!(
        inside[..3],
        [
            " PreToolUse hook has no switch of its own; Claude Code turns hooks off all at",
            " once.",
            " Writes   .claude/settings.local.json → disableAllHooks = true",
        ]
    );
    let inside = card(
        &screen(&app, 120, 40),
        "Turn off all hooks for this project",
    );
    // Every hook stays in the tree once the switch is on; what leaves comes from the switch:
    // the other hooks on it and the status line. The injecting ones make the saving unmeasured.
    assert!(
        inside
            .iter()
            .any(|l| l.starts_with(" Leaves   2 hooks, 1 status line (next session)")),
        "{inside:#?}"
    );
    // The count at start is one phrase: it starts the next line rather than split.
    assert!(
        inside.join("\n").contains(
            " Saves    an amount only `harn verify --run` can measure\n          · 2 fewer items at start\n"
        ),
        "{inside:#?}"
    );
    let predicted = fewer_on_card(&inside);
    press(&mut app, KeyCode::Right);
    let mut list = card(&screen(&app, 120, 40), "What leaves with PreToolUse hook");
    list.pop();
    assert_eq!(
        list,
        [
            " SessionStart (hook)",
            " UserPromptSubmit (hook)",
            " statusLine (status line — not a row above; it stops with the hooks)",
        ]
    );
    press(&mut app, KeyCode::Esc);
    let before = app.screen().title.items;
    // Applied: every hook stays listed, each saying it went off with the others; the group
    // keeps its count.
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        message(&app),
        format!(
            "turned off all hooks · 2 fewer items load at start ({before} → {}) · u undoes · \
             takes effect next session",
            before - 2
        ),
        "SessionStart and the alpha plugin's UserPromptSubmit printed into the request"
    );
    assert_eq!(app.screen().title.items, before - 2);
    assert_eq!(predicted, 2, "the card said what the header shows");
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let frame = screen(&app, width, height);
        let all = frame.join("\n");
        let group = frame.iter().find(|l| l.starts_with(" ▾ Hooks")).unwrap();
        assert_eq!(
            group.trim_end(),
            " ▾ Hooks (2)  all off for this project · u undoes",
            "{all}"
        );
        let rows = hook_rows(&app, width, height);
        assert_eq!(rows.len(), 2, "{all}");
        for row in &rows {
            // The same source words as when it was on, and the whole status, never cut.
            assert!(row.contains("  ~/.claude "), "{row}");
            assert!(
                row.ends_with("—  off, with every other hook"),
                "{width}: {row}"
            );
        }
    }
    // The message row keeps whole clauses; the detail follows it.
    let detail = panel(&screen(&app, 80, 24));
    assert_eq!(
        detail[0],
        format!(
            " turned off all hooks · 2 fewer items load at start ({before} → {}) · u undoes",
            before - 2
        )
    );
    assert_eq!(
        format!("{} {}", detail[1], detail[2].trim_start()),
        " PreToolUse hook · turned off with every other hook · u undoes · takes effect next session"
    );
    // `o` on the other hook reverses the same change.
    let other = id_of(&app, "SessionStart hook");
    assert!(app.select(&other));
    press(&mut app, KeyCode::Char('o'));
    card(&screen(&app, 80, 24), "Turn all hooks back on");
    press(&mut app, KeyCode::Esc);
    // Undone: every hook is back with the words it had.
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        message(&app),
        "undone: all hooks back on · .claude/settings.local.json restored byte for byte"
    );
    let frame = screen(&app, 80, 24);
    let group = frame.iter().find(|l| l.starts_with(" ▾ Hooks")).unwrap();
    assert!(group.starts_with(" ▾ Hooks (2) "), "{}", frame.join("\n"));
    assert_eq!([hook_rows(&app, 80, 24), hook_rows(&app, 120, 40)], on);
}

#[test]
fn hooks_your_own_settings_turned_off_are_listed_and_not_offered_to_o() {
    let f = fixture();
    let file = f.project.join(".claude/settings.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    json["disableAllHooks"] = serde_json::json!(true);
    std::fs::write(&file, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    let mut app = app(&f);
    let id = model::walk(app.tree())
        .find(|i| i.label.starts_with("SessionStart hook"))
        .unwrap()
        .id
        .clone();
    assert!(app.select(&id));
    let frame = screen(&app, 80, 24);
    let all = frame.join("\n");
    let group = frame.iter().find(|l| l.starts_with(" ▾ Hooks")).unwrap();
    assert_eq!(group.trim_end(), " ▾ Hooks (2)  all off here", "{all}");
    let rows: Vec<&String> = frame
        .iter()
        .filter(|l| l.starts_with("     ") && l.contains(" hook "))
        .collect();
    assert_eq!(rows.len(), 2, "{all}");
    assert!(rows
        .iter()
        .all(|r| r.trim_end().ends_with("—  off, with every other hook")));
    // harn did not make the change: no `o` in the key bar, and the keys say why.
    assert!(!frame[23].contains("turn back on"), "{}", frame[23]);
    assert_eq!(
        panel(&frame)[0],
        " SessionStart hook · off with every other hook"
    );
    press(&mut app, KeyCode::Char('o'));
    assert!(!app.has_card());
    assert!(message(&app).contains("did not turn"), "{}", message(&app));
    press(&mut app, KeyCode::Char('d'));
    assert!(!app.has_card());
    assert_eq!(
        message(&app),
        "SessionStart hook is already off: your settings turn every hook off (disableAllHooks)"
    );
}

#[test]
fn a_server_turned_off_that_stays_listed_says_off_and_why() {
    let f = fixture();
    let mut app = app(&f);
    let id = model::walk(app.tree())
        .find(|i| i.kind == model::Kind::McpServer && i.id.ends_with("/project-server"))
        .unwrap()
        .id
        .clone();
    assert!(app.select(&id));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    // The resolver keeps a disabled server (mode none): its row stays, greyed, saying `off`.
    let item = model::walk(app.tree()).find(|i| i.id == id).unwrap();
    assert_eq!(item.mode, model::Mode::None);
    let (g, path) = app.screen().find(&id).unwrap();
    let row = app.screen().row(g, &path).unwrap();
    assert_eq!(row.state, State::Off);
    assert_eq!(row.absent, Some("off"));
    let frame = screen(&app, 80, 24);
    let line = frame
        .iter()
        .find(|l| l.starts_with("     project-server"))
        .unwrap();
    assert!(line.trim_end().ends_with("—  off"), "{line}");
    assert!(
        panel(&frame)
            .join(" ")
            .contains("it stays listed, and none of it loads."),
        "{}",
        frame.join("\n")
    );
}

/// Rows from a box's top border to its bottom border, with whatever is drawn beside the box on
/// each: the text before its left border and after its right border.
fn beside_box(frame: &[String]) -> Vec<(String, String)> {
    let top = frame.iter().position(|l| l.contains('┌')).expect("a box");
    let bottom = top + frame[top..].iter().position(|l| l.contains('└')).unwrap();
    frame[top..=bottom]
        .iter()
        .map(|row| {
            let left = row.find(['┌', '│', '└']).unwrap();
            let right = row.rfind(['┐', '│', '┘']).unwrap();
            let after = right + row[right..].chars().next().unwrap().len_utf8();
            (row[..left].to_string(), row[after..].to_string())
        })
        .collect()
}

#[test]
fn a_card_blanks_the_full_width_of_the_rows_it_takes() {
    let f = fixture();
    let mut app = app(&f);
    // Every group open: list text runs under every row a card can take.
    open_all(&mut app);
    assert!(app.select(ALPHA));
    // At 120×40 an 80-column card sits over a 120-column list: no half-words beside it.
    for key in ['w', 'd', '?', 'i'] {
        press(&mut app, KeyCode::Char(key));
        let frame = screen(&app, 120, 40);
        let (top, bottom) = (
            frame.iter().position(|l| l.contains('┌')).unwrap(),
            frame.iter().rposition(|l| l.contains('└')).unwrap(),
        );
        assert!(
            bottom < 34,
            "{key}: the card is over the list, not the panel"
        );
        assert!(top > 3, "{key}");
        for (before, after) in beside_box(&frame) {
            assert!(
                before.trim().is_empty() && after.trim().is_empty(),
                "{key}: {before:?} | {after:?}\n{}",
                frame.join("\n")
            );
        }
        press(&mut app, KeyCode::Esc);
    }
}

#[test]
fn the_undo_card_blanks_its_rows_across_the_frame() {
    let f = fixture();
    let mut app = app(&f);
    open_all(&mut app);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('u'));
    // An 80-column card in 120: the list's markers must not show beside it.
    let frame = screen(&app, 120, 40);
    let rows = beside_box(&frame);
    assert!(rows.len() >= 5, "{}", frame.join("\n"));
    for (before, after) in rows {
        assert!(
            before.trim().is_empty() && after.trim().is_empty(),
            "{before:?} | {after:?}\n{}",
            frame.join("\n")
        );
        assert!(!before.contains(['▸', '▾']) && !after.contains(['▸', '▾']));
    }
}

#[test]
fn an_inline_command_is_never_split_and_a_small_share_is_never_zero() {
    let f = fixture();
    let mut app = app(&f);
    open_all(&mut app);
    let total = app.rows().len();
    press(&mut app, KeyCode::Char('g'));
    let (mut commands, mut small) = (0, 0);
    for _ in 0..total {
        for (width, height) in [(80u16, 24u16), (120, 40)] {
            let frame = screen(&app, width, height);
            for row in panel(&frame) {
                // A backtick span opens and closes on the same row.
                assert_eq!(row.matches('`').count() % 2, 0, "{width}: {row}");
                commands += row.matches("`harn verify --run`").count();
            }
            for row in list_rows(&frame.join("\n")) {
                let row = row.trim_end();
                // A size above zero never reads as 0% of the total.
                if row.ends_with(" 0%") {
                    assert!(
                        row.contains(" 0 B ") || !row.contains(" KB "),
                        "{width}: {row}"
                    );
                }
                small += usize::from(row.ends_with("<1%"));
            }
        }
        press(&mut app, KeyCode::Char('j'));
    }
    assert!(commands > 0, "some panel names `harn verify --run`");
    assert!(small > 0, "some row is under a percent");
}

/// The left column and the width of the box whose top border holds `title`.
fn box_at(frame: &[String], title: &str) -> (usize, usize) {
    let top = frame
        .iter()
        .find(|l| l.contains(&format!("┌ {title} ")))
        .unwrap_or_else(|| panic!("no box {title}:\n{}", frame.join("\n")));
    let left = top.find('┌').unwrap();
    let right = top.rfind('┐').unwrap();
    (
        top[..left].chars().count(),
        top[left..right].chars().count() + 1,
    )
}

#[test]
fn the_undo_card_is_drawn_and_keyed_like_every_other_card() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        press(&mut app, KeyCode::Char('d'));
        let off = box_at(
            &screen(&app, width, height),
            "Turn off alpha for this project",
        );
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('u'));
        let frame = screen(&app, width, height);
        // Same column and width as the `d` card, the body indented as on it, the answer on
        // the bottom right, the key bar in words.
        assert_eq!(box_at(&frame, "Undo the last change"), off, "{width}");
        let inside = card(&frame, "Undo the last change");
        assert_eq!(inside[0], " alpha comes back on");
        assert_eq!(inside.last().unwrap().trim(), "Enter undo · Esc keep");
        assert!(inside.last().unwrap().starts_with("   "));
        assert_eq!(
            frame[usize::from(height) - 1].trim_end(),
            " Enter undo it   Esc keep it"
        );
        // Other keys do nothing; `n` keeps, as Esc does; `y` undoes, as Enter does.
        press(&mut app, KeyCode::Char('x'));
        assert!(app.has_card());
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.has_card());
        assert_eq!(message(&app), "cancelled: nothing written");
        press(&mut app, KeyCode::Char('u'));
        press(&mut app, KeyCode::Char('y'));
        assert!(journal::history(&journal(&f)).unwrap()[0]
            .undone_at
            .is_some());
        assert!(app.select(ALPHA));
    }
}

/// The `N` of `· N fewer item(s) at start` on a card's Saves line (wrapped lines joined).
fn fewer_on_card(lines: &[String]) -> usize {
    let text = lines.iter().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
    let at = text
        .find(" fewer item")
        .unwrap_or_else(|| panic!("no count on the card: {text}"));
    text[..at]
        .rsplit(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no number before `fewer`: {text}"))
}

/// A card's body lines (its answer line left out).
fn card_body(frame: &[String], title: &str) -> Vec<String> {
    let mut lines = card(frame, title);
    lines.pop();
    lines
}

#[test]
fn informational_cards_scroll_when_their_body_is_taller_than_the_frame() {
    let f = fixture();
    let words = forbidden(&f);
    let mut app = app(&f);
    // The notes at 80 columns, the keys at 60 (one key per line there): taller than a 10-row
    // frame's card.
    for (key, title, width) in [('i', "Notes", 80u16), ('?', "Keys", 60)] {
        press(&mut app, KeyCode::Char(key));
        let tall = screen(&app, width, 40);
        let whole = card_body(&tall, title);
        assert!(
            !tall[39].contains("scroll"),
            "{title}: it all fits, nothing to scroll: {}",
            tall[39]
        );
        assert!(tall[39].contains("Esc close"));
        let last = whole.last().unwrap().clone();
        let small = screen(&app, width, 10);
        let shown = card_body(&small, title);
        assert!(shown.len() < whole.len(), "{title}: {shown:#?}");
        assert_eq!(shown[..], whole[..shown.len()], "{title}: the top first");
        assert!(!shown.contains(&last), "{title}: the last line is below");
        assert!(small[9].contains("scroll"), "{title}: {}", small[9]);
        assert_clean(&small.join("\n"), &words, title);
        // ↓ reveals the last line; past the end it stays put; ↑ goes back.
        let hidden = whole.len() - shown.len();
        for _ in 0..hidden {
            press(&mut app, KeyCode::Down);
        }
        let end = card_body(&screen(&app, width, 10), title);
        assert_eq!(end[..], whole[hidden..], "{title}");
        assert!(end.contains(&last));
        press(&mut app, KeyCode::Down);
        assert_eq!(card_body(&screen(&app, width, 10), title), end, "clamped");
        for _ in 0..hidden {
            press(&mut app, KeyCode::Up);
        }
        let back = screen(&app, width, 10);
        assert_eq!(card_body(&back, title), shown, "{title}: back at the top");
        assert_clean(&back.join("\n"), &words, title);
        press(&mut app, KeyCode::Esc);
        assert!(!app.has_card());
    }
    // The keys at 80×10 fit in three columns: no scroll offered.
    press(&mut app, KeyCode::Char('?'));
    let frame = screen(&app, 80, 10);
    assert_eq!(card_body(&frame, "Keys").len(), 5, "{}", frame.join("\n"));
    assert_eq!(frame[9].trim_end(), " Esc close");
}

#[test]
fn o_reverses_only_the_entry_its_card_showed() {
    let f = fixture();
    let t = target(&f);
    // Two live entries turn alpha off: A, then B (planned from the tree before A).
    let before_a = tree(&f);
    let plan_a = plan::build_plan(&ClaudeCode, &t, &before_a, ALPHA, None).unwrap();
    let a = writers::apply(&plan_a, &mut journal(&f)).unwrap().id;
    let plan_b = plan::build_plan(&ClaudeCode, &t, &before_a, ALPHA, None).unwrap();
    let b = writers::apply(&plan_b, &mut journal(&f)).unwrap().id;
    let mut app = app(&f);
    assert!(app.select(ALPHA), "the off row");
    press(&mut app, KeyCode::Char('o'));
    assert!(app.has_card(), "{}", message(&app));
    // The card shows B; B is undone elsewhere before Enter.
    journal::undo_entry(&mut journal(&f), &b).unwrap();
    let now = files(&f);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        files(&f),
        now,
        "Enter reversed a change the card did not show"
    );
    let m = app.message().unwrap().clone();
    assert_eq!(m.text(), "that change was already undone; r refreshes");
    assert_eq!(m.tone, Tone::Error);
    let live: Vec<String> = journal::history(&journal(&f))
        .unwrap()
        .into_iter()
        .filter(|e| e.undone_at.is_none())
        .map(|e| e.id)
        .collect();
    assert_eq!(live, [a], "A untouched");
}

#[test]
fn o_refuses_an_entry_with_no_file_to_restore() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    let id = journal::history(&journal(&f)).unwrap()[0].id.clone();
    // The entry on disk lists no snapshot.
    let dir = f.root.join("state/journal");
    let entry = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path().join("entry.json"))
        .find(|p| std::fs::read_to_string(p).unwrap().contains(&id))
        .unwrap();
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&entry).unwrap()).unwrap();
    json["snapshots"] = serde_json::json!([]);
    std::fs::write(&entry, serde_json::to_vec(&json).unwrap()).unwrap();
    let before = files(&f);
    press(&mut app, KeyCode::Char('o'));
    assert!(!app.has_card(), "no card for nothing to restore");
    assert_eq!(
        message(&app),
        format!("journal entry {id} has no file to restore")
    );
    assert_eq!(files(&f), before);
}

#[test]
fn library_text_shows_its_paths_home_and_project_relative() {
    let f = fixture();
    let words = forbidden(&f);
    let mut app = app(&f);
    app.report_error(format!(
        "error: cannot open {}/.claude/CLAUDE.md: permission denied",
        f.home.display()
    ));
    assert_eq!(
        message(&app),
        "error: cannot open ~/.claude/CLAUDE.md: permission denied"
    );
    clean_both(&app, &words, "a library error naming a home path");
    app.report_error(format!(
        "error: {}/.claude/settings.local.json is not valid JSON",
        f.project.display()
    ));
    assert_eq!(
        message(&app),
        "error: .claude/settings.local.json is not valid JSON"
    );
    clean_both(&app, &words, "a library error naming a project path");
}
