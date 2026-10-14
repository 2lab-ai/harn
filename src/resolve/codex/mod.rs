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
    pub window_from: Option<String>,
    /// The catalog entry of the slug in `$CODEX_HOME/models_cache.json`.
    pub catalog: Option<Value>,
    /// `client_version` of `models_cache.json`: the codex that last refreshed the catalog.
    pub catalog_version: Option<String>,
    /// `"<file>#/model_catalog_json"` when config names its own catalog: harn does not read it,
    /// so every catalog fact (base instructions, window) stays unknown rather than mixing in
    /// `models_cache.json`.
    pub catalog_unread: Option<String>,
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
    let model = model_facts(&fs, target, &codex_home, &config, &mut warnings);
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
    tree.agent_version = agent_version(target);
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
        _ => format!("{model} · context window unknown"),
    };
    // The cache is rewritten by whichever codex refreshed it last (two installs on one machine
    // take turns), so its version is a fact about the file, not the installed codex.
    match &m.catalog_version {
        Some(v) => format!("{model} · models_cache.json written by codex {v}"),
        None => model,
    }
}

/// The model (`target.model`, else the effective `model` key), its `models_cache.json` entry,
/// and the context window: `target.context_window`, else `model_context_window` clamped to the
/// catalog's `max_context_window` (`models-manager/src/model_info.rs:20-27`), else the catalog's
/// `context_window`.
fn model_facts(
    fs: &Fs,
    target: &Target,
    codex_home: &Path,
    config: &Config,
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
    let cache = match fs.read_json(&cache_path) {
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
    let catalog_window = m
        .catalog
        .as_ref()
        .and_then(|e| e.get("context_window"))
        .and_then(Value::as_u64);
    let catalog_max = m
        .catalog
        .as_ref()
        .and_then(|e| e.get("max_context_window"))
        .and_then(Value::as_u64);
    if let Some(w) = target.context_window {
        m.context_window = Some(w);
        m.window_from = Some("target".to_string());
    } else if let Some((value, layer)) = config.get("/model_context_window") {
        // Set but not a non-negative integer: unknown, never the catalog's value instead.
        if let Some(w) = value.as_u64() {
            m.context_window = Some(catalog_max.map_or(w, |max| w.min(max)));
            m.window_from = Some(match catalog_max {
                Some(max) if max < w => format!(
                    "{} clamped to models_cache.json max_context_window {max}",
                    layer.at("/model_context_window")
                ),
                _ => layer.at("/model_context_window"),
            });
        }
    } else if let Some(w) = catalog_window {
        m.context_window = Some(w);
        m.window_from = Some("models_cache.json".to_string());
    }
    m
}

/// The installed codex version, from files only: the first `codex` on the target's `PATH`,
/// followed through symlinks, named by the `@openai/codex` `package.json` above it (npm, also
/// under a versioned Node such as `…/node/22.1.0/lib/node_modules/…`), else by a `x.y.z` path
/// component right under a `codex` component (Homebrew: `…/Caskroom/codex/0.160.1/bin/codex`).
/// Never runs the binary.
fn agent_version(target: &Target) -> Option<String> {
    let path = target.env.get("PATH")?;
    let exe = std::env::split_paths(path)
        .map(|d| d.join("codex"))
        .find(|p| is_executable(p))?;
    let real = std::fs::canonicalize(&exe).ok()?;
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
    fn agent_version_follows_the_codex_on_path() {
        let t = tmp();
        let root = &t.0;
        let mut target = target(root);
        assert_eq!(agent_version(&target), None, "no PATH");

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
        assert_eq!(agent_version(&target).as_deref(), Some("0.161.0"));

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
        assert_eq!(agent_version(&target).as_deref(), Some("0.160.1"));

        // A version component not under `codex`, and no package.json: unknown.
        let other = root.join("other/1.2.3/bin/codex");
        write(&other, "binary");
        exec(&other);
        target.env.insert(
            "PATH".to_string(),
            other.parent().unwrap().display().to_string(),
        );
        assert_eq!(agent_version(&target), None);
    }

    #[test]
    fn version_key_orders_numerically() {
        assert!(version_key("0.162.1") > version_key("0.160.1"));
        assert!(version_key("0.160.10") > version_key("0.160.9"));
    }
}
