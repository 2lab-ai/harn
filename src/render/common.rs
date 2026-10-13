//! Cell formatting shared by every renderer: byte units, cost/freed tags, compact knobs, the
//! row sort, and the header facts (drift, totals, tool search, signals, warnings).

use crate::model::{Agent, Cost, Freed, Item, Knob, Mode, Origin, Tree};
use std::path::Path;

/// Renderer glyphs; `ASCII` under `--no-unicode`.
pub(crate) struct Glyphs {
    pub dash: &'static str,
    pub dot: &'static str,
    pub arrow: &'static str,
    pub delta: &'static str,
    pub ellipsis: &'static str,
    pub down: &'static str,
    pub left: &'static str,
}

pub(crate) const UNICODE: Glyphs = Glyphs {
    dash: "—",
    dot: "·",
    arrow: "→",
    delta: "Δ?",
    ellipsis: "…",
    down: "▼",
    left: "←",
};

pub(crate) const ASCII: Glyphs = Glyphs {
    dash: "-",
    dot: "|",
    arrow: "->",
    delta: "D?",
    ellipsis: "...",
    down: "v",
    left: "<-",
};

pub(crate) fn glyphs(unicode: bool) -> &'static Glyphs {
    if unicode {
        &UNICODE
    } else {
        &ASCII
    }
}

