//! The interactive view on a copy of the zbrain-like fixture, driven by key events and drawn on a
//! ratatui `TestBackend` (no terminal): first frames, folding, filter, sort, why, stage → apply
//! → undo / on through a journal in the copy, `--readonly`.

mod common;

use common::{fixture, target, Fixture};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use harn::journal::{self, Journal, KIND_EDIT};
use harn::model::{self, Tree};
use harn::plan;
use harn::render::{self, RenderOpts};
use harn::resolve::{ClaudeCode, Resolver};
use harn::tui::{App, Opts, Pending, SortKey, Tone, TRIM_HINT};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use ratatui::Terminal;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const ALPHA: &str = "claude-code/plugin/plugin/alpha@market-one";
const LISTING: &str = "claude-code/skill-listing/builtin/skills";

/// Resolved and freed-annotated, as `harn` does before the table or the view.
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

fn app_with(f: &Fixture, readonly: bool) -> App {
    App::new(
        target(f),
        vec![(Box::new(ClaudeCode) as Box<dyn Resolver>, tree(f))],
        journal(f),
        Opts {
            unicode: true,
            color: false,
            readonly,
        },
    )
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

fn lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
    lines(&draw(app, width, height))
}

fn local_settings(f: &Fixture) -> PathBuf {
    f.project.join(".claude/settings.local.json")
}

fn labels(app: &App) -> Vec<String> {
    app.rows().into_iter().map(|(_, label)| label).collect()
}

/// Index of the first row whose label starts with `label`.
fn position(app: &App, label: &str) -> usize {
    labels(app)
        .iter()
        .position(|l| l.starts_with(label))
        .unwrap_or_else(|| panic!("no row {label}: {:?}", labels(app)))
}

fn message(app: &App) -> String {
    app.message().map(|m| m.text.clone()).unwrap_or_default()
}

/// `d` on the alpha plugin, `a`, `y`: applied through the journal.
fn turn_alpha_off(app: &mut App) {
    assert!(app.select(ALPHA));
    press(app, KeyCode::Char('d'));
    press(app, KeyCode::Char('a'));
    press(app, KeyCode::Char('y'));
}

#[test]
fn first_frame_at_80x24_folds_children_in_table_order_with_the_key_bar_last() {
    let f = fixture();
    let app = app(&f);
    let frame = screen(&app, 80, 24);
    if std::env::var_os("HARN_TUI_DUMP").is_some() {
        eprintln!("{}", frame.join("\n"));
    }
    let text = frame.join("\n");
    assert!(frame[0].starts_with("harn "), "{text}");
    assert!(
        text.contains("first request: predicted") && text.contains("sort: freed ▼"),
        "{text}"
    );
    assert!(text.contains("tool search: OFF"), "{text}");
    assert!(
        frame.iter().any(|l| l.starts_with(" claude-code ")),
        "{text}"
    );
    let columns = frame
        .iter()
        .position(|l| l.contains("freed") && l.contains("predicted") && l.contains("origin"))
        .unwrap_or_else(|| panic!("no column header:\n{text}"));

    // Rows: every top-level item, folded, in the table's order.
    let rows = app.rows();
    assert!(rows.iter().all(|(depth, _)| *depth == 0), "{rows:?}");
    let table = render::table(
        app.tree(),
        &RenderOpts {
            unicode: true,
            color: false,
            width: Some(400),
        },
    );
    let mut from = 0;
    for (_, label) in &rows {
        let probe: String = label.chars().take(30).collect();
        let at = table[from..]
            .find(&probe)
            .unwrap_or_else(|| panic!("{label} out of table order:\n{table}"));
        from += at + probe.len();
    }
    let first: String = rows[0].1.chars().take(20).collect();
    assert!(frame[columns + 1].contains(&first), "{text}");

    // The listing's 29 skills fold under it.
    let listing = frame
        .iter()
        .find(|l| l.contains("skill listing ·"))
        .unwrap_or_else(|| panic!("no listing row:\n{text}"));
    assert!(listing.contains("▸ 29"), "{text}");
    let at = position(&app, "skill listing");
    assert_eq!(
        rows[at + 1].0,
        0,
        "the row after the folded listing is top-level"
    );

    // 80 columns: no bar, the item column keeps a short path whole.
    assert!(!text.contains('█'), "{text}");
    assert!(text.contains(".claude/rules/style.md"), "{text}");
    assert!(
        frame[23].contains("? help") && frame[23].contains("q quit"),
        "{text}"
    );
    assert!(frame[22].contains("rows shown of"), "{text}");
    assert_eq!(frame.len(), 24);
    assert!(frame.iter().all(|l| l.chars().count() <= 80));
}

