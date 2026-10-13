//! harn: the effective harness an AI coding agent loads in a directory — every item with its
//! origin, load mode, byte cost and the agent's official off switch — and reversible changes to
//! it through a journal. Contract between modules: `.prd/03-p0-interfaces.md`.

pub mod journal;
pub mod model;
pub mod oracle;
pub mod plan;
pub mod render;
pub mod resolve;
pub mod trim;
pub mod tui;
pub mod verify;
pub mod writers;

/// `harn <ver> (<channel> <rev12>) rules: claude-code <verified>` — the `harn --version` line and
/// the HTML footer. Channel and build id come from `HARN_BUILD_CHANNEL` / `HARN_BUILD_ID` at
/// compile time (`dev` / `unknown` otherwise); the rules version of every default agent follows.
pub fn version_line() -> String {
    let channel = option_env!("HARN_BUILD_CHANNEL").unwrap_or("dev");
    let rev12: String = option_env!("HARN_BUILD_ID")
        .unwrap_or("unknown")
        .chars()
        .take(12)
        .collect();
    let rules: Vec<String> = resolve::default_agents()
        .iter()
        .map(|agent| {
            format!(
                "{agent} {}",
                resolve::for_agent(*agent).rules_verified_against()
            )
        })
        .collect();
    format!(
        "harn {} ({channel} {rev12}) rules: {}",
        env!("CARGO_PKG_VERSION"),
        rules.join(", ")
    )
}