/// `77.5 KB` (1 KB = 1000 B, one decimal, as in 01-spec §First screen); `1.2 MB` from 999.95 KB.
pub(crate) fn kb(bytes: u64) -> String {
    let tenths = bytes.saturating_add(50) / 100;
    if tenths < 10_000 {
        format!("{}.{} KB", tenths / 10, tenths % 10)
    } else {
        let tenths = bytes.saturating_add(50_000) / 100_000;
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

/// `77,536 B`.
pub(crate) fn exact(bytes: u64) -> String {
    format!("{} B", grouped(bytes))
}

/// `77536` (the brief: exact, no unit, no separators).
pub(crate) fn plain(bytes: u64) -> String {
    bytes.to_string()
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The evidence level of a cost, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CostCell {
    /// `—`: runtime/none, no first-request cost.
    NotApplicable,
    Observed(u64),
    /// `~` when `estimate`.
    Predicted {
        bytes: u64,
        estimate: bool,
    },
    /// Bytes on disk only; not a first-request number.
    Source(u64),
    /// `?`.
    Unknown,
}

pub(crate) fn cost_cell(cost: &Cost) -> CostCell {
    if cost.not_applicable {
        return CostCell::NotApplicable;
    }
    if let Some(bytes) = cost.observed {
        return CostCell::Observed(bytes);
    }
    if let Some(bytes) = cost.predicted {
        return CostCell::Predicted {
            bytes,
            estimate: cost.predicted_is_estimate,
        };
    }
    match cost.source {
        Some(bytes) => CostCell::Source(bytes),
        None => CostCell::Unknown,
    }
}

/// The row's first-request bytes (observed, else predicted); the share and bar numerator.
pub(crate) fn request_bytes(cost: &Cost) -> Option<u64> {
    match cost_cell(cost) {
        CostCell::Observed(bytes) | CostCell::Predicted { bytes, .. } => Some(bytes),
        _ => None,
    }
}

/// Cost with its tag spelled out (`predicted 6.8 KB`, `observed 6.8 KB`, `source 1.2 KB`, `?`, `—`).
pub(crate) fn cost_tagged(cost: &Cost, g: &Glyphs, unit: fn(u64) -> String) -> String {
    match cost_cell(cost) {
        CostCell::NotApplicable => g.dash.to_string(),
        CostCell::Unknown => "?".to_string(),
        CostCell::Observed(b) => format!("observed {}", unit(b)),
        CostCell::Predicted { bytes, estimate } => {
            let tilde = if estimate { "~" } else { "" };
            format!("predicted {tilde}{}", unit(bytes))
        }
        CostCell::Source(b) => format!("source {}", unit(b)),
    }
}

/// The table/html cost cell. The column header names the tag (`observed` when a capture is
/// bound, else `predicted`); a cell with another tag says so (`obs`, `pred`, `src`, `~`).
pub(crate) fn cost_column(cost: &Cost, observed_column: bool, g: &Glyphs) -> String {
    match cost_cell(cost) {
        CostCell::NotApplicable => g.dash.to_string(),
        CostCell::Unknown => "?".to_string(),
        CostCell::Source(b) => format!("src {}", kb(b)),
        CostCell::Observed(b) if observed_column => kb(b),
        CostCell::Observed(b) => format!("{} obs", kb(b)),
        CostCell::Predicted { bytes, estimate } => {
            let tilde = if estimate { "~" } else { "" };
            let tag = if observed_column { " pred" } else { "" };
            format!("{tilde}{}{tag}", kb(bytes))
        }
    }
}

/// `knobs[0]` exists and is not `Knob::None`: the row has an action that can free bytes.
pub(crate) fn has_action(item: &Item) -> bool {
    matches!(item.knobs.first(), Some(knob) if !matches!(knob, Knob::None { .. }))
}

/// `77.5 KB+Δ?`, `~3.3 KB`, `Δ?` (nothing known, delta unknown).
pub(crate) fn freed_value(freed: &Freed, g: &Glyphs, unit: fn(u64) -> String) -> String {
    if freed.known == 0 && freed.delta_unknown {
        return g.delta.to_string();
    }
    let tilde = if freed.is_estimate { "~" } else { "" };
    let delta = if freed.delta_unknown {
        format!("+{}", g.delta)
    } else {
        String::new()
    };
    format!("{tilde}{}{delta}", unit(freed.known))
}

/// The freed cell of a row: `—` without an applicable knob, blank when not computed.
pub(crate) fn freed_cell(item: &Item, g: &Glyphs, unit: fn(u64) -> String) -> String {
    if !has_action(item) {
        return g.dash.to_string();
    }
    match &item.freed {
        Some(freed) => freed_value(freed, g, unit),
        None => String::new(),
    }
}

/// Sort key: `freed.known`. A row without an applicable knob (freed `—`) frees nothing: 0, so it
/// sinks to the bottom of its block. `freed` None with a knob (not computed) falls back to the
/// first-request bytes.
pub(crate) fn sort_key(item: &Item) -> u64 {
    if !has_action(item) {
        return 0;
    }
    match &item.freed {
        Some(freed) => freed.known,
        None => request_bytes(&item.cost).unwrap_or(0),
    }
}

/// Descending by [`sort_key`]; stable, so ties keep the resolver's order.
pub(crate) fn sorted(items: &[Item]) -> Vec<&Item> {
    let mut rows: Vec<&Item> = items.iter().collect();
    rows.sort_by_key(|item| std::cmp::Reverse(sort_key(item)));
    rows
}

/// Modes with no first-request cost; their top-level rows go to the trailing block.
pub(crate) fn is_trailing(item: &Item) -> bool {
    matches!(item.mode, Mode::Runtime | Mode::None)
}

/// Top-level rows in display order: the sorted main block, then the sorted trailing block.
pub(crate) fn partition(tree: &Tree) -> (Vec<&Item>, Vec<&Item>) {
    sorted(&tree.items)
        .into_iter()
        .partition(|item| !is_trailing(item))
}

/// First segment of a JSON pointer, unescaped (`/enabledPlugins/slack@x` → `enabledPlugins`).
pub(crate) fn pointer_key(pointer: &str) -> String {
    let first = pointer.strip_prefix('/').unwrap_or(pointer);
    let first = first.split('/').next().unwrap_or_default();
    if first.is_empty() {
        return "(document)".to_string();
    }
    first.replace("~1", "/").replace("~0", "~")
}

/// Table/brief form: `enabledPlugins → false (local)`, `claudeMdExcludes (local)` and
/// `permissions.deny (local)` (append pointers `…/-` name the list), `NAME=1 (local)`,
/// `quarantine (project)`, `none (built-in)`. The full pointer and value are in `why`.
pub(crate) fn knob_compact(knob: &Knob, g: &Glyphs) -> String {
    match knob {
        Knob::Setting {
            pointer,
            value,
            scope,
            ..
        } => {
            if value.is_boolean() || value.is_number() || value.is_null() {
                return format!("{} {} {value} ({scope})", pointer_key(pointer), g.arrow);
            }
            match pointer.strip_suffix("/-") {
                Some(list) => {
                    let path: Vec<String> = list
                        .trim_start_matches('/')
                        .split('/')
                        .map(|seg| seg.replace("~1", "/").replace("~0", "~"))
                        .collect();
                    format!("{} ({scope})", path.join("."))
                }
                None => format!("{} ({scope})", pointer_key(pointer)),
            }
        }
        Knob::Env {
            name, value, scope, ..
        } => format!("{name}={value} ({scope})"),
        Knob::Quarantine { scope, .. } => format!("quarantine ({scope})"),
        Knob::None { reason } => format!("none ({reason})"),
    }
}

/// Default-action cell: `knobs[0]` compact, its note after `·`; `—` without knobs.
pub(crate) fn default_action(item: &Item, g: &Glyphs) -> String {
    match item.knobs.first() {
        None => g.dash.to_string(),
        Some(knob) => {
            let note = match knob {
                Knob::Setting { note: Some(n), .. } | Knob::Env { note: Some(n), .. } => {
                    format!(" {} {n}", g.dot)
                }
                _ => String::new(),
            };
            format!("{}{note}", knob_compact(knob, g))
        }
    }
}

/// Detail form (why, plan): file relative to `base` when inside it, full pointer and value.
pub(crate) fn knob_full(knob: &Knob, base: &Path, g: &Glyphs) -> String {
    let note = |note: &Option<String>| match note {
        Some(n) => format!("  {} {n}", g.dash),
        None => String::new(),
    };
    match knob {
        Knob::Setting {
            file,
            pointer,
            value,
            scope,
            note: n,
        } => format!(
            "{}  {pointer} = {value}  ({scope}){}",
            display_path(file, base),
            note(n)
        ),
        Knob::Env {
            name,
            value,
            scope,
            note: n,
        } => format!("env {name}={value} via settings.env  ({scope}){}", note(n)),
        Knob::Quarantine { from, to, scope } => format!(
            "move {} {} {}  ({scope})",
            display_path(from, base),
            g.arrow,
            display_path(to, base)
        ),
        Knob::None { reason } => format!("none {} {reason}", g.dash),
    }
}

/// `plugin slack@official`, `project`, `builtin`.
pub(crate) fn origin_short(origin: &Origin) -> String {
    match &origin.plugin {
        Some(plugin) => format!("{} {plugin}", origin.layer),
        None => origin.layer.to_string(),
    }
}

/// `path` relative to `base` when inside it (`.claude/settings.local.json`), else as is.
pub(crate) fn display_path(path: &Path, base: &Path) -> String {
    if base.as_os_str().is_empty() {
        return path.display().to_string();
    }
    match path.strip_prefix(base) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

/// Cuts `text` to `max` chars, ending with the ellipsis glyph.
pub(crate) fn truncate(text: &str, max: usize, g: &Glyphs) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(g.ellipsis.chars().count());
    let mut out: String = text.chars().take(keep).collect();
    out.push_str(g.ellipsis);
    out
}

/// `share` in tenths of a percent, `None` without bytes or denominator.
pub(crate) fn share_tenths(bytes: Option<u64>, denominator: u64) -> Option<u64> {
    let bytes = bytes?;
    if denominator == 0 {
        return None;
    }
    let tenths = (u128::from(bytes) * 1000 + u128::from(denominator) / 2) / u128::from(denominator);
    Some(u64::try_from(tenths).unwrap_or(u64::MAX))
}

/// `23.3%`.
pub(crate) fn percent(tenths: u64) -> String {
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// The share denominator: the observed body when a capture is bound, else the known sum.
pub(crate) fn denominator(tree: &Tree) -> u64 {
    tree.observed_body.unwrap_or(tree.predicted_known)
}

/// Header facts every renderer shows, already worded; `unit` formats bytes.
pub(crate) struct Facts {
    pub title: String,
    pub drift: Option<String>,
    pub totals: String,
    pub tool_search: Vec<String>,
    pub signals: Vec<String>,
    pub warnings: Vec<String>,
}

pub(crate) fn facts(tree: &Tree, g: &Glyphs, unit: fn(u64) -> String) -> Facts {
    let version = tree.agent_version.as_deref().unwrap_or("?");
    let title = format!(
        "{} {version} (rules verified {})",
        tree.agent, tree.rules_verified_against
    );
    let drift = match &tree.agent_version {
        Some(v) if *v != tree.rules_verified_against => Some(format!(
            "drift: installed {} {v}, rules verified against {} {} rows may be wrong until `harn verify` passes",
            tree.agent, tree.rules_verified_against, g.dash
        )),
        _ => None,
    };
    let unknown = match tree.unknown_rows {
        0 => String::new(),
        1 => " + ? (1 row unknown)".to_string(),
        n => format!(" + ? ({n} rows unknown)"),
    };
    let totals = match tree.observed_body {
        Some(body) => format!(
            "first request: observed {}{} (bound capture) {} static prediction {}{unknown}",
            unit(body),
            observed_split(tree)
                .map(|(a, p, f)| format!(
                    " = attributed {} + parameters {} + framing {}",
                    unit(a),
                    unit(p),
                    unit(f)
                ))
                .unwrap_or_default(),
            g.dot,
            unit(tree.predicted_known)
        ),
        None => format!(
            "first request: predicted {}{unknown}",
            unit(tree.predicted_known)
        ),
    };
    let cause = tree.tool_search_cause.as_deref();
    let tool_search = match tree.tool_search {
        Some(true) => vec![format!(
            "tool search: ON{} {} MCP tool schemas deferred (stubs until used)",
            cause.map(|c| format!(" ({c})")).unwrap_or_default(),
            g.arrow
        )],
        Some(false) => vec![
            format!(
                "tool search: OFF ({}) {} MCP tool schemas load eagerly",
                cause.unwrap_or("ANTHROPIC_BASE_URL is non-first-party"),
                g.arrow
            ),
            format!(
                "knob: env ENABLE_TOOL_SEARCH=true  [docs {} effect not measured; verify after]  freed: ?",
                g.dash
            ),
        ],
        None if tree.agent == Agent::ClaudeCode => vec![format!(
            "tool search: ? (undecidable from settings and env{})",
            cause.map(|c| format!(": {c}")).unwrap_or_default()
        )],
        None => Vec::new(),
    };
    Facts {
        title,
        drift,
        totals,
        tool_search,
        signals: tree
            .signals
            .iter()
            .map(|s| format!("signal: {s}"))
            .collect(),
        warnings: tree
            .warnings
            .iter()
            .map(|w| format!("warning: {w}"))
            .collect(),
    }
}

/// The observed body split by region (`Tree.observed_regions`, a partition of the body):
/// `(attributed, request parameters, framing)`. None without regions.
pub(crate) fn observed_split(tree: &Tree) -> Option<(u64, u64, u64)> {
    if tree.observed_regions.is_empty() {
        return None;
    }
    let (mut attributed, mut parameters, mut framing) = (0, 0, 0);
    for (name, bytes) in &tree.observed_regions {
        let group = name.split(" · ").next().unwrap_or(name);
        match group {
            "request parameters" => parameters += bytes,
            "framing" => framing += bytes,
            _ => attributed += bytes,
        }
    }
    Some((attributed, parameters, framing))
}

/// Whether any row's freed bytes were computed (the table names its sort key from this).
pub(crate) fn freed_computed(tree: &Tree) -> bool {
    crate::model::walk(tree).any(|item| item.freed.is_some())
}

/// The byte where the JSON string `text` ends with opens: `text` ends in an unescaped `"` and
/// this is the unescaped `"` before it (a quote after an odd run of `\` is escaped). None when
/// `text` ends in no such string.
pub(crate) fn json_string_tail(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let escaped = |at: usize| {
        bytes[..at]
            .iter()
            .rev()
            .take_while(|b| **b == b'\\')
            .count()
            % 2
            == 1
    };
    let last = bytes.len().checked_sub(1)?;
    if bytes[last] != b'"' || escaped(last) {
        return None;
    }
    (0..last).rev().find(|&i| bytes[i] == b'"' && !escaped(i))
}

/// A link value as a person reads it: a JSON string as its text (`"~/memory (old)"` →
/// `~/memory (old)`), anything else (`true`) as it is.
pub(crate) fn link_value_text(value: &str) -> String {
    serde_json::from_str::<String>(value).unwrap_or_else(|_| value.to_string())
}

/// A `why` step with its string value as text: `<link> = "<text>"` → `<link> = <text>`; any
/// other step as it is.
pub(crate) fn step_text(step: &str) -> String {
    match json_string_tail(step) {
        Some(open) if step[..open].ends_with(" = ") => {
            format!("{}{}", &step[..open], link_value_text(&step[open..]))
        }
        _ => step.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kb_matches_the_mockup_rounding() {
        assert_eq!(kb(77_536), "77.5 KB");
        assert_eq!(kb(332_548), "332.5 KB");
        assert_eq!(kb(738), "0.7 KB");
        assert_eq!(kb(2), "0.0 KB");
        assert_eq!(kb(999_949), "999.9 KB");
        assert_eq!(kb(999_950), "1.0 MB");
        assert!(kb(u64::MAX).ends_with(" MB"));
    }

    #[test]
    fn exact_groups_thousands() {
        assert_eq!(exact(0), "0 B");
        assert_eq!(exact(999), "999 B");
        assert_eq!(exact(77_536), "77,536 B");
        assert_eq!(exact(1_234_567), "1,234,567 B");
    }

    #[test]
    fn pointer_key_unescapes_the_first_segment() {
        assert_eq!(
            pointer_key("/enabledPlugins/slack@claude-plugins-official"),
            "enabledPlugins"
        );
        assert_eq!(pointer_key("/a~1b~0c/x"), "a/b~c");
        assert_eq!(pointer_key(""), "(document)");
    }

    #[test]
    fn share_rounds_half_up_and_guards_zero() {
        assert_eq!(share_tenths(Some(77_536), 332_548), Some(233));
        assert_eq!(share_tenths(Some(1), 0), None);
        assert_eq!(share_tenths(None, 10), None);
        assert_eq!(percent(233), "23.3%");
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate("ΔΔΔΔ", 4, &UNICODE), "ΔΔΔΔ");
        assert_eq!(truncate("ΔΔΔΔΔ", 4, &UNICODE), "ΔΔΔ…");
        assert_eq!(truncate("abcdefgh", 6, &ASCII), "abc...");
    }
}
