//! Drawing [`App`] (`.prd/04-tui-design.md`): two header lines, the list grouped by kind with a
//! size, a bar relative to the largest line shown and a share of the total, the detail panel of
//! the cursor line, the key bar in words; the cards (`d`, `o`, `u`, `w`, `?`, `i`) over it. Under `--no-unicode` everything harn draws is ASCII.

use super::app::{editable, App, Card, Mode, Tone};
use super::model::{size, Detail, Line as ListLine, OffCard, State};
use crate::render::common::{glyphs, Glyphs};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The widest a card is drawn.
const CARD_MAX: u16 = 80;
/// The smallest frame a card (the `u` one too) is drawn in. Smaller, it is not drawn, its keys
/// do nothing but close it, and the message row says so. An action card (`d`, `o`, `u`) also
/// needs its whole body in view: Enter never applies what the reader cannot see.
pub(crate) const CARD_MIN: (u16, u16) = (40, 10);
/// The size column (`  48 KB`, `0.7 KB`, `?`, `—`).
const SIZE_W: usize = 7;
/// The share column (` 32%`).
const SHARE_W: usize = 4;
/// What the size column says when nobody knows the size yet; it reaches left of the column.
const UNMEASURED: &str = "unmeasured";
/// The label column of the `d` and `o` cards (`Writes   `).
const LABEL_W: usize = 9;

/// Harn's glyphs and their ASCII stand-ins under `--no-unicode`.
const ASCII_SWAPS: [(&str, &str); 12] = [
    ("×", "x"),
    ("·", "|"),
    ("—", "-"),
    ("→", "->"),
    ("←", "<-"),
    ("›", ">"),
    ("…", "..."),
    ("▸", ">"),
    ("▾", "v"),
    ("↑", "^"),
    ("↓", "v"),
    ("─", "-"),
];

/// `text` with harn's glyphs swapped for ASCII.
pub(crate) fn ascii(text: &str) -> String {
    let mut out = text.to_string();
    for (from, to) in ASCII_SWAPS {
        out = out.replace(from, to);
    }
    out
}

/// One right-hand block of a list line: size, bar (or why it costs nothing), share.
struct Cols {
    size: bool,
    bar: usize,
    share: bool,
    /// Room left of it (after the left margin, before the gap).
    left: usize,
}

impl Cols {
    fn right(&self) -> usize {
        let mut w = 0;
        if self.size {
            w += SIZE_W;
        }
        if self.bar > 0 {
            w += 1 + self.bar;
        }
        if self.share {
            w += 1 + SHARE_W;
        }
        w
    }

    /// The bar is 16 wide from 120 columns, 14 below; narrower frames shrink it, then drop the
    /// share and the size before the names get less than 24 columns.
    fn fit(width: usize) -> Cols {
        let mut c = Cols {
            size: true,
            bar: if width >= 120 { 16 } else { 14 },
            share: true,
            left: 0,
        };
        let need = |c: &Cols| 1 + 24 + 1 + c.right() + 1;
        while need(&c) > width && c.bar > 0 {
            c.bar -= 1;
        }
        if need(&c) > width {
            c.share = false;
        }
        if need(&c) > width {
            c.size = false;
        }
        c.left = width.saturating_sub(1 + 1 + c.right() + 1);
        c
    }
}

impl App {
    /// `text` as drawn: as it is, or with harn's glyphs in ASCII.
    fn t(&self, text: &str) -> String {
        if self.opts.unicode {
            text.to_string()
        } else {
            ascii(text)
        }
    }

    fn g(&self) -> &'static Glyphs {
        glyphs(self.opts.unicode)
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        let h = area.height;
        let want = if h >= 40 { 5 } else { 4 };
        let panel_h = want.min(h.saturating_sub(9)).max(u16::from(h >= 8));
        let [head, rule1, list, rule2, panel, rule3, keys] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(panel_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let width = area.width as usize;
        // Cards take the rows above the key bar: the keys that work on them stay in view.
        let cards = Rect {
            height: area.height.saturating_sub(1),
            ..area
        };
        let blocked = self.card_blocked(area);
        self.blocked.set(blocked.is_some());
        let scrolls = self.card_scrolls(cards);
        frame.render_widget(Paragraph::new(self.header(width)), head);
        frame.render_widget(Paragraph::new(self.rule(width, None)), rule1);
        let footer = self.draw_list(frame, list);
        frame.render_widget(Paragraph::new(self.rule(width, footer)), rule2);
        self.draw_panel(frame, panel, blocked.as_deref());
        frame.render_widget(Paragraph::new(self.rule(width, None)), rule3);
        frame.render_widget(Paragraph::new(self.key_bar(width, scrolls)), keys);
        if blocked.is_none() {
            if let Some(width) = card_width(cards) {
                if let Some((title, body, answer)) = self.open_parts(usize::from(width - 2)) {
                    self.draw_box(frame, cards, &title, body, &answer);
                }
            }
        }
    }

    fn warn(&self) -> Style {
        if self.opts.color {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        }
    }

    fn grey(&self) -> Style {
        let style = Style::new().add_modifier(Modifier::DIM);
        if self.opts.color {
            style.fg(Color::DarkGray)
        } else {
            style
        }
    }

    /// Line 1: directory, agent, how many items, the total. Line 2: the biggest groups and the
    /// notes count. Never a warning's text.
    fn header(&self, width: usize) -> Vec<Line<'static>> {
        let screen = &self.tab().screen;
        let t = &screen.title;
        let total = if t.measured {
            format!("measured {}", size(t.total))
        } else if t.estimate {
            format!("~{}", size(t.total))
        } else {
            size(t.total)
        };
        let n = t.items;
        let version = t
            .version
            .as_deref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default();
        // Said before the count: no key has to be pressed to learn nothing will be written.
        let ro = if self.opts.readonly {
            " · read-only"
        } else {
            ""
        };
        let lefts = [
            format!(
                " {} · {}{version}{ro} · {n} items load before your first message",
                t.dir, t.agent
            ),
            format!(
                " {} · {}{ro} · {n} items load before your first message",
                t.dir, t.agent
            ),
            format!(" {}{ro} · {n} items load before your first message", t.dir),
            format!(" {}{ro} · {n} items before your first message", t.dir),
            format!(" {}{ro} · {n} items", t.dir),
            format!(" {} · {n} items", t.dir),
        ];
        let lefts: Vec<String> = lefts.iter().map(|l| self.t(l)).collect();
        let (left1, right1) = spread_parts(&lefts, &total, width, self.g());
        let first = format!("{left1}{right1}");

