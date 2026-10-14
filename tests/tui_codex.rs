//! The interactive view on a Codex tree (P0.5 C6), drawn on a ratatui `TestBackend`: one tab per
//! agent, the Codex header and notes, Codex group names and sentences, and `d` on a Codex row —
//! the card names the TOML file and key, Enter writes it through the journal, `u` restores the
//! bytes. The fixture is built per test in a temp dir (`home/.codex` = the Codex home, `repo/` =
//! a git checkout); `/etc/codex` is assumed absent, as in `tests/resolve_codex.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use harn::journal::Journal;
use harn::model::{Agent, Target, Tree, Trust, TrustGate, TrustLevel};
use harn::plan;
use harn::resolve::{ClaudeCode, Codex, Resolver};
use harn::tui::{App, Opts};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fx {
    root: PathBuf,
    home: PathBuf,
    codex: PathBuf,
    repo: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `fetched_at` of every fixture catalog: a fixed instant, so the notes are the same every run.
const FETCHED_AT: &str = "2026-10-14T00:00:00Z";

/// `models_cache.json` as codex 0.160.1 writes it — every field codex checks before it uses the
/// file (`fetched_at`, `etag`, `client_version`, `identity`) — with these `models`. With
/// [`Fx::target`]'s codex 0.160.1 on `PATH` it is an accepted snapshot: its catalog facts count.
fn catalog(models: serde_json::Value) -> String {
    serde_json::json!({
        "fetched_at": FETCHED_AT,
        "etag": "W/\"catalog-1\"",
        "client_version": "0.160.1",
        "identity": "test-login",
        "models": models
    })
    .to_string()
}

/// A codex `version` in the Homebrew layout (`Caskroom/codex/<v>/bin/codex`) under `root`, the
/// version harn reads from its path; the dir to put on `PATH`.
fn codex_bin(root: &Path, version: &str) -> PathBuf {
    let exe = root.join("Caskroom/codex").join(version).join("bin/codex");
    write(&exe, "binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    exe.parent().unwrap().to_path_buf()
}

/// A Codex home with a model in an accepted `models_cache.json` (codex 0.160.1 on the target's
/// `PATH`), a global `AGENTS.md`, and a git checkout with its own `AGENTS.md`; the checkout is
/// trusted when `trusted`.
fn fx(trusted: bool) -> Fx {
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "harn-tui-codex-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(&root).unwrap();
    let home = root.join("home");
    let codex = home.join(".codex");
    let repo = root.join("repo");
    write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(
        &repo.join("AGENTS.md"),
        &format!(
            "# Project rules\n\n{}\n",
            "Keep changes small. ".repeat(150)
        ),
    );
    write(
        &codex.join("AGENTS.md"),
        &format!("# Mine\n\n{}\n", "Answer briefly. ".repeat(40)),
    );
    codex_bin(&root, "0.160.1");
    write(
        &codex.join("models_cache.json"),
        &catalog(serde_json::json!([{
            "slug": "gpt-test",
            "context_window": 200000,
            "multi_agent_version": "v2",
            "include_plugin_usage_instructions": true,
            "model_messages": {"instructions_template": "You are a coding agent. ".repeat(500)}
        }])),
    );
    let mut config = "model = \"gpt-test\"\n".to_string();
    if trusted {
        config.push_str(&format!(
            "\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            repo.display()
        ));
    }
    write(&codex.join("config.toml"), &config);
    Fx {
        root,
        home,
        codex,
        repo,
    }
}

impl Fx {
    /// The checkout, with the fixture's codex 0.160.1 on `PATH`.
    fn target(&self) -> Target {
        let path = self.root.join("Caskroom/codex/0.160.1/bin");
        Target {
            dir: self.repo.clone(),
            home: self.home.clone(),
            env: BTreeMap::from([("PATH".to_string(), path.display().to_string())]),
            model: None,
            context_window: None,
            bytes_per_token: None,
        }
    }

    fn journal(&self) -> Journal {
        Journal {
            dir: self.root.join("state"),
        }
    }

    fn tree(&self, resolver: &dyn Resolver) -> Tree {
        let t = self.target();
        let mut tree = resolver.resolve(&t, None).expect("resolve");
        plan::annotate_freed(resolver, &t, &mut tree).expect("annotate");
        tree
    }

    /// Both agents, Claude Code first, as `harn` without `--agent` opens them.
    fn app(&self, readonly: bool) -> App {
        App::new(
            self.target(),
            vec![
                (
                    Box::new(ClaudeCode) as Box<dyn Resolver>,
                    self.tree(&ClaudeCode),
                ),
                (Box::new(Codex) as Box<dyn Resolver>, self.tree(&Codex)),
            ],
            self.journal(),
            Opts {
                unicode: true,
                color: false,
                readonly,
            },
        )
    }

    fn user_file(&self) -> PathBuf {
        self.codex.join("config.toml")
    }

    fn project_file(&self) -> PathBuf {
        self.repo.join(".codex/config.toml")
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
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

/// The detail panel as one text: the lines between the last two rules, joined by a space.
fn panel_text(frame: &[String]) -> String {
    panel(frame)
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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

fn message(app: &App) -> String {
    app.message().map(|m| m.text()).unwrap_or_default()
}

/// The Codex tab with the cursor on the row of `id`.
fn codex_on(app: &mut App, id: &str) {
    if app.tree().agent != Agent::Codex {
        press(app, KeyCode::Tab);
    }
    assert_eq!(app.tree().agent, Agent::Codex);
    assert!(app.select(id), "no row for {id}");
}

const ENV: &str = "codex/env-block/builtin/environment_context";
const PROJECT_AGENTS: &str = "codex/instruction/project/";

#[test]
fn both_agents_get_a_tab_and_tab_switches_between_them() {
    let f = fx(true);
    let mut app = f.app(false);
    assert_eq!(
        app.tree().agent,
        Agent::ClaudeCode,
        "Claude Code is the first tab"
    );
    let first = screen(&app, 80, 24);
    assert!(first[0].contains("· Claude Code ·"), "{}", first[0]);
    // The rule under the header names the other tab.
    assert!(
        first[2].starts_with("── Tab shows Codex ──"),
        "{}",
        first[2]
    );

    press(&mut app, KeyCode::Tab);
    assert_eq!(app.tree().agent, Agent::Codex);
    let codex = screen(&app, 80, 24);
    assert!(
        codex[0].starts_with(" repo · Codex · trusted · "),
        "{}",
        codex[0]
    );
    assert!(
        codex[2].starts_with("── Tab shows Claude Code ──"),
        "{}",
        codex[2]
    );
    // Back with Tab, and with Shift-Tab the other way round.
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.tree().agent, Agent::ClaudeCode);
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.tree().agent, Agent::Codex);

    // One agent: no tab, no hint; the rule is a plain rule.
    let one = App::new(
        f.target(),
        vec![(Box::new(Codex) as Box<dyn Resolver>, f.tree(&Codex))],
        f.journal(),
        Opts {
            unicode: true,
            color: false,
            readonly: true,
        },
    );
    let frame = screen(&one, 80, 24);
    assert!(frame[2].chars().all(|c| c == '─'), "{}", frame[2]);
    // At 80 columns read-only wins the room over the trust word (the notes still say it).
    assert!(
        frame[0].starts_with(" repo · Codex · read-only · "),
        "{}",
        frame[0]
    );
    // With room, the installed codex's version (the fixture's 0.160.1 on PATH) too.
    let wide = screen(&one, 120, 40);
    assert!(
        wide[0].starts_with(" repo · Codex 0.160.1 · trusted · read-only · "),
        "{}",
        wide[0]
    );
}

/// The model note of [`fx`] / [`fx_full`] with the accepted catalog: what verify confirms is what
/// Codex's dump shows, so the base instructions keep the catalog's figure.
const ACCEPTED_MODEL_NOTE: &str = "The model is gpt-test (set in ~/.codex/config.toml), with a \
    context window of 200000 tokens. Sizes that depend on the model come from Codex's model \
    catalog as fetched at 2026-10-14T00:00:00Z (~/.codex/models_cache.json); Codex re-checks a \
    catalog older than 5 minutes when it starts, and harn does not check it was fetched for your \
    login. `harn verify --run --agent codex` confirms what Codex's dump shows; the base \
    instructions are not in it, so their size stays the catalog's figure.";

#[test]
fn the_codex_header_names_the_agent_and_the_trust_state_and_the_notes_say_what_it_means() {
    for (trusted, word, says) in [
        (
            true,
            "trusted",
            "This folder is trusted by Codex (set in ~/.codex/config.toml): its \
             .codex/config.toml, hooks, rules and AGENTS.md files load; skills load either way.",
        ),
        (
            false,
            "not trusted",
            "Codex has no trust decision for this folder: its .codex/config.toml, hooks and \
             rules are ignored; its AGENTS.md files load; skills load either way.",
        ),
    ] {
        let f = fx(trusted);
        let mut app = f.app(false);
        press(&mut app, KeyCode::Tab);
        for width in [80u16, 120] {
            let frame = screen(&app, width, 24);
            assert!(
                frame[0].starts_with(" repo · Codex"),
                "{width}: {}",
                frame[0]
            );
            assert!(
                frame[0].contains(&format!(" · {word} · ")),
                "{width}: {}",
                frame[0]
            );
        }
        let notes = &app.screen().notes;
        assert_eq!(notes[0].text, says);
        // The model note is written from the resolver's facts: the model and its file, the
        // window, and the catalog snapshot with what harn did not check.
        assert_eq!(notes[1].text, ACCEPTED_MODEL_NOTE, "{notes:?}");
        press(&mut app, KeyCode::Char('i'));
        let frame = screen(&app, 80, 24);
        let body = card(&frame, "Notes").join(" ");
        assert!(body.contains("trust"), "{body}");
        assert!(!body.contains("#/"), "{body}");
        assert!(!body.contains(f.root.to_str().unwrap()), "{body}");
    }
}

#[test]
fn codex_groups_are_named_for_people() {
    let f = fx(true);
    let mut app = f.app(false);
    press(&mut app, KeyCode::Tab);
    let names: Vec<&str> = app
        .screen()
        .groups
        .iter()
        .map(|g| g.name.as_str())
        .collect();
    // Base instructions, the AGENTS.md wrapper, permissions, collaboration, apps, multi-agent,
    // environment, and recommended plugins (shown while features.plugins is on; the plugins
    // instructions block needs the model catalog to ask for it).
    assert_eq!(names, ["AGENTS.md files (2)", "Built-in blocks (8)"]);
    let blocks = &app.screen().groups[1];
    assert_eq!(blocks.count_line, "6 of 8 can be turned off");
    let rows: Vec<&str> = blocks.rows.iter().map(|r| r.name.as_str()).collect();
    assert!(rows.contains(&"recommended plugins"), "{rows:?}");
    assert!(rows.contains(&"AGENTS.md wrapper"), "{rows:?}");
    assert!(rows.contains(&"base instructions (gpt-test)"), "{rows:?}");
    let files: Vec<(&str, &str)> = app.screen().groups[0]
        .rows
        .iter()
        .map(|r| (r.name.as_str(), r.source.as_str()))
        .collect();
    assert_eq!(
        files,
        [("AGENTS.md", "project"), ("~/.codex/AGENTS.md", "~/.codex")]
    );
    let frame = text(&app, 80, 24);
    assert!(frame.contains("biggest: AGENTS.md "), "{frame}");
    // Nothing of Claude Code's on the Codex screen.
    assert!(!frame.contains("Claude Code itself"), "{frame}");
    assert!(!frame.contains(".claude"), "{frame}");
}

#[test]
fn d_on_a_codex_block_names_the_toml_file_and_key_and_enter_writes_it_and_u_restores_it() {
    for (trusted, file, scope, card_title) in [
        (
            true,
            ".codex/config.toml",
            "for this project only",
            "Turn off environment context (cwd, shell, date) for this project",
        ),
        (
            false,
            "~/.codex/config.toml",
            "for every project",
            "Turn off environment context (cwd, shell, date) for every project",
        ),
    ] {
        let f = fx(trusted);
        let path = if trusted {
            f.project_file()
        } else {
            f.user_file()
        };
        let before = std::fs::read(&path).ok();
        let mut app = f.app(false);
        codex_on(&mut app, ENV);
        // The 120×40 panel has room for the action sentence.
        let frame = screen(&app, 120, 40);
        let detail = panel_text(&frame);
        assert!(
            detail.contains(&format!(
                "d turns it off {scope} by writing include_environment_context = false to \
                 {file}."
            )),
            "{detail}"
        );

        press(&mut app, KeyCode::Char('d'));
        let frame = screen(&app, 80, 24);
        let body = card(&frame, card_title);
        assert_eq!(
            body[0],
            format!(" Writes   {file} → include_environment_context = false"),
            "{body:?}"
        );
        assert!(body.iter().any(|l| l.contains("byte for byte")), "{body:?}");
        // The plan's scope warning reads as words, without the item's id.
        let note = body.join(" ");
        let says = if trusted {
            ".codex/config.toml is shared with everyone using this repository; Codex has no file"
        } else {
            "~/.codex/config.toml applies to every project: Codex reads this folder's .codex/config.toml only while it trusts the folder"
        };
        let note: String = note.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(note.contains(says), "{note}");
        assert!(!note.contains(ENV), "{note}");

        press(&mut app, KeyCode::Enter);
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("include_environment_context = false"),
            "{written}"
        );
        assert!(
            message(&app).starts_with(
                "turned off environment context (cwd, shell, date) · 1 fewer item loads at start"
            ),
            "{}",
            message(&app)
        );
        // The block left the tree: its row stays under Built-in blocks, named as it was, off.
        assert!(!harn::model::walk(app.tree()).any(|i| i.id == ENV));
        let names: Vec<&str> = app
            .screen()
            .groups
            .iter()
            .map(|g| g.name.as_str())
            .collect();
        assert!(!names.iter().any(|n| n.starts_with("Other")), "{names:?}");
        let blocks = app
            .screen()
            .groups
            .iter()
            .find(|g| g.name.starts_with("Built-in blocks"))
            .unwrap();
        let off = blocks
            .rows
            .iter()
            .find(|r| r.id.as_deref() == Some(ENV))
            .expect("the off row under Built-in blocks");
        assert_eq!(
            (off.name.as_str(), off.source.as_str(), off.absent),
            (
                "environment context (cwd, shell, date)",
                "Codex",
                Some("off")
            )
        );
        let frame = text(&app, 80, 24);
        assert!(!frame.contains("environment_context"), "{frame}");

        press(&mut app, KeyCode::Char('u'));
        let body = card(&screen(&app, 80, 24), "Undo the last change").join(" ");
        assert!(
            body.contains("environment context (cwd, shell, date) comes back on"),
            "{body}"
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(std::fs::read(&path).ok(), before, "restored byte for byte");
        assert!(
            message(&app).starts_with("undone: environment context (cwd, shell, date) back"),
            "{}",
            message(&app)
        );
        assert!(harn::model::walk(app.tree()).any(|i| i.id == ENV));
    }
}

#[test]
fn d_on_a_project_agents_file_says_the_cap_key_and_what_leaves() {
    let f = fx(true);
    let mut app = f.app(false);
    let id = harn::model::walk(&f.tree(&Codex))
        .find(|i| i.id.starts_with(PROJECT_AGENTS))
        .map(|i| i.id.clone())
        .expect("the project AGENTS.md row");
    codex_on(&mut app, &id);
    let detail = panel_text(&screen(&app, 80, 24));
    assert!(
        detail.contains(
            "d leaves out the project AGENTS.md files for this project only by writing \
             project_doc_max_bytes = 0 to .codex/config.toml."
        ),
        "{detail}"
    );
    press(&mut app, KeyCode::Char('d'));
    let body = card(&screen(&app, 80, 24), "Turn off AGENTS.md for this project");
    assert_eq!(
        body[0], " Writes   .codex/config.toml → project_doc_max_bytes = 0",
        "{body:?}"
    );
    press(&mut app, KeyCode::Esc);
    assert!(!f.project_file().exists(), "Esc writes nothing");
}

#[test]
fn a_codex_file_without_a_switch_says_so_in_words() {
    let f = fx(false);
    let mut app = f.app(false);
    let id = harn::model::walk(&f.tree(&Codex))
        .find(|i| i.label == "~/.codex/AGENTS.md")
        .map(|i| i.id.clone())
        .expect("the global AGENTS.md row");
    codex_on(&mut app, &id);
    let detail = panel_text(&screen(&app, 80, 24));
    assert!(
        detail.contains(
            "It cannot be turned off: Codex has no setting for it; empty the file or move it."
        ),
        "{detail}"
    );
    press(&mut app, KeyCode::Char('d'));
    assert!(!app.has_card());
    assert!(
        message(&app).starts_with("refused: ~/.codex/AGENTS.md cannot be turned off"),
        "{}",
        message(&app)
    );
}

#[test]
fn w_on_a_codex_block_names_its_default_and_its_switches() {
    let f = fx(false);
    let mut app = f.app(false);
    codex_on(&mut app, ENV);
    press(&mut app, KeyCode::Char('w'));
    let frame = screen(&app, 80, 24);
    let body = card(
        &frame,
        "Where environment context (cwd, shell, date) comes from",
    )
    .join("\n");
    assert!(
        body.contains("include_environment_context = true (Codex's default: no file sets it)"),
        "{body}"
    );
    assert!(
        body.contains("Switches  every project (~/.codex/config.toml)"),
        "{body}"
    );
    assert!(!body.contains("organisation"), "{body}");
    assert!(!body.contains(".rs:"), "{body}");
}

#[test]
fn t_on_the_codex_tab_says_trim_is_claude_codes_for_now() {
    let f = fx(true);
    let mut app = f.app(true);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(message(&app), harn::tui::TRIM_HINT);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(message(&app), harn::tui::TRIM_HINT_CODEX);
}

/// Every frame a reader can reach on the Codex tab: each line (every group and row opened) at
/// 80×24 and 120×40, its `w` card, its `d` card with the list of what leaves (or the message `d`
/// gives instead), then the notes and keys cards. Nothing is applied.
fn every_codex_frame(app: &mut App) -> Vec<String> {
    if app.tree().agent != Agent::Codex {
        press(app, KeyCode::Tab);
    }
    press(app, KeyCode::Char('g'));
    let mut frames = Vec::new();
    loop {
        press(app, KeyCode::Char('l'));
        for (w, h) in [(80u16, 24u16), (120, 40)] {
            frames.push(text(app, w, h));
        }
        press(app, KeyCode::Char('w'));
        if app.has_card() {
            frames.push(text(app, 80, 24));
            frames.push(text(app, 120, 40));
            press(app, KeyCode::Esc);
        }
        press(app, KeyCode::Char('d'));
        frames.push(text(app, 80, 24));
        if app.has_card() {
            frames.push(text(app, 120, 40));
            press(app, KeyCode::Right);
            frames.push(text(app, 120, 40));
            press(app, KeyCode::Esc);
            if app.has_card() {
                press(app, KeyCode::Esc);
            }
        }
        let at = app.cursor();
        press(app, KeyCode::Char('j'));
        if app.cursor() == at {
            break;
        }
    }
    for key in ['i', '?'] {
        press(app, KeyCode::Char(key));
        frames.push(text(app, 80, 24));
        frames.push(text(app, 120, 40));
        press(app, KeyCode::Esc);
    }
    frames
}

/// No frame shows an item id, the fixture's absolute paths, a `file#/pointer`, a citation of
/// codex's source or research, a developer's note, or harn's internal tags; no line is cut.
fn assert_codex_frames_clean(frames: &[String], tree: &Tree, root: &Path) {
    let root = root.display().to_string();
    let ids: Vec<&str> = harn::model::walk(tree).map(|i| i.id.as_str()).collect();
    for frame in frames {
        for id in &ids {
            assert!(!frame.contains(id), "{id} in:\n{frame}");
        }
        for bad in [
            root.as_str(),
            "#/",
            ".rs:",
            ".rs)",
            "research/",
            "not verified",
            "lead,",
            "[unverified]",
            "Δ?",
            "knob",
            "mode always",
            "mode none",
            "sha256",
            "Claude Code itself",
            "~/.claude",
            "rendered:",
            "(file: r",
            "listed as always",
            "target dir",
            // On the Codex tab the command names its agent.
            "harn verify --run`",
        ] {
            assert!(!frame.contains(bad), "{bad:?} in:\n{frame}");
        }
        assert!(
            !frame
                .chars()
                .any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)),
            "a tag in:\n{frame}"
        );
        // No sentence is cut with an ellipsis.
        for line in frame.lines() {
            let line = line.trim_end().trim_end_matches('│').trim_end();
            assert!(!line.ends_with('…'), "a cut line in:\n{frame}");
        }
    }
}

#[test]
fn no_codex_frame_shows_a_raw_id_an_absolute_path_a_source_ref_or_an_internal_tag() {
    let f = fx(true);
    let mut app = f.app(false);
    let frames = every_codex_frame(&mut app);
    assert_codex_frames_clean(&frames, app.tree(), &f.root);
}

/// [`fx`] trusted, with every kind Codex loads: a user and a repo skill, a plugin with a skill,
/// an MCP server and a hook, user hooks (one approved with the hash codex records, one not), a
/// rules file, MCP servers (deferred behind tool search: the model supports it; one turned off).
fn fx_full() -> Fx {
    let f = fx(true);
    let skill = |dir: &Path, name: &str, description: &str| {
        write(
            &dir.join(name).join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: {description}\n---\nBODY\n"),
        );
    };
    skill(
        &f.codex.join("skills"),
        "review",
        "Review a change before it lands.",
    );
    skill(
        &f.repo.join(".agents/skills"),
        "lint",
        "Run the linters of this repo.",
    );
    let plugin = f.codex.join("plugins/cache/mkt/docs/1.0.0");
    write(
        &plugin.join(".codex-plugin/plugin.json"),
        r#"{"name": "docs", "version": "1.0.0"}"#,
    );
    skill(&plugin.join("skills"), "search", "Search the docs.");
    write(
        &plugin.join(".mcp.json"),
        r#"{"mcpServers": {"docs": {"command": "docs-mcp"}}}"#,
    );
    write(
        &plugin.join("hooks/hooks.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo docs"}]}]}}"#,
    );
    write(
        &f.codex.join("hooks.json"),
        r#"{"hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo hello"}]}],
            "Stop": [{"hooks": [{"type": "command", "command": "echo bye"}]}]
        }}"#,
    );
    write(
        &f.codex.join("rules/default.rules"),
        "prefix_rule(pattern = [\"git\", \"status\"], decision = \"allow\")\n",
    );
    write(
        &f.codex.join("models_cache.json"),
        &catalog(serde_json::json!([{
            "slug": "gpt-test",
            "context_window": 200000,
            "supports_search_tool": true,
            "multi_agent_version": "v2",
            "include_plugin_usage_instructions": true,
            "model_messages": {"instructions_template": "You are a coding agent. ".repeat(500)}
        }])),
    );
    let base = format!(
        "model = \"gpt-test\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n\n\
         [plugins.\"docs@mkt\"]\nenabled = true\n\n\
         [mcp_servers.github]\ncommand = \"gh-mcp\"\n\n\
         [mcp_servers.old]\ncommand = \"old-mcp\"\nenabled = false\n",
        f.repo.display()
    );
    write(&f.user_file(), &base);
    // Approve the user SessionStart hook the way codex /hooks records it.
    let tree = Codex.resolve(&f.target(), None).expect("resolve");
    let start = harn::model::walk(&tree)
        .find(|i| {
            i.kind == Kind::Hook && i.origin.layer == Layer::User && i.id.contains("session_start")
        })
        .expect("the user SessionStart hook");
    let hash = start
        .origin
        .why
        .iter()
        .find_map(|w| w.strip_prefix("current hash "))
        .expect("its current hash");
    let key = start.id.splitn(4, '/').nth(3).unwrap();
    write(
        &f.user_file(),
        &format!("{base}\n[hooks.state.\"{key}\"]\ntrusted_hash = \"{hash}\"\n"),
    );
    f
}

