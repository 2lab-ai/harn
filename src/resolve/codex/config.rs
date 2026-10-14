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
//! `SetJson` patches applied, so `plan` computes freed bytes by re-resolving. Every non-null patch
//! goes through the writer itself ([`writers::patch_toml`]) and its output is parsed exactly like
//! an unpatched file, so the re-resolve reads the very bytes `harn off` would write (quoted keys,
//! `[[…]]` appends, a missing file created) and the non-finite list below is recomputed from them.
//! A patch the writer refuses (a file that does not parse or is not UTF-8, a path through a
//! scalar, an integer past i64) fails the resolve: the plan cannot be applied, so its freed bytes
//! are unknown, never a guess. The one exception is a top-level `null` patch, which no writer
//! writes (TOML has none) and no knob uses: it is applied to the JSON view after the others and
//! reads as "key absent". A file that is not UTF-8 is unreadable, as it is to codex
//! (`file-system/src/lib.rs:650-659`); it is never decoded lossily. A non-finite float (`nan`,
//! `inf`) has no JSON form: its key stays set — no lower layer shows through — but unreadable as
//! any type ([`Lookup::Unreadable`]; [`Config::get`] hands it out as JSON `null`, which no typed
//! reader accepts). The list is taken after the overlay, so a patch that replaces the value
//! clears it.
//!
//! Trust (`projects."<path>".trust_level`):
//! - a project layer in dir `D` is enabled iff the first entry found for `D`, else for the project
//!   root, else for the main repository root, is `trusted` (`loader:1041-1084`), read from the
//!   defaults, system, user, profile and managed layers only (`loader:336-356`, `:1249-1268`);
//! - the AGENTS.md chain is skipped iff the active project — the entry for the target dir, else for
//!   the main repository root, with no project-root step — is `untrusted`
//!   (`config/src/config_toml.rs:884-898`, `core/src/config/mod.rs:3459-3465`,
//!   `core/src/agents_md.rs:63-66`), read from the merged config of every *enabled* layer,
//!   trusted project layers included: `projects` is not on the project denylist and nothing strips
//!   it (`config/src/state.rs:475-484`, `:548-551`; `core/src/config/mod.rs:1913-1916`). So a
//!   trusted repo's `.codex/config.toml` can re-trust a subdir for AGENTS.md, not for its config;
//! - each lookup tries the canonical path, then the path as given (`loader:1378-1394`).

use crate::model::{Overlay, OverlayOp};
use crate::resolve::claude::fs::{self, Fs};
use crate::writers;
use anyhow::Context;
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
    /// Pointers set to a non-finite float: set, but readable as no type (`null` in `value`).
    pub unreadable: Vec<String>,
}

/// What one layer says about a pointer.
pub(crate) enum Lookup<'a> {
    /// Missing, or `null` (an overlay's "remove the key").
    Absent,
    Set(&'a Value),
    /// The pointer or a parent of it is a non-finite float.
    Unreadable,
}

/// [`Config::get`]'s value for a [`Lookup::Unreadable`] key: `null`, rejected by every typed
/// reader, so no caller mistakes it for a setting and none falls back to a lower layer.
static UNREADABLE: Value = Value::Null;

impl LayerFile {
    /// `"<file>#<pointer>"` (`"codex defaults#<pointer>"` for the packaged defaults).
    pub fn at(&self, pointer: &str) -> String {
        match &self.path {
            Some(path) => format!("{}#{pointer}", path.display()),
            None => format!("codex defaults#{pointer}"),
        }
    }

