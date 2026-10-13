//! Static tree vs a bound capture (WU-D), on the oracle's observable projection
//! (`01-spec.md` Goal 5, acceptance A3).
//!
//! Items join by id; when one side's layer is `unresolved` (a capture cannot tell a user skill
//! from a project one), or the item is named by a file path, they join by kind + name. A listing
//! entry joins a static skill or command of the same name (Claude Code lists both in the Skill
//! tool's listing).
//!
//! - **Projection.** Static items whose mode (or an ancestor's) is `runtime`, `none` or
//!   `on-demand` are expected absent and never counted missing; if the request carries one, it is
//!   `seen-not-predicted`. Built-in tools removed by the run's launch options
//!   (`--disallowedTools`, `--tools`) are not expected. Plugins are observable only with an
//!   `init` event; an MCP server the `init` event reports as not connected (other than
//!   `disabled`) is a runtime condition, reported as a warning.
//! - **Presence gate** on the statically discoverable kinds ([`gated`]): instruction files,
//!   memory, skills / commands, subagents, the two listings, MCP servers, plugins, built-in
//!   tools — `predicted-not-seen` and `seen-not-predicted` both fail.
//! - **Capture-only children** of a matched parent whose cost was `?` (an MCP server's tools and
//!   instructions), and capture-only items of non-gated kinds under any matched parent, are
//!   attached as observed children; never `seen-not-predicted`.
//! - **Content conformance** ([`conformance`]): instruction files, memory, listing entries and
//!   roster lines with an exact `predicted` must equal `observed` within
//!   [`CONTENT_TOLERANCE`] bytes, else `content-mismatch` (a resolver defect). A listing entry
//!   that arrives name-only although the resolver predicted a description is the listing's
//!   overflow, not a mismatch: it feeds the over-cap signal.
//! - **Per-run values** (git status, environment, date, hook stdout, MCP runtime text, built-in
//!   prompt) never fail; matched ones are promoted.
//! - **Promotion**: every matched static item gets `cost.observed`; the tree gets
//!   `observed_body`, the observed tool-search state, and the over-cap signal.

use crate::model::{Cost, Item, Kind, Layer, Mode, RunContext, Tree};
use crate::oracle::claude::{version_triple, NAME_ONLY};
use crate::oracle::Capture;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Bytes a reproducible contribution may differ by (a trailing newline the agent adds or drops).
pub const CONTENT_TOLERANCE: u64 = 2;

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

/// Kinds under the presence gate: statically discoverable and observable in the request.
pub fn gated(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Instruction
            | Kind::Memory
            | Kind::Skill
            | Kind::Command
            | Kind::SubAgent
            | Kind::SkillListing
            | Kind::AgentRoster
            | Kind::McpServer
            | Kind::Plugin
            | Kind::BuiltinTool
    )
}

/// Kinds whose bytes are reproducible from files in the same context.
pub fn conformance(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Instruction | Kind::Memory | Kind::Skill | Kind::Command | Kind::SubAgent
    )
}

/// One item of a tree, flattened, with the index path that reaches it.
struct Node {
    id: String,
    kind: Kind,
    layer: Layer,
    name: String,
    mode: Mode,
    cost: Cost,
    signal: Option<String>,
    path: Vec<usize>,
    parent: Option<usize>,
    /// Its own or an ancestor's mode keeps it out of the first request.
    absent: bool,
}

fn flatten(items: &[Item]) -> Vec<Node> {
    fn go(
        items: &[Item],
        prefix: &[usize],
        parent: Option<usize>,
        absent: bool,
        out: &mut Vec<Node>,
    ) {
        for (i, item) in items.iter().enumerate() {
            let mut path = prefix.to_vec();
            path.push(i);
            let absent = absent || matches!(item.mode, Mode::Runtime | Mode::None | Mode::OnDemand);
            let (layer, name) = id_parts(&item.id).unwrap_or((item.origin.layer, &item.id));
            out.push(Node {
                id: item.id.clone(),
                kind: item.kind,
                layer,
                name: name.to_string(),
                mode: item.mode,
                cost: item.cost.clone(),
                signal: item.signal.clone(),
                path: path.clone(),
                parent,
                absent,
            });
            let me = out.len() - 1;
            go(&item.children, &path, Some(me), absent, out);
        }
    }
    let mut out = Vec::new();
    go(items, &[], None, false, &mut out);
    out
}

