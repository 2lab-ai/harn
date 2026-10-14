//! Overlay-aware file access. Every read the Claude Code resolver makes goes through [`Fs`], so an
//! [`Overlay`] (the virtual change `plan::annotate_freed` re-resolves with) is honoured everywhere:
//! `RemovePath` hides a file or a whole directory from `exists`, `read*` and `read_dir`;
//! `SetJson` patches a JSON file's parsed value before any consumer sees it (a missing file
//! patched by the overlay reads as `{}` plus the patch, the way `harn off` would create it).
//! Paths are compared after normalisation (realpath of the longest existing prefix), so
//! `/var/…` and `/private/var/…` or a symlinked skill dir and its target name the same thing.

use crate::model::{Overlay, OverlayOp};
use serde_json::{Map, Value};
use std::path::{Component, Path, PathBuf};

pub(crate) enum Json {
    Missing,
    Ok(Value),
    Invalid(String),
}

pub(crate) struct Fs {
    removed: Vec<PathBuf>,
    patches: Vec<(PathBuf, String, Value)>,
}

impl Fs {
    pub fn new(overlay: Option<&Overlay>) -> Fs {
        let mut removed = Vec::new();
        let mut patches = Vec::new();
        for op in overlay.map(|o| o.ops.as_slice()).unwrap_or_default() {
            match op {
                OverlayOp::RemovePath { path } => removed.push(normalize(path)),
                OverlayOp::SetJson {
                    file,
                    pointer,
                    value,
                } => patches.push((normalize(file), pointer.clone(), value.clone())),
            }
        }
        Fs { removed, patches }
    }

    fn is_removed(&self, path: &Path) -> bool {
        if self.removed.is_empty() {
            return false;
        }
        let norm = normalize(path);
        self.removed.iter().any(|r| norm.starts_with(r))
    }

    fn is_patched(&self, path: &Path) -> bool {
        if self.patches.is_empty() {
            return false;
        }
        let norm = normalize(path);
        self.patches.iter().any(|(file, _, _)| *file == norm)
    }

    pub fn is_file(&self, path: &Path) -> bool {
        !self.is_removed(path) && (path.is_file() || self.is_patched(path))
    }

    pub fn is_dir(&self, path: &Path) -> bool {
        !self.is_removed(path) && path.is_dir()
    }

    /// `std::fs::metadata` (symlinks followed); `NotFound` for an overlay-removed path.
    pub fn metadata(&self, path: &Path) -> std::io::Result<std::fs::Metadata> {
        if self.is_removed(path) {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        std::fs::metadata(path)
    }

    pub fn read(&self, path: &Path) -> Option<Vec<u8>> {
        if self.is_removed(path) {
            return None;
        }
        std::fs::read(path).ok()
    }

    pub fn read_string(&self, path: &Path) -> Option<String> {
        self.read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn len(&self, path: &Path) -> Option<u64> {
        if self.is_removed(path) {
            return None;
        }
        std::fs::metadata(path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len())
    }

    /// Entries of `dir`, sorted by file name, minus overlay-removed ones.
    pub fn read_dir(&self, dir: &Path) -> Vec<PathBuf> {
        if self.is_removed(dir) {
            return Vec::new();
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| !self.is_removed(p))
            .collect();
        out.sort();
        out
    }

    /// `*.md` files under `root`, recursively, sorted; skips dot-dirs and `node_modules`.
    pub fn walk_md(&self, root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        self.walk_md_into(root, 0, &mut out);
        out.sort();
        out
    }

    fn walk_md_into(&self, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 8 {
            return;
        }
        for entry in self.read_dir(dir) {
            let name = file_name(&entry);
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            if entry.is_dir() {
                self.walk_md_into(&entry, depth + 1, out);
            } else if name.ends_with(".md") && entry.is_file() {
                out.push(entry);
            }
        }
    }

    /// Realpath when the file exists, else the normalised path.
    pub fn realpath(&self, path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| normalize(path))
    }

    /// Parsed JSON with every `SetJson` patch for this file applied.
    pub fn read_json(&self, path: &Path) -> Json {
        if self.is_removed(path) {
            return Json::Missing;
        }
        let patches: Vec<&(PathBuf, String, Value)> = if self.patches.is_empty() {
            Vec::new()
        } else {
            let norm = normalize(path);
            self.patches.iter().filter(|(f, _, _)| *f == norm).collect()
        };
        let mut value = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => v,
                Err(e) => return Json::Invalid(e.to_string()),
            },
            Err(_) if !patches.is_empty() => Value::Object(Map::new()),
            Err(_) => return Json::Missing,
        };
        for (_, pointer, patch) in patches {
            set_pointer(&mut value, pointer, patch.clone());
        }
        Json::Ok(value)
    }
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `.`/`..` folded lexically, symlinks kept (the path as Claude Code prints it).
pub(crate) fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Realpath of the longest existing prefix + the rest, with `.`/`..` folded lexically.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other.as_os_str()),
        }
    }
    let mut existing = lexical.clone();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&existing) {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return lexical,
        }
    }
}

