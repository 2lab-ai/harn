//! Codex: static tree vs a [`CodexCapture`] (P0.5, acceptance A7–A8). The oracle is codex's own
//! inspection output ([`crate::oracle::codex`]), so the projection is what those commands show.
//!
//! - **Joined** by kind + name, the layer ignored (the capture names no layer): blocks by their
//!   [`BLOCKS`](crate::oracle::codex::BLOCKS) name, listing entries by skill name (plugin skills
//!   keep their `plugin:` prefix), MCP servers and plugins by name; the observed
//!   `developer_instructions` joins the row named `<file>#/developer_instructions`.
//! - **Presence gate** ([`gated`]): instruction rows (the AGENTS.md message, each AGENTS file,
//!   `developer_instructions`), the skill listing and its entries, the blocks, the environment
//!   block, MCP servers (`codex mcp list --json`, enabled ones), plugins (`codex plugin list
//!   --json`, installed and enabled ones; a plugin it does not list — its marketplace is not
//!   configured — counts as seen when one of its skills is in the listing, else not checkable).
//!   Rows whose mode (or an ancestor's) is `runtime`, `none` or `on-demand` are expected absent; a
//!   plugin row's mode is its load state. Blocks in [`RUNTIME_DECIDED`] that did not render are
//!   a warning, not a diff.
//! - **AGENTS files** carry no header in the message, so the capture cannot name them: harn
//!   assembles the message it predicts from the rows (the files read now, cut to their predicted
//!   bytes, the framing codex adds) and compares it byte for byte with the observed one. A file is
//!   seen when its text is where predicted, or later in the message; the row holding the first
//!   differing byte is `content-mismatch`, and the report names that offset.
//! - **Content conformance** (exact predictions only, never `~` or `?`): the AGENTS.md message as
//!   above; each listing entry's line (text compared when the row carries its `rendered:` line,
//!   else bytes); the `<skills_instructions>` framing and every other block with an exact
//!   prediction by bytes — tolerance 0.
//! - **Not checkable** (rows of their own, never a failure): hooks, rules, tools and the base
//!   instructions — see [`OUTSIDE_THE_DUMP`], which every report states.
//! - **Promotion**: matched rows get `cost.observed`; `?` blocks become observed. The input list is
//!   not the request (no instructions, no tools), so `observed_body` stays unset; a signal names
//!   the observed input bytes instead.

use super::{flatten, get, get_mut, id_parts, Node, RowVerdict, VerifyReport};
use crate::model::{item_id, Agent, Item, Kind, Layer, Mode, Tree};
use crate::oracle::codex::{names, CodexCapture};
use std::collections::HashMap;
use std::path::Path;

/// What codex's inspection commands do not show (acceptance A8); stated in every Codex report.
pub const OUTSIDE_THE_DUMP: &str = "outside codex's dump, not verified: tools[] (built-in and MCP tool schemas), the base instructions sent as the request's `instructions` field (`codex debug models` has the catalog template, not the text sent), and hook output (SessionStart / UserPromptSubmit run inside run_turn, which `codex debug prompt-input` does not enter: core/src/prompt_debug.rs:89)";

/// Blocks codex renders or not at session start from processes, login or network rather than
/// files: absent in a run = a warning, not a diff.
pub const RUNTIME_DECIDED: &[(&str, &str)] = &[
    (
        "apps_instructions",
        "rendered only when apps are on for the login and a connector is reachable through the running MCP servers (core/src/session/world_state.rs:255-270)",
    ),
    (
        "recommended_plugins",
        "rendered from the remote plugin catalog for the login (core/src/session/mod.rs:4330-4351, :4484-4489)",
    ),
];

