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
//!   configured still loads but is not listed (measured on 0.160.1, 2026-10-14).
//!
//! `--profile` (from [`PROFILE_ENV`], as the resolver reads it) goes to prompt-input and mcp list
//! only: codex rejects it for every other subcommand (`cli/src/main.rs:1861-1889`). No other flag
//! is passed — none that changes config.
//!
//! # Capture directory
//!
//! Every run keeps its raw outputs in a directory so `harn verify --capture <dir>` re-checks them
//! offline: [`MANIFEST`] (schema [`MANIFEST_SCHEMA`]: the `RunContext`, the environment that
//! decides what codex reads, and each command's argv, exit status and files) plus one stdout and
//! one stderr file per command.
//!
//! # How the prompt input is cut
//!
//! Each content item of each message is one block. A message carries
//! `internal_chat_message_metadata_passthrough.content_item_kinds`, one kind per content item
//! (`core/src/context_manager/updates.rs:12-30`); the kind names the block. Without it (or for a
//! kind not in [`BLOCKS`]) the block's opening marker decides; an unmarked first part of the
//! first developer message is `developer_instructions` (`core/src/session/mod.rs:4321-4328`).
//! Anything else is kept under its kind (or position) so `verify` can report it. A region is one
//! block: `input[<i>] <role> · <block>`, its bytes the UTF-8 length of the text; the regions
//! partition the text of the input list, not a wire body (instructions and tools are absent).

use super::Region;
use crate::model::{
    item_id, now_iso8601, Agent, Cost, Item, Kind, Layer, Mode, Origin, RunContext, Target, Tree,
};
use crate::resolve::codex::{codex_home, PROFILE_ENV};
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
    /// `content_item_kinds[part]`, when the message carries it.
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
}

/// One command of a run, as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// The argv after the binary.
    pub args: Vec<String>,
    /// Stdout file, relative to the capture dir.
    pub stdout: String,
    /// Stderr file, relative to the capture dir.
    pub stderr: String,
    /// `None` when it was killed or could not start.
    pub exit: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub context: RunContext,
    /// [`BINDING_ENV`] as the run saw them (absent = unset).
    pub env: BTreeMap<String, Option<String>>,
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
        let stderr = stdout
            .rsplit_once('.')
            .map_or(stdout.to_string(), |(stem, _)| stem.to_string())
            + ".stderr";
        let exit = run_step(
            target,
            codex_bin,
            &args,
            &dir.join(stdout),
            &dir.join(&stderr),
        )?;
        if stdout == "prompt-input.json" && exit != Some(0) {
            let tail = std::fs::read_to_string(dir.join(&stderr)).unwrap_or_default();
            let tail: String = tail.lines().rev().take(5).collect::<Vec<_>>().join(" | ");
            bail!(
                "{} {} exited with {} (outputs kept in {}); stderr tail: {tail}",
                codex_bin.display(),
                args.join(" "),
                exit.map_or("no status (killed)".to_string(), |c| c.to_string()),
                dir.display()
            );
        }
        steps.push(Step {
            args,
            stdout: stdout.to_string(),
            stderr,
            exit,
        });
    }
    let version = std::fs::read_to_string(dir.join("version.txt"))
        .ok()
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
        steps,
    };
    std::fs::write(
        dir.join(MANIFEST),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )
    .with_context(|| format!("writing {}", dir.join(MANIFEST).display()))?;
    read_capture_dir(&dir, target)
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
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(std::fs::canonicalize(&dir).unwrap_or(dir))
}

