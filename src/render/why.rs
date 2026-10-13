//! `harn why <sel>`: the detail pane (01-spec §Detail pane).

use super::common::{
    cost_tagged, exact, freed_cell, freed_value, glyphs, kb, knob_full, request_bytes, sorted,
    Glyphs,
};
use crate::model::{Item, Tree};
use crate::plan::ActionPlan;
use std::path::Path;

/// Header (label, id, target), cost and mode, origin, signal, the `why loaded` chain, every
/// child (`also contributes`), last used, one action line per plan (or per knob candidate when
/// nothing was planned), warnings. `unicode` false = ASCII decorations (`|`, `->`, `D?`); labels,
/// ids and paths are printed as they are.
pub fn why(tree: &Tree, item: &Item, plans: &[ActionPlan], unicode: bool) -> String {
    let g = glyphs(unicode);
    let base = tree.target_dir.as_path();
    let mut out = vec![
        format!(
            "{}   ({})   target: {}",
            item.label,
            item.id,
            base.display()
        ),
        format!(
            "  {} {} {} {} mode {}",
            item.kind,
            g.dot,
            cost_tagged(&item.cost, g, exact),
            g.dot,
            item.mode
        ),
    ];

    let mut origin = vec![item.origin.layer.to_string()];
    origin.extend(item.origin.plugin.clone());
    origin.extend(item.origin.path.as_ref().map(|p| p.display().to_string()));
    origin.extend(item.origin.setting.clone());
    out.push(format!(
        "  origin      {}",
        origin.join(&format!(" {} ", g.dot))
    ));
    if let Some(signal) = &item.signal {
        out.push(format!("  signal      {signal}"));
    }

    if item.origin.why.is_empty() {
        out.push("  why loaded  ? (no admission chain recorded)".to_string());
    } else {
        out.push("  why loaded".to_string());
        for (i, step) in item.origin.why.iter().enumerate() {
            let arrow = if i == 0 {
                String::new()
            } else {
                format!("{} ", g.arrow)
            };
            out.push(format!("    {arrow}{step}"));
        }
    }

    if !item.children.is_empty() {
        out.push(format!(
            "  also contributes  {}",
            children_summary(&item.children, g)
        ));
        push_children(&mut out, &item.children, 2, g);
    }

    out.push(format!(
        "  last used   {}",
        item.last_used.as_deref().unwrap_or("unknown")
    ));

    let mut warnings: Vec<String> = Vec::new();
    out.extend(actions(item, plans, base, g, &mut warnings));
    for (i, warning) in warnings.iter().enumerate() {
        let lead = if i == 0 {
            "  warnings  "
        } else {
            "            "
        };
        out.push(format!("{lead}{warning}"));
    }
    out.join("\n")
}

/// `29 children: 27 mcp-tool · 2 skill · 12.3 KB known · 27 ?`.
fn children_summary(children: &[Item], g: &Glyphs) -> String {
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for child in children {
        let kind = child.kind.to_string();
        match kinds.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => *n += 1,
            None => kinds.push((kind, 1)),
        }
    }
    let mut parts: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
    let known: u64 = children.iter().filter_map(|c| request_bytes(&c.cost)).sum();
    let unknown = children
        .iter()
        .filter(|c| request_bytes(&c.cost).is_none() && !c.cost.not_applicable)
        .count();
    if children.iter().any(|c| request_bytes(&c.cost).is_some()) {
        parts.push(format!("{} known", kb(known)));
    }
    if unknown > 0 {
        parts.push(format!("{unknown} ?"));
    }
    let noun = if children.len() == 1 {
        "child"
    } else {
        "children"
    };
    format!(
        "{} {noun}: {}",
        children.len(),
        parts.join(&format!(" {} ", g.dot))
    )
}

/// One line per descendant: `cost  mode  label  (id)`, indented by depth.
fn push_children(out: &mut Vec<String>, children: &[Item], depth: usize, g: &Glyphs) {
    for child in sorted(children) {
        out.push(format!(
            "{}{}  {}  {}  ({})",
            "  ".repeat(depth),
            cost_tagged(&child.cost, g, kb),
            child.mode,
            child.label,
            child.id
        ));
        push_children(out, &child.children, depth + 1, g);
    }
}

fn actions(
    item: &Item,
    plans: &[ActionPlan],
    base: &Path,
    g: &Glyphs,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    // (knob text, is default, freed text, side effects)
    let mut rows: Vec<(String, bool, String, Vec<String>)> = Vec::new();
    if plans.is_empty() {
        for (i, knob) in item.knobs.iter().enumerate() {
            let freed = if i == 0 {
                match freed_cell(item, g, kb) {
                    empty if empty.is_empty() => "not computed".to_string(),
                    text => text,
                }
            } else {
                "not planned".to_string()
            };
            rows.push((knob_full(knob, base, g), i == 0, freed, Vec::new()));
        }
    } else {
        for (i, plan) in plans.iter().enumerate() {
            // Only this item's op: another op's knob is not an action on this item.
            let op = plan.ops.iter().find(|op| op.item_id == item.id);
            let knob = op.map_or_else(
                || "(no op for this item in this plan)".to_string(),
                |op| knob_full(&op.knob, base, g),
            );
            let default = op.is_some_and(|op| item.knobs.first() == Some(&op.knob));
            let freed = format!(
                "{}  (predicted {} {} {})",
                freed_value(&plan.freed, g, kb),
                kb(plan.predicted_before),
                g.arrow,
                kb(plan.predicted_after)
            );
            rows.push((knob, default, freed, plan.side_effects.clone()));
            warnings.extend(plan.warnings.iter().map(|w| format!("[{}] {w}", i + 1)));
        }
    }
    if rows.is_empty() {
        return vec![format!(
            "  actions     none (no knob {} nothing to turn off)",
            g.dash
        )];
    }
    let width = rows
        .iter()
        .map(|(knob, ..)| knob.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = vec!["  actions".to_string()];
    for (i, (knob, default, freed, side_effects)) in rows.iter().enumerate() {
        let mark = if *default {
            format!("{} default", g.left)
        } else {
            String::new()
        };
        let line = format!("    [{}] {knob:<width$}  {mark:<9}  freed {freed}", i + 1);
        out.push(line.trim_end().to_string());
        for effect in side_effects {
            out.push(format!("        side effect: {effect}"));
        }
    }
    out
}
