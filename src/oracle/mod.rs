//! The agents' own ground truth (WU-D): a headless run under the official capture, or a capture
//! read from disk, turned into a [`Capture`] bound to its [`RunContext`].

pub mod capture;
pub mod claude;

pub use capture::{read_capture, read_llmux_record, read_otel_dir};
pub use claude::{parse_init_event, parse_request_body, run_claude, InitEvent};

use crate::model::{RunContext, Tree};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub context: RunContext,
    pub body_bytes: u64,
    /// Top-level request regions with bytes.
    pub regions: Vec<Region>,
    /// Observed items, same id rules as the resolver.
    pub tree: Tree,
}

/// One top-level region of a request body, named like `01-spec.md` §Problem
/// (`tools`, `system`, `messages[0] block 0`, `messages[1] system text`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    pub bytes: u64,
}