/// The AGENTS.md message framing (`core/src/context/user_instructions.rs:23-34`).
const AGENTS_HEADER: &str = "# AGENTS.md instructions";
const AGENTS_OPEN: &str = "\n\n<INSTRUCTIONS>\n";
const AGENTS_CLOSE: &str = "\n</INSTRUCTIONS>";
/// Between the global file and the project files (`core/src/agents_md.rs:375-405`).
const AGENTS_PROJECT_SEPARATOR: &str = "\n\n--- project-doc ---\n\n";
const AGENTS_JOIN: &str = "\n\n";
/// The `origin.why` prefix under which the resolver records a listing entry's rendered line.
const RENDERED_PREFIX: &str = "rendered: ";
const DEVELOPER_INSTRUCTIONS_SUFFIX: &str = "#/developer_instructions";

/// Kinds under the Codex presence gate.
pub fn gated(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Instruction
            | Kind::SkillListing
            | Kind::Skill
            | Kind::SystemPrompt
            | Kind::EnvBlock
            | Kind::McpServer
            | Kind::Plugin
    )
}

/// Why a row in the projection cannot be checked against codex's output, or None.
fn not_checkable(node: &Node) -> Option<&'static str> {
    match node.kind {
        Kind::Hook => Some("hook output is runtime: `codex debug prompt-input` does not run hooks (core/src/prompt_debug.rs:89)"),
        Kind::PermissionRule => Some("its allow prefixes render inside <permissions instructions>; harn predicts neither their text nor their bytes"),
        Kind::BuiltinTool | Kind::McpTool | Kind::McpInstructions => Some("tools[] is not in `codex debug prompt-input`"),
        Kind::SystemPrompt if node.name == names::BASE_INSTRUCTIONS => Some("the request's `instructions` field is not in `codex debug prompt-input`"),
        _ => None,
    }
}

/// The join name of a node: `developer_instructions` for the row named by its config file.
fn join_name(node: &Node) -> &str {
    if node.kind == Kind::Instruction && node.name.ends_with(DEVELOPER_INSTRUCTIONS_SUFFIX) {
        names::DEVELOPER_INSTRUCTIONS
    } else {
        &node.name
    }
}

/// Absent from the first request: by mode (an ancestor's included), a plugin row by its own
/// mode too (it is a load switch: `always` while loaded, `none` otherwise).
fn absent(node: &Node) -> bool {
    node.absent
        || (node.kind == Kind::Plugin
            && matches!(node.mode, Mode::Runtime | Mode::None | Mode::OnDemand))
}

fn exact(node: &Node) -> Option<u64> {
    (!node.cost.predicted_is_estimate)
        .then_some(node.cost.predicted)
        .flatten()
}

/// Length of the common prefix of `a` and `b`, in bytes.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

/// Up to 24 bytes of `text` from `at`, debug-escaped, on char boundaries.
fn excerpt(text: &str, at: usize) -> String {
    let mut start = at.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + 24).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{:?}", &text[start..end])
}

/// Rows and notes of one verification, built in order.
struct Out {
    rows: Vec<(String, RowVerdict)>,
    notes: Vec<String>,
    warnings: Vec<String>,
    promote: Vec<(Vec<usize>, u64)>,
}