        let mut groups: Vec<(&str, u64, Option<u64>)> = screen
            .groups
            .iter()
            .filter(|g| g.kind != super::model::GroupKind::Builtin)
            .filter_map(|g| g.bytes.map(|b| (g.kind.short(), b, g.share)))
            .collect();
        groups.sort_by_key(|g| std::cmp::Reverse(g.1));
        groups.truncate(4);
        let parts: Vec<String> = groups
            .iter()
            .enumerate()
            .map(|(i, (name, bytes, share))| match (i, share) {
                (0, Some(p)) => format!("{name} {} ({p}%)", size(*bytes)),
                _ => format!("{name} {}", size(*bytes)),
            })
            .collect();
        let notes = match screen.notes.len() {
            0 => String::new(),
            1 => "! 1 note".to_string(),
            n => format!("! {n} notes"),
        };
        let dot = format!(" {} ", self.g().dot);
        let lefts: Vec<String> = (1..=parts.len())
            .rev()
            .map(|k| format!(" biggest: {}", parts[..k].join(&dot)))
            .chain(std::iter::once(String::new()))
            .collect();
        let (left, right) = spread_parts(&lefts, &notes, width, self.g());
        vec![
            Line::styled(first, Style::new().add_modifier(Modifier::BOLD)),
            Line::from(vec![Span::raw(left), Span::styled(right, self.warn())]),
        ]
    }

    /// A rule across the frame; `footer` sits in it after the first two columns.
    fn rule(&self, width: usize, footer: Option<String>) -> Line<'static> {
        let bar = if self.opts.unicode { "─" } else { "-" };
        match footer {
            Some(text) => {
                let text = self.t(&format!(" {text} "));
                let head = bar.repeat(2.min(width));
                let used = cols(&head) + cols(&text);
                let text = cut_right(&text, width.saturating_sub(2), self.g());
                Line::styled(
                    format!("{head}{text}{}", bar.repeat(width.saturating_sub(used))),
                    self.grey(),
                )
            }
            None => Line::styled(bar.repeat(width), self.grey()),
        }
    }

    /// Size of a list line (a group's or a row's; a line standing for entries listed
    /// elsewhere has none).
    fn line_bytes(&self, line: &ListLine) -> Option<u64> {
        let screen = &self.tab().screen;
        if line.listed.is_some() {
            return None;
        }
        match screen.line_row(line) {
            Some(row) => row.bytes,
            None => screen.groups[line.group].bytes,
        }
    }

    /// Draws the list; returns the footer when a filter or the frame hides lines.
    fn draw_list(&self, frame: &mut Frame, area: Rect) -> Option<String> {
        let tab = self.tab();
        let height = area.height as usize;
        let width = area.width as usize;
        self.list_height.set(height.max(1));
        if height == 0 || width == 0 {
            return None;
        }
        let mut offset = tab.offset.get();
        if tab.cursor < offset {
            offset = tab.cursor;
        } else if tab.cursor >= offset + height {
            offset = tab.cursor + 1 - height;
        }
        offset = offset.min(tab.lines.len().saturating_sub(height));
        tab.offset.set(offset);

        let cols_fit = Cols::fit(width);
        let max = tab
            .lines
            .iter()
            .filter_map(|l| self.line_bytes(l))
            .max()
            .unwrap_or(0);
        let name_w = self.name_widths(&cols_fit);
        let (open, closed) = if self.opts.unicode {
            ("▾", "▸")
        } else {
            ("v", ">")
        };

        let mut out: Vec<Line> = Vec::new();
        for (i, line) in tab.lines.iter().enumerate().skip(offset).take(height) {
            let screen = &tab.screen;
            let group = &screen.groups[line.group];
            // Entries listed elsewhere: where they are, in grey, in a leaf's name column; no
            // marker, no size.
            if let Some(listed) = &line.listed {
                let text = format!(" {}  {}", " ".repeat(2 * line.depth), self.t(&listed.text));
                let text = pad_right(&cut_right(&text, width, self.g()), width);
                let mut style = self.grey();
                if i == tab.cursor {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                out.push(Line::styled(text, style));
                continue;
            }
            let row = screen.line_row(line);
            let has_children = match row {
                Some(r) => !r.children.is_empty(),
                None => !group.rows.is_empty(),
            };
            let marker = if !has_children {
                " "
            } else if tab.expanded.contains(&line.key)
                || (!self.filter.trim().is_empty() && self.opened_by_filter(i))
            {
                open
            } else {
                closed
            };
            let indent = 2 * line.depth;
            let (bytes, share, absent) = match row {
                Some(r) => (r.bytes, r.share, r.absent),
                None => (group.bytes, group.share, group.absent),
            };
            // The Plugins group line has no size, bar or share: its rows overlap the listings.
            let overlap = row.is_none() && (group.overlap || group.all_off);
            // A row harn turned off that still resolves keeps its size and says `off`.
            let off = row.is_some_and(|r| r.state == State::Off);
            let right = if overlap {
                String::new()
            } else if off && bytes.is_some() {
                self.right_cells(&cols_fit, bytes, None, Some("off"), 0)
            } else {
                self.right_cells(&cols_fit, bytes, share, absent, max)
            };
            // `unmeasured` is wider than the size column: the names give it the room. A line
            // with nothing on the right (the Plugins group, all hooks off) has the whole row.
            let left_w = if overlap {
                width.saturating_sub(1)
            } else {
                cols_fit
                    .left
                    .saturating_sub(cols(&right).saturating_sub(cols_fit.right()))
            };
            let room = left_w.saturating_sub(indent + 2);
            let left = match row {
                None => {
                    let with = |count: &str| {
                        if count.is_empty() {
                            group.name.clone()
                        } else {
                            format!("{}  {count}", group.name)
                        }
                    };
                    let mut name = self.t(&with(&group.count_line));
                    if cols(&name) > room && !group.count_short.is_empty() {
                        name = self.t(&with(&group.count_short));
                    }
                    format!("{marker} {}", cut_right(&name, room, self.g()))
                }
                Some(r) => {
                    let source = self.t(&r.source);
                    let col = name_w.get(&(line.group, line.depth)).copied().flatten();
                    let text = match col {
                        Some(w)
                            if !source.is_empty()
                                && cols(&r.name).max(w) + 2 + cols(&source) <= room =>
                        {
                            format!("{}  {source}", pad_right(&r.name, w))
                        }
                        _ => cut_left(&r.name, room, self.g()),
                    };
                    format!("{}{marker} {text}", " ".repeat(indent))
                }
            };
            let text = format!(
                " {}{}{right}",
                pad_right(&left, left_w),
                if cols_fit.right() > 0 && !overlap {
                    " "
                } else {
                    ""
                }
            );
            let text = pad_right(&cut_right(&text, width, self.g()), width);
            let mut style = Style::new();
            if group.kind == super::model::GroupKind::Builtin
                || row.is_some_and(|r| r.state == State::Off)
            {
                style = self.grey();
            }
            if row.is_none() {
                style = style.add_modifier(Modifier::BOLD);
            }
            if i == tab.cursor {
                style = style.add_modifier(Modifier::REVERSED);
            }
            out.push(Line::styled(text, style));
        }
        if tab.lines.is_empty() {
            let text = if self.filter.trim().is_empty() {
                "  nothing loads here".to_string()
            } else {
                format!("  nothing matches \"{}\" (Esc clears)", self.filter)
            };
            out.push(Line::styled(text, self.grey()));
        }
        frame.render_widget(Paragraph::new(out), area);
        let hidden = tab.lines.len() > height;
        (hidden || !self.filter.trim().is_empty()).then(|| {
            let filter = if self.filter.trim().is_empty() {
                String::new()
            } else {
                format!(" · filter: \"{}\"", self.filter)
            };
            let shown = tab.lines.len().saturating_sub(offset).min(height);
            format!(
                "{} items · {shown} of {} lines shown{filter}",
                tab.screen.title.items,
                tab.lines.len()
            )
        })
    }

    /// While a filter is on, a line with a match below is drawn open.
    fn opened_by_filter(&self, index: usize) -> bool {
        let tab = self.tab();
        let Some(line) = tab.lines.get(index) else {
            return false;
        };
        tab.lines
            .get(index + 1)
            .is_some_and(|next| next.depth > line.depth)
    }

    /// The name column of each group and depth (None = no source column there): as wide as
    /// the widest name shown there, narrowed so the widest source still fits.
    fn name_widths(&self, c: &Cols) -> HashMap<(usize, usize), Option<usize>> {
        let tab = self.tab();
        let mut widest: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
        for line in &tab.lines {
            let Some(row) = tab.screen.line_row(line) else {
                continue;
            };
            let w = widest.entry((line.group, line.depth)).or_insert((0, 0));
            w.0 = w.0.max(cols(&row.name));
            w.1 = w.1.max(cols(&self.t(&row.source)));
        }
        widest
            .into_iter()
            .map(|((group, depth), (name, source))| {
                let room = c.left.saturating_sub(2 * depth + 2);
                let w = name.min(room.saturating_sub(source + 2));
                ((group, depth), (w >= 8 || w == name).then_some(w.max(1)))
            })
            .collect()
    }

    /// `  48 KB ████████        32%`, `      — not loaded`, `      ?`.
    fn right_cells(
        &self,
        c: &Cols,
        bytes: Option<u64>,
        share: Option<u64>,
        absent: Option<&str>,
        max: u64,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();
        if c.size {
            let (cell, w) = match (bytes, absent) {
                (Some(b), _) => (size(b), SIZE_W),
                (None, Some(_)) => (self.g().dash.to_string(), SIZE_W),
                (None, None) => (UNMEASURED.to_string(), SIZE_W.max(UNMEASURED.len())),
            };
            parts.push(pad_left(&cell, w));
        }
        if c.bar > 0 {
            let cell = match (bytes, absent) {
                // A size and a word: the word in the bar's place (a row turned off that still
                // resolves).
                (Some(_), Some(word)) => format!(" {word}"),
                (Some(b), None) => bar(b, max, c.bar, self.opts.unicode),
                (None, Some(word)) => format!(" {word}"),
                (None, None) => String::new(),
            };
            // A word wider than the bar runs on over the share column (the names give it room),
            // never cut.
            if cols(&cell) > c.bar {
                let room = c.bar + if c.share { 1 + SHARE_W } else { 0 };
                parts.push(pad_right(&cell, room));
                return parts.join(" ");
            }
            parts.push(pad_right(&cell, c.bar));
        }
        if c.share {
            let cell = match (bytes, share) {
                // Some bytes, under a percent: not nothing.
                (Some(b), Some(0)) if b > 0 => "<1%".to_string(),
                (Some(_), Some(p)) => format!("{p}%"),
                _ => String::new(),
            };
            parts.push(pad_left(&cell, SHARE_W));
        }
        parts.join(" ")
    }

    /// The detail of the cursor line.
    fn cursor_detail(&self) -> Option<&Detail> {
        let line = self.cursor_line()?;
        let screen = &self.tab().screen;
        if let Some(listed) = &line.listed {
            return Some(&listed.detail);
        }
        Some(match screen.line_row(line) {
            Some(row) => &row.detail,
            None => &screen.groups[line.group].detail,
        })
    }

    /// The panel: a message first (until the next key), then the cursor line's sentences.
    /// The panel: one message row (a card too big for the frame says so there), then the
    /// cursor line's sentences.
    fn draw_panel(&self, frame: &mut Frame, area: Rect, blocked: Option<&str>) {
        let height = area.height as usize;
        let width = (area.width as usize).saturating_sub(1).max(1);
        if height == 0 {
            return;
        }
        let mut lines: Vec<(String, Style)> = Vec::new();
        let error = if self.opts.color {
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        };
        let message = match (blocked, &self.message) {
            (Some(text), _) => Some((text.to_string(), error)),
            (None, Some(message)) => Some((
                message.text(),
                match message.tone {
                    Tone::Error => error,
                    Tone::Info => Style::new().add_modifier(Modifier::BOLD),
                },
            )),
            (None, None) => None,
        };
        // Exactly one row: the detail keeps the rest of the panel.
        if let Some((text, style)) = message {
            lines.push((one_row(&self.t(&text), width), style));
        }
        lines.truncate(height);
        let room = height - lines.len();
        if let Some(detail) = self.cursor_detail() {
            let what = match &detail.what_short {
                Some(short) if cols(&self.t(&detail.what)) > width => short.clone(),
                _ => detail.what.clone(),
            };
            let bold = Style::new().add_modifier(Modifier::BOLD);
            let mut what = wrap_words(&self.t(&what), width);
            let brings = self.t(&detail.brings);
            let sentences: Vec<String> = detail.sentences().iter().map(|s| self.t(s)).collect();
            let wrap = |text: &str| -> Vec<String> {
                if text.is_empty() {
                    Vec::new()
                } else {
                    wrap_words(text, width)
                }
            };
            // The most sentences that fit, each one whole: `brings` on its own line, else in
            // one paragraph with the rest; sentences go from the end, never a cut one.
            let mut body: Option<Vec<String>> = None;
            for keep in (0..=sentences.len()).rev() {
                let tail = sentences[..keep].join(" ");
                let apart: Vec<String> = wrap(&brings).into_iter().chain(wrap(&tail)).collect();
                if what.len() + apart.len() <= room {
                    body = Some(apart);
                    break;
                }
                let joined = [brings.as_str(), tail.as_str()]
                    .iter()
                    .filter(|t| !t.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" ");
                let together = wrap(&joined);
                if what.len() + together.len() <= room {
                    body = Some(together);
                    break;
                }
            }
            what.truncate(room);
            for l in what {
                lines.push((l, bold));
            }
            for l in body.unwrap_or_default() {
                lines.push((l, Style::new()));
            }
        }
        let out: Vec<Line> = lines
            .into_iter()
            .map(|(text, style)| Line::styled(format!(" {text}"), style))
            .collect();
        frame.render_widget(Paragraph::new(out), area);
    }

    /// The keys valid now, in words; the least used go first when the frame is narrow. Under
    /// `--readonly` the write keys (d, o, u, e) are left out: the `?` card says they are off.
    /// `scrolls` = the open card's body is taller than the card: ↑↓ move it.
    fn key_bar(&self, width: usize, scrolls: bool) -> Line<'static> {
        let ro = self.opts.readonly;
        let u = self.opts.unicode;
        // (key, words, enabled, priority: higher stays longer)
        let mut keys: Vec<(String, String, bool, u8)> = Vec::new();
        let mut push = |k: &str, w: &str, enabled: bool, p: u8| {
            keys.push((k.to_string(), w.to_string(), enabled, p));
        };
        match &self.mode {
            // A card too big for the frame: closing it is all its keys do.
            Mode::Card(_) | Mode::Undo { .. } if self.blocked.get() => {
                push("Esc", "close", true, 9);
            }
            Mode::Filter => {
                let prompt = format!("/{}_", self.filter);
                let mut spans = vec![Span::styled(
                    format!(" {prompt}"),
                    Style::new().add_modifier(Modifier::BOLD),
                )];
                let hint = format!(
                    "   Enter keep   Esc clear   {} move",
                    if u { "↑↓" } else { "up/down" }
                );
                spans.push(Span::raw(cut_right(
                    &hint,
                    width.saturating_sub(cols(&prompt) + 1),
                    self.g(),
                )));
                return Line::from(spans);
            }
            Mode::Card(card) => match card.as_ref() {
                Card::Off { list: true, .. } => {
                    if scrolls {
                        push(if u { "↑↓" } else { "j/k" }, "scroll", true, 5);
                    }
                    push("Esc", "back", true, 9);
                }
                Card::Off { card, .. } => {
                    push("Enter", "apply", true, 9);
                    if !card.leave_list.is_empty() {
                        push(if u { "→" } else { "l" }, "what leaves", true, 5);
                    }
                    push("Esc", "cancel", true, 9);
                }
                Card::On { .. } => {
                    push("Enter", "apply", true, 9);
                    push("Esc", "cancel", true, 9);
                }
                Card::Where { .. } | Card::Keys | Card::Notes => {
                    if scrolls {
                        push(if u { "↑↓" } else { "j/k" }, "scroll", true, 5);
                    }
                    push("Esc", "close", true, 9);
                }
            },
            Mode::Undo { .. } => {
                push("Enter", "undo it", true, 9);
                push("Esc", "keep it", true, 9);
            }
            Mode::Normal => {
                let line = self.cursor_line().cloned();
                let row = self.cursor_row();
                let group = line.as_ref().map(|l| &self.tab().screen.groups[l.group]);
                push(if u { "↑↓" } else { "j/k" }, "move", true, 9);
                let folds = line.as_ref().is_some_and(|l| {
                    l.depth > 0
                        || row.map_or_else(
                            || group.is_some_and(|g| !g.rows.is_empty()),
                            |r| !r.children.is_empty(),
                        )
                });
                if folds {
                    push(if u { "→←" } else { "l/h" }, "open/close", true, 8);
                }
                push("/", "search", true, 3);
                let can_off = match row {
                    Some(r) => {
                        r.state == State::On
                            && self.cursor_item().is_some_and(|i| {
                                i.knobs.iter().any(|k| {
                                    crate::plan::knob_scope(k) == Some(crate::model::Scope::Local)
                                })
                            })
                    }
                    // Entries listed elsewhere: d acts on them there.
                    None if line.as_ref().is_some_and(|l| l.listed.is_some()) => false,
                    // The Built-in group has no switch of its own.
                    None => group.is_some_and(|g| {
                        g.can_turn_off && g.kind != super::model::GroupKind::Builtin
                    }),
                };
                if can_off && !ro {
                    push("d", "turn off", true, 7);
                }
                // Only what harn turned off here: a hook your own settings turned off has
                // nothing for `o` to reverse.
                let can_on = row.is_some_and(|r| {
                    r.id.as_ref()
                        .is_some_and(|id| self.tab().journaled.contains_key(id))
                });
                if can_on && !ro {
                    push("o", "turn back on", true, 7);
                }
                if self.last_live.is_some() && !ro {
                    push("u", "undo", true, 6);
                }
                if row.is_some_and(|r| r.id.is_some()) {
                    push("w", "where from", true, 5);
                }
                if self.cursor_item().is_some_and(editable) && !ro {
                    push("e", "edit", true, 2);
                }
                if !self.tab().screen.notes.is_empty() {
                    push("i", "notes", true, 1);
                }
                push("?", "keys", true, 10);
                push("q", "quit", true, 10);
            }
        }
        let len = |k: &[(String, String, bool, u8)], sep: usize| -> usize {
            1 + k
                .iter()
                .map(|(key, words, _, _)| cols(key) + 1 + cols(words))
                .sum::<usize>()
                + sep * k.len().saturating_sub(1)
        };
        // Three spaces between keys, two before any key is left out.
        let sep = if len(&keys, 3) <= width { 3 } else { 2 };
        while keys.len() > 1 && len(&keys, sep) > width {
            let lowest = keys
                .iter()
                .enumerate()
                .min_by_key(|(i, k)| (k.3, std::cmp::Reverse(*i)))
                .map(|(i, _)| i)
                .expect("keys is not empty");
            keys.remove(lowest);
        }
        let mut spans = vec![Span::raw(" ")];
        for (i, (key, words, enabled, _)) in keys.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" ".repeat(sep)));
            }
            let (key_style, words_style) = if enabled {
                (Style::new().add_modifier(Modifier::BOLD), Style::new())
            } else {
                (self.grey(), self.grey())
            };
            spans.push(Span::styled(key, key_style));
            spans.push(Span::styled(format!(" {words}"), words_style));
        }
        Line::from(spans)
    }

    // ---- cards ------------------------------------------------------------------------------

    /// A card's title, body lines (wrapped to `inner` columns) and answer line.
    fn card_parts(&self, card: &Card, inner: usize) -> (String, Vec<String>, String) {
        match card {
            Card::Off {
                card, list: false, ..
            } => (
                card.title.clone(),
                self.off_lines(card, inner),
                format!("Enter apply {} Esc", self.g().dot),
            ),
            Card::Off {
                card,
                staged,
                list: true,
            } => (
                format!("What leaves with {}", staged.name),
                // Headings (`at start (6)`) flush, their entries indented under them.
                card.leave_list
                    .iter()
                    .flat_map(|l| match l.strip_prefix("  ") {
                        Some(entry) => indent_wrap("  ", &self.t(entry), inner),
                        None => indent_wrap("", &self.t(l), inner),
                    })
                    .collect(),
                "Esc back".to_string(),
            ),
            Card::On { name, files, .. } => {
                let (files, were) = match files.len() {
                    0 => ("the files harn changed".to_string(), "they were"),
                    1 => (files[0].clone(), "it was"),
                    _ => (files.join(", "), "they were"),
                };
                let mut body = labelled(
                    "Writes",
                    &self.t(&format!("{files} back as {were} before harn's change")),
                    inner,
                    None,
                );
                body.extend(labelled(
                    "Returns",
                    &self.t(&format!("{name} (next session)")),
                    inner,
                    None,
                ));
                body.extend(labelled("Undo", "d turns it off again", inner, None));
                (
                    format!("Turn {name} back on"),
                    body,
                    format!("Enter apply {} Esc", self.g().dot),
                )
            }
            Card::Where { title, lines } => {
                let mut body = Vec::new();
                for line in lines {
                    let line = self.t(line);
                    let (head, rest) = match line.find("  ") {
                        Some(at) if at <= 8 => {
                            let rest = line[at..].trim_start();
                            let head_w = line.len() - rest.len();
                            (line[..head_w].to_string(), rest.to_string())
                        }
                        _ => (String::new(), line.clone()),
                    };
                    body.extend(indent_wrap(&head, &rest, inner));
                }
                (title.clone(), body, "Esc close".to_string())
            }
            Card::Keys => (
                "Keys".to_string(),
                self.keys_lines(inner),
                "Esc close".to_string(),
            ),
            Card::Notes => {
                let body = self
                    .tab()
                    .screen
                    .notes
                    .iter()
                    .enumerate()
                    .flat_map(|(i, n)| {
                        indent_wrap(&format!("{}  ", i + 1), &self.t(&n.text), inner)
                    })
                    .collect();
                ("Notes".to_string(), body, "Esc close".to_string())
            }
        }
    }

    /// Why the open card cannot be drawn in `area`, as the message row says it;
    /// None when it can.
    pub(crate) fn card_blocked(&self, area: Rect) -> Option<String> {
        let (min_w, min_h) = CARD_MIN;
        let action = match &self.mode {
            Mode::Card(card) => matches!(
                card.as_ref(),
                Card::Off { list: false, .. } | Card::On { .. }
            ),
            Mode::Undo { .. } => true,
            Mode::Normal | Mode::Filter => return None,
        };
        let need_h = match self.open_parts(usize::from(area.width.min(CARD_MAX).saturating_sub(2)))
        {
            // Borders, answer line and the key bar row below the card.
            Some((_, body, _)) if action && area.width >= min_w => {
                u16::try_from(body.len() + 4).unwrap_or(u16::MAX).max(min_h)
            }
            _ => min_h,
        };
        (area.width < min_w || area.height < need_h)
            .then(|| format!("terminal too small for this card; needs at least {min_w}×{need_h}"))
    }

    /// Whether the open card's body is taller than the rows it gets in `area` (it scrolls).
    fn card_scrolls(&self, area: Rect) -> bool {
        let Some(width) = card_width(area) else {
            return false;
        };
        self.open_parts(usize::from(width - 2))
            .is_some_and(|(_, body, _)| body.len() > card_rows(area))
    }

    /// Title, body (wrapped to `inner`) and answer of the open card or the `u` card; None when
    /// none is open.
    fn open_parts(&self, inner: usize) -> Option<(String, Vec<String>, String)> {
        match &self.mode {
            Mode::Card(card) => Some(self.card_parts(card, inner)),
            Mode::Undo { entry } => Some((
                "Undo the last change".to_string(),
                self.undo_lines(entry)
                    .iter()
                    .flat_map(|l| indent_wrap("", &self.t(l), inner))
                    .collect(),
                format!("Enter undo {} Esc keep", self.g().dot),
            )),
            Mode::Normal | Mode::Filter => None,
        }
    }

    /// A card over `area`: `title` in its top border, `body` (scrolled), `answer` bold at the
    /// bottom right, the rows it takes blank across the frame.
    fn draw_box(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        body: Vec<String>,
        answer: &str,
    ) {
        let Some(width) = card_width(area) else {
            return;
        };
        let inner = usize::from(width - 2);
        let title = cut_right(&self.t(title), inner.saturating_sub(2), self.g());
        let rows = card_rows(area);
        let max = body.len().saturating_sub(rows);
        self.card_max.set(max);
        let scroll = self.card_scroll.min(max);
        let mut text: Vec<Line> = body
            .into_iter()
            .skip(scroll)
            .take(rows)
            .map(Line::from)
            .collect();
        let answer = self.t(answer);
        text.push(Line::styled(
            pad_left(&format!("{answer} "), inner),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        let height = u16::try_from(text.len() + 2)
            .unwrap_or(u16::MAX)
            .min(area.height);
        let popup = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        };
        // The band the card takes is blank across the whole frame: no half-words of the list
        // beside it.
        frame.render_widget(Clear, band(area, popup));
        frame.render_widget(
            Paragraph::new(text).block(bordered(&format!(" {title} "), self.opts.unicode)),
            popup,
        );
    }

    /// The `d` card's body: Writes, Leaves (with `→ list`), Saves, Undo, then the plan's notes.
    fn off_lines(&self, card: &OffCard, inner: usize) -> Vec<String> {
        let mut body = Vec::new();
        if let Some(lead) = &card.lead {
            body.extend(indent_wrap("", &self.t(lead), inner));
        }
        for (i, w) in card.writes.iter().enumerate() {
            let label = if i == 0 { "Writes" } else { "" };
            body.extend(labelled(label, &self.t(w), inner, None));
        }
        let list = format!("{} list", self.g().arrow);
        let hint = (!card.leave_list.is_empty()).then_some(list.as_str());
        body.extend(labelled("Leaves", &self.t(&card.leaves), inner, hint));
        body.extend(labelled("Saves", &self.t(&card.saves), inner, None));
        body.extend(labelled("Undo", &self.t(&card.undo), inner, None));
        for note in &card.notes {
            body.extend(labelled("Note", &self.t(note), inner, None));
        }
        body
    }

    /// The `?` card: three key/words pairs per line, aligned; one pair per line when narrower.
    fn keys_lines(&self, inner: usize) -> Vec<String> {
        let u = self.opts.unicode;
        let dot = self.g().dot;
        let rows: [[(&str, String); 3]; 5] = [
            [
                (if u { "↑ ↓" } else { "j k" }, "move".to_string()),
                (
                    if u { "→  ←" } else { "l  h" },
                    "open / close group".to_string(),
                ),
                ("/", "search (Esc clears)".to_string()),
            ],
            [
                ("g G", "top / bottom".to_string()),
                ("s", format!("sort: size {dot} name")),
                ("i", "notes".to_string()),
            ],
            [
                ("d", "turn off".to_string()),
                ("o", "turn back on".to_string()),
                ("u", "undo last change".to_string()),
            ],
            [
                ("w", "where from".to_string()),
                ("e", "edit the file ($EDITOR)".to_string()),
                ("t", "let the AI trim".to_string()),
            ],
            [
                ("?", "this card".to_string()),
                ("q Esc", "quit / close".to_string()),
                ("r", "refresh".to_string()),
            ],
        ];
        let width = |col: usize, key: bool| {
            rows.iter()
                .map(|r| if key { cols(r[col].0) } else { cols(&r[col].1) })
                .max()
                .unwrap_or(0)
        };
        let (k, d): (Vec<usize>, Vec<usize>) =
            (0..3).map(|c| (width(c, true), width(c, false))).unzip();
        let gaps = [2, 2, 3];
        // A space before the border, as on every card line.
        let need = 1 + (0..3).map(|c| k[c] + gaps[c] + d[c]).sum::<usize>() + 3 * 2 + 1;
        // The write keys are left out of the key bar under `--readonly`: here they stay, and
        // the card says they are off.
        let note = self
            .opts
            .readonly
            .then(|| indent_wrap("", "read-only: d, o, u and e are off", inner));
        if need <= inner {
            return rows
                .iter()
                .map(|r| {
                    let mut line = String::from(" ");
                    for (c, (key, words)) in r.iter().enumerate() {
                        if c > 0 {
                            line.push_str("   ");
                        }
                        line.push_str(&pad_right(key, k[c] + gaps[c]));
                        line.push_str(&if c == 2 {
                            words.clone()
                        } else {
                            pad_right(words, d[c])
                        });
                    }
                    line
                })
                .chain(note.into_iter().flatten())
                .collect();
        }
        let kw = k.iter().copied().max().unwrap_or(0);
        rows.iter()
            .flat_map(|r| r.iter())
            .flat_map(|(key, words)| indent_wrap(&pad_right(key, kw + 2), words, inner))
            .chain(note.into_iter().flatten())
            .collect()
    }
}

