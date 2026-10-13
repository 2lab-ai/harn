//! Static resolvers: one per agent, files and settings only (`.prd/03-p0-interfaces.md`
//! §resolve). A resolver never starts the agent and never spawns a process.

pub mod claude;

use crate::model::{Agent, Item, Knob, Overlay, Target, Tree};
use anyhow::anyhow;

/// `Send + Sync`: `plan::annotate_freed` re-resolves distinct overlays on several threads.
pub trait Resolver: Send + Sync {
    fn agent(&self) -> Agent;
    /// Agent version the discovery rules were checked against (`harn --version`, drift warning).
    fn rules_verified_against(&self) -> &'static str;
    fn resolve(&self, target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree>;

    /// The tree after `item`'s `knob` is applied, derived from `tree` without reading a file —
    /// only when the resolver's own rules prove the knob removes exactly `item` with its
    /// descendants and changes nothing else with a first-request cost. It must equal what
    /// [`Resolver::resolve`] with the knob's overlay gives, as far as freed bytes can tell;
    /// `plan::annotate_freed` uses it to skip that re-resolve. None (the default) = re-resolve.
    fn after_removal(&self, _tree: &Tree, _item: &Item, _knob: &Knob) -> Option<Tree> {
        None
    }
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

    fn resolve(&self, target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree> {
        claude::resolve(target, overlay)
    }

    fn after_removal(&self, tree: &Tree, item: &Item, knob: &Knob) -> Option<Tree> {
        claude::after_removal(tree, item, knob)
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