#[test]
fn every_frame_of_a_full_codex_tree_reads_as_words() {
    let f = fx_full();
    let tree = f.tree(&Codex);
    // The fixture has what it claims: every kind, a deferred server, hooks on and off.
    let kinds: Vec<Kind> = harn::model::walk(&tree).map(|i| i.kind).collect();
    for kind in [
        Kind::Skill,
        Kind::SkillListing,
        Kind::Plugin,
        Kind::McpServer,
        Kind::Hook,
        Kind::PermissionRule,
    ] {
        assert!(kinds.contains(&kind), "{kind:?}");
    }
    let hooks: Vec<(&str, Mode)> = harn::model::walk(&tree)
        .filter(|i| i.kind == Kind::Hook)
        .map(|i| (i.label.as_str(), i.mode))
        .collect();
    assert!(hooks.iter().any(|(_, m)| *m == Mode::Always), "{hooks:?}");
    assert!(hooks.iter().any(|(_, m)| *m == Mode::None), "{hooks:?}");
    assert!(harn::model::walk(&tree).any(|i| i.mode == Mode::Deferred));
    assert!(harn::model::walk(&tree).any(|i| i.label == "old" && i.mode == Mode::None));

    for readonly in [false, true] {
        let mut app = f.app(readonly);
        let names: Vec<String> = {
            press(&mut app, KeyCode::Tab);
            app.screen().groups.iter().map(|g| g.name.clone()).collect()
        };
        for group in [
            "Plugins (1)",
            "Skill listing",
            "Hooks (2)",
            "MCP servers (2)",
            "Rules files (1)",
            "AGENTS.md files (2)",
        ] {
            assert!(names.iter().any(|n| n == group), "{group}: {names:?}");
        }
        let frames = every_codex_frame(&mut app);
        assert!(frames.len() > 60, "{} frames", frames.len());
        assert_codex_frames_clean(&frames, app.tree(), &f.root);
        // The recommended-plugins block's `w` card keeps its human line, not the lead.
        let joined = frames.join("\n");
        assert!(
            joined.contains("Where recommended plugins comes from"),
            "no w card"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The kinds the skills/plugins and hooks/rules/MCP resolvers add, on a tree built here in their
// shapes (`src/resolve/codex/{skills,plugins,hooks,rules,mcp}.rs`): a plugin row that is a switch
// with its parts tagged `origin.plugin` (not nested), the listing container with its entries and
// its wrapper row, hooks per handler (mode none with an `off:` signal when they do not run), MCP
// servers deferred behind tool search, a rules file.

use harn::model::{Cost, Item, Kind, Knob, Layer, Mode, Origin, Scope};
use harn::tui::model::{self as vm, GroupKind, Row, State};
use std::collections::HashMap;

fn row_item(id: &str, label: &str, kind: Kind, layer: Layer, mode: Mode) -> Item {
    Item {
        id: id.to_string(),
        label: label.to_string(),
        kind,
        origin: Origin {
            layer,
            path: None,
            setting: None,
            plugin: None,
            why: Vec::new(),
        },
        mode,
        cost: Cost::default(),
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

fn bytes(n: u64) -> Cost {
    Cost {
        source: Some(n),
        predicted: Some(n),
        ..Cost::default()
    }
}

fn none_cost() -> Cost {
    Cost {
        not_applicable: true,
        ..Cost::default()
    }
}

fn user_knob(f: &Fx, pointer: &str, value: serde_json::Value) -> Knob {
    Knob::Setting {
        file: f.user_file(),
        pointer: pointer.to_string(),
        value,
        scope: Scope::User,
        note: None,
    }
}

const PLUGIN: &str = "codex/plugin/plugin/docs@market";
const HOOK_ON: &str = "codex/hook/user/~session_start:0:0";
const HOOK_OFF: &str = "codex/hook/user/~stop:0:0";
const HOOK_UNTRUSTED: &str = "codex/hook/plugin/docs@market:hooks/hooks.json:session_start:0:0";
const MCP_ON: &str = "codex/mcp-server/user/github";
const MCP_OFF: &str = "codex/mcp-server/user/old";
const RULES: &str = "codex/permission-rule/user/rules";
const SKILL_USER: &str = "codex/skill/user/review";

fn synthetic(f: &Fx) -> Tree {
    let home = &f.home;
    let codex = home.join(".codex");
    let hooks_file = codex.join("hooks.json");
    let off = |pointer: &str| user_knob(f, pointer, serde_json::json!(false));
    let plugin_knob = off("/plugins/docs@market/enabled");

    let mut plugin = row_item(
        PLUGIN,
        "docs@market",
        Kind::Plugin,
        Layer::Plugin,
        Mode::Always,
    );
    plugin.cost = none_cost();
    plugin.origin.plugin = Some("docs@market".to_string());
    plugin.origin.path = Some(codex.join("plugins/cache/market/docs/1.2.0"));
    plugin.origin.why = vec![format!(
        "{}#/plugins/docs@market/enabled = true",
        f.user_file().display()
    )];
    plugin.knobs = vec![plugin_knob.clone()];

    let mut search = row_item(
        "codex/skill/plugin/docs:search",
        "docs:search",
        Kind::Skill,
        Layer::Plugin,
        Mode::Advertised,
    );
    search.cost = bytes(120);
    search.origin.plugin = Some("docs@market".to_string());
    search.knobs = vec![plugin_knob.clone()];
    let mut review = row_item(
        SKILL_USER,
        "review",
        Kind::Skill,
        Layer::User,
        Mode::Advertised,
    );
    review.cost = bytes(80);
    review.origin.path = Some(codex.join("skills/review/SKILL.md"));
    review.knobs = vec![user_knob(
        f,
        "/skills/config/-",
        serde_json::json!({"path": codex.join("skills/review/SKILL.md"), "enabled": false}),
    )];
    let mut listing = row_item(
        "codex/skill-listing/builtin/skills",
        "skill listing · 2",
        Kind::SkillListing,
        Layer::Builtin,
        Mode::Advertised,
    );
    listing.cost = bytes(200);
    listing.children = vec![search, review];
    listing.knobs = vec![off("/skills/include_instructions")];
    let mut wrapper = row_item(
        "codex/system-prompt/builtin/skills_instructions",
        "skills instructions (framing)",
        Kind::SystemPrompt,
        Layer::Builtin,
        Mode::Always,
    );
    wrapper.cost = bytes(400);
    wrapper.knobs = vec![off("/skills/include_instructions")];

    let mut server = row_item(
        "codex/mcp-server/plugin/docs",
        "docs",
        Kind::McpServer,
        Layer::Plugin,
        Mode::Deferred,
    );
    server.origin.plugin = Some("docs@market".to_string());
    server.knobs = vec![plugin_knob.clone()];
    let mut plugin_hook = row_item(
        HOOK_UNTRUSTED,
        "SessionStart hook · plugin docs",
        Kind::Hook,
        Layer::Plugin,
        Mode::None,
    );
    plugin_hook.cost = none_cost();
    plugin_hook.origin.plugin = Some("docs@market".to_string());
    plugin_hook.signal =
        Some("off: untrusted (no trusted_hash; review it in codex /hooks)".to_string());
    plugin_hook.knobs = vec![plugin_knob];

    let key = format!("{}:session_start:0:0", hooks_file.display());
    let mut hook_on = row_item(
        HOOK_ON,
        "SessionStart hook · ~/.codex/hooks.json 0:0",
        Kind::Hook,
        Layer::User,
        Mode::Always,
    );
    hook_on.origin.path = Some(hooks_file.clone());
    hook_on.knobs = vec![off(&format!(
        "/hooks/state/{}/enabled",
        key.replace('~', "~0").replace('/', "~1")
    ))];
    let mut hook_off = row_item(
        HOOK_OFF,
        "Stop hook · ~/.codex/hooks.json 0:0",
        Kind::Hook,
        Layer::User,
        Mode::None,
    );
    hook_off.cost = none_cost();
    hook_off.origin.path = Some(hooks_file);
    hook_off.signal = Some("off: disabled in [hooks.state]".to_string());
    hook_off.knobs = hook_on.knobs.clone();

    let mut github = row_item(
        MCP_ON,
        "github",
        Kind::McpServer,
        Layer::User,
        Mode::Deferred,
    );
    github.knobs = vec![off("/mcp_servers/github/enabled")];
    let mut old = row_item(MCP_OFF, "old", Kind::McpServer, Layer::User, Mode::None);
    old.cost = none_cost();
    old.signal = Some("off: mcp_servers.old.enabled = false".to_string());
    old.knobs = vec![off("/mcp_servers/old/enabled")];

    let mut rules = row_item(
        RULES,
        "~/.codex/rules/default.rules",
        Kind::PermissionRule,
        Layer::User,
        Mode::Always,
    );
    rules.origin.path = Some(codex.join("rules/default.rules"));
    rules.knobs = vec![Knob::None {
        reason: "no config key turns one rules file off; remove it or rename it to a name not ending in .rules".to_string(),
    }];

    let trusted = TrustGate {
        level: Some(TrustLevel::Trusted),
        key: f.repo.display().to_string(),
        set_in: Some(f.user_file()),
    };
    Tree {
        agent: Agent::Codex,
        target_dir: f.repo.clone(),
        agent_version: Some("0.160.1".to_string()),
        rules_verified_against: "0.160.1".to_string(),
        tool_search: Some(true),
        signals: vec![format!(
            "trust: trusted (projects.\"{}\".trust_level in {}): project .codex/config.toml, hooks and rules load",
            f.repo.display(),
            f.user_file().display()
        )],
        items: vec![
            wrapper, listing, hook_on, hook_off, rules, github, old, server, plugin_hook, plugin,
        ],
        predicted_known: 600,
        unknown_rows: 3,
        observed_body: None,
        warnings: Vec::new(),
        home: Some(home.clone()),
        observed_regions: Vec::new(),
        tool_search_cause: Some("models_cache.json: gpt-test supports_search_tool = true".to_string()),
        trust: Some(Trust {
            folder: trusted.clone(),
            agents_md: trusted,
        }),
        codex: None,
    }
}

fn find<'a>(rows: &'a [Row], name: &str) -> &'a Row {
    rows.iter().find(|r| r.name == name).unwrap_or_else(|| {
        panic!(
            "no row {name}: {:?}",
            rows.iter().map(|r| &r.name).collect::<Vec<_>>()
        )
    })
}

fn group(screen: &vm::Screen, kind: GroupKind) -> &vm::Group {
    screen
        .groups
        .iter()
        .find(|g| g.kind == kind)
        .unwrap_or_else(|| panic!("no group {kind:?}"))
}

#[test]
fn every_codex_kind_lands_in_its_group_with_a_name_and_a_sentence() {
    let f = fx(true);
    let tree = synthetic(&f);
    let screen = vm::build(&tree, &[], &HashMap::new(), vm::Sort::Size);
    let names: Vec<&str> = screen.groups.iter().map(|g| g.name.as_str()).collect();
    for name in [
        "Plugins (1)",
        "Skill listing",
        "Hooks (2)",
        "MCP servers (2)",
        "Rules files (1)",
        "Built-in blocks (1)",
    ] {
        assert!(names.contains(&name), "{name}: {names:?}");
    }
    assert_eq!(names.last(), Some(&"Built-in blocks (1)"));

    // The plugin: one row, its parts folded under it, its switch in TOML words.
    let plugins = group(&screen, GroupKind::Plugins);
    assert_eq!(
        plugins.detail.brings,
        "Turn one off and its skills, MCP servers and hooks leave next session."
    );
    let docs = find(&plugins.rows, "docs");
    assert_eq!(docs.source, "market");
    let parts: Vec<&str> = docs.children.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(parts.len(), 3, "{parts:?}");
    for part in ["search", "docs", "SessionStart hook"] {
        assert!(parts.contains(&part), "{part}: {parts:?}");
    }
    assert!(
        docs.detail.what.starts_with(
            "docs · plugin from the market marketplace, v1.2.0 · enabled for every project"
        ),
        "{}",
        docs.detail.what
    );
    assert_eq!(
        docs.detail.action,
        "d turns it off for every project by writing plugins.\"docs@market\".enabled = false to ~/.codex/config.toml."
    );
    // Its hook is not approved: it says so, and offers neither d nor o.
    let hook = find(&docs.children, "SessionStart hook");
    assert_eq!(
        (hook.state, hook.absent),
        (State::Off, Some("not approved"))
    );
    assert!(
        hook.detail
            .brings
            .starts_with("Codex runs a hook only after you approve it in codex /hooks"),
        "{}",
        hook.detail.brings
    );

    // The listing: entries by owner; its wrapper is a built-in block with a name for people.
    let listing = group(&screen, GroupKind::SkillListing);
    assert_eq!(listing.count_line, "2 entries");
    assert!(
        listing.detail.what.ends_with("one line per skill"),
        "{}",
        listing.detail.what
    );
    let owners: Vec<(&str, &str)> = listing
        .rows
        .iter()
        .map(|r| (r.name.as_str(), r.source.as_str()))
        .collect();
    assert!(owners.contains(&("you", "~/.codex")), "{owners:?}");
    let you = find(&listing.rows, "you");
    assert_eq!(
        find(&you.children, "review").detail.action,
        "d hides it for every project by adding an entry to skills.config in ~/.codex/config.toml."
    );
    let blocks = group(&screen, GroupKind::Builtin);
    let wrapper = find(&blocks.rows, "skill listing wrapper");
    assert_eq!(
        wrapper.detail.what,
        "skill listing wrapper · the heading and intro Codex puts around the skill listing"
    );
    assert_eq!(
        wrapper.detail.action,
        "d drops the whole skill listing for every project by writing skills.include_instructions = false to ~/.codex/config.toml."
    );

    // Hooks outside plugins: one on (its switch is its [hooks.state] entry), one the user's
    // settings turned off.
    let hooks = group(&screen, GroupKind::Hooks);
    assert_eq!(
        hooks.detail.what,
        "Hooks · commands Codex runs on events, outside any plugin"
    );
    let on = find(&hooks.rows, "SessionStart hook");
    assert_eq!(
        on.detail.action,
        "d turns it off for every project by writing hooks.state.\"~/.codex/hooks.json:session_start:0:0\".enabled = false to ~/.codex/config.toml."
    );
    assert_eq!(
        on.detail.brings,
        "What it prints enters the first request; Codex's dump does not include hook output, \
         so harn cannot measure it."
    );
    assert!(
        hooks
            .detail
            .brings
            .ends_with("Codex's dump does not include hook output, so harn cannot measure it."),
        "{}",
        hooks.detail.brings
    );
    let stop = find(&hooks.rows, "Stop hook");
    assert_eq!((stop.state, stop.absent), (State::Off, Some("off")));
    assert_eq!(stop.detail.what, "Stop hook · off here");
    assert_eq!(
        stop.detail.brings,
        "Your settings turn it off ([hooks.state]); it does not run here."
    );

    // MCP servers: deferred behind tool search, or off by the user's own setting.
    let servers = group(&screen, GroupKind::McpServers);
    assert!(
        servers.detail.brings.starts_with("Tool search is on"),
        "{}",
        servers.detail.brings
    );
    assert_eq!(
        servers.detail.brings,
        "Tool search is on: their tools are sent only when the model looks them up; their \
         schemas are not in Codex's dump, so harn cannot measure them."
    );
    let github = find(&servers.rows, "github");
    // Deferred, its stub unmeasured: no size, and the sentence says why harn cannot tell.
    assert_eq!((github.bytes, github.absent), (None, None));
    assert_eq!(
        github.detail.brings,
        "Tool search is on: its tools are sent only when the model looks them up; their \
         schemas are not in Codex's dump, so harn cannot measure them."
    );
    let old = find(&servers.rows, "old");
    assert_eq!((old.state, old.absent), (State::Off, Some("off")));
    assert_eq!(
        old.detail.brings,
        "Your settings turn it off (mcp_servers.old.enabled = false); it does not load here."
    );

    // A rules file: named by its file, no switch, said in words, no promise of a measurement.
    let rules = group(&screen, GroupKind::Rules);
    assert_eq!(
        rules.detail.brings,
        "Their approved commands go inside the permissions block; harn does not measure them \
         separately."
    );
    let file = find(&rules.rows, "default.rules");
    assert_eq!(
        file.detail.brings,
        "Its approved commands go inside the permissions block; harn does not measure them \
         separately."
    );
    assert_eq!(
        (file.source.as_str(), file.state),
        ("~/.codex", State::Fixed)
    );
    assert_eq!(
        file.detail.action,
        "It cannot be turned off: no config key turns one rules file off; remove it or rename it to a name not ending in .rules."
    );

    // The notes: trust first, then tool search (there is an MCP server).
    assert!(
        screen.notes[0]
            .text
            .starts_with("This folder is trusted by Codex"),
        "{:?}",
        screen.notes
    );
    assert!(
        screen.notes[1].text.starts_with("Tool search is on"),
        "{:?}",
        screen.notes
    );
    // No sentence carries an id, an absolute path or a `file#/pointer`.
    let root = f.root.display().to_string();
    let ids: Vec<&str> = harn::model::walk(&tree).map(|i| i.id.as_str()).collect();
    for row in screen.all_rows() {
        for text in [
            &row.name,
            &row.detail.what,
            &row.detail.brings,
            &row.detail.action,
        ] {
            assert!(!text.contains(&root), "{text}");
            assert!(!text.contains("#/"), "{text}");
            assert!(!ids.iter().any(|id| text.contains(id)), "{text}");
        }
    }
}

#[test]
fn d_on_a_codex_row_that_does_not_run_says_why_and_opens_no_card() {
    let f = fx(true);
    let tree = synthetic(&f);
    let mut app = App::new(
        f.target(),
        vec![(Box::new(Codex) as Box<dyn Resolver>, tree)],
        f.journal(),
        Opts {
            unicode: true,
            color: false,
            readonly: false,
        },
    );
    for (id, says) in [
        (
            MCP_OFF,
            "old is already off: your settings turn it off (mcp_servers.old.enabled = false)",
        ),
        (
            HOOK_OFF,
            "Stop hook is already off: your settings turn it off ([hooks.state])",
        ),
        (
            HOOK_UNTRUSTED,
            "SessionStart hook is not approved: Codex runs it only after you approve it in codex /hooks",
        ),
    ] {
        assert!(app.select(id), "{id}");
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.has_card(), "{id}");
        assert_eq!(message(&app), says);
    }
}

#[test]
fn a_codex_row_known_only_by_its_id_gets_a_name_for_people() {
    let f = fx(true);
    let places = vm::Places::of(&synthetic(&f));
    let hooks = f.codex.join("hooks.json");
    for (id, name) in [
        (
            "codex/system-prompt/builtin/permissions_instructions".to_string(),
            "permissions instructions".to_string(),
        ),
        (
            "codex/system-prompt/builtin/skills_instructions".to_string(),
            "skill listing wrapper".to_string(),
        ),
        (
            format!(
                "codex/instruction/user/{}#/developer_instructions",
                f.user_file().display()
            ),
            "developer_instructions".to_string(),
        ),
        (
            format!("codex/hook/user/{}:session_start:0:1", hooks.display()),
            "SessionStart hook".to_string(),
        ),
        (
            format!(
                "codex/permission-rule/user/{}",
                f.codex.join("rules/default.rules").display()
            ),
            "~/.codex/rules/default.rules".to_string(),
        ),
        (
            "codex/plugin/plugin/docs@market".to_string(),
            "docs".to_string(),
        ),
    ] {
        assert_eq!(vm::id_name(&id, &places).0, name, "{id}");
    }
}

#[test]
fn the_codex_where_card_reads_the_switches_by_scope_without_source_refs() {
    let f = fx(true);
    let mut tree = synthetic(&f);
    let places = vm::Places::of(&tree);
    let plugin = tree.items.iter_mut().find(|i| i.id == PLUGIN).unwrap();
    plugin.origin.why.push(format!(
        "{} (semver; core-plugin-common/src/installed.rs:47-68)",
        f.codex.join("plugins/cache/market/docs/1.2.0").display()
    ));
    let (title, lines) = vm::where_card(plugin, "docs", &places);
    assert_eq!(title, "Where docs comes from");
    let text = lines.join("\n");
    assert!(
        text.contains(
            "~/.codex/config.toml › plugins › docs@market › enabled = true   (every project)"
        ),
        "{text}"
    );
    assert!(
        text.contains("~/.codex/plugins/cache/market/docs/1.2.0 (semver)"),
        "{text}"
    );
    assert!(
        text.contains("Switches  every project (~/.codex/config.toml)"),
        "{text}"
    );
    assert!(!text.contains(".rs:"), "{text}");
}

#[test]
fn an_agents_file_that_does_not_land_says_why() {
    let f = fx(false);
    write(&f.codex.join("AGENTS.override.md"), "  \n");
    let mut app = f.app(false);
    press(&mut app, KeyCode::Tab);
    let files = &app.screen().groups[0];
    assert_eq!(files.name, "AGENTS.md files (3)");
    let empty = files
        .rows
        .iter()
        .find(|r| r.name == "~/.codex/AGENTS.override.md")
        .expect("the empty override row");
    assert_eq!(empty.absent, Some("not loaded"));
    assert_eq!(
        empty.detail.brings,
        "It does not load (empty: codex falls through to ~/.codex/AGENTS.md)."
    );
}

/// Card or panel lines as one text, every run of spaces one space.
fn words(lines: &[String]) -> String {
    lines
        .iter()
        .flat_map(|l| l.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The id of the first item of `kind` whose id contains `part`.
fn id_of(tree: &Tree, kind: Kind, part: &str) -> String {
    harn::model::walk(tree)
        .find(|i| i.kind == kind && i.id.contains(part))
        .map(|i| i.id.clone())
        .unwrap_or_else(|| panic!("no {kind:?} with {part}"))
}

#[test]
fn d_on_a_codex_part_uses_its_own_switch_not_a_wider_one_in_a_narrower_file() {
    let f = fx_full();
    let tree = f.tree(&Codex);
    let mut app = f.app(false);

    // An approved hook: its own [hooks.state] entry (user config: codex reads hook state there
    // only), never features.hooks in the project file, which would stop every hook.
    let hook = id_of(&tree, Kind::Hook, "hooks.json:session_start");
    codex_on(&mut app, &hook);
    press(&mut app, KeyCode::Char('d'));
    let body = card(
        &screen(&app, 120, 40),
        "Turn off SessionStart hook for every project",
    );
    let writes = words(&body);
    assert!(
        writes.contains("~/.codex/config.toml → hooks.state.\"~/.codex/hooks.json:session_start:0:0\".enabled = false"),
        "{writes}"
    );
    assert!(!writes.contains("features.hooks"), "{writes}");
    assert!(
        writes.contains(
            "~/.codex/config.toml applies to every project: Codex reads this setting from that \
             file only."
        ),
        "{writes}"
    );
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        plan::default_scope(harn::model::walk(&tree).find(|i| i.id == hook).unwrap()),
        harn::model::Scope::User
    );

    // A plugin's skill: hidden by its own [[skills.config]] entry, the plugin stays.
    let skill = id_of(&tree, Kind::Skill, "docs:search");
    codex_on(&mut app, &skill);
    press(&mut app, KeyCode::Char('d'));
    let body = words(&card(
        &screen(&app, 120, 40),
        "Turn off search for every project",
    ));
    assert!(body.contains("skills.config += {"), "{body}");
    assert!(!body.contains("plugins.\"docs@mkt\".enabled"), "{body}");
    press(&mut app, KeyCode::Esc);

    // A block's switch is the same key in two files: the narrower (this trusted folder) wins.
    codex_on(&mut app, ENV);
    press(&mut app, KeyCode::Char('d'));
    assert!(card(
        &screen(&app, 120, 40),
        "Turn off environment context (cwd, shell, date) for this project"
    )[0]
    .contains(".codex/config.toml → include_environment_context = false"));
    press(&mut app, KeyCode::Esc);

    // What leaves with the plugin is counted in Codex's words.
    let plugin = id_of(&tree, Kind::Plugin, "docs@mkt");
    codex_on(&mut app, &plugin);
    press(&mut app, KeyCode::Char('d'));
    let body = words(&card(
        &screen(&app, 120, 40),
        "Turn off docs for this project",
    ));
    assert!(body.contains("1 block"), "{body}");
    assert!(!body.contains("system prompt"), "{body}");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('w'));
    let body = words(&card(&screen(&app, 120, 40), "Where docs comes from"));
    assert!(
        body.contains("contributes 1 skill · 1 MCP server · 1 hook"),
        "{body}"
    );
    assert!(!body.contains("modelled"), "{body}");
}

#[test]
#[ignore = "frame dump for reading: HARN_TUI_DUMP=1 cargo test --test tui_codex dump_full -- --ignored --nocapture"]
fn dump_full() {
    for f in [fx_full(), fx_unknown_model()] {
        let mut app = f.app(false);
        let _ = every_codex_frame(&mut app);
    }
}

/// [`fx_full`] the way the author's machine is: no global AGENTS.md, and a model the cached
/// catalog does not know (base instructions and tool search unknown).
fn fx_unknown_model() -> Fx {
    let f = fx_full();
    std::fs::remove_file(f.codex.join("AGENTS.md")).unwrap();
    write(
        &f.codex.join("models_cache.json"),
        &catalog(serde_json::json!([{"slug": "other", "base_instructions": "Base."}])),
    );
    f
}

fn group_line<'a>(frame: &'a [String], name: &str) -> &'a str {
    frame
        .iter()
        .find(|l| l.contains(name))
        .unwrap_or_else(|| panic!("no {name} line:\n{}", frame.join("\n")))
}

