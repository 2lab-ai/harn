//! Codex oracle (P0.5 WU C5): what codex-cli itself says a directory loads, from its official
//! inspection commands (`research/02` §8), run with the `codex` on `PATH` (paths in upstream
//! references are relative to `codex-rs/` at tag `rust-v0.160.1`, commit d27764b8):
//!
//! - `codex --version` — the agent version of the run;
//! - `codex [--profile P] -C <dir> debug prompt-input` — the exact model-visible `input` list
//!   (`Prompt.input`, `core/src/prompt_debug.rs:24-113`): the developer message (developer
//!   instructions, skills, permissions, collaboration, apps, plugins blocks), the multi-agent
//!   messages, and the user message (AGENTS.md, environment context). Not in it: the request's
//!   `instructions` field (base instructions), `tools[]`, and hook output — prompt-input builds the
//!   prompt "without entering run_turn" (`prompt_debug.rs:89`), where hooks run. It **starts the
//!   configured MCP servers** and **writes into `CODEX_HOME`** (installation id, bundled skills,
//!   shell snapshots; `research/02` §0 headline 1);
//! - `codex -C <dir> debug models` — the model catalog (base-instruction templates, not the text
//!   sent);
//! - `codex [--profile P] -C <dir> mcp list --json` — the configured MCP servers, plugin servers
//!   included, with `enabled` (`cli/src/mcp_cmd.rs:664-745`);
//! - `codex -C <dir> plugin list --json` — plugins of the configured marketplaces
//!   (`cli/src/plugin_cmd.rs:445-519`). A plugin enabled in config whose marketplace is not
//!   configured still loads but is not listed (measured on 0.160.1, 2026-10-14). Plugins installed
//!   for the ChatGPT account are listed with `source.source = "remote"` (marketplace
//!   `openai-curated-remote`); no local file names them (measured on 0.160.1, 2026-10-14).
//!
//! `--profile` (from [`PROFILE_ENV`], as the resolver reads it) goes to prompt-input and mcp list
//! only: codex rejects it for every other subcommand (`cli/src/main.rs:1861-1889`). No other flag
//! is passed — none that changes config.
//!
//! # Capture directory
//!
//! Every run keeps its raw outputs in a directory so `harn verify --capture <dir>` re-checks them
//! offline: [`MANIFEST`] (schema [`MANIFEST_SCHEMA`]: the `RunContext`, the environment that
//! decides what codex reads, the bundled skill root as the run resolved it, and each command's
//! argv, exit status and files) plus one stdout and one stderr file per command.
//!
//! The bundled root (`<CODEX_HOME>/skills/.system`) decides which listed skills `verify` may
//! exempt as codex's own, so it is bound to the run, never to the verifier's filesystem now
//! ([`CodexCapture::bundled_root`]): the manifest's `bundled_root` (written by [`run_codex`]:
//! `CODEX_HOME` resolved against the run's cwd, canonicalised after the run) when present and
//! absolute; for a manifest without it (an optional field: older captures read unchanged, the
//! schema stays `/1`) the fallback touches no file ([`legacy_bundled_root`]).
//!
//! LEGACY RULE: an old capture binds a bundled root only when the recorded `CODEX_HOME` is unset
//! (then `.codex` under the target's home) or is an absolute path with no `.` or `..` component,
//! and only when codex listed exactly `<that home>/skills/.system` in its `### Skill roots`
//! table. In every other case — a relative `CODEX_HOME` included — it binds nothing, and warns
//! when codex listed any skill (with an empty listing no row depends on the root). The reason: without the filesystem as it was at capture time, harn cannot prove what a
//! relative or dotted home resolved to (`link/..` cancelled lexically is not where codex went
//! when `link` was a symlink), nor whether a home was reached through a symlink.
//!
//! Trust boundary: a capture directory is trusted data written by harn (`run_codex`, in a private
//! `0700` dir). harn checks its shape (schema, artifact names, regular files, the working
//! directory it is bound to) but does not authenticate it: a hand-edited manifest, a recorded
//! `bundled_root` included, is believed.
//!
//! # How the prompt input is cut
//!
//! Each content item of each message is one block. A message carries
//! `internal_chat_message_metadata_passthrough.content_item_kinds`, one kind per content item
//! (`core/src/context_manager/updates.rs:12-30`); the kind names the block. Without it (or for a
//! kind not in [`BLOCKS`]) the block's opening marker decides; an unmarked first part of the
//! first developer message is `developer_instructions` (`core/src/session/mod.rs:4321-4328`).
//! Anything else is kept under its kind (or position) so `verify` can report it.
//!
//! Kinds harn cannot pair with their content are left out of the naming and reported as a
//! capture warning: a message whose kinds are not a list with one per content item is named as if
//! it had none (paired by index, a kind would name another item), and a part whose kind names one
//! block while its text opens with another block's marker is named by neither — it is kept as an
//! unknown block that shows both. A warning alone does not fail `verify`; the rows decide (a
//! block named by neither is seen-not-predicted, and the row it was predicted for is not seen).
//!
//! A region is one block: `input[<i>] <role> · <block>`, its bytes the UTF-8 length of the text;
//! the regions partition the text of the input list, not a wire body (instructions and tools are
//! absent).

use super::Region;
use crate::model::{
    item_id, now_iso8601, Agent, Cost, Item, Kind, Layer, Mode, Origin, RunContext, Target, Tree,
};
use crate::resolve::codex::{codex_home, json_type, PROFILE_ENV};
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The manifest file of a capture directory.
pub const MANIFEST: &str = "harn-codex-capture.json";
pub const MANIFEST_SCHEMA: &str = "harn-codex-capture/1";
/// `RunContext.capture_kind` of a capture made by [`run_codex`].
pub const CAPTURE_KIND: &str = "codex-run";

/// The `origin.why` prefix under which an observed listing entry carries its line.
pub const LISTED_PREFIX: &str = "listed: ";

/// How long one codex command may take before it is killed (prompt-input waits for MCP servers).
const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The environment variables that decide what codex reads, recorded in the manifest so an
/// offline re-check can say when its own environment differs.
pub const BINDING_ENV: &[&str] = &["CODEX_HOME", PROFILE_ENV];

/// Id names of the observed rows; the Codex resolver builds the same ids.
pub mod names {
    /// `Kind::Instruction`, layer builtin: the AGENTS.md user message.
    pub const AGENTS_MD: &str = "agents-md-message";
    /// `Kind::Instruction`, layer unresolved: the `developer_instructions` text (the resolver
    /// names its row by the config file that sets it; `verify` joins on the suffix).
    pub const DEVELOPER_INSTRUCTIONS: &str = "developer_instructions";
    /// `Kind::SkillListing`, layer builtin: the listing container; entries are its children.
    pub const SKILL_LISTING: &str = "skills";
    /// `Kind::SystemPrompt`, layer builtin: the `<skills_instructions>` block minus its entries.
    pub const SKILLS_FRAMING: &str = "skills_instructions";
    /// `Kind::SystemPrompt`, layer builtin: `<multi_agent_role>` + `<multi_agent_mode>`.
    pub const MULTI_AGENT: &str = "multi_agent";
    /// `Kind::EnvBlock`, layer builtin.
    pub const ENVIRONMENT: &str = "environment_context";
    /// `Kind::SystemPrompt`, layer builtin: the request's `instructions` field.
    pub const BASE_INSTRUCTIONS: &str = "instructions";
}

/// A block codex 0.160.1 renders into the input list: its name (the observed row's id name),
/// the content kinds that carry it, its opening marker, and the row kind.
pub struct BlockSpec {
    pub name: &'static str,
    pub kinds: &'static [&'static str],
    pub marker: &'static str,
    pub row: Kind,
}

