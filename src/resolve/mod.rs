//! Static resolvers: one per agent, files and settings only (`.prd/03-p0-interfaces.md`
//! §resolve). A resolver never starts the agent and never spawns a process.

pub mod claude;

use crate::model::{Agent, Overlay, Target, Tree};
use anyhow::anyhow;

pub trait Resolver {
    fn agent(&self) -> Agent;
    /// Agent version the discovery rules were checked against (`harn --version`, drift warning).
    fn rules_verified_against(&self) -> &'static str;
    fn resolve(&self, target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree>;
}

/// Claude Code resolver. Reads only files; never spawns a process.
pub struct ClaudeCode;

impl ClaudeCode {
    /// Claude Code version `research/01` was written against.
    pub const VERIFIED_AGAINST: &'static str = "2.1.296";
}

impl Resolver for ClaudeCode {
    fn agent(&self) -> Agent {
        Agent::ClaudeCode
    }

    fn rules_verified_against(&self) -> &'static str {
        Self::VERIFIED_AGAINST
    }

    fn resolve(&self, _target: &Target, _overlay: Option<&Overlay>) -> anyhow::Result<Tree> {
        Err(anyhow!("not implemented: WU-B"))
    }
}

/// Codex is P0.5; until its resolver lands, selecting it explicitly fails with this.
struct CodexPending;

impl Resolver for CodexPending {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn rules_verified_against(&self) -> &'static str {
        "unverified"
    }

    fn resolve(&self, _target: &Target, _overlay: Option<&Overlay>) -> anyhow::Result<Tree> {
        Err(anyhow!("not implemented: codex resolver (P0.5)"))
    }
}

pub fn for_agent(agent: Agent) -> Box<dyn Resolver> {
    match agent {
        Agent::ClaudeCode => Box::new(ClaudeCode),
        Agent::Codex => Box::new(CodexPending),
    }
}

/// Agents listed when `--agent` is not given ("default: all, one table each"): every agent with
/// a resolver in this build. P0 = Claude Code; P0.5 adds Codex here.
pub fn default_agents() -> &'static [Agent] {
    &[Agent::ClaudeCode]
}