#[test]
fn a_group_with_unmeasured_rows_shows_its_known_part_as_a_floor() {
    // The base instructions known (12 KB from the catalog), or only the wrappers.
    for f in [fx_full(), fx_unknown_model()] {
        let mut app = f.app(false);
        press(&mut app, KeyCode::Tab);
        let blocks = app
            .screen()
            .groups
            .iter()
            .find(|g| g.kind == GroupKind::Builtin)
            .unwrap();
        assert_eq!(blocks.absent, Some(vm::PLUS_UNMEASURED));
        let known = vm::size(blocks.bytes.expect("a known part"));
        assert_eq!(blocks.share, None, "a floor gets no share");
        for (w, h) in [(80u16, 24u16), (120, 40)] {
            let frame = screen(&app, w, h);
            let line = group_line(&frame, "Built-in blocks");
            assert!(
                line.contains(&format!("{known}  + unmeasured")),
                "{w}: {line}"
            );
            assert!(!line.contains('%'), "{w}: {line}");
            assert!(!line.contains('█') && !line.contains('▏'), "{w}: {line}");
        }
        // A group with every size known keeps its bar and share.
        let frame = screen(&app, 120, 40);
        assert!(group_line(&frame, "AGENTS.md files").contains('%'));
    }
}

/// A catalog snapshot harn does not use, kept refused on purpose: no codex on `PATH`, another
/// codex than the one that wrote the file, a codex whose version no file names. The notes and
/// the base instructions' detail say why in words, from the resolver's facts — never a code
/// reference or a `#/` pointer.
#[test]
fn a_catalog_snapshot_harn_does_not_use_is_explained_in_words() {
    let base = "codex/system-prompt/builtin/instructions";
    let path_to = |dir: &Path| BTreeMap::from([("PATH".to_string(), dir.display().to_string())]);
    for (case, why) in [
        (
            "no codex on PATH",
            "codex is not on PATH, so harn cannot tell whether Codex accepts it",
        ),
        (
            "another codex",
            "codex 0.160.1 wrote it and codex 0.159.0 is installed, so Codex fetches a new \
             catalog when it starts",
        ),
        (
            "unversioned codex",
            "the installed codex's version cannot be read, so harn cannot tell whether Codex \
             accepts it",
        ),
    ] {
        let f = fx_full();
        let env = match case {
            "no codex on PATH" => BTreeMap::new(),
            "another codex" => path_to(&codex_bin(&f.root.join("other"), "0.159.0")),
            _ => {
                let exe = f.root.join("plain/bin/codex");
                write(&exe, "binary");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
                path_to(exe.parent().unwrap())
            }
        };
        let target = Target { env, ..f.target() };
        let mut tree = Codex.resolve(&target, None).expect("resolve");
        plan::annotate_freed(&Codex, &target, &mut tree).expect("annotate");
        let unused =
            format!("Codex's model catalog (~/.codex/models_cache.json) is not used: {why}");
        let notes: Vec<String> = vm::build(&tree, &[], &HashMap::new(), vm::Sort::Size)
            .notes
            .into_iter()
            .map(|n| n.text)
            .collect();
        // The why is said once, in the model note; the tool-search note right after it points
        // there and keeps its consequence.
        let model = notes
            .iter()
            .position(|n| n.starts_with("The model is"))
            .unwrap_or_else(|| panic!("{case}: {notes:#?}"));
        assert_eq!(
            notes[model],
            format!(
                "The model is gpt-test (set in ~/.codex/config.toml), its context window \
                 unknown. {unused}; sizes that depend on the model are unknown."
            ),
            "{case}"
        );
        assert_eq!(
            notes[model + 1],
            "Tool search cannot be decided: it comes from the model's entry in Codex's model \
             catalog, which harn cannot use here (the model note says why); MCP servers are \
             shown as loading up front.",
            "{case}"
        );
        assert_eq!(
            notes.iter().filter(|n| n.contains(why)).count(),
            1,
            "{case}: the why once: {notes:#?}"
        );
        for n in &notes {
            assert!(
                !n.contains("#/") && !n.contains(".rs:") && !n.contains("models-manager"),
                "{case}: {n}"
            );
        }
        let mut app = App::new(
            target,
            vec![(Box::new(Codex) as Box<dyn Resolver>, tree)],
            f.journal(),
            Opts {
                unicode: true,
                color: false,
                readonly: false,
            },
        );
        codex_on(&mut app, base);
        let detail = panel_text(&screen(&app, 120, 40));
        assert!(
            detail.contains(&format!(
                "Its size is unknown: {unused}; Codex sends these instructions outside its \
                 dump, so harn cannot measure them."
            )),
            "{case}: {detail}"
        );
        assert!(
            !detail.contains("not in the model catalog"),
            "{case}: {detail}"
        );
    }
}

