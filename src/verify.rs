//! Static tree vs a bound capture (WU-D), on the oracle's observable projection.

use crate::model::{RunContext, Tree};
use crate::oracle::Capture;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowVerdict {
    Seen,
    PredictedNotSeen,
    SeenNotPredicted,
    ContentMismatch { predicted: u64, observed: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyReport {
    pub context: RunContext,
    /// `(item id, verdict)`.
    pub rows: Vec<(String, RowVerdict)>,
    pub passed: bool,
    pub promoted: u32,
}

/// Projection: Runtime/None skipped; capture-only children attach under `?` parents (never
/// SeenNotPredicted); presence gate on static ids; content conformance on reproducible static
/// items; promotes predicted→observed on matched rows; fills `Tree.observed_body` and the over-cap
/// signal.
pub fn verify(static_tree: &mut Tree, capture: &Capture) -> VerifyReport {
    let _ = (static_tree, capture);
    // Not reachable from the CLI yet: every capture source still returns Err (WU-D).
    todo!("not implemented: WU-D")
}