/// `label` in the card's label column, `text` wrapped after it; `hint` right-aligned on the
/// first line when it fits there, else on the last, else on a line of its own: never dropped.
fn labelled(label: &str, text: &str, inner: usize, hint: Option<&str>) -> Vec<String> {
    let room = inner.saturating_sub(2 + LABEL_W).max(1);
    let mut lines = wrap_words(text, room);
    let mut out = Vec::new();
    let first = lines.remove(0);
    out.push(format!(" {}{first}", pad_right(label, LABEL_W)));
    for line in lines {
        out.push(format!(" {}{line}", " ".repeat(LABEL_W)));
    }
    if let Some(hint) = hint {
        let fits = |line: &String| cols(line) + 3 + cols(hint) < inner;
        let at = if fits(&out[0]) {
            Some(0)
        } else {
            out.len().checked_sub(1).filter(|last| fits(&out[*last]))
        };
        let at = at.unwrap_or_else(|| {
            out.push(" ".to_string());
            out.len() - 1
        });
        let used = cols(&out[at]);
        let gap = inner.saturating_sub(1 + used + cols(hint));
        out[at] = format!("{}{}{hint}", out[at], " ".repeat(gap));
    }
    out
}

/// `head` then `text` wrapped, later lines indented under `text`'s start.
fn indent_wrap(head: &str, text: &str, inner: usize) -> Vec<String> {
    let indent = cols(head);
    let room = inner.saturating_sub(2 + indent).max(1);
    wrap_words(text, room)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                format!(" {head}{line}")
            } else {
                format!(" {}{line}", " ".repeat(indent))
            }
        })
        .collect()
}