#[test]
fn base_instructions_say_where_their_size_comes_from_and_that_verify_cannot_check_them() {
    let base = "codex/system-prompt/builtin/instructions";
    let f = fx_full();
    let mut app = f.app(false);
    codex_on(&mut app, base);
    let detail = panel_text(&screen(&app, 120, 40));
    assert!(
        detail.contains(
            "12 KB of your first request, counted from the model catalog; Codex sends it outside \
             its dump, so harn cannot check it against a session."
        ),
        "{detail}"
    );
    // The model note on the same screen agrees: verify confirms what the dump shows, and the
    // base instructions keep the catalog's figure.
    assert!(
        app.screen()
            .notes
            .iter()
            .any(|n| n.text == ACCEPTED_MODEL_NOTE),
        "{:#?}",
        app.screen().notes
    );
    let f = fx_unknown_model();
    let mut app = f.app(false);
    codex_on(&mut app, base);
    let detail = panel_text(&screen(&app, 120, 40));
    assert!(
        detail.contains(
            "Its size is unknown: gpt-test is not in Codex's model catalog; Codex sends these \
             instructions outside its dump, so harn cannot measure them."
        ),
        "{detail}"
    );
    assert!(!detail.contains("verify"), "{detail}");
    press(&mut app, KeyCode::Char('w'));
    let body = words(&card(
        &screen(&app, 120, 40),
        "Where base instructions comes from",
    ));
    assert!(!body.contains("capture"), "{body}");
}

