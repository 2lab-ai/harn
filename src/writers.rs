//! Writes (WU-E): atomic replace (temp + fsync + rename); JSON via serde_json with
//! `preserve_order`, 2-space; TOML (Codex's `config.toml`, P0.5 C4) via toml_edit, which keeps the
//! file's comments, key order and formatting; quarantine moves to
//! `<discovery-root-parent>/harn-off/<kind>/<name>` + `manifest.json`. Every write goes through
//! the journal (snapshot first). A setting's file format is its extension: `.toml` = TOML, every
//! other file = JSON ([`patch_file`]).
//!
//! harn never deletes: a file that did not exist before an apply is moved into the journal entry
//! on undo, and a quarantined item is moved, never removed.

use crate::journal::{self, Journal, JournalEntry, Snapshot};
use crate::model::{Knob, Refused, RunContext, Stale};
use crate::plan::{ActionPlan, PlannedOp};
use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Snapshots every file the plan touches into the journal, applies the ops, records the entry.
///
/// Refuses with [`Stale`] when a file changed since the plan read it (its `hash_before`), and
/// with [`Refused`] when a quarantine destination already exists. Nothing is written then.
pub fn apply(plan: &ActionPlan, journal: &mut Journal) -> anyhow::Result<JournalEntry> {
    journal::check_location(journal)?;
    if plan.ops.is_empty() {
        return Err(Refused("the plan has no ops".to_string()).into());
    }
    let steps = plan
        .ops
        .iter()
        .map(step_of)
        .collect::<anyhow::Result<Vec<_>>>()?;

    // One patch per distinct settings file, in first-appearance order. Each file is read once:
    // the stale check, the patch and the snapshot all use the same bytes.
    let mut set_files: Vec<(PathBuf, Vec<(String, Value)>)> = Vec::new();
    for step in &steps {
        if let Step::Set {
            file,
            pointer,
            value,
        } = step
        {
            let set = (pointer.clone(), value.clone());
            match set_files.iter_mut().find(|(f, _)| f == file) {
                Some((_, sets)) => sets.push(set),
                None => set_files.push((file.clone(), vec![set])),
            }
        }
    }
    let mut current: Vec<Option<Vec<u8>>> = Vec::new();
    for (file, _) in &set_files {
        current.push(read_optional(file)?);
    }

    // Stale check against the hashes the plan was built on: every file, before any write.
    let mut stale: Vec<PathBuf> = Vec::new();
    for (op, step) in plan.ops.iter().zip(&steps) {
        let (path, now) = match step {
            Step::Set { file, .. } => {
                let i = set_files
                    .iter()
                    .position(|(f, _)| f == file)
                    .expect("grouped above");
                (file, current[i].as_deref().map(hash_bytes))
            }
            Step::Move { from, .. } => (from, hash_path(from)?),
        };
        if now != op.hash_before && !stale.contains(path) {
            stale.push(path.clone());
        }
    }
    if !stale.is_empty() {
        return Err(Stale { files: stale }.into());
    }
    for step in &steps {
        if let Step::Move { to, .. } = step {
            if exists_no_follow(to) {
                return Err(Refused(format!(
                    "quarantine destination {} already exists",
                    to.display()
                ))
                .into());
            }
        }
    }

    let mut patched = Vec::new();
    for ((file, sets), current) in set_files.iter().zip(current) {
        let new = patch_file(file, current.as_deref(), sets)
            .with_context(|| file.display().to_string())?;
        patched.push((file.clone(), current, new));
    }

    let (id, entry_dir) = journal::new_entry_dir(journal)?;
    let snap_dir = entry_dir.join("snapshots");
    journal::create_private_dir(&snap_dir)?;
    let mut snapshots = Vec::new();
    for (n, (file, current, _)) in patched.iter().enumerate() {
        let snapshot_file = snap_dir.join(format!("{n}-{}", file_label(file)));
        if let Some(bytes) = current {
            // The bytes that were checked and patched, with the source file's mode.
            fs::write(&snapshot_file, bytes)
                .and_then(|()| fs::metadata(file))
                .and_then(|meta| fs::set_permissions(&snapshot_file, meta.permissions()))
                .with_context(|| {
                    format!(
                        "snapshot {} into {}",
                        file.display(),
                        snapshot_file.display()
                    )
                })?;
        }
        snapshots.push(Snapshot {
            path: file.clone(),
            file: snapshot_file,
            hash_before: current
                .as_deref()
                .map(hash_bytes)
                .unwrap_or_else(|| journal::ABSENT.to_string()),
            hash_after: None,
        });
    }
    for step in &steps {
        if let Step::Move { from, to, .. } = step {
            // A move is its own snapshot: `file` is where the item now lives.
            snapshots.push(Snapshot {
                path: from.clone(),
                file: to.clone(),
                hash_before: hash_path(from)?.unwrap_or_else(|| journal::ABSENT.to_string()),
                hash_after: None,
            });
        }
    }

    let mut entry = JournalEntry {
        id: id.clone(),
        context: RunContext {
            cwd: plan.target_dir.clone(),
            agent: plan.agent,
            agent_version: plan.agent_version.clone(),
            model: None,
            launch_opts: Vec::new(),
            settings_fingerprint: None,
            capture_kind: journal::KIND_APPLY.to_string(),
            captured_at: crate::model::now_iso8601(),
        },
        ops: plan.ops.clone(),
        snapshots,
        applied_at: crate::model::now_iso8601(),
        diff: None,
        warnings: plan.warnings.clone(),
        undone_at: None,
    };
    // Written before the first change, so a crash mid-apply leaves the snapshots findable.
    journal::write_entry(&entry_dir, &entry)?;

    let mut diff = String::new();
    let mut done: Vec<Done> = Vec::new();
    let result = (|| -> anyhow::Result<()> {
        for (file, current, new) in &patched {
            write_atomic(file, new)?;
            done.push(Done::Set(file.clone(), current.is_some()));
            diff.push_str(&unified_diff(file, current.as_deref(), Some(new)));
        }
        for step in &steps {
            if let Step::Move {
                from,
                to,
                kind,
                item_id,
            } = step
            {
                move_no_clobber(from, to)?;
                done.push(Done::Move(from.clone(), to.clone()));
                diff.push_str(&format!(
                    "rename from {}\nrename to {}\n",
                    from.display(),
                    to.display()
                ));
                let manifest = manifest_path(to);
                let before = read_optional(&manifest)?;
                manifest_add(&manifest, from, to, kind, item_id, &id)?;
                let after = read_optional(&manifest)?;
                diff.push_str(&unified_diff(
                    &manifest,
                    before.as_deref(),
                    after.as_deref(),
                ));
            }
        }
        Ok(())
    })();
    if let Err(err) = result {
        // Put back what this apply already changed. A reverted attempt stays on record as undone,
        // so it never blocks a later `undo` as stale.
        return Err(match rollback(&entry, &done, &entry_dir) {
            Ok(()) => {
                entry.undone_at = Some(crate::model::now_iso8601());
                entry
                    .warnings
                    .push(format!("apply failed and was reverted: {err:#}"));
                match journal::write_entry(&entry_dir, &entry) {
                    Ok(()) => err.context("apply failed; its earlier writes were reverted"),
                    Err(record) => err.context(format!(
                        "apply failed; its earlier writes were reverted, but marking journal \
                         entry {id} as undone failed ({record:#})"
                    )),
                }
            }
            Err(rb) => err.context(format!(
                "apply failed and reverting its earlier writes also failed ({rb:#}); \
                 snapshots are in {}",
                entry_dir.display()
            )),
        });
    }

    for snapshot in &mut entry.snapshots {
        let moved = steps
            .iter()
            .any(|s| matches!(s, Step::Move { from, .. } if *from == snapshot.path));
        let now_at = if moved {
            &snapshot.file
        } else {
            &snapshot.path
        };
        snapshot.hash_after = hash_path(now_at)?;
    }
    entry.diff = Some(diff);
    journal::write_entry(&entry_dir, &entry)?;
    journal::append_log(journal, "apply", &entry)?;
    Ok(entry)
}

