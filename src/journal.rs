//! Journal (WU-E): snapshots + ops with their `RunContext` in harn's own state dir (never inside
//! `~/.claude` or `~/.codex`); undo with a stale check (hash mismatch → `model::Stale`, nothing
//! written); entries are deleted only by `history --prune`.
//!
//! Layout: `<state>/journal/<id>/entry.json` + `<id>/snapshots/<n>-<name>` (copies keep the
//! source mode) + `<id>/parked/` (files an undo took away because they did not exist before),
//! and `<state>/ops.jsonl`, one line per apply / edit / undo.

use crate::model::{Agent, Item, Knob, Layer, Refused, RunContext, SelectError, Stale, Target};
use crate::plan::PlannedOp;
use crate::writers;
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// `Snapshot.hash_before` of a file that did not exist before the write.
pub const ABSENT: &str = "absent";
/// `RunContext.capture_kind` of an entry written by `off`/`apply`.
pub const KIND_APPLY: &str = "harn apply";
/// `RunContext.capture_kind` of an entry written by `edit`.
pub const KIND_EDIT: &str = "harn edit";

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

    /// The journal in [`Journal::state_dir`], created (0700) when missing.
    pub fn open() -> anyhow::Result<Journal> {
        let journal = Journal {
            dir: Self::state_dir()?,
        };
        check_location(&journal)?;
        create_private_dir(&journal.dir)?;
        Ok(journal)
    }

    fn entries_dir(&self) -> PathBuf {
        self.dir.join("journal")
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join("ops.jsonl")
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
    /// `<ts>-<rand>`; sorts by time.
    pub id: String,
    pub context: RunContext,
    pub ops: Vec<PlannedOp>,
    pub snapshots: Vec<Snapshot>,
    pub applied_at: String,
    /// Unified diff of every file the entry changed (printed by `off`/`apply`/`edit`).
    pub diff: Option<String>,
    /// The plan's warnings; for `edit`, a file that no longer parses.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Set when `undo`/`on` reversed the entry.
    #[serde(default)]
    pub undone_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub path: PathBuf,
    /// In the journal dir. For a quarantine move: where the item was moved to.
    pub file: PathBuf,
    /// [`ABSENT`] when `path` did not exist.
    pub hash_before: String,
    /// `None` = nothing at `path` after the write (or the write never finished).
    pub hash_after: Option<String>,
}

/// Reverses the last `n` entries. Stale (hash mismatch) → error listing the files, nothing written.
pub fn undo(journal: &mut Journal, n: usize) -> anyhow::Result<Vec<JournalEntry>> {
    let live: Vec<(PathBuf, JournalEntry)> = read_entries(journal)?
        .into_iter()
        .filter(|(_, entry)| entry.undone_at.is_none())
        .take(n)
        .collect();
    undo_entries(journal, live)
}

/// Reverses the live entry `id` (the one a user confirmed), wherever it sits in the journal.
/// Same stale check as [`undo`]; an unknown or already undone id → [`Refused`], nothing written.
pub fn undo_entry(journal: &mut Journal, id: &str) -> anyhow::Result<JournalEntry> {
    let (dir, entry) = read_entries(journal)?
        .into_iter()
        .find(|(_, entry)| entry.id == id)
        .ok_or_else(|| Refused(format!("journal entry {id} not found")))?;
    if entry.undone_at.is_some() {
        return Err(Refused(format!("journal entry {id} is already undone")).into());
    }
    let mut undone = undo_entries(journal, vec![(dir, entry)])?;
    Ok(undone.remove(0))
}

/// Every entry, newest first (undone ones included, with `undone_at`).
pub fn history(journal: &Journal) -> anyhow::Result<Vec<JournalEntry>> {
    Ok(read_entries(journal)?
        .into_iter()
        .map(|(_, entry)| entry)
        .collect())
}