/// The first of `lefts` that fits with `right` right-aligned (three spaces between at least,
/// one column of margin), padded to `width`, and `right` with its margin; the last of `lefts`
/// cut when none fits.
fn spread_parts(lefts: &[String], right: &str, width: usize, g: &Glyphs) -> (String, String) {
    let right = if right.is_empty() {
        String::new()
    } else {
        format!("{right} ")
    };
    let gap = if right.is_empty() { 0 } else { 3 };
    let room = width.saturating_sub(cols(&right));
    if room == 0 {
        return (String::new(), cut_right(&right, width, g));
    }
    let left = lefts
        .iter()
        .find(|l| cols(l) + gap <= room)
        .cloned()
        .unwrap_or_else(|| {
            cut_right(
                lefts.last().map(String::as_str).unwrap_or(""),
                room.saturating_sub(gap.min(1)),
                g,
            )
        });
    (pad_right(&left, room), right)
}

/// A bar of `width` cells for `bytes` of `max`, in eighth blocks (`#` per cell in ASCII).
pub(crate) fn bar(bytes: u64, max: u64, width: usize, unicode: bool) -> String {
    if max == 0 || width == 0 {
        return String::new();
    }
    let scale = |unit: u128| {
        let n = (u128::from(bytes) * unit * width as u128 + u128::from(max) / 2) / u128::from(max);
        usize::try_from(n).unwrap_or(usize::MAX)
    };
    if !unicode {
        return "#".repeat(scale(1).min(width));
    }
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = scale(8).min(width * 8);
    let mut out = "█".repeat(eighths / 8);
    out.push_str(PARTS[eighths % 8]);
    out
}

