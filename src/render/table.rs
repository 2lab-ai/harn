//! `harn [DIR]`: one table per agent (01-spec §First screen).

use super::common::{
    cost_column, default_action, denominator, facts, freed_cell, freed_computed, glyphs, kb,
    label_with_count, partition, percent, request_bytes, share_tenths, sorted, truncate, Glyphs,
};
use super::RenderOpts;
use crate::model::{Item, Origin, Tree};

const DEFAULT_WIDTH: usize = 120;
/// Bar cells at 100 % share (eighths: 8 per cell).
const BAR_CELLS: usize = 10;
/// A container with at most this many children is expanded; a bigger one collapses to one line.
pub(crate) const EXPAND_MAX: usize = 12;
const ITEM_WIDTH: (usize, usize) = (4, 40);
const ORIGIN_WIDTH: (usize, usize) = (6, 24);
const FREED_HEADER: &str = "freed(pred)";

const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];

enum Line {
    Row(Cells),
    /// `… N more (harn why <id>)`: never truncated, the id must stay usable.
    More {
        depth: usize,
        text: String,
    },
}

struct Cells {
    freed: String,
    cost: String,
    share: String,
    bar: String,
    item: String,
    origin: String,
    mode: String,
    action: String,
}

struct Ctx<'a> {
    g: &'a Glyphs,
    unicode: bool,
    denominator: u64,
    observed_column: bool,
}

/// The lines [`table`] prints above its rows: title, drift, totals (with the
/// `observed = attributed + parameters + framing` split when a capture is bound), tool search
/// with its cause, signals, warnings. `harn verify` prints them after its report.
pub fn header(tree: &Tree, opts: &RenderOpts) -> String {
    header_lines(tree, opts, None).join("\n")
}

fn paint(opts: &RenderOpts, text: String, code: &str) -> String {
    if opts.color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text
    }
}

/// [`header`]; `sort` (the rows' order) is appended to the totals line.
fn header_lines(tree: &Tree, opts: &RenderOpts, sort: Option<&str>) -> Vec<String> {
    let g = glyphs(opts.unicode);
    let f = facts(tree, g, kb);
    let mut out = vec![format!("harn {}   {}", tree.target_dir.display(), f.title)];
    if let Some(drift) = f.drift {
        out.push(paint(opts, drift, "33"));
    }
    out.push(match sort {
        Some(sort) => format!("{}   {sort}", f.totals),
        None => f.totals,
    });
    for (i, line) in f.tool_search.into_iter().enumerate() {
        out.push(if i == 0 { line } else { format!("    {line}") });
    }
    out.extend(f.signals.into_iter().map(|s| paint(opts, s, "33")));
    out.extend(f.warnings.into_iter().map(|w| paint(opts, w, "33")));
    out
}

/// Header lines (title, drift, totals, tool search, signals, warnings), then rows sorted by
/// freed.known desc (freed not computed → first-request bytes), children under their container,
/// Runtime/None rows in a trailing block, then the signal footnotes.
pub fn table(tree: &Tree, opts: &RenderOpts) -> String {
    let g = glyphs(opts.unicode);
    let width = opts.width.unwrap_or(DEFAULT_WIDTH);
    let paint = |text: String, code: &str| paint(opts, text, code);
    let sort = if freed_computed(tree) {
        format!("sort: freed {}", g.down)
    } else {
        format!("sort: predicted {} (freed not computed)", g.down)
    };
    let mut out = header_lines(tree, opts, Some(&sort));

    let ctx = Ctx {
        g,
        unicode: opts.unicode,
        denominator: denominator(tree),
        observed_column: tree.observed_body.is_some(),
    };
    let mut notes: Vec<String> = Vec::new();
    let (main, trailing) = partition(tree);
    let mut main_lines = Vec::new();
    for item in main {
        push_item(&mut main_lines, &mut notes, item, 0, &ctx);
    }
    let mut trailing_lines = Vec::new();
    for item in trailing {
        push_item(&mut trailing_lines, &mut notes, item, 0, &ctx);
    }

    let cost_header = if ctx.observed_column {
        "observed"
    } else {
        "predicted"
    };
    let widths = Widths::of(main_lines.iter().chain(&trailing_lines), cost_header);
    let header = Cells {
        freed: FREED_HEADER.to_string(),
        cost: cost_header.to_string(),
        share: "share".to_string(),
        bar: String::new(),
        item: "item".to_string(),
        origin: "origin".to_string(),
        mode: "mode".to_string(),
        action: "default action".to_string(),
    };
    out.push(String::new());
    out.push(paint(widths.row(&header, width, g), "1"));
    out.extend(main_lines.iter().map(|line| widths.line(line, width, g)));
    if !trailing_lines.is_empty() {
        out.push(paint("  runtime / no first-request cost:".to_string(), "2"));
        out.extend(
            trailing_lines
                .iter()
                .map(|line| widths.line(line, width, g)),
        );
    }
    for (i, note) in notes.iter().enumerate() {
        out.push(format!("  [{}] {note}", i + 1));
    }
    out.join("\n")
}