/// Runs one command; its exit code, or `None` when it was killed at [`RUN_TIMEOUT`].
fn run_step(
    target: &Target,
    codex_bin: &Path,
    args: &[String],
    stdout: &Path,
    stderr: &Path,
) -> anyhow::Result<Option<i32>> {
    use std::process::{Command, Stdio};
    let out = std::fs::File::create(stdout)?;
    let err = std::fs::File::create(stderr)?;
    let mut child = Command::new(codex_bin)
        .args(args)
        .current_dir(&target.dir)
        .env_clear()
        .envs(&target.env)
        .env("HOME", &target.home)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .with_context(|| format!("starting {}", codex_bin.display()))?;
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
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

/// Reads a capture directory (or its manifest) and binds it to `target`: the working directory
/// the input list itself names (`<environment_context>` `<cwd>`, else the AGENTS.md header) must
/// be the manifest's and the target's, else Err naming the mismatch.
pub fn read_capture_dir(path: &Path, target: &Target) -> anyhow::Result<CodexCapture> {
    let dir = if path.is_file() {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        path.to_path_buf()
    };
    let manifest_path = dir.join(MANIFEST);
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
    let mut warnings = Vec::new();
    let read = |file: &str, warnings: &mut Vec<String>| -> Option<String> {
        let step = manifest.steps.iter().find(|s| s.stdout == file);
        if let Some(step) = step {
            if step.exit != Some(0) {
                let tail = std::fs::read_to_string(dir.join(&step.stderr)).unwrap_or_default();
                let tail: String = tail.lines().take(2).collect::<Vec<_>>().join(" | ");
                warnings.push(format!(
                    "codex {} exited with {}: not read ({tail})",
                    step.args.join(" "),
                    step.exit
                        .map_or("no status (killed)".to_string(), |c| c.to_string())
                ));
                return None;
            }
        }
        match std::fs::read_to_string(dir.join(file)) {
            Ok(text) => Some(text),
            Err(err) => {
                warnings.push(format!("{file}: {err}"));
                None
            }
        }
    };
    let prompt = read("prompt-input.json", &mut warnings).ok_or_else(|| {
        anyhow!(
            "{}: no prompt-input.json ({})",
            dir.display(),
            warnings.join("; ")
        )
    })?;
    let blocks = parse_prompt_input(&prompt).context("prompt-input.json")?;
    let mcp_servers = read("mcp-list.json", &mut warnings).and_then(|t| {
        parse_mcp_list(&t)
            .map_err(|e| warnings.push(format!("mcp-list.json: {e:#}")))
            .ok()
    });
    let plugins = read("plugin-list.json", &mut warnings).and_then(|t| {
        parse_plugin_list(&t)
            .map_err(|e| warnings.push(format!("plugin-list.json: {e:#}")))
            .ok()
    });
    let models = read("models.json", &mut warnings).and_then(|t| {
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

/// Err unless the capture is about `target.dir`.
fn bind(capture: &CodexCapture, target: &Target) -> anyhow::Result<()> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if let Some((cwd, evidence)) = derived_cwd(&capture.blocks) {
        let cwd = canonical(&cwd);
        if cwd != canonical(&capture.context.cwd) {
            bail!(
                "the input list names {} ({evidence}), the manifest {}",
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

/// The working directory the input list names, and where.
pub fn derived_cwd(blocks: &[Block]) -> Option<(PathBuf, &'static str)> {
    if let Some(env) = blocks.iter().find(|b| b.name == names::ENVIRONMENT) {
        let start = env.text.find("<cwd>")? + "<cwd>".len();
        let end = env.text[start..].find("</cwd>")? + start;
        return Some((
            PathBuf::from(&env.text[start..end]),
            "<environment_context> <cwd>",
        ));
    }
    let agents = blocks.iter().find(|b| b.name == names::AGENTS_MD)?;
    let first = agents.text.lines().next()?;
    let dir = first.strip_prefix("# AGENTS.md instructions for ")?;
    Some((PathBuf::from(dir), "AGENTS.md message header"))
}

/// The input list (`codex debug prompt-input` stdout: a JSON array of `ResponseItem`s, see
/// `protocol/src/models.rs`) cut into blocks.
pub fn parse_prompt_input(text: &str) -> anyhow::Result<Vec<Block>> {
    let items: Value = serde_json::from_str(text.trim()).context("not JSON")?;
    let items = items
        .as_array()
        .ok_or_else(|| anyhow!("not a JSON array of response items"))?;
    let mut blocks = Vec::new();
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
        let kinds: Vec<Option<&str>> = item["internal_chat_message_metadata_passthrough"]
            ["content_item_kinds"]
            .as_array()
            .map(|a| a.iter().map(Value::as_str).collect())
            .unwrap_or_default();
        let content = item["content"].as_array().cloned().unwrap_or_default();
        for (j, part) in content.iter().enumerate() {
            let content_kind = kinds.get(j).copied().flatten();
            let text = match part["text"].as_str() {
                Some(t) => t.to_string(),
                None => serde_json::to_string(part).unwrap_or_default(),
            };
            let unmarked_first = first_developer == Some(i) && j == 0;
            let (name, known) = classify(content_kind, &text, unmarked_first, i, j);
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
    Ok(blocks)
}

/// The block's name: by content kind, else by marker, else `developer_instructions` for the
/// unmarked first part of the first developer message, else its kind or position (unknown).
fn classify(
    kind: Option<&str>,
    text: &str,
    unmarked_first: bool,
    input: usize,
    part: usize,
) -> (String, bool) {
    if let Some(spec) = kind.and_then(|k| BLOCKS.iter().find(|b| b.kinds.contains(&k))) {
        return (spec.name.to_string(), true);
    }
    if let Some(spec) = BLOCKS
        .iter()
        .find(|b| !b.marker.is_empty() && text.starts_with(b.marker))
    {
        return (spec.name.to_string(), true);
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
/// skills keep their `plugin:` prefix.
pub fn parse_listing(text: &str) -> Vec<ListedSkill> {
    let mut out = Vec::new();
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
        out.push(ListedSkill {
            name: name.to_string(),
            line: line.to_string(),
        });
    }
    out
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

/// `codex plugin list --json`: `{installed: [{pluginId, enabled, …}], available: […]}`; only
/// `installed` entries are kept.
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
        item.origin.why = vec!["codex plugin list --json: installed, enabled".to_string()];
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
        let blocks = parse_prompt_input(&input.to_string()).unwrap();
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
            Some((
                PathBuf::from("/synthetic/repo"),
                "<environment_context> <cwd>"
            ))
        );
    }

    #[test]
    fn without_kinds_an_unmarked_first_developer_part_is_developer_instructions() {
        let input = json!([msg(
            "developer",
            &[("plain text", None), ("other plain", None)],
            false
        )]);
        let blocks = parse_prompt_input(&input.to_string()).unwrap();
        assert_eq!(blocks[0].name, "developer_instructions");
        assert_eq!(blocks[1].name, "input[0].part[1]");
        assert!(!blocks[1].known);
    }

    #[test]
    fn listing_entries_keep_plugin_prefixes_and_skip_the_roots_table() {
        let entries = parse_listing(LISTING);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "demo:beta"]);
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
        let blocks = parse_prompt_input(&input.to_string()).unwrap();
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
}