/// `text` right-aligned in `width` terminal columns.
fn pad_left(text: &str, width: usize) -> String {
    format!("{}{text}", " ".repeat(width.saturating_sub(cols(text))))
}

/// `text` left-aligned in `width` terminal columns.
fn pad_right(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(cols(text))))
}

/// `+-|` borders for `--no-unicode`.
const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// A bordered box: line drawing, or `+-|` under `--no-unicode`.
fn bordered(title: &str, unicode: bool) -> Block<'static> {
    let block = Block::bordered().title(title.to_string());
    if unicode {
        block
    } else {
        block.border_set(ASCII_BORDER)
    }
}

/// The width a card takes in `area`; None when `area` has no room for one.
fn card_width(area: Rect) -> Option<u16> {
    let width = area.width.min(CARD_MAX);
    (width >= 8 && area.height >= 3).then_some(width)
}

/// Body rows a card has in `area`: its height less two borders and the answer line.
fn card_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(2)).saturating_sub(1)
}

/// The rows `popup` takes in `area`, across the full width of `area`.
fn band(area: Rect, popup: Rect) -> Rect {
    Rect {
        x: area.x,
        y: popup.y,
        width: area.width,
        height: popup.height,
    }
}

/// Terminal columns of `text`, grapheme by grapheme (the way ratatui lays out cells).
pub(crate) fn cols(text: &str) -> usize {
    text.graphemes(true).map(UnicodeWidthStr::width).sum()
}

