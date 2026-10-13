//! The visible rows of one agent's tree: the table's order (`render::common::partition` +
//! `sorted`), children folded unless expanded, the runtime/none block after a separator, then the
//! items harn turned off here (journaled, no longer in the tree). Pure: no terminal, no files.

use crate::model::{Item, Tree};
use crate::render::common::{is_trailing, request_bytes, sort_key};
use std::collections::HashSet;

/// Sort key; `s` steps freed ▼ → freed ▲ → cost ▼ → cost ▲ → name ▲ → name ▼ → freed ▼.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// `freed.known` descending, the table's order.
    Freed,
    /// First-request bytes (observed, else predicted) descending; rows without them last.
    Cost,
    /// Label ascending, case-insensitive.
    Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    /// Flipped from the key's natural direction.
    pub reversed: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort {
            key: SortKey::Freed,
            reversed: false,
        }
    }
}

impl Sort {
    /// The next state of the `s` cycle: the same key reversed, else the next key.
    pub fn next(self) -> Sort {
        if !self.reversed {
            return Sort {
                reversed: true,
                ..self
            };
        }
        let key = match self.key {
            SortKey::Freed => SortKey::Cost,
            SortKey::Cost => SortKey::Name,
            SortKey::Name => SortKey::Freed,
        };
        Sort {
            key,
            reversed: false,
        }
    }

    /// `freed ▼`, `cost ▲`, `name ▲`: the arrow points the way the values run down the screen.
    pub fn label(self, unicode: bool) -> String {
        let (down, up) = if unicode { ("▼", "▲") } else { ("v", "^") };
        let descending = match self.key {
            SortKey::Freed | SortKey::Cost => !self.reversed,
            SortKey::Name => self.reversed,
        };
        let name = match self.key {
            SortKey::Freed => "freed",
            SortKey::Cost => "cost",
            SortKey::Name => "name",
        };
        format!("{name} {}", if descending { down } else { up })
    }

    /// `items` with their index among their siblings, in this order (stable; reversed = the
    /// exact reverse of the natural order). Freed is `render::common::sorted`'s key.
    fn order(self, items: &[Item]) -> Vec<Sorted<'_>> {
        let mut rows: Vec<Sorted> = items.iter().enumerate().collect();
        match self.key {
            SortKey::Freed => rows.sort_by_key(|(_, item)| std::cmp::Reverse(sort_key(item))),
            SortKey::Cost => {
                rows.sort_by_key(|(_, item)| std::cmp::Reverse(request_bytes(&item.cost)))
            }
            SortKey::Name => rows.sort_by_key(|(_, item)| item.label.to_lowercase()),
        }
        if self.reversed {
            rows.reverse();
        }
        rows
    }
}

/// An item with its index among its siblings.
type Sorted<'a> = (usize, &'a Item);

/// An item turned off by a live journal entry for this target, gone from the resolved tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffItem {
    pub item_id: String,
    pub entry_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// Index path into `Tree.items` (then `children`).
    Item(Vec<usize>),
    /// Index into the tab's off items.
    Off(usize),
    /// A block title; the cursor never stops here.
    Separator(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub depth: usize,
}

pub const TRAILING_TITLE: &str = "runtime / no first-request cost:";
pub const OFF_TITLE: &str = "turned off by harn here (o turns back on):";

/// Case-insensitive filter over what a row shows: label and origin.
pub struct Filter {
    needle: String,
}

impl Filter {
    pub fn new(text: &str) -> Option<Filter> {
        let needle = text.trim().to_lowercase();
        (!needle.is_empty()).then_some(Filter { needle })
    }

    pub fn matches(&self, item: &Item) -> bool {
        item.label.to_lowercase().contains(&self.needle)
            || origin_text(item).to_lowercase().contains(&self.needle)
    }

    fn matches_text(&self, text: &str) -> bool {
        text.to_lowercase().contains(&self.needle)
    }

    /// `item` or one of its descendants matches.
    fn reaches(&self, item: &Item) -> bool {
        self.matches(item) || item.children.iter().any(|c| self.reaches(c))
    }
}

