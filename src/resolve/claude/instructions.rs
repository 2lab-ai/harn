//! Instruction files (`research/01` §1), in load order: managed `CLAUDE.md` (and managed
//! settings' inline `claudeMd`), `~/.claude/CLAUDE.md`, `~/.claude/rules/**/*.md`, then for every
//! directory from the filesystem root down to the target: `CLAUDE.md`, `.claude/CLAUDE.md`,
//! `.claude/rules/**/*.md` (a `paths:` frontmatter makes a rule on-demand), `CLAUDE.local.md`.
//! `AGENTS.md` loads only when no `CLAUDE.md` / `.claude/CLAUDE.md` / `CLAUDE.local.md` exists at
//! or above the target (`pluginConfigs["cc-plugin-agents-md@builtin"].options.instructionFiles`
//! changes that). `@path` imports (relative to the importing file, ≤ 4 hops, never inside code
//! spans, fences or HTML comments; the token runs to the next whitespace, so `@rules/STV.md.`
//! names a file that does not exist — observed 2026-10-13) are children of the importing file.
//! `claudeMdExcludes` globs (absolute paths; arrays merge) drop a file and its imports; managed
//! files cannot be excluded. Files over 4 MiB are skipped. `cost.source` = file bytes;
//! `cost.predicted` = the embedded section (`Contents of <path> (<desc>):` header + 3 + the file
//! without its YAML frontmatter), the unit the request-body parser measures.

use super::fs::esc;
use super::{cost_na, frontmatter, glob, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode, Scope};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAX_DEPTH: usize = 4;

/// The `(<desc>)` of a `Contents of <path> (<desc>):` header, by layer (the 2.1.296 binary's
/// switch: Managed / User / Project / Local; imports keep their importer's layer).
pub(crate) fn describe(layer: Layer) -> &'static str {
    match layer {
        Layer::Managed => "organization-managed policy instructions",
        Layer::User => "user's private global instructions for all projects",
        Layer::Local => "user's private project instructions, not checked in",
        _ => "project instructions, checked into the codebase",
    }
}

/// Bytes of one embedded section: `Contents of <path> (<desc>):` + 3 + the embedded text
/// (03-p0-interfaces §Amendments after WU-D; byte-exact on the 2026-10-13 capture for
/// `~/.claude/CLAUDE.md` 738, zbrain `CLAUDE.md` 6,835 and `rules/PRINCIPLES.md` 4,261).
pub(crate) fn section_bytes(path: &Path, desc: &str, body: u64) -> u64 {
    format!("Contents of {} ({desc}):", path.display()).len() as u64 + 3 + body
}

/// The text Claude Code embeds: YAML frontmatter and the blank line after it are dropped.
pub(crate) fn strip_frontmatter(text: &str) -> &str {
    match frontmatter::split(text) {
        Some((_, body)) => body
            .strip_prefix("\r\n")
            .or_else(|| body.strip_prefix('\n'))
            .unwrap_or(body),
        None => text,
    }
}
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const AGENTS_MD_PLUGIN: &str = "cc-plugin-agents-md@builtin";

struct Walk {
    seen: HashSet<PathBuf>,
    excludes: Vec<(String, String)>,
}

