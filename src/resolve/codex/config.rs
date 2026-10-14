//! `config.toml` layering and the project trust context (`research/02` §0.3, §2), as codex-cli
//! 0.160.1 builds them (`codex-rs/config/src/loader/mod.rs` at `rust-v0.160.1`, cited below as
//! `loader:<line>`). Layers, lowest first:
//!
//! 1. packaged defaults (`config/defaults.toml:1-14`; only the keys the request depends on);
//! 2. system `/etc/codex/config.toml` (`loader:79`, `loader:266-279`);
//! 3. user `$CODEX_HOME/config.toml` (`loader:286-321`);
//! 4. profile `$CODEX_HOME/<name>.config.toml`, only when `--profile <name>` is passed
//!    (`loader:323-333`, `cli/src/lib.rs:64`). Codex reads no environment variable for it, so
//!    harn resolves without a profile unless `HARN_CODEX_PROFILE=<name>` is in the target's
//!    environment — harn's own switch, named so it cannot be mistaken for a Codex one;
//! 5. project `<dir>/.codex/config.toml` for every dir from the project root down to the target
//!    (closest wins; `loader:1639-1700`), each loaded but **disabled unless its trust decision is
//!    explicitly `trusted`** (`loader:1086-1104`), with the project denylist removed
//!    (`loader:88-101`, `loader:1137-1153`); a `.codex` dir that is `CODEX_HOME` is skipped
//!    (`loader:1683-1685`);
//! 6. legacy managed `/etc/codex/managed_config.toml`, pushed last = highest (`loader:427-447`).
//!
//! Not modelled (no static fact or not a file): `-c`/`--enable` session flags, the enterprise
//! cloud bundle, thread config layers, macOS MDM managed preferences (`loader:448-465`).
//!
//! Every file is read through [`Fs`] (`RemovePath` hides it) and viewed as JSON with the overlay's
//! `SetJson` patches applied, so `plan` computes freed bytes by re-resolving: a patch on a missing
//! file reads as `{}` plus the patch, the way `harn off` would create it. A JSON `null` written by
//! a patch reads as "key absent" (TOML has no null).
//!
//! Trust (`projects."<path>".trust_level`, read from the defaults, system, user, profile and
//! managed layers — never from project layers; `loader:336-356`, `loader:1249-1268`):
//! - a project layer in dir `D` is enabled iff the first entry found for `D`, else for the project
//!   root, else for the main repository root, is `trusted` (`loader:1041-1084`);
//! - the AGENTS.md chain is skipped iff the active project — the entry for the target dir, else for
//!   the main repository root, with no project-root step — is `untrusted`
//!   (`config/src/config_toml.rs:884-898`, `core/src/config/mod.rs:3459-3465`,
//!   `core/src/agents_md.rs:63-66`);
//! - each lookup tries the canonical path, then the path as given (`loader:1378-1394`).

use crate::model::{Overlay, OverlayOp};
use crate::resolve::claude::fs::{self, Fs};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// `/etc/codex` on Unix (`loader:79`, `loader:117`): `config.toml` (system layer) and
/// `managed_config.toml` (legacy managed layer).
pub(crate) const SYSTEM_DIR: &str = "/etc/codex";

/// harn's own environment switch for Codex's `--profile <name>` launch flag.
pub const PROFILE_ENV: &str = "HARN_CODEX_PROFILE";

/// The packaged defaults that change the request (`config/defaults.toml:1-14`).
const DEFAULTS_TOML: &str = r#"include_permissions_instructions = true
include_apps_instructions = true
include_collaboration_mode_instructions = true
include_environment_context = true
project_doc_max_bytes = 32768
project_doc_fallback_filenames = []
project_root_markers = [".git"]
"#;

/// Keys a project `.codex/config.toml` may not set (`loader:88-101`).
pub(crate) const PROJECT_DENYLIST: &[&str] = &[
    "openai_base_url",
    "chatgpt_base_url",
    "apps_mcp_product_sku",
    "responses_api_metadata",
    "model_provider",
    "model_providers",
    "notify",
    "profile",
    "profiles",
    "experimental_realtime_webrtc_call_base_url",
    "experimental_realtime_ws_base_url",
    "otel",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Defaults,
    System,
    User,
    Profile,
    Project,
    Managed,
}