/// Static tree vs a Codex capture; see the module docs.
pub fn verify_codex(static_tree: &mut Tree, capture: &CodexCapture) -> VerifyReport {
    let statics = flatten(&static_tree.items);
    let observed = flatten(&capture.tree.items);
    let mut out = Out {
        rows: Vec::new(),
        notes: vec![OUTSIDE_THE_DUMP.to_string()],
        warnings: Vec::new(),
        promote: Vec::new(),
    };

    // ---- join (AGENTS.md message and its files are handled apart) ----------------------------
    let agents_msg = statics
        .iter()
        .position(|s| s.kind == Kind::Instruction && s.name == names::AGENTS_MD);
    let in_agents =
        |si: usize| agents_msg.is_some_and(|m| si == m || statics[si].parent == Some(m));
    let mut by_name: HashMap<(Kind, &str), Vec<usize>> = HashMap::new();
    for (ci, c) in observed.iter().enumerate() {
        by_name
            .entry((c.kind, c.name.as_str()))
            .or_default()
            .push(ci);
    }
    let mut static_of: Vec<Option<usize>> = vec![None; observed.len()];
    let mut capture_of: Vec<Option<usize>> = vec![None; statics.len()];
    for (si, s) in statics.iter().enumerate() {
        if in_agents(si) || not_checkable(s).is_some() {
            continue;
        }
        let candidates = by_name
            .get(&(s.kind, join_name(s)))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if let Some(&ci) = candidates.iter().find(|&&ci| static_of[ci].is_none()) {
            static_of[ci] = Some(si);
            capture_of[si] = Some(ci);
        }
    }

    // ---- static side ---------------------------------------------------------------------------
    let listed_skills: Vec<&str> = capture.skills.iter().map(|e| e.name.as_str()).collect();
    let mut trimmed = 0usize;
    for (si, s) in statics.iter().enumerate() {
        if in_agents(si) {
            continue;
        }
        if absent(s) {
            if capture_of[si].is_some() && gated(s.kind) {
                out.rows.push((s.id.clone(), RowVerdict::SeenNotPredicted));
            }
            continue;
        }
        if let Some(reason) = not_checkable(s) {
            out.rows.push((
                s.id.clone(),
                RowVerdict::NotCheckable {
                    reason: reason.to_string(),
                },
            ));
            continue;
        }
        if !gated(s.kind) {
            continue;
        }
        let Some(ci) = capture_of[si] else {
            out.rows
                .push((s.id.clone(), unmatched(s, capture, &listed_skills)));
            continue;
        };
        let c = &observed[ci];
        let mut verdict = RowVerdict::Seen;
        if let (Some(p), Some(o)) = (exact(s), c.cost.observed) {
            let lines = (s.kind == Kind::Skill)
                .then(|| {
                    rendered_line(static_tree, s)
                        .zip(capture.skills.iter().find(|e| e.name == c.name))
                })
                .flatten();
            let differs = match lines {
                Some((predicted, listed)) => predicted != listed.line,
                None => s.kind != Kind::SkillListing && p != o,
            };
            if differs {
                verdict = RowVerdict::ContentMismatch {
                    predicted: p,
                    observed: o,
                };
                out.notes.push(match lines {
                    Some((predicted, listed)) => {
                        let at = common_prefix(predicted.as_bytes(), listed.line.as_bytes());
                        format!(
                            "{}: listing line differs from byte {at}: predicted {}, observed {}",
                            s.id,
                            excerpt(predicted, at),
                            excerpt(&listed.line, at)
                        )
                    }
                    None => format!(
                        "{}: predicted {p} B, observed {o} B; harn predicts its size, not its text, so the differing offset is unknown",
                        s.id
                    ),
                });
            }
        }
        if s.kind == Kind::Skill
            && s.cost.predicted_is_estimate
            && s.cost
                .predicted
                .zip(c.cost.observed)
                .is_some_and(|(p, o)| o < p)
        {
            trimmed += 1;
        }
        if let Some(o) = c.cost.observed {
            out.promote.push((s.path.clone(), o));
        }
        out.rows.push((s.id.clone(), verdict));
    }

    // ---- AGENTS.md message ---------------------------------------------------------------------
    agents_md(static_tree, &statics, agents_msg, capture, &mut out);

    // ---- capture side --------------------------------------------------------------------------
    let mut attach: Vec<(Vec<usize>, Item)> = Vec::new();
    for (ci, c) in observed.iter().enumerate() {
        if static_of[ci].is_some() || c.name == names::AGENTS_MD {
            continue;
        }
        match c.parent.and_then(|p| static_of[p]) {
            Some(sp)
                if statics[sp].cost.predicted.is_none() && !statics[sp].cost.not_applicable =>
            {
                let mut item = get(&capture.tree.items, &c.path).clone();
                relayer(&mut item, statics[sp].layer);
                attach.push((statics[sp].path.clone(), item));
            }
            _ if gated(c.kind) => out.rows.push((c.id.clone(), RowVerdict::SeenNotPredicted)),
            _ => {}
        }
    }
    for server in capture.mcp_servers.iter().flatten().filter(|s| !s.enabled) {
        let known = statics
            .iter()
            .any(|s| s.kind == Kind::McpServer && s.name == server.name);
        if !known {
            out.warnings.push(format!(
                "codex mcp list names the disabled server {}; the tree has no row for it",
                server.name
            ));
        }
    }

    // ---- apply ---------------------------------------------------------------------------------
    let promoted = out.promote.len() as u32;
    for (path, bytes) in out.promote {
        get_mut(&mut static_tree.items, &path).cost.observed = Some(bytes);
    }
    for (path, item) in attach {
        get_mut(&mut static_tree.items, &path).children.push(item);
    }
    if trimmed > 0 {
        static_tree.signals.push(format!(
            "skill listing: {trimmed} of {} descriptions trimmed by codex (over its budget, observed)",
            capture.skills.len()
        ));
    }
    static_tree.signals.push(format!(
        "codex debug prompt-input ({} {}): input list observed {} B in {} blocks; instructions, tools and hook output are not in it",
        capture.context.capture_kind,
        capture.context.captured_at,
        capture.input_bytes,
        capture.blocks.len()
    ));

    // ---- context -------------------------------------------------------------------------------
    let mut context_ok = true;
    if static_tree.target_dir != capture.context.cwd {
        context_ok = false;
        out.warnings.push(format!(
            "capture bound to {}, tree resolved for {}",
            capture.context.cwd.display(),
            static_tree.target_dir.display()
        ));
    }
    if let (Some(tree_v), Some(capture_v)) =
        (&static_tree.agent_version, &capture.context.agent_version)
    {
        if tree_v != capture_v {
            out.warnings.push(format!(
                "capture from codex {capture_v}, tree resolved for {tree_v}"
            ));
        }
    }
    out.warnings.extend(capture.warnings.iter().cloned());
    out.notes
        .extend(out.warnings.iter().map(|w| format!("warning: {w}")));
    static_tree.warnings.extend(out.warnings);

    let passed = context_ok && out.rows.iter().all(|(_, v)| v.passes());
    VerifyReport {
        context: capture.context.clone(),
        rows: out.rows,
        passed,
        promoted,
        notes: out.notes,
    }
}