#[test]
fn first_frame_at_120x40_has_share_bars_and_the_mode_column() {
    let f = fixture();
    let app = app(&f);
    let frame = screen(&app, 120, 40);
    let text = frame.join("\n");
    assert!(text.contains('█'), "{text}");
    let columns = frame
        .iter()
        .find(|l| l.contains("freed") && l.contains("origin"))
        .unwrap_or_else(|| panic!("no column header:\n{text}"));
    assert!(
        columns.contains("share") && columns.contains("mode"),
        "{text}"
    );
    assert!(text.contains("advertised"), "{text}");
    assert!(frame[39].contains("? help"), "{text}");
    assert!(frame.iter().all(|l| l.chars().count() <= 120));
}

#[test]
fn cursor_moves_folds_and_jumps() {
    let f = fixture();
    let mut app = app(&f);
    let rows = app.rows();
    let first = app.cursor_id().unwrap();
    press(&mut app, KeyCode::Char('j'));
    assert_ne!(app.cursor_id().unwrap(), first);
    press(&mut app, KeyCode::Up);
    assert_eq!(app.cursor_id().unwrap(), first);
    press(&mut app, KeyCode::Char('G'));
    let last_label = &rows.last().unwrap().1;
    let last = model::walk(app.tree())
        .find(|i| &i.label == last_label)
        .unwrap();
    assert_eq!(app.cursor_id().unwrap(), last.id);
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(app.cursor_id().unwrap(), first);

    assert!(app.select(LISTING));
    let at = position(&app, "skill listing");
    press(&mut app, KeyCode::Char('l'));
    let rows = app.rows();
    assert_eq!(rows[at + 1].0, 1, "expanded: a skill follows its listing");
    let children = rows[at + 1..].iter().take_while(|(d, _)| *d == 1).count();
    assert_eq!(children, 29);
    let frame = screen(&app, 120, 40).join("\n");
    assert!(frame.contains("▾ 29"), "{frame}");
    // Onto a child, `h` goes back to the parent; `h` again folds it.
    press(&mut app, KeyCode::Char('j'));
    assert_ne!(app.cursor_id().unwrap(), LISTING);
    press(&mut app, KeyCode::Left);
    assert_eq!(app.cursor_id().unwrap(), LISTING);
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(app.rows()[at + 1].0, 0, "folded again");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.rows()[at + 1].0, 1, "Enter toggles");
}

#[test]
fn filter_keeps_the_parent_unfolds_matches_and_esc_restores() {
    let f = fixture();
    let mut app = app(&f);
    let before = app.rows();
    press(&mut app, KeyCode::Char('/'));
    keys(&mut app, "beta");
    let rows = app.rows();
    let labels: Vec<&str> = rows.iter().map(|(_, l)| l.as_str()).collect();
    let roster = labels
        .iter()
        .position(|l| l.starts_with("agent roster"))
        .unwrap_or_else(|| panic!("the roster parent stays: {labels:?}"));
    assert_eq!(rows[roster].0, 0);
    assert!(rows[roster + 1..]
        .iter()
        .take_while(|(depth, _)| *depth == 1)
        .any(|(_, l)| l == "beta:beta-agent"));
    assert!(labels.contains(&"beta@market-two"), "{labels:?}");
    assert!(!labels.contains(&"proj-agent"), "{labels:?}");
    assert!(!labels.contains(&"MEMORY.md"), "{labels:?}");
    let frame = screen(&app, 100, 30).join("\n");
    assert!(frame.contains("/beta_"), "{frame}");

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter(), "beta");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.filter(), "");
    assert_eq!(app.rows(), before, "folds restored");
    assert!(!app.should_quit());
    press(&mut app, KeyCode::Esc);
    assert!(app.should_quit(), "Esc with nothing open quits");
}