pub(crate) fn collect(ctx: &mut Ctx) -> Vec<Item> {
    let mut out = Vec::new();
    let mut w = Walk {
        seen: HashSet::new(),
        excludes: ctx.settings.strings("/claudeMdExcludes"),
    };

    for dir in super::settings::MANAGED_DIRS {
        let path = Path::new(dir).join("CLAUDE.md");
        if ctx.fs.is_file(&path) {
            let why = "managed CLAUDE.md (loaded first, cannot be excluded)".to_string();
            out.extend(file_item(
                ctx,
                &mut w,
                &path,
                Layer::Managed,
                Mode::Always,
                why,
                0,
            ));
        }
    }
    for file in &ctx.settings.files {
        if file.source != super::settings::Source::Managed {
            continue;
        }
        if let Some(Value::String(text)) = file.value.get("claudeMd") {
            let setting = file.at("/claudeMd");
            let mut item = new_item(
                Kind::Instruction,
                Layer::Managed,
                &setting,
                "managed claudeMd",
            );
            item.cost = Cost {
                source: Some(text.len() as u64),
                predicted: Some(text.len() as u64),
                // The embedding form of an inline managed `claudeMd` is not captured yet.
                predicted_is_estimate: true,
                ..Cost::default()
            };
            item.origin.setting = Some(setting.clone());
            item.origin.why = vec![setting];
            item.knobs = vec![Knob::None {
                reason: "managed: cannot be excluded".to_string(),
            }];
            out.push(item);
        }
    }

    if ctx.env_on("CLAUDE_CODE_DISABLE_CLAUDE_MDS") {
        return out;
    }

    let user_md = ctx.home().join(".claude/CLAUDE.md");
    out.extend(file_item(
        ctx,
        &mut w,
        &user_md,
        Layer::User,
        Mode::Always,
        "user CLAUDE.md".to_string(),
        0,
    ));
    for rule in ctx.fs.walk_md(&ctx.home().join(".claude/rules")) {
        let mode = rule_mode(ctx, &rule);
        out.extend(file_item(
            ctx,
            &mut w,
            &rule,
            Layer::User,
            mode,
            "user rule (~/.claude/rules)".to_string(),
            0,
        ));
    }

    let instruction_files = ctx
        .settings
        .get_str(&format!(
            "/pluginConfigs/{}/options/instructionFiles",
            esc(AGENTS_MD_PLUGIN)
        ))
        .map(|(v, _)| v.to_string())
        .unwrap_or_else(|| "claude-md-or-agents-md".to_string());
    if instruction_files == "managed-only" {
        return out;
    }
    let agents_plugin_on = !matches!(
        ctx.settings
            .get_bool(&format!("/enabledPlugins/{}", esc(AGENTS_MD_PLUGIN))),
        Some((false, _))
    );
    let chain = ctx.chain_dirs();
    let has_claude_md = chain.iter().any(|d| {
        ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"]
            .iter()
            .map(|f| d.join(f))
            .any(|p| p != user_md && ctx.fs.is_file(&p))
    });
    let load_agents_md = agents_plugin_on
        && match instruction_files.as_str() {
            "claude-md-and-agents-md" => true,
            "claude-md" => false,
            _ => !has_claude_md,
        };

    let target_dir = ctx.dir().to_path_buf();
    let home = ctx.home().to_path_buf();
    for dir in &chain {
        let rel_why = |f: &str| {
            if *dir == target_dir {
                format!("{f} in the target dir")
            } else {
                format!("{f} in ancestor {}", dir.display())
            }
        };
        for name in ["CLAUDE.md", ".claude/CLAUDE.md"] {
            let path = dir.join(name);
            if ctx.fs.is_file(&path) {
                let why = rel_why(name);
                out.extend(file_item(
                    ctx,
                    &mut w,
                    &path,
                    Layer::Project,
                    Mode::Always,
                    why,
                    0,
                ));
            }
        }
        if *dir != home {
            for rule in ctx.fs.walk_md(&dir.join(".claude/rules")) {
                let mode = rule_mode(ctx, &rule);
                let why = rel_why(".claude/rules");
                out.extend(file_item(ctx, &mut w, &rule, Layer::Project, mode, why, 0));
            }
        }
        if load_agents_md {
            let path = dir.join("AGENTS.md");
            if ctx.fs.is_file(&path) {
                let why = format!(
                    "{} (instructionFiles = {instruction_files}; no CLAUDE.md at or above the target)",
                    rel_why("AGENTS.md")
                );
                if let Some(mut item) =
                    file_item(ctx, &mut w, &path, Layer::Project, Mode::Always, why, 0)
                {
                    item.knobs.push(setting_knob(
                        ctx,
                        Scope::Local,
                        &format!(
                            "/pluginConfigs/{}/options/instructionFiles",
                            esc(AGENTS_MD_PLUGIN)
                        ),
                        json!("claude-md"),
                        Some("stops every AGENTS.md".to_string()),
                    ));
                    out.push(item);
                }
            }
        }
        let local = dir.join("CLAUDE.local.md");
        if ctx.fs.is_file(&local) {
            let why = rel_why("CLAUDE.local.md");
            out.extend(file_item(
                ctx,
                &mut w,
                &local,
                Layer::Local,
                Mode::Always,
                why,
                0,
            ));
        }
    }
    out
}