/// The head of the ellipsis that fits `max` columns (`…`, or `.` of `...` in one column): what a
/// cut shows when not one grapheme of the text fits.
fn ellipsis_in(max: usize, g: &Glyphs) -> String {
    let mut out = String::new();
    for grapheme in g.ellipsis.graphemes(true) {
        if cols(&out) + grapheme.width() > max {
            break;
        }
        out.push_str(grapheme);
    }
    out
}

/// `text` in at most `max` columns: whole when it fits, else its head and the ellipsis; never
/// blank for a non-empty `text` when `max` > 0.
pub(crate) fn cut_right(text: &str, max: usize, g: &Glyphs) -> String {
    if cols(text) <= max {
        return text.to_string();
    }
    let ellipsis = cols(g.ellipsis);
    let (room, mark) = if max > ellipsis {
        (max - ellipsis, g.ellipsis)
    } else {
        (max, "")
    };
    let mut out = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let w = grapheme.width();
        if used + w > room {
            break;
        }
        out.push_str(grapheme);
        used += w;
    }
    if out.is_empty() && mark.is_empty() {
        return ellipsis_in(max, g);
    }
    out.push_str(mark);
    out
}

/// The tail of `text` in `max` columns behind an ellipsis; `text` when it fits. A cut result is
/// exactly `max` columns (a space after the ellipsis takes the column a wide char cannot) and
/// never blank: when no grapheme fits, the ellipsis alone.
pub(crate) fn cut_left(text: &str, max: usize, g: &Glyphs) -> String {
    if cols(text) <= max {
        return text.to_string();
    }
    let ellipsis = cols(g.ellipsis);
    let (room, mark) = if max > ellipsis {
        (max - ellipsis, g.ellipsis)
    } else {
        (max, "")
    };
    let mut tail: Vec<&str> = Vec::new();
    let mut used = 0;
    for grapheme in text.graphemes(true).rev() {
        let w = grapheme.width();
        if used + w > room {
            break;
        }
        tail.push(grapheme);
        used += w;
    }
    tail.reverse();
    if tail.is_empty() && mark.is_empty() {
        let mark = ellipsis_in(max, g);
        return format!("{mark}{}", " ".repeat(max - cols(&mark)));
    }
    format!("{mark}{}{}", " ".repeat(room - used), tail.concat())
}