    pub fn lookup(&self, pointer: &str) -> Lookup<'_> {
        let blocked = self
            .unreadable
            .iter()
            .any(|u| pointer == u || pointer.starts_with(&format!("{u}/")));
        if blocked {
            return Lookup::Unreadable;
        }
        match self.value.pointer(pointer) {
            None | Some(Value::Null) => Lookup::Absent,
            Some(v) => Lookup::Set(v),
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

/// One trust lookup: the level found (None = no entry) and the `projects` key that decided it
/// (when None, the key Codex tells the user to add).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub level: Option<TrustLevel>,
    pub key: String,
    /// `"<file>#<pointer>"` of the entry, when one was found.
    pub set_at: Option<String>,
    /// The file of the entry, kept apart from its pointer (a path may hold a `#`).
    pub set_in: Option<PathBuf>,
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

/// Non-finite floats of a document: `(pointer, TOML literal)`.
type NonFinite = Vec<(String, &'static str)>;

enum Toml {
    Missing,
    /// The document (overlay applied) and its non-finite floats still there: `(pointer, literal)`.
    Ok(Value, NonFinite),
    /// Not TOML.
    Invalid(String),
    /// Not UTF-8: codex reads config files with `String::from_utf8` and fails on the error
    /// (`file-system/src/lib.rs:650-659`).
    NotUtf8(String),
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

    /// `path` as JSON with its patches applied (module docs): the writer's output for every
    /// non-null patch, parsed strictly like an unpatched file, then the `null` ones on the JSON
    /// view. Err = the writer refuses a patch.
    fn read(&self, path: &Path) -> anyhow::Result<Toml> {
        let patches: Vec<&(PathBuf, String, Value)> = if self.patches.is_empty() {
            Vec::new()
        } else {
            let norm = fs::normalize(path);
            self.patches.iter().filter(|(f, _, _)| *f == norm).collect()
        };
        let bytes = match self.fs.read(path) {
            Some(bytes) => Some(bytes),
            // A file the patch creates.
            None if !patches.is_empty() && self.fs.is_file(path) => None,
            None => return Ok(Toml::Missing),
        };
        let (nulls, sets): (Vec<_>, Vec<_>) =
            patches.into_iter().partition(|(_, _, v)| v.is_null());
        let text = if sets.is_empty() {
            match &bytes {
                Some(bytes) => match std::str::from_utf8(bytes) {
                    Ok(text) => text.to_string(),
                    Err(e) => return Ok(Toml::NotUtf8(format!("not UTF-8: {e}"))),
                },
                None => String::new(),
            }
        } else {
            let sets: Vec<(String, Value)> = sets
                .into_iter()
                .map(|(_, pointer, value)| (pointer.clone(), value.clone()))
                .collect();
            // The writer refuses a file that is not UTF-8 or not TOML, like any other refusal.
            let written = writers::patch_toml(bytes.as_deref(), &sets)
                .with_context(|| path.display().to_string())?;
            String::from_utf8(written).context("the TOML writer wrote UTF-8")?
        };
        // The same strict parse as an unpatched file: non-finite floats left after the writer's
        // patches are listed again from the patched document.
        let (mut value, mut non_finite) = match parse(&text) {
            Ok(parsed) => parsed,
            Err(e) => return Ok(Toml::Invalid(e)),
        };
        for (_, pointer, null) in nulls {
            fs::set_pointer(&mut value, pointer, null.clone());
            non_finite.retain(|(u, _)| !overwrites(pointer, u));
        }
        Ok(Toml::Ok(value, non_finite))
    }

    /// A non-project layer: missing → None; invalid → a warning (Codex refuses to start).
    fn layer(
        &self,
        source: Source,
        path: &Path,
        warnings: &mut Vec<String>,
    ) -> anyhow::Result<Option<LayerFile>> {
        Ok(match self.read(path)? {
            Toml::Ok(value, non_finite) if value.is_object() => {
                warn_non_finite(path, &non_finite, warnings);
                Some(LayerFile {
                    source,
                    path: Some(self.fs.realpath(path)),
                    value,
                    disabled: None,
                    unreadable: non_finite.into_iter().map(|(p, _)| p).collect(),
                })
            }
            Toml::Ok(..) | Toml::Missing => None,
            Toml::Invalid(err) | Toml::NotUtf8(err) => {
                warnings.push(format!(
                    "{}: invalid TOML, codex refuses to start with it ({err})",
                    path.display()
                ));
                None
            }
        })
    }
}

/// A `null` patch at `patch` (`set_pointer` on the JSON view; every other patch is re-parsed from
/// the writer's output) replaces the value at `unreadable`: the same pointer, a parent of it, or a
/// child of it (descending turns the scalar into a table).
fn overwrites(patch: &str, unreadable: &str) -> bool {
    patch.is_empty()
        || patch == unreadable
        || unreadable.starts_with(&format!("{patch}/"))
        || patch.starts_with(&format!("{unreadable}/"))
}

fn warn_non_finite(path: &Path, found: &[(String, &'static str)], warnings: &mut Vec<String>) {
    for (pointer, literal) in found {
        warnings.push(format!(
            "{}#{pointer}: non-finite float `{literal}`; harn keeps the key set but unreadable (no lower layer's value is used) and codex rejects it unless the key takes a float",
            path.display()
        ));
    }
}

/// A TOML document as JSON: tables and inline tables → objects, arrays of tables → arrays,
/// datetimes → strings, non-finite floats → `null`, listed with their pointer and literal.
pub(crate) fn parse(text: &str) -> Result<(Value, NonFinite), String> {
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string().trim().replace('\n', " "))?;
    let mut non_finite = Vec::new();
    let value = table_json(doc.as_table(), "", &mut non_finite);
    Ok((value, non_finite))
}

fn table_json(table: &toml_edit::Table, at: &str, non_finite: &mut NonFinite) -> Value {
    let mut map = Map::new();
    for (key, item) in table.iter() {
        let pointer = format!("{at}/{}", fs::esc(key));
        if let Some(v) = item_json(item, &pointer, non_finite) {
            map.insert(key.to_string(), v);
        }
    }
    Value::Object(map)
}

fn item_json(item: &toml_edit::Item, at: &str, non_finite: &mut NonFinite) -> Option<Value> {
    match item {
        toml_edit::Item::None => None,
        toml_edit::Item::Value(v) => Some(value_json(v, at, non_finite)),
        toml_edit::Item::Table(t) => Some(table_json(t, at, non_finite)),
        toml_edit::Item::ArrayOfTables(a) => Some(Value::Array(
            a.iter()
                .enumerate()
                .map(|(i, t)| table_json(t, &format!("{at}/{i}"), non_finite))
                .collect(),
        )),
    }
}

fn value_json(value: &toml_edit::Value, at: &str, non_finite: &mut NonFinite) -> Value {
    match value {
        toml_edit::Value::String(s) => Value::String(s.value().clone()),
        toml_edit::Value::Integer(i) => Value::from(*i.value()),
        toml_edit::Value::Float(f) => {
            let x = *f.value();
            match serde_json::Number::from_f64(x) {
                Some(n) => Value::Number(n),
                None => {
                    let literal = if x.is_nan() {
                        "nan"
                    } else if x > 0.0 {
                        "inf"
                    } else {
                        "-inf"
                    };
                    non_finite.push((at.to_string(), literal));
                    Value::Null
                }
            }
        }
        toml_edit::Value::Boolean(b) => Value::Bool(*b.value()),
        toml_edit::Value::Datetime(d) => Value::String(d.value().to_string()),
        toml_edit::Value::Array(a) => Value::Array(
            a.iter()
                .enumerate()
                .map(|(i, v)| value_json(v, &format!("{at}/{i}"), non_finite))
                .collect(),
        ),
        toml_edit::Value::InlineTable(t) => {
            let mut map = Map::new();
            for (key, v) in t.iter() {
                let pointer = format!("{at}/{}", fs::esc(key));
                map.insert(key.to_string(), value_json(v, &pointer, non_finite));
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
    /// The layers as Codex stacks them. Err only when an overlay patch is one the writer refuses
    /// (module docs); a resolve without an overlay never fails here.
    pub fn load(
        fs: &Fs,
        overlay: Option<&Overlay>,
        paths: &Paths,
        warnings: &mut Vec<String>,
    ) -> anyhow::Result<Config> {
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
            value: parse(DEFAULTS_TOML).expect("the packaged defaults parse").0,
            disabled: None,
            unreadable: Vec::new(),
        }];
        let system_file = paths.system_dir.join("config.toml");
        layers.extend(reader.layer(Source::System, &system_file, warnings)?);
        let user_file = paths.codex_home.join("config.toml");
        layers.extend(reader.layer(Source::User, &user_file, warnings)?);
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
            layers.extend(reader.layer(Source::Profile, &profile_file, warnings)?);
        }
        let managed_file = paths.system_dir.join("managed_config.toml");
        let managed: Vec<LayerFile> = reader
            .layer(Source::Managed, &managed_file, warnings)?
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
            let mut unreadable = Vec::new();
            let mut value = match reader.read(&config_file)? {
                Toml::Ok(v, non_finite) if v.is_object() => {
                    if disabled.is_none() {
                        warn_non_finite(&config_file, &non_finite, warnings);
                    }
                    unreadable = non_finite.into_iter().map(|(p, _)| p).collect();
                    v
                }
                Toml::Invalid(err) if disabled.is_none() => {
                    warnings.push(format!(
                        "{}: invalid TOML in a trusted project, codex refuses to start with it ({err})",
                        config_file.display()
                    ));
                    Value::Object(Map::new())
                }
                // A read error stops codex before the trust check, trusted or not
                // (`loader:1743-1762`).
                Toml::NotUtf8(err) => {
                    warnings.push(format!(
                        "{}: {err}; codex refuses to start (project config files are read before the trust check)",
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
                unreadable,
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
                set_in: None,
            },
        };
        config.active_project = config.active_project(dir, warnings);
        if let Some((name, layer)) = config.get_str("/profile") {
            warnings.push(format!(
                "{}: legacy `profile = \"{name}\"` is no longer supported, codex refuses to start (core/src/config/mod.rs:3379-3386)",
                layer.at("/profile")
            ));
        }
        Ok(config)
    }

    /// The effective value at `pointer` and the layer that set it: the highest enabled layer
    /// where it is present and not null. A [`Lookup::Unreadable`] key stops the search there and
    /// comes back as `null` (the only time `get` returns `null`).
    pub fn get(&self, pointer: &str) -> Option<(&Value, &LayerFile)> {
        self.layers
            .iter()
            .rev()
            .filter(|l| l.enabled())
            .find_map(|l| match l.lookup(pointer) {
                Lookup::Absent => None,
                Lookup::Set(v) => Some((v, l)),
                Lookup::Unreadable => Some((&UNREADABLE, l)),
            })
    }

    /// A string setting; None when unset or set to anything else (callers that must not treat
    /// "set but not a string" as unset match on [`Config::get`]).
    pub fn get_str(&self, pointer: &str) -> Option<(&str, &LayerFile)> {
        self.get(pointer)
            .and_then(|(v, l)| v.as_str().map(|s| (s, l)))
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
                && !matches!(l.lookup(pointer), Lookup::Absent)
        })
    }

    /// `get_active_project(cwd, repo_root)`: the entry for the target dir, else for the main
    /// repository root, from the effective config of every enabled layer — trusted project
    /// layers included, as codex does (`config/src/config_toml.rs:884-898`,
    /// `core/src/config/mod.rs:1913-1916`, `:3460-3465`).
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
            set_in: None,
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
        let (value, layer) = self
            .layers
            .iter()
            .rev()
            .find_map(|l| match l.lookup(&pointer) {
                Lookup::Absent => None,
                Lookup::Set(v) => Some((v, *l)),
                Lookup::Unreadable => Some((&UNREADABLE, *l)),
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
            set_in: layer.path.clone(),
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
            set_in: None,
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
    let found = layers
        .iter()
        .rev()
        .find_map(|l| match l.lookup("/project_root_markers") {
            Lookup::Absent => None,
            Lookup::Set(v) => Some((v, *l)),
            Lookup::Unreadable => Some((&UNREADABLE, *l)),
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
    use crate::model::Refused;
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
        )
        .unwrap();
        (config, warnings)
    }

    #[test]
    fn toml_reads_as_json() {
        let (v, non_finite) = parse(
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
        assert!(non_finite.is_empty());
        assert!(parse("a = ").is_err());
    }

    fn load_with(root: &Path, overlay: Option<&Overlay>) -> anyhow::Result<Config> {
        Config::load(
            &Fs::new(overlay),
            overlay,
            &Paths {
                codex_home: &root.join("home/.codex"),
                system_dir: &root.join("etc/codex"),
                dir: &root.join("repo"),
                profile: None,
            },
            &mut Vec::new(),
        )
    }

    fn sets_on(file: &Path, sets: &[(&str, Value)]) -> Overlay {
        Overlay {
            ops: sets
                .iter()
                .map(|(pointer, value)| OverlayOp::SetJson {
                    file: file.to_path_buf(),
                    pointer: pointer.to_string(),
                    value: value.clone(),
                })
                .collect(),
        }
    }

    fn user_layer(root: &Path, overlay: Option<&Overlay>) -> Value {
        load_with(root, overlay)
            .unwrap()
            .layers
            .into_iter()
            .find(|l| l.source == Source::User)
            .map(|l| l.value)
            .unwrap_or(Value::Null)
    }

    /// The knobs C2/C3 write (quoted plugin keys, `[[skills.config]]` appends, hook state keys
    /// with `/`, keys with `.`), set through the overlay and through the writer on disk: the
    /// re-resolve sees exactly what the write leaves.
    #[test]
    fn overlay_reads_what_the_writer_writes() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let file = root.join("home/.codex/config.toml");
        write(
            &file,
            "# mine\nmodel = \"m\" # pinned\n\n[plugins.\"a@b\"]\nenabled = true\n\n[[skills.config]]\nname = \"keep\"\nenabled = false\n",
        );
        let sets = [
            ("/plugins/x@y/enabled", json!(false)),
            ("/plugins/a@b/enabled", json!(false)),
            (
                "/skills/config/-",
                json!({"path": "/s/one/SKILL.md", "enabled": false}),
            ),
            (
                "/skills/config/-",
                json!({"path": "/s/two/SKILL.md", "enabled": false}),
            ),
            (
                "/hooks/state/~1abs~1hooks.json:session_start:0:0/enabled",
                json!(false),
            ),
            ("/mcp_servers/srv.v2/enabled", json!(false)),
        ];
        let overlay = Overlay {
            ops: sets
                .iter()
                .map(|(pointer, value)| OverlayOp::SetJson {
                    file: file.clone(),
                    pointer: pointer.to_string(),
                    value: value.clone(),
                })
                .collect(),
        };
        let virtual_view = user_layer(root, Some(&overlay));
        assert_eq!(
            virtual_view.pointer("/skills/config/2/path"),
            Some(&json!("/s/two/SKILL.md"))
        );
        assert_eq!(
            virtual_view.pointer("/hooks/state/~1abs~1hooks.json:session_start:0:0/enabled"),
            Some(&json!(false))
        );

        let before = std::fs::read(&file).unwrap();
        let sets: Vec<(String, Value)> = sets
            .iter()
            .map(|(p, v)| (p.to_string(), v.clone()))
            .collect();
        std::fs::write(&file, writers::patch_toml(Some(&before), &sets).unwrap()).unwrap();
        assert_eq!(user_layer(root, None), virtual_view);

        // A patch on a missing file reads as the file `harn off` would create.
        let project = root.join("repo/.codex/config.toml");
        let overlay = Overlay {
            ops: vec![OverlayOp::SetJson {
                file: project.clone(),
                pointer: "/plugins/x@y/enabled".to_string(),
                value: json!(false),
            }],
        };
        let fs = Fs::new(Some(&overlay));
        let reader = Reader {
            fs: &fs,
            patches: vec![(
                fs::normalize(&project),
                "/plugins/x@y/enabled".to_string(),
                json!(false),
            )],
        };
        match reader.read(&project).unwrap() {
            Toml::Ok(v, _) => assert_eq!(v, json!({"plugins": {"x@y": {"enabled": false}}})),
            _ => panic!("not read"),
        }
    }

    /// Through the JSON view the TOML writer and the JSON writer make the same change.
    #[test]
    fn toml_writer_agrees_with_the_json_writer() {
        let source = "a = 1\n[t]\nk = \"v\"\ninline = { x = 1 }\n[[aot]]\nn = 1\n[d]\ne.f = true\n";
        let cases = [
            ("/a", json!(2)),
            ("/t/k", json!(["x", {"y": 1}])),
            ("/t/inline/z", json!({"w": false})),
            ("/aot/-", json!({"n": 2})),
            ("/aot/0/m", json!("s")),
            ("/d/e/g", json!(1.5)),
            ("/new/deep/key@x", json!("v")),
            ("/arr/-", json!("first")),
            ("/arr2/-/k", json!(1)),
        ];
        for (pointer, value) in cases {
            let written = writers::patch_toml(
                Some(source.as_bytes()),
                &[(pointer.to_string(), value.clone())],
            )
            .unwrap();
            let mut expected = parse(source).unwrap().0;
            writers::set_json_pointer(&mut expected, pointer, value).unwrap();
            assert_eq!(
                parse(&String::from_utf8(written).unwrap()).unwrap().0,
                expected,
                "{pointer}"
            );
        }
    }

    /// A writer patch re-parses the file strictly: a non-finite float it does not touch stays
    /// listed (set, unreadable), one it replaces is gone.
    #[test]
    fn writer_patches_recompute_non_finite_markers() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let file = root.join("home/.codex/config.toml");
        write(
            &file,
            "project_doc_max_bytes = nan\ninclude_environment_context = inf\n",
        );
        let overlay = sets_on(&file, &[("/include_environment_context", json!(false))]);
        let config = load_with(root, Some(&overlay)).unwrap();
        let user = config
            .layers
            .iter()
            .find(|l| l.source == Source::User)
            .unwrap();
        assert_eq!(user.unreadable, ["/project_doc_max_bytes"]);
        assert!(matches!(
            user.lookup("/project_doc_max_bytes"),
            Lookup::Unreadable
        ));
        assert_eq!(
            config.get("/include_environment_context").unwrap().0,
            &json!(false)
        );
    }

    /// The one patch the writer has no form for: `null` reads as "key absent"; the other patches
    /// of the file still go through the writer.
    #[test]
    fn a_null_patch_reads_as_key_absent() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let file = root.join("home/.codex/config.toml");
        write(&file, "model = \"m\"\nkeep = 1\n");
        let overlay = sets_on(&file, &[("/model", Value::Null), ("/keep", json!(2))]);
        let config = load_with(root, Some(&overlay)).unwrap();
        assert!(config.get_str("/model").is_none());
        assert_eq!(config.get("/keep").unwrap().0, &json!(2));
    }

    /// Any other patch the writer refuses fails the load (so the re-resolve, so `freed`): a
    /// guess here would show bytes freed by a plan that `harn off` refuses to apply.
    #[test]
    fn a_patch_the_writer_refuses_fails_the_load() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let file = root.join("home/.codex/config.toml");
        write(&file, "model = \"m\"\n");
        for (pointer, value, says) in [
            (
                "/model/x",
                json!(false),
                "/model is a string, not a table or array",
            ),
            ("/n", json!(u64::MAX), "past TOML's 64-bit integer range"),
            ("/m", json!({"k": null}), "TOML has no null"),
        ] {
            let Err(err) = load_with(root, Some(&sets_on(&file, &[(pointer, value)]))) else {
                panic!("{pointer}: loaded");
            };
            let err = format!("{err:#}");
            assert!(
                err.starts_with(&file.display().to_string()) && err.contains(says),
                "{pointer}: {err}"
            );
        }
        // A file the writer will not rewrite: not TOML, not UTF-8.
        for bytes in [b"model = ".as_slice(), b"model = \"\xff\"\n".as_slice()] {
            std::fs::write(&file, bytes).unwrap();
            let overlay = sets_on(&file, &[("/include_apps_instructions", json!(false))]);
            let Err(err) = load_with(root, Some(&overlay)) else {
                panic!("{bytes:?}: loaded");
            };
            assert!(
                err.chain().any(|c| c.downcast_ref::<Refused>().is_some()),
                "{err:#}"
            );
            // Without a patch the file is only a warning, as before.
            assert!(load_with(root, None).is_ok());
        }
    }

    #[test]
    fn non_finite_floats_stay_set_and_are_listed() {
        let (v, non_finite) =
            parse("a = nan\n[t]\nb = [1.0, -inf]\n[\"x/y\"]\nc = { d = +inf }\n").unwrap();
        assert_eq!(
            v,
            json!({"a": null, "t": {"b": [1.0, null]}, "x/y": {"c": {"d": null}}})
        );
        assert_eq!(
            non_finite,
            [
                ("/a".to_string(), "nan"),
                ("/t/b/1".to_string(), "-inf"),
                ("/x~1y/c/d".to_string(), "inf")
            ]
        );
        assert!(overwrites("/t", "/t/b/1"));
        assert!(overwrites("/a", "/a"));
        assert!(overwrites("/a/x", "/a"));
        assert!(overwrites("", "/a"));
        assert!(!overwrites("/t/b/-", "/t/b/1"));
        assert!(!overwrites("/ab", "/a"));
    }

    #[test]
    fn non_finite_value_masks_the_lower_layer() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        write(
            &root.join("etc/codex/config.toml"),
            "project_doc_max_bytes = 100\n",
        );
        write(
            &root.join("home/.codex/config.toml"),
            "project_doc_max_bytes = nan\n",
        );
        let (c, warnings) = load(root, None);
        let (v, from) = c.get("/project_doc_max_bytes").unwrap();
        assert_eq!(v, &Value::Null, "set, readable as no type");
        assert_eq!(from.source, Source::User);
        assert_eq!(c.get("/project_doc_max_bytes").unwrap().0.as_u64(), None);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("config.toml#/project_doc_max_bytes")
                    && w.contains("non-finite")),
            "{warnings:?}"
        );
    }