/// `paths:` frontmatter → on-demand (loads when a matching file is read).
fn rule_mode(ctx: &Ctx, rule: &Path) -> Mode {
    let paths = ctx
        .fs
        .read_string(rule)
        .and_then(|t| frontmatter::parse(&t))
        .and_then(|fm| fm.get("paths").cloned());
    match paths {
        Some(Value::Null) | None => Mode::Always,
        Some(_) => Mode::OnDemand,
    }
}

fn excluded_by<'a>(ctx: &Ctx, w: &'a Walk, path: &Path, real: &Path) -> Option<&'a str> {
    let home = ctx.home().display().to_string();
    w.excludes.iter().find_map(|(pattern, origin)| {
        let pattern = match pattern.strip_prefix("~/") {
            Some(rest) => format!("{home}/{rest}"),
            None => pattern.clone(),
        };
        let hit = [path, real].iter().any(|p| {
            let s = p.display().to_string();
            glob::matches(&pattern, &s)
                || (!pattern.starts_with('/')
                    && p.strip_prefix(ctx.dir())
                        .is_ok_and(|rel| glob::matches(&pattern, &rel.display().to_string())))
        });
        hit.then_some(origin.as_str())
    })
}

fn file_item(
    ctx: &mut Ctx,
    w: &mut Walk,
    path: &Path,
    layer: Layer,
    mode: Mode,
    why: String,
    depth: usize,
) -> Option<Item> {
    let real = ctx.fs.realpath(path);
    if w.seen.contains(&real) {
        return None;
    }
    if layer != Layer::Managed && excluded_by(ctx, w, path, &real).is_some() {
        return None;
    }
    let len = ctx.fs.len(path)?;
    if len > MAX_BYTES {
        ctx.warnings.push(format!(
            "{}: {len} bytes, over the 4 MiB limit — Claude Code skips it",
            ctx.display(&real)
        ));
        return None;
    }
    let text = ctx.fs.read_string(path)?;
    w.seen.insert(real.clone());
    let name = real.display().to_string();
    let mut item = new_item(Kind::Instruction, layer, &name, &ctx.display(&real));
    item.mode = mode;
    item.cost = if mode == Mode::Always {
        Cost {
            source: Some(len),
            predicted: Some(section_bytes(
                &super::fs::normalize_lexical(path),
                describe(layer),
                strip_frontmatter(&text).len() as u64,
            )),
            ..Cost::default()
        }
    } else {
        cost_na(Some(len))
    };
    item.origin.path = Some(real.clone());
    item.origin.why = vec![why];
    if mode == Mode::OnDemand && depth == 0 {
        item.origin
            .why
            .push("paths: frontmatter → loads when a matching file is read".to_string());
    }
    item.knobs = if layer == Layer::Managed {
        vec![Knob::None {
            reason: "managed CLAUDE.md cannot be excluded".to_string(),
        }]
    } else {
        vec![setting_knob(
            ctx,
            Scope::Local,
            "/claudeMdExcludes/-",
            json!(glob::escape(&name)),
            Some("excluding a file drops its @imports with it".to_string()),
        )]
    };
    if depth < MAX_DEPTH {
        let base = real.parent().map(Path::to_path_buf).unwrap_or_default();
        for (line, token) in imports(&text) {
            let target = resolve_import(ctx, &base, &token);
            if !ctx.fs.is_file(&target) {
                continue;
            }
            let why = format!("@{token} in {}:{line}", ctx.display(&real));
            let mut child_mode = mode;
            let external = matches!(layer, Layer::Project | Layer::Local)
                && !is_inside_project(ctx, &ctx.fs.realpath(&target));
            let approved = ctx
                .project_json()
                .and_then(|p| p.get("hasClaudeMdExternalIncludesApproved"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if external && !approved {
                child_mode = Mode::None;
            }
            if let Some(mut child) = file_item(ctx, w, &target, layer, child_mode, why, depth + 1) {
                if external && !approved {
                    child.cost = cost_na(child.cost.source);
                    child.origin.why.push(
                        "external import not approved (~/.claude.json hasClaudeMdExternalIncludesApproved)"
                            .to_string(),
                    );
                }
                item.children.push(child);
            }
        }
    }
    Some(item)
}

fn is_inside_project(ctx: &Ctx, real: &Path) -> bool {
    real.starts_with(ctx.dir())
        || ctx
            .git_root
            .as_ref()
            .is_some_and(|root| real.starts_with(root))
}

fn resolve_import(ctx: &Ctx, base: &Path, token: &str) -> PathBuf {
    if let Some(rest) = token.strip_prefix("~/") {
        ctx.home().join(rest)
    } else if token.starts_with('/') {
        PathBuf::from(token)
    } else {
        base.join(token)
    }
}

/// `(1-based line, path)` of every `@path` outside code fences, code spans and HTML comments.
/// An `@` counts only at the start of a line or after whitespace (so `a@b.c` is not an import);
/// the path runs to the next unescaped whitespace (`\ ` is a space).
pub(crate) fn imports(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence: Option<String> = None;
    let mut in_comment = false;
    for (n, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        if let Some(fence) = &in_fence {
            if trimmed.starts_with(fence.as_str()) {
                in_fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = Some(trimmed[..3].to_string());
            continue;
        }
        // Blank out comments and code spans, keeping positions irrelevant.
        let mut line = String::new();
        let mut rest = raw;
        while !rest.is_empty() {
            if in_comment {
                match rest.find("-->") {
                    Some(end) => {
                        rest = &rest[end + 3..];
                        in_comment = false;
                        line.push(' ');
                    }
                    None => rest = "",
                }
                continue;
            }
            let next_comment = rest.find("<!--");
            let next_tick = rest.find('`');
            match (next_comment, next_tick) {
                (Some(c), t) if t.is_none_or(|t| c < t) => {
                    line.push_str(&rest[..c]);
                    rest = &rest[c + 4..];
                    in_comment = true;
                }
                (_, Some(t)) => {
                    line.push_str(&rest[..t]);
                    let ticks = rest[t..].chars().take_while(|c| *c == '`').count();
                    let after = &rest[t + ticks..];
                    let closing = "`".repeat(ticks);
                    match after.find(&closing) {
                        Some(end) => {
                            rest = &after[end + ticks..];
                            line.push(' ');
                        }
                        None => {
                            line.push_str(&rest[t..t + ticks]);
                            rest = after;
                        }
                    }
                }
                _ => {
                    line.push_str(rest);
                    rest = "";
                }
            }
        }
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '@' && (i == 0 || chars[i - 1].is_whitespace()) {
                let mut token = String::new();
                let mut j = i + 1;
                while j < chars.len() {
                    if chars[j] == '\\' && chars.get(j + 1) == Some(&' ') {
                        token.push(' ');
                        j += 2;
                        continue;
                    }
                    if chars[j].is_whitespace() {
                        break;
                    }
                    token.push(chars[j]);
                    j += 1;
                }
                if !token.is_empty() {
                    out.push((n + 1, token));
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_skip_code_comments_and_emails() {
        let text = "Owner: a@b.com, z@x.ai\n\
                    workflow = @rules/DEV.md — and @rules/STV.md.\n\
                    loaded via `@rules/PRINCIPLES.md` here\n\
                    <!-- @hidden.md -->\n\
                    ```\n@fenced.md\n```\n\
                    @~/x/y.md\n\
                    path\\ with @a\\ b.md end\n";
        let got: Vec<(usize, String)> = imports(text);
        assert_eq!(
            got,
            vec![
                (2, "rules/DEV.md".to_string()),
                (2, "rules/STV.md.".to_string()),
                (8, "~/x/y.md".to_string()),
                (9, "a b.md".to_string()),
            ]
        );
    }

    #[test]
    fn frontmatter_and_one_blank_line_are_stripped() {
        assert_eq!(strip_frontmatter("---\na: b\n---\n\n# T\n"), "# T\n");
        assert_eq!(strip_frontmatter("---\na: b\n---\n# T\n"), "# T\n");
        assert_eq!(strip_frontmatter("# T\n---\n"), "# T\n---\n");
        assert_eq!(
            section_bytes(Path::new("/r/CLAUDE.md"), describe(Layer::User), 10),
            ("Contents of /r/CLAUDE.md (user's private global instructions for all projects):"
                .len()
                + 3
                + 10) as u64
        );
    }

    #[test]
    fn multi_line_comment_hides_imports() {
        let text = "<!-- start\n@inside.md\nend -->\n@outside.md\n";
        assert_eq!(imports(text), vec![(4, "outside.md".to_string())]);
    }
}