/// The verdict of a gated row codex did not show.
fn unmatched(s: &Node, capture: &CodexCapture, listed_skills: &[&str]) -> RowVerdict {
    if let Some((_, why)) = RUNTIME_DECIDED.iter().find(|(name, _)| *name == s.name) {
        return RowVerdict::NotCheckable {
            reason: format!("not rendered in this run: {why}"),
        };
    }
    match s.kind {
        Kind::McpServer => match &capture.mcp_servers {
            None => RowVerdict::NotCheckable {
                reason: "`codex mcp list --json` gave no list in this capture".to_string(),
            },
            Some(_) => RowVerdict::PredictedNotSeen,
        },
        Kind::Plugin => {
            let listed = capture.plugins.iter().flatten().any(|p| p.id == s.name);
            let prefix = format!("{}:", s.name.split('@').next().unwrap_or(&s.name));
            if listed {
                // Listed, but not enabled.
                RowVerdict::PredictedNotSeen
            } else if listed_skills.iter().any(|n| n.starts_with(&prefix)) {
                RowVerdict::Seen
            } else {
                RowVerdict::NotCheckable {
                    reason: "not in `codex plugin list` (it lists configured marketplaces only) and none of its skills is in the listing".to_string(),
                }
            }
        }
        _ => RowVerdict::PredictedNotSeen,
    }
}

/// The listing line the resolver rendered for a skill row (its `rendered: ` why entry).
fn rendered_line<'a>(tree: &'a Tree, node: &Node) -> Option<&'a str> {
    get(&tree.items, &node.path)
        .origin
        .why
        .iter()
        .find_map(|w| w.strip_prefix(RENDERED_PREFIX))
}

/// One span of the predicted AGENTS.md message: the static row it belongs to (None = framing).
struct Span {
    row: Option<usize>,
    start: usize,
    end: usize,
}

