//! The agents' own ground truth (WU-D): a headless run under the official capture, or a capture
//! read from disk, turned into a [`Capture`] bound to its [`RunContext`].

pub mod capture;
pub mod claude;

pub use capture::{read_capture, read_llmux_record, read_otel_dir};
pub use claude::{
    find_init_event, parse_init_event, parse_request_body, run_claude, settings_fingerprint,
    InitEvent, InitMcpServer, InitPlugin,
};

use crate::model::{RunContext, Tree};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub context: RunContext,
    pub body_bytes: u64,
    /// Request regions with bytes: a partition of the body (leaf regions), so they sum to
    /// `body_bytes` exactly. See [`Region`] for the naming.
    pub regions: Vec<Region>,
    /// Observed items, same id rules as the resolver.
    pub tree: Tree,
    /// The `system/init` event of the same session, when the capture has one (an OTEL run or
    /// directory with its `stream.jsonl`; never an llmux record). `verify` reads plugin presence
    /// and MCP server status from it.
    #[serde(default)]
    pub init: Option<InitEvent>,
}

/// One leaf region of a request body, named like `01-spec.md` §Problem: `<group> · <part>`, e.g.
/// `tools · builtin (26 tools)`, `messages[0] block 0 · /repo/CLAUDE.md`,
/// `messages[1] system text · skill list (216 entries, 70 with descriptions)`,
/// `request parameters`, `framing`. The group is the spec's top-level region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    pub bytes: u64,
}

impl Region {
    /// Separator between the top-level region and the part inside it.
    pub const SEP: &'static str = " · ";

    /// The top-level region (`tools`, `system`, `messages[0] block 0`, `messages[1] system text`,
    /// `messages[1] tool_addition`, `request parameters`, `framing`, ...).
    pub fn group(&self) -> &str {
        self.name
            .split_once(Self::SEP)
            .map_or(self.name.as_str(), |(group, _)| group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_group_is_the_part_before_the_separator() {
        let r = |name: &str| Region {
            name: name.to_string(),
            bytes: 0,
        };
        assert_eq!(r("tools · builtin (2 tools)").group(), "tools");
        assert_eq!(
            r("messages[0] block 0 · /a/b · c/CLAUDE.md").group(),
            "messages[0] block 0"
        );
        assert_eq!(r("framing").group(), "framing");
    }
}
