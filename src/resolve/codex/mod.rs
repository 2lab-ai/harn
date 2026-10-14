//! Codex discovery rules (P0.5 WU C1). The rules are `.prd/research/02-codex-loading-rules.md` and
//! `.prd/01-spec.md` §Codex coverage, checked against the codex-cli source at tag `rust-v0.160.1`
//! (commit d27764b8; paths below are relative to `codex-rs/`): `CODEX_HOME`, the config layers
//! and the three trust gates ([`config`]), the AGENTS.md message and `developer_instructions`
//! ([`instructions`]), and the built-in blocks with their `include_*` / `agents.enabled` switches
//! ([`builtins`]), hooks ([`hooks`]), exec-policy rules files ([`rules`]) and MCP servers with
//! the tool-search state ([`mcp`]) (WU C3); the skill listing with its 2 % budget ([`skills`]) and
//! plugins with their skills, MCP servers, hooks and developer blocks ([`plugins`]) (WU C2).
//!
//! Reads files only, all of them through [`Fs`] so an [`Overlay`] is honoured everywhere
//! (`SetJson` patches a TOML file viewed as JSON); never spawns a process and never touches the
//! network.
//!
//! Item ids, in request order: `codex/system-prompt/builtin/instructions` (the request's
//! `instructions` field), `codex/instruction/<layer>/<file>#/developer_instructions`,
//! `codex/system-prompt/builtin/{permissions_instructions,collaboration_mode,apps_instructions,multi_agent}`,
//! `codex/system-prompt/builtin/skills_instructions` + `codex/skill-listing/builtin/skills` (the
//! `<skills_instructions>` block: framing row, then the listing with one `codex/skill/<layer>/<name>`
//! child per skill), `codex/system-prompt/builtin/{plugins_instructions,recommended_plugins}`,
//! `codex/instruction/builtin/agents-md-message` (the `# AGENTS.md instructions` user message:
//! its framing bytes, with one child per file — `codex/instruction/user/<realpath>` for the
//! global file, `codex/instruction/project/<realpath>` for the chain), and
//! `codex/env-block/builtin/environment_context`, then plugin MCP servers
//! (`codex/mcp-server/plugin/<server>`) and hooks (`codex/hook/plugin/<state key>`), then one
//! `codex/plugin/plugin/<name@marketplace>` row per configured plugin.
//!
//! Some built-in blocks render only when the model's catalog entry carries their text or flag
//! (`<collaboration_mode>`, `<multi_agent_role>` + `<multi_agent_mode>`, `<plugins_instructions>`).
//! `$CODEX_HOME/models_cache.json` is codex's last fetched catalog, a snapshot; the contract for
//! using it is on [`CacheUse`]. When it holds, the row follows the entry, read with codex's serde
//! defaults ([`Ctx::catalog_entry`]); otherwise the row is listed with a [`CATALOG_GATED`] signal.
//!
//! Knobs are Codex's own config keys. Codex has no local-only config file: the target dir's
//! `.codex/config.toml` is `project` scope (and takes effect only while that dir is trusted),
//! `$CODEX_HOME/config.toml` is `user` scope. A knob is listed only where it would take effect
//! (no enabled layer above it sets the key); otherwise the row says which file decides.
//!
//! Totals follow the Claude Code resolver's rule ([`super::claude::totals`]).

mod builtins;
mod config;
mod hooks;
mod instructions;
mod mcp;
mod plugins;
mod rules;
mod skills;

use crate::model::{
    item_id, Agent, Cost, Item, Kind, Knob, Layer, Mode, Origin, Overlay, Scope, Target, Tree,
};
use crate::resolve::claude::fs::{self, Fs, Json};
use config::{Config, LayerFile, Paths, Source, TrustLevel};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub use config::PROFILE_ENV;
pub use skills::CAP_STATE_UNKNOWN;

/// Signal prefix of a row whose presence the model's catalog entry decides while harn does not
/// know what codex will read there ([`Ctx::catalog_entry`]: no model in config, `model_catalog_json`
/// set, a `models_cache.json` outside the contract on [`CacheUse`], or the model missing from it). The row stays listed (`always`, its size if rendered in `why`), so a capture
/// without it is not a prediction miss: verify treats `item.signal.starts_with(CATALOG_GATED)` as
/// not checkable. The rest of the signal names the catalog condition and why the entry is unknown.
pub const CATALOG_GATED: &str = "catalog-gated:";

pub(crate) struct Ctx<'a> {
    pub target: &'a Target,
    pub fs: Fs,
    pub codex_home: PathBuf,
    /// `/etc/codex` (injectable in tests): `requirements.toml`, `hooks.json`, `rules/`, and
    /// `skills/` (the admin skill root).
    pub system_dir: PathBuf,
    pub config: Config,
    /// The effective model and context window (C2 needs the window for the skills cap).
    pub model: ModelFacts,
    pub warnings: Vec<String>,
}