/// Snapshot → `$EDITOR <realpath>` → diff → entry; a JSON/TOML/frontmatter file that no longer
/// parses is journaled with a warning, not reverted. An edit that changed nothing is not
/// journaled (error). The entry's `context.cwd` is `target.dir`, the directory `item` was resolved
/// for; `context.agent_version` is `agent_version`, the version the resolver reported for the tree
/// `item` came from.
pub fn edit(
    item: &Item,
    target: &Target,
    journal: &mut Journal,
    editor: &str,
    agent_version: Option<String>,
) -> anyhow::Result<JournalEntry> {
    if matches!(item.origin.layer, Layer::Builtin | Layer::Managed) {
        return Err(Refused(format!(
            "{} is a {} item; harn edit opens only files you own",
            item.id, item.origin.layer
        ))
        .into());
    }
    let path = item
        .origin
        .path
        .clone()
        .ok_or_else(|| Refused(format!("{} has no file to edit", item.id)))?;
    if !path.is_file() {
        return Err(Refused(format!("{} is not a file", path.display())).into());
    }
    check_location(journal)?;
    let mut words = editor.split_whitespace();
    let program = words
        .next()
        .ok_or_else(|| anyhow!("no editor: $EDITOR is empty"))?;

    let before = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let (id, entry_dir) = new_entry_dir(journal)?;
    let snap_dir = entry_dir.join("snapshots");
    create_private_dir(&snap_dir)?;
    let snapshot_file = snap_dir.join(format!("0-{}", writers::file_label(&path)));
    fs::copy(&path, &snapshot_file).with_context(|| format!("snapshot {}", path.display()))?;

    let status = Command::new(program)
        .args(words)
        .arg(&path)
        .status()
        .with_context(|| format!("start editor `{editor}`"));
    let after = writers::read_optional(&path)?;
    if after.as_deref() == Some(before.as_slice()) {
        // Nothing changed: the snapshot dir never became an entry.
        let _ = fs::remove_dir_all(&entry_dir);
        let status = status?;
        let why = if status.success() {
            String::new()
        } else {
            format!("editor exited with {status}; ")
        };
        return Err(anyhow!(
            "{why}{} unchanged, nothing journaled",
            path.display()
        ));
    }

    let mut warnings = Vec::new();
    match &status {
        Ok(status) if !status.success() => {
            warnings.push(format!("editor `{editor}` exited with {status}"))
        }
        Ok(_) => {}
        Err(err) => warnings.push(format!("{err:#}")),
    }
    match &after {
        Some(bytes) => {
            if let Some(problem) = parse_problem(&path, bytes) {
                let again = if parse_problem(&path, &before).is_some() {
                    " (it did not parse before either)"
                } else {
                    ""
                };
                warnings.push(format!(
                    "{} no longer parses: {problem}{again}; kept as written, `harn undo` restores it",
                    path.display()
                ));
            }
        }
        None => warnings.push(format!("the editor removed {}", path.display())),
    }

    let hash_before = writers::hash_bytes(&before);
    let entry = JournalEntry {
        id,
        context: RunContext {
            cwd: target.dir.clone(),
            agent: agent_of(&item.id),
            agent_version,
            model: None,
            launch_opts: Vec::new(),
            settings_fingerprint: None,
            capture_kind: KIND_EDIT.to_string(),
            captured_at: crate::model::now_iso8601(),
        },
        ops: vec![PlannedOp {
            item_id: item.id.clone(),
            knob: Knob::None {
                reason: "harn edit: content edited in $EDITOR".to_string(),
            },
            file: Some(path.clone()),
            hash_before: Some(hash_before.clone()),
            reason: None,
        }],
        snapshots: vec![Snapshot {
            path: path.clone(),
            file: snapshot_file,
            hash_before,
            hash_after: writers::hash_path(&path)?,
        }],
        applied_at: crate::model::now_iso8601(),
        diff: Some(writers::unified_diff(
            &path,
            Some(&before),
            after.as_deref(),
        )),
        warnings,
        undone_at: None,
    };
    write_entry(&entry_dir, &entry)?;
    append_log(journal, "edit", &entry)?;
    Ok(entry)
}

/// `harn on <sel>`: reverses the journaled op that turned the selected item off in `target`.
/// The item is no longer in the resolved tree, so `selector` is matched against journaled item
/// ids (same rules as `model::select`; ambiguity = `model::SelectError::Ambiguous`).
pub fn on(journal: &mut Journal, target: &Target, selector: &str) -> anyhow::Result<JournalEntry> {
    let mut live: Vec<(PathBuf, JournalEntry)> = read_entries(journal)?
        .into_iter()
        .filter(|(_, e)| {
            e.undone_at.is_none()
                && e.context.capture_kind == KIND_APPLY
                && e.context.cwd == target.dir
        })
        .collect();
    let ids: BTreeSet<&str> = live
        .iter()
        .flat_map(|(_, e)| e.ops.iter().map(|op| op.item_id.as_str()))
        .collect();
    let chosen = if ids.contains(selector) {
        selector.to_string()
    } else {
        let suffix: Vec<&str> = ids
            .iter()
            .copied()
            .filter(|id| id.ends_with(selector))
            .collect();
        match suffix.as_slice() {
            [only] => only.to_string(),
            [] => return Err(SelectError::NotFound(selector.to_string()).into()),
            many => {
                return Err(
                    SelectError::Ambiguous(many.iter().map(|s| s.to_string()).collect()).into(),
                )
            }
        }
    };
    let pos = live
        .iter()
        .position(|(_, e)| e.ops.iter().any(|op| op.item_id == chosen))
        .expect("the chosen id comes from a live entry");
    let entry = live.swap_remove(pos);
    let mut undone = undo_entries(journal, vec![entry])?;
    Ok(undone.remove(0))
}

