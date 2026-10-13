//! Drawing [`App`] (`.prd/04-tui-design.md`): two header lines, the list grouped by kind with a
//! size, a bar relative to the largest line shown and a share of the total, the detail panel of
//! the cursor line, the key bar in words; the cards (`d`, `o`, `w`, `?`, `i`) and the `u` confirm
//! over it. Under `--no-unicode` everything harn draws is ASCII.

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
/// The size column (`  48 KB`, `0.7 KB`, `?`, `—`).
const SIZE_W: usize = 7;
/// The share column (` 32%`).
const SHARE_W: usize = 4;
/// What the size column says when nobody knows the size yet; it reaches left of the column.
const UNMEASURED: &str = "unmeasured";
/// The label column of the `d` and `o` cards (`Writes   `).
const LABEL_W: usize = 9;

/// Harn's glyphs and their ASCII stand-ins under `--no-unicode`.
const ASCII_SWAPS: [(&str, &str); 11] = [
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
        frame.render_widget(Paragraph::new(self.header(width)), head);
        frame.render_widget(Paragraph::new(self.rule(width, None)), rule1);
        let footer = self.draw_list(frame, list);
        frame.render_widget(Paragraph::new(self.rule(width, footer)), rule2);
        self.draw_panel(frame, panel);
        frame.render_widget(Paragraph::new(self.rule(width, None)), rule3);
        frame.render_widget(Paragraph::new(self.key_bar(width)), keys);
        match &self.mode {
            Mode::Card(card) => self.draw_card(frame, area, card),
            Mode::Undo { entry } => {
                let lines: Vec<String> = self.undo_lines(entry).iter().map(|l| self.t(l)).collect();
                confirm_box(frame, area, &lines, self.g(), self.opts.unicode);
            }
            Mode::Normal | Mode::Filter => {}
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
        let lefts = [
            format!(
                " {} · {}{version} · {n} items load before your first message",
                t.dir, t.agent
            ),
            format!(
                " {} · {} · {n} items load before your first message",
                t.dir, t.agent
            ),
            format!(" {} · {n} items load before your first message", t.dir),
            format!(" {} · {n} items before your first message", t.dir),
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

    /// Size of a list line (a group's or a row's).
    fn line_bytes(&self, line: &ListLine) -> Option<u64> {
        let screen = &self.tab().screen;
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
            let overlap = row.is_none() && group.overlap;
            let right = if overlap {
                " ".repeat(cols_fit.right())
            } else {
                self.right_cells(&cols_fit, bytes, share, absent, max)
            };
            // `unmeasured` is wider than the size column: the names give it the room.
            let left_w = cols_fit
                .left
                .saturating_sub(cols(&right).saturating_sub(cols_fit.right()));
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
                if cols_fit.right() > 0 { " " } else { "" }
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
                (Some(b), _) => bar(b, max, c.bar, self.opts.unicode),
                (None, Some(word)) => format!(" {}", cut_right(word, c.bar - 1, self.g())),
                (None, None) => String::new(),
            };
            parts.push(pad_right(&cell, c.bar));
        }
        if c.share {
            let cell = match (bytes, share) {
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
        Some(match screen.line_row(line) {
            Some(row) => &row.detail,
            None => &screen.groups[line.group].detail,
        })
    }

    /// The panel: a message first (until the next key), then the cursor line's sentences.
    fn draw_panel(&self, frame: &mut Frame, area: Rect) {
        let height = area.height as usize;
        let width = (area.width as usize).saturating_sub(1).max(1);
        if height == 0 {
            return;
        }
        let mut lines: Vec<(String, Style)> = Vec::new();
        if let Some(message) = &self.message {
            let style = match message.tone {
                Tone::Error if self.opts.color => {
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)
                }
                Tone::Error => Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED),
                Tone::Info => Style::new().add_modifier(Modifier::BOLD),
            };
            for l in wrap_words(&self.t(&message.text), width) {
                lines.push((l, style));
            }
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
    /// `--readonly` the write keys stay listed, greyed.
    fn key_bar(&self, width: usize) -> Line<'static> {
        let ro = self.opts.readonly;
        let u = self.opts.unicode;
        // (key, words, enabled, priority: higher stays longer)
        let mut keys: Vec<(String, String, bool, u8)> = Vec::new();
        let mut push = |k: &str, w: &str, enabled: bool, p: u8| {
            keys.push((k.to_string(), w.to_string(), enabled, p));
        };
        match &self.mode {
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
                    push(if u { "↑↓" } else { "j/k" }, "scroll", true, 5);
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
                    push(if u { "↑↓" } else { "j/k" }, "scroll", true, 5);
                    push("Esc", "close", true, 9);
                }
            },
            Mode::Undo { .. } => {
                push("y", "undo it", true, 9);
                push("n Esc", "keep it", true, 9);
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
                    None => group.is_some_and(|g| g.can_turn_off),
                };
                if can_off {
                    push("d", "turn off", !ro, 7);
                }
                let can_on = row.is_some_and(|r| {
                    r.state == State::Off
                        || r.id
                            .as_ref()
                            .is_some_and(|id| self.tab().journaled.contains_key(id))
                });
                if can_on {
                    push("o", "turn back on", !ro, 7);
                }
                if self.last_live.is_some() {
                    push("u", "undo", !ro, 6);
                }
                if row.is_some_and(|r| r.id.is_some()) {
                    push("w", "where from", true, 5);
                }
                if self.cursor_item().is_some_and(editable) {
                    push("e", "edit", !ro, 2);
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

    fn draw_card(&self, frame: &mut Frame, area: Rect, card: &Card) {
        let width = area.width.min(CARD_MAX);
        if width < 8 || area.height < 3 {
            return;
        }
        let inner = usize::from(width - 2);
        let (title, body, answer) = match card {
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
                card.leave_list
                    .iter()
                    .flat_map(|l| indent_wrap("", &self.t(l), inner))
                    .collect(),
                "Esc back".to_string(),
            ),
            Card::On { name, files, .. } => {
                let files = if files.is_empty() {
                    "the files harn changed".to_string()
                } else {
                    files.join(", ")
                };
                let mut body = labelled(
                    "Writes",
                    &self.t(&format!("{files} back as they were before harn's change")),
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
        };
        let title = cut_right(&self.t(&title), inner.saturating_sub(2), self.g());
        let rows = usize::from(area.height - 2).saturating_sub(1);
        let max = body.len().saturating_sub(rows);
        self.card_max.set(max);
        let scroll = self.card_scroll.min(max);
        let mut text: Vec<Line> = body
            .into_iter()
            .skip(scroll)
            .take(rows)
            .map(Line::from)
            .collect();
        let answer = self.t(&answer);
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
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(text).block(bordered(&format!(" {title} "), self.opts.unicode)),
            popup,
        );
    }

    /// The `d` card's body: Writes, Leaves (with `→ list`), Saves, Undo, then the plan's notes.
    fn off_lines(&self, card: &OffCard, inner: usize) -> Vec<String> {
        let mut body = Vec::new();
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
        let gaps = [3, 2, 3];
        let need = 1 + (0..3).map(|c| k[c] + gaps[c] + d[c]).sum::<usize>() + 3 * 2;
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
                .collect();
        }
        let kw = k.iter().copied().max().unwrap_or(0);
        rows.iter()
            .flat_map(|r| r.iter())
            .flat_map(|(key, words)| indent_wrap(&pad_right(key, kw + 2), words, inner))
            .collect()
    }
}

/// `label` in the card's label column, `text` wrapped after it; `hint` right-aligned on the
/// first line when it fits.
fn labelled(label: &str, text: &str, inner: usize, hint: Option<&str>) -> Vec<String> {
    let room = inner.saturating_sub(2 + LABEL_W).max(1);
    let mut lines = wrap_words(text, room);
    let mut out = Vec::new();
    let first = lines.remove(0);
    let mut head = format!(" {}{first}", pad_right(label, LABEL_W));
    if let Some(hint) = hint {
        let used = cols(&head);
        if used + 3 + cols(hint) < inner {
            head = format!("{head}{}{hint}", " ".repeat(inner - 1 - used - cols(hint)));
        }
    }
    out.push(head);
    for line in lines {
        out.push(format!(" {}{line}", " ".repeat(LABEL_W)));
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

/// The smallest frame a confirm popup is drawn in (border + the answer line).
const CONFIRM_MIN: (u16, u16) = (8, 3);
/// The answer line when the full one does not fit.
const CONFIRM_SHORT: &str = "y/N";

/// A bordered box: line drawing, or `+-|` under `--no-unicode`.
fn bordered(title: &str, unicode: bool) -> Block<'static> {
    let block = Block::bordered().title(title.to_string());
    if unicode {
        block
    } else {
        block.border_set(ASCII_BORDER)
    }
}

/// The first of `candidates` that fits `width` columns.
fn first_fitting(candidates: Vec<String>, width: usize) -> Option<String> {
    candidates.into_iter().find(|c| cols(c) <= width)
}

/// The confirm popup's rect (inside `area`, centred, at most 76 wide) and its lines: the body
/// wrapped, a blank, the answer (`y = yes · n/Esc = no`, or `y/N` when narrower). When the body
/// does not fit, its tail becomes `(+N more)` (or `(+N)`, `+N`, whichever fits); the answer line
/// always stays. None when `area` is under [`CONFIRM_MIN`].
pub(crate) fn confirm_layout(
    area: Rect,
    body: &[String],
    g: &Glyphs,
) -> Option<(Rect, Vec<String>)> {
    if area.width < CONFIRM_MIN.0 || area.height < CONFIRM_MIN.1 {
        return None;
    }
    let width = area.width.saturating_sub(4).clamp(CONFIRM_MIN.0, 76);
    let inner_width = usize::from(width.saturating_sub(2)).max(1);
    let answer = first_fitting(
        vec![
            format!("y = yes {} n/Esc = no", g.dot),
            CONFIRM_SHORT.to_string(),
        ],
        inner_width,
    )
    .unwrap_or_else(|| CONFIRM_SHORT.to_string());
    // A line's leading spaces stay its indent on every row it wraps to.
    let body: Vec<String> = body
        .iter()
        .flat_map(|l| {
            let text = l.trim_start_matches(' ');
            let indent = (l.len() - text.len()).min(inner_width.saturating_sub(1));
            wrap_words(text, inner_width - indent)
                .into_iter()
                .map(move |row| format!("{}{row}", " ".repeat(indent)))
        })
        .collect();
    let rows = usize::from(area.height.saturating_sub(2));
    let mut lines = body.clone();
    if body.len() + 2 <= rows {
        lines.push(String::new());
    } else {
        let keep = rows.saturating_sub(2).min(body.len());
        lines.truncate(keep);
        let n = body.len() - keep;
        let notice = first_fitting(
            vec![format!("(+{n} more)"), format!("(+{n})"), format!("+{n}")],
            inner_width,
        );
        if let (true, Some(notice)) = (rows >= 2, notice) {
            lines.push(notice);
        }
    }
    lines.push(answer);
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let popup = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    Some((popup, lines))
}

/// Draws the confirm popup for `body` over `area` (nothing under [`CONFIRM_MIN`]).
pub(crate) fn confirm_box(
    frame: &mut Frame,
    area: Rect,
    body: &[String],
    g: &Glyphs,
    unicode: bool,
) {
    let Some((popup, text)) = confirm_layout(area, body, g) else {
        return;
    };
    frame.render_widget(Clear, popup);
    let text: Vec<Line> = text.into_iter().map(Line::from).collect();
    frame.render_widget(
        Paragraph::new(text).block(bordered(" confirm ", unicode)),
        popup,
    );
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

/// Where a word too wide for a row may break: right after one of these.
fn breaks_after(grapheme: &str) -> bool {
    matches!(grapheme, "/" | "›" | "." | "-" | "_" | ":" | "," | ">")
}

/// `text` in rows of at most `width` columns, broken at spaces (a run of spaces inside a row is
/// kept). A word wider than a row breaks after a `/` (or `›`, `.`, `-`, `_`, `:`), so a path
/// wraps on its separators; only a word with none of those in reach is cut between graphemes.
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
        let end = trimmed.find(' ').unwrap_or(trimmed.len());
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
    use crate::render::common::UNICODE;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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

    #[test]
    fn the_confirm_popup_stays_inside_any_frame_and_keeps_the_answer_line() {
        let body: Vec<String> = (0..30).map(|i| format!("line {i} of the plan")).collect();
        for width in 0..=100u16 {
            for height in 0..=40u16 {
                let area = Rect::new(3, 2, width, height);
                match confirm_layout(area, &body, &UNICODE) {
                    None => assert!(width < 8 || height < 3, "{width}x{height}"),
                    Some((popup, lines)) => {
                        assert!(popup.x >= area.x && popup.y >= area.y, "{width}x{height}");
                        assert!(popup.right() <= area.right(), "{width}x{height}");
                        assert!(popup.bottom() <= area.bottom(), "{width}x{height}");
                        let inner = usize::from(popup.width - 2);
                        assert!(lines.iter().all(|l| cols(l) <= inner), "{width}x{height}");
                        let answer = lines.last().unwrap();
                        assert!(answer == "y/N" || answer.starts_with("y = yes"));
                        assert!(lines.len() + 2 <= usize::from(popup.height).max(3));
                    }
                }
            }
        }
        let (_, lines) = confirm_layout(Rect::new(0, 0, 80, 8), &body, &UNICODE).unwrap();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[3], "line 3 of the plan");
        assert_eq!(lines[4], "(+26 more)");
        assert_eq!(lines[5], "y = yes · n/Esc = no");
    }

    fn drawn(width: u16, height: u16, body: &[String]) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| confirm_box(frame, frame.area(), body, &UNICODE, true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn the_answer_is_drawn_whole_at_the_smallest_popup() {
        let body = vec!["Undo the last change?".to_string()];
        let rows = drawn(8, 3, &body);
        assert_eq!(rows[1], "│y/N   │", "{rows:#?}");
        let long: Vec<String> = (0..30).map(|i| format!("l{i}")).collect();
        let rows = drawn(12, 5, &long);
        assert_eq!(rows[1], "  │l0    │  ", "{rows:#?}");
        assert_eq!(rows[2], "  │(+29) │  ", "{rows:#?}");
        assert_eq!(rows[3], "  │y/N   │  ", "{rows:#?}");
        let rows = drawn(40, 6, &body);
        assert!(
            rows.iter().any(|r| r.contains("y = yes · n/Esc = no")),
            "{rows:#?}"
        );
    }
}