/// What the static files say about the model.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModelFacts {
    pub slug: Option<String>,
    /// Where the slug came from (`"<file>#/model"` or `"target"`).
    pub slug_from: Option<String>,
    /// `"<file>#/model"` when the effective `model` is set but not a string (unknown model).
    pub slug_unreadable: Option<String>,
    pub context_window: Option<u64>,
    /// Where `context_window` came from; with no window, why it is unknown although config sets
    /// one.
    pub window_from: Option<String>,
    /// The entry of the slug in `$CODEX_HOME/models_cache.json` as read, accepted or not: catalog
    /// facts come only through [`Ctx::catalog_entry`], which returns it only for an accepted
    /// snapshot ([`CacheUse::Used`]). Read directly only to quote the file in an explanation.
    pub catalog: Option<Value>,
    /// `client_version` of `models_cache.json`: the codex that last refreshed the catalog.
    pub catalog_version: Option<String>,
    /// `"<file>#/model_catalog_json"` when config names its own catalog: harn does not read it,
    /// so every catalog fact (base instructions, window) stays unknown rather than mixing in
    /// `models_cache.json`.
    pub catalog_unread: Option<String>,
    /// Whether harn accepts `models_cache.json` as its snapshot ([`CacheUse`], [`cache_use`]).
    /// Every catalog fact depends on it, without exception: the catalog-gated rows, the skills
    /// usage section, the tool-search state, the base instructions' size, the context window and
    /// the `max_context_window` clamp.
    pub cache_use: CacheUse,
}

/// The codex on the target's `PATH` ([`installed_codex`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Installed {
    NotFound,
    /// Found, but neither an `@openai/codex` `package.json` nor a versioned install dir names it.
    Unversioned,
    Version(String),
}

/// Whether harn uses `$CODEX_HOME/models_cache.json` for the selected model's catalog facts.
///
/// CONTRACT: `models_cache.json` is codex's last fetched catalog — a snapshot. Codex treats it as
/// stale after 300 s (`models-manager/src/manager.rs:35`, fixed at `:310-315`; `cache.rs:90-99`,
/// `:212-220`) and revalidates it with the server when it starts, so "codex uses this file as it
/// is" is not a claim harn makes. harn uses its facts for the selected model when
/// (a) the file exists and is JSON,
/// (b) its required top-level fields have the right JSON types (`fetched_at` a string, `models` an
/// array; `client_version`, `identity`, `etag` a string or null),
/// (c) every entry passes the checks for the fields harn reads ([`validate_cache`]), and
/// (d) codex is on `PATH` with a known version equal to `client_version` (and `identity` is
/// present: an unscoped legacy cache is a miss, `manager.rs:747-752`).
/// A failure in (a)–(d) makes catalog facts unknown ([`CATALOG_GATED`]) with a true reason. When
/// used ([`CacheUse::Used`]), the `model:` signal says the facts come from codex's catalog fetched
/// at `fetched_at` (as written: codex writes UTC), that codex re-checks a catalog older than
/// 5 minutes with the server when it starts, that the cache's `identity` is not compared with the
/// login (that needs the credentials, which harn does not read), and that
/// `harn verify --run --agent codex` confirms. harn does not check fields it does not read
/// (`display_name` and the other `ModelInfo` fields, nested enums) and does not parse `fetched_at`
/// as a date.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum CacheUse {
    /// The contract holds: `fetched_at` as written in the file.
    Used { fetched_at: String },
    /// No file yet: codex writes it on its first run.
    #[default]
    Missing,
    /// Not accepted, and codex would not load it either: it cannot parse it, or the file belongs
    /// to another version or login. Why.
    Refetched(String),
    /// Not accepted: harn cannot tell whether codex loads it (no codex version, a shape harn does
    /// not read). Why.
    Unknown(String),
}

impl CacheUse {
    /// Why harn does not accept the file as its snapshot; None when it does.
    pub fn reason(&self) -> Option<String> {
        match self {
            CacheUse::Used { .. } => None,
            CacheUse::Missing => {
                Some("no models_cache.json yet (codex writes it on its first run)".to_string())
            }
            CacheUse::Refetched(why) | CacheUse::Unknown(why) => Some(why.clone()),
        }
    }
}

impl Ctx<'_> {
    pub fn dir(&self) -> &Path {
        &self.target.dir
    }

    pub fn home(&self) -> &Path {
        &self.target.home
    }

    /// `~/…` for paths under home, the target-relative path for paths under the target.
    pub fn display(&self, path: &Path) -> String {
        if let Ok(rel) = path.strip_prefix(self.dir()) {
            if !rel.as_os_str().is_empty() {
                return rel.display().to_string();
            }
        }
        if let Ok(rel) = path.strip_prefix(self.home()) {
            return format!("~/{}", rel.display());
        }
        path.display().to_string()
    }

    /// The model's slug and its entry in harn's accepted snapshot of the catalog, or why harn has
    /// none: config names its own catalog, there is no (readable) model, `models_cache.json` is
    /// not accepted ([`ModelFacts::cache_use`]), or the model is not in it. The one gate for
    /// catalog facts; an entry returned here has passed [`validate_cache`].
    pub fn catalog_entry(&self) -> Result<(&str, &Value), String> {
        let m = &self.model;
        if let Some(at) = &m.catalog_unread {
            return Err(format!("{at}: that catalog is not read"));
        }
        let Some(slug) = m.slug.as_deref() else {
            return Err(match &m.slug_unreadable {
                Some(at) => format!("{at} is not a string"),
                None => "no model in config: codex picks its default at startup".to_string(),
            });
        };
        if let Some(why) = m.cache_use.reason() {
            return Err(why);
        }
        match &m.catalog {
            Some(entry) => Ok((slug, entry)),
            None => Err(format!(
                "{slug} is not in {}",
                self.display(&self.codex_home.join("models_cache.json"))
            )),
        }
    }

    /// A `#[serde(default)] bool` of the catalog entry as codex reads it
    /// (`include_skills_usage_instructions`, `include_plugin_usage_instructions`,
    /// `protocol/src/openai_models.rs:432-435`): missing is `false` (any other type has failed
    /// [`validate_cache`]); `Err` = unknown, with why ([`Ctx::catalog_entry`]).
    pub fn catalog_flag(&self, flag: &str) -> Result<bool, String> {
        let (_, entry) = self.catalog_entry()?;
        Ok(entry.get(flag).and_then(Value::as_bool).unwrap_or(false))
    }

    /// The [`CATALOG_GATED`] signal: the row renders only if the catalog entry has `condition`.
    pub fn catalog_gated(condition: &str, unknown: &str) -> String {
        format!(
            "{CATALOG_GATED} renders only if the model's catalog entry has {condition}; {unknown}"
        )
    }

    pub fn user_config(&self) -> PathBuf {
        self.codex_home.join("config.toml")
    }

    /// The target dir's own `.codex/config.toml` (None when that `.codex` is `CODEX_HOME`).
    pub fn project_config(&self) -> Option<PathBuf> {
        let dot_codex = self.dir().join(".codex");
        (fs::normalize(&dot_codex) != fs::normalize(&self.codex_home))
            .then(|| dot_codex.join("config.toml"))
    }
}