#[test]
fn an_undecidable_tool_search_is_one_note_in_words() {
    let f = fx_unknown_model();
    let mut app = f.app(false);
    press(&mut app, KeyCode::Tab);
    assert!(app.tree().tool_search.is_none());
    let notes: Vec<&str> = app.screen().notes.iter().map(|n| n.text.as_str()).collect();
    let about: Vec<&&str> = notes.iter().filter(|n| n.contains("ool search")).collect();
    assert_eq!(about.len(), 1, "{notes:#?}");
    assert!(
        about[0].ends_with("MCP servers are shown as loading up front."),
        "{}",
        about[0]
    );
    for note in &notes {
        assert!(!note.contains("listed as always"), "{note}");
        assert!(!note.contains("MCP rows"), "{note}");
    }
}

#[test]
fn an_agents_file_says_which_folder_it_is_in_and_what_d_frees_with_it() {
    // This folder, a global file beside it: the wrapper stays, part of it goes.
    let f = fx_full();
    let tree = f.tree(&Codex);
    let project = id_of(&tree, Kind::Instruction, "/repo/AGENTS.md");
    let mut app = f.app(false);
    codex_on(&mut app, &project);
    let detail = panel_text(&screen(&app, 120, 40));
    assert!(
        detail.contains(": the file and part of the wrapper around the AGENTS.md files."),
        "{detail}"
    );
    press(&mut app, KeyCode::Char('w'));
    let body = words(&card(&screen(&app, 120, 40), "Where AGENTS.md comes from"));
    assert!(
        body.contains("1 AGENTS.md in this folder Switches"),
        "{body}"
    );

    // No global file: the wrapper leaves with the last project file.
    let f = fx_unknown_model();
    let tree = f.tree(&Codex);
    let project = id_of(&tree, Kind::Instruction, "/repo/AGENTS.md");
    let mut app = f.app(false);
    codex_on(&mut app, &project);
    let detail = panel_text(&screen(&app, 120, 40));
    assert!(
        detail.contains(": the file and the wrapper around the AGENTS.md files."),
        "{detail}"
    );
    // The card counts the wrapper as the block it is, and its list names it.
    press(&mut app, KeyCode::Char('d'));
    let body = words(&card(
        &screen(&app, 120, 40),
        "Turn off AGENTS.md for this project",
    ));
    assert!(
        body.contains("Leaves 2 at start (AGENTS.md, 1 block)"),
        "{body}"
    );
    press(&mut app, KeyCode::Right);
    let list = words(&card(&screen(&app, 120, 40), "What leaves with AGENTS.md"));
    assert!(list.contains("AGENTS.md wrapper (block)"), "{list}");
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);

    // A file in an ancestor of the target: one of the chain.
    let sub = f.repo.join("pkg");
    write(&sub.join("AGENTS.md"), "# Package rules\n");
    let target = Target {
        dir: sub.clone(),
        ..f.target()
    };
    let tree = Codex.resolve(&target, None).expect("resolve");
    let places = vm::Places::of(&tree);
    let root_file = harn::model::walk(&tree)
        .find(|i| i.id.ends_with("/repo/AGENTS.md"))
        .expect("the repo root's AGENTS.md");
    let (_, lines) = vm::where_card(root_file, "AGENTS.md", &places);
    let text = lines.join("\n");
    assert!(
        text.contains(&format!(
            "AGENTS.md in {}, one of the files from the project root down to this folder",
            f.repo.display()
        )),
        "{text}"
    );
    let here = harn::model::walk(&tree)
        .find(|i| i.id.ends_with("/pkg/AGENTS.md"))
        .expect("the target's AGENTS.md");
    let (_, lines) = vm::where_card(here, "AGENTS.md", &places);
    assert!(lines[0].ends_with("AGENTS.md in this folder"), "{lines:?}");
}

