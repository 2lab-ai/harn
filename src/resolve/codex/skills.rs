//! The skill listing (`research/02` §3), as codex-cli 0.160.1 builds it (paths below are relative
//! to `codex-rs/` at commit d27764b8; `render.rs` = `ext/skills/src/render.rs`).
//!
//! Roots, in Codex's order (`ext/skills/src/host_roots.rs:56-131`), none of them gated by trust:
//! `<dir>/.codex/skills` of every project layer, closest first and disabled (untrusted) layers
//! included; `$CODEX_HOME/skills` and `~/.agents/skills` (user); `$CODEX_HOME/skills/.system`
//! (bundled, dropped by `skills.bundled.enabled = false`, `ext/skills/src/host_service.rs:221-236`);
//! `/etc/codex/skills` (admin); each loaded plugin's skill roots; then `<dir>/.agents/skills` for
//! every dir from the project root down to the target. Each root is walked for `SKILL.md` up to 6
//! levels deep, hidden dirs pruned, dir symlinks followed except in `.system`
//! (`ext/skills/src/loader/discovery.rs:54-191`, `loader/host.rs:146-185`).
//!
//! A `SKILL.md` needs a frontmatter with a non-empty `description`; `name` defaults to the dir name
//! and may not exceed 64 chars; both are whitespace-collapsed (`skills/src/parser.rs:44-96`,
//! `loader/host.rs:394-404`). A skill that fails is skipped with a warning (silently in `.system`,
//! `loader/host.rs:331-334`). Duplicates are dropped by canonical `SKILL.md` path only, never by
//! name (`loader/host_merge.rs:201-249`). `[[skills.config]]` rules disable skills; Codex reads
//! them from the user (and profile) layers and from `-c` session flags only
//! (`config/src/skills_config.rs:150-186`). harn resolves without launch flags, so a session-flag
//! rule is not modelled: a knob written to the user config can still be overridden at launch.
//! `agents/openai.yaml` (read with serde_yaml, as codex does) `policy.allow_implicit_invocation:
//! false` keeps a skill out of the listing (`ext/skills/src/provider/host.rs:147-149`), and a
//! `policy.products` list without `codex` keeps it out of the CLI (`skills/src/model.rs:95-108`).
//!
//! The listing is one developer block: `<skills_instructions>` + body + `</skills_instructions>`
//! (`ext/skills/src/world_state.rs:206-208`, `protocol/src/protocol.rs:126-127`); the body is
//! `\n` + lines joined by `\n` + `\n` (`ext/skills/src/catalog_prompt.rs:81-106`) with one line per
//! skill, `- {name}: {description} ({locator_kind}: {locator})` (render.rs:258-267), in catalog
//! order System, Admin, Repo, User, then name, then path (render.rs:48-67). The locator is the
//! `SKILL.md` path, or `rN/…` against a `### Skill roots` table when that costs fewer tokens
//! (render.rs:983-1061, `aliases.rs:13-50`, `host_aliases.rs:8-68`). `### How to use skills`
//! follows only when the model's catalog entry sets `include_skills_usage_instructions`
//! (`fragments.rs:18-36`; a `#[serde(default)] bool`, `protocol/src/openai_models.rs:432-433`, so
//! missing is false; unknown while harn does not accept `models_cache.json` as its snapshot —
//! the contract on `CacheUse` — [`super::Ctx::catalog_flag`]). SKILL.md bodies are not loaded
//! (entries are `advertised`).
//!
//! Budget: `skills.max_context_tokens` capped at 10,000 when set, else 2 % of the context window
//! (render.rs:126-152), counted in Codex's own tokens, `ceil(bytes / 4)` per line + `\n`
//! (render.rs:154-160, `utils/string/src/truncate.rs:4`, `:71-74`) — a code constant, so harn
//! decides the fit from bytes whenever the window is known. Over the budget Codex trims
//! descriptions round-robin (render.rs:331-453); that cut is not modelled: the rows keep their
//! full size as an estimate and the listing says so.
//!
//! Bundled skills: when codex starts it compares the fingerprint of its embedded skill set with
//! the one in `$CODEX_HOME/skills/.system/.codex-system-skills.marker`, and replaces the folder
//! with its own set only when they differ (`skills/src/lib.rs:59-121`, the skip at `:84-99`). The
//! set depends on the codex version, and every codex on a machine (an app, a daemon, the CLI)
//! shares the folder, so it holds the set of the last codex whose fingerprint differed. harn
//! cannot know a codex's embedded set without its binary: each `.system` row carries a
//! [`BUNDLED_SKILL`] signal, the tree one note ([`bundled_note`]), and verify does not count a
//! `.system` difference as a failure.
//!
//! Attribution (as the AGENTS.md message): each entry row costs its line + `\n`; the
//! `skills_instructions` row costs the rest of the block (tags, intro, roots table, usage), so a
//! sum over rows counts every byte once. The `skills` listing row is a container (its cost is the
//! sum of its entries, skipped by the totals).

use super::config::Source;
use super::plugins::Plugin;
use super::{config_knobs, new_item, Ctx, Installed};
use crate::model::{item_id, Agent, Cost, Item, Kind, Knob, Layer, Mode, Scope};
use crate::resolve::claude::{cost_na, frontmatter, fs};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Pushed to `Tree.warnings` when the budget cannot be computed (the window is unknown to harn).
pub const CAP_STATE_UNKNOWN: &str =
    "codex skill listing budget unknown (the model's context window is unknown: see the model: signal); fit state unknown until a capture";

/// Signal prefix of a skill row found in `$CODEX_HOME/skills/.system` (see the module docs): its
/// presence and listing line are what the folder holds now, which the codex that runs replaces
/// with its own set when it starts if that set's fingerprint differs. Verify treats `item.signal.starts_with(BUNDLED_SKILL)`
/// as not checkable when codex lists it otherwise or not at all.
pub const BUNDLED_SKILL: &str = "bundled-skill:";

/// What the `.system` folder holds, after its path; shared by the rows' signal, the tree note and
/// verify's reasons.
pub const BUNDLED_SKILLS_WHY: &str = "holds the system skills of the last codex that installed its set there (a codex replaces it with its own embedded set when that set's fingerprint differs from the folder's marker, skills/src/lib.rs:59-121)";

/// The `origin.why` prefix under which the framing row records each line of the roots table it
/// predicts (`` - `rN` = `<dir>` ``).
pub const ROOT_PREFIX: &str = "rendered root: ";

/// The `origin.why` prefix under which the framing row names the roots-table value that is the
/// bundled root (`$CODEX_HOME/skills/.system` resolved), when the table it predicts has it.
pub const BUNDLED_ROOT_PREFIX: &str = "bundled root: ";

/// The listing container's id name and the framing row's id name.
pub(crate) const LISTING: &str = "skills";
pub(crate) const FRAMING: &str = "skills_instructions";

const OPEN: &str = "<skills_instructions>";
const CLOSE: &str = "</skills_instructions>";
const SKILLS_FILE: &str = "SKILL.md";
/// render.rs:20, :22, :23-24.
const MAX_CONFIGURED_TOKENS: usize = 10_000;
const WINDOW_PERCENT: u64 = 2;
const MAX_DESCRIPTION_CHARS: usize = 1_024;
const TRUNCATED_SUFFIX: &str = "...";
/// `utils/string/src/truncate.rs:4`.
const BYTES_PER_TOKEN: usize = 4;
/// `skills/src/parser.rs:4`; `ext/skills/src/loader/mod.rs:23`, `:31-32`.
const MAX_NAME_CHARS: usize = 64;
const MAX_QUALIFIED_NAME_CHARS: usize = MAX_NAME_CHARS * 2 + 1;
const MAX_SCAN_DEPTH: usize = 6;
const MAX_DIRS_PER_ROOT: usize = 2_000;