/// What one planned op writes.
enum Step {
    /// A value at a JSON pointer of a settings file (JSON, or TOML: [`patch_file`]).
    Set {
        file: PathBuf,
        pointer: String,
        value: Value,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
        kind: String,
        item_id: String,
    },
}

fn step_of(op: &PlannedOp) -> anyhow::Result<Step> {
    let file = || {
        op.file
            .clone()
            .ok_or_else(|| anyhow!("the op for {} names no file", op.item_id))
    };
    Ok(match &op.knob {
        Knob::Setting { pointer, value, .. } => Step::Set {
            file: file()?,
            pointer: pointer.clone(),
            value: value.clone(),
        },
        Knob::Env { name, value, .. } => Step::Set {
            file: file()?,
            pointer: crate::plan::env_pointer(name),
            value: Value::String(value.clone()),
        },
        Knob::Quarantine { from, to, .. } => Step::Move {
            from: from.clone(),
            to: to.clone(),
            kind: op.item_id.split('/').nth(1).unwrap_or("item").to_string(),
            item_id: op.item_id.clone(),
        },
        Knob::None { reason } => {
            return Err(Refused(format!("{} has no knob: {reason}", op.item_id)).into())
        }
    })
}

enum Done {
    /// File written; `true` when it existed before.
    Set(PathBuf, bool),
    Move(PathBuf, PathBuf),
}

fn rollback(entry: &JournalEntry, done: &[Done], entry_dir: &Path) -> anyhow::Result<()> {
    for step in done.iter().rev() {
        match step {
            Done::Set(file, existed) => {
                let snapshot = entry
                    .snapshots
                    .iter()
                    .find(|s| s.path == *file)
                    .ok_or_else(|| anyhow!("no snapshot for {}", file.display()))?;
                if *existed {
                    restore_file(&snapshot.file, file)?;
                } else {
                    journal::park(entry_dir, file)?;
                }
            }
            Done::Move(from, to) => move_no_clobber(to, from)?,
        }
    }
    Ok(())
}

/// Restores `path` from the snapshot copy `snapshot`, atomically, with the snapshot's mode.
pub(crate) fn restore_file(snapshot: &Path, path: &Path) -> anyhow::Result<()> {
    let bytes = fs::read(snapshot).with_context(|| format!("read {}", snapshot.display()))?;
    let mode = fs::metadata(snapshot)?.permissions().mode() & 0o7777;
    write_atomic_mode(path, &bytes, Some(mode))
}

/// `rename`, refusing to replace an existing destination; creates the destination's parent.
pub(crate) fn move_no_clobber(from: &Path, to: &Path) -> anyhow::Result<()> {
    if exists_no_follow(to) {
        bail!(
            "{} already exists; not moving {} over it",
            to.display(),
            from.display()
        );
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::rename(from, to).with_context(|| format!("move {} to {}", from.display(), to.display()))
}

/// Something (a file, dir or symlink, dangling or not) is at `path`.
pub(crate) fn exists_no_follow(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// `harn-off/manifest.json` for a quarantine destination `<…>/harn-off/<kind>/<name>`
/// (the destination's parent when it is not under a `harn-off` dir).
pub(crate) fn manifest_path(to: &Path) -> PathBuf {
    to.ancestors()
        .skip(1)
        .find(|a| a.file_name().is_some_and(|n| n == "harn-off"))
        .or_else(|| to.parent())
        .unwrap_or(Path::new("/"))
        .join("manifest.json")
}

fn manifest_add(
    manifest: &Path,
    from: &Path,
    to: &Path,
    kind: &str,
    item_id: &str,
    journal_id: &str,
) -> anyhow::Result<()> {
    let mut doc = manifest_read(manifest)?;
    let record = serde_json::json!({
        "from": from,
        "to": to,
        "kind": kind,
        "item": item_id,
        "date": crate::model::now_iso8601(),
        "journal": journal_id,
    });
    entries_mut(&mut doc, manifest)?.push(record);
    write_atomic(manifest, &render_json(&doc))
}

/// Drops the manifest records of `journal_id` (undo of a quarantine). A missing manifest is fine.
pub(crate) fn manifest_remove(manifest: &Path, journal_id: &str) -> anyhow::Result<()> {
    if !exists_no_follow(manifest) {
        return Ok(());
    }
    let mut doc = manifest_read(manifest)?;
    entries_mut(&mut doc, manifest)?
        .retain(|r| r.get("journal").and_then(Value::as_str) != Some(journal_id));
    write_atomic(manifest, &render_json(&doc))
}

fn manifest_read(manifest: &Path) -> anyhow::Result<Value> {
    match read_optional(manifest)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not JSON", manifest.display())),
        None => Ok(serde_json::json!({ "schema": "harn-off/1", "entries": [] })),
    }
}

fn entries_mut<'a>(doc: &'a mut Value, manifest: &Path) -> anyhow::Result<&'a mut Vec<Value>> {
    doc.get_mut("entries")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("{} has no `entries` array", manifest.display()))
}

/// `Some(bytes)`, or `None` when the file does not exist.
pub(crate) fn read_optional(path: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}

/// Lowercase hex SHA-256.
pub fn hash_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hash of what is at `path` now: `None` when nothing is there; a file = SHA-256 of its bytes
/// (symlinks followed); a directory = `dir:` + SHA-256 over its sorted tree (relative names,
/// symlink targets, file contents); a dangling symlink = `link:` + SHA-256 of its target.
pub fn hash_path(path: &Path) -> anyhow::Result<Option<String>> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(fs::read_link(path)
                .ok()
                .map(|target| format!("link:{}", hash_bytes(target.as_os_str().as_bytes()))));
        }
        Err(err) => return Err(err).with_context(|| format!("stat {}", path.display())),
    };
    if !meta.is_dir() {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        return Ok(Some(hash_bytes(&bytes)));
    }
    let mut hasher = Sha256::new();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.with_context(|| format!("walk {}", path.display()))?;
        let rel = entry.path().strip_prefix(path).unwrap_or(entry.path());
        hasher.update(rel.as_os_str().as_bytes());
        hasher.update([0]);
        let kind = entry.file_type();
        if kind.is_symlink() {
            hasher.update(b"l");
            hasher.update(fs::read_link(entry.path())?.as_os_str().as_bytes());
        } else if kind.is_dir() {
            hasher.update(b"d");
        } else {
            hasher.update(b"f");
            hasher.update(Sha256::digest(fs::read(entry.path())?));
        }
        hasher.update([0]);
    }
    Ok(Some(format!("dir:{}", hex(&hasher.finalize()))))
}