#[test]
fn rules_files_are_named_by_their_file_and_promise_no_measurement() {
    let f = fx_full();
    let mut app = f.app(false);
    press(&mut app, KeyCode::Tab);
    let rules = app
        .screen()
        .groups
        .iter()
        .find(|g| g.kind == GroupKind::Rules)
        .unwrap();
    let row = &rules.rows[0];
    assert_eq!(
        (row.name.as_str(), row.source.as_str()),
        ("default.rules", "~/.codex")
    );
    assert!(
        !row.detail.brings.contains("verify"),
        "{}",
        row.detail.brings
    );
    assert!(!rules.detail.brings.contains("verify"));
}

/// The first note when the two trust gates differ: each claim from the gate that decides it, the
/// folder's for the project config, hooks and rules, the AGENTS.md gate's for the AGENTS.md
/// files. The four ways the gates can stand, built from files (as in `tests/resolve_codex.rs`).
#[test]
fn the_trust_note_reads_each_gate_for_what_it_decides() {
    let note = |f: &Fx, dir: &Path| -> String {
        let target = Target {
            dir: dir.to_path_buf(),
            ..f.target()
        };
        let tree = Codex.resolve(&target, None).expect("resolve");
        vm::build(&tree, &[], &HashMap::new(), vm::Sort::Size).notes[0]
            .text
            .clone()
    };
    let model = "model = \"gpt-test\"\n";
    let entry = |dir: &Path, level: &str| {
        format!(
            "[projects.\"{}\"]\ntrust_level = \"{level}\"\n",
            dir.display()
        )
    };

    // Same level.
    let f = fx(true);
    assert_eq!(
        note(&f, &f.repo),
        "This folder is trusted by Codex (set in ~/.codex/config.toml): its .codex/config.toml, \
         hooks, rules and AGENTS.md files load; skills load either way."
    );

    // The folder untrusted, its AGENTS.md files re-trusted by the repo's own layer.
    let f = fx(false);
    let child = f.repo.join("child");
    write(&child.join("AGENTS.md"), "child doc\n");
    write(
        &f.repo.join(".codex/config.toml"),
        &entry(&child, "trusted"),
    );
    write(
        &f.user_file(),
        &format!(
            "{model}{}{}",
            entry(&f.repo, "trusted"),
            entry(&child, "untrusted")
        ),
    );
    let text = note(&f, &child);
    assert!(
        text.starts_with(
            "This folder is marked untrusted for Codex (set in ~/.codex/config.toml): its \
             .codex/config.toml, hooks and rules are skipped; its AGENTS.md files load, by a \
             separate entry (set in "
        ),
        "{text}"
    );
    assert!(
        text.ends_with("/repo/.codex/config.toml); skills load either way."),
        "{text}"
    );

    // The folder trusted (through its repo), its AGENTS.md files untrusted by the repo's layer.
    let f = fx(false);
    let child = f.repo.join("child");
    write(&child.join("AGENTS.md"), "child doc\n");
    write(
        &f.repo.join(".codex/config.toml"),
        &entry(&child, "untrusted"),
    );
    write(
        &f.user_file(),
        &format!("{model}{}", entry(&f.repo, "trusted")),
    );
    let text = note(&f, &child);
    assert!(
        text.starts_with(
            "This folder is trusted by Codex (set in ~/.codex/config.toml): its \
             .codex/config.toml, hooks and rules load; its AGENTS.md files are skipped, by a \
             separate entry (set in "
        ),
        "{text}"
    );

    // No decision for the folder, its AGENTS.md files untrusted by an enabled layer above it.
    let f = fx(false);
    let mid = f.repo.join("mid");
    let leaf = mid.join("leaf");
    write(&leaf.join("AGENTS.md"), "leaf doc\n");
    write(&mid.join(".codex/config.toml"), &entry(&leaf, "untrusted"));
    write(
        &f.user_file(),
        &format!("{model}{}", entry(&mid, "trusted")),
    );
    let text = note(&f, &leaf);
    assert!(
        text.starts_with(
            "Codex has no trust decision for this folder: its .codex/config.toml, hooks and \
             rules are ignored; its AGENTS.md files are skipped, by a separate entry (set in "
        ),
        "{text}"
    );
}