/// The blocks harn knows, with where codex names them.
pub const BLOCKS: &[BlockSpec] = &[
    // core/src/context/developer_instructions.rs:18-32 (unmarked).
    BlockSpec {
        name: names::DEVELOPER_INSTRUCTIONS,
        kinds: &["generic.developer_instructions"],
        marker: "",
        row: Kind::Instruction,
    },
    // protocol/src/protocol.rs:126-127; ext/skills/src/world_state.rs:12-14 section ids, kind
    // `<id>.instructions` (core/src/context/world_state/mod.rs:174).
    BlockSpec {
        name: names::SKILLS_FRAMING,
        kinds: &[
            "host_skills.instructions",
            "skills.instructions",
            "cloud_skills.instructions",
        ],
        marker: "<skills_instructions>",
        row: Kind::SystemPrompt,
    },
    // prompts/src/permissions_instructions.rs:264, :271-272.
    BlockSpec {
        name: "permissions_instructions",
        kinds: &["permissions.instructions"],
        marker: "<permissions instructions>",
        row: Kind::SystemPrompt,
    },
    // core/src/context/world_state/collaboration_mode.rs:154-167; protocol.rs:132.
    BlockSpec {
        name: "collaboration_mode",
        kinds: &["collaboration_mode.instructions"],
        marker: "<collaboration_mode>",
        row: Kind::SystemPrompt,
    },
    // core/src/context/apps_instructions.rs:13, :24-25; protocol.rs:124.
    BlockSpec {
        name: "apps_instructions",
        kinds: &["apps.instructions"],
        marker: "<apps_instructions>",
        row: Kind::SystemPrompt,
    },
    // core/src/context/available_plugins_instructions.rs:12, :23-26; protocol.rs:128.
    BlockSpec {
        name: "plugins_instructions",
        kinds: &["plugins.usage_instructions"],
        marker: "<plugins_instructions>",
        row: Kind::SystemPrompt,
    },
    // core/src/context/recommended_plugins_instructions.rs:31, :42-43.
    BlockSpec {
        name: "recommended_plugins",
        kinds: &["plugins.recommendations"],
        marker: "<recommended_plugins>",
        row: Kind::SystemPrompt,
    },
    // prompts/src/multi_agent_instructions.rs:37, :55-56.
    BlockSpec {
        name: "multi_agent_role",
        kinds: &["multi_agent.role_instructions"],
        marker: "<multi_agent_role>",
        row: Kind::SystemPrompt,
    },
    // core/src/context/multi_agent_mode_instructions.rs:28, :39-40; protocol.rs:134.
    BlockSpec {
        name: "multi_agent_mode",
        kinds: &["multi_agent.mode_instructions"],
        marker: "<multi_agent_mode>",
        row: Kind::SystemPrompt,
    },
    // core/src/context/model_switch_instructions.rs:19, :34-35.
    BlockSpec {
        name: "model_switch",
        kinds: &["model_switch.instructions"],
        marker: "<model_switch>",
        row: Kind::SystemPrompt,
    },
    // core/src/context/user_instructions.rs:12, :23-34.
    BlockSpec {
        name: names::AGENTS_MD,
        kinds: &["agents_md.instructions"],
        marker: "# AGENTS.md instructions",
        row: Kind::Instruction,
    },
    // core/src/context/world_state/environment.rs:199-200; protocol.rs:120.
    BlockSpec {
        name: names::ENVIRONMENT,
        kinds: &["environments.environment_context"],
        marker: "<environment_context>",
        row: Kind::EnvBlock,
    },
];

/// One content item of the input list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// Index of its message in the input list.
    pub input: usize,
    pub role: String,
    /// Index inside the message's content.
    pub part: usize,
    /// `content_item_kinds[part]`, when the message carries one kind per content item.
    pub content_kind: Option<String>,
    /// A [`BlockSpec::name`], or for an unknown block its content kind or `input[i].part[j]`.
    pub name: String,
    /// Whether `name` is one of [`BLOCKS`].
    pub known: bool,
    pub text: String,
}

impl Block {
    pub fn bytes(&self) -> u64 {
        self.text.len() as u64
    }
}

/// One entry line of the `<skills_instructions>` listing (`- name: description (file: …)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedSkill {
    pub name: String,
    /// The line without its trailing newline.
    pub line: String,
    /// The `SKILL.md` its `file` locator names, through the `### Skill roots` table when the
    /// locator is `rN/…`; None for another locator kind or an alias the table does not define.
    pub path: Option<PathBuf>,
}

impl ListedSkill {
    /// Bytes it adds to the block: the line and its newline (the resolver's entry cost).
    pub fn bytes(&self) -> u64 {
        self.line.len() as u64 + 1
    }
}

/// One `codex mcp list --json` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedServer {
    pub name: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
}

/// One `codex plugin list --json` entry of `installed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedPlugin {
    /// `name@marketplace`.
    pub id: String,
    pub enabled: bool,
    /// `source.source` as written: `local` (a path) or [`REMOTE_SOURCE`] (an account plugin).
    #[serde(default)]
    pub source: Option<String>,
}

/// `source.source` of a plugin installed for the ChatGPT account on the server.
pub const REMOTE_SOURCE: &str = "remote";

/// The files a capture dir may hold besides [`MANIFEST`]: each command's stdout and stderr. A
/// manifest naming any other file is refused, and so is any of these that is a symlink.
pub const ARTIFACTS: &[(&str, &str)] = &[
    ("version.txt", "version.stderr"),
    ("prompt-input.json", "prompt-input.stderr"),
    ("models.json", "models.stderr"),
    ("mcp-list.json", "mcp-list.stderr"),
    ("plugin-list.json", "plugin-list.stderr"),
];

/// One command of a run, as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// The argv after the binary.
    pub args: Vec<String>,
    /// Stdout file, one of [`ARTIFACTS`].
    pub stdout: String,
    /// Stderr file, the one [`ARTIFACTS`] pairs with `stdout`.
    pub stderr: String,
    /// `None` when it was killed or could not start.
    pub exit: Option<i32>,
    /// Why it could not run (it could not start, or harn could not wait for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub context: RunContext,
    /// [`BINDING_ENV`] as the run saw them (absent = unset).
    pub env: BTreeMap<String, Option<String>>,
    /// `<CODEX_HOME>/skills/.system` as the run resolved it ([`run_bundled_root`]); absent in
    /// captures written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled_root: Option<PathBuf>,
    pub steps: Vec<Step>,
}

/// What codex said about a directory: the input list cut into blocks, the listing entries, the
/// MCP servers and plugins it lists, the catalog's model slugs — and the observed rows built from
/// them (same id rules as the resolver).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexCapture {
    pub context: RunContext,
    pub env: BTreeMap<String, Option<String>>,
    /// Text bytes of the input list (the sum of `regions`).
    pub input_bytes: u64,
    pub regions: Vec<Region>,
    pub blocks: Vec<Block>,
    /// Entries of the `<skills_instructions>` block, in order (empty when there is none).
    pub skills: Vec<ListedSkill>,
    /// `None` when `mcp list` failed or its output is missing.
    pub mcp_servers: Option<Vec<ListedServer>>,
    /// `None` when `plugin list` failed or its output is missing.
    pub plugins: Option<Vec<ListedPlugin>>,
    /// Codex's bundled skill root in the run (`<CODEX_HOME>/skills/.system`), bound to the
    /// capture (module docs); `None` when it cannot be established, and then no listed skill is
    /// treated as bundled.
    #[serde(default)]
    pub bundled_root: Option<PathBuf>,
    /// Model slugs of `debug models` (`None` when it failed).
    pub models: Option<Vec<String>>,
    pub tree: Tree,
    /// Where the raw outputs are kept.
    pub dir: Option<PathBuf>,
    /// Problems reading the outputs (a failed command, an unparsable file).
    pub warnings: Vec<String>,
}

impl CodexCapture {
    /// The first block named `name`.
    pub fn block(&self, name: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.name == name)
    }
}

/// Whether `path` is a Codex capture directory (or its manifest).
pub fn is_capture_dir(path: &Path) -> bool {
    path.join(MANIFEST).is_file()
        || (path.file_name().and_then(|n| n.to_str()) == Some(MANIFEST) && path.is_file())
}

/// The commands a run makes, argv after the binary, with each stdout file name. `--profile`
/// only where codex accepts it.
pub fn planned_steps(target: &Target) -> Vec<(Vec<String>, &'static str)> {
    let dir = target.dir.display().to_string();
    let profile: Vec<String> = match target
        .env
        .get(PROFILE_ENV)
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
    {
        Some(p) => vec!["--profile".to_string(), p.to_string()],
        None => Vec::new(),
    };
    let args = |profiled: bool, rest: &[&str]| -> Vec<String> {
        let mut out = if profiled {
            profile.clone()
        } else {
            Vec::new()
        };
        out.extend(["-C".to_string(), dir.clone()]);
        out.extend(rest.iter().map(|s| s.to_string()));
        out
    };
    vec![
        (vec!["--version".to_string()], "version.txt"),
        (args(true, &["debug", "prompt-input"]), "prompt-input.json"),
        (args(false, &["debug", "models"]), "models.json"),
        (args(true, &["mcp", "list", "--json"]), "mcp-list.json"),
        (
            args(false, &["plugin", "list", "--json"]),
            "plugin-list.json",
        ),
    ]
}