pub(crate) fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// [`CacheUse`] for the file and the installed codex, in the contract's order (a)–(d): the file is
/// missing or not JSON; [`validate_cache`] fails — codex cannot load it
/// (`models-manager/src/cache.rs:229-239`, a miss at `manager.rs:725-735`), or harn cannot read a
/// shape; codex is not on `PATH` or its version is unknown; `client_version` is missing or not the
/// installed version (`manager.rs:739-746`); `identity` is missing (`manager.rs:747-752`).
fn cache_use(cache: &Json, installed: &Installed) -> CacheUse {
    let refetch = "codex fetches the catalog instead (models-manager/src/manager.rs:725-752)";
    let cache = match cache {
        Json::Missing => return CacheUse::Missing,
        Json::Invalid(_) => {
            return CacheUse::Refetched(format!("models_cache.json is not valid JSON; {refetch}"))
        }
        Json::Ok(v) => v,
    };
    let fetched_at = match validate_cache(cache) {
        Ok(fetched_at) => fetched_at.to_string(),
        Err(Invalid::Rejected(why)) => {
            return CacheUse::Refetched(format!(
                "models_cache.json: {why}, so codex cannot parse the file; {refetch}"
            ))
        }
        Err(Invalid::Unreadable(why)) => {
            return CacheUse::Unknown(format!(
                "models_cache.json: {why}, a shape harn does not read; catalog facts unknown"
            ))
        }
    };
    let codex_version = match installed {
        Installed::Version(v) => v,
        Installed::NotFound => {
            return CacheUse::Unknown(
                "codex not found on PATH: cannot tell whether it accepts models_cache.json"
                    .to_string(),
            )
        }
        Installed::Unversioned => {
            return CacheUse::Unknown(
                "the installed codex's version could not be determined: cannot tell whether it accepts models_cache.json"
                    .to_string(),
            )
        }
    };
    match cache.get("client_version").and_then(Value::as_str) {
        None => {
            return CacheUse::Refetched(format!(
                "models_cache.json has no client_version; {refetch}"
            ))
        }
        Some(v) if v != codex_version => {
            return CacheUse::Refetched(format!(
                "models_cache.json was written by codex {v}, not {codex_version}; {refetch}"
            ))
        }
        Some(_) => {}
    }
    if cache.get("identity").and_then(Value::as_str).is_none() {
        return CacheUse::Refetched(format!(
            "models_cache.json has no identity (a legacy cache); {refetch}"
        ));
    }
    CacheUse::Used { fetched_at }
}

/// Why [`validate_cache`] fails.
enum Invalid {
    /// codex's serde rejects the file: a missing required field harn checks, or `null` / a
    /// scalar of the wrong type for a typed field.
    Rejected(String),
    /// A shape harn does not read (an array where an object is expected: serde derive may read a
    /// struct from a sequence). Whether codex accepts it is not known; the facts stay unknown.
    Unreadable(String),
}

/// The JSON type serde accepts for a field harn reads.
#[derive(Clone, Copy)]
enum Ty {
    /// `#[serde(default)] bool`: missing is fine, anything else must be a boolean (`null` fails).
    Bool,
    /// `Option<String>` (also `deserialize_optional_model_selector`, which reads an
    /// `Option<String>` first, `protocol/src/openai_models.rs:352-361`): missing, `null` or a string.
    OptStr,
    /// `Option<i64>`: missing, `null` or an integer in range.
    OptI64,
    /// `Option<struct>`: missing, `null` or an object.
    OptObj,
}