/// `<agent>/<kind>/<layer>/<name>` → (layer, name).
fn id_parts(id: &str) -> Option<(Layer, &str)> {
    let mut parts = id.splitn(4, '/');
    let (_agent, _kind, layer, name) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let layer = Layer::ALL.iter().copied().find(|l| l.as_str() == layer)?;
    Some((layer, name))
}

fn join_class(kind: Kind) -> Kind {
    if kind == Kind::Command {
        Kind::Skill
    } else {
        kind
    }
}

fn get_mut<'a>(items: &'a mut [Item], path: &[usize]) -> &'a mut Item {
    let (first, rest) = path.split_first().expect("non-empty item path");
    let item = &mut items[*first];
    if rest.is_empty() {
        item
    } else {
        get_mut(&mut item.children, rest)
    }
}

fn get<'a>(items: &'a [Item], path: &[usize]) -> &'a Item {
    let (first, rest) = path.split_first().expect("non-empty item path");
    let item = &items[*first];
    if rest.is_empty() {
        item
    } else {
        get(&item.children, rest)
    }
}

/// Built-in tools the launch options removed: `(--disallowedTools set, --tools allow-list)`.
fn launch_tool_filter(opts: &[String]) -> (HashSet<String>, Option<HashSet<String>>) {
    let mut disallowed = HashSet::new();
    let mut allowed: Option<HashSet<String>> = None;
    let mut it = opts.iter();
    while let Some(opt) = it.next() {
        let (flag, inline) = match opt.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (opt.as_str(), None),
        };
        let into = match flag {
            "--disallowedTools" | "--disallowed-tools" => &mut disallowed,
            "--tools" => allowed.get_or_insert_with(HashSet::new),
            _ => continue,
        };
        let Some(value) = inline.or_else(|| it.next().cloned()) else {
            continue;
        };
        for name in value.split([',', ' ']).filter(|n| !n.is_empty()) {
            into.insert(name.split('(').next().unwrap_or(name).to_string());
        }
    }
    (disallowed, allowed)
}

/// Rewrites an attached capture subtree's `unresolved` layer to its static parent's.
fn relayer(item: &mut Item, layer: Layer) {
    if item.origin.layer == Layer::Unresolved && layer != Layer::Unresolved {
        if let Some((_, name)) = id_parts(&item.id) {
            item.id =
                crate::model::item_id(crate::model::Agent::ClaudeCode, item.kind, layer, name);
        }
        item.origin.layer = layer;
    }
    for child in &mut item.children {
        relayer(child, layer);
    }
}