#[test]
fn s_cycles_freed_cost_name_and_reverses_each() {
    let f = fixture();
    let mut app = app(&f);
    let freed = labels(&app);
    let main = position(&app, "statusLine").min(freed.len());
    let trailing_first = |app: &App| {
        let l = labels(app);
        let user_md = l.iter().position(|x| x == "~/.claude/CLAUDE.md").unwrap();
        let status = l.iter().position(|x| x == "statusLine").unwrap();
        assert!(
            status > user_md,
            "runtime/none rows stay after the main block: {l:?}"
        );
    };

    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.sort().key, SortKey::Freed);
    assert!(app.sort().reversed);
    trailing_first(&app);
    let reversed = labels(&app);
    let mut expected_head: Vec<String> = freed.clone();
    expected_head.truncate(main);
    // The main block, reversed, is still a contiguous head.
    let n = reversed
        .iter()
        .position(|l| !expected_head.contains(l))
        .unwrap();
    let mut head = reversed[..n].to_vec();
    head.reverse();
    let mut original: Vec<String> = freed[..n].to_vec();
    original.sort();
    let mut sorted_head = head.clone();
    sorted_head.sort();
    assert_eq!(sorted_head, original);
    assert_eq!(
        head,
        freed[..n].to_vec(),
        "exact reverse of the freed order"
    );

    press(&mut app, KeyCode::Char('s'));
    assert_eq!(
        (app.sort().key, app.sort().reversed),
        (SortKey::Cost, false)
    );
    let memory = model::walk(app.tree())
        .filter(|i| i.kind == model::Kind::Memory)
        .map(|i| i.label.clone())
        .next()
        .unwrap();
    assert_eq!(labels(&app)[0], memory, "the biggest request bytes first");
    trailing_first(&app);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!((app.sort().key, app.sort().reversed), (SortKey::Cost, true));
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(
        (app.sort().key, app.sort().reversed),
        (SortKey::Name, false)
    );
    let by_name = labels(&app);
    let head: Vec<String> = by_name[..5].to_vec();
    let mut sorted: Vec<String> = head.clone();
    sorted.sort_by_key(|l| l.to_lowercase());
    assert_eq!(head, sorted);
    assert!(screen(&app, 100, 30).join("\n").contains("sort: name ▲"));
    press(&mut app, KeyCode::Char('s'));
    assert_eq!((app.sort().key, app.sort().reversed), (SortKey::Name, true));
    assert!(screen(&app, 100, 30).join("\n").contains("sort: name ▼"));
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(labels(&app), freed, "back to the table order");
}

#[test]
fn d_on_a_plugin_stages_a_plan_whose_side_effects_list_its_skills() {
    let f = fixture();
    let before = std::fs::read(local_settings(&f)).unwrap();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    let staged = app.staged().expect("staged");
    assert_eq!(staged.item_id, ALPHA);
    let effects = staged.plan.side_effects.join("\n");
    assert!(
        effects.contains("alpha:alpha-skill") && effects.contains("alpha:alpha-other"),
        "{effects}"
    );
    assert!(
        staged.plan.diff.contains("alpha@market-one"),
        "{}",
        staged.plan.diff
    );
    let frame = screen(&app, 120, 40).join("\n");
    assert!(
        frame.contains("staged plan") && frame.contains("side effects"),
        "{frame}"
    );
    assert!(message(&app).starts_with("staged alpha@market-one off"));
    // Staging writes nothing; `d` again unstages.
    assert_eq!(std::fs::read(local_settings(&f)).unwrap(), before);
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('d'));
    assert!(app.staged().is_none());
}