/// `harn history --prune <age>`: deletes entries applied longer than `older_than` ago and returns
/// them. The only path by which journal entries are ever deleted. An entry whose `applied_at`
/// does not parse is kept.
pub fn prune(journal: &mut Journal, older_than: Duration) -> anyhow::Result<Vec<JournalEntry>> {
    let now = time::OffsetDateTime::now_utc();
    let mut pruned = Vec::new();
    for (dir, entry) in read_entries(journal)? {
        let Some(applied) = parse_utc(&entry.applied_at) else {
            continue;
        };
        if (now - applied).whole_nanoseconds() > older_than.as_nanos() as i128 {
            fs::remove_dir_all(&dir).with_context(|| format!("prune {}", dir.display()))?;
            pruned.push(entry);
        }
    }
    if !pruned.is_empty() {
        let gone: BTreeSet<&str> = pruned.iter().map(|e| e.id.as_str()).collect();
        let log = journal.log_path();
        if let Some(bytes) = writers::read_optional(&log)? {
            let kept: String = String::from_utf8_lossy(&bytes)
                .lines()
                .filter(|line| {
                    let id = serde_json::from_str::<serde_json::Value>(line)
                        .ok()
                        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from));
                    !id.is_some_and(|id| gone.contains(id.as_str()))
                })
                .map(|line| format!("{line}\n"))
                .collect();
            writers::write_atomic(&log, kept.as_bytes())?;
        }
    }
    Ok(pruned)
}