/// Projection: Runtime/None skipped; capture-only children attach under `?` parents (never
/// SeenNotPredicted); presence gate on static ids; content conformance on reproducible static
/// items; promotes predicted→observed on matched rows; fills `Tree.observed_body` and the over-cap
/// signal. See the module docs for the rules.
pub fn verify(static_tree: &mut Tree, capture: &Capture) -> VerifyReport {
    let statics = flatten(&static_tree.items);
    let observed = flatten(&capture.tree.items);
    let mut warnings: Vec<String> = Vec::new();

    // ---- join ------------------------------------------------------------------------------
    let mut static_of: Vec<Option<usize>> = vec![None; observed.len()];
    let mut capture_of: Vec<Option<usize>> = vec![None; statics.len()];
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    let mut by_name: HashMap<(Kind, &str), Vec<usize>> = HashMap::new();
    for (si, s) in statics.iter().enumerate() {
        by_id.entry(s.id.as_str()).or_insert(si);
        by_name
            .entry((join_class(s.kind), s.name.as_str()))
            .or_default()
            .push(si);
    }
    for (ci, c) in observed.iter().enumerate() {
        if let Some(&si) = by_id.get(c.id.as_str()) {
            if capture_of[si].is_none() {
                capture_of[si] = Some(ci);
                static_of[ci] = Some(si);
            }
        }
    }
    for (ci, c) in observed.iter().enumerate() {
        if static_of[ci].is_some() {
            continue;
        }
        let path_named = matches!(c.kind, Kind::Instruction | Kind::Memory);
        let candidates = by_name
            .get(&(join_class(c.kind), c.name.as_str()))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let free = |si: &&usize| {
            let s = &statics[**si];
            capture_of[**si].is_none()
                && (path_named
                    || c.layer == s.layer
                    || c.layer == Layer::Unresolved
                    || s.layer == Layer::Unresolved)
        };
        let pick = candidates
            .iter()
            .filter(free)
            .find(|si| !statics[**si].absent)
            .or_else(|| candidates.iter().find(free))
            .copied();
        if let Some(si) = pick {
            let s = &statics[si];
            if path_named
                && s.layer != c.layer
                && s.layer != Layer::Unresolved
                && c.layer != Layer::Unresolved
            {
                warnings.push(format!(
                    "{}: layer {} in the tree, {} in the capture",
                    s.id, s.layer, c.layer
                ));
            }
            capture_of[si] = Some(ci);
            static_of[ci] = Some(si);
        }
    }

    // ---- static side: presence, conformance, promotion ---------------------------------------
    let (disallowed, allowed) = launch_tool_filter(&capture.context.launch_opts);
    let init = capture.init.as_ref();
    let mut rows: Vec<(String, RowVerdict)> = Vec::new();
    let mut promote: Vec<(Vec<usize>, u64)> = Vec::new();
    let mut dropped: Vec<Vec<usize>> = Vec::new();
    for (si, s) in statics.iter().enumerate() {
        let Some(ci) = capture_of[si] else {
            if s.absent || !gated(s.kind) {
                continue;
            }
            if !matches!(s.mode, Mode::Always | Mode::Advertised | Mode::Deferred) {
                continue;
            }
            if s.kind == Kind::BuiltinTool
                && (disallowed.contains(&s.name)
                    || allowed.as_ref().is_some_and(|a| !a.contains(&s.name)))
            {
                continue;
            }
            if s.kind == Kind::Plugin && init.is_none() {
                continue;
            }
            if s.kind == Kind::McpServer {
                let status = init
                    .and_then(|i| i.mcp_servers.iter().find(|m| m.name == s.name))
                    .map(|m| m.status.as_str());
                if let Some(status) = status.filter(|st| *st != "connected" && *st != "disabled") {
                    warnings.push(format!(
                        "{}: not connected when captured ({status}); not counted",
                        s.id
                    ));
                    continue;
                }
            }
            rows.push((s.id.clone(), RowVerdict::PredictedNotSeen));
            continue;
        };
        let c = &observed[ci];
        if s.absent {
            if gated(s.kind) {
                rows.push((s.id.clone(), RowVerdict::SeenNotPredicted));
            }
            continue;
        }
        let mut verdict = RowVerdict::Seen;
        if let (true, Some(p), Some(o)) = (conformance(s.kind), s.cost.predicted, c.cost.observed) {
            let name_only = c.signal.as_deref() == Some(NAME_ONLY);
            if s.cost.predicted_is_estimate {
                // `~` is a modelled number, not a reproducible claim.
            } else if name_only
                && matches!(s.kind, Kind::Skill | Kind::Command)
                && p > o + CONTENT_TOLERANCE
            {
                dropped.push(s.path.clone());
            } else if p.abs_diff(o) > CONTENT_TOLERANCE {
                verdict = RowVerdict::ContentMismatch {
                    predicted: p,
                    observed: o,
                };
            }
        }
        if let Some(o) = c.cost.observed {
            promote.push((s.path.clone(), o));
        }
        rows.push((s.id.clone(), verdict));
    }

    // ---- capture side: seen-not-predicted, attachment ------------------------------------------
    let mut covered = vec![false; observed.len()];
    let mut attach: Vec<(Vec<usize>, Item)> = Vec::new();
    for (ci, c) in observed.iter().enumerate() {
        if static_of[ci].is_some() {
            continue;
        }
        if c.parent.is_some_and(|p| covered[p]) {
            covered[ci] = true;
            continue;
        }
        match c.parent.and_then(|p| static_of[p]) {
            Some(sp) => {
                let parent = &statics[sp];
                let unknown = parent.cost.predicted.is_none() && !parent.cost.not_applicable;
                if unknown || !gated(c.kind) {
                    let mut item = get(&capture.tree.items, &c.path).clone();
                    relayer(&mut item, parent.layer);
                    attach.push((parent.path.clone(), item));
                    covered[ci] = true;
                } else {
                    rows.push((c.id.clone(), RowVerdict::SeenNotPredicted));
                }
            }
            None if gated(c.kind) => rows.push((c.id.clone(), RowVerdict::SeenNotPredicted)),
            // A per-run or built-in part the tree does not model: not part of the gate.
            None => {}
        }
    }

    // ---- apply ---------------------------------------------------------------------------------
    let promoted = promote.len() as u32;
    for (path, bytes) in promote {
        get_mut(&mut static_tree.items, &path).cost.observed = Some(bytes);
    }
    let listing = observed.iter().position(|c| c.kind == Kind::SkillListing);
    if let (false, Some(li)) = (dropped.is_empty(), listing) {
        let total = observed.iter().filter(|c| c.parent == Some(li)).count();
        let n = dropped.len();
        static_tree.signals.push(format!(
            "skill listing: OVER CAP — {n} of {total} descriptions dropped (observed)"
        ));
        if let Some(sp) = static_of[li] {
            get_mut(&mut static_tree.items, &statics[sp].path).signal = Some(format!(
                "over cap: {n} of {total} descriptions dropped (observed)"
            ));
        }
        for path in &dropped {
            get_mut(&mut static_tree.items, path).signal =
                Some("description dropped from the listing (over cap, observed)".to_string());
        }
    }
    for (path, item) in attach {
        get_mut(&mut static_tree.items, &path).children.push(item);
    }
    static_tree.observed_body = Some(capture.body_bytes);
    match (static_tree.tool_search, capture.tree.tool_search) {
        (None, observed) => static_tree.tool_search = observed,
        (Some(a), Some(b)) if a != b => warnings.push(format!(
            "tool search: predicted {}, observed {}",
            if a { "on" } else { "off" },
            if b { "on" } else { "off" }
        )),
        _ => {}
    }

    // ---- context -------------------------------------------------------------------------------
    let mut context_ok = true;
    if static_tree.target_dir != capture.context.cwd && static_tree.target_dir != PathBuf::new() {
        context_ok = false;
        warnings.push(format!(
            "capture bound to {}, tree resolved for {}",
            capture.context.cwd.display(),
            static_tree.target_dir.display()
        ));
    }
    if let (Some(tree_v), Some(capture_v)) =
        (&static_tree.agent_version, &capture.context.agent_version)
    {
        if version_triple(tree_v) != version_triple(capture_v) {
            warnings.push(format!(
                "capture from Claude Code {capture_v}, tree resolved for {tree_v}"
            ));
        }
    }
    static_tree.warnings.extend(warnings);

    let passed = context_ok && rows.iter().all(|(_, v)| *v == RowVerdict::Seen);
    VerifyReport {
        context: capture.context.clone(),
        rows,
        passed,
        promoted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_filter_reads_both_spellings() {
        let opts: Vec<String> = [
            "-p",
            "hi",
            "--disallowedTools",
            "Edit,Bash(git:*)",
            "--tools=Read Grep",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (disallowed, allowed) = launch_tool_filter(&opts);
        assert!(disallowed.contains("Edit") && disallowed.contains("Bash"));
        let allowed = allowed.unwrap();
        assert!(allowed.contains("Read") && allowed.contains("Grep") && !allowed.contains("Edit"));
    }

    #[test]
    fn id_parts_keep_slashes_in_the_name() {
        assert_eq!(
            id_parts("claude-code/instruction/project//r/CLAUDE.md"),
            Some((Layer::Project, "/r/CLAUDE.md"))
        );
        assert_eq!(
            id_parts("claude-code/mcp-tool/plugin/plugin:x:x/t"),
            Some((Layer::Plugin, "plugin:x:x/t"))
        );
        assert_eq!(id_parts("nope"), None);
    }
}
