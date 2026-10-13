//! ActionPlan builder (WU-E): knob candidates per item by scope, quarantine only when no knob
//! exists and only in the same scope, `freed = predicted(before) − predicted(after)` via a second
//! resolve with an [`Overlay`](crate::model::Overlay), side effects and warnings.

use crate::model::{
    self, Agent, Cost, Freed, Item, Kind, Knob, Overlay, OverlayOp, Refused, Scope, SelectError,
    Target, Tree,
};
use crate::resolve::Resolver;
use crate::writers;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedOp {
    pub item_id: String,
    pub knob: Knob,
    /// The file the op writes (`Env`: the scope's settings file; `Quarantine`: `from`).
    pub file: Option<PathBuf>,
    /// `writers::hash_path` of `file` when the plan was built; None = nothing there.
    pub hash_before: Option<String>,
    /// The operator's reason, from a `harn-plan/1` request.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionPlan {
    pub target_dir: PathBuf,
    pub agent: Agent,
    pub ops: Vec<PlannedOp>,
    pub predicted_before: u64,
    pub predicted_after: u64,
    pub freed: Freed,
    pub side_effects: Vec<String>,
    pub warnings: Vec<String>,
    /// Unified diff, all files.
    pub diff: String,
}

/// Plans turning `item_id` off. `scope` None = the default (`local`); a scope with no knob is
/// refused (`model::Refused`), never widened.
///
/// Knob choice: the knob in exactly the requested scope, else the narrowest knob below it;
/// only wider knobs → refused, naming the scope to pass.
pub fn build_plan(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &Tree,
    item_id: &str,
    scope: Option<Scope>,
) -> anyhow::Result<ActionPlan> {
    let item = model::walk(tree)
        .find(|item| item.id == item_id)
        .ok_or_else(|| SelectError::NotFound(item_id.to_string()))?;
    let knob = choose_knob(item, scope)?;
    build(
        resolver,
        target,
        tree,
        &[Pick {
            item,
            knob,
            reason: None,
        }],
    )
}

/// Schema of an AI-authored plan file (`harn apply <plan.json>`).
pub const PLAN_SCHEMA: &str = "harn-plan/1";

/// AI-authored plan: `{"schema","target_dir","ops":[{"selector","scope"?,"knob"?,"reason"}]}`.
/// Unknown keys are rejected so a misspelt `knob`/`scope` cannot fall back to a default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    /// [`PLAN_SCHEMA`].
    pub schema: String,
    pub target_dir: PathBuf,
    pub ops: Vec<PlanOp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOp {
    pub selector: String,
    /// `local` | `project` | `user`; None = default (`local`).
    pub scope: Option<Scope>,
    /// Index into `Item.knobs`; None = `knobs[0]`.
    pub knob: Option<usize>,
    pub reason: String,
}