/// Sets `value` at JSON `pointer` (RFC 6901), creating missing intermediate objects (an array
/// when the next token is `-`). `-` appends to an array. Refuses to replace the whole document or
/// to descend through a scalar.
pub fn set_json_pointer(doc: &mut Value, pointer: &str, value: Value) -> anyhow::Result<()> {
    let tokens = pointer_tokens(pointer)?;
    let mut value = Some(value);
    let mut cur = doc;
    for (i, token) in tokens.iter().enumerate() {
        let last = i + 1 == tokens.len();
        let fresh = || {
            if tokens.get(i + 1).is_some_and(|t| t == "-") {
                Value::Array(Vec::new())
            } else {
                Value::Object(serde_json::Map::new())
            }
        };
        cur = match cur {
            Value::Object(map) => {
                if last {
                    map.insert(token.clone(), value.take().expect("set once"));
                    return Ok(());
                }
                map.entry(token.clone()).or_insert_with(fresh)
            }
            Value::Array(items) => {
                let len = items.len();
                let index = array_index(pointer, token, len)?;
                if index == len {
                    if last {
                        items.push(value.take().expect("set once"));
                        return Ok(());
                    }
                    items.push(fresh());
                } else if last {
                    items[index] = value.take().expect("set once");
                    return Ok(());
                }
                &mut items[index]
            }
            other => bail!(
                "`{pointer}`: /{} is {}, not an object or array",
                tokens[..i].join("/"),
                json_type(other)
            ),
        };
    }
    unreachable!("a pointer with a leading `/` has at least one token")
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn unescape_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// The decoded tokens of a JSON pointer (`~1` → `/`, `~0` → `~`); the JSON and the TOML writer
/// both walk these. Refuses a pointer without a leading `/` (the whole document).
fn pointer_tokens(pointer: &str) -> anyhow::Result<Vec<String>> {
    let Some(rest) = pointer.strip_prefix('/') else {
        bail!(
            "JSON pointer `{pointer}` must start with `/` (the whole document is never replaced)"
        );
    };
    Ok(rest.split('/').map(unescape_token).collect())
}

/// The index `token` names in an array of `len`: `-` = `len` (append), else a number up to `len`
/// without leading zeros.
fn array_index(pointer: &str, token: &str, len: usize) -> anyhow::Result<usize> {
    if token == "-" {
        return Ok(len);
    }
    token
        .parse::<usize>()
        .ok()
        .filter(|n| *n <= len && (token == "0" || !token.starts_with('0')))
        .ok_or_else(|| anyhow!("`{pointer}`: `{token}` is not an index of an array of {len}"))
}

/// One JSON pointer token for `raw` (RFC 6901: `~` → `~0`, `/` → `~1`).
pub fn escape_token(raw: &str) -> String {
    raw.replace('~', "~0").replace('/', "~1")
}

/// `current` (None or blank = `{}`) with every `(pointer, value)` set in order, rendered the way
/// harn writes JSON: key order preserved, 2-space indent, trailing newline. An array of scalars
/// that the file had on one line stays on one line (`["a", "b"]`), appended entries included, so
/// the diff shows only the change; every other array and object is multi-line. A file that does
/// not parse is [`Refused`], never overwritten.
pub fn patch_json(current: Option<&[u8]>, sets: &[(String, Value)]) -> anyhow::Result<Vec<u8>> {
    let (mut doc, one_line) = match current {
        Some(bytes) if !bytes.iter().all(u8::is_ascii_whitespace) => {
            let doc = serde_json::from_slice::<Value>(bytes).map_err(|e| {
                Refused(format!(
                    "does not parse as JSON ({e}); harn will not rewrite it"
                ))
            })?;
            (doc, one_line_arrays(bytes))
        }
        _ => (Value::Object(serde_json::Map::new()), Vec::new()),
    };
    for (pointer, value) in sets {
        set_json_pointer(&mut doc, pointer, value.clone())?;
    }
    let mut out = String::new();
    write_json(&mut out, &doc, "", 0, &one_line);
    out.push('\n');
    Ok(out.into_bytes())
}

/// JSON pointers of the arrays written on one line in `source` (no newline between `[` and `]`).
/// `source` already parsed; anything the scan does not expect yields no pointers (all multi-line).
fn one_line_arrays(source: &[u8]) -> Vec<String> {
    struct Scan<'a> {
        src: &'a [u8],
        at: usize,
        path: Vec<String>,
        found: Vec<String>,
    }
    impl Scan<'_> {
        fn skip_ws(&mut self) {
            while self.src.get(self.at).is_some_and(u8::is_ascii_whitespace) {
                self.at += 1;
            }
        }
        fn peek(&mut self) -> Option<u8> {
            self.skip_ws();
            self.src.get(self.at).copied()
        }
        fn expect(&mut self, byte: u8) -> Option<()> {
            (self.peek()? == byte).then(|| self.at += 1)
        }
        /// The decoded string starting at `at` (on its opening quote).
        fn string(&mut self) -> Option<String> {
            let start = self.at;
            self.at += 1;
            loop {
                match *self.src.get(self.at)? {
                    b'\\' => self.at += 2,
                    b'"' => break,
                    _ => self.at += 1,
                }
            }
            self.at += 1;
            serde_json::from_slice(&self.src[start..self.at]).ok()
        }
        fn pointer(&self) -> String {
            self.path
                .iter()
                .map(|t| format!("/{}", escape_token(t)))
                .collect()
        }
        fn value(&mut self) -> Option<()> {
            match self.peek()? {
                b'{' => {
                    self.at += 1;
                    if self.peek()? == b'}' {
                        self.at += 1;
                        return Some(());
                    }
                    loop {
                        if self.peek()? != b'"' {
                            return None;
                        }
                        let key = self.string()?;
                        self.expect(b':')?;
                        self.path.push(key);
                        self.value()?;
                        self.path.pop();
                        match self.peek()? {
                            b',' => self.at += 1,
                            b'}' => {
                                self.at += 1;
                                return Some(());
                            }
                            _ => return None,
                        }
                    }
                }
                b'[' => {
                    let start = self.at;
                    self.at += 1;
                    if self.peek()? != b']' {
                        let mut index = 0usize;
                        loop {
                            self.path.push(index.to_string());
                            self.value()?;
                            self.path.pop();
                            index += 1;
                            match self.peek()? {
                                b',' => self.at += 1,
                                b']' => break,
                                _ => return None,
                            }
                        }
                    }
                    self.at += 1;
                    if !self.src[start..self.at].contains(&b'\n') {
                        self.found.push(self.pointer());
                    }
                    Some(())
                }
                b'"' => self.string().map(|_| ()),
                _ => {
                    while self.src.get(self.at).is_some_and(|b| {
                        !matches!(b, b',' | b']' | b'}') && !b.is_ascii_whitespace()
                    }) {
                        self.at += 1;
                    }
                    Some(())
                }
            }
        }
    }
    let mut scan = Scan {
        src: source,
        at: 0,
        path: Vec::new(),
        found: Vec::new(),
    };
    match scan.value() {
        Some(()) => scan.found,
        None => Vec::new(),
    }
}

/// `serde_json`'s 2-space pretty layout, except that an array of scalars whose pointer is in
/// `one_line` is written `[a, b]`.
fn write_json(out: &mut String, value: &Value, pointer: &str, depth: usize, one_line: &[String]) {
    let indent = |out: &mut String, depth: usize| out.push_str(&"  ".repeat(depth));
    match value {
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            out.push_str("{\n");
            for (i, (key, child)) in map.iter().enumerate() {
                indent(out, depth + 1);
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                let child_pointer = format!("{pointer}/{}", escape_token(key));
                write_json(out, child, &child_pointer, depth + 1, one_line);
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push('}');
        }
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items)
            if one_line.iter().any(|p| p == pointer)
                && items.iter().all(|v| !v.is_array() && !v.is_object()) =>
        {
            let parts: Vec<String> = items.iter().map(Value::to_string).collect();
            out.push('[');
            out.push_str(&parts.join(", "));
            out.push(']');
        }
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, child) in items.iter().enumerate() {
                indent(out, depth + 1);
                write_json(out, child, &format!("{pointer}/{i}"), depth + 1, one_line);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// 2-space JSON with a trailing newline (`serde_json` pretty printer, `preserve_order`).
pub fn render_json(doc: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(doc).expect("a serde_json::Value always serializes");
    bytes.push(b'\n');
    bytes
}

/// `current` (None = absent) with every `(pointer, value)` set in order, in the file's own format:
/// TOML when `path` ends in `.toml` ([`patch_toml`]), JSON otherwise ([`patch_json`]).
pub fn patch_file(
    path: &Path,
    current: Option<&[u8]>,
    sets: &[(String, Value)],
) -> anyhow::Result<Vec<u8>> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
    {
        patch_toml(current, sets)
    } else {
        patch_json(current, sets)
    }
}