/// The AGENTS.md message: presence of the message and of each file, and the byte-for-byte check.
fn agents_md(
    tree: &Tree,
    statics: &[Node],
    msg: Option<usize>,
    capture: &CodexCapture,
    out: &mut Out,
) {
    let observed = capture.block(names::AGENTS_MD);
    let observed_id = item_id(
        Agent::Codex,
        Kind::Instruction,
        Layer::Builtin,
        names::AGENTS_MD,
    );
    let msg = msg.filter(|&m| !absent(&statics[m]));
    let (m, o) = match (msg, observed) {
        (None, None) => return,
        (None, Some(_)) => {
            out.rows.push((observed_id, RowVerdict::SeenNotPredicted));
            return;
        }
        (Some(m), None) => {
            out.rows
                .push((statics[m].id.clone(), RowVerdict::PredictedNotSeen));
            for (si, s) in statics.iter().enumerate() {
                if s.parent == Some(m) && !absent(s) {
                    out.rows
                        .push((statics[si].id.clone(), RowVerdict::PredictedNotSeen));
                }
            }
            return;
        }
        (Some(m), Some(o)) => (m, o),
    };
    let files: Vec<usize> = (0..statics.len())
        .filter(|&si| statics[si].parent == Some(m) && !absent(&statics[si]))
        .collect();

    // The predicted message, span by span: global file first, then the project chain.
    let global: Vec<usize> = files
        .iter()
        .copied()
        .filter(|&f| statics[f].layer == Layer::User)
        .collect();
    let project: Vec<usize> = files
        .iter()
        .copied()
        .filter(|&f| statics[f].layer != Layer::User)
        .collect();
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |row: Option<usize>, part: &str| {
        let start = text.len();
        text.push_str(part);
        spans.push(Span {
            row,
            start,
            end: text.len(),
        });
    };
    let mut header = AGENTS_HEADER.to_string();
    if !project.is_empty() {
        header.push_str(&format!(" for {}", tree.target_dir.display()));
    }
    header.push_str(AGENTS_OPEN);
    push(None, &header);
    let mut unreadable = Vec::new();
    for (k, &f) in global.iter().chain(project.iter()).enumerate() {
        if k > 0 {
            let first_project = k == global.len() && !global.is_empty();
            push(
                None,
                if first_project {
                    AGENTS_PROJECT_SEPARATOR
                } else {
                    AGENTS_JOIN
                },
            );
        }
        let part = predicted_file_text(tree, &statics[f]).unwrap_or_else(|| {
            unreadable.push(statics[f].id.clone());
            String::new()
        });
        push(Some(f), &part);
    }
    push(None, AGENTS_CLOSE);
    if !unreadable.is_empty() {
        out.notes.push(format!(
            "AGENTS.md message: {} could not be read now; compared as empty",
            unreadable.join(", ")
        ));
    }
    let tree_sum: u64 = statics[m].cost.predicted.unwrap_or(0)
        + files
            .iter()
            .filter_map(|&f| statics[f].cost.predicted)
            .sum::<u64>();
    if tree_sum != text.len() as u64 {
        out.notes.push(format!(
            "{}: the message harn assembles from the files now is {} B, the tree's rows sum to {tree_sum} B (a file changed since the tree was resolved?)",
            statics[m].id,
            text.len()
        ));
    }

    // Compare: `d` = the first differing byte; the span holding it decides whose mismatch it is.
    let observed_text = o.text.as_str();
    let equal = text == observed_text;
    let d = common_prefix(text.as_bytes(), observed_text.as_bytes());
    let delta = observed_text.len() as i64 - text.len() as i64;
    let holder = (!equal)
        .then(|| spans.iter().position(|s| s.start <= d && d < s.end))
        .flatten();
    // A framing span holds it, or the observed message only adds bytes at the end.
    let mut framing_mismatch = !equal && holder.is_none_or(|h| spans[h].row.is_none());
    let mut cursor = 0usize;
    let mut files_observed = 0u64;
    for (k, span) in spans.iter().enumerate() {
        let Some(f) = span.row else {
            continue;
        };
        let part = &text[span.start..span.end];
        let len = part.len() as u64;
        let id = statics[f].id.clone();
        if equal || span.end <= d {
            out.rows.push((id, RowVerdict::Seen));
            out.promote.push((statics[f].path.clone(), len));
            files_observed += len;
            cursor = span.end;
        } else if holder == Some(k) && span.start < d {
            // Its first bytes are where predicted: present, but different.
            let obs = (len as i64 + delta).max(0) as u64;
            out.rows.push((
                id.clone(),
                RowVerdict::ContentMismatch {
                    predicted: len,
                    observed: obs,
                },
            ));
            out.notes.push(format!(
                "{id}: first differing byte at offset {} of the file's text ({d} of the AGENTS.md message): predicted {}, observed {}",
                d - span.start,
                excerpt(&text, d),
                excerpt(observed_text, d)
            ));
            out.promote.push((statics[f].path.clone(), obs));
            files_observed += obs;
            cursor = d;
        } else {
            // At or after the first difference: present when its text comes later.
            let found = (!part.is_empty())
                .then(|| observed_text.get(cursor..).and_then(|rest| rest.find(part)))
                .flatten();
            match found {
                Some(at) => {
                    if holder == Some(k) {
                        // Bytes harn did not predict come before it.
                        framing_mismatch = true;
                    }
                    out.rows.push((id, RowVerdict::Seen));
                    out.promote.push((statics[f].path.clone(), len));
                    files_observed += len;
                    cursor += at + part.len();
                }
                None => out.rows.push((id, RowVerdict::PredictedNotSeen)),
            }
        }
    }
    let framing_observed = (observed_text.len() as u64).saturating_sub(files_observed);
    out.promote
        .push((statics[m].path.clone(), framing_observed));
    if framing_mismatch {
        out.rows.push((
            statics[m].id.clone(),
            RowVerdict::ContentMismatch {
                predicted: statics[m].cost.predicted.unwrap_or(0),
                observed: framing_observed,
            },
        ));
        out.notes.push(format!(
            "{}: first differing byte at offset {d} of the AGENTS.md message (its framing, or text harn did not predict): predicted {}, observed {}",
            statics[m].id,
            excerpt(&text, d),
            excerpt(observed_text, d)
        ));
    } else {
        out.rows.push((statics[m].id.clone(), RowVerdict::Seen));
    }
}

