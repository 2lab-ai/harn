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
use harn::writers;
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

/// The last 24 chars of `label` (all of it when shorter): what a cut row still shows.
fn tail(label: &str) -> String {
    let n = label.chars().count();
    label.chars().skip(n.saturating_sub(24)).collect()
}

/// The label of the row with `id`: the item's, or the id's last segment for an off row.
fn label_of(app: &App, id: &str) -> String {
    model::walk(app.tree())
        .find(|i| i.id == id)
        .map(|i| i.label.clone())
        .unwrap_or_else(|| id.splitn(4, '/').nth(3).unwrap().to_string())
}

/// The list lines of a frame: after the column header, before the message line, without
/// blanks and block titles.
fn list_lines(frame: &[String]) -> Vec<String> {
    let header = frame
        .iter()
        .position(|l| l.contains("freed") && l.contains("item"))
        .expect("a column header");
    frame[header + 1..frame.len() - 2]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !l.starts_with("  runtime /") && !l.starts_with("  turned off"))
        .cloned()
        .collect()
}

/// The list line drawn reversed (the cursor row; the agent tab above is reversed too).
fn cursor_line(buffer: &Buffer) -> Option<String> {
    let rows = lines(buffer);
    let header = rows
        .iter()
        .position(|l| l.contains("freed") && l.contains("item"))? as u16;
    (header + 1..buffer.area.height).find_map(|y| {
        let reversed =
            (0..buffer.area.width).any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED));
        reversed.then(|| rows[y as usize].clone())
    })
}

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
    // 80 columns: the item column wins; only freed · item · origin are left.
    let columns = frame
        .iter()
        .position(|l| l.contains("freed") && l.contains("item"))
        .unwrap_or_else(|| panic!("no column header:\n{text}"));
    let header: Vec<&str> = frame[columns].split_whitespace().collect();
    assert_eq!(header, ["freed", "item", "origin"], "{text}");

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
    assert!(frame[columns + 1].contains(&tail(&rows[0].1)), "{text}");

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
fn wider_frames_add_columns_back_once_the_item_column_is_whole() {
    let f = fixture();
    let app = app(&f);
    let frame = screen(&app, 120, 40);
    let text = frame.join("\n");
    let columns = frame
        .iter()
        .find(|l| l.contains("freed") && l.contains("item"))
        .unwrap_or_else(|| panic!("no column header:\n{text}"));
    let header: Vec<&str> = columns.split_whitespace().collect();
    assert_eq!(
        header,
        ["freed", "predicted", "share", "item", "origin", "mode"],
        "120: the bar is the first to go: {text}"
    );
    assert!(!text.contains('█'), "{text}");
    assert!(text.contains("memory/MEMORY.md (auto-memory)"), "{text}");
    assert!(frame[39].contains("? help"), "{text}");

    let wide = screen(&app, 160, 40).join("\n");
    assert!(wide.contains('█'), "160: room for the bar too: {wide}");
    assert!(wide.contains("advertised"), "{wide}");
}

#[test]
fn every_row_at_80x24_shows_the_tail_of_its_label_down_to_the_bottom() {
    let f = fixture();
    let mut app = app(&f);
    // Unfold everything: depth costs indent, and the long MEMORY.md path is a top row.
    let parents: Vec<String> = model::walk(app.tree())
        .filter(|i| !i.children.is_empty())
        .map(|i| i.children[0].id.clone())
        .collect();
    for child in &parents {
        assert!(app.select(child));
    }
    press(&mut app, KeyCode::Char('g'));
    let mut seen = 0;
    loop {
        let id = app.cursor_id().unwrap();
        let line = cursor_line(&draw(&app, 80, 24)).expect("a cursor line");
        let label = label_of(&app, &id);
        assert!(line.contains(&tail(&label)), "{label:?} not in {line:?}");
        seen += 1;
        press(&mut app, KeyCode::Char('j'));
        if app.cursor_id().unwrap() == id {
            break;
        }
    }
    assert_eq!(seen, app.rows().len(), "every row visited");
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('G'));
    let bottom = draw(&app, 80, 24);
    let label = label_of(&app, &app.cursor_id().unwrap());
    assert!(cursor_line(&bottom).unwrap().contains(&tail(&label)));
}

