//! `harn brief`: the same facts as `--json`, compact text for an LLM operator (01-spec §P0
//! `brief`, A12: < 40 KB for zbrain). Ends with the `harn-plan/1` format and an example op.
//!
//! Budget levers: the why step only on top-level rows, `~` for `$HOME` in why steps and
//! warnings (ids stay verbatim so every printed id is a selector), at most [`CHILDREN_SHOWN`]
//! children per row (the rest are one `… N more (harn --json)` line), and `Δ?` unknowns counted
//! per parent instead of listed per child.

use super::common::{
    cost_cell, facts, freed_cell, has_action, knob_compact, partition, plain, sorted, step_text,
    CostCell, Glyphs, UNICODE,
};
use crate::model::{Document, Item, Knob, Scope, Tree};
use crate::plan::{PlanOp, PlanRequest, PLAN_SCHEMA};
use std::path::Path;

/// Children printed per row; the rest collapse into one `… N more (harn --json)` line.
pub const CHILDREN_SHOWN: usize = 40;

const LEGEND: &[&str] = &[
    "All numbers are bytes. One section per agent; never add agents together.",
    "row: id|layer|mode|cost|freed|knob0|why. Leading spaces = child of the nearest less-indented row above. ^ = same as the line above (for knob0: the same knob). `… N more (harn --json)` = children beyond the first {CHILDREN_SHOWN}, in the same order.",
    "cost (first request): 1234 predicted · ~1234 estimated · o1234 observed · s1234 on disk only, not a first-request number · ? unknown · — none (mode runtime/none).",
    "freed: bytes that knob0 frees (predicted) · +Δ? plus an unknown re-render delta · Δ? delta unknown · — no applicable knob · empty = not computed.",
    "knob0: the item's default knob (index 0), scope local unless (project)/(user) is shown · +N = N more knobs (harn why <id>).",
    "why (top-level rows only): the last step of the admission chain; empty when the id names it or it is the item's own file (harn path <id>); ./ = the target dir, ~ = $HOME.",
];

/// One header per agent (title, drift, totals, tool search, signals, warnings), every row with
/// its children, the open unknowns, then the plan format with an example op from the document.
pub fn brief(doc: &Document) -> String {
    let g = &UNICODE;
    let mut out = vec![format!(
        "harn brief {} schema {} {} generated {}",
        g.dot, doc.schema, g.dot, doc.generated_at
    )];
    out.extend(
        LEGEND
            .iter()
            .map(|line| line.replace("{CHILDREN_SHOWN}", &CHILDREN_SHOWN.to_string())),
    );
    let mut unknowns: Vec<String> = Vec::new();
    for tree in &doc.agents {
        out.push(String::new());
        agent_section(&mut out, &mut unknowns, tree, g);
    }
    out.push(String::new());
    if unknowns.is_empty() {
        out.push("UNKNOWNS: none".to_string());
    } else {
        out.push("UNKNOWNS:".to_string());
        out.extend(unknowns.into_iter().map(|u| format!("- {u}")));
    }
    out.push(String::new());
    out.extend(plan_format(doc));
    out.join("\n")
}

fn agent_section(out: &mut Vec<String>, unknowns: &mut Vec<String>, tree: &Tree, g: &Glyphs) {
    let f = facts(tree, g, plain);
    out.push(format!(
        "== {} {} target {}",
        f.title,
        g.dot,
        tree.target_dir.display()
    ));
    out.extend(f.drift);
    out.push(f.totals);
    if !f.tool_search.is_empty() {
        out.push(f.tool_search.join("; "));
    }
    let home = tree.home.as_deref();
    out.extend(f.signals.into_iter().map(|line| tilde(&line, home)));
    out.extend(f.warnings.into_iter().map(|line| tilde(&line, home)));
    out.push("rows:".to_string());

    let paths = Paths {
        base: format!("{}/", tree.target_dir.display()),
        home,
    };
    let (main, trailing) = partition(tree);
    let mut not_computed = 0usize;
    let mut above = None;
    let start = unknowns.len();
    for item in main.into_iter().chain(trailing) {
        push_row(out, item, 0, &mut above, &paths, g);
        collect_unknowns(unknowns, &mut not_computed, item, true);
    }
    if unknowns[start..]
        .iter()
        .any(|u| u.contains(": freed has Δ?"))
    {
        unknowns.insert(
            start,
            format!(
                "{}: Δ? = a token-budgeted listing re-renders or a row of unknown cost leaves; exact only from a before/after capture pair",
                tree.agent
            ),
        );
    }

    if tree.agent_version.is_none() {
        unknowns.push(format!(
            "{}: agent version unknown, drift unchecked",
            tree.agent
        ));
    }
    if tree.tool_search.is_none() && !f.tool_search.is_empty() {
        unknowns.push(format!(
            "{}: tool search state undecidable statically",
            tree.agent
        ));
    }
    if not_computed > 0 {
        unknowns.push(format!(
            "{}: freed not computed for {not_computed} row(s) with a knob (empty freed cells)",
            tree.agent
        ));
    }
}

