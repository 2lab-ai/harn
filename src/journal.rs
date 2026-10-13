//! Journal (WU-E): snapshots + ops with their `RunContext` in harn's own state dir (never inside
//! `~/.claude` or `~/.codex`); undo with a stale check (hash mismatch → `model::Stale`, nothing
//! written); entries are deleted only by `history --prune`.

use crate::model::{Item, RunContext, Target};
use crate::plan::PlannedOp;
use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// State dir, see [`Journal::state_dir`].
    pub dir: PathBuf,
}

impl Journal {
    /// `$XDG_STATE_HOME/harn` or `~/.local/state/harn` on Linux,
    /// `~/Library/Application Support/harn` on macOS.
    pub fn state_dir() -> anyhow::Result<PathBuf> {
        state_dir_for(
            std::env::consts::OS,
            std::env::var_os("XDG_STATE_HOME"),
            dirs::home_dir(),
        )
        .ok_or_else(|| anyhow!("cannot locate harn's state dir: no home directory"))
    }
}

fn state_dir_for(
    os: &str,
    xdg_state_home: Option<OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if os == "macos" {
        return home.map(|home| home.join("Library/Application Support/harn"));
    }
    // XDG Base Directory: a relative $XDG_STATE_HOME is invalid and ignored.
    match xdg_state_home.map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => Some(dir.join("harn")),
        _ => home.map(|home| home.join(".local/state/harn")),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// `<ts>-<rand>`.
    pub id: String,
    pub context: RunContext,
    pub ops: Vec<PlannedOp>,
    pub snapshots: Vec<Snapshot>,
    pub applied_at: String,
    /// Unified diff of every file the entry changed (printed by `off`/`apply`/`edit`).
    pub diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub path: PathBuf,
    /// In the journal dir.
    pub file: PathBuf,
    pub hash_before: String,
    pub hash_after: Option<String>,
}

/// Reverses the last `n` entries. Stale (hash mismatch) → error listing the files, nothing written.
pub fn undo(journal: &mut Journal, n: usize) -> anyhow::Result<Vec<JournalEntry>> {
    let _ = (journal, n);
    Err(anyhow!("not implemented: WU-E"))
}

pub fn history(journal: &Journal) -> anyhow::Result<Vec<JournalEntry>> {
    let _ = journal;
    Err(anyhow!("not implemented: WU-E"))
}

/// Snapshot → `$EDITOR <realpath>` → diff → entry; a JSON/TOML/frontmatter file that no longer
/// parses is journaled with a warning, not reverted.
pub fn edit(item: &Item, journal: &mut Journal, editor: &str) -> anyhow::Result<JournalEntry> {
    let _ = (item, journal, editor);
    Err(anyhow!("not implemented: WU-E"))
}

/// `harn on <sel>`: reverses the journaled op that turned the selected item off in `target`.
/// The item is no longer in the resolved tree, so `selector` is matched against journaled item
/// ids (same rules as `model::select`; ambiguity = `model::SelectError::Ambiguous`).
pub fn on(journal: &mut Journal, target: &Target, selector: &str) -> anyhow::Result<JournalEntry> {
    let _ = (journal, target, selector);
    Err(anyhow!("not implemented: WU-E"))
}

/// `harn history --prune <age>`: deletes entries applied longer than `older_than` ago and returns
/// them. The only path by which journal entries are ever deleted.
pub fn prune(journal: &mut Journal, older_than: Duration) -> anyhow::Result<Vec<JournalEntry>> {
    let _ = (journal, older_than);
    Err(anyhow!("not implemented: WU-E"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_state_dir_is_application_support() {
        assert_eq!(
            state_dir_for("macos", Some("/xdg".into()), Some("/Users/me".into())),
            Some(PathBuf::from("/Users/me/Library/Application Support/harn"))
        );
    }

    #[test]
    fn linux_state_dir_prefers_absolute_xdg_state_home() {
        assert_eq!(
            state_dir_for("linux", Some("/xdg/state".into()), Some("/home/me".into())),
            Some(PathBuf::from("/xdg/state/harn"))
        );
    }

    #[test]
    fn linux_state_dir_falls_back_to_local_state() {
        let expected = Some(PathBuf::from("/home/me/.local/state/harn"));
        assert_eq!(
            state_dir_for("linux", None, Some("/home/me".into())),
            expected
        );
        assert_eq!(
            state_dir_for(
                "linux",
                Some("relative/state".into()),
                Some("/home/me".into())
            ),
            expected
        );
        assert_eq!(
            state_dir_for("linux", Some("".into()), Some("/home/me".into())),
            expected
        );
    }

    #[test]
    fn no_home_no_state_dir() {
        assert_eq!(state_dir_for("linux", None, None), None);
        assert_eq!(state_dir_for("macos", None, None), None);
    }
}
