//! Plain-text reports: `harn verify`, `harn off`/`apply` plans, `harn history`.

use super::common::{exact, freed_value, glyphs, kb, knob_full, UNICODE};
use crate::journal::JournalEntry;
use crate::model::RunContext;
use crate::plan::ActionPlan;
use crate::verify::{RowVerdict, VerifyReport};

/// Verdict groups in print order, named like the serde values.
const GROUPS: [&str; 4] = [
    "seen",
    "predicted-not-seen",
    "seen-not-predicted",
    "content-mismatch",
];

fn group(verdict: &RowVerdict) -> &'static str {
    match verdict {
        RowVerdict::Seen => GROUPS[0],
        RowVerdict::PredictedNotSeen => GROUPS[1],
        RowVerdict::SeenNotPredicted => GROUPS[2],
        RowVerdict::ContentMismatch { .. } => GROUPS[3],
    }
}

/// The context line, then rows grouped by verdict (`seen`, `predicted-not-seen`,
/// `seen-not-predicted`, `content-mismatch` with both numbers), then `passed` and `promoted`.
pub fn verify_report(report: &VerifyReport) -> String {
    let g = &UNICODE;
    let mut out = vec![format!("verify {}", context_line(&report.context))];
    for name in GROUPS {
        let rows: Vec<&(String, RowVerdict)> = report
            .rows
            .iter()
            .filter(|(_, v)| group(v) == name)
            .collect();
        out.push(format!("{name} ({})", rows.len()));
        for (id, verdict) in rows {
            match verdict {
                RowVerdict::ContentMismatch {
                    predicted,
                    observed,
                } => out.push(format!(
                    "  {id}   predicted {} {} observed {}",
                    exact(*predicted),
                    g.dot,
                    exact(*observed)
                )),
                _ => out.push(format!("  {id}")),
            }
        }
    }
    out.push(format!(
        "passed: {}",
        if report.passed { "yes" } else { "no" }
    ));
    out.push(format!(
        "promoted: {} row(s) predicted {} observed",
        report.promoted, g.arrow
    ));
    out.join("\n")
}

/// `claude-code 2.1.296 · model m · in /dir · <capture_kind> <captured_at>`.
fn context_line(ctx: &RunContext) -> String {
    let g = &UNICODE;
    let mut parts = vec![format!(
        "{} {}",
        ctx.agent,
        ctx.agent_version.as_deref().unwrap_or("?")
    )];
    if let Some(model) = &ctx.model {
        parts.push(format!("model {model}"));
    }
    parts.push(format!("in {}", ctx.cwd.display()));
    parts.push(format!("{} {}", ctx.capture_kind, ctx.captured_at));
    parts.join(&format!(" {} ", g.dot))
}

/// Target dir, ops (item id, knob, scope, file, reason), predicted before → after, freed,
/// side effects, warnings, then the unified diff verbatim. `unicode` false = ASCII decorations;
/// ids, paths, side effects and the diff are printed as they are.
pub fn plan(plan: &ActionPlan, unicode: bool) -> String {
    let g = glyphs(unicode);
    let base = plan.target_dir.as_path();
    let mut out = vec![
        format!("plan for {} {} {}", base.display(), g.dot, plan.agent),
        format!("ops ({})", plan.ops.len()),
    ];
    for (i, op) in plan.ops.iter().enumerate() {
        out.push(format!("  [{}] {}", i + 1, op.item_id));
        out.push(format!("      knob    {}", knob_full(&op.knob, base, g)));
        if let Some(file) = &op.file {
            let hash = op
                .hash_before
                .as_deref()
                .map(|h| format!("  (sha256 {})", h.chars().take(12).collect::<String>()))
                .unwrap_or_default();
            out.push(format!("      file    {}{hash}", file.display()));
        }
        if let Some(reason) = &op.reason {
            out.push(format!("      reason  {reason}"));
        }
    }
    out.push(format!(
        "predicted  {} {} {}   freed {}",
        exact(plan.predicted_before),
        g.arrow,
        exact(plan.predicted_after),
        freed_value(&plan.freed, g, kb)
    ));
    push_list(&mut out, "side effects", &plan.side_effects);
    push_list(&mut out, "warnings", &plan.warnings);
    let diff = plan.diff.trim_end_matches('\n');
    if diff.is_empty() {
        out.push("diff: (no file changes)".to_string());
    } else {
        out.push("diff".to_string());
        out.push(diff.to_string());
    }
    out.join("\n")
}

fn push_list(out: &mut Vec<String>, title: &str, items: &[String]) {
    if items.is_empty() {
        out.push(format!("{title}: none"));
    } else {
        out.push(title.to_string());
        out.extend(items.iter().map(|item| format!("  {item}")));
    }
}

/// One line per entry, newest first: id, applied_at, its `RunContext` (agent, version, cwd),
/// op count, and each op's item with its reason; an undone entry ends with `undone <undone_at>`.
pub fn history(entries: &[JournalEntry]) -> String {
    if entries.is_empty() {
        return "no journal entries".to_string();
    }
    let g = &UNICODE;
    let mut newest_first: Vec<&JournalEntry> = entries.iter().collect();
    newest_first.sort_by(|a, b| {
        b.applied_at
            .cmp(&a.applied_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    newest_first
        .iter()
        .map(|entry| {
            let ctx = &entry.context;
            let ops: Vec<String> = entry
                .ops
                .iter()
                .map(|op| match &op.reason {
                    Some(reason) => format!("{} ({reason})", op.item_id),
                    None => op.item_id.clone(),
                })
                .collect();
            let noun = if entry.ops.len() == 1 { "op" } else { "ops" };
            let undone = match &entry.undone_at {
                Some(at) => format!("  undone {at}"),
                None => String::new(),
            };
            format!(
                "{}  {}  {} {} {} {}  {} {noun}: {}{undone}",
                entry.id,
                entry.applied_at,
                ctx.agent,
                ctx.agent_version.as_deref().unwrap_or("?"),
                g.dot,
                ctx.cwd.display(),
                entry.ops.len(),
                ops.join("; ")
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}
