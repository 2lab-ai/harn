//! Drawing [`App`]: header facts, agent tabs, the tree with its columns, a pane (why / plan /
//! help) or a confirm popup over it, the message line and the context key bar (lazygit style).

use super::app::{can_stage, short_id, App, Confirm, Mode, Tone};
use super::rows::{self, origin_text, RowKind};
use crate::model::{Item, Layer};
use crate::render::common::{
    cost_column, denominator, facts, freed_cell, freed_value, glyphs, kb, knob_compact, percent,
    request_bytes, share_tenths, truncate, Glyphs,
};
use crate::render::{self, share_bar};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

const FREED_HEADER: &str = "freed";
const SHARE_WIDTH: usize = 6;
/// The table's bar: 10 cells at 100 %.
const BAR_WIDTH: usize = 10;
/// Below this many item columns the bar goes, then the mode, then origin shrinks, then share.
const ITEM_WANT: usize = 36;
const ORIGIN_MAX: usize = 20;
const ORIGIN_MIN: usize = 8;
const GUTTER: usize = 2;

struct Cells {
    freed: String,
    cost: String,
    share: String,
    bar: String,
    indent: usize,
    label: String,
    /// `▸ 29` / `▾ 29` after a container's label; kept when the label is cut.
    fold: String,
    origin: String,
    mode: String,
    staged: bool,
    dim: bool,
}

struct Columns {
    freed: usize,
    cost: usize,
    share: bool,
    bar: bool,
    item: usize,
    origin: usize,
    mode: Option<usize>,
}

impl Columns {
    fn fixed(&self) -> usize {
        GUTTER
            + self.freed
            + 1
            + self.cost
            + 1
            + if self.share { SHARE_WIDTH + 1 } else { 0 }
            + if self.bar { BAR_WIDTH + 1 } else { 0 }
            + 1
            + self.origin
            + self.mode.map_or(0, |m| m + 1)
    }

    /// Fits `width`: the item column takes what the others leave, and when that is under what
    /// the labels want, the bar goes first, then the mode, then origin narrows, then share.
    fn fit(cells: &[&Cells], cost_header: &str, width: usize) -> Columns {
        let widest = |f: fn(&Cells) -> usize| cells.iter().map(|c| f(c)).max().unwrap_or(0);
        let mut c = Columns {
            freed: widest(|c| c.freed.chars().count()).max(FREED_HEADER.len()),
            cost: widest(|c| c.cost.chars().count()).max(cost_header.len()),
            share: true,
            bar: true,
            item: 0,
            origin: widest(|c| c.origin.chars().count()).clamp(6, ORIGIN_MAX),
            mode: Some(widest(|c| c.mode.chars().count()).max(4)),
        };
        let want = widest(|c| c.indent + c.label.chars().count() + c.fold.chars().count() + 1)
            .clamp(4, ITEM_WANT);
        let room = |c: &Columns| width.saturating_sub(c.fixed());
        if room(&c) < want {
            c.bar = false;
        }
        if room(&c) < want {
            c.mode = None;
        }
        if room(&c) < want {
            let short = want - room(&c);
            c.origin = c.origin.saturating_sub(short).max(ORIGIN_MIN.min(c.origin));
        }
        if room(&c) < want {
            c.share = false;
        }
        c.item = room(&c).max(1);
        c
    }
}

fn pad_left(text: &str, width: usize) -> String {
    format!("{text:>width$}")
}

fn pad_right(text: &str, width: usize) -> String {
    format!("{text:<width$}")
}

impl App {
    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let g = glyphs(self.opts.unicode);
        let header = self.header_lines(g, area.height as usize, area.width as usize);
        let [head, tabs, body, message, keys] = Layout::vertical([
            Constraint::Length(header.len() as u16),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(header), head);
        frame.render_widget(Paragraph::new(self.tabs_line()), tabs);
        match &self.mode {
            Mode::Why { text } => self.draw_pane(frame, body, " why ", text),
            Mode::Plan => {
                let text = match &self.staged {
                    Some(staged) => {
                        format!("turn off {}\n{}", staged.label, render::plan(&staged.plan))
                    }
                    None => "nothing staged".to_string(),
                };
                self.draw_pane(frame, body, " staged plan ", &text);
            }
            Mode::Help => self.draw_pane(frame, body, " keys ", &help_text()),
            Mode::Confirm { back, .. } => match back.as_ref() {
                Mode::Plan => {
                    let text = self
                        .staged
                        .as_ref()
                        .map(|s| render::plan(&s.plan))
                        .unwrap_or_default();
                    self.draw_pane(frame, body, " staged plan ", &text);
                }
                _ => self.draw_list(frame, body, g),
            },
            Mode::Normal | Mode::Filter => self.draw_list(frame, body, g),
        }
        frame.render_widget(Paragraph::new(self.message_line(g)), message);
        frame.render_widget(Paragraph::new(self.key_bar(area.width as usize)), keys);
        if let Mode::Confirm { what, .. } = &self.mode {
            self.draw_confirm(frame, area, what, g);
        }
    }