/// Every selector unique (else Err listing candidates), knob index within `item.knobs` and not
/// `Knob::None`, scope never wider than requested; reasons kept in `PlannedOp.reason`.
/// All ops become one overlay and one plan (before/after resolved once).
pub fn build_plan_from_request(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &Tree,
    req: &PlanRequest,
) -> anyhow::Result<ActionPlan> {
    if req.schema != PLAN_SCHEMA {
        return Err(Refused(format!(
            "plan schema is {:?}, expected \"{PLAN_SCHEMA}\"",
            req.schema
        ))
        .into());
    }
    let plan_dir = std::fs::canonicalize(&req.target_dir)
        .map_err(|e| Refused(format!("plan target_dir {}: {e}", req.target_dir.display())))?;
    if plan_dir != target.dir {
        return Err(Refused(format!(
            "plan is for {} but the target is {}",
            plan_dir.display(),
            target.dir.display()
        ))
        .into());
    }
    if req.ops.is_empty() {
        return Err(Refused("plan has no ops".to_string()).into());
    }
    let mut picks: Vec<Pick> = Vec::new();
    for (i, op) in req.ops.iter().enumerate() {
        let n = i + 1;
        if op.reason.trim().is_empty() {
            return Err(Refused(format!("op {n} ({}): reason is empty", op.selector)).into());
        }
        let item = model::select(tree, &op.selector).map_err(|e| {
            anyhow::Error::new(e).context(format!("op {n}: selector `{}`", op.selector))
        })?;
        let knob = match op.knob {
            None => {
                choose_knob(item, op.scope).with_context(|| format!("op {n} ({})", op.selector))?
            }
            Some(k) => {
                let knob = item.knobs.get(k).ok_or_else(|| {
                    Refused(format!(
                        "op {n}: knob {k} is out of range: {} has {} knob(s)",
                        item.id,
                        item.knobs.len()
                    ))
                })?;
                let wanted = op.scope.unwrap_or(Scope::Local);
                match (knob, knob_scope(knob)) {
                    (Knob::None { reason }, _) => {
                        return Err(Refused(format!(
                            "op {n}: knob {k} of {} is none: {reason}",
                            item.id
                        ))
                        .into())
                    }
                    (_, Some(scope)) if scope > wanted => {
                        return Err(Refused(format!(
                            "op {n}: knob {k} of {} writes {scope} scope, wider than {wanted}; \
                             set \"scope\": \"{scope}\" to widen",
                            item.id
                        ))
                        .into())
                    }
                    _ => knob,
                }
            }
        };
        if let Some(m) = picks.iter().position(|p| p.item.id == item.id) {
            return Err(Refused(format!("op {n}: {} is already op {}", item.id, m + 1)).into());
        }
        picks.push(Pick {
            item,
            knob,
            reason: Some(op.reason.clone()),
        });
    }
    build(resolver, target, tree, &picks)
}

/// Fills `Item.freed` for `knobs[0]` of every item via an overlay re-resolve.
///
/// Only the local-scope default is annotated (an item whose knobs are all wider keeps `None`).
/// Re-resolves are memoized by overlay, so a plugin's rows share one. Over
/// [`ANNOTATE_CAP`] candidates, only top-level items and the first [`ANNOTATE_CHILDREN_CAP`]
/// children of each are annotated and the tree says so. Spawns no process.
pub fn annotate_freed(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &mut Tree,
) -> anyhow::Result<()> {
    let mut candidates: Vec<(Vec<usize>, Knob)> = Vec::new();
    collect_candidates(&tree.items, &mut Vec::new(), &mut candidates);
    let total = candidates.len();
    let capped = total > ANNOTATE_CAP;
    if capped {
        let mut per_parent: HashMap<usize, usize> = HashMap::new();
        candidates.retain(|(path, _)| match path.as_slice() {
            [_] => true,
            [parent, _] => {
                let seen = per_parent.entry(*parent).or_default();
                *seen += 1;
                *seen <= ANNOTATE_CHILDREN_CAP
            }
            _ => false,
        });
    }

    let mut results: Vec<(Vec<usize>, Result<Freed, String>)> = Vec::new();
    {
        let before = Index::of(tree);
        let mut memo: HashMap<String, Result<Freed, String>> = HashMap::new();
        for (path, knob) in candidates {
            let overlay = Overlay {
                ops: overlay_op(target, &knob).into_iter().collect(),
            };
            let key = serde_json::to_string(&overlay)?;
            let freed = memo
                .entry(key)
                .or_insert_with(|| {
                    resolver
                        .resolve(target, Some(&overlay))
                        .map(|after| {
                            Delta::between(&before, &Index::of(&after)).freed(tree, &after)
                        })
                        .map_err(|e| format!("{e:#}"))
                })
                .clone();
            results.push((path, freed));
        }
    }

    let annotated = results.len();
    let mut failed = 0;
    let mut first_error = None;
    for (path, freed) in results {
        match freed {
            Ok(freed) => {
                if let Some(item) = item_at_mut(&mut tree.items, &path) {
                    item.freed = Some(freed);
                }
            }
            Err(err) => {
                failed += 1;
                first_error.get_or_insert(err);
            }
        }
    }
    if let Some(err) = first_error {
        tree.warnings.push(format!(
            "freed not computed for {failed} item(s); first error: {err}"
        ));
    }
    if capped {
        tree.warnings.push(format!(
            "freed computed for {annotated} of {total} items (over {ANNOTATE_CAP}: top-level \
             items and the first {ANNOTATE_CHILDREN_CAP} children of each); `harn why <item>` \
             computes any other"
        ));
    }
    Ok(())
}

