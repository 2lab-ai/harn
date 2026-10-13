//! Settings layering (`research/01` §2). Precedence, lowest first: `~/.claude/settings.json` →
//! `<dir>/.claude/settings.json` → `<dir>/.claude/settings.local.json` → managed
//! (`managed-settings.json` + `managed-settings.d/*.json`). Scalars: the highest layer wins.
//! Lists (`permissions.*`, `claudeMdExcludes`, `hooks.<event>`, …) concatenate across layers.
//! Every effective value is returned with the file that set it, so origins read
//! `"<file>#<json-pointer>"`. `--settings` and other CLI flags are not static facts and are not
//! modelled.

use super::fs::{Fs, Json};
use crate::model::{Layer, Scope};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    User,
    Project,
    Local,
    Managed,
}

impl Source {
    pub fn layer(self) -> Layer {
        match self {
            Source::User => Layer::User,
            Source::Project => Layer::Project,
            Source::Local => Layer::Local,
            Source::Managed => Layer::Managed,
        }
    }
}

pub(crate) struct File {
    pub source: Source,
    pub path: PathBuf,
    pub value: Value,
}

impl File {
    /// `"<file>#<pointer>"`.
    pub fn at(&self, pointer: &str) -> String {
        format!("{}#{pointer}", self.path.display())
    }
}

pub(crate) struct Settings {
    /// Lowest precedence first.
    pub files: Vec<File>,
}

/// Managed settings / CLAUDE.md / MCP directories (macOS, Linux).
pub(crate) const MANAGED_DIRS: &[&str] = &[
    "/Library/Application Support/ClaudeCode",
    "/etc/claude-code",
];

pub(crate) fn settings_path(dir: &Path, home: &Path, scope: Scope) -> PathBuf {
    match scope {
        Scope::Local => dir.join(".claude/settings.local.json"),
        Scope::Project => dir.join(".claude/settings.json"),
        Scope::User => home.join(".claude/settings.json"),
    }
}

impl Settings {
    pub fn load(fs: &Fs, dir: &Path, home: &Path, warnings: &mut Vec<String>) -> Settings {
        let mut candidates = vec![
            (Source::User, settings_path(dir, home, Scope::User)),
            (Source::Project, settings_path(dir, home, Scope::Project)),
            (Source::Local, settings_path(dir, home, Scope::Local)),
        ];
        for managed in MANAGED_DIRS {
            let base = Path::new(managed);
            candidates.push((Source::Managed, base.join("managed-settings.json")));
            let drop_in = base.join("managed-settings.d");
            for entry in fs.read_dir(&drop_in) {
                if entry.extension().is_some_and(|e| e == "json") {
                    candidates.push((Source::Managed, entry));
                }
            }
        }
        let mut files = Vec::new();
        for (source, path) in candidates {
            match fs.read_json(&path) {
                Json::Ok(value) if value.is_object() => files.push(File {
                    source,
                    path: fs.realpath(&path),
                    value,
                }),
                Json::Ok(_) => {
                    warnings.push(format!("{}: not a JSON object, ignored", path.display()))
                }
                Json::Invalid(err) => {
                    warnings.push(format!("{}: invalid JSON, ignored ({err})", path.display()))
                }
                Json::Missing => {}
            }
        }
        Settings { files }
    }

    /// The effective value at `pointer` and the file that set it (highest precedence wins).
    pub fn get(&self, pointer: &str) -> Option<(&Value, &File)> {
        self.files
            .iter()
            .rev()
            .find_map(|f| f.value.pointer(pointer).map(|v| (v, f)))
    }

    pub fn get_bool(&self, pointer: &str) -> Option<(bool, &File)> {
        self.get(pointer)
            .and_then(|(v, f)| v.as_bool().map(|b| (b, f)))
    }

    pub fn get_str(&self, pointer: &str) -> Option<(&str, &File)> {
        self.get(pointer)
            .and_then(|(v, f)| v.as_str().map(|s| (s, f)))
    }

    /// Every element of the array at `pointer` across layers, lowest first, with its file and
    /// index.
    pub fn list(&self, pointer: &str) -> Vec<(&Value, &File, usize)> {
        let mut out = Vec::new();
        for f in &self.files {
            if let Some(Value::Array(items)) = f.value.pointer(pointer) {
                out.extend(items.iter().enumerate().map(|(i, v)| (v, f, i)));
            }
        }
        out
    }

    /// Strings of [`Self::list`].
    pub fn strings(&self, pointer: &str) -> Vec<(String, String)> {
        self.list(pointer)
            .into_iter()
            .filter_map(|(v, f, i)| {
                v.as_str()
                    .map(|s| (s.to_string(), f.at(&format!("{pointer}/{i}"))))
            })
            .collect()
    }

    /// `env` from settings (highest layer per key).
    pub fn env(&self, name: &str) -> Option<(String, &File)> {
        self.get(&format!("/env/{}", super::fs::esc(name)))
            .and_then(|(v, f)| match v {
                Value::String(s) => Some((s.clone(), f)),
                Value::Bool(b) => Some((b.to_string(), f)),
                Value::Number(n) => Some((n.to_string(), f)),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(source: Source, value: Value) -> File {
        File {
            source,
            path: PathBuf::from(format!("/{source:?}.json")),
            value,
        }
    }

    #[test]
    fn highest_layer_wins_and_lists_concatenate() {
        let s = Settings {
            files: vec![
                file(
                    Source::User,
                    json!({"enabledPlugins": {"a@m": true}, "permissions": {"allow": ["A"]}}),
                ),
                file(Source::Project, json!({"enabledPlugins": {"a@m": true}})),
                file(
                    Source::Local,
                    json!({"enabledPlugins": {"a@m": false}, "permissions": {"allow": ["B", "C"]}}),
                ),
            ],
        };
        let (enabled, from) = s.get_bool("/enabledPlugins/a@m").unwrap();
        assert!(!enabled);
        assert_eq!(from.source, Source::Local);
        assert_eq!(
            from.at("/enabledPlugins/a@m"),
            "/Local.json#/enabledPlugins/a@m"
        );
        let allow: Vec<String> = s
            .strings("/permissions/allow")
            .into_iter()
            .map(|(v, _)| v)
            .collect();
        assert_eq!(allow, vec!["A", "B", "C"]);
    }
}
