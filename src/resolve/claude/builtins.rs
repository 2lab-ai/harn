//! Built-in blocks (`research/01` §0, §10) and the settings-only rows (§2, §6). The built-in
//! prompt (`system` with its three parts), the environment section, the date, userEmail, the
//! gitStatus block, the attribution reminder and the built-in tools carry no byte count a file can
//! give: cost `?` until a capture. Ids are the ones the request-body parser builds
//! (`oracle::claude::names`, 03-p0-interfaces §Amendments after WU-D; hardcoded here because this
//! branch predates that module): `system`, `system/billing-header`, `system/identity`,
//! `system/harness-rules`, `environment`, `date`, `gitStatus`, `userEmail` (an `attribution` kind),
//! and each built-in tool by the name it has in the request body (`Agent`, not the `init` event's
//! `Task`; `advisor`). Off switches: gitStatus → `includeGitInstructions: false` /
//! `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS=1`; attribution reminder → `attribution: {commit: "", pr:
//! ""}`; a tool → a bare `permissions.deny` entry (removes it from context) or its
//! `CLAUDE_CODE_DISABLE_*` variable; prompt / environment / date / userEmail → none. Output style
//! (`outputStyle`), `statusLine` (mode none: runs locally; `disableAllHooks` stops it, and so does
//! a managed `allowManagedHooksOnly` when it is set outside managed settings, said in its
//! `signal`) and permission rule lists (mode none) come from the merged settings.

use super::fs::file_name;
use super::hooks::{managed_hooks_only, OFF_ALL_HOOKS, OFF_MANAGED_HOOKS_ONLY};
use super::settings::Source;
use super::{cost_na, env_knob, frontmatter, json_string, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::{json, Value};

/// Built-in tool names as the request body's `tools[].name` spells them (2026-10-13 capture of
/// Claude Code 2.1.296: 25 plus `advisor`, which is listed only when `advisorModel` is set) plus the
/// four that run had disallowed (Bash, Edit, Write, NotebookEdit). Static table.
const TOOLS: &[&str] = &[
    "Agent",
    "Bash",
    "CronCreate",
    "CronDelete",
    "CronList",
    "DesignSync",
    "Edit",
    "EnterWorktree",
    "ExitWorktree",
    "Glob",
    "Grep",
    "ListAgents",
    "ListMcpResourcesTool",
    "Monitor",
    "NotebookEdit",
    "PushNotification",
    "Read",
    "ReadMcpResourceDirTool",
    "ReadMcpResourceTool",
    "RemoteTrigger",
    "ReportFindings",
    "ScheduleWakeup",
    "SendMessage",
    "Skill",
    "TaskStop",
    "WebFetch",
    "WebSearch",
    "Workflow",
    "Write",
];

/// The three parts of the `system` array (`oracle::claude` names them by content).
const SYSTEM_PARTS: &[(&str, &str)] = &[
    ("system/billing-header", "billing header"),
    ("system/identity", "identity"),
    ("system/harness-rules", "harness rules"),
];

/// Tool → the documented `CLAUDE_CODE_DISABLE_*` variable that removes it.
fn tool_env(tool: &str) -> Option<&'static str> {
    match tool {
        "WebFetch" => Some("CLAUDE_CODE_DISABLE_WEB_FETCH"),
        "CronCreate" | "CronDelete" | "CronList" => Some("CLAUDE_CODE_DISABLE_CRON"),
        _ => None,
    }
}

fn builtin(kind: Kind, name: &str, label: &str, why: &str, knobs: Vec<Knob>) -> Item {
    let mut item = new_item(kind, Layer::Builtin, name, label);
    item.origin.why = vec![why.to_string()];
    item.knobs = knobs;
    item
}