/// The `ModelInfo` fields harn reads (and their parents), parents before children
/// (`protocol/src/openai_models.rs:404-511`, `:544-561`, `:750-753`, the legacy wrapper `:857-863`).
const ENTRY_FIELDS: &[(&str, Ty)] = &[
    ("/include_skills_usage_instructions", Ty::Bool),
    ("/include_plugin_usage_instructions", Ty::Bool),
    ("/include_apps_usage_instructions", Ty::Bool),
    ("/supports_search_tool", Ty::Bool),
    ("/context_window", Ty::OptI64),
    ("/max_context_window", Ty::OptI64),
    ("/multi_agent_version", Ty::OptStr),
    ("/base_instructions", Ty::OptStr),
    ("/model_messages", Ty::OptObj),
    ("/model_messages/instructions_template", Ty::OptStr),
    ("/model_messages/collaboration_modes", Ty::OptObj),
    ("/model_messages/collaboration_modes/default", Ty::OptStr),
    ("/model_messages/collaboration_modes/plan", Ty::OptStr),
];

/// Contract steps (b) and (c) on [`CacheUse`]: the top-level fields (`ModelsCacheEntry`,
/// `models-manager/src/cache.rs:64-88`) and, per entry, a string `slug`, [`ENTRY_FIELDS`], and
/// `base_instructions` or `model_messages.instructions_template`
/// (`protocol/src/openai_models.rs:904-943`). Codex loads the whole file, so one bad field in any
/// entry fails it for every model. Only the fields harn reads are checked; `fetched_at` is checked
/// as a string, not parsed as a date, and returned.
fn validate_cache(cache: &Value) -> Result<&str, Invalid> {
    use Invalid::{Rejected, Unreadable};
    let top = match cache {
        Value::Object(top) => top,
        Value::Array(_) => return Err(Unreadable("the file is an array".to_string())),
        other => return Err(Rejected(format!("the file {}", is(other)))),
    };
    let fetched_at = match top.get("fetched_at") {
        Some(Value::String(at)) => at.as_str(),
        Some(other) => return Err(Rejected(format!("fetched_at {}", is(other)))),
        None => return Err(Rejected("fetched_at is missing".to_string())),
    };
    for key in ["client_version", "identity", "etag"] {
        if let Some(v) = top.get(key).filter(|v| !v.is_null() && !v.is_string()) {
            return Err(Rejected(format!("{key} {}", is(v))));
        }
    }
    let models = match top.get("models") {
        Some(Value::Array(models)) => models,
        Some(other) => return Err(Rejected(format!("models {}", is(other)))),
        None => return Err(Rejected("models is missing".to_string())),
    };
    for (i, entry) in models.iter().enumerate() {
        let map = match entry {
            Value::Object(map) => map,
            Value::Array(_) => return Err(Unreadable(format!("models[{i}] is an array"))),
            other => return Err(Rejected(format!("models[{i}] {}", is(other)))),
        };
        let slug = match map.get("slug") {
            Some(Value::String(slug)) => slug,
            Some(other) => return Err(Rejected(format!("models[{i}].slug {}", is(other)))),
            None => return Err(Rejected(format!("models[{i}] has no slug"))),
        };
        for (pointer, ty) in ENTRY_FIELDS {
            let Some(v) = entry.pointer(pointer) else {
                continue;
            };
            let field = || format!("models[{i}] ({slug}) {}", pointer[1..].replace('/', "."));
            match (ty, v) {
                (Ty::Bool, Value::Bool(_))
                | (Ty::OptStr, Value::Null | Value::String(_))
                | (Ty::OptObj, Value::Null | Value::Object(_)) => {}
                (Ty::OptI64, Value::Null) => {}
                (Ty::OptI64, Value::Number(n)) if n.as_i64().is_some() => {}
                (Ty::OptI64, Value::Number(_)) => {
                    return Err(Rejected(format!(
                        "{} is not an integer in i64 range",
                        field()
                    )))
                }
                (Ty::OptObj, Value::Array(_)) => {
                    return Err(Unreadable(format!("{} is an array", field())))
                }
                (_, other) => return Err(Rejected(format!("{} {}", field(), is(other)))),
            }
        }
        let text = |pointer: &str| entry.pointer(pointer).is_some_and(Value::is_string);
        if !text("/base_instructions") && !text("/model_messages/instructions_template") {
            return Err(Rejected(format!(
                "models[{i}] ({slug}) has neither base_instructions nor model_messages.instructions_template"
            )));
        }
    }
    Ok(fetched_at)
}

/// `is null` / `is a string` / `is an array` …
fn is(v: &Value) -> String {
    match v {
        Value::Null => "is null".to_string(),
        Value::Array(_) | Value::Object(_) => format!("is an {}", json_type(v)),
        other => format!("is a {}", json_type(other)),
    }
}