/// Above this many candidate items, [`annotate_freed`] annotates a bounded subset.
pub const ANNOTATE_CAP: usize = 400;
/// Children annotated per top-level item when [`ANNOTATE_CAP`] is exceeded.
pub const ANNOTATE_CHILDREN_CAP: usize = 50;

/// `/env/<NAME>` (escaped), where a `Knob::Env` lands in its scope's settings file.
pub fn env_pointer(name: &str) -> String {
    format!("/env/{}", writers::escape_token(name))
}

/// The settings file of a scope: `local` = `<dir>/.claude/settings.local.json`, `project` =
/// `<dir>/.claude/settings.json`, `user` = `<home>/.claude/settings.json`.
pub fn settings_file(target: &Target, scope: Scope) -> PathBuf {
    match scope {
        Scope::Local => target.dir.join(".claude/settings.local.json"),
        Scope::Project => target.dir.join(".claude/settings.json"),
        Scope::User => target.home.join(".claude/settings.json"),
    }
}

/// The scope a knob writes; None for `Knob::None`.
pub fn knob_scope(knob: &Knob) -> Option<Scope> {
    match knob {
        Knob::Setting { scope, .. } | Knob::Env { scope, .. } | Knob::Quarantine { scope, .. } => {
            Some(*scope)
        }
        Knob::None { .. } => None,
    }
}

/// The knob `scope` selects: exact scope first, else the narrowest knob narrower than it.
fn choose_knob(item: &Item, scope: Option<Scope>) -> anyhow::Result<&Knob> {
    let wanted = scope.unwrap_or(Scope::Local);
    let usable: Vec<(&Knob, Scope)> = item
        .knobs
        .iter()
        .filter_map(|k| knob_scope(k).map(|s| (k, s)))
        .collect();
    if usable.is_empty() {
        let reason = item
            .knobs
            .iter()
            .find_map(|k| match k {
                Knob::None { reason } => Some(reason.as_str()),
                _ => None,
            })
            .unwrap_or("no knob");
        return Err(Refused(format!("{}: {reason}", item.id)).into());
    }
    if let Some((knob, _)) = usable.iter().find(|(_, s)| *s == wanted) {
        return Ok(knob);
    }
    if let Some((knob, _)) = usable.iter().find(|(_, s)| *s < wanted) {
        return Ok(knob);
    }
    let nearest = usable
        .iter()
        .map(|(_, s)| *s)
        .min()
        .expect("usable is not empty");
    Err(match scope {
        None => Refused(format!(
            "{}: scope would widen to {nearest}; pass --scope {nearest}",
            item.id
        )),
        Some(_) => Refused(format!(
            "{}: no knob in {wanted} scope or narrower; scope would widen to {nearest}; \
             pass --scope {nearest}",
            item.id
        )),
    }
    .into())
}

struct Pick<'a> {
    item: &'a Item,
    knob: &'a Knob,
    reason: Option<String>,
}

/// The virtual change of a knob; None for `Knob::None` (which `choose_knob` never returns).
fn overlay_op(target: &Target, knob: &Knob) -> Option<OverlayOp> {
    Some(match knob {
        Knob::Setting {
            file,
            pointer,
            value,
            ..
        } => OverlayOp::SetJson {
            file: file.clone(),
            pointer: pointer.clone(),
            value: value.clone(),
        },
        Knob::Env {
            name, value, scope, ..
        } => OverlayOp::SetJson {
            file: settings_file(target, *scope),
            pointer: env_pointer(name),
            value: serde_json::Value::String(value.clone()),
        },
        Knob::Quarantine { from, .. } => OverlayOp::RemovePath { path: from.clone() },
        Knob::None { .. } => return None,
    })
}