/// RFC 6901 token escape (`~` → `~0`, `/` → `~1`).
pub(crate) fn esc(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn unesc(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// Sets `pointer` in `root`, creating missing objects; a final `-` appends to an array.
pub(crate) fn set_pointer(root: &mut Value, pointer: &str, value: Value) {
    if pointer.is_empty() {
        *root = value;
        return;
    }
    let tokens: Vec<String> = pointer
        .trim_start_matches('/')
        .split('/')
        .map(unesc)
        .collect();
    let mut cur = root;
    for (i, token) in tokens.iter().enumerate() {
        let last = i + 1 == tokens.len();
        let next_is_append = tokens.get(i + 1).is_some_and(|t| t == "-");
        if !cur.is_object() && !cur.is_array() {
            *cur = Value::Object(Map::new());
        }
        if let Value::Array(items) = cur {
            if token == "-" {
                items.push(value);
                return;
            }
            let Ok(index) = token.parse::<usize>() else {
                return;
            };
            if index >= items.len() {
                if last {
                    items.push(value);
                }
                return;
            }
            if last {
                items[index] = value;
                return;
            }
            cur = &mut items[index];
            continue;
        }
        let Value::Object(map) = cur else {
            return;
        };
        if last {
            map.insert(token.clone(), value);
            return;
        }
        let empty = if next_is_append {
            Value::Array(Vec::new())
        } else {
            Value::Object(Map::new())
        };
        let entry = map.entry(token.clone()).or_insert(empty);
        if next_is_append && !entry.is_array() {
            *entry = Value::Array(Vec::new());
        }
        cur = entry;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_pointer_creates_objects_and_appends() {
        let mut v = json!({});
        set_pointer(&mut v, "/enabledPlugins/a@b", json!(false));
        set_pointer(&mut v, "/permissions/deny/-", json!("WebSearch"));
        set_pointer(&mut v, "/permissions/deny/-", json!("Bash"));
        set_pointer(&mut v, "/projects/~1x~1y/disabledMcpServers/-", json!("s"));
        assert_eq!(
            v,
            json!({
                "enabledPlugins": {"a@b": false},
                "permissions": {"deny": ["WebSearch", "Bash"]},
                "projects": {"/x/y": {"disabledMcpServers": ["s"]}}
            })
        );
    }

    #[test]
    fn set_pointer_overwrites_scalars() {
        let mut v = json!({"autoMemoryEnabled": true, "attribution": "x"});
        set_pointer(&mut v, "/autoMemoryEnabled", json!(false));
        set_pointer(&mut v, "/attribution", json!({"commit": "", "pr": ""}));
        assert_eq!(
            v,
            json!({"autoMemoryEnabled": false, "attribution": {"commit": "", "pr": ""}})
        );
    }

    #[test]
    fn normalize_keeps_a_missing_tail() {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let p = tmp.join("harn-missing-dir/x/../settings.json");
        assert_eq!(normalize(&p), tmp.join("harn-missing-dir/settings.json"));
    }

    #[test]
    fn esc_round_trips() {
        assert_eq!(esc("/a~b"), "~1a~0b");
        assert_eq!(unesc(&esc("/a~b")), "/a~b");
    }
}