fn push_item(lines: &mut Vec<Line>, notes: &mut Vec<String>, item: &Item, depth: usize, ctx: &Ctx) {
    lines.push(Line::Row(cells(item, depth, notes, ctx)));
    if item.children.is_empty() {
        return;
    }
    if item.children.len() <= EXPAND_MAX {
        for child in sorted(&item.children) {
            push_item(lines, notes, child, depth + 1, ctx);
        }
    } else {
        lines.push(Line::More {
            depth: depth + 1,
            text: format!(
                "{} {} more (harn why {})",
                ctx.g.ellipsis,
                item.children.len(),
                item.id
            ),
        });
    }
}

fn cells(item: &Item, depth: usize, notes: &mut Vec<String>, ctx: &Ctx) -> Cells {
    let g = ctx.g;
    let mut label = format!(
        "{}{}",
        "  ".repeat(depth),
        label_with_count(&item.label, item.children.len(), g)
    );
    if let Some(signal) = &item.signal {
        notes.push(format!("{}: {signal}", item.label));
        label.push_str(&format!(" [{}]", notes.len()));
    }
    let tenths = share_tenths(request_bytes(&item.cost), ctx.denominator);
    Cells {
        freed: freed_cell(item, g, kb),
        cost: cost_column(&item.cost, ctx.observed_column, g),
        share: tenths.map(percent).unwrap_or_default(),
        bar: tenths.map(|t| bar(t, ctx.unicode)).unwrap_or_default(),
        item: label,
        origin: origin_cell(&item.origin),
        mode: item.mode.to_string(),
        action: default_action(item, g),
    }
}

/// `plugin slack` (the marketplace is in `why`, `brief` and the html report), else the layer.
fn origin_cell(origin: &Origin) -> String {
    match &origin.plugin {
        Some(plugin) => {
            let name = plugin.split('@').next().unwrap_or(plugin);
            format!("{} {name}", origin.layer)
        }
        None => origin.layer.to_string(),
    }
}

/// Eighth-block bar of `share` (tenths of a percent), [`BAR_CELLS`] wide at 100 %; `#` per
/// rounded cell without unicode.
pub(crate) fn bar(tenths: u64, unicode: bool) -> String {
    let full_scale = (BAR_CELLS * 8) as u64;
    let eighths = ((tenths.min(1000) * full_scale + 500) / 1000) as usize;
    if unicode {
        let mut out = "█".repeat(eighths / 8);
        if !eighths.is_multiple_of(8) {
            out.push(EIGHTHS[eighths % 8]);
        }
        out
    } else {
        "#".repeat((eighths + 4) / 8)
    }
}

struct Widths {
    freed: usize,
    cost: usize,
    item: usize,
    origin: usize,
    mode: usize,
}

impl Widths {
    fn of<'a>(lines: impl Iterator<Item = &'a Line>, cost_header: &str) -> Widths {
        let mut w = Widths {
            freed: FREED_HEADER.len(),
            cost: cost_header.len(),
            item: ITEM_WIDTH.0,
            origin: ORIGIN_WIDTH.0,
            mode: "mode".len(),
        };
        for line in lines {
            if let Line::Row(c) = line {
                w.freed = w.freed.max(c.freed.chars().count());
                w.cost = w.cost.max(c.cost.chars().count());
                w.item = w.item.max(c.item.chars().count());
                w.origin = w.origin.max(c.origin.chars().count());
                w.mode = w.mode.max(c.mode.chars().count());
            }
        }
        w.item = w.item.min(ITEM_WIDTH.1);
        w.origin = w.origin.min(ORIGIN_WIDTH.1);
        w
    }

    fn line(&self, line: &Line, width: usize, g: &Glyphs) -> String {
        match line {
            Line::Row(cells) => self.row(cells, width, g),
            Line::More { depth, text } => {
                let lead = 2 + self.freed + 1 + self.cost + 1 + 6 + 1 + BAR_CELLS + 1;
                format!("{}{}{text}", " ".repeat(lead), "  ".repeat(*depth))
            }
        }
    }

    fn row(&self, c: &Cells, width: usize, g: &Glyphs) -> String {
        let line = format!(
            "  {:>fw$} {:>cw$} {:>6} {:<bw$} {:<iw$} {:<ow$} {:<mw$} {}",
            c.freed,
            c.cost,
            c.share,
            c.bar,
            truncate(&c.item, self.item, g),
            truncate(&c.origin, self.origin, g),
            c.mode,
            c.action,
            fw = self.freed,
            cw = self.cost,
            bw = BAR_CELLS,
            iw = self.item,
            ow = self.origin,
            mw = self.mode,
        );
        truncate(line.trim_end(), width, g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighth_block_bars_descend_from_full() {
        // 10 cells = 100 %; 1 eighth = 1.25 %; U+2588 full, U+258F one eighth.
        assert_eq!(bar(1000, true), "█".repeat(10));
        assert_eq!(bar(500, true), "█████");
        assert_eq!(bar(100, true), "█");
        assert_eq!(bar(125, true), "█▎"); // 12.5 % → 10 eighths
        assert_eq!(bar(13, true), "▏"); // 1.3 % → 1 eighth
        assert_eq!(bar(233, true), "██▍"); // 23.3 % → 19 eighths
        assert_eq!(bar(0, true), "");
    }

    #[test]
    fn ascii_bars_round_to_cells() {
        assert_eq!(bar(500, false), "#####");
        assert_eq!(bar(233, false), "##");
        assert_eq!(bar(0, false), "");
    }
}