pub(crate) fn blocks(ctx: &Ctx) -> Vec<Item> {
    let none = |reason: &str| {
        vec![Knob::None {
            reason: reason.to_string(),
        }]
    };
    let mut system = builtin(
        Kind::SystemPrompt,
        "system",
        "system prompt (built-in)",
        "built into Claude Code; the text is not published",
        none("built-in"),
    );
    system.children = SYSTEM_PARTS
        .iter()
        .map(|(name, label)| {
            builtin(
                Kind::SystemPrompt,
                name,
                label,
                "built into Claude Code",
                none("built-in"),
            )
        })
        .collect();
    let mut out = vec![
        system,
        builtin(
            Kind::EnvBlock,
            "environment",
            "environment section (cwd, platform, shell, OS)",
            "built into Claude Code",
            none("built-in; no documented switch"),
        ),
        builtin(
            Kind::EnvBlock,
            "date",
            "date",
            "built into Claude Code",
            none("built-in; no documented switch"),
        ),
    ];
    let email = ctx
        .claude_json
        .as_ref()
        .and_then(|c| c.pointer("/oauthAccount/emailAddress"))
        .and_then(Value::as_str)
        .is_some_and(|e| !e.is_empty());
    if email {
        out.push(builtin(
            Kind::Attribution,
            "userEmail",
            "userEmail",
            "~/.claude.json oauthAccount (claude.ai login); undocumented injection",
            none("undocumented; no switch (anthropics/claude-code#81138)"),
        ));
    }

    let git_off = ctx
        .settings
        .get_bool("/includeGitInstructions")
        .is_some_and(|(b, _)| !b)
        || ctx.env_on("CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS");
    if !git_off {
        let why = match &ctx.git_root {
            Some(root) => format!(
                "git repository at {}: gitStatus snapshot + Bash git instructions",
                root.display()
            ),
            None => "not a git repository: Bash git instructions only".to_string(),
        };
        out.push(builtin(
            Kind::GitBlock,
            "gitStatus",
            "gitStatus + git instructions",
            &why,
            vec![
                setting_knob(
                    ctx,
                    Scope::Local,
                    "/includeGitInstructions",
                    json!(false),
                    None,
                ),
                env_knob(
                    "CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS",
                    "1",
                    Scope::Local,
                    None,
                ),
            ],
        ));
    }

    let attribution = ctx.settings.get("/attribution").map(|(v, _)| v.clone());
    let attribution_off = match &attribution {
        Some(v) => {
            v.get("commit").and_then(Value::as_str) == Some("")
                && v.get("pr").and_then(Value::as_str) == Some("")
        }
        None => ctx
            .settings
            .get_bool("/includeCoAuthoredBy")
            .is_some_and(|(b, _)| !b),
    };
    if !attribution_off {
        out.push(builtin(
            Kind::Attribution,
            "attribution",
            "attribution reminder (commit / PR)",
            "built-in reminder; text from the attribution setting when set",
            vec![setting_knob(
                ctx,
                Scope::Local,
                "/attribution",
                json!({"commit": "", "pr": ""}),
                None,
            )],
        ));
    }

    let denied: Vec<String> = ctx
        .settings
        .strings("/permissions/deny")
        .into_iter()
        .map(|(v, _)| v)
        .collect();
    for tool in TOOLS {
        let env = tool_env(tool);
        if denied.iter().any(|d| d == tool) || env.is_some_and(|e| ctx.env_on(e)) {
            continue;
        }
        let mut knobs = vec![setting_knob(
            ctx,
            Scope::Local,
            "/permissions/deny/-",
            json!(tool),
            Some("a bare tool name removes the tool from context".to_string()),
        )];
        if let Some(e) = env {
            knobs.push(env_knob(e, "1", Scope::Local, None));
        }
        out.push(builtin(
            Kind::BuiltinTool,
            tool,
            tool,
            "built-in tool (static table, Claude Code 2.1.296 request body)",
            knobs,
        ));
    }
    if !ctx.env_on("CLAUDE_CODE_DISABLE_ADVISOR_TOOL") {
        if let Some((model, file)) = ctx.settings.get_str("/advisorModel") {
            out.push(builtin(
                Kind::BuiltinTool,
                "advisor",
                "advisor",
                &format!("{} = {}", file.at("/advisorModel"), json_string(model)),
                vec![env_knob(
                    "CLAUDE_CODE_DISABLE_ADVISOR_TOOL",
                    "1",
                    Scope::Local,
                    None,
                )],
            ));
        }
    }
    out
}