pub(crate) fn new_item(kind: Kind, layer: Layer, name: &str, label: &str) -> Item {
    Item {
        id: item_id(Agent::Codex, kind, layer, name),
        label: label.to_string(),
        kind,
        origin: Origin {
            layer,
            path: None,
            setting: None,
            plugin: None,
            why: Vec::new(),
        },
        mode: Mode::Always,
        cost: Cost::default(),
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

/// The layer kind an item of a config layer belongs to.
pub(crate) fn layer_of(file: &LayerFile) -> Layer {
    match file.source {
        Source::Defaults => Layer::Builtin,
        // `/etc/codex/config.toml` is admin-owned (below the user layer); `Managed` is the
        // nearest layer kind the model has.
        Source::System | Source::Managed => Layer::Managed,
        Source::User | Source::Profile => Layer::User,
        Source::Project => Layer::Project,
    }
}

/// Knobs setting `pointer` = `value`, narrowest scope first: the target dir's `.codex/config.toml`
/// (`project`, only while that dir's trust decision is `trusted` and no managed layer sets the
/// key), then `$CODEX_HOME/config.toml` (`user`, only while no profile, enabled project or
/// managed layer sets it). Empty → one `Knob::None` naming the file that decides.
pub(crate) fn config_knobs(
    ctx: &Ctx,
    pointer: &str,
    value: Value,
    note: Option<&str>,
) -> Vec<Knob> {
    let mut out = Vec::new();
    if let Some(file) = ctx.project_config() {
        if ctx.config.dir_decision.is_trusted()
            && ctx.config.set_above(Source::Project, pointer).is_none()
        {
            out.push(Knob::Setting {
                file,
                pointer: pointer.to_string(),
                value: value.clone(),
                scope: Scope::Project,
                note: note.map(str::to_string),
            });
        }
    }
    if ctx.config.set_above(Source::User, pointer).is_none() {
        out.push(Knob::Setting {
            file: ctx.user_config(),
            pointer: pointer.to_string(),
            value,
            scope: Scope::User,
            note: Some(match note {
                Some(note) => format!("{note}; every project"),
                None => "every project".to_string(),
            }),
        });
    }
    if out.is_empty() {
        let decider = ctx
            .config
            .set_above(Source::User, pointer)
            .map(|l| l.at(pointer))
            .unwrap_or_else(|| pointer.to_string());
        out.push(Knob::None {
            reason: format!("{decider} decides it; no file harn writes is above it"),
        });
    }
    out
}

pub fn resolve(target: &Target, overlay: Option<&Overlay>) -> anyhow::Result<Tree> {
    resolve_with(target, overlay, Path::new(config::SYSTEM_DIR))
}

/// [`resolve`] with the system config dir injectable (tests; `/etc/codex` otherwise).
pub(crate) fn resolve_with(
    target: &Target,
    overlay: Option<&Overlay>,
    system_dir: &Path,
) -> anyhow::Result<Tree> {
    let fs = Fs::new(overlay);
    let mut warnings = Vec::new();
    let codex_home = codex_home(target);
    let mut tree = Tree {
        agent: Agent::Codex,
        target_dir: target.dir.clone(),
        agent_version: None,
        rules_verified_against: super::Codex::VERIFIED_AGAINST.to_string(),
        tool_search: None,
        signals: Vec::new(),
        items: Vec::new(),
        predicted_known: 0,
        unknown_rows: 0,
        observed_body: None,
        warnings: Vec::new(),
        home: Some(target.home.clone()),
        observed_regions: Vec::new(),
        tool_search_cause: None,
    };
    if !fs.is_dir(&codex_home) {
        let why = match target.env.get("CODEX_HOME").filter(|v| !v.is_empty()) {
            Some(_) => "CODEX_HOME points to a missing directory; codex refuses to start (utils/home-dir/src/lib.rs:26-35)",
            None => "codex has not run with this home (a first run creates it)",
        };
        tree.warnings.push(format!(
            "no codex home at {}: {why}; nothing to list",
            codex_home.display()
        ));
        return Ok(tree);
    }

    let profile = target
        .env
        .get(config::PROFILE_ENV)
        .map(|p| p.trim())
        .filter(|p| !p.is_empty());
    let config = Config::load(
        &fs,
        overlay,
        &Paths {
            codex_home: &codex_home,
            system_dir,
            dir: &target.dir,
            profile,
        },
        &mut warnings,
    )?;
    // The installed codex decides which models_cache.json it accepts.
    let installed = installed_codex(target);
    let model = model_facts(&fs, target, &codex_home, &config, &installed, &mut warnings);
    let mut ctx = Ctx {
        target,
        fs,
        codex_home,
        system_dir: system_dir.to_path_buf(),
        config,
        model,
        warnings,
    };

    // Plugins first: their skill roots feed the listing, their hook sources the hook rows.
    let plugins = plugins::discover(&mut ctx);
    let listing = skills::listing(&mut ctx, &plugins);
    let (plugin_servers, plugin_hooks) = plugins::parts(&mut ctx, &plugins);
    // Config hooks then plugin hooks (`hooks/src/engine/discovery.rs:128-197`), one rule set.
    let mut hook_rows = hooks::collect(&mut ctx, plugin_hooks);
    plugins::decorate_hooks(&plugins, &mut hook_rows);
    let mut contributions = listing.clone();
    contributions.extend(plugin_servers.iter().cloned());
    contributions.extend(hook_rows.iter().cloned());
    let plugin_blocks = plugins::blocks(&ctx, &plugins, &contributions);

    // Request order (`research/02` §0): developer_instructions, skills, the developer blocks with
    // the plugin blocks after apps, the multi-agent messages, the AGENTS.md message, environment.
    let mut items = Vec::new();
    items.push(builtins::base_instructions(&mut ctx));
    items.extend(instructions::developer_instructions(&ctx));
    items.extend(listing);
    let mut blocks = builtins::developer_blocks(&ctx);
    let at = blocks
        .iter()
        .position(|i| i.id.ends_with("/builtin/multi_agent"))
        .unwrap_or(blocks.len());
    blocks.splice(at..at, plugin_blocks);
    items.extend(blocks);
    items.extend(instructions::agents_md(&mut ctx));
    items.extend(builtins::environment_context(&ctx));
    items.extend(hook_rows);
    items.extend(rules::collect(&mut ctx));
    let (tool_search, tool_search_cause) = mcp::tool_search(&ctx);
    items.extend(mcp::collect(&mut ctx, tool_search));
    items.extend(plugin_servers);
    let plugin_rows = plugins::items(&plugins, &items);
    items.extend(plugin_rows);
    tree.items = items;
    tree.tool_search = tool_search;
    tree.tool_search_cause = Some(tool_search_cause);

    tree.signals.push(trust_signal(&ctx));
    tree.signals.push(model_signal(&ctx));
    if let Some(profile) = &ctx.config.profile {
        tree.signals.push(format!(
            "profile: {profile} ({} = {profile}, as `codex --profile {profile}`)",
            config::PROFILE_ENV
        ));
    }
    tree.agent_version = match installed {
        Installed::Version(v) => Some(v),
        Installed::NotFound | Installed::Unversioned => None,
    };
    let mut warnings = ctx.warnings;
    if let Some(v) = tree.agent_version.as_deref() {
        if version_key(v) > version_key(super::Codex::VERIFIED_AGAINST) {
            warnings.push(format!(
                "codex {v} is newer than the rules (verified against {}); run `harn verify`",
                super::Codex::VERIFIED_AGAINST
            ));
        }
    }
    tree.warnings = warnings;
    let (known, unknown) = super::claude::totals(&tree);
    tree.predicted_known = known;
    tree.unknown_rows = unknown;
    Ok(tree)
}

/// `CODEX_HOME` when set and non-empty, else `~/.codex` (`utils/home-dir/src/lib.rs:13-17`).
pub fn codex_home(target: &Target) -> PathBuf {
    match target.env.get("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(v) => {
            let path = PathBuf::from(v);
            std::fs::canonicalize(&path).unwrap_or(path)
        }
        None => target.home.join(".codex"),
    }
}

/// `trust: <state> (<where>)` for the target dir: the AGENTS.md gate and the project-config gate,
/// which look the entry up differently (`config.rs` module docs).
fn trust_signal(ctx: &Ctx) -> String {
    let c = &ctx.config;
    let entry = |d: &config::Decision| match &d.set_at {
        Some(at) => format!(
            "projects.{}.trust_level in {}",
            json_key(&d.key),
            at_file(at)
        ),
        None => format!("no projects.{} entry", json_key(&d.key)),
    };
    let dir = &c.dir_decision;
    let agents = &c.active_project;
    let state = match dir.level {
        Some(level) => level.as_str(),
        None => "unset",
    };
    let mut out = format!("trust: {state} ({})", entry(dir));
    match (dir.level, agents.level) {
        (Some(TrustLevel::Trusted), a) if a != Some(TrustLevel::Untrusted) => {
            out.push_str(": project .codex/config.toml, hooks and rules load");
        }
        (_, Some(TrustLevel::Untrusted)) => {
            out.push_str(
                ": project AGENTS.md files, .codex/config.toml, hooks and rules are skipped; skills still load",
            );
        }
        _ => out.push_str(
            ": project .codex/config.toml, hooks and rules are ignored; AGENTS.md and skills still load",
        ),
    }
    if agents.level != dir.level || (agents.set_at.is_some() && agents.key != dir.key) {
        out.push_str(&format!(" · AGENTS.md gate: {}", entry(agents)));
    }
    out
}

fn at_file(at: &str) -> &str {
    at.split_once('#').map(|(f, _)| f).unwrap_or(at)
}

/// A `projects` key as it is written in TOML (`"…"`).
fn json_key(key: &str) -> String {
    serde_json::to_string(key).expect("a string serializes")
}

fn model_signal(ctx: &Ctx) -> String {
    let m = &ctx.model;
    let model = match (&m.slug, &m.slug_from, &m.slug_unreadable) {
        (Some(slug), Some(from), _) => format!("model: {slug} ({from})"),
        (Some(slug), None, _) => format!("model: {slug}"),
        (None, _, Some(at)) => {
            format!("model: unknown ({at} is not a string; codex refuses to start with it)")
        }
        _ => "model: not set in config (codex picks its default from the catalog at startup)"
            .to_string(),
    };
    let model = match (m.context_window, &m.window_from, &m.catalog_unread) {
        (Some(w), Some(from), _) => format!("{model} · context window {w} tokens ({from})"),
        (_, _, Some(at)) => {
            format!("{model} · context window unknown ({at}: that catalog is not read)")
        }
        (None, Some(why), None) => format!("{model} · context window unknown ({why})"),
        _ => format!("{model} · context window unknown"),
    };
    // The cache is rewritten by whichever codex refreshed it last (two installs on one machine
    // take turns), so its version is a fact about the file, not the installed codex.
    let model = match &m.catalog_version {
        Some(v) => format!("{model} · models_cache.json written by codex {v}"),
        None => model,
    };
    // Every definite catalog-gated row rests on this entry: say what it is and what was not
    // checked (the contract on `CacheUse`).
    if let (Ok(_), CacheUse::Used { fetched_at }) = (ctx.catalog_entry(), &m.cache_use) {
        return format!(
            "{model} · catalog facts from codex's catalog fetched at {fetched_at} (models_cache.json): codex re-checks a catalog older than 5 minutes with the server when it starts, identity is not compared with your login, and `harn verify --run --agent codex` confirms"
        );
    }
    match (&m.catalog_unread, &m.cache_use) {
        (None, CacheUse::Refetched(why) | CacheUse::Unknown(why)) => {
            format!("{model} · models_cache.json not used: {why}")
        }
        _ => model,
    }
}

/// The model (`target.model`, else the effective `model` key), its `models_cache.json` entry,
/// and the context window: `target.context_window`, else `model_context_window` clamped to the
/// catalog's `max_context_window` (`models-manager/src/model_info.rs:20-27`), else the catalog's
/// `context_window`. The catalog's two values count only from an accepted snapshot
/// ([`CacheUse::Used`]); without one a configured window is named but unknown, since the
/// catalog's maximum may lower it. `installed` is the codex whose version the snapshot must
/// match.
fn model_facts(
    fs: &Fs,
    target: &Target,
    codex_home: &Path,
    config: &Config,
    installed: &Installed,
    warnings: &mut Vec<String>,
) -> ModelFacts {
    let mut m = ModelFacts::default();
    if let Some(slug) = &target.model {
        m.slug = Some(slug.clone());
        m.slug_from = Some("target".to_string());
    } else if let Some((value, layer)) = config.get("/model") {
        match value.as_str() {
            Some(slug) => {
                m.slug = Some(slug.to_string());
                m.slug_from = Some(layer.at("/model"));
            }
            // Set but unreadable: unknown, never a lower layer's model.
            None => m.slug_unreadable = Some(layer.at("/model")),
        }
    }
    if let Some((_, layer)) = config.get("/model_catalog_json") {
        m.catalog_unread = Some(layer.at("/model_catalog_json"));
        if let Some(w) = target.context_window {
            m.context_window = Some(w);
            m.window_from = Some("target".to_string());
        }
        return m;
    }
    let cache_path = codex_home.join("models_cache.json");
    let json = fs.read_json(&cache_path);
    m.cache_use = cache_use(&json, installed);
    let cache = match json {
        Json::Ok(v) => Some(v),
        Json::Invalid(err) => {
            warnings.push(format!(
                "{}: invalid JSON, ignored ({err})",
                cache_path.display()
            ));
            None
        }
        Json::Missing => None,
    };
    if let Some(cache) = &cache {
        m.catalog_version = cache
            .get("client_version")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(slug) = &m.slug {
            m.catalog = cache
                .get("models")
                .and_then(Value::as_array)
                .and_then(|models| {
                    models
                        .iter()
                        .find(|e| e.get("slug").and_then(Value::as_str) == Some(slug.as_str()))
                })
                .cloned();
        }
    }
    // Catalog facts only from an accepted snapshot (the contract on `CacheUse`).
    let accepted = m
        .catalog
        .as_ref()
        .filter(|_| matches!(m.cache_use, CacheUse::Used { .. }));
    let catalog_window = accepted
        .and_then(|e| e.get("context_window"))
        .and_then(Value::as_u64);
    let catalog_max = accepted
        .and_then(|e| e.get("max_context_window"))
        .and_then(Value::as_u64);
    if let Some(w) = target.context_window {
        m.context_window = Some(w);
        m.window_from = Some("target".to_string());
    } else if let Some((value, layer)) = config.get("/model_context_window") {
        // Set but not a non-negative integer: unknown, never the catalog's value instead.
        if let Some(w) = value.as_u64() {
            let at = layer.at("/model_context_window");
            match m.cache_use.reason() {
                Some(why) => {
                    m.window_from = Some(format!(
                        "configured {w} ({at}); the catalog's max_context_window may lower it, and models_cache.json is not used: {why}"
                    ));
                }
                None => {
                    m.context_window = Some(catalog_max.map_or(w, |max| w.min(max)));
                    m.window_from = Some(match catalog_max {
                        Some(max) if max < w => {
                            format!("{at} clamped to models_cache.json max_context_window {max}")
                        }
                        _ => at,
                    });
                }
            }
        }
    } else if let Some(w) = catalog_window {
        m.context_window = Some(w);
        m.window_from = Some("models_cache.json".to_string());
    }
    m
}

/// The installed codex, from files only: the first `codex` on the target's `PATH`, followed
/// through symlinks, its version named by the `@openai/codex` `package.json` above it (npm, also
/// under a versioned Node such as `…/node/22.1.0/lib/node_modules/…`), else by a `x.y.z` path
/// component right under a `codex` component (Homebrew: `…/Caskroom/codex/0.160.1/bin/codex`);
/// neither = [`Installed::Unversioned`]. Never runs the binary.
fn installed_codex(target: &Target) -> Installed {
    match codex_on_path(target) {
        None => Installed::NotFound,
        Some(exe) => version_of(&exe).map_or(Installed::Unversioned, Installed::Version),
    }
}

fn codex_on_path(target: &Target) -> Option<PathBuf> {
    let path = target.env.get("PATH")?;
    std::env::split_paths(path)
        .map(|d| d.join("codex"))
        .find(|p| is_executable(p))
}

fn version_of(exe: &Path) -> Option<String> {
    let real = std::fs::canonicalize(exe).ok()?;
    let from_package = real.ancestors().skip(1).find_map(|dir| {
        let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
        let pkg: Value = serde_json::from_str(&text).ok()?;
        (pkg.get("name").and_then(Value::as_str) == Some("@openai/codex"))
            .then(|| {
                pkg.get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .flatten()
    });
    if from_package.is_some() {
        return from_package;
    }
    let parts: Vec<&str> = real
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    parts
        .windows(2)
        .rfind(|w| w[0] == "codex" && version_key(w[1]).is_some_and(|k| k.len() == 3))
        .map(|w| w[1].to_string())
}

/// A regular file a shell would run from `PATH` (any execute bit on Unix).
fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

fn version_key(v: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = v.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| p.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
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
            "harn-codex-mod-{}-{}",
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

    fn target(root: &Path) -> Target {
        Target {
            dir: root.join("repo"),
            home: root.join("home"),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        }
    }

    #[test]
    fn managed_key_leaves_no_writable_knob() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join("home/.codex/config.toml"), "");
        write(
            &root.join("etc/codex/managed_config.toml"),
            "include_permissions_instructions = true\n",
        );
        let tree = resolve_with(&target(root), None, &root.join("etc/codex")).unwrap();
        let row = tree
            .items
            .iter()
            .find(|i| i.id.ends_with("/permissions_instructions"))
            .unwrap();
        match row.knobs.as_slice() {
            [Knob::None { reason }] => assert!(reason.contains("managed_config.toml"), "{reason}"),
            other => panic!("{other:?}"),
        }
        // The environment block is not set by managed: the user file is its knob.
        let env = tree
            .items
            .iter()
            .find(|i| i.id.ends_with("/environment_context"))
            .unwrap();
        assert!(matches!(
            env.knobs.as_slice(),
            [Knob::Setting {
                scope: Scope::User,
                ..
            }]
        ));
    }

    #[test]
    fn developer_instructions_knob_masks_from_the_user_file_unless_managed() {
        let t = tmp();
        let root = &t.0;
        std::fs::create_dir_all(root.join("repo")).unwrap();
        write(&root.join("home/.codex/config.toml"), "");
        write(
            &root.join("etc/codex/config.toml"),
            "developer_instructions = \"be brief\"\n",
        );
        let tree = resolve_with(&target(root), None, &root.join("etc/codex")).unwrap();
        let row = tree
            .items
            .iter()
            .find(|i| i.id.ends_with("#/developer_instructions"))
            .unwrap();
        assert_eq!(row.origin.layer, Layer::Managed);
        assert_eq!(row.cost.predicted, Some(8));
        // The system layer sits below the user layer: `""` in the user file masks it.
        match row.knobs.as_slice() {
            [Knob::Setting {
                file, value, scope, ..
            }] => {
                assert_eq!(file, &root.join("home/.codex/config.toml"));
                assert_eq!(value, &Value::String(String::new()));
                assert_eq!(*scope, Scope::User);
            }
            other => panic!("{other:?}"),
        }

        write(
            &root.join("etc/codex/managed_config.toml"),
            "developer_instructions = \"managed\"\n",
        );
        let tree = resolve_with(&target(root), None, &root.join("etc/codex")).unwrap();
        let row = tree
            .items
            .iter()
            .find(|i| i.id.ends_with("#/developer_instructions"))
            .unwrap();
        assert_eq!(row.cost.predicted, Some(7));
        assert!(matches!(row.knobs.as_slice(), [Knob::None { .. }]));
    }

    #[test]
    #[cfg(unix)]
    fn installed_codex_follows_the_codex_on_path() {
        let t = tmp();
        let root = &t.0;
        let mut target = target(root);
        assert_eq!(installed_codex(&target), Installed::NotFound, "no PATH");

        // Homebrew cask layout: the version is a path component of the symlink target.
        use std::os::unix::fs::PermissionsExt;
        let exec =
            |p: &Path| std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cask = root.join("Caskroom/codex/0.161.0/bin/codex");
        write(&cask, "binary");
        exec(&cask);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::os::unix::fs::symlink(&cask, root.join("bin/codex")).unwrap();
        target.env.insert(
            "PATH".to_string(),
            format!(
                "{}:{}",
                root.join("empty").display(),
                root.join("bin").display()
            ),
        );
        assert_eq!(
            installed_codex(&target),
            Installed::Version("0.161.0".to_string())
        );

        // npm under a versioned Node: the package.json decides, not Node's version component.
        let npm = root.join("Cellar/node/22.1.0");
        write(
            &npm.join("lib/node_modules/@openai/codex/package.json"),
            r#"{"name": "@openai/codex", "version": "0.160.1"}"#,
        );
        write(
            &npm.join("lib/node_modules/@openai/codex/bin/codex.js"),
            "js",
        );
        exec(&npm.join("lib/node_modules/@openai/codex/bin/codex.js"));
        std::fs::create_dir_all(npm.join("bin")).unwrap();
        std::os::unix::fs::symlink(
            npm.join("lib/node_modules/@openai/codex/bin/codex.js"),
            npm.join("bin/codex"),
        )
        .unwrap();
        target
            .env
            .insert("PATH".to_string(), npm.join("bin").display().to_string());
        assert_eq!(
            installed_codex(&target),
            Installed::Version("0.160.1".to_string())
        );

        // A version component not under `codex`, and no package.json: unknown.
        let other = root.join("other/1.2.3/bin/codex");
        write(&other, "binary");
        exec(&other);
        target.env.insert(
            "PATH".to_string(),
            other.parent().unwrap().display().to_string(),
        );
        assert_eq!(installed_codex(&target), Installed::Unversioned);
    }

    #[test]
    fn version_key_orders_numerically() {
        assert!(version_key("0.162.1") > version_key("0.160.1"));
        assert!(version_key("0.160.10") > version_key("0.160.9"));
    }
}
