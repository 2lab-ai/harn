//! Capture readers (WU-D): an OTEL raw-body directory or an llmux `raw-io.jsonl` record. A record
//! is used only when its `RunContext` matches the target; "the newest" is never a selection rule.

use super::Capture;
use crate::model::Target;
use anyhow::anyhow;
use std::path::Path;

/// `harn verify --capture <path>`: dispatches on what `path` is (OTEL raw-body dir, llmux
/// `raw-io.jsonl` file) and returns the one record whose `RunContext` matches `target`, or Err
/// naming the mismatch.
pub fn read_capture(path: &Path, target: &Target) -> anyhow::Result<Capture> {
    let _ = (path, target);
    Err(anyhow!("not implemented: WU-D"))
}

pub fn read_otel_dir(dir: &Path, target: &Target) -> anyhow::Result<Capture> {
    let _ = (dir, target);
    Err(anyhow!("not implemented: WU-D"))
}

/// `RunContext` derived from the body (gitStatus cwd, `Contents of <path>` headers, `cc_version`,
/// `model`); Err if underivable.
pub fn read_llmux_record(line: &str, target: &Target) -> anyhow::Result<Capture> {
    let _ = (line, target);
    Err(anyhow!("not implemented: WU-D"))
}
