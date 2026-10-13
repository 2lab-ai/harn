//! Claude Code oracle (WU-D): one headless `claude -p` under `OTEL_LOG_RAW_API_BODIES=file:<tmp>`
//! with `--output-format stream-json --verbose`; the request body and the `system/init` event.

use super::{Capture, Region};
use crate::model::{Target, Tree};
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The `system/init` stream-json event: what is enabled, from where (`research/04` §E.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitEvent {
    pub tools: Vec<String>,
    pub mcp_servers: Vec<InitMcpServer>,
    pub skills: Vec<String>,
    pub agents: Vec<String>,
    pub plugins: Vec<InitPlugin>,
    /// e.g. `{"auto": "~/.claude/projects/<slug>/memory/"}`.
    pub memory_paths: BTreeMap<String, String>,
    pub claude_code_version: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitMcpServer {
    pub name: String,
    pub status: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitPlugin {
    pub name: String,
    pub path: Option<PathBuf>,
    pub source: Option<String>,
    pub version: Option<String>,
}

/// `OTEL_LOG_RAW_API_BODIES=file:<tmp>` + `--output-format stream-json --verbose -p "hi"
/// --max-turns 1 --disallowedTools Edit,Write,NotebookEdit,Bash`, run in `target.dir`.
pub fn run_claude(target: &Target, claude_bin: &Path) -> anyhow::Result<Capture> {
    let _ = (target, claude_bin);
    Err(anyhow!("not implemented: WU-D"))
}

/// tools / system / messages[0] blocks / messages[1] sections, named like 01-spec §Problem.
pub fn parse_request_body(bytes: &[u8]) -> anyhow::Result<(Vec<Region>, Tree)> {
    let _ = bytes;
    Err(anyhow!("not implemented: WU-D"))
}

pub fn parse_init_event(line: &str) -> anyhow::Result<InitEvent> {
    let _ = line;
    Err(anyhow!("not implemented: WU-D"))
}