/// `current` (None = an empty document) with every `(pointer, value)` set in order through
/// toml_edit: comments, blank lines, key order, quoting and a missing final newline stay as they
/// were (rules at [`set_toml_pointer`]). A file that is not UTF-8 or does not parse is [`Refused`],
/// never overwritten.
pub fn patch_toml(current: Option<&[u8]>, sets: &[(String, Value)]) -> anyhow::Result<Vec<u8>> {
    let text = match current {
        Some(bytes) => std::str::from_utf8(bytes)
            .map_err(|e| Refused(format!("is not UTF-8 ({e}); harn will not rewrite it")))?,
        None => "",
    };
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| {
        Refused(format!(
            "does not parse as TOML ({}); harn will not rewrite it",
            e.to_string().trim().replace('\n', " ")
        ))
    })?;
    for (pointer, value) in sets {
        set_toml_pointer(&mut doc, pointer, value)?;
    }
    let mut out = doc.to_string();
    // toml_edit ends every key line with a newline; a file that had no final newline keeps none.
    if !text.is_empty() && !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    Ok(out.into_bytes())
}

/// Sets `value` at JSON `pointer` in a TOML document, as seen through the JSON view the Codex
/// resolver reads (tables and inline tables = objects; arrays of tables and arrays = arrays):
///
/// - every token is a key, written bare when TOML allows it and quoted otherwise, so
///   `/plugins/slack@openai-curated/enabled` lands in `[plugins."slack@openai-curated"]`; `~1` and
///   `~0` decode to `/` and `~` (`/hooks/state/~1abs~1hooks.json:session_start:0:0/enabled` →
///   `[hooks.state."/abs/hooks.json:session_start:0:0"]`);
/// - a missing container is created: a table (implicit, so only the deepest header is written;
///   dotted keys inside a dotted-key table; an inline table inside an inline value or array), or an
///   array when the next token is `-` — an array of tables when it will hold tables;
/// - `-` appends: a new `[[…]]` block to an array of tables (which refuses `-` then `-`, an
///   array it cannot hold), or an element to an array, indented like the last element's line
///   without copying its comments; an index replaces (the length appends);
/// - the last token replaces a value in place, keeping its decor (`key = new # comment`) — an
///   object or array replacing a value is written inline on that line, and an equal value is left
///   as spelled (`'lit'`, `1_000`) — or replaces a table or `[[…]]` blocks, carrying the comments
///   above and after the old header to the new key line or header; a new key goes after the
///   table's last key;
/// - JSON scalars become TOML scalars and arrays arrays; a new object becomes a table (dotted keys
///   inside a dotted-key table, an inline table inside an inline value or an array, `{}` when
///   empty) and a new array of objects `[[…]]` blocks (inline inside a dotted-key table). `null`
///   (TOML has none) and integers past `i64` are refused.
///
/// Refuses to replace the whole document or to descend through a scalar.
pub fn set_toml_pointer(
    doc: &mut toml_edit::DocumentMut,
    pointer: &str,
    value: &Value,
) -> anyhow::Result<()> {
    let tokens = pointer_tokens(pointer)?;
    TomlSet {
        pointer,
        tokens: &tokens,
        value,
    }
    .table(doc.as_table_mut(), 0)
}

/// One [`set_toml_pointer`] walk; `i` = the token the current container is indexed by.
struct TomlSet<'a> {
    pointer: &'a str,
    tokens: &'a [String],
    value: &'a Value,
}

impl TomlSet<'_> {
    fn last(&self, i: usize) -> bool {
        i + 1 == self.tokens.len()
    }

    fn not_container(&self, i: usize, what: &str) -> anyhow::Error {
        anyhow!(
            "`{}`: /{} is {what}, not a table or array",
            self.pointer,
            self.tokens[..i].join("/")
        )
    }

    /// The element appended by the `-` at token `i` is a table: the value set is an object, or
    /// the path continues with a key.
    fn appends_a_table(&self, i: usize) -> bool {
        match self.tokens.get(i + 1) {
            None => self.value.is_object(),
            Some(next) => next != "-",
        }
    }

    fn table(&self, table: &mut toml_edit::Table, i: usize) -> anyhow::Result<()> {
        let key = self.tokens[i].as_str();
        let dotted = table.is_dotted();
        if self.last(i) {
            match table.get_key_value_mut(key) {
                // A value stays a value on its line (an object becomes an inline table).
                Some((_, toml_edit::Item::Value(old))) => {
                    replace_value(old, toml_value(self.value, self.pointer)?)
                }
                Some((mut old_key, slot)) if !slot.is_none() => {
                    let new = toml_item(self.value, dotted, self.pointer)?;
                    replace_item(&mut old_key, slot, new);
                }
                _ => {
                    table.insert(key, toml_item(self.value, dotted, self.pointer)?);
                }
            }
            return Ok(());
        }
        if table.get(key).is_none_or(toml_edit::Item::is_none) {
            let fresh = if self.tokens[i + 1] == "-" {
                if self.appends_a_table(i + 1) && !dotted {
                    toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new())
                } else {
                    toml_edit::Item::Value(toml_edit::Array::new().into())
                }
            } else {
                toml_edit::Item::Table(new_table(dotted))
            };
            table.insert(key, fresh);
        }
        let child = table.get_mut(key).expect("present or just inserted");
        self.item(child, i + 1)
    }

    fn item(&self, item: &mut toml_edit::Item, i: usize) -> anyhow::Result<()> {
        match item {
            toml_edit::Item::Table(table) => self.table(table, i),
            toml_edit::Item::ArrayOfTables(tables) => self.array_of_tables(tables, i),
            toml_edit::Item::Value(value) => self.value_at(value, i),
            toml_edit::Item::None => Err(self.not_container(i, "absent")),
        }
    }

    fn array_of_tables(
        &self,
        tables: &mut toml_edit::ArrayOfTables,
        i: usize,
    ) -> anyhow::Result<()> {
        let len = tables.len();
        let index = array_index(self.pointer, &self.tokens[i], len)?;
        if self.last(i) {
            let Value::Object(map) = self.value else {
                bail!(
                    "`{}`: /{} is an array of tables; only an object goes in it",
                    self.pointer,
                    self.tokens[..i].join("/")
                );
            };
            let mut table = toml_table(map, false, self.pointer)?;
            if index == len {
                tables.push(table);
            } else {
                let old = tables.get_mut(index).expect("index within the array");
                table.set_position(old.position());
                *table.decor_mut() = old.decor().clone();
                *old = table;
            }
            return Ok(());
        }
        if index == len {
            if !self.appends_a_table(i) {
                bail!(
                    "`{}`: /{} is an array of tables; the `-` before `-` appends an array, which \
                     it cannot hold",
                    self.pointer,
                    self.tokens[..i].join("/")
                );
            }
            tables.push(toml_edit::Table::new());
        }
        let table = tables.get_mut(index).expect("index within the array");
        self.table(table, i + 1)
    }

    fn value_at(&self, value: &mut toml_edit::Value, i: usize) -> anyhow::Result<()> {
        match value {
            toml_edit::Value::InlineTable(table) => self.inline_table(table, i),
            toml_edit::Value::Array(array) => self.array(array, i),
            other => Err(self.not_container(i, toml_type(other))),
        }
    }

    fn inline_table(&self, table: &mut toml_edit::InlineTable, i: usize) -> anyhow::Result<()> {
        let key = self.tokens[i].as_str();
        if self.last(i) {
            let new = toml_value(self.value, self.pointer)?;
            match table.get_mut(key) {
                Some(slot) => replace_value(slot, new),
                None => inline_insert(table, key, new),
            }
            return Ok(());
        }
        if table.get(key).is_none() {
            inline_insert(table, key, self.fresh_value(i));
        }
        let child = table.get_mut(key).expect("present or just inserted");
        self.value_at(child, i + 1)
    }

    fn array(&self, array: &mut toml_edit::Array, i: usize) -> anyhow::Result<()> {
        let len = array.len();
        let index = array_index(self.pointer, &self.tokens[i], len)?;
        if self.last(i) {
            let new = toml_value(self.value, self.pointer)?;
            match array.get_mut(index) {
                Some(slot) => replace_value(slot, new),
                None => array_push(array, new),
            }
            return Ok(());
        }
        if index == len {
            array_push(array, self.fresh_value(i));
        }
        let child = array.get_mut(index).expect("index within the array");
        self.value_at(child, i + 1)
    }

    /// A missing container below an inline value: an array when token `i + 1` is `-`.
    fn fresh_value(&self, i: usize) -> toml_edit::Value {
        if self.tokens[i + 1] == "-" {
            toml_edit::Array::new().into()
        } else {
            toml_edit::InlineTable::new().into()
        }
    }
}