/// `text` in one row of `width` columns: as many whole sentences from its start as fit (a
/// sentence ends at `. ` or at the ` · ` between a message's clauses); when not even the first
/// fits, that sentence cut at a word boundary, with no ellipsis.
pub(crate) fn one_row(text: &str, width: usize) -> String {
    let mut pieces: Vec<(&str, &str)> = Vec::new();
    let mut rest = text;
    loop {
        let next = [". ", " · ", " | "]
            .iter()
            .filter_map(|sep| rest.find(sep).map(|at| (at, *sep)))
            .min_by_key(|(at, _)| *at);
        match next {
            // `. ` keeps its full stop with the sentence; a clause dot goes between.
            Some((at, ". ")) => {
                pieces.push((&rest[..=at], " "));
                rest = &rest[at + 2..];
            }
            Some((at, sep)) => {
                pieces.push((&rest[..at], sep));
                rest = &rest[at + sep.len()..];
            }
            None => {
                pieces.push((rest, ""));
                break;
            }
        }
    }
    // An empty clause (` · ready` starts with one) is no sentence to keep.
    pieces.retain(|(piece, _)| !piece.trim().is_empty());
    if pieces.is_empty() {
        return String::new();
    }
    let mut kept = String::new();
    let mut between = "";
    for (piece, sep) in &pieces {
        let next = format!("{kept}{between}{piece}");
        if cols(&next) > width {
            break;
        }
        kept = next;
        between = sep;
    }
    if kept.is_empty() {
        return wrap_words(pieces[0].0, width).swap_remove(0);
    }
    kept
}

/// Where a word too wide for a row may break: right after one of these.
fn breaks_after(grapheme: &str) -> bool {
    matches!(grapheme, "/" | "›" | "." | "-" | "_" | ":" | "," | ">")
}

/// Where the word `text` starts with ends: at the first space outside a backtick span, so an
/// inline command (`harn verify --run`) is one word (a tick with no closing one spans nothing),
/// not between a number and its unit (`32 KB:`, `0.8 KB;`, `5 %`), and not between a count
/// and the word it counts (`1 hook)`, `(2 hooks),`, `32 items`).
fn word_end(text: &str) -> usize {
    let mut at = 0;
    loop {
        let space = text[at..].find(' ').map_or(text.len(), |i| at + i);
        let open = text[..space].matches('`').count() % 2 == 1;
        if let (Some(close), true) = (text[space..].find('`'), open) {
            at = space + close + 1;
            continue;
        }
        let word = &text[text[..space].rfind(' ').map_or(0, |i| i + 1)..space];
        let next = text.get(space + 1..).unwrap_or("");
        let next = &next[..next.find(' ').unwrap_or(next.len())];
        let counts = is_count(word) && next.starts_with(|c: char| c.is_alphabetic());
        if text[space..].starts_with(' ') && (is_number(word) && is_unit(next) || counts) {
            at = space + 1 + next.len();
            continue;
        }
        return space;
    }
}