/// The file an op writes (for hashes, warnings and the diff).
fn op_file(target: &Target, knob: &Knob) -> Option<PathBuf> {
    match knob {
        Knob::Setting { file, .. } => Some(file.clone()),
        Knob::Env { scope, .. } => Some(settings_file(target, *scope)),
        Knob::Quarantine { from, .. } => Some(from.clone()),
        Knob::None { .. } => None,
    }
}

fn build(
    resolver: &dyn Resolver,
    target: &Target,
    tree: &Tree,
    picks: &[Pick],
) -> anyhow::Result<ActionPlan> {
    let overlay = Overlay {
        ops: picks
            .iter()
            .filter_map(|p| overlay_op(target, p.knob))
            .collect(),
    };
    let after = resolver
        .resolve(target, Some(&overlay))
        .context("re-resolve with the plan applied as an overlay")?;
    let before_index = Index::of(tree);
    let after_index = Index::of(&after);
    let delta = Delta::between(&before_index, &after_index);
    let freed = delta.freed(tree, &after);

    let mut own: HashSet<&str> = HashSet::new();
    for pick in picks {
        own.extend(walk_item(pick.item).map(|i| i.id.as_str()));
    }
    let side_effects = delta.side_effects(&own);

    let mut ops: Vec<PlannedOp> = picks
        .iter()
        .map(|pick| PlannedOp {
            item_id: pick.item.id.clone(),
            knob: pick.knob.clone(),
            file: op_file(target, pick.knob),
            hash_before: None,
            reason: pick.reason.clone(),
        })
        .collect();
    // The hash of a JSON file is taken from the very bytes its diff was rendered from.
    let (diff, read) = render_diff(&ops)?;
    for op in &mut ops {
        op.hash_before = match (&op.knob, &op.file) {
            (Knob::Quarantine { from, .. }, _) => writers::hash_path(from)?,
            (_, Some(file)) => read.get(file).cloned().flatten(),
            (_, None) => None,
        };
    }
    let warnings = warnings(target, picks, &ops);
    Ok(ActionPlan {
        target_dir: target.dir.clone(),
        agent: tree.agent,
        ops,
        predicted_before: tree.predicted_known,
        predicted_after: after.predicted_known,
        freed,
        side_effects,
        warnings,
        diff,
    })
}

/// The diff the apply will produce (each JSON file patched in op order, renames for moves),
/// and the hash of every JSON file as read for it (None = absent).
fn render_diff(ops: &[PlannedOp]) -> anyhow::Result<(String, HashMap<PathBuf, Option<String>>)> {
    let mut files: Vec<(PathBuf, Vec<(String, serde_json::Value)>)> = Vec::new();
    let mut moves = String::new();
    for op in ops {
        let set = match &op.knob {
            Knob::Setting { pointer, value, .. } => (pointer.clone(), value.clone()),
            Knob::Env { name, value, .. } => {
                (env_pointer(name), serde_json::Value::String(value.clone()))
            }
            Knob::Quarantine { from, to, .. } => {
                moves.push_str(&format!(
                    "rename from {}\nrename to {}\n",
                    from.display(),
                    to.display()
                ));
                continue;
            }
            Knob::None { .. } => continue,
        };
        let Some(file) = &op.file else { continue };
        match files.iter_mut().find(|(f, _)| f == file) {
            Some((_, sets)) => sets.push(set),
            None => files.push((file.clone(), vec![set])),
        }
    }
    let mut diff = String::new();
    let mut read = HashMap::new();
    for (file, sets) in &files {
        let current = writers::read_optional(file)?;
        let new = writers::patch_json(current.as_deref(), sets)
            .with_context(|| file.display().to_string())?;
        diff.push_str(&writers::unified_diff(file, current.as_deref(), Some(&new)));
        read.insert(file.clone(), current.as_deref().map(writers::hash_bytes));
    }
    diff.push_str(&moves);
    Ok((diff, read))
}