// `ext/skills/src/catalog_prompt.rs:3-40`, verbatim.
const SKILLS_INTRO_WITH_SOURCE_LOCATORS: &str = "A skill is a set of instructions provided through a `SKILL.md` source. Below is the list of skills that can be used. Each entry includes a name, description, and source locator. `file` locators are on the host filesystem, `executor package` locators are owned by their execution environment, `cloud package` locators are opaque package identifiers, and `custom resource` locators use their provider's access mechanism.";
const SKILLS_INTRO_WITH_HOST_ALIASES: &str = "A skill is a set of local instructions to follow that is stored in a `SKILL.md` file. Below is the list of skills that can be used. Each entry includes a name, description, and a short path that can be expanded into an absolute path using the skill roots table.";
const SKILLS_HOW_TO_USE_WITH_SOURCE_LOCATORS: &str = r###"- Discovery: The list above is the skills available in this session (name + description + source locator). `file` entries live on the host filesystem, `executor package` and `cloud package` entries are accessed directly through `skills.read`, and `custom resource` entries use their provider's access mechanism.
- Trigger rules: If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a skill's description shown above, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.
- Missing/blocked: If a named skill isn't in the list or its source can't be read, say so briefly and continue with the best fallback.
- How to use a skill (progressive disclosure):
  1) After deciding to use a skill, the main agent must read its `SKILL.md` completely before taking task actions. For a `file` entry, open the listed path. For an `executor package` or `cloud package`, pass the listed locator directly to `skills.read` as `package`; root aliases are resolved automatically. Omit `resource` to read `SKILL.md` directly without calling `skills.list`. If a read is paginated, follow `next_cursor` until EOF.
  2) When `SKILL.md` references another resource, use the same access mechanism. For executor and cloud skills, pass the complete package-contained resource identifier with the same package to `skills.read`; do not treat `skill://` identifiers as filesystem paths.
  3) If `SKILL.md` points to extra folders such as `references/`, use its routing instructions to identify the resources required for the task. The main agent must read each required instruction or reference file itself before acting on it. Do not delegate reading, summarizing, or interpreting skill instructions to a subagent. Subagents may still perform task work when the selected skill allows it.
  4) For filesystem-backed skills, prefer running or patching provided scripts instead of retyping large code blocks. For executor and cloud skills, use `skills.read` and the available tools; do not invent a local path.
  5) Reuse provided assets or templates through the same source access mechanism instead of recreating them.
- Coordination and sequencing:
  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.
  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.
- Context hygiene:
  - Progressive disclosure applies to selecting relevant files, not partially reading a selected instruction file. Do not load unrelated references, scripts, or assets.
  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.
  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.
- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue."###;
const SKILLS_HOW_TO_USE_WITH_HOST_ALIASES: &str = r###"- Discovery: The list above is the skills available in this session (name + description + short path). Skill bodies live on disk at the listed paths after expanding the matching alias from `### Skill roots`.
- Trigger rules: If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a skill's description shown above, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.
- Missing/blocked: If a named skill isn't in the list or the path can't be read, say so briefly and continue with the best fallback.
- How to use a skill (progressive disclosure):
  1) After deciding to use a skill, the main agent must expand the listed short `path` with the matching alias from `### Skill roots`, then open and read its `SKILL.md` completely before taking task actions. If a read is truncated or paginated, continue until EOF.
  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the directory containing that expanded `SKILL.md` first, and only consider other paths if needed.
  3) If `SKILL.md` points to extra folders such as `references/`, use its routing instructions to identify the files required for the task. The main agent must read each required instruction or reference file itself before acting on it. Do not delegate reading, summarizing, or interpreting skill instructions to a subagent. Subagents may still perform task work when the selected skill allows it.
  4) If `scripts/` exist, prefer running or patching them instead of retyping large code blocks.
  5) If `assets/` or templates exist, reuse them instead of recreating from scratch.
- Coordination and sequencing:
  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.
  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.
- Context hygiene:
  - Progressive disclosure applies to selecting relevant files, not partially reading a selected instruction file. Do not load unrelated references, scripts, or assets.
  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.
  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.
- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue."###;

/// `SkillScope` (`protocol`): where a root sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillScope {
    Repo,
    User,
    System,
    Admin,
}

impl SkillScope {
    /// Catalog order (render.rs:51-57).
    fn catalog_rank(self) -> u8 {
        match self {
            SkillScope::System => 0,
            SkillScope::Admin => 1,
            SkillScope::Repo => 2,
            SkillScope::User => 3,
        }
    }

    fn layer(self) -> Layer {
        match self {
            SkillScope::Repo => Layer::Project,
            SkillScope::User => Layer::User,
            SkillScope::System => Layer::Builtin,
            SkillScope::Admin => Layer::Managed,
        }
    }
}

struct Root {
    path: PathBuf,
    scope: SkillScope,
    /// Index into the plugins slice for a plugin's skill root.
    plugin: Option<usize>,
    origin: String,
}

struct Skill {
    /// Qualified (`namespace:name` for a plugin skill).
    name: String,
    /// Whitespace-collapsed, not yet cut.
    description: String,
    /// Canonical `SKILL.md` path: the identity and the `[[skills.config]]` selector.
    path: PathBuf,
    /// The path as discovered under the canonical root (the rendered locator).
    display: String,
    /// Canonical root that found it.
    root: PathBuf,
    scope: SkillScope,
    plugin: Option<usize>,
    /// `agents/openai.yaml` `policy.allow_implicit_invocation` is not `false`.
    implicit: bool,
    origin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    Unaliased,
    HostAliases,
}

impl PromptKind {
    fn intro(self) -> &'static str {
        match self {
            PromptKind::Unaliased => SKILLS_INTRO_WITH_SOURCE_LOCATORS,
            PromptKind::HostAliases => SKILLS_INTRO_WITH_HOST_ALIASES,
        }
    }

    fn usage(self) -> &'static str {
        match self {
            PromptKind::Unaliased => SKILLS_HOW_TO_USE_WITH_SOURCE_LOCATORS,
            PromptKind::HostAliases => SKILLS_HOW_TO_USE_WITH_HOST_ALIASES,
        }
    }
}

/// `approx_token_count` (`utils/string/src/truncate.rs:71-74`).
fn tokens(bytes: usize) -> usize {
    bytes.div_ceil(BYTES_PER_TOKEN)
}

/// `metadata_line_cost` for a token budget (render.rs:154-160): the line and its newline.
fn line_tokens(line: &str) -> usize {
    tokens(line.len() + 1)
}

/// `sanitize_single_line` (`skills/src/parser.rs:94-96`).
fn sanitize(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `truncate_catalog_skill_description` (render.rs:1091-1109): 1,024 chars, else 1,021 + `...`.
fn cut_description(description: &str) -> (String, bool) {
    if description.chars().nth(MAX_DESCRIPTION_CHARS).is_none() {
        return (description.to_string(), false);
    }
    let keep = MAX_DESCRIPTION_CHARS - TRUNCATED_SUFFIX.chars().count();
    let mut out: String = description.chars().take(keep).collect();
    out.push_str(TRUNCATED_SUFFIX);
    (out, true)
}

/// One catalog line (render.rs:258-267); host skills are `file` locators.
fn render_line(name: &str, description: &str, locator: &str) -> String {
    if description.is_empty() {
        format!("- {name}: (file: {locator})")
    } else {
        format!("- {name}: {description} (file: {locator})")
    }
}

/// `render_available_skills_body` (`catalog_prompt.rs:81-106`) with the usage section of
/// `AvailableSkillsInstructions::from_skill_lines` (`fragments.rs:18-36`).
fn render_body(kind: PromptKind, roots: &[String], lines: &[String], usage: bool) -> String {
    let mut out: Vec<&str> = vec!["## Skills", kind.intro()];
    if !roots.is_empty() {
        out.push("### Skill roots");
        out.extend(roots.iter().map(String::as_str));
    }
    out.push("### Available skills");
    out.extend(lines.iter().map(String::as_str));
    if usage {
        out.push("### How to use skills");
        out.push(kind.usage());
    }
    format!("\n{}\n", out.join("\n"))
}

/// `extract_frontmatter` (`skills/src/parser.rs:200-221`): the first line must be `---` (trimmed),
/// up to the next `---`; empty or unclosed = none.
fn extract_frontmatter(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut out = Vec::new();
    for line in lines {
        if line.trim() == "---" {
            return (!out.is_empty()).then(|| out.join("\n"));
        }
        out.push(line);
    }
    None
}

/// `(name, description)` of a `SKILL.md` (`skills/src/parser.rs:44-92`), or why it is skipped.
fn parse_skill(text: &str, path: &Path) -> Result<(String, String), String> {
    let yaml = extract_frontmatter(text).ok_or("missing YAML frontmatter delimited by ---")?;
    let map = frontmatter::parse_yaml(&yaml);
    let name = frontmatter::get_str(&map, "name")
        .map(|n| sanitize(&n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_name(path));
    let description = frontmatter::get_str(&map, "description")
        .map(|d| sanitize(&d))
        .unwrap_or_default();
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "invalid name: exceeds maximum length of {MAX_NAME_CHARS} characters"
        ));
    }
    if description.is_empty() {
        return Err("missing field `description`".to_string());
    }
    Ok((name, description))
}

