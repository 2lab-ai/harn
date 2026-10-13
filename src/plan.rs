//! ActionPlan builder (WU-E): knob candidates per item by scope, quarantine only when no knob
//! exists and only in the same scope, `freed = predicted(before) − predicted(after)` via a second
//! resolve with an [`Overlay`](crate::model::Overlay), side effects and warnings.

use crate::model::{Agent, Freed, Knob, Scope, Target, Tree};
use crate::resolve::Resolver;
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedOp {
    pub item_id: String,
    pub knob: Knob,
    pub file: Option<PathBuf>,
    pub hash_before: Option<String>,
    /// The operator's reason, from a `harn-plan/1` request.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionPlan {
    pub target_dir: PathBuf,
    pub agent: Agent,
    pub ops: Vec<PlannedOp>,
    pub predicted_before: u64,
    pub predicted_after: u64,
    pub freed: Freed,
    pub side_effects: Vec<String>,
    pub warnings: Vec<String>,
    /// Unified diff, all files.
    pub diff: String,
}

/// Plans turning `item_id` off. `scope` None = the default (`local`); a scope with no knob is
/// refused (`model::Refused`), never widened.
pub fn build_plan(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &Tree,
    item_id: &str,
    scope: Option<Scope>,
) -> anyhow::Result<ActionPlan> {
    let _ = (resolver, target, tree, item_id, scope);
    Err(anyhow!("not implemented: WU-E"))
}

/// Schema of an AI-authored plan file (`harn apply <plan.json>`).
pub const PLAN_SCHEMA: &str = "harn-plan/1";

/// AI-authored plan: `{"schema","target_dir","ops":[{"selector","scope"?,"knob"?,"reason"}]}`.
/// Unknown keys are rejected so a misspelt `knob`/`scope` cannot fall back to a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    /// [`PLAN_SCHEMA`].
    pub schema: String,
    pub target_dir: PathBuf,
    pub ops: Vec<PlanOp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOp {
    pub selector: String,
    /// `local` | `project` | `user`; None = default (`local`).
    pub scope: Option<Scope>,
    /// Index into `Item.knobs`; None = `knobs[0]`.
    pub knob: Option<usize>,
    pub reason: String,
}

/// Every selector unique (else Err listing candidates), knob index within `item.knobs` and not
/// `Knob::None`, scope never wider than requested; reasons kept in `PlannedOp.reason`.
pub fn build_plan_from_request(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &Tree,
    req: &PlanRequest,
) -> anyhow::Result<ActionPlan> {
    let _ = (resolver, target, tree, req);
    Err(anyhow!("not implemented: WU-E"))
}

/// Fills `Item.freed` for `knobs[0]` of every item via an overlay re-resolve.
pub fn annotate_freed(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &mut Tree,
) -> anyhow::Result<()> {
    // WU-E fills `Item.freed`; until then the listing is honest with `freed: None`.
    let _ = (resolver, target, tree);
    Ok(())
}
