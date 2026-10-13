//! Writes (WU-E): atomic replace (temp + fsync + rename); JSON via serde_json with
//! `preserve_order`, 2-space; TOML via toml_edit; quarantine moves to
//! `<discovery-root-parent>/harn-off/<kind>/<name>` + `manifest.json`. Every write goes through
//! the journal (snapshot first).
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

    // One patch per distinct JSON file, in first-appearance order. Each file is read once: the
    // stale check, the patch and the snapshot all use the same bytes.
    let mut json_files: Vec<(PathBuf, Vec<(String, Value)>)> = Vec::new();
    for step in &steps {
        if let Step::Json {
            file,
            pointer,
            value,
        } = step
        {
            let set = (pointer.clone(), value.clone());
            match json_files.iter_mut().find(|(f, _)| f == file) {
                Some((_, sets)) => sets.push(set),
                None => json_files.push((file.clone(), vec![set])),
            }
        }
    }
    let mut current: Vec<Option<Vec<u8>>> = Vec::new();
    for (file, _) in &json_files {
        current.push(read_optional(file)?);
    }

    // Stale check against the hashes the plan was built on: every file, before any write.
    let mut stale: Vec<PathBuf> = Vec::new();
    for (op, step) in plan.ops.iter().zip(&steps) {
        let (path, now) = match step {
            Step::Json { file, .. } => {
                let i = json_files
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
    for ((file, sets), current) in json_files.iter().zip(current) {
        let new =
            patch_json(current.as_deref(), sets).with_context(|| file.display().to_string())?;
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
            done.push(Done::Json(file.clone(), current.is_some()));
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
    Json {
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
        Knob::Setting { pointer, value, .. } => Step::Json {
            file: file()?,
            pointer: pointer.clone(),
            value: value.clone(),
        },
        Knob::Env { name, value, .. } => Step::Json {
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
    Json(PathBuf, bool),
    Move(PathBuf, PathBuf),
}

fn rollback(entry: &JournalEntry, done: &[Done], entry_dir: &Path) -> anyhow::Result<()> {
    for step in done.iter().rev() {
        match step {
            Done::Json(file, existed) => {
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
    let Some(rest) = pointer.strip_prefix('/') else {
        bail!(
            "JSON pointer `{pointer}` must start with `/` (the whole document is never replaced)"
        );
    };
    let tokens: Vec<String> = rest.split('/').map(unescape_token).collect();
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
                let index = if token == "-" {
                    len
                } else {
                    token
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n <= len && (token == "0" || !token.starts_with('0')))
                        .ok_or_else(|| {
                            anyhow!("`{pointer}`: `{token}` is not an index of an array of {len}")
                        })?
                };
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

    #[test]
    fn unified_diff_marks_new_files() {
        let d = unified_diff(Path::new("/x/s.json"), None, Some(b"{}\n"));
        assert!(d.starts_with("--- /dev/null\n+++ b/x/s.json\n"), "{d}");
        assert!(unified_diff(Path::new("/x"), Some(b"a"), Some(b"a")).is_empty());
    }
}