pub(crate) struct LayerFile {
    pub source: Source,
    /// None for the packaged defaults.
    pub path: Option<PathBuf>,
    pub value: Value,
    /// Why Codex skips this layer: a project layer whose dir is not explicitly trusted. A
    /// disabled layer stays listed (its `.codex/skills` still load, `research/02` §3).
    pub disabled: Option<String>,
}

impl LayerFile {
    /// `"<file>#<pointer>"` (`"codex defaults#<pointer>"` for the packaged defaults).
    pub fn at(&self, pointer: &str) -> String {
        match &self.path {
            Some(path) => format!("{}#{pointer}", path.display()),
            None => format!("codex defaults#{pointer}"),
        }
    }

    pub fn enabled(&self) -> bool {
        self.disabled.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustLevel {
    Trusted,
    Untrusted,
}

impl TrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustLevel::Trusted => "trusted",
            TrustLevel::Untrusted => "untrusted",
        }
    }
}

/// One trust lookup: the level found (None = no entry) and the `projects` key that decided it
/// (when None, the key Codex tells the user to add).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub level: Option<TrustLevel>,
    pub key: String,
    /// `"<file>#<pointer>"` of the entry, when one was found.
    pub set_at: Option<String>,
}

impl Decision {
    pub fn is_trusted(&self) -> bool {
        self.level == Some(TrustLevel::Trusted)
    }

    pub fn is_untrusted(&self) -> bool {
        self.level == Some(TrustLevel::Untrusted)
    }
}

pub(crate) struct Config {
    /// Lowest precedence first; disabled project layers included.
    pub layers: Vec<LayerFile>,
    pub profile: Option<String>,
    /// The config/trust project root: nearest ancestor with a marker, where a `.git` dir counts
    /// only when it holds `HEAD` (`loader:1491-1527`); the target dir when none.
    pub project_root: PathBuf,
    /// The main repository root (a linked worktree's main checkout), for trust lookups.
    pub repo_root: Option<PathBuf>,
    /// The AGENTS.md chain root: nearest ancestor where any marker path exists
    /// (`core/src/agents_md.rs:200-221`); None = the target dir only.
    pub agents_root: Option<PathBuf>,
    /// The trust decision for the target dir's own project layer.
    pub dir_decision: Decision,
    /// The active project (the AGENTS.md gate).
    pub active_project: Decision,
}

enum Toml {
    Missing,
    Ok(Value),
    Invalid(String),
}

struct Reader<'a> {
    fs: &'a Fs,
    patches: Vec<(PathBuf, String, Value)>,
}

impl Reader<'_> {
    fn patched(&self, path: &Path) -> bool {
        if self.patches.is_empty() {
            return false;
        }
        let norm = fs::normalize(path);
        self.patches.iter().any(|(file, _, _)| *file == norm)
    }

    fn read(&self, path: &Path) -> Toml {
        let patches: Vec<&(PathBuf, String, Value)> = if self.patches.is_empty() {
            Vec::new()
        } else {
            let norm = fs::normalize(path);
            self.patches.iter().filter(|(f, _, _)| *f == norm).collect()
        };
        let mut value = match self.fs.read(path) {
            Some(bytes) => match parse(&String::from_utf8_lossy(&bytes)) {
                Ok(v) => v,
                Err(e) => return Toml::Invalid(e),
            },
            None if !patches.is_empty() && self.fs.is_file(path) => Value::Object(Map::new()),
            None => return Toml::Missing,
        };
        for (_, pointer, patch) in patches {
            fs::set_pointer(&mut value, pointer, patch.clone());
        }
        Toml::Ok(value)
    }

    /// A non-project layer: missing → None; invalid → a warning (Codex refuses to start).
    fn layer(&self, source: Source, path: &Path, warnings: &mut Vec<String>) -> Option<LayerFile> {
        match self.read(path) {
            Toml::Ok(value) if value.is_object() => Some(LayerFile {
                source,
                path: Some(self.fs.realpath(path)),
                value,
                disabled: None,
            }),
            Toml::Ok(_) | Toml::Missing => None,
            Toml::Invalid(err) => {
                warnings.push(format!(
                    "{}: invalid TOML, codex refuses to start with it ({err})",
                    path.display()
                ));
                None
            }
        }
    }
}