#[test]
fn w_shows_the_full_label_of_a_row_cut_from_the_left() {
    let f = fixture();
    let mut app = app(&f);
    let memory = model::walk(app.tree())
        .find(|i| i.kind == model::Kind::Memory)
        .unwrap()
        .clone();
    assert!(
        memory.label.chars().count() > 70,
        "a label the row must cut"
    );
    assert!(app.select(&memory.id));
    let row = cursor_line(&draw(&app, 80, 24)).unwrap();
    assert!(row.contains('…') && !row.contains(&memory.label), "{row}");
    assert!(row.contains("memory/MEMORY.md (auto-memory)"), "{row}");

    press(&mut app, KeyCode::Char('w'));
    let frame = screen(&app, 80, 24);
    let top = frame
        .iter()
        .position(|l| l.starts_with('┌') && l.contains(" why "))
        .unwrap_or_else(|| panic!("no why pane:\n{}", frame.join("\n")));
    // The first line of the pane, wrapped at the pane's width.
    let inner: String = frame[top + 1..top + 4]
        .iter()
        .map(|l| {
            let chars: Vec<char> = l.chars().collect();
            chars[1..chars.len() - 1].iter().collect::<String>()
        })
        .collect();
    assert!(inner.starts_with(&memory.label), "{inner}");
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
    press(&mut app, KeyCode::Down);
    assert_ne!(app.cursor_id().unwrap(), first);
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(app.cursor_id().unwrap(), first);
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(app.cursor_id().unwrap(), first, "k stops at the top");
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
    let frame = screen(&app, 100, 30);
    assert!(frame.join("\n").contains("/beta_"));
    // The rendered rows are exactly the beta items and the parent that holds two of them.
    let roster = &rows[roster].1;
    let expected = [
        "beta@market-two",
        roster.as_str(),
        "beta:beta-agent",
        "beta:beta-reviewer",
    ];
    let shown = list_lines(&frame);
    assert_eq!(shown.len(), expected.len(), "{shown:#?}");
    for (line, label) in shown.iter().zip(expected) {
        assert!(line.contains(label), "{label} not in {line}");
    }
    let unrelated: Vec<String> = model::walk(app.tree())
        .map(|i| i.label.clone())
        .filter(|l| !expected.contains(&l.as_str()) && l.chars().count() >= 4)
        .filter(|l| shown.iter().any(|line| line.contains(l.as_str())))
        .collect();
    assert!(unrelated.is_empty(), "{unrelated:?} in {shown:#?}");

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.filter(), "beta");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.filter(), "");
    assert_eq!(app.rows(), before, "folds restored");
    assert!(!app.should_quit());
    press(&mut app, KeyCode::Esc);
    assert!(app.should_quit(), "Esc with nothing open quits");
}

/// The request bytes a row sorts by under `cost` (observed, else predicted; `—` rows none).
fn request(item: &model::Item) -> Option<u64> {
    if item.cost.not_applicable {
        return None;
    }
    item.cost.observed.or(item.cost.predicted)
}

/// The table's freed key: freed.known with an applicable default knob, request bytes when
/// freed was not computed, 0 without a knob.
fn freed_key(item: &model::Item) -> u64 {
    match item.knobs.first() {
        Some(model::Knob::None { .. }) | None => 0,
        Some(_) => item
            .freed
            .as_ref()
            .map_or_else(|| request(item).unwrap_or(0), |f| f.known),
    }
}

/// Labels of `items` in a key's natural direction (stable), reversed on request.
fn expected_order(items: &[&model::Item], key: SortKey, reversed: bool) -> Vec<String> {
    let mut items = items.to_vec();
    match key {
        SortKey::Freed => items.sort_by_key(|i| std::cmp::Reverse(freed_key(i))),
        SortKey::Cost => items.sort_by_key(|i| std::cmp::Reverse(request(i))),
        SortKey::Name => items.sort_by_key(|i| i.label.to_lowercase()),
    }
    let mut labels: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
    if reversed {
        labels.reverse();
    }
    labels
}