    fn warn(&self) -> Style {
        if self.opts.color {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new()
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

    /// The table's header facts (title, drift, totals with the sort, tool search, signals,
    /// warnings), at most a third of the screen; the cut ones are counted on the last line.
    fn header_lines(&self, g: &Glyphs, height: usize, width: usize) -> Vec<Line<'static>> {
        let tree = &self.tab().tree;
        let f = facts(tree, g, kb);
        let mut lines: Vec<(String, Style)> = vec![(
            format!("harn {}   {}", tree.target_dir.display(), f.title),
            Style::new().add_modifier(Modifier::BOLD),
        )];
        if let Some(drift) = f.drift {
            lines.push((drift, self.warn()));
        }
        lines.push((
            format!(
                "{}   sort: {}",
                f.totals,
                self.sort.label(self.opts.unicode)
            ),
            Style::new(),
        ));
        for (i, line) in f.tool_search.into_iter().enumerate() {
            let line = if i == 0 { line } else { format!("    {line}") };
            lines.push((line, Style::new()));
        }
        for line in f.signals.into_iter().chain(f.warnings) {
            lines.push((line, self.warn()));
        }
        let cap = (height / 3).max(3);
        if lines.len() > cap {
            let cut = lines.len() - (cap - 1);
            lines.truncate(cap - 1);
            lines.push((
                format!(
                    "{} {cut} more header line(s): `harn --table` prints them all",
                    g.ellipsis
                ),
                self.grey(),
            ));
        }
        lines
            .into_iter()
            .map(|(text, style)| Line::styled(truncate(&text, width, g), style))
            .collect()
    }

    fn tabs_line(&self) -> Line<'static> {
        let mut spans = Vec::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            let style = if i == self.active {
                Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::new()
            };
            spans.push(Span::styled(format!(" {} ", tab.tree.agent), style));
            spans.push(Span::raw(" "));
        }
        Line::from(spans)
    }

    fn cells(&self, g: &Glyphs) -> Vec<Option<Cells>> {
        let tab = self.tab();
        let tree = &tab.tree;
        let observed = tree.observed_body.is_some();
        let denom = denominator(tree);
        let (open, closed) = if self.opts.unicode {
            ("▾", "▸")
        } else {
            ("v", ">")
        };
        tab.rows
            .iter()
            .map(|row| match &row.kind {
                RowKind::Item(path) => {
                    let item = rows::item_at(&tree.items, path)?;
                    let tenths = share_tenths(request_bytes(&item.cost), denom);
                    let mut fold = String::new();
                    if !item.children.is_empty() {
                        let mark = if tab.expanded.contains(&item.id) {
                            open
                        } else {
                            closed
                        };
                        fold = format!(" {mark} {}", item.children.len());
                    }
                    if item.signal.is_some() {
                        fold.push_str(" !");
                    }
                    Some(Cells {
                        freed: freed_cell(item, g, kb),
                        cost: cost_column(&item.cost, observed, g),
                        share: tenths.map(percent).unwrap_or_default(),
                        bar: tenths
                            .map(|t| share_bar(t, self.opts.unicode))
                            .unwrap_or_default(),
                        indent: 2 * row.depth,
                        label: item.label.clone(),
                        fold,
                        origin: origin_text(item),
                        mode: item.mode.to_string(),
                        staged: self
                            .staged
                            .as_ref()
                            .is_some_and(|s| s.tab == self.active && s.item_id == item.id),
                        dim: false,
                    })
                }
                RowKind::Off(i) => {
                    let off = &tab.off[*i];
                    let layer = off.item_id.split('/').nth(2).unwrap_or_default();
                    Some(Cells {
                        freed: String::new(),
                        cost: String::new(),
                        share: String::new(),
                        bar: String::new(),
                        indent: 0,
                        label: short_id(&off.item_id),
                        fold: String::new(),
                        origin: layer.to_string(),
                        mode: "off".to_string(),
                        staged: false,
                        dim: true,
                    })
                }
                RowKind::Separator(_) => None,
            })
            .collect()
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect, g: &Glyphs) {
        if area.height == 0 {
            return;
        }
        let width = area.width as usize;
        let tab = self.tab();
        let cost_header = if tab.tree.observed_body.is_some() {
            "observed"
        } else {
            "predicted"
        };
        let cells = self.cells(g);
        let present: Vec<&Cells> = cells.iter().flatten().collect();
        let cols = Columns::fit(&present, cost_header, width);

        let mut lines = vec![Line::styled(
            pad_right(
                &self.row_text(
                    &cols,
                    g,
                    FREED_HEADER,
                    cost_header,
                    "share",
                    "",
                    "item",
                    "origin",
                    "mode",
                ),
                width,
            ),
            Style::new().add_modifier(Modifier::BOLD),
        )];
        let height = area.height as usize - 1;
        self.list_height.set(height.max(1));
        let mut offset = tab.offset.get();
        if tab.cursor < offset {
            offset = tab.cursor;
        } else if height > 0 && tab.cursor >= offset + height {
            offset = tab.cursor + 1 - height;
        }
        offset = offset.min(tab.rows.len().saturating_sub(height));
        tab.offset.set(offset);

        for (i, (row, cell)) in tab
            .rows
            .iter()
            .zip(&cells)
            .enumerate()
            .skip(offset)
            .take(height)
        {
            let line = match (&row.kind, cell) {
                (RowKind::Separator(title), _) => Line::styled(
                    pad_right(&truncate(&format!("  {title}"), width, g), width),
                    self.grey(),
                ),
                (_, Some(c)) => {
                    let item = self.item_cell(c, cols.item, g);
                    let text = self.row_text(
                        &cols, g, &c.freed, &c.cost, &c.share, &c.bar, &item, &c.origin, &c.mode,
                    );
                    let gutter = if c.staged {
                        if self.opts.unicode {
                            "✗ "
                        } else {
                            "x "
                        }
                    } else {
                        "  "
                    };
                    let text = pad_right(&format!("{gutter}{}", &text[GUTTER..]), width);
                    let mut style = if c.dim { self.grey() } else { Style::new() };
                    if c.staged && self.opts.color {
                        style = style.fg(Color::Red);
                    }
                    if i == tab.cursor {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    Line::styled(text, style)
                }
                _ => Line::raw(""),
            };
            lines.push(line);
        }
        if tab.rows.is_empty() {
            let text = if self.filter.is_empty() {
                "  (no items)".to_string()
            } else {
                format!("  no item matches \"{}\" (Esc clears)", self.filter)
            };
            lines.push(Line::styled(text, self.grey()));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    /// Indent + label (cut to fit) + the fold marker, padded to `width`.
    fn item_cell(&self, c: &Cells, width: usize, g: &Glyphs) -> String {
        let fold = c.fold.chars().count();
        if width > c.indent + fold + 1 {
            let label = truncate(&c.label, width - c.indent - fold, g);
            format!("{}{label}{}", " ".repeat(c.indent), c.fold)
        } else {
            truncate(
                &format!("{}{}{}", " ".repeat(c.indent), c.label, c.fold),
                width,
                g,
            )
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn row_text(
        &self,
        cols: &Columns,
        g: &Glyphs,
        freed: &str,
        cost: &str,
        share: &str,
        bar: &str,
        item: &str,
        origin: &str,
        mode: &str,
    ) -> String {
        let mut out = " ".repeat(GUTTER);
        out.push_str(&pad_left(&truncate(freed, cols.freed, g), cols.freed));
        out.push(' ');
        out.push_str(&pad_left(&truncate(cost, cols.cost, g), cols.cost));
        out.push(' ');
        if cols.share {
            out.push_str(&pad_left(share, SHARE_WIDTH));
            out.push(' ');
        }
        if cols.bar {
            out.push_str(&pad_right(bar, BAR_WIDTH));
            out.push(' ');
        }
        out.push_str(&pad_right(&truncate(item, cols.item, g), cols.item));
        out.push(' ');
        out.push_str(&pad_right(&truncate(origin, cols.origin, g), cols.origin));
        if let Some(width) = cols.mode {
            out.push(' ');
            out.push_str(&pad_right(&truncate(mode, width, g), width));
        }
        out.trim_end().to_string()
    }

    fn draw_pane(&self, frame: &mut Frame, area: Rect, title: &str, text: &str) {
        let block = Block::bordered().title(title.to_string());
        let inner = block.inner(area);
        let width = (inner.width as usize).max(1);
        let lines: Vec<String> = text.lines().flat_map(|line| wrap(line, width)).collect();
        let height = inner.height as usize;
        self.list_height.set(height.max(1));
        let max = lines.len().saturating_sub(height);
        self.pane_max.set(max);
        let scroll = self.pane_scroll.min(max);
        let shown: Vec<Line> = lines
            .into_iter()
            .skip(scroll)
            .take(height)
            .map(Line::from)
            .collect();
        frame.render_widget(Paragraph::new(shown).block(block), area);
    }

    fn draw_confirm(&self, frame: &mut Frame, area: Rect, what: &Confirm, g: &Glyphs) {
        let mut lines: Vec<String> = Vec::new();
        match what {
            Confirm::Apply => {
                if let Some(staged) = &self.staged {
                    lines.push(format!("Apply: turn off {}?", staged.label));
                    for op in &staged.plan.ops {
                        lines.push(format!("  {}", knob_compact(&op.knob, g)));
                        if let Some(file) = &op.file {
                            lines.push(format!("  writes {}", file.display()));
                        }
                    }
                    lines.push(format!(
                        "  freed {}",
                        freed_value(&staged.plan.freed, g, kb)
                    ));
                    if !staged.plan.warnings.is_empty() {
                        lines.push(format!(
                            "  {} warning(s) in the plan pane",
                            staged.plan.warnings.len()
                        ));
                    }
                    lines.push("  journaled: u undoes it".to_string());
                }
            }
            Confirm::On { label, .. } => {
                lines.push(format!("Turn {label} back on?"));
                lines.push("  reverses the journal entry that turned it off".to_string());
            }
            Confirm::Undo { entry } => {
                lines.push(format!("Undo journal {}?", entry.id));
                for op in &entry.ops {
                    lines.push(format!("  {}", short_id(&op.item_id)));
                }
                if entry.context.cwd != self.target.dir {
                    lines.push(format!(
                        "  applied in {} (not this directory)",
                        entry.context.cwd.display()
                    ));
                }
            }
        }
        lines.push(String::new());
        lines.push("y = yes   n / Esc = no".to_string());
        let width = (area.width as usize).saturating_sub(4).clamp(1, 76);
        let lines: Vec<String> = lines.iter().flat_map(|l| wrap(l, width - 2)).collect();
        let height = (lines.len() + 2).min(area.height as usize);
        let popup = Rect {
            x: area.x + (area.width.saturating_sub(width as u16)) / 2,
            y: area.y + (area.height.saturating_sub(height as u16)) / 2,
            width: width as u16,
            height: height as u16,
        };
        frame.render_widget(Clear, popup);
        let text: Vec<Line> = lines.into_iter().map(Line::from).collect();
        frame.render_widget(
            Paragraph::new(text).block(Block::bordered().title(" confirm ")),
            popup,
        );
    }

    fn message_line(&self, g: &Glyphs) -> Line<'static> {
        if matches!(self.mode, Mode::Filter) {
            return Line::from(vec![
                Span::styled("/", Style::new().add_modifier(Modifier::BOLD)),
                Span::raw(self.filter.clone()),
                Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            ]);
        }
        if let Some(message) = &self.message {
            let style = match message.tone {
                Tone::Error if self.opts.color => Style::new().fg(Color::Red),
                Tone::Error => Style::new().add_modifier(Modifier::BOLD),
                Tone::Info => Style::new(),
            };
            return Line::styled(message.text.clone(), style);
        }
        let tab = self.tab();
        let shown = tab
            .rows
            .iter()
            .filter(|r| !matches!(r.kind, RowKind::Separator(_)))
            .count();
        let total = crate::model::walk(&tab.tree).count();
        let mut parts = vec![format!("{shown} rows shown of {total} items")];
        if !self.filter.is_empty() {
            parts.push(format!("filter \"{}\"", self.filter));
        }
        if let Some(staged) = &self.staged {
            parts.push(format!("staged: {} off (a applies)", staged.label));
        }
        if self.opts.readonly {
            parts.push("readonly".to_string());
        }
        Line::styled(parts.join(&format!(" {} ", g.dot)), self.grey())
    }

    /// The keys valid here, most used first; cut from the end to fit, `?` and `q` always kept.
    /// Under `--readonly` the write keys stay listed, greyed.
    fn key_bar(&self, width: usize) -> Line<'static> {
        let ro = self.opts.readonly;
        let mut keys: Vec<(&str, &str, bool)> = Vec::new();
        let mut tail: Vec<(&str, &str, bool)> = vec![("?", "help", true)];
        match &self.mode {
            Mode::Normal => {
                let item = self.cursor_item();
                let off = self.cursor_off().is_some();
                keys.push(("j/k", "move", true));
                if item.is_some_and(|i| !i.children.is_empty()) {
                    keys.push(("l/h", "fold", true));
                }
                keys.push(("/", "filter", true));
                if ro || item.is_some_and(can_stage) {
                    keys.push(("d", "off", !ro));
                }
                if ro || self.staged.is_some() {
                    keys.push(("a", "apply", !ro));
                }
                let journaled = item.is_some_and(|i| self.tab().journaled.contains_key(&i.id));
                if ro || off || journaled {
                    keys.push(("o", "on", !ro));
                }
                if ro || self.last_live.is_some() {
                    keys.push(("u", "undo", !ro));
                }
                keys.push(("w", "why", true));
                if ro || item.is_some_and(editable) {
                    keys.push(("e", "edit", !ro));
                }
                keys.push(("s", "sort", true));
                if self.tabs.len() > 1 {
                    keys.push(("tab", "agent", true));
                }
                keys.push(("r", "refresh", true));
                keys.push(("t", "trim", true));
                tail.push(("q", "quit", true));
            }
            Mode::Filter => {
                keys.push(("type", "filter", true));
                keys.push(("enter", "keep", true));
                keys.push(("esc", "clear", true));
                keys.push((
                    if self.opts.unicode {
                        "↑/↓"
                    } else {
                        "up/down"
                    },
                    "move",
                    true,
                ));
                tail.clear();
            }
            Mode::Plan => {
                keys.push(("a", "apply", !ro));
                keys.push(("d", "unstage", true));
                keys.push(("j/k", "scroll", true));
                tail.push(("esc", "close", true));
            }
            Mode::Why { .. } | Mode::Help => {
                keys.push(("j/k", "scroll", true));
                keys.push(("g/G", "top/bottom", true));
                tail.push(("esc", "close", true));
            }
            Mode::Confirm { .. } => {
                keys.push(("y", "yes", true));
                keys.push(("n/esc", "no", true));
                tail.clear();
            }
        }
        let len = |k: &[(&str, &str, bool)]| -> usize {
            k.iter()
                .map(|(key, desc, _)| key.chars().count() + 1 + desc.chars().count() + 3)
                .sum()
        };
        while !keys.is_empty() && len(&keys) + len(&tail) > width {
            keys.pop();
        }
        let mut spans = Vec::new();
        for (key, desc, enabled) in keys.into_iter().chain(tail) {
            let (key_style, desc_style) = if enabled {
                (Style::new().add_modifier(Modifier::BOLD), Style::new())
            } else {
                (self.grey(), self.grey())
            };
            spans.push(Span::styled(key.to_string(), key_style));
            spans.push(Span::styled(format!(" {desc}   "), desc_style));
        }
        Line::from(spans)
    }
}

/// A file the user owns: `e` opens it.
fn editable(item: &Item) -> bool {
    item.origin.path.is_some() && !matches!(item.origin.layer, Layer::Builtin | Layer::Managed)
}

/// `line` cut into pieces of at most `width` chars.
fn wrap(line: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(width.max(1))
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn help_text() -> String {
    [
        "move        j / k / ↓ / ↑   PgDn / PgUp   g / G top / bottom",
        "fold        l / → expand · h / ← collapse (or go to the parent) · Enter toggles",
        "filter      /  type to filter (the hierarchy stays, matches unfold) · Enter keeps · Esc clears",
        "sort        s  freed ▼ → freed ▲ → cost ▼ → cost ▲ → name ▲ → name ▼",
        "why         w  the admission chain, every knob by scope and its freed bytes",
        "stage off   d / space  plan the cursor row off (default scope local) · d again unstages",
        "apply       a  write the staged plan through the journal (asks y/N)",
        "on          o  reverse the journal op that turned the cursor row off (asks y/N)",
        "undo        u  reverse the newest journal entry (asks y/N)",
        "edit        e  open the row's file in $EDITOR, journaled (u undoes)",
        "refresh     r  re-resolve every agent",
        "AI trim     t  how to run `harn trim`",
        "agents      Tab / Shift-Tab next / previous agent",
        "quit        q · Esc (Esc first closes a pane or clears the filter) · Ctrl-C",
        "",
        "columns     freed = predicted bytes the default knob removes (Δ? = an unknown part)",
        "            predicted / observed = first-request bytes · share of the request · bar",
        "--readonly  d a o u e are greyed and refused",
    ]
    .join("\n")
}