/// A TOML document as JSON: tables and inline tables → objects, arrays of tables → arrays,
/// datetimes → strings.
pub(crate) fn parse(text: &str) -> Result<Value, String> {
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string().trim().replace('\n', " "))?;
    Ok(table_json(doc.as_table()))
}

fn table_json(table: &toml_edit::Table) -> Value {
    let mut map = Map::new();
    for (key, item) in table.iter() {
        if let Some(v) = item_json(item) {
            map.insert(key.to_string(), v);
        }
    }
    Value::Object(map)
}

fn item_json(item: &toml_edit::Item) -> Option<Value> {
    match item {
        toml_edit::Item::None => None,
        toml_edit::Item::Value(v) => Some(value_json(v)),
        toml_edit::Item::Table(t) => Some(table_json(t)),
        toml_edit::Item::ArrayOfTables(a) => Some(Value::Array(a.iter().map(table_json).collect())),
    }
}

fn value_json(value: &toml_edit::Value) -> Value {
    match value {
        toml_edit::Value::String(s) => Value::String(s.value().clone()),
        toml_edit::Value::Integer(i) => Value::from(*i.value()),
        toml_edit::Value::Float(f) => Value::from(*f.value()),
        toml_edit::Value::Boolean(b) => Value::Bool(*b.value()),
        toml_edit::Value::Datetime(d) => Value::String(d.value().to_string()),
        toml_edit::Value::Array(a) => Value::Array(a.iter().map(value_json).collect()),
        toml_edit::Value::InlineTable(t) => {
            let mut map = Map::new();
            for (key, v) in t.iter() {
                map.insert(key.to_string(), value_json(v));
            }
            Value::Object(map)
        }
    }
}

/// Inputs of [`Config::load`] besides the file system.
pub(crate) struct Paths<'a> {
    pub codex_home: &'a Path,
    pub system_dir: &'a Path,
    pub dir: &'a Path,
    pub profile: Option<&'a str>,
}