/// The dir name, sanitized; `skill` when empty (`loader/host.rs:394-404`).
fn default_name(path: &Path) -> String {
    path.parent()
        .map(|p| sanitize(&fs::file_name(p)))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "skill".to_string())
}

/// `SkillMetadataFile` (`ext/skills/src/loader/metadata.rs:27-75`): `agents/openai.yaml`, typed as
/// codex types it so a file codex cannot deserialize is rejected here too. Only `policy` is used;
/// the other fields are typed for that check alone.
#[derive(serde::Deserialize, Default)]
#[allow(dead_code)]
struct MetadataFile {
    #[serde(default)]
    interface: Option<InterfaceFile>,
    #[serde(default)]
    dependencies: Option<Dependencies>,
    #[serde(default)]
    policy: Option<MetadataPolicy>,
}

/// `SkillInterfaceFile` (`skills/src/interface.rs:13-22`).
#[derive(serde::Deserialize, Default)]
#[allow(dead_code)]
struct InterfaceFile {
    display_name: Option<String>,
    short_description: Option<String>,
    icon_small: Option<PathBuf>,
    icon_large: Option<PathBuf>,
    brand_color: Option<String>,
    default_prompt: Option<String>,
}

#[derive(serde::Deserialize, Default)]
#[allow(dead_code)]
struct Dependencies {
    #[serde(default)]
    tools: Vec<DependencyTool>,
}

#[derive(serde::Deserialize, Default)]
#[allow(dead_code)]
struct DependencyTool {
    #[serde(rename = "type")]
    kind: Option<String>,
    value: Option<String>,
    description: Option<String>,
    transport: Option<String>,
    command: Option<String>,
    url: Option<String>,
    oauth: Option<DependencyOAuth>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct DependencyOAuth {
    #[serde(alias = "callback_port")]
    callback_port: Option<u16>,
}

#[derive(serde::Deserialize)]
struct MetadataPolicy {
    #[serde(default)]
    allow_implicit_invocation: Option<bool>,
    #[serde(default)]
    products: Vec<Product>,
}

/// `Product` (`protocol/src/protocol.rs:3849-3859`).
#[derive(serde::Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Product {
    #[serde(alias = "CHATGPT")]
    Chatgpt,
    #[serde(alias = "CODEX")]
    Codex,
    #[serde(alias = "ATLAS")]
    Atlas,
}

/// What `agents/openai.yaml` beside a `SKILL.md` decides.
struct SkillPolicy {
    /// `allow_implicit_invocation` is not `false` (`skills/src/model.rs:22-27`).
    implicit: bool,
    /// `products` is empty or names codex: the CLI loads the skill only then
    /// (`skills/src/model.rs:95-108`, `loader/host_merge.rs:149-153`, `host_service.rs:121-123`).
    for_codex: bool,
}

/// Reads `agents/openai.yaml` with serde_yaml as codex does; an unreadable or invalid file is
/// ignored (fail open, `loader/metadata.rs:83-139`).
fn skill_policy(ctx: &Ctx, skill_md: &Path, warnings: &mut Vec<String>) -> SkillPolicy {
    let open = SkillPolicy {
        implicit: true,
        for_codex: true,
    };
    let Some(dir) = skill_md.parent() else {
        return open;
    };
    let path = dir.join("agents/openai.yaml");
    let Some(text) = ctx.fs.read_string(&path) else {
        return open;
    };
    let policy = match serde_yaml::from_str::<MetadataFile>(&text) {
        Ok(file) => file.policy,
        Err(err) => {
            warnings.push(format!(
                "{}: invalid agents/openai.yaml, codex ignores it ({err}; ext/skills/src/loader/metadata.rs:130-139)",
                ctx.display(&path)
            ));
            return open;
        }
    };
    let Some(policy) = policy else {
        return open;
    };
    SkillPolicy {
        implicit: policy.allow_implicit_invocation != Some(false),
        for_codex: policy.products.is_empty() || policy.products.contains(&Product::Codex),
    }
}

/// Every `SKILL.md` under `root` (the canonical root), as discovered: entries 1–6 levels deep,
/// hidden dirs pruned, file symlinks omitted, dir symlinks followed when `follow`
/// (`loader/discovery.rs:54-191`). Stops at 2,000 dirs.
fn walk(ctx: &Ctx, root: &Path, follow: bool, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut state = WalkState {
        dirs: 0,
        truncated: false,
        seen: HashSet::from([root.to_path_buf()]),
        out: Vec::new(),
    };
    walk_dir(ctx, root, 1, follow, &mut state);
    if state.truncated {
        warnings.push(format!(
            "{}: skills scan reached its traversal limit ({MAX_DIRS_PER_ROOT} dirs; ext/skills/src/loader/discovery.rs:109-113)",
            root.display()
        ));
    }
    state.out
}

struct WalkState {
    dirs: usize,
    truncated: bool,
    seen: HashSet<PathBuf>,
    out: Vec<PathBuf>,
}

fn walk_dir(ctx: &Ctx, dir: &Path, depth: usize, follow: bool, state: &mut WalkState) {
    for entry in ctx.fs.read_dir(dir) {
        let Ok(meta) = std::fs::symlink_metadata(&entry) else {
            continue;
        };
        let name = fs::file_name(&entry);
        let is_dir = if meta.file_type().is_symlink() {
            if !follow || !ctx.fs.is_dir(&entry) {
                continue;
            }
            true
        } else {
            meta.is_dir()
        };
        if is_dir {
            if name.starts_with('.') || depth >= MAX_SCAN_DEPTH {
                continue;
            }
            if state.dirs >= MAX_DIRS_PER_ROOT {
                state.truncated = true;
                return;
            }
            if !state.seen.insert(ctx.fs.realpath(&entry)) {
                continue;
            }
            state.dirs += 1;
            walk_dir(ctx, &entry, depth + 1, follow, state);
        } else if meta.is_file() && name == SKILLS_FILE && ctx.fs.is_file(&entry) {
            state.out.push(entry);
        }
    }
}