/// A table harn creates: implicit (no header of its own while it holds only tables), dotted
/// inside a dotted-key table so its keys stay where the parent's are.
fn new_table(dotted: bool) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    table.set_implicit(true);
    table.set_dotted(dotted);
    table
}

/// `value` where a table holds it: a non-empty object = a table, a non-empty array of non-empty
/// objects = an array of tables (inline in a dotted-key table, which has no header to give them),
/// anything else = a value.
fn toml_item(value: &Value, dotted: bool, pointer: &str) -> anyhow::Result<toml_edit::Item> {
    match value {
        Value::Object(map) if !map.is_empty() => {
            Ok(toml_edit::Item::Table(toml_table(map, dotted, pointer)?))
        }
        Value::Array(items)
            if !dotted
                && !items.is_empty()
                && items
                    .iter()
                    .all(|v| v.as_object().is_some_and(|m| !m.is_empty())) =>
        {
            let mut tables = toml_edit::ArrayOfTables::new();
            for item in items {
                let map = item.as_object().expect("checked above");
                tables.push(toml_table(map, false, pointer)?);
            }
            Ok(toml_edit::Item::ArrayOfTables(tables))
        }
        _ => Ok(toml_edit::Item::Value(toml_value(value, pointer)?)),
    }
}

fn toml_table(
    map: &serde_json::Map<String, Value>,
    dotted: bool,
    pointer: &str,
) -> anyhow::Result<toml_edit::Table> {
    let mut table = new_table(dotted);
    for (key, value) in map {
        table.insert(key, toml_item(value, dotted, pointer)?);
    }
    Ok(table)
}

/// `value` as an inline TOML value (objects = inline tables).
fn toml_value(value: &Value, pointer: &str) -> anyhow::Result<toml_edit::Value> {
    Ok(match value {
        Value::Null => bail!("`{pointer}`: TOML has no null"),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => i.into(),
            (None, Some(_), _) => bail!("`{pointer}`: {n} is past TOML's 64-bit integer range"),
            (None, None, Some(f)) => f.into(),
            _ => bail!("`{pointer}`: {n} is not a TOML number"),
        },
        Value::String(s) => s.as_str().into(),
        Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(toml_value(item, pointer)?);
            }
            array.into()
        }
        Value::Object(map) => {
            let mut table = toml_edit::InlineTable::new();
            for (key, item) in map {
                table.insert(key, toml_value(item, pointer)?);
            }
            table.into()
        }
    })
}

fn toml_type(value: &toml_edit::Value) -> &'static str {
    match value {
        toml_edit::Value::String(_) => "a string",
        toml_edit::Value::Integer(_) => "an integer",
        toml_edit::Value::Float(_) => "a float",
        toml_edit::Value::Boolean(_) => "a boolean",
        toml_edit::Value::Datetime(_) => "a datetime",
        toml_edit::Value::Array(_) => "an array",
        toml_edit::Value::InlineTable(_) => "an inline table",
    }
}

/// `*slot = new`, keeping the old value's decor (`key = old # comment` → `key = new # comment`) or
/// the old table's place and the comments above its header.
/// `*slot = new` for a table or an array of tables at `key`. A table replacing a table keeps its
/// place and decor; any other kind change carries the comments — the lines above the header
/// and the comment after it — to where the new item is written (the key line of a value, the
/// header of a table or of the first `[[…]]` block), and resets the key's spacing to the new
/// kind's.
fn replace_item(
    key: &mut toml_edit::KeyMut<'_>,
    slot: &mut toml_edit::Item,
    mut new: toml_edit::Item,
) {
    if let (toml_edit::Item::Table(old), toml_edit::Item::Table(table)) = (&*slot, &mut new) {
        table.set_position(old.position());
        *table.decor_mut() = old.decor().clone();
        *slot = new;
        return;
    }
    let old_decor = match &*slot {
        toml_edit::Item::Table(table) => Some(table.decor().clone()),
        toml_edit::Item::ArrayOfTables(tables) => tables.get(0).map(|t| t.decor().clone()),
        toml_edit::Item::Value(value) => {
            let mut decor = value.decor().clone();
            if let Some(above) = key.leaf_decor().prefix() {
                decor.set_prefix(above.clone());
            }
            Some(decor)
        }
        toml_edit::Item::None => None,
    };
    let above = old_decor.as_ref().and_then(|d| comment(d.prefix()));
    let after = old_decor.as_ref().and_then(|d| comment(d.suffix()));
    key.leaf_decor_mut().clear();
    let decor = match &mut new {
        toml_edit::Item::Value(value) => {
            if let Some(above) = above {
                key.leaf_decor_mut().set_prefix(above);
            }
            Some(value.decor_mut())
        }
        toml_edit::Item::Table(table) => {
            if let Some(above) = above {
                table.decor_mut().set_prefix(above);
            }
            Some(table.decor_mut())
        }
        toml_edit::Item::ArrayOfTables(tables) => tables.get_mut(0).map(|first| {
            if let Some(above) = above {
                first.decor_mut().set_prefix(above);
            }
            first.decor_mut()
        }),
        toml_edit::Item::None => None,
    };
    if let (Some(decor), Some(after)) = (decor, after) {
        decor.set_suffix(after);
    }
    *slot = new;
}

/// The decor text when it holds a comment (whitespace alone is the old kind's spacing).
fn comment(raw: Option<&toml_edit::RawString>) -> Option<String> {
    raw.and_then(|r| r.as_str())
        .filter(|text| text.contains('#'))
        .map(str::to_string)
}

/// `*slot = new` keeping the slot's decor (`key = old # c` → `key = new # c`); a value equal to
/// the one there is left as written (`'lit'` stays a literal string, `1_000` keeps its `_`).
fn replace_value(slot: &mut toml_edit::Value, mut new: toml_edit::Value) {
    if toml_eq(slot, &new) {
        return;
    }
    *new.decor_mut() = slot.decor().clone();
    *slot = new;
}