/// `plugin alpha` (the table's origin cell: plugin name without the marketplace), else the layer.
pub fn origin_text(item: &Item) -> String {
    match &item.origin.plugin {
        Some(plugin) => {
            let name = plugin.split('@').next().unwrap_or(plugin);
            format!("{} {name}", item.origin.layer)
        }
        None => item.origin.layer.to_string(),
    }
}

/// Ids of every ancestor of a matching item: expanding them shows each match in place.
pub fn ancestors_of_matches(tree: &Tree, filter: &Filter) -> HashSet<String> {
    fn visit(item: &Item, filter: &Filter, out: &mut HashSet<String>) -> bool {
        let mut below = false;
        for child in &item.children {
            if visit(child, filter, out) {
                below = true;
            }
        }
        if below {
            out.insert(item.id.clone());
        }
        below || filter.matches(item)
    }
    let mut out = HashSet::new();
    for item in &tree.items {
        visit(item, filter, &mut out);
    }
    out
}

/// The rows to show: main block, trailing block, off block, each sorted by `sort`; a child is
/// listed only under an expanded parent. With a filter, a row stays when it matches, has a
/// matching descendant (the hierarchy is kept) or sits under a matching ancestor.
pub fn flatten(
    tree: &Tree,
    expanded: &HashSet<String>,
    sort: Sort,
    filter: Option<&Filter>,
    off: &[OffItem],
) -> Vec<Row> {
    let (trailing, main): (Vec<Sorted>, Vec<Sorted>) = sort
        .order(&tree.items)
        .into_iter()
        .partition(|(_, item)| is_trailing(item));

    let mut rows = Vec::new();
    for (i, item) in main {
        push(&mut rows, item, vec![i], 0, expanded, sort, filter, false);
    }
    let mark = rows.len();
    for (i, item) in trailing {
        push(&mut rows, item, vec![i], 0, expanded, sort, filter, false);
    }
    if rows.len() > mark {
        rows.insert(
            mark,
            Row {
                kind: RowKind::Separator(TRAILING_TITLE),
                depth: 0,
            },
        );
    }
    let mark = rows.len();
    for (i, item) in off.iter().enumerate() {
        if filter.is_none_or(|f| f.matches_text(&item.item_id)) {
            rows.push(Row {
                kind: RowKind::Off(i),
                depth: 0,
            });
        }
    }
    if rows.len() > mark {
        rows.insert(
            mark,
            Row {
                kind: RowKind::Separator(OFF_TITLE),
                depth: 0,
            },
        );
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn push(
    rows: &mut Vec<Row>,
    item: &Item,
    path: Vec<usize>,
    depth: usize,
    expanded: &HashSet<String>,
    sort: Sort,
    filter: Option<&Filter>,
    under_match: bool,
) {
    let matched = filter.is_some_and(|f| f.matches(item));
    if let Some(f) = filter {
        if !under_match && !f.reaches(item) {
            return;
        }
    }
    rows.push(Row {
        kind: RowKind::Item(path.clone()),
        depth,
    });
    if item.children.is_empty() || !expanded.contains(&item.id) {
        return;
    }
    for (i, child) in sort.order(&item.children) {
        let mut child_path = path.clone();
        child_path.push(i);
        push(
            rows,
            child,
            child_path,
            depth + 1,
            expanded,
            sort,
            filter,
            under_match || matched,
        );
    }
}

/// The item at an index path.
pub fn item_at<'a>(items: &'a [Item], path: &[usize]) -> Option<&'a Item> {
    let (first, rest) = path.split_first()?;
    let item = items.get(*first)?;
    if rest.is_empty() {
        Some(item)
    } else {
        item_at(&item.children, rest)
    }
}

