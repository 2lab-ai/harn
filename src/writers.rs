//! Writes (WU-E): atomic replace (temp + fsync + rename); JSON via serde_json with
//! `preserve_order`, 2-space; TOML via toml_edit; quarantine moves to
//! `<discovery-root-parent>/harn-off/<kind>/<name>` + `manifest.json`. Every write goes through
//! the journal (snapshot first).

use crate::journal::{Journal, JournalEntry};
use crate::plan::ActionPlan;
use anyhow::anyhow;

/// Snapshots every file the plan touches into the journal, applies the ops, records the entry.
pub fn apply(plan: &ActionPlan, journal: &mut Journal) -> anyhow::Result<JournalEntry> {
    let _ = (plan, journal);
    Err(anyhow!("not implemented: WU-E"))
}