/// Same TOML value, spelling aside.
fn toml_eq(a: &toml_edit::Value, b: &toml_edit::Value) -> bool {
    use toml_edit::Value as V;
    match (a, b) {
        (V::String(x), V::String(y)) => x.value() == y.value(),
        (V::Integer(x), V::Integer(y)) => x.value() == y.value(),
        (V::Float(x), V::Float(y)) => x.value() == y.value(),
        (V::Boolean(x), V::Boolean(y)) => x.value() == y.value(),
        (V::Datetime(x), V::Datetime(y)) => x.value() == y.value(),
        (V::Array(x), V::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| toml_eq(p, q))
        }
        (V::InlineTable(x), V::InlineTable(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| toml_eq(v, w)))
        }
        _ => false,
    }
}

/// Adds `key = value` after the last entry of an inline table: the space before `}` moves from
/// the last value to the new one (`{ a = 1 }` → `{ a = 1, b = 2 }`).
fn inline_insert(table: &mut toml_edit::InlineTable, key: &str, mut value: toml_edit::Value) {
    if let Some((_, last)) = table.iter_mut().last() {
        let suffix = last.decor().suffix().cloned();
        last.decor_mut().set_suffix("");
        value.decor_mut().set_prefix(" ");
        if let Some(suffix) = suffix {
            value.decor_mut().set_suffix(suffix);
        }
    }
    table.insert(key, value);
}

/// Appends `value` after the array's last element. Multi-line (a newline follows the last
/// element): on a new line with the last element's indentation — its whitespace only, never the
/// comment lines above it — and a comment after the last element stays on that element's line,
/// after its new comma. One line: after `, `, the space before `]` moving along.
fn array_push(array: &mut toml_edit::Array, mut value: toml_edit::Value) {
    let text = |raw: Option<&toml_edit::RawString>| {
        raw.and_then(|r| r.as_str()).unwrap_or_default().to_string()
    };
    let trailing_comma = array.trailing_comma();
    let trailing = array.trailing().as_str().unwrap_or_default().to_string();
    let Some(last) = array.iter_mut().last() else {
        array.push_formatted(value);
        return;
    };
    let above = text(last.decor().prefix());
    let indent: String = match above.rfind('\n') {
        Some(nl) => above[nl + 1..]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect(),
        None => String::new(),
    };
    // What follows the last element before `]`: its suffix, or after a trailing comma the
    // array's trailing text. Up to its last newline it stays on the last element's line.
    let gap = if trailing_comma {
        trailing.clone()
    } else {
        text(last.decor().suffix())
    };
    let (prefix, rest) = match gap.rfind('\n') {
        Some(nl) => (
            format!("{}{indent}", &gap[..=nl]),
            format!("\n{}", &gap[nl + 1..]),
        ),
        None => (" ".to_string(), gap.clone()),
    };
    value.decor_mut().set_prefix(prefix);
    if trailing_comma {
        array.set_trailing(rest);
    } else {
        last.decor_mut().set_suffix("");
        value.decor_mut().set_suffix(rest);
    }
    array.push_formatted(value);
}

/// Atomic replace keeping the current file's mode (a new file gets the umask default).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic_mode(path, bytes, None)
}

/// Temp file in the same directory → write → fsync → rename over `path` → fsync the directory.
/// A symlinked `path` is written through (its target is replaced, the link is kept).
pub(crate) fn write_atomic_mode(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> anyhow::Result<()> {
    let path = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::canonicalize(path).with_context(|| format!("resolve symlink {}", path.display()))?
        }
        _ => path.to_path_buf(),
    };
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let mode = mode.or_else(|| {
        fs::metadata(&path)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777)
    });
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tmp = dir.join(format!(
        ".{}.harn-{}-{}",
        file_label(&path),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(bytes)?;
        if let Some(mode) = mode {
            fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        }
        file.sync_all()?;
        fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} to {}", tmp.display(), path.display()))
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp); // harn's own temp file, never the target
    }
    written?;
    if let Ok(dir) = fs::File::open(dir) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The file name, for temp and snapshot names (`settings.local.json`).
pub(crate) fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

