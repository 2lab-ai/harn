//! Output (WU-C). Every function returns text without a trailing newline; the CLI adds it.

use crate::journal::JournalEntry;
use crate::model::{Document, Item, Tree};
use crate::plan::ActionPlan;
use crate::verify::VerifyReport;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderOpts {
    pub unicode: bool,
    pub color: bool,
    pub width: Option<usize>,
}

/// Header lines (totals, tool search, signals, drift) + rows sorted by freed.known desc,
/// Runtime/None rows in a trailing block.
pub fn table(tree: &Tree, opts: &RenderOpts) -> String {
    let _ = opts;
    format!(
        "[render::table not implemented: WU-C] {} · {} top-level items",
        tree.agent,
        tree.items.len()
    )
}

pub fn why(tree: &Tree, item: &Item, plans: &[ActionPlan]) -> String {
    let _ = tree;
    format!(
        "[render::why not implemented: WU-C] {} · {} plan(s)",
        item.id,
        plans.len()
    )
}

/// Schema `harn/1`, serde pretty (2-space).
pub fn json(doc: &Document) -> String {
    // Fails only on a path that is not UTF-8; the signature has no error channel.
    serde_json::to_string_pretty(doc).expect("harn/1 document serializes (paths must be UTF-8)")
}

pub fn verify_report(report: &VerifyReport) -> String {
    format!(
        "[render::verify_report not implemented: WU-C] {} row(s), passed: {}",
        report.rows.len(),
        report.passed
    )
}

/// LLM-oriented; ends with the `harn-plan/1` format and an example op.
pub fn brief(doc: &Document) -> String {
    format!(
        "[render::brief not implemented: WU-C] {} agent tree(s)",
        doc.agents.len()
    )
}

/// One self-contained read-only page, inline CSS, no writing script, works at 400 px.
pub fn html(doc: &Document) -> String {
    format!(
        "<!-- render::html not implemented: WU-C; {} agent tree(s) -->",
        doc.agents.len()
    )
}

/// `harn off` / `harn apply`: target dir, diff, freed delta, side effects, warnings.
pub fn plan(plan: &ActionPlan) -> String {
    format!(
        "[render::plan not implemented: WU-C] {} op(s) in {}",
        plan.ops.len(),
        plan.target_dir.display()
    )
}

/// `harn history` / `undo` / `on` / `edit`: journal entries with their `RunContext`.
pub fn history(entries: &[JournalEntry]) -> String {
    format!(
        "[render::history not implemented: WU-C] {} entr(y/ies)",
        entries.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SCHEMA;

    #[test]
    fn json_is_pretty_two_space_with_the_schema_first() {
        let doc = Document {
            schema: SCHEMA.to_string(),
            generated_at: "2026-10-13T00:00:00Z".to_string(),
            agents: Vec::new(),
        };
        assert_eq!(
            json(&doc),
            "{\n  \"schema\": \"harn/1\",\n  \"generated_at\": \"2026-10-13T00:00:00Z\",\n  \"agents\": []\n}"
        );
    }
}