/// `32`, `0.8`, `~15`, `1,024`: a word that is a number.
fn is_number(word: &str) -> bool {
    let digits = word.trim_start_matches(['~', '(', '+', '-']);
    digits.chars().any(|c| c.is_ascii_digit())
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

/// `1`, `32`, `(2`: a count from 1 to 999 (a year, an id, `0` or `00` is no count).
fn is_count(word: &str) -> bool {
    let digits = word.trim_start_matches('(');
    (1..=3).contains(&digits.len())
        && digits.chars().all(|c| c.is_ascii_digit())
        && digits.parse::<u32>().is_ok_and(|n| (1..=999).contains(&n))
}

/// `KB`, `KB:`, `B;`, `%)`: a size or percent unit, with what punctuation follows it.
fn is_unit(word: &str) -> bool {
    matches!(
        word.trim_end_matches([':', ';', ',', '.', ')']),
        "B" | "KB" | "MB" | "GB" | "%"
    )
}

/// `text` in rows of at most `width` columns, broken at spaces (a run of spaces inside a row is
/// kept) but never inside a backtick span. A word wider than a row breaks after a `/` (or `›`,
/// `.`, `-`, `_`, `:`), so a path wraps on its separators; only a word with none of those in
/// reach is cut between graphemes.
pub(crate) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start_matches(' ');
        let spaces = rest.len() - trimmed.len();
        if trimmed.is_empty() {
            break;
        }
        let end = word_end(trimmed);
        let word = &trimmed[..end];
        rest = &trimmed[end..];
        let gap = if line.is_empty() { 0 } else { spaces };
        if cols(&line) + gap + cols(word) <= width {
            line.push_str(&" ".repeat(gap));
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            rows.push(std::mem::take(&mut line));
        }
        let mut word = word.to_string();
        while cols(&word) > width {
            let graphemes: Vec<&str> = word.graphemes(true).collect();
            let mut used = 0;
            let mut fit = 0;
            let mut cut = 0;
            for (i, g) in graphemes.iter().enumerate() {
                if used + g.width() > width {
                    break;
                }
                used += g.width();
                fit = i + 1;
                if breaks_after(g) {
                    cut = i + 1;
                }
            }
            let at = if cut > 0 { cut } else { fit.max(1) };
            rows.push(graphemes[..at].concat());
            word = graphemes[at..].concat();
        }
        line = word;
    }
    if !line.is_empty() || rows.is_empty() {
        rows.push(line);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_without_spaces_breaks_between_graphemes_and_loses_nothing() {
        let word = "計画を確認する👍🏽family👨‍👩‍👧done—cafe\u{0301}漢字漢字漢字";
        assert!(
            word.contains("e\u{0301}") && !word.contains('é'),
            "decomposed"
        );
        for width in 2..=12 {
            let rows = wrap_words(word, width);
            assert_eq!(rows.concat(), word, "width {width}");
            for row in &rows {
                assert!(cols(row) <= width, "{width}: {row:?}");
                assert!(!row.starts_with('\u{0301}'), "{row:?}");
            }
        }
        assert_eq!(wrap_words("計画を", 4), ["計画", "を"]);
    }

    #[test]
    fn words_wrap_at_spaces_and_paths_at_their_separators() {
        let text = "d turns it off for this project only (.claude/settings.local.json); the skill \
                    listing shrinks too";
        for width in 10..=80 {
            for row in wrap_words(text, width) {
                assert!(cols(&row) <= width, "{width}: {row}");
                // Never a word cut in two: every row piece is a whole word of the text, or a
                // path piece ending at a separator.
                for word in row.split(' ').filter(|w| !w.is_empty()) {
                    assert!(
                        text.split(' ').any(|w| w == word)
                            || word.ends_with('/')
                            || word.ends_with('.')
                            || text.contains(word),
                        "{width}: {word}"
                    );
                }
            }
        }
        assert_eq!(
            wrap_words(
                "files: ~/.claude/plugins/cache/claude-plugins-official/slack/1.4.1",
                30
            ),
            [
                "files:",
                "~/.claude/plugins/cache/",
                "claude-plugins-official/slack/",
                "1.4.1"
            ]
        );
        assert_eq!(
            wrap_words("a = true   (every project)", 40),
            ["a = true   (every project)"]
        );
        assert_eq!(wrap_words("", 5), [""]);
    }

    #[test]
    fn a_number_keeps_its_unit_on_its_row() {
        // Each width would break between the number and its unit.
        let text = "Turning it off frees 32 KB: its entries in the listing.";
        assert_eq!(
            wrap_words(text, "Turning it off frees 32".len()),
            [
                "Turning it off frees",
                "32 KB: its entries in",
                "the listing."
            ]
        );
        assert_eq!(
            wrap_words("That frees 0.8 KB; the skill listing", 14),
            ["That frees", "0.8 KB; the", "skill listing"]
        );
        assert_eq!(wrap_words("frees 4.0 KB.", 8), ["frees", "4.0 KB."]);
        assert_eq!(wrap_words("is ~15 MB now", 6), ["is", "~15 MB", "now"]);
        // A count keeps the word it counts, wherever it wraps.
        assert_eq!(wrap_words("frees 32 items", 8), ["frees", "32 items"]);
        assert_eq!(
            wrap_words("2 at start (2 hooks), 2 later", 12),
            ["2 at start", "(2 hooks),", "2 later"]
        );
        // Not a count: a year, an arrow after a number.
        assert_eq!(wrap_words("in 2026 now", 7), ["in 2026", "now"]);
        assert_eq!(wrap_words("00 items", 5), ["00", "items"]);
        assert_eq!(wrap_words("000 x", 4), ["000", "x"]);
        assert_eq!(wrap_words("(83 → 77)", 4), ["(83", "→", "77)"]);
    }

    #[test]
    fn an_inline_command_wraps_as_one_word() {
        let text = "Its size is unknown until `harn verify --run` measures a session.";
        for width in 20..=text.len() {
            for row in wrap_words(text, width) {
                assert_eq!(row.matches('`').count() % 2, 0, "{width}: {row:?}");
            }
        }
        assert_eq!(
            wrap_words(text, 40),
            [
                "Its size is unknown until",
                "`harn verify --run` measures a session.",
            ]
        );
        // Punctuation after the closing tick stays with it; a tick with no closing one spans
        // nothing.
        assert_eq!(
            wrap_words("needs `harn verify --run`.", 20),
            ["needs", "`harn verify --run`."]
        );
        assert_eq!(wrap_words("a `b c d", 4), ["a `b", "c d"]);
    }

    #[test]
    fn a_message_row_keeps_whole_sentences_and_cuts_a_lone_one_at_a_word() {
        let text = "undone: alpha back · .claude/settings.local.json restored byte for byte";
        assert_eq!(one_row(text, 80), text);
        assert_eq!(one_row(text, 40), "undone: alpha back");
        assert_eq!(one_row(&ascii(text), 40), "undone: alpha back");
        assert_eq!(one_row("One. Two words. Three", 15), "One. Two words.");
        assert_eq!(one_row("One. Two words. Three", 3), "One");
        // A first sentence wider than the row: cut at a word, never an ellipsis.
        assert_eq!(
            one_row("terminal too small for this card", 20),
            "terminal too small"
        );
        assert_eq!(one_row("", 10), "");
        // An empty leading clause does not hide what fits.
        assert_eq!(one_row(" · ready", 5), "ready");
        assert_eq!(one_row(" · ready · set", 12), "ready · set");
    }

    #[test]
    fn bars_are_relative_and_in_eighths() {
        assert_eq!(bar(87, 87, 14, true), "█".repeat(14));
        // 48 / 87 × 14 cells = 7.72 → 62 eighths: 7 full cells and ▊ (6/8).
        assert_eq!(bar(48, 87, 14, true), "███████▊");
        assert_eq!(bar(1, 87_000, 14, true), "");
        assert_eq!(bar(48, 87, 14, false), "########");
        assert_eq!(bar(5, 0, 14, true), "");
    }

    #[test]
    fn ascii_swaps_every_harn_glyph() {
        let text = "a · b — c → d ← e › f … ▸ ▾ ↑ ↓ ─";
        assert!(ascii(text).is_ascii(), "{}", ascii(text));
    }
}