/// The ditto state: what the line above showed.
struct Above<'a> {
    layer: String,
    mode: String,
    knobs: &'a [Knob],
}

/// The prefixes a why step is shortened by.
struct Paths<'a> {
    /// `<target dir>/` → `./`.
    base: String,
    /// `$HOME` → `~`.
    home: Option<&'a Path>,
}

/// `<home>/…` → `~/…` everywhere in `text`.
fn tilde(text: &str, home: Option<&Path>) -> String {
    match home.map(|h| format!("{}/", h.display())) {
        Some(prefix) if prefix.len() > 1 => text.replace(&prefix, "~/"),
        _ => text.to_string(),
    }
}

/// One row per item, children after their parent, one leading space per level. A layer or mode
/// equal to the line above prints as `^`; knob0 does only when the knobs themselves are equal
/// (`enabledPlugins → false` of two plugins is the same text but not the same knob). The why
/// step is printed on top-level rows only; past [`CHILDREN_SHOWN`] children one line counts the
/// rest.
fn push_row<'a>(
    out: &mut Vec<String>,
    item: &'a Item,
    depth: usize,
    above: &mut Option<Above<'a>>,
    paths: &Paths,
    g: &Glyphs,
) {
    let layer = item.origin.layer.to_string();
    let mode = item.mode.to_string();
    let (layer_cell, mode_cell, same_knobs) = match above {
        Some(a) => (
            if a.layer == layer {
                "^"
            } else {
                layer.as_str()
            },
            if a.mode == mode { "^" } else { mode.as_str() },
            !item.knobs.is_empty() && a.knobs == item.knobs.as_slice(),
        ),
        None => (layer.as_str(), mode.as_str(), false),
    };
    let knob = match item.knobs.first() {
        None => g.dash.to_string(),
        Some(_) if same_knobs => "^".to_string(),
        Some(first) => match item.knobs.len() {
            1 => knob_brief(first, g),
            n => format!("{} +{}", knob_brief(first, g), n - 1),
        },
    };
    let line = format!(
        "{}{}|{layer_cell}|{mode_cell}|{}|{}|{knob}|{}",
        " ".repeat(depth),
        item.id,
        cost(item, g),
        freed_cell(item, g, plain),
        if depth == 0 {
            why_cell(item, paths)
        } else {
            String::new()
        },
    );
    out.push(line.trim_end_matches('|').to_string());
    *above = Some(Above {
        layer,
        mode,
        knobs: &item.knobs,
    });
    let children = sorted(&item.children);
    for child in children.iter().take(CHILDREN_SHOWN) {
        push_row(out, child, depth + 1, above, paths, g);
    }
    if children.len() > CHILDREN_SHOWN {
        out.push(format!(
            "{}{} {} more (harn --json)",
            " ".repeat(depth + 1),
            g.ellipsis,
            children.len() - CHILDREN_SHOWN
        ));
        // The next row's `^` refers to a printed row, never to a collapsed one.
        *above = None;
    }
}

/// [`knob_compact`] without the default ` (local)`.
fn knob_brief(knob: &Knob, g: &Glyphs) -> String {
    let compact = knob_compact(knob, g);
    match compact.strip_suffix(" (local)") {
        Some(short) => short.to_string(),
        None => compact,
    }
}

/// The last admission step, unless the row already names it: the id contains it, it starts
/// with the id's name/realpath, or it is the item's own file (`harn path <id>`). The target dir
/// prefix becomes `./`, the home prefix `~/`.
fn why_cell(item: &Item, paths: &Paths) -> String {
    let Some(step) = item.origin.why.last() else {
        return String::new();
    };
    // A string setting as its text, as every surface shows it.
    let step = &step_text(step);
    let name = item.id.splitn(4, '/').nth(3).unwrap_or_default();
    let own_file = item
        .origin
        .path
        .as_ref()
        .is_some_and(|path| path.as_os_str() == step.as_str());
    if own_file || item.id.contains(step.as_str()) || (!name.is_empty() && step.starts_with(name)) {
        return String::new();
    }
    match step.strip_prefix(&paths.base) {
        Some(rel) => format!("./{rel}"),
        None => tilde(step, paths.home),
    }
}