fn warnings(target: &Target, picks: &[Pick], ops: &[PlannedOp]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (pick, op) in picks.iter().zip(ops) {
        let file = op
            .file
            .as_deref()
            .map(|f| f.display().to_string())
            .unwrap_or_default();
        match knob_scope(pick.knob) {
            Some(Scope::Project) => out.push(format!(
                "{}: scope project, wider than local — {file} is shared with everyone using this repo",
                pick.item.id
            )),
            Some(Scope::User) => out.push(format!(
                "{}: scope user, wider than local — {file} applies to every project",
                pick.item.id
            )),
            _ => {}
        }
    }
    let mut files: Vec<&Path> = Vec::new();
    for op in ops {
        if let Some(file) = op.file.as_deref() {
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    for file in &files {
        if git_tracked(file) {
            out.push(format!(
                "{} is tracked by git: the change shows up in the repo",
                file.display()
            ));
        }
    }
    let claude_json = target.home.join(".claude.json");
    if files.iter().any(|f| *f == claude_json) {
        let running = if process_running("claude") {
            " (a `claude` process is running now)"
        } else {
            ""
        };
        out.push(format!(
            "{} is also written by Claude Code: apply with no claude running{running}",
            claude_json.display()
        ));
    }
    out
}

/// `git ls-files` lists `path` (a file or a directory with tracked files). False when git is
/// missing or `path` is outside a repository.
fn git_tracked(path: &Path) -> bool {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    if !dir.is_dir() {
        return false;
    }
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "-z", "--"])
        .arg(name)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|out| out.status.success() && !out.stdout.is_empty())
        .unwrap_or(false)
}

fn process_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// An item and its descendants, depth-first.
fn walk_item(item: &Item) -> impl Iterator<Item = &Item> {
    let mut stack = vec![item];
    std::iter::from_fn(move || {
        let item = stack.pop()?;
        stack.extend(item.children.iter().rev());
        Some(item)
    })
}

fn collect_candidates(items: &[Item], path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, Knob)>) {
    for (i, item) in items.iter().enumerate() {
        path.push(i);
        let default_is_a_knob = item.knobs.first().is_some_and(|k| knob_scope(k).is_some());
        if default_is_a_knob {
            if let Ok(knob) = choose_knob(item, None) {
                out.push((path.clone(), knob.clone()));
            }
        }
        collect_candidates(&item.children, path, out);
        path.pop();
    }
}

fn item_at_mut<'a>(items: &'a mut [Item], path: &[usize]) -> Option<&'a mut Item> {
    let (first, rest) = path.split_first()?;
    let item = items.get_mut(*first)?;
    if rest.is_empty() {
        Some(item)
    } else {
        item_at_mut(&mut item.children, rest)
    }
}

/// Every item of a tree with its parent, in walk order, by id.
struct Index<'a> {
    rows: Vec<(&'a Item, Option<&'a Item>)>,
    by_id: HashMap<&'a str, usize>,
}

impl<'a> Index<'a> {
    fn of(tree: &'a Tree) -> Self {
        let mut rows = Vec::new();
        fn visit<'a>(
            items: &'a [Item],
            parent: Option<&'a Item>,
            rows: &mut Vec<(&'a Item, Option<&'a Item>)>,
        ) {
            for item in items {
                rows.push((item, parent));
                visit(&item.children, Some(item), rows);
            }
        }
        visit(&tree.items, None, &mut rows);
        let by_id = rows
            .iter()
            .enumerate()
            .map(|(i, (item, _))| (item.id.as_str(), i))
            .collect();
        Index { rows, by_id }
    }

    fn get(&self, id: &str) -> Option<&(&'a Item, Option<&'a Item>)> {
        self.by_id.get(id).map(|i| &self.rows[*i])
    }
}