/// Runs [`planned_steps`] with `codex_bin` in `target.dir` under the target's environment
/// (`HOME` = `target.home`), keeps every output in a new private directory under the system temp
/// dir, and reads it back as a capture bound to `target`. A failed `prompt-input` is an error;
/// a failed `models` / `mcp list` / `plugin list` leaves that part out with a warning.
pub fn run_codex(target: &Target, codex_bin: &Path) -> anyhow::Result<CodexCapture> {
    let dir = capture_dir()?;
    let mut steps = Vec::new();
    for (args, stdout) in planned_steps(target) {
        let stderr = ARTIFACTS
            .iter()
            .find(|(out, _)| *out == stdout)
            .map(|(_, err)| err.to_string())
            .expect("every planned step has an artifact pair");
        let run = run_step(
            target,
            codex_bin,
            &args,
            &dir.join(stdout),
            &dir.join(&stderr),
            RUN_TIMEOUT,
        );
        let (exit, error) = match run {
            Ok(exit) => (exit, None),
            Err(err) => (None, Some(format!("{err:#}"))),
        };
        if stdout == "prompt-input.json" && exit != Some(0) {
            let why = match &error {
                Some(err) => err.clone(),
                None => {
                    let tail = std::fs::read_to_string(dir.join(&stderr)).unwrap_or_default();
                    let tail: String = tail.lines().rev().take(5).collect::<Vec<_>>().join(" | ");
                    format!(
                        "exited with {}; stderr tail: {tail}",
                        exit.map_or("no status (killed)".to_string(), |c| c.to_string())
                    )
                }
            };
            bail!(
                "{} {}: {why} (outputs kept in {})",
                codex_bin.display(),
                args.join(" "),
                dir.display()
            );
        }
        steps.push(Step {
            args,
            stdout: stdout.to_string(),
            stderr,
            exit,
            error,
        });
    }
    // The version only from a `--version` that succeeded.
    let version = steps
        .iter()
        .find(|s| s.stdout == "version.txt" && s.exit == Some(0))
        .and_then(|_| std::fs::read_to_string(dir.join("version.txt")).ok())
        .and_then(|v| parse_version(&v));
    let prompt_args = steps
        .iter()
        .find(|s| s.stdout == "prompt-input.json")
        .map(|s| s.args.clone())
        .unwrap_or_default();
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        context: RunContext {
            cwd: target.dir.clone(),
            agent: Agent::Codex,
            agent_version: version,
            model: None,
            launch_opts: prompt_args,
            settings_fingerprint: Some(settings_fingerprint(target)),
            capture_kind: CAPTURE_KIND.to_string(),
            captured_at: now_iso8601(),
        },
        env: binding_env(target),
        bundled_root: Some(run_bundled_root(target)),
        steps,
    };
    std::fs::write(
        dir.join(MANIFEST),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )
    .with_context(|| format!("writing {}", dir.join(MANIFEST).display()))?;
    read_capture_dir(&dir, target).with_context(|| format!("outputs kept in {}", dir.display()))
}

/// `<CODEX_HOME>/skills/.system` as a run of codex in `target.dir` resolves it: `CODEX_HOME`
/// against that dir (the run's cwd; `utils/home-dir/src/lib.rs:13-35` canonicalises it), else
/// `.codex` in the target's home; canonicalised (the root itself when it exists, else its home).
pub fn run_bundled_root(target: &Target) -> PathBuf {
    let home = match target.env.get("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(v) => target.dir.join(v),
        None => target.home.join(".codex"),
    };
    let root = home.join(BUNDLED_DIR);
    std::fs::canonicalize(&root)
        .or_else(|_| std::fs::canonicalize(&home).map(|h| h.join(BUNDLED_DIR)))
        .unwrap_or(root)
}

/// The bundled root below `CODEX_HOME`.
const BUNDLED_DIR: &str = "skills/.system";

/// The bundled root of a capture: the recorded one when it is an absolute path; else (none
/// recorded, or a malformed one, ignored with a warning) [`legacy_bundled_root`]. Warnings go to
/// `warnings`.
fn bind_bundled_root(
    recorded: Option<PathBuf>,
    capture: &CodexCapture,
    target: &Target,
    warnings: &mut Vec<String>,
) -> Option<PathBuf> {
    match recorded {
        Some(root) if root.is_absolute() => return Some(root),
        Some(root) => warnings.push(format!(
            "bundled skills: the capture's recorded bundled_root {:?} is not an absolute path; ignored",
            root.display().to_string()
        )),
        None => {}
    }
    let (root, warning) = legacy_bundled_root(capture, target);
    warnings.extend(warning);
    root
}

/// The bundled root of a capture that records none, by the LEGACY RULE (module docs), without
/// touching a file: `<home>/skills/.system`, where home is `.codex` in the target's home (the
/// manifest records no home) when the recorded `CODEX_HOME` is unset, else that `CODEX_HOME`
/// when it is absolute with no `.` or `..` component — bound only if codex listed exactly that
/// path in its `### Skill roots` table. Otherwise None, and a warning when codex listed any skill
/// (one it may have taken from its bundled root stays unexempted).
fn legacy_bundled_root(
    capture: &CodexCapture,
    target: &Target,
) -> (Option<PathBuf>, Option<String>) {
    let warn = |why: String| (None, (!capture.skills.is_empty()).then_some(why));
    let recorded = capture
        .env
        .get("CODEX_HOME")
        .and_then(Option::as_deref)
        .filter(|v| !v.is_empty());
    let home = match recorded {
        None => target.home.join(".codex"),
        Some(v) => PathBuf::from(v),
    };
    if !plain_absolute(&home) {
        let which = match recorded {
            Some(v) => format!("its CODEX_HOME ({v:?})"),
            None => format!(
                "the codex home under the target's home ({:?})",
                home.display().to_string()
            ),
        };
        return warn(format!(
            "bundled skills: this capture records no bundled root, and {which} is relative or has a `.` or `..` component, which harn cannot resolve without the filesystem as it was at capture time; no listed skill is treated as bundled (a capture from `harn verify --run --agent codex` records the root)"
        ));
    }
    let rebuilt = home.join(BUNDLED_DIR);
    let written = rebuilt.to_string_lossy();
    let listed = capture.block(names::SKILLS_FRAMING).is_some_and(|b| {
        parse_skill_roots(&b.text)
            .into_iter()
            .any(|(_, root)| root == written)
    });
    if listed {
        return (Some(rebuilt), None);
    }
    warn(format!(
        "bundled skills: this capture records no bundled root, and the one harn rebuilds from its env ({written}) is not a skill root codex listed; no listed skill is treated as bundled (a home reached through a symlink cannot be proven from an old capture: a capture from `harn verify --run --agent codex` records the root)"
    ))
}

/// An absolute path with no `.` or `..` segment, read from its text (`Path::components` drops an
/// inner `.`).
fn plain_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .to_string_lossy()
            .split(std::path::is_separator)
            .any(|segment| segment == "." || segment == "..")
}

/// [`BINDING_ENV`] from the target's environment.
pub fn binding_env(target: &Target) -> BTreeMap<String, Option<String>> {
    BINDING_ENV
        .iter()
        .map(|k| {
            (
                k.to_string(),
                target.env.get(*k).filter(|v| !v.is_empty()).cloned(),
            )
        })
        .collect()
}

/// `codex-cli 0.160.1` → `0.160.1`.
pub fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))
        .map(str::to_string)
}

fn capture_dir() -> anyhow::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir =
        std::env::temp_dir().join(format!("harn-codex-capture-{}-{nanos}", std::process::id()));
    create_private_dir(&dir)?;
    Ok(std::fs::canonicalize(&dir).unwrap_or(dir))
}

/// Creates `dir`, which must not exist (an existing dir or a symlink there is an error), readable
/// by its owner only from the start.
fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .with_context(|| format!("creating {}", dir.display()))
}

/// Runs one command in its own process group; its exit code, or `None` when it was killed at
/// `timeout`. On the timeout, or when waiting for it fails other than with ECHILD
/// ([`supervise`]), the whole group is killed (the MCP servers `debug prompt-input` starts are in
/// it) before the command is reaped, so the group id cannot have been reused.
fn run_step(
    target: &Target,
    codex_bin: &Path,
    args: &[String],
    stdout: &Path,
    stderr: &Path,
    timeout: std::time::Duration,
) -> anyhow::Result<Option<i32>> {
    use std::process::{Command, Stdio};
    let out = std::fs::File::create(stdout)?;
    let err = std::fs::File::create(stderr)?;
    let mut command = Command::new(codex_bin);
    command
        .args(args)
        .current_dir(&target.dir)
        .env_clear()
        .envs(&target.env)
        .env("HOME", &target.home)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {}", codex_bin.display()))?;
    let kill_all = |child: &mut std::process::Child| {
        kill_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
    };
    let deadline = std::time::Instant::now() + timeout;
    supervise(&mut child, |c| c.try_wait(), kill_all, deadline).map_err(|e| {
        anyhow::Error::from(e).context(format!(
            "waiting for {} {}",
            codex_bin.display(),
            args.join(" ")
        ))
    })
}