/// The roots in Codex's order, deduped by path.
fn roots(ctx: &Ctx, plugins: &[Plugin], bundled: bool) -> Vec<Root> {
    let mut out: Vec<Root> = Vec::new();
    for layer in ctx.config.layers.iter().rev() {
        if layer.source != Source::Project {
            continue;
        }
        let Some(dot_codex) = layer.path.as_ref().and_then(|p| p.parent()) else {
            continue;
        };
        let state = match &layer.disabled {
            Some(why) => format!("project layer disabled ({why}); skills load anyway"),
            None => "trusted project layer".to_string(),
        };
        out.push(Root {
            path: dot_codex.join("skills"),
            scope: SkillScope::Repo,
            plugin: None,
            origin: format!("{} ({state})", ctx.display(&dot_codex.join("skills"))),
        });
    }
    let user = ctx.codex_home.join("skills");
    out.push(Root {
        origin: format!(
            "{} (user; deprecated location, still read)",
            ctx.display(&user)
        ),
        path: user,
        scope: SkillScope::User,
        plugin: None,
    });
    let agents = ctx.home().join(".agents/skills");
    out.push(Root {
        origin: format!("{} (user)", ctx.display(&agents)),
        path: agents,
        scope: SkillScope::User,
        plugin: None,
    });
    if bundled {
        let system = ctx.codex_home.join("skills/.system");
        out.push(Root {
            origin: format!("{} (bundled with codex)", ctx.display(&system)),
            path: system,
            scope: SkillScope::System,
            plugin: None,
        });
    }
    let admin = ctx.system_dir.join("skills");
    out.push(Root {
        origin: format!("{} (admin)", admin.display()),
        path: admin,
        scope: SkillScope::Admin,
        plugin: None,
    });
    for (i, p) in plugins.iter().enumerate() {
        for path in p.skill_roots(ctx) {
            out.push(Root {
                origin: format!("{} (plugin {})", path.display(), p.id),
                path,
                scope: SkillScope::User,
                plugin: Some(i),
            });
        }
    }
    let dir = ctx.dir().to_path_buf();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for a in dir.ancestors() {
        dirs.push(a.to_path_buf());
        if ctx.config.agents_root.as_deref().is_none_or(|r| a == r) {
            break;
        }
    }
    dirs.reverse();
    for d in dirs {
        let path = d.join(".agents/skills");
        if ctx.fs.is_dir(&path) {
            out.push(Root {
                origin: format!("{} (repo)", ctx.display(&path)),
                path,
                scope: SkillScope::Repo,
                plugin: None,
            });
        }
    }
    let mut seen: HashSet<PathBuf> = HashSet::new();
    out.retain(|r| seen.insert(fs::normalize_lexical(&r.path)));
    out
}

/// Loads one root: its canonical path and its skills (skipped ones become warnings, except in
/// `.system`).
fn load_root(
    ctx: &Ctx,
    root: &Root,
    plugins: &[Plugin],
    warnings: &mut Vec<String>,
) -> (PathBuf, Vec<Skill>) {
    let canonical = ctx.fs.realpath(&root.path);
    if !ctx.fs.is_dir(&canonical) {
        return (canonical, Vec::new());
    }
    let follow = root.scope != SkillScope::System;
    let mut skills = Vec::new();
    for found in walk(ctx, &canonical, follow, warnings) {
        let path = ctx.fs.realpath(&found);
        let display = found.to_string_lossy().replace('\\', "/");
        let parsed = ctx
            .fs
            .read_string(&path)
            .ok_or_else(|| "failed to read file".to_string())
            .and_then(|text| parse_skill(&text, &path));
        let parsed = parsed.and_then(|(name, description)| {
            let name = match root.plugin.map(|i| &plugins[i]) {
                Some(p) => format!("{}:{name}", p.namespace),
                None => name,
            };
            if name.chars().count() > MAX_QUALIFIED_NAME_CHARS {
                return Err(format!(
                    "invalid qualified name: exceeds maximum length of {MAX_QUALIFIED_NAME_CHARS} characters"
                ));
            }
            Ok((name, description))
        });
        match parsed {
            Ok((name, description)) => {
                let policy = skill_policy(ctx, &path, warnings);
                if !policy.for_codex {
                    warnings.push(format!(
                        "{}: agents/openai.yaml policy.products does not name codex; codex does not load it (skills/src/model.rs:95-108)",
                        ctx.display(&found)
                    ));
                    continue;
                }
                skills.push(Skill {
                    implicit: policy.implicit,
                    name,
                    description,
                    path,
                    display,
                    root: canonical.clone(),
                    scope: root.scope,
                    plugin: root.plugin,
                    origin: root.origin.clone(),
                });
            }
            Err(message) if root.scope != SkillScope::System => warnings.push(format!(
                "{}: skipped by codex: {message} (skills/src/parser.rs:44-92)",
                ctx.display(&found)
            )),
            Err(_) => {}
        }
    }
    (canonical, skills)
}

enum Selector {
    Path(PathBuf),
    Name(String),
}

struct Rule {
    selector: Selector,
    enabled: bool,
    /// The layer the rule came from is the profile file.
    profile: bool,
}

impl Rule {
    fn matches(&self, skill: &Skill) -> bool {
        match &self.selector {
            Selector::Path(p) => *p == skill.path,
            Selector::Name(n) => *n == skill.name,
        }
    }
}

/// Why a layer's `skills` table would not deserialize as `SkillsConfig`
/// (`config/src/skills_config.rs:18-60`): a wrong-typed field anywhere fails the whole table.
/// Unknown keys are accepted (`deny_unknown_fields` there is a schemars attribute, not serde's).
/// A `null` written by an overlay reads as absent.
fn skills_table_error(skills: &Value) -> Option<String> {
    fn get<'v>(table: &'v serde_json::Map<String, Value>, key: &str) -> Option<&'v Value> {
        table.get(key).filter(|v| !v.is_null())
    }
    let Some(table) = skills.as_object() else {
        return Some("`skills` is not a table".to_string());
    };
    if let Some(bundled) = get(table, "bundled") {
        let Some(bundled) = bundled.as_object() else {
            return Some("`bundled` is not a table".to_string());
        };
        if get(bundled, "enabled").is_some_and(|v| !v.is_boolean()) {
            return Some("`bundled.enabled` is not a boolean".to_string());
        }
    }
    if get(table, "include_instructions").is_some_and(|v| !v.is_boolean()) {
        return Some("`include_instructions` is not a boolean".to_string());
    }
    if get(table, "max_context_tokens").is_some_and(|v| !v.as_u64().is_some_and(|n| n > 0)) {
        return Some("`max_context_tokens` is not a positive integer".to_string());
    }
    // No `config` array: valid; `None` here means "no error".
    let config = get(table, "config")?;
    let Some(entries) = config.as_array() else {
        return Some("`config` is not an array of tables".to_string());
    };
    for (i, entry) in entries.iter().enumerate() {
        let Some(entry) = entry.as_object() else {
            return Some(format!("`config[{i}]` is not a table"));
        };
        match get(entry, "enabled") {
            Some(Value::Bool(_)) => {}
            Some(_) => return Some(format!("`config[{i}].enabled` is not a boolean")),
            None => return Some(format!("`config[{i}]` has no `enabled`")),
        }
        for key in ["path", "name"] {
            if get(entry, key).is_some_and(|v| !v.is_string()) {
                return Some(format!("`config[{i}].{key}` is not a string"));
            }
        }
    }
    None
}

/// `skill_config_rules_from_stack` (`config/src/skills_config.rs:150-211`): user and profile
/// layers in order (codex also reads `-c` session-flag layers; harn sees no launch flags, so those
/// are not modelled); a later rule with the same selector replaces an earlier one. A layer whose
/// `skills` table does not deserialize contributes nothing (`:160-169`). A relative `path` is
/// taken against the config file's dir and `~/` against home, as the loader resolves it
/// (`config/src/loader/mod.rs:1424-1446`, `utils/absolute-path/src/lib.rs:59-63`).
fn skill_rules(ctx: &Ctx, warnings: &mut Vec<String>) -> Vec<Rule> {
    let mut rules: Vec<Rule> = Vec::new();
    for layer in &ctx.config.layers {
        if !matches!(layer.source, Source::User | Source::Profile) {
            continue;
        }
        let Some(skills) = layer.value.get("skills").filter(|v| !v.is_null()) else {
            continue;
        };
        if let Some(err) = skills_table_error(skills) {
            warnings.push(format!(
                "{}: {err}; codex cannot read this layer's skills config and applies none of its [[skills.config]] rules (config/src/skills_config.rs:160-169)",
                layer.at("/skills")
            ));
            continue;
        }
        let Some(entries) = skills.get("config").and_then(Value::as_array) else {
            continue;
        };
        let at = layer.at("/skills/config");
        for entry in entries {
            let enabled = entry
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let path = entry.get("path").and_then(Value::as_str);
            let name = entry.get("name").and_then(Value::as_str);
            let selector = match (path, name) {
                (Some(path), None) => {
                    let path = match path.strip_prefix("~/") {
                        Some(rest) => ctx.home().join(rest),
                        None if path == "~" => ctx.home().to_path_buf(),
                        None => PathBuf::from(path),
                    };
                    let path = match (path.is_absolute(), layer.path.as_ref()) {
                        (false, Some(file)) => file.parent().unwrap_or(Path::new("/")).join(path),
                        _ => path,
                    };
                    Selector::Path(std::fs::canonicalize(&path).unwrap_or(path))
                }
                (None, Some(name)) if !name.trim().is_empty() => {
                    Selector::Name(name.trim().to_string())
                }
                _ => {
                    warnings.push(format!(
                        "{at}: an entry needs exactly one non-empty `path` or `name`; codex ignores it (config/src/skills_config.rs:188-211)"
                    ));
                    continue;
                }
            };
            let rule = Rule {
                selector,
                enabled,
                profile: layer.source == Source::Profile,
            };
            rules.retain(|r| !same_selector(&r.selector, &rule.selector));
            rules.push(rule);
        }
    }
    rules
}