#[test]
fn s_cycles_freed_cost_name_and_reverses_each() {
    let f = fixture();
    let mut app = app(&f);
    let tree = app.tree().clone();
    let trailing = |i: &&model::Item| matches!(i.mode, model::Mode::Runtime | model::Mode::None);
    let main: Vec<&model::Item> = tree.items.iter().filter(|i| !trailing(i)).collect();
    let tail_block: Vec<&model::Item> = tree.items.iter().filter(trailing).collect();
    let cycle = [
        (SortKey::Freed, false, "freed ▼"),
        (SortKey::Freed, true, "freed ▲"),
        (SortKey::Cost, false, "cost ▼"),
        (SortKey::Cost, true, "cost ▲"),
        (SortKey::Name, false, "name ▲"),
        (SortKey::Name, true, "name ▼"),
    ];
    for (n, (key, reversed, label)) in cycle.into_iter().enumerate() {
        if n > 0 {
            press(&mut app, KeyCode::Char('s'));
        }
        assert_eq!((app.sort().key, app.sort().reversed), (key, reversed));
        let got = labels(&app);
        let (got_main, got_tail) = got.split_at(main.len());
        assert_eq!(
            got_main,
            expected_order(&main, key, reversed),
            "{label}: main block"
        );
        assert_eq!(
            got_tail,
            expected_order(&tail_block, key, reversed),
            "{label}: runtime/none block"
        );
        // The frame draws that order (a 120×80 screen shows every row).
        let frame = screen(&app, 120, 80);
        assert!(frame.join("\n").contains(&format!("sort: {label}")));
        let shown = list_lines(&frame);
        assert_eq!(shown.len(), got.len(), "{label}");
        for (line, row) in shown.iter().zip(&got) {
            assert!(line.contains(&tail(row)), "{label}: {row} not in {line}");
        }
    }
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(
        labels(&app)[..main.len()],
        expected_order(&main, SortKey::Freed, false)[..],
        "back to freed ▼"
    );
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
fn the_plan_pane_shows_every_side_effect_at_80x24_and_120x40() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    let effects = app.staged().unwrap().plan.side_effects.clone();
    assert!(effects.len() >= 2, "{effects:?}");
    for (width, height) in [(80, 24), (120, 40)] {
        press(&mut app, KeyCode::Char('g'));
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..100 {
            let frame = screen(&app, width, height);
            let done = seen.last().is_some_and(|last| *last == frame.join("\n"));
            seen.push(frame.join("\n"));
            if done {
                break;
            }
            press(&mut app, KeyCode::Char('j'));
        }
        for effect in &effects {
            assert!(
                seen.iter().any(|frame| frame.contains(effect.as_str())),
                "{width}x{height}: {effect} never drawn"
            );
        }
    }
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
fn u_undoes_only_the_entry_it_confirmed() {
    let f = fixture();
    let mut app = app(&f);
    turn_alpha_off(&mut app);
    let a = journal::history(&journal(&f)).unwrap()[0].id.clone();
    press(&mut app, KeyCode::Char('u'));
    assert!(screen(&app, 100, 30)
        .join("\n")
        .contains(&format!("Undo journal {a}?")));
    // Another harn appends B before the user answers.
    let t = target(&f);
    let fresh = tree(&f);
    let beta = model::select(&fresh, "beta@market-two").unwrap().id.clone();
    let plan = plan::build_plan(&ClaudeCode, &t, &fresh, &beta, None).unwrap();
    let b = writers::apply(&plan, &mut journal(&f)).unwrap().id;
    let after_b = std::fs::read(local_settings(&f)).unwrap();

    press(&mut app, KeyCode::Char('y'));
    let m = app.message().unwrap().clone();
    assert_eq!(m.tone, Tone::Error);
    assert_eq!(
        m.text,
        format!("journal changed since you confirmed ({b}); press u again")
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
    assert!(screen(&app, 100, 30)
        .join("\n")
        .contains(&format!("Undo journal {b}?")));
}

#[test]
fn undo_from_the_off_row_keeps_the_cursor_on_the_restored_item() {
    let f = fixture();
    let mut app = app(&f);
    turn_alpha_off(&mut app);
    assert!(app.select(ALPHA), "the off row");
    assert!(model::walk(app.tree()).all(|i| i.id != ALPHA));
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Char('y'));
    assert!(model::walk(app.tree()).any(|i| i.id == ALPHA), "restored");
    assert_eq!(app.cursor_id().as_deref(), Some(ALPHA));
    let (depth, label) = app.rows()[position(&app, "alpha@market-one")].clone();
    assert_eq!(
        (depth, label.as_str()),
        (0, "alpha@market-one"),
        "an item row"
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
    assert!(m.text.starts_with("readonly:"), "{}", m.text);
    assert_eq!(
        std::fs::read(rule.origin.path.as_ref().unwrap()).unwrap(),
        before
    );
    assert!(journal::history(&journal(&f)).unwrap().is_empty());
    assert!(!f.root.join("state").exists());
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
    let before = snapshot(&f.root);
    let mut app = app_with(&f, true);
    assert!(app.select(ALPHA));
    for key in ['d', ' ', 'a', 'o', 'u', 'e'] {
        press(&mut app, KeyCode::Char(key));
        let m = app.message().expect("a message").clone();
        assert_eq!(m.tone, Tone::Error);
        assert!(m.text.starts_with("readonly:"), "{key}: {}", m.text);
    }
    // `y` after every refusal: nothing was asked, nothing confirms.
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
fn ascii_mode_draws_every_widget_in_ascii() {
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
    // Only a resolver label may carry non-ASCII; everything harn draws is ASCII.
    let mut labels: Vec<String> = model::walk(app.tree())
        .map(|i| i.label.clone())
        .filter(|l| !l.is_ascii())
        .collect();
    labels.sort_by_key(|l| std::cmp::Reverse(l.len()));
    let check = |app: &App, what: &str| {
        let frame = screen(app, 160, 40);
        for line in &frame {
            let mut rest = line.clone();
            for label in &labels {
                rest = rest.replace(label.as_str(), "");
            }
            assert!(rest.is_ascii(), "{what}: {line}");
        }
        frame.join("\n")
    };
    let list = check(&app, "list");
    assert!(list.contains(" > 29"), "{list}");
    assert!(list.contains('#'), "{list}");
    assert!(list.contains("sort: freed v"), "{list}");

    press(&mut app, KeyCode::Char('?'));
    let help = check(&app, "help");
    assert!(help.contains("+ keys -"), "{help}");
    press(&mut app, KeyCode::Esc);

    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('w'));
    let why = check(&app, "why");
    assert!(
        why.contains("+ why -") && why.contains("why loaded"),
        "{why}"
    );
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('d'));
    let plan = check(&app, "plan");
    assert!(
        plan.contains("+ staged plan -") && plan.contains("->"),
        "{plan}"
    );
    press(&mut app, KeyCode::Char('a'));
    let confirm = check(&app, "confirm");
    assert!(
        confirm.contains("+ confirm -") && confirm.contains("y = yes"),
        "{confirm}"
    );
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Esc);
    let staged = check(&app, "staged row");
    assert!(staged.contains("x "), "the staged row's mark: {staged}");
}