/// Polls `try_wait` on `child` until it exits (its exit code; `None` when a signal ended it) or
/// `deadline` passes (`None`). `kill` runs once on the timeout, and once when waiting fails —
/// except with ECHILD: the child was reaped elsewhere, so its pid — the group id — may already
/// name another process; neither is signalled then. A wait error, EINTR included, is returned,
/// never retried.
fn supervise<C>(
    child: &mut C,
    mut try_wait: impl FnMut(&mut C) -> std::io::Result<Option<std::process::ExitStatus>>,
    mut kill: impl FnMut(&mut C),
    deadline: std::time::Instant,
) -> std::io::Result<Option<i32>> {
    loop {
        match try_wait(child) {
            Ok(Some(status)) => return Ok(status.code()),
            Ok(None) => {}
            Err(e) => {
                if !reaped_elsewhere(&e) {
                    kill(child);
                }
                return Err(e);
            }
        }
        if std::time::Instant::now() >= deadline {
            kill(child);
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// SIGKILL to the process group led by `pid` (a child spawned with `process_group(0)`).
#[cfg(unix)]
fn kill_group(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: kill(2) takes plain integers; a negative pid names the process group.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

/// `try_wait` failed because the child is no longer ours to wait for (ECHILD).
#[cfg(unix)]
fn reaped_elsewhere(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ECHILD)
}

#[cfg(not(unix))]
fn reaped_elsewhere(_e: &std::io::Error) -> bool {
    false
}

/// `sha256:<hex>` over the config files that decide what codex loads for `target`, in a fixed
/// order, each hashed as `path NUL contents NUL` (`<absent>` for a missing file):
/// `/etc/codex/config.toml`, `$CODEX_HOME/config.toml`, the profile's
/// `$CODEX_HOME/<profile>.config.toml`, `.codex/config.toml` of the target and each ancestor,
/// `/etc/codex/managed_config.toml`. A difference between a capture and now is a warning.
pub fn settings_fingerprint(target: &Target) -> String {
    use sha2::{Digest, Sha256};
    let home = codex_home(target);
    let mut files = vec![
        PathBuf::from("/etc/codex/config.toml"),
        home.join("config.toml"),
    ];
    if let Some(p) = target
        .env
        .get(PROFILE_ENV)
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
    {
        files.push(home.join(format!("{p}.config.toml")));
    }
    let mut ancestors: Vec<&Path> = target.dir.ancestors().collect();
    ancestors.reverse();
    files.extend(ancestors.iter().map(|d| d.join(".codex/config.toml")));
    files.push(PathBuf::from("/etc/codex/managed_config.toml"));
    let mut hasher = Sha256::new();
    for file in &files {
        hasher.update(file.to_string_lossy().as_bytes());
        hasher.update([0]);
        match std::fs::read(file) {
            Ok(bytes) => hasher.update(&bytes),
            Err(_) => hasher.update(b"<absent>"),
        }
        hasher.update([0]);
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// Reads a capture directory (or its manifest) and binds it to `target`: every working directory
/// the input list itself names (each `<environment_context>` `<cwd>`, each AGENTS.md header; at
/// least one absolute) must be the manifest's, and that the target's, else Err naming the
/// mismatch. The manifest and every artifact present must be regular files (not followed).
pub fn read_capture_dir(path: &Path, target: &Target) -> anyhow::Result<CodexCapture> {
    let dir = if path.is_file() {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        path.to_path_buf()
    };
    let manifest_path = dir.join(MANIFEST);
    // Not followed, and checked before opening: a FIFO or device would block or misread.
    match std::fs::symlink_metadata(&manifest_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("{}: a symlink, refused", manifest_path.display())
        }
        Ok(meta) if !meta.is_file() => {
            bail!("{}: not a regular file, refused", manifest_path.display())
        }
        _ => {}
    }
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text).with_context(|| {
        format!(
            "{}: not a harn Codex capture manifest",
            manifest_path.display()
        )
    })?;
    if manifest.schema != MANIFEST_SCHEMA {
        bail!(
            "{}: schema {:?}, expected {MANIFEST_SCHEMA:?}",
            manifest_path.display(),
            manifest.schema
        );
    }
    for step in &manifest.steps {
        if !ARTIFACTS
            .iter()
            .any(|(out, err)| *out == step.stdout && *err == step.stderr)
        {
            bail!(
                "{}: a step names {:?} / {:?}, not one of the capture's own files",
                manifest_path.display(),
                step.stdout,
                step.stderr
            );
        }
    }
    // Every artifact present must be a regular file in the dir itself.
    for name in ARTIFACTS.iter().flat_map(|(out, err)| [*out, *err]) {
        artifact(&dir, name)?;
    }
    let mut warnings = Vec::new();
    let read = |file: &str, warnings: &mut Vec<String>| -> anyhow::Result<Option<String>> {
        let step = manifest.steps.iter().find(|s| s.stdout == file);
        if let Some(step) = step {
            if step.exit != Some(0) {
                let why = match &step.error {
                    Some(err) => err.clone(),
                    None => {
                        let stderr = match artifact(&dir, &step.stderr)? {
                            Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
                            None => String::new(),
                        };
                        let tail: String = stderr.lines().take(2).collect::<Vec<_>>().join(" | ");
                        format!(
                            "exited with {} ({tail})",
                            step.exit
                                .map_or("no status (killed)".to_string(), |c| c.to_string())
                        )
                    }
                };
                warnings.push(format!("codex {}: {why}; not read", step.args.join(" ")));
                return Ok(None);
            }
        }
        let Some(path) = artifact(&dir, file)? else {
            warnings.push(format!("{file}: missing"));
            return Ok(None);
        };
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(err) => {
                warnings.push(format!("{file}: {err}"));
                Ok(None)
            }
        }
    };
    let prompt = read("prompt-input.json", &mut warnings)?.ok_or_else(|| {
        anyhow!(
            "{}: no prompt-input.json ({})",
            dir.display(),
            warnings.join("; ")
        )
    })?;
    let (blocks, naming) = parse_prompt_input(&prompt).context("prompt-input.json")?;
    warnings.extend(
        naming
            .into_iter()
            .map(|w| format!("prompt-input.json: {w}")),
    );
    let mcp_servers = read("mcp-list.json", &mut warnings)?.and_then(|t| {
        parse_mcp_list(&t)
            .map_err(|e| warnings.push(format!("mcp-list.json: {e:#}")))
            .ok()
    });
    let plugins = read("plugin-list.json", &mut warnings)?.and_then(|t| {
        parse_plugin_list(&t)
            .map_err(|e| warnings.push(format!("plugin-list.json: {e:#}")))
            .ok()
    });
    let models = read("models.json", &mut warnings)?.and_then(|t| {
        parse_models(&t)
            .map_err(|e| warnings.push(format!("models.json: {e:#}")))
            .ok()
    });
    let mut capture = build(
        manifest.context,
        manifest.env,
        blocks,
        mcp_servers,
        plugins,
        models,
    );
    capture.dir = Some(dir);
    capture.warnings.extend(warnings);
    let mut bound = Vec::new();
    capture.bundled_root = bind_bundled_root(manifest.bundled_root, &capture, target, &mut bound);
    capture.warnings.extend(bound);
    bind(&capture, target)?;
    capture.warnings.extend(drift(&capture, target));
    Ok(capture)
}

/// What changed since the capture that decides what codex reads: the config files (settings
/// fingerprint) and [`BINDING_ENV`]. Rows may differ for that reason, so it is said.
fn drift(capture: &CodexCapture, target: &Target) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(then) = &capture.context.settings_fingerprint {
        if *then != settings_fingerprint(target) {
            out.push(
                "the codex config files changed since the capture (settings fingerprint differs); rows may differ for that reason"
                    .to_string(),
            );
        }
    }
    let now = binding_env(target);
    for (name, value) in &capture.env {
        let current = now.get(name).cloned().flatten();
        if *value != current {
            out.push(format!(
                "{name} was {} at capture time, is {} now",
                value.as_deref().unwrap_or("unset"),
                current.as_deref().unwrap_or("unset")
            ));
        }
    }
    out
}

/// `dir/name` when it is there as a regular file; None when absent; Err when `name` is not one
/// of [`ARTIFACTS`] or the entry is a symlink or not a regular file.
fn artifact(dir: &Path, name: &str) -> anyhow::Result<Option<PathBuf>> {
    if !ARTIFACTS
        .iter()
        .any(|(out, err)| *out == name || *err == name)
    {
        bail!("{name:?} is not one of the capture's own files");
    }
    let path = dir.join(name);
    match std::fs::symlink_metadata(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(anyhow::Error::from(err).context(path.display().to_string())),
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("{}: a symlink, refused", path.display())
        }
        Ok(meta) if !meta.is_file() => bail!("{}: not a regular file", path.display()),
        Ok(_) => Ok(Some(path)),
    }
}