/// The trust note names the file of each gate however its path is spelled: a `(x)` or two
/// spaces in the folders above the Codex home (`CODEX_HOME`) and above the repo whose own layer
/// re-trusts the AGENTS.md files. Built from files; the note is written from the resolver's facts.
#[test]
fn the_trust_note_names_each_gate_file_with_parentheses_or_two_spaces() {
    for name in ["old (x)", "a  b"] {
        let f = fx(false);
        let base = f.root.join(name);
        let codex_home = base.join("codex-home");
        let repo = base.join("repo");
        let child = repo.join("child");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        write(&child.join("AGENTS.md"), "child doc\n");
        let entry = |dir: &Path, level: &str| {
            format!(
                "[projects.\"{}\"]\ntrust_level = \"{level}\"\n",
                dir.display()
            )
        };
        write(&repo.join(".codex/config.toml"), &entry(&child, "trusted"));
        write(
            &codex_home.join("config.toml"),
            &format!(
                "model = \"gpt-test\"\n{}{}",
                entry(&repo, "trusted"),
                entry(&child, "untrusted")
            ),
        );
        let target = Target {
            dir: child.clone(),
            env: BTreeMap::from([("CODEX_HOME".to_string(), codex_home.display().to_string())]),
            ..f.target()
        };
        let tree = Codex.resolve(&target, None).expect("resolve");
        let text = vm::build(&tree, &[], &HashMap::new(), vm::Sort::Size).notes[0]
            .text
            .clone();
        assert_eq!(
            text,
            format!(
                "This folder is marked untrusted for Codex (set in {}): its .codex/config.toml, \
                 hooks and rules are skipped; its AGENTS.md files load, by a separate entry (set \
                 in {}); skills load either way.",
                codex_home.join("config.toml").display(),
                repo.join(".codex/config.toml").display()
            ),
            "{name}"
        );
    }
}