impl Config {
    pub fn load(
        fs: &Fs,
        overlay: Option<&Overlay>,
        paths: &Paths,
        warnings: &mut Vec<String>,
    ) -> Config {
        let mut patches = Vec::new();
        for op in overlay.map(|o| o.ops.as_slice()).unwrap_or_default() {
            if let OverlayOp::SetJson {
                file,
                pointer,
                value,
            } = op
            {
                patches.push((fs::normalize(file), pointer.clone(), value.clone()));
            }
        }
        let reader = Reader { fs, patches };

        let mut layers = vec![LayerFile {
            source: Source::Defaults,
            path: None,
            value: parse(DEFAULTS_TOML).expect("the packaged defaults parse"),
            disabled: None,
        }];
        let system_file = paths.system_dir.join("config.toml");
        layers.extend(reader.layer(Source::System, &system_file, warnings));
        let user_file = paths.codex_home.join("config.toml");
        layers.extend(reader.layer(Source::User, &user_file, warnings));
        if let Some(name) = paths.profile {
            let profile_file = paths.codex_home.join(format!("{name}.config.toml"));
            if let Some(user) = layers.iter().find(|l| l.source == Source::User) {
                let legacy_selected =
                    user.value.get("profile").and_then(Value::as_str) == Some(name);
                let legacy_table = user
                    .value
                    .get("profiles")
                    .and_then(Value::as_object)
                    .is_some_and(|p| p.contains_key(name));
                if legacy_selected || legacy_table {
                    warnings.push(format!(
                        "{}: legacy `profile = \"{name}\"` / `[profiles.{name}]` with --profile {name}: codex refuses to start (config/src/loader/mod.rs:300-318)",
                        user_file.display()
                    ));
                }
            }
            if !reader.fs.is_file(&profile_file) {
                warnings.push(format!(
                    "{PROFILE_ENV}={name}: {} not found",
                    profile_file.display()
                ));
            }
            layers.extend(reader.layer(Source::Profile, &profile_file, warnings));
        }
        let managed_file = paths.system_dir.join("managed_config.toml");
        let managed: Vec<LayerFile> = reader
            .layer(Source::Managed, &managed_file, warnings)
            .into_iter()
            .collect();

        // Project root, markers and trust come from every non-project layer, managed included
        // (`loader:336-356`).
        let discovery: Vec<&LayerFile> = layers.iter().chain(managed.iter()).collect();
        let markers = root_markers(&discovery, warnings);
        let dir = paths.dir;
        let found_root = discover_root(fs, dir, &markers, true);
        let project_root = found_root.clone().unwrap_or_else(|| dir.to_path_buf());
        let repo_root = repo_root(fs, dir);
        let agents_root = discover_root(fs, dir, &markers, false);
        let trust = Trust {
            layers: discovery,
            project_root: project_root.clone(),
            repo_root: repo_root.clone(),
        };
        let dir_decision = trust.for_dir(dir, warnings);

        let codex_home_norm = fs::normalize(paths.codex_home);
        let mut project_layers = Vec::new();
        let mut dirs: Vec<&Path> = Vec::new();
        for ancestor in dir.ancestors() {
            dirs.push(ancestor);
            if ancestor == project_root {
                break;
            }
        }
        dirs.reverse();
        for d in dirs {
            let dot_codex = d.join(".codex");
            let config_file = dot_codex.join("config.toml");
            if !fs.is_dir(&dot_codex) && !reader.patched(&config_file) {
                continue;
            }
            if fs::normalize(&dot_codex) == codex_home_norm {
                continue;
            }
            let decision = trust.for_dir(d, warnings);
            let disabled = (!decision.is_trusted()).then(|| match decision.level {
                Some(TrustLevel::Untrusted) => format!("{} is marked untrusted", decision.key),
                _ => format!(
                    "{} is not a trusted project (add projects.\"{}\".trust_level = \"trusted\" to {})",
                    decision.key,
                    decision.key,
                    user_file.display()
                ),
            });
            let mut value = match reader.read(&config_file) {
                Toml::Ok(v) if v.is_object() => v,
                Toml::Invalid(err) if disabled.is_none() => {
                    warnings.push(format!(
                        "{}: invalid TOML in a trusted project, codex refuses to start with it ({err})",
                        config_file.display()
                    ));
                    Value::Object(Map::new())
                }
                _ => Value::Object(Map::new()),
            };
            let ignored: Vec<String> = match value.as_object_mut() {
                Some(table) => PROJECT_DENYLIST
                    .iter()
                    .filter(|key| table.remove(**key).is_some())
                    .map(|key| key.to_string())
                    .collect(),
                None => Vec::new(),
            };
            if disabled.is_none() && !ignored.is_empty() {
                warnings.push(format!(
                    "{}: ignored in a project config (config/src/loader/mod.rs:88-101): {}",
                    config_file.display(),
                    ignored.join(", ")
                ));
            }
            project_layers.push(LayerFile {
                source: Source::Project,
                path: Some(fs.realpath(&config_file)),
                value,
                disabled,
            });
        }
        layers.extend(project_layers);
        layers.extend(managed);

        let mut config = Config {
            layers,
            profile: paths.profile.map(str::to_string),
            project_root,
            repo_root,
            agents_root,
            dir_decision,
            active_project: Decision {
                level: None,
                key: String::new(),
                set_at: None,
            },
        };
        config.active_project = config.active_project(dir, warnings);
        if let Some((name, layer)) = config.get_str("/profile") {
            warnings.push(format!(
                "{}: legacy `profile = \"{name}\"` is no longer supported, codex refuses to start (core/src/config/mod.rs:3379-3386)",
                layer.at("/profile")
            ));
        }
        config
    }

    /// The effective value at `pointer` and the layer that set it: the highest enabled layer
    /// where it is present and not null.
    pub fn get(&self, pointer: &str) -> Option<(&Value, &LayerFile)> {
        self.layers
            .iter()
            .rev()
            .filter(|l| l.enabled())
            .find_map(|l| match l.value.pointer(pointer) {
                Some(Value::Null) | None => None,
                Some(v) => Some((v, l)),
            })
    }

    pub fn get_bool(&self, pointer: &str) -> Option<(bool, &LayerFile)> {
        self.get(pointer)
            .and_then(|(v, l)| v.as_bool().map(|b| (b, l)))
    }

    pub fn get_str(&self, pointer: &str) -> Option<(&str, &LayerFile)> {
        self.get(pointer)
            .and_then(|(v, l)| v.as_str().map(|s| (s, l)))
    }