#[test]
fn a_applies_through_the_journal_and_u_restores_the_bytes() {
    let f = fixture();
    let before = std::fs::read(local_settings(&f)).unwrap();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('a'));
    let frame = screen(&app, 100, 30).join("\n");
    assert!(
        frame.contains("confirm") && frame.contains("Apply: turn off alpha@market-one?"),
        "{frame}"
    );
    press(&mut app, KeyCode::Char('y'));

    let entries = journal::history(&journal(&f)).unwrap();
    assert_eq!(entries.len(), 1);
    let id = &entries[0].id;
    let text = message(&app);
    assert!(
        text.contains(id.as_str()) && text.contains("u undoes"),
        "{text}"
    );
    assert!(app.staged().is_none());
    assert_ne!(std::fs::read(local_settings(&f)).unwrap(), before);
    // Re-resolved: the plugin left the tree and is listed as turned off here.
    assert!(model::walk(app.tree()).all(|i| i.id != ALPHA));
    assert!(labels(&app).contains(&ALPHA.to_string()));
    press(&mut app, KeyCode::Char('G'));
    assert_eq!(
        app.cursor_id().as_deref(),
        Some(ALPHA),
        "the off row is last"
    );
    let frame = screen(&app, 120, 40).join("\n");
    assert!(frame.contains("turned off by harn here"), "{frame}");
    assert!(
        frame.contains("o on"),
        "the key bar offers o on the off row: {frame}"
    );

    press(&mut app, KeyCode::Char('u'));
    let frame = screen(&app, 100, 30).join("\n");
    assert!(frame.contains(&format!("Undo journal {id}?")), "{frame}");
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(std::fs::read(local_settings(&f)).unwrap(), before);
    assert!(
        message(&app).starts_with("undone: journal"),
        "{}",
        message(&app)
    );
    let entries = journal::history(&journal(&f)).unwrap();
    assert!(entries[0].undone_at.is_some());
    assert!(model::walk(app.tree()).any(|i| i.id == ALPHA));
}

#[test]
fn apply_and_undo_keep_the_cursor_on_the_same_item_id() {
    let f = fixture();
    let mut app = app(&f);
    // Stored after the plugin: its index path shifts when the plugin's rows leave and return.
    let last = app.tree().items.last().unwrap().id.clone();
    let alpha_at = app.tree().items.iter().position(|i| i.id == ALPHA).unwrap();
    let last_at = app.tree().items.len() - 1;
    assert!(alpha_at < last_at);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Esc);
    assert!(app.select(&last));
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Char('y'));
    assert!(model::walk(app.tree()).all(|i| i.id != ALPHA), "applied");
    assert_eq!(
        app.cursor_id().as_deref(),
        Some(last.as_str()),
        "after apply"
    );
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Char('y'));
    assert!(model::walk(app.tree()).any(|i| i.id == ALPHA), "undone");
    assert_eq!(
        app.cursor_id().as_deref(),
        Some(last.as_str()),
        "after undo"
    );
}

#[test]
fn o_on_an_off_row_turns_it_back_on() {
    let f = fixture();
    let before = std::fs::read(local_settings(&f)).unwrap();
    let mut app = app(&f);
    turn_alpha_off(&mut app);
    assert!(app.select(ALPHA), "the off row");
    press(&mut app, KeyCode::Char('o'));
    assert!(screen(&app, 100, 30).join("\n").contains("back on?"));
    press(&mut app, KeyCode::Char('y'));
    assert_eq!(std::fs::read(local_settings(&f)).unwrap(), before);
    assert!(model::walk(app.tree()).any(|i| i.id == ALPHA));
    assert_eq!(
        app.cursor_id().as_deref(),
        Some(ALPHA),
        "the cursor stays on it"
    );
    assert!(message(&app).starts_with("on: "), "{}", message(&app));
}