/// [`fx_full`] with a second plugin that brings only a skill.
fn fx_two_plugins() -> Fx {
    let f = fx_full();
    let plugin = f.codex.join("plugins/cache/mkt/notes/1.0.0");
    write(
        &plugin.join(".codex-plugin/plugin.json"),
        r#"{"name": "notes", "version": "1.0.0"}"#,
    );
    write(
        &plugin.join("skills/jot/SKILL.md"),
        "---\nname: jot\ndescription: Jot a note down.\n---\nBODY\n",
    );
    let mut config = std::fs::read_to_string(f.user_file()).unwrap();
    config.push_str("\n[plugins.\"notes@mkt\"]\nenabled = true\n");
    write(&f.user_file(), &config);
    f
}

#[test]
fn what_a_saving_leaves_unmeasured_follows_the_rows_that_leave() {
    let f = fx_two_plugins();
    let tree = f.tree(&Codex);
    let mut app = f.app(false);

    // A plugin with a skill, an MCP server and a hook: its unknown part is the server's tool
    // schemas, which `harn verify` cannot measure — never promised to it.
    let docs = id_of(&tree, Kind::Plugin, "docs@mkt");
    codex_on(&mut app, &docs);
    let detail = panel_text(&screen(&app, 120, 40));
    assert!(!detail.contains("verify"), "{detail}");
    press(&mut app, KeyCode::Char('d'));
    let body = words(&card(
        &screen(&app, 120, 40),
        "Turn off docs for this project",
    ));
    assert!(!body.contains("verify"), "{body}");
    assert!(body.contains("plus its own unmeasured part"), "{body}");
    press(&mut app, KeyCode::Esc);

    // A plugin with only a skill: everything it frees is known.
    let notes = id_of(&tree, Kind::Plugin, "notes@mkt");
    codex_on(&mut app, &notes);
    press(&mut app, KeyCode::Char('d'));
    let body = words(&card(
        &screen(&app, 120, 40),
        "Turn off notes for this project",
    ));
    assert!(
        body.contains("Saves 0.1 KB: its entries in the skill listing"),
        "{body}"
    );
    assert!(
        !body.contains("unmeasured") && !body.contains("verify"),
        "{body}"
    );
    press(&mut app, KeyCode::Esc);

    // A block whose switch takes every plugin: its own size is for `harn verify`, the plugins'
    // MCP tool schemas are not — both said.
    let recommended = "codex/system-prompt/builtin/recommended_plugins";
    codex_on(&mut app, recommended);
    let detail = panel_text(&screen(&app, 120, 40));
    assert!(
        detail.contains(
            ", a part only `harn verify --run --agent codex` can measure, and MCP tool schemas, \
             which harn cannot measure."
        ),
        "{detail}"
    );

    // A hook alone: harn cannot measure it.
    let hook = id_of(&tree, Kind::Hook, "hooks.json:session_start");
    codex_on(&mut app, &hook);
    let detail = panel_text(&screen(&app, 120, 40));
    assert!(
        detail.contains("harn cannot measure how much that frees."),
        "{detail}"
    );
}

/// Claude Code's `d` card shows a plan warning as it always has, the item's id included: only a
/// Codex card rewords it.
#[test]
fn a_claude_card_keeps_its_plan_warning_as_written() {
    use harn::model::{Freed, Scope as S};
    use harn::plan::{ActionPlan, PlannedOp};
    let home = PathBuf::from("/home/me");
    let dir = PathBuf::from("/home/me/repo");
    let id = "claude-code/skill/user/review";
    let item = row_item(id, "review", Kind::Skill, Layer::User, Mode::Advertised);
    let tree = Tree {
        agent: Agent::ClaudeCode,
        target_dir: dir.clone(),
        agent_version: None,
        rules_verified_against: "2.1.296".to_string(),
        tool_search: None,
        signals: Vec::new(),
        items: vec![item.clone()],
        predicted_known: 0,
        unknown_rows: 0,
        observed_body: None,
        warnings: Vec::new(),
        home: Some(home.clone()),
        observed_regions: Vec::new(),
        tool_search_cause: None,
        trust: None,
        codex: None,
    };
    let file = home.join(".claude/settings.json");
    let warning = format!(
        "{id}: scope user, wider than local — {} applies to every project",
        file.display()
    );
    let plan = ActionPlan {
        target_dir: dir,
        agent: Agent::ClaudeCode,
        agent_version: None,
        ops: vec![PlannedOp {
            item_id: id.to_string(),
            knob: Knob::Setting {
                file: file.clone(),
                pointer: "/skillOverrides/review".to_string(),
                value: serde_json::json!("off"),
                scope: S::User,
                note: None,
            },
            file: Some(file),
            hash_before: None,
            reason: None,
        }],
        predicted_before: 0,
        predicted_after: 0,
        items_after: 0,
        freed: Freed::default(),
        leaving: Vec::new(),
        side_effects: Vec::new(),
        warnings: vec![warning],
        diff: String::new(),
    };
    let card = vm::off_card(&plan, &tree, &item, "review", "→", None);
    // As main renders it: the warning with paths home-relative, as a sentence.
    assert_eq!(
        card.notes,
        [
            "Claude-code/skill/user/review: scope user, wider than local — \
          ~/.claude/settings.json applies to every project."
                .to_string()
        ]
    );
}

/// The `w` card of a plugin's skill, at the path a real install has (`openai-primary-runtime`
/// marketplace, plugin `documents`, version `26.630.12135`): the manifest path wraps after a `/`,
/// never inside `.codex-plugin`, and the skill's line names its root once.
#[test]
fn a_plugin_skill_where_card_wraps_paths_at_slashes_and_names_its_root_once() {
    let f = fx(true);
    let plugin = f
        .codex
        .join("plugins/cache/openai-primary-runtime/documents/26.630.12135");
    write(
        &plugin.join(".codex-plugin/plugin.json"),
        r#"{"name": "documents", "version": "26.630.12135"}"#,
    );
    write(
        &plugin.join("skills/documents/SKILL.md"),
        "---\nname: documents\ndescription: Read and write documents.\n---\nBODY\n",
    );
    let mut config = std::fs::read_to_string(f.user_file()).unwrap();
    config.push_str("\n[plugins.\"documents@openai-primary-runtime\"]\nenabled = true\n");
    write(&f.user_file(), &config);
    let tree = f.tree(&Codex);
    let skill = id_of(&tree, Kind::Skill, "documents:documents");
    let mut app = f.app(false);
    codex_on(&mut app, &skill);
    press(&mut app, KeyCode::Char('w'));
    for (w, h) in [(120u16, 40u16), (80, 24)] {
        let body = card(&screen(&app, w, h), "Where documents comes from");
        let text = body.join("\n");
        // No row ends on a `.` that starts the next row's dot-directory.
        for pair in body.windows(2) {
            assert!(
                !(pair[0].trim_end().ends_with('.')
                    && pair[1].trim_start().starts_with("codex-plugin")),
                "{w}: {text}"
            );
        }
        assert!(
            body.iter()
                .any(|l| l.trim_start().starts_with(".codex-plugin/plugin.json")),
            "{w}: {text}"
        );
        // The skill's file with its plugin, the root it sits under not repeated; rows that end
        // on a `/` continue the same path.
        let joined: String = body
            .iter()
            .map(|l| l.trim())
            .fold(String::new(), |mut acc, l| {
                if !acc.is_empty() && !acc.ends_with('/') {
                    acc.push(' ');
                }
                acc.push_str(l);
                acc
            });
        assert!(
            joined.contains(
                "26.630.12135/skills/documents/SKILL.md (plugin documents@openai-primary-runtime)"
            ),
            "{w}: {joined}"
        );
        assert!(!joined.contains(" under "), "{w}: {joined}");
        // Its setting reads as the user's, for every project.
        assert!(
            joined.contains("documents@openai-primary-runtime › enabled = true   (every project)"),
            "{w}: {joined}"
        );
    }
}