/// Undoes `entries` (newest first). Checks every file of every entry first, simulating the
/// earlier undos so two entries on one file chain correctly; any mismatch → [`Stale`], nothing
/// written.
fn undo_entries(
    journal: &mut Journal,
    entries: Vec<(PathBuf, JournalEntry)>,
) -> anyhow::Result<Vec<JournalEntry>> {
    // What each path holds once the newer undos in this batch have run (disk, lazily).
    let mut state: HashMap<PathBuf, Option<String>> = HashMap::new();
    fn now_at(
        path: &Path,
        state: &mut HashMap<PathBuf, Option<String>>,
    ) -> anyhow::Result<Option<String>> {
        if let Some(hash) = state.get(path) {
            return Ok(hash.clone());
        }
        let hash = writers::hash_path(path)?;
        state.insert(path.to_path_buf(), hash.clone());
        Ok(hash)
    }
    let mut stale: Vec<PathBuf> = Vec::new();
    let mut missing: Vec<PathBuf> = Vec::new();
    for (_, entry) in &entries {
        for snapshot in entry.snapshots.iter().rev() {
            if is_move(entry, snapshot) {
                let (from, to) = (&snapshot.path, &snapshot.file);
                if now_at(to, &mut state)? != snapshot.hash_after {
                    stale.push(to.clone());
                }
                if now_at(from, &mut state)?.is_some() {
                    stale.push(from.clone());
                }
                state.insert(to.clone(), None);
                state.insert(from.clone(), snapshot.hash_after.clone());
            } else {
                if now_at(&snapshot.path, &mut state)? != snapshot.hash_after {
                    stale.push(snapshot.path.clone());
                }
                let restored = if snapshot.hash_before == ABSENT {
                    None
                } else {
                    if !snapshot.file.is_file() {
                        missing.push(snapshot.file.clone());
                    }
                    Some(snapshot.hash_before.clone())
                };
                state.insert(snapshot.path.clone(), restored);
            }
        }
    }
    if !stale.is_empty() {
        let mut seen = BTreeSet::new();
        stale.retain(|p| seen.insert(p.clone()));
        return Err(Stale { files: stale }.into());
    }
    if !missing.is_empty() {
        return Err(anyhow!(
            "journal snapshots are missing, nothing written: {}",
            missing
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let mut undone = Vec::new();
    for (dir, mut entry) in entries {
        for snapshot in entry.snapshots.iter().rev() {
            if is_move(&entry, snapshot) {
                writers::move_no_clobber(&snapshot.file, &snapshot.path)?;
                writers::manifest_remove(&writers::manifest_path(&snapshot.file), &entry.id)?;
            } else if snapshot.hash_before == ABSENT {
                if writers::exists_no_follow(&snapshot.path) {
                    park(&dir, &snapshot.path)?;
                }
            } else {
                writers::restore_file(&snapshot.file, &snapshot.path)?;
            }
        }
        entry.undone_at = Some(crate::model::now_iso8601());
        write_entry(&dir, &entry)?;
        append_log(journal, "undo", &entry)?;
        undone.push(entry);
    }
    Ok(undone)
}

/// The snapshot records a quarantine move (its `path` is a quarantine op's `from`).
fn is_move(entry: &JournalEntry, snapshot: &Snapshot) -> bool {
    entry
        .ops
        .iter()
        .any(|op| matches!(&op.knob, Knob::Quarantine { from, .. } if *from == snapshot.path))
}

/// Moves `file` (which did not exist before the entry) into `<entry>/parked/` instead of
/// deleting it. Across filesystems: copy, then remove the original (its bytes are kept).
pub(crate) fn park(entry_dir: &Path, file: &Path) -> anyhow::Result<PathBuf> {
    let parked = entry_dir.join("parked");
    create_private_dir(&parked)?;
    let label = writers::file_label(file);
    let mut dest = parked.join(&label);
    let mut n = 1;
    while writers::exists_no_follow(&dest) {
        dest = parked.join(format!("{label}.{n}"));
        n += 1;
    }
    match fs::rename(file, &dest) {
        Ok(()) => {}
        // EXDEV: the state dir is on another filesystem.
        Err(err) if err.raw_os_error() == Some(18) => {
            fs::copy(file, &dest).with_context(|| format!("park {}", file.display()))?;
            fs::remove_file(file).with_context(|| format!("park {}", file.display()))?;
        }
        Err(err) => {
            return Err(err)
                .with_context(|| format!("park {} in {}", file.display(), dest.display()))
        }
    }
    Ok(dest)
}

/// harn's state never lives inside an agent's own config dir.
pub(crate) fn check_location(journal: &Journal) -> anyhow::Result<()> {
    if let Some(home) = dirs::home_dir() {
        for agent_dir in [".claude", ".codex"] {
            if journal.dir.starts_with(home.join(agent_dir)) {
                return Err(Refused(format!(
                    "journal dir {} is inside ~/{agent_dir}",
                    journal.dir.display()
                ))
                .into());
            }
        }
    }
    Ok(())
}

/// `mkdir -p` with mode 0700 for every directory it creates.
pub(crate) fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("create {}", dir.display()))
}

/// A fresh `<state>/journal/<id>/`.
pub(crate) fn new_entry_dir(journal: &Journal) -> anyhow::Result<(String, PathBuf)> {
    let root = journal.entries_dir();
    create_private_dir(&root)?;
    for _ in 0..16 {
        let id = new_id();
        let dir = root.join(&id);
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err).with_context(|| format!("create {}", dir.display())),
        }
    }
    Err(anyhow!(
        "cannot allocate a journal entry id in {}",
        root.display()
    ))
}

/// `20261013T040506.123456789Z-1a2b3c4d`: fixed width, so ids sort by time.
fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = time::OffsetDateTime::now_utc();
    let seed = format!(
        "{}-{}-{}",
        now.unix_timestamp_nanos(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}.{:09}Z-{}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond(),
        &writers::hash_bytes(seed.as_bytes())[..8]
    )
}

pub(crate) fn write_entry(entry_dir: &Path, entry: &JournalEntry) -> anyhow::Result<()> {
    let value = serde_json::to_value(entry)?;
    writers::write_atomic(&entry_dir.join("entry.json"), &writers::render_json(&value))
}

