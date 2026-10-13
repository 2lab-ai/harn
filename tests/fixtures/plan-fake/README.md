# plan-fake

Input of the FAKE resolver in `tests/plan_journal.rs` (WU-E): not a Claude Code layout, a tree
spec. `tree.json` lists items with `${PROJECT}` / `${HOME}` placeholders (replaced by the test's
temp copies of `project/` and `home/`), a `predicted` byte cost (`null` = `?`), the knobs, and
`off_if` gates — `{file, pointer, equals | contains}` read through the overlay — that drop the
item (and its children). An item with a `path` is also dropped when an overlay `remove-path`
covers it or the path no longer exists. `sum_children` containers cost the sum of their entries
and are not counted twice in the total.

`project/.claude/settings.local.json` has two keys, 4-space indent and no trailing newline, so a
test sees harn re-render it (2-space, order kept) and `undo` restore the original bytes.