/// Unified diff of one file (`None` = absent, shown as `/dev/null`); empty when unchanged.
pub fn unified_diff(path: &Path, old: Option<&[u8]>, new: Option<&[u8]>) -> String {
    if old == new {
        return String::new();
    }
    let old_text = old.map(String::from_utf8_lossy).unwrap_or_default();
    let new_text = new.map(String::from_utf8_lossy).unwrap_or_default();
    let shown = path.display();
    let a = match old {
        Some(_) => format!("a{shown}"),
        None => "/dev/null".to_string(),
    };
    let b = match new {
        Some(_) => format!("b{shown}"),
        None => "/dev/null".to_string(),
    };
    similar::TextDiff::from_lines(old_text.as_ref(), new_text.as_ref())
        .unified_diff()
        .context_radius(3)
        .header(&a, &b)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set(doc: &mut Value, pointer: &str, value: Value) -> anyhow::Result<()> {
        set_json_pointer(doc, pointer, value)
    }

    #[test]
    fn pointer_creates_intermediate_objects_and_keeps_order() {
        let mut doc = json!({"b": 1, "a": 2});
        set(&mut doc, "/enabledPlugins/slack@x", json!(false)).unwrap();
        set(&mut doc, "/b", json!(3)).unwrap();
        assert_eq!(
            serde_json::to_string(&doc).unwrap(),
            r#"{"b":3,"a":2,"enabledPlugins":{"slack@x":false}}"#
        );
    }

    #[test]
    fn pointer_dash_appends_and_creates_arrays() {
        let mut doc = json!({});
        set(
            &mut doc,
            "/projects/~1repo/disabledMcpServers/-",
            json!("a"),
        )
        .unwrap();
        set(
            &mut doc,
            "/projects/~1repo/disabledMcpServers/-",
            json!("b"),
        )
        .unwrap();
        set(
            &mut doc,
            "/projects/~1repo/disabledMcpServers/0",
            json!("c"),
        )
        .unwrap();
        assert_eq!(
            doc,
            json!({"projects": {"/repo": {"disabledMcpServers": ["c", "b"]}}})
        );
    }

    #[test]
    fn pointer_refuses_root_scalars_and_bad_indexes() {
        let mut doc = json!({"model": "opus", "list": [1]});
        assert!(set(&mut doc, "", json!(1)).is_err());
        assert!(set(&mut doc, "/model/x", json!(1)).is_err());
        assert!(set(&mut doc, "/list/5", json!(1)).is_err());
        assert!(set(&mut doc, "/list/01", json!(1)).is_err());
        assert_eq!(doc, json!({"model": "opus", "list": [1]}));
    }

    #[test]
    fn escaped_tokens_round_trip() {
        let mut doc = json!({});
        set(
            &mut doc,
            &format!("/env/{}", escape_token("A/B~C")),
            json!("1"),
        )
        .unwrap();
        assert_eq!(doc, json!({"env": {"A/B~C": "1"}}));
    }

    #[test]
    fn patch_json_is_two_space_with_newline_and_refuses_garbage() {
        let out = patch_json(Some(b"{\"x\":1}"), &[("/y".into(), json!(true))]).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"x\": 1,\n  \"y\": true\n}\n"
        );
        let blank = patch_json(Some(b"  \n"), &[("/y".into(), json!(1))]).unwrap();
        assert_eq!(blank, b"{\n  \"y\": 1\n}\n");
        let err = patch_json(Some(b"{nope"), &[("/y".into(), json!(1))]).unwrap_err();
        assert!(err.is::<Refused>());
    }

    #[test]
    fn patch_json_keeps_one_line_scalar_arrays_on_one_line() {
        let source = br#"{
    "enabledMcpjsonServers": ["project-server"],
    "permissions": {"allow": ["Bash(ls:*)", "Read"], "deny": [
        "WebSearch"
    ]},
    "nums": [1,2],
    "objs": [{"a": 1}],
    "k\/ey": ["x"],
    "empty": []
}"#;
        let out = patch_json(
            Some(source),
            &[
                ("/permissions/allow/-".into(), json!("Grep")),
                ("/permissions/deny/-".into(), json!("Bash")),
                ("/nums/-".into(), json!({"n": 3})),
                ("/skillOverrides/s".into(), json!("off")),
                ("/fresh".into(), json!(["a", "b"])),
            ],
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"{
  "enabledMcpjsonServers": ["project-server"],
  "permissions": {
    "allow": ["Bash(ls:*)", "Read", "Grep"],
    "deny": [
      "WebSearch",
      "Bash"
    ]
  },
  "nums": [
    1,
    2,
    {
      "n": 3
    }
  ],
  "objs": [
    {
      "a": 1
    }
  ],
  "k/ey": ["x"],
  "empty": [],
  "skillOverrides": {
    "s": "off"
  },
  "fresh": [
    "a",
    "b"
  ]
}
"#
        );
    }

    #[test]
    fn patch_json_without_one_line_arrays_is_serde_pretty() {
        let doc = json!({
            "a": [1, [2, 3], {"b": []}, "x\"y"],
            "c": {"d": {}, "e": [null, true, 1.5]},
            "u": "é/~"
        });
        let source = serde_json::to_vec_pretty(&doc).unwrap();
        let out = patch_json(Some(&source), &[]).unwrap();
        assert_eq!(out, render_json(&doc));
    }

    #[test]
    fn manifest_path_finds_the_harn_off_root() {
        assert_eq!(
            manifest_path(Path::new("/r/.claude/harn-off/skills/foo")),
            PathBuf::from("/r/.claude/harn-off/manifest.json")
        );
        assert_eq!(
            manifest_path(Path::new("/r/elsewhere/foo")),
            PathBuf::from("/r/elsewhere/manifest.json")
        );
    }

    fn toml(source: &str, sets: &[(&str, Value)]) -> String {
        let sets: Vec<(String, Value)> = sets
            .iter()
            .map(|(p, v)| (p.to_string(), v.clone()))
            .collect();
        String::from_utf8(patch_toml(Some(source.as_bytes()), &sets).unwrap()).unwrap()
    }

    #[test]
    fn toml_set_existing_key_keeps_comments_and_order() {
        let source = "# codex config\nmodel = \"gpt-x\" # pinned\ninclude_environment_context = true  # on\n\n# trust\n[projects.\"/r\"]\ntrust_level = \"trusted\"\n";
        let out = toml(
            source,
            &[
                ("/include_environment_context", json!(false)),
                ("/developer_instructions", json!("")),
            ],
        );
        assert_eq!(
            out,
            "# codex config\nmodel = \"gpt-x\" # pinned\ninclude_environment_context = false  # on\ndeveloper_instructions = \"\"\n\n# trust\n[projects.\"/r\"]\ntrust_level = \"trusted\"\n"
        );
    }

    #[test]
    fn toml_creates_nested_tables_with_quoted_keys() {
        let out = toml(
            "model = \"m\"\n",
            &[
                ("/plugins/slack@openai-curated/enabled", json!(false)),
                (
                    "/hooks/state/~1abs~1hooks.json:session_start:0:0/enabled",
                    json!(false),
                ),
                ("/mcp_servers/a.b/enabled", json!(false)),
                ("/odd/say \"hi\"~0/n", json!(1)),
                ("/agents/enabled", json!(false)),
            ],
        );
        assert_eq!(
            out,
            "model = \"m\"\n\n[plugins.\"slack@openai-curated\"]\nenabled = false\n\n[hooks.state.\"/abs/hooks.json:session_start:0:0\"]\nenabled = false\n\n[mcp_servers.\"a.b\"]\nenabled = false\n\n[odd.'say \"hi\"~']\nn = 1\n\n[agents]\nenabled = false\n"
        );
        // A second key lands in the table the first one created.
        let out = toml(&out, &[("/plugins/slack@openai-curated/x", json!(1))]);
        assert!(
            out.contains("[plugins.\"slack@openai-curated\"]\nenabled = false\nx = 1\n"),
            "{out}"
        );
    }

    #[test]
    fn toml_appends_skills_config_blocks() {
        let entry = |p: &str| json!({"path": p, "enabled": false});
        let source = "[skills]\ninclude_instructions = true\n\n[other]\nk = 1\n";
        let once = toml(source, &[("/skills/config/-", entry("/a/SKILL.md"))]);
        let twice = toml(&once, &[("/skills/config/-", entry("/b/SKILL.md"))]);
        assert_eq!(
            twice,
            "[skills]\ninclude_instructions = true\n\n[[skills.config]]\npath = \"/a/SKILL.md\"\nenabled = false\n\n[[skills.config]]\npath = \"/b/SKILL.md\"\nenabled = false\n\n[other]\nk = 1\n"
        );
        // No `[skills]` yet: only the `[[skills.config]]` header is written.
        assert_eq!(
            toml("", &[("/skills/config/-", entry("/a/SKILL.md"))]),
            "[[skills.config]]\npath = \"/a/SKILL.md\"\nenabled = false\n"
        );
        // An index replaces that block; a scalar is refused in an array of tables.
        let replaced = toml(&twice, &[("/skills/config/0/enabled", json!(true))]);
        assert!(replaced.contains("path = \"/a/SKILL.md\"\nenabled = true\n"));
        let err = patch_toml(
            Some(twice.as_bytes()),
            &[("/skills/config/-".into(), json!("x"))],
        )
        .unwrap_err();
        assert!(err.to_string().contains("only an object"), "{err}");
    }

    #[test]
    fn toml_inline_parents_stay_inline() {
        let source = "plugins = { \"a@b\" = { enabled = true } }\nskills = { config = [{ path = \"/a\", enabled = false }] }\nempty = {}\n";
        let out = toml(
            source,
            &[
                ("/plugins/x@y/enabled", json!(false)),
                ("/plugins/a@b/enabled", json!(false)),
                ("/skills/config/-", json!({"path": "/b", "enabled": false})),
                ("/empty/k", json!([1, "s"])),
            ],
        );
        assert_eq!(
            out,
            "plugins = { \"a@b\" = { enabled = false }, \"x@y\" = { enabled = false } }\nskills = { config = [{ path = \"/a\", enabled = false }, { path = \"/b\", enabled = false }] }\nempty = { k = [1, \"s\"] }\n"
        );
    }

    #[test]
    fn toml_dotted_parents_stay_dotted() {
        let out = toml(
            "skills.bundled.enabled = true\n[other]\nk = 1\n",
            &[
                ("/skills/bundled/enabled", json!(false)),
                ("/skills/include_instructions", json!(false)),
                ("/skills/x/y", json!({"z": 1})),
            ],
        );
        assert_eq!(
            out,
            "skills.bundled.enabled = false\nskills.include_instructions = false\nskills.x.y.z = 1\n[other]\nk = 1\n"
        );
    }

    #[test]
    fn toml_array_appends_follow_the_array_style() {
        let out = toml(
            "a = [\"x\"]\nb = [\n  \"x\",\n]\nc = [ \"x\" ]\nd = []\n",
            &[
                ("/a/-", json!("y")),
                ("/b/-", json!("y")),
                ("/c/-", json!("y")),
                ("/d/-", json!("y")),
                ("/e/-", json!("y")),
                ("/a/0", json!("w")),
            ],
        );
        assert_eq!(
            out,
            "a = [\"w\", \"y\"]\nb = [\n  \"x\",\n  \"y\",\n]\nc = [ \"x\", \"y\" ]\nd = [\"y\"]\ne = [\"y\"]\n"
        );
    }

    #[test]
    fn toml_values_map_from_json() {
        let out = toml(
            "",
            &[
                ("/s", json!("a\"b")),
                ("/i", json!(-3)),
                ("/f", json!(1.5)),
                ("/b", json!(true)),
                ("/arr", json!([1, [2], {"k": "v"}])),
                ("/o", json!({"k": 1, "inner": {"j": 2}})),
                ("/aot", json!([{"n": 1}, {"n": 2}])),
                ("/e", json!({})),
            ],
        );
        assert_eq!(
            out,
            "s = 'a\"b'\ni = -3\nf = 1.5\nb = true\narr = [1, [2], { k = \"v\" }]\ne = {}\n\n[o]\nk = 1\n\n[o.inner]\nj = 2\n\n[[aot]]\nn = 1\n\n[[aot]]\nn = 2\n"
        );
    }

    #[test]
    fn toml_new_file_and_refusals() {
        let created = patch_toml(None, &[("/include_apps_instructions".into(), json!(false))]);
        assert_eq!(created.unwrap(), b"include_apps_instructions = false\n");
        let unchanged = patch_toml(Some(b"# only a comment\nx = 1"), &[]).unwrap();
        assert_eq!(unchanged, b"# only a comment\nx = 1");
        let no_final_newline = patch_toml(Some(b"x = 1 # c"), &[("/y".into(), json!(2))]);
        assert_eq!(no_final_newline.unwrap(), b"x = 1 # c\ny = 2");

        let garbage = patch_toml(Some(b"x = "), &[("/y".into(), json!(1))]).unwrap_err();
        assert!(garbage.is::<Refused>(), "{garbage}");
        let binary = patch_toml(Some(b"x = \"\xff\""), &[("/y".into(), json!(1))]).unwrap_err();
        assert!(binary.is::<Refused>(), "{binary}");
        for (pointer, value, says) in [
            ("/y", json!(null), "TOML has no null"),
            ("/y", json!(u64::MAX), "64-bit"),
            ("/x/y", json!(1), "/x is an integer, not a table or array"),
            ("/a/5", json!(1), "not an index"),
            ("", json!(1), "must start with `/`"),
        ] {
            let err =
                patch_toml(Some(b"x = 1\na = [0]\n"), &[(pointer.into(), value)]).unwrap_err();
            assert!(err.to_string().contains(says), "{pointer}: {err}");
        }
    }

    /// `/a/-/-` appends an array to `a`: an inline array takes it (as the JSON writer does), an
    /// array of tables cannot hold one and refuses instead of inventing a `-` key.
    #[test]
    fn toml_append_of_an_array_follows_the_array_kind() {
        assert_eq!(
            toml("a = [{ x = 1 }]\n", &[("/a/-/-", json!(false))]),
            "a = [{ x = 1 }, [false]]\n"
        );
        let mut json_view = json!({"a": [{"x": 1}]});
        set_json_pointer(&mut json_view, "/a/-/-", json!(false)).unwrap();
        assert_eq!(json_view, json!({"a": [{"x": 1}, [false]]}));
        let err =
            patch_toml(Some(b"[[a]]\nx = 1\n"), &[("/a/-/-".into(), json!(false))]).unwrap_err();
        assert!(err.to_string().contains("array of tables"), "{err}");
        // A key after `-` is a new block, as before.
        assert_eq!(
            toml("[[a]]\nx = 1\n", &[("/a/-/x", json!(2))]),
            "[[a]]\nx = 1\n\n[[a]]\nx = 2\n"
        );
    }

    /// A replacement of another kind carries the comments of what it replaces: the lines above
    /// and the comment at the end of the header or key line.
    #[test]
    fn toml_kind_changes_keep_comments() {
        let cases = [
            // table → value: the value joins the root keys with the header's comments.
            (
                "y = 1\n\n# above a\n[a] # retain this\nx = 1\n",
                "/a",
                json!(5),
                "y = 1\n\n# above a\na = 5 # retain this\n",
            ),
            // value → object: stays a value on its line (an inline table), comments kept.
            (
                "# above x\nx = 1 # trail x\n",
                "/x",
                json!({"k": 1}),
                "# above x\nx = { k = 1 } # trail x\n",
            ),
            // array of tables → value.
            (
                "# above b\n[[b]] # b0\nz = 1\n",
                "/b",
                json!(2),
                "# above b\nb = 2 # b0\n",
            ),
            // table → array of tables, and back.
            (
                "# above a\n[a] # c\nx = 1\n",
                "/a",
                json!([{"x": 2}]),
                "# above a\n[[a]] # c\nx = 2\n",
            ),
            (
                "# above a\n[[a]] # c\nx = 1\n",
                "/a",
                json!({"x": 2}),
                "# above a\n[a] # c\nx = 2\n",
            ),
            // value in a dotted-key table → object: stays on its line.
            (
                "t.x = 1 # c\n",
                "/t/x",
                json!({"k": 1}),
                "t.x = { k = 1 } # c\n",
            ),
        ];
        for (source, pointer, value, expected) in cases {
            assert_eq!(toml(source, &[(pointer, value)]), expected, "{source}");
        }
    }

    /// An append takes the indentation of the last element's line, never its comments; a
    /// comment after the last element stays on that element's line.
    #[test]
    fn toml_array_append_never_copies_comments() {
        let cases = [
            (
                "a = [\n  # explanation for x\n  \"x\",\n]\n",
                "a = [\n  # explanation for x\n  \"x\",\n  \"y\",\n]\n",
            ),
            (
                "a = [\n  \"x\" # about x\n]\n",
                "a = [\n  \"x\", # about x\n  \"y\"\n]\n",
            ),
            (
                "a = [\n  \"x\", # about x\n]\n",
                "a = [\n  \"x\", # about x\n  \"y\",\n]\n",
            ),
            (
                "a = [\n    # one\n    \"w\",\n    # two\n    \"x\"\n]\n",
                "a = [\n    # one\n    \"w\",\n    # two\n    \"x\",\n    \"y\"\n]\n",
            ),
        ];
        for (source, expected) in cases {
            let out = toml(source, &[("/a/-", json!("y"))]);
            assert_eq!(out, expected, "{source}");
            assert_eq!(
                out.matches('#').count(),
                source.matches('#').count(),
                "{out}"
            );
        }
    }

    /// Setting a value equal to the one there keeps its spelling (literal vs basic string,
    /// digit separators); a different value is written in TOML's default form.
    #[test]
    fn toml_equal_values_keep_their_spelling() {
        let source = "a = 'lit' # c\nb = \"basic\"\nn = 1_000\nl = [ 'x' ]\nt = { s = 'v' }\n";
        let out = toml(
            source,
            &[
                ("/a", json!("lit")),
                ("/b", json!("basic")),
                ("/n", json!(1000)),
                ("/l/0", json!("x")),
                ("/t/s", json!("v")),
            ],
        );
        assert_eq!(out, source);
        assert_eq!(
            toml(source, &[("/a", json!("new"))]),
            source.replace("'lit'", "\"new\"")
        );
    }

    #[test]
    fn patch_file_picks_the_format_by_extension() {
        let sets = [("/k".to_string(), json!(false))];
        assert_eq!(
            patch_file(Path::new("/x/config.toml"), None, &sets).unwrap(),
            b"k = false\n"
        );
        assert_eq!(
            patch_file(Path::new("/x/settings.json"), None, &sets).unwrap(),
            b"{\n  \"k\": false\n}\n"
        );
    }

    #[test]
    fn unified_diff_marks_new_files() {
        let d = unified_diff(Path::new("/x/s.json"), None, Some(b"{}\n"));
        assert!(d.starts_with("--- /dev/null\n+++ b/x/s.json\n"), "{d}");
        assert!(unified_diff(Path::new("/x"), Some(b"a"), Some(b"a")).is_empty());
    }
}