pub(crate) fn settings_rows(ctx: &Ctx) -> Vec<Item> {
    let mut out = Vec::new();
    if let Some((style, file)) = ctx.settings.get_str("/outputStyle") {
        if !style.is_empty() && !style.eq_ignore_ascii_case("default") {
            let setting = file.at("/outputStyle");
            let mut item = new_item(
                Kind::OutputStyle,
                file.source.layer(),
                style,
                &format!("output style: {style}"),
            );
            item.origin.setting = Some(setting.clone());
            item.origin.why = vec![format!("{setting} = {}", json_string(style))];
            match find_style(ctx, style) {
                Some((path, bytes)) => {
                    item.origin.path = Some(ctx.fs.realpath(&path));
                    item.origin.why.push(ctx.display(&path));
                    item.cost = Cost {
                        source: Some(bytes),
                        predicted: Some(bytes),
                        predicted_is_estimate: true,
                        ..Cost::default()
                    };
                }
                None => item
                    .origin
                    .why
                    .push("built-in or plugin style: size unknown until a capture".to_string()),
            }
            item.knobs = vec![setting_knob(
                ctx,
                Scope::Local,
                "/outputStyle",
                json!("default"),
                None,
            )];
            out.push(item);
        }
    }

    // `disableAllHooks` stops the status line too: it stays a row, saying so.
    let hooks_off = ctx
        .settings
        .get_bool("/disableAllHooks")
        .filter(|(b, _)| *b)
        .map(|(_, f)| format!("{} = true", f.at("/disableAllHooks")));
    if let Some((_, file)) = ctx.settings.get("/statusLine") {
        let setting = file.at("/statusLine");
        let mut item = new_item(
            Kind::StatusLine,
            file.source.layer(),
            &setting,
            "statusLine",
        );
        item.mode = Mode::None;
        item.cost = cost_na(None);
        item.origin.path = Some(file.path.clone());
        item.origin.setting = Some(setting.clone());
        item.origin.why = vec![setting, "runs locally; never reaches the model".to_string()];
        item.knobs = vec![Knob::None {
            reason: "not in the request".to_string(),
        }];
        if let Some(set_at) = hooks_off {
            item.origin.why.push(set_at);
            item.signal = Some(OFF_ALL_HOOKS.to_string());
        } else if let (Some(set_at), false) =
            (managed_hooks_only(ctx), file.source == Source::Managed)
        {
            // `allowManagedHooksOnly` reads the status line from managed settings only.
            item.origin.why.push(set_at);
            item.signal = Some(OFF_MANAGED_HOOKS_ONLY.to_string());
            item.knobs = vec![Knob::None {
                reason: "managed policy allowManagedHooksOnly runs a status line from managed \
                         settings only; no user setting overrides it"
                    .to_string(),
            }];
        }
        out.push(item);
    }

    for file in &ctx.settings.files {
        for list in ["allow", "deny", "ask"] {
            let Some(Value::Array(rules)) = file.value.pointer(&format!("/permissions/{list}"))
            else {
                continue;
            };
            if rules.is_empty() {
                continue;
            }
            let setting = file.at(&format!("/permissions/{list}"));
            let mut item = new_item(
                Kind::PermissionRule,
                file.source.layer(),
                &setting,
                &format!(
                    "permissions.{list} · {} rules · {}",
                    rules.len(),
                    file_name(&file.path)
                ),
            );
            item.mode = Mode::None;
            item.cost = cost_na(None);
            item.origin.path = Some(file.path.clone());
            item.origin.setting = Some(setting.clone());
            item.origin.why = vec![setting];
            if list == "deny" {
                item.origin.why.push(
                    "bare tool names here remove the tool from context (see built-in tools)"
                        .to_string(),
                );
            }
            item.knobs = vec![Knob::None {
                reason: "not in the request".to_string(),
            }];
            out.push(item);
        }
    }
    out
}

/// `<name>.md` (or a file whose frontmatter `name` matches) in `~/.claude/output-styles` or
/// `.claude/output-styles` of the target and its parents up to the git root; bytes = body.
fn find_style(ctx: &Ctx, style: &str) -> Option<(std::path::PathBuf, u64)> {
    let mut roots = vec![ctx.home().join(".claude/output-styles")];
    roots.extend(
        ctx.project_dirs()
            .into_iter()
            .map(|d| d.join(".claude/output-styles")),
    );
    for root in roots {
        for file in ctx.fs.walk_md(&root) {
            let Some(text) = ctx.fs.read_string(&file) else {
                continue;
            };
            let fm_name =
                frontmatter::parse(&text).and_then(|fm| frontmatter::get_str(&fm, "name"));
            let stem = file_name(&file).trim_end_matches(".md").to_string();
            if fm_name.as_deref() == Some(style) || stem == style {
                let body = frontmatter::split(&text).map(|(_, b)| b).unwrap_or(&text);
                return Some((file, body.len() as u64));
            }
        }
    }
    None
}