fn same_selector(a: &Selector, b: &Selector) -> bool {
    match (a, b) {
        (Selector::Path(a), Selector::Path(b)) => a == b,
        (Selector::Name(a), Selector::Name(b)) => a == b,
        _ => false,
    }
}

/// `resolve_disabled_paths` (`config/src/skills_config.rs:94-125`).
fn disabled_paths(rules: &[Rule], skills: &[Skill]) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    for rule in rules {
        match &rule.selector {
            Selector::Path(p) => {
                if rule.enabled {
                    out.remove(p);
                } else {
                    out.insert(p.clone());
                }
            }
            Selector::Name(_) => {
                for s in skills.iter().filter(|s| rule.matches(s)) {
                    if rule.enabled {
                        out.remove(&s.path);
                    } else {
                        out.insert(s.path.clone());
                    }
                }
            }
        }
    }
    out
}

/// `plugin_marketplace_base` (`host_aliases.rs:43-54`): `…/plugins/cache/<marketplace>`.
fn marketplace_base(path: &Path) -> Option<PathBuf> {
    let mut candidate = path;
    while let Some(parent) = candidate.parent() {
        if parent.file_name()?.to_str()? == "cache"
            && parent.parent()?.file_name()?.to_str()? == "plugins"
        {
            return Some(candidate.to_path_buf());
        }
        candidate = parent;
    }
    None
}