#[test]
fn n_cancels_and_writes_nothing() {
    let f = fixture();
    let before = std::fs::read(local_settings(&f)).unwrap();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(std::fs::read(local_settings(&f)).unwrap(), before);
    assert!(app.staged().is_some(), "still staged");
    assert!(journal::history(&journal(&f)).unwrap().is_empty());
}

#[test]
fn readonly_refuses_every_write_key_and_greys_it() {
    let f = fixture();
    let mut app = app_with(&f, true);
    assert!(app.select(ALPHA));
    for key in ['d', ' ', 'a', 'o', 'u', 'e'] {
        press(&mut app, KeyCode::Char(key));
        let m = app.message().expect("a message").clone();
        assert_eq!(m.tone, Tone::Error);
        assert!(m.text.starts_with("readonly:"), "{key}: {}", m.text);
    }
    assert!(app.staged().is_none());
    assert!(app.take_pending().is_none());
    assert!(!f.root.join("state").exists(), "no journal dir created");

    let buffer = draw(&app, 120, 40);
    let bar = lines(&buffer)[39].clone();
    let x = bar.find("d off").unwrap_or_else(|| panic!("{bar}")) as u16;
    assert!(buffer[(x, 39)].modifier.contains(Modifier::DIM), "{bar}");
    let x = bar.find("w why").unwrap() as u16;
    assert!(!buffer[(x, 39)].modifier.contains(Modifier::DIM), "{bar}");
}

#[test]
fn refused_items_say_why_in_the_message_line() {
    let f = fixture();
    let mut app = app(&f);
    let prompt = model::walk(app.tree())
        .find(|i| i.kind == model::Kind::SystemPrompt)
        .unwrap()
        .id
        .clone();
    assert!(app.select(&prompt));
    press(&mut app, KeyCode::Char('d'));
    let m = app.message().unwrap().clone();
    assert!(m.text.starts_with("refused: "), "{}", m.text);
    assert!(app.staged().is_none());
    press(&mut app, KeyCode::Char('e'));
    assert!(message(&app).starts_with("refused: "), "{}", message(&app));
    assert!(app.take_pending().is_none());
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
    assert!(
        message(&app).contains(entries[0].id.as_str()),
        "{}",
        message(&app)
    );
    assert_eq!(app.cursor_id().as_deref(), Some(rule.as_str()));
}

#[test]
fn why_trim_help_and_quit() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('w'));
    let frame = screen(&app, 120, 40).join("\n");
    assert!(
        frame.contains(" why ") && frame.contains("why loaded"),
        "{frame}"
    );
    assert!(frame.contains("esc close"), "{frame}");
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.should_quit(), "q closes the pane first");

    press(&mut app, KeyCode::Char('t'));
    assert_eq!(message(&app), TRIM_HINT);

    press(&mut app, KeyCode::Char('?'));
    let frame = screen(&app, 120, 40).join("\n");
    assert!(
        frame.contains(" keys ") && frame.contains("stage off"),
        "{frame}"
    );
    press(&mut app, KeyCode::Esc);
    assert!(!app.should_quit());
    press(&mut app, KeyCode::Char('q'));
    assert!(app.should_quit());
}

#[test]
fn ascii_frame_has_no_unicode_glyphs() {
    let f = fixture();
    let mut app = App::new(
        target(&f),
        vec![(Box::new(ClaudeCode) as Box<dyn Resolver>, tree(&f))],
        journal(&f),
        Opts {
            unicode: false,
            color: false,
            readonly: false,
        },
    );
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Esc);
    let text = screen(&app, 120, 40).join("\n");
    assert!(text.contains(" > 29"), "{text}");
    assert!(text.contains('#'), "{text}");
    assert!(text.contains("sort: freed v"), "{text}");
    assert!(text.contains("x "), "the staged row's mark: {text}");
    // Labels come from the resolver; the glyphs harn draws are ASCII.
    for glyph in ['▸', '▾', '█', '▏', '▼', '▲', '✗', '…', '→'] {
        assert!(!text.contains(glyph), "{glyph}: {text}");
    }
}