/// The text a file row predicts: the file read now, decoded lossily like codex
/// (`codex-home/src/instructions/mod.rs:68-79`, `core/src/agents_md.rs:139-187`), trimmed for the
/// global file, cut to the row's predicted bytes for a project file over the budget.
fn predicted_file_text(tree: &Tree, node: &Node) -> Option<String> {
    let path: &Path = get(&tree.items, &node.path).origin.path.as_deref()?;
    let data = std::fs::read(path).ok()?;
    let whole = String::from_utf8_lossy(&data);
    if node.layer == Layer::User {
        return Some(whole.trim().to_string());
    }
    let predicted = node.cost.predicted? as usize;
    if whole.len() == predicted {
        return Some(whole.into_owned());
    }
    Some(String::from_utf8_lossy(&data[..predicted.min(data.len())]).into_owned())
}

/// Rewrites an attached capture subtree's `unresolved` layer to its static parent's.
fn relayer(item: &mut Item, layer: Layer) {
    if item.origin.layer == Layer::Unresolved && layer != Layer::Unresolved {
        if let Some((_, name)) = id_parts(&item.id) {
            item.id = item_id(Agent::Codex, item.kind, layer, name);
        }
        item.origin.layer = layer;
    }
    for child in &mut item.children {
        relayer(child, layer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_stays_on_char_boundaries() {
        let text = "ab한글cd";
        assert_eq!(excerpt(text, 3), "\"한글cd\"");
        assert_eq!(excerpt(text, 100), "\"\"");
    }

    #[test]
    fn common_prefix_counts_bytes() {
        assert_eq!(common_prefix(b"abc", b"abd"), 2);
        assert_eq!(common_prefix(b"abc", b"abc"), 3);
        assert_eq!(common_prefix(b"", b"x"), 0);
    }
}