    pub fn get_u64(&self, pointer: &str) -> Option<(u64, &LayerFile)> {
        self.get(pointer)
            .and_then(|(v, l)| v.as_u64().map(|n| (n, l)))
    }

    /// The enabled layers above `source` that set `pointer` (a write to `source`'s file would
    /// not take effect while one exists).
    pub fn set_above(&self, source: Source, pointer: &str) -> Option<&LayerFile> {
        let rank = |s: Source| match s {
            Source::Defaults => 0,
            Source::System => 1,
            Source::User => 2,
            Source::Profile => 3,
            Source::Project => 4,
            Source::Managed => 5,
        };
        self.layers.iter().rev().find(|l| {
            l.enabled()
                && rank(l.source) > rank(source)
                && !matches!(l.value.pointer(pointer), None | Some(Value::Null))
        })
    }

    /// `get_active_project(cwd, repo_root)`: the entry for the target dir, else for the main
    /// repository root, from the effective config (`config/src/config_toml.rs:884-898`).
    fn active_project(&self, dir: &Path, warnings: &mut Vec<String>) -> Decision {
        let layers: Vec<&LayerFile> = self.layers.iter().filter(|l| l.enabled()).collect();
        let trust = Trust {
            layers,
            project_root: self.project_root.clone(),
            repo_root: self.repo_root.clone(),
        };
        let mut candidates = keys(dir);
        if let Some(root) = &self.repo_root {
            candidates.extend(keys(root));
        }
        for key in &candidates {
            if let Some(found) = trust.lookup(key, warnings) {
                return found;
            }
        }
        Decision {
            level: None,
            key: candidates.into_iter().next().unwrap_or_default(),
            set_at: None,
        }
    }
}

/// The `projects` trust map of a set of layers (highest layer per key wins).
struct Trust<'a> {
    layers: Vec<&'a LayerFile>,
    project_root: PathBuf,
    repo_root: Option<PathBuf>,
}

impl Trust<'_> {
    fn lookup(&self, key: &str, warnings: &mut Vec<String>) -> Option<Decision> {
        let pointer = format!("/projects/{}/trust_level", fs::esc(key));
        let (value, layer) = self.layers.iter().rev().find_map(|l| {
            l.value
                .pointer(&pointer)
                .filter(|v| !v.is_null())
                .map(|v| (v, *l))
        })?;
        let level = match value.as_str() {
            Some("trusted") => TrustLevel::Trusted,
            Some("untrusted") => TrustLevel::Untrusted,
            _ => {
                warnings.push(format!(
                    "{}: trust_level must be \"trusted\" or \"untrusted\"; codex refuses to start",
                    layer.at(&pointer)
                ));
                return None;
            }
        };
        Some(Decision {
            level: Some(level),
            key: key.to_string(),
            set_at: Some(layer.at(&pointer)),
        })
    }

    /// `decision_for_dir` (`loader:1041-1084`): the dir, else the project root, else the main
    /// repository root.
    fn for_dir(&self, dir: &Path, warnings: &mut Vec<String>) -> Decision {
        let mut candidates = keys(dir);
        candidates.extend(keys(&self.project_root));
        if let Some(root) = &self.repo_root {
            candidates.extend(keys(root));
        }
        for key in &candidates {
            if let Some(found) = self.lookup(key, warnings) {
                return found;
            }
        }
        let fallback = self.repo_root.as_ref().unwrap_or(&self.project_root);
        Decision {
            level: None,
            key: keys(fallback).into_iter().next().unwrap_or_default(),
            set_at: None,
        }
    }
}

/// Canonical spelling first, then the path as given (`loader:1378-1394`).
fn keys(path: &Path) -> Vec<String> {
    let given = path.display().to_string();
    let canonical = std::fs::canonicalize(path)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| given.clone());
    if canonical == given {
        vec![canonical]
    } else {
        vec![canonical, given]
    }
}