/// Err unless the capture is about `target.dir`: every working directory the input list names
/// (there must be one) is the manifest's, and that is the target.
fn bind(capture: &CodexCapture, target: &Target) -> anyhow::Result<()> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let evidence = derived_cwd(&capture.blocks);
    if !evidence.iter().any(|(p, _)| p.is_absolute()) {
        bail!(
            "the input list names no working directory (no <environment_context> <cwd>, no `# AGENTS.md instructions for <dir>` header); it cannot be bound to {}",
            target.dir.display()
        );
    }
    for (cwd, source) in evidence {
        let cwd = canonical(&cwd);
        if cwd != canonical(&capture.context.cwd) {
            bail!(
                "the input list names {} ({source}), the manifest {}",
                cwd.display(),
                capture.context.cwd.display()
            );
        }
    }
    if canonical(&capture.context.cwd) != target.dir {
        bail!(
            "capture bound to {}, not to the target {}",
            capture.context.cwd.display(),
            target.dir.display()
        );
    }
    Ok(())
}

/// Every working directory the input list names, with where: each `<cwd>` of every
/// `<environment_context>` block, and the `# AGENTS.md instructions for <dir>` header of every
/// AGENTS.md message. All of them are returned, so a contradictory later one is not missed.
pub fn derived_cwd(blocks: &[Block]) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    for env in blocks.iter().filter(|b| b.name == names::ENVIRONMENT) {
        let mut rest = env.text.as_str();
        while let Some(open) = rest.find("<cwd>") {
            rest = &rest[open + "<cwd>".len()..];
            let Some(close) = rest.find("</cwd>") else {
                break;
            };
            out.push((PathBuf::from(&rest[..close]), "<environment_context> <cwd>"));
            rest = &rest[close + "</cwd>".len()..];
        }
    }
    for agents in blocks.iter().filter(|b| b.name == names::AGENTS_MD) {
        let first = agents.text.lines().next().unwrap_or_default();
        if let Some(dir) = first.strip_prefix("# AGENTS.md instructions for ") {
            out.push((PathBuf::from(dir), "AGENTS.md message header"));
        }
    }
    out
}

/// The input list (`codex debug prompt-input` stdout: a JSON array of `ResponseItem`s, see
/// `protocol/src/models.rs`) cut into blocks, and warnings for what could not be named by its
/// kind (a message whose kinds are not one per content item, a kind and a marker that disagree).
pub fn parse_prompt_input(text: &str) -> anyhow::Result<(Vec<Block>, Vec<String>)> {
    let items: Value = serde_json::from_str(text.trim()).context("not JSON")?;
    let items = items
        .as_array()
        .ok_or_else(|| anyhow!("not a JSON array of response items"))?;
    let mut blocks = Vec::new();
    let mut warnings = Vec::new();
    let mut first_developer: Option<usize> = None;
    for (i, item) in items.iter().enumerate() {
        let role = item["role"].as_str().unwrap_or_default().to_string();
        if item["type"] != "message" {
            blocks.push(Block {
                input: i,
                role,
                part: 0,
                content_kind: None,
                name: format!("input[{i}].{}", item["type"].as_str().unwrap_or("unknown")),
                known: false,
                text: serde_json::to_string(item).unwrap_or_default(),
            });
            continue;
        }
        if role == "developer" && first_developer.is_none() {
            first_developer = Some(i);
        }
        // `null` is no kinds, like a missing key.
        let kinds = item
            .get("internal_chat_message_metadata_passthrough")
            .and_then(|m| m.get("content_item_kinds"))
            .filter(|v| !v.is_null());
        let Some(content) = item["content"].as_array() else {
            // Not the `ResponseItem::Message` shape (`content` is a list): kept, so verify
            // reports it rather than losing it.
            blocks.push(Block {
                input: i,
                role,
                part: 0,
                content_kind: None,
                name: format!("input[{i}].content"),
                known: false,
                text: serde_json::to_string(&item["content"]).unwrap_or_default(),
            });
            continue;
        };
        // Kinds pair with content items by index only when they are a list, one per item.
        let kinds: Vec<Option<&str>> = match kinds.map(|v| v.as_array().ok_or(v)) {
            None => Vec::new(),
            Some(Err(other)) => {
                let article = if other.is_object() { "an" } else { "a" };
                warnings.push(format!(
                    "input[{i}]: content_item_kinds is {article} {}, not an array; its blocks are named by their markers, not their kinds",
                    json_type(other)
                ));
                Vec::new()
            }
            Some(Ok(kinds)) if kinds.len() != content.len() => {
                warnings.push(format!(
                    "input[{i}] has {} content items but {} content_item_kinds; its blocks are named by their markers, not their kinds",
                    content.len(),
                    kinds.len()
                ));
                Vec::new()
            }
            Some(Ok(kinds)) => kinds.iter().map(Value::as_str).collect(),
        };
        for (j, part) in content.iter().enumerate() {
            let content_kind = kinds.get(j).copied().flatten();
            let text = match part["text"].as_str() {
                Some(t) => t.to_string(),
                None => serde_json::to_string(part).unwrap_or_default(),
            };
            let unmarked_first = first_developer == Some(i) && j == 0;
            let (name, known) =
                classify(content_kind, &text, unmarked_first, (i, j), &mut warnings);
            blocks.push(Block {
                input: i,
                role: role.clone(),
                part: j,
                content_kind: content_kind.map(str::to_string),
                name,
                known,
                text,
            });
        }
    }
    Ok((blocks, warnings))
}

/// The block's name: by content kind, else by marker, else `developer_instructions` for the
/// unmarked first part of the first developer message, else its kind or position (unknown). A
/// kind naming one block on a text that opens with another block's marker names neither: the
/// block is unknown, named by its position with both shown, and a warning says so.
fn classify(
    kind: Option<&str>,
    text: &str,
    unmarked_first: bool,
    (input, part): (usize, usize),
    warnings: &mut Vec<String>,
) -> (String, bool) {
    let by_kind = kind.and_then(|k| BLOCKS.iter().find(|b| b.kinds.contains(&k)));
    let by_marker = BLOCKS
        .iter()
        .find(|b| !b.marker.is_empty() && text.starts_with(b.marker));
    match (by_kind, by_marker) {
        (Some(a), Some(b)) if a.name != b.name => {
            let kind = kind.unwrap_or_default();
            warnings.push(format!(
                "input[{input}].part[{part}]: its content kind {kind} names {}, but its text opens with the marker of {} ({}); named by neither, kept as an unknown block",
                a.name, b.name, b.marker
            ));
            return (
                format!(
                    "input[{input}].part[{part}] (kind {kind}, marker {})",
                    b.marker
                ),
                false,
            );
        }
        (Some(spec), _) | (None, Some(spec)) => return (spec.name.to_string(), true),
        (None, None) => {}
    }
    if kind.is_none() && unmarked_first {
        return (names::DEVELOPER_INSTRUCTIONS.to_string(), true);
    }
    match kind {
        Some(k) => (k.to_string(), false),
        None => (format!("input[{input}].part[{part}]"), false),
    }
}