/// Appends one line `{"event","id","at","context","ops"}` to `<state>/ops.jsonl`.
pub(crate) fn append_log(
    journal: &Journal,
    event: &str,
    entry: &JournalEntry,
) -> anyhow::Result<()> {
    create_private_dir(&journal.dir)?;
    let line = serde_json::json!({
        "event": event,
        "id": entry.id,
        "at": crate::model::now_iso8601(),
        "context": entry.context,
        "ops": entry.ops,
    });
    let path = journal.log_path();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(format!("{line}\n").as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// `(entry dir, entry)` for every entry, newest first. Dirs without `entry.json` are skipped.
fn read_entries(journal: &Journal) -> anyhow::Result<Vec<(PathBuf, JournalEntry)>> {
    let root = journal.entries_dir();
    let listing = match fs::read_dir(&root) {
        Ok(listing) => listing,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("read {}", root.display())),
    };
    let mut entries = Vec::new();
    for dirent in listing {
        let dir = dirent?.path();
        let Some(bytes) = writers::read_optional(&dir.join("entry.json"))? else {
            continue;
        };
        let entry: JournalEntry = serde_json::from_slice(&bytes)
            .with_context(|| format!("{}/entry.json", dir.display()))?;
        entries.push((dir, entry));
    }
    entries.sort_by(|a, b| b.1.id.cmp(&a.1.id));
    Ok(entries)
}

fn agent_of(item_id: &str) -> Agent {
    let prefix = item_id.split('/').next().unwrap_or_default();
    serde_json::from_value(serde_json::Value::String(prefix.to_string()))
        .unwrap_or(Agent::ClaudeCode)
}

/// Why `bytes` no longer parse as what `path` is (`.json`, `.toml`, `---` frontmatter).
fn parse_problem(path: &Path, bytes: &[u8]) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let text = String::from_utf8_lossy(bytes);
    match ext.as_deref() {
        Some("json") => serde_json::from_slice::<serde_json::Value>(bytes)
            .err()
            .map(|e| format!("JSON: {e}")),
        Some("toml") => text
            .parse::<toml_edit::DocumentMut>()
            .err()
            .map(|e| format!("TOML: {}", e.to_string().trim())),
        _ => {
            let mut lines = text.lines();
            if lines.next().map(str::trim_end) != Some("---") {
                return None;
            }
            if lines.any(|l| l.trim_end() == "---") {
                None
            } else {
                Some("frontmatter opened with `---` is never closed".to_string())
            }
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction]Z` (what `model::now_iso8601` writes).
fn parse_utc(s: &str) -> Option<time::OffsetDateTime> {
    let s = s.strip_suffix('Z')?;
    let (date, clock) = s.split_once('T')?;
    let mut ymd = date.splitn(3, '-');
    let year: i32 = ymd.next()?.parse().ok()?;
    let month: u8 = ymd.next()?.parse().ok()?;
    let day: u8 = ymd.next()?.parse().ok()?;
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = hms.splitn(3, ':');
    let hour: u8 = parts.next()?.parse().ok()?;
    let minute: u8 = parts.next()?.parse().ok()?;
    let second: u8 = parts.next()?.parse().ok()?;
    if !fraction.bytes().all(|b| b.is_ascii_digit()) || fraction.len() > 9 {
        return None;
    }
    let nanos: u32 = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<9}").parse().ok()?
    };
    let date =
        time::Date::from_calendar_date(year, time::Month::try_from(month).ok()?, day).ok()?;
    let clock = time::Time::from_hms_nano(hour, minute, second, nanos).ok()?;
    Some(time::PrimitiveDateTime::new(date, clock).assume_utc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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

    #[test]
    fn ids_are_fixed_width_and_sort_by_time() {
        let a = new_id();
        std::thread::sleep(Duration::from_millis(2));
        let b = new_id();
        assert_eq!(a.len(), b.len());
        assert!(a < b, "{a} !< {b}");
    }

    #[test]
    fn parse_utc_reads_now_iso8601() {
        let now = time::OffsetDateTime::now_utc();
        let parsed = parse_utc(&crate::model::now_iso8601()).expect("parses");
        assert!((parsed - now).whole_seconds().abs() < 5);
        assert_eq!(
            parse_utc("2020-01-02T03:04:05Z").unwrap().unix_timestamp(),
            1_577_934_245
        );
        assert_eq!(
            parse_utc("2020-01-02T03:04:05.5Z").unwrap().nanosecond(),
            500_000_000
        );
        assert!(parse_utc("2020-01-02T03:04:05+01:00").is_none());
        assert!(parse_utc("garbage").is_none());
    }

    #[test]
    fn parse_problem_by_kind() {
        assert!(parse_problem(Path::new("a.json"), b"{\"x\":").is_some());
        assert!(parse_problem(Path::new("a.json"), b"{}").is_none());
        assert!(parse_problem(Path::new("a.toml"), b"x = ").is_some());
        assert!(parse_problem(Path::new("a.toml"), b"x = 1").is_none());
        assert!(parse_problem(Path::new("SKILL.md"), b"---\nname: x\nbody").is_some());
        assert!(parse_problem(Path::new("SKILL.md"), b"---\nname: x\n---\nbody").is_none());
        assert!(parse_problem(Path::new("CLAUDE.md"), b"# plain").is_none());
    }

    #[test]
    fn private_dirs_are_0700() {
        let root = std::env::temp_dir().join(format!("harn-journal-unit-{}", std::process::id()));
        let dir = root.join("a/b");
        create_private_dir(&dir).unwrap();
        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let _ = fs::remove_dir_all(&root);
        assert_eq!(mode, 0o700);
    }
}