    #[test]
    fn invalid_utf8_is_invalid_not_rewritten() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        write(&root.join("etc/codex/config.toml"), "model = \"sys\"\n");
        let user = root.join("home/.codex/config.toml");
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, b"model = \"a\xffb\"\n").unwrap();
        let (c, warnings) = load(root, None);
        // The user layer is not read at all (codex would not start); no mangled value leaks in.
        assert_eq!(c.get_str("/model").unwrap().0, "sys");
        assert!(c.layers.iter().all(|l| l.source != Source::User));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("not UTF-8") && w.contains("refuses to start")),
            "{warnings:?}"
        );

        // A project file that is not UTF-8 stops codex even while the project is untrusted.
        std::fs::write(&user, "").unwrap();
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        let project = root.join("repo/.codex/config.toml");
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&project, b"model = \"\xff\"\n").unwrap();
        let (_, warnings) = load(root, None);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("not UTF-8") && w.contains("before the trust check")),
            "{warnings:?}"
        );
    }

    /// Codex's AGENTS.md gate reads `projects` from the merged config of every enabled layer,
    /// trusted project layers included (`config/src/state.rs:475-484`, `:548-551`;
    /// `core/src/config/mod.rs:1913-1916`, `:3460-3465`); `projects` is not on the project
    /// denylist (`loader:88-101`) and `sanitize_project_config` (`loader:1137-1230`) keeps it.
    /// The config gate reads only non-project layers (`loader:336-356`). This pins both.
    #[test]
    fn a_trusted_project_layer_can_retrust_a_subdir_for_agents_md_only() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        let repo = root.join("repo");
        let child = repo.join("child");
        std::fs::create_dir_all(child.join(".codex")).unwrap();
        write(
            &root.join("home/.codex/config.toml"),
            &format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
                repo.display(),
                child.display()
            ),
        );
        write(
            &repo.join(".codex/config.toml"),
            &format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                child.display()
            ),
        );
        let fs = Fs::new(None);
        let mut warnings = Vec::new();
        let c = Config::load(
            &fs,
            None,
            &Paths {
                codex_home: &root.join("home/.codex"),
                system_dir: &root.join("etc/codex"),
                dir: &child,
                profile: None,
            },
            &mut warnings,
        )
        .unwrap();
        assert!(
            c.dir_decision.is_untrusted(),
            "config gate: non-project layers only"
        );
        let child_layer = c
            .layers
            .iter()
            .find(|l| l.path.as_deref() == Some(child.join(".codex/config.toml").as_path()))
            .unwrap();
        assert!(!child_layer.enabled());
        assert!(
            c.active_project.is_trusted(),
            "AGENTS.md gate: every enabled layer"
        );
        assert!(c
            .active_project
            .set_at
            .as_deref()
            .unwrap()
            .starts_with(&repo.join(".codex/config.toml").display().to_string()));
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
        assert_eq!(
            c.get("/include_apps_instructions").unwrap().0,
            &json!(false)
        );
        // Managed is pushed last: it beats the user layer.
        let (env, from) = c.get("/include_environment_context").unwrap();
        assert_eq!(env, &json!(true));
        assert_eq!(from.source, Source::Managed);
        assert_eq!(c.get("/project_doc_max_bytes").unwrap().0, &json!(32768));
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