#[test]
fn tiny_frames_draw_without_panic_and_the_answer_line_stays() {
    let f = fixture();
    let mut app = app(&f);
    assert!(app.select(ALPHA));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('a'));
    for width in 0..=12 {
        for height in 0..=6 {
            // A panic here = a cell drawn outside the frame or an underflow.
            let frame = screen(&app, width, height);
            assert_eq!(frame.len(), usize::from(height));
        }
    }
    // A confirm taller than the frame keeps the y/N line and counts what it cut.
    let short = screen(&app, 80, 8).join("\n");
    assert!(short.contains("y = yes   n / Esc = no"), "{short}");
    assert!(short.contains(" more)"), "{short}");
    press(&mut app, KeyCode::Char('n'));
}

#[test]
fn short_screens_keep_the_cursor_row_visible() {
    let f = fixture();
    let mut app = app(&f);
    for height in 10..=16 {
        for key in ['g', 'j', 'j', 'G', 'k'] {
            press(&mut app, KeyCode::Char(key));
            let buffer = draw(&app, 80, height);
            let line = cursor_line(&buffer).unwrap_or_else(|| {
                panic!("{height}: no cursor row:\n{}", lines(&buffer).join("\n"))
            });
            let label = label_of(&app, &app.cursor_id().unwrap());
            assert!(
                line.contains(&tail(&label)),
                "{height}: {label} not in {line}"
            );
        }
    }
}