/// What differs between the tree before and after an overlay.
struct Delta<'a> {
    /// In the before tree, not in the after tree (walk order).
    removed: Vec<(&'a Item, Option<&'a Item>)>,
    /// In the after tree, not in the before tree.
    appeared: Vec<(&'a Item, Option<&'a Item>)>,
    /// Token-budgeted containers present in both whose cost or entries changed.
    rerendered: bool,
}

impl<'a> Delta<'a> {
    fn between(before: &Index<'a>, after: &Index<'a>) -> Self {
        let removed: Vec<_> = before
            .rows
            .iter()
            .filter(|(item, _)| after.get(&item.id).is_none())
            .copied()
            .collect();
        let appeared: Vec<_> = after
            .rows
            .iter()
            .filter(|(item, _)| before.get(&item.id).is_none())
            .copied()
            .collect();
        let rerendered = before.rows.iter().any(|(item, _)| {
            token_budgeted(item.kind)
                && after.get(&item.id).is_some_and(|(now, _)| {
                    now.cost != item.cost || child_ids(now) != child_ids(item)
                })
        });
        Delta {
            removed,
            appeared,
            rerendered,
        }
    }

    /// `known` = the drop in the tree's known total; `Δ?` when a changed row's cost is unknown or
    /// a token-budgeted listing re-renders; `~` when a changed row's cost is an estimate.
    fn freed(&self, before: &Tree, after: &Tree) -> Freed {
        let changed = self.removed.iter().chain(&self.appeared);
        let mut delta_unknown = self.rerendered;
        let mut is_estimate = false;
        for (item, parent) in changed {
            if cost_unknown(&item.cost) || parent.is_some_and(|p| token_budgeted(p.kind)) {
                delta_unknown = true;
            }
            if item.cost.predicted_is_estimate {
                is_estimate = true;
            }
        }
        Freed {
            known: before.predicted_known.saturating_sub(after.predicted_known),
            delta_unknown,
            is_estimate,
        }
    }

    /// Rows other than `own` (the planned items and their descendants) that leave or appear;
    /// a row whose parent also leaves is folded into the parent's line.
    fn side_effects(&self, own: &HashSet<&str>) -> Vec<String> {
        let removed_ids: HashSet<&str> = self.removed.iter().map(|(i, _)| i.id.as_str()).collect();
        let mut out = Vec::new();
        for (item, parent) in &self.removed {
            if own.contains(item.id.as_str())
                || parent.is_some_and(|p| removed_ids.contains(p.id.as_str()))
            {
                continue;
            }
            let children = walk_item(item).count() - 1;
            out.push(match children {
                0 => format!("{} ({}) leaves", item.label, item.kind),
                n => format!("{} ({}) leaves with {n} children", item.label, item.kind),
            });
        }
        let appeared_ids: HashSet<&str> =
            self.appeared.iter().map(|(i, _)| i.id.as_str()).collect();
        for (item, parent) in &self.appeared {
            if parent.is_some_and(|p| appeared_ids.contains(p.id.as_str())) {
                continue;
            }
            out.push(format!("{} ({}) appears", item.label, item.kind));
        }
        out
    }
}

fn child_ids(item: &Item) -> Vec<&str> {
    item.children.iter().map(|c| c.id.as_str()).collect()
}

/// `?`: nothing says what the row costs in the first request.
fn cost_unknown(cost: &Cost) -> bool {
    !cost.not_applicable && cost.predicted.is_none() && cost.observed.is_none()
}

/// Listings whose budget is in tokens: a removal re-renders them by an amount bytes cannot give.
fn token_budgeted(kind: Kind) -> bool {
    matches!(kind, Kind::SkillListing | Kind::AgentRoster)
}