/// Index path of the item with `id`, depth-first.
pub fn path_of(items: &[Item], id: &str) -> Option<Vec<usize>> {
    for (i, item) in items.iter().enumerate() {
        if item.id == id {
            return Some(vec![i]);
        }
        if let Some(mut rest) = path_of(&item.children, id) {
            rest.insert(0, i);
            return Some(rest);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Agent, Cost, Kind, Layer, Mode, Origin};
    use std::path::PathBuf;

    fn item(id: &str, mode: Mode, children: Vec<Item>) -> Item {
        Item {
            id: id.to_string(),
            label: id.to_string(),
            kind: Kind::Skill,
            origin: Origin {
                layer: Layer::Project,
                path: None,
                setting: None,
                plugin: None,
                why: Vec::new(),
            },
            mode,
            cost: Cost::default(),
            knobs: Vec::new(),
            freed: None,
            signal: None,
            last_used: None,
            children,
        }
    }

    fn tree(items: Vec<Item>) -> Tree {
        Tree {
            agent: Agent::ClaudeCode,
            target_dir: PathBuf::from("/repo"),
            agent_version: None,
            rules_verified_against: "0.0.0".to_string(),
            tool_search: None,
            signals: Vec::new(),
            items,
            predicted_known: 0,
            unknown_rows: 0,
            observed_body: None,
            warnings: Vec::new(),
            home: None,
            observed_regions: Vec::new(),
            tool_search_cause: None,
        }
    }

    fn shape(rows: &[Row]) -> Vec<(RowKind, usize)> {
        rows.iter().map(|r| (r.kind.clone(), r.depth)).collect()
    }

    #[test]
    fn sorted_and_filtered_rows_carry_the_stored_index_paths() {
        // Stored order: zeta, alpha (with kids "kx-b", "kx-a", "other"), runtime-x, mid.
        let t = tree(vec![
            item("zeta", Mode::Always, Vec::new()),
            item(
                "alpha",
                Mode::Always,
                vec![
                    item("kx-b", Mode::Always, Vec::new()),
                    item("kx-a", Mode::Always, Vec::new()),
                    item("other", Mode::Always, Vec::new()),
                ],
            ),
            item("runtime-x", Mode::Runtime, Vec::new()),
            item("mid", Mode::Always, Vec::new()),
        ]);
        let expanded: HashSet<String> = HashSet::from(["alpha".to_string()]);
        let name = Sort {
            key: SortKey::Name,
            reversed: false,
        };
        let item_row = |path: &[usize], depth| (RowKind::Item(path.to_vec()), depth);
        assert_eq!(
            shape(&flatten(&t, &expanded, name, None, &[])),
            [
                item_row(&[1], 0),
                item_row(&[1, 1], 1),
                item_row(&[1, 0], 1),
                item_row(&[1, 2], 1),
                item_row(&[3], 0),
                item_row(&[0], 0),
                (RowKind::Separator(TRAILING_TITLE), 0),
                item_row(&[2], 0),
            ]
        );
        // Filtered on "x": the parent of the matches stays, "other", "zeta", "mid" go.
        let filter = Filter::new("x").unwrap();
        let reversed = Sort {
            reversed: true,
            ..name
        };
        let rows = flatten(&t, &expanded, reversed, Some(&filter), &[]);
        assert_eq!(
            shape(&rows),
            [
                item_row(&[1], 0),
                item_row(&[1, 0], 1),
                item_row(&[1, 1], 1),
                (RowKind::Separator(TRAILING_TITLE), 0),
                item_row(&[2], 0),
            ]
        );
        for row in &rows {
            if let RowKind::Item(path) = &row.kind {
                let found = item_at(&t.items, path).unwrap();
                assert_eq!(path_of(&t.items, &found.id).as_ref(), Some(path));
            }
        }
    }

    #[test]
    fn sort_cycle_visits_every_key_both_ways_and_returns() {
        let mut sort = Sort::default();
        let mut seen = Vec::new();
        for _ in 0..6 {
            seen.push(sort.label(true));
            sort = sort.next();
        }
        assert_eq!(
            seen,
            ["freed ▼", "freed ▲", "cost ▼", "cost ▲", "name ▲", "name ▼"]
        );
        assert_eq!(sort, Sort::default());
        assert_eq!(Sort::default().label(false), "freed v");
    }
}