fn cost(item: &Item, g: &Glyphs) -> String {
    match cost_cell(&item.cost) {
        CostCell::NotApplicable => g.dash.to_string(),
        CostCell::Unknown => "?".to_string(),
        CostCell::Observed(b) => format!("o{b}"),
        CostCell::Predicted { bytes, estimate } => {
            format!("{}{bytes}", if estimate { "~" } else { "" })
        }
        CostCell::Source(b) => format!("s{b}"),
    }
}

/// `?` costs and `Δ?` deltas (top-level rows by id; children counted under their parent), and
/// the number of rows whose freed bytes were not computed.
fn collect_unknowns(unknowns: &mut Vec<String>, not_computed: &mut usize, item: &Item, top: bool) {
    if top && cost_cell(&item.cost) == CostCell::Unknown {
        unknowns.push(format!("{}: cost ?", item.id));
    }
    if top && delta_unknown(item) {
        unknowns.push(format!("{}: freed has Δ?", item.id));
    }
    if item.freed.is_none() && has_action(item) {
        *not_computed += 1;
    }
    let n = item.children.len();
    let unknown_children = item
        .children
        .iter()
        .filter(|c| cost_cell(&c.cost) == CostCell::Unknown)
        .count();
    if unknown_children > 0 {
        unknowns.push(format!(
            "{}: {unknown_children} of {n} children cost ?",
            item.id
        ));
    }
    let delta_children = item.children.iter().filter(|c| delta_unknown(c)).count();
    if delta_children > 0 {
        unknowns.push(format!(
            "{}: freed has Δ? on {delta_children} of {n} children",
            item.id
        ));
    }
    for child in &item.children {
        collect_unknowns(unknowns, not_computed, child, false);
    }
}

fn delta_unknown(item: &Item) -> bool {
    item.freed.as_ref().is_some_and(|f| f.delta_unknown) && has_action(item)
}

/// The `harn-plan/1` format, an example op on a real id, and the rules.
fn plan_format(doc: &Document) -> Vec<String> {
    let mut out = vec![
        format!("PLAN FORMAT ({PLAN_SCHEMA}): write a JSON file, run `harn apply <file> --dry-run`, then `harn apply <file>`."),
        format!(
            r#"{{"schema": "{PLAN_SCHEMA}", "target_dir": "<the target dir above>", "ops": [{{"selector": "<item id>", "scope": "local|project|user" (optional, default local), "knob": <index into the item's knobs> (optional, default 0), "reason": "<why this item goes>"}}]}}"#
        ),
        "Unknown keys are rejected. A selector is a full id, or a unique suffix, label or substring of one.".to_string(),
    ];
    let example = example_request(doc);
    match example {
        Some((req, illustrative)) => {
            out.push(if illustrative {
                "example (no item here has an applicable knob; shape only):".to_string()
            } else {
                "example:".to_string()
            });
            out.push(
                serde_json::to_string_pretty(&req)
                    .expect("a PlanRequest serializes (paths must be UTF-8)"),
            );
        }
        None => out.push("example: (no items)".to_string()),
    }
    out.push("rules:".to_string());
    out.push(
        "- knob is an index into the item's knobs; 0 = knob0 in the row; a `none` knob cannot be applied."
            .to_string(),
    );
    out.push(
        "- scope never wider than needed: omit it (= local, this project only) unless the item must go everywhere; harn never widens a scope itself."
            .to_string(),
    );
    out.push("- one reason per op.".to_string());
    out
}

/// The first row in display order with an applicable knob; else any item (`illustrative`).
fn example_request(doc: &Document) -> Option<(PlanRequest, bool)> {
    let mut fallback: Option<(&Tree, &Item)> = None;
    for tree in &doc.agents {
        let (main, trailing) = partition(tree);
        for item in main.into_iter().chain(trailing) {
            if let Some(found) = first_actionable(item) {
                return Some((request(tree, found), false));
            }
            fallback = fallback.or(Some((tree, item)));
        }
    }
    fallback.map(|(tree, item)| (request(tree, item), true))
}

fn first_actionable(item: &Item) -> Option<&Item> {
    if has_action(item) {
        return Some(item);
    }
    sorted(&item.children)
        .into_iter()
        .find_map(first_actionable)
}

fn request(tree: &Tree, item: &Item) -> PlanRequest {
    PlanRequest {
        schema: PLAN_SCHEMA.to_string(),
        target_dir: tree.target_dir.clone(),
        ops: vec![PlanOp {
            selector: item.id.clone(),
            scope: Some(Scope::Local),
            knob: Some(0),
            reason: "example only: replace with the real reason for this item".to_string(),
        }],
    }
}