/// The entry lines of a `<skills_instructions>` block: the `- ` lines after `### Available
/// skills` up to the next `### ` heading or the closing tag (`ext/skills/src/catalog_prompt.rs:81-106`,
/// `ext/skills/src/render.rs:258-267`). The name is the text before the first `": "`; plugin
/// skills keep their `plugin:` prefix (the plugin's manifest name, `core-plugins/src/loader.rs:906`).
/// The `(file: …)` locator at the line's end resolves through the `### Skill roots` lines
/// (`` - `rN` = `<dir>` ``) or is absolute.
pub fn parse_listing(text: &str) -> Vec<ListedSkill> {
    let mut out = Vec::new();
    let roots: BTreeMap<&str, &str> = parse_skill_roots(text).into_iter().collect();
    let mut inside = false;
    for line in text.split('\n') {
        if line == "### Available skills" {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if line.starts_with("### ") || line.starts_with("</skills_instructions>") {
            break;
        }
        let Some(entry) = line.strip_prefix("- ") else {
            continue;
        };
        let name = match entry.split_once(": ") {
            Some((name, _)) => name,
            None => entry.split(" (").next().unwrap_or(entry),
        };
        let locator = line
            .strip_suffix(')')
            .and_then(|l| l.rsplit_once(" (file: "))
            .map(|(_, locator)| locator);
        let path = locator.and_then(|locator| match locator.split_once('/') {
            Some((alias, rest)) if roots.contains_key(alias) => {
                Some(Path::new(roots[alias]).join(rest))
            }
            _ if Path::new(locator).is_absolute() => Some(PathBuf::from(locator)),
            _ => None,
        });
        out.push(ListedSkill {
            name: name.to_string(),
            line: line.to_string(),
            path,
        });
    }
    out
}

/// The `### Skill roots` table of a `<skills_instructions>` block, `(alias, root)` in order: the
/// `` - `rN` = `<dir>` `` lines of that section before `### Available skills` (empty when codex
/// lists absolute paths).
pub fn parse_skill_roots(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut section = "";
    for line in text.split('\n') {
        if line == "### Available skills" {
            break;
        }
        if line.starts_with("### ") {
            section = line;
        } else if section == "### Skill roots" {
            out.extend(parse_root_line(line));
        }
    }
    out
}

/// One roots-table line, `` - `rN` = `<dir>` `` → `(rN, dir)`.
pub fn parse_root_line(line: &str) -> Option<(&str, &str)> {
    line.strip_prefix("- `")
        .and_then(|rest| rest.split_once("` = `"))
        .and_then(|(alias, root)| Some((alias, root.strip_suffix('`')?)))
}

/// `codex mcp list --json`: an array of `{name, enabled, disabled_reason, …}`.
pub fn parse_mcp_list(text: &str) -> anyhow::Result<Vec<ListedServer>> {
    let v: Value = serde_json::from_str(text.trim()).context("not JSON")?;
    let entries = v.as_array().ok_or_else(|| anyhow!("not a JSON array"))?;
    Ok(entries
        .iter()
        .filter_map(|e| {
            Some(ListedServer {
                name: e["name"].as_str()?.to_string(),
                enabled: e["enabled"].as_bool().unwrap_or(true),
                disabled_reason: e["disabled_reason"].as_str().map(str::to_string),
            })
        })
        .collect())
}

/// `codex plugin list --json`: `{installed: [{pluginId, enabled, source: {source, …}, …}],
/// available: […]}`; only `installed` entries are kept.
pub fn parse_plugin_list(text: &str) -> anyhow::Result<Vec<ListedPlugin>> {
    let v: Value = serde_json::from_str(text.trim()).context("not JSON")?;
    let installed = v["installed"]
        .as_array()
        .ok_or_else(|| anyhow!("no `installed` array"))?;
    Ok(installed
        .iter()
        .filter(|e| e["installed"].as_bool().unwrap_or(true))
        .filter_map(|e| {
            Some(ListedPlugin {
                id: e["pluginId"].as_str()?.to_string(),
                enabled: e["enabled"].as_bool().unwrap_or(false),
                source: e["source"]["source"].as_str().map(str::to_string),
            })
        })
        .collect())
}

/// `codex debug models`: `{models: [{slug, …}]}` → the slugs.
pub fn parse_models(text: &str) -> anyhow::Result<Vec<String>> {
    let v: Value = serde_json::from_str(text.trim()).context("not JSON")?;
    let models = v["models"]
        .as_array()
        .ok_or_else(|| anyhow!("no `models` array"))?;
    Ok(models
        .iter()
        .filter_map(|m| m["slug"].as_str().map(str::to_string))
        .collect())
}

fn row(kind: Kind, layer: Layer, name: &str, observed: Option<u64>) -> Item {
    Item {
        id: item_id(Agent::Codex, kind, layer, name),
        label: name.to_string(),
        kind,
        origin: Origin {
            layer,
            path: None,
            setting: None,
            plugin: None,
            why: vec!["observed by codex".to_string()],
        },
        mode: Mode::Always,
        cost: Cost {
            observed,
            ..Cost::default()
        },
        knobs: Vec::new(),
        freed: None,
        signal: None,
        last_used: None,
        children: Vec::new(),
    }
}

/// The capture from its parsed parts: regions, listing entries and the observed rows.
pub fn build(
    context: RunContext,
    env: BTreeMap<String, Option<String>>,
    blocks: Vec<Block>,
    mcp_servers: Option<Vec<ListedServer>>,
    plugins: Option<Vec<ListedPlugin>>,
    models: Option<Vec<String>>,
) -> CodexCapture {
    let regions: Vec<Region> = blocks
        .iter()
        .map(|b| Region {
            name: format!("input[{}] {}{}{}", b.input, b.role, Region::SEP, b.name),
            bytes: b.bytes(),
        })
        .collect();
    let input_bytes = regions.iter().map(|r| r.bytes).sum();
    let mut items: Vec<Item> = Vec::new();
    let mut skills: Vec<ListedSkill> = Vec::new();
    let mut multi_agent: Option<usize> = None;
    for b in &blocks {
        let spec = BLOCKS.iter().find(|s| s.name == b.name);
        match (b.name.as_str(), spec) {
            (names::SKILLS_FRAMING, _) => {
                let entries = parse_listing(&b.text);
                let entry_bytes: u64 = entries.iter().map(ListedSkill::bytes).sum();
                items.push(row(
                    Kind::SystemPrompt,
                    Layer::Builtin,
                    names::SKILLS_FRAMING,
                    Some(b.bytes().saturating_sub(entry_bytes)),
                ));
                let mut listing = row(
                    Kind::SkillListing,
                    Layer::Builtin,
                    names::SKILL_LISTING,
                    Some(entry_bytes),
                );
                listing.mode = Mode::Advertised;
                listing.label = format!("skill listing · {}", entries.len());
                for e in &entries {
                    let mut item = row(Kind::Skill, Layer::Unresolved, &e.name, Some(e.bytes()));
                    item.mode = Mode::Advertised;
                    item.origin.path = e.path.clone();
                    item.origin.why = vec![format!("{LISTED_PREFIX}{}", e.line)];
                    listing.children.push(item);
                }
                items.push(listing);
                skills.extend(entries);
            }
            ("multi_agent_role" | "multi_agent_mode", _) => match multi_agent {
                Some(at) => {
                    let o = items[at].cost.observed.unwrap_or(0) + b.bytes();
                    items[at].cost.observed = Some(o);
                }
                None => {
                    multi_agent = Some(items.len());
                    items.push(row(
                        Kind::SystemPrompt,
                        Layer::Builtin,
                        names::MULTI_AGENT,
                        Some(b.bytes()),
                    ));
                }
            },
            (names::DEVELOPER_INSTRUCTIONS, _) => items.push(row(
                Kind::Instruction,
                Layer::Unresolved,
                names::DEVELOPER_INSTRUCTIONS,
                Some(b.bytes()),
            )),
            (name, Some(spec)) => items.push(row(spec.row, Layer::Builtin, name, Some(b.bytes()))),
            (name, None) => items.push(row(
                Kind::SystemPrompt,
                Layer::Unresolved,
                name,
                Some(b.bytes()),
            )),
        }
    }
    for s in mcp_servers.iter().flatten().filter(|s| s.enabled) {
        let mut item = row(Kind::McpServer, Layer::Unresolved, &s.name, None);
        item.origin.why = vec!["codex mcp list --json: enabled".to_string()];
        items.push(item);
    }
    for p in plugins.iter().flatten().filter(|p| p.enabled) {
        let mut item = row(Kind::Plugin, Layer::Unresolved, &p.id, None);
        item.origin.why = vec![match p.source.as_deref() {
            Some(source) => {
                format!("codex plugin list --json: installed, enabled, source {source}")
            }
            None => "codex plugin list --json: installed, enabled".to_string(),
        }];
        items.push(item);
    }
    let tree = Tree {
        agent: Agent::Codex,
        target_dir: context.cwd.clone(),
        agent_version: context.agent_version.clone(),
        rules_verified_against: String::new(),
        tool_search: None,
        signals: Vec::new(),
        items,
        predicted_known: 0,
        unknown_rows: 0,
        observed_body: None,
        warnings: Vec::new(),
        home: None,
        observed_regions: regions.iter().map(|r| (r.name.clone(), r.bytes)).collect(),
        tool_search_cause: None,
    };
    CodexCapture {
        context,
        env,
        input_bytes,
        regions,
        blocks,
        skills,
        mcp_servers,
        plugins,
        bundled_root: None,
        models,
        tree,
        dir: None,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::walk;
    use serde_json::json;

    fn msg(role: &str, parts: &[(&str, Option<&str>)], with_kinds: bool) -> Value {
        let content: Vec<Value> = parts
            .iter()
            .map(|(text, _)| json!({"type": "input_text", "text": text}))
            .collect();
        let mut item = json!({"type": "message", "id": "msg_1", "role": role, "content": content});
        if with_kinds {
            let kinds: Vec<Value> = parts.iter().map(|(_, k)| json!(k)).collect();
            item["internal_chat_message_metadata_passthrough"] =
                json!({"turn_id": "t", "content_item_kinds": kinds});
        }
        item
    }

    const LISTING: &str = "<skills_instructions>\n## Skills\nintro\n### Skill roots\n- `r0` = `/synthetic/skills`\n### Available skills\n- alpha: First skill. (file: r0/alpha/SKILL.md)\n- demo:beta: Plugin skill: with a colon. (file: r0/beta/SKILL.md)\n### How to use skills\n- Discovery: text\n</skills_instructions>";

    #[test]
    fn blocks_are_named_by_content_kind_then_marker() {
        let input = json!([
            msg(
                "developer",
                &[
                    ("be brief", Some("generic.developer_instructions")),
                    (LISTING, Some("host_skills.instructions")),
                    ("<permissions instructions>\nx\n</permissions instructions>", Some("permissions.instructions")),
                    ("<persistent_mode>\ny</persistent_mode>", Some("persistent_mode.instructions")),
                ],
                true,
            ),
            msg("developer", &[("<multi_agent_role>\nr\n</multi_agent_role>", None)], false),
            msg(
                "user",
                &[
                    ("# AGENTS.md instructions for /synthetic/repo\n\n<INSTRUCTIONS>\nhi\n</INSTRUCTIONS>", None),
                    ("<environment_context>\n  <cwd>/synthetic/repo</cwd>\n</environment_context>", None),
                ],
                false,
            ),
        ]);
        let (blocks, warnings) = parse_prompt_input(&input.to_string()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let names: Vec<(&str, bool)> = blocks.iter().map(|b| (b.name.as_str(), b.known)).collect();
        assert_eq!(
            names,
            vec![
                ("developer_instructions", true),
                ("skills_instructions", true),
                ("permissions_instructions", true),
                ("persistent_mode.instructions", false),
                ("multi_agent_role", true),
                ("agents-md-message", true),
                ("environment_context", true),
            ]
        );
        assert_eq!(
            derived_cwd(&blocks),
            vec![
                (
                    PathBuf::from("/synthetic/repo"),
                    "<environment_context> <cwd>"
                ),
                (PathBuf::from("/synthetic/repo"), "AGENTS.md message header"),
            ]
        );
    }

    #[test]
    fn a_kind_and_a_marker_that_disagree_name_neither_and_warn() {
        // One kind per item, but the permissions and plugins kinds are swapped.
        let permissions = "<permissions instructions>\nx\n</permissions instructions>";
        let plugins = "<plugins_instructions>\ny\n</plugins_instructions>";
        let input = json!([msg(
            "developer",
            &[
                ("be brief", Some("generic.developer_instructions")),
                (plugins, Some("permissions.instructions")),
                (permissions, Some("plugins.usage_instructions")),
                (LISTING, Some("host_skills.instructions")),
            ],
            true,
        )]);
        let (blocks, warnings) = parse_prompt_input(&input.to_string()).unwrap();
        let names: Vec<(&str, bool)> = blocks.iter().map(|b| (b.name.as_str(), b.known)).collect();
        assert_eq!(
            names,
            vec![
                ("developer_instructions", true),
                ("input[0].part[1] (kind permissions.instructions, marker <plugins_instructions>)", false),
                ("input[0].part[2] (kind plugins.usage_instructions, marker <permissions instructions>)", false),
                ("skills_instructions", true),
            ]
        );
        assert_eq!(
            blocks[1].content_kind.as_deref(),
            Some("permissions.instructions")
        );
        assert_eq!(
            warnings,
            vec![
                "input[0].part[1]: its content kind permissions.instructions names permissions_instructions, but its text opens with the marker of plugins_instructions (<plugins_instructions>); named by neither, kept as an unknown block",
                "input[0].part[2]: its content kind plugins.usage_instructions names plugins_instructions, but its text opens with the marker of permissions_instructions (<permissions instructions>); named by neither, kept as an unknown block",
            ]
        );
        // A kind on a text that opens with no block's marker still names it; the
        // developer-instructions kind (it has no marker) on a marked text disagrees.
        let input = json!([msg(
            "developer",
            &[
                (permissions, Some("generic.developer_instructions")),
                ("plain", Some("permissions.instructions")),
            ],
            true,
        )]);
        let (blocks, warnings) = parse_prompt_input(&input.to_string()).unwrap();
        assert_eq!(
            blocks[0].name,
            "input[0].part[0] (kind generic.developer_instructions, marker <permissions instructions>)"
        );
        assert!(!blocks[0].known);
        assert_eq!(blocks[1].name, "permissions_instructions");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn kinds_not_one_per_content_item_are_set_aside_with_a_warning() {
        let mut input = json!([msg(
            "developer",
            &[
                ("be brief", Some("generic.developer_instructions")),
                (
                    "<plugins_instructions>\ny\n</plugins_instructions>",
                    Some("permissions.instructions")
                ),
            ],
            true,
        )]);
        input[0]["internal_chat_message_metadata_passthrough"]["content_item_kinds"]
            .as_array_mut()
            .unwrap()
            .push(json!("plugins.usage_instructions"));
        let (blocks, warnings) = parse_prompt_input(&input.to_string()).unwrap();
        let names: Vec<(&str, bool, Option<&str>)> = blocks
            .iter()
            .map(|b| (b.name.as_str(), b.known, b.content_kind.as_deref()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("developer_instructions", true, None),
                ("plugins_instructions", true, None),
            ]
        );
        assert_eq!(
            warnings,
            vec!["input[0] has 2 content items but 3 content_item_kinds; its blocks are named by their markers, not their kinds"]
        );
    }

    #[test]
    fn kinds_that_are_not_a_list_are_set_aside_with_a_warning() {
        let plugins = "<plugins_instructions>\ny\n</plugins_instructions>";
        let with_kinds = |kinds: Value| {
            let mut m = msg("developer", &[("be brief", None), (plugins, None)], true);
            m["internal_chat_message_metadata_passthrough"]["content_item_kinds"] = kinds;
            m
        };
        let input = json!([
            with_kinds(json!("permissions.instructions")),
            with_kinds(json!({"1": "permissions.instructions"})),
            with_kinds(Value::Null),
        ]);
        let (blocks, warnings) = parse_prompt_input(&input.to_string()).unwrap();
        let names: Vec<(&str, bool, Option<&str>)> = blocks
            .iter()
            .map(|b| (b.name.as_str(), b.known, b.content_kind.as_deref()))
            .collect();
        // By marker; the unmarked first part is developer_instructions in the first developer
        // message only.
        assert_eq!(
            names,
            vec![
                ("developer_instructions", true, None),
                ("plugins_instructions", true, None),
                ("input[1].part[0]", false, None),
                ("plugins_instructions", true, None),
                ("input[2].part[0]", false, None),
                ("plugins_instructions", true, None),
            ]
        );
        // `null` is no kinds, like a missing key: no warning.
        assert_eq!(
            warnings,
            vec![
                "input[0]: content_item_kinds is a string, not an array; its blocks are named by their markers, not their kinds",
                "input[1]: content_item_kinds is an object, not an array; its blocks are named by their markers, not their kinds",
            ]
        );
    }

    #[test]
    fn without_kinds_an_unmarked_first_developer_part_is_developer_instructions() {
        let input = json!([msg(
            "developer",
            &[("plain text", None), ("other plain", None)],
            false
        )]);
        let (blocks, _) = parse_prompt_input(&input.to_string()).unwrap();
        assert_eq!(blocks[0].name, "developer_instructions");
        assert_eq!(blocks[1].name, "input[0].part[1]");
        assert!(!blocks[1].known);
    }

    #[test]
    fn only_a_plain_absolute_home_is_provable() {
        assert!(plain_absolute(Path::new("/h/.codex")));
        assert!(plain_absolute(Path::new("/h/codex-home/")));
        for p in [
            "rel/home",
            ".codex",
            "/h/link/..",
            "/h/./x",
            "/h/x/.",
            "/../h",
        ] {
            assert!(!plain_absolute(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn listing_entries_keep_plugin_prefixes_and_skip_the_roots_table() {
        let entries = parse_listing(LISTING);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "demo:beta"]);
        assert_eq!(
            entries[0].path.as_deref(),
            Some(Path::new("/synthetic/skills/alpha/SKILL.md"))
        );
        assert_eq!(
            entries[0].bytes(),
            "- alpha: First skill. (file: r0/alpha/SKILL.md)\n".len() as u64
        );
    }

    #[test]
    fn capture_rows_split_the_listing_and_sum_the_multi_agent_messages() {
        let input = json!([
            msg(
                "developer",
                &[(LISTING, Some("host_skills.instructions"))],
                true
            ),
            msg(
                "developer",
                &[(
                    "<multi_agent_role>\nr\n</multi_agent_role>",
                    Some("multi_agent.role_instructions")
                )],
                true
            ),
            msg(
                "developer",
                &[(
                    "<multi_agent_mode>\nm\n</multi_agent_mode>",
                    Some("multi_agent.mode_instructions")
                )],
                true
            ),
        ]);
        let (blocks, _) = parse_prompt_input(&input.to_string()).unwrap();
        let context = RunContext {
            cwd: PathBuf::from("/synthetic/repo"),
            agent: Agent::Codex,
            agent_version: Some("0.160.1".to_string()),
            model: None,
            launch_opts: Vec::new(),
            settings_fingerprint: None,
            capture_kind: CAPTURE_KIND.to_string(),
            captured_at: String::new(),
        };
        let mcp = parse_mcp_list(r#"[{"name":"on","enabled":true},{"name":"off","enabled":false,"disabled_reason":null}]"#).unwrap();
        let plugins = parse_plugin_list(
            r#"{"installed":[{"pluginId":"p@m","installed":true,"enabled":true}],"available":[]}"#,
        )
        .unwrap();
        let c = build(
            context,
            BTreeMap::new(),
            blocks,
            Some(mcp),
            Some(plugins),
            None,
        );
        let observed = |id: &str| {
            walk(&c.tree)
                .find(|i| i.id == id)
                .unwrap_or_else(|| panic!("{id}"))
                .cost
                .observed
        };
        let entries: u64 = c.skills.iter().map(ListedSkill::bytes).sum();
        assert_eq!(
            observed("codex/skill-listing/builtin/skills"),
            Some(entries)
        );
        assert_eq!(
            observed("codex/system-prompt/builtin/skills_instructions"),
            Some(LISTING.len() as u64 - entries)
        );
        assert_eq!(
            observed("codex/skill/unresolved/demo:beta"),
            Some(c.skills[1].bytes())
        );
        let role = "<multi_agent_role>\nr\n</multi_agent_role>".len() as u64;
        let mode = "<multi_agent_mode>\nm\n</multi_agent_mode>".len() as u64;
        assert_eq!(
            observed("codex/system-prompt/builtin/multi_agent"),
            Some(role + mode)
        );
        // Two regions for the two messages, so a later split of the row joins cleanly.
        assert!(c
            .regions
            .iter()
            .any(|r| r.name == "input[1] developer · multi_agent_role"));
        assert!(walk(&c.tree).any(|i| i.id == "codex/mcp-server/unresolved/on"));
        assert!(!walk(&c.tree).any(|i| i.id == "codex/mcp-server/unresolved/off"));
        assert!(walk(&c.tree).any(|i| i.id == "codex/plugin/unresolved/p@m"));
        assert_eq!(
            c.input_bytes,
            c.regions.iter().map(|r| r.bytes).sum::<u64>()
        );
    }

    #[test]
    fn profile_goes_to_prompt_input_and_mcp_list_only() {
        let mut target = Target {
            dir: PathBuf::from("/synthetic/repo"),
            home: PathBuf::from("/synthetic/home"),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        };
        target
            .env
            .insert(PROFILE_ENV.to_string(), "work".to_string());
        let steps = planned_steps(&target);
        let profiled: Vec<&str> = steps
            .iter()
            .filter(|(args, _)| args.first().map(String::as_str) == Some("--profile"))
            .map(|(_, file)| *file)
            .collect();
        assert_eq!(profiled, vec!["prompt-input.json", "mcp-list.json"]);
        for (args, file) in &steps {
            if *file != "version.txt" {
                let at = args.iter().position(|a| a == "-C").unwrap();
                assert_eq!(args[at + 1], "/synthetic/repo", "{file}");
            }
            assert!(!args
                .iter()
                .any(|a| a == "-c" || a == "--enable" || a == "--disable"));
        }
    }

    #[test]
    fn version_line_parses() {
        assert_eq!(
            parse_version("codex-cli 0.160.1\n").as_deref(),
            Some("0.160.1")
        );
        assert_eq!(parse_version("nothing"), None);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "harn-oracle-codex-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn a_message_whose_content_is_not_an_array_is_kept_as_unknown() {
        let input = json!([{"type": "message", "role": "developer", "content": "loose"}]);
        let (blocks, _) = parse_prompt_input(&input.to_string()).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].name, "input[0].content");
        assert!(!blocks[0].known);
        assert_eq!(blocks[0].text, "\"loose\"");
    }

    #[test]
    #[cfg(unix)]
    fn the_capture_dir_is_created_exclusively_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("dir");
        let dir = root.join("capture");
        create_private_dir(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert!(
            create_private_dir(&dir).is_err(),
            "an existing dir is not reused"
        );
        let target = root.join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, root.join("link")).unwrap();
        assert!(
            create_private_dir(&root.join("link")).is_err(),
            "a symlink is not followed"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn the_wait_loop_kills_once_on_timeout_or_a_wait_error_never_on_echild_or_an_exit() {
        // `run_step`'s wait loop, with a fake child that counts its polls and the kill action's
        // calls.
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;
        #[derive(Default)]
        struct Child {
            polls: u32,
            kills: u32,
        }
        let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let kill = |c: &mut Child| c.kills += 1;
        let failing =
            |errno: i32| move |_: &mut Child| Err(std::io::Error::from_raw_os_error(errno));

        // Exited with a code: the code, no kill.
        let mut c = Child::default();
        let exit = supervise(
            &mut c,
            |_| Ok(Some(ExitStatus::from_raw(3 << 8))),
            kill,
            later,
        );
        assert_eq!((exit.unwrap(), c.kills), (Some(3), 0));

        // Ended by a signal: no code, no kill.
        let mut c = Child::default();
        let signalled = ExitStatus::from_raw(libc::SIGTERM);
        let exit = supervise(&mut c, |_| Ok(Some(signalled)), kill, later);
        assert_eq!((exit.unwrap(), c.kills), (None, 0));

        // Still running for a few polls, then exited: polled until then, its code, no kill.
        let mut c = Child::default();
        let exit = supervise(
            &mut c,
            |c| {
                c.polls += 1;
                Ok((c.polls == 3).then(|| ExitStatus::from_raw(0)))
            },
            kill,
            later,
        );
        assert_eq!((exit.unwrap(), c.polls, c.kills), (Some(0), 3, 0));

        // Reaped elsewhere (ECHILD): the error, no kill.
        let mut c = Child::default();
        let err = supervise(&mut c, failing(libc::ECHILD), kill, later).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ECHILD));
        assert_eq!(c.kills, 0, "a child reaped elsewhere is not signalled");

        // Any other wait error, EINTR included, is not retried: one kill, then the error.
        let mut c = Child::default();
        let err = supervise(
            &mut c,
            |c| {
                c.polls += 1;
                Err(std::io::Error::from_raw_os_error(libc::EINTR))
            },
            kill,
            later,
        )
        .unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::EINTR));
        assert_eq!((c.polls, c.kills), (1, 1), "polled once, killed once");

        // Still running at the deadline: killed once, no exit code.
        let mut c = Child::default();
        let exit = supervise(&mut c, |_| Ok(None), kill, std::time::Instant::now());
        assert_eq!((exit.unwrap(), c.kills), (None, 1));
    }

    #[test]
    #[cfg(unix)]
    fn a_timeout_kills_the_whole_process_group() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("group");
        let pidfile = root.join("descendant.pid");
        let bin = root.join("codex");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\n/bin/sleep 30 &\necho $! > {}\nexec /bin/sleep 30\n",
                pidfile.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = Target {
            dir: root.clone(),
            home: root.clone(),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        };
        // Under load the group can be killed before the script has recorded its descendant (no
        // pidfile, or an empty one): such a run proves nothing either way, so it is run again
        // with twice the timeout, up to 3 runs.
        let timeouts = [1500, 3000, 6000].map(std::time::Duration::from_millis);
        let mut recorded = None;
        for timeout in timeouts {
            let _ = std::fs::remove_file(&pidfile);
            let exit = run_step(
                &target,
                &bin,
                &[],
                &root.join("out"),
                &root.join("err"),
                timeout,
            )
            .unwrap();
            assert_eq!(exit, None, "killed at the timeout");
            recorded = std::fs::read_to_string(&pidfile)
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
                .filter(|&pid| pid > 0);
            if recorded.is_some() {
                break;
            }
        }
        let Some(pid) = recorded else {
            std::fs::remove_dir_all(&root).unwrap();
            panic!(
                "the script never recorded its descendant before being killed (timeouts {timeouts:?}): the host is too loaded for this run to show anything"
            );
        };
        // A killed descendant is a zombie until init reaps the orphan; poll for that.
        let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let survived = alive(pid);
        if survived {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        std::fs::remove_dir_all(root).unwrap();
        assert!(!survived, "the descendant {pid} outlived the timeout");
    }
}