/// `plugin_version_base` (`host_aliases.rs:56-68`): `…/plugins/cache/<marketplace>/<plugin>/<version>`.
fn version_base(path: &Path) -> Option<PathBuf> {
    let base = marketplace_base(path)?;
    let mut rest = path.strip_prefix(&base).ok()?.components();
    let plugin = rest.next()?;
    let version = rest.next()?;
    Some(base.join(plugin).join(version))
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// `(alias, root)` pairs (`render.rs:983-1009`, `host_aliases.rs:8-41`, `aliases.rs:13-30`):
/// entries ordered by their root's discovery order; a plugin version holding one entry shares its
/// marketplace dir as the alias root.
fn alias_plan(entries: &[&Skill], skill_roots: &[PathBuf]) -> Vec<(String, String)> {
    let order = |s: &Skill| {
        skill_roots
            .iter()
            .position(|r| *r == s.root)
            .unwrap_or(usize::MAX)
    };
    let mut ordered: Vec<&Skill> = entries.to_vec();
    ordered.sort_by_key(|s| order(s));
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    for s in &ordered {
        if let Some(v) = version_base(&s.root) {
            *counts.entry(v).or_default() += 1;
        }
    }
    let mut plan: Vec<(String, String)> = Vec::new();
    for s in &ordered {
        let shared = match version_base(&s.root) {
            Some(v) if counts.get(&v).copied().unwrap_or_default() <= 1 => {
                marketplace_base(&s.root).unwrap_or_else(|| s.root.clone())
            }
            _ => s.root.clone(),
        };
        let value = slash(&shared);
        if !plan.iter().any(|(_, v)| *v == value) {
            plan.push((format!("r{}", plan.len()), value));
        }
    }
    plan
}

/// `AliasPlan::shorten` (`aliases.rs:32-43`): the longest root the locator sits under.
fn shorten(plan: &[(String, String)], locator: &str) -> String {
    plan.iter()
        .filter_map(|(name, value)| {
            let suffix = locator
                .strip_prefix(value.trim_end_matches('/'))?
                .strip_prefix('/')?;
            Some((name, value, suffix))
        })
        .max_by_key(|(_, value, _)| value.len())
        .map(|(name, _, suffix)| format!("{name}/{suffix}"))
        .unwrap_or_else(|| locator.to_string())
}

/// The listing budget in Codex tokens and where it comes from (render.rs:126-152).
fn budget(ctx: &Ctx, warnings: &mut Vec<String>) -> (Option<(usize, String)>, Option<String>) {
    if let Some((value, layer)) = ctx.config.get("/skills/max_context_tokens") {
        let at = layer.at("/skills/max_context_tokens");
        match value.as_u64().filter(|n| *n > 0) {
            Some(n) => {
                let limit = (n as usize).min(MAX_CONFIGURED_TOKENS);
                return (
                    Some((
                        limit,
                        format!("{at} = {n}, capped at 10,000 (render.rs:130-136)"),
                    )),
                    None,
                );
            }
            // `NonZeroUsize` (`config/src/skills_config.rs:40-43`): set to anything else, codex
            // refuses to start; the budget is unknown, never the 2 % default.
            None => {
                let why = format!(
                    "{}; codex refuses to start with it; budget unknown",
                    super::hooks::not_a(&at, value, "a positive integer")
                );
                warnings.push(why.clone());
                return (None, Some(why));
            }
        }
    }
    (window_budget(ctx), None)
}

fn window_budget(ctx: &Ctx) -> Option<(usize, String)> {
    let window = ctx.model.context_window?;
    let limit = ((window * WINDOW_PERCENT) / 100).max(1) as usize;
    let from = ctx
        .model
        .window_from
        .clone()
        .unwrap_or_else(|| "unknown source".to_string());
    Some((
        limit,
        format!("2 % of the {window}-token context window ({from}; render.rs:138-148)"),
    ))
}

/// What Codex renders for the visible entries: the locator form, the lines (catalog order), the
/// roots table, and the fit against the budget.
struct Rendered {
    kind: PromptKind,
    roots: Vec<String>,
    lines: Vec<String>,
    cut: Vec<bool>,
    why: Vec<String>,
    /// `Some(true)` fits, `Some(false)` over budget, None = budget unknown.
    fits: Option<bool>,
    signal: Option<String>,
}

fn render(
    entries: &[&Skill],
    skill_roots: &[PathBuf],
    budget: Option<&(usize, String)>,
) -> Rendered {
    let cut: Vec<(String, bool)> = entries
        .iter()
        .map(|s| cut_description(&s.description))
        .collect();
    let absolute: Vec<String> = entries
        .iter()
        .zip(&cut)
        .map(|(s, (d, _))| render_line(&s.name, d, &s.display))
        .collect();
    let plan = alias_plan(entries, skill_roots);
    let aliased: Vec<String> = entries
        .iter()
        .zip(&cut)
        .map(|(s, (d, _))| render_line(&s.name, d, &shorten(&plan, &s.display)))
        .collect();
    let root_lines: Vec<String> = plan
        .iter()
        .map(|(name, value)| format!("- `{name}` = `{value}`"))
        .collect();
    // `aliased_metadata_overhead_cost` (render.rs:1022-1044); host aliases add no alias line.
    let overhead = tokens(render_body(PromptKind::HostAliases, &root_lines, &[], false).len())
        .saturating_sub(tokens(
            render_body(PromptKind::Unaliased, &[], &[], false).len(),
        ));
    let abs_cost: usize = absolute.iter().map(|l| line_tokens(l)).sum();
    let alias_cost: usize = aliased.iter().map(|l| line_tokens(l)).sum();
    let cheaper = overhead + alias_cost < abs_cost;
    let mut why = vec![format!(
        "absolute paths: {abs_cost} codex tokens; `rN/…` short paths: {alias_cost} + {overhead} for the roots table (render.rs:1046-1084)"
    )];
    let (use_alias, fits, signal) = match budget {
        Some((limit, from)) => {
            let limit = *limit;
            why.push(format!("budget {limit} codex tokens: {from}"));
            let abs_fits = abs_cost <= limit;
            let alias_fits = overhead < limit && alias_cost <= limit - overhead;
            match (abs_fits, alias_fits) {
                (true, true) => (cheaper, Some(true), None),
                (false, true) => (true, Some(true), None),
                (true, false) => (false, Some(true), None),
                (false, false) => {
                    let need = abs_cost.min(overhead + alias_cost);
                    (
                        cheaper,
                        Some(false),
                        Some(format!(
                            "over budget: the listing needs {need} codex tokens, the budget is {limit}; codex trims descriptions round-robin (render.rs:331-453), not modelled — sizes are estimates"
                        )),
                    )
                }
            }
        }
        None => (cheaper, None, None),
    };
    let (kind, roots, lines) = if use_alias {
        (PromptKind::HostAliases, root_lines, aliased)
    } else {
        (PromptKind::Unaliased, Vec::new(), absolute)
    };
    let used: usize =
        lines.iter().map(|l| line_tokens(l)).sum::<usize>() + if use_alias { overhead } else { 0 };
    why.push(if use_alias {
        "codex lists short `rN/…` paths against a `### Skill roots` table (fewer tokens)"
            .to_string()
    } else {
        "codex lists absolute SKILL.md paths (the roots table would not save tokens)".to_string()
    });
    match (fits, budget) {
        (Some(true), Some((limit, _))) => why.push(format!(
            "within budget: {used} of {limit} codex tokens (ceil(bytes / 4) per line, render.rs:154-160); assumes no cloud or executor skill catalogs, which share the budget at runtime (world_state_catalogs.rs:394-434)"
        )),
        (None, _) => why.push(
            "budget unknown (see the warnings: the context window is not known, or skills.max_context_tokens is unreadable); the form above assumes everything fits".to_string(),
        ),
        _ => {}
    }
    Rendered {
        kind,
        roots,
        lines,
        cut: cut.into_iter().map(|(_, c)| c).collect(),
        why,
        fits,
        signal,
    }
}

/// The `[[skills.config]]` knob for one skill: appended to the user config (the profile file when
/// it re-enables the skill, since its rules come later).
fn skill_knob(ctx: &Ctx, skill: &Skill, rules: &[Rule]) -> Knob {
    let profile_file = ctx
        .config
        .layers
        .iter()
        .find(|l| l.source == Source::Profile)
        .and_then(|l| l.path.clone());
    let reenabled_by_profile = rules
        .iter()
        .any(|r| r.profile && r.enabled && r.matches(skill));
    let (file, note) = match (profile_file, reenabled_by_profile) {
        (Some(file), true) => (file, "the profile re-enables it, so the rule goes there"),
        _ => (
            ctx.user_config(),
            "every project; codex reads [[skills.config]] from the user and profile config (a `-c` flag at launch can still override it; harn does not see launch flags)",
        ),
    };
    Knob::Setting {
        file,
        pointer: "/skills/config/-".to_string(),
        value: json!({"path": skill.path.display().to_string(), "enabled": false}),
        scope: Scope::User,
        note: Some(note.to_string()),
    }
}

/// The listing rows: the framing row and the `skills` container with one child per skill, or
/// nothing when `skills.include_instructions = false` or no skill is found.
pub(crate) fn listing(ctx: &mut Ctx, plugins: &[Plugin]) -> Vec<Item> {
    let mut warnings = Vec::new();
    let items = listing_with(ctx, plugins, &mut warnings);
    ctx.warnings.extend(warnings);
    items
}

/// A boolean setting: `Some(false)` only when set to `false`. Set to anything else (a non-finite
/// float reads as `null`), codex refuses to start: the second value is that reason, also pushed to
/// `warnings`; the caller keeps the default behaviour and shows the reason — never silently.
fn bool_setting(
    ctx: &Ctx,
    pointer: &str,
    warnings: &mut Vec<String>,
) -> (Option<bool>, Option<String>) {
    match ctx.config.get(pointer) {
        Some((Value::Bool(b), _)) => (Some(*b), None),
        None => (None, None),
        Some((value, layer)) => {
            let why = format!(
                "{}; codex refuses to start with it",
                super::hooks::not_a(&layer.at(pointer), value, "a boolean")
            );
            warnings.push(why.clone());
            (None, Some(why))
        }
    }
}

fn listing_with(ctx: &Ctx, plugins: &[Plugin], warnings: &mut Vec<String>) -> Vec<Item> {
    let (include, include_bad) = bool_setting(ctx, "/skills/include_instructions", warnings);
    if include == Some(false) {
        return Vec::new();
    }
    let (bundled, bundled_bad) = bool_setting(ctx, "/skills/bundled/enabled", warnings);
    let bundled_on = bundled != Some(false);
    if bundled_on && !ctx.fs.is_dir(&ctx.codex_home.join("skills/.system")) {
        warnings.push(format!(
            "{}: missing; codex installs its bundled skills there at startup (ext/skills/src/host_service.rs:221-225), not listed until then",
            ctx.display(&ctx.codex_home.join("skills/.system"))
        ));
    }

    // Load and merge (`loader/host_merge.rs:201-249`).
    let mut all: Vec<Skill> = Vec::new();
    let mut skill_roots: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for root in roots(ctx, plugins, bundled_on) {
        let (canonical, skills) = load_root(ctx, &root, plugins, warnings);
        for s in skills {
            if seen.insert(s.path.clone()) {
                if !skill_roots.contains(&canonical) {
                    skill_roots.push(canonical.clone());
                }
                all.push(s);
            }
        }
    }
    if all.is_empty() {
        return Vec::new();
    }
    let rules = skill_rules(ctx, warnings);
    let disabled = disabled_paths(&rules, &all);
    all.retain(|s| !disabled.contains(&s.path));
    all.sort_by(|a, b| {
        a.scope
            .catalog_rank()
            .cmp(&b.scope.catalog_rank())
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    let visible: Vec<&Skill> = all.iter().filter(|s| s.implicit).collect();
    let (budget, budget_bad) = budget(ctx, warnings);
    let rendered = (!visible.is_empty()).then(|| render(&visible, &skill_roots, budget.as_ref()));
    if budget.is_none() && budget_bad.is_none() && rendered.is_some() {
        warnings.push(CAP_STATE_UNKNOWN.to_string());
    }

    // Rows.
    let bundled_knob = config_knobs(
        ctx,
        "/skills/bundled/enabled",
        json!(false),
        Some("turns off every bundled (.system) skill"),
    );
    let mut ids: HashSet<String> = HashSet::new();
    let mut children: Vec<Item> = Vec::new();
    let mut lines: HashMap<&Path, (&str, bool)> = HashMap::new();
    if let Some(r) = &rendered {
        for ((s, line), cut) in visible.iter().zip(&r.lines).zip(&r.cut) {
            lines.insert(s.path.as_path(), (line.as_str(), *cut));
        }
    }
    for s in &all {
        let layer = match s.plugin {
            Some(_) => Layer::Plugin,
            None => s.scope.layer(),
        };
        let mut id_name = s.name.clone();
        if !ids.insert(item_id(Agent::Codex, Kind::Skill, layer, &id_name)) {
            id_name = s.path.display().to_string();
            ids.insert(item_id(Agent::Codex, Kind::Skill, layer, &id_name));
            warnings.push(format!(
                "skill name {} is listed twice in the same layer; the second row is named by its path",
                s.name
            ));
        }
        let mut item = new_item(Kind::Skill, layer, &id_name, &s.name);
        item.origin.path = Some(s.path.clone());
        let plugin = s.plugin.map(|i| &plugins[i]);
        item.origin.plugin = plugin.map(|p| p.id.clone());
        let found = format!("{} under {}", ctx.display(Path::new(&s.display)), s.origin);
        item.origin.why = match plugin {
            Some(p) => p.why_with(found),
            None => vec![found],
        };
        match lines.get(s.path.as_path()) {
            Some((line, cut)) => {
                let bytes = line.len() as u64 + 1;
                item.mode = Mode::Advertised;
                item.cost = Cost {
                    source: Some(bytes),
                    predicted: Some(bytes),
                    predicted_is_estimate: rendered.as_ref().is_some_and(|r| r.fits == Some(false)),
                    ..Cost::default()
                };
                item.origin.why.push(format!("rendered: {line}"));
                if *cut {
                    item.origin
                        .why
                        .push("description cut at 1,024 chars (render.rs:1091-1109)".to_string());
                }
            }
            None => {
                let line = render_line(&s.name, &cut_description(&s.description).0, &s.display);
                item.mode = Mode::OnDemand;
                item.cost = cost_na(Some(line.len() as u64 + 1));
                item.origin.why.push(
                    "agents/openai.yaml policy.allow_implicit_invocation = false: not in the listing; `$name` still invokes it (ext/skills/src/provider/host.rs:147-149)"
                        .to_string(),
                );
            }
        }
        let mut knobs = vec![skill_knob(ctx, s, &rules)];
        match (s.scope, plugin) {
            (_, Some(p)) => knobs.push(p.contribution_knob()),
            (SkillScope::System, None) => {
                knobs.extend(bundled_knob.iter().cloned());
                item.origin.why.extend(bundled_bad.clone());
                item.signal = Some(format!(
                    "{BUNDLED_SKILL} {} {BUNDLED_SKILLS_WHY}; {}, so this skill's presence and listing line are confirmed by `harn verify --run --agent codex`",
                    ctx.display(&ctx.codex_home.join("skills/.system")),
                    installs_at_start(ctx)
                ));
            }
            _ => {}
        }
        item.knobs = knobs;
        children.push(item);
    }

    let include_knobs = config_knobs(
        ctx,
        "/skills/include_instructions",
        json!(false),
        Some("drops the whole <skills_instructions> block; skills stay invocable by name"),
    );
    let listed: Vec<&Item> = children
        .iter()
        .filter(|c| c.mode == Mode::Advertised)
        .collect();
    let mut container = new_item(
        Kind::SkillListing,
        Layer::Builtin,
        LISTING,
        &format!("skill listing · {}", listed.len()),
    );
    if listed.is_empty() {
        // No visible entry: codex renders no block at all (`render_dedup.rs:69-71`).
        container.mode = Mode::None;
        container.cost = cost_na(None);
    } else {
        container.mode = Mode::Advertised;
        container.cost = Cost {
            source: Some(listed.iter().filter_map(|c| c.cost.source).sum()),
            predicted: Some(listed.iter().filter_map(|c| c.cost.predicted).sum()),
            predicted_is_estimate: listed.iter().any(|c| c.cost.predicted_is_estimate),
            ..Cost::default()
        };
    }
    container.origin.why = vec![format!(
        "`<skills_instructions>` developer block: one `- name: description (file: …)` line per skill, descriptions ≤ 1,024 chars (render.rs:258-267); {} listed, {} not (allow_implicit_invocation = false)",
        listed.len(),
        children.len() - listed.len()
    )];
    container.origin.why.extend(include_bad);
    container.origin.why.extend(budget_bad);
    if let Some(r) = &rendered {
        container.origin.why.extend(r.why.iter().cloned());
        container.signal = r.signal.clone();
    }
    container.knobs = include_knobs.clone();
    container.children = children;

    let mut out = Vec::new();
    if let Some(r) = &rendered {
        out.push(framing_row(ctx, r, include_knobs));
    }
    out.push(container);
    out
}

/// Which codex replaces the `.system` folder at start, as far as harn knows.
fn installs_at_start(ctx: &Ctx) -> String {
    match &ctx.installed {
        Installed::Version(v) => {
            format!("the installed codex {v} replaces it at start when its set's fingerprint differs")
        }
        Installed::Unversioned => {
            "the installed codex (version unknown) replaces it at start when its set's fingerprint differs".to_string()
        }
        Installed::NotFound => {
            "no codex is on PATH; the codex that runs replaces it at start when its set's fingerprint differs".to_string()
        }
    }
}

/// The tree's one note on the `.system` folder, when a listing row came from it.
pub(crate) fn bundled_note(ctx: &Ctx, items: &[Item]) -> Option<String> {
    let rows = items
        .iter()
        .filter(|i| i.kind == Kind::SkillListing)
        .flat_map(|l| &l.children)
        .filter(|c| {
            c.signal
                .as_deref()
                .is_some_and(|g| g.starts_with(BUNDLED_SKILL))
        })
        .count();
    (rows > 0).then(|| {
        format!(
            "bundled skills: {} {BUNDLED_SKILLS_WHY}; {}, so the presence and listing lines of its {rows} skill{} are confirmed by `harn verify --run --agent codex`",
            ctx.display(&ctx.codex_home.join("skills/.system")),
            installs_at_start(ctx),
            if rows == 1 { "" } else { "s" }
        )
    })
}

/// The block minus its entry lines: tags, intro, roots table, usage section.
fn framing_row(ctx: &Ctx, r: &Rendered, knobs: Vec<Knob>) -> Item {
    let entries: usize = r.lines.iter().map(|l| l.len() + 1).sum();
    let framing = |usage: bool| {
        (OPEN.len() + render_body(r.kind, &r.roots, &r.lines, usage).len() + CLOSE.len())
            .saturating_sub(entries) as u64
    };
    let mut item = new_item(
        Kind::SystemPrompt,
        Layer::Builtin,
        FRAMING,
        "skills instructions (framing)",
    );
    item.origin.why = vec![
        "`<skills_instructions>` developer block around the listing: `## Skills`, the intro, the roots table when used, `### Available skills` (ext/skills/src/catalog_prompt.rs:81-106)".to_string(),
    ];
    // The roots table as rendered, and which of its lines is the bundled root: verify compares it
    // with codex's when the bundled root's line is on one side only.
    item.origin
        .why
        .extend(r.roots.iter().map(|line| format!("{ROOT_PREFIX}{line}")));
    let bundled = slash(&ctx.fs.realpath(&ctx.codex_home.join("skills/.system")));
    if r.roots
        .iter()
        .any(|line| line.ends_with(&format!(" = `{bundled}`")))
    {
        item.origin
            .why
            .push(format!("{BUNDLED_ROOT_PREFIX}{bundled}"));
    }
    match ctx.catalog_flag("include_skills_usage_instructions") {
        Ok(usage) => {
            item.cost = Cost {
                predicted: Some(framing(usage)),
                predicted_is_estimate: r.fits == Some(false),
                ..Cost::default()
            };
            item.origin.why.push(format!(
                "models_cache.json: include_skills_usage_instructions = {usage}: `### How to use skills` {} (fragments.rs:24-30)",
                if usage { "follows" } else { "is left out" }
            ));
        }
        Err(why) => {
            item.origin.why.push(format!(
                "size unknown: {} bytes without `### How to use skills`, {} with it (fragments.rs:24-30); include_skills_usage_instructions unknown: {why}",
                framing(false),
                framing(true)
            ));
        }
    }
    item.knobs = knobs;
    item
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Overlay, Target};
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
            "harn-codex-skills-unit-{}-{}",
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

    fn skill_md(dir: &Path, description: &str) {
        write(
            &dir.join(SKILLS_FILE),
            &format!("---\ndescription: {description}\n---\n"),
        );
    }

    fn target(root: &Path) -> Target {
        Target {
            dir: root.join("repo"),
            home: root.join("home"),
            env: BTreeMap::new(),
            model: None,
            context_window: Some(272_000),
            bytes_per_token: None,
        }
    }

    fn resolve(root: &Path, overlay: Option<&Overlay>) -> crate::model::Tree {
        super::super::resolve_with(&target(root), overlay, &root.join("etc/codex")).unwrap()
    }

    fn skill_names(tree: &crate::model::Tree) -> Vec<String> {
        crate::model::walk(tree)
            .filter(|i| i.kind == Kind::Skill)
            .map(|i| i.label.clone())
            .collect()
    }

    fn entry(name: &str, root: &str, rel: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: format!("Does {name}."),
            path: PathBuf::from(format!("{root}/{rel}")),
            display: format!("{root}/{rel}"),
            root: PathBuf::from(root),
            scope: SkillScope::User,
            plugin: None,
            implicit: true,
            origin: String::new(),
        }
    }

    #[test]
    fn admin_root_is_the_system_dir_skills() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join("home/.codex/config.toml"), "");
        std::fs::create_dir_all(root.join("home/.codex/skills/.system")).unwrap();
        skill_md(&root.join("etc/codex/skills/adm"), "Admin skill.");
        let tree = resolve(root, None);
        let adm = crate::model::walk(&tree)
            .find(|i| i.id == "codex/skill/managed/adm")
            .expect("admin skill");
        assert!(
            adm.origin.why[0].contains("(admin)"),
            "{:?}",
            adm.origin.why
        );
        // Admin sorts before repo and user skills, after bundled ones (render.rs:51-57).
        skill_md(&root.join("home/.codex/skills/.system/sys"), "Bundled.");
        skill_md(&root.join("home/.codex/skills/aaa"), "User.");
        assert_eq!(skill_names(&resolve(root, None)), ["sys", "adm", "aaa"]);
    }

    #[test]
    fn walk_prunes_hidden_dirs_stops_at_depth_six_and_follows_dir_symlinks_outside_system() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        write(&root.join("home/.codex/config.toml"), "");
        let user = root.join("home/.codex/skills");
        skill_md(&user.join("top"), "Depth 2.");
        skill_md(&user.join("a/b/c/d/deep6"), "Depth 6.");
        skill_md(&user.join("a/b/c/d/e/deep7"), "Depth 7.");
        skill_md(&user.join(".hidden/x"), "Hidden.");
        skill_md(&root.join("elsewhere/linked"), "Through a dir symlink.");
        skill_md(
            &root.join("elsewhere2/syslinked"),
            "Not followed in .system.",
        );
        skill_md(&root.join("filetarget"), "File symlink target.");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(root.join("elsewhere/linked"), user.join("linked")).unwrap();
            std::fs::create_dir_all(user.join(".system")).unwrap();
            symlink(
                root.join("elsewhere2/syslinked"),
                user.join(".system/syslinked"),
            )
            .unwrap();
            std::fs::create_dir_all(user.join("filelink")).unwrap();
            symlink(
                root.join("filetarget/SKILL.md"),
                user.join("filelink/SKILL.md"),
            )
            .unwrap();
        }
        let mut names = skill_names(&resolve(root, None));
        names.sort();
        let mut want = vec!["deep6", "top"];
        if cfg!(unix) {
            want.push("linked");
        }
        want.sort();
        assert_eq!(names, want);
    }

    #[test]
    fn frontmatter_needs_a_trimmed_fence_and_content() {
        assert_eq!(
            extract_frontmatter("--- \nname: x\n---\n").as_deref(),
            Some("name: x")
        );
        assert_eq!(extract_frontmatter("\u{feff}---\nname: x\n---\n"), None);
        assert_eq!(extract_frontmatter("---\n---\n"), None);
        assert_eq!(extract_frontmatter("---\nname: x\n"), None);
        let path = Path::new("/r/my skill/SKILL.md");
        assert_eq!(
            parse_skill("---\ndescription: d\n---\n", path),
            Ok(("my skill".to_string(), "d".to_string()))
        );
        let long = "n".repeat(65);
        assert!(parse_skill(&format!("---\nname: {long}\ndescription: d\n---\n"), path).is_err());
    }

    #[test]
    fn absolute_paths_win_when_the_roots_table_costs_more() {
        // Ten roots with one skill each and short paths: the table (16 + 10 lines) outweighs the
        // shorter intro and the one byte each line saves.
        let skills: Vec<Skill> = (0..10)
            .map(|i| entry(&format!("s{i}"), &format!("/r{i}"), "x/SKILL.md"))
            .collect();
        let entries: Vec<&Skill> = skills.iter().collect();
        let roots: Vec<PathBuf> = skills.iter().map(|s| s.root.clone()).collect();
        let r = render(&entries, &roots, Some(&(5_000, "test".to_string())));
        assert_eq!(r.kind, PromptKind::Unaliased);
        assert!(r.roots.is_empty());
        assert_eq!(r.lines[0], "- s0: Does s0. (file: /r0/x/SKILL.md)");

        // One deep root holding them all: the table pays for itself.
        let root = "/a/very/long/path/to/the/codex/home/skills";
        let skills: Vec<Skill> = (0..3)
            .map(|i| entry(&format!("s{i}"), root, &format!("s{i}/SKILL.md")))
            .collect();
        let entries: Vec<&Skill> = skills.iter().collect();
        let r = render(
            &entries,
            &[PathBuf::from(root)],
            Some(&(5_000, "test".to_string())),
        );
        assert_eq!(r.kind, PromptKind::HostAliases);
        assert_eq!(r.roots, [format!("- `r0` = `{root}`")]);
        assert_eq!(r.lines[1], "- s1: Does s1. (file: r0/s1/SKILL.md)");
        assert_eq!(r.fits, Some(true));
        // Over a 10-token budget nothing fits: the sizes become estimates.
        let r = render(
            &entries,
            &[PathBuf::from(root)],
            Some(&(10, "test".to_string())),
        );
        assert_eq!(r.fits, Some(false));
        assert!(r.signal.unwrap().starts_with("over budget:"));
    }

    #[test]
    fn a_plugin_version_with_one_skill_shares_its_marketplace_alias() {
        let one = "/h/.codex/plugins/cache/mkt/solo/1.0.0/skills";
        let many = "/h/.codex/plugins/cache/mkt/multi/2.0.0/skills";
        let skills = [
            entry("solo:a", one, "a/SKILL.md"),
            entry("multi:b", many, "b/SKILL.md"),
            entry("multi:c", many, "c/SKILL.md"),
        ];
        let entries: Vec<&Skill> = skills.iter().collect();
        let plan = alias_plan(&entries, &[PathBuf::from(one), PathBuf::from(many)]);
        assert_eq!(
            plan,
            [
                ("r0".to_string(), "/h/.codex/plugins/cache/mkt".to_string()),
                ("r1".to_string(), many.to_string()),
            ]
        );
        assert_eq!(
            shorten(&plan, &format!("{one}/a/SKILL.md")),
            "r0/solo/1.0.0/skills/a/SKILL.md"
        );
        // The longest matching root wins.
        assert_eq!(
            shorten(&plan, &format!("{many}/b/SKILL.md")),
            "r1/b/SKILL.md"
        );
    }

    #[test]
    fn usage_section_and_description_cut_match_the_constants() {
        let lines = vec!["- a: d (file: r0/a/SKILL.md)".to_string()];
        let roots = vec!["- `r0` = `/x`".to_string()];
        let with = render_body(PromptKind::HostAliases, &roots, &lines, true);
        let without = render_body(PromptKind::HostAliases, &roots, &lines, false);
        assert_eq!(
            with.len() - without.len(),
            "\n### How to use skills\n".len() + SKILLS_HOW_TO_USE_WITH_HOST_ALIASES.len()
        );
        assert_eq!(SKILLS_HOW_TO_USE_WITH_HOST_ALIASES.len(), 2_531);
        assert_eq!(SKILLS_HOW_TO_USE_WITH_SOURCE_LOCATORS.len(), 3_031);
        assert_eq!(SKILLS_INTRO_WITH_SOURCE_LOCATORS.len(), 418);
        assert_eq!(SKILLS_INTRO_WITH_HOST_ALIASES.len(), 261);
        let (cut, was) = cut_description(&"é".repeat(1_024));
        assert!(!was);
        assert_eq!(cut.chars().count(), 1_024);
        let (cut, was) = cut_description(&"é".repeat(1_025));
        assert!(was);
        assert_eq!(cut, format!("{}...", "é".repeat(1_021)));
        assert_eq!(tokens(0), 0);
        assert_eq!(tokens(5), 2);
        assert_eq!(line_tokens("abc"), 1);
    }
}