/// `project_root_markers` from the given layers; the default `[".git"]` when unset or invalid.
fn root_markers(layers: &[&LayerFile], warnings: &mut Vec<String>) -> Vec<String> {
    let found = layers.iter().rev().find_map(|l| {
        l.value
            .get("project_root_markers")
            .filter(|v| !v.is_null())
            .map(|v| (v, *l))
    });
    match found {
        Some((Value::Array(items), layer)) => {
            let markers: Option<Vec<String>> = items
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect();
            markers.unwrap_or_else(|| {
                warnings.push(format!(
                    "{}: not an array of strings, codex uses [\".git\"]",
                    layer.at("/project_root_markers")
                ));
                vec![".git".to_string()]
            })
        }
        Some((_, layer)) => {
            warnings.push(format!(
                "{}: not an array of strings, codex uses [\".git\"]",
                layer.at("/project_root_markers")
            ));
            vec![".git".to_string()]
        }
        None => vec![".git".to_string()],
    }
}

/// Nearest ancestor of `dir` (inclusive) holding one of `markers`. `git_head`: a `.git` dir
/// counts only when it holds `HEAD` (the config loader, `loader:1491-1527`); otherwise any
/// existing path counts (the AGENTS.md walk, `file-system/src/find_up.rs:101-108`). No markers =
/// no root.
pub(crate) fn discover_root(
    fs: &Fs,
    dir: &Path,
    markers: &[String],
    git_head: bool,
) -> Option<PathBuf> {
    if markers.is_empty() {
        return None;
    }
    for ancestor in dir.ancestors() {
        for marker in markers {
            let path = ancestor.join(marker);
            let exists = fs.is_dir(&path) || fs.is_file(&path);
            if !exists {
                continue;
            }
            if git_head && marker == ".git" && fs.is_dir(&path) && !fs.is_file(&path.join("HEAD")) {
                continue;
            }
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// `resolve_root_git_project_for_trust` (`git-utils/src/trust.rs:71-223`): the nearest checkout
/// (a `.git` dir with `HEAD`, or a `.git` file); for a linked worktree, its main checkout. None
/// when a `.git` file does not point into `<main>/.git/worktrees/<name>`.
pub(crate) fn repo_root(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    let checkout = dir.ancestors().find(|a| {
        let dot_git = a.join(".git");
        fs.is_file(&dot_git) || (fs.is_dir(&dot_git) && fs.is_file(&dot_git.join("HEAD")))
    })?;
    let dot_git = checkout.join(".git");
    if fs.is_dir(&dot_git) {
        return Some(checkout.to_path_buf());
    }
    let text = fs.read_string(&dot_git)?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    let git_dir = std::fs::canonicalize(checkout.join(target)).ok()?;
    let worktrees = git_dir.parent()?;
    if worktrees.file_name()? != "worktrees" {
        return None;
    }
    let common = worktrees.parent()?;
    let main = common.parent()?;
    let main_git = std::fs::canonicalize(main.join(".git")).ok()?;
    (main_git == common).then(|| main.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp() -> Tmp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "harn-codex-config-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Tmp(std::fs::canonicalize(&root).unwrap())
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn load(root: &Path, profile: Option<&str>) -> (Config, Vec<String>) {
        let fs = Fs::new(None);
        let mut warnings = Vec::new();
        let config = Config::load(
            &fs,
            None,
            &Paths {
                codex_home: &root.join("home/.codex"),
                system_dir: &root.join("etc/codex"),
                dir: &root.join("repo"),
                profile,
            },
            &mut warnings,
        );
        (config, warnings)
    }

    #[test]
    fn toml_reads_as_json() {
        let v = parse(
            "a = 1\nb = \"x\"\n[t]\nc = [true, 2.5]\nd = { e = \"f\" }\n[[arr]]\ng = 1\n[projects.\"/x/y\"]\ntrust_level = \"trusted\"\n",
        )
        .unwrap();
        assert_eq!(
            v,
            json!({
                "a": 1, "b": "x",
                "t": {"c": [true, 2.5], "d": {"e": "f"}},
                "arr": [{"g": 1}],
                "projects": {"/x/y": {"trust_level": "trusted"}}
            })
        );
        assert!(parse("a = ").is_err());
    }

    #[test]
    fn layers_stack_defaults_system_user_profile_managed() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        write(
            &root.join("etc/codex/config.toml"),
            "model = \"sys\"\ndeveloper_instructions = \"sys\"\ninclude_apps_instructions = false\n",
        );
        write(
            &root.join("home/.codex/config.toml"),
            "model = \"user\"\ninclude_environment_context = false\n",
        );
        write(
            &root.join("home/.codex/work.config.toml"),
            "model = \"work\"\n",
        );
        write(
            &root.join("etc/codex/managed_config.toml"),
            "include_environment_context = true\n",
        );

        let (c, warnings) = load(root, None);
        assert!(warnings.is_empty(), "{warnings:?}");
        let sources: Vec<Source> = c.layers.iter().map(|l| l.source).collect();
        assert_eq!(
            sources,
            [
                Source::Defaults,
                Source::System,
                Source::User,
                Source::Managed
            ]
        );
        assert_eq!(c.get_str("/model").unwrap().0, "user");
        assert_eq!(
            c.get_str("/developer_instructions").unwrap().1.source,
            Source::System
        );
        assert!(!c.get_bool("/include_apps_instructions").unwrap().0);
        // Managed is pushed last: it beats the user layer.
        let (env, from) = c.get_bool("/include_environment_context").unwrap();
        assert!(env);
        assert_eq!(from.source, Source::Managed);
        assert_eq!(c.get_u64("/project_doc_max_bytes").unwrap().0, 32768);
        assert_eq!(
            c.set_above(Source::User, "/include_environment_context")
                .map(|l| l.source),
            Some(Source::Managed)
        );

        let (c, _) = load(root, Some("work"));
        assert_eq!(c.get_str("/model").unwrap().0, "work");
        assert_eq!(c.get_str("/model").unwrap().1.source, Source::Profile);
    }

    #[test]
    fn managed_trust_entry_gates_project_layers() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join("repo/.codex/config.toml"), "model = \"proj\"\n");
        let key = root.join("repo").display().to_string();
        write(
            &root.join("home/.codex/config.toml"),
            &format!("[projects.\"{key}\"]\ntrust_level = \"trusted\"\n"),
        );
        write(
            &root.join("etc/codex/managed_config.toml"),
            &format!("[projects.\"{key}\"]\ntrust_level = \"untrusted\"\n"),
        );
        let (c, _) = load(root, None);
        assert!(c.dir_decision.is_untrusted());
        assert!(c
            .dir_decision
            .set_at
            .as_deref()
            .unwrap()
            .contains("managed_config.toml"));
        assert!(c.get_str("/model").is_none());
        assert!(c.active_project.is_untrusted());
    }

    #[test]
    fn invalid_layers_warn_and_legacy_profile_warns() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        write(&root.join("etc/codex/config.toml"), "x = \n");
        write(
            &root.join("home/.codex/config.toml"),
            "profile = \"old\"\n[profiles.old]\nmodel = \"m\"\n",
        );
        let (_, warnings) = load(root, None);
        assert!(
            warnings.iter().any(|w| w.contains("invalid TOML")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("legacy `profile = \"old\"`")),
            "{warnings:?}"
        );
    }

    #[test]
    fn git_head_rule_differs_between_config_root_and_agents_root() {
        let t = tmp();
        let root = &t.0;
        // An empty `.git` dir: a marker for the AGENTS.md walk, not for the config loader.
        std::fs::create_dir_all(root.join("outer/.git")).unwrap();
        std::fs::create_dir_all(root.join("outer/inner")).unwrap();
        let fs = Fs::new(None);
        let markers = vec![".git".to_string()];
        let dir = root.join("outer/inner");
        assert_eq!(
            discover_root(&fs, &dir, &markers, false),
            Some(root.join("outer"))
        );
        assert_eq!(discover_root(&fs, &dir, &markers, true), None);
        assert_eq!(discover_root(&fs, &dir, &[], false), None);
    }

    #[test]
    fn linked_worktree_trust_falls_back_to_the_main_checkout() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("main/.git/HEAD"), "ref: refs/heads/main\n");
        let wt_git = root.join("main/.git/worktrees/wt");
        write(&wt_git.join("HEAD"), "ref: refs/heads/wt\n");
        write(&wt_git.join("commondir"), "../..\n");
        write(
            &root.join("main/.worktrees/wt/.git"),
            &format!("gitdir: {}\n", wt_git.display()),
        );
        let fs = Fs::new(None);
        let wt = root.join("main/.worktrees/wt");
        assert_eq!(repo_root(&fs, &wt), Some(root.join("main")));
        assert_eq!(repo_root(&fs, &root.join("main")), Some(root.join("main")));
    }
}
