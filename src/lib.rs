//! harn: the effective harness an AI coding agent loads in a directory — every item with its
//! origin, load mode, byte cost and the agent's official off switch — and reversible changes to
//! it through a journal. Contract between modules: `.prd/03-p0-interfaces.md`.

pub mod journal;
pub mod model;
pub mod oracle;
pub mod plan;
pub mod render;
pub mod resolve;
pub mod verify;
pub mod writers;
